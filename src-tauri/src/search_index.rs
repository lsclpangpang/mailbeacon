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

use std::{
    collections::{HashMap, HashSet},
    fmt,
    future::Future,
    path::Path,
    sync::Mutex,
    time::Duration,
};

use anyhow::{Context, Result};
use async_imap::Session;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

use crate::imap::{self, OP_TIMEOUT, Preview, Summary};
#[path = "flag_query.rs"]
mod flag_query;
#[path = "search_query.rs"]
mod query;
#[path = "sidebar_query.rs"]
mod sidebar_query;
pub use query::{SearchPage, SearchRequest, SearchTarget, SnapshotPage};

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
    -- 1 = 这份摘要是用会修邮件头编码的解析器建的；0 = 旧版本建的、可能带着乱码，
    -- 索引时按 U+FFFD 找出来重新取一次
    encoding_fixed INTEGER NOT NULL DEFAULT 0,
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
    // Only live workers need a revision. The incomplete metadata survives restart.
    revisions: Mutex<HashMap<(String, String), u64>>,
    // An online page can be actionable before its first summary reaches the index.
    unindexed_flags: Mutex<HashMap<(String, String, u32, u32), flag_query::PendingFlags>>,
}

#[derive(Debug)]
struct StaleIndexPass;
impl fmt::Display for StaleIndexPass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("文件夹已变化，重新核对索引")
    }
}
impl std::error::Error for StaleIndexPass {}

struct FolderPass<'a> {
    index: &'a SearchIndex,
    key: (String, String),
    revision: u64,
}
impl FolderPass<'_> {
    // Keep the revision check and each database mutation atomic against invalidation.
    // Always acquire revisions before conn; never retain either lock across an await.
    fn write<T>(&self, action: impl FnOnce(&SearchIndex) -> Result<T>) -> Result<T> {
        let revisions = self.index.revisions.lock().unwrap();
        if revisions.get(&self.key).copied().unwrap_or(0) != self.revision {
            return Err(StaleIndexPass.into());
        }
        action(self.index)
    }
}

