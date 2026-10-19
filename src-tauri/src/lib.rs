//! MailPulse 邮件客户端。
//!
//! 每个账号两条 IMAP 连接：
//! - 同步连接（sync.rs）：常驻 IDLE 盯收件箱，新邮件实时推给界面；
//! - 命令连接（这里）：读信、翻页、标记、移动等按需操作，空闲超过一分钟先探活再用。

mod auto_download;
mod bulk_download;
mod config;
mod download_protocol;
#[cfg(test)]
mod e2e;
mod export;
mod imap;
mod mime;
mod smtp;
mod sync;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures::future::BoxFuture;
use serde::Serialize;
use tauri::{AppHandle, Manager, State, async_runtime::JoinHandle};
use tauri_plugin_opener::OpenerExt;
use tokio::{sync::broadcast, time::timeout};

use config::{Account, NewAccount, Store};
use imap::{Folder, Imap, MessageView, OP_TIMEOUT, Preview, Summary};
use sync::Event;

type CmdResult<T> = std::result::Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

struct Cmd {
    s: Imap,
    selected: Option<String>,
    last: Instant,
}

impl Cmd {
    async fn select(&mut self, folder: &str) -> Result<()> {
        if self.selected.as_deref() != Some(folder) {
            self.selected = None;
            timeout(
                OP_TIMEOUT,
                download_protocol::checked_command(
                    &mut self.s,
                    &format!("SELECT {}", download_protocol::quoted_mailbox(folder)?),
                ),
            )
            .await
            .context("打开文件夹超时")?
            .with_context(|| format!("打开文件夹 {} 失败", imap::decode_mutf7(folder)))?;
            self.selected = Some(folder.to_string());
        }
        Ok(())
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Cmd>>>;
/// UID 只有在同一 UIDVALIDITY 内才唯一，文件夹重建后不能复用旧原文。
type MsgKey = (String, String, u32, u32);
type CachedBody = (Arc<Vec<u8>>, i64);
const BODY_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct BodyCache {
    entries: VecDeque<(MsgKey, CachedBody)>,
    bytes: usize,
}

impl BodyCache {
    fn insert(&mut self, key: MsgKey, raw: Arc<Vec<u8>>, date: i64) {
        self.entries.retain(|(k, _)| *k != key);
        self.bytes = self.entries.iter().map(|(_, (v, _))| v.len()).sum();
        // 大邮件仍可打开/导出，只是不长期留在缓存中。
        if raw.len() > BODY_CACHE_BYTES {
            return;
        }
        while self.entries.len() >= 30 || self.bytes > BODY_CACHE_BYTES - raw.len() {
            if let Some((_, (old, _))) = self.entries.pop_front() {
                self.bytes -= old.len();
            }
        }
        self.bytes += raw.len();
        self.entries.push_back((key, (raw, date)));
    }

    fn get(&self, key: &MsgKey) -> Option<CachedBody> {
        self.entries
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    fn remove_account(&mut self, id: &str) {
        self.entries.retain(|(k, _)| k.0 != id);
        self.bytes = self.entries.iter().map(|(_, (v, _))| v.len()).sum();
    }

    fn retain(&mut self, keep: impl FnMut(&(MsgKey, CachedBody)) -> bool) {
        self.entries.retain(keep);
        self.bytes = self.entries.iter().map(|(_, (v, _))| v.len()).sum();
    }
}

/// 界面启动时一次性拿到的当前状态；之后靠 "mail" 事件增量更新。
#[derive(Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Live {
    state: String,
    detail: String,
    unread: usize,
    inbox: Vec<Summary>,
}

pub struct AppState {
    store: Mutex<Store>,
    passwords: Mutex<HashMap<String, String>>,
    watchers: Mutex<HashMap<String, JoinHandle<()>>>,
    download_workers: Mutex<HashMap<String, JoinHandle<()>>>,
    download_gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    auto_downloads: Mutex<HashMap<String, auto_download::Status>>,
    bulk_downloads: Mutex<HashMap<String, bulk_download::Task>>,
    download_wake: broadcast::Sender<String>,
    slots: Mutex<HashMap<String, Slot>>,
    folders: Mutex<HashMap<String, Vec<Folder>>>,
    live: Mutex<HashMap<String, Live>>,
    /// 摘要请求（部分 FETCH）出错过的账号，不再请求，避免反复断线重连
    no_preview: Mutex<std::collections::HashSet<String>>,
    /// 最近打开的几封邮件原文，保存附件时不用再下载
    bodies: Arc<Mutex<BodyCache>>,
    wake: broadcast::Sender<()>,
}

impl AppState {
    fn download_gate(&self, id: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.download_gates
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    fn start_downloader(&self, app: &AppHandle, id: String) {
        let handle = tauri::async_runtime::spawn(auto_download::watch(app.clone(), id.clone()));
        if let Some(old) = self.download_workers.lock().unwrap().insert(id, handle) {
            old.abort();
        }
    }

    fn account(&self, id: &str) -> Result<(Account, String)> {
        let acct = self
            .store
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let pass = self
            .passwords
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| anyhow!("没有找到这个账号的授权码，请删除后重新添加"))?;
        Ok((acct, pass))
    }

    fn slot(&self, id: &str) -> Slot {
        self.slots
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_default()
            .clone()
    }

    /// 在该账号的命令连接上执行一组操作；出错就丢弃连接，下次重新登录。
    async fn with_cmd<T, F>(&self, id: &str, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(&'c mut Cmd, &'c Account) -> BoxFuture<'c, Result<T>>,
    {
        let (acct, pass) = self.account(id)?;
        let mut guard = self.slot(id).lock_owned().await;
        if let Some(c) = guard.as_mut() {
            // 空闲久了的连接可能已被服务器或 NAT 断开，先探活
            if c.last.elapsed() > Duration::from_secs(60)
                && !matches!(timeout(Duration::from_secs(5), c.s.noop()).await, Ok(Ok(_)))
            {
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(Cmd {
                s: imap::login(&acct, &pass).await?,
                selected: None,
                last: Instant::now(),
            });
        }
        let c = guard.as_mut().unwrap();
        let r = f(c, &acct).await;
        match &r {
            Ok(_) => c.last = Instant::now(),
            Err(_) => *guard = None,
        }
        r
    }

    async fn folders(&self, id: &str) -> Result<Vec<Folder>> {
        if let Some(f) = self.folders.lock().unwrap().get(id) {
            return Ok(f.clone());
        }
        let list = self
            .with_cmd(id, |c, _| {
                Box::pin(async move { imap::list_folders(&mut c.s).await })
            })
            .await?;
        self.folders
            .lock()
            .unwrap()
            .insert(id.to_string(), list.clone());
        Ok(list)
    }

    async fn folder_by_role(&self, id: &str, role: &str) -> Result<Option<String>> {
        Ok(self
            .folders(id)
            .await?
            .into_iter()
            .find(|f| f.role == role)
            .map(|f| f.path))
    }

    /// 同步任务推送事件时顺带记一份，界面（重新）加载时可以直接拿到。
    fn record(&self, ev: &Event) {
        // 全量下载任务在自己的任务锁内记录，防止旧任务事件覆盖新任务。
        if matches!(ev, Event::BulkDownload { .. }) {
            return;
        }
        if let Event::AutoDownload {
            account_id,
            state,
            detail,
            last_path,
            saved_count,
        } = ev
        {
            self.auto_downloads.lock().unwrap().insert(
                account_id.to_string(),
                auto_download::Status {
                    state: state.to_string(),
                    detail: detail.clone(),
                    last_path: last_path.clone(),
                    saved_count: *saved_count,
                },
            );
            return;
        }
        let mut live = self.live.lock().unwrap();
        match ev {
            Event::Status {
                account_id,
                state,
                detail,
            } => {
                let l = live.entry(account_id.to_string()).or_default();
                l.state = state.to_string();
                l.detail = detail.clone();
            }
            Event::Inbox {
                account_id,
                messages,
            } => live.entry(account_id.to_string()).or_default().inbox = messages.clone(),
            Event::NewMail {
                account_id,
                messages,
            } => {
                let l = live.entry(account_id.to_string()).or_default();
                let mut all = messages.clone();
                all.append(&mut l.inbox);
                l.inbox = all;
            }
            Event::Unread { account_id, count } => {
                live.entry(account_id.to_string()).or_default().unread = *count
            }
            Event::Flags { .. } | Event::AutoDownload { .. } | Event::BulkDownload { .. } => {}
        }
    }

    fn start_watcher(&self, app: &AppHandle, acct: Account, pass: String) {
        let id = acct.id.clone();
        self.start_downloader(app, id.clone());
        let handle =
            tauri::async_runtime::spawn(sync::watch(app.clone(), acct, pass, self.wake.clone()));
        if let Some(old) = self.watchers.lock().unwrap().insert(id, handle) {
            old.abort();
        }
    }
}

// ---------------------------------------------------------------------------
// 给界面调用的命令
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    accounts: Vec<Account>,
    live: HashMap<String, Live>,
    auto_downloads: HashMap<String, auto_download::Status>,
    bulk_downloads: HashMap<String, bulk_download::Status>,
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Snapshot {
    Snapshot {
        accounts: state.store.lock().unwrap().accounts.clone(),
        live: state.live.lock().unwrap().clone(),
        auto_downloads: state.auto_downloads.lock().unwrap().clone(),
        bulk_downloads: state
            .bulk_downloads
            .lock()
            .unwrap()
            .iter()
            .map(|(id, task)| (id.clone(), task.status.clone()))
            .collect(),
    }
}

/// 显示原生目录选择器；取消时返回空值且不改变配置。
#[tauri::command]
async fn choose_download_directory(directory: Option<String>) -> CmdResult<Option<String>> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("选择邮件保存文件夹")
        .set_can_create_directories(true);
    if let Some(directory) = directory.filter(|d| !d.is_empty()) {
        dialog = dialog.set_directory(directory);
    }
    Ok(dialog
        .pick_folder()
        .await
        .map(|folder| folder.path().display().to_string()))
}

/// 开始一次全账号归档；可继承已保存关键词，保持自动下载设置及持久进度不变。
#[tauri::command]
async fn start_bulk_download(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    directory: String,
    inherit_filters: Option<bool>,
) -> CmdResult<bulk_download::Status> {
    let _gate = state.download_gate(&account_id).lock_owned().await;
    if state
        .bulk_downloads
        .lock()
        .unwrap()
        .get(&account_id)
        .is_some_and(|task| task.status.is_active())
    {
        return Err("此账号正在下载全部邮件，请等待完成或先停止".into());
    }
    let (account, password) = state.account(&account_id).map_err(err)?;
    let inherit_filters = inherit_filters.unwrap_or(false);
    let filter = bulk_download::effective_filter(&account, inherit_filters).map_err(err)?;
    let copy = account.clone();
    let (parent, root) = tokio::task::spawn_blocking(move || -> Result<_> {
        let parent = auto_download::validate_directory(&directory)?;
        let root = bulk_download::account_directory(&parent, &copy)?;
        Ok((parent, root))
    })
    .await
    .map_err(|_| "无法准备邮件下载目录")?
    .map_err(err)?;
    let status = bulk_download::Status {
        state: "preparing".into(),
        detail: "正在连接邮箱并读取所有文件夹…".into(),
        directory: root.display().to_string(),
        inherit_filters,
        filter_mode: filter.filter_mode,
        keyword_scope: filter.keyword_scope,
        ..Default::default()
    };
    let control = Arc::new(bulk_download::Control::default());
    state.bulk_downloads.lock().unwrap().insert(
        account_id.clone(),
        bulk_download::Task {
            status: status.clone(),
            control: control.clone(),
        },
    );
    sync::emit(
        &app,
        Event::BulkDownload {
            account_id: &account_id,
            status: status.clone(),
        },
    );
    tauri::async_runtime::spawn(bulk_download::watch(
        app,
        account,
        password,
        parent,
        control,
        status.clone(),
    ));
    Ok(status)
}

/// 请求停止；后台确认网络读取和当前文件提交结束后发布最终状态。
#[tauri::command]
async fn cancel_bulk_download(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<bulk_download::Status> {
    let mut tasks = state.bulk_downloads.lock().unwrap();
    let task = tasks
        .get_mut(&account_id)
        .ok_or("此账号尚未开始下载全部邮件")?;
    if task.status.is_active() {
        task.control.cancel();
        task.status.state = "cancelling".into();
        task.status.detail = "正在停止，已保存的邮件将保留…".into();
        sync::emit(
            &app,
            Event::BulkDownload {
                account_id: &account_id,
                status: task.status.clone(),
            },
        );
    }
    Ok(task.status.clone())
}

/// 验证目录和收件边界后启用；关闭、路径和过滤规则变更与后台文件提交串行化。
#[tauri::command]
async fn save_auto_download(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    enabled: bool,
    directory: String,
    filter_mode: Option<auto_download::FilterMode>,
    keywords: Option<Vec<String>>,
    keyword_scope: Option<auto_download::KeywordScope>,
    field_keywords: Option<auto_download::FieldKeywords>,
) -> CmdResult<Account> {
    let _gate = state.download_gate(&account_id).lock_owned().await;
    let account = state
        .store
        .lock()
        .unwrap()
        .get(&account_id)
        .cloned()
        .ok_or("账号不存在")?;
    let old = &account.auto_download;
    let mut settings = old.clone();
    settings.enabled = enabled;
    // 兼容旧调用者：省略新字段时保留现有规则，避免意外取消过滤。
    settings.filter_mode = filter_mode.unwrap_or(old.filter_mode);
    settings.keyword_scope = keyword_scope.unwrap_or(old.keyword_scope);
    settings.keywords = keywords.unwrap_or_else(|| old.keywords.clone());
    settings.field_keywords = field_keywords.unwrap_or_else(|| old.field_keywords.clone());
    settings.normalize_filter_rules().map_err(err)?;
    if enabled {
        let canonical =
            tokio::task::spawn_blocking(move || auto_download::validate_directory(&directory))
                .await
                .map_err(|e| e.to_string())?
                .map_err(err)?;
        settings.directory = canonical.display().to_string();
        if old.needs_new_baseline(&settings) {
            let (_, pass) = state.account(&account_id).map_err(err)?;
            let mut session = imap::login(&account, &pass).await.map_err(err)?;
            let checkpoint = auto_download::baseline(&mut session).await.map_err(err)?;
            let _ = timeout(Duration::from_secs(3), session.logout()).await;
            settings.checkpoint = Some(checkpoint);
            settings.saved_count = 0;
            settings.last_path = None;
        }
    } else {
        settings.directory = directory;
    }
    // 相同设置保留进度与错误状态，不把已暂停的认证失败伪装成等待收件。
    if settings == *old {
        let _ = state.download_wake.send(account_id);
        return Ok(account);
    }
    settings.revision = old
        .revision
        .checked_add(1)
        .ok_or("设置版本已耗尽，请重新添加账号")?;
    let account = state
        .store
        .lock()
        .unwrap()
        .set_auto_download(&account_id, settings)
        .map_err(err)?;
    let status = auto_download::Status::from_settings(&account.auto_download);
    sync::emit(
        &app,
        Event::AutoDownload {
            account_id: &account_id,
            state: &status.state,
            detail: status.detail,
            last_path: status.last_path,
            saved_count: status.saved_count,
        },
    );
    let _ = state.download_wake.send(account_id);
    Ok(account)
}

/// 更新账号签名；授权码和服务器配置保持原值。
#[tauri::command]
fn save_signature(
    state: State<'_, AppState>,
    account_id: String,
    signature: String,
) -> CmdResult<Account> {
    state
        .store
        .lock()
        .unwrap()
        .set_signature(&account_id, signature)
        .map_err(err)
}

#[tauri::command]
async fn add_account(
    app: AppHandle,
    state: State<'_, AppState>,
    input: NewAccount,
) -> CmdResult<Account> {
    let id = format!("a{}", chrono::Utc::now().timestamp_millis());
    let (acct, pass) = input.into_account(id).map_err(err)?;
    if state
        .store
        .lock()
        .unwrap()
        .accounts
        .iter()
        .any(|a| a.email.eq_ignore_ascii_case(&acct.email))
    {
        return Err("这个邮箱已经添加过了".into());
    }
    // 先试登录，填错了当场提示
    let mut s = imap::login(&acct, &pass).await.map_err(err)?;
    let _ = timeout(Duration::from_secs(5), s.logout()).await;

    config::save_password(&acct.id, &pass).map_err(err)?;
    {
        let mut store = state.store.lock().unwrap();
        store.accounts.push(acct.clone());
        store.save().map_err(err)?;
    }
    state
        .passwords
        .lock()
        .unwrap()
        .insert(acct.id.clone(), pass.clone());
    state.start_watcher(&app, acct.clone(), pass);
    Ok(acct)
}

#[tauri::command]
async fn remove_account(state: State<'_, AppState>, account_id: String) -> CmdResult<()> {
    let _gate = state.download_gate(&account_id).lock_owned().await;
    let bulk_control = state
        .bulk_downloads
        .lock()
        .unwrap()
        .get(&account_id)
        .map(|task| task.control.clone());
    if let Some(control) = bulk_control {
        control.cancel();
        control.wait_for_writes().await;
    }
    {
        let mut store = state.store.lock().unwrap();
        let previous = store.accounts.clone();
        store.accounts.retain(|a| a.id != account_id);
        if let Err(error) = store.save() {
            store.accounts = previous;
            return Err(err(error));
        }
    }
    if let Some(h) = state.watchers.lock().unwrap().remove(&account_id) {
        h.abort();
    }
    if let Some(h) = state.download_workers.lock().unwrap().remove(&account_id) {
        h.abort();
    }
    state.auto_downloads.lock().unwrap().remove(&account_id);
    state.bulk_downloads.lock().unwrap().remove(&account_id);
    state.slots.lock().unwrap().remove(&account_id);
    state.folders.lock().unwrap().remove(&account_id);
    state.live.lock().unwrap().remove(&account_id);
    state.passwords.lock().unwrap().remove(&account_id);
    state.bodies.lock().unwrap().remove_account(&account_id);
    state.no_preview.lock().unwrap().remove(&account_id);
    config::delete_password(&account_id);
    Ok(())
}

#[tauri::command]
async fn list_folders(state: State<'_, AppState>, account_id: String) -> CmdResult<Vec<Folder>> {
    state.folders(&account_id).await.map_err(err)
}

/// 在邮箱服务器创建文件夹并刷新本地文件夹列表。
#[tauri::command]
async fn create_folder(
    state: State<'_, AppState>,
    account_id: String,
    name: String,
) -> CmdResult<Folder> {
    // 即使 CREATE 后回读失败，也不能继续保留旧列表。
    state.folders.lock().unwrap().remove(&account_id);
    let (folder, folders) = state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move { imap::create_folder(&mut c.s, &name).await })
        })
        .await
        .map_err(err)?;
    state.folders.lock().unwrap().insert(account_id, folders);
    Ok(folder)
}

#[tauri::command]
async fn list_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    before: Option<u32>,
    flagged_only: Option<bool>,
) -> CmdResult<Vec<Summary>> {
    state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                c.select(&folder).await?;
                let uids = if flagged_only == Some(true) {
                    let mut u: Vec<u32> = timeout(OP_TIMEOUT, c.s.uid_search("FLAGGED"))
                        .await??
                        .into_iter()
                        .collect();
                    u.sort_unstable();
                    let start = u.len().saturating_sub(imap::PAGE);
                    u.split_off(start)
                } else {
                    imap::page_uids(&mut c.s, before).await?
                };
                if uids.is_empty() {
                    return Ok(Vec::new());
                }
                imap::fetch_summaries(&mut c.s, &imap::uid_set(&uids), &acct.id, &folder).await
            })
        })
        .await
        .map_err(err)
}

