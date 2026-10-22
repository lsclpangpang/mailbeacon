//! Local snooze/follow-up state and conservative, versioned reply evidence.
use super::*;
use crate::mail_ref::MessageRef;
use mailbeacon_parser::MimeHeaders;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReminderKind {
    Snooze,
    Followup,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReminderState {
    Scheduled,
    Checking,
    Due,
    Completed,
    Cancelled,
    Orphaned,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCoverage {
    pub folder: String,
    pub uid_validity: u32,
    pub scanned_uid: u32,
    pub upper_uid: u32,
    pub completed_at: i64,
    pub exhausted: bool,
    pub error: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reminder {
    pub id: String,
    pub kind: ReminderKind,
    pub account_id: String,
    pub receive_epoch: u64,
    pub source: Option<MessageRef>,
    pub send_record_id: Option<String>,
    pub anchor_message_id: String,
    pub thread_root_message_id: String,
    pub anchors: Vec<String>,
    pub accepted_at: Option<i64>,
    pub due_at: i64,
    pub state: ReminderState,
    pub version: u64,
    pub created_at: i64,
    pub notified_at: Option<i64>,
    pub last_verified_at: Option<i64>,
    pub scope: Vec<String>,
    pub coverage: Vec<FolderCoverage>,
    pub error: Option<String>,
}
impl Reminder {
    pub fn active(&self) -> bool {
        matches!(
            self.state,
            ReminderState::Scheduled | ReminderState::Checking | ReminderState::Due
        )
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplyEvidence {
    pub source: MessageRef,
    pub message_id: String,
    pub references: Vec<String>,
    pub sender: String,
    pub automatic: bool,
    pub observed_at: i64,
    #[serde(default)]
    pub received_at: Option<i64>,
}

pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mail_reminders(id TEXT PRIMARY KEY,account_id TEXT NOT NULL,receive_epoch INTEGER NOT NULL,source_key TEXT UNIQUE,send_id TEXT UNIQUE,record TEXT NOT NULL);
 CREATE INDEX IF NOT EXISTS mail_reminders_account ON mail_reminders(account_id,receive_epoch);
 CREATE TABLE IF NOT EXISTS mail_reply_evidence(source_key TEXT PRIMARY KEY,account_id TEXT NOT NULL,receive_epoch INTEGER NOT NULL,record TEXT NOT NULL);
 CREATE TABLE IF NOT EXISTS mail_reminder_invalid_epochs(account_id TEXT NOT NULL,receive_epoch INTEGER NOT NULL,PRIMARY KEY(account_id,receive_epoch));
 INSERT OR IGNORE INTO mail_productivity_migrations VALUES('mail_reminders',1);")?;
    Ok(())
}
fn read_reminder(c: &Connection, id: &str) -> Result<Option<Reminder>> {
    let v: Option<String> = c
        .query_row("SELECT record FROM mail_reminders WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    v.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
/// Shared transactional writer; immutable source identity is indexed separately.
pub(super) fn write_reminder(c: &Connection, r: &Reminder) -> Result<()> {
    let source_key = r
        .source
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?
        .map(|s| hash(s.as_bytes()));
    c.execute("INSERT INTO mail_reminders(id,account_id,receive_epoch,source_key,send_id,record) VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET record=excluded.record",params![r.id,r.account_id,r.receive_epoch,source_key,r.send_record_id,serde_json::to_string(r)?])?;
    Ok(())
}
fn all(c: &Connection, account: Option<&str>) -> Result<Vec<Reminder>> {
    let mut stmt=c.prepare("SELECT record FROM mail_reminders WHERE (?1 IS NULL OR account_id=?1) ORDER BY json_extract(record,'$.dueAt'),id")?;
    stmt.query_map([account], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect()
}
fn initial(id: String, account: String, epoch: u64, due: i64, at: i64) -> Reminder {
    Reminder {
        id,
        kind: ReminderKind::Snooze,
        account_id: account,
        receive_epoch: epoch,
        source: None,
        send_record_id: None,
        anchor_message_id: String::new(),
        thread_root_message_id: String::new(),
        anchors: vec![],
        accepted_at: None,
        due_at: due,
        state: ReminderState::Scheduled,
        version: 1,
        created_at: at,
        notified_at: None,
        last_verified_at: None,
        scope: vec![],
        coverage: vec![],
        error: None,
    }
}
fn id_token(value: &str) -> &str {
    value.trim().trim_matches(['<', '>'])
}
fn related(r: &Reminder, e: &ReplyEvidence, own: &[String]) -> bool {
    r.active()
        && r.account_id == e.source.account_id
        && r.receive_epoch == e.source.receive_epoch
        && !e.automatic
        && !e.sender.is_empty()
        && !own.iter().any(|s| s.eq_ignore_ascii_case(&e.sender))
        && !r.source.as_ref().is_some_and(|s| s == &e.source)
        && (r.kind == ReminderKind::Followup
            || (e.observed_at >= r.created_at
                && e.received_at.is_some_and(|at| at >= r.created_at)))
        && e.references.iter().any(|reference| {
            r.anchors
                .iter()
                .any(|anchor| !anchor.is_empty() && id_token(reference) == id_token(anchor))
        })
}
/// Called in the same transaction as the accepted send record.
pub(super) fn activate_followup(c: &Connection, record: &Record) -> Result<()> {
    if record.status != Status::Accepted {
        return Ok(());
    }
    let Some(at) = record.accepted_at else {
        return Ok(());
    };
    let meta: Option<String> = c
        .query_row(
            "SELECT metadata FROM send_buffer WHERE send_id=?1",
            [&record.id],
            |r| r.get(0),
        )
        .optional()?;
    let Some(meta) = meta else {
        return Ok(());
    };
    let meta: buffer::Metadata = serde_json::from_str(&meta)?;
    let Some(seconds) = meta.input.followup_seconds else {
        return Ok(());
    };
    let id = format!("followup-{}", record.id);
    if read_reminder(c, &id)?.is_some() {
        return Ok(());
    }
    let mut r = initial(
        id,
        record.account_id.clone(),
        meta.input.receive_epoch,
        at.checked_add((seconds * 1000) as i64)
            .ok_or_else(|| anyhow!("跟进时间溢出"))?,
        at,
    );
    r.kind = ReminderKind::Followup;
    r.send_record_id = Some(record.id.clone());
    r.anchor_message_id = id_token(&record.message_id).into();
    r.anchors = vec![r.anchor_message_id.clone()];
    r.thread_root_message_id = r.anchor_message_id.clone();
    r.accepted_at = Some(at);
    // Evidence may have arrived between SMTP acceptance and this transaction.
    let mut stmt = c.prepare(
        "SELECT record FROM mail_reply_evidence WHERE account_id=?1 AND receive_epoch=?2",
    )?;
    for row in stmt.query_map(params![r.account_id, r.receive_epoch], |row| {
        row.get::<_, String>(0)
    })? {
        let e: ReplyEvidence = serde_json::from_str(&row?)?;
        if related(&r, &e, std::slice::from_ref(&record.from)) {
            r.state = ReminderState::Completed;
            break;
        }
    }
    write_reminder(c, &r)
}
impl Store {
    pub fn reconcile_followups(&self) -> Result<()> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let records = {
            let mut q = tx.prepare(
                "SELECT record FROM send_records WHERE json_extract(record,'$.status')='accepted'",
            )?;
            q.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for json in records {
            activate_followup(&tx, &serde_json::from_str(&json)?)?;
        }
        tx.commit()?;
        Ok(())
    }
    pub fn list_reminders(&self, account: Option<&str>) -> Result<Vec<Reminder>> {
        let c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        all(&c, account)
    }
    pub fn get_reminder(&self, id: &str) -> Result<Option<Reminder>> {
        let c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        read_reminder(&c, id)
    }
    /// Local-only snooze, with a unique stable source and optimistic edits.
    pub fn set_snooze(
        &self,
        source: MessageRef,
        due: i64,
        id: Option<&str>,
        expected: Option<u64>,
        at: i64,
        anchors: Vec<String>,
    ) -> Result<Reminder> {
        source.validate()?;
        if due <= at {
            bail!("请选择未来的提醒时间");
        }
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mail_reminder_invalid_epochs WHERE account_id=?1 AND receive_epoch=?2)",params![source.account_id,source.receive_epoch],|r|r.get(0))?;
        if invalid {
            bail!("原邮箱身份已失效");
        }
        let key = hash(serde_json::to_string(&source)?.as_bytes());
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM mail_reminders WHERE source_key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?;
        let mut r = if let Some(existing) = existing {
            let mut r = read_reminder(&tx, &existing)?.ok_or_else(|| anyhow!("提醒不存在"))?;
            if expected != Some(r.version) || id.is_some_and(|id| id != r.id) {
                bail!("此邮件已有提醒，请刷新后修改");
            }
            r.version += 1;
            r
        } else {
            if id.is_some() || expected.is_some() {
                bail!("提醒不存在");
            }
            initial(
                new_id()?,
                source.account_id.clone(),
                source.receive_epoch,
                due,
                at,
            )
        };
        r.source = Some(source);
        r.due_at = due;
        r.state = ReminderState::Scheduled;
        r.notified_at = None;
        r.error = None;
        r.anchors = anchors
            .into_iter()
            .map(|a| id_token(&a).to_string())
            .filter(|a| !a.is_empty())
            .collect();
        r.anchor_message_id = r.anchors.first().cloned().unwrap_or_default();
        if r.anchor_message_id.is_empty() {
            r.error = Some("原邮件没有可用的邮件标识，将仅按设定时间恢复".into());
        }
        write_reminder(&tx, &r)?;
        tx.commit()?;
        Ok(r)
    }
    /// Versioned user operation. Completed/cancelled reminders require explicit rescheduling.
    pub fn change_reminder(
        &self,
        id: &str,
        expected: u64,
        action: &str,
        due: Option<i64>,
        at: i64,
    ) -> Result<Reminder> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut r = read_reminder(&tx, id)?.ok_or_else(|| anyhow!("提醒不存在"))?;
        if r.version != expected {
            bail!("提醒已更新，请刷新");
        }
        let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mail_reminder_invalid_epochs WHERE account_id=?1 AND receive_epoch=?2)",params![r.account_id,r.receive_epoch],|r|r.get(0))?;
        if invalid || r.state == ReminderState::Orphaned {
            bail!("原邮件身份失效，不能重新启用");
        }
        match action {
            "cancel" => r.state = ReminderState::Cancelled,
            "complete" => r.state = ReminderState::Completed,
            "restore" => {
                if r.kind != ReminderKind::Snooze {
                    bail!("此提醒不是稍后处理");
                }
                r.state = ReminderState::Due;
            }
            "reschedule" => {
                let due = due
                    .filter(|t| *t > at)
                    .ok_or_else(|| anyhow!("请选择未来的提醒时间"))?;
                r.due_at = due;
                r.state = ReminderState::Scheduled;
                r.notified_at = None;
                r.last_verified_at = None;
                r.coverage.clear();
            }
            _ => bail!("无效提醒操作"),
        };
        r.version += 1;
        write_reminder(&tx, &r)?;
        tx.commit()?;
        Ok(r)
    }
    /// Atomically record due notifications; OS delivery is best effort, persistent inbox is authoritative.
    pub fn tick_reminders(&self, at: i64) -> Result<Vec<Reminder>> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut notified = vec![];
        for mut r in all(&tx, None)? {
            if !r.active() || r.due_at > at {
                continue;
            }
            let old = r.state;
            if r.state == ReminderState::Scheduled {
                r.state = if r.kind == ReminderKind::Snooze {
                    ReminderState::Due
                } else {
                    ReminderState::Checking
                };
            }
            let notify = r.state == ReminderState::Due && r.notified_at.is_none();
            if notify {
                r.notified_at = Some(at);
            }
            if old != r.state || notify {
                r.version += 1;
                write_reminder(&tx, &r)?;
                if notify {
                    notified.push(r);
                }
            }
        }
        tx.commit()?;
        Ok(notified)
    }
    /// Persist genuine header evidence and resolve only exact anchors within the same receive epoch.
    pub fn observe_reply(&self, e: &ReplyEvidence, own: &[String]) -> Result<Vec<Reminder>> {
        e.source.validate()?;
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let invalid:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM mail_reminder_invalid_epochs WHERE account_id=?1 AND receive_epoch=?2)",params![e.source.account_id,e.source.receive_epoch],|r|r.get(0))?;
        if invalid {
            return Ok(vec![]);
        }
        let key = hash(serde_json::to_string(&e.source)?.as_bytes());
        let seen: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM mail_reply_evidence WHERE source_key=?1)",
            [&key],
            |r| r.get(0),
        )?;
        if seen {
            return Ok(vec![]);
        }
        let mut stored = e.clone();
        stored.automatic |= own.iter().any(|a| a.eq_ignore_ascii_case(&e.sender));
        tx.execute(
            "INSERT OR REPLACE INTO mail_reply_evidence VALUES(?1,?2,?3,?4)",
            params![
                key,
                e.source.account_id,
                e.source.receive_epoch,
                serde_json::to_string(&stored)?
            ],
        )?;
        let mut changed = vec![];
        for mut r in all(&tx, Some(&e.source.account_id))? {
            if related(&r, &stored, own) {
                r.state = if r.kind == ReminderKind::Followup {
                    ReminderState::Completed
                } else {
                    ReminderState::Due
                };
                if r.kind == ReminderKind::Snooze {
                    r.due_at = e.observed_at;
                    // The reply event already explains this early restoration; do not notify again at the old deadline.
                    r.notified_at = Some(e.observed_at);
                }
                r.version += 1;
                r.error = None;
                write_reminder(&tx, &r)?;
                changed.push(r);
            }
        }
        tx.commit()?;
        Ok(changed)
    }
    /// Complete a negative verification only from a fresh, exhausted snapshot of every scoped folder.
    #[allow(clippy::too_many_arguments)]
    pub fn finish_reminder_round(
        &self,
        id: &str,
        expected: u64,
        started: i64,
        epoch: u64,
        mut roster: Vec<String>,
        coverage: Vec<FolderCoverage>,
        at: i64,
    ) -> Result<Reminder> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut r = read_reminder(&tx, id)?.ok_or_else(|| anyhow!("提醒不存在"))?;
        if r.version != expected || !r.active() {
            bail!("提醒已变化，旧扫描不再写回");
        }
        roster.sort();
        roster.dedup();
        let mut checked: Vec<_> = coverage.iter().map(|c| c.folder.clone()).collect();
        checked.sort();
        if started < r.due_at
            || epoch != r.receive_epoch
            || roster.is_empty()
            || checked != roster
            || coverage.iter().any(|c| {
                c.uid_validity == 0
                    || !c.exhausted
                    || c.error.is_some()
                    || c.completed_at < started
                    || c.scanned_uid < c.upper_uid
            })
        {
            bail!("核对尚未完整，请继续同步");
        }
        r.state = ReminderState::Due;
        r.last_verified_at = coverage.iter().map(|c| c.completed_at).min();
        r.scope = roster;
        r.coverage = coverage;
        r.error = None;
        r.version += 1;
        let _ = at;
        write_reminder(&tx, &r)?;
        tx.commit()?;
        Ok(r)
    }
    pub fn reminder_check_error(
        &self,
        id: &str,
        expected: u64,
        error: &str,
        at: i64,
    ) -> Result<()> {
        let c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        if let Some(mut r) = read_reminder(&c, id)? {
            if r.version == expected && r.active() {
                r.error = Some(error.into());
                if r.due_at <= at && r.kind == ReminderKind::Followup {
                    r.state = ReminderState::Checking;
                }
                r.version += 1;
                write_reminder(&c, &r)?;
            }
        }
        Ok(())
    }
    /// A tombstone also refuses late evidence from an old or removed account.
    pub fn invalidate_reminders(&self, account: &str, epoch: u64, removed: bool) -> Result<()> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO mail_reminder_invalid_epochs VALUES(?1,?2)",
            params![account, epoch],
        )?;
        for mut r in all(&tx, Some(account))? {
            if (removed || r.receive_epoch == epoch) && r.active() {
                r.state = if removed {
                    ReminderState::Cancelled
                } else {
                    ReminderState::Orphaned
                };
                r.version += 1;
                write_reminder(&tx, &r)?;
            }
        }
        tx.execute(
            "DELETE FROM mail_reply_evidence WHERE account_id=?1 AND (?2 OR receive_epoch=?3)",
            params![account, removed, epoch],
        )?;
        tx.commit()?;
        Ok(())
    }
}
/// Parse only complete headers; automatic replies, delivery reports and receipts never close follow-ups.
pub fn reply_from_headers(source: MessageRef, raw: &[u8], at: i64) -> Result<ReplyEvidence> {
    let fixed = crate::mime::fix_headers(raw);
    let parsed = mailbeacon_parser::MessageParser::default()
        .parse_headers(&fixed)
        .ok_or_else(|| anyhow!("邮件头无法解析"))?;
    let (message_id, in_reply_to, mut references) = crate::imap::thread_headers(Some(&parsed));
    if !in_reply_to.is_empty() {
        references.push(in_reply_to);
    }
    let sender = parsed
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .unwrap_or("")
        .to_lowercase();
    let auto = parsed
        .header_raw("Auto-Submitted")
        .is_some_and(|h| !h.trim().eq_ignore_ascii_case("no"));
    let report = parsed.content_type().is_some_and(|t| {
        t.ctype().eq_ignore_ascii_case("multipart")
            && t.subtype()
                .is_some_and(|s| s.eq_ignore_ascii_case("report"))
    }) || parsed.header_raw("Content-Type").is_some_and(|h| {
        h.to_ascii_lowercase().contains("report-type=")
            || h.to_ascii_lowercase()
                .contains("message/disposition-notification")
            || h.to_ascii_lowercase().contains("message/delivery-status")
    });
    let returned = parsed
        .header_raw("Return-Path")
        .is_some_and(|h| h.trim() == "<>")
        || sender.starts_with("mailer-daemon@")
        || sender.starts_with("postmaster@");
    let automatic = auto
        || report
        || returned
        || parsed.header_raw("X-Autoreply").is_some()
        || parsed.header_raw("X-Autorespond").is_some();
    Ok(ReplyEvidence {
        source,
        message_id,
        references,
        sender,
        automatic,
        observed_at: at,
        received_at: None,
    })
}

