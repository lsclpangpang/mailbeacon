//! Durable pre-SMTP queue metadata. Execution state lives only in send_records.
use super::*;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BufferInput {
    pub request_id: String,
    pub payload_hash: String,
    pub compose_session_id: String,
    pub compose_revision: u64,
    pub deadline: i64,
    pub sender_fingerprint: String,
    pub receive_epoch: u64,
    pub followup_seconds: Option<u64>,
    pub request_receipt: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    #[serde(flatten)]
    pub input: BufferInput,
    pub draft_hash: String,
    pub restore_requested: bool,
}
/// A non-cloneable capability created only by the persisted claim transaction.
pub struct Claimed {
    pub(crate) record: Record,
    raw: Vec<u8>,
}

pub(super) fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mail_productivity_migrations(name TEXT PRIMARY KEY, version INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS send_buffer(send_id TEXT PRIMARY KEY REFERENCES send_records(id),account_id TEXT NOT NULL,request_id TEXT NOT NULL,metadata TEXT NOT NULL,UNIQUE(account_id,request_id));
    INSERT OR IGNORE INTO mail_productivity_migrations VALUES('send_buffer',1);")?;
    Ok(())
}
fn metadata(conn: &Connection, id: &str) -> Result<Option<Metadata>> {
    let raw: Option<String> = conn
        .query_row(
            "SELECT metadata FROM send_buffer WHERE send_id=?1",
            [id],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|v| serde_json::from_str(&v).map_err(Into::into))
        .transpose()
}
impl Store {
    /// Return an existing request only when its immutable logical payload agrees.
    pub fn buffered_request(
        &self,
        account: &str,
        request: &str,
        payload: &str,
    ) -> Result<Option<Record>> {
        let conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let id: Option<String> = conn
            .query_row(
                "SELECT send_id FROM send_buffer WHERE account_id=?1 AND request_id=?2",
                params![account, request],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = id {
            let m = metadata(&conn, &id)?.ok_or_else(|| anyhow!("发送元数据缺失"))?;
            if m.input.payload_hash != payload {
                bail!("同一发送请求的内容已经改变，请重新发送");
            }
            return read_record(&conn, &id);
        }
        Ok(None)
    }
    /// Publish immutable raw/draft files and queue metadata as one database transaction.
    pub fn enqueue_buffer(
        &self,
        input: NewRecord,
        raw: &[u8],
        draft: &[u8],
        meta: BufferInput,
    ) -> Result<Record> {
        valid_id(&input.id)?;
        valid_id(&meta.request_id)?;
        if input.recipients.is_empty()
            || meta.receive_epoch == 0
            || meta.sender_fingerprint.is_empty()
            || meta.compose_session_id.is_empty()
        {
            bail!("发送任务缺少身份或收件人");
        }
        if meta
            .followup_seconds
            .is_some_and(|s| !(60..=366 * 86400).contains(&s))
        {
            bail!("跟进间隔须为1分钟至366天");
        }
        let mut conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT send_id FROM send_buffer WHERE account_id=?1 AND request_id=?2",
                params![input.account_id, meta.request_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            let m = metadata(&tx, &id)?.ok_or_else(|| anyhow!("发送元数据缺失"))?;
            if m.input.payload_hash != meta.payload_hash
                || m.input.compose_session_id != meta.compose_session_id
                || m.input.compose_revision != meta.compose_revision
            {
                bail!("发送请求内容冲突");
            }
            return read_record(&tx, &id)?.ok_or_else(|| anyhow!("发送记录缺失"));
        }
        if read_record(&tx, &input.id)?.is_some() {
            bail!("发送标识已被使用");
        }
        let owned:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM send_buffer WHERE json_extract(metadata,'$.composeSessionId')=?1)",[&meta.compose_session_id],|r|r.get(0))?;
        if owned {
            bail!("此写信会话已经交给发送队列，请先检查发送记录");
        }
        // No committed record can reference these paths while this write transaction is held.
        // A prior failed publish may have left different MIME boundaries/Message-ID behind.
        for extension in ["eml", "draft"] {
            match std::fs::remove_file(self.dir.join(format!("{}.{}", input.id, extension))) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.persist_file(&input.id, "eml", raw)?;
        self.persist_file(&input.id, "draft", draft)?;
        let record = Record {
            id: input.id,
            account_id: input.account_id,
            from: input.from,
            subject: input.subject,
            recipients: input.recipients,
            message_id: input.message_id,
            created_at: now(),
            updated_at: now(),
            status: Status::Buffered,
            copy_status: CopyStatus::Pending,
            error: None,
            copy_error: None,
            endpoint: input.endpoint,
            scheduled_id: None,
            previous_id: None,
            raw_hash: hash(raw),
            version: 1,
            accepted_at: None,
            deadline: Some(meta.deadline),
        };
        tx.execute("INSERT INTO send_records(id,account_id,created_at,previous_id,record) VALUES(?1,?2,?3,NULL,?4)",params![record.id,record.account_id,record.created_at,serde_json::to_string(&record)?])?;
        let m = Metadata {
            input: meta,
            draft_hash: hash(draft),
            restore_requested: false,
        };
        tx.execute(
            "INSERT INTO send_buffer VALUES(?1,?2,?3,?4)",
            params![
                record.id,
                record.account_id,
                m.input.request_id,
                serde_json::to_string(&m)?
            ],
        )?;
        tx.commit()?;
        Ok(record)
    }
    /// Read only checked, immutable draft bytes; Bcc and receipt headers are preserved.
    pub fn buffer_draft(&self, id: &str) -> Result<Vec<u8>> {
        valid_id(id)?;
        let m = self
            .buffer_metadata(id)?
            .ok_or_else(|| anyhow!("缓冲任务不存在"))?;
        let raw = std::fs::read(self.dir.join(format!("{id}.draft")))?;
        if hash(&raw) != m.draft_hash {
            bail!("恢复草稿校验失败");
        }
        Ok(raw)
    }
    pub fn buffer_metadata(&self, id: &str) -> Result<Option<Metadata>> {
        let c = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        metadata(&c, id)
    }
    /// Cancel without acquiring any account/network lock. Recovery intent commits with it.
    pub fn cancel_buffered(&self, id: &str) -> Result<Record> {
        let mut conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut r = read_record(&tx, id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        if r.status == Status::Cancelled {
            return Ok(r);
        }
        if !matches!(r.status, Status::Buffered | Status::Paused) {
            bail!("已开始发送，无法撤销");
        }
        let mut m = metadata(&tx, id)?.ok_or_else(|| anyhow!("缓冲任务不存在"))?;
        r.status = Status::Cancelled;
        r.error = None;
        r.version += 1;
        r.updated_at = now();
        m.restore_requested = true;
        write_record(&tx, &r)?;
        tx.execute(
            "UPDATE send_buffer SET metadata=?2 WHERE send_id=?1",
            params![id, serde_json::to_string(&m)?],
        )?;
        tx.commit()?;
        Ok(r)
    }
    /// Gate submissions by state, persisted deadline/version and the current SMTP identity.
    pub fn claim_buffered(
        &self,
        id: &str,
        at: i64,
        force: bool,
        sender: &str,
        expected: Option<u64>,
    ) -> Result<Option<Claimed>> {
        let raw = self.raw(id)?;
        let mut conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let mut r = read_record(&tx, id)?.ok_or_else(|| anyhow!("发送记录不存在"))?;
        if !matches!(r.status, Status::Buffered | Status::Paused) {
            return Ok(None);
        }
        if r.status == Status::Paused && (!force || expected.is_none()) {
            return Ok(None);
        }
        if expected.is_some_and(|v| v != r.version) {
            bail!("发送状态已更新，请刷新");
        }
        let m = metadata(&tx, id)?.ok_or_else(|| anyhow!("缓冲元数据不存在"))?;
        if m.input.sender_fingerprint != sender {
            r.status = Status::Paused;
            r.error = Some("发件服务器或身份已变化，请恢复草稿后重新发送".into());
            r.version += 1;
            write_record(&tx, &r)?;
            tx.commit()?;
            bail!("发件身份已改变，发送已暂停");
        }
        if !force && at < m.input.deadline {
            return Ok(None);
        }
        r.status = Status::Submitting;
        r.error = None;
        r.updated_at = at;
        r.version += 1;
        write_record(&tx, &r)?;
        tx.commit()?;
        Ok(Some(Claimed { record: r, raw }))
    }
    pub fn pending_buffers(&self) -> Result<Vec<Record>> {
        Ok(self
            .list(None)?
            .into_iter()
            .filter(|r| r.status == Status::Buffered)
            .collect())
    }
    /// Account removal must never turn unsubmitted raw into a future submission.
    pub fn cancel_account_buffers(&self, account: &str) -> Result<()> {
        for r in self.list(Some(account))? {
            if matches!(r.status, Status::Buffered | Status::Paused) {
                self.cancel_buffered(&r.id)?;
            }
        }
        Ok(())
    }
}
/// Consume a claim once. Network failure and bookkeeping failure never cause automatic resend.
pub async fn execute_claimed<F, Fut, C>(
    store: &Store,
    claim: Claimed,
    clock: C,
    send: F,
) -> Result<Record>
where
    F: FnOnce(Vec<u8>) -> Fut,
    Fut: Future<Output = std::result::Result<(), SendError>>,
    C: FnOnce() -> i64,
{
    let current = store
        .get(&claim.record.id)?
        .ok_or_else(|| anyhow!("发送记录不存在"))?;
    if current.status != Status::Submitting || current.version != claim.record.version {
        bail!("发送领取已失效");
    }
    let result = send(claim.raw).await;
    let saved = store.finish_submission(&claim.record.id, result, clock());
    if saved.is_err() {
        let _ = store.update(&claim.record.id, |r| {
            r.status = Status::Unknown;
            r.error = Some("提交后本地记账失败，结果待确认；不会自动重发".into());
            Ok(())
        });
    }
    saved
}
impl Store {
    /// A retry is a new immutable submission with the original follow-up intent.
    /// The record and optional metadata publish together; concurrent clicks reuse one child.
    pub fn prepare_resend(
        &self,
        previous: &str,
        input: NewRecord,
        raw: &[u8],
        sender: &str,
        epoch: u64,
    ) -> Result<Record> {
        valid_id(&input.id)?;
        if input.recipients.is_empty() || epoch == 0 {
            bail!("重发身份或收件人无效");
        }
        let draft = if self.buffer_metadata(previous)?.is_some() {
            Some(self.buffer_draft(previous)?)
        } else {
            None
        };
        let mut conn = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM send_records WHERE previous_id=?1",
                [previous],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = existing {
            return read_record(&tx, &id)?.ok_or_else(|| anyhow!("重发记录缺失"));
        }
        let parent = read_record(&tx, previous)?.ok_or_else(|| anyhow!("原发送记录不存在"))?;
        if !matches!(
            parent.status,
            Status::Prepared | Status::Rejected | Status::Unknown
        ) || parent.account_id != input.account_id
            || parent.from != input.from
        {
            bail!("原发送状态或身份已改变");
        }
        let mut inherited = metadata(&tx, previous)?;
        self.persist_file(&input.id, "eml", raw)?;
        if let (Some(m), Some(draft)) = (inherited.as_mut(), draft.as_ref()) {
            self.persist_file(&input.id, "draft", draft)?;
            m.input.request_id = format!("resend-{}", input.id);
            m.input.sender_fingerprint = sender.to_string();
            m.input.receive_epoch = epoch;
            m.restore_requested = false;
        }
        let record = Record {
            id: input.id,
            account_id: input.account_id,
            from: input.from,
            subject: input.subject,
            recipients: input.recipients,
            message_id: input.message_id,
            endpoint: input.endpoint,
            scheduled_id: None,
            previous_id: Some(previous.into()),
            created_at: now(),
            updated_at: now(),
            status: Status::Prepared,
            copy_status: CopyStatus::Pending,
            error: None,
            copy_error: None,
            raw_hash: hash(raw),
            version: 1,
            accepted_at: None,
            deadline: None,
        };
        tx.execute("INSERT INTO send_records(id,account_id,created_at,previous_id,record) VALUES(?1,?2,?3,?4,?5)",params![record.id,record.account_id,record.created_at,record.previous_id,serde_json::to_string(&record)?])?;
        if let Some(m) = inherited {
            tx.execute(
                "INSERT INTO send_buffer VALUES(?1,?2,?3,?4)",
                params![
                    record.id,
                    record.account_id,
                    m.input.request_id,
                    serde_json::to_string(&m)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(record)
    }
    /// All published sessions stay owned, including cancellation (recovery uses its separate draft).
    pub fn owns_compose_session(&self, session: &str) -> Result<bool> {
        let c = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM send_buffer WHERE json_extract(metadata,'$.composeSessionId')=?1)",[session],|r|r.get(0))?)
    }
}

impl Store {
    pub fn active_sends(&self) -> Result<Vec<Record>> {
        let c = self.conn.lock().map_err(|_| anyhow!("发送记录锁不可用"))?;
        let mut q=c.prepare("SELECT record FROM send_records WHERE json_extract(record,'$.status') IN ('buffered','paused','submitting') ORDER BY created_at,id")?;
        q.query_map([], |row| row.get::<_, String>(0))?
            .map(|row| Ok(serde_json::from_str(&row?)?))
            .collect()
    }
}