#[tauri::command]
async fn get_previews(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uids: Vec<u32>,
) -> CmdResult<Vec<Preview>> {
    if uids.is_empty() || state.no_preview.lock().unwrap().contains(&account_id) {
        return Ok(Vec::new());
    }
    let r = state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move {
                c.select(&folder).await?;
                imap::fetch_previews(&mut c.s, &imap::uid_set(&uids)).await
            })
        })
        .await;
    match r {
        Ok(v) => Ok(v),
        // 摘要只是锦上添花：服务器回包不合规范时关掉这个账号的摘要，列表照常显示
        Err(e) if format!("{e:#}").contains("during parsing") => {
            eprintln!("账号 {account_id} 的摘要回包无法解析，已停用摘要：{e:#}");
            state.no_preview.lock().unwrap().insert(account_id);
            Ok(Vec::new())
        }
        Err(e) => Err(err(e)),
    }
}

// 每次操作重新确认文件夹代际，再访问缓存；引用来源必须保持原代际。
async fn read_raw<T>(
    session: &mut async_imap::Session<T>,
    cache: &Mutex<BodyCache>,
    account_id: &str,
    folder: &str,
    uid: u32,
    peek: bool,
    expected_validity: Option<u32>,
) -> Result<(Arc<Vec<u8>>, i64, u32)>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    use download_protocol::{ArchiveData, checked_command, quoted_mailbox};
    if uid == 0 {
        bail!("邮件位置无效，请刷新后重试");
    }
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(session, &format!("SELECT {}", quoted_mailbox(folder)?)),
    )
    .await
    .context("打开文件夹超时")??;
    let validity = responses
        .into_iter()
        .filter_map(|response| match response {
            ArchiveData::Validity(value) if value > 0 => Some(value),
            _ => None,
        })
        .last()
        .ok_or_else(|| anyhow!("服务器未提供 UIDVALIDITY，无法确认邮件身份"))?;
    if expected_validity.is_some_and(|expected| expected != validity) {
        bail!("原邮件文件夹已发生变化，请重新打开原邮件后重试");
    }
    let key = (account_id.to_string(), folder.to_string(), validity, uid);
    if peek {
        if let Some((raw, date)) = cache.lock().unwrap().get(&key) {
            return Ok((raw, date, validity));
        }
    }
    let items = if peek {
        "(UID INTERNALDATE BODY.PEEK[])"
    } else {
        "(UID INTERNALDATE BODY[])"
    };
    let responses = timeout(
        OP_TIMEOUT * 3,
        checked_command(session, &format!("UID FETCH {uid} {items}")),
    )
    .await
    .context("下载邮件超时")??;
    let (raw, date) = responses
        .into_iter()
        .find_map(|response| match response {
            ArchiveData::Message {
                uid: Some(found),
                raw: Some(raw),
                date,
            } if found == uid => Some((raw, date)),
            _ => None,
        })
        .ok_or_else(|| anyhow!("服务器没有返回邮件原文，邮件可能已在别处删除，请刷新后重试"))?;
    let raw = Arc::new(raw);
    cache.lock().unwrap().insert(key, raw.clone(), date);
    Ok((raw, date, validity))
}