impl Store {
    /// Previously parsed complete headers allow later scans to fetch only new immutable identities.
    pub fn indexed_reply_uids(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
        validity: u32,
    ) -> Result<std::collections::HashSet<u32>> {
        let c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let mut q=c.prepare("SELECT json_extract(record,'$.source.uid') FROM mail_reply_evidence WHERE account_id=?1 AND receive_epoch=?2 AND json_extract(record,'$.source.folder')=?3 AND json_extract(record,'$.source.uidValidity')=?4")?;
        Ok(
            q.query_map(params![account, epoch, folder, validity], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        )
    }
}
pub struct HeaderRound {
    pub roster: Vec<String>,
    pub coverage: Vec<FolderCoverage>,
}
fn scope_roster(folders: &[crate::imap::Folder]) -> Vec<String> {
    let mut out = folders
        .iter()
        .filter(|f| {
            !matches!(
                f.role.as_str(),
                "sent" | "drafts" | "junk" | "trash" | "outbox"
            )
        })
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();
    out.sort();
    out.dedup();
    out
}
async fn examine<T>(s: &mut async_imap::Session<T>, folder: &str) -> Result<u32>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let command = format!(
        "EXAMINE {}",
        crate::download_protocol::quoted_mailbox(folder)?
    );
    let responses = tokio::time::timeout(
        crate::imap::OP_TIMEOUT,
        crate::download_protocol::checked_command(s, &command),
    )
    .await
    .context("核对文件夹超时")??;
    responses
        .into_iter()
        .find_map(|r| {
            if let crate::download_protocol::ArchiveData::Validity(v) = r {
                Some(v)
            } else {
                None
            }
        })
        .filter(|v| *v > 0)
        .ok_or_else(|| anyhow!("文件夹代际尚未确认"))
}
/// Read-only real header scan with immutable UID snapshots, bounded batches and a final roster check.
pub async fn scan_reply_round<T, C, K, I, O>(
    s: &mut async_imap::Session<T>,
    account: &str,
    epoch: u64,
    cancel: C,
    known: K,
    mut inventory: I,
    mut observe: O,
) -> Result<HeaderRound>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
    C: Fn() -> bool,
    K: Fn(&str, u32) -> Result<std::collections::HashSet<u32>>,
    I: FnMut(&str, u32, &[u32]) -> Result<()>,
    O: FnMut(ReplyEvidence) -> Result<()>,
{
    use futures::TryStreamExt;
    if cancel() {
        bail!("核对已取消或邮箱身份已变化");
    }
    let folders = crate::imap::list_folders(s).await?;
    if !folders.iter().any(|f| f.role == "inbox") {
        bail!("未确认收件箱目录，核对范围不完整");
    }
    let roster = scope_roster(&folders);
    if roster.is_empty() {
        bail!("核对范围为空");
    }
    let mut coverage = vec![];
    for folder in &roster {
        if cancel() {
            bail!("核对已取消或邮箱身份已变化");
        }
        let validity = examine(s, folder).await?;
        let snapshot_at = now();
        let uids = crate::imap::all_uids(s).await?;
        let upper = uids.last().copied().unwrap_or(0);
        let indexed = known(folder, validity)?;
        let pending = uids
            .iter()
            .copied()
            .filter(|u| !indexed.contains(u))
            .collect::<Vec<_>>();
        for batch in pending.chunks(100) {
            if cancel() {
                bail!("核对已取消或邮箱身份已变化");
            }
            let fetches = tokio::time::timeout(crate::imap::OP_TIMEOUT * 3, async {
                s.uid_fetch(
                    crate::imap::uid_set(batch),
                    "UID INTERNALDATE BODY.PEEK[HEADER]",
                )
                .await?
                .try_collect::<Vec<_>>()
                .await
            })
            .await
            .context("核对邮件头超时")??;
            if cancel() {
                bail!("核对已取消或邮箱身份已变化");
            }
            let mut fetched = std::collections::HashSet::new();
            for fetch in &fetches {
                let Some(uid) = fetch.uid.filter(|u| batch.contains(u)) else {
                    continue;
                };
                let raw = fetch
                    .header()
                    .ok_or_else(|| anyhow!("核对返回缺失的邮件头"))?;
                if raw.len() > 256 * 1024 {
                    bail!("邮件头超过核对大小限制");
                }
                let source = MessageRef {
                    account_id: account.into(),
                    receive_epoch: epoch,
                    folder: folder.clone(),
                    uid_validity: validity,
                    uid,
                };
                let mut evidence = reply_from_headers(source, raw, now())?;
                evidence.received_at = fetch.internal_date().map(|d| d.timestamp_millis());
                observe(evidence)?;
                fetched.insert(uid);
            }
            if fetched.len() != batch.len() {
                bail!("核对期间邮件集合发生变化，请重新核对");
            }
        }
        if cancel() || examine(s, folder).await? != validity {
            bail!("核对期间邮箱代际发生变化");
        }
        inventory(folder, validity, &uids)?;
        // The snapshot time is deliberately conservative, before enumerating its UID upper bound.
        coverage.push(FolderCoverage {
            folder: folder.clone(),
            uid_validity: validity,
            scanned_uid: upper,
            upper_uid: upper,
            completed_at: snapshot_at,
            exhausted: true,
            error: None,
        });
    }
    let final_folders = crate::imap::list_folders(s).await?;
    if cancel() || scope_roster(&final_folders) != roster {
        bail!("核对期间目录集合发生变化，请重新核对");
    }
    for checked in &coverage {
        if cancel() || examine(s, &checked.folder).await? != checked.uid_validity {
            bail!("核对结束前文件夹代际发生变化，请重新核对");
        }
    }
    Ok(HeaderRound { roster, coverage })
}

impl Store {
    /// An authoritative folder snapshot invalidates removed or replaced source identities.
    pub fn reconcile_snooze_sources(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
        validity: u32,
        live: &[u32],
    ) -> Result<Vec<Reminder>> {
        let mut c = self.conn.lock().map_err(|_| anyhow!("提醒锁不可用"))?;
        let tx = c.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut changed = vec![];
        for mut r in all(&tx, Some(account))? {
            if !r.active() || r.kind != ReminderKind::Snooze || r.receive_epoch != epoch {
                continue;
            }
            let Some(source) = r.source.as_ref().filter(|s| s.folder == folder) else {
                continue;
            };
            if source.uid_validity != validity || !live.contains(&source.uid) {
                r.state = ReminderState::Orphaned;
                r.version += 1;
                r.error = Some(
                    if source.uid_validity != validity {
                        "原邮件文件夹已失效，请重新定位邮件"
                    } else {
                        "原邮件已删除或不可用"
                    }
                    .into(),
                );
                write_reminder(&tx, &r)?;
                changed.push(r);
            }
        }
        tx.commit()?;
        Ok(changed)
    }
}
