//! Incremental MIME metadata. Entries never imply that the source bytes are still cached.
use crate::{
    mail_ref::{MessageRef, namespace},
    search_index::SearchTarget,
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use mailbeacon_parser::MimeHeaders;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

pub const PARSER_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentRef {
    pub source: MessageRef,
    pub part_id: usize,
    pub raw_hash: String,
    pub parser_version: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    #[serde(flatten)]
    pub reference: AttachmentRef,
    pub filename: String,
    pub mime: String,
    pub size: usize,
    pub sender: String,
    pub message_date: i64,
    pub availability: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Query {
    pub targets: Vec<SearchTarget>,
    pub query: String,
    pub kind: String,
    pub sender: String,
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub page_size: usize,
    pub cursor: Option<String>,
}
impl Default for Query {
    fn default() -> Self {
        Self {
            targets: vec![],
            query: String::new(),
            kind: "all".into(),
            sender: String::new(),
            after: None,
            before: None,
            page_size: 100,
            cursor: None,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Coverage {
    pub account_id: String,
    pub receive_epoch: u64,
    pub folder: String,
    pub indexed_messages: u64,
    pub parsed_messages: u64,
    pub attachment_count: u64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    pub items: Vec<Item>,
    pub coverage: Vec<Coverage>,
    pub has_more: bool,
    pub next_cursor: Option<String>,
    pub revision: u64,
    pub catalog_incomplete: Vec<String>,
}
#[derive(Serialize, Deserialize)]
struct Cursor {
    revision: u64,
    mail_revision: u64,
    fingerprint: String,
    offset: usize,
}

/// Hash the exact raw MIME bytes, before parser normalization.
pub fn raw_hash(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Add an independent schema and clean metadata when its exact mail snapshot disappears.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch("CREATE TABLE IF NOT EXISTS attachment_metadata (
      account_id TEXT NOT NULL, receive_epoch INTEGER NOT NULL, storage_account TEXT NOT NULL,
      folder TEXT NOT NULL, uid_validity INTEGER NOT NULL, uid INTEGER NOT NULL, part_id INTEGER NOT NULL,
      filename TEXT NOT NULL,mime TEXT NOT NULL,sender TEXT NOT NULL,message_date INTEGER NOT NULL,item TEXT NOT NULL,
      PRIMARY KEY(account_id,receive_epoch,folder,uid_validity,uid,part_id));
      CREATE TABLE IF NOT EXISTS attachment_sources (
      storage_account TEXT NOT NULL,folder TEXT NOT NULL,uid_validity INTEGER NOT NULL,uid INTEGER NOT NULL,
      raw_hash TEXT NOT NULL,parser_version INTEGER NOT NULL,
      PRIMARY KEY(storage_account,folder,uid_validity,uid));
      CREATE TABLE IF NOT EXISTS attachment_revision (id INTEGER PRIMARY KEY CHECK(id=1),revision INTEGER NOT NULL);
      INSERT OR IGNORE INTO attachment_revision VALUES(1,0);
      CREATE TRIGGER IF NOT EXISTS attachment_mail_removed AFTER DELETE ON mail_index BEGIN
      DELETE FROM attachment_metadata WHERE storage_account=OLD.account_id AND folder=OLD.folder AND uid_validity=OLD.uid_validity AND uid=OLD.uid;
      DELETE FROM attachment_sources WHERE storage_account=OLD.account_id AND folder=OLD.folder AND uid_validity=OLD.uid_validity AND uid=OLD.uid;
      UPDATE attachment_revision SET revision=revision+1 WHERE id=1; END;
      INSERT OR IGNORE INTO productivity_migrations VALUES ('attachment-metadata-v1');")?;
    Ok(())
}

/// Parse ordinary attachments while retaining the parser's original index for existing actions.
pub fn parse(source: &MessageRef, raw: &[u8], fallback_date: i64) -> Result<Vec<Item>> {
    source.validate()?;
    if raw.len() > crate::body_store::MAX_FILE {
        bail!("邮件超过附件元数据解析上限");
    }
    let fixed = crate::mime::fix_headers(raw);
    let message = crate::mime::parse(&fixed)?;
    let hash = raw_hash(raw);
    let sender = message
        .from()
        .map(|a| {
            a.iter()
                .map(|p| {
                    format!(
                        "{} <{}>",
                        p.name.as_deref().unwrap_or(""),
                        p.address.as_deref().unwrap_or("")
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let date = message
        .date()
        .map(|d| d.to_timestamp() * 1000)
        .unwrap_or(fallback_date);
    let mut items = Vec::new();
    for (part_id, part) in message.attachments().enumerate() {
        let disposition = part.content_disposition().map(|d| d.ctype());
        if disposition.is_some_and(|d| d.eq_ignore_ascii_case("inline"))
            || (part.content_id().is_some()
                && !disposition.is_some_and(|d| d.eq_ignore_ascii_case("attachment")))
        {
            continue;
        }
        let filename = part
            .attachment_name()
            .map(str::to_string)
            .unwrap_or_else(|| format!("附件{}", part_id + 1));
        let mime = part
            .content_type()
            .map(|c| format!("{}/{}", c.ctype(), c.subtype().unwrap_or("octet-stream")))
            .unwrap_or_else(|| "application/octet-stream".into());
        items.push(Item {
            reference: AttachmentRef {
                source: source.clone(),
                part_id,
                raw_hash: hash.clone(),
                parser_version: PARSER_VERSION,
            },
            filename,
            mime,
            size: part.len(),
            sender: sender.clone(),
            message_date: date,
            availability: "unknown".into(),
        });
    }
    Ok(items)
}

/// Replace one proven message's metadata inside the caller's database transaction.
pub fn replace(
    conn: &Connection,
    source: &MessageRef,
    raw: &[u8],
    fallback_date: i64,
) -> Result<()> {
    let items = parse(source, raw, fallback_date)?;
    let key = namespace(&source.account_id, source.receive_epoch);
    let hash = raw_hash(raw);
    let exists:bool=conn.query_row("SELECT EXISTS(SELECT 1 FROM mail_index i JOIN mail_index_meta m ON i.account_id=m.account_id AND i.folder=m.folder AND i.uid_validity=m.uid_validity WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity=?3 AND i.uid=?4)",params![key,source.folder,source.uid_validity,source.uid],|r|r.get(0))?;
    if !exists {
        bail!("原邮件快照已失效，未建立附件索引");
    }
    let previous:Option<(String,u32)>=conn.query_row("SELECT raw_hash,parser_version FROM attachment_sources WHERE storage_account=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if previous
        .as_ref()
        .is_some_and(|(h, v)| h == &hash && *v == PARSER_VERSION)
    {
        return Ok(());
    }
    conn.execute("DELETE FROM attachment_metadata WHERE storage_account=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![key,source.folder,source.uid_validity,source.uid])?;
    for item in &items {
        conn.execute(
            "INSERT INTO attachment_metadata VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            params![
                source.account_id,
                source.receive_epoch,
                key,
                source.folder,
                source.uid_validity,
                source.uid,
                item.reference.part_id as i64,
                item.filename,
                item.mime,
                item.sender,
                item.message_date,
                serde_json::to_string(item)?
            ],
        )?;
    }
    conn.execute(
        "INSERT OR REPLACE INTO attachment_sources VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            key,
            source.folder,
            source.uid_validity,
            source.uid,
            hash,
            PARSER_VERSION
        ],
    )?;
    conn.execute(
        "UPDATE attachment_revision SET revision=revision+1 WHERE id=1",
        [],
    )?;
    Ok(())
}

/// Validate source/part/content together before any preview or export reads decoded bytes.
pub fn verified_bytes(reference: &AttachmentRef, raw: &[u8]) -> Result<(String, String, Vec<u8>)> {
    reference.source.validate()?;
    if reference.parser_version != PARSER_VERSION || raw_hash(raw) != reference.raw_hash {
        bail!("附件原文或解析版本已改变，请刷新附件列表");
    }
    let items = parse(&reference.source, raw, 0)?;
    let item = items
        .into_iter()
        .find(|i| i.reference.part_id == reference.part_id)
        .context("普通附件不存在，请刷新列表")?;
    let (_, bytes) = crate::imap::attachment_bytes(raw, reference.part_id)?;
    Ok((item.filename, item.mime, bytes))
}

/// Query only parsed metadata; this function performs no network or filesystem reads.
pub fn query(conn: &Connection, input: &Query) -> Result<Page> {
    let mut normalized = input.clone();
    normalized.cursor = None;
    normalized.targets.sort();
    normalized.targets.dedup();
    normalized.page_size = normalized.page_size.clamp(1, 200);
    if normalized.targets.len() > 200
        || normalized.targets.iter().any(|t| {
            t.account_id.is_empty()
                || t.receive_epoch == 0
                || t.folder.is_empty()
                || t.folder.starts_with("__")
        })
    {
        bail!("附件范围无效或超过200个文件夹");
    }
    if !matches!(
        normalized.kind.as_str(),
        "all" | "pdf" | "image" | "spreadsheet" | "other"
    ) {
        bail!("附件类型筛选无效");
    }
    let revision: u64 = conn.query_row(
        "SELECT revision FROM attachment_revision WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let mail_revision: u64 = conn.query_row(
        "SELECT revision FROM mail_index_revision WHERE id=1",
        [],
        |r| r.get(0),
    )?;
    let fingerprint = raw_hash(&serde_json::to_vec(&normalized)?);
    let offset = if let Some(cursor) = &input.cursor {
        let old: Cursor = serde_json::from_slice(
            &URL_SAFE_NO_PAD
                .decode(cursor)
                .context("附件分页已失效，请刷新")?,
        )
        .context("附件分页已失效，请刷新")?;
        if old.revision != revision
            || old.mail_revision != mail_revision
            || old.fingerprint != fingerprint
        {
            bail!("附件索引或条件已改变，请刷新");
        }
        old.offset
    } else {
        0
    };
    let mut coverage = Vec::new();
    let mut clauses = Vec::new();
    let mut values: Vec<rusqlite::types::Value> = Vec::new();
    for target in &normalized.targets {
        let key = namespace(&target.account_id, target.receive_epoch);
        let counts:(u64,u64)=conn.query_row("SELECT COUNT(*),COALESCE(SUM(EXISTS(SELECT 1 FROM attachment_sources s WHERE s.storage_account=i.account_id AND s.folder=i.folder AND s.uid_validity=i.uid_validity AND s.uid=i.uid AND s.parser_version=?3)),0) FROM mail_index i JOIN mail_index_meta m ON i.account_id=m.account_id AND i.folder=m.folder AND i.uid_validity=m.uid_validity WHERE i.account_id=?1 AND i.folder=?2",params![key,target.folder,PARSER_VERSION],|r|Ok((r.get(0)?,r.get(1)?)))?;
        let count:u64=conn.query_row("SELECT COUNT(*) FROM attachment_metadata a JOIN mail_index_meta m ON a.storage_account=m.account_id AND a.folder=m.folder AND a.uid_validity=m.uid_validity WHERE a.storage_account=?1 AND a.folder=?2",params![key,target.folder],|r|r.get(0))?;
        coverage.push(Coverage {
            account_id: target.account_id.clone(),
            receive_epoch: target.receive_epoch,
            folder: target.folder.clone(),
            indexed_messages: counts.0,
            parsed_messages: counts.1,
            attachment_count: count,
        });
        clauses.push("(a.storage_account=? AND a.folder=?)".to_string());
        values.extend([key.into(), target.folder.clone().into()]);
    }
    if clauses.is_empty() {
        return Ok(Page {
            items: vec![],
            coverage,
            has_more: false,
            next_cursor: None,
            revision,
            catalog_incomplete: vec![],
        });
    }
    let mut sql = format!(
        "SELECT a.item FROM attachment_metadata a JOIN mail_index_meta m ON a.storage_account=m.account_id AND a.folder=m.folder AND a.uid_validity=m.uid_validity WHERE ({})",
        clauses.join(" OR ")
    );
    for (field, value) in [
        ("filename", &normalized.query),
        ("sender", &normalized.sender),
    ] {
        if !value.trim().is_empty() {
            sql.push_str(&format!(" AND instr(lower(a.{field}),?)>0"));
            values.push(value.trim().to_lowercase().into());
        }
    }
    match normalized.kind.as_str(){"pdf"=>sql.push_str(" AND lower(a.mime)='application/pdf'"),"image"=>sql.push_str(" AND lower(a.mime) LIKE 'image/%'"),"spreadsheet"=>sql.push_str(" AND (lower(a.mime) LIKE '%spreadsheet%' OR lower(a.mime) LIKE '%excel%' OR lower(a.mime)='text/csv')"),"other"=>sql.push_str(" AND lower(a.mime) NOT LIKE 'image/%' AND lower(a.mime)!='application/pdf' AND lower(a.mime) NOT LIKE '%spreadsheet%' AND lower(a.mime) NOT LIKE '%excel%' AND lower(a.mime)!='text/csv'"),_=>{}}
    if let Some(v) = normalized.after {
        sql.push_str(" AND a.message_date>=?");
        values.push(v.into());
    }
    if let Some(v) = normalized.before {
        sql.push_str(" AND a.message_date<?");
        values.push(v.into());
    }
    sql.push_str(" ORDER BY a.message_date DESC,a.storage_account,a.folder,a.uid_validity,a.uid DESC,a.part_id LIMIT ? OFFSET ?");
    values.push(((normalized.page_size + 1) as i64).into());
    values.push(i64::try_from(offset)?.into());
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), |r| {
        r.get::<_, String>(0)
    })?;
    let mut items = Vec::new();
    for row in rows {
        items.push(serde_json::from_str::<Item>(&row?).context("附件索引损坏，请重新解析邮件")?);
    }
    let has_more = items.len() > normalized.page_size;
    items.truncate(normalized.page_size);
    let next_cursor = if has_more {
        Some(URL_SAFE_NO_PAD.encode(serde_json::to_vec(&Cursor {
            revision,
            mail_revision,
            fingerprint,
            offset: offset + items.len(),
        })?))
    } else {
        None
    };
    Ok(Page {
        items,
        coverage,
        has_more,
        next_cursor,
        revision,
        catalog_incomplete: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    pub fn source() -> MessageRef {
        MessageRef {
            account_id: "a".into(),
            receive_epoch: 2,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 1,
        }
    }
    pub fn raw() -> Vec<u8> {
        b"From: sender@example.test\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nhello\r\n--x\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=same.png\r\n\r\nfirst\r\n--x\r\nContent-Type: image/png\r\nContent-Disposition: inline; filename=signature.png\r\nContent-ID: <sig>\r\n\r\ninline\r\n--x\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=same.png\r\n\r\nsecond\r\n--x--\r\n".to_vec()
    }
    #[test]
    fn same_name_parts_keep_identity_and_inline_is_excluded() {
        let items = parse(&source(), &raw(), 42).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].filename, items[1].filename);
        assert_ne!(items[0].reference.part_id, items[1].reference.part_id);
        assert_eq!(items[0].reference.source.receive_epoch, 2);
    }
    #[test]
    fn changed_raw_or_parser_never_reuses_old_part() {
        let bytes = raw();
        let mut r = parse(&source(), &bytes, 0).unwrap().remove(0).reference;
        assert!(verified_bytes(&r, &bytes).is_ok());
        let mut changed = bytes.clone();
        changed.extend_from_slice(b"changed");
        assert!(verified_bytes(&r, &changed).is_err());
        r.parser_version += 1;
        assert!(verified_bytes(&r, &bytes).is_err());
    }
}
