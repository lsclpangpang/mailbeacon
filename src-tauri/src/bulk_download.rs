//! 用户主动发起的全账号归档，可复用已保存的过滤规则，不修改自动下载设置或进度。

#[cfg(test)]
use crate::download_protocol::checked_command;
use crate::{
    AppState,
    auto_download::{self, Checkpoint, FilterMode, KeywordScope, Settings},
    config::Account,
    download_protocol::{checked_boundary, checked_folders, checked_raw_message, checked_search},
    imap::{self, Folder, Imap},
    sync::{self, Event},
};
use anyhow::{Context, Result, bail};
use async_imap::Session;
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{AppHandle, Manager};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{Mutex, Notify},
};

fn connection_reusable(error: &anyhow::Error) -> bool {
    !error.chain().any(|cause| {
        cause.is::<std::io::Error>()
            || cause.is::<tokio::time::error::Elapsed>()
            || cause
                .downcast_ref::<async_imap::error::Error>()
                .is_some_and(|error| {
                    !matches!(
                        error,
                        async_imap::error::Error::No(_)
                            | async_imap::error::Error::Bad(_)
                            | async_imap::error::Error::Validate(_)
                    )
                })
    })
}

// SEARCH 返回实际存在的 UID；UIDNEXT 或 UID 区间长度都不能作为邮件封数。
// 固定上界后一次枚举，避免删除产生的大量 UID 空洞让统计变成逐窗口扫描。
async fn planned_uids<T>(
    session: &mut Session<T>,
    last_uid: u32,
    control: &Control,
) -> Result<Option<Vec<u32>>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    if control.is_cancelled() {
        return Ok(None);
    }
    if last_uid == 0 {
        return Ok(Some(Vec::new()));
    }
    tokio::select! {
        biased;
        _ = control.cancelled() => Ok(None),
        result = checked_search(session, 1, last_uid) => result.map(Some),
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub state: String,
    pub detail: String,
    pub directory: String,
    pub current_folder: Option<String>,
    pub total_count: Option<u64>,
    pub processed_count: u64,
    pub vanished_count: u64,
    pub saved_count: u64,
    pub existing_count: u64,
    pub skipped_count: u64,
    pub failed_count: u64,
    pub inherit_filters: bool,
    pub filter_mode: FilterMode,
    pub keyword_scope: KeywordScope,
    pub folders_done: u64,
    pub folders_scanned: u64,
    pub folder_count: u64,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            state: "idle".into(),
            detail: "尚未下载".into(),
            directory: String::new(),
            current_folder: None,
            total_count: None,
            processed_count: 0,
            vanished_count: 0,
            saved_count: 0,
            existing_count: 0,
            skipped_count: 0,
            failed_count: 0,
            inherit_filters: false,
            filter_mode: FilterMode::None,
            keyword_scope: KeywordScope::Any,
            folders_done: 0,
            folders_scanned: 0,
            folder_count: 0,
        }
    }
}

impl Status {
    pub fn is_active(&self) -> bool {
        matches!(
            self.state.as_str(),
            "preparing" | "scanning" | "downloading" | "cancelling"
        )
    }
}

/// 取消可以中断网络等待；文件提交屏障用于账号移除和取消完成确认。
#[derive(Default)]
pub struct Control {
    cancelled: AtomicBool,
    notify: Notify,
    write_gate: Arc<Mutex<()>>,
}

impl Control {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        self.notify.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }

    pub async fn wait_for_writes(&self) {
        let _gate = self.write_gate.lock().await;
    }
}

pub struct Task {
    pub status: Status,
    pub control: Arc<Control>,
}

// 固定双摘要只用于短目录名；目录内完整身份文件再次核验，摘要冲突绝不混写。
fn stable_id(value: &str) -> String {
    fn hash(value: &[u8], seed: u64) -> u64 {
        value
            .iter()
            .fold(seed, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100000001b3))
    }
    format!(
        "{:016x}{:016x}",
        hash(value.as_bytes(), 0xcbf29ce484222325),
        hash(value.as_bytes(), 0x84222325cbf29ce4)
    )
}

fn safe_label(value: &str) -> String {
    let mut name = String::new();
    for ch in value.chars() {
        if name.len() + ch.len_utf8() > 72 {
            break;
        }
        name.push(if ch.is_control() || "/\\:*?\"<>|".contains(ch) {
            '_'
        } else {
            ch
        });
    }
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        "邮箱".into()
    } else {
        name.into()
    }
}

fn real_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path).context("无法访问邮件保存文件夹")?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() || fs::canonicalize(path)? != path {
        bail!("邮件保存目录已被替换或包含符号链接：{}", path.display());
    }
    Ok(())
}

