//! 通讯录：联系人、分组、最近联系人，以及 vCard / CSV 导入导出。

use std::{path::Path, sync::Mutex};

use anyhow::{Context, Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

/// 当前数据库结构版本（PRAGMA user_version）。
pub const SCHEMA_VERSION: i64 = 3;

/// 第 1 版结构。
const V1_SCHEMA: &str = r#"
    CREATE TABLE contacts (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL DEFAULT '',
        org TEXT NOT NULL DEFAULT '',
        note TEXT NOT NULL DEFAULT '',
        updated_at INTEGER NOT NULL DEFAULT 0
    );
    CREATE TABLE contact_emails (
        contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
        email TEXT NOT NULL COLLATE NOCASE UNIQUE,
        position INTEGER NOT NULL
    );
    CREATE INDEX contact_emails_contact ON contact_emails(contact_id);
    CREATE TABLE contact_groups (
        id INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE group_members (
        group_id INTEGER NOT NULL REFERENCES contact_groups(id) ON DELETE CASCADE,
        contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        PRIMARY KEY (group_id, contact_id)
    );
    CREATE TABLE recent_addresses (
        email TEXT PRIMARY KEY COLLATE NOCASE,
        name TEXT NOT NULL DEFAULT '',
        sent INTEGER NOT NULL DEFAULT 0,
        received INTEGER NOT NULL DEFAULT 0,
        last_used INTEGER NOT NULL DEFAULT 0
    );
"#;

/// 邮箱判重用的键。SQLite 的 NOCASE 和 lower() 只处理 ASCII，这里用 Rust 做完整的小写转换。
fn email_key(email: &str) -> String {
    email.trim().to_lowercase()
}

/// 第 2 版：邮箱按完整 Unicode 小写判重（新增 email_key 列）。
fn migrate_v2(tx: &Transaction) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE contact_emails RENAME TO contact_emails_v1;
         DROP INDEX IF EXISTS contact_emails_contact;
         CREATE TABLE contact_emails (
             contact_id INTEGER NOT NULL REFERENCES contacts(id) ON DELETE CASCADE,
             email TEXT NOT NULL,
             email_key TEXT NOT NULL UNIQUE,
             position INTEGER NOT NULL
         );
         CREATE INDEX contact_emails_contact ON contact_emails(contact_id);
         ALTER TABLE recent_addresses RENAME TO recent_addresses_v1;
         CREATE TABLE recent_addresses (
             email_key TEXT PRIMARY KEY,
             email TEXT NOT NULL,
             name TEXT NOT NULL DEFAULT '',
             sent INTEGER NOT NULL DEFAULT 0,
             received INTEGER NOT NULL DEFAULT 0,
             last_used INTEGER NOT NULL DEFAULT 0
         );",
    )?;
    let emails: Vec<(i64, String, i64)> = tx
        .prepare("SELECT contact_id, email, position FROM contact_emails_v1 ORDER BY contact_id, position")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (contact, email, position) in emails {
        // 旧数据里只差非 ASCII 大小写的重复地址，保留先出现的那个
        tx.execute(
            "INSERT OR IGNORE INTO contact_emails (contact_id, email, email_key, position) VALUES (?, ?, ?, ?)",
            params![contact, email, email_key(&email), position],
        )?;
    }
    let recent: Vec<(String, String, i64, i64, i64)> = tx
        .prepare("SELECT email, name, sent, received, last_used FROM recent_addresses_v1")?
        .query_map([], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    for (email, name, sent, received, last_used) in recent {
        tx.execute(
            "INSERT INTO recent_addresses (email_key, email, name, sent, received, last_used) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(email_key) DO UPDATE SET sent = sent + ?4, received = received + ?5, last_used = MAX(last_used, ?6)",
            params![email_key(&email), email, name, sent, received, last_used],
        )?;
    }
    tx.execute_batch("DROP TABLE contact_emails_v1; DROP TABLE recent_addresses_v1;")?;
    Ok(())
}

/// 把数据库从第 to-1 版升到第 to 版。
fn migrate(tx: &Transaction, to: i64) -> Result<()> {
    match to {
        1 => Ok(tx.execute_batch(V1_SCHEMA)?),
        2 => migrate_v2(tx),
        3 => {
            Ok(tx
                .execute_batch("CREATE TABLE prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL);")?)
        }
        _ => bail!("未知的数据库版本 {to}"),
    }
}

/// 从文件导入的联系人（尚未校验邮箱）。
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ImportedContact {
    pub name: String,
    pub emails: Vec<String>,
    pub org: String,
    pub note: String,
}

/// 界面提交的联系人；`id` 为空表示新建。
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContactInput {
    pub id: Option<i64>,
    pub name: String,
    pub emails: Vec<String>,
    #[serde(default)]
    pub org: String,
    #[serde(default)]
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub id: i64,
    pub name: String,
    pub emails: Vec<String>,
    pub org: String,
    pub note: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Group {
    pub id: i64,
    pub name: String,
    pub members: Vec<i64>,
}

/// 写信时的联想项：联系人、最近联系人或分组（分组带上成员地址）。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    pub kind: String,
    pub name: String,
    pub email: String,
    pub group_id: Option<i64>,
    pub members: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub added: usize,
    pub updated: usize,
    pub skipped: usize,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 规范化并校验一个邮箱地址；无效时返回 None。
pub fn valid_email(email: &str) -> Option<String> {
    let email = email.trim().trim_start_matches("mailto:").trim();
    if email.is_empty() || email.len() > 254 {
        return None;
    }
    email
        .parse::<lettre::Address>()
        .ok()
        .map(|_| email.to_string())
}

/// 打开一封邮件时是否把发件人记为最近联系人：垃圾邮件、已发送、草稿和已删除里的不算，
/// 自己的地址也不算。
pub fn sender_worth_recording(folder_role: &str, sender: &str, own: &[String]) -> bool {
    !matches!(
        folder_role,
        // unknown：文件夹列表还没加载（如离线启动），不确定是不是垃圾邮件，宁可不记
        "junk" | "sent" | "drafts" | "trash" | "local-drafts" | "unknown"
    ) && valid_email(sender).is_some()
        && !own.iter().any(|o| email_key(o) == email_key(sender))
}

/// 显示用的「名字 <地址>」；名字里有特殊字符时加引号。
pub fn format_address(name: &str, email: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return email.to_string();
    }
    if name.chars().any(|c| {
        matches!(
            c,
            ',' | ';' | '<' | '>' | '@' | '"' | '(' | ')' | ':' | '\\'
        )
    }) {
        format!(
            "\"{}\" <{email}>",
            name.replace('\\', "\\\\").replace('"', "\\\"")
        )
    } else {
        format!("{name} <{email}>")
    }
}

fn is_cjk(text: &str) -> bool {
    text.chars()
        .any(|c| ('\u{3400}'..='\u{9fff}').contains(&c) || ('\u{f900}'..='\u{faff}').contains(&c))
}

/// 姓和名拼成显示名：中文「姓名」不加空格，西文「名 姓」。
fn join_name(given: &str, family: &str) -> String {
    let (given, family) = (given.trim(), family.trim());
    match (given.is_empty(), family.is_empty()) {
        (true, true) => String::new(),
        (false, true) => given.to_string(),
        (true, false) => family.to_string(),
        _ if is_cjk(given) || is_cjk(family) => format!("{family}{given}"),
        _ => format!("{given} {family}"),
    }
}

// ---------------------------------------------------------------------------
// vCard
// ---------------------------------------------------------------------------

/// 按未转义的分隔符切分（vCard 里 `\;` `\,` 是普通字符）。
fn split_unescaped(value: &str, sep: char) -> Vec<String> {
    let mut parts = vec![String::new()];
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            parts.last_mut().unwrap().push(c);
            if let Some(next) = chars.next() {
                parts.last_mut().unwrap().push(next);
            }
        } else if c == sep {
            parts.push(String::new());
        } else {
            parts.last_mut().unwrap().push(c);
        }
    }
    parts
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n' | 'N') => out.push('\n'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\r', "")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn decode_quoted_printable(value: &str, charset: Option<&str>) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    let encoding = charset
        .and_then(|c| encoding_rs::Encoding::for_label(c.as_bytes()))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(&out).0.into_owned()
}