impl SearchIndex {
    /// 打开（或创建）本地索引库。与通讯录共用一个 local.db。
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("打开本地数据库 {} 失败", path.display()))?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.execute_batch(SCHEMA).context("创建搜索索引表失败")?;
        query::init(&conn)?;
        // 旧库升级：加一列标记摘要是不是用会修邮件头编码的解析器建的
        let _ = conn.execute(
            "ALTER TABLE mail_index ADD COLUMN encoding_fixed INTEGER NOT NULL DEFAULT 0",
            [],
        );
        Ok(Self {
            conn: Mutex::new(conn),
            revisions: Mutex::default(),
            unindexed_flags: Mutex::default(),
        })
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        query::init(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            revisions: Mutex::default(),
            unindexed_flags: Mutex::default(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().unwrap()
    }

    /// A move or explicit refresh makes the previous UID enumeration untrustworthy.
    /// Preserve cached mail for offline reading, but never serve it as a complete list.
    pub fn invalidate_folder(&self, account_id: &str, folder: &str) -> Result<()> {
        let mut revisions = self.revisions.lock().unwrap();
        *revisions
            .entry((account_id.to_owned(), folder.to_owned()))
            .or_default() += 1;
        self.lock().execute(
            "UPDATE mail_index_meta SET complete=0,total=0 WHERE account_id=?1 AND folder=?2",
            params![account_id, folder],
        )?;
        self.lock().execute(
            "DELETE FROM mail_sidebar_status WHERE account_id=?1 AND folder=?2",
            params![account_id, folder],
        )?;
        Ok(())
    }

    /// A confirmed move removes an exact identity without discarding a complete source list.
    pub fn remove_confirmed(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> Result<()> {
        let mut revisions = self.revisions.lock().unwrap();
        *revisions
            .entry((account.to_owned(), folder.to_owned()))
            .or_default() += 1;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let removed = tx.execute(
            "DELETE FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",
            params![account, folder, validity, uid],
        )?;
        // A zero total on an incomplete folder is the invalidation marker; never clear it here.
        tx.execute(
            "UPDATE mail_index_meta SET
                complete=CASE WHEN complete=1 OR (total>0 AND indexed=total AND total=?4) THEN 1 ELSE 0 END,
                total=MAX(0,total-?4),
                indexed=(SELECT COUNT(*) FROM mail_index WHERE account_id=?1 AND folder=?2),
                body_indexed=(SELECT COALESCE(SUM(has_body),0) FROM mail_index WHERE account_id=?1 AND folder=?2)
             WHERE account_id=?1 AND folder=?2 AND uid_validity=?3",
            params![account, folder, validity, removed],
        )?;
        tx.commit()?;
        Ok(())
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
                 updated_at = excluded.updated_at
                 WHERE mail_index_meta.uid_validity = excluded.uid_validity",
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
        // Generation checks and replacement share one write transaction: an older worker
        // cannot clear a newer generation between separate lock acquisitions.
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let existing: Option<u32> = tx
            .query_row(
                "SELECT uid_validity FROM mail_index_meta WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
                |row| row.get(0),
            )
            .optional()?;
        if existing.is_some_and(|stored| uid_validity > 0 && stored > uid_validity) {
            anyhow::bail!("旧文件夹索引任务已过期，请重新同步");
        }
        let replaced = uid_validity != 0 && existing.is_some_and(|stored| stored != uid_validity);
        if replaced {
            tx.execute(
                "DELETE FROM mail_index WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
            )?;
            tx.execute(
                "DELETE FROM mail_index_meta WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
            )?;
        }
        if existing.is_none() || replaced {
            tx.execute("INSERT INTO mail_index_meta (account_id,folder,uid_validity,complete,indexed,total,body_indexed,updated_at) VALUES (?1,?2,?3,0,0,0,0,?4)",
                params![account_id, folder, uid_validity, chrono::Utc::now().timestamp_millis()])?;
        }
        tx.commit()?;
        drop(conn);
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

    fn count(&self, account_id: &str, folder: &str) -> Result<u32> {
        Ok(self.lock().query_row(
            "SELECT COUNT(*) FROM mail_index WHERE account_id = ?1 AND folder = ?2",
            params![account_id, folder],
            |row| row.get(0),
        )?)
    }

    /// 摘要是否已经覆盖整个文件夹：列表可以直接用本地索引，不必再分页。
    ///
    /// `complete` 表示连正文索引都跑完了；正文阶段之前摘要就齐了，这时
    /// （`indexed == total`）也能直接列出全部邮件，用户不用等正文索引。
    fn headers_ready(meta: Option<&(u32, bool, u32, u32)>) -> bool {
        meta.is_some_and(|(_, complete, total, indexed)| {
            *complete || (*total > 0 && indexed == total)
        })
    }

    /// 这个文件夹建索引时记下的 UIDVALIDITY；还没建过索引时为 None。
    pub fn folder_validity(&self, account_id: &str, folder: &str) -> Result<Option<u32>> {
        Ok(self
            .folder_meta(account_id, folder)?
            .map(|(validity, ..)| validity)
            .filter(|validity| *validity != 0))
    }

    /// 旧版本建索引时把邮件头里没做 RFC 2047 编码的 GBK 字节按 UTF-8 解，
    /// 发件人/主题变成 U+FFFD。这些行要重新取一次摘要（`encoding_fixed` 置 1
    /// 后就不再选出来，不会反复重拉）。
    fn uids_with_broken_encoding(&self, account_id: &str, folder: &str) -> Result<Vec<u32>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT uid FROM mail_index
             WHERE account_id = ?1 AND folder = ?2 AND encoding_fixed = 0
               AND instr(summary, ?3) > 0
             ORDER BY uid DESC",
        )?;
        let rows = stmt.query_map(params![account_id, folder, "\u{fffd}"], |row| {
            row.get::<_, u32>(0)
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Shared with list_messages: only an explicit first-page refresh invalidates
    /// coverage. Pagination and index-completion reloads must not restart indexing.
    pub(crate) fn list_request_snapshot(
        &self,
        account: &str,
        folder: &str,
        before: Option<u32>,
        force: bool,
    ) -> Result<Option<Vec<Summary>>> {
        if before.is_some() {
            return Ok(None);
        }
        if force {
            self.invalidate_folder(account, folder)?;
            return Ok(None);
        }
        self.listed(account, folder)
    }

    /// 列表用：本地索引里这个文件夹的全部摘要，按收件时间从新到旧。
    ///
    /// 索引还没覆盖整个文件夹时返回 `None`，调用方继续用服务器分页；返回
    /// `Some` 时就是整个文件夹，界面不用再「加载更早」。
    pub fn listed(&self, account_id: &str, folder: &str) -> Result<Option<Vec<Summary>>> {
        let _revision = self.revisions.lock().unwrap();
        if !Self::headers_ready(self.folder_meta(account_id, folder)?.as_ref()) {
            return Ok(None);
        }
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT summary,uid_validity FROM mail_index
             WHERE account_id = ?1 AND folder = ?2
             ORDER BY date DESC, uid DESC",
        )?;
        let rows = stmt.query_map(params![account_id, folder], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, u32>(1)?))
        })?;
        let mut items = Vec::new();
        for row in rows {
            // 索引内容损坏时跳过这一封，不影响其他邮件
            let (json, validity) = row?;
            if let Ok(mut summary) = serde_json::from_str::<Summary>(&json) {
                query::stamp(&mut summary, account_id, validity);
                items.push(summary);
            }
        }
        Ok(Some(items))
    }

    /// 用服务器最新的已读/星标刷新已索引的邮件。只改本地已经有的 UID。
    pub fn update_flags(
        &self,
        account_id: &str,
        folder: &str,
        flags: &[(u32, bool, bool)],
    ) -> Result<()> {
        if flags.is_empty() {
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
                "UPDATE mail_index SET summary = ?4
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for &(uid, seen, flagged) in flags {
                let stored: Option<String> = read
                    .query_row(params![account_id, folder, uid], |row| row.get(0))
                    .optional()?;
                let Some(json) = stored else { continue };
                let Ok(mut summary) = serde_json::from_str::<Summary>(&json) else {
                    continue;
                };
                if summary.seen == seen && summary.flagged == flagged {
                    continue;
                }
                summary.seen = seen;
                summary.flagged = flagged;
                write.execute(params![
                    account_id,
                    folder,
                    uid,
                    serde_json::to_string(&summary)?
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// 界面改了某几封的已读或星标：只更新给出的字段，其余保持不变。
    pub fn set_flags(
        &self,
        account_id: &str,
        folder: &str,
        uids: &[u32],
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<()> {
        self.set_flags_at_validity(account_id, folder, uids, seen, flagged, None)
            .map(|_| ())
    }

    fn set_flags_at_validity(
        &self,
        account_id: &str,
        folder: &str,
        uids: &[u32],
        seen: Option<bool>,
        flagged: Option<bool>,
        validity: Option<u32>,
    ) -> Result<u64> {
        let mut revisions = self.revisions.lock().unwrap();
        let revision = revisions
            .entry((account_id.into(), folder.into()))
            .or_default();
        if uids.is_empty() {
            return Ok(*revision);
        }
        let mut missing = Vec::new();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut read = tx.prepare(
                "SELECT summary FROM mail_index
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3
                   AND (?4 IS NULL OR uid_validity = ?4)",
            )?;
            let mut write = tx.prepare(
                "UPDATE mail_index SET summary = ?4
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for &uid in uids {
                let stored: Option<String> = read
                    .query_row(params![account_id, folder, uid, validity], |row| row.get(0))
                    .optional()?;
                let Some(json) = stored else {
                    missing.push(uid);
                    continue;
                };
                let Ok(mut summary) = serde_json::from_str::<Summary>(&json) else {
                    continue;
                };
                let next_seen = seen.unwrap_or(summary.seen);
                let next_flagged = flagged.unwrap_or(summary.flagged);
                if next_seen == summary.seen && next_flagged == summary.flagged {
                    continue;
                }
                summary.seen = next_seen;
                summary.flagged = next_flagged;
                write.execute(params![
                    account_id,
                    folder,
                    uid,
                    serde_json::to_string(&summary)?
                ])?;
            }
        }
        tx.commit()?;
        // Advance even for an idempotent confirmation: a previously started FETCH
        // may still contain an older server flag. Do not invalidate list coverage.
        *revision += 1;
        if let Some(validity) = validity {
            let mut pending = self.unindexed_flags.lock().unwrap();
            for uid in missing {
                let flags = pending
                    .entry((account_id.into(), folder.into(), validity, uid))
                    .or_default();
                if let Some(seen) = seen {
                    flags.seen = Some((*revision, seen));
                }
                if let Some(flagged) = flagged {
                    flags.flagged = Some((*revision, flagged));
                }
            }
        }
        Ok(*revision)
    }

    /// 邮件在服务器上被删除或移走：从本地索引里去掉，列表立即不再显示。
    pub fn remove_uids(&self, account_id: &str, folder: &str, uids: &[u32]) -> Result<()> {
        if uids.is_empty() {
            return Ok(());
        }
        let mut revisions = self.revisions.lock().unwrap();
        *revisions
            .entry((account_id.to_owned(), folder.to_owned()))
            .or_default() += 1;
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut removed = 0;
        {
            let mut stmt = tx.prepare(
                "DELETE FROM mail_index WHERE account_id = ?1 AND folder = ?2 AND uid = ?3",
            )?;
            for uid in uids {
                removed += stmt.execute(params![account_id, folder, uid])?;
            }
        }
        tx.execute("UPDATE mail_index_meta SET
            complete=CASE WHEN complete=1 OR (total>0 AND indexed=total AND total=?3) THEN 1 ELSE 0 END,
            total=MAX(0,total-?3),
            indexed=(SELECT COUNT(*) FROM mail_index WHERE account_id=?1 AND folder=?2),
            body_indexed=(SELECT COALESCE(SUM(has_body),0) FROM mail_index WHERE account_id=?1 AND folder=?2),
            updated_at=?4
            WHERE account_id=?1 AND folder=?2",params![account_id,folder,removed,chrono::Utc::now().timestamp_millis()])?;
        tx.commit()?;
        Ok(())
    }

    /// 写入（或更新）邮件摘要。
    ///
    /// 索引阶段用来补缺的邮件头；收信时也用它把新邮件、以及服务器上最新的
    /// 已读/星标状态写回本地，列表就不用等下一次索引。已经补过正文的邮件
    /// 保留原来的正文索引，不会被重新写回的摘要清掉。
    pub fn upsert_summaries(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        summaries: &[Summary],
    ) -> Result<()> {
        if summaries.is_empty() {
            return Ok(());
        }
        if self.folder_meta(account_id, folder)?.is_none() {
            self.record_snapshot(account_id, folder, uid_validity)?;
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let current: u32 = tx.query_row(
            "SELECT uid_validity FROM mail_index_meta WHERE account_id=?1 AND folder=?2",
            params![account_id, folder],
            |r| r.get(0),
        )?;
        if current != uid_validity {
            anyhow::bail!("旧文件夹索引任务已过期，请重新同步");
        }
        {
            let mut stmt = tx.prepare(
                "INSERT INTO mail_index
                     (account_id, folder, uid, uid_validity, date, summary, haystack, has_body, encoding_fixed)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 1)
                 ON CONFLICT(account_id, folder, uid) DO UPDATE SET
                     uid_validity = excluded.uid_validity,
                     date = excluded.date,
                     summary = excluded.summary,
                     haystack = CASE WHEN mail_index.has_body = 1
                         THEN mail_index.haystack ELSE excluded.haystack END,
                     has_body = mail_index.has_body,
                     encoding_fixed = 1",
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
    fn update_bodies(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        texts: &[(u32, String)],
    ) -> Result<()> {
        if texts.is_empty() {
            return Ok(());
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let mut read = tx.prepare(
                "SELECT summary FROM mail_index
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?4",
            )?;
            let mut write = tx.prepare(
                "UPDATE mail_index SET haystack = ?4, has_body = 1, body_coverage = CASE WHEN body_coverage='full' THEN 'full' ELSE 'prefix' END
                 WHERE account_id = ?1 AND folder = ?2 AND uid = ?3 AND uid_validity = ?5 AND body_coverage != 'full'",
            )?;
            for (uid, body) in texts {
                let summary: Option<String> = read
                    .query_row(params![account_id, folder, uid, uid_validity], |row| {
                        row.get(0)
                    })
                    .optional()?;
                let Some(json) = summary else { continue };
                let Ok(summary) = serde_json::from_str::<Summary>(&json) else {
                    continue;
                };
                write.execute(params![
                    account_id,
                    folder,
                    uid,
                    haystack(&summary, body),
                    uid_validity
                ])?;
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
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR IGNORE INTO removed_mail_accounts VALUES(?1)",
            [account_id],
        )?;
        let prefix = format!("{account_id}\u{1f}receive:");
        for table in [
            "mail_index",
            "mail_index_meta",
            "mail_folder_snapshot",
            "mail_sidebar_status",
            "mail_sidebar_revision",
        ] {
            tx.execute(
                &format!(
                    "DELETE FROM {table} WHERE account_id=?1 OR substr(account_id,1,length(?2))=?2"
                ),
                params![account_id, prefix],
            )?;
        }
        tx.commit()?;
        self.unindexed_flags
            .lock()
            .unwrap()
            .retain(|(account, _, _, _), _| account != account_id && !account.starts_with(&prefix));
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

/// 一批取多少封随服务器速度调整：超时减半（最少 1 封），之后每成功一批长回四分之一。
#[derive(Clone, Copy, Debug)]
struct Batch {
    size: usize,
    max: usize,
}

impl Batch {
    fn new(max: usize) -> Self {
        Self { size: max, max }
    }
    fn shrink(&mut self) {
        self.size = (self.size / 2).max(1);
    }
    fn grow(&mut self) {
        self.size = (self.size + self.size.div_ceil(4)).min(self.max);
    }
}

/// 一次索引任务跨重连保留的状态：批次大小，以及服务器上的 UID 列表（大文件夹枚举一次就够）。
struct Run {
    headers: Batch,
    bodies: Batch,
    uids: Option<(u32, Vec<u32>)>,
    revision: Option<u64>,
    /// 上次单独取正文超时的那一封
    slow_body: Option<u32>,
}

impl Run {
    fn pass<'a>(
        &mut self,
        index: &'a SearchIndex,
        account: &str,
        folder: &str,
    ) -> Result<FolderPass<'a>> {
        let key = (account.to_owned(), folder.to_owned());
        let revision = index
            .revisions
            .lock()
            .unwrap()
            .get(&key)
            .copied()
            .unwrap_or(0);
        if self.revision.is_some_and(|old| old != revision) {
            return Err(StaleIndexPass.into());
        }
        self.revision = Some(revision);
        Ok(FolderPass {
            index,
            key,
            revision,
        })
    }
    /// 单独取 `uid` 的正文失败后，要不要跳过它的正文：只算超时（断线不是这封邮件的问题），
    /// 而且同一封连续超时第二次才跳过，免得服务器偶尔慢一下就丢掉正文。
    fn skip_body_after(&mut self, uid: u32, error: &anyhow::Error) -> bool {
        if !error.chain().any(|c| c.is::<tokio::time::error::Elapsed>()) {
            return false;
        }
        if self.slow_body == Some(uid) {
            self.slow_body = None;
            return true;
        }
        self.slow_body = Some(uid);
        false
    }
}

impl Default for Run {
    fn default() -> Self {
        Self {
            headers: Batch::new(BATCH_HEADERS),
            bodies: Batch::new(BATCH_BODIES),
            uids: None,
            revision: None,
            slow_body: None,
        }
    }
}

/// 连续这么多次重连都没有任何进展才放弃。批次从 40 减半到 1 要 6 次，留些余量。
const MAX_STALLED_ATTEMPTS: u32 = 8;

/// 增量索引一个文件夹：先补摘要（主题/发件人），再补正文开头。只用一条连接，出错就停
/// （测试用；App 里用 `index_folder_resilient`）。
///
/// 用独立连接（调用方自己登录），不占用界面操作的命令连接；`report` 每完成
/// 一批就回调一次，用来更新进度和推送 `searchIndex` 事件。中断后重开会接着
/// 已索引的部分继续，只补缺的。
#[cfg(test)]
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
    index_pass(
        session,
        index,
        account_id,
        folder,
        &mut Run::default(),
        &mut report,
    )
    .await
}

/// 后台索引整个失败（重连多次都没进展、服务器回了意外的错误）以后多久再试：
/// 1、5、15 分钟，之后每 30 分钟一次。不然要等用户下次搜索才会接着建。
pub fn retry_delay(failures: u32) -> Duration {
    const MINUTES: [u64; 4] = [1, 5, 15, 30];
    let i = (failures.max(1) as usize - 1).min(MINUTES.len() - 1);
    Duration::from_secs(MINUTES[i] * 60)
}

/// 密码错了重试只会触发风控，其余错误都值得过一阵再试。
pub fn worth_retrying(e: &str) -> bool {
    !e.contains("登录失败")
}

/// 邮件多、服务器慢时也能建完的索引：超时或断线后缩小批次、重新登录接着建
/// （已建好的部分不重拉）；某一封的正文单独取也超时就跳过它的正文。
/// 连续多次重连都没有进展才报错；服务器拒绝命令之类的错误不重试。
pub async fn index_folder_resilient<T, C, Fut, F>(
    mut connect: C,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
    mut report: F,
) -> Result<Progress>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<Session<T>>>,
    F: FnMut(&Progress),
{
    let mut run = Run::default();
    let mut stalled = 0;
    loop {
        let before = index.stats(account_id, folder)?;
        let attempt = async {
            let mut session = connect().await?;
            index_pass(
                &mut session,
                index,
                account_id,
                folder,
                &mut run,
                &mut report,
            )
            .await
        }
        .await;
        let error = match attempt {
            Ok(progress) => return Ok(progress),
            Err(e) if e.is::<StaleIndexPass>() => {
                run = Run::default();
                stalled = 0;
                continue;
            }
            Err(e) if crate::download_protocol::connection_problem(&e) => e,
            Err(e) => return Err(e),
        };
        stalled = if index.stats(account_id, folder)? == before {
            stalled + 1
        } else {
            0
        };
        if stalled >= MAX_STALLED_ATTEMPTS {
            return Err(error.context(format!("连续重试 {stalled} 次都没有进展")));
        }
        // 连接已不可用（超时后结果未知）：稍等再重新登录，等得越来越久
        tokio::time::sleep(Duration::from_secs(1 << stalled.min(5))).await;
    }
}

/// 阶段一：补齐缺的邮件头，再用服务器最新的已读/星标刷新一遍。
///
/// 完整索引（搜索用）和只建摘要的列表索引（`index_headers_pass`）共用这一段。
/// 列表和搜索结果都直接读本地索引，旧邮件不能一直显示成未读；标记刷新只是
/// 锦上添花：服务器慢、超时都不该让整个索引失败，下次索引或收信时会再补。
async fn index_summaries<T, F>(
    session: &mut Session<T>,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
    uid_validity: u32,
    pass: &FolderPass<'_>,
    run: &mut Run,
    progress: &mut Progress,
    report: &mut F,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
    F: FnMut(&Progress),
{
    let all = match &run.uids {
        Some((validity, uids)) if *validity == uid_validity => uids.clone(),
        _ => {
            let uids = imap::all_uids(session).await?;
            run.uids = Some((uid_validity, uids.clone()));
            uids
        }
    };
    let all_set: HashSet<u32> = all.iter().copied().collect();
    let stored_set: HashSet<u32> = index.stored_uids(account_id, folder)?.into_iter().collect();

    let stale: Vec<u32> = stored_set.difference(&all_set).copied().collect();
    pass.write(|index| {
        let conn = index.lock();
        for uid in &stale {
            conn.execute("DELETE FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid=?3 AND uid_validity=?4", params![account_id, folder, uid, uid_validity])?;
        }
        Ok(())
    })?;

    // 摘要阶段：从最新和最旧两端交替，两端都能较快搜到。
    let mut missing: Vec<u32> = all_set.difference(&stored_set).copied().collect();
    // 旧解析器留下的乱码行也重新取一次（取完 encoding_fixed=1，下次不再重拉）
    let mut queued: HashSet<u32> = missing.iter().copied().collect();
    for uid in index.uids_with_broken_encoding(account_id, folder)? {
        if all_set.contains(&uid) && queued.insert(uid) {
            missing.push(uid);
        }
    }
    missing.sort_unstable_by(|a, b| b.cmp(a));
    let missing = interleave_from_ends(&missing);

    progress.total = all.len() as u32;
    progress.indexed = index.count(account_id, folder)?;
    pass.write(|index| {
        index.save_meta(
            account_id,
            folder,
            uid_validity,
            false,
            progress.indexed,
            progress.total,
            progress.body_indexed,
        )
    })?;
    report(progress);

    let mut rest = &missing[..];
    while !rest.is_empty() {
        let (chunk, after) = rest.split_at(run.headers.size.min(rest.len()));
        let summaries =
            match imap::fetch_summaries(session, &imap::uid_set(chunk), account_id, folder).await {
                Ok(summaries) => summaries,
                Err(e) => {
                    if crate::download_protocol::connection_problem(&e) {
                        run.headers.shrink();
                    }
                    return Err(e);
                }
            };
        run.headers.grow();
        rest = after;
        pass.write(|index| index.upsert_summaries(account_id, folder, uid_validity, &summaries))?;
        progress.indexed = index.count(account_id, folder)?;
        pass.write(|index| {
            index.save_meta(
                account_id,
                folder,
                uid_validity,
                false,
                progress.indexed,
                progress.total,
                progress.body_indexed,
            )
        })?;
        report(progress);
    }

    refresh_flags(session, &pass, account_id, folder).await?;
    Ok(())
}

/// 用服务器上最新的已读/星标刷新本地索引（失败不影响索引本身）。
async fn refresh_flags<T>(
    session: &mut Session<T>,
    pass: &FolderPass<'_>,
    account_id: &str,
    folder: &str,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    if let Ok(flags) = imap::fetch_flags(session, "1:*").await {
        let flags: Vec<(u32, bool, bool)> = flags
            .into_iter()
            .map(|f| (f.uid, f.seen, f.flagged))
            .collect();
        pass.write(|index| index.update_flags(account_id, folder, &flags))?;
    }
    Ok(())
}

async fn index_pass<T, F>(
    session: &mut Session<T>,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
    run: &mut Run,
    report: &mut F,
) -> Result<Progress>
where
    T: AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send,
    F: FnMut(&Progress),
{
    let pass = run.pass(index, account_id, folder)?;
    let mailbox = timeout(OP_TIMEOUT, session.select(folder))
        .await
        .context("打开文件夹超时")?
        .with_context(|| format!("打开文件夹 {} 失败", imap::decode_mutf7(folder)))?;
    let uid_validity = mailbox.uid_validity.unwrap_or(0);

    let mut progress = pass.write(|index| index.begin_folder(account_id, folder, uid_validity))?;
    progress.running = true;
    progress.complete = false;
    progress.error = None;
    report(&progress);

    index_summaries(
        session,
        index,
        account_id,
        folder,
        uid_validity,
        &pass,
        run,
        &mut progress,
        report,
    )
    .await?;

    // 阶段二：正文开头。中断过或上次没补完的都在这里继续。
    let pending = index.uids_without_body(account_id, folder)?;
    let mut rest = &pending[..];
    while !rest.is_empty() {
        let (chunk, after) = rest.split_at(run.bodies.size.min(rest.len()));
        let previews: Vec<Preview> =
            match imap::fetch_body_texts(session, &imap::uid_set(chunk)).await {
                Ok(previews) => previews,
                Err(e) => {
                    if crate::download_protocol::connection_problem(&e) {
                        if let [uid] = chunk {
                            // 单独取这一封也反复超时：跳过它的正文（主题、发件人照样能搜），免得卡住整个索引
                            if run.skip_body_after(*uid, &e) {
                                pass.write(|index| {
                                    index.skip_body(account_id, folder, uid_validity, *uid)
                                })?;
                            }
                        } else {
                            run.bodies.shrink();
                        }
                    }
                    return Err(e);
                }
            };
        run.bodies.grow();
        rest = after;
        let texts: Vec<(u32, String)> = previews
            .into_iter()
            .map(|Preview { uid, text }| (uid, text))
            .collect();
        pass.write(|index| index.update_bodies(account_id, folder, uid_validity, &texts))?;
        progress.body_indexed = index.stats(account_id, folder)?.1;
        pass.write(|index| {
            index.save_meta(
                account_id,
                folder,
                uid_validity,
                false,
                progress.indexed,
                progress.total,
                progress.body_indexed,
            )
        })?;
        report(&progress);
    }

    progress.running = false;
    progress.complete = true;
    progress.indexed = index.count(account_id, folder)?;
    progress.body_indexed = index.stats(account_id, folder)?.1;
    progress.body_total = progress.indexed;
    pass.write(|index| {
        index.save_meta(
            account_id,
            folder,
            uid_validity,
            true,
            progress.indexed,
            progress.total,
            progress.body_indexed,
        )
    })?;
    report(&progress);
    pass.write(|_| Ok(progress))
}

/// 只建摘要（并刷新已读/星标）、不下载正文开头的索引。
///
/// 打开一个还没索引过的文件夹时用：列表要读本地索引，只需要摘要；正文索引
/// 留给搜索时再补（每封约 0.7 秒，比摘要贵得多）。失败时和其它索引一样自动
/// 重连重试；服务器一直慢的话下次打开文件夹还会再试。
pub async fn index_headers_resilient<T, C, Fut>(
    mut connect: C,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
) -> Result<Progress>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
    C: FnMut() -> Fut,
    Fut: Future<Output = Result<Session<T>>>,
{
    let mut run = Run::default();
    let mut stalled = 0;
    loop {
        let before = index.stats(account_id, folder)?;
        let attempt = async {
            let mut session = connect().await?;
            index_headers_pass(&mut session, index, account_id, folder, &mut run).await
        }
        .await;
        let error = match attempt {
            Ok(progress) => return Ok(progress),
            Err(e) if e.is::<StaleIndexPass>() => {
                run = Run::default();
                stalled = 0;
                continue;
            }
            Err(e) if crate::download_protocol::connection_problem(&e) => e,
            Err(e) => return Err(e),
        };
        stalled = if index.stats(account_id, folder)? == before {
            stalled + 1
        } else {
            0
        };
        if stalled >= MAX_STALLED_ATTEMPTS {
            return Err(error.context(format!("连续重试 {stalled} 次都没有进展")));
        }
        tokio::time::sleep(Duration::from_secs(1 << stalled.min(5))).await;
    }
}

async fn index_headers_pass<T>(
    session: &mut Session<T>,
    index: &SearchIndex,
    account_id: &str,
    folder: &str,
    run: &mut Run,
) -> Result<Progress>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let pass = run.pass(index, account_id, folder)?;
    let mailbox = timeout(OP_TIMEOUT, session.select(folder))
        .await
        .context("打开文件夹超时")?
        .with_context(|| format!("打开文件夹 {} 失败", imap::decode_mutf7(folder)))?;
    let uid_validity = mailbox.uid_validity.unwrap_or(0);

    let mut progress = pass.write(|index| index.begin_folder(account_id, folder, uid_validity))?;
    progress.running = true;
    progress.complete = false;
    progress.error = None;
    index_summaries(
        session,
        index,
        account_id,
        folder,
        uid_validity,
        &pass,
        run,
        &mut progress,
        &mut |_| {},
    )
    .await?;

    progress.running = false;
    progress.indexed = index.count(account_id, folder)?;
    progress.body_indexed = index.stats(account_id, folder)?.1;
    progress.body_total = progress.indexed;
    // 空文件夹没有正文可补，直接算完整；否则 complete 保持 false，
    // 用户搜索这个文件夹时再补正文
    let complete = progress.total == 0;
    progress.complete = complete;
    pass.write(|index| {
        index.save_meta(
            account_id,
            folder,
            uid_validity,
            complete,
            progress.indexed,
            progress.total,
            progress.body_indexed,
        )
    })?;
    pass.write(|_| Ok(progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(uid: u32, subject: &str, from_name: &str, from_addr: &str, to: &str) -> Summary {
        Summary {
            account_id: "acct".into(),
            receive_epoch: 1,
            uid_validity: 0,
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
    fn invalidation_rejects_empty_and_nonempty_complete_lists_without_losing_cached_rows() {
        let index = SearchIndex::in_memory().unwrap();
        index.begin_folder("acct", "Trash", 9).unwrap();
        index.save_meta("acct", "Trash", 9, true, 0, 0, 0).unwrap();
        index
            .upsert("acct", "INBOX", 7, &[row(1, "retained", "offline body")])
            .unwrap();
        index.save_meta("acct", "INBOX", 7, false, 1, 1, 1).unwrap();
        assert!(index.listed("acct", "Trash").unwrap().is_some());
        assert!(index.listed("acct", "INBOX").unwrap().is_some());
        for folder in ["Trash", "INBOX"] {
            index.invalidate_folder("acct", folder).unwrap();
            assert!(
                index.listed("acct", folder).unwrap().is_none(),
                "{folder} must be re-enumerated"
            );
            let p = index.progress("acct", folder).unwrap();
            assert!(!p.complete);
            assert_eq!(p.total, 0);
        }
        assert_eq!(
            index.search("acct", "INBOX", "offline body").unwrap().len(),
            1
        );
    }

    #[test]
    fn invalidation_survives_restart_and_does_not_invalidate_other_folders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.db");
        let index = SearchIndex::open(&path).unwrap();
        for folder in ["Trash", "Archive"] {
            index.begin_folder("acct", folder, 9).unwrap();
            index.save_meta("acct", folder, 9, true, 0, 0, 0).unwrap();
        }
        index
            .upsert("acct", "INBOX", 7, &[row(1, "retained", "offline body")])
            .unwrap();
        index.save_meta("acct", "INBOX", 7, true, 1, 1, 1).unwrap();
        index.invalidate_folder("acct", "INBOX").unwrap();
        index.invalidate_folder("acct", "Trash").unwrap();
        drop(index);
        let index = SearchIndex::open(&path).unwrap();
        assert!(index.listed("acct", "Trash").unwrap().is_none());
        assert!(index.listed("acct", "Archive").unwrap().is_some());
        assert!(index.listed("acct", "INBOX").unwrap().is_none());
        assert_eq!(
            index.search("acct", "INBOX", "offline body").unwrap().len(),
            1
        );
    }

    #[tokio::test]
    async fn explicit_first_page_refresh_recovers_an_old_complete_empty_snapshot() {
        let index = SearchIndex::in_memory().unwrap();
        index.begin_folder("acct", "Trash", 9).unwrap();
        index.save_meta("acct", "Trash", 9, true, 0, 0, 0).unwrap();
        let server = crate::test_imap::state("IMAP4rev1 UIDPLUS", &[("Trash", 9)]);
        server.lock().unwrap().mailbox("Trash").next_uid = 42;
        server
            .lock()
            .unwrap()
            .add("Trash", "Subject: moved already\r\n\r\nbody", &[]);
        assert!(
            index
                .list_request_snapshot("acct", "Trash", None, false)
                .unwrap()
                .unwrap()
                .is_empty()
        );
        assert!(
            index
                .list_request_snapshot("acct", "Trash", Some(50), true)
                .unwrap()
                .is_none()
        );
        assert!(
            index.progress("acct", "Trash").unwrap().complete,
            "pagination must not invalidate coverage"
        );
        assert!(
            index
                .list_request_snapshot("acct", "Trash", None, true)
                .unwrap()
                .is_none(),
            "force must bypass old complete cache"
        );
        assert!(!index.progress("acct", "Trash").unwrap().complete);
        // The command's None branch can now fetch the actual server UID, not a guessed source UID.
        let mut session = crate::test_imap::session(server.clone()).await;
        session.select("Trash").await.unwrap();
        let (uids, _) = imap::page_uids_matching_limited(&mut session, "", None, 40)
            .await
            .unwrap();
        let items = imap::fetch_summaries(&mut session, &imap::uid_set(&uids), "acct", "Trash")
            .await
            .unwrap();
        assert_eq!(items.iter().map(|m| m.uid).collect::<Vec<_>>(), [42]);
        index_headers_resilient(
            || {
                let server = server.clone();
                async move { Ok(crate::test_imap::session(server).await) }
            },
            &index,
            "acct",
            "Trash",
        )
        .await
        .unwrap();
        assert_eq!(
            index
                .list_request_snapshot("acct", "Trash", None, false)
                .unwrap()
                .unwrap()[0]
                .uid,
            42
        );
    }

    #[test]
    fn confirmed_source_removal_preserves_ready_counts_and_invalidated_coverage() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert(
                "acct",
                "INBOX",
                7,
                &[row(1, "one", "body"), row(2, "two", "body")],
            )
            .unwrap();
        index.save_meta("acct", "INBOX", 7, false, 2, 2, 2).unwrap();
        index.remove_confirmed("acct", "INBOX", 7, 1).unwrap();
        assert_eq!(index.listed("acct", "INBOX").unwrap().unwrap().len(), 1);
        assert_eq!(index.progress("acct", "INBOX").unwrap().total, 1);
        assert_eq!(index.progress("acct", "INBOX").unwrap().body_indexed, 1);
        // Replaying cache cleanup must not deduct another row; stale identities cannot delete new mail.
        index.remove_confirmed("acct", "INBOX", 7, 1).unwrap();
        index.remove_confirmed("acct", "INBOX", 6, 2).unwrap();
        assert_eq!(index.progress("acct", "INBOX").unwrap().total, 1);
        index.invalidate_folder("acct", "INBOX").unwrap();
        index.remove_confirmed("acct", "INBOX", 7, 2).unwrap();
        assert!(index.listed("acct", "INBOX").unwrap().is_none());
        assert!(!index.progress("acct", "INBOX").unwrap().complete);
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
    fn failed_indexing_is_retried_later_with_growing_waits() {
        let minutes = |n| retry_delay(n).as_secs() / 60;
        assert_eq!([1, 2, 3, 4, 10].map(minutes), [1, 5, 15, 30, 30]);
        assert!(worth_retrying("连续重试 8 次都没有进展: 拉取正文超时"));
        let auth = crate::imap::AuthFailed("NO".into()).to_string();
        assert!(!worth_retrying(&auth), "{auth}");
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

    /// 真正下载邮件内容（摘要或正文）的 FETCH 次数；只刷新已读/星标的
    /// `UID FETCH … (UID FLAGS)` 不算，它不重新下载邮件。
    fn fetch_count(server: &crate::test_imap::Shared) -> usize {
        server
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|command| {
                command.starts_with("UID FETCH")
                    && !(command.contains("FLAGS") && !command.contains("BODY"))
            })
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
            .upsert_summaries("acct", "INBOX", validity, &summaries)
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
        index
            .update_bodies("acct", "INBOX", validity, &texts)
            .unwrap();
        assert_eq!(index.search("acct", "INBOX", "正文").unwrap().len(), 1);
        assert!(index.uids_without_body("acct", "INBOX").unwrap().is_empty());
    }

    fn plain_mail(n: u32) -> String {
        format!(
            "From: Sender {n} <s{n}@example.com>\r\nTo: me@example.com\r\n\
             Subject: subject-{n}\r\nDate: Mon, 20 Oct 2025 10:00:00 +0800\r\n\r\nbody-{n}\r\n"
        )
    }

    fn mailbox_with(count: u32) -> crate::test_imap::Shared {
        let server = crate::test_imap::state("", &[("INBOX", 5)]);
        for n in 1..=count {
            server.lock().unwrap().add("INBOX", &plain_mail(n), &[]);
        }
        server
    }

    /// 带自动重连的索引；返回结果和登录次数。
    async fn index_resilient(
        server: &crate::test_imap::Shared,
        index: &SearchIndex,
    ) -> (Result<Progress>, usize) {
        let logins = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = logins.clone();
        let server = server.clone();
        let result = index_folder_resilient(
            move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let server = server.clone();
                async move { Ok(crate::test_imap::session(server).await) }
            },
            index,
            "acct",
            "INBOX",
            |_| {},
        )
        .await;
        (result, logins.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn searches(server: &crate::test_imap::Shared) -> usize {
        server
            .lock()
            .unwrap()
            .commands
            .iter()
            .filter(|c| c.starts_with("UID SEARCH"))
            .count()
    }

    // 邮件多、服务器慢：一批太大就超时。批次要自动缩小，重连后接着索引，最终建完。
    #[tokio::test(start_paused = true)]
    async fn slow_servers_get_smaller_batches_until_indexing_completes() {
        let server = mailbox_with(60);
        server.lock().unwrap().stall_fetch_over = Some(3);
        let index = SearchIndex::in_memory().unwrap();
        let (result, logins) = index_resilient(&server, &index).await;
        let progress = result.expect("超时后应该缩小批次继续，而不是报错");
        assert!(progress.complete);
        assert_eq!((progress.indexed, progress.body_indexed), (60, 60));
        assert_eq!(index.search("acct", "INBOX", "body-42").unwrap().len(), 1);
        assert!(logins > 1, "超时后的连接不能再用，要重新登录");
        assert_eq!(searches(&server), 1, "重连后不必再枚举一遍所有邮件");
    }

    // 某一封的正文怎么都取不回来：跳过它的正文，其他邮件照常建完，它的主题仍可搜。
    #[tokio::test(start_paused = true)]
    async fn a_mail_whose_body_never_arrives_is_skipped() {
        let server = mailbox_with(8);
        server.lock().unwrap().stall_bodies = vec![5];
        let index = SearchIndex::in_memory().unwrap();
        let (result, _) = index_resilient(&server, &index).await;
        let progress = result.expect("一封邮件取不回正文不该让整个索引失败");
        assert!(progress.complete);
        assert_eq!(progress.indexed, 8);
        assert_eq!(index.search("acct", "INBOX", "subject-5").unwrap().len(), 1);
        assert_eq!(index.search("acct", "INBOX", "body-6").unwrap().len(), 1);
        assert!(index.search("acct", "INBOX", "body-5").unwrap().is_empty());
    }

    // 服务器完全不回：重试几次没有任何进展就报错，不能无休止地重连。
    #[tokio::test(start_paused = true)]
    async fn indexing_gives_up_when_nothing_progresses() {
        let server = mailbox_with(4);
        server.lock().unwrap().stall_fetch_over = Some(0);
        let index = SearchIndex::in_memory().unwrap();
        let (result, logins) = index_resilient(&server, &index).await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(err.contains("超时"), "{err}");
        assert!(logins <= 8, "重试次数要有上限：{logins}");
    }

    // 不是超时/断线的错误（比如服务器拒绝命令）照旧直接报错，不重试。
    #[tokio::test(start_paused = true)]
    async fn rejected_commands_are_not_retried() {
        let server = mailbox_with(2);
        server.lock().unwrap().fail.push("SELECT".into());
        let index = SearchIndex::in_memory().unwrap();
        let (result, logins) = index_resilient(&server, &index).await;
        assert!(format!("{:#}", result.unwrap_err()).contains("打开文件夹"));
        assert_eq!(logins, 1);
    }

    // 单独取一封正文失败时：断线不算它的错，超时一次也可能只是服务器那会儿慢，
    // 同一封连续超时两次才跳过它的正文。
    #[tokio::test]
    async fn a_body_is_skipped_only_after_timing_out_twice_on_its_own() {
        async fn timed_out() -> anyhow::Error {
            let elapsed = timeout(Duration::ZERO, std::future::pending::<()>())
                .await
                .unwrap_err();
            anyhow::Error::new(elapsed).context("拉取正文超时")
        }
        let dropped = anyhow::Error::new(std::io::Error::from(std::io::ErrorKind::ConnectionReset));
        let mut run = Run::default();
        assert!(!run.skip_body_after(7, &dropped), "断线不跳过");
        assert!(
            !run.skip_body_after(7, &timed_out().await),
            "第一次超时先重试"
        );
        assert!(
            !run.skip_body_after(8, &timed_out().await),
            "换了一封重新计数"
        );
        assert!(
            run.skip_body_after(8, &timed_out().await),
            "同一封第二次超时才跳过"
        );
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
            receive_epoch: 1,
            connection_revision: 1,
            previous_archive_dirs: Vec::new(),
            id: "probe".into(),
            kind: "custom".into(),
            email,
            name: String::new(),
            signature: String::new(),
            auto_download: Default::default(),
            body_cache: Default::default(),
            archive_dirs: Vec::new(),
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

    #[test]
    fn listed_returns_the_whole_folder_once_headers_are_indexed() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert(
                "acct",
                "INBOX",
                1,
                &[row(1, "旧邮件", "正文"), row(2, "新邮件", "正文")],
            )
            .unwrap();
        // 摘要齐了、正文还没补：列表已经可以直接用本地索引
        index.save_meta("acct", "INBOX", 1, false, 2, 2, 1).unwrap();
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert_eq!(
            items.iter().map(|m| m.uid).collect::<Vec<_>>(),
            [2, 1],
            "按收件时间从新到旧"
        );
        // 还有邮件的摘要没建：继续走服务器分页
        index.save_meta("acct", "INBOX", 1, false, 2, 3, 1).unwrap();
        assert!(index.listed("acct", "INBOX").unwrap().is_none());
        // 正文索引也跑完时同样能列出全部
        index.save_meta("acct", "INBOX", 1, true, 2, 2, 2).unwrap();
        assert_eq!(index.listed("acct", "INBOX").unwrap().unwrap().len(), 2);
        // 别的账号、别的文件夹不串
        assert!(index.listed("other", "INBOX").unwrap().is_none());
        assert!(index.listed("acct", "Sent").unwrap().is_none());
    }

    #[test]
    fn server_flags_refresh_indexed_mail_and_keep_body_search() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert("acct", "INBOX", 1, &[row(1, "报价单", "合同编号 A-123")])
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 1, 1, 1).unwrap();
        index
            .update_flags("acct", "INBOX", &[(1, true, true), (99, false, false)])
            .unwrap();
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert!(items[0].seen && items[0].flagged);
        assert_eq!(
            index.search("acct", "INBOX", "合同编号").unwrap().len(),
            1,
            "标记变化不影响正文搜索"
        );
    }

    #[test]
    fn ui_flags_update_only_the_given_field() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert("acct", "INBOX", 1, &[row(1, "a", "")])
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 1, 1, 1).unwrap();
        index
            .set_flags("acct", "INBOX", &[1], Some(true), None)
            .unwrap();
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert!(items[0].seen && !items[0].flagged);
        index
            .set_flags("acct", "INBOX", &[1], None, Some(true))
            .unwrap();
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert!(items[0].seen && items[0].flagged);
    }

    #[test]
    fn old_background_flags_cannot_overwrite_a_confirmed_user_flag() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert("acct", "INBOX", 1, &[row(1, "a", "")])
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 1, 1, 1).unwrap();
        let mut run = Run::default();
        let background = run.pass(&index, "acct", "INBOX").unwrap();

        // The background FETCH started before the user's STORE was confirmed.
        index
            .set_flags("acct", "INBOX", &[1], None, Some(true))
            .unwrap();
        let delayed =
            background.write(|index| index.update_flags("acct", "INBOX", &[(1, false, false)]));

        assert!(
            delayed.is_err(),
            "a delayed pre-mutation flag snapshot must be discarded"
        );
        assert!(index.listed("acct", "INBOX").unwrap().unwrap()[0].flagged);
    }

    #[test]
    fn upserting_a_summary_keeps_the_indexed_body() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert("acct", "INBOX", 1, &[row(1, "a", "正文关键字")])
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 1, 1, 1).unwrap();
        let mut fresh = row(1, "a", "").0;
        fresh.seen = true;
        index
            .upsert_summaries("acct", "INBOX", 1, &[fresh])
            .unwrap();
        assert_eq!(
            index.search("acct", "INBOX", "正文关键字").unwrap().len(),
            1,
            "重新写入摘要不会清掉正文索引"
        );
        assert!(index.listed("acct", "INBOX").unwrap().unwrap()[0].seen);
    }

    #[tokio::test]
    async fn headers_index_makes_the_list_local_without_downloading_bodies() {
        let server = mailbox_with(3);
        let index = SearchIndex::in_memory().unwrap();
        let shared = server.clone();
        let progress = index_headers_resilient(
            move || {
                let shared = shared.clone();
                async move { Ok(crate::test_imap::session(shared).await) }
            },
            &index,
            "acct",
            "INBOX",
        )
        .await
        .unwrap();
        assert_eq!((progress.indexed, progress.total), (3, 3));
        assert!(!progress.complete, "只建摘要，正文索引等搜索时再补");
        // 列表已经能读本地、按时间从新到旧
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert_eq!(items.iter().map(|m| m.uid).collect::<Vec<_>>(), [3, 2, 1]);
        // 主题能搜，正文还没下
        assert_eq!(index.search("acct", "INBOX", "subject-2").unwrap().len(), 1);
        assert!(index.search("acct", "INBOX", "body-2").unwrap().is_empty());
        let commands = server.lock().unwrap().commands.clone();
        assert!(
            !commands.iter().any(|c| c.contains("BODY.PEEK[]<0.")),
            "不该下载正文：{commands:?}"
        );
    }

    #[test]
    fn summaries_with_broken_encoding_are_refetched_once() {
        let index = SearchIndex::in_memory().unwrap();
        let mut broken = row(1, "主题", "").0;
        broken.from_name = "\u{fffd}\u{fffd}".into();
        index
            .upsert("acct", "INBOX", 1, &[(broken, String::new())])
            .unwrap();
        assert_eq!(
            index.uids_with_broken_encoding("acct", "INBOX").unwrap(),
            [1]
        );
        // 重新取到修好的摘要后不再重拉
        index
            .upsert_summaries("acct", "INBOX", 1, &[row(1, "主题", "").0])
            .unwrap();
        assert!(
            index
                .uids_with_broken_encoding("acct", "INBOX")
                .unwrap()
                .is_empty()
        );
        assert!(
            index
                .uids_with_broken_encoding("acct", "Sent")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn removed_mail_disappears_from_the_local_list() {
        let index = SearchIndex::in_memory().unwrap();
        index
            .upsert(
                "acct",
                "INBOX",
                1,
                &[row(1, "旧邮件", ""), row(2, "新邮件", "")],
            )
            .unwrap();
        index.save_meta("acct", "INBOX", 1, true, 2, 2, 2).unwrap();
        index.remove_uids("acct", "INBOX", &[2]).unwrap();
        let items = index.listed("acct", "INBOX").unwrap().unwrap();
        assert_eq!(items.iter().map(|m| m.uid).collect::<Vec<_>>(), [1]);
    }
}

#[cfg(test)]
#[path = "search_query_tests.rs"]
mod query_tests;

#[cfg(test)]
#[path = "sidebar_count_tests.rs"]
mod sidebar_count_tests;
