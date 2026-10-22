//! 普通发送、定时发送和副本恢复共用持久记录，恢复任务从不重发 SMTP。
use super::*;
use crate::send_records::{CopyStatus, NewRecord, Record, SendError, Status};

impl AppState {
    fn sending(&self) -> Result<&send_records::Store> {
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
        .prepare(
            NewRecord {
                id: id.clone(),
                account_id: acct.id.clone(),
                from: acct.email.clone(),
                subject: previous.subject,
                recipients: previous.recipients,
                message_id,
                endpoint: endpoint(&acct),
                scheduled_id: None,
                previous_id: Some(send_id),
            },
            &raw,
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