/// 属性名/参数和值之间的冒号（参数值引号里的冒号不算）。
fn value_colon(line: &str) -> Option<usize> {
    let mut quoted = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => quoted = !quoted,
            ':' if !quoted => return Some(i),
            _ => {}
        }
    }
    None
}

fn is_quoted_printable(line: &str) -> bool {
    value_colon(line).is_some_and(|i| line[..i].to_ascii_uppercase().contains("QUOTED-PRINTABLE"))
}

/// 解析 vCard 2.1 / 3.0 / 4.0，返回带邮箱的联系人。
pub fn parse_vcard(text: &str) -> Vec<ImportedContact> {
    // 展开逻辑行：2.1 quoted-printable 行尾的 = 表示接下一行（下一行可能还被折行缩进），
    // 其余以空格或 Tab 开头的行接到上一行
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let soft_break = lines
            .last()
            .is_some_and(|prev| prev.ends_with('=') && is_quoted_printable(prev));
        if soft_break {
            let prev = lines.last_mut().unwrap();
            prev.pop();
            prev.push_str(line.trim_start_matches([' ', '\t']));
        } else if (line.starts_with(' ') || line.starts_with('\t')) && !lines.is_empty() {
            lines.last_mut().unwrap().push_str(&line[1..]);
        } else {
            lines.push(line.to_string());
        }
    }
    let finish = |out: &mut Vec<ImportedContact>, current: Option<(ImportedContact, String)>| {
        if let Some((mut contact, from_n)) = current {
            // 导出时没有名字的联系人 FN 写的是邮箱，导入时还原成没有名字
            if contact.name.is_empty()
                || contact
                    .emails
                    .iter()
                    .any(|e| e.eq_ignore_ascii_case(&contact.name))
            {
                contact.name = from_n;
            }
            if !contact.emails.is_empty() {
                out.push(contact);
            }
        }
    };
    let mut out = Vec::new();
    let mut current: Option<(ImportedContact, String)> = None; // (联系人, N 字段拼出的名字)
    let mut nested = 0usize; // 2.1 AGENT 里嵌套的 vCard
    let mut after_agent = false;
    for line in lines {
        let Some(colon) = value_colon(&line) else {
            continue;
        };
        let (head, value) = (&line[..colon], &line[colon + 1..]);
        let mut params = head.split(';');
        let name = params
            .next()
            .unwrap_or("")
            .rsplit('.')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        let params: Vec<String> = params.map(|p| p.to_ascii_uppercase()).collect();
        let begin_or_end = value.trim().eq_ignore_ascii_case("VCARD");
        if name == "BEGIN" && begin_or_end {
            if current.is_some() && (after_agent || nested > 0) {
                nested += 1;
            } else {
                // 上一张没有 END 的名片到此结束
                finish(&mut out, current.take());
                current = Some((ImportedContact::default(), String::new()));
            }
            after_agent = false;
            continue;
        }
        if name == "END" && begin_or_end {
            if nested > 0 {
                nested -= 1;
            } else {
                finish(&mut out, current.take());
            }
            continue;
        }
        after_agent = name == "AGENT";
        if nested > 0 {
            continue;
        }
        let Some((contact, from_n)) = current.as_mut() else {
            continue;
        };
        let quoted = params
            .iter()
            .any(|p| p == "QUOTED-PRINTABLE" || p == "ENCODING=QUOTED-PRINTABLE");
        let charset = params
            .iter()
            .find_map(|p| p.strip_prefix("CHARSET="))
            .map(|c| c.trim_matches('"').to_string());
        let value = if quoted {
            decode_quoted_printable(value, charset.as_deref())
        } else {
            value.to_string()
        };
        match name.as_str() {
            "FN" => contact.name = unescape(value.trim()),
            "N" => {
                let parts = split_unescaped(&value, ';');
                let family = unescape(parts.first().map(String::as_str).unwrap_or(""));
                let given = unescape(parts.get(1).map(String::as_str).unwrap_or(""));
                *from_n = join_name(&given, &family);
            }
            "EMAIL" => {
                let email = unescape(value.trim());
                if !email.is_empty()
                    && !contact
                        .emails
                        .iter()
                        .any(|e| e.eq_ignore_ascii_case(&email))
                {
                    contact.emails.push(email);
                }
            }
            "ORG" => {
                contact.org = split_unescaped(&value, ';')
                    .iter()
                    .map(|p| unescape(p).trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            "NOTE" => contact.note = unescape(&value),
            _ => {}
        }
    }
    finish(&mut out, current.take());
    out
}

/// 按 RFC 6350 把一行折成不超过 75 字节。
fn fold(line: &str) -> String {
    let mut out = String::new();
    let mut width = 0;
    for c in line.chars() {
        if width + c.len_utf8() > 75 {
            out.push_str("\r\n ");
            width = 1;
        }
        out.push(c);
        width += c.len_utf8();
    }
    out
}

/// 导出为 vCard 3.0。
pub fn to_vcard(contacts: &[ImportedContact]) -> String {
    let mut out = String::new();
    for c in contacts {
        let mut lines = vec!["BEGIN:VCARD".to_string(), "VERSION:3.0".to_string()];
        let display = if c.name.is_empty() {
            c.emails.first().cloned().unwrap_or_default()
        } else {
            c.name.clone()
        };
        lines.push(format!("FN:{}", escape(&display)));
        lines.push(format!("N:;{};;;", escape(&c.name)));
        if !c.org.is_empty() {
            lines.push(format!("ORG:{}", escape(&c.org)));
        }
        for email in &c.emails {
            lines.push(format!("EMAIL;TYPE=INTERNET:{}", escape(email)));
        }
        if !c.note.is_empty() {
            lines.push(format!("NOTE:{}", escape(&c.note)));
        }
        lines.push("END:VCARD".into());
        for line in lines {
            out.push_str(&fold(&line));
            out.push_str("\r\n");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// CSV
// ---------------------------------------------------------------------------

/// UTF-8（可带 BOM）、UTF-16（带 BOM）或 GBK/GB18030。
fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xef\xbb\xbf") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if bytes.starts_with(b"\xff\xfe") {
        return encoding_rs::UTF_16LE.decode(bytes).0.into_owned();
    }
    if bytes.starts_with(b"\xfe\xff") {
        return encoding_rs::UTF_16BE.decode(bytes).0.into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => encoding_rs::GB18030.decode(bytes).0.into_owned(),
    }
}

/// RFC 4180：引号字段、"" 转义、字段内换行。
fn parse_records(text: &str, sep: char) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    field.push('"');
                    chars.next();
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
        } else if c == '"' && field.trim().is_empty() {
            // 允许分隔符和开头引号之间有空格
            field.clear();
            quoted = true;
        } else if c == sep {
            record.push(std::mem::take(&mut field));
        } else if c == '\n' || c == '\r' {
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            record.push(std::mem::take(&mut field));
            if record.iter().any(|f| !f.trim().is_empty()) {
                records.push(std::mem::take(&mut record));
            } else {
                record.clear();
            }
        } else {
            field.push(c);
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        if record.iter().any(|f| !f.trim().is_empty()) {
            records.push(record);
        }
    }
    records
}