async fn fetch_raw(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    peek: bool,
    expected_validity: Option<u32>,
) -> Result<(Arc<Vec<u8>>, i64, u32)> {
    let folder = folder.to_string();
    let cache = state.bodies.clone();
    state
        .with_cmd(account_id, |c, acct| {
            Box::pin(async move {
                c.selected = None;
                let result = read_raw(
                    &mut c.s,
                    &cache,
                    &acct.id,
                    &folder,
                    uid,
                    peek,
                    expected_validity,
                )
                .await?;
                c.selected = Some(folder);
                Ok(result)
            })
        })
        .await
}

/// 打开一封邮件（同时标记为已读）
#[tauri::command]
async fn get_message(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<MessageView> {
    let (raw, date, validity) = fetch_raw(&state, &account_id, &folder, uid, false, None)
        .await
        .map_err(err)?;
    let mut view = imap::parse_message(&raw, date).map_err(err)?;
    view.uid_validity = Some(validity);
    Ok(view)
}

/// 下载整封邮件原文为 .eml，保留附件和邮件头且不更改未读状态。
#[tauri::command]
async fn save_message(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<String> {
    let raw = fetch_raw(&state, &account_id, &folder, uid, true, None)
        .await
        .map_err(err)?
        .0;
    let dir = dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|dir| dir.join("Downloads")))
        .ok_or("找不到下载文件夹")?;
    let path = export::save_eml(&dir, &raw, uid).map_err(err)?;
    if let Err(error) = app.opener().reveal_item_in_dir(&path) {
        eprintln!(
            "邮件已保存到 {}，但无法在访达中显示：{error}",
            path.display()
        );
    }
    Ok(path.display().to_string())
}

