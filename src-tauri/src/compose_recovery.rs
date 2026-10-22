//! Durable editing snapshots. An editor view is never the owner of recovery data.
use crate::smtp::OutgoingAttachment;
use anyhow::{Context, Result, bail};
use base64::Engine;
use mailbeacon_parser::MimeHeaders;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{io::Write, path::PathBuf, sync::Mutex};

const MAX_RECORD_BYTES: u64 = 200 * 1024 * 1024;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub session_id: String,
    pub revision: u64,
    pub account_id: String,
    pub subject: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct ManagedAttachment {
    name: String,
    content_type: String,
    data: String,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub session_id: String,
    pub revision: u64,
    pub draft: Value,
    pub editor: Value,
    attachments: Vec<ManagedAttachment>,
    #[serde(default)]
    deleted: bool,
}
impl Snapshot {
    fn summary(&self) -> Summary {
        Summary {
            session_id: self.session_id.clone(),
            revision: self.revision,
            account_id: self.draft["accountId"].as_str().unwrap_or_default().into(),
            subject: self.draft["subject"].as_str().unwrap_or_default().into(),
        }
    }
}

/// One lock covers revision checks and atomic file replacement.
pub struct ComposeRecovery {
    dir: PathBuf,
    lock: Mutex<()>,
}
impl ComposeRecovery {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lock: Mutex::new(()),
        }
    }
    fn path(&self, session_id: &str) -> Result<PathBuf> {
        if session_id.len() > 128
            || session_id.is_empty()
            || !session_id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            bail!("草稿会话标识无效");
        }
        Ok(self.dir.join(format!("{session_id}.json")))
    }
    fn read(&self, session_id: &str) -> Result<Option<Snapshot>> {
        let path = self.path(session_id)?;
        let meta = match std::fs::metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        if meta.len() > MAX_RECORD_BYTES {
            bail!("恢复草稿超过大小限制");
        }
        let record: Snapshot = serde_json::from_slice(&std::fs::read(path)?)
            .context("恢复草稿已损坏，原文件已保留")?;
        if record.session_id != session_id {
            bail!("恢复草稿身份不一致");
        }
        Ok(Some(record))
    }
    fn write(&self, record: &Snapshot) -> Result<()> {
        let path = self.path(&record.session_id)?;
        std::fs::create_dir_all(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let encoded = serde_json::to_vec(record)?;
        if encoded.len() as u64 > MAX_RECORD_BYTES {
            bail!("恢复草稿超过大小限制");
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        file.write_all(&encoded)?;
        file.as_file().sync_all()?;
        file.persist(path).map_err(|e| e.error)?;
        std::fs::File::open(&self.dir)?.sync_all()?;
        Ok(())
    }
    /// Commit an entire revision. Older callbacks and ended sessions cannot overwrite it.
    pub fn save(
        &self,
        session_id: &str,
        revision: u64,
        draft: Value,
        editor: Value,
        attachments: Vec<OutgoingAttachment>,
    ) -> Result<Summary> {
        self.path(session_id)?;
        crate::drafts::safe_account(draft["accountId"].as_str().unwrap_or_default())?;
        if attachments.iter().map(|a| a.data.len()).sum::<usize>()
            > crate::smtp::MAX_ATTACHMENT_BYTES
        {
            bail!("附件总大小超过上限");
        }
        let record = Snapshot {
            session_id: session_id.into(),
            revision,
            draft,
            editor,
            deleted: false,
            attachments: attachments
                .into_iter()
                .map(|a| ManagedAttachment {
                    name: a.name,
                    content_type: a.content_type,
                    data: base64::engine::general_purpose::STANDARD.encode(a.data),
                })
                .collect(),
        };
        let _lock = self.lock.lock().unwrap();
        if let Some(previous) = self.read(session_id)? {
            if previous.deleted {
                bail!("写信会话已结束，不可恢复旧保存");
            }
            if previous.revision > revision {
                bail!("草稿保存版本已过期");
            }
            if previous.revision == revision {
                if previous == record {
                    return Ok(previous.summary());
                }
                bail!("相同草稿版本包含不同内容");
            }
        }
        self.write(&record)?;
        Ok(record.summary())
    }
    /// List only live records. Corruption is surfaced rather than silently losing drafts.
    pub fn list(&self) -> Result<Vec<Summary>> {
        let _lock = self.lock.lock().unwrap();
        let entries = match std::fs::read_dir(&self.dir) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(e.into()),
        };
        let mut out = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let Some(id) = name.to_str().and_then(|s| s.strip_suffix(".json")) else {
                continue;
            };
            if let Some(record) = self.read(id)? {
                if !record.deleted {
                    out.push(record.summary());
                }
            }
        }
        out.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        Ok(out)
    }
    pub fn list_editable(&self, sends: &crate::send_records::Store) -> Result<Vec<Summary>> {
        let mut editable = Vec::new();
        for record in self.list()? {
            if !sends.owns_compose_session(&record.session_id)? {
                editable.push(record);
            }
        }
        Ok(editable)
    }
    pub fn restore_editable(
        &self,
        session_id: &str,
        sends: &crate::send_records::Store,
    ) -> Result<Snapshot> {
        if sends.owns_compose_session(session_id)? {
            bail!("该草稿已交给发送缓冲，请从发送记录恢复，避免重复发送");
        }
        self.restore(session_id)
    }

    /// Read a confirmed snapshot; attachment bytes are independent of process tokens.
    pub fn restore(&self, session_id: &str) -> Result<Snapshot> {
        let _lock = self.lock.lock().unwrap();
        let record = self.read(session_id)?.context("恢复草稿不存在")?;
        if record.deleted {
            bail!("写信会话已结束");
        }
        Ok(record)
    }
    /// Keep a small tombstone so a late save cannot resurrect a discarded draft.
    pub fn remove(&self, session_id: &str, revision: u64) -> Result<()> {
        let _lock = self.lock.lock().unwrap();
        let old = self.read(session_id)?;
        if old.as_ref().is_some_and(|r| r.deleted) {
            return Ok(());
        }
        if old.as_ref().is_some_and(|r| r.revision > revision) {
            bail!("草稿关闭版本已过期");
        }
        self.write(&Snapshot {
            session_id: session_id.into(),
            revision,
            draft: Value::Null,
            editor: Value::Null,
            attachments: vec![],
            deleted: true,
        })
    }
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        for record in self
            .list()?
            .into_iter()
            .filter(|r| r.account_id == account_id)
        {
            self.remove(&record.session_id, record.revision.saturating_add(1))?;
        }
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveInput {
    pub session_id: String,
    pub revision: u64,
    pub draft: Value,
    pub editor: Value,
}

