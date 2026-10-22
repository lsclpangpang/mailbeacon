//! 写信附件和草稿。
//!
//! - 附件：用户在文件对话框或拖拽中选中的文件登记为随机令牌，网页端只拿到令牌，
//!   发送时由这里按令牌读回文件，网页端无法让后端读取任意路径。
//! - 草稿：IMAP 账号存进服务器「草稿箱」（带 \Draft 标记），每次保存追加新版本、
//!   删除旧版本；POP3 账号、没有草稿箱或离线时存为本地 .eml。

use std::{
    collections::HashMap,
    hash::{BuildHasher, Hasher},
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::{Context, Result, anyhow, bail};
use async_imap::Session;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

use crate::{
    download_protocol::{ArchiveData, checked_command, quoted_mailbox},
    imap::{OP_TIMEOUT, Summary},
    smtp::{MAX_ATTACHMENT_BYTES, OutgoingAttachment, content_type_for},
};

/// 本地草稿在界面上的虚拟文件夹名。
pub const LOCAL_DRAFTS: &str = "__local_drafts";

/// 返回给网页端的已选文件：只有令牌、文件名和大小，没有路径。
#[derive(Clone, Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PickedFile {
    pub token: String,
    pub name: String,
    pub size: u64,
}

#[derive(Default)]
pub struct AttachmentRegistry {
    files: Mutex<HashMap<String, Entry>>,
}

/// 登记的附件：用户选中的本地文件，或从已有草稿里取出、暂存在内存里的部分。
enum Entry {
    Path(PathBuf),
    Memory {
        name: String,
        content_type: String,
        data: std::sync::Arc<Vec<u8>>,
    },
}

/// 128 位随机令牌；RandomState 每次新建都带随机种子。
fn random_token() -> String {
    let part = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or_default(),
        );
        hasher.finish()
    };
    format!("{:016x}{:016x}", part(), part())
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "附件".into())
}

impl AttachmentRegistry {
    /// 登记用户选中的文件。只接受普通文件。
    pub fn register(&self, path: &Path) -> Result<PickedFile> {
        let meta =
            std::fs::metadata(path).with_context(|| format!("无法读取 {}", path.display()))?;
        if !meta.is_file() {
            bail!("{} 不是文件，不能作为附件", file_name(path));
        }
        let token = random_token();
        self.files
            .lock()
            .unwrap()
            .insert(token.clone(), Entry::Path(path.to_path_buf()));
        Ok(PickedFile {
            token,
            name: file_name(path),
            size: meta.len(),
        })
    }

    /// 登记从草稿里取出的附件。重新打开草稿后，它的附件不再依赖会被自动保存替换掉的旧版本。
    pub fn register_bytes(&self, name: String, content_type: String, data: Vec<u8>) -> PickedFile {
        let token = random_token();
        let size = data.len() as u64;
        self.files.lock().unwrap().insert(
            token.clone(),
            Entry::Memory {
                name: name.clone(),
                content_type,
                data: std::sync::Arc::new(data),
            },
        );
        PickedFile { token, name, size }
    }

    /// 写信窗口关闭或邮件发出后释放令牌（和暂存在内存里的内容）。
    pub fn forget(&self, tokens: &[String]) {
        let mut files = self.files.lock().unwrap();
        for token in tokens {
            files.remove(token);
        }
    }

    /// 按令牌读回附件内容；文件在选中之后被改大也会再检查一次大小。
    pub fn read(&self, token: &str) -> Result<OutgoingAttachment> {
        let path = match self.files.lock().unwrap().get(token) {
            None => bail!("附件已失效，请重新添加"),
            Some(Entry::Memory {
                name,
                content_type,
                data,
            }) => {
                return Ok(OutgoingAttachment {
                    name: name.clone(),
                    content_type: content_type.clone(),
                    data: data.as_ref().clone(),
                });
            }
            Some(Entry::Path(path)) => path.clone(),
        };
        let too_large = || {
            anyhow!(
                "附件 {} 超过 {} MB 上限",
                file_name(&path),
                MAX_ATTACHMENT_BYTES / 1024 / 1024
            )
        };
        let size = std::fs::metadata(&path)
            .with_context(|| format!("附件 {} 已被移动或删除", file_name(&path)))?
            .len();
        if size > MAX_ATTACHMENT_BYTES as u64 {
            return Err(too_large());
        }
        let data =
            std::fs::read(&path).with_context(|| format!("无法读取附件 {}", file_name(&path)))?;
        // 检查大小和读取之间文件可能又变大了
        if data.len() > MAX_ATTACHMENT_BYTES {
            return Err(too_large());
        }
        let name = file_name(&path);
        Ok(OutgoingAttachment {
            content_type: content_type_for(&name).into(),
            name,
            data,
        })
    }
}

