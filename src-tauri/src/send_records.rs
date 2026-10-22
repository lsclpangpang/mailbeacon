//! 持久发送记录：本地提交标识只防本机重复调用，不代表 SMTP 恰好发送一次。
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Status {
    Buffered,
    Paused,
    Cancelled,
    Prepared,
    Submitting,
    Accepted,
    Rejected,
    Unknown,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CopyStatus {
    Pending,
    Saved,
    Failed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub account_id: String,
    pub from: String,
    pub subject: String,
    pub recipients: Vec<String>,
    pub message_id: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub status: Status,
    pub copy_status: CopyStatus,
    pub error: Option<String>,
    pub copy_error: Option<String>,
    pub endpoint: String,
    pub scheduled_id: Option<u32>,
    pub previous_id: Option<String>,
    pub raw_hash: String,
    #[serde(default)]
    pub version: u64,
    #[serde(default)]
    pub accepted_at: Option<i64>,
    #[serde(default)]
    pub deadline: Option<i64>,
}
/// 调用方先组好实际 SMTP 原文，再将原文与信封一起持久化。
pub struct NewRecord {
    pub id: String,
    pub account_id: String,
    pub from: String,
    pub subject: String,
    pub recipients: Vec<String>,
    pub message_id: String,
    pub endpoint: String,
    pub scheduled_id: Option<u32>,
    pub previous_id: Option<String>,
}
pub struct Store {
    conn: Mutex<Connection>,
    dir: PathBuf,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn valid_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 160
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        bail!("无效的发送记录标识");
    }
    Ok(())
}
fn hash(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
/// 使用系统随机数生成本机提交标识。
pub fn new_id() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow!("无法生成发送标识"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn read_record(conn: &Connection, id: &str) -> Result<Option<Record>> {
    let json: Option<String> = conn
        .query_row("SELECT record FROM send_records WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .optional()?;
    json.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
fn write_record(conn: &Connection, record: &Record) -> Result<()> {
    conn.execute(
        "UPDATE send_records SET record=?2 WHERE id=?1",
        params![record.id, serde_json::to_string(record)?],
    )?;
    Ok(())
}
impl Store {
    fn persist_file(&self, id: &str, extension: &str, raw: &[u8]) -> Result<()> {
        valid_id(id)?;
        let path = self.dir.join(format!("{id}.{extension}"));
        if path.exists() {
            if std::fs::read(&path)? != raw {
                bail!("持久原文已存在且内容不同");
            }
            return Ok(());
        }
        let mut file = tempfile::NamedTempFile::new_in(&self.dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(raw)?;
        file.as_file().sync_all()?;
        file.persist_noclobber(path).map_err(|e| e.error)?;
        #[cfg(unix)]
        std::fs::File::open(&self.dir)?.sync_all()?;
        Ok(())
    }
    /// Commit SMTP outcome and follow-up activation atomically; never infer acceptance.
    pub(crate) fn finish_submission(
        &self,
        id: &str,
        outcome: std::result::Result<(), SendError>,
        at: i64,
    ) -> Result<Record> {
        let mut conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut record = read_record(&tx, id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        if record.status != Status::Submitting {
            bail!("提交状态已经变化");
        }
        match outcome {
            Ok(()) => {
                record.status = Status::Accepted;
                record.accepted_at = Some(at);
                record.error = None;
            }
            Err(SendError::Rejected(e)) => {
                record.status = Status::Rejected;
                record.error = Some(e);
            }
            Err(SendError::Unknown(e)) => {
                record.status = Status::Unknown;
                record.error = Some(e);
            }
        }
        record.updated_at = at;
        record.version += 1;
        write_record(&tx, &record)?;
        reminders::activate_followup(&tx, &record)?;
        tx.commit()?;
        Ok(record)
    }
    /// 与联系人共用 local.db，但不接管其 user_version。仅在启动时恢复中断状态。
    pub fn open(config: &Path) -> Result<Self> {
        std::fs::create_dir_all(config)?;
        let dir = config.join("send-records");
        std::fs::create_dir_all(&dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let mut conn = Connection::open(config.join("local.db"))?;
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        conn.execute_batch("CREATE TABLE IF NOT EXISTS send_records (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, created_at INTEGER NOT NULL, previous_id TEXT UNIQUE, record TEXT NOT NULL); CREATE INDEX IF NOT EXISTS send_records_account ON send_records(account_id, created_at DESC);")?;
        {
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            buffer::init(&tx)?;
            reminders::init(&tx)?;
            tx.commit()?;
        }
        let store = Self {
            conn: Mutex::new(conn),
            dir,
        };
        for record in store.list(None)? {
            if record.status == Status::Buffered {
                store.update(&record.id, |r| {
                    r.status = Status::Paused;
                    r.error = Some("上次退出时尚未提交，请确认发送或恢复草稿".into());
                    Ok(())
                })?;
            } else if record.status == Status::Submitting {
                store.update(&record.id, |r| {
                    r.status = Status::Unknown;
                    r.error = Some(
                        "上次提交中断，邮件可能已经发出；不会自动重发。请先检查已发送记录。".into(),
                    );
                    Ok(())
                })?;
            }
        }
        store.reconcile_followups()?;
        Ok(store)
    }
    /// 先原子落盘并同步原文，再提交记录；失败时不允许开始 SMTP。
    pub fn prepare(&self, input: NewRecord, raw: &[u8]) -> Result<Record> {
        self.prepare_with_state(input, raw, Status::Prepared, None)
    }
    /// 导入旧版中断提交时，第一次落库就必须保持未知，不能留下可提交的中间态。
    pub fn import_unknown(&self, input: NewRecord, raw: &[u8], reason: String) -> Result<Record> {
        self.prepare_with_state(input, raw, Status::Unknown, Some(reason))
    }
    fn prepare_with_state(
        &self,
        input: NewRecord,
        raw: &[u8],
        status: Status,
        error: Option<String>,
    ) -> Result<Record> {
        valid_id(&input.id)?;
        if input.recipients.is_empty() {
            bail!("没有收件人");
        }
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let raw_hash = hash(raw);
        if let Some(existing) = read_record(&conn, &input.id)? {
            if existing.raw_hash != raw_hash
                || existing.account_id != input.account_id
                || existing.recipients != input.recipients
            {
                bail!("提交标识已用于另一封邮件");
            }
            return Ok(existing);
        }
        self.persist_file(&input.id, "eml", raw)?;
        let record = Record {
            id: input.id,
            account_id: input.account_id,
            from: input.from,
            subject: input.subject,
            recipients: input.recipients,
            message_id: input.message_id,
            endpoint: input.endpoint,
            scheduled_id: input.scheduled_id,
            previous_id: input.previous_id,
            created_at: now(),
            updated_at: now(),
            status,
            copy_status: CopyStatus::Pending,
            error,
            copy_error: None,
            raw_hash,
            version: 1,
            accepted_at: None,
            deadline: None,
        };
        conn.execute("INSERT INTO send_records(id,account_id,created_at,previous_id,record) VALUES(?1,?2,?3,?4,?5)", params![record.id, record.account_id, record.created_at, record.previous_id, serde_json::to_string(&record)?])?;
        Ok(record)
    }
    pub fn get(&self, id: &str) -> Result<Option<Record>> {
        valid_id(id)?;
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        read_record(&conn, id)
    }
    pub fn list(&self, account: Option<&str>) -> Result<Vec<Record>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let mut stmt = conn.prepare("SELECT record FROM send_records WHERE (?1 IS NULL OR account_id=?1) ORDER BY created_at DESC,id DESC")?;
        let rows = stmt.query_map([account], |r| r.get::<_, String>(0))?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    pub fn raw(&self, id: &str) -> Result<Vec<u8>> {
        let record = self.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        let raw =
            std::fs::read(self.dir.join(format!("{id}.eml"))).context("本地发送原文无法读取")?;
        if hash(&raw) != record.raw_hash {
            bail!("发送原文校验失败，已停止发送或补存");
        }
        Ok(raw)
    }
    pub fn update(
        &self,
        id: &str,
        change: impl FnOnce(&mut Record) -> Result<()>,
    ) -> Result<Record> {
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let mut record = read_record(&conn, id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        change(&mut record)?;
        record.updated_at = now();
        record.version += 1;
        write_record(&conn, &record)?;
        Ok(record)
    }
    /// 只有 prepared 能进入网络提交，终态及运行中的重复请求均不会再次发送。
    pub fn begin(&self, id: &str) -> Result<bool> {
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let mut record = read_record(&conn, id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        if record.status != Status::Prepared {
            return Ok(false);
        }
        let replacement: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM send_records WHERE previous_id=?1)",
            [id],
            |r| r.get(0),
        )?;
        if replacement {
            record.status = Status::Rejected;
            record.error = Some("此提交已由手动重新发送的记录接管，不会重复发送".into());
        } else {
            record.status = Status::Submitting;
        }
        record.updated_at = now();
        record.version += 1;
        write_record(&conn, &record)?;
        Ok(!replacement)
    }
    pub fn pending_copies(&self) -> Result<Vec<Record>> {
        Ok(self
            .list(None)?
            .into_iter()
            .filter(|r| r.status == Status::Accepted && r.copy_status != CopyStatus::Saved)
            .collect())
    }
    pub fn copy_result(&self, id: &str, result: std::result::Result<(), String>) -> Result<Record> {
        self.update(id, |r| {
            if r.status != Status::Accepted {
                bail!("只有服务器确认接受的邮件可以补存");
            }
            match result {
                Ok(()) => {
                    r.copy_status = CopyStatus::Saved;
                    r.copy_error = None;
                }
                Err(e) => {
                    r.copy_status = CopyStatus::Failed;
                    r.copy_error = Some(e);
                }
            }
            Ok(())
        })
    }
}
/// 明确未接受才是 Rejected；传输中断等不确定错误必须保留 Unknown。
pub enum SendError {
    Rejected(String),
    Unknown(String),
}
/// 原子领取本机提交；网络调用仅发生在原文完整、提交状态已持久化之后。
pub async fn execute<F, Fut>(store: &Store, id: &str, send: F) -> Result<Record>
where
    F: FnOnce(Vec<u8>) -> Fut,
    Fut: Future<Output = std::result::Result<(), SendError>>,
{
    let raw = store.raw(id)?;
    if !store.begin(id)? {
        return store.get(id)?.ok_or_else(|| anyhow!("发送记录不存在"));
    }
    let outcome = send(raw).await;
    let saved = store.finish_submission(id, outcome, now());
    if saved.is_err() {
        // 最终记账失败也不得重新发送；即使此写入失败，重启会将 submitting 恢复为 unknown。
        let _ = store.update(id, |r| {
            r.status = Status::Unknown;
            r.error = Some("提交后的本地记录保存失败，结果需要确认；不会自动重发。".into());
            Ok(())
        });
    }
    saved
}

#[path = "send_buffer.rs"]
pub(crate) mod buffer;
#[cfg(test)]
#[path = "send_records/buffer_tests.rs"]
mod buffer_tests;
#[cfg(test)]
#[path = "send_records/reminder_tests.rs"]
mod reminder_tests;
#[path = "mail_reminders.rs"]
pub(crate) mod reminders;
#[cfg(test)]
mod tests;