// Match complete CID URLs, including the case-insensitive scheme and escaped
// identifiers accepted by compose-quote.js; never rewrite a different CID prefix.
fn embed_cid(html: &str, cid: &str, data: &str) -> String {
    let bytes = html.as_bytes();
    let mut output = String::with_capacity(html.len());
    let mut copied = 0;
    let mut at = 0;
    while at + 4 <= bytes.len() {
        if !bytes[at..at + 4].eq_ignore_ascii_case(b"cid:") {
            at += 1;
            continue;
        }
        let start = at;
        at += 4;
        let value_start = at;
        while at < bytes.len()
            && !bytes[at].is_ascii_whitespace()
            && !b"<>\"')".contains(&bytes[at])
        {
            at += 1;
        }
        let value = &bytes[value_start..at];
        let mut decoded = Vec::new();
        let mut offset = 0;
        while offset < value.len() {
            if value[offset] == b'%' && offset + 2 < value.len() {
                let hex = std::str::from_utf8(&value[offset + 1..offset + 3])
                    .ok()
                    .and_then(|s| u8::from_str_radix(s, 16).ok());
                if let Some(byte) = hex {
                    decoded.push(byte);
                    offset += 3;
                    continue;
                }
            }
            decoded.push(value[offset]);
            offset += 1;
        }
        if decoded == cid.as_bytes() {
            output.push_str(&html[copied..start]);
            output.push_str(data);
            copied = at;
        }
    }
    output.push_str(&html[copied..]);
    output
}

/// Make CID images self-contained before persistence; later endpoint changes
/// cannot silently substitute a different message for the quoted original.
fn preserve_quote_images(draft: &mut Value, editor: &mut Value, raw: &[u8]) -> Result<()> {
    let parsed = crate::mime::parse(raw)?;
    let cids = draft["quoteCids"].as_array().cloned().unwrap_or_default();
    for value in cids {
        let cid = value.as_str().context("引用图片标识无效")?;
        let part = parsed
            .attachments()
            .find(|p| {
                p.content_id()
                    .is_some_and(|id| id.trim_matches(['<', '>']) == cid)
            })
            .context("引用图片缺失，草稿尚未完整保存")?;
        let ct = part.content_type();
        let mime = ct
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
            .unwrap_or_else(|| "application/octet-stream".into());
        if !mime.starts_with("image/") {
            bail!("引用资源不是可保存的内嵌图片");
        }
        let data = format!(
            "data:{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(part.contents())
        );
        for pointer in ["/htmlBody"] {
            if let Some(text) = draft.pointer(pointer).and_then(Value::as_str) {
                let converted = embed_cid(text, cid, &data);
                *draft.pointer_mut(pointer).unwrap() = Value::String(converted);
            }
        }
        for pointer in ["/quote/html", "/quotePreview/html"] {
            if let Some(text) = editor.pointer(pointer).and_then(Value::as_str) {
                let converted = embed_cid(text, cid, &data);
                *editor.pointer_mut(pointer).unwrap() = Value::String(converted);
            }
        }
    }
    draft["quoteCids"] = serde_json::json!([]);
    draft["quoteSource"] = Value::Null;
    if editor["quote"].is_object() {
        editor["quote"]["cids"] = serde_json::json!([]);
    }
    editor["quoteSource"] = Value::Null;
    Ok(())
}

