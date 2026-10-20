//! 本地邮件搜索索引。
//!
//! 腾讯企业邮 / QQ 的 IMAP SEARCH 会忽略所有文本条件、直接返回整个文件夹
//! （实测：用一个肯定不存在的关键词也能拿到收件箱全部 UID）；Coremail 虽然
//! 支持 `CHARSET UTF-8`，但一次全文搜索要几十秒。所以文本搜索统一改成：
//! 后台把邮件摘要和正文开头增量落进本地 SQLite，搜索只查本地库。
//!
//! 索引分两阶段，因为腾讯服务器对每封邮件都有固定延迟（实测摘要 ~0.15s/封、
//! 正文 ~0.7s/封），大邮箱第一次要跑不少时间：
//! 1. 摘要阶段：主题/发件人/收件人，按 UID 从新到旧，建完就能按主题、发件人搜；
//! 2. 正文阶段：补每封开头 16KB 的正文，搜索才覆盖正文里的关键词。
//! 进度会通过事件推给界面；两阶段都按 UID 从新到旧，最近常搜的邮件先可用。

use std::{collections::HashSet, fmt, path::Path, sync::Mutex, time::Duration};

use anyhow::{Context, Result};
use async_imap::Session;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

use crate::imap::{self, OP_TIMEOUT, Preview, Summary};

/// 本地索引表。和通讯录共用 local.db，但用自己的表、不改 PRAGMA user_version。
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS mail_index (
    account_id TEXT NOT NULL,
    folder TEXT NOT NULL,
    uid INTEGER NOT NULL,
    uid_validity INTEGER NOT NULL DEFAULT 0,
    date INTEGER NOT NULL DEFAULT 0,
    summary TEXT NOT NULL,
    haystack TEXT NOT NULL DEFAULT '',
    has_body INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, folder, uid)
);
CREATE INDEX IF NOT EXISTS mail_index_folder ON mail_index(account_id, folder);
CREATE TABLE IF NOT EXISTS mail_index_meta (
    account_id TEXT NOT NULL,
    folder TEXT NOT NULL,
    uid_validity INTEGER NOT NULL DEFAULT 0,
    complete INTEGER NOT NULL DEFAULT 0,
    indexed INTEGER NOT NULL DEFAULT 0,
    total INTEGER NOT NULL DEFAULT 0,
    body_indexed INTEGER NOT NULL DEFAULT 0,
    updated_at INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (account_id, folder)
);
"#;

/// 摘要阶段每批多少封：只取邮件头，一条命令别太大。
const BATCH_HEADERS: usize = 40;
/// 正文阶段每批多少封：每封开头 16KB，服务器每封约 0.7 秒。
const BATCH_BODIES: usize = 20;
/// 一次搜索最多返回多少封，避免关键词太宽时把界面卡住。
const SEARCH_LIMIT: usize = 2000;

/// 建索引的进度，会随 `searchIndex` 事件推给界面，也作为 search_messages 的返回值。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub running: bool,
    pub complete: bool,
    /// 已完成摘要索引的数量（主题/发件人/收件人可用）
    pub indexed: u32,
    pub total: u32,
    /// 已补正文开头的数量（正文关键词可用）
    pub body_indexed: u32,
    pub body_total: u32,
    pub error: Option<String>,
}

pub struct SearchIndex {
    conn: Mutex<Connection>,
}

impl SearchIndex {
    /// 打开（或创建）本地索引库。与通讯录共用一个 local.db。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("打开本地数据库 {} 失败", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(SCHEMA).context("创建搜索索引表失败")?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    /// 本地库里记着的进度（没有索引过就是默认值）。
    pub fn progress(&self, account_id: &str, folder: &str) -> Result<Progress> {
        let meta = self.folder_meta(account_id, folder)?;
        let (indexed, body_indexed) = self.stats(account_id, folder)?;
        Ok(Progress {
            running: false,
            complete: meta
                .as_ref()
                .map(|(_, complete, ..)| *complete)
                .unwrap_or(false),
            indexed,
            total: meta
                .as_ref()
                .map(|(_, _, total, _)| *total)
                .unwrap_or(indexed),
            body_indexed,
            body_total: indexed,
            error: None,
        })
    }

