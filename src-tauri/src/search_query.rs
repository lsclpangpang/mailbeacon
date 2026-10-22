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
    conn.execute_batch("CREATE TABLE IF NOT EXISTS mail_folder_snapshot (
        account_id TEXT NOT NULL, folder TEXT NOT NULL, name TEXT NOT NULL, role TEXT NOT NULL,
        verified_at INTEGER NOT NULL, PRIMARY KEY(account_id,folder));
        CREATE TABLE IF NOT EXISTS mail_index_revision (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL);
        INSERT OR IGNORE INTO mail_index_revision VALUES(1,0);
        CREATE TRIGGER IF NOT EXISTS mail_index_insert_rev AFTER INSERT ON mail_index BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS mail_index_update_rev AFTER UPDATE ON mail_index BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;
        CREATE TRIGGER IF NOT EXISTS mail_index_delete_rev AFTER DELETE ON mail_index BEGIN UPDATE mail_index_revision SET revision=revision+1 WHERE id=1; END;")?;
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
            let meta:Option<(u32,u32,i64)>=conn.query_row("SELECT uid_validity,total,updated_at FROM mail_index_meta WHERE account_id=?1 AND folder=?2",params![key,target.folder],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
            let validity = meta.filter(|m| m.0 > 0).map(|m| m.0);
            let (indexed,full,prefix,skipped):(u32,u32,u32,u32)=conn.query_row("SELECT COUNT(*),COALESCE(SUM(body_coverage='full'),0),COALESCE(SUM(body_coverage='prefix'),0),COALESCE(SUM(body_coverage='skipped'),0) FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3",params![key,target.folder,validity],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?)))?;
            coverage.push(Coverage {
                account_id: target.account_id.clone(),
                receive_epoch: target.receive_epoch,
                folder: target.folder.clone(),
                uid_validity: validity,
                indexed,
                total: meta.map(|m| m.1),
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
        sql.push_str(
            " ORDER BY date DESC,account_id,folder,uid_validity,uid DESC LIMIT ? OFFSET ?",
        );
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
        let conn = self.lock();
        let json:Option<String>=conn.query_row("SELECT summary FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid],|r|r.get(0)).optional()?;
        let Some(json) = json else {
            bail!("原邮件快照身份已变化，未更新索引");
        };
        let summary: Summary = serde_json::from_str(&json)?;
        conn.execute("UPDATE mail_index SET haystack=?5,has_body=1,body_coverage='full' WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid,haystack(&summary,&text)])?;
        Ok(())
    }
    /// Remember an unavailable prefix separately so skipped mail is never advertised as full text.
    pub fn skip_body(&self, account: &str, folder: &str, validity: u32, uid: u32) -> Result<()> {
        self.lock().execute("UPDATE mail_index SET has_body=1,body_coverage='skipped' WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4 AND body_coverage!='full'",params![account,folder,validity,uid])?;
        Ok(())
    }
}