fn identified_directory(parent: &Path, name: &str, identity: &str) -> Result<PathBuf> {
    real_directory(parent)?;
    let path = parent.join(name);
    match fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error).context("无法创建邮件保存文件夹"),
    }
    real_directory(&path)?;
    let marker = path.join(".mailpulse-identity");
    if !marker.try_exists()? {
        // 不覆盖已有身份；并发首次运行只会有一个身份文件被原子发布。
        let mut temporary = tempfile::NamedTempFile::new_in(&path)?;
        temporary.write_all(identity.as_bytes())?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&marker) {
            Ok(_) => {}
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.error).context("保存目录身份失败"),
        }
    }
    let metadata = fs::symlink_metadata(&marker)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || fs::read(&marker)? != identity.as_bytes()
    {
        bail!(
            "保存目录属于不同邮箱或文件夹，已停止写入：{}",
            path.display()
        );
    }
    Ok(path)
}

/// 父目录必须是现有目录；返回稳定的账号输出目录。
pub(crate) fn account_directory(parent: &Path, account: &Account) -> Result<PathBuf> {
    let parent = auto_download::validate_directory(&parent.to_string_lossy())?;
    identified_directory(
        &parent,
        &format!("{}-{}", safe_label(&account.email), stable_id(&account.id)),
        &format!("account:{}", account.id),
    )
}

/// 原始 IMAP 路径决定身份；显示名仅用于可读前缀，嵌套与同名文件夹不会混写。
pub(crate) fn stable_folder_directory(root: &Path, folder: &Folder) -> Result<PathBuf> {
    identified_directory(
        root,
        &format!("{}-{}", safe_label(&folder.name), stable_id(&folder.path)),
        &format!("folder:{}", folder.path),
    )
}

/// 返回是否已存在完全相同的原文；不同内容或非普通文件视为错误。
fn save_message(
    root: &Path,
    folder: &Folder,
    account_id: &str,
    checkpoint: Checkpoint,
    uid: u32,
    raw: &[u8],
) -> Result<bool> {
    let directory = stable_folder_directory(root, folder)?;
    auto_download::save_atomic_with_status(&directory, account_id, checkpoint, uid, raw)
        .map(|(_, existing)| existing)
}

