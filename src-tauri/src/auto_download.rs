//! 每个账号独立的收件箱自动存档；进度落盘，网络与 IDLE / 界面命令互不占用。

use crate::{
    AppState,
    config::{Account, Store},
    download_protocol,
    imap::{self, Imap},
    sync::{self, Event},
};
use anyhow::{Context, Result, anyhow, bail};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use mailbeacon_parser::{Address, MessageParser, MimeHeaders, PartType};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncRead, AsyncWrite};

const UID_WINDOW: u32 = 500;
const BATCH: usize = 20;
const RETRY: Duration = Duration::from_secs(15);
const MAX_KEYWORDS: usize = 100;
const MAX_KEYWORD_CHARS: usize = 200;

#[derive(Debug)]
struct MailboxIdentityChanged;
impl std::fmt::Display for MailboxIdentityChanged {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "服务器已重建收件箱或邮件编号发生变化；请关闭后重新开启，从新的收件边界继续"
        )
    }
}
impl std::error::Error for MailboxIdentityChanged {}

/// 单值枚举保证黑名单和白名单不能同时启用；旧账号默认不过滤。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum FilterMode {
    #[default]
    None,
    Whitelist,
    Blacklist,
}

/// 任意命中保留原范围；按字段模式对每个字段使用独立关键词。
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeywordScope {
    #[default]
    Any,
    Fields,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct FieldKeywords {
    pub subject: Vec<String>,
    pub sender: Vec<String>,
    pub recipient: Vec<String>,
    pub body: Vec<String>,
}

impl FieldKeywords {
    fn groups(&self) -> [&Vec<String>; 4] {
        [&self.subject, &self.sender, &self.recipient, &self.body]
    }

    fn normalized(&self) -> Result<Self> {
        let normalized = Self {
            subject: normalize_keywords(FilterMode::None, self.subject.clone())?,
            sender: normalize_keywords(FilterMode::None, self.sender.clone())?,
            recipient: normalize_keywords(FilterMode::None, self.recipient.clone())?,
            body: normalize_keywords(FilterMode::None, self.body.clone())?,
        };
        if normalized
            .groups()
            .iter()
            .map(|group| group.len())
            .sum::<usize>()
            > MAX_KEYWORDS
        {
            bail!("按字段设置的关键词合计最多 100 个");
        }
        Ok(normalized)
    }

    fn is_empty(&self) -> bool {
        self.groups().iter().all(|group| group.is_empty())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub enabled: bool,
    pub directory: String,
    pub filter_mode: FilterMode,
    pub keywords: Vec<String>,
    pub keyword_scope: KeywordScope,
    pub field_keywords: FieldKeywords,
    pub revision: u64,
    pub checkpoint: Option<Checkpoint>,
    pub saved_count: u64,
    pub last_path: Option<String>,
}

/// 保留用户拼写；去掉首尾空白、空项和大小写重复，并校验数量及长度。
pub fn normalize_keywords(mode: FilterMode, keywords: Vec<String>) -> Result<Vec<String>> {
    let mut seen = BTreeSet::new();
    let mut normalized = Vec::new();
    for keyword in keywords {
        let keyword = keyword.trim();
        if keyword.is_empty() {
            continue;
        }
        if keyword.chars().count() > MAX_KEYWORD_CHARS {
            bail!("每个关键词最多 200 个字符");
        }
        if seen.insert(keyword.to_lowercase()) {
            normalized.push(keyword.to_owned());
        }
        if normalized.len() > MAX_KEYWORDS {
            bail!("最多可设置 100 个关键词");
        }
    }
    if mode != FilterMode::None && normalized.is_empty() {
        bail!("开启黑名单或白名单时，请至少填写一个关键词");
    }
    Ok(normalized)
}

impl Settings {
    /// 保存时校验两个模式的草稿；只有当前模式要求至少填写一个关键词。
    pub fn normalize_filter_rules(&mut self) -> Result<()> {
        self.keywords = normalize_keywords(FilterMode::None, self.keywords.clone())?;
        self.field_keywords = self.field_keywords.normalized()?;
        if self.filter_mode != FilterMode::None {
            self.validate_active_keywords()?;
        }
        Ok(())
    }

    fn validate_active_keywords(&self) -> Result<()> {
        let empty = match self.keyword_scope {
            KeywordScope::Any => self.keywords.is_empty(),
            KeywordScope::Fields => self.field_keywords.is_empty(),
        };
        if empty {
            bail!("开启黑名单或白名单时，请至少填写一个关键词");
        }
        Ok(())
    }

    /// 仅复制活动规则，供全量任务固定规则快照；不携带进度或另一模式的草稿。
    pub fn active_filter(&self) -> Result<Self> {
        if self.filter_mode == FilterMode::None {
            return Ok(Self::default());
        }
        let mut active = Self {
            filter_mode: self.filter_mode,
            keyword_scope: self.keyword_scope,
            ..Default::default()
        };
        match self.keyword_scope {
            KeywordScope::Any => {
                active.keywords = normalize_keywords(self.filter_mode, self.keywords.clone())?;
            }
            KeywordScope::Fields => {
                active.field_keywords = self.field_keywords.normalized()?;
                active.validate_active_keywords()?;
            }
        }
        Ok(active)
    }

    /// 只有启用、目录或有效规则变化才需要重取边界；未使用的草稿不影响进度。
    pub fn needs_new_baseline(&self, next: &Self) -> bool {
        let words = |keywords: &[String]| -> BTreeSet<String> {
            keywords.iter().map(|word| word.to_lowercase()).collect()
        };
        let rules_changed = self.keyword_scope != next.keyword_scope
            || match next.keyword_scope {
                KeywordScope::Any => words(&self.keywords) != words(&next.keywords),
                KeywordScope::Fields => self
                    .field_keywords
                    .groups()
                    .iter()
                    .zip(next.field_keywords.groups())
                    .any(|(old, new)| words(old) != words(new)),
            };
        next.enabled
            && (!self.enabled
                || self.directory != next.directory
                || self.filter_mode != next.filter_mode
                || (next.filter_mode != FilterMode::None && rules_changed))
    }

    /// 在写文件前按活动范围匹配；字段之间及每个字段的关键词之间均为任意命中。
    pub fn should_save(&self, raw: &[u8]) -> Result<bool> {
        if self.filter_mode == FilterMode::None {
            return Ok(true);
        }
        let active = self.active_filter()?;
        let keywords = |values: &[String]| {
            values
                .iter()
                .map(|word| word.to_lowercase())
                .collect::<Vec<_>>()
        };
        let any = active.keyword_scope == KeywordScope::Any;
        let subject_words = keywords(if any {
            &active.keywords
        } else {
            &active.field_keywords.subject
        });
        let sender_words = keywords(if any {
            &active.keywords
        } else {
            &active.field_keywords.sender
        });
        let recipient_words = keywords(&active.field_keywords.recipient);
        let body_words = keywords(if any {
            &active.keywords
        } else {
            &active.field_keywords.body
        });
        let parse_body = !body_words.is_empty();
        // 只匹配头字段时无需展开 MIME 正文/附件，也不受无关正文解码错误影响。
        let raw = crate::mime::fix_headers(raw);
        let message = if parse_body {
            crate::mime::parse(&raw)?
        } else {
            MessageParser::default()
                .parse_headers(&raw)
                .ok_or_else(|| anyhow!("无法解析邮件头以检查下载关键词"))?
        };
        let matches_text = |text: &str, words: &[String]| {
            let text = text.to_lowercase();
            words.iter().any(|word| text.contains(word))
        };
        let matches_addresses = |addresses: Option<&Address<'_>>, words: &[String]| {
            addresses.is_some_and(|addresses| {
                addresses.iter().any(|address| {
                    address.name().is_some_and(|name| matches_text(name, words))
                        || address
                            .address()
                            .is_some_and(|address| matches_text(address, words))
                })
            })
        };
        let mut matched = message
            .subject()
            .is_some_and(|subject| matches_text(subject, &subject_words))
            || matches_addresses(message.from(), &sender_words)
            || (!any
                && message
                    .all_to()
                    .chain(message.all_cc())
                    .chain(message.all_bcc())
                    .any(|addresses| matches_addresses(Some(addresses), &recipient_words)));
        if parse_body {
            let body_ids: BTreeSet<_> = message
                .text_body
                .iter()
                .chain(&message.html_body)
                .copied()
                .collect();
            if message.parts.iter().enumerate().any(|(id, part)| {
                // Failed body decoding cannot silently pass a blacklist. Attachments do not participate.
                let explicit_attachment = part.attachment_name().is_some()
                    || part
                        .content_disposition()
                        .is_some_and(|d| d.ctype().eq_ignore_ascii_case("attachment"));
                let text_body_candidate = !explicit_attachment
                    && part.content_type().is_none_or(|ct| {
                        ct.ctype().eq_ignore_ascii_case("text")
                            && ct.subtype().is_none_or(|sub| {
                                sub.eq_ignore_ascii_case("plain")
                                    || sub.eq_ignore_ascii_case("html")
                            })
                    });
                part.is_encoding_problem && (body_ids.contains(&(id as u32)) || text_body_candidate)
            }) {
                bail!("邮件内容解码失败，无法可靠检查下载关键词");
            }
            // 使用 MIME 正文索引，不扫描附件、转发附件或原始 HTML 属性/脚本。
            for id in body_ids {
                if let Some(part) = message.part(id) {
                    matched |= match &part.body {
                        PartType::Text(text) => matches_text(text, &body_words),
                        PartType::Html(html) => {
                            let document = dom_query::Document::from(html.as_ref());
                            document
                                .select("head, script, style, template, [hidden]")
                                .remove();
                            matches_text(&document.select("body").formatted_text(), &body_words)
                        }
                        _ => false,
                    };
                }
            }
        }
        Ok(match self.filter_mode {
            FilterMode::Whitelist => matched,
            FilterMode::Blacklist => !matched,
            FilterMode::None => true,
        })
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Checkpoint {
    pub uid_validity: u32,
    pub last_uid: u32,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub detail: String,
    pub saved_count: u64,
    pub last_path: Option<String>,
}

impl Status {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            state: if settings.enabled {
                "waiting"
            } else {
                "disabled"
            }
            .into(),
            detail: if settings.enabled {
                "等待新邮件；应用运行时自动下载"
            } else {
                "未开启"
            }
            .into(),
            saved_count: settings.saved_count,
            last_path: settings.last_path.clone(),
        }
    }
}

/// 必须明确选择现有绝对目录；写入探针在成功与失败时均自动清理。
pub fn validate_directory(directory: &str) -> Result<PathBuf> {
    let path = Path::new(directory);
    if !path.is_absolute() || directory.chars().any(char::is_control) {
        bail!("请选择有效的本地绝对路径");
    }
    let canonical = fs::canonicalize(path).context("下载目录不存在或无法访问")?;
    if !canonical.is_dir() {
        bail!("下载路径必须是文件夹");
    }
    let mut probe = tempfile::Builder::new()
        .prefix(".mailpulse-write-test-")
        .tempfile_in(&canonical)
        .context("下载目录不可写，请选择其他文件夹")?;
    probe.write_all(b"MailBeacon").context("下载目录不可写")?;
    probe
        .as_file()
        .sync_all()
        .context("无法确认下载目录写入成功")?;
    Ok(canonical)
}

/// 读取启用瞬间的边界；没有可靠 UID 信息时拒绝开启，避免下载历史邮件。
pub async fn baseline<T>(s: &mut async_imap::Session<T>) -> Result<Checkpoint>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    download_protocol::checked_boundary(s, "INBOX").await
}