#[tauri::command]
async fn set_flag(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uids: Vec<u32>,
    flag: String,
    value: bool,
) -> CmdResult<()> {
    let flag = match flag.as_str() {
        "seen" => "\\Seen",
        "flagged" => "\\Flagged",
        _ => return Err("未知标记".into()),
    };
    state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move {
                c.select(&folder).await?;
                let op = format!("{}FLAGS.SILENT ({flag})", if value { "+" } else { "-" });
                timeout(
                    OP_TIMEOUT,
                    download_protocol::checked_command(
                        &mut c.s,
                        &format!("UID STORE {} {op}", imap::uid_set(&uids)),
                    ),
                )
                .await??;
                Ok(())
            })
        })
        .await
        .map_err(err)
}

fn resolve_move_target(
    folders: &[Folder],
    source: &str,
    role: &str,
    explicit: Option<&str>,
) -> Result<Option<String>> {
    if let Some(path) = explicit {
        let target = folders
            .iter()
            .find(|f| imap::mailbox_eq(&f.path, path))
            .ok_or_else(|| anyhow!("目标文件夹不存在或不可选择，请刷新文件夹列表"))?;
        if imap::mailbox_eq(&target.path, source) {
            bail!("邮件已经在这个文件夹中");
        }
        return Ok(Some(target.path.clone()));
    }
    let target = folders
        .iter()
        .find(|f| f.role == role)
        .or_else(|| {
            if role == "archive" {
                folders.iter().find(|f| f.role == "all")
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow!(match role {
                "archive" => "这个邮箱没有归档文件夹",
                "junk" => "这个邮箱没有垃圾邮件文件夹",
                "trash" => "这个邮箱没有可用的已删除文件夹，请在邮箱网页版设置后重试",
                _ => "找不到目标文件夹",
            })
        })?;
    if imap::mailbox_eq(&target.path, source) {
        if role == "trash" && target.role == "trash" {
            return Ok(None);
        }
        bail!("邮件已经在这个文件夹中");
    }
    Ok(Some(target.path.clone()))
}

fn remove_moved_inbox(live: &mut Live, uids: &[u32]) {
    let removed_unread = live
        .inbox
        .iter()
        .filter(|m| uids.contains(&m.uid) && !m.seen)
        .count();
    live.inbox.retain(|m| !uids.contains(&m.uid));
    live.unread = live.unread.saturating_sub(removed_unread);
}

/// 移动到某类文件夹或明确指定的可选文件夹；仅在已确认的已删除文件夹中允许彻底删除。
#[tauri::command]
async fn move_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uids: Vec<u32>,
    role: String,
    target: Option<String>,
) -> CmdResult<()> {
    if uids.is_empty() || uids.contains(&0) {
        return Err("请选择有效的邮件".into());
    }
    // 用服务器的最新可选列表校验目标，避免使用已被其他客户端删除的缓存路径。
    let folders = state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move { imap::list_folders(&mut c.s).await })
        })
        .await
        .map_err(err)?;
    state
        .folders
        .lock()
        .unwrap()
        .insert(account_id.clone(), folders.clone());
    let target = resolve_move_target(&folders, &folder, &role, target.as_deref()).map_err(err)?;
    let source = folder.clone();
    let destination = target.clone();
    let set = imap::uid_set(&uids);
    state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move {
                c.select(&source).await?;
                imap::move_uids(&mut c.s, &set, destination.as_deref()).await
            })
        })
        .await
        .map_err(err)?;

    state
        .bodies
        .lock()
        .unwrap()
        .retain(|((account, path, _, uid), _)| {
            account != &account_id
                || !((imap::mailbox_eq(path, &folder) && uids.contains(uid))
                    || target.as_deref().is_some_and(|t| imap::mailbox_eq(path, t)))
        });
    let source_inbox = folder.eq_ignore_ascii_case("INBOX");
    let target_inbox = target
        .as_deref()
        .is_some_and(|p| p.eq_ignore_ascii_case("INBOX"));
    if source_inbox || target_inbox {
        {
            let mut live = state.live.lock().unwrap();
            let live = live.entry(account_id.clone()).or_default();
            if source_inbox {
                remove_moved_inbox(live, &uids);
            }
            if target_inbox {
                live.inbox.clear();
            }
        }
        // 移动已经成功；刷新出错不能让界面重试同一移动。
        let refreshed = state
            .with_cmd(&account_id, |c, acct| {
                Box::pin(async move {
                    c.select("INBOX").await?;
                    let uids = imap::page_uids(&mut c.s, None).await?;
                    let inbox = if uids.is_empty() {
                        Vec::new()
                    } else {
                        imap::fetch_summaries(&mut c.s, &imap::uid_set(&uids), &acct.id, "INBOX")
                            .await?
                    };
                    let unread = imap::unseen_count(&mut c.s).await?;
                    Ok((inbox, unread))
                })
            })
            .await;
        match refreshed {
            Ok((inbox, unread)) => {
                let mut live = state.live.lock().unwrap();
                let live = live.entry(account_id.clone()).or_default();
                live.inbox = inbox;
                live.unread = unread;
            }
            Err(error) => {
                eprintln!("移动成功后刷新收件箱失败：{error:#}");
                let _ = state.wake.send(());
            }
        }
    }
    Ok(())
}