/// 固定本次下载使用的已保存规则；自动下载开关、目录及检查点不参与全量下载。
pub(crate) fn effective_filter(account: &Account, inherit_filters: bool) -> Result<Settings> {
    if inherit_filters {
        account.auto_download.active_filter()
    } else {
        Ok(Settings::default())
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SaveOutcome {
    Saved,
    Existing,
    Skipped,
    Vanished,
}

fn record_outcome(status: &mut Status, outcome: SaveOutcome) {
    status.processed_count += 1;
    match outcome {
        SaveOutcome::Saved => status.saved_count += 1,
        SaveOutcome::Existing => status.existing_count += 1,
        SaveOutcome::Skipped => status.skipped_count += 1,
        SaveOutcome::Vanished => status.vanished_count += 1,
    }
}

fn save_filtered_message(
    root: &Path,
    folder: &Folder,
    account_id: &str,
    checkpoint: Checkpoint,
    uid: u32,
    raw: &[u8],
    filter: &Settings,
) -> Result<SaveOutcome> {
    // 必须先过滤再检查本地文件；规则排除的已有文件保持原样，不计入本次下载。
    if !filter.should_save(raw)? {
        return Ok(SaveOutcome::Skipped);
    }
    Ok(
        if save_message(root, folder, account_id, checkpoint, uid, raw)? {
            SaveOutcome::Existing
        } else {
            SaveOutcome::Saved
        },
    )
}

fn failure(status: &mut Status, error: impl std::fmt::Display) {
    status.failed_count += 1;
    // 保留第一个失败原因，后续连接错误不会淹没真正原因。
    if status.failed_count == 1 {
        status.detail = error.to_string();
    }
}

fn message_failure(status: &mut Status, error: impl std::fmt::Display) {
    status.processed_count += 1;
    failure(status, error);
}

fn finish(status: &mut Status, control: &Control) {
    status.current_folder = None;
    if control.is_cancelled() {
        status.state = "cancelled".into();
        status.detail = "已停止；已保存的邮件保留，再次开始会核对并跳过相同邮件".into();
    } else if status.failed_count > 0 || status.total_count != Some(status.processed_count) {
        if status.failed_count == 0 {
            failure(status, "邮件统计或处理尚未完成，请重新下载");
        }
        status.state = "error".into();
        status.detail = format!("部分邮件未能保存；已保存内容保留。{}", status.detail);
    } else {
        status.state = "complete".into();
        status.detail = if status.filter_mode == FilterMode::None {
            "全部文件夹已处理；完整邮件与附件已保存为 EML".into()
        } else {
            "全部文件夹已处理；符合规则的邮件与附件已保存为 EML".into()
        };
    }
}

/// 独立于 Tauri 的实际下载循环，便于用隔离服务器验证完整邮件与未读标记。
/// parent 是用户选择的父目录。取消时保留已提交文件，再次运行可核对复用。
pub(crate) async fn run_session<F>(
    session: &mut Imap,
    account: &Account,
    parent: &Path,
    inherit_filters: bool,
    control: Arc<Control>,
    mut progress: F,
) -> Status
where
    F: FnMut(Status) + Send,
{
    let mut status = Status {
        state: "preparing".into(),
        detail: "正在读取所有邮件文件夹…".into(),
        inherit_filters,
        filter_mode: if inherit_filters {
            account.auto_download.filter_mode
        } else {
            FilterMode::None
        },
        keyword_scope: if inherit_filters && account.auto_download.filter_mode != FilterMode::None {
            account.auto_download.keyword_scope
        } else {
            KeywordScope::Any
        },
        ..Default::default()
    };
    if control.is_cancelled() {
        finish(&mut status, &control);
        progress(status.clone());
        return status;
    }
    let filter = match effective_filter(account, inherit_filters) {
        Ok(filter) => filter,
        Err(error) => {
            failure(&mut status, format!("下载过滤规则无效：{error:#}"));
            finish(&mut status, &control);
            progress(status.clone());
            return status;
        }
    };
    let root = match account_directory(parent, account) {
        Ok(root) => root,
        Err(error) => {
            failure(&mut status, format!("{error:#}"));
            finish(&mut status, &control);
            progress(status.clone());
            return status;
        }
    };
    status.directory = root.display().to_string();
    progress(status.clone());
    let result = tokio::select! {
        biased;
        _ = control.cancelled() => None,
        result = checked_folders(session) => Some(result),
    };
    let folders = match result {
        None => {
            finish(&mut status, &control);
            progress(status.clone());
            return status;
        }
        Some(Err(error)) => {
            failure(&mut status, format!("读取文件夹失败：{error:#}"));
            finish(&mut status, &control);
            progress(status.clone());
            return status;
        }
        Some(Ok(folders)) => folders,
    };
    status.folder_count = folders.len() as u64;
    status.state = "scanning".into();
    status.detail = "正在确认文件夹范围并统计邮件总数…".into();
    let mut boundaries = Vec::new();
    // 先固定各文件夹边界，再开始下载，避免新收件让任务永远无法结束。
    for folder in folders {
        status.current_folder = Some(folder.name.clone());
        progress(status.clone());
        let selected = tokio::select! {
            biased;
            _ = control.cancelled() => break,
            result = checked_boundary(session, &folder.path) => result,
        };
        match selected {
            Ok(checkpoint) => boundaries.push((folder, checkpoint)),
            Err(error) => {
                failure(&mut status, format!("{}：{error:#}", folder.name));
                status.folders_done += 1;
                if !connection_reusable(&error) {
                    finish(&mut status, &control);
                    progress(status.clone());
                    return status;
                }
            }
        }
    }
    let mut plans = Vec::new();
    for (folder, checkpoint) in boundaries {
        if control.is_cancelled() {
            break;
        }
        status.current_folder = Some(folder.name.clone());
        progress(status.clone());
        let scanned = tokio::select! {
            biased;
            _ = control.cancelled() => break,
            result = async {
                let mailbox = checked_boundary(session, &folder.path).await?;
                if mailbox.uid_validity != checkpoint.uid_validity {
                    bail!("文件夹已被服务器重建，请重新开始下载");
                }
                planned_uids(session, checkpoint.last_uid, &control).await
            } => result,
        };
        match scanned {
            Ok(Some(uids)) => {
                plans.push((folder, checkpoint, uids));
                status.folders_scanned += 1;
            }
            Ok(None) => break,
            Err(error) => {
                failure(
                    &mut status,
                    format!("统计 {} 的邮件失败：{error:#}", folder.name),
                );
                status.folders_done += 1;
                if !connection_reusable(&error) {
                    finish(&mut status, &control);
                    progress(status.clone());
                    return status;
                }
            }
        }
        progress(status.clone());
    }
    if !control.is_cancelled()
        && status.failed_count == 0
        && status.folders_scanned == status.folder_count
    {
        status.total_count = Some(plans.iter().map(|(_, _, uids)| uids.len() as u64).sum());
        progress(status.clone());
    }
    for (folder, checkpoint, uids) in plans {
        if control.is_cancelled() {
            break;
        }
        status.state = "downloading".into();
        if status.failed_count == 0 {
            status.detail = "正在保存完整邮件…".into();
        }
        status.current_folder = Some(folder.name.clone());
        progress(status.clone());
        match download_folder(
            session,
            account,
            &root,
            &folder,
            checkpoint,
            &uids,
            &filter,
            &control,
            &mut status,
            &mut progress,
        )
        .await
        {
            Err(error) => {
                failure(&mut status, format!("{}：{error:#}", folder.name));
                if !connection_reusable(&error) {
                    status.folders_done += 1;
                    break;
                }
            }
            Ok(()) => {}
        }
        if control.is_cancelled() {
            break;
        }
        status.folders_done += 1;
        progress(status.clone());
    }
    finish(&mut status, &control);
    progress(status.clone());
    status
}

async fn download_folder<F>(
    session: &mut Imap,
    account: &Account,
    root: &Path,
    folder: &Folder,
    checkpoint: Checkpoint,
    uids: &[u32],
    filter: &Settings,
    control: &Arc<Control>,
    status: &mut Status,
    progress: &mut F,
) -> Result<()>
where
    F: FnMut(Status) + Send,
{
    let mailbox = tokio::select! {
        biased;
        _ = control.cancelled() => return Ok(()),
        result = checked_boundary(session, &folder.path) => result?,
    };
    if mailbox.uid_validity != checkpoint.uid_validity {
        bail!("文件夹已被服务器重建，请重新开始下载");
    }
    for &uid in uids {
        if control.is_cancelled() {
            return Ok(());
        }
        let raw = tokio::select! {
            biased;
            _ = control.cancelled() => return Ok(()),
            result = checked_raw_message(session, uid) => result,
        };
        let raw = match raw {
            Ok(Some(raw)) => raw,
            Ok(None) => {
                record_outcome(status, SaveOutcome::Vanished);
                progress(status.clone());
                continue;
            }
            Err(error) if connection_reusable(&error) => {
                message_failure(status, format!("{} / UID {}：{error:#}", folder.name, uid));
                progress(status.clone());
                continue;
            }
            Err(error) => return Err(error),
        };
        let gate = control.write_gate.clone().lock_owned().await;
        let control = control.clone();
        let root = root.to_path_buf();
        let save_folder = folder.clone();
        let id = account.id.clone();
        let filter = filter.clone();
        let saved = tokio::task::spawn_blocking(move || {
            let _gate = gate;
            if control.is_cancelled() {
                return Ok(None);
            }
            save_filtered_message(&root, &save_folder, &id, checkpoint, uid, &raw, &filter)
                .map(Some)
        })
        .await
        .context("邮件保存任务中断")?;
        match saved {
            Ok(Some(outcome)) => record_outcome(status, outcome),
            Ok(None) => return Ok(()),
            // 单封文件冲突不妨碍其余邮件，错误计数确保最终不会假报完成。
            Err(error) => {
                message_failure(status, format!("{} / UID {}：{error:#}", folder.name, uid))
            }
        }
        progress(status.clone());
    }
    Ok(())
}

/// 只发布仍属于当前任务的状态；移除账号或新任务开始后，旧任务不能覆盖界面。
fn publish(app: &AppHandle, account_id: &str, control: &Arc<Control>, mut status: Status) {
    let state = app.state::<AppState>();
    let mut tasks = state.bulk_downloads.lock().unwrap();
    let Some(task) = tasks.get_mut(account_id) else {
        return;
    };
    if !Arc::ptr_eq(&task.control, control) {
        return;
    }
    if control.is_cancelled() && status.is_active() {
        status.state = "cancelling".into();
    }
    task.status = status.clone();
    sync::emit(app, Event::BulkDownload { account_id, status });
}

/// 每个任务独立登录，与常驻同步和按需命令连接互不占用。
pub async fn watch(
    app: AppHandle,
    account: Account,
    password: String,
    parent: PathBuf,
    control: Arc<Control>,
    mut status: Status,
) {
    let login = tokio::select! {
        biased;
        _ = control.cancelled() => None,
        result = imap::login(&account, &password) => Some(result),
    };
    match login {
        Some(Ok(mut session)) => {
            run_session(
                &mut session,
                &account,
                &parent,
                status.inherit_filters,
                control.clone(),
                |status| publish(&app, &account.id, &control, status),
            )
            .await;
            // Drop 关闭专属连接；取消不等待 LOGOUT 的网络往返。
        }
        Some(Err(error)) => {
            failure(&mut status, format!("连接邮箱失败：{error:#}"));
            finish(&mut status, &control);
            publish(&app, &account.id, &control, status);
        }
        None => {
            finish(&mut status, &control);
            publish(&app, &account.id, &control, status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio::time::timeout;

    #[test]
    fn bulk_status_exposes_filter_defaults() {
        let status = serde_json::to_value(Status::default()).unwrap();
        assert_eq!(status["inheritFilters"], false);
        assert_eq!(status["filterMode"], "none");
        assert_eq!(status["keywordScope"], "any");
        assert_eq!(status["skippedCount"], 0);
    }

    #[test]
    fn bulk_progress_starts_unknown_and_scanning_is_active() {
        let status = serde_json::to_value(Status::default()).unwrap();
        assert_eq!(status.get("totalCount"), Some(&serde_json::Value::Null));
        assert_eq!(status["processedCount"], 0);
        assert_eq!(status["vanishedCount"], 0);
        assert_eq!(status["foldersScanned"], 0);
        assert!(
            Status {
                state: "scanning".into(),
                ..Default::default()
            }
            .is_active()
        );
    }

    async fn response_session(
        response: &str,
        completion: Option<&str>,
    ) -> async_imap::Session<DuplexStream> {
        let (client, server) = tokio::io::duplex(8192);
        let response = response.to_string();
        let completion = completion.map(str::to_string);
        tokio::spawn(async move {
            let mut server = BufReader::new(server);
            let mut line = String::new();
            server.read_line(&mut line).await.unwrap();
            let tag = line.split_whitespace().next().unwrap();
            server
                .get_mut()
                .write_all(format!("{tag} OK login complete\r\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            server.read_line(&mut line).await.unwrap();
            let tag = line.split_whitespace().next().unwrap().to_string();
            server
                .get_mut()
                .write_all(response.as_bytes())
                .await
                .unwrap();
            if let Some(completion) = completion {
                server
                    .get_mut()
                    .write_all(format!("{tag} {completion}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        async_imap::Client::new(client)
            .login("test", "pw")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn bulk_plan_counts_actual_uids_with_gaps_and_ignores_duplicates_and_new_mail() {
        let mut session =
            response_session("* SEARCH 1002 1001 1 501 2 1 0\r\n", Some("OK done")).await;
        let planned = planned_uids(&mut session, 1001, &Control::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(planned, [1, 2, 501, 1001]);
        assert_eq!(
            planned.len(),
            4,
            "UID upper bound must not be treated as total count"
        );

        let mut empty = response_session("* SEARCH\r\n", Some("OK done")).await;
        assert_eq!(
            planned_uids(&mut empty, 18, &Control::default())
                .await
                .unwrap(),
            Some(vec![])
        );
        let mut never_sent = response_session("", None).await;
        assert_eq!(
            planned_uids(&mut never_sent, 0, &Control::default())
                .await
                .unwrap(),
            Some(vec![])
        );
    }

    #[tokio::test]
    async fn bulk_incomplete_scan_never_supplies_a_partial_plan() {
        for completion in [Some("NO denied"), Some("BAD failed"), None] {
            let mut session = response_session("* SEARCH 1 4\r\n", completion).await;
            assert!(
                planned_uids(&mut session, 4, &Control::default())
                    .await
                    .is_err()
            );
        }
        let mut missing_search = response_session("* 2 EXISTS\r\n", Some("OK done")).await;
        assert!(
            planned_uids(&mut missing_search, 4, &Control::default())
                .await
                .is_err()
        );
        let control = Control::default();
        control.cancel();
        let mut cancelled = response_session("* SEARCH 1 4\r\n", Some("OK done")).await;
        assert_eq!(
            planned_uids(&mut cancelled, 4, &control).await.unwrap(),
            None
        );
    }

    #[test]
    fn bulk_progress_counts_terminal_mail_outcomes_but_not_folder_failures() {
        let mut status = Status {
            total_count: Some(5),
            ..Default::default()
        };
        record_outcome(&mut status, SaveOutcome::Saved);
        record_outcome(&mut status, SaveOutcome::Existing);
        record_outcome(&mut status, SaveOutcome::Skipped);
        record_outcome(&mut status, SaveOutcome::Vanished);
        message_failure(&mut status, "mail parse failed");
        assert_eq!(status.processed_count, 5);
        assert_eq!(status.saved_count, 1);
        assert_eq!(status.existing_count, 1);
        assert_eq!(status.skipped_count, 1);
        assert_eq!(status.vanished_count, 1);
        assert_eq!(status.failed_count, 1);
        failure(&mut status, "folder inaccessible");
        assert_eq!(status.processed_count, 5);
        assert_eq!(status.failed_count, 2);
        finish(&mut status, &Control::default());
        assert_eq!(status.state, "error");
        assert_eq!(status.total_count, Some(5));
    }

    #[test]
    fn bulk_completion_requires_known_total_and_all_planned_mail_processed() {
        for total_count in [None, Some(1)] {
            let mut incomplete = Status {
                total_count,
                ..Default::default()
            };
            finish(&mut incomplete, &Control::default());
            assert_eq!(incomplete.state, "error");
            assert_eq!(incomplete.processed_count, 0);
        }
        let mut empty = Status {
            total_count: Some(0),
            ..Default::default()
        };
        finish(&mut empty, &Control::default());
        assert_eq!(empty.state, "complete");
        let mut vanished = Status {
            total_count: Some(1),
            ..Default::default()
        };
        record_outcome(&mut vanished, SaveOutcome::Vanished);
        finish(&mut vanished, &Control::default());
        assert_eq!(vanished.state, "complete");
        assert_eq!(vanished.skipped_count, 0);
    }

    #[tokio::test]
    async fn bulk_rejects_tagged_no_bad_and_incomplete_command_results() {
        for (command, partial) in [
            ("LIST \"\" \"*\"", "* LIST () \"/\" \"INBOX\"\r\n"),
            ("UID SEARCH ALL", "* SEARCH 1 2 3\r\n"),
            (
                "UID FETCH 1 (UID BODY.PEEK[])",
                "* 1 FETCH (UID 1 BODY[] {4}\r\nmail)\r\n",
            ),
            (
                "EXAMINE \"INBOX\"",
                "* OK [UIDVALIDITY 1] valid\r\n* OK [UIDNEXT 2] next\r\n",
            ),
        ] {
            for completion in [Some("NO denied"), Some("BAD failed"), None] {
                let mut session = response_session(partial, completion).await;
                assert!(
                    checked_command(&mut session, command).await.is_err(),
                    "accepted {command} / {completion:?}"
                );
            }
            let mut session = response_session(partial, Some("OK done")).await;
            assert!(
                !checked_command(&mut session, command)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn bulk_transport_errors_must_not_reuse_the_connection() {
        assert!(!connection_reusable(
            &std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()
        ));
        assert!(!connection_reusable(
            &async_imap::error::Error::ConnectionLost.into()
        ));
        let elapsed = timeout(Duration::ZERO, tokio::time::sleep(Duration::from_secs(1)))
            .await
            .unwrap_err();
        assert!(!connection_reusable(&elapsed.into()));
        assert!(connection_reusable(
            &async_imap::error::Error::No("no access".into()).into()
        ));
    }

    fn account(id: &str) -> Account {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind":"custom", "email":"test@example.test", "name":"Test",
            "imapHost":"localhost", "imapPort":3993, "smtpHost":"localhost", "smtpPort":3465
        }))
        .unwrap()
    }

    #[test]
    fn bulk_filters_are_opt_in_and_snapshot_saved_rules_without_auto_enabled() {
        let mut acct = account("filter-snapshot");
        acct.auto_download.filter_mode = FilterMode::Blacklist;
        acct.auto_download.keywords = vec![" invoice ".into()];
        assert!(!acct.auto_download.enabled);
        let saved = acct.auto_download.clone();
        let raw = b"From: sender@example.test\r\nSubject: Invoice ready\r\n\r\nPayment attached";
        let off = effective_filter(&acct, false).unwrap();
        assert_eq!(off.filter_mode, FilterMode::None);
        assert!(off.should_save(raw).unwrap());
        let blacklist = effective_filter(&acct, true).unwrap();
        assert_eq!(blacklist.filter_mode, FilterMode::Blacklist);
        assert!(!blacklist.should_save(raw).unwrap());
        assert_eq!(acct.auto_download, saved);

        acct.auto_download.filter_mode = FilterMode::Whitelist;
        let whitelist = effective_filter(&acct, true).unwrap();
        assert!(whitelist.should_save(raw).unwrap());
        acct.auto_download.keywords = vec!["missing".into()];
        assert!(whitelist.should_save(raw).unwrap(), "run snapshot changed");
        assert!(
            !effective_filter(&acct, true)
                .unwrap()
                .should_save(raw)
                .unwrap()
        );

        acct.auto_download.filter_mode = FilterMode::None;
        let none = effective_filter(&acct, true).unwrap();
        assert_eq!(none.filter_mode, FilterMode::None);
        assert!(none.should_save(b"unparseable").unwrap());
    }

    #[test]
    fn bulk_invalid_inherited_rules_fail_but_unselected_rules_are_ignored() {
        let mut acct = account("invalid-rules");
        acct.auto_download.filter_mode = FilterMode::Whitelist;
        for keywords in [vec![], vec!["   ".into()], vec!["中".repeat(201)]] {
            acct.auto_download.keywords = keywords;
            assert!(effective_filter(&acct, true).is_err());
            assert!(effective_filter(&acct, false).is_ok());
        }
        acct.auto_download.filter_mode = FilterMode::None;
        assert!(effective_filter(&acct, true).is_ok());
    }

    #[test]
    fn bulk_field_snapshot_keeps_active_scope_and_never_inherits_inactive_drafts() {
        let mut acct = account("field-snapshot");
        acct.auto_download = serde_json::from_value(serde_json::json!({
            "keywordScope":"fields", "filterMode":"blacklist", "keywords":["inactive"],
            "fieldKeywords":{"subject":["notice"], "recipient":[" Target@Example.test "]}
        }))
        .unwrap();
        let raw = b"Subject: ordinary\r\nTo: target@example.test\r\n\r\nbody";
        let snapshot = effective_filter(&acct, true).unwrap();
        assert_eq!(snapshot.keyword_scope, KeywordScope::Fields);
        assert!(snapshot.keywords.is_empty());
        assert!(!snapshot.should_save(raw).unwrap());
        acct.auto_download.field_keywords.recipient = vec!["unrelated".into()];
        assert!(!snapshot.should_save(raw).unwrap());
        assert!(
            effective_filter(&acct, true)
                .unwrap()
                .should_save(raw)
                .unwrap()
        );
        let unfiltered = effective_filter(&acct, false).unwrap();
        assert_eq!(unfiltered.filter_mode, FilterMode::None);
        assert_eq!(unfiltered.keyword_scope, KeywordScope::Any);
        assert!(unfiltered.should_save(b"invalid MIME").unwrap());
        acct.auto_download.field_keywords = Default::default();
        assert!(effective_filter(&acct, true).is_err());
        acct.auto_download.filter_mode = FilterMode::None;
        assert_eq!(
            effective_filter(&acct, true).unwrap().keyword_scope,
            KeywordScope::Any
        );
        // Invalid inactive drafts do not affect a run that uses the other valid mode.
        acct.auto_download.filter_mode = FilterMode::Whitelist;
        acct.auto_download.keyword_scope = KeywordScope::Any;
        acct.auto_download.field_keywords.body = vec!["x".repeat(201)];
        assert!(effective_filter(&acct, true).is_ok());
    }

    #[test]
    fn bulk_filtering_precedes_file_creation_and_preserves_excluded_existing_files() {
        let parent = tempfile::tempdir().unwrap();
        let mut acct = account("filter-files");
        acct.auto_download.filter_mode = FilterMode::Blacklist;
        acct.auto_download.keywords = vec!["invoice".into()];
        let blacklist = effective_filter(&acct, true).unwrap();
        acct.auto_download.filter_mode = FilterMode::Whitelist;
        let whitelist = effective_filter(&acct, true).unwrap();
        let root = account_directory(parent.path(), &acct).unwrap();
        let folder = folder("INBOX");
        let checkpoint = Checkpoint {
            uid_validity: 51,
            last_uid: 2,
        };
        let raw = b"Subject: Invoice ready\r\n\r\nPayment attached";

        assert_eq!(
            save_filtered_message(&root, &folder, &acct.id, checkpoint, 1, raw, &blacklist)
                .unwrap(),
            SaveOutcome::Skipped
        );
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "filter must not create a folder or mail file"
        );
        assert_eq!(
            save_filtered_message(&root, &folder, &acct.id, checkpoint, 1, raw, &whitelist)
                .unwrap(),
            SaveOutcome::Saved
        );
        assert_eq!(
            save_filtered_message(&root, &folder, &acct.id, checkpoint, 1, raw, &whitelist)
                .unwrap(),
            SaveOutcome::Existing
        );
        let dir = stable_folder_directory(&root, &folder).unwrap();
        let path = dir.join("Invoice ready.eml");
        fs::write(&path, b"previous local content").unwrap();
        assert_eq!(
            save_filtered_message(&root, &folder, &acct.id, checkpoint, 1, raw, &blacklist)
                .unwrap(),
            SaveOutcome::Skipped
        );
        assert_eq!(fs::read(&path).unwrap(), b"previous local content");
        assert!(
            save_filtered_message(&root, &folder, &acct.id, checkpoint, 1, raw, &whitelist)
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"previous local content");
    }

    #[test]
    fn bulk_filter_parse_errors_are_failures_and_keep_status_rule_metadata() {
        let parent = tempfile::tempdir().unwrap();
        let mut acct = account("decode-error");
        acct.auto_download.filter_mode = FilterMode::Blacklist;
        acct.auto_download.keywords = vec!["invoice".into()];
        let filter = effective_filter(&acct, true).unwrap();
        let root = account_directory(parent.path(), &acct).unwrap();
        let invalid = b"Subject: broken\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%";
        let error = save_filtered_message(
            &root,
            &folder("INBOX"),
            &acct.id,
            Checkpoint {
                uid_validity: 52,
                last_uid: 1,
            },
            1,
            invalid,
            &filter,
        )
        .unwrap_err();
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let mut status = Status {
            inherit_filters: true,
            filter_mode: filter.filter_mode,
            skipped_count: 2,
            ..Default::default()
        };
        failure(&mut status, error);
        finish(&mut status, &Control::default());
        assert_eq!(status.state, "error");
        assert_eq!(status.failed_count, 1);
        assert_eq!(status.skipped_count, 2);
        let value = serde_json::to_value(&status).unwrap();
        assert_eq!(value["inheritFilters"], true);
        assert_eq!(value["filterMode"], "blacklist");
        assert_eq!(value["skippedCount"], 2);
        let control = Control::default();
        control.cancel();
        finish(&mut status, &control);
        assert_eq!(status.state, "cancelled");
        assert!(status.inherit_filters);
        assert_eq!(status.filter_mode, FilterMode::Blacklist);
    }

    fn folder(path: &str) -> Folder {
        Folder {
            path: path.into(),
            name: "项目资料".into(),
            role: "other".into(),
        }
    }

    #[test]
    fn bulk_paths_are_readable_stable_and_isolated() {
        let parent = tempfile::tempdir().unwrap();
        let one = account_directory(parent.path(), &account("one")).unwrap();
        let two = account_directory(parent.path(), &account("two")).unwrap();
        assert_ne!(one, two);
        assert_eq!(
            one,
            account_directory(parent.path(), &account("one")).unwrap()
        );
        let a = stable_folder_directory(&one, &folder("../escape")).unwrap();
        let b = stable_folder_directory(&one, &folder("..\\escape")).unwrap();
        assert_ne!(a, b);
        assert!(a.starts_with(&one));
        assert_eq!(a.parent(), Some(one.as_path()));
        assert!(
            a.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .contains("项目资料")
        );
    }

    #[test]
    fn bulk_retries_reuse_exact_original_without_overwriting_conflicts() {
        let parent = tempfile::tempdir().unwrap();
        let acct = account("one");
        let root = account_directory(parent.path(), &acct).unwrap();
        let raw = b"Subject: Test\r\n\r\nwhole email\0binary attachment";
        let checkpoint = Checkpoint {
            uid_validity: 23,
            last_uid: 1,
        };
        assert!(!save_message(&root, &folder("INBOX"), &acct.id, checkpoint, 1, raw).unwrap());
        assert!(save_message(&root, &folder("INBOX"), &acct.id, checkpoint, 1, raw).unwrap());
        assert!(
            save_message(&root, &folder("INBOX"), &acct.id, checkpoint, 1, b"changed").is_err()
        );
        assert!(!save_message(&root, &folder("Sent"), &acct.id, checkpoint, 1, raw).unwrap());
        assert!(
            !save_message(
                &root,
                &folder("INBOX"),
                &acct.id,
                Checkpoint {
                    uid_validity: 24,
                    last_uid: 1
                },
                1,
                raw
            )
            .unwrap()
        );
        let dir = stable_folder_directory(&root, &folder("INBOX")).unwrap();
        let path = dir.join("Test.eml");
        assert_eq!(fs::read(path).unwrap(), raw);
    }

    #[cfg(unix)]
    #[test]
    fn bulk_rejects_symlinked_account_and_folder_directories() {
        use std::os::unix::fs::symlink;
        let parent = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let acct = account("one");
        let root = account_directory(parent.path(), &acct).unwrap();
        let dir = stable_folder_directory(&root, &folder("INBOX")).unwrap();
        fs::remove_dir_all(&dir).unwrap();
        symlink(outside.path(), &dir).unwrap();
        assert!(stable_folder_directory(&root, &folder("INBOX")).is_err());
        fs::remove_file(&dir).unwrap();
        fs::remove_dir_all(&root).unwrap();
        symlink(outside.path(), &root).unwrap();
        assert!(account_directory(parent.path(), &acct).is_err());
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn bulk_cancel_wakes_waiters_and_blocks_future_file_commits() {
        let control = Arc::new(Control::default());
        let waiting = control.clone();
        let task = tokio::spawn(async move { waiting.cancelled().await });
        control.cancel();
        timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
        assert!(control.is_cancelled());
        control.cancelled().await;
    }
}