/// 一个有界 UID 窗口，全部写入后才可越过窗口中已删除的 UID。
pub struct Batch {
    pub uids: Vec<u32>,
    pub through: u32,
    pub more: bool,
}

pub async fn next_batch<T>(s: &mut async_imap::Session<T>, checkpoint: Checkpoint) -> Result<Batch>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let current = baseline(s).await?;
    if current.uid_validity != checkpoint.uid_validity || current.last_uid < checkpoint.last_uid {
        return Err(MailboxIdentityChanged.into());
    }
    if current.last_uid == checkpoint.last_uid {
        return Ok(Batch {
            uids: vec![],
            through: checkpoint.last_uid,
            more: false,
        });
    }
    let start = checkpoint.last_uid + 1;
    let end = checkpoint
        .last_uid
        .saturating_add(UID_WINDOW)
        .min(current.last_uid);
    let mut uids = download_protocol::checked_search(s, start, end).await?;
    let through = if uids.len() > BATCH {
        uids[BATCH - 1]
    } else {
        end
    };
    uids.truncate(BATCH);
    Ok(Batch {
        uids,
        through,
        more: through < current.last_uid,
    })
}

/// PEEK 读取原文不改变未读标记；精确确认删除竞争后才跳过已消失的邮件。
pub async fn raw_message<T>(s: &mut async_imap::Session<T>, uid: u32) -> Result<Option<Vec<u8>>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    download_protocol::checked_raw_message(s, uid).await
}

fn archive_name(account_id: &str, checkpoint: Checkpoint, uid: u32) -> String {
    archive_name_for(account_id, checkpoint.uid_validity, uid)
}

fn archive_name_for(account_id: &str, uid_validity: u32, uid: u32) -> String {
    format!("{}{uid}.eml", archive_prefix(account_id, uid_validity))
}

fn archive_prefix(account_id: &str, uid_validity: u32) -> String {
    format!(
        "MailBeacon-{}-{uid_validity}-",
        URL_SAFE_NO_PAD.encode(account_id.as_bytes())
    )
}

/// 普通文件（不是符号链接）才算数；不存在返回 false
fn plain_file(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(metadata.is_file() && !metadata.file_type().is_symlink()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("无法读取已下载的邮件"),
    }
}

/// 只读：在一个下载目录里查这封邮件保存成了哪个文件。不加锁、不创建也不改任何东西
/// （下载是先写临时文件再改名发布的，读不到半封）。
pub(crate) fn find_archived(
    dir: &Path,
    account_id: &str,
    uid_validity: u32,
    uid: u32,
) -> Result<Option<PathBuf>> {
    let identity = archive_name_for(account_id, uid_validity, uid);
    let mapping = dir
        .join(".mailpulse-index")
        .join("identities")
        .join(format!("{identity}.json"));
    if let Some(record) = read_record(&mapping)? {
        if record.identity != identity {
            bail!("邮件下载索引身份不一致：{}", mapping.display());
        }
        let path = dir.join(&record.filename);
        return Ok(plain_file(&path)?.then_some(path));
    }
    // 旧版直接以身份命名保存的文件
    let legacy = dir.join(&identity);
    Ok(plain_file(&legacy)?.then_some(legacy))
}

/// 只读：一个下载目录里某账号、某一代 UIDVALIDITY 已保存的 UID（只看文件名，不打开文件）。
/// 目录不存在或读不了时返回空集合。
pub(crate) fn archived_uids(dir: &Path, account_id: &str, uid_validity: u32) -> BTreeSet<u32> {
    let prefix = archive_prefix(account_id, uid_validity);
    // 只认保存时写出的样子：纯数字、不以 0 开头（「+5」「05」都不是）
    let uid_of = |name: &str, suffix: &str| {
        let digits = name.strip_prefix(&prefix)?.strip_suffix(suffix)?;
        if digits.starts_with('0') || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        digits.parse::<u32>().ok()
    };
    let names = |dir: &Path| -> Vec<String> {
        fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect()
    };
    let indexed = names(&dir.join(".mailpulse-index").join("identities"))
        .into_iter()
        .filter_map(|name| uid_of(&name, ".eml.json"));
    let legacy = names(dir)
        .into_iter()
        .filter_map(|name| uid_of(&name, ".eml"));
    indexed.chain(legacy).collect()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ArchiveRecord {
    version: u32,
    identity: String,
    filename: String,
}

fn sync_directory(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    fs::File::open(dir)?
        .sync_all()
        .context("确认邮件目录写入失败")?;
    Ok(())
}

fn index_directory(parent: &Path, name: &str) -> Result<PathBuf> {
    let path = parent.join(name);
    let mut builder = fs::DirBuilder::new();
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&path) {
        Ok(()) => sync_directory(parent)?,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("无法创建邮件下载索引"),
    }
    let metadata = fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("邮件下载索引不是普通文件夹：{}", path.display());
    }
    Ok(path)
}

fn read_record(path: &Path) -> Result<Option<ArchiveRecord>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("无法读取邮件下载索引"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("邮件下载索引不是普通文件：{}", path.display());
    }
    let record: ArchiveRecord = serde_json::from_slice(&fs::read(path)?)
        .context("邮件下载索引损坏，请保留现有文件并检查保存目录")?;
    if record.version != 1
        || record.identity.is_empty()
        || record.filename.is_empty()
        || record.filename.contains(['/', '\\'])
        || record.filename.chars().any(char::is_control)
        || !record.filename.ends_with(".eml")
        || Path::new(&record.filename).components().count() != 1
    {
        bail!("邮件下载索引包含无效文件名或版本：{}", path.display());
    }
    Ok(Some(record))
}

fn publish_record(path: &Path, record: &ArchiveRecord) -> Result<()> {
    if let Some(existing) = read_record(path)? {
        if existing != *record {
            bail!("邮件下载索引身份冲突，已保留原文件：{}", path.display());
        }
        return sync_directory(path.parent().unwrap());
    }
    let parent = path.parent().unwrap();
    let mut temporary = tempfile::Builder::new()
        .prefix(".mailpulse-")
        .tempfile_in(parent)?;
    temporary.write_all(&serde_json::to_vec(record)?)?;
    temporary.as_file().sync_all()?;
    match temporary.persist_noclobber(path) {
        Ok(_) => {}
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_record(path)?.as_ref() != Some(record) {
                bail!("邮件下载索引身份冲突，已保留原文件：{}", path.display());
            }
        }
        Err(error) => return Err(error.error).context("保存邮件下载索引失败"),
    }
    sync_directory(parent)
}

fn verify_original(path: &Path, raw: &[u8]) -> Result<bool> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(error).context("无法读取已下载的邮件"),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        bail!("同名路径不是普通邮件文件，已停止写入：{}", path.display());
    }
    if fs::read(path).context("无法核对已下载的邮件")? != raw {
        bail!(
            "同名邮件内容不一致，已保留原文件，请移走冲突文件后重试：{}",
            path.display()
        );
    }
    Ok(true)
}