#[derive(Default)]
struct Columns {
    name: Option<usize>,
    given: Option<usize>,
    family: Option<usize>,
    emails: Vec<usize>,
    org: Option<usize>,
    note: Option<usize>,
}

fn columns(header: &[String]) -> Columns {
    let mut cols = Columns::default();
    for (i, h) in header.iter().enumerate() {
        let h = h.trim().to_lowercase();
        let is = |names: &[&str]| names.contains(&h.as_str());
        if is(&[
            "name",
            "display name",
            "full name",
            "姓名",
            "名字",
            "显示名",
            "全名",
            "联系人",
            "联系人姓名",
        ]) {
            cols.name.get_or_insert(i);
        } else if is(&["first name", "given name", "名"]) {
            cols.given.get_or_insert(i);
        } else if is(&["last name", "family name", "surname", "姓"]) {
            cols.family.get_or_insert(i);
        } else if is(&[
            "company",
            "organization",
            "organization 1 - name",
            "公司",
            "单位",
            "工作单位",
        ]) {
            cols.org.get_or_insert(i);
        } else if is(&["notes", "note", "备注"]) {
            cols.note.get_or_insert(i);
        } else if (h.contains("e-mail")
            || h.contains("email")
            || h.contains("邮箱")
            || h.contains("电子邮件")
            || h.contains("邮件地址"))
            && !h.contains("type")
            && !h.contains("类型")
            && !h.contains("display")
        {
            cols.emails.push(i);
        }
    }
    cols
}