fn unique_path(dir: PathBuf, name: &str) -> PathBuf {
    let clean: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = if clean.trim().is_empty() {
        "附件".to_string()
    } else {
        clean
    };
    let p = dir.join(&clean);
    if !p.exists() {
        return p;
    }
    let (stem, ext) = match clean.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() => (s.to_string(), format!(".{e}")),
        _ => (clean.clone(), String::new()),
    };
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// 附件保存到「下载」并在访达中显示
#[tauri::command]
async fn save_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
) -> CmdResult<String> {
    let raw = fetch_raw(&state, &account_id, &folder, uid, true, uid_validity)
        .await
        .map_err(err)?
        .0;
    let (name, data) = imap::attachment_bytes(&raw, index).map_err(err)?;
    let dir = dirs::download_dir()
        .or_else(dirs::home_dir)
        .ok_or("找不到下载文件夹")?;
    let path = unique_path(dir, &name);
    std::fs::write(&path, data).map_err(|e| format!("保存失败：{e}"))?;
    let _ = app.opener().reveal_item_in_dir(&path);
    Ok(path.display().to_string())
}

#[tauri::command]
fn open_link(app: AppHandle, url: String) -> CmdResult<()> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:"))
    {
        return Err("不支持打开这种链接".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn send_mail(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: smtp::Draft,
) -> CmdResult<()> {
    let (acct, pass) = state.account(&draft.account_id).map_err(err)?;
    let quote_raw = if draft.html_body.is_some() && !draft.quote_cids.is_empty() {
        let source = draft
            .quote_source
            .as_ref()
            .ok_or("缺少引用邮件，无法读取内嵌图片")?;
        if source.uid == 0 || source.folder.is_empty() {
            return Err("引用邮件的位置无效，请重新打开原邮件后回复".into());
        }
        // 引用来源与当前发件账号可以不同，但必须仍是已配置、可访问的账号。
        state.account(&source.account_id).map_err(err)?;
        let validity = source
            .uid_validity
            .filter(|value| *value > 0)
            .ok_or("无法确认引用邮件的身份，请重新打开原邮件后回复")?;
        Some(
            fetch_raw(
                &state,
                &source.account_id,
                &source.folder,
                source.uid,
                true,
                Some(validity),
            )
            .await
            .map_err(|e| format!("读取引用邮件的内嵌图片失败：{e:#}"))?
            .0,
        )
    } else {
        None
    };
    let (msg, message_id) = match quote_raw.as_ref() {
        Some(raw) => smtp::build_with_quote(&acct, &draft, Some(raw.as_slice())),
        None => smtp::build(&acct, &draft),
    }
    .map_err(err)?;
    let raw = msg.formatted();
    smtp::send(&acct, &pass, msg).await.map_err(err)?;

    // 后台确认「已发送」里有这封，没有再补存，界面不用等
    let id = acct.id.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(4)).await;
        let state = app.state::<AppState>();
        if let Err(e) = save_to_sent(&state, &id, &message_id, raw).await {
            eprintln!("保存到已发送失败：{e:#}");
        }
    });
    Ok(())
}