/// 草稿保存在哪里。`uid` 为空表示服务器没告诉我们新 UID，下次按 Message-ID 查找。
#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DraftRef {
    pub account_id: String,
    #[serde(default = "initial_receive_epoch")]
    pub receive_epoch: u64,
    pub folder: String,
    pub uid: Option<u32>,
    pub uid_validity: Option<u32>,
    pub message_id: String,
    /// 还留在服务器上、下次成功保存到服务器时要删掉的旧版本
    /// （离线改存本地时，或新版本存好但没能删掉旧版本时）
    #[serde(default)]
    pub superseded: Vec<DraftRef>,
}

fn initial_receive_epoch() -> u64 {
    1
}

impl DraftRef {
    /// UIDVALIDITY 和 Message-ID 都只用于同一收信身份；不能跨服务器认领草稿。
    pub fn ensure_receive_epoch(&self, receive_epoch: u64) -> Result<()> {
        if self.receive_epoch == 0 || self.receive_epoch != receive_epoch {
            bail!("这份草稿属于更改前的收信服务器，未删除旧版本；请在原服务器处理");
        }
        Ok(())
    }
}

/// 保存新版本后应当删除的服务器旧版本：上一版本身（如果在服务器上）以及它记着的旧版本。
pub fn server_versions_to_delete(previous: Option<&DraftRef>) -> Vec<DraftRef> {
    let mut out = Vec::new();
    let mut stack: Vec<&DraftRef> = previous.into_iter().collect();
    while let Some(r) = stack.pop() {
        if r.folder != LOCAL_DRAFTS {
            out.push(DraftRef {
                superseded: Vec::new(),
                ..r.clone()
            });
        }
        stack.extend(r.superseded.iter());
    }
    out
}

/// 改存本地时，要把服务器上的旧版本记下来，回到在线后再删。
pub fn superseded_by_local(previous: Option<&DraftRef>) -> Vec<DraftRef> {
    server_versions_to_delete(previous)
}

/// 草稿命令只能删除草稿：本地草稿，或该账号的草稿箱。
pub fn draft_folder_allowed(folder: &str, drafts_folder: Option<&str>) -> bool {
    folder == LOCAL_DRAFTS || drafts_folder == Some(folder)
}

/// 本地草稿：<配置目录>/drafts/<账号>/<id>.eml
pub struct LocalDrafts {
    dir: PathBuf,
}

pub(crate) fn safe_account(account_id: &str) -> Result<&str> {
    if account_id.is_empty()
        || !account_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        || account_id.starts_with('.')
    {
        bail!("账号标识无效");
    }
    Ok(account_id)
}

impl LocalDrafts {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn account_dir(&self, account_id: &str) -> Result<PathBuf> {
        Ok(self.dir.join(safe_account(account_id)?))
    }