/// 解析 Outlook、Gmail、网易等导出的通讯录 CSV，返回带邮箱的联系人。
pub fn parse_csv(bytes: &[u8]) -> Vec<ImportedContact> {
    let mut text = decode_text(bytes);
    let first_line = text.lines().next().unwrap_or("").trim().to_string();
    // Excel 导出的 CSV 第一行可能是「sep=;」，指明分隔符
    let declared = first_line
        .strip_prefix("sep=")
        .or_else(|| first_line.strip_prefix("SEP="))
        .and_then(|rest| rest.chars().next());
    if declared.is_some() {
        text = text
            .split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default();
    }
    let header_line = text.lines().next().unwrap_or("");
    let sep = declared.unwrap_or_else(|| {
        [',', ';', '\t']
            .into_iter()
            .max_by_key(|sep| header_line.matches(*sep).count())
            .unwrap_or(',')
    });
    let mut records = parse_records(&text, sep).into_iter();
    let Some(header) = records.next() else {
        return Vec::new();
    };
    let cols = columns(&header);
    let get = |record: &[String], index: Option<usize>| {
        index
            .and_then(|i| record.get(i))
            .map(|v| v.trim().to_string())
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    for record in records {
        let mut emails: Vec<String> = Vec::new();
        for &i in &cols.emails {
            for email in record.get(i).map(String::as_str).unwrap_or("").split(":::") {
                let email = email.trim();
                if !email.is_empty() && !emails.iter().any(|e| e.eq_ignore_ascii_case(email)) {
                    emails.push(email.to_string());
                }
            }
        }
        if emails.is_empty() {
            continue;
        }
        let mut name = get(&record, cols.name);
        if name.is_empty() {
            name = join_name(&get(&record, cols.given), &get(&record, cols.family));
        }
        out.push(ImportedContact {
            name,
            emails,
            org: get(&record, cols.org),
            note: get(&record, cols.note),
        });
    }
    out
}

// ---------------------------------------------------------------------------
// 数据库
// ---------------------------------------------------------------------------

pub struct Db {
    conn: Mutex<Connection>,
}

fn like_pattern(query: &str) -> String {
    let escaped = query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

fn constraint(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("无法打开 {}", path.display()))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    pub(crate) fn init(mut conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version > SCHEMA_VERSION {
            bail!("本地数据库来自更新版本的 MailBeacon，请升级后再打开");
        }
        for to in version + 1..=SCHEMA_VERSION {
            let tx = conn.transaction()?;
            migrate(&tx, to)?;
            tx.pragma_update(None, "user_version", to)?;
            tx.commit()?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    #[cfg(test)]
    pub fn schema_version(&self) -> Result<i64> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .pragma_query_value(None, "user_version", |r| r.get(0))?)
    }

    fn emails_of(conn: &Connection, id: i64) -> Result<Vec<String>> {
        let mut stmt = conn.prepare_cached(
            "SELECT email FROM contact_emails WHERE contact_id = ? ORDER BY position",
        )?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    fn owner_of(tx: &Connection, email: &str) -> Result<Option<i64>> {
        Ok(tx
            .query_row(
                "SELECT contact_id FROM contact_emails WHERE email_key = ?",
                [email_key(email)],
                |r| r.get(0),
            )
            .optional()?)
    }

    fn write_emails(tx: &Transaction, id: i64, emails: &[String]) -> Result<()> {
        tx.execute("DELETE FROM contact_emails WHERE contact_id = ?", [id])?;
        for (i, email) in emails.iter().enumerate() {
            tx.execute(
                "INSERT INTO contact_emails (contact_id, email, email_key, position) VALUES (?, ?, ?, ?)",
                params![id, email, email_key(email), i as i64],
            )
            .map_err(|e| {
                if constraint(&e) {
                    anyhow!("邮箱 {email} 已属于其他联系人")
                } else {
                    e.into()
                }
            })?;
        }
        Ok(())
    }

    /// 校验并去重邮箱（不区分大小写，保留第一次出现的写法）。
    fn clean_emails(emails: &[String]) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        for email in emails {
            if email.trim().is_empty() {
                continue;
            }
            let email =
                valid_email(email).ok_or_else(|| anyhow!("邮箱地址无效：{}", email.trim()))?;
            if !out.iter().any(|e| email_key(e) == email_key(&email)) {
                out.push(email);
            }
        }
        if out.is_empty() {
            bail!("请至少填写一个邮箱地址");
        }
        Ok(out)
    }

    pub fn save_contact(&self, input: &ContactInput) -> Result<i64> {
        let emails = Self::clean_emails(&input.emails)?;
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let id =
            match input.id {
                Some(id) => {
                    let changed = tx.execute(
                    "UPDATE contacts SET name = ?, org = ?, note = ?, updated_at = ? WHERE id = ?",
                    params![input.name.trim(), input.org.trim(), input.note.trim(), now(), id],
                )?;
                    if changed == 0 {
                        bail!("联系人不存在");
                    }
                    id
                }
                None => {
                    tx.execute(
                        "INSERT INTO contacts (name, org, note, updated_at) VALUES (?, ?, ?, ?)",
                        params![
                            input.name.trim(),
                            input.org.trim(),
                            input.note.trim(),
                            now()
                        ],
                    )?;
                    tx.last_insert_rowid()
                }
            };
        Self::write_emails(&tx, id, &emails)?;
        tx.commit()?;
        Ok(id)
    }

    pub fn delete_contact(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM contacts WHERE id = ?", [id])?;
        Ok(())
    }

    /// 按名字、单位、备注或任一邮箱搜索；空查询返回全部。
    pub fn list_contacts(&self, query: &str) -> Result<Vec<Contact>> {
        let conn = self.conn.lock().unwrap();
        let pattern = like_pattern(query.trim());
        let mut stmt = conn.prepare(
            "SELECT id, name, org, note FROM contacts c
             WHERE ?1 = '%%' OR name LIKE ?1 ESCAPE '\\' OR org LIKE ?1 ESCAPE '\\' OR note LIKE ?1 ESCAPE '\\'
                OR EXISTS (SELECT 1 FROM contact_emails e WHERE e.contact_id = c.id AND e.email LIKE ?1 ESCAPE '\\')
             ORDER BY CASE WHEN name = '' THEN 1 ELSE 0 END, name COLLATE NOCASE, id",
        )?;
        let rows: Vec<(i64, String, String, String)> = stmt
            .query_map([&pattern], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        rows.into_iter()
            .map(|(id, name, org, note)| {
                Ok(Contact {
                    id,
                    name,
                    emails: Self::emails_of(&conn, id)?,
                    org,
                    note,
                })
            })
            .collect()
    }

    pub fn save_group(&self, id: Option<i64>, name: &str) -> Result<i64> {
        let name = name.trim();
        if name.is_empty() {
            bail!("分组名称不能为空");
        }
        let conn = self.conn.lock().unwrap();
        let result = match id {
            Some(id) => match conn.execute(
                "UPDATE contact_groups SET name = ? WHERE id = ?",
                params![name, id],
            ) {
                Ok(0) => bail!("分组不存在"),
                other => other.map(|_| id),
            },
            None => conn
                .execute("INSERT INTO contact_groups (name) VALUES (?)", [name])
                .map(|_| conn.last_insert_rowid()),
        };
        result.map_err(|e| {
            if constraint(&e) {
                anyhow!("已有名为「{name}」的分组")
            } else {
                e.into()
            }
        })
    }

    pub fn delete_group(&self, id: i64) -> Result<()> {
        self.conn
            .lock()
            .unwrap()
            .execute("DELETE FROM contact_groups WHERE id = ?", [id])?;
        Ok(())
    }

    pub fn set_group_members(&self, group: i64, contacts: &[i64]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM group_members WHERE group_id = ?", [group])?;
        for (i, contact) in contacts.iter().enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO group_members (group_id, contact_id, position) VALUES (?, ?, ?)",
                params![group, contact, i as i64],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_groups(&self) -> Result<Vec<Group>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt =
            conn.prepare("SELECT id, name FROM contact_groups ORDER BY name COLLATE NOCASE")?;
        let groups: Vec<(i64, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let mut members = conn
            .prepare("SELECT contact_id FROM group_members WHERE group_id = ? ORDER BY position")?;
        groups
            .into_iter()
            .map(|(id, name)| {
                Ok(Group {
                    id,
                    name,
                    members: members
                        .query_map([id], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?,
                })
            })
            .collect()
    }

    /// 导入：按邮箱（不区分大小写）合并到已有联系人，补上缺的名字、单位和邮箱。
    pub fn import(&self, contacts: &[ImportedContact]) -> Result<ImportResult> {
        let mut result = ImportResult::default();
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        for contact in contacts {
            let emails: Vec<String> = contact
                .emails
                .iter()
                .filter_map(|e| valid_email(e))
                .collect();
            if emails.is_empty() {
                result.skipped += 1;
                continue;
            }
            let mut existing = None;
            for email in &emails {
                if let Some(id) = Self::owner_of(&tx, email)? {
                    existing = Some(id);
                    break;
                }
            }
            match existing {
                Some(id) => {
                    tx.execute(
                        "UPDATE contacts SET
                           name = CASE WHEN name = '' THEN ?2 ELSE name END,
                           org = CASE WHEN org = '' THEN ?3 ELSE org END,
                           note = CASE WHEN note = '' THEN ?4 ELSE note END,
                           updated_at = ?5
                         WHERE id = ?1",
                        params![
                            id,
                            contact.name.trim(),
                            contact.org.trim(),
                            contact.note.trim(),
                            now()
                        ],
                    )?;
                    let mut position: i64 = tx.query_row(
                        "SELECT COALESCE(MAX(position) + 1, 0) FROM contact_emails WHERE contact_id = ?",
                        [id],
                        |r| r.get(0),
                    )?;
                    for email in &emails {
                        if Self::owner_of(&tx, email)?.is_none() {
                            tx.execute(
                                "INSERT INTO contact_emails (contact_id, email, email_key, position) VALUES (?, ?, ?, ?)",
                                params![id, email, email_key(email), position],
                            )?;
                            position += 1;
                        }
                    }
                    result.updated += 1;
                }
                None => {
                    tx.execute(
                        "INSERT INTO contacts (name, org, note, updated_at) VALUES (?, ?, ?, ?)",
                        params![
                            contact.name.trim(),
                            contact.org.trim(),
                            contact.note.trim(),
                            now()
                        ],
                    )?;
                    let id = tx.last_insert_rowid();
                    let mut deduped: Vec<String> = Vec::new();
                    for email in emails {
                        if !deduped.iter().any(|e| email_key(e) == email_key(&email)) {
                            deduped.push(email);
                        }
                    }
                    Self::write_emails(&tx, id, &deduped)?;
                    result.added += 1;
                }
            }
        }
        tx.commit()?;
        Ok(result)
    }

    /// 全部联系人，用于导出 vCard。
    pub fn export(&self) -> Result<Vec<ImportedContact>> {
        Ok(self
            .list_contacts("")?
            .into_iter()
            .map(|c| ImportedContact {
                name: c.name,
                emails: c.emails,
                org: c.org,
                note: c.note,
            })
            .collect())
    }

    fn bump(&self, name: &str, email: &str, column: &str) -> Result<()> {
        let Some(email) = valid_email(email) else {
            return Ok(());
        };
        let sql = format!(
            "INSERT INTO recent_addresses (email_key, email, name, {column}, last_used) VALUES (?4, ?1, ?2, 1, ?3)
             ON CONFLICT(email_key) DO UPDATE SET {column} = {column} + 1, last_used = ?3,
               name = CASE WHEN ?2 = '' THEN name ELSE ?2 END"
        );
        self.conn
            .lock()
            .unwrap()
            .execute(&sql, params![email, name.trim(), now(), email_key(&email)])?;
        Ok(())
    }

    /// 应用偏好（缺失或损坏时用默认值）。
    pub fn load_prefs(&self) -> Result<crate::prefs::Prefs> {
        let value: Option<String> = self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM prefs WHERE key = 'app'", [], |r| {
                r.get(0)
            })
            .optional()?;
        Ok(value
            .and_then(|v| serde_json::from_str(&v).ok())
            .unwrap_or_default())
    }

    pub fn save_prefs(&self, prefs: &crate::prefs::Prefs) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO prefs (key, value) VALUES ('app', ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [serde_json::to_string(prefs)?],
        )?;
        Ok(())
    }

    pub fn raw_pref(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT value FROM prefs WHERE key = ?", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn save_raw_pref(&self, key: &str, value: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "INSERT INTO prefs (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [key, value],
        )?;
        Ok(())
    }

    /// 存成 JSON 的一项设置。内容损坏时先原样备份到 `<key>.corrupt`，再当作空的，
    /// 之后保存新内容也不会丢掉原来的数据。
    fn load_json_list<T: serde::de::DeserializeOwned>(&self, key: &str) -> Result<Vec<T>> {
        let Some(value) = self.raw_pref(key)? else {
            return Ok(Vec::new());
        };
        match serde_json::from_str(&value) {
            Ok(list) => Ok(list),
            Err(e) => {
                eprintln!("本地数据 {key} 已损坏，已备份为 {key}.corrupt：{e}");
                let backup = format!("{key}.corrupt");
                if self.raw_pref(&backup)?.is_none() {
                    self.save_raw_pref(&backup, &value)?;
                }
                Ok(Vec::new())
            }
        }
    }

    /// 收信规则（按顺序）。
    pub fn load_rules(&self) -> Result<Vec<crate::rules::Rule>> {
        self.load_json_list("rules")
    }

    pub fn save_rules(&self, rules: &[crate::rules::Rule]) -> Result<()> {
        self.save_raw_pref("rules", &serde_json::to_string(rules)?)
    }

    pub fn load_templates(&self) -> Result<Vec<crate::templates::Template>> {
        self.load_json_list("templates")
    }

    pub fn save_templates(&self, list: &[crate::templates::Template]) -> Result<()> {
        self.save_raw_pref("templates", &serde_json::to_string(list)?)
    }

    /// 从联想里去掉一个最近联系人（不影响通讯录里的联系人）。
    pub fn forget_recent(&self, email: &str) -> Result<()> {
        self.conn.lock().unwrap().execute(
            "DELETE FROM recent_addresses WHERE email_key = ?",
            [email_key(email)],
        )?;
        Ok(())
    }

    /// 发信后记下收件人，作为最近联系人。
    pub fn record_sent(&self, recipients: &[(String, String)]) -> Result<()> {
        for (name, email) in recipients {
            self.bump(name, email, "sent")?;
        }
        Ok(())
    }

    /// 打开来信时记下发件人（权重低于发信）。
    pub fn record_received(&self, name: &str, email: &str) -> Result<()> {
        self.bump(name, email, "received")
    }

    /// 写信时的联想：联系人和最近联系人按往来频率排序，匹配的分组排在后面。
    pub fn suggest(&self, query: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let pattern = like_pattern(query);
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(
            "SELECT kind, name, email, score FROM (
               SELECT 'contact' AS kind, c.name AS name, e.email AS email,
                      1 + COALESCE(r.sent, 0) * 3 + COALESCE(r.received, 0) AS score,
                      COALESCE(r.last_used, 0) AS last_used
               FROM contact_emails e JOIN contacts c ON c.id = e.contact_id
               LEFT JOIN recent_addresses r ON r.email_key = e.email_key
               WHERE c.name LIKE ?1 ESCAPE '\\' OR e.email LIKE ?1 ESCAPE '\\'
               UNION ALL
               SELECT 'recent', r.name, r.email, r.sent * 3 + r.received, r.last_used
               FROM recent_addresses r
               WHERE (r.name LIKE ?1 ESCAPE '\\' OR r.email LIKE ?1 ESCAPE '\\')
                 AND NOT EXISTS (SELECT 1 FROM contact_emails e WHERE e.email_key = r.email_key)
             ) ORDER BY score DESC, last_used DESC, name COLLATE NOCASE LIMIT ?2",
        )?;
        let mut out: Vec<Suggestion> = stmt
            .query_map(params![pattern, limit as i64], |r| {
                Ok(Suggestion {
                    kind: r.get(0)?,
                    name: r.get(1)?,
                    email: r.get(2)?,
                    group_id: None,
                    members: Vec::new(),
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut groups = conn.prepare(
            "SELECT id, name FROM contact_groups WHERE name LIKE ?1 ESCAPE '\\' ORDER BY name LIMIT ?2",
        )?;
        let found: Vec<(i64, String)> = groups
            .query_map(params![pattern, limit as i64], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let mut members = conn.prepare(
            "SELECT c.name, (SELECT email FROM contact_emails e WHERE e.contact_id = c.id ORDER BY position LIMIT 1)
             FROM group_members m JOIN contacts c ON c.id = m.contact_id
             WHERE m.group_id = ? ORDER BY m.position",
        )?;
        for (id, name) in found {
            let addresses: Vec<String> = members
                .query_map([id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
                })?
                .filter_map(|r| r.ok())
                .filter_map(|(name, email)| email.map(|e| format_address(&name, &e)))
                .collect();
            if addresses.is_empty() {
                continue;
            }
            out.push(Suggestion {
                kind: "group".into(),
                name,
                email: String::new(),
                group_id: Some(id),
                members: addresses,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, emails: &[&str]) -> ContactInput {
        ContactInput {
            id: None,
            name: name.into(),
            emails: emails.iter().map(|e| e.to_string()).collect(),
            org: String::new(),
            note: String::new(),
        }
    }

    fn imported(name: &str, emails: &[&str], org: &str) -> ImportedContact {
        ImportedContact {
            name: name.into(),
            emails: emails.iter().map(|e| e.to_string()).collect(),
            org: org.into(),
            note: String::new(),
        }
    }

    #[test]
    fn vcard_versions_folding_escapes_and_multiple_emails() {
        let text = concat!(
            "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:张三\r\nN:张;三;;;\r\n",
            "ORG:示例公司\\, 研发部\r\n",
            "EMAIL;TYPE=INTERNET,WORK:zhang@example.com\r\n",
            "item1.EMAIL;type=INTERNET:zhang.san@personal.example\r\n",
            "NOTE:第一行\\n第二\r\n 行\r\nEND:VCARD\r\n",
            "BEGIN:VCARD\nVERSION:2.1\n",
            "N;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=E6=9D=8E;=E5=9B=9B\n",
            "EMAIL;PREF;INTERNET:li@example.com\nEND:VCARD\n",
            "BEGIN:VCARD\nVERSION:4.0\nFN:No Email\nTEL:123\nEND:VCARD\n",
            "BEGIN:VCARD\nVERSION:4.0\nEMAIL:only@example.com\nEND:VCARD\n",
        );
        let contacts = parse_vcard(text);
        assert_eq!(contacts.len(), 3, "没有邮箱的联系人跳过");
        assert_eq!(contacts[0].name, "张三");
        assert_eq!(contacts[0].org, "示例公司, 研发部");
        assert_eq!(
            contacts[0].emails,
            ["zhang@example.com", "zhang.san@personal.example"]
        );
        assert_eq!(contacts[0].note, "第一行\n第二行");
        assert_eq!(
            contacts[1].name, "李四",
            "2.1 的 quoted-printable 和 N 字段"
        );
        assert_eq!(contacts[1].emails, ["li@example.com"]);
        assert_eq!(contacts[2].name, "", "没有名字时留空，界面显示邮箱");
    }

    #[test]
    fn csv_from_outlook_gmail_and_gbk_163_exports() {
        let outlook = "First Name,Last Name,E-mail Address,E-mail 2 Address,Company\r\nSan,Zhang,z@example.com,z2@example.com,ACME\r\n";
        let c = parse_csv(outlook.as_bytes());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "San Zhang");
        assert_eq!(c[0].emails, ["z@example.com", "z2@example.com"]);
        assert_eq!(c[0].org, "ACME");

        let gmail = concat!(
            "Name,Given Name,Family Name,E-mail 1 - Type,E-mail 1 - Value,Organization 1 - Name\n",
            "\"Li, Si\",Si,Li,* Work,li@example.com ::: li2@example.com,\"Org \"\"X\"\"\"\n"
        );
        let c = parse_csv(gmail.as_bytes());
        assert_eq!(c[0].name, "Li, Si");
        assert_eq!(c[0].emails, ["li@example.com", "li2@example.com"]);
        assert_eq!(c[0].org, "Org \"X\"");

        let (gbk, _, _) = encoding_rs::GBK.encode("姓名,邮箱地址,公司\n王五,wang@163.com,网易\n");
        let c = parse_csv(&gbk);
        assert_eq!(c[0].name, "王五");
        assert_eq!(c[0].emails, ["wang@163.com"]);
        assert_eq!(c[0].org, "网易");

        let bom = "\u{feff}显示名,电子邮件\n赵六,zhao@example.com\n无邮箱,\n";
        let c = parse_csv(bom.as_bytes());
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].name, "赵六");
    }

    #[test]
    fn vcard_export_round_trips_and_escapes() {
        let contact = ImportedContact {
            name: "张三, 研发".into(),
            emails: vec!["a@example.com".into(), "b@example.com".into()],
            org: "公司;部门".into(),
            note: "两行\n备注".into(),
        };
        let text = to_vcard(std::slice::from_ref(&contact));
        assert!(text.contains("FN:张三\\, 研发"));
        assert!(text.contains("ORG:公司\\;部门"));
        assert_eq!(parse_vcard(&text), vec![contact]);
    }

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    #[test]
    fn importing_merges_by_email_case_insensitively() {
        let db = db();
        let first = db.import(&[imported("", &["Z@Example.com"], "")]).unwrap();
        assert_eq!((first.added, first.updated), (1, 0));
        let second = db
            .import(&[
                imported("张三", &["z@example.com", "z2@example.com"], "ACME"),
                imported("坏地址", &["not-an-address"], ""),
            ])
            .unwrap();
        assert_eq!((second.added, second.updated, second.skipped), (0, 1, 1));
        let all = db.list_contacts("").unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "张三", "原来没有名字时用导入的名字补上");
        assert_eq!(all[0].emails, ["Z@Example.com", "z2@example.com"]);
    }

    #[test]
    fn contacts_groups_and_crud() {
        let db = db();
        let id = db.save_contact(&input("张三", &["z@example.com"])).unwrap();
        let other = db.save_contact(&input("李四", &["l@example.com"])).unwrap();
        assert!(
            db.save_contact(&input("重复", &["Z@example.com"])).is_err(),
            "同一个邮箱不能属于两个联系人"
        );
        let mut edit = input("张三丰", &["z@example.com", "z3@example.com"]);
        edit.id = Some(id);
        edit.org = "武当".into();
        db.save_contact(&edit).unwrap();
        let group = db.save_group(None, "项目组").unwrap();
        assert!(db.save_group(None, "项目组").is_err(), "分组不能重名");
        db.set_group_members(group, &[id, other]).unwrap();
        let groups = db.list_groups().unwrap();
        assert_eq!(groups[0].name, "项目组");
        assert_eq!(groups[0].members, vec![id, other]);
        db.delete_contact(other).unwrap();
        assert_eq!(
            db.list_groups().unwrap()[0].members,
            vec![id],
            "删除联系人同时移出分组"
        );
        assert_eq!(db.list_contacts("武当").unwrap().len(), 1, "按单位搜索");
        assert_eq!(
            db.list_contacts("z3@").unwrap()[0].name,
            "张三丰",
            "按任一邮箱搜索"
        );
        db.delete_group(group).unwrap();
        assert!(db.list_groups().unwrap().is_empty());
    }

    #[test]
    fn suggestions_rank_frequent_recipients_and_include_groups() {
        let db = db();
        let zhang = db
            .save_contact(&input("张三", &["zhang@example.com"]))
            .unwrap();
        for _ in 0..3 {
            db.record_sent(&[("赵敏".into(), "zhao@example.com".into())])
                .unwrap();
        }
        db.record_received("周芷若", "zhou@example.com").unwrap();
        let g = db.save_group(None, "z项目").unwrap();
        db.set_group_members(g, &[zhang]).unwrap();

        let s = db.suggest("z", 10).unwrap();
        let emails: Vec<_> = s.iter().map(|x| x.email.as_str()).collect();
        assert_eq!(emails[0], "zhao@example.com", "常发的排在最前");
        assert!(emails.contains(&"zhang@example.com"));
        assert!(emails.contains(&"zhou@example.com"));
        let group = s
            .iter()
            .find(|x| x.kind == "group")
            .expect("分组也能被联想");
        assert_eq!(group.members, vec!["张三 <zhang@example.com>"]);
        assert!(db.suggest("", 10).unwrap().is_empty());
        assert_eq!(db.suggest("赵", 10).unwrap()[0].name, "赵敏", "按名字匹配");
    }

    #[test]
    fn vcard_quoted_params_missing_end_agent_and_qp_folding() {
        let text = concat!(
            "BEGIN:VCARD\nVERSION:3.0\nFN:引号\nEMAIL;TYPE=\"x:y\":q@example.com\nEND:VCARD\n",
            "BEGIN:VCARD\nVERSION:3.0\nFN:没结尾\nEMAIL:noend@example.com\n",
            "BEGIN:VCARD\nVERSION:2.1\nFN:外层\nEMAIL:outer@example.com\n",
            "AGENT:\nBEGIN:VCARD\nVERSION:2.1\nFN:代理\nEMAIL:agent@example.com\nEND:VCARD\n",
            "END:VCARD\n",
            "BEGIN:VCARD\nVERSION:2.1\nFN;CHARSET=UTF-8;ENCODING=QUOTED-PRINTABLE:=E5=BC=A0=\n =E4=B8=89\nEMAIL:qp@example.com\nEND:VCARD\n",
        );
        let contacts = parse_vcard(text);
        let names: Vec<_> = contacts.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["引号", "没结尾", "外层", "张三"], "{contacts:?}");
        assert_eq!(contacts[0].emails, ["q@example.com"]);
        assert_eq!(
            contacts[2].emails,
            ["outer@example.com"],
            "嵌套的 AGENT 不覆盖外层联系人"
        );
    }

    #[test]
    fn nameless_contacts_round_trip_without_gaining_their_email_as_name() {
        let contact = imported("", &["a@example.com"], "");
        assert_eq!(
            parse_vcard(&to_vcard(std::slice::from_ref(&contact))),
            vec![contact]
        );
    }

    #[test]
    fn csv_excel_separator_line_and_space_before_quotes() {
        let excel = "sep=;\nName;E-mail Address\nZhang;z@example.com\n";
        assert_eq!(parse_csv(excel.as_bytes())[0].emails, ["z@example.com"]);
        let spaced = "Name, E-mail Address\n \"Li, Si\", l@example.com\n";
        let c = parse_csv(spaced.as_bytes());
        assert_eq!(c[0].name, "Li, Si");
        assert_eq!(c[0].emails, ["l@example.com"]);
    }

    #[test]
    fn unicode_emails_are_unique_regardless_of_case() {
        let db = db();
        db.save_contact(&input("A", &["Ärger@example.com"]))
            .unwrap();
        assert!(
            db.save_contact(&input("B", &["ärger@example.com"]))
                .is_err()
        );
        let r = db
            .import(&[imported("C", &["ÄRGER@example.com"], "")])
            .unwrap();
        assert_eq!((r.added, r.updated), (0, 1));
    }

    #[test]
    fn saving_a_missing_group_fails_and_recent_addresses_can_be_forgotten() {
        let db = db();
        assert!(db.save_group(Some(42), "不存在").is_err());
        db.record_sent(&[("噪音".into(), "noise@example.com".into())])
            .unwrap();
        assert_eq!(db.suggest("noise", 5).unwrap().len(), 1);
        db.forget_recent("NOISE@example.com").unwrap();
        assert!(db.suggest("noise", 5).unwrap().is_empty());
    }

    #[test]
    fn version_one_databases_migrate_and_keep_their_data() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch(V1_SCHEMA).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute("INSERT INTO contacts (id, name) VALUES (1, '旧')", [])
            .unwrap();
        conn.execute("INSERT INTO contact_emails (contact_id, email, position) VALUES (1, 'Old@Example.com', 0)", []).unwrap();
        conn.execute("INSERT INTO recent_addresses (email, name, sent, last_used) VALUES ('r@example.com', 'R', 2, 1)", []).unwrap();
        let db = Db::init(conn).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.list_contacts("").unwrap()[0].emails, ["Old@Example.com"]);
        assert!(
            db.save_contact(&input("x", &["old@example.com"])).is_err(),
            "迁移后仍按小写判重"
        );
        assert_eq!(db.suggest("r@", 5).unwrap()[0].email, "r@example.com");
    }

    #[test]
    fn only_senders_of_real_incoming_mail_are_recorded() {
        let own = ["me@example.com".to_string(), "Work@Example.com".to_string()];
        assert!(sender_worth_recording("inbox", "friend@example.com", &own));
        assert!(sender_worth_recording("other", "friend@example.com", &own));
        for role in ["junk", "sent", "drafts", "trash", "local-drafts"] {
            assert!(
                !sender_worth_recording(role, "friend@example.com", &own),
                "{role}"
            );
        }
        assert!(
            !sender_worth_recording("inbox", "work@example.com", &own),
            "自己的地址不记录"
        );
        assert!(!sender_worth_recording("inbox", "", &own));
        assert!(
            !sender_worth_recording("unknown", "friend@example.com", &own),
            "文件夹类型未知时不记录"
        );
    }

    #[test]
    fn schema_migrates_and_reopens() {
        let dir = std::env::temp_dir().join(format!("mailbeacon-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("local.db");
        {
            let db = Db::open(&path).unwrap();
            db.save_contact(&input("A", &["a@example.com"])).unwrap();
        }
        let db = Db::open(&path).unwrap();
        assert_eq!(db.schema_version().unwrap(), SCHEMA_VERSION);
        assert_eq!(db.list_contacts("").unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
