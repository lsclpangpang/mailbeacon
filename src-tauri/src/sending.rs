//! 普通发送、定时发送和副本恢复共用持久记录，恢复任务从不重发 SMTP。
use super::*;
use crate::send_records::{CopyStatus, NewRecord, Record, SendError, Status};

impl AppState {
    pub(crate) fn sending(&self) -> Result<&send_records::Store> {
        self.send_records
            .as_ref()
            .map(|s| s.as_ref())
            .map_err(|e| anyhow!("{e}"))
    }
}
fn endpoint(acct: &Account) -> String {
    format!(
        "{:?}|{}|{}|{}|{}",
        acct.receive_protocol, acct.imap_host, acct.imap_port, acct.email, acct.receive_epoch
    )
}

fn changed(app: &AppHandle, record: &Record) {
    let _ = app.emit("send-record-changed", record);
    if record.status == Status::Accepted {
        if let Ok(store) = app.state::<AppState>().sending() {
            if let Ok(Some(reminder)) = store.get_reminder(&format!("followup-{}", record.id)) {
                let _ = app.emit("mail-reminder-changed", &reminder);
            }
        }
    }
}
fn scheduled_key(account_id: &str, id: u32) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, account_id.as_bytes());
    let account: String = digest
        .as_ref()
        .iter()
        .take(16)
        .map(|b| format!("{b:02x}"))
        .collect();
    format!("scheduled-{account}-{id}")
}
async fn submit(
    app: &AppHandle,
    state: &AppState,
    acct: &Account,
    pass: &str,
    id: &str,
) -> Result<Record> {
    let store = state.sending()?;
    let existing = store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
    let recipients = existing.recipients.clone();
    let result = send_records::execute(store, id, |raw| async move {
        smtp::send_raw(acct, pass, &recipients, &raw)
            .await
            .map_err(|e| {
                if smtp::not_accepted(&e) {
                    SendError::Rejected(format!("{e:#}"))
                } else {
                    SendError::Unknown(format!("{e:#}"))
                }
            })
    })
    .await;
    match &result {
        Ok(record) => {
            changed(app, record);
            if record.status == Status::Accepted {
                let recipients = record
                    .recipients
                    .iter()
                    .map(|r| (String::new(), r.clone()))
                    .collect::<Vec<_>>();
                let _ = state.db().and_then(|db| db.record_sent(&recipients));
                state.send_records_wake.notify_one();
            }
        }
        Err(_) => {
            if let Ok(Some(record)) = store.get(id) {
                changed(app, &record);
            }
        }
    }
    result
}