    fn ids(&self, account_id: &str) -> Result<Vec<u32>> {
        let dir = self.account_dir(account_id)?;
        let mut ids: Vec<u32> = match std::fs::read_dir(&dir) {
            Ok(entries) => entries
                .flatten()
                .filter_map(|e| {
                    let name = e.file_name().to_string_lossy().into_owned();
                    name.strip_suffix(".eml")?.parse().ok()
                })
                .collect(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        ids.sort_unstable();
        Ok(ids)
    }

    /// 保存新版本并删除被替换的旧版本，返回新 id。
    pub fn save(&self, account_id: &str, raw: &[u8], replace: Option<u32>) -> Result<u32> {
        let dir = self.account_dir(account_id)?;
        std::fs::create_dir_all(&dir)?;
        // 编号只增不减（记在 next_id 里），删掉最新草稿后也不会被新草稿复用。
        let counter = dir.join("next_id");
        let recorded: u32 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        let id = recorded.max(self.ids(account_id)?.last().copied().unwrap_or(0) + 1);
        std::fs::write(&counter, (id + 1).to_string())?;
        let path = dir.join(format!("{id}.eml"));
        let tmp = dir.join(format!("{id}.eml.tmp"));
        std::fs::write(&tmp, raw)?;
        std::fs::rename(&tmp, &path)?;
        if let Some(old) = replace.filter(|old| *old != id) {
            let _ = std::fs::remove_file(dir.join(format!("{old}.eml")));
        }
        Ok(id)
    }

    pub fn read(&self, account_id: &str, id: u32) -> Result<Vec<u8>> {
        std::fs::read(self.account_dir(account_id)?.join(format!("{id}.eml")))
            .map_err(|_| anyhow!("本地草稿不存在或已删除"))
    }

    pub fn delete(&self, account_id: &str, id: u32) -> Result<()> {
        match std::fs::remove_file(self.account_dir(account_id)?.join(format!("{id}.eml"))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// 从新到旧列出本地草稿。
    pub fn list(&self, account_id: &str) -> Result<Vec<Summary>> {
        let mut out = Vec::new();
        for id in self.ids(account_id)?.into_iter().rev() {
            let Ok(raw) = self.read(account_id, id) else {
                continue;
            };
            if let Ok(mut summary) = crate::pop3::summary_from_raw(&raw, account_id, id, 0) {
                summary.folder = LOCAL_DRAFTS.into();
                summary.seen = true;
                out.push(summary);
            }
        }
        Ok(out)
    }

    pub fn remove_account(&self, account_id: &str) {
        if let Ok(dir) = self.account_dir(account_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

async fn select_validity<T>(s: &mut Session<T>, folder: &str) -> Result<u32>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(s, &format!("SELECT {}", quoted_mailbox(folder)?)),
    )
    .await
    .context("打开草稿箱超时")??;
    responses
        .into_iter()
        .filter_map(|r| match r {
            ArchiveData::Validity(v) if v > 0 => Some(v),
            _ => None,
        })
        .last()
        .ok_or_else(|| anyhow!("服务器未提供 UIDVALIDITY，无法确认草稿身份"))
}

/// 只接受 `<...>` 形式、非空的 Message-ID：空字符串的 HEADER 搜索会匹配所有邮件。
fn valid_message_id(message_id: &str) -> bool {
    message_id.len() > 2
        && message_id.starts_with('<')
        && message_id.ends_with('>')
        && !message_id
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '"' || c == '\\')
}

async fn search_message_id<T>(s: &mut Session<T>, message_id: &str) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    if !valid_message_id(message_id) {
        bail!("草稿标识无效，无法确认要删除的旧版本");
    }
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(
            s,
            &format!(
                "UID SEARCH HEADER Message-ID {}",
                quoted_mailbox(message_id)?
            ),
        ),
    )
    .await
    .context("查找草稿超时")??;
    Ok(responses
        .into_iter()
        .flat_map(|r| match r {
            ArchiveData::Uids(uids) => uids,
            _ => Vec::new(),
        })
        .collect())
}

/// 删除已选中文件夹里的指定草稿：支持 UIDPLUS 时彻底删除，否则只打 \Deleted
/// 标记（普通客户端不再显示），不做会误删他人邮件的全量 EXPUNGE。
async fn remove_uids<T>(s: &mut Session<T>, uids: &[u32]) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    if uids.is_empty() {
        return Ok(());
    }
    let set = crate::imap::uid_set(uids);
    let caps = timeout(OP_TIMEOUT, s.capabilities()).await??;
    if caps.has_str("UIDPLUS") {
        return crate::imap::move_uids(s, &set, None).await;
    }
    timeout(
        OP_TIMEOUT,
        checked_command(s, &format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)")),
    )
    .await
    .context("删除旧草稿超时")??;
    Ok(())
}

/// 找出旧版本草稿的 UID：UIDVALIDITY 一致时用记下的 UID，否则按 Message-ID 查。
async fn previous_uids<T>(
    s: &mut Session<T>,
    validity: u32,
    previous: &DraftRef,
) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    match (previous.uid, previous.uid_validity) {
        (Some(uid), Some(v)) if v == validity => Ok(vec![uid]),
        _ => {
            let uids = search_message_id(s, &previous.message_id).await?;
            // 每次保存都用新的 Message-ID；匹配到多封说明不是我们的，宁可不删。
            if uids.len() > 1 {
                bail!("找到多封相同标识的草稿，未删除");
            }
            Ok(uids)
        }
    }
}