async fn save_to_sent(state: &AppState, id: &str, message_id: &str, raw: Vec<u8>) -> Result<()> {
    let Some(sent) = state.folder_by_role(id, "sent").await? else {
        bail!("没有已发送文件夹")
    };
    let mid = message_id.to_string();
    state
        .with_cmd(id, |c, _| {
            Box::pin(async move {
                c.select(&sent).await?;
                imap::ensure_saved(&mut c.s, &sent, &mid, &raw)
                    .await
                    .map(|_| ())
            })
        })
        .await
}

/// Dock 图标上的未读数。只在 macOS 上设置：Linux 上这个调用走 D-Bus，没有会话总线时会卡住主线程。
#[tauri::command]
fn set_badge(app: AppHandle, count: i64) {
    #[cfg(target_os = "macos")]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if count > 0 { Some(count) } else { None });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = (app, count);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let dir = app.path().app_config_dir()?;
            let store = Store::load(dir)?;
            let (wake, _) = broadcast::channel(4);
            let (download_wake, _) = broadcast::channel(64);
            sync::spawn_wake_detector(wake.clone());

            let mut passwords = HashMap::new();
            let mut live = HashMap::new();
            for a in &store.accounts {
                match config::load_password(&a.id) {
                    Ok(p) => {
                        passwords.insert(a.id.clone(), p);
                    }
                    Err(e) => {
                        live.insert(
                            a.id.clone(),
                            Live {
                                state: "error".into(),
                                detail: format!("{e:#}"),
                                ..Default::default()
                            },
                        );
                    }
                }
            }
            let accounts = store.accounts.clone();
            let auto_downloads = accounts
                .iter()
                .map(|account| {
                    (
                        account.id.clone(),
                        auto_download::Status::from_settings(&account.auto_download),
                    )
                })
                .collect();
            app.manage(AppState {
                store: Mutex::new(store),
                passwords: Mutex::new(passwords.clone()),
                watchers: Mutex::default(),
                download_workers: Mutex::default(),
                download_gates: Mutex::default(),
                auto_downloads: Mutex::new(auto_downloads),
                bulk_downloads: Mutex::default(),
                download_wake,
                slots: Mutex::default(),
                folders: Mutex::default(),
                live: Mutex::new(live),
                no_preview: Mutex::default(),
                bodies: Arc::default(),
                wake,
            });
            // 仅 debug 构建：把 MAILPULSE_DEBUG_SCRIPT 指向的 JS 注入页面，用于本地自动化测试
            #[cfg(debug_assertions)]
            if let Ok(path) = std::env::var("MAILPULSE_DEBUG_SCRIPT") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if let (Ok(js), Some(w)) = (
                        std::fs::read_to_string(&path),
                        handle.get_webview_window("main"),
                    ) {
                        let _ = w.eval(&js);
                    }
                });
            }
            let state = app.state::<AppState>();
            for a in accounts {
                if let Some(p) = passwords.remove(&a.id) {
                    state.start_watcher(app.handle(), a, p);
                } else {
                    state.start_downloader(app.handle(), a.id);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            add_account,
            remove_account,
            save_signature,
            save_auto_download,
            choose_download_directory,
            start_bulk_download,
            cancel_bulk_download,
            list_folders,
            create_folder,
            list_messages,
            get_previews,
            get_message,
            save_message,
            set_flag,
            move_messages,
            save_attachment,
            open_link,
            send_mail,
            set_badge,
        ])
        .run(tauri::generate_context!())
        .expect("启动 MailPulse 失败");
}