#[tauri::command]
pub async fn compose_recovery_save(
    state: tauri::State<'_, crate::AppState>,
    mut input: SaveInput,
) -> crate::CmdResult<Summary> {
    let draft: crate::smtp::Draft =
        serde_json::from_value(input.draft.clone()).map_err(|e| crate::err(e.into()))?;
    let _account = state.account_gate(&draft.account_id).read_owned().await;
    state.account(&draft.account_id).map_err(crate::err)?;
    let attachments = crate::resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(crate::err)?;
    if let Some(raw) = crate::quote_raw(&state, &draft).await? {
        preserve_quote_images(&mut input.draft, &mut input.editor, &raw).map_err(crate::err)?;
    }
    state
        .compose_recovery
        .save(
            &input.session_id,
            input.revision,
            input.draft,
            input.editor,
            attachments,
        )
        .map_err(crate::err)
}

#[tauri::command]
pub fn compose_recovery_list(
    state: tauri::State<'_, crate::AppState>,
) -> crate::CmdResult<Vec<Summary>> {
    let sends = state.send_records.as_ref().map_err(Clone::clone)?;
    state
        .compose_recovery
        .list_editable(sends)
        .map_err(crate::err)
}

#[tauri::command]
pub fn compose_recovery_restore(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
) -> crate::CmdResult<Value> {
    let sends = state.send_records.as_ref().map_err(Clone::clone)?;
    let record = state
        .compose_recovery
        .restore_editable(&session_id, sends)
        .map_err(crate::err)?;
    state
        .account(record.draft["accountId"].as_str().unwrap_or_default())
        .map_err(crate::err)?;
    let decoded: Vec<_> = record
        .attachments
        .iter()
        .map(|a| {
            base64::engine::general_purpose::STANDARD
                .decode(&a.data)
                .map(|data| (a, data))
        })
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| crate::err(e.into()))?;
    let picked: Vec<_> = decoded
        .into_iter()
        .map(|(a, data)| {
            state
                .attachments
                .register_bytes(a.name.clone(), a.content_type.clone(), data)
        })
        .collect();
    let mut draft = record.draft;
    draft["attachments"] = Value::Array(
        picked
            .iter()
            .map(|p| serde_json::json!({"kind":"file","token":p.token}))
            .collect(),
    );
    Ok(
        serde_json::json!({"sessionId":record.session_id,"revision":record.revision,"draft":draft,"editor":record.editor,"attachments":picked}),
    )
}