/// 完整身份索引确保同主题甚至相同原文的不同 UID 分开保存。
/// 先持久化名称预留和身份映射，再原子发布原文；失败后继续使用已预留的文件名。
/// 返回实际路径，以及是否核对复用了已有（包括旧版命名）邮件。
pub(crate) fn save_atomic_with_status(
    dir: &Path,
    account_id: &str,
    checkpoint: Checkpoint,
    uid: u32,
    raw: &[u8],
) -> Result<(PathBuf, bool)> {
    if !dir.is_dir() {
        bail!("下载目录已不存在，请恢复目录或修改设置");
    }
    let index = index_directory(dir, ".mailpulse-index")?;
    let lock_path = index.join(".lock");
    if let Ok(metadata) = fs::symlink_metadata(&lock_path) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("邮件下载索引锁不是普通文件");
        }
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(&lock_path).context("无法锁定邮件下载索引")?;
    lock.lock().context("无法锁定邮件下载索引")?;
    // 文件锁随句柄释放；多账号共享目录及多个进程均使用同一锁，崩溃不留下死锁。
    let identities = index_directory(&index, "identities")?;
    let names = index_directory(&index, "names")?;
    let identity = archive_name(account_id, checkpoint, uid);
    let legacy = dir.join(&identity);
    let legacy_claim = read_record(&names.join(format!("{identity}.json")))?;
    let legacy_exists = if legacy_claim.is_none() {
        verify_original(&legacy, raw)?
    } else {
        // 已预留的名称属于新格式文件；大小写不敏感系统中可能正是当前主题名的别名。
        false
    };
    let mapping_path = identities.join(format!("{identity}.json"));
    let record = if let Some(record) = read_record(&mapping_path)? {
        if record.identity != identity {
            bail!("邮件下载索引身份不一致，已保留现有文件");
        }
        publish_record(&names.join(format!("{}.json", record.filename)), &record)?;
        record
    } else {
        let stem = crate::export::subject_stem(raw, uid);
        let record = crate::export::with_filesystem_stem(&names, &stem, |stem| {
            let mut chosen = None;
            for number in 1u64.. {
                let filename = if number == 1 {
                    format!("{stem}.eml")
                } else {
                    format!("{stem} ({number}).eml")
                };
                let claim = names.join(format!("{filename}.json"));
                if let Some(reserved) = read_record(&claim)? {
                    if reserved.identity == identity {
                        if reserved.filename != filename {
                            bail!("邮件下载名称预留与索引文件名不一致，已保留现有文件");
                        }
                        // 上次可能只完成了名称预留，尚未发布身份映射或邮件。
                        chosen = Some(reserved);
                        break;
                    }
                    continue;
                }
                match fs::symlink_metadata(dir.join(&filename)) {
                    Ok(_) => continue, // 无身份关联的同名文件不按字节合并，也绝不覆盖。
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error).context("无法检查邮件保存路径"),
                }
                let record = ArchiveRecord {
                    version: 1,
                    identity: identity.clone(),
                    filename,
                };
                publish_record(&claim, &record)?;
                chosen = Some(record);
                break;
            }
            chosen.ok_or_else(|| anyhow!("无法分配邮件文件名"))
        })?;
        publish_record(&mapping_path, &record)?;
        record
    };
    let path = dir.join(&record.filename);
    let existing = verify_original(&path, raw)?;
    if !existing {
        let mut temporary = tempfile::Builder::new()
            .prefix(".mailpulse-")
            .tempfile_in(dir)
            .context("无法写入自动下载目录")?;
        temporary.write_all(raw).context("写入邮件失败")?;
        temporary.as_file().sync_all().context("确认邮件写入失败")?;
        match temporary.persist_noclobber(&path) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                verify_original(&path, raw)?;
            }
            Err(error) => return Err(error.error).context("保存邮件文件失败"),
        }
    }
    sync_directory(dir)?;
    if legacy_exists && legacy != path {
        // 先确认新文件和索引已持久化，才移除已再次核对的旧名；中途崩溃可重试完成。
        verify_original(&legacy, raw)?;
        fs::remove_file(&legacy).context("迁移完成，但旧邮件文件未能清理，请重试")?;
        sync_directory(dir)?;
    }
    Ok((path, legacy_exists || existing))
}

/// 临时文件写完并同步后才原子发布；以主题命名，同一身份重试复用，冲突不覆盖。
pub fn save_atomic(
    dir: &Path,
    account_id: &str,
    checkpoint: Checkpoint,
    uid: u32,
    raw: &[u8],
) -> Result<PathBuf> {
    save_atomic_with_status(dir, account_id, checkpoint, uid, raw).map(|(path, _)| path)
}

/// 调用者持有该账号的提交锁；设置变更或移除后的旧网络结果被丢弃。
pub fn commit(
    store: &Mutex<Store>,
    id: &str,
    revision: u64,
    checkpoint: Checkpoint,
    raw: Option<(u32, &[u8])>,
) -> Result<bool> {
    let mut settings = {
        let guard = store.lock().unwrap();
        let Some(account) = guard.get(id) else {
            return Ok(false);
        };
        let settings = &account.auto_download;
        if !settings.enabled || settings.revision != revision {
            return Ok(false);
        }
        settings.clone()
    };
    if settings.checkpoint.is_some_and(|old| {
        old.uid_validity != checkpoint.uid_validity || old.last_uid > checkpoint.last_uid
    }) {
        bail!("自动下载进度发生变化，稍后重试");
    }
    if let Some((uid, raw)) = raw {
        if settings.checkpoint.is_some_and(|old| old.last_uid >= uid) {
            return Ok(true);
        }
        if settings.should_save(raw)? {
            let path = save_atomic(Path::new(&settings.directory), id, checkpoint, uid, raw)?;
            // 累计值来自持久配置；极端或手动编辑的边界值不应阻止后续邮件保存。
            settings.saved_count = settings.saved_count.saturating_add(1);
            settings.last_path = Some(path.display().to_string());
        }
    }
    settings.checkpoint = Some(checkpoint);
    store.lock().unwrap().set_auto_download(id, settings)?;
    Ok(true)
}

fn emit_status(app: &AppHandle, id: &str, revision: u64, state_name: &str, detail: String) {
    let state = app.state::<AppState>();
    let store = state.store.lock().unwrap();
    let Some(account) = store.get(id) else {
        return;
    };
    if account.auto_download.revision != revision {
        return;
    }
    let settings = &account.auto_download;
    // 保持设置锁到事件入队，避免旧任务在关闭成功之后发出过时状态。
    sync::emit(
        app,
        Event::AutoDownload {
            account_id: id,
            state: state_name,
            detail,
            last_path: settings.last_path.clone(),
            saved_count: settings.saved_count,
        },
    );
}

async fn commit_async(
    app: &AppHandle,
    id: &str,
    revision: u64,
    checkpoint: Checkpoint,
    raw: Option<(u32, Vec<u8>)>,
) -> Result<bool> {
    let gate = app.state::<AppState>().download_gate(id).lock_owned().await;
    let handle = app.clone();
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let _gate = gate;
        commit(
            &handle.state::<AppState>().store,
            &id,
            revision,
            checkpoint,
            raw.as_ref().map(|(uid, bytes)| (*uid, bytes.as_slice())),
        )
    })
    .await
    .context("自动下载写入任务中断")?
}

async fn download_batch(app: &AppHandle, account: &Account, s: &mut Imap) -> Result<bool> {
    let settings = &account.auto_download;
    let mut checkpoint = settings
        .checkpoint
        .ok_or_else(|| anyhow!("自动下载进度缺失，请关闭后重新开启"))?;
    let batch = next_batch(s, checkpoint).await?;
    if !batch.uids.is_empty() {
        emit_status(
            app,
            &account.id,
            settings.revision,
            "downloading",
            "正在保存新邮件…".into(),
        );
    }
    for uid in batch.uids {
        // 设置可能在任何网络等待期间改变；写入时再次持锁检查。
        let still_current = app
            .state::<AppState>()
            .store
            .lock()
            .unwrap()
            .get(&account.id)
            .is_some_and(|a| {
                a.auto_download.enabled && a.auto_download.revision == settings.revision
            });
        if !still_current {
            return Ok(false);
        }
        let raw = raw_message(s, uid).await?;
        checkpoint.last_uid = uid;
        if !commit_async(
            app,
            &account.id,
            settings.revision,
            checkpoint,
            raw.map(|raw| (uid, raw)),
        )
        .await?
        {
            return Ok(false);
        }
    }
    if batch.through > checkpoint.last_uid {
        checkpoint.last_uid = batch.through;
        if !commit_async(app, &account.id, settings.revision, checkpoint, None).await? {
            return Ok(false);
        }
    }
    emit_status(
        app,
        &account.id,
        settings.revision,
        "waiting",
        "等待新邮件；应用运行时自动下载".into(),
    );
    Ok(batch.more)
}