/// 追加新版本草稿，再删除旧版本。先追加后删除：中途失败最多留下重复，不会丢草稿。
/// 追加成功后的步骤失败不算保存失败：没能删掉的旧版本记在 `superseded` 里，下次再删。
pub async fn save_on_server<T>(
    s: &mut Session<T>,
    account_id: &str,
    receive_epoch: u64,
    folder: &str,
    raw: &[u8],
    message_id: &str,
    previous: Option<&DraftRef>,
) -> Result<DraftRef>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    timeout(
        OP_TIMEOUT,
        s.append(folder, Some("(\\Draft \\Seen)"), None, raw),
    )
    .await
    .context("保存草稿超时")??;
    let old_versions: Vec<DraftRef> = server_versions_to_delete(previous)
        .into_iter()
        .filter(|r| {
            r.folder == folder && r.account_id == account_id && r.receive_epoch == receive_epoch
        })
        .collect();
    let mut saved = DraftRef {
        account_id: account_id.into(),
        receive_epoch,
        folder: folder.into(),
        uid: None,
        uid_validity: None,
        message_id: message_id.into(),
        superseded: Vec::new(),
    };
    let located = async {
        let validity = select_validity(s, folder).await?;
        let uid = search_message_id(s, message_id).await?.into_iter().max();
        anyhow::Ok((validity, uid))
    }
    .await;
    let (validity, uid) = match located {
        Ok(found) => found,
        Err(e) => {
            eprintln!("草稿已保存，但没能确认它的位置：{e:#}");
            saved.superseded = old_versions;
            return Ok(saved);
        }
    };
    saved.uid = uid;
    saved.uid_validity = Some(validity);
    for old in old_versions {
        let removed = async {
            let uids: Vec<u32> = previous_uids(s, validity, &old)
                .await?
                .into_iter()
                .filter(|u| Some(*u) != uid)
                .collect();
            remove_uids(s, &uids).await
        }
        .await;
        if let Err(e) = removed {
            eprintln!("新草稿已保存，但删除旧版本失败：{e:#}");
            saved.superseded.push(old);
        }
    }
    Ok(saved)
}

