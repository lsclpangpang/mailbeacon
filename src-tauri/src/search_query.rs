//! Query/snapshot contract shared by online search and offline reading.
use super::*;
use crate::mail_ref::{MessageRef, namespace};
use anyhow::bail;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub struct SearchTarget {
    pub account_id: String,
    pub receive_epoch: u64,
    pub folder: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchRequest {
    pub query: String,
    pub targets: Vec<SearchTarget>,
    pub unread_only: bool,
    pub flagged_only: bool,
    pub attachment_only: bool,
    pub sender: String,
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub page_size: usize,
    pub cursor: Option<String>,
    pub sort: String,
}
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub account_id: String,
    pub receive_epoch: u64,
    pub folder: String,
    pub uid_validity: Option<u32>,
    pub indexed: u32,
    pub total: Option<u32>,
    pub full: u32,
    pub prefix: u32,
    pub skipped: u32,
    pub snapshot_at: Option<i64>,
    pub error: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<Summary>,
    pub coverage: Vec<Coverage>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub revision: u64,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    revision: u64,
    query: String,
    offset: usize,
}

pub(super) fn init(conn: &Connection) -> Result<()> {
    let columns: Vec<String> = conn
        .prepare("PRAGMA table_info(mail_index)")?
        .query_map([], |row| row.get(1))?
        .collect::<rusqlite::Result<_>>()?;
    if !columns.iter().any(|c| c == "body_coverage") {
        conn.execute(
            "ALTER TABLE mail_index ADD COLUMN body_coverage TEXT NOT NULL DEFAULT 'none'",
            [],
        )?;
        // Previous has_body only certifies that a prefix attempt finished, never full text.
        conn.execute(
            "UPDATE mail_index SET body_coverage='prefix' WHERE has_body=1",
            [],
        )?;
    }
    if !columns.iter().any(|c| c == "local_checked") {
        conn.execute(
            "ALTER TABLE mail_index ADD COLUMN local_checked INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mail_folder_snapshot (
        account_id TEXT NOT NULL, folder TEXT NOT NULL, name TEXT NOT NULL, role TEXT NOT NULL,
        verified_at INTEGER NOT NULL, PRIMARY KEY(account_id,folder));
        CREATE TABLE IF NOT EXISTS mail_index_revision (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO mail_index_revision VALUES(1,0);
        CREATE TRIGGER IF NOT EXISTS mail_index_insert_rev AFTER INSERT ON mail_index BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;
        DROP TRIGGER IF EXISTS mail_index_update_rev;
        CREATE TRIGGER mail_index_update_rev AFTER UPDATE OF summary,haystack,body_coverage ON mail_index WHEN OLD.summary != NEW.summary OR OLD.haystack != NEW.haystack OR OLD.body_coverage != NEW.body_coverage BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS mail_index_delete_rev AFTER DELETE ON mail_index BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;")?;
    // This connection owns every background index writer. A process-local tombstone
    // blocks stale jobs without leaving permanent restrictions on future app runs.
    conn.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS removed_mail_accounts (account_id TEXT PRIMARY KEY);",
    )?;
    for table in ["mail_index", "mail_index_meta", "mail_folder_snapshot"] {
        conn.execute_batch(&format!("CREATE TEMP TRIGGER IF NOT EXISTS block_removed_{table} BEFORE INSERT ON main.{table}
            WHEN EXISTS(SELECT 1 FROM removed_mail_accounts r WHERE NEW.account_id = r.account_id OR substr(NEW.account_id,1,length(r.account_id || char(31) || 'receive:')) = r.account_id || char(31) || 'receive:')
            BEGIN SELECT RAISE(ABORT,'account removed; stale index write refused'); END;"))?;
    }
    let migration = conn.unchecked_transaction()?;
    crate::saved_views::init(&migration)?;
    crate::attachment_index::init(&migration)?;
    migration.commit()?;
    Ok(())
}

fn identity(storage: &str) -> (&str, u64) {
    storage
        .rsplit_once('\u{1f}')
        .and_then(|(id, suffix)| {
            suffix
                .strip_prefix("receive:")?
                .parse()
                .ok()
                .map(|epoch| (id, epoch))
        })
        .unwrap_or((storage, 1))
}
pub(super) fn stamp(summary: &mut Summary, storage: &str, validity: u32) {
    let (id, epoch) = identity(storage);
    summary.account_id = id.into();
    summary.receive_epoch = epoch;
    summary.uid_validity = validity;
}

impl SearchIndex {
    /// Persist the proven generation without claiming that a partial list is complete.
    pub fn has_folder_catalog(&self, account: &str, epoch: u64) -> Result<bool> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM mail_folder_snapshot WHERE account_id=?1)",
            [namespace(account, epoch)],
            |r| r.get(0),
        )?)
    }
    pub fn mark_local_checked(&self, source: &MessageRef) -> Result<()> {
        self.lock().execute("UPDATE mail_index SET local_checked=?5 WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4", params![namespace(&source.account_id, source.receive_epoch), source.folder, source.uid_validity, source.uid, chrono::Utc::now().timestamp_millis()])?;
        Ok(())
    }
    pub fn record_snapshot(&self, account: &str, folder: &str, validity: u32) -> Result<()> {
        if validity == 0 {
            bail!("服务器未确认文件夹代际，不能建立可读快照");
        }
        self.begin_folder(account, folder, validity)?;
        Ok(())
    }
    /// Check every component before giving an offline reader access to cached MIME bytes.
    pub fn snapshot_reference(&self, source: &MessageRef) -> Result<(Option<i64>, bool)> {
        source.validate()?;
        let key = namespace(&source.account_id, source.receive_epoch);
        let conn = self.lock();
        let row:Option<(Option<i64>,String)>=conn.query_row("SELECT m.updated_at,i.body_coverage FROM mail_index i JOIN mail_index_meta m ON i.account_id=m.account_id AND i.folder=m.folder AND i.uid_validity=m.uid_validity WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity=?3 AND i.uid=?4",params![key,source.folder,source.uid_validity,source.uid],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        row.map(|(at, coverage)| (at, coverage == "full"))
            .ok_or_else(|| anyhow::anyhow!("该邮件的本地快照身份无法确认，请联网刷新后重试"))
    }
    /// Return an exact, generation-checked summary even when the folder list is incomplete.
    pub fn snapshot_summary(&self, source: &MessageRef) -> Result<Summary> {
        source.validate()?;
        let key = namespace(&source.account_id, source.receive_epoch);
        let raw: Option<String> = self.lock().query_row("SELECT i.summary FROM mail_index i JOIN mail_index_meta m ON i.account_id=m.account_id AND i.folder=m.folder AND i.uid_validity=m.uid_validity WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity=?3 AND i.uid=?4", params![key,source.folder,source.uid_validity,source.uid], |row| row.get(0)).optional()?;
        let mut summary: Summary =
            serde_json::from_str(&raw.ok_or_else(|| {
                anyhow::anyhow!("该邮件的本地快照身份无法确认，请联网刷新后重试")
            })?)?;
        stamp(&mut summary, &key, source.uid_validity);
        Ok(summary)
    }
    /// Known identities whose complete local MIME may improve prefix-only search coverage.
    pub fn incomplete_references(
        &self,
        target: &SearchTarget,
        limit: usize,
    ) -> Result<Vec<MessageRef>> {
        let key = namespace(&target.account_id, target.receive_epoch);
        let conn = self.lock();
        let mut stmt=conn.prepare("SELECT i.uid,i.uid_validity FROM mail_index i JOIN mail_index_meta m ON i.account_id=m.account_id AND i.folder=m.folder AND i.uid_validity=m.uid_validity WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity>0 AND i.body_coverage!='full' ORDER BY i.local_checked ASC,i.date DESC,i.uid DESC LIMIT ?3")?;
        Ok(stmt
            .query_map(params![key, target.folder, limit as i64], |r| {
                Ok(MessageRef {
                    account_id: target.account_id.clone(),
                    receive_epoch: target.receive_epoch,
                    folder: target.folder.clone(),
                    uid: r.get(0)?,
                    uid_validity: r.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    /// Persist successfully fetched folder names/roles. Network errors never replace this snapshot.
    pub fn save_folders(
        &self,
        account_id: &str,
        epoch: u64,
        folders: &[imap::Folder],
    ) -> Result<()> {
        let key = namespace(account_id, epoch);
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "DELETE FROM mail_folder_snapshot WHERE account_id=?1",
            [&key],
        )?;
        for folder in folders {
            tx.execute(
                "INSERT INTO mail_folder_snapshot VALUES(?1,?2,?3,?4,?5)",
                params![
                    key,
                    folder.path,
                    folder.name,
                    folder.role,
                    chrono::Utc::now().timestamp_millis()
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Return folder metadata without contacting the server, including partially indexed folders.
    pub fn saved_folders(&self, account_id: &str, epoch: u64) -> Result<Vec<imap::Folder>> {
        let key = namespace(account_id, epoch);
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT folder,name,role FROM mail_folder_snapshot WHERE account_id=?1 ORDER BY folder",
        )?;
        let mut rows = stmt
            .query_map([&key], |row| {
                Ok(imap::Folder {
                    path: row.get(0)?,
                    name: row.get(1)?,
                    role: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut stmt=conn.prepare("SELECT folder FROM mail_index_meta WHERE account_id=?1 AND uid_validity>0 ORDER BY folder")?;
        for folder in stmt.query_map([&key], |r| r.get::<_, String>(0))? {
            let folder = folder?;
            if !rows.iter().any(|f| f.path == folder) {
                rows.push(imap::Folder {
                    name: imap::decode_mutf7(&folder),
                    role: if folder.eq_ignore_ascii_case("INBOX") {
                        "inbox"
                    } else {
                        "other"
                    }
                    .into(),
                    path: folder,
                });
            }
        }
        Ok(rows)
    }
    /// Read persisted list rows regardless of whether the initial enumeration has finished.
    pub fn snapshot_page(
        &self,
        target: &SearchTarget,
        cursor: Option<String>,
        page_size: usize,
    ) -> Result<SearchPage> {
        self.search_page(&SearchRequest {
            targets: vec![target.clone()],
            cursor,
            page_size,
            ..Default::default()
        })
    }
    /// Query exact receive namespaces with explicit coverage and revision-bound pagination.
    pub fn search_page(&self, request: &SearchRequest) -> Result<SearchPage> {
        let mut normalized = request.clone();
        normalized.cursor = None;
        normalized.targets.sort();
        normalized.targets.dedup();
        normalized.page_size = normalized.page_size.clamp(1, 200);
        normalized.query = normalized.query.trim().to_lowercase();
        if normalized.sort.is_empty() {
            normalized.sort = "dateDesc".into();
        }
        if !matches!(normalized.sort.as_str(), "dateAsc" | "dateDesc") {
            bail!("搜索排序无效");
        }
        if normalized
            .after
            .zip(normalized.before)
            .is_some_and(|(a, b)| a >= b)
        {
            bail!("搜索日期范围无效");
        }
        if normalized.targets.len() > 200 {
            bail!("本次范围超过 200 个文件夹，请缩小搜索范围");
        }
        if normalized
            .targets
            .iter()
            .any(|t| t.account_id.is_empty() || t.receive_epoch == 0 || t.folder.is_empty())
        {
            bail!("搜索范围包含未确认的邮箱身份");
        }
        let fingerprint = URL_SAFE_NO_PAD.encode(
            ring::digest::digest(&ring::digest::SHA256, &serde_json::to_vec(&normalized)?).as_ref(),
        );
        let conn = self.lock();
        let revision = conn.query_row(
            "SELECT revision FROM mail_index_revision WHERE id=1",
            [],
            |r| r.get::<_, u64>(0),
        )?;
        let offset = if let Some(cursor) = &request.cursor {
            let cursor: Cursor = serde_json::from_slice(
                &URL_SAFE_NO_PAD
                    .decode(cursor)
                    .context("搜索分页已失效，请刷新结果")?,
            )
            .context("搜索分页已失效，请刷新结果")?;
            if cursor.revision != revision || cursor.query != fingerprint {
                bail!("搜索结果已有变化，请刷新后继续浏览");
            }
            cursor.offset
        } else {
            0
        };
        let mut coverage = Vec::new();
        let mut clauses = Vec::new();
        let mut values: Vec<rusqlite::types::Value> = Vec::new();
        for target in &normalized.targets {
            let key = namespace(&target.account_id, target.receive_epoch);
            let meta:Option<(u32,u32,i64,bool)>=conn.query_row("SELECT uid_validity,total,updated_at,complete FROM mail_index_meta WHERE account_id=?1 AND folder=?2",params![key,target.folder],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
            let validity = meta.filter(|m| m.0 > 0).map(|m| m.0);
            let (indexed,full,prefix,skipped):(u32,u32,u32,u32)=conn.query_row("SELECT COUNT(*),COALESCE(SUM(body_coverage='full'),0),COALESCE(SUM(body_coverage='prefix'),0),COALESCE(SUM(body_coverage='skipped'),0) FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3",params![key,target.folder,validity],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            coverage.push(Coverage {
                account_id: target.account_id.clone(),
                receive_epoch: target.receive_epoch,
                folder: target.folder.clone(),
                uid_validity: validity,
                indexed,
                total: meta.filter(|m| m.1 > 0 || m.3).map(|m| m.1),
                full,
                prefix,
                skipped,
                snapshot_at: meta.map(|m| m.2),
                error: None,
            });
            clauses.push("(account_id=? AND folder=? AND uid_validity=?)".to_string());
            values.extend([
                key.into(),
                target.folder.clone().into(),
                validity
                    .map(|v| (v as i64).into())
                    .unwrap_or(rusqlite::types::Value::Null),
            ]);
        }
        if clauses.is_empty() {
            return Ok(SearchPage {
                items: vec![],
                coverage,
                has_more: false,
                next_cursor: None,
                revision,
            });
        }
        let mut sql = format!(
            "SELECT summary,account_id,uid_validity FROM mail_index WHERE ({}) AND instr(haystack,?)>0",
            clauses.join(" OR ")
        );
        values.push(normalized.query.clone().into());
        if normalized.unread_only {
            sql.push_str(" AND json_extract(summary,'$.seen')=0");
        }
        if normalized.flagged_only {
            sql.push_str(" AND json_extract(summary,'$.flagged')=1");
        }
        if normalized.attachment_only {
            sql.push_str(" AND json_extract(summary,'$.attachment')=1");
        }
        if !normalized.sender.trim().is_empty() {
            sql.push_str(" AND instr(lower(json_extract(summary,'$.fromName')||' '||json_extract(summary,'$.fromAddr')),?)>0");
            values.push(normalized.sender.trim().to_lowercase().into());
        }
        if let Some(date) = normalized.after {
            sql.push_str(" AND date>=?");
            values.push(date.into());
        }
        if let Some(date) = normalized.before {
            sql.push_str(" AND date<?");
            values.push(date.into());
        }
        sql.push_str(if normalized.sort == "dateAsc" {
            " ORDER BY date ASC,account_id,folder,uid_validity,uid ASC LIMIT ? OFFSET ?"
        } else {
            " ORDER BY date DESC,account_id,folder,uid_validity,uid DESC LIMIT ? OFFSET ?"
        });
        values.push(((normalized.page_size + 1) as i64).into());
        values.push(i64::try_from(offset)?.into());
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })?;
        let mut items = Vec::new();
        for row in rows {
            let (json, key, validity) = row?;
            let mut summary: Summary =
                serde_json::from_str(&json).context("本地摘要损坏，请重新建立索引")?;
            stamp(&mut summary, &key, validity);
            items.push(summary);
        }
        let has_more = items.len() > normalized.page_size;
        items.truncate(normalized.page_size);
        let next_cursor = if has_more {
            Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
                revision,
                query: fingerprint,
                offset: offset + items.len(),
            })?))
        } else {
            None
        };
        Ok(SearchPage {
            items,
            coverage,
            has_more,
            next_cursor,
            revision,
        })
    }
    /// Upgrade searchable text from a fully cached MIME message; never parse attachment bodies.
    pub fn index_full_body(&self, source: &MessageRef, raw: &[u8]) -> Result<()> {
        source.validate()?;
        if raw.len() > crate::body_store::MAX_FILE {
            bail!("邮件超过本地全文索引大小上限");
        }
        let fixed = crate::mime::fix_headers(raw);
        let parsed = crate::mime::parse(&fixed)?;
        let mut text = String::new();
        for n in 0..parsed.text_body_count().max(parsed.html_body.len()) {
            if let Some(body) = parsed.body_text(n) {
                text.push_str(&body);
                text.push('\n');
            }
        }
        let key = namespace(&source.account_id, source.receive_epoch);
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let json:Option<String>=tx.query_row("SELECT summary FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid],|r|r.get(0)).optional()?;
        let Some(json) = json else {
            bail!("原邮件快照身份已变化，未更新索引");
        };
        let summary: Summary = serde_json::from_str(&json)?;
        tx.execute("UPDATE mail_index SET haystack=?5,has_body=1,body_coverage='full' WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid,haystack(&summary,&text)])?;
        crate::attachment_index::replace(&tx, source, raw, summary.date)?;
        tx.commit()?;
        Ok(())
    }

    /// Read durable smart folder definitions without resolving them against UI selection.
    pub fn saved_views(&self) -> Result<Vec<crate::saved_views::SavedView>> {
        crate::saved_views::list(&self.lock())
    }
    /// Persist one explicit smart folder edit using a revision guard.
    pub fn save_view(
        &self,
        id: Option<String>,
        name: String,
        position: i64,
        spec: serde_json::Value,
        expected_revision: Option<u64>,
    ) -> Result<crate::saved_views::SavedView> {
        crate::saved_views::save(
            &mut self.lock(),
            id,
            name,
            position,
            spec,
            expected_revision,
        )
    }
    /// Delete only a smart folder definition.
    pub fn delete_view(&self, id: &str, revision: u64) -> Result<()> {
        crate::saved_views::delete(&self.lock(), id, revision)
    }
    /// Validate a folder against the last successful LIST, without adding stale index folders.
    pub fn catalog_contains(&self, target: &SearchTarget) -> Result<bool> {
        Ok(self.lock().query_row(
            "SELECT EXISTS(SELECT 1 FROM mail_folder_snapshot WHERE account_id=?1 AND folder=?2)",
            params![
                namespace(&target.account_id, target.receive_epoch),
                target.folder
            ],
            |r| r.get(0),
        )?)
    }
    /// Return only folders from the last successful LIST, excluding stale indexed folders.
    pub fn catalog_folders(&self, account: &str, epoch: u64) -> Result<Vec<imap::Folder>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT folder,name,role FROM mail_folder_snapshot WHERE account_id=?1 ORDER BY folder",
        )?;
        Ok(stmt
            .query_map([namespace(account, epoch)], |r| {
                Ok(imap::Folder {
                    path: r.get(0)?,
                    name: r.get(1)?,
                    role: r.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }
    /// Query metadata only. Opening the attachment center never downloads mailbox bodies.
    pub fn attachment_page(
        &self,
        input: &crate::attachment_index::Query,
    ) -> Result<crate::attachment_index::Page> {
        crate::attachment_index::query(&self.lock(), input)
    }
    /// Incrementally index already-obtained complete MIME without touching server flags.
    pub fn index_attachments(&self, source: &MessageRef, raw: &[u8]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        crate::attachment_index::replace(&tx, source, raw, 0)?;
        tx.commit()?;
        Ok(())
    }
    /// Remember an unavailable prefix separately so skipped mail is never advertised as full text.
    pub fn skip_body(&self, account: &str, folder: &str, validity: u32, uid: u32) -> Result<()> {
        self.lock().execute("UPDATE mail_index SET has_body=1,body_coverage='skipped' WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4 AND body_coverage!='full'",params![account,folder,validity,uid])?;
        Ok(())
    }
}