#[tauri::command]
pub fn compose_recovery_remove(
    state: tauri::State<'_, crate::AppState>,
    session_id: String,
    revision: u64,
) -> crate::CmdResult<()> {
    state
        .compose_recovery
        .remove(&session_id, revision)
        .map_err(crate::err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn complete_snapshot_survives_reopen_without_attachment_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = ComposeRecovery::new(dir.path().to_owned());
        store.save("session-1", 1, json!({"accountId":"a", "subject":"恢复", "bcc":"hidden@example.com", "requestReceipt":true, "htmlBody":"<b>正文</b>"}), json!({"html":"<b>正文</b>","followupSeconds":86400}), vec![OutgoingAttachment {name:"同名.pdf".into(),content_type:"application/pdf".into(),data:b"attachment".to_vec()}]).unwrap();
        drop(store);
        let reopened = ComposeRecovery::new(dir.path().to_owned());
        assert_eq!(
            reopened.list().unwrap().len(),
            1,
            "saved compose snapshots must survive process restart"
        );
        let record = reopened.restore("session-1").unwrap();
        assert_eq!(record.draft["bcc"], "hidden@example.com");
        assert_eq!(record.draft["requestReceipt"], true);
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(&record.attachments[0].data)
                .unwrap(),
            b"attachment"
        );
        assert_eq!(record.editor["followupSeconds"], 86400);
    }
    #[test]
    fn stale_revisions_and_late_writes_after_discard_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = ComposeRecovery::new(dir.path().to_owned());
        let draft = json!({"accountId":"a","subject":"new"});
        store
            .save("s", 2, draft.clone(), json!({}), vec![])
            .unwrap();
        assert!(
            store
                .save("s", 1, draft.clone(), json!({}), vec![])
                .is_err()
        );
        assert!(
            store
                .save(
                    "s",
                    2,
                    json!({"accountId":"a","subject":"different"}),
                    json!({}),
                    vec![]
                )
                .is_err()
        );
        store.remove("s", 3).unwrap();
        store.remove("s", 3).unwrap();
        assert!(store.save("s", 4, draft, json!({}), vec![]).is_err());
        assert!(store.restore("s").is_err());
        assert!(store.list().unwrap().is_empty());
    }
    #[test]
    fn failed_write_keeps_previous_snapshot_and_paths_cannot_escape() {
        let dir = tempfile::tempdir().unwrap();
        let store = ComposeRecovery::new(dir.path().to_owned());
        let draft = json!({"accountId":"a"});
        assert!(
            store
                .save("../escape", 1, draft.clone(), json!({}), vec![])
                .is_err()
        );
        store
            .save("s", 1, draft.clone(), json!({"html":"first"}), vec![])
            .unwrap();
        // An invalid next payload never replaces the last confirmed revision.
        assert!(
            store
                .save("s", 2, json!({"accountId":"../other"}), json!({}), vec![])
                .is_err()
        );
        assert_eq!(store.restore("s").unwrap().editor["html"], "first");
    }
    #[test]
    fn quote_cid_bytes_are_embedded_in_both_send_payload_and_editor() {
        let raw=b"MIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=x\r\n\r\n--x\r\nContent-Type: text/html\r\n\r\n<img src=\"cid:photo\">\r\n--x\r\nContent-Type: image/png\r\nContent-ID: <photo>\r\nContent-Transfer-Encoding: base64\r\n\r\nYWJj\r\n--x--\r\n";
        let mut draft = json!({"htmlBody":"<img src=\"CID:%70hoto\"><img src=\"cid:photo-other\">","quoteCids":["photo"]});
        let mut editor = json!({"quote":{"html":"<img src=\"cid:photo\">","cids":["photo"]}});
        preserve_quote_images(&mut draft, &mut editor, raw).unwrap();
        assert!(
            draft["htmlBody"]
                .as_str()
                .unwrap()
                .contains("data:image/png;base64,YWJj")
        );
        assert!(
            editor["quote"]["html"]
                .as_str()
                .unwrap()
                .contains("data:image/png;base64,YWJj")
        );
        assert!(
            draft["htmlBody"]
                .as_str()
                .unwrap()
                .contains("cid:photo-other")
        );
        assert_eq!(draft["quoteCids"], json!([]));
    }
    #[test]
    fn queued_sessions_never_return_as_editable_recoveries_even_after_cancel_or_restart() {
        use crate::send_records::{NewRecord, Status, Store, buffer::BufferInput};
        let dir = tempfile::tempdir().unwrap();
        let recovery = ComposeRecovery::new(dir.path().join("compose"));
        for id in ["queued", "unrelated"] {
            recovery
                .save(id, 3, json!({"accountId":"a"}), json!({}), vec![])
                .unwrap();
        }
        let sends = Store::open(dir.path()).unwrap();
        sends
            .enqueue_buffer(
                NewRecord {
                    id: "send1".into(),
                    account_id: "a".into(),
                    from: "a@example.com".into(),
                    subject: "s".into(),
                    recipients: vec!["b@example.com".into()],
                    message_id: "<m@example.com>".into(),
                    endpoint: "a".into(),
                    scheduled_id: None,
                    previous_id: None,
                },
                b"raw",
                b"draft",
                BufferInput {
                    request_id: "request1".into(),
                    payload_hash: "hash".into(),
                    compose_session_id: "queued".into(),
                    compose_revision: 3,
                    deadline: 2000,
                    sender_fingerprint: "smtp-a".into(),
                    receive_epoch: 1,
                    followup_seconds: None,
                    request_receipt: false,
                },
            )
            .unwrap();
        for status in [
            Status::Buffered,
            Status::Paused,
            Status::Submitting,
            Status::Unknown,
            Status::Accepted,
            Status::Rejected,
            Status::Cancelled,
        ] {
            sends
                .update("send1", |r| {
                    r.status = status;
                    Ok(())
                })
                .unwrap();
            assert!(
                recovery.restore_editable("queued", &sends).is_err(),
                "queued session in {status:?} must not become a duplicate editable draft"
            );
            assert_eq!(
                recovery
                    .list_editable(&sends)
                    .unwrap()
                    .iter()
                    .map(|r| r.session_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["unrelated"]
            );
        }
        drop(sends);
        let reopened = Store::open(dir.path()).unwrap();
        assert!(recovery.restore_editable("queued", &reopened).is_err());
        assert!(recovery.restore_editable("unrelated", &reopened).is_ok());
    }
}