/// 发送成功或放弃草稿后删除它。
pub async fn delete_on_server<T>(
    s: &mut Session<T>,
    draft: &DraftRef,
    receive_epoch: u64,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    draft.ensure_receive_epoch(receive_epoch)?;
    let validity = select_validity(s, &draft.folder).await?;
    let uids = previous_uids(s, validity, draft).await?;
    remove_uids(s, &uids).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_imap;

    #[test]
    fn legacy_draft_refs_keep_the_original_receive_identity() {
        let value = serde_json::json!({
            "accountId": "acct", "folder": "Drafts", "uid": 1,
            "uidValidity": 7, "messageId": "<old@x>",
            "superseded": [{ "accountId": "acct", "folder": "Drafts", "uid": 2,
                "uidValidity": 7, "messageId": "<older@x>" }]
        });
        let draft: DraftRef = serde_json::from_value(value).unwrap();
        assert_eq!(draft.receive_epoch, 1);
        assert_eq!(draft.superseded[0].receive_epoch, 1);
        assert_eq!(serde_json::to_value(draft).unwrap()["receiveEpoch"], 1);
    }

    #[tokio::test]
    async fn old_endpoint_draft_never_deletes_a_reused_uid_or_searches_message_id() {
        for known_uid in [true, false] {
            let server = test_imap::state("UIDPLUS", &[("Drafts", 7)]);
            let uid = server.lock().unwrap().add(
                "Drafts",
                &String::from_utf8(eml("新服务器草稿", "<same@x>")).unwrap(),
                &["\\Draft"],
            );
            let mut s = test_imap::session(server.clone()).await;
            let old = DraftRef {
                account_id: "acct".into(),
                receive_epoch: 1,
                folder: "Drafts".into(),
                uid: known_uid.then_some(uid),
                uid_validity: known_uid.then_some(7),
                message_id: "<same@x>".into(),
                superseded: vec![],
            };
            server.lock().unwrap().commands.clear();
            assert!(
                delete_on_server(&mut s, &old, 2).await.is_err(),
                "旧收信身份必须先拒绝"
            );
            let state = server.lock().unwrap();
            assert_eq!(state.mailboxes[0].messages.len(), 1);
            assert!(
                state.commands.is_empty(),
                "不能对新端点 SELECT、SEARCH 或删除旧引用"
            );
        }
    }

    #[tokio::test]
    async fn saving_on_new_endpoint_preserves_colliding_previous_and_superseded_drafts() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 7)]);
        let raw = String::from_utf8(eml("新服务器原有草稿", "<collision@x>")).unwrap();
        let uid = server.lock().unwrap().add("Drafts", &raw, &["\\Draft"]);
        let mut s = test_imap::session(server.clone()).await;
        let previous = DraftRef {
            account_id: "acct".into(),
            receive_epoch: 1,
            folder: "Drafts".into(),
            uid: Some(uid),
            uid_validity: Some(7),
            message_id: "<old@x>".into(),
            superseded: vec![DraftRef {
                account_id: "acct".into(),
                receive_epoch: 1,
                folder: "Drafts".into(),
                uid: None,
                uid_validity: None,
                message_id: "<collision@x>".into(),
                superseded: vec![],
            }],
        };
        let saved = save_on_server(
            &mut s,
            "acct",
            2,
            "Drafts",
            &eml("新版本", "<new@x>"),
            "<new@x>",
            Some(&previous),
        )
        .await
        .unwrap();
        assert_eq!(saved.receive_epoch, 2);
        let state = server.lock().unwrap();
        assert_eq!(
            state.mailboxes[0].messages.len(),
            2,
            "旧服务器引用不能删掉新服务器的原有草稿"
        );
        assert!(
            !state
                .commands
                .iter()
                .any(|command| command.contains("<collision@x>")
                    || command.starts_with("UID STORE")
                    || command.starts_with("UID EXPUNGE"))
        );
    }

    fn temp_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("mailbeacon-drafts-{label}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn eml(subject: &str, message_id: &str) -> Vec<u8> {
        format!(
            "From: me@example.com\r\nTo: you@example.com\r\nSubject: {subject}\r\nMessage-ID: {message_id}\r\nDate: Mon, 19 Oct 2026 10:00:00 +0800\r\n\r\nbody\r\n"
        )
        .into_bytes()
    }

    #[test]
    fn registry_hands_out_unguessable_tokens_and_reads_files_back() {
        let dir = temp_dir("registry");
        let path = dir.join("报价单.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let registry = AttachmentRegistry::default();
        let picked = registry.register(&path).unwrap();
        assert_eq!(picked.name, "报价单.pdf");
        assert_eq!(picked.size, 4);
        assert!(picked.token.len() >= 32, "令牌必须足够长，不能被猜到");
        let other = registry.register(&path).unwrap();
        assert_ne!(picked.token, other.token);
        let file = registry.read(&picked.token).unwrap();
        assert_eq!(file.data, b"%PDF");
        assert_eq!(file.content_type, "application/pdf");
        assert!(registry.read("not-a-token").is_err());
        assert!(registry.register(&dir).is_err(), "不能把文件夹当附件");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_drafts_save_replace_list_and_delete() {
        let dir = temp_dir("local");
        let store = LocalDrafts::new(dir.clone());
        let first = store.save("acct", &eml("第一版", "<a@x>"), None).unwrap();
        let second = store
            .save("acct", &eml("第二版", "<b@x>"), Some(first))
            .unwrap();
        assert_ne!(first, second);
        assert!(store.read("acct", first).is_err(), "替换后旧版本要删除");
        let list = store.list("acct").unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].subject, "第二版");
        assert_eq!(list[0].folder, LOCAL_DRAFTS);
        assert_eq!(list[0].uid, second);
        assert!(store.list("other").unwrap().is_empty(), "账号之间互不可见");
        assert!(store.read("other", second).is_err());
        store.delete("acct", second).unwrap();
        assert!(store.list("acct").unwrap().is_empty());
        // 账号 id 不能借路径穿越到配置目录之外
        assert!(store.save("../evil", &eml("x", "<c@x>"), None).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn server_draft_replaces_previous_version_with_uidplus() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        let first = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v1", "<v1@x>"),
            "<v1@x>",
            None,
        )
        .await
        .unwrap();
        assert_eq!(first.folder, "Drafts");
        assert_eq!(first.uid_validity, Some(7));
        assert!(first.uid.is_some());
        let second = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v2", "<v2@x>"),
            "<v2@x>",
            Some(&first),
        )
        .await
        .unwrap();
        let st = server.lock().unwrap();
        let drafts = &st.mailboxes[0].messages;
        assert_eq!(drafts.len(), 1, "旧版本必须删除");
        assert_eq!(Some(drafts[0].uid), second.uid);
        assert!(drafts[0].flags.iter().any(|f| f == "\\Draft"));
        assert!(drafts[0].flags.iter().any(|f| f == "\\Seen"));
    }

    #[tokio::test]
    async fn without_uidplus_old_version_is_only_flagged_deleted() {
        let server = test_imap::state("", &[("Drafts", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        let first = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v1", "<v1@x>"),
            "<v1@x>",
            None,
        )
        .await
        .unwrap();
        assert!(
            first.uid.is_some(),
            "没有 APPENDUID 时按 Message-ID 找回新 UID"
        );
        save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v2", "<v2@x>"),
            "<v2@x>",
            Some(&first),
        )
        .await
        .unwrap();
        let st = server.lock().unwrap();
        assert!(
            !st.commands.iter().any(|c| c.starts_with("UID EXPUNGE")),
            "不支持 UIDPLUS 时不能 EXPUNGE，以免清掉其他客户端标记删除的邮件"
        );
        let old = st.mailboxes[0]
            .messages
            .iter()
            .find(|m| Some(m.uid) == first.uid)
            .unwrap();
        assert!(old.flags.iter().any(|f| f == "\\Deleted"));
    }

    #[tokio::test]
    async fn stale_uid_falls_back_to_message_id_and_never_deletes_other_mail() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 9)]);
        let other = server.lock().unwrap().add(
            "Drafts",
            &String::from_utf8(eml("别人的草稿", "<other@x>")).unwrap(),
            &["\\Draft"],
        );
        let mut s = test_imap::session(server.clone()).await;
        let mine = server.lock().unwrap().add(
            "Drafts",
            &String::from_utf8(eml("v1", "<v1@x>")).unwrap(),
            &["\\Draft"],
        );
        // 文件夹重建过：旧引用的 UIDVALIDITY 已失效，UID 恰好指向别人的草稿
        let stale = DraftRef {
            account_id: "acct".into(),
            receive_epoch: 1,
            folder: "Drafts".into(),
            uid: Some(other),
            uid_validity: Some(1),
            message_id: "<v1@x>".into(),
            superseded: vec![],
        };
        save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v2", "<v2@x>"),
            "<v2@x>",
            Some(&stale),
        )
        .await
        .unwrap();
        let st = server.lock().unwrap();
        let uids: Vec<u32> = st.mailboxes[0].messages.iter().map(|m| m.uid).collect();
        assert!(uids.contains(&other), "UIDVALIDITY 不符时不能按 UID 删除");
        assert!(
            !uids.contains(&mine),
            "应按 Message-ID 找到并删除自己的旧版本"
        );
    }

    #[tokio::test]
    async fn empty_or_ambiguous_message_id_never_deletes_anything() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 9)]);
        for n in 0..3 {
            let raw = String::from_utf8(eml(&format!("d{n}"), "<same@x>")).unwrap();
            server.lock().unwrap().add("Drafts", &raw, &["\\Draft"]);
        }
        let raw = String::from_utf8(eml("other", "<o@x>")).unwrap();
        server.lock().unwrap().add("Drafts", &raw, &["\\Draft"]);
        let mut s = test_imap::session(server.clone()).await;
        for message_id in ["", "no-brackets@x", "<same@x>"] {
            let stale = DraftRef {
                account_id: "acct".into(),
                receive_epoch: 1,
                folder: "Drafts".into(),
                uid: None,
                uid_validity: None,
                message_id: message_id.into(),
                superseded: vec![],
            };
            let _ = delete_on_server(&mut s, &stale, 1).await;
        }
        let st = server.lock().unwrap();
        assert_eq!(
            st.mailboxes[0].messages.len(),
            4,
            "空、格式不对或匹配多封的 Message-ID 不能用来删除"
        );
        assert!(!st.commands.iter().any(|c| c.contains("Message-ID \"\"")));
    }

    #[tokio::test]
    async fn append_that_succeeded_is_not_reported_as_a_failure() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 9)]);
        let mut s = test_imap::session(server.clone()).await;
        let first = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v1", "<v1@x>"),
            "<v1@x>",
            None,
        )
        .await
        .unwrap();
        server.lock().unwrap().fail = vec!["SELECT".into()];
        let second = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v2", "<v2@x>"),
            "<v2@x>",
            Some(&first),
        )
        .await
        .expect("APPEND 已成功，不能再当成失败去存本地副本");
        assert_eq!(second.uid, None);
        assert_eq!(
            second.superseded,
            vec![first],
            "没能删掉的旧版本留到下次删除"
        );
    }

    #[test]
    fn local_fallback_remembers_server_versions_to_delete_later() {
        let server_ref = DraftRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "Drafts".into(),
            uid: Some(3),
            uid_validity: Some(1),
            message_id: "<v1@x>".into(),
            superseded: vec![],
        };
        let carried = superseded_by_local(Some(&server_ref));
        assert_eq!(carried, vec![server_ref.clone()]);
        let local = DraftRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: LOCAL_DRAFTS.into(),
            uid: Some(1),
            uid_validity: None,
            message_id: "<v2@x>".into(),
            superseded: carried,
        };
        assert_eq!(superseded_by_local(Some(&local)), vec![server_ref.clone()]);
        assert_eq!(
            server_versions_to_delete(Some(&local)),
            vec![server_ref.clone()]
        );
        let mut chained = server_ref.clone();
        chained.superseded = vec![DraftRef {
            uid: Some(2),
            ..server_ref.clone()
        }];
        assert_eq!(server_versions_to_delete(Some(&chained)).len(), 2);
    }

    #[test]
    fn registry_holds_extracted_parts_in_memory_and_forgets_tokens() {
        let registry = AttachmentRegistry::default();
        let picked = registry.register_bytes(
            "合同.pdf".into(),
            "application/pdf".into(),
            b"%PDF".to_vec(),
        );
        assert_eq!(picked.size, 4);
        let file = registry.read(&picked.token).unwrap();
        assert_eq!(file.name, "合同.pdf");
        assert_eq!(file.data, b"%PDF");
        registry.forget(&[picked.token.clone()]);
        assert!(registry.read(&picked.token).is_err());
    }

    #[test]
    fn local_draft_ids_are_never_reused() {
        let dir = temp_dir("ids");
        let store = LocalDrafts::new(dir.clone());
        let first = store.save("acct", &eml("a", "<a@x>"), None).unwrap();
        store.delete("acct", first).unwrap();
        let second = store.save("acct", &eml("b", "<b@x>"), None).unwrap();
        assert_ne!(first, second, "删掉最新草稿后，新草稿不能复用它的编号");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_drafts_folders_may_be_deleted_through_draft_commands() {
        assert!(draft_folder_allowed(LOCAL_DRAFTS, None));
        assert!(draft_folder_allowed("Drafts", Some("Drafts")));
        assert!(!draft_folder_allowed("INBOX", Some("Drafts")));
        assert!(!draft_folder_allowed("Drafts", None));
    }

    #[tokio::test]
    async fn delete_on_server_removes_the_draft() {
        let server = test_imap::state("UIDPLUS", &[("Drafts", 3)]);
        let mut s = test_imap::session(server.clone()).await;
        let draft = save_on_server(
            &mut s,
            "acct",
            1,
            "Drafts",
            &eml("v1", "<v1@x>"),
            "<v1@x>",
            None,
        )
        .await
        .unwrap();
        delete_on_server(&mut s, &draft, 1).await.unwrap();
        assert!(server.lock().unwrap().mailboxes[0].messages.is_empty());
    }
}