#[cfg(test)]
mod command_tests {
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

    fn selected(validity: u32) -> (String, Option<&'static str>) {
        (
            format!("* OK [UIDVALIDITY {validity}] valid\r\n"),
            Some("OK selected"),
        )
    }

    fn body_response(raw: &str) -> String {
        format!(
            "* 1 FETCH (UID 1 FLAGS (\\Seen))\r\n* 1 FETCH (UID 1 INTERNALDATE \"02-Jan-2025 03:04:05 +0000\" BODY[] {{{}}}\r\n{raw})\r\n",
            raw.len()
        )
    }

    #[tokio::test]
    async fn manual_download_cannot_reuse_previous_mailbox_generation() {
        let cache = Mutex::new(BodyCache::default());
        cache.lock().unwrap().insert(
            ("a".into(), "INBOX".into(), 7, 1),
            Arc::new(b"old mail".to_vec()),
            0,
        );
        let mut session = scripted_session(vec![
            selected(8),
            (body_response("new mail"), Some("OK fetched")),
        ])
        .await;
        let (raw, date, validity) = read_raw(&mut session, &cache, "a", "INBOX", 1, true, None)
            .await
            .unwrap();
        assert_eq!(raw.as_slice(), b"new mail");
        assert_eq!(validity, 8);
        assert_eq!(date, 1_735_787_045_000);
    }