/// 常驻但低开销的独立工作任务：收件事件立即唤醒，错误与离线也会定时重试。
pub async fn watch(app: AppHandle, id: String) {
    let state = app.state::<AppState>();
    let mut triggers = state.download_wake.subscribe();
    let mut wake = state.wake.subscribe();
    drop(state);
    let mut session: Option<Imap> = None;
    let mut previous_revision = None;
    let mut blocked_revision = None;
    loop {
        let account = app.state::<AppState>().account(&id);
        let (account, pass) = match account {
            Ok(value) => value,
            Err(error) => {
                let config = app
                    .state::<AppState>()
                    .store
                    .lock()
                    .unwrap()
                    .get(&id)
                    .cloned();
                let Some(account) = config else {
                    break;
                };
                if account.auto_download.enabled {
                    emit_status(
                        &app,
                        &id,
                        account.auto_download.revision,
                        "error",
                        format!("自动下载暂停：{error:#}"),
                    );
                }
                tokio::select! { _ = tokio::time::sleep(RETRY) => {}, _ = triggers.recv() => {}, _ = wake.recv() => {} }
                continue;
            }
        };
        let settings = &account.auto_download;
        if previous_revision != Some(settings.revision) {
            session = None;
            previous_revision = Some(settings.revision);
            blocked_revision = None;
        }
        let mut more = false;
        if settings.enabled && blocked_revision != Some(settings.revision) {
            let result = async {
                if session.is_none() {
                    session = Some(imap::login(&account, &pass).await?);
                }
                download_batch(&app, &account, session.as_mut().unwrap()).await
            }
            .await;
            match result {
                Ok(pending) => more = pending,
                Err(error) if error.is::<MailboxIdentityChanged>() => {
                    session = None;
                    blocked_revision = Some(settings.revision);
                    emit_status(
                        &app,
                        &id,
                        settings.revision,
                        "error",
                        format!("自动下载已暂停：{error}"),
                    );
                }
                Err(error) if error.is::<imap::AuthFailed>() => {
                    session = None;
                    blocked_revision = Some(settings.revision);
                    emit_status(
                        &app,
                        &id,
                        settings.revision,
                        "error",
                        "邮箱认证失败，自动下载已暂停；请检查授权码并重新添加账号".into(),
                    );
                }
                Err(error) => {
                    session = None;
                    emit_status(
                        &app,
                        &id,
                        settings.revision,
                        "error",
                        format!("自动下载失败，将自动重试：{error:#}"),
                    );
                }
            }
        } else {
            session = None;
        }
        if more {
            tokio::time::sleep(Duration::from_millis(25)).await;
            continue;
        }
        // 其他账号的消息无需打断当前账号的定时重试。
        let sleep = tokio::time::sleep(RETRY);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                _ = &mut sleep => break,
                message = triggers.recv() => match message {
                    Ok(account) if account != id => continue,
                    _ => break,
                },
                _ = wake.recv() => { session = None; break; },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    async fn scripted_session(
        replies: Vec<(String, Option<&'static str>)>,
    ) -> async_imap::Session<DuplexStream> {
        let (client, server) = tokio::io::duplex(8192);
        tokio::spawn(async move {
            let mut server = BufReader::new(server);
            let mut line = String::new();
            for (data, completion) in
                std::iter::once((String::new(), Some("OK login"))).chain(replies)
            {
                line.clear();
                if server.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let tag = line.split_whitespace().next().unwrap().to_string();
                server.write_all(data.as_bytes()).await.unwrap();
                if let Some(completion) = completion {
                    server
                        .write_all(format!("{tag} {completion}\r\n").as_bytes())
                        .await
                        .unwrap();
                } else {
                    break;
                }
            }
        });
        async_imap::Client::new(client)
            .login("test", "pw")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn automatic_search_does_not_advance_on_rejection_or_unconfirmed_result() {
        for completion in [Some("NO unavailable"), Some("BAD rejected"), None] {
            let mut session = scripted_session(vec![
                (
                    "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 21] next\r\n".into(),
                    Some("OK selected"),
                ),
                ("* SEARCH\r\n".into(), completion),
            ])
            .await;
            let result = next_batch(
                &mut session,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: 10,
                },
            )
            .await;
            assert!(
                result.is_err(),
                "failed or incomplete SEARCH must not advance checkpoint: {completion:?}"
            );
        }
        let mut missing = scripted_session(vec![
            (
                "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 21] next\r\n".into(),
                Some("OK selected"),
            ),
            (String::new(), Some("OK no search data")),
        ])
        .await;
        assert!(
            next_batch(
                &mut missing,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: 10
                }
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn automatic_fetch_requires_success_before_saving_or_confirming_disappearance() {
        let raw = "Subject: original\r\n\r\nbody";
        for completion in [Some("NO unavailable"), Some("BAD rejected"), None] {
            let mut session = scripted_session(vec![(
                format!("* 1 FETCH (UID 11 BODY[] {{{}}}\r\n{raw})\r\n", raw.len()),
                completion,
            )])
            .await;
            assert!(
                raw_message(&mut session, 11).await.is_err(),
                "unconfirmed FETCH must not be saved: {completion:?}"
            );
            let mut session = scripted_session(vec![
                (String::new(), Some("OK empty fetch")),
                ("* SEARCH\r\n".into(), completion),
            ])
            .await;
            assert!(
                raw_message(&mut session, 11).await.is_err(),
                "unconfirmed SEARCH must not mean vanished: {completion:?}"
            );
        }
        let mut vanished = scripted_session(vec![
            (String::new(), Some("OK empty fetch")),
            ("* SEARCH\r\n".into(), Some("OK searched")),
        ])
        .await;
        assert_eq!(raw_message(&mut vanished, 11).await.unwrap(), None);
    }

    #[tokio::test]
    async fn automatic_baseline_requires_completed_mailbox_selection() {
        let mut session = scripted_session(vec![(
            "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT 21] next\r\n".into(),
            None,
        )])
        .await;
        assert!(
            baseline(&mut session).await.is_err(),
            "unconfirmed mailbox selection must not be accepted"
        );
    }

    #[tokio::test]
    async fn automatic_fetch_accepts_body_after_unsolicited_flags_for_the_same_uid() {
        let raw = "Subject: original\r\n\r\nbody";
        let mut session = scripted_session(vec![(
            format!("* 1 FETCH (UID 11 FLAGS (\\Seen))\r\n* 1 FETCH (UID 11 BODY[] {{{}}}\r\n{raw})\r\n", raw.len()),
            Some("OK fetched"),
        )]).await;
        assert_eq!(
            raw_message(&mut session, 11).await.unwrap(),
            Some(raw.as_bytes().to_vec())
        );
    }

    #[tokio::test]
    async fn automatic_uid_window_stays_bounded_at_u32_max() {
        let mut session = scripted_session(vec![
            (
                format!(
                    "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT {}] next\r\n",
                    u32::MAX
                ),
                Some("OK selected"),
            ),
            (
                format!("* SEARCH 1 {} {}\r\n", u32::MAX - 1, u32::MAX),
                Some("OK searched"),
            ),
        ])
        .await;
        let batch = next_batch(
            &mut session,
            Checkpoint {
                uid_validity: 7,
                last_uid: u32::MAX - 2,
            },
        )
        .await
        .unwrap();
        assert_eq!(batch.uids, vec![u32::MAX - 1]);
        assert_eq!(batch.through, u32::MAX - 1);
        assert!(!batch.more);
        let mut session = scripted_session(vec![(
            format!(
                "* OK [UIDVALIDITY 7] valid\r\n* OK [UIDNEXT {}] next\r\n",
                u32::MAX
            ),
            Some("OK selected"),
        )])
        .await;
        assert!(
            next_batch(
                &mut session,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: u32::MAX
                }
            )
            .await
            .is_err()
        );
    }

    fn eml_files(dir: &Path) -> Vec<PathBuf> {
        fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "eml"))
            .collect()
    }

    #[test]
    fn subject_names_decode_chinese_and_preserve_distinct_message_identities() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 8,
            last_uid: 4,
        };
        let raw = b"Subject: =?UTF-8?B?5rWL6K+V?=\r\n\r\nidentical bytes";
        let first = save_atomic(dir.path(), "one", cp, 1, raw).unwrap();
        let second = save_atomic(dir.path(), "one", cp, 2, raw).unwrap();
        let third = save_atomic(dir.path(), "two", cp, 1, raw).unwrap();
        assert_eq!(first.file_name().unwrap(), "测试.eml");
        assert_eq!(second.file_name().unwrap(), "测试 (2).eml");
        assert_eq!(third.file_name().unwrap(), "测试 (3).eml");
        assert_eq!(save_atomic(dir.path(), "one", cp, 1, raw).unwrap(), first);
        assert_eq!(save_atomic(dir.path(), "one", cp, 2, raw).unwrap(), second);
        let empty = save_atomic(dir.path(), "one", cp, 4, b"Subject: \r\n\r\nbody").unwrap();
        assert_eq!(empty.file_name().unwrap(), "邮件-4.eml");
        assert_eq!(eml_files(dir.path()).len(), 4);
        for path in [first, second, third] {
            assert_eq!(fs::read(path).unwrap(), raw);
        }
    }

    #[test]
    fn legacy_files_migrate_only_after_verifying_original_and_never_replace_other_titles() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 8,
            last_uid: 1,
        };
        let raw = b"Subject: Legacy\r\n\r\noriginal";
        let legacy = dir.path().join(archive_name("one", cp, 1));
        fs::write(&legacy, raw).unwrap();
        fs::write(dir.path().join("Legacy.eml"), b"unrelated same title").unwrap();
        let (path, existed) = save_atomic_with_status(dir.path(), "one", cp, 1, raw).unwrap();
        assert!(existed);
        assert_eq!(path.file_name().unwrap(), "Legacy (2).eml");
        assert!(!legacy.exists());
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_eq!(
            fs::read(dir.path().join("Legacy.eml")).unwrap(),
            b"unrelated same title"
        );
        assert_eq!(save_atomic(dir.path(), "one", cp, 1, raw).unwrap(), path);
        assert_eq!(eml_files(dir.path()).len(), 2);
        // 模拟新名已发布而旧名尚未来得及清理的崩溃窗口。
        fs::write(&legacy, raw).unwrap();
        assert!(
            save_atomic_with_status(dir.path(), "one", cp, 1, raw)
                .unwrap()
                .1
        );
        assert!(!legacy.exists());
        fs::write(&legacy, b"conflicting old file").unwrap();
        assert!(save_atomic(dir.path(), "one", cp, 1, raw).is_err());
        assert_eq!(fs::read(&legacy).unwrap(), b"conflicting old file");
        assert_eq!(fs::read(&path).unwrap(), raw);
    }

    #[test]
    fn rejected_unicode_subjects_save_resume_and_migrate_without_overwriting() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 1621562782,
            last_uid: 646,
        };
        let raw = "Subject: 通知\u{03a2}\u{ffff}📨\r\n\r\noriginal MIME bytes".as_bytes();
        let unrelated = dir.path().join("通知__📨.eml");
        fs::write(&unrelated, b"unrelated message").unwrap();
        let (first, existed) = save_atomic_with_status(dir.path(), "one", cp, 644, raw).unwrap();
        assert!(!existed);
        assert_eq!(first.file_name().unwrap(), "通知__📨 (2).eml");
        // Different subjects may sanitize to the same stem; never merge their UIDs.
        let colliding_raw =
            "Subject: 通知\u{0378}\u{fffe}📨\r\n\r\ndifferent MIME bytes".as_bytes();
        let second = save_atomic(dir.path(), "one", cp, 645, colliding_raw).unwrap();
        assert_eq!(second.file_name().unwrap(), "通知__📨 (3).eml");
        assert_eq!(
            save_atomic_with_status(dir.path(), "one", cp, 644, raw).unwrap(),
            (first.clone(), true)
        );
        // Resume with only the durable name reservation remaining.
        let mapping = dir
            .path()
            .join(".mailpulse-index/identities")
            .join(format!("{}.json", archive_name("one", cp, 644)));
        fs::remove_file(&mapping).unwrap();
        fs::remove_file(&first).unwrap();
        assert_eq!(
            save_atomic_with_status(dir.path(), "one", cp, 644, raw).unwrap(),
            (first.clone(), false)
        );
        let legacy = dir.path().join(archive_name("one", cp, 646));
        fs::write(&legacy, raw).unwrap();
        let (migrated, existed) = save_atomic_with_status(dir.path(), "one", cp, 646, raw).unwrap();
        assert!(existed);
        assert_eq!(migrated.file_name().unwrap(), "通知__📨 (4).eml");
        assert!(!legacy.exists());
        assert_eq!(fs::read(&second).unwrap(), colliding_raw);
        assert_eq!(
            save_atomic(dir.path(), "one", cp, 645, colliding_raw).unwrap(),
            second
        );
        for path in [first, migrated] {
            assert_eq!(fs::read(path).unwrap(), raw);
        }
        assert_eq!(fs::read(unrelated).unwrap(), b"unrelated message");
        assert_eq!(eml_files(dir.path()).len(), 4);
    }

    #[test]
    fn subject_matching_legacy_identity_case_alias_survives_retry() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 8,
            last_uid: 1,
        };
        let identity = archive_name("one", cp, 1);
        let subject = identity.strip_suffix(".eml").unwrap().to_lowercase();
        let raw = format!("Subject: {subject}\r\n\r\noriginal");
        let first = save_atomic(dir.path(), "one", cp, 1, raw.as_bytes()).unwrap();
        assert_eq!(fs::read(&first).unwrap(), raw.as_bytes());
        let retried = save_atomic_with_status(dir.path(), "one", cp, 1, raw.as_bytes()).unwrap();
        assert_eq!(retried, (first.clone(), true));
        assert_eq!(fs::read(&first).unwrap(), raw.as_bytes());
        assert_eq!(eml_files(dir.path()).len(), 1);
    }

    #[test]
    fn assigned_unicode_subjects_save_and_resume_with_the_same_identity() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 1,
            last_uid: 2,
        };
        let message = format!(
            "Subject: 通知{}\r\n\r\noriginal MIME bytes",
            "\u{1fae9}\u{1e6c0}📨".repeat(100)
        );
        let raw = message.as_bytes();
        let (first, existed) = save_atomic_with_status(dir.path(), "one", cp, 1, raw).unwrap();
        assert!(!existed);
        assert_eq!(
            save_atomic_with_status(dir.path(), "one", cp, 1, raw).unwrap(),
            (first.clone(), true)
        );
        let second = save_atomic(dir.path(), "one", cp, 2, raw).unwrap();
        assert_ne!(first, second);
        for path in [&first, &second] {
            let filename = path.file_name().unwrap().to_str().unwrap();
            assert!(filename.starts_with("通知"));
            assert!(filename.len() <= 188);
        }
        for entry in fs::read_dir(dir.path().join(".mailpulse-index/names")).unwrap() {
            assert!(entry.unwrap().file_name().to_str().unwrap().len() < 255);
        }
        fs::remove_file(&first).unwrap();
        assert_eq!(save_atomic(dir.path(), "one", cp, 1, raw).unwrap(), first);
        assert_eq!(eml_files(dir.path()).len(), 2);
        assert_eq!(fs::read(first).unwrap(), raw);
        assert_eq!(fs::read(second).unwrap(), raw);
    }

    #[test]
    fn reserved_names_and_mappings_recover_before_and_after_original_publication() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 9,
            last_uid: 1,
        };
        let raw = b"Subject: Resume\r\n\r\noriginal";
        let path = save_atomic(dir.path(), "one", cp, 1, raw).unwrap();
        let identity = archive_name("one", cp, 1);
        let index = dir.path().join(".mailpulse-index");
        let mapping = index.join("identities").join(format!("{identity}.json"));
        fs::remove_file(&path).unwrap();
        fs::remove_file(&mapping).unwrap(); // 仅名称预留持久化。
        let (resumed, existing) = save_atomic_with_status(dir.path(), "one", cp, 1, raw).unwrap();
        assert_eq!(resumed, path);
        assert!(!existing);
        assert_eq!(eml_files(dir.path()).len(), 1);
        fs::remove_file(&path).unwrap(); // 映射持久化但原文尚未发布。
        assert_eq!(save_atomic(dir.path(), "one", cp, 1, raw).unwrap(), path);
        fs::write(&path, b"unexpected local edit").unwrap();
        assert!(save_atomic(dir.path(), "one", cp, 1, raw).is_err());
        assert_eq!(eml_files(dir.path()).len(), 1);
        assert_eq!(fs::read(&path).unwrap(), b"unexpected local edit");
    }

    #[test]
    fn concurrent_subject_downloads_keep_unique_uids_and_reuse_each_identity() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 10,
            last_uid: 4,
        };
        let raw = b"Subject: Shared\r\n\r\nidentical bytes";
        let workers: Vec<_> = (0..12)
            .map(|index| {
                let directory = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    (
                        index % 4 + 1,
                        save_atomic(&directory, "one", cp, index % 4 + 1, raw).unwrap(),
                    )
                })
            })
            .collect();
        let mut paths = std::collections::HashMap::new();
        for worker in workers {
            let (uid, path) = worker.join().unwrap();
            if let Some(existing) = paths.insert(uid, path.clone()) {
                assert_eq!(existing, path);
            }
            assert_eq!(fs::read(path).unwrap(), raw);
        }
        assert_eq!(
            paths
                .values()
                .collect::<std::collections::HashSet<_>>()
                .len(),
            4
        );
        assert_eq!(eml_files(dir.path()).len(), 4);
    }

    #[test]
    fn invalid_index_data_never_overwrites_or_silently_allocates_another_name() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 11,
            last_uid: 1,
        };
        let raw = b"Subject: Protected\r\n\r\noriginal";
        let path = save_atomic(dir.path(), "one", cp, 1, raw).unwrap();
        let index = dir.path().join(".mailpulse-index");
        let mapping = index
            .join("identities")
            .join(format!("{}.json", archive_name("one", cp, 1)));
        fs::write(&mapping, b"broken JSON").unwrap();
        assert!(save_atomic(dir.path(), "one", cp, 1, raw).is_err());
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_eq!(eml_files(dir.path()).len(), 1);
        assert_eq!(fs::read(&mapping).unwrap(), b"broken JSON");
    }

    #[cfg(unix)]
    #[test]
    fn archive_rejects_symlinked_indexes_locks_and_mapped_messages() {
        use std::os::unix::fs::symlink;
        let outside = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 12,
            last_uid: 1,
        };
        let raw = b"Subject: Safe\r\n\r\noriginal";
        for component in [
            ".mailpulse-index",
            ".mailpulse-index/.lock",
            ".mailpulse-index/names",
            ".mailpulse-index/identities",
        ] {
            let dir = tempfile::tempdir().unwrap();
            let link = dir.path().join(component);
            fs::create_dir_all(link.parent().unwrap()).unwrap();
            symlink(outside.path(), &link).unwrap();
            assert!(
                save_atomic(dir.path(), "one", cp, 1, raw).is_err(),
                "accepted {component}"
            );
            assert!(eml_files(dir.path()).is_empty());
        }
        let dir = tempfile::tempdir().unwrap();
        let path = save_atomic(dir.path(), "one", cp, 1, raw).unwrap();
        let target = outside.path().join("outside.eml");
        fs::write(&target, raw).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(save_atomic(dir.path(), "one", cp, 1, raw).is_err());
        assert_eq!(fs::read(&target).unwrap(), raw);
    }
    #[test]
    fn filter_settings_migrate_and_accept_only_one_mode() {
        let old: Settings =
            serde_json::from_str(r#"{"enabled":false,"directory":"/tmp"}"#).unwrap();
        assert_eq!(old.filter_mode, FilterMode::None);
        assert!(old.keywords.is_empty());
        assert_eq!(old.keyword_scope, KeywordScope::Any);
        assert!(old.field_keywords.is_empty());
        assert!(serde_json::from_str::<Settings>(r#"{"filterMode":"both"}"#).is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"keywordScope":"all"}"#).is_err());
        assert_eq!(
            serde_json::to_value(FilterMode::Whitelist).unwrap(),
            "whitelist"
        );
        assert_eq!(
            serde_json::to_value(FilterMode::Blacklist).unwrap(),
            "blacklist"
        );
    }

    #[test]
    fn keywords_normalize_validate_and_compare_effective_rules() {
        let words = normalize_keywords(
            FilterMode::Whitelist,
            vec![
                "  发票  ".into(),
                "APPLE".into(),
                "apple".into(),
                " ".into(),
            ],
        )
        .unwrap();
        assert_eq!(words, ["发票", "APPLE"]);
        assert!(normalize_keywords(FilterMode::Blacklist, vec![" ".into()]).is_err());
        assert!(normalize_keywords(FilterMode::None, vec![]).is_ok());
        assert!(normalize_keywords(FilterMode::Whitelist, vec!["中".repeat(201)]).is_err());
        assert!(
            normalize_keywords(
                FilterMode::Whitelist,
                (0..101).map(|i| format!("word{i}")).collect()
            )
            .is_err()
        );
        let mut old = Settings {
            enabled: true,
            directory: "/tmp".into(),
            filter_mode: FilterMode::Whitelist,
            keywords: words,
            ..Default::default()
        };
        let mut next = old.clone();
        next.keywords = vec!["apple".into(), "发票".into()];
        assert!(!old.needs_new_baseline(&next));
        next.keywords.push("新增".into());
        assert!(old.needs_new_baseline(&next));
        next.filter_mode = FilterMode::Blacklist;
        assert!(old.needs_new_baseline(&next));
        old.filter_mode = FilterMode::None;
        next.filter_mode = FilterMode::None;
        assert!(!old.needs_new_baseline(&next));
        next.directory = "/other".into();
        assert!(old.needs_new_baseline(&next));
        next.enabled = false;
        assert!(!old.needs_new_baseline(&next));
    }

    fn filter(mode: FilterMode, keyword: &str) -> Settings {
        Settings {
            filter_mode: mode,
            keywords: vec![keyword.into()],
            ..Default::default()
        }
    }

    fn scoped_filter(field: &str, keyword: &str) -> Settings {
        serde_json::from_value(serde_json::json!({
            "filterMode": "whitelist", "keywordScope": "fields",
            "fieldKeywords": {field: [keyword]}, "keywords": ["inactive draft"]
        }))
        .unwrap()
    }

    #[test]
    fn field_filters_match_only_the_selected_field_and_preserve_legacy_scope() {
        let raw = b"From: Sender Name <sender@example.test>\r\nTo: To Name <to@example.test>\r\nCc: Cc Name <cc@example.test>\r\nBcc: Bcc Name <bcc@example.test>\r\nSubject: subject-only\r\n\r\nbody-only";
        for (field, value) in [
            ("subject", "subject-only"),
            ("sender", "SENDER NAME"),
            ("sender", "sender@EXAMPLE.test"),
            ("recipient", "to@example.test"),
            ("recipient", "CC NAME"),
            ("recipient", "bcc@example.test"),
            ("body", "body-only"),
        ] {
            assert!(
                scoped_filter(field, value).should_save(raw).unwrap(),
                "{field}: {value}"
            );
            for other in ["subject", "sender", "recipient", "body"] {
                if other != field {
                    assert!(
                        !scoped_filter(other, value).should_save(raw).unwrap(),
                        "{value} leaked into {other}"
                    );
                }
            }
            let mut black = scoped_filter(field, value);
            black.filter_mode = FilterMode::Blacklist;
            assert!(!black.should_save(raw).unwrap());
        }
        assert!(
            !filter(FilterMode::Whitelist, "to@example.test")
                .should_save(raw)
                .unwrap()
        );
    }

    #[test]
    fn field_filters_decode_addresses_and_use_or_across_populated_fields() {
        let raw = b"From: =?UTF-8?B?6LSi5Yqh?= <Finance@example.test>\r\nTo: Group: First <first@example.test>, =?UTF-8?B?5rWL6K+V?= <last@example.test>;\r\nSubject: =?UTF-8?B?5Y+R56Wo?=\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n5L2g5aW9IEFwcGxl\r\n";
        for (field, value) in [
            ("subject", "发票"),
            ("sender", "财务"),
            ("recipient", "测试"),
            ("recipient", "last@example.test"),
            ("body", "你好"),
        ] {
            assert!(scoped_filter(field, value).should_save(raw).unwrap());
        }
        let rules: Settings = serde_json::from_value(serde_json::json!({
            "filterMode":"whitelist", "keywordScope":"fields",
            "fieldKeywords":{"subject":["missing"], "sender":["absent"], "recipient":["FIRST@example.test"], "body":["absent"]}
        })).unwrap();
        assert!(rules.should_save(raw).unwrap());
    }

    #[test]
    fn recipient_filters_match_every_repeated_destination_header_in_either_order() {
        for header in ["To", "Cc", "Bcc"] {
            for target_first in [true, false] {
                let target = "Group: Ordinary <ordinary@example.test>, Blocked Person <blocked@example.test>;";
                let other = "Other <other@example.test>";
                let (first, second) = if target_first {
                    (target, other)
                } else {
                    (other, target)
                };
                let raw = format!(
                    "Subject: ordinary\r\n{header}: {first}\r\n{header}: {second}\r\n\r\nbody"
                );
                for keyword in ["blocked@example.test", "BLOCKED PERSON"] {
                    for mode in [FilterMode::Whitelist, FilterMode::Blacklist] {
                        let mut settings = scoped_filter("recipient", keyword);
                        settings.filter_mode = mode;
                        assert_eq!(
                            settings.should_save(raw.as_bytes()).unwrap(),
                            mode == FilterMode::Whitelist,
                            "{header}, target_first={target_first}, keyword={keyword}, mode={mode:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn header_only_field_filters_do_not_require_body_decoding() {
        let raw = b"Subject: wanted\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%";
        assert!(scoped_filter("subject", "wanted").should_save(raw).unwrap());
        assert!(scoped_filter("body", "wanted").should_save(raw).is_err());
        assert!(scoped_filter("subject", "wanted").should_save(b"").is_err());
    }

    #[test]
    fn scoped_keyword_validation_bounds_both_drafts_and_only_requires_active_words() {
        let mut settings = scoped_filter("subject", "  APPLE  ");
        settings
            .field_keywords
            .subject
            .extend(["apple".into(), " ".into()]);
        settings.field_keywords.sender = vec!["Apple".into()];
        settings.keywords = vec!["  Draft  ".into(), "draft".into()];
        settings.normalize_filter_rules().unwrap();
        assert_eq!(settings.field_keywords.subject, ["APPLE"]);
        assert_eq!(settings.field_keywords.sender, ["Apple"]);
        assert_eq!(settings.keywords, ["Draft"]);
        settings.keywords.clear();
        settings.normalize_filter_rules().unwrap();
        settings.keyword_scope = KeywordScope::Any;
        assert!(settings.normalize_filter_rules().is_err());
        settings.filter_mode = FilterMode::None;
        settings.normalize_filter_rules().unwrap();
        settings.field_keywords.body = vec!["𠮷".repeat(200)];
        settings.normalize_filter_rules().unwrap();
        settings.field_keywords.body = vec!["𠮷".repeat(201)];
        assert!(settings.normalize_filter_rules().is_err());
        settings.field_keywords = FieldKeywords {
            subject: (0..50).map(|i| format!("s{i}")).collect(),
            sender: (0..50).map(|i| format!("f{i}")).collect(),
            ..Default::default()
        };
        settings.normalize_filter_rules().unwrap();
        settings.field_keywords.recipient.push("101st".into());
        assert!(settings.normalize_filter_rules().is_err());
        settings.field_keywords = FieldKeywords::default();
        settings.keywords = (0..101).map(|i| format!("k{i}")).collect();
        settings.keyword_scope = KeywordScope::Fields;
        assert!(settings.normalize_filter_rules().is_err());
    }

    #[test]
    fn inactive_field_drafts_do_not_reset_baseline_but_active_fields_do() {
        let mut old = filter(FilterMode::Blacklist, "invoice");
        old.enabled = true;
        old.directory = "/tmp".into();
        let mut next = old.clone();
        next.field_keywords.recipient = vec!["Person@example.test".into()];
        assert!(!old.needs_new_baseline(&next));
        old = next.clone();
        next.keyword_scope = KeywordScope::Fields;
        assert!(old.needs_new_baseline(&next));
        old = next.clone();
        next.keywords = vec!["different inactive draft".into()];
        next.field_keywords.recipient = vec!["person@EXAMPLE.test".into()];
        assert!(!old.needs_new_baseline(&next));
        next.field_keywords.subject = next.field_keywords.recipient.clone();
        next.field_keywords.recipient.clear();
        assert!(old.needs_new_baseline(&next));
        old.filter_mode = FilterMode::None;
        next.filter_mode = FilterMode::None;
        next.keyword_scope = KeywordScope::Any;
        assert!(!old.needs_new_baseline(&next));
    }

    #[test]
    fn body_field_excludes_html_code_hidden_content_and_attachments() {
        let raw = b"Subject: subject-only\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><head><title>HEADONLY</title><style>.CSSONLY{color:red}</style></head><body><p title=\"ATTRONLY\">Visible &amp; payment</p><p hidden>HIDDENONLY</p><template>TEMPLATEONLY</template><!--COMMENTONLY--><script>SCRIPTONLY</script></body></html>\r\n--x\r\nContent-Type: text/plain; name=attachment.txt\r\nContent-Disposition: attachment; filename=attachment.txt\r\n\r\nATTACHMENTONLY\r\n--x\r\nContent-Type: message/rfc822\r\nContent-Disposition: attachment\r\n\r\nSubject: forwarded\r\n\r\nFORWARDEDONLY\r\n--x--\r\n";
        assert!(
            scoped_filter("body", "Visible & payment")
                .should_save(raw)
                .unwrap()
        );
        for keyword in [
            "subject-only",
            "HEADONLY",
            "CSSONLY",
            "ATTRONLY",
            "HIDDENONLY",
            "TEMPLATEONLY",
            "COMMENTONLY",
            "SCRIPTONLY",
            "ATTACHMENTONLY",
            "FORWARDEDONLY",
            "attachment.txt",
        ] {
            assert!(
                !scoped_filter("body", keyword).should_save(raw).unwrap(),
                "{keyword}"
            );
        }
    }

    #[test]
    fn filter_matches_decoded_subject_sender_and_body_case_insensitively() {
        let raw = b"From: =?UTF-8?B?6LSi5Yqh?= <Finance@Example.com>\r\nSubject: =?UTF-8?B?5Y+R56Wo?=\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n5L2g5aW9IEFwcGxl\r\n";
        for keyword in ["发票", "财务", "finance@example.COM", "你好", "APPLE"] {
            assert!(
                filter(FilterMode::Whitelist, keyword)
                    .should_save(raw)
                    .unwrap(),
                "{keyword}"
            );
            assert!(
                !filter(FilterMode::Blacklist, keyword)
                    .should_save(raw)
                    .unwrap(),
                "{keyword}"
            );
        }
        assert!(
            !filter(FilterMode::Whitelist, "missing")
                .should_save(raw)
                .unwrap()
        );
        assert!(
            filter(FilterMode::Blacklist, "missing")
                .should_save(raw)
                .unwrap()
        );
        assert!(
            filter(FilterMode::None, "ignored")
                .should_save(b"unparseable")
                .unwrap()
        );
        let mut any = filter(FilterMode::Whitelist, "missing");
        any.keywords.push("APPLE".into());
        assert!(any.should_save(raw).unwrap());
    }

    #[test]
    fn filter_uses_visible_html_excluding_code_and_attachments() {
        let raw = b"From: sender@example.com\r\nSubject: ordinary\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><head><title>HEADONLY</title><style>.CSSONLY{color:red}</style></head><body><p data-secret=\"ATTRONLY\">Visible &amp; payment</p><!--COMMENTONLY--><script>SCRIPTONLY</script></body></html>\r\n--x\r\nContent-Type: text/plain; name=attachment.txt\r\nContent-Disposition: attachment; filename=attachment.txt\r\n\r\nATTACHMENTONLY\r\n--x--\r\n";
        assert!(
            filter(FilterMode::Whitelist, "Visible & payment")
                .should_save(raw)
                .unwrap()
        );
        for keyword in [
            "HEADONLY",
            "CSSONLY",
            "ATTRONLY",
            "COMMENTONLY",
            "SCRIPTONLY",
            "ATTACHMENTONLY",
            "attachment.txt",
        ] {
            assert!(
                !filter(FilterMode::Whitelist, keyword)
                    .should_save(raw)
                    .unwrap(),
                "{keyword}"
            );
        }
    }

    #[test]
    fn filter_does_not_match_html_attribute_values_containing_angle_brackets() {
        let raw = b"From: sender@example.com\r\nSubject: ordinary\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p title=\"one > ATTRIBUTEONLY\">Visible text</p>";
        assert!(
            filter(FilterMode::Whitelist, "Visible text")
                .should_save(raw)
                .unwrap()
        );
        assert!(
            !filter(FilterMode::Whitelist, "ATTRIBUTEONLY")
                .should_save(raw)
                .unwrap()
        );
    }

    #[test]
    fn broken_attachment_does_not_block_matching_valid_body() {
        let raw = b"From: sender@example.com\r\nSubject: ordinary\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nwanted\r\n--x\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=broken.bin\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%\r\n--x--\r\n";
        assert!(
            filter(FilterMode::Whitelist, "wanted")
                .should_save(raw)
                .unwrap()
        );
        assert!(
            !filter(FilterMode::Blacklist, "wanted")
                .should_save(raw)
                .unwrap()
        );
    }

    #[test]
    fn filtered_skip_advances_progress_without_writing_or_counting() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store.lock().unwrap().accounts[0].auto_download.filter_mode = FilterMode::Whitelist;
        store.lock().unwrap().accounts[0].auto_download.keywords = vec!["wanted".into()];
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        assert!(
            commit(
                &store,
                "test",
                1,
                cp,
                Some((11, b"Subject: unwanted match?\r\n\r\nnone"))
            )
            .unwrap()
        );
        // A literal substring does match; use a different subject for the skipped message.
        let cp = Checkpoint { last_uid: 12, ..cp };
        assert!(
            commit(
                &store,
                "test",
                1,
                cp,
                Some((12, b"Subject: ordinary\r\n\r\nnone"))
            )
            .unwrap()
        );
        let reloaded = Store::load(dir.path().join("config")).unwrap();
        let saved = &reloaded.get("test").unwrap().auto_download;
        assert_eq!(saved.checkpoint, Some(cp));
        assert_eq!(saved.saved_count, 1);
        assert!(saved.last_path.is_some());
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "eml"))
                .count(),
            1
        );
    }

    #[test]
    fn invalid_mime_does_not_advance_filtered_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store.lock().unwrap().accounts[0].auto_download.filter_mode = FilterMode::Blacklist;
        store.lock().unwrap().accounts[0].auto_download.keywords = vec!["blocked".into()];
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        assert!(commit(&store, "test", 1, cp, Some((11, b""))).is_err());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get("test")
                .unwrap()
                .auto_download
                .checkpoint
                .unwrap()
                .last_uid,
            10
        );
        assert!(filter(FilterMode::Whitelist, "wanted").should_save(b"Subject: broken\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%").is_err());
        assert!(filter(FilterMode::Whitelist, "wanted").should_save(b"Subject: broken\r\nContent-Type: text/html\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%").is_err());
    }

    #[test]
    fn excessive_mime_depth_does_not_advance_filtered_checkpoint() {
        let nested = format!(
            "{}Subject: nested\r\n\r\nbody",
            "Content-Type: message/rfc822\r\n\r\n".repeat(100)
        );
        let encoded = format!(
            "Content-Type: message/rfc822\r\nContent-Transfer-Encoding: base64\r\n\r\n{}",
            base64::engine::general_purpose::STANDARD.encode(&nested)
        );
        for raw in [nested, encoded] {
            let dir = tempfile::tempdir().unwrap();
            let store = store(dir.path());
            {
                let mut guard = store.lock().unwrap();
                let settings = &mut guard.accounts[0].auto_download;
                settings.filter_mode = FilterMode::Blacklist;
                settings.keywords = vec!["blocked".into()];
            }
            let result = commit(
                &store,
                "test",
                1,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: 11,
                },
                Some((11, raw.as_bytes())),
            );
            assert!(
                result.is_err(),
                "unreliable deep MIME parsing must not skip or save the message"
            );
            let guard = store.lock().unwrap();
            let settings = &guard.accounts[0].auto_download;
            assert_eq!(settings.checkpoint.unwrap().last_uid, 10);
            assert_eq!(settings.saved_count, 0);
            assert!(eml_files(dir.path()).is_empty());
        }
    }

    #[test]
    fn skipped_mail_checkpoint_failure_is_retryable_without_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store.lock().unwrap().accounts[0].auto_download.filter_mode = FilterMode::Blacklist;
        store.lock().unwrap().accounts[0].auto_download.keywords = vec!["blocked".into()];
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        let raw = b"Subject: blocked\r\n\r\nbody";
        fs::create_dir(dir.path().join("config/accounts.json.tmp")).unwrap();
        assert!(commit(&store, "test", 1, cp, Some((11, raw))).is_err());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get("test")
                .unwrap()
                .auto_download
                .checkpoint
                .unwrap()
                .last_uid,
            10
        );
        fs::remove_dir(dir.path().join("config/accounts.json.tmp")).unwrap();
        assert!(commit(&store, "test", 1, cp, Some((11, raw))).unwrap());
        let guard = store.lock().unwrap();
        let settings = &guard.get("test").unwrap().auto_download;
        assert_eq!(settings.checkpoint, Some(cp));
        assert_eq!(settings.saved_count, 0);
        assert_eq!(settings.last_path, None);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    fn store(dir: &Path) -> Mutex<Store> {
        let mut store = Store::load(dir.join("config")).unwrap();
        let mut account: Account = serde_json::from_str(r#"{"id":"test","kind":"qq","email":"test@example.com","name":"测试","imapHost":"imap.example.com","imapPort":993,"smtpHost":"smtp.example.com","smtpPort":465}"#).unwrap();
        account.auto_download = Settings {
            enabled: true,
            directory: dir.display().to_string(),
            revision: 1,
            checkpoint: Some(Checkpoint {
                uid_validity: 7,
                last_uid: 10,
            }),
            ..Default::default()
        };
        store.accounts.push(account);
        store.save().unwrap();
        Mutex::new(store)
    }
    #[test]
    fn automatic_count_at_max_keeps_saving_without_wrapping_or_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        store.lock().unwrap().accounts[0].auto_download.saved_count = u64::MAX;
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        let raw = b"Subject: Counter boundary\r\n\r\noriginal";
        assert!(commit(&store, "test", 1, cp, Some((11, raw))).unwrap());
        let loaded = Store::load(dir.path().join("config")).unwrap();
        let settings = &loaded.get("test").unwrap().auto_download;
        assert_eq!(settings.saved_count, u64::MAX);
        assert_eq!(settings.checkpoint, Some(cp));
        assert_eq!(fs::read(settings.last_path.as_ref().unwrap()).unwrap(), raw);
    }
    #[test]
    fn automatic_files_are_atomic_idempotent_and_never_overwrite_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        let raw = b"Subject: Test\r\n\r\noriginal bytes\0\xff";
        let path = save_atomic(dir.path(), "a/中文", cp, 11, raw).unwrap();
        assert_eq!(path.file_name().unwrap(), "Test.eml");
        assert_eq!(
            save_atomic(dir.path(), "a/中文", cp, 11, raw).unwrap(),
            path
        );
        assert_eq!(eml_files(dir.path()).len(), 1);
        assert!(save_atomic(dir.path(), "a/中文", cp, 11, b"different").is_err());
        assert_eq!(fs::read(&path).unwrap(), raw);
        assert_ne!(
            save_atomic(dir.path(), "another", cp, 11, raw).unwrap(),
            path
        );
        assert_ne!(
            save_atomic(
                dir.path(),
                "a/中文",
                Checkpoint {
                    uid_validity: 8,
                    ..cp
                },
                11,
                raw
            )
            .unwrap(),
            path
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn failed_checkpoint_write_retries_existing_file_without_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        fs::create_dir(dir.path().join("config/accounts.json.tmp")).unwrap();
        assert!(commit(&store, "test", 1, cp, Some((11, b"raw MIME"))).is_err());
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get("test")
                .unwrap()
                .auto_download
                .checkpoint
                .unwrap()
                .last_uid,
            10
        );
        fs::remove_dir(dir.path().join("config/accounts.json.tmp")).unwrap();
        assert!(commit(&store, "test", 1, cp, Some((11, b"raw MIME"))).unwrap());
        assert!(commit(&store, "test", 1, cp, Some((11, b"raw MIME"))).unwrap());
        let loaded = Store::load(dir.path().join("config")).unwrap();
        let settings = &loaded.get("test").unwrap().auto_download;
        assert_eq!(settings.checkpoint, Some(cp));
        assert_eq!(settings.saved_count, 1);
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|e| e == "eml"))
                .count(),
            1
        );
    }
    #[test]
    fn disabled_removed_and_changed_settings_reject_stale_network_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        assert!(!commit(&store, "test", 0, cp, Some((11, b"raw"))).unwrap());
        store.lock().unwrap().accounts[0].auto_download.enabled = false;
        assert!(!commit(&store, "test", 1, cp, Some((11, b"raw"))).unwrap());
        store.lock().unwrap().accounts.clear();
        assert!(!commit(&store, "test", 1, cp, Some((11, b"raw"))).unwrap());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
    #[test]
    fn invalid_and_unavailable_directory_does_not_advance_progress() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        assert!(validate_directory("relative/path").is_err());
        assert!(validate_directory(dir.path().join("missing").to_str().unwrap()).is_err());
        assert!(
            validate_directory(dir.path().join("config/accounts.json").to_str().unwrap()).is_err()
        );
        let valid = validate_directory(dir.path().to_str().unwrap()).unwrap();
        assert!(valid.is_absolute());
        store.lock().unwrap().accounts[0].auto_download.directory =
            dir.path().join("missing").display().to_string();
        assert!(
            commit(
                &store,
                "test",
                1,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: 11
                },
                Some((11, b"raw"))
            )
            .is_err()
        );
        assert_eq!(
            store
                .lock()
                .unwrap()
                .get("test")
                .unwrap()
                .auto_download
                .checkpoint
                .unwrap()
                .last_uid,
            10
        );
    }

    fn saved(dir: &Path, account: &str, validity: u32, uid: u32) -> (PathBuf, Vec<u8>) {
        let raw =
            format!("Subject: Mail {uid}\r\nFrom: a@example.com\r\n\r\nbody {uid}").into_bytes();
        let cp = Checkpoint {
            uid_validity: validity,
            last_uid: uid,
        };
        (save_atomic(dir, account, cp, uid, &raw).unwrap(), raw)
    }

    fn tree(dir: &Path) -> Vec<(PathBuf, std::time::SystemTime)> {
        let mut out = Vec::new();
        for entry in walk(dir) {
            let modified = fs::symlink_metadata(&entry).unwrap().modified().unwrap();
            out.push((entry, modified));
        }
        out.sort();
        out
    }

    fn walk(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                out.extend(walk(&path));
            }
            out.push(path);
        }
        out
    }

    #[test]
    fn find_archived_returns_the_file_saved_for_that_mail() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), "a", 7, 1);
        let (path, raw) = saved(dir.path(), "a", 7, 2);
        let found = find_archived(dir.path(), "a", 7, 2).unwrap().unwrap();
        assert_eq!(found, path);
        assert_eq!(fs::read(found).unwrap(), raw);
    }

    #[test]
    fn find_archived_misses_other_generation_account_or_uid() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), "a", 7, 2);
        assert!(find_archived(dir.path(), "a", 8, 2).unwrap().is_none());
        assert!(find_archived(dir.path(), "b", 7, 2).unwrap().is_none());
        assert!(find_archived(dir.path(), "a", 7, 3).unwrap().is_none());
    }

    #[test]
    fn find_archived_reads_legacy_named_files() {
        let dir = tempfile::tempdir().unwrap();
        let name = format!("MailBeacon-{}-7-5.eml", URL_SAFE_NO_PAD.encode("a"));
        fs::write(dir.path().join(&name), b"Subject: old\r\n\r\nold").unwrap();
        assert_eq!(
            find_archived(dir.path(), "a", 7, 5).unwrap(),
            Some(dir.path().join(name))
        );
        assert_eq!(archived_uids(dir.path(), "a", 7), BTreeSet::from([5]));
    }

    #[test]
    fn find_archived_rejects_tampered_index() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let (path, raw) = saved(dir.path(), "a", 7, 1);
        let mapping = dir
            .path()
            .join(".mailpulse-index/identities")
            .join(format!("{}.json", archive_name_for("a", 7, 1)));
        let good = fs::read(&mapping).unwrap();

        let escaped = String::from_utf8(good.clone())
            .unwrap()
            .replace(path.file_name().unwrap().to_str().unwrap(), "../x.eml");
        fs::write(&mapping, escaped).unwrap();
        assert!(!matches!(find_archived(dir.path(), "a", 7, 1), Ok(Some(_))));

        fs::write(&mapping, b"{not json").unwrap();
        assert!(!matches!(find_archived(dir.path(), "a", 7, 1), Ok(Some(_))));

        fs::write(&mapping, &good).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("x.eml");
        fs::write(&target, &raw).unwrap();
        fs::remove_file(&path).unwrap();
        symlink(&target, &path).unwrap();
        assert!(!matches!(find_archived(dir.path(), "a", 7, 1), Ok(Some(_))));

        fs::remove_file(&path).unwrap();
        assert!(find_archived(dir.path(), "a", 7, 1).unwrap().is_none());
    }

    #[test]
    fn find_archived_creates_and_changes_nothing() {
        let empty = tempfile::tempdir().unwrap();
        assert!(find_archived(empty.path(), "a", 7, 1).unwrap().is_none());
        assert!(archived_uids(empty.path(), "a", 7).is_empty());
        assert_eq!(fs::read_dir(empty.path()).unwrap().count(), 0);

        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), "a", 7, 1);
        let before = tree(dir.path());
        std::thread::sleep(Duration::from_millis(20));
        find_archived(dir.path(), "a", 7, 1).unwrap().unwrap();
        archived_uids(dir.path(), "a", 7);
        assert_eq!(tree(dir.path()), before);
    }

    #[test]
    fn archived_uids_lists_one_generation_of_one_account() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), "a", 7, 1);
        saved(dir.path(), "a", 7, 2);
        saved(dir.path(), "a", 8, 3);
        saved(dir.path(), "b", 7, 4);
        // 账号 id 是另一个的前缀时不能串号
        saved(dir.path(), "a7", 7, 9);
        assert_eq!(archived_uids(dir.path(), "a", 7), BTreeSet::from([1, 2]));
    }

    #[test]
    fn archived_uids_accept_only_plain_uid_numbers() {
        let dir = tempfile::tempdir().unwrap();
        saved(dir.path(), "a", 7, 1);
        let ids = dir.path().join(".mailpulse-index/identities");
        let prefix = archive_prefix("a", 7);
        // 另一个账号的 base64 恰好是「YQ-7」这种形式时，剩下的部分不是纯数字
        for name in ["+5", "05", "8-9", "", "4294967296"] {
            fs::write(ids.join(format!("{prefix}{name}.eml.json")), b"{}").unwrap();
        }
        assert_eq!(archived_uids(dir.path(), "a", 7), BTreeSet::from([1]));
    }
}