    /// (UIDVALIDITY, 是否完整, 上次枚举到的总数, 上次记录的摘要数)
    fn folder_meta(&self, account_id: &str, folder: &str) -> Result<Option<(u32, bool, u32, u32)>> {
        let conn = self.lock();
        Ok(conn
            .query_row(
                "SELECT uid_validity, complete, total, indexed FROM mail_index_meta
                 WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
                |row| {
                    Ok((
                        row.get::<_, u32>(0)?,
                        row.get::<_, i64>(1)? != 0,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u32>(3)?,
                    ))
                },
            )
            .optional()?)
    }

    /// (已索引摘要数, 已补正文数)
    fn stats(&self, account_id: &str, folder: &str) -> Result<(u32, u32)> {
        let conn = self.lock();
        Ok(conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(has_body), 0) FROM mail_index
             WHERE account_id = ?1 AND folder = ?2",
            params![account_id, folder],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    }

    fn save_meta(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        complete: bool,
        indexed: u32,
        total: u32,
        body_indexed: u32,
    ) -> Result<()> {
        self.lock().execute(
            "INSERT INTO mail_index_meta
                 (account_id, folder, uid_validity, complete, indexed, total, body_indexed, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(account_id, folder) DO UPDATE SET
                 uid_validity = excluded.uid_validity,
                 complete = excluded.complete,
                 indexed = excluded.indexed,
                 total = excluded.total,
                 body_indexed = excluded.body_indexed,
                 updated_at = excluded.updated_at",
            params![
                account_id,
                folder,
                uid_validity,
                i64::from(complete),
                indexed,
                total,
                body_indexed,
                chrono::Utc::now().timestamp_millis()
            ],
        )?;
        Ok(())
    }

    /// 开始处理一个文件夹：UIDVALIDITY 变了就清空旧索引，并返回已有进度。
    fn begin_folder(&self, account_id: &str, folder: &str, uid_validity: u32) -> Result<Progress> {
        let existing = self.folder_meta(account_id, folder)?;
        // 服务器没报 UIDVALIDITY（0）时无法判断是否重建，保留已有索引
        if uid_validity != 0 && existing.is_some_and(|(stored, ..)| stored != uid_validity) {
            self.clear_folder(account_id, folder)?;
        }
        if self.folder_meta(account_id, folder)?.is_none() {
            self.save_meta(account_id, folder, uid_validity, false, 0, 0, 0)?;
        }
        self.progress(account_id, folder)
    }

    fn clear_folder(&self, account_id: &str, folder: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM mail_index WHERE account_id = ?1 AND folder = ?2",
            params![account_id, folder],
        )?;
        conn.execute(
            "DELETE FROM mail_index_meta WHERE account_id = ?1 AND folder = ?2",
            params![account_id, folder],
        )?;
        Ok(())
    }