    #[tokio::test]
    async fn quote_and_attachment_reject_changed_generation_even_with_cached_source() {
        let cache = Mutex::new(BodyCache::default());
        cache.lock().unwrap().insert(
            ("a".into(), "INBOX".into(), 7, 1),
            Arc::new(b"old mail".to_vec()),
            0,
        );
        let mut session = scripted_session(vec![selected(8)]).await;
        let error = read_raw(&mut session, &cache, "a", "INBOX", 1, true, Some(7))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("文件夹已发生变化"));
    }

    #[tokio::test]
    async fn validated_same_generation_can_reuse_cached_body() {
        let cache = Mutex::new(BodyCache::default());
        cache.lock().unwrap().insert(
            ("a".into(), "INBOX".into(), 7, 1),
            Arc::new(b"same mail".to_vec()),
            42,
        );
        let mut session = scripted_session(vec![selected(7)]).await;
        let (raw, date, _) = read_raw(&mut session, &cache, "a", "INBOX", 1, true, Some(7))
            .await
            .unwrap();
        assert_eq!(raw.as_slice(), b"same mail");
        assert_eq!(date, 42);
    }

    #[tokio::test]
    async fn reader_requires_confirmed_select_and_fetch_before_caching() {
        for completion in [Some("NO rejected"), Some("BAD rejected"), None] {
            let cache = Mutex::new(BodyCache::default());
            let mut session = scripted_session(vec![(selected(7).0, completion)]).await;
            assert!(
                read_raw(&mut session, &cache, "a", "INBOX", 1, false, None)
                    .await
                    .is_err()
            );
            let mut session =
                scripted_session(vec![selected(7), (body_response("incomplete"), completion)])
                    .await;
            assert!(
                read_raw(&mut session, &cache, "a", "INBOX", 1, false, None)
                    .await
                    .is_err()
            );
            assert!(cache.lock().unwrap().entries.is_empty());
        }
    }

    #[test]
    fn cache_isolated_by_account_folder_and_generation_and_bounded_by_count() {
        let mut cache = BodyCache::default();
        cache.insert(("a".into(), "INBOX".into(), 7, 1), Arc::new(vec![1]), 0);
        for key in [
            ("b", "INBOX", 7, 1),
            ("a", "Sent", 7, 1),
            ("a", "INBOX", 8, 1),
        ] {
            assert!(
                cache
                    .get(&(key.0.into(), key.1.into(), key.2, key.3))
                    .is_none()
            );
        }
        for uid in 2..=31 {
            cache.insert(("a".into(), "INBOX".into(), 7, uid), Arc::new(vec![1]), 0);
        }
        assert_eq!(cache.entries.len(), 30);
        assert_eq!(cache.bytes, 30);
        assert!(cache.get(&("a".into(), "INBOX".into(), 7, 1)).is_none());
    }

    #[test]
    fn mail_cache_evicts_by_bytes_and_does_not_retain_oversize_mail() {
        let mut cache = BodyCache::default();
        let key = |uid| ("a".into(), "INBOX".into(), 7, uid);
        cache.insert(key(1), Arc::new(vec![0; 33 * 1024 * 1024]), 0);
        cache.insert(key(2), Arc::new(vec![0; 33 * 1024 * 1024]), 0);
        assert!(
            cache.get(&key(1)).is_none(),
            "cache must stay within 64 MiB"
        );
        cache.insert(key(3), Arc::new(vec![0; 65 * 1024 * 1024]), 0);
        assert!(
            cache.get(&key(3)).is_none(),
            "oversize mail must not be retained"
        );
        assert!(cache.get(&key(2)).is_some());
        assert_eq!(cache.bytes, 33 * 1024 * 1024);
        cache.remove_account("a");
        assert_eq!(cache.bytes, 0);
    }

    fn folder(path: &str, role: &str) -> Folder {
        Folder {
            path: path.into(),
            name: path.into(),
            role: role.into(),
        }
    }

    #[test]
    fn move_target_must_exist_and_permanent_delete_requires_confirmed_trash() {
        let folders = vec![
            folder("INBOX", "inbox"),
            folder("Trash", "trash"),
            folder("Work", "other"),
        ];
        assert_eq!(
            resolve_move_target(&folders, "INBOX", "", Some("Work")).unwrap(),
            Some("Work".into())
        );
        assert!(resolve_move_target(&folders, "INBOX", "", Some("missing")).is_err());
        assert!(resolve_move_target(&folders, "INBOX", "", Some("inbox")).is_err());
        assert!(resolve_move_target(&folders, "Trash", "trash", Some("Trash")).is_err());
        assert!(
            resolve_move_target(&folders, "Trash", "trash", None)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            resolve_move_target(&folders, "INBOX", "trash", None).unwrap(),
            Some("Trash".into())
        );
        assert!(resolve_move_target(&[folder("INBOX", "inbox")], "INBOX", "trash", None).is_err());
    }

    #[test]
    fn successful_move_removes_live_inbox_entries_and_unread_count() {
        let make = |uid, seen| Summary {
            account_id: "a".into(),
            folder: "INBOX".into(),
            uid,
            from_name: String::new(),
            from_addr: String::new(),
            to: String::new(),
            subject: String::new(),
            date: 0,
            seen,
            flagged: false,
            attachment: false,
        };
        let mut live = Live {
            unread: 5,
            inbox: vec![make(1, false), make(2, true), make(3, false)],
            ..Default::default()
        };
        remove_moved_inbox(&mut live, &[1, 2]);
        assert_eq!(
            live.inbox.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![3]
        );
        assert_eq!(live.unread, 4);
    }
}