#[tauri::command]
pub(crate) async fn send_mail(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: smtp::Draft,
    draft_refs: Option<Vec<drafts::DraftRef>>,
    send_id: Option<String>,
) -> CmdResult<Record> {
    let _sending = state.outbox_send.lock().await;
    let _account_access = state.account_gate(&draft.account_id).read_owned().await;
    let id = send_id
        .map(Ok)
        .unwrap_or_else(send_records::new_id)
        .map_err(err)?;
    // 重复 IPC 不再解析已释放的附件令牌，也不重新组装 Message-ID。
    if let Some(existing) = state.sending().map_err(err)?.get(&id).map_err(err)? {
        if existing.account_id != draft.account_id {
            return Err("发送记录不属于此邮箱".into());
        }
        return Ok(existing);
    }
    let (acct, pass) = state.account(&draft.account_id).map_err(err)?;
    let quote = quote_raw(&state, &draft).await?;
    let attachments = resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(err)?;
    let (msg, message_id) = smtp::compose(
        &acct,
        &draft,
        quote.as_deref().map(Vec::as_slice),
        attachments,
        smtp::Purpose::Send,
    )
    .map_err(err)?;
    let recipients = msg
        .envelope()
        .to()
        .iter()
        .map(ToString::to_string)
        .collect();
    state
        .sending()
        .map_err(err)?
        .prepare(
            NewRecord {
                id: id.clone(),
                account_id: acct.id.clone(),
                from: acct.email.clone(),
                subject: draft.subject,
                recipients,
                message_id,
                endpoint: endpoint(&acct),
                scheduled_id: None,
                previous_id: None,
            },
            &msg.formatted(),
        )
        .map_err(err)?;
    let record = submit(&app, &state, &acct, &pass, &id).await.map_err(err)?;
    // 提交结果已持久化；清理草稿按各自账户重新加锁并核验收信身份。
    drop(_account_access);
    if record.status == Status::Accepted {
        let tokens = draft
            .attachments
            .iter()
            .filter_map(|a| match a {
                smtp::AttachmentRef::File { token } => Some(token.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        state.attachments.forget(&tokens);
        for draft in draft_refs.unwrap_or_default() {
            if let Err(e) = remove_draft(&state, &draft).await {
                eprintln!("邮件已发送，但删除草稿失败：{e:#}");
            }
        }
    }
    Ok(record)
}

fn legacy_uncertain(item: &outbox::Item) -> bool {
    item.sending
        || item
            .last_error
            .as_deref()
            .is_some_and(|e| e.contains("可能已经发出"))
}
pub(crate) fn ensure_reschedulable(store: &send_records::Store, item: &outbox::Item) -> Result<()> {
    if legacy_uncertain(item)
        || store
            .get(&scheduled_key(&item.account_id, item.id))?
            .is_some()
    {
        bail!("这封邮件已有提交记录或结果未确认，不能通过改时间再次发送；请查看发送记录");
    }
    Ok(())
}

/// 兼容旧版发送中的定时任务，不能通过改时间或立即发送重新激活未知结果。
pub(crate) fn recover_outbox(
    store: &send_records::Store,
    outbox: &outbox::Outbox,
    accounts: &[Account],
) -> Result<Vec<(String, u32)>> {
    let mut accepted = Vec::new();
    for item in outbox.list(None)? {
        let key = scheduled_key(&item.account_id, item.id);
        let mut record = store.get(&key)?;
        if record.is_none() && legacy_uncertain(&item) {
            let Some(acct) = accounts.iter().find(|a| a.id == item.account_id) else {
                continue;
            };
            record = Some(store.import_unknown(
                NewRecord {
                    id: key.clone(),
                    account_id: item.account_id.clone(),
                    from: acct.email.clone(),
                    subject: item.subject.clone(),
                    recipients: item.recipients.clone(),
                    message_id: item.message_id.clone(),
                    endpoint: endpoint(acct),
                    scheduled_id: Some(item.id),
                    previous_id: None,
                },
                &outbox.raw(&item.account_id, item.id)?,
                "旧版定时提交曾被中断，邮件可能已经发出；不会自动重发。".into(),
            )?);
        }
        match record.map(|r| r.status) {
            Some(Status::Accepted) => {
                let _ = outbox.remove(&item.account_id, item.id);
                accepted.push((item.account_id, item.id));
            }
            Some(Status::Unknown | Status::Submitting | Status::Rejected) => {
                outbox.modify(&item.account_id, item.id, |i| {
                    i.sending = false;
                    i.attempts = outbox::MAX_ATTEMPTS;
                    i.last_error =
                        Some("提交已有持久记录，请在发送记录中确认；已停止自动重试".into());
                })?;
            }
            _ => {}
        }
    }
    Ok(accepted)
}

pub(crate) async fn send_scheduled(
    app: &AppHandle,
    account_id: &str,
    id: u32,
    force: bool,
) -> Result<()> {
    let state = app.state::<AppState>();
    let _sending = state.outbox_send.lock().await;
    let _account_access = state.account_gate(account_id).read_owned().await;
    let Ok(item) = state.outbox.get(account_id, id) else {
        return Ok(());
    };
    if !force && !item.due(now_ms()) {
        return Ok(());
    }
    let store = state.sending()?;
    let key = scheduled_key(account_id, id);
    let (acct, pass) = state.account(account_id)?;
    if store.get(&key)?.is_none() {
        // 旧版遗留的未知任务禁止因“立即发送”绕过保护。
        if legacy_uncertain(&item) {
            bail!("这封邮件上次发送结果未确认，请检查已发送后取消定时并另行编辑");
        }
        let raw = smtp::with_date(&state.outbox.raw(account_id, id)?, &chrono::Local::now());
        store.prepare(
            NewRecord {
                id: key.clone(),
                account_id: account_id.into(),
                from: acct.email.clone(),
                subject: item.subject.clone(),
                recipients: item.recipients.clone(),
                message_id: item.message_id.clone(),
                endpoint: endpoint(&acct),
                scheduled_id: Some(id),
                previous_id: None,
            },
            &raw,
        )?;
    }
    state.outbox.modify(account_id, id, |i| i.sending = true)?;
    let result = submit(app, &state, &acct, &pass, &key).await;
    match result {
        Ok(record) if record.status == Status::Accepted => {
            // 发送记录已持久确认，再删除待发入口；删除失败也绝不会再走 SMTP。
            if let Err(e) = state.outbox.remove(account_id, id) {
                eprintln!("已发送，但清理定时入口失败：{e:#}");
            }
            state
                .outbox_sent
                .lock()
                .unwrap()
                .insert((account_id.into(), id));
            outbox_changed(app, account_id, id, "sent", &item.subject);
            Ok(())
        }
        other => {
            let detail = match other {
                Ok(r) => r
                    .error
                    .unwrap_or_else(|| "发送结果需要确认，请查看发送记录".into()),
                Err(e) => format!("{e:#}"),
            };
            state.outbox.modify(account_id, id, |i| {
                i.sending = false;
                i.attempts = outbox::MAX_ATTEMPTS;
                i.last_error = Some(format!("{detail}；已停止自动重试，请查看发送记录"));
            })?;
            outbox_changed(app, account_id, id, "failed", &detail);
            bail!("{detail}")
        }
    }
}

/// 仅补存服务器已确认接受的记录。全局串行避免同一 Message-ID 并发查询后重复 APPEND。
async fn copy_one(app: &AppHandle, id: &str) -> Result<Record> {
    let state = app.state::<AppState>();
    let _copy = state.send_copy.lock().await;
    let store = state.sending()?;
    let record = store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
    if record.status != Status::Accepted {
        bail!("发送结果尚未确认，不能自动补存为已发送");
    }
    if record.copy_status == CopyStatus::Saved {
        return Ok(record);
    }
    let _account_access = state.account_gate(&record.account_id).read_owned().await;
    let result = async {
        let acct = state
            .store
            .lock()
            .unwrap()
            .get(&record.account_id)
            .cloned()
            .ok_or_else(|| anyhow!("原邮箱已移除；本地原文仍保留，可导出"))?;
        if endpoint(&acct) != record.endpoint {
            bail!("收信服务器或身份已改变，旧邮件副本未写入新服务器；本地原文仍保留");
        }
        if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
            bail!("POP3 没有已发送文件夹，本地发送记录和原文已保留");
        }
        let raw = store.raw(id)?;
        save_to_sent(&state, &record.account_id, &record.message_id, raw).await
    }
    .await;
    let record = store.copy_result(id, result.map_err(|e| format!("{e:#}")))?;
    changed(app, &record);
    Ok(record)
}

pub(crate) async fn copy_worker(app: AppHandle) {
    loop {
        let state = app.state::<AppState>();
        if let Ok(store) = state.sending() {
            match store.pending_copies() {
                Ok(records) => {
                    for record in records {
                        if let Some(id) = record.scheduled_id {
                            let _ = state.outbox.remove(&record.account_id, id);
                        }
                        if let Err(e) = copy_one(&app, &record.id).await {
                            eprintln!("已发送副本恢复失败：{e:#}");
                        }
                    }
                }
                Err(e) => eprintln!("读取待补存记录失败：{e:#}"),
            }
        }
        tokio::select! { _ = tokio::time::sleep(Duration::from_secs(60)) => {}, _ = state.send_records_wake.notified() => {} }
    }
}

#[tauri::command]
pub(crate) fn list_send_records(
    state: State<'_, AppState>,
    account_id: Option<String>,
    offset: Option<usize>,
) -> CmdResult<Vec<Record>> {
    let records = state
        .sending()
        .map_err(err)?
        .list(account_id.as_deref())
        .map_err(err)?;
    Ok(records
        .into_iter()
        .skip(offset.unwrap_or(0))
        .take(100)
        .collect())
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReadRecord {
    record: Record,
    text: String,
    raw: String,
    truncated: bool,
}
#[tauri::command]
pub(crate) fn read_send_record(
    state: State<'_, AppState>,
    send_id: String,
) -> CmdResult<ReadRecord> {
    let store = state.sending().map_err(err)?;
    let record = store.get(&send_id).map_err(err)?.ok_or("发送记录不存在")?;
    let raw = store.raw(&send_id).map_err(err)?;
    let text = imap::parse_message(&raw, record.created_at)
        .map_err(err)?
        .text;
    // 预览有界，完整字节始终可导出。
    let decoded = String::from_utf8_lossy(&raw);
    let truncated = decoded.chars().count() > 200_000 || text.chars().count() > 200_000;
    Ok(ReadRecord {
        record,
        text: text.chars().take(200_000).collect(),
        raw: decoded.chars().take(200_000).collect(),
        truncated,
    })
}
#[tauri::command]
pub(crate) async fn export_send_record(
    app: AppHandle,
    state: State<'_, AppState>,
    send_id: String,
) -> CmdResult<String> {
    let raw = state.sending().map_err(err)?.raw(&send_id).map_err(err)?;
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or("找不到下载文件夹")?;
    let path = export::save_eml(&dir, &raw, 0).map_err(err)?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.display().to_string())
}
#[tauri::command]
pub(crate) async fn retry_send_copy(app: AppHandle, send_id: String) -> CmdResult<Record> {
    copy_one(&app, &send_id).await.map_err(err)
}
#[tauri::command]
pub(crate) async fn resend_record(
    app: AppHandle,
    state: State<'_, AppState>,
    send_id: String,
    confirmed: bool,
) -> CmdResult<Record> {
    if !confirmed {
        return Err("重新发送需要明确确认；原邮件可能已经发出".into());
    }
    let _sending = state.outbox_send.lock().await;
    let store = state.sending().map_err(err)?;
    let previous = store.get(&send_id).map_err(err)?.ok_or("发送记录不存在")?;
    let _account_access = state.account_gate(&previous.account_id).read_owned().await;
    if !matches!(
        previous.status,
        Status::Prepared | Status::Rejected | Status::Unknown
    ) {
        return Err("已发送或正在发送的记录不能重复发送".into());
    }
    // 重复点击同一原记录只能产生一个新提交。
    if let Some(existing) = store
        .list(Some(&previous.account_id))
        .map_err(err)?
        .into_iter()
        .find(|r| r.previous_id.as_deref() == Some(&send_id))
    {
        return Ok(existing);
    }
    let (acct, pass) = state.account(&previous.account_id).map_err(err)?;
    if acct.email != previous.from {
        return Err("发件身份已改变，请导出后重新编辑".into());
    }
    let id = send_records::new_id().map_err(err)?;
    let message_id = format!("<{id}@mailbeacon.local>");
    let raw = smtp::with_message_id(
        &smtp::with_date(&store.raw(&send_id).map_err(err)?, &chrono::Local::now()),
        &message_id,
    );
    store
        .prepare_resend(
            &send_id,
            NewRecord {
                id: id.clone(),
                account_id: acct.id.clone(),
                from: acct.email.clone(),
                subject: previous.subject,
                recipients: previous.recipients,
                message_id,
                endpoint: endpoint(&acct),
                scheduled_id: None,
                previous_id: Some(send_id.clone()),
            },
            &raw,
            &smtp_fingerprint(&acct),
            acct.receive_epoch,
        )
        .map_err(err)?;
    let record = submit(&app, &state, &acct, &pass, &id).await.map_err(err)?;
    if record.status == Status::Accepted {
        if let Some(scheduled_id) = previous.scheduled_id {
            let _ = state.outbox.remove(&record.account_id, scheduled_id);
            outbox_changed(
                &app,
                &record.account_id,
                scheduled_id,
                "sent",
                &record.subject,
            );
        }
    }
    Ok(record)
}

// Durable short sending buffer and local reminders.
use crate::send_records::{buffer, reminders};

fn smtp_fingerprint(acct: &Account) -> String {
    format!(
        "{}|{}|{}",
        acct.email.to_ascii_lowercase(),
        acct.smtp_host.to_ascii_lowercase(),
        acct.smtp_port
    )
}
/// A logical request hash is checked before touching released attachment tokens.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue_send(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: serde_json::Value,
    draft_refs: Option<Vec<drafts::DraftRef>>,
    request_id: String,
    compose_session_id: String,
    compose_revision: u64,
    undo_seconds: Option<u64>,
    followup_seconds: Option<u64>,
) -> CmdResult<Record> {
    let seconds = undo_seconds.unwrap_or(10);
    if ![0, 5, 10, 20, 30].contains(&seconds) {
        return Err("缓冲时间须为关闭、5、10、20或30秒".into());
    }
    let payload=serde_json::to_vec(&serde_json::json!({"draft":draft,"session":compose_session_id,"revision":compose_revision,"undo":seconds,"followup":followup_seconds})).map_err(|e|e.to_string())?;
    let payload_hash = ring::digest::digest(&ring::digest::SHA256, &payload)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let draft: smtp::Draft = serde_json::from_value(draft).map_err(|e| e.to_string())?;
    let _publish = state.compose_publish_gate.lock().await;
    if state
        .outbox
        .owns_compose_session(&compose_session_id)
        .map_err(err)?
    {
        return Err("此会话已定时发送，请先检查定时发送列表".into());
    }
    let _gate = state.account_gate(&draft.account_id).read_owned().await;
    let store = state.sending().map_err(err)?;
    if let Some(record) = store
        .buffered_request(&draft.account_id, &request_id, &payload_hash)
        .map_err(err)?
    {
        return Ok(record);
    }
    let (acct, _) = state.account(&draft.account_id).map_err(err)?;
    if followup_seconds.is_some() && acct.receive_protocol != config::ReceiveProtocol::Imap {
        return Err("POP3暂不支持可靠的未回复跟进".into());
    }
    let quote = quote_raw(&state, &draft).await?;
    let attachments = resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(err)?;
    let (message, message_id) = smtp::compose(
        &acct,
        &draft,
        quote.as_deref().map(Vec::as_slice),
        attachments.clone(),
        smtp::Purpose::Send,
    )
    .map_err(err)?;
    let (draft_message, _) = smtp::compose(
        &acct,
        &draft,
        quote.as_deref().map(Vec::as_slice),
        attachments,
        smtp::Purpose::Draft,
    )
    .map_err(err)?;
    let identity = format!("{}\0{}", acct.id, request_id);
    let id = format!(
        "buffer-{}",
        ring::digest::digest(&ring::digest::SHA256, identity.as_bytes())
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    );
    let record = store
        .enqueue_buffer(
            NewRecord {
                id,
                account_id: acct.id.clone(),
                from: acct.email.clone(),
                subject: draft.subject.clone(),
                recipients: message
                    .envelope()
                    .to()
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                message_id,
                endpoint: endpoint(&acct),
                scheduled_id: None,
                previous_id: None,
            },
            &message.formatted(),
            &draft_message.formatted(),
            buffer::BufferInput {
                request_id,
                payload_hash,
                compose_session_id,
                compose_revision,
                deadline: now_ms() + seconds as i64 * 1000,
                sender_fingerprint: smtp_fingerprint(&acct),
                receive_epoch: acct.receive_epoch,
                followup_seconds,
                request_receipt: draft.request_receipt,
            },
        )
        .map_err(err)?;
    let tokens = draft
        .attachments
        .iter()
        .filter_map(|a| match a {
            smtp::AttachmentRef::File { token } => Some(token.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    state.attachments.forget(&tokens);
    // Durable queue owns all content now. Old draft cleanup cannot turn enqueue into failure.
    drop(_gate);
    changed(&app, &record);
    state.send_buffer_wake.notify_one();
    let cleanup_app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = cleanup_app.state::<AppState>();
        for source in draft_refs.unwrap_or_default() {
            if let Err(e) = remove_draft(&state, &source).await {
                eprintln!("待发送已保存，旧草稿清理失败：{e:#}");
            }
        }
    });
    Ok(record)
}
#[tauri::command]
pub(crate) fn cancel_buffered_send(
    app: AppHandle,
    state: State<'_, AppState>,
    send_id: String,
) -> CmdResult<Record> {
    let record = state
        .sending()
        .map_err(err)?
        .cancel_buffered(&send_id)
        .map_err(err)?;
    changed(&app, &record);
    state.send_buffer_wake.notify_one();
    Ok(record)
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RestoredSend {
    record: Record,
    message: imap::MessageView,
    attachments: Vec<drafts::PickedFile>,
    draft_ref: drafts::DraftRef,
    request_receipt: bool,
    followup_seconds: Option<u64>,
}
async fn restore_local_buffer(state: &AppState, id: &str) -> Result<drafts::DraftRef> {
    let store = state.sending()?;
    let record = store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
    if record.status != Status::Cancelled {
        bail!("请先撤销发送，再恢复草稿");
    }
    let _gate = state.account_gate(&record.account_id).read_owned().await;
    let account = state.configured_account(&record.account_id)?;
    let raw = store.buffer_draft(id)?;
    let uid = state
        .local_drafts
        .restore_once(&record.account_id, id, &raw)?;
    let message = imap::parse_message(&raw, record.created_at)?;
    Ok(drafts::DraftRef {
        account_id: record.account_id,
        receive_epoch: account.receive_epoch,
        folder: drafts::LOCAL_DRAFTS.into(),
        uid: Some(uid),
        uid_validity: None,
        message_id: message.message_id.unwrap_or_default(),
        superseded: vec![],
    })
}
#[tauri::command]
pub(crate) async fn restore_buffered_send(
    state: State<'_, AppState>,
    send_id: String,
) -> CmdResult<RestoredSend> {
    let draft_ref = restore_local_buffer(&state, &send_id).await.map_err(err)?;
    let store = state.sending().map_err(err)?;
    let record = store.get(&send_id).map_err(err)?.ok_or("发送记录不存在")?;
    let meta = store
        .buffer_metadata(&send_id)
        .map_err(err)?
        .ok_or("缓冲元数据不存在")?;
    let raw = state
        .local_drafts
        .read(
            &draft_ref.account_id,
            draft_ref.uid.ok_or("恢复草稿缺少编号")?,
        )
        .map_err(err)?;
    let message = imap::parse_message(&raw, record.created_at).map_err(err)?;
    let mut attachments = vec![];
    for attachment in &message.attachments {
        let part = imap::attachment_part(&raw, attachment.index).map_err(err)?;
        attachments.push(
            state
                .attachments
                .register_bytes(part.name, part.content_type, part.data),
        );
    }
    let request_receipt = message.receipt_to.is_some();
    Ok(RestoredSend {
        record,
        message,
        attachments,
        draft_ref,
        request_receipt,
        followup_seconds: meta.input.followup_seconds,
    })
}
async fn submit_buffer(
    app: &AppHandle,
    id: &str,
    force: bool,
    version: Option<u64>,
) -> Result<Record> {
    let state = app.state::<AppState>();
    let store = state.sending()?;
    let before = store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
    let _gate = state.account_gate(&before.account_id).read_owned().await;
    let (acct, pass) = match state.account(&before.account_id) {
        Ok(v) => v,
        Err(e) => {
            if before.status == Status::Buffered {
                let paused = store.update(id, |r| {
                    if r.status == Status::Buffered {
                        r.status = Status::Paused;
                        r.error = Some(format!("发送已暂停：{e:#}"));
                    }
                    Ok(())
                })?;
                changed(app, &paused);
            }
            return Err(e);
        }
    };
    let Some(claim) = store
        .claim_buffered(id, now_ms(), force, &smtp_fingerprint(&acct), version)
        .map_err(|e| {
            if let Ok(Some(record)) = store.get(id) {
                changed(app, &record);
            }
            e
        })?
    else {
        return store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"));
    };
    changed(app, &claim.record);
    let recipients = claim.record.recipients.clone();
    let result = buffer::execute_claimed(store, claim, now_ms, |raw| async move {
        smtp::send_raw(&acct, &pass, &recipients, &raw)
            .await
            .map_err(|e| {
                if smtp::not_accepted(&e) {
                    SendError::Rejected(format!("{e:#}"))
                } else {
                    SendError::Unknown(format!("{e:#}"))
                }
            })
    })
    .await;
    if let Ok(Some(record)) = store.get(id) {
        changed(app, &record);
        if record.status == Status::Accepted {
            let recipients = record
                .recipients
                .iter()
                .map(|r| (String::new(), r.clone()))
                .collect::<Vec<_>>();
            let _ = state.db().and_then(|db| db.record_sent(&recipients));
            state.send_records_wake.notify_one();
        }
    }
    result
}
#[tauri::command]
pub(crate) async fn send_buffered_now(
    app: AppHandle,
    send_id: String,
    expected_version: u64,
) -> CmdResult<Record> {
    submit_buffer(&app, &send_id, true, Some(expected_version))
        .await
        .map_err(err)
}
/// Deadline-driven worker. SMTP runs independently so one slow message cannot block cancellation.
pub(crate) async fn buffer_worker(app: AppHandle) {
    loop {
        let state = app.state::<AppState>();
        let mut delay = Duration::from_secs(30);
        if let Ok(store) = state.sending() {
            if let Ok(records) = store.list(None) {
                for mut record in records {
                    if matches!(record.status, Status::Buffered | Status::Paused)
                        && state.configured_account(&record.account_id).is_err()
                    {
                        if let Ok(cancelled) = store.cancel_buffered(&record.id) {
                            changed(&app, &cancelled);
                            record = cancelled;
                        }
                    }
                    if record.status == Status::Cancelled
                        && store
                            .buffer_metadata(&record.id)
                            .ok()
                            .flatten()
                            .is_some_and(|m| m.restore_requested)
                    {
                        let _ = restore_local_buffer(&state, &record.id).await;
                    }
                }
            }
            match store.pending_buffers() {
                Ok(records) => {
                    for record in records {
                        let wait = record.deadline.unwrap_or(i64::MAX).saturating_sub(now_ms());
                        if wait <= 0 {
                            let app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(e) = submit_buffer(&app, &record.id, false, None).await {
                                    eprintln!("缓冲发送未完成：{e:#}");
                                }
                            });
                            delay = delay.min(Duration::from_millis(100));
                        } else {
                            delay = delay.min(Duration::from_millis(wait as u64));
                        }
                    }
                }
                Err(e) => eprintln!("读取待发送队列失败：{e:#}"),
            }
        }
        tokio::select! {_=tokio::time::sleep(delay)=>{},_=state.send_buffer_wake.notified()=>{}}
    }
}
#[tauri::command]
pub(crate) fn list_mail_reminders(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> CmdResult<Vec<reminders::Reminder>> {
    state
        .sending()
        .map_err(err)?
        .list_reminders(account_id.as_deref())
        .map_err(err)
}
#[tauri::command]
pub(crate) async fn set_mail_snooze(
    app: AppHandle,
    state: State<'_, AppState>,
    source: mail_ref::MessageRef,
    due_at: i64,
    id: Option<String>,
    expected_version: Option<u64>,
    anchors: Option<Vec<String>>,
) -> CmdResult<reminders::Reminder> {
    let _gate = state.account_gate(&source.account_id).read_owned().await;
    let account = state.configured_account(&source.account_id).map_err(err)?;
    source
        .validate_current(account.receive_epoch, source.uid_validity)
        .map_err(err)?;
    if account.receive_protocol != config::ReceiveProtocol::Imap || source.folder.starts_with("__")
    {
        return Err("此邮件不支持持久稍后处理".into());
    }
    if let Ok(folders) = state
        .search_index()
        .and_then(|s| s.saved_folders(&account.id, account.receive_epoch))
    {
        if folders.iter().any(|f| {
            f.path == source.folder
                && matches!(f.role.as_str(), "sent" | "drafts" | "junk" | "trash")
        }) {
            return Err("此文件夹不支持稍后处理".into());
        }
    }
    let summary = state
        .search_index()
        .map_err(err)?
        .snapshot_summary(&source)
        .map_err(err)?;
    // Caller hints cannot bind a snooze to an unrelated thread. Use the exact persisted source.
    let _ = anchors;
    let anchors = if summary.message_id.trim().is_empty() {
        vec![]
    } else {
        vec![summary.message_id]
    };
    let r = state
        .sending()
        .map_err(err)?
        .set_snooze(
            source,
            due_at,
            id.as_deref(),
            expected_version,
            now_ms(),
            anchors,
        )
        .map_err(err)?;
    let _ = app.emit("mail-reminder-changed", &r);
    Ok(r)
}
#[tauri::command]
pub(crate) fn update_mail_reminder(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    expected_version: u64,
    action: String,
    due_at: Option<i64>,
) -> CmdResult<reminders::Reminder> {
    let r = state
        .sending()
        .map_err(err)?
        .change_reminder(&id, expected_version, &action, due_at, now_ms())
        .map_err(err)?;
    let _ = app.emit("mail-reminder-changed", &r);
    Ok(r)
}

async fn check_reminder_account(app: &AppHandle, account_id: &str) -> Result<()> {
    use std::sync::atomic::Ordering;
    let state = app.state::<AppState>();
    let flag = Arc::new(AtomicBool::new(false));
    {
        let mut scans = state.mail_reminder_scans.lock().unwrap();
        if scans.contains_key(account_id) {
            return Ok(());
        }
        scans.insert(account_id.into(), flag.clone());
    }
    let result = async {
        let store = state.sending()?;
        let tasks = store
            .list_reminders(Some(account_id))?
            .into_iter()
            .filter(|r| r.active())
            .collect::<Vec<_>>();
        if tasks.is_empty() {
            return Ok(());
        }
        let (account, pass) = state.account(account_id)?;
        if account.receive_protocol != config::ReceiveProtocol::Imap {
            bail!("POP3暂不支持持久回复核对");
        }
        let own = state
            .store
            .lock()
            .unwrap()
            .accounts
            .iter()
            .map(|a| a.email.to_ascii_lowercase())
            .collect::<Vec<_>>();
        let started = now_ms();
        let _ = app.emit("mail-reminder-scan", serde_json::json!({"accountId":account_id,"state":"checking","processedHeaders":0}));
        let mut processed = 0u64;
        let mut session = imap::login(&account, &pass).await?;
        let round = reminders::scan_reply_round(
            &mut session,
            account_id,
            account.receive_epoch,
            || flag.load(Ordering::SeqCst) || !state.connection_is_current(&account),
            |folder, validity| {
                store.indexed_reply_uids(account_id, account.receive_epoch, folder, validity)
            },
            |folder,validity,uids| {
                for r in store.reconcile_snooze_sources(account_id,account.receive_epoch,folder,validity,uids)? { let _=app.emit("mail-reminder-changed",&r); }
                Ok(())
            },
            |e| {
                processed += 1;
                if processed == 1 || processed % 100 == 0 { let _ = app.emit("mail-reminder-scan", serde_json::json!({"accountId":account_id,"state":"checking","processedHeaders":processed,"folder":e.source.folder})); }
                let changed = store.observe_reply(&e, &own)?;
                for r in changed {
                    let _ = app.emit("mail-reminder-changed", &r);
                }
                Ok(())
            },
        )
        .await;
        let _ = tokio::time::timeout(Duration::from_secs(5), session.logout()).await;
        let round = round?;
        if flag.load(Ordering::SeqCst) || !state.connection_is_current(&account) {
            bail!("核对已取消或邮箱身份已变化");
        }
        for original in tasks {
            if let Some(source)=original.source.as_ref().filter(|s|!round.roster.contains(&s.folder)) {
                for r in store.reconcile_snooze_sources(account_id,account.receive_epoch,&source.folder,0,&[])? {let _=app.emit("mail-reminder-changed",&r);}
            }
            let Some(current) = store.get_reminder(&original.id)? else {
                continue;
            };
            if current.kind == reminders::ReminderKind::Followup
                && current.active()
                && current.due_at <= started
                && current.receive_epoch == account.receive_epoch
            {
                match store.finish_reminder_round(
                    &current.id,
                    current.version,
                    started,
                    account.receive_epoch,
                    round.roster.clone(),
                    round.coverage.clone(),
                    now_ms(),
                ) {
                    Ok(r) => {
                        let _ = app.emit("mail-reminder-changed", &r);
                    }
                    Err(e) => {
                        eprintln!("提醒核对结果未写回：{e:#}");
                    }
                }
            }
        }
        Ok(())
    }
    .await;
    state.mail_reminder_scans.lock().unwrap().remove(account_id);
    let _=app.emit("mail-reminder-scan",serde_json::json!({"accountId":account_id,"state":if result.is_ok(){"complete"}else{"incomplete"}}));
    if let Err(ref error) = result {
        if let Ok(store) = state.sending() {
            if let Ok(tasks) = store.list_reminders(Some(account_id)) {
                for task in tasks.into_iter().filter(|r| r.active()) {
                    let _ = store.reminder_check_error(
                        &task.id,
                        task.version,
                        &format!("{error:#}"),
                        now_ms(),
                    );
                    if let Ok(Some(updated)) = store.get_reminder(&task.id) {
                        let _ = app.emit("mail-reminder-changed", &updated);
                    }
                }
            }
        }
    }
    result
}
#[tauri::command]
pub(crate) async fn check_mail_reminders(
    app: AppHandle,
    account_id: Option<String>,
) -> CmdResult<Vec<reminders::Reminder>> {
    let accounts = match &account_id {
        Some(id) => vec![id.clone()],
        None => app
            .state::<AppState>()
            .store
            .lock()
            .unwrap()
            .accounts
            .iter()
            .map(|a| a.id.clone())
            .collect(),
    };
    // Per-account errors are persisted and remain visible alongside successful accounts.
    for id in accounts {
        if let Err(e) = check_reminder_account(&app, &id).await {
            eprintln!("跟进核对未完成：{e:#}");
        }
    }
    app.state::<AppState>()
        .sending()
        .map_err(err)?
        .list_reminders(account_id.as_deref())
        .map_err(err)
}
#[tauri::command]
pub(crate) fn cancel_mail_reminder_scan(state: State<'_, AppState>, account_id: String) {
    if let Some(flag) = state.mail_reminder_scans.lock().unwrap().get(&account_id) {
        flag.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}
/// The durable list is authoritative; aggregate system delivery is deliberately best effort.
pub(crate) async fn reminder_worker(app: AppHandle) {
    let mut next_scan = 0;
    loop {
        let state = app.state::<AppState>();
        if let Ok(store) = state.sending() {
            if let Ok(tasks) = store.list_reminders(None) {
                for task in tasks.into_iter().filter(|r| r.active()) {
                    match state.configured_account(&task.account_id) {
                        Ok(account) if account.receive_epoch != task.receive_epoch => {
                            let _ = store.invalidate_reminders(
                                &task.account_id,
                                task.receive_epoch,
                                false,
                            );
                        }
                        Err(_) => {
                            let _ = store.invalidate_reminders(
                                &task.account_id,
                                task.receive_epoch,
                                true,
                            );
                        }
                        _ => {}
                    }
                }
            }
            match store.tick_reminders(now_ms()) {
                Ok(records) => {
                    for r in &records {
                        let _ = app.emit("mail-reminder-changed", r);
                    }
                    if !records.is_empty() {
                        let _ = app
                            .notification()
                            .builder()
                            .title("MailBeacon 待跟进")
                            .body(format!(
                                "{} 项邮件提醒已到期，请打开待跟进查看",
                                records.len()
                            ))
                            .show();
                    }
                }
                Err(e) => eprintln!("提醒调度失败：{e:#}"),
            }
            if now_ms() >= next_scan {
                next_scan = now_ms() + 120_000;
                if let Ok(tasks) = store.list_reminders(None) {
                    let mut accounts = std::collections::HashSet::new();
                    for task in tasks.into_iter().filter(|r| r.active()) {
                        if accounts.insert(task.account_id.clone()) {
                            let app = app.clone();
                            tauri::async_runtime::spawn(async move {
                                if let Err(e) = check_reminder_account(&app, &task.account_id).await
                                {
                                    eprintln!("后台跟进核对未完成：{e:#}");
                                }
                            });
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(15)).await;
    }
}

#[tauri::command]
pub(crate) fn list_active_sends(state: State<'_, AppState>) -> CmdResult<Vec<Record>> {
    state.sending().map_err(err)?.active_sends().map_err(err)
}