    fn stored_uids(&self, account_id: &str, folder: &str) -> Result<Vec<u32>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT uid FROM mail_index WHERE account_id = ?1 AND folder = ?2 ORDER BY uid",
        )?;
        let rows = stmt.query_map(params![account_id, folder], |row| row.get::<_, u32>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// 还没补正文的 UID，从新到旧（最近的邮件先补）。
    fn uids_without_body(&self, account_id: &str, folder: &str) -> Result<Vec<u32>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT uid FROM mail_index
             WHERE account_id = ?1 AND folder = ?2 AND has_body = 0
             ORDER BY uid DESC",
        )?;
        let rows = stmt.query_map(params![account_id, folder], |row| row.get::<_, u32>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn remove_uids(&self, account_id: &str, folder: &str, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "DELETE FROM mail_index WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for uid in uids {
                stmt.execute(params![account_id, folder, uid])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    fn count(&self, account_id: &str, folder: &str) -> Result<u32> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM mail_index WHERE account_id = ?1 AND folder = ?2",
            params![account_id, folder],
            |row| row.get(0),
        )?)
    }

    /// 摘要阶段：写入邮件头（正文留空，等正文阶段补）。
    fn insert_summaries(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        summaries: &[Summary],
    ) -> Result<()> {
        if summaries.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO mail_index
                     (account_id, folder, uid, uid_validity, date, summary, haystack, has_body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
                 ON CONFLICT(account_id, folder, uid) DO UPDATE SET
                     uid_validity = excluded.uid_validity,
                     date = excluded.date,
                     summary = excluded.summary,
                     haystack = excluded.haystack,
                     has_body = excluded.has_body",
            )?;
            for summary in summaries {
                stmt.execute(params![
                    account_id,
                    folder,
                    summary.uid,
                    uid_validity,
                    summary.date,
                    serde_json::to_string(summary)?,
                    haystack(summary, ""),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 正文阶段：给已经建过摘要的邮件补正文开头，并更新可搜索文本。
    fn update_bodies(&self, account_id: &str, folder: &str, texts: &[(u32, String)]) -> Result<()> {
        if texts.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut read = tx.prepare(
                "SELECT summary FROM mail_index
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            let mut write = tx.prepare(
                "UPDATE mail_index SET haystack = ?4, has_body = 1
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for (uid, body) in texts {
                let summary: Option<String> = read
                    .query_row(params![account_id, folder, uid], |row| row.get(0))
                    .optional()?;
                let Some(json) = summary else { continue };
                let Ok(summary) = serde_json::from_str::<Summary>(&json) else {
                    continue;
                };
                write.execute(params![account_id, folder, uid, haystack(&summary, body)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 测试用：直接写一整条记录（含正文）。
    #[cfg(test)]
    fn upsert(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        rows: &[(Summary, String)],
    ) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO mail_index
                     (account_id, folder, uid, uid_validity, date, summary, haystack, has_body)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(account_id, folder, uid) DO UPDATE SET
                     uid_validity = excluded.uid_validity,
                     date = excluded.date,
                     summary = excluded.summary,
                     haystack = excluded.haystack,
                     has_body = excluded.has_body",
            )?;
            for (summary, body) in rows {
                stmt.execute(params![
                    account_id,
                    folder,
                    summary.uid,
                    uid_validity,
                    summary.date,
                    serde_json::to_string(summary)?,
                    haystack(summary, body),
                    i64::from(!body.trim().is_empty()),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 在当前账号的文件夹里搜索主题、发件人、收件人和正文开头。
    pub fn search(&self, account_id: &str, folder: &str, query: &str) -> Result<Vec<Summary>> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT summary FROM mail_index
             WHERE account_id = ?1 AND folder = ?2 AND instr(haystack, ?3) > 0
             ORDER BY date DESC, uid DESC LIMIT ?4",
        )?;
        let rows = stmt.query_map(
            params![account_id, folder, needle, SEARCH_LIMIT as i64],
            |row| row.get::<_, String>(0),
        )?;
        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            // 索引内容损坏时跳过这一封，不影响其他结果
            if let Ok(summary) = serde_json::from_str::<Summary>(&json) {
                out.push(summary);
            }
        }
        Ok(out)
    }

    /// 界面删账号时清掉该账号的索引，避免本地库越积越大。
    pub fn remove_account(&self, account_id: &str) -> Result<()> {
        let conn = self.lock();
        conn.execute(
            "DELETE FROM mail_index WHERE account_id = ?1",
            params![account_id],
        )?;
        conn.execute(
            "DELETE FROM mail_index_meta WHERE account_id = ?1",
            params![account_id],
        )?;
        Ok(())
    }
}

/// 从两端往中间取：最新和最旧的邮件都先索引。
///
/// 用户既会搜最近的邮件，也会翻很早的旧邮件；只从新到旧会让最旧的那批
/// 排到最后，只从旧到新又会让最近的排到最后，交替取可以把两端都提前。
fn interleave_from_ends(uids: &[u32]) -> Vec<u32> {
    let mut out = Vec::with_capacity(uids.len());
    let (mut front, mut back) = (0, uids.len());
    while front < back {
        out.push(uids[front]);
        front += 1;
        if front < back {
            back -= 1;
            out.push(uids[back]);
        }
    }
    out
}

/// 拼一个用于子串匹配的小写文本：主题、发件人、收件人、正文开头。
fn haystack(summary: &Summary, body: &str) -> String {
    let mut text = String::with_capacity(body.len() + 256);
    for part in [
        summary.subject.as_str(),
        summary.from_name.as_str(),
        summary.from_addr.as_str(),
        summary.to.as_str(),
    ] {
        text.push_str(part);
        text.push('\n');
    }
    for addr in &summary.recipients {
        text.push_str(addr);
        text.push('\n');
    }
    text.push_str(body);
    text.to_lowercase()
}

/// 增量索引一个文件夹：先补摘要（主题/发件人），再补正文开头。
///
/// 用独立连接（调用方自己登录），不占用界面操作的命令连接；`report` 每完成
/// 一批就回调一次，用来更新进度和推送 `searchIndex` 事件。中断后重开会接着
/// 已索引的部分继续，只补缺的。
pub async fn index_folder<T, F>(
    session: &mut Session<T>,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
    mut report: F,
) -> Result<Progress>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
    F: FnMut(&Progress),
{
    let mailbox = timeout(OP_TIMEOUT, session.select(folder))
        .await
        .context("打开文件夹超时")?
        .with_context(|| format!("打开文件夹 {} 失败", imap::decode_mutf7(folder)))?;
    let uid_validity = mailbox.uid_validity.unwrap_or(0);

    let mut progress = index.begin_folder(account_id, folder, uid_validity)?;
    progress.running = true;
    progress.complete = false;
    progress.error = None;
    report(&progress);

    let all = imap::all_uids(session).await?;
    let all_set: HashSet<u32> = all.iter().copied().collect();
    let stored_set: HashSet<u32> = index.stored_uids(account_id, folder)?.into_iter().collect();

    let stale: Vec<u32> = stored_set.difference(&all_set).copied().collect();
    index.remove_uids(account_id, folder, &stale)?;

    // 阶段一：摘要。从最新和最旧两端交替，两端都能较快搜到。
    let mut missing: Vec<u32> = all_set.difference(&stored_set).copied().collect();
    missing.sort_unstable_by(|a, b| b.cmp(a));
    let missing = interleave_from_ends(&missing);

    progress.total = all.len() as u32;
    progress.indexed = index.count(account_id, folder)?;
    index.save_meta(
        account_id,
        folder,
        uid_validity,
        false,
        progress.indexed,
        progress.total,
        progress.body_indexed,
    )?;
    report(&progress);

    for chunk in missing.chunks(BATCH_HEADERS) {
        let set = imap::uid_set(chunk);
        let summaries = imap::fetch_summaries(session, &set, account_id, folder).await?;
        index.insert_summaries(account_id, folder, uid_validity, &summaries)?;
        progress.indexed = index.count(account_id, folder)?;
        index.save_meta(
            account_id,
            folder,
            uid_validity,
            false,
            progress.indexed,
            progress.total,
            progress.body_indexed,
        )?;
        report(&progress);
    }

    // 阶段二：正文开头。中断过或上次没补完的都在这里继续。
    for chunk in index
        .uids_without_body(account_id, folder)?
        .chunks(BATCH_BODIES)
    {
        let set = imap::uid_set(chunk);
        let previews: Vec<Preview> = imap::fetch_body_texts(session, &set).await?;
        let texts: Vec<(u32, String)> = previews
            .into_iter()
            .map(|Preview { uid, text }| (uid, text))
            .collect();
        index.update_bodies(account_id, folder, &texts)?;
        progress.body_indexed = index.stats(account_id, folder)?.1;
        index.save_meta(
            account_id,
            folder,
            uid_validity,
            false,
            progress.indexed,
            progress.total,
            progress.body_indexed,
        )?;
        report(&progress);
    }

    progress.running = false;
    progress.complete = true;
    progress.indexed = index.count(account_id, folder)?;
    progress.body_indexed = index.stats(account_id, folder)?.1;
    progress.body_total = progress.indexed;
    index.save_meta(
        account_id,
        folder,
        uid_validity,
        true,
        progress.indexed,
        progress.total,
        progress.body_indexed,
    )?;
    report(&progress);
    Ok(progress)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uid: u32, subject: &str, from_name: &str, from_addr: &str, to: &str) -> Summary {
        Summary {
            account_id: "acct".into(),
            folder: "INBOX".into(),
            uid,
            from_name: from_name.into(),
            from_addr: from_addr.into(),
            to: to.into(),
            subject: subject.into(),
            date: 1_700_000_000_000 + i64::from(uid),
            seen: false,
            flagged: false,
            attachment: false,
            recipients: vec![to.to_lowercase()],
            message_id: format!("<{uid}@example.com>"),
            in_reply_to: String::new(),
            references: Vec::new(),
        }
    }

    fn row(uid: u32, subject: &str, body: &str) -> (Summary, String) {
        (
            summary(uid, subject, "张三", "zhang@example.com", "me@example.com"),
            body.into(),
        )
    }

    #[test]
    fn search_matches_subject_sender_recipient_and_body() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert(
                "acct",
                "INBOX",
                1,
                &[
                    row(1, "2025年报价单", "请查收正文里的合同编号 A-123"),
                    row(2, "Weekly Report", "build passed"),
                ],
            )
            .unwrap();
        let uid = |result: Vec<Summary>| result.into_iter().map(|m| m.uid).collect::<Vec<_>>();

        assert_eq!(uid(index.search("acct", "INBOX", "报价单").unwrap()), [1]);
        assert_eq!(uid(index.search("acct", "INBOX", "合同编号").unwrap()), [1]);
        let mut by_name = uid(index.search("acct", "INBOX", "张三").unwrap());
        by_name.sort_unstable();
        assert_eq!(by_name, [1, 2]);
        // ASCII 不区分大小写；收件人也能搜到
        assert_eq!(uid(index.search("acct", "INBOX", "WEEKLY").unwrap()), [2]);
        let mut by_recipient = uid(index.search("acct", "INBOX", "ME@EXAMPLE").unwrap());
        by_recipient.sort_unstable();
        assert_eq!(by_recipient, [1, 2]);
        assert!(
            index
                .search("acct", "INBOX", "不存在的词")
                .unwrap()
                .is_empty()
        );
        // 不同文件夹 / 不同账号不串结果
        assert!(index.search("acct", "Sent", "报价单").unwrap().is_empty());
        assert!(index.search("other", "INBOX", "报价单").unwrap().is_empty());
    }

    #[test]
    fn index_order_takes_both_newest_and_oldest_first() {
        assert_eq!(interleave_from_ends(&[9, 8, 7, 6, 5]), [9, 5, 8, 6, 7]);
        assert_eq!(interleave_from_ends(&[3, 2, 1]), [3, 1, 2]);
        assert_eq!(interleave_from_ends(&[1]), [1]);
        assert!(interleave_from_ends(&[]).is_empty());
    }

    #[test]
    fn uid_validity_change_clears_folder() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert("acct", "INBOX", 1, &[row(1, "报价单", "正文")])
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 1, 1, 1).unwrap();
        assert_eq!(index.progress("acct", "INBOX").unwrap().indexed, 1);

        let progress = index.begin_folder("acct", "INBOX", 2).unwrap();
        assert!(!progress.complete);
        assert_eq!(progress.indexed, 0);
        assert!(index.search("acct", "INBOX", "报价单").unwrap().is_empty());
    }

    fn utf8_mail(subject_b64: &str, body_b64: &str) -> String {
        format!(
            "From: =?UTF-8?B?5byg5LiJ?= <zhang@example.com>\r\n\
             To: me@example.com\r\n\
             Subject: =?UTF-8?B?{subject_b64}?=\r\n\
             Date: Mon, 20 Oct 2025 10:00:00 +0800\r\n\
             MIME-Version: 1.0\r\n\
             Content-Type: text/plain; charset=UTF-8\r\n\
             Content-Transfer-Encoding: base64\r\n\r\n\
             {body_b64}\r\n"
        )
    }

    fn gbk_mail() -> String {
        // 主题"报价单"、正文"发票已开好，请查收"都用 GBK 编码，验证解码后才建索引
        "From: =?GBK?B?sai827Wl?= <billing@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: =?GBK?B?sai827Wl?=\r\n\
         Date: Tue, 21 Oct 2025 10:00:00 +0800\r\n\
         MIME-Version: 1.0\r\n\
         Content-Type: text/plain; charset=GBK\r\n\
         Content-Transfer-Encoding: quoted-printable\r\n\r\n\
         =B7=A2=C6=B1=D2=D1=BF=AA=BA=C3=A3=AC=C7=EB=B2=E9=CA=D5\r\n"
            .to_string()
    }

    fn fetch_count(server: &crate::test_imap::Shared) -> usize {
        server
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| command.starts_with("UID FETCH"))
            .count()
    }

    async fn index_test_folder(server: &crate::test_imap::Shared, index: &SearchIndex) -> Progress {
        let mut session = crate::test_imap::session(server.clone()).await;
        index_folder(&mut session, index, "acct", "INBOX", |_| {})
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn index_folder_decodes_headers_and_bodies_and_is_incremental() {
        let server = crate::test_imap::state("", &[("INBOX", 1)]);
        server.lock().unwrap().add(
            "INBOX",
            &utf8_mail(
                "MjAyNeW5tOaKpeS7t+WNlQ==",
                "6K+35p+l5pS26ZmE5Lu26YeM55qE5ZCI5ZCM77yM57yW5Y+3IEEtMTIz44CC",
            ),
            &[],
        );
        server.lock().unwrap().add("INBOX", &gbk_mail(), &[]);

        let index = SearchIndex::in_memory().unwrap();
        let progress = index_test_folder(&server, &index).await;
        assert!(progress.complete);
        assert_eq!(progress.indexed, 2);
        assert_eq!(progress.total, 2);
        assert_eq!(progress.body_indexed, 2);
        assert_eq!(progress.body_total, 2);

        // base64 主题、GBK 主题、GBK QP 正文都解码后进了索引
        let subjects: Vec<String> = index
            .search("acct", "INBOX", "报价单")
            .unwrap()
            .into_iter()
            .map(|m| m.subject)
            .collect();
        assert_eq!(subjects.len(), 2);
        assert!(subjects.iter().any(|s| s.contains("2025年报价单")));
        assert!(subjects.iter().any(|s| s == "报价单"));
        assert_eq!(index.search("acct", "INBOX", "合同").unwrap().len(), 1);
        assert_eq!(
            index.search("acct", "INBOX", "发票已开好").unwrap().len(),
            1
        );
        assert_eq!(index.search("acct", "INBOX", "张三").unwrap().len(), 1);

        // 再跑一次：没有新邮件就不该再 FETCH
        let before = fetch_count(&server);
        let progress = index_test_folder(&server, &index).await;
        assert!(progress.complete);
        assert_eq!(progress.indexed, 2);
        assert_eq!(progress.body_indexed, 2);
        assert_eq!(fetch_count(&server), before, "增量索引不应重拉已索引邮件");
        assert_eq!(index.search("acct", "INBOX", "报价单").unwrap().len(), 2);

        // 新到一封：只补这一封的摘要和正文
        server
            .lock()
            .unwrap()
            .add("INBOX", &utf8_mail("5paw5Lq66YKu5Lu2", "5L2g5aW9"), &[]);
        let progress = index_test_folder(&server, &index).await;
        assert!(progress.complete);
        assert_eq!(progress.indexed, 3);
        assert_eq!(progress.body_indexed, 3);
        assert_eq!(index.search("acct", "INBOX", "新人邮件").unwrap().len(), 1);
        assert_eq!(index.search("acct", "INBOX", "你好").unwrap().len(), 1);
    }

    #[tokio::test]
    async fn index_folder_drops_messages_deleted_on_server() {
        let server = crate::test_imap::state("", &[("INBOX", 7)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", &utf8_mail("5oql5Lu35Y2V", "5q2j5paH"), &[]);
        let index = SearchIndex::in_memory().unwrap();
        index_test_folder(&server, &index).await;
        assert_eq!(index.search("acct", "INBOX", "报价单").unwrap().len(), 1);

        server.lock().unwrap().mailbox("INBOX").messages.clear();
        let progress = index_test_folder(&server, &index).await;
        assert!(progress.complete);
        assert_eq!(progress.indexed, 0);
        assert!(index.search("acct", "INBOX", "报价单").unwrap().is_empty());
    }

    #[tokio::test]
    async fn partial_index_is_searchable_before_bodies_finish() {
        let server = crate::test_imap::state("", &[("INBOX", 3)]);
        for mail in [
            utf8_mail("5oql5Lu35Y2V", "5q2j5paH"),
            utf8_mail("5Lya6K6u6YCa55+l", "5Y2g5Lya"),
        ] {
            server.lock().unwrap().add("INBOX", &mail, &[]);
        }
        let index = SearchIndex::in_memory().unwrap();
        let mut session = crate::test_imap::session(server.clone()).await;
        // 只跑摘要阶段：模拟“正文还没补完”的中间状态
        let mailbox = session.select("INBOX").await.unwrap();
        let validity = mailbox.uid_validity.unwrap_or(0);
        let mut progress = index.begin_folder("acct", "INBOX", validity).unwrap();
        progress.running = true;
        progress.total = 2;
        let summaries = imap::fetch_summaries(&mut session, "1:2", "acct", "INBOX")
            .await
            .unwrap();
        index
            .insert_summaries("acct", "INBOX", validity, &summaries)
            .unwrap();
        // 主题马上可搜，正文还搜不到
        assert_eq!(index.search("acct", "INBOX", "报价单").unwrap().len(), 1);
        assert!(index.search("acct", "INBOX", "正文").unwrap().is_empty());
        assert_eq!(index.uids_without_body("acct", "INBOX").unwrap(), [2, 1]);

        // 正文阶段补上后，正文也能搜
        let previews = imap::fetch_body_texts(&mut session, "1:2").await.unwrap();
        let texts: Vec<(u32, String)> = previews
            .into_iter()
            .map(|Preview { uid, text }| (uid, text))
            .collect();
        index.update_bodies("acct", "INBOX", &texts).unwrap();
        assert_eq!(index.search("acct", "INBOX", "正文").unwrap().len(), 1);
        assert!(index.uids_without_body("acct", "INBOX").unwrap().is_empty());
    }

    /// 真实账号冒烟：只读，用来确认真实 IMAP 服务器上的增量索引和中文搜索。
    /// 不放进 CI：需要 `MP_EMAIL` / `MP_HOST` / `MP_PASS` 环境变量。
    #[tokio::test]
    #[ignore]
    async fn real_account_index_smoke() {
        let email = std::env::var("MP_EMAIL").expect("MP_EMAIL");
        let host = std::env::var("MP_HOST").expect("MP_HOST");
        let pass = std::env::var("MP_PASS").expect("MP_PASS");
        let account = crate::config::Account {
            id: "probe".into(),
            kind: "custom".into(),
            email,
            name: String::new(),
            signature: String::new(),
            auto_download: Default::default(),
            receive_protocol: Default::default(),
            pop3_host: String::new(),
            pop3_port: 995,
            pop3_tls: true,
            imap_host: host,
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
        };
        let dir = tempfile::tempdir().unwrap();
        let index = SearchIndex::open(&dir.path().join("index.db")).unwrap();
        let mut session = crate::imap::login(&account, &pass).await.unwrap();
        let started = std::time::Instant::now();
        let progress = index_folder(&mut session, &index, "probe", "INBOX", |p| {
            if p.indexed % 200 == 0 || p.body_indexed % 200 == 0 || p.complete {
                eprintln!(
                    "摘要 {}/{}，正文 {}/{}，用时 {:.0}s",
                    p.indexed,
                    p.total,
                    p.body_indexed,
                    p.body_total,
                    started.elapsed().as_secs_f32()
                );
            }
        })
        .await
        .unwrap();
        eprintln!(
            "索引完成：摘要 {}/{}，正文 {}/{}，用时 {:.1} 分钟",
            progress.indexed,
            progress.total,
            progress.body_indexed,
            progress.body_total,
            started.elapsed().as_secs_f32() / 60.0
        );
        assert!(progress.complete);
        assert!(progress.indexed > 0);

        // 用最旧一封的主题里的中文片段回搜：旧邮件没有加载到界面上，正是要覆盖的场景
        let (uid, json): (u32, String) = index
            .lock()
            .query_row(
                "SELECT uid, summary FROM mail_index
                 WHERE account_id = 'probe' AND folder = 'INBOX'
                 ORDER BY uid ASC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        let oldest: Summary = serde_json::from_str(&json).unwrap();
        let term: String = oldest
            .subject
            .chars()
            .filter(|c| ('\u{4e00}'..='\u{9fff}').contains(c))
            .take(2)
            .collect();
        assert!(
            term.chars().count() >= 2,
            "最旧一封主题里没有中文：{:?}",
            oldest.subject
        );
        let found = index.search("probe", "INBOX", &term).unwrap();
        eprintln!(
            "搜索 {:?} 命中 {} 封，最旧一封 uid={} found={}",
            term,
            found.len(),
            uid,
            found.iter().any(|m| m.uid == uid)
        );
        assert!(found.iter().any(|m| m.uid == uid));

        // 第二次走增量：不应再下载任何邮件
        let again = index_folder(&mut session, &index, "probe", "INBOX", |_| {})
            .await
            .unwrap();
        assert!(again.complete);
        assert_eq!(again.indexed, progress.indexed);
        eprintln!("增量索引完成：{} 封", again.indexed);
    }
}
