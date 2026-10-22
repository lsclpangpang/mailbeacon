//! Explicit, durable outcomes for user-initiated mail mutations.
use crate::{AppState, download_protocol, imap, lanes::Stream, mail_ref::MessageRef};
use anyhow::{Result, anyhow, bail};
use async_imap::{
    Session,
    imap_proto::{AttributeValue, Response, ResponseCode, Status, rfc4315::UidSetMember},
};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::Path, sync::Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub operation_id: String,
    pub action: String,
    pub items: Vec<MessageRef>,
    pub value: Option<bool>,
    pub target: Option<String>,
    #[serde(default)]
    pub permanent_confirmed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Undo {
    pub destination: MessageRef,
    pub original: MessageRef,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemResult {
    pub source: MessageRef,
    pub status: String,
    pub message: String,
    pub undo: Option<Undo>,
}
impl ItemResult {
    fn new(source: &MessageRef, status: &str, message: impl ToString) -> Self {
        Self {
            source: source.clone(),
            status: status.into(),
            message: message.to_string(),
            undo: None,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outcome {
    pub operation_id: String,
    pub items: Vec<ItemResult>,
    pub undo_token: Option<String>,
    pub expires_at: i64,
}
pub struct Journal(Mutex<Connection>);
impl Journal {
    pub fn open(path: &Path) -> Result<Self> {
        let db = Connection::open(path)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS mail_operations (id TEXT PRIMARY KEY, request TEXT NOT NULL, result TEXT NOT NULL, finished INTEGER NOT NULL DEFAULT 0, undo_used INTEGER NOT NULL DEFAULT 0);")?;
        // An interrupted mutation has no safe replay. Its per-item checkpoints are retained.
        db.execute("UPDATE mail_operations SET finished=1 WHERE finished=0", [])?;
        Ok(Self(Mutex::new(db)))
    }
    fn claim(&self, request: &Request) -> Result<Option<Outcome>> {
        if request.operation_id.is_empty()
            || request.operation_id.len() > 128
            || request.items.is_empty()
            || request.items.len() > 2000
        {
            bail!("操作请求无效或超过每次 2000 封，请重新选择");
        }
        let mut unique = HashSet::new();
        if request.items.iter().any(|m| !unique.insert(m)) {
            bail!("操作中包含重复邮件");
        }
        let encoded = serde_json::to_string(request)?;
        let db = self.0.lock().unwrap();
        let old: Option<(String, String)> = db
            .query_row(
                "SELECT request,result FROM mail_operations WHERE id=?1",
                [&request.operation_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((old_request, result)) = old {
            if old_request != encoded {
                bail!("操作编号已用于其他请求，已阻止重复提交");
            }
            return Ok(Some(serde_json::from_str(&result)?));
        }
        let outcome = Outcome {
            operation_id: request.operation_id.clone(),
            items: request
                .items
                .iter()
                .map(|m| ItemResult::new(m, "unknown", "尚未确认完成，请刷新核对；不会自动重试"))
                .collect(),
            undo_token: None,
            expires_at: 0,
        };
        db.execute(
            "INSERT INTO mail_operations (id,request,result) VALUES (?1,?2,?3)",
            params![
                request.operation_id,
                encoded,
                serde_json::to_string(&outcome)?
            ],
        )?;
        Ok(None)
    }
    fn save(&self, result: &Outcome, finished: bool) -> Result<()> {
        self.0.lock().unwrap().execute(
            "UPDATE mail_operations SET result=?2,finished=?3 WHERE id=?1",
            params![
                result.operation_id,
                serde_json::to_string(result)?,
                finished
            ],
        )?;
        Ok(())
    }
    fn take_undo(&self, token: &str, now: i64) -> Result<Vec<Undo>> {
        let db = self.0.lock().unwrap();
        let (json, used, finished): (String, bool, bool) = db.query_row(
            "SELECT result,undo_used,finished FROM mail_operations WHERE id=?1",
            [token],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let result: Outcome = serde_json::from_str(&json)?;
        if used
            || !finished
            || result.undo_token.as_deref() != Some(token)
            || now >= result.expires_at
        {
            bail!("撤销已过期或已执行，请到目标文件夹核对");
        }
        db.execute(
            "UPDATE mail_operations SET undo_used=1 WHERE id=?1",
            [token],
        )?;
        Ok(result.items.into_iter().filter_map(|m| m.undo).collect())
    }
    pub fn recent(&self) -> Result<Vec<Outcome>> {
        let db = self.0.lock().unwrap();
        let mut stmt =
            db.prepare("SELECT result FROM mail_operations ORDER BY rowid DESC LIMIT 30")?;
        let values = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        values
            .into_iter()
            .map(|s| serde_json::from_str(&s).map_err(Into::into))
            .collect()
    }
}

fn one_uid(values: &[UidSetMember]) -> Option<u32> {
    match values {
        [UidSetMember::Uid(uid)] if *uid > 0 => Some(*uid),
        [UidSetMember::UidRange(r)] if r.start() == r.end() && *r.start() > 0 => Some(*r.start()),
        _ => None,
    }
}
fn copy_mapping(code: &ResponseCode<'_>, expected: u32) -> Option<(u32, u32)> {
    match code {
        ResponseCode::CopyUid(validity, from, to)
            if *validity > 0 && one_uid(from) == Some(expected) =>
        {
            Some((*validity, one_uid(to)?))
        }
        _ => None,
    }
}
#[derive(Default)]
struct Reply {
    mapping: Option<(u32, u32)>,
    flags: Option<Vec<String>>,
}
// Preserve tagged AND untagged COPYUID, but only after the matching tagged OK.
async fn command<T: Stream>(s: &mut Session<T>, text: &str, uid: u32) -> Result<Reply> {
    tokio::time::timeout(imap::OP_TIMEOUT, async {
        let tag = s.run_command(text).await?;
        let mut reply = Reply::default();
        loop {
            let next = s
                .read_response()
                .await?
                .ok_or_else(|| anyhow!("连接中断，服务器未确认操作结果"))?;
            match next.parsed() {
                Response::Done {
                    tag: actual,
                    status,
                    outcome,
                } => {
                    if actual != &tag || *status != Status::Ok {
                        bail!("服务器未确认操作成功：{outcome:?}");
                    }
                    if let Some(code) = &outcome.code {
                        reply.mapping = copy_mapping(code, uid).or(reply.mapping);
                    }
                    return Ok(reply);
                }
                Response::Data {
                    status: Status::Bye,
                    ..
                } => bail!("服务器断开连接，结果尚未确认"),
                Response::Data {
                    status: Status::Ok,
                    outcome,
                } => {
                    if let Some(code) = &outcome.code {
                        reply.mapping = copy_mapping(code, uid).or(reply.mapping);
                    }
                }
                Response::Fetch(_, attrs)
                    if attrs
                        .iter()
                        .any(|a| matches!(a,AttributeValue::Uid(u) if *u == uid)) =>
                {
                    reply.flags = attrs.iter().find_map(|a| match a {
                        AttributeValue::Flags(flags) => {
                            Some(flags.iter().map(ToString::to_string).collect())
                        }
                        _ => None,
                    });
                }
                _ => {}
            }
        }
    })
    .await?
}
async fn validity<T: Stream>(s: &mut Session<T>, folder: &str) -> Result<u32> {
    let result = tokio::time::timeout(
        imap::OP_TIMEOUT,
        download_protocol::checked_command(
            s,
            &format!("SELECT {}", download_protocol::quoted_mailbox(folder)?),
        ),
    )
    .await??;
    result
        .into_iter()
        .find_map(|r| match r {
            download_protocol::ArchiveData::Validity(v) if v > 0 => Some(v),
            _ => None,
        })
        .ok_or_else(|| anyhow!("服务器未确认文件夹代际，已停止操作"))
}
// Once a modifying command has been issued, any failure is conservatively unknown.
// COPY + STORE + EXPUNGE can partially succeed even when a later command says NO.
async fn mutate<T: Stream>(
    s: &mut Session<T>,
    source: &MessageRef,
    epoch: u64,
    action: &str,
    value: Option<bool>,
    target: Option<&str>,
    confirmed: bool,
    expected_target_validity: Option<u32>,
) -> ItemResult {
    let mut started = false;
    let result: Result<Option<Undo>> = async {
        source.validate()?;
        if source.receive_epoch != epoch {
            bail!("账户收信服务器已变化，请刷新后重新选择");
        }
        if let Some(expected) = expected_target_validity {
            let actual = validity(s, target.ok_or_else(|| anyhow!("缺少撤销目标"))?).await?;
            if actual != expected {
                bail!("原文件夹已被替换，无法安全撤销");
            }
        }
        let actual = validity(s, &source.folder).await?;
        source.validate_current(epoch, actual)?;
        let flags = command(
            s,
            &format!("UID FETCH {} (UID FLAGS)", source.uid),
            source.uid,
        )
        .await?
        .flags
        .ok_or_else(|| anyhow!("原邮件已不存在，请刷新列表"))?;
        if action == "seen" || action == "flagged" {
            let value = value.ok_or_else(|| anyhow!("缺少标记值"))?;
            let flag = if action == "seen" {
                "\\Seen"
            } else {
                "\\Flagged"
            };
            if flags.iter().any(|f| f.eq_ignore_ascii_case(flag)) == value {
                return Ok(None);
            }
            started = true;
            command(
                s,
                &format!(
                    "UID STORE {} {}FLAGS.SILENT ({flag})",
                    source.uid,
                    if value { "+" } else { "-" }
                ),
                source.uid,
            )
            .await?;
            let after = command(
                s,
                &format!("UID FETCH {} (UID FLAGS)", source.uid),
                source.uid,
            )
            .await?
            .flags
            .ok_or_else(|| anyhow!("标记后邮件已消失"))?;
            if after.iter().any(|f| f.eq_ignore_ascii_case(flag)) != value {
                bail!("服务器未保留请求的标记，请刷新核对");
            }
            return Ok(None);
        }
        if target.is_none() && !confirmed {
            bail!("彻底删除前需要确认；尚未删除邮件");
        }
        let caps = tokio::time::timeout(imap::OP_TIMEOUT, s.capabilities()).await??;
        let can_move = caps.has_str("MOVE");
        let uidplus = caps.has_str("UIDPLUS");
        if (target.is_none() || !can_move) && !uidplus {
            bail!("服务器无法按单封安全删除，已停止操作");
        }
        let mapped = if let Some(target) = target {
            if imap::mailbox_eq(&source.folder, target) {
                bail!("邮件已在目标文件夹");
            }
            let mailbox = download_protocol::quoted_mailbox(target)?;
            started = true;
            let reply = command(
                s,
                &format!(
                    "UID {} {} {mailbox}",
                    if can_move { "MOVE" } else { "COPY" },
                    source.uid
                ),
                source.uid,
            )
            .await?;
            if reply.mapping.is_none() {
                bail!(if can_move {
                    "服务器未返回可靠的移动编号，结果待确认；请刷新目标文件夹核对，勿重复操作"
                } else {
                    "服务器未返回可靠的复制编号，副本可能已建立；已保留原邮件，请核对后再处理"
                });
            }
            reply.mapping
        } else {
            None
        };
        if target.is_none() || !can_move {
            started = true;
            command(
                s,
                &format!("UID STORE {} +FLAGS.SILENT (\\Deleted)", source.uid),
                source.uid,
            )
            .await?;
            command(s, &format!("UID EXPUNGE {}", source.uid), source.uid).await?;
        }
        // A tagged OK on MOVE may be a no-op for a vanished UID. Verify source removal.
        if command(
            s,
            &format!("UID FETCH {} (UID FLAGS)", source.uid),
            source.uid,
        )
        .await?
        .flags
        .is_some()
        {
            bail!("服务器未移除原邮件，请刷新核对，勿重复操作");
        }
        Ok(target
            .zip(mapped)
            .map(|(folder, (uid_validity, uid))| Undo {
                original: source.clone(),
                destination: MessageRef {
                    folder: folder.into(),
                    uid_validity,
                    uid,
                    ..source.clone()
                },
            }))
    }
    .await;
    match result {
        Ok(undo) => ItemResult {
            undo,
            ..ItemResult::new(source, "succeeded", "已完成")
        },
        Err(e) => ItemResult::new(
            source,
            if started { "unknown" } else { "failed" },
            format!("{e:#}"),
        ),
    }
}

#[derive(Debug)]
struct MutationError(ItemResult);
impl std::fmt::Display for MutationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.message)
    }
}
impl std::error::Error for MutationError {}

async fn run_item(
    state: &AppState,
    request: &Request,
    source: &MessageRef,
    undo: Option<&Undo>,
) -> ItemResult {
    // The account gate is held from identity validation through the durable checkpoint.
    let account = match state.account(&source.account_id) {
        Ok((a, _)) => a,
        Err(e) => return ItemResult::new(source, "failed", e),
    };
    if account.receive_epoch != source.receive_epoch {
        return ItemResult::new(source, "failed", "收信服务器已变化，请刷新后重新选择");
    }
    if state.is_pop3(&source.account_id).unwrap_or(false) {
        if request.action == "seen" && source.folder == "INBOX" && source.uid > 0 {
            if let Some(value) = request.value {
                return match state
                    .pop3_mailbox(&source.account_id)
                    .set_read_confirmed(&[source.uid], value)
                    .await
                {
                    Ok(()) => ItemResult::new(source, "succeeded", "已保存本地已读状态"),
                    Err(e) => ItemResult::new(source, "failed", format!("{e:#}")),
                };
            }
        }
        return ItemResult::new(source, "failed", "POP3 不支持此操作");
    }
    let src = source.clone();
    let req = request.clone();
    let expected = undo.map(|u| u.original.uid_validity);
    let result = state
        .with_cmd(&source.account_id, move |c, acct| {
            Box::pin(async move {
                c.selected = None; // mutate SELECTs both folders, never leave the lane's cached selection stale.
                let target = if req.action == "seen" || req.action == "flagged" {
                    None
                } else {
                    let folders = imap::list_folders(&mut c.s).await?;
                    crate::resolve_move_target(
                        &folders,
                        &src.folder,
                        &req.action,
                        req.target.as_deref(),
                    )?
                };
                let result = mutate(
                    &mut c.s,
                    &src,
                    acct.receive_epoch,
                    &req.action,
                    req.value,
                    target.as_deref(),
                    req.permanent_confirmed,
                    expected,
                )
                .await;
                if result.status != "succeeded" {
                    return Err(MutationError(result).into());
                }
                Ok(result)
            })
        })
        .await;
    match result {
        Ok(outcome) => {
            if request.action == "seen" || request.action == "flagged" {
                state.index_set_flags(
                    &source.account_id,
                    &source.folder,
                    &[source.uid],
                    (request.action == "seen").then_some(request.value.unwrap_or(false)),
                    (request.action == "flagged").then_some(request.value.unwrap_or(false)),
                );
            } else {
                state.index_remove(&source.account_id, &source.folder, &[source.uid]);
                let namespace =
                    crate::mail_ref::namespace(&source.account_id, source.receive_epoch);
                let disk = {
                    let mut cache = state.bodies.lock().unwrap();
                    cache.retain(|((account, folder, _, uid), _)| {
                        !(account == &namespace
                            && folder == &source.folder
                            && *uid == source.uid)
                    });
                    cache.disk.clone()
                };
                if let Some(disk) = disk {
                    disk.remove(&namespace, &source.folder, Some(&[source.uid]));
                }
                if source.folder.eq_ignore_ascii_case("INBOX") {
                    if let Some(live) = state.live.lock().unwrap().get_mut(&source.account_id) {
                        crate::remove_moved_inbox(live, &[source.uid]);
                    }
                }
            }
            outcome
        }
        Err(e) => e
            .downcast_ref::<MutationError>()
            .map(|e| e.0.clone())
            .unwrap_or_else(|| ItemResult::new(source, "failed", format!("{e:#}"))),
    }
}

async fn execute(state: &AppState, request: Request, undo: &[Undo]) -> Result<Outcome> {
    if !matches!(
        request.action.as_str(),
        "seen" | "flagged" | "archive" | "trash" | "junk" | "inbox" | "move"
    ) {
        bail!("未知操作");
    }
    let journal = state
        .operations
        .as_ref()
        .map_err(|e| anyhow!("操作记录不可用，尚未执行：{e}"))?;
    if let Some(previous) = journal.claim(&request)? {
        return Ok(previous);
    }
    let mut outcome = Outcome {
        operation_id: request.operation_id.clone(),
        items: request
            .items
            .iter()
            .map(|m| ItemResult::new(m, "unknown", "等待服务器确认"))
            .collect(),
        undo_token: None,
        expires_at: 0,
    };
    for (i, source) in request.items.iter().enumerate() {
        // The account repair writer cannot change credentials/endpoints during a mutation.
        let _guard = state.account_gate(&source.account_id).read_owned().await;
        outcome.items[i] = run_item(
            state,
            &request,
            source,
            undo.iter().find(|u| u.destination == *source),
        )
        .await;
        journal.save(&outcome, false)?;
    }
    if undo.is_empty() && outcome.items.iter().any(|m| m.undo.is_some()) {
        outcome.undo_token = Some(request.operation_id.clone());
        outcome.expires_at = chrono::Utc::now().timestamp_millis() + 10_000;
    }
    journal.save(&outcome, true)?;
    Ok(outcome)
}
#[tauri::command]
pub async fn execute_mail_operation(
    state: tauri::State<'_, AppState>,
    request: Request,
) -> crate::CmdResult<Outcome> {
    execute(&state, request, &[]).await.map_err(crate::err)
}
#[tauri::command]
pub async fn undo_mail_operation(
    state: tauri::State<'_, AppState>,
    token: String,
) -> crate::CmdResult<Outcome> {
    let journal = state.operations.as_ref().map_err(Clone::clone)?;
    let receipts = journal
        .take_undo(&token, chrono::Utc::now().timestamp_millis())
        .map_err(crate::err)?;
    let mut outcomes = Vec::new();
    // Different source folders require separate requests, but each receipt is consumed once.
    for (i, receipt) in receipts.iter().enumerate() {
        let request = Request {
            operation_id: format!("undo:{token}:{i}"),
            action: "move".into(),
            items: vec![receipt.destination.clone()],
            value: None,
            target: Some(receipt.original.folder.clone()),
            permanent_confirmed: false,
        };
        outcomes.extend(
            execute(&state, request, std::slice::from_ref(receipt))
                .await
                .map_err(crate::err)?
                .items,
        );
    }
    Ok(Outcome {
        operation_id: format!("undo:{token}"),
        items: outcomes,
        undo_token: None,
        expires_at: 0,
    })
}
#[tauri::command]
pub fn list_mail_operations(state: tauri::State<'_, AppState>) -> crate::CmdResult<Vec<Outcome>> {
    state
        .operations
        .as_ref()
        .map_err(Clone::clone)?
        .recent()
        .map_err(crate::err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> MessageRef {
        MessageRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 1,
        }
    }
    fn request() -> Request {
        Request {
            operation_id: "op-1".into(),
            action: "archive".into(),
            items: vec![source()],
            value: None,
            target: None,
            permanent_confirmed: false,
        }
    }
    #[test]
    fn operation_claim_is_durable_and_never_replays_a_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("operations.db");
        let store = Journal::open(&path).unwrap();
        assert!(store.claim(&request()).unwrap().is_none());
        assert!(store.claim(&request()).unwrap().is_some());
        drop(store);
        let store = Journal::open(&path).unwrap();
        let recovered = store.claim(&request()).unwrap().unwrap();
        assert_eq!(recovered.items[0].status, "unknown");
        let mut changed = request();
        changed.items[0].uid = 2;
        assert!(store.claim(&changed).is_err());
    }
    #[tokio::test]
    async fn changed_mailbox_identity_never_mutates_a_reused_uid() {
        let server =
            crate::test_imap::state("IMAP4rev1 MOVE UIDPLUS", &[("INBOX", 8), ("Archive", 9)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: new\r\n\r\nbody", &[]);
        let mut s = crate::test_imap::session(server.clone()).await;
        let result = mutate(
            &mut s,
            &source(),
            1,
            "archive",
            None,
            Some("Archive"),
            false,
            None,
        )
        .await;
        assert_eq!(result.status, "failed");
        assert!(
            !server
                .lock()
                .unwrap()
                .commands
                .iter()
                .any(|c| c.contains("UID MOVE") || c.contains("UID STORE"))
        );
    }
    #[tokio::test]
    async fn permanent_delete_requires_confirmation_and_uidplus() {
        let server = crate::test_imap::state("IMAP4rev1 UIDPLUS", &[("INBOX", 7)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: test\r\n\r\nbody", &[]);
        let mut s = crate::test_imap::session(server.clone()).await;
        let result = mutate(&mut s, &source(), 1, "trash", None, None, false, None).await;
        assert_eq!(result.status, "failed");
        assert!(
            !server
                .lock()
                .unwrap()
                .commands
                .iter()
                .any(|c| c.contains("UID STORE"))
        );
    }
    #[tokio::test]
    async fn copy_followed_by_delete_failure_is_unknown_and_not_undoable() {
        let server = crate::test_imap::state("IMAP4rev1 UIDPLUS", &[("INBOX", 7), ("Archive", 9)]);
        {
            let mut st = server.lock().unwrap();
            st.add("INBOX", "Subject: test\r\n\r\nbody", &[]);
            st.fail.push("UID STORE".into());
        }
        let mut s = crate::test_imap::session(server.clone()).await;
        let result = mutate(
            &mut s,
            &source(),
            1,
            "archive",
            None,
            Some("Archive"),
            false,
            None,
        )
        .await;
        assert_eq!(result.status, "unknown");
        assert!(result.undo.is_none());
        assert_eq!(server.lock().unwrap().mailbox("Archive").messages.len(), 1);
    }
    #[tokio::test]
    async fn successful_move_can_only_undo_the_uid_returned_by_copyuid() {
        let server =
            crate::test_imap::state("IMAP4rev1 MOVE UIDPLUS", &[("INBOX", 7), ("Archive", 9)]);
        {
            let mut st = server.lock().unwrap();
            st.add("INBOX", "Subject: original\r\n\r\nbody", &[]);
            st.add("Archive", "Subject: unrelated\r\n\r\nbody", &[]);
        }
        let mut s = crate::test_imap::session(server.clone()).await;
        let result = mutate(
            &mut s,
            &source(),
            1,
            "archive",
            None,
            Some("Archive"),
            false,
            None,
        )
        .await;
        assert_eq!(result.status, "succeeded");
        let undo = result.undo.expect("COPYUID should allow precise undo");
        assert_eq!(undo.destination.uid, 2);
        let restored = mutate(
            &mut s,
            &undo.destination,
            1,
            "move",
            None,
            Some("INBOX"),
            false,
            Some(7),
        )
        .await;
        assert_eq!(restored.status, "succeeded");
        let mut st = server.lock().unwrap();
        assert_eq!(st.mailbox("Archive").messages.len(), 1);
        assert_eq!(st.mailbox("Archive").messages[0].uid, 1);
        assert_eq!(st.mailbox("INBOX").messages.len(), 1);
    }
    #[tokio::test]
    async fn removing_flags_is_verified_and_keeps_other_flags() {
        let server = crate::test_imap::state("IMAP4rev1 UIDPLUS", &[("INBOX", 7)]);
        server.lock().unwrap().add(
            "INBOX",
            "Subject: original\r\n\r\nbody",
            &["\\Seen", "\\Flagged"],
        );
        let mut s = crate::test_imap::session(server.clone()).await;
        assert_eq!(
            mutate(&mut s, &source(), 1, "seen", Some(false), None, false, None)
                .await
                .status,
            "succeeded"
        );
        let flags = server.lock().unwrap().mailbox("INBOX").messages[0]
            .flags
            .clone();
        assert_eq!(flags, vec!["\\Flagged"]);
    }
    #[test]
    fn undo_token_is_single_use_and_expiration_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let store = Journal::open(&dir.path().join("operations.db")).unwrap();
        store.claim(&request()).unwrap();
        let mut item = ItemResult::new(&source(), "succeeded", "");
        item.undo = Some(Undo {
            original: source(),
            destination: MessageRef {
                folder: "Archive".into(),
                uid_validity: 9,
                uid: 2,
                ..source()
            },
        });
        let result = Outcome {
            operation_id: "op-1".into(),
            items: vec![item],
            undo_token: Some("op-1".into()),
            expires_at: 1000,
        };
        store.save(&result, true).unwrap();
        assert!(store.take_undo("op-1", 1000).is_err());
        assert_eq!(store.take_undo("op-1", 999).unwrap().len(), 1);
        assert!(store.take_undo("op-1", 999).is_err());
    }
    #[test]
    fn only_exact_single_uid_copy_receipts_authorize_undo() {
        use async_imap::imap_proto::{ResponseCode, rfc4315::UidSetMember};
        assert_eq!(
            copy_mapping(
                &ResponseCode::CopyUid(9, vec![UidSetMember::Uid(1)], vec![UidSetMember::Uid(4)]),
                1
            ),
            Some((9, 4))
        );
        assert!(
            copy_mapping(
                &ResponseCode::CopyUid(
                    9,
                    vec![UidSetMember::UidRange(1..=u32::MAX)],
                    vec![UidSetMember::Uid(4)]
                ),
                1
            )
            .is_none()
        );
        assert!(
            copy_mapping(
                &ResponseCode::CopyUid(0, vec![UidSetMember::Uid(1)], vec![UidSetMember::Uid(4)]),
                1
            )
            .is_none()
        );
    }
    #[tokio::test]
    async fn disappearance_between_preflight_and_move_never_reports_success() {
        for caps in ["IMAP4rev1 MOVE UIDPLUS", "IMAP4rev1 UIDPLUS"] {
            let server = crate::test_imap::state(caps, &[("INBOX", 7), ("Archive", 9)]);
            {
                let mut st = server.lock().unwrap();
                st.add("INBOX", "Subject: concurrent\r\n\r\nbody", &[]);
                st.vanish_before_move = true;
            }
            let mut session = crate::test_imap::session(server.clone()).await;
            let result = mutate(
                &mut session,
                &source(),
                1,
                "archive",
                None,
                Some("Archive"),
                false,
                None,
            )
            .await;
            assert_eq!(
                result.status, "unknown",
                "{caps}: tagged OK on a vanished UID is not proof of a move"
            );
            assert!(result.undo.is_none());
            let mut st = server.lock().unwrap();
            assert!(st.mailbox("Archive").messages.is_empty());
            assert!(
                !st.commands
                    .iter()
                    .any(|c| c.starts_with("UID STORE") || c.starts_with("UID EXPUNGE"))
            );
        }
    }

    #[tokio::test]
    async fn copying_without_a_receipt_keeps_the_original() {
        let server = crate::test_imap::state("IMAP4rev1 UIDPLUS", &[("INBOX", 7), ("Archive", 9)]);
        {
            let mut st = server.lock().unwrap();
            st.add("INBOX", "Subject: keep original\r\n\r\nbody", &[]);
            st.omit_copyuid = true;
        }
        let mut session = crate::test_imap::session(server.clone()).await;
        let result = mutate(
            &mut session,
            &source(),
            1,
            "archive",
            None,
            Some("Archive"),
            false,
            None,
        )
        .await;
        assert_eq!(result.status, "unknown");
        assert!(result.undo.is_none());
        let mut st = server.lock().unwrap();
        assert_eq!(st.mailbox("Archive").messages.len(), 1);
        assert_eq!(st.mailbox("INBOX").messages.len(), 1);
        assert!(
            !st.commands
                .iter()
                .any(|c| c.starts_with("UID STORE") || c.starts_with("UID EXPUNGE"))
        );
    }

    #[tokio::test]
    async fn move_without_copyuid_is_unknown_even_if_source_disappears() {
        let server = crate::test_imap::state("IMAP4rev1 MOVE", &[("INBOX", 7), ("Archive", 9)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: no receipt\r\n\r\nbody", &[]);
        let mut session = crate::test_imap::session(server.clone()).await;
        let result = mutate(
            &mut session,
            &source(),
            1,
            "archive",
            None,
            Some("Archive"),
            false,
            None,
        )
        .await;
        assert_eq!(result.status, "unknown");
        assert!(result.undo.is_none());
    }
}
