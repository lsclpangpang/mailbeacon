//! MailBeacon 邮件客户端。
//!
//! 每个账号两条 IMAP 连接：
//! - 同步连接（sync.rs）：常驻 IDLE 盯收件箱，新邮件实时推给界面；
//! - 命令连接（这里）：读信、翻页、标记、移动等按需操作，空闲超过一分钟先探活再用。

mod auto_download;
mod bulk_download;
mod config;
mod contacts;
mod download_protocol;
mod drafts;
#[cfg(test)]
mod e2e;
mod export;
mod html_mail;
mod imap;
mod mime;
mod outbox;
mod pop3;
mod prefs;
mod rules;
mod smtp;
mod sync;
mod templates;
#[cfg(test)]
mod test_imap;
mod threads;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures::future::BoxFuture;
use serde::Serialize;
use tauri::menu::{MenuBuilder, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, State, async_runtime::JoinHandle};
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
    /// POP3 账号的稳定 id、列表缓存和本地已读状态；锁同时串行化该账号的 POP3 连接
    pop3: Mutex<HashMap<String, pop3::SharedMailbox>>,
    /// 写信时选中的本地文件（令牌 → 路径）
    attachments: drafts::AttachmentRegistry,
    /// POP3、没有草稿箱或离线时的本地草稿
    local_drafts: drafts::LocalDrafts,
    /// 定时发送的邮件
    outbox: outbox::Outbox,
    /// 叫醒后台发送任务（刚定了时间、改了时间、立即发送）
    outbox_wake: tokio::sync::Notify,
    /// 后台到点发送和「立即发送」不能同时发同一封
    outbox_send: tokio::sync::Mutex<()>,
    /// 已经发出、但本地副本没删掉的定时邮件：不能再发
    outbox_sent: Mutex<std::collections::HashSet<(String, u32)>>,
    /// 本地数据库：通讯录、最近联系人等。打不开时为错误信息，其余功能照常使用。
    db: std::result::Result<contacts::Db, String>,
    /// 已记录过发件人的邮件（账号、文件夹、UID），同一封邮件只记一次
    recorded_senders: Mutex<std::collections::HashSet<(String, String, u32)>>,
    /// 通知、托盘、开机启动等应用偏好（存在本地数据库里）
    prefs: Mutex<prefs::Prefs>,
    /// 收信规则（按顺序执行）
    rules: Mutex<Vec<rules::Rule>>,
    /// 每个账号执行规则时要记住的东西（看到哪里了、移走过哪些、失败了几次）
    rule_memory: Mutex<HashMap<String, rules::Memory>>,
    /// 服务器不支持按邮件头搜索的账号（及发现的时间）：一小时内会话里只显示已加载的邮件
    no_thread_search: Mutex<HashMap<String, Instant>>,
    wake: broadcast::Sender<()>,
}

impl AppState {
    fn rule_memory<R>(&self, account_id: &str, f: impl FnOnce(&mut rules::Memory) -> R) -> R {
        f(self
            .rule_memory
            .lock()
            .unwrap()
            .entry(account_id.to_string())
            .or_insert_with(|| rules::Memory::started(chrono::Utc::now().timestamp_millis())))
    }

    fn db(&self) -> Result<&contacts::Db> {
        self.db.as_ref().map_err(|e| anyhow!("{e}"))
    }

    /// 打开邮件后记下发件人（见 contacts::sender_worth_recording），同一封邮件只记一次。
    fn record_sender(&self, account_id: &str, folder: &str, uid: u32, name: &str, addr: &str) {
        let Ok(db) = self.db() else {
            return;
        };
        let role = if folder == "INBOX" {
            "inbox".to_string()
        } else if folder == drafts::LOCAL_DRAFTS {
            "local-drafts".to_string()
        } else {
            self.folders
                .lock()
                .unwrap()
                .get(account_id)
                .and_then(|list| {
                    list.iter()
                        .find(|f| f.path == folder)
                        .map(|f| f.role.clone())
                })
                .unwrap_or_else(|| "unknown".into())
        };
        let own: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .accounts
            .iter()
            .map(|a| a.email.clone())
            .collect();
        if !contacts::sender_worth_recording(&role, addr, &own) {
            return;
        }
        let key = (account_id.to_string(), folder.to_string(), uid);
        if !self.recorded_senders.lock().unwrap().insert(key) {
            return;
        }
        let _ = db.record_received(name, addr);
    }

    fn is_pop3(&self, id: &str) -> Result<bool> {
        Ok(self
            .store
            .lock()
            .unwrap()
            .get(id)
            .ok_or_else(|| anyhow!("账号不存在"))?
            .receive_protocol
            == config::ReceiveProtocol::Pop3)
    }

    fn pop3_mailbox(&self, id: &str) -> pop3::SharedMailbox {
        let dir = self.store.lock().unwrap().dir();
        self.pop3
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(pop3::Handle::load(&dir, id)))
            .clone()
    }

    async fn pop3_session(&self, id: &str) -> Result<pop3::Session> {
        let (account, password) = self.account(id)?;
        pop3::Session::open(self.pop3_mailbox(id), &account, &password, false).await
    }

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
        if self.is_pop3(id)? {
            return Ok(vec![Folder {
                path: "INBOX".into(),
                name: "收件箱".into(),
                role: "inbox".into(),
            }]);
        }
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
            Event::Flags { .. }
            | Event::AutoDownload { .. }
            | Event::BulkDownload { .. }
            | Event::RuleError { .. } => {}
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

async fn pop3_list(
    state: &AppState,
    account_id: &str,
    before: Option<u32>,
) -> Result<Vec<Summary>> {
    let mut session = state.pop3_session(account_id).await?;
    let mut ids = session.ids();
    if let Some(before) = before {
        ids.retain(|id| *id < before);
    }
    let start = ids.len().saturating_sub(imap::PAGE);
    let mut result = session.summaries(&ids[start..]).await?;
    session.close().await;
    result.reverse();
    Ok(result)
}

/// POP3 的 id 在本进程内固定对应一个 UIDL，所以缓存的原文不会被
/// 删除后重新编号的另一封邮件冒用；每次仍确认邮件还在服务器上。
async fn pop3_raw(
    state: &AppState,
    account_id: &str,
    uid: u32,
    mark_read: bool,
) -> Result<(Arc<Vec<u8>>, i64)> {
    let mut session = state.pop3_session(account_id).await?;
    if !session.ids().contains(&uid) {
        session.close().await;
        bail!("POP3 邮件已从服务器删除，请刷新后重试");
    }
    if mark_read {
        session.set_read(&[uid], true);
    }
    let key = (account_id.to_string(), "INBOX".to_string(), 0, uid);
    let cached = { state.bodies.lock().unwrap().get(&key) };
    if let Some((raw, date)) = cached {
        session.close().await;
        return Ok((raw, date));
    }
    let raw = Arc::new(session.raw(uid).await?);
    session.close().await;
    let date = pop3::summary_from_raw(&raw, account_id, uid, 0)
        .map(|summary| summary.date)
        .unwrap_or(0);
    state.bodies.lock().unwrap().insert(key, raw.clone(), date);
    Ok((raw, date))
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
    if account.receive_protocol == config::ReceiveProtocol::Pop3 {
        return Err(
            "POP3 账号当前支持收件箱、搜索、阅读和导出；自动下载与全量归档需要 IMAP".into(),
        );
    }
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
    if account.receive_protocol == config::ReceiveProtocol::Pop3 && enabled {
        return Err("POP3 账号暂不支持自动下载，请改用 IMAP 接收协议".into());
    }
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
    // 先试登录，填错了当场提示。POP3 只验证收信连接；SMTP 会在首次发信时验证。
    match acct.receive_protocol {
        config::ReceiveProtocol::Imap => {
            let mut s = imap::login(&acct, &pass).await.map_err(err)?;
            let _ = timeout(Duration::from_secs(5), s.logout()).await;
        }
        config::ReceiveProtocol::Pop3 => {
            let s = pop3::Pop3::login(&acct, &pass).await.map_err(err)?;
            let _ = s.quit().await;
        }
    }

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
    state.local_drafts.remove_account(&account_id);
    state.outbox.remove_account(&account_id);
    let dir = state.store.lock().unwrap().dir();
    let mailbox = state.pop3.lock().unwrap().remove(&account_id);
    let id = account_id.clone();
    // 先停止搜索和登录，等进行中的会话结束后删除状态文件。
    tauri::async_runtime::spawn(async move {
        match mailbox {
            Some(mailbox) => mailbox.forget(&dir, &id).await,
            None => pop3::remove_state(&dir, &id),
        }
    });
    state.no_preview.lock().unwrap().remove(&account_id);
    config::delete_password(&account_id);
    let mut prefs = state.prefs.lock().unwrap();
    if prefs.forget_account(&account_id) {
        if let Ok(db) = state.db() {
            let _ = db.save_prefs(&prefs);
        }
    }
    state.rule_memory.lock().unwrap().remove(&account_id);
    let mut rules = state.rules.lock().unwrap();
    let before = rules.len();
    rules.retain(|r| r.account_id != account_id);
    if rules.len() != before {
        if let Ok(db) = state.db() {
            let _ = db.save_rules(&rules);
        }
    }
    Ok(())
}

#[tauri::command]
async fn list_folders(state: State<'_, AppState>, account_id: String) -> CmdResult<Vec<Folder>> {
    let mut folders = state.folders(&account_id).await.map_err(err)?;
    // 有本地草稿时才显示这个虚拟文件夹
    if !state
        .local_drafts
        .list(&account_id)
        .unwrap_or_default()
        .is_empty()
    {
        folders.push(Folder {
            path: drafts::LOCAL_DRAFTS.into(),
            name: "本地草稿".into(),
            role: "local-drafts".into(),
        });
    }
    if !pending_outbox(&state, Some(&account_id)).is_empty() {
        folders.push(Folder {
            path: outbox::OUTBOX.into(),
            name: "定时发送".into(),
            role: "outbox".into(),
        });
    }
    Ok(folders)
}

/// 在邮箱服务器创建文件夹并刷新本地文件夹列表。
#[tauri::command]
async fn create_folder(
    state: State<'_, AppState>,
    account_id: String,
    name: String,
) -> CmdResult<Folder> {
    if state.is_pop3(&account_id).map_err(err)? {
        return Err("POP3 只有收件箱，不支持创建文件夹".into());
    }
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
    unread_only: Option<bool>,
) -> CmdResult<Vec<Summary>> {
    let (flagged_only, unread_only) = (flagged_only == Some(true), unread_only == Some(true));
    if folder == drafts::LOCAL_DRAFTS {
        if before.is_some() || flagged_only || unread_only {
            return Ok(Vec::new());
        }
        return state.local_drafts.list(&account_id).map_err(err);
    }
    if folder == outbox::OUTBOX {
        if before.is_some() || flagged_only || unread_only {
            return Ok(Vec::new());
        }
        let (acct, _) = state.account(&account_id).map_err(err)?;
        let sent = state.outbox_sent.lock().unwrap().clone();
        return Ok(state
            .outbox
            .list(Some(&account_id))
            .map_err(err)?
            .iter()
            .filter(|i| !sent.contains(&(i.account_id.clone(), i.id)))
            .map(|i| i.summary(&acct.name, &acct.email))
            .collect());
    }
    if state.is_pop3(&account_id).map_err(err)? {
        if flagged_only {
            return Ok(Vec::new());
        }
        let mut list = pop3_list(&state, &account_id, before).await.map_err(err)?;
        if unread_only {
            list.retain(|m| !m.seen);
        }
        return Ok(list);
    }
    // 只看星标、只看未读都在服务器上搜索，不局限于已加载的一页
    let criteria = if flagged_only {
        "FLAGGED"
    } else if unread_only {
        "UNSEEN"
    } else {
        ""
    };
    state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                c.select(&folder).await?;
                let uids = imap::page_uids_matching(&mut c.s, criteria, before).await?;
                if uids.is_empty() {
                    return Ok(Vec::new());
                }
                imap::fetch_summaries(&mut c.s, &imap::uid_set(&uids), &acct.id, &folder).await
            })
        })
        .await
        .map_err(err)
}

fn quoted_search_term(value: &str) -> Result<String> {
    if value.chars().any(char::is_control) {
        bail!("搜索内容包含无效控制字符");
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// 在服务器端搜索当前文件夹，避免只在已加载的 100 封邮件中筛选。
#[tauri::command]
async fn search_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    query: String,
) -> CmdResult<Vec<Summary>> {
    let query = query.trim().to_string();
    if query.is_empty() {
        cancel_search(state.clone(), account_id.clone());
        return list_messages(state, account_id, folder, None, None, None).await;
    }
    if state.is_pop3(&account_id).map_err(err)? {
        // 先领取代次再排队等锁：排队期间清空关键词也能取消这次搜索。
        let generation = state.pop3_mailbox(&account_id).begin_search();
        let session = state.pop3_session(&account_id).await.map_err(err)?;
        let mut result = session.search(&query, generation).await.map_err(err)?;
        result.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.uid.cmp(&a.uid)));
        return Ok(result);
    }
    let term = quoted_search_term(&query).map_err(err)?;
    // 不声明字符集时，服务器不接受（或匹配不到）非 ASCII 的搜索词。
    let charset = if query.is_ascii() {
        ""
    } else {
        "CHARSET UTF-8 "
    };
    let set = format!("{charset}OR FROM {term} OR TO {term} OR SUBJECT {term} TEXT {term}");
    state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                c.select(&folder).await?;
                let mut uids: Vec<u32> = timeout(OP_TIMEOUT, c.s.uid_search(&set))
                    .await??
                    .into_iter()
                    .collect();
                uids.sort_unstable();
                if uids.is_empty() {
                    return Ok(Vec::new());
                }
                imap::fetch_summaries(&mut c.s, &imap::uid_set(&uids), &acct.id, &folder).await
            })
        })
        .await
        .map_err(err)
}

/// 停止该账号正在进行的 POP3 搜索（清空关键词或退出搜索时调用）。
#[tauri::command]
fn cancel_search(state: State<'_, AppState>, account_id: String) {
    if let Some(mailbox) = state.pop3.lock().unwrap().get(&account_id) {
        mailbox.cancel_search();
    }
}

#[tauri::command]
async fn get_previews(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uids: Vec<u32>,
) -> CmdResult<Vec<Preview>> {
    if uids.is_empty()
        || folder == outbox::OUTBOX
        || state.no_preview.lock().unwrap().contains(&account_id)
    {
        return Ok(Vec::new());
    }
    if state.is_pop3(&account_id).map_err(err)? {
        let mut session = state.pop3_session(&account_id).await.map_err(err)?;
        let result = session.previews(&uids).await.map_err(err)?;
        session.close().await;
        return Ok(result);
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
    let cached = { cache.lock().unwrap().get(&key) };
    if let Some((raw, date)) = cached {
        // BODY[] normally marks a message as read. Once the body is cached we
        // must preserve that side effect explicitly without downloading the
        // MIME payload again.
        if !peek {
            timeout(
                OP_TIMEOUT,
                checked_command(session, &format!("UID STORE {uid} +FLAGS.SILENT (\\Seen)")),
            )
            .await
            .context("标记邮件已读超时")??;
        }
        return Ok((raw, date, validity));
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
    if folder == drafts::LOCAL_DRAFTS {
        let raw = state.local_drafts.read(&account_id, uid).map_err(err)?;
        return imap::parse_message(&raw, 0).map_err(err);
    }
    if folder == outbox::OUTBOX {
        let raw = state.outbox.draft_raw(&account_id, uid).map_err(err)?;
        let mut view = imap::parse_message(&raw, 0).map_err(err)?;
        // 自己的邮件，不显示「请求回执」提示
        view.receipt_to = None;
        return Ok(view);
    }
    if state.is_pop3(&account_id).map_err(err)? {
        let (raw, date) = pop3_raw(&state, &account_id, uid, true)
            .await
            .map_err(err)?;
        let view = pop3::parse_message(&raw, date).map_err(err)?;
        state.record_sender(&account_id, "INBOX", uid, &view.from.name, &view.from.addr);
        return Ok(view);
    }
    let (raw, date, validity) = fetch_raw(&state, &account_id, &folder, uid, false, None)
        .await
        .map_err(err)?;
    let mut view = imap::parse_message(&raw, date).map_err(err)?;
    view.uid_validity = Some(validity);
    state.record_sender(&account_id, &folder, uid, &view.from.name, &view.from.addr);
    // 请求了已读回执的邮件：看看是不是已经回过（别的客户端也会打 $MDNSent 标记）
    if view.receipt_to.is_some() {
        let f = folder.clone();
        view.receipt_handled = state
            .with_cmd(&account_id, |c, _| {
                Box::pin(async move {
                    c.select(&f).await?;
                    imap::has_keyword(&mut c.s, uid, imap::MDN_SENT).await
                })
            })
            .await
            .unwrap_or(false);
    }
    Ok(view)
}

// ---------------------------------------------------------------------------
// 定时发送
// ---------------------------------------------------------------------------

const MINUTE_MS: i64 = 60 * 1000;

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn check_send_at(send_at: i64) -> Result<()> {
    let now = now_ms();
    if send_at < now + MINUTE_MS {
        bail!("定时发送的时间至少要在一分钟之后");
    }
    if send_at > now + 366 * 24 * 60 * MINUTE_MS {
        bail!("最多只能定在一年以内");
    }
    Ok(())
}

fn outbox_changed(app: &AppHandle, account_id: &str, id: u32, kind: &str, detail: &str) {
    let _ = app.emit(
        "outbox",
        serde_json::json!({ "accountId": account_id, "id": id, "state": kind, "detail": detail }),
    );
}

/// 定时发送：现在就把邮件组好存进「定时发送」，到点由后台发出。
#[tauri::command]
async fn schedule_mail(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: smtp::Draft,
    draft_refs: Option<Vec<drafts::DraftRef>>,
    send_at: i64,
) -> CmdResult<outbox::Item> {
    check_send_at(send_at).map_err(err)?;
    let (acct, _) = state.account(&draft.account_id).map_err(err)?;
    let quote = quote_raw(&state, &draft).await?;
    let attachments = resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(err)?;
    let quote = quote.as_deref().map(Vec::as_slice);
    let (msg, message_id) = smtp::compose(
        &acct,
        &draft,
        quote,
        attachments.clone(),
        smtp::Purpose::Send,
    )
    .map_err(err)?;
    let recipients: Vec<String> = msg.envelope().to().iter().map(|a| a.to_string()).collect();
    // 取消定时后转成草稿：草稿要保留密送
    let (draft_msg, _) =
        smtp::compose(&acct, &draft, quote, attachments, smtp::Purpose::Draft).map_err(err)?;
    let to = [&draft.to, &draft.cc]
        .into_iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(", ");
    let item = outbox::Item {
        account_id: draft.account_id.clone(),
        send_at,
        subject: draft.subject.clone(),
        to,
        recipients,
        message_id,
        ..Default::default()
    };
    let item = state
        .outbox
        .add(item, &msg.formatted(), &draft_msg.formatted())
        .map_err(err)?;
    let tokens: Vec<String> = draft
        .attachments
        .iter()
        .filter_map(|a| match a {
            smtp::AttachmentRef::File { token } => Some(token.clone()),
            smtp::AttachmentRef::Message { .. } => None,
        })
        .collect();
    state.attachments.forget(&tokens);
    // 邮件已经在「定时发送」里了，对应的草稿删掉
    let draft_refs = draft_refs.unwrap_or_default();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AppState>();
        for draft in draft_refs {
            if let Err(e) = remove_draft(&state, &draft).await {
                eprintln!("已定时发送，但删除草稿失败：{e:#}");
            }
        }
    });
    state.outbox_wake.notify_one();
    outbox_changed(&app, &item.account_id, item.id, "scheduled", "");
    Ok(item)
}

#[tauri::command]
fn outbox_list(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> CmdResult<Vec<outbox::Item>> {
    Ok(pending_outbox(&state, account_id.as_deref()))
}

/// 还没发出的定时邮件（发出但本地副本没删掉的不算）。
fn pending_outbox(state: &AppState, account_id: Option<&str>) -> Vec<outbox::Item> {
    let sent = state.outbox_sent.lock().unwrap().clone();
    state
        .outbox
        .list(account_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|i| !sent.contains(&(i.account_id.clone(), i.id)))
        .collect()
}

#[tauri::command]
async fn outbox_send_now(app: AppHandle, account_id: String, id: u32) -> CmdResult<()> {
    send_scheduled(&app, &account_id, id, true)
        .await
        .map_err(err)
}

#[tauri::command]
fn outbox_reschedule(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    id: u32,
    send_at: i64,
) -> CmdResult<outbox::Item> {
    check_send_at(send_at).map_err(err)?;
    let item = state
        .outbox
        .modify(&account_id, id, |i| {
            i.send_at = send_at;
            i.attempts = 0;
            i.tries = 0;
            i.next_try = 0;
            i.last_error = None;
        })
        .map_err(err)?;
    state.outbox_wake.notify_one();
    outbox_changed(&app, &account_id, id, "scheduled", "");
    Ok(item)
}

/// 取消定时：邮件转成本地草稿，返回草稿编号。
#[tauri::command]
async fn outbox_cancel(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    id: u32,
) -> CmdResult<drafts::DraftRef> {
    // 正在发送时等它结束，免得一边发出一边又转成草稿
    let _sending = state.outbox_send.lock().await;
    let raw = state.outbox.draft_raw(&account_id, id).map_err(err)?;
    let draft = store_draft(&state, &account_id, raw).await.map_err(err)?;
    state.outbox.remove(&account_id, id).map_err(err)?;
    outbox_changed(&app, &account_id, id, "cancelled", "");
    Ok(draft)
}

/// 把一份草稿原文存起来：先存服务器的草稿箱，存不了（POP3、离线）再存本地草稿。
async fn store_draft(state: &AppState, account_id: &str, raw: Vec<u8>) -> Result<drafts::DraftRef> {
    let message_id = imap::parse_message(&raw, 0)
        .ok()
        .and_then(|v| v.message_id)
        .map(|id| format!("<{}>", id.trim_matches(['<', '>'])))
        .unwrap_or_default();
    let server_folder = if state.is_pop3(account_id)? {
        None
    } else {
        state
            .folder_by_role(account_id, "drafts")
            .await
            .ok()
            .flatten()
    };
    if let Some(folder) = server_folder {
        let (r, f, m, a) = (
            raw.clone(),
            folder,
            message_id.clone(),
            account_id.to_string(),
        );
        let saved = state
            .with_cmd(account_id, |c, _| {
                Box::pin(async move {
                    c.selected = None;
                    let saved = drafts::save_on_server(&mut c.s, &a, &f, &r, &m, None).await?;
                    c.selected = Some(f);
                    Ok(saved)
                })
            })
            .await;
        match saved {
            Ok(r) => return Ok(r),
            Err(e) => eprintln!("草稿未能存到服务器，改存本地：{e:#}"),
        }
    }
    let uid = state.local_drafts.save(account_id, &raw, None)?;
    Ok(drafts::DraftRef {
        account_id: account_id.to_string(),
        folder: drafts::LOCAL_DRAFTS.into(),
        uid: Some(uid),
        uid_validity: None,
        message_id,
        superseded: Vec::new(),
    })
}

/// 发出一封定时邮件；失败时记下原因，稍后自动重试。
/// `force`（立即发送）不管计划时间；后台发送时要再确认一遍还到点了（可能刚改了时间）。
async fn send_scheduled(app: &AppHandle, account_id: &str, id: u32, force: bool) -> Result<()> {
    let state = app.state::<AppState>();
    let _sending = state.outbox_send.lock().await;
    // 等锁期间可能已被取消、发出或改了时间
    let Ok(item) = state.outbox.get(account_id, id) else {
        return Ok(());
    };
    if state
        .outbox_sent
        .lock()
        .unwrap()
        .contains(&(account_id.to_string(), id))
    {
        return Ok(());
    }
    if !force && !item.due(now_ms()) {
        return Ok(());
    }
    // 先记下「正在发送」：发出去后、删掉之前程序被关，重启时不会自动再发一遍
    let item = state.outbox.modify(account_id, id, |i| i.sending = true)?;
    let sent = async {
        let (acct, pass) = state.account(account_id)?;
        let raw = state.outbox.raw(account_id, id)?;
        // 邮件是提前组好的，Date 换成真正发出的时间
        let raw = smtp::with_date(&raw, &chrono::Local::now());
        smtp::send_raw(&acct, &pass, &item.recipients, &raw).await?;
        Ok::<_, anyhow::Error>(raw)
    }
    .await;
    match sent {
        Ok(raw) => {
            if let Err(e) = state.outbox.remove(account_id, id) {
                // 删不掉也不能再发：记在内存里，界面上照样当作已发出
                eprintln!("定时邮件已发出，但删除本地副本失败：{e:#}");
                state
                    .outbox_sent
                    .lock()
                    .unwrap()
                    .insert((account_id.to_string(), id));
            }
            let recipients: Vec<(String, String)> = item
                .recipients
                .iter()
                .map(|r| (String::new(), r.clone()))
                .collect();
            if let Err(e) = state.db().and_then(|db| db.record_sent(&recipients)) {
                eprintln!("记录最近联系人失败：{e:#}");
            }
            outbox_changed(app, account_id, id, "sent", &item.subject);
            let handle = app.clone();
            let (account, message_id) = (account_id.to_string(), item.message_id.clone());
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(4)).await;
                let state = handle.state::<AppState>();
                if let Err(e) = save_to_sent(&state, &account, &message_id, raw).await {
                    eprintln!("保存到已发送失败：{e:#}");
                }
            });
            Ok(())
        }
        Err(e) => {
            let detail = format!("{e:#}");
            // 在锁里读最新的再改：发送期间用户可能改了时间，不能把旧时间写回去
            let refused = smtp::rejected(&e);
            let _ = state
                .outbox
                .modify(account_id, id, |i| i.failed(&detail, now_ms(), refused));
            outbox_changed(app, account_id, id, "failed", &detail);
            Err(e)
        }
    }
}

/// 后台：每 15 秒（或被叫醒时）发出到点的定时邮件。启动时补发关闭期间到点的。
async fn outbox_worker(app: AppHandle) {
    match app.state::<AppState>().outbox.recover_interrupted() {
        Ok(0) => {}
        Ok(n) => eprintln!("{n} 封定时邮件上次发送时被中断，等待用户确认"),
        Err(e) => eprintln!("检查定时邮件失败：{e:#}"),
    }
    loop {
        let due = app
            .state::<AppState>()
            .outbox
            .due(now_ms())
            .unwrap_or_default();
        for item in due {
            let _ = send_scheduled(&app, &item.account_id, item.id, false).await;
        }
        let state = app.state::<AppState>();
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(15)) => {}
            _ = state.outbox_wake.notified() => {}
        }
    }
}

// ---------------------------------------------------------------------------
// 已读回执
// ---------------------------------------------------------------------------

/// 用户点了「发送回执」：按 RFC 8098 发一封已读回执给对方。从不自动发送。
#[tauri::command]
async fn send_receipt(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<()> {
    let (acct, pass) = state.account(&account_id).map_err(err)?;
    let view = if state.is_pop3(&account_id).map_err(err)? {
        let (raw, date) = pop3_raw(&state, &account_id, uid, false)
            .await
            .map_err(err)?;
        pop3::parse_message(&raw, date).map_err(err)?
    } else {
        let (raw, date, _) = fetch_raw(&state, &account_id, &folder, uid, true, None)
            .await
            .map_err(err)?;
        imap::parse_message(&raw, date).map_err(err)?
    };
    let (to, raw) = smtp::receipt(&acct, &view).map_err(err)?;
    smtp::send_raw(&acct, &pass, &[to], &raw)
        .await
        .map_err(err)?;
    mark_receipt_handled(&state, &account_id, &folder, uid).await;
    Ok(())
}

/// 用户选择不发回执：同样打上 $MDNSent，别的客户端也不会再问。
#[tauri::command]
async fn ignore_receipt(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<()> {
    mark_receipt_handled(&state, &account_id, &folder, uid).await;
    Ok(())
}

/// 尽力给邮件打上 $MDNSent；服务器不允许自定义标记时界面自己记住。
async fn mark_receipt_handled(state: &AppState, account_id: &str, folder: &str, uid: u32) {
    if folder == drafts::LOCAL_DRAFTS || state.is_pop3(account_id).unwrap_or(true) {
        return;
    }
    let folder = folder.to_string();
    let marked = state
        .with_cmd(account_id, |c, _| {
            Box::pin(async move {
                c.select(&folder).await?;
                imap::add_keyword(&mut c.s, uid, imap::MDN_SENT).await
            })
        })
        .await;
    if let Err(e) = marked {
        eprintln!("没能给邮件打上 $MDNSent 标记：{e:#}");
    }
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
    let raw = raw_at(&state, &account_id, &folder, uid, None)
        .await
        .map_err(err)?;
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
    // 本地草稿没有服务器状态；它的编号也可能和 POP3 邮件的编号相同，不能落到下面的分支。
    if folder == drafts::LOCAL_DRAFTS || folder == outbox::OUTBOX {
        return Ok(());
    }
    if state.is_pop3(&account_id).map_err(err)? {
        if flag != "seen" {
            return Err("POP3 不支持星标".into());
        }
        // POP3 没有服务器端已读状态，只记在本地，供未读数和列表使用。
        state.pop3_mailbox(&account_id).set_read(&uids, value).await;
        return Ok(());
    }
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
    if folder == drafts::LOCAL_DRAFTS {
        return Err("本地草稿只能继续编辑或删除".into());
    }
    if folder == outbox::OUTBOX {
        return Err("定时发送的邮件请用「取消定时」或「立即发送」".into());
    }
    if state.is_pop3(&account_id).map_err(err)? {
        return Err("POP3 只有收件箱，不支持移动或删除邮件".into());
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
        refresh_live_inbox(&state, &account_id).await;
    }
    Ok(())
}

/// 收件箱在命令连接上被改动后，重新读一遍首屏，供界面重新打开时使用。
async fn refresh_live_inbox(state: &AppState, account_id: &str) {
    {
        let refreshed = state
            .with_cmd(account_id, |c, acct| {
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
                let live = live.entry(account_id.to_string()).or_default();
                live.inbox = inbox;
                live.unread = unread;
            }
            Err(error) => {
                eprintln!("移动成功后刷新收件箱失败：{error:#}");
                let _ = state.wake.send(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

/// 界面列表里已有的会话信息。
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadHint {
    message_id: String,
    #[serde(default)]
    in_reply_to: String,
    #[serde(default)]
    references: Vec<String>,
}

/// 和这封邮件同一会话的所有邮件（当前文件夹、收件箱、已发送、归档），按时间从早到晚。
/// POP3 只有收件箱，返回空，由界面按已加载的列表分组。
#[tauri::command]
async fn thread_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    hint: Option<ThreadHint>,
) -> CmdResult<Vec<Summary>> {
    if folder == drafts::LOCAL_DRAFTS
        || folder == outbox::OUTBOX
        || state.is_pop3(&account_id).map_err(err)?
        || state
            .no_thread_search
            .lock()
            .unwrap()
            .get(&account_id)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(3600))
    {
        return Ok(Vec::new());
    }
    let known = state.folders(&account_id).await.map_err(err)?;
    let mut folders = vec![folder.clone(), "INBOX".to_string()];
    for role in ["sent", "archive"] {
        if let Some(f) = known.iter().find(|f| f.role == role) {
            folders.push(f.path.clone());
        }
    }
    let mut unique: Vec<String> = Vec::new();
    for f in folders {
        if !unique.iter().any(|u| imap::mailbox_eq(u, &f)) {
            unique.push(f);
        }
    }
    let hint = hint.filter(|h| !h.message_id.is_empty());
    let (mails, refused) = state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                // 界面已经有这封的会话信息时不用再读一遍
                let open = match hint {
                    Some(h) => Summary {
                        account_id: acct.id.clone(),
                        folder: folder.clone(),
                        uid,
                        message_id: h.message_id,
                        in_reply_to: h.in_reply_to,
                        references: h.references,
                        ..Default::default()
                    },
                    None => {
                        c.select(&folder).await?;
                        imap::fetch_summaries(&mut c.s, &uid.to_string(), &acct.id, &folder)
                            .await?
                            .into_iter()
                            .find(|m| m.uid == uid)
                            .ok_or_else(|| anyhow!("邮件已不存在，请刷新后再试"))?
                    }
                };
                // collect 会 EXAMINE 别的文件夹
                c.selected = None;
                let found = threads::collect(&mut c.s, &acct.id, &unique, &open).await?;
                Ok((found.mails, found.search_refused))
            })
        })
        .await
        .map_err(err)?;
    if refused {
        // 也可能只是服务器一时忙；一小时后再试
        state
            .no_thread_search
            .lock()
            .unwrap()
            .insert(account_id, Instant::now());
    }
    Ok(mails)
}

// ---------------------------------------------------------------------------
// 邮件模板
// ---------------------------------------------------------------------------

#[tauri::command]
fn templates_list(state: State<'_, AppState>) -> CmdResult<Vec<templates::Template>> {
    state.db().map_err(err)?.load_templates().map_err(err)
}

/// 保存整套模板，返回规范化后的结果。
#[tauri::command]
fn templates_save(
    state: State<'_, AppState>,
    templates: Vec<templates::Template>,
) -> CmdResult<Vec<templates::Template>> {
    let list = templates::validated(templates).map_err(err)?;
    state
        .db()
        .map_err(err)?
        .save_templates(&list)
        .map_err(err)?;
    Ok(list)
}

// ---------------------------------------------------------------------------
// 收信规则
// ---------------------------------------------------------------------------

#[tauri::command]
fn get_rules(state: State<'_, AppState>) -> Vec<rules::Rule> {
    state.rules.lock().unwrap().clone()
}

#[tauri::command]
fn save_rules(state: State<'_, AppState>, rules: Vec<rules::Rule>) -> CmdResult<Vec<rules::Rule>> {
    let rules = rules::validated(rules).map_err(err)?;
    let known: Vec<String> = state
        .store
        .lock()
        .unwrap()
        .accounts
        .iter()
        .map(|a| a.id.clone())
        .collect();
    if let Some(r) = rules.iter().find(|r| !known.contains(&r.account_id)) {
        return Err(format!("「{}」对应的邮箱账号已不存在", r.name));
    }
    if let Some(r) = rules
        .iter()
        .find(|r| state.is_pop3(&r.account_id).unwrap_or(false))
    {
        return Err(format!(
            "「{}」：POP3 账号只有收件箱，不支持收信规则",
            r.name
        ));
    }
    {
        let stored = state.rules.lock().unwrap().clone();
        let folders = state.folders.lock().unwrap();
        for r in rules::targets_to_check(&stored, &rules) {
            rules::check_target(r, folders.get(&r.account_id).map(Vec::as_slice)).map_err(err)?;
        }
    }
    state.db().map_err(err)?.save_rules(&rules).map_err(err)?;
    *state.rules.lock().unwrap() = rules.clone();
    Ok(rules)
}

/// 「立即执行」最多处理收件箱最近这么多封邮件。
const RUN_RULES_LIMIT: usize = 500;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RulesRun {
    /// 命中规则的邮件数
    matched: usize,
    moved: usize,
    errors: Vec<String>,
}

/// 对收件箱最近的邮件立即执行一遍收信规则。
#[tauri::command]
async fn run_rules(state: State<'_, AppState>, account_id: String) -> CmdResult<RulesRun> {
    if state.is_pop3(&account_id).map_err(err)? {
        return Err("POP3 账号只有收件箱，不支持收信规则".into());
    }
    let rules = state.rules.lock().unwrap().clone();
    if !rules
        .iter()
        .any(|r| r.enabled && r.account_id == account_id)
    {
        return Err("这个账号还没有启用的规则".into());
    }
    let report = state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                c.select("INBOX").await?;
                let found = timeout(
                    OP_TIMEOUT * 2,
                    download_protocol::checked_command(&mut c.s, "UID SEARCH UNDELETED"),
                )
                .await
                .context("读取收件箱超时")??;
                let mut uids: Vec<u32> = found
                    .into_iter()
                    .flat_map(|r| match r {
                        download_protocol::ArchiveData::Uids(uids) => uids,
                        _ => Vec::new(),
                    })
                    .collect();
                uids.sort_unstable();
                let uids = uids.split_off(uids.len().saturating_sub(RUN_RULES_LIMIT));
                let mut mails = Vec::new();
                for chunk in uids.chunks(imap::PAGE) {
                    mails.extend(
                        imap::fetch_summaries(&mut c.s, &imap::uid_set(chunk), &acct.id, "INBOX")
                            .await?,
                    );
                }
                let batches = rules::batches(&rules, &mails);
                let matched = batches.iter().map(|(_, uids)| uids.len()).sum();
                // 超时或断线时报错，with_cmd 会丢掉这条连接
                let done = rules::execute(&mut c.s, &batches).await?;
                let moved: Vec<Summary> = mails
                    .into_iter()
                    .filter(|m| done.moved.contains(&m.uid))
                    .collect();
                Ok((
                    RulesRun {
                        matched,
                        moved: done.moved.len(),
                        errors: done.errors,
                    },
                    moved,
                ))
            })
        })
        .await
        .map_err(err)?;
    let (report, moved) = report;
    // 用户再把它们移回收件箱时，同步连接上的规则不再动它们
    state.rule_memory(&account_id, |m| m.moved(&moved));
    if report.matched > 0 {
        state
            .bodies
            .lock()
            .unwrap()
            .retain(|((account, folder, _, _), _)| {
                account != &account_id || !imap::mailbox_eq(folder, "INBOX")
            });
        refresh_live_inbox(&state, &account_id).await;
    }
    Ok(report)
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
    let raw = raw_at(&state, &account_id, &folder, uid, uid_validity)
        .await
        .map_err(err)?;
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
    if !supported_link(&url) {
        return Err("不支持打开这种链接".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

fn supported_link(url: &str) -> bool {
    if url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return false;
    }
    let Ok(u) = tauri::Url::parse(url) else {
        return false;
    };
    match u.scheme() {
        // tauri.localhost 是 Windows 上应用自己的页面，邮件里的相对链接会解析到这里
        "http" | "https" => u
            .host_str()
            .is_some_and(|h| h.trim_end_matches('.') != "tauri.localhost"),
        "mailto" => !u.path().is_empty() || u.query().is_some(),
        "tel" => u.path().chars().any(|c| c.is_ascii_digit()),
        _ => false,
    }
}

/// 读取任意位置的邮件原文（不改变已读状态）：本地草稿、POP3 或 IMAP。
async fn raw_at(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    uid_validity: Option<u32>,
) -> Result<Arc<Vec<u8>>> {
    if folder == drafts::LOCAL_DRAFTS {
        return Ok(Arc::new(state.local_drafts.read(account_id, uid)?));
    }
    if folder == outbox::OUTBOX {
        return Ok(Arc::new(state.outbox.draft_raw(account_id, uid)?));
    }
    if state.is_pop3(account_id)? {
        return Ok(pop3_raw(state, account_id, uid, false).await?.0);
    }
    Ok(
        fetch_raw(state, account_id, folder, uid, true, uid_validity)
            .await?
            .0,
    )
}

/// 回复时引用的原邮件原文，用来复制引用中的内嵌图片。
async fn quote_raw(state: &AppState, draft: &smtp::Draft) -> CmdResult<Option<Arc<Vec<u8>>>> {
    if draft.html_body.is_none() || draft.quote_cids.is_empty() {
        return Ok(None);
    }
    let source = draft
        .quote_source
        .as_ref()
        .ok_or("缺少引用邮件，无法读取内嵌图片")?;
    if source.uid == 0 || source.folder.is_empty() {
        return Err("引用邮件的位置无效，请重新打开原邮件后回复".into());
    }
    // 引用来源与当前发件账号可以不同，但必须仍是已配置、可访问的账号。
    state.account(&source.account_id).map_err(err)?;
    // POP3 的 id 本身就固定对应一封邮件，不需要 UIDVALIDITY；IMAP 必须确认还是同一封。
    let validity = if state.is_pop3(&source.account_id).map_err(err)? {
        None
    } else {
        Some(
            source
                .uid_validity
                .filter(|value| *value > 0)
                .ok_or("无法确认引用邮件的身份，请重新打开原邮件后回复")?,
        )
    };
    raw_at(
        state,
        &source.account_id,
        &source.folder,
        source.uid,
        validity,
    )
    .await
    .map(Some)
    .map_err(|e| format!("读取引用邮件的内嵌图片失败：{e:#}"))
}

/// 读出草稿里引用的所有附件。同一封邮件的多个附件只下载一次。
async fn resolve_attachments(
    state: &AppState,
    refs: &[smtp::AttachmentRef],
) -> Result<Vec<smtp::OutgoingAttachment>> {
    let mut out = Vec::with_capacity(refs.len());
    let mut sources: HashMap<(String, String, u32), Arc<Vec<u8>>> = HashMap::new();
    let mut total = 0usize;
    for r in refs {
        let file = match r {
            smtp::AttachmentRef::File { token } => state.attachments.read(token)?,
            smtp::AttachmentRef::Message {
                account_id,
                folder,
                uid,
                uid_validity,
                index,
            } => {
                state.account(account_id)?;
                let needs_validity =
                    folder != drafts::LOCAL_DRAFTS && !state.is_pop3(account_id)?;
                if needs_validity && uid_validity.is_none_or(|v| v == 0) {
                    bail!("无法确认附件所在邮件的身份，请重新打开原邮件");
                }
                let key = (account_id.clone(), folder.clone(), *uid);
                let raw = match sources.get(&key) {
                    Some(raw) => raw.clone(),
                    None => {
                        let raw = raw_at(state, account_id, folder, *uid, *uid_validity).await?;
                        sources.insert(key, raw.clone());
                        raw
                    }
                };
                imap::attachment_part(&raw, *index)?
            }
        };
        total += file.data.len();
        if total > smtp::MAX_ATTACHMENT_BYTES {
            bail!(
                "附件总大小超过 {} MB 上限",
                smtp::MAX_ATTACHMENT_BYTES / 1024 / 1024
            );
        }
        out.push(file);
    }
    Ok(out)
}

/// 删除一份草稿。只能删本地草稿或该账号草稿箱里的邮件。
async fn remove_one_draft(state: &AppState, draft: &drafts::DraftRef) -> Result<()> {
    if draft.folder == drafts::LOCAL_DRAFTS {
        if let Some(uid) = draft.uid {
            state.local_drafts.delete(&draft.account_id, uid)?;
        }
        return Ok(());
    }
    let drafts_folder = state.folder_by_role(&draft.account_id, "drafts").await?;
    if !drafts::draft_folder_allowed(&draft.folder, drafts_folder.as_deref()) {
        bail!("只能删除草稿箱里的草稿");
    }
    let target = draft.clone();
    state
        .with_cmd(&draft.account_id, |c, _| {
            Box::pin(async move {
                c.selected = None;
                drafts::delete_on_server(&mut c.s, &target).await?;
                c.selected = Some(target.folder.clone());
                Ok(())
            })
        })
        .await
}

/// 删除草稿（发送成功或用户放弃时），连同它记着的、还留在服务器上的旧版本。
async fn remove_draft(state: &AppState, draft: &drafts::DraftRef) -> Result<()> {
    let result = remove_one_draft(state, draft).await;
    for old in draft
        .superseded
        .iter()
        .flat_map(|r| drafts::server_versions_to_delete(Some(r)))
    {
        if let Err(e) = remove_one_draft(state, &old).await {
            eprintln!("删除草稿旧版本失败：{e:#}");
        }
    }
    result
}

/// 重新打开草稿时，把它的附件取出来暂存为令牌：之后的自动保存会替换掉这份草稿，
/// 附件不能再指向它。
#[tauri::command]
async fn draft_attachments(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    uid_validity: Option<u32>,
    indices: Vec<usize>,
) -> CmdResult<Vec<drafts::PickedFile>> {
    let raw = raw_at(&state, &account_id, &folder, uid, uid_validity)
        .await
        .map_err(err)?;
    // indices 来自邮件视图列出的附件；被正文引用的内嵌图片不在其中。
    indices
        .into_iter()
        .map(|index| {
            let part = imap::attachment_part(&raw, index).map_err(err)?;
            Ok(state
                .attachments
                .register_bytes(part.name, part.content_type, part.data))
        })
        .collect()
}

/// 清理放进编辑器的 HTML，只保留常见排版。粘贴时保留远程图片（用户自己选择粘贴的）；
/// 重新打开草稿时 `block_remote`，避免一打开就加载追踪像素。
#[tauri::command]
fn clean_html(html: String, block_remote: Option<bool>) -> String {
    if block_remote == Some(true) {
        html_mail::clean_blocking_remote(&html)
    } else {
        html_mail::clean(&html)
    }
}

/// 选一张本地图片插入正文，返回 data URL；取消时为空。
#[tauri::command]
async fn pick_inline_image() -> CmdResult<Option<String>> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("插入图片")
        .add_filter(
            "图片",
            &["png", "jpg", "jpeg", "gif", "webp", "bmp", "heic"],
        )
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let name = file.file_name();
    let path = file.path().to_path_buf();
    let size = std::fs::metadata(&path).map_err(|e| e.to_string())?.len();
    if size > html_mail::MAX_INLINE_IMAGE_BYTES as u64 {
        return Err(format!(
            "图片超过 {} MB，请作为附件发送",
            html_mail::MAX_INLINE_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let data = std::fs::read(&path).map_err(|e| format!("无法读取图片：{e}"))?;
    html_mail::image_data_url(&name, &data)
        .map(Some)
        .map_err(err)
}

// ---------------------------------------------------------------------------
// 通讯录
// ---------------------------------------------------------------------------

#[tauri::command]
fn contacts_suggest(
    state: State<'_, AppState>,
    query: String,
) -> CmdResult<Vec<contacts::Suggestion>> {
    state.db().map_err(err)?.suggest(&query, 8).map_err(err)
}

#[tauri::command]
fn contacts_list(
    state: State<'_, AppState>,
    query: Option<String>,
) -> CmdResult<Vec<contacts::Contact>> {
    state
        .db()
        .map_err(err)?
        .list_contacts(query.as_deref().unwrap_or(""))
        .map_err(err)
}

#[tauri::command]
fn contact_save(state: State<'_, AppState>, contact: contacts::ContactInput) -> CmdResult<i64> {
    state.db().map_err(err)?.save_contact(&contact).map_err(err)
}

#[tauri::command]
fn contact_delete(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.db().map_err(err)?.delete_contact(id).map_err(err)
}

/// 从联想里去掉一个最近联系人。
#[tauri::command]
fn contacts_forget(state: State<'_, AppState>, email: String) -> CmdResult<()> {
    state.db().map_err(err)?.forget_recent(&email).map_err(err)
}

#[tauri::command]
fn groups_list(state: State<'_, AppState>) -> CmdResult<Vec<contacts::Group>> {
    state.db().map_err(err)?.list_groups().map_err(err)
}

#[tauri::command]
fn group_save(state: State<'_, AppState>, id: Option<i64>, name: String) -> CmdResult<i64> {
    state.db().map_err(err)?.save_group(id, &name).map_err(err)
}

#[tauri::command]
fn group_delete(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.db().map_err(err)?.delete_group(id).map_err(err)
}

#[tauri::command]
fn group_members_set(state: State<'_, AppState>, id: i64, members: Vec<i64>) -> CmdResult<()> {
    state
        .db()
        .map_err(err)?
        .set_group_members(id, &members)
        .map_err(err)
}

/// 选择 vCard（.vcf）或 CSV 文件导入；取消时返回空。
#[tauri::command]
async fn contacts_import(state: State<'_, AppState>) -> CmdResult<Option<contacts::ImportResult>> {
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("导入通讯录")
        .add_filter("通讯录", &["vcf", "vcard", "csv"])
        .pick_file()
        .await
    else {
        return Ok(None);
    };
    let size = std::fs::metadata(file.path())
        .map_err(|e| format!("无法读取文件：{e}"))?
        .len();
    if size > 20 * 1024 * 1024 {
        return Err("文件太大（超过 20 MB），不像是通讯录".into());
    }
    let bytes = std::fs::read(file.path()).map_err(|e| format!("无法读取文件：{e}"))?;
    let name = file.file_name().to_ascii_lowercase();
    let text_start = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_uppercase();
    let parsed = if name.ends_with(".csv") && !text_start.contains("BEGIN:VCARD") {
        contacts::parse_csv(&bytes)
    } else {
        contacts::parse_vcard(&String::from_utf8_lossy(&bytes))
    };
    if parsed.is_empty() {
        return Err("文件里没有找到带邮箱的联系人".into());
    }
    state
        .db()
        .map_err(err)?
        .import(&parsed)
        .map(Some)
        .map_err(err)
}

/// 导出全部联系人为 vCard；取消时返回空，否则返回保存路径。
#[tauri::command]
async fn contacts_export(state: State<'_, AppState>) -> CmdResult<Option<String>> {
    let text = contacts::to_vcard(&state.db().map_err(err)?.export().map_err(err)?);
    let Some(file) = rfd::AsyncFileDialog::new()
        .set_title("导出通讯录")
        .set_file_name("MailBeacon 通讯录.vcf")
        .save_file()
        .await
    else {
        return Ok(None);
    };
    std::fs::write(file.path(), text).map_err(|e| format!("保存失败：{e}"))?;
    Ok(Some(file.path().display().to_string()))
}

/// 写信窗口关闭时释放它登记的附件令牌。
#[tauri::command]
fn release_attachments(state: State<'_, AppState>, tokens: Vec<String>) {
    state.attachments.forget(&tokens);
}

/// 弹出文件选择框，登记选中的文件，返回令牌、文件名和大小。
#[tauri::command]
async fn pick_attachments(state: State<'_, AppState>) -> CmdResult<Vec<drafts::PickedFile>> {
    let Some(files) = rfd::AsyncFileDialog::new()
        .set_title("添加附件")
        .pick_files()
        .await
    else {
        return Ok(Vec::new());
    };
    files
        .iter()
        .map(|f| state.attachments.register(f.path()).map_err(err))
        .collect()
}

/// 保存草稿：IMAP 存进服务器草稿箱，POP3、没有草稿箱或保存失败（如离线）时存本地。
/// 返回新版本的位置；`previous` 是上一次保存的位置，新版本存好后删除它。
#[tauri::command]
async fn save_draft(
    state: State<'_, AppState>,
    draft: smtp::Draft,
    previous: Option<drafts::DraftRef>,
) -> CmdResult<drafts::DraftRef> {
    let (acct, _) = state.account(&draft.account_id).map_err(err)?;
    let quote = quote_raw(&state, &draft).await?;
    let attachments = resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(err)?;
    let (msg, message_id) = smtp::compose(
        &acct,
        &draft,
        quote.as_deref().map(Vec::as_slice),
        attachments,
        smtp::Purpose::Draft,
    )
    .map_err(err)?;
    let raw = msg.formatted();
    // 切换了发件账号：旧账号里的上一版在新版本存好后删掉。
    let (same_account, other_account) = match previous {
        Some(p) if p.account_id == acct.id => (Some(p), None),
        Some(p) => (None, Some(p)),
        None => (None, None),
    };
    let server_folder = if state.is_pop3(&acct.id).map_err(err)? {
        None
    } else {
        state
            .folder_by_role(&acct.id, "drafts")
            .await
            .ok()
            .flatten()
    };
    let mut saved = None;
    if let Some(folder) = server_folder {
        let (raw, folder, message_id, account_id) =
            (raw.clone(), folder, message_id.clone(), acct.id.clone());
        let previous = same_account.clone();
        match state
            .with_cmd(&acct.id, |c, _| {
                Box::pin(async move {
                    c.selected = None;
                    let r = drafts::save_on_server(
                        &mut c.s,
                        &account_id,
                        &folder,
                        &raw,
                        &message_id,
                        previous.as_ref(),
                    )
                    .await?;
                    c.selected = Some(folder);
                    Ok(r)
                })
            })
            .await
        {
            Ok(r) => saved = Some(r),
            Err(e) => eprintln!("草稿未能存到服务器，改存本地：{e:#}"),
        }
    }
    let saved = match saved {
        Some(r) => {
            // 之前存在本地的版本已经有了服务器副本
            if let Some(local) = same_account
                .as_ref()
                .filter(|p| p.folder == drafts::LOCAL_DRAFTS)
                .and_then(|p| p.uid)
            {
                let _ = state.local_drafts.delete(&acct.id, local);
            }
            r
        }
        None => {
            let replace = same_account
                .as_ref()
                .filter(|p| p.folder == drafts::LOCAL_DRAFTS)
                .and_then(|p| p.uid);
            let id = state
                .local_drafts
                .save(&acct.id, &raw, replace)
                .map_err(err)?;
            drafts::DraftRef {
                account_id: acct.id.clone(),
                folder: drafts::LOCAL_DRAFTS.into(),
                uid: Some(id),
                uid_validity: None,
                message_id,
                // 服务器上的旧版本先留着，回到在线、成功存到服务器时再删
                superseded: drafts::superseded_by_local(same_account.as_ref()),
            }
        }
    };
    if let Some(old) = other_account {
        if let Err(e) = remove_draft(&state, &old).await {
            eprintln!("删除原发件账号里的旧草稿失败：{e:#}");
        }
    }
    Ok(saved)
}

#[tauri::command]
async fn delete_draft(state: State<'_, AppState>, draft: drafts::DraftRef) -> CmdResult<()> {
    remove_draft(&state, &draft).await.map_err(err)
}

#[tauri::command]
async fn send_mail(
    app: AppHandle,
    state: State<'_, AppState>,
    draft: smtp::Draft,
    draft_refs: Option<Vec<drafts::DraftRef>>,
) -> CmdResult<()> {
    let (acct, pass) = state.account(&draft.account_id).map_err(err)?;
    let quote = quote_raw(&state, &draft).await?;
    let attachments = resolve_attachments(&state, &draft.attachments)
        .await
        .map_err(err)?;
    let (msg, message_id) = smtp::compose(
        &acct,
        &draft,
        quote.as_deref().map(Vec::as_slice),
        attachments,
        smtp::Purpose::Send,
    )
    .map_err(err)?;
    let raw = msg.formatted();
    smtp::send(&acct, &pass, msg).await.map_err(err)?;
    // 收件人记为最近联系人，写信时优先联想
    if let Err(e) = smtp::recipients(&draft).and_then(|r| state.db()?.record_sent(&r)) {
        eprintln!("记录最近联系人失败：{e:#}");
    }
    let tokens: Vec<String> = draft
        .attachments
        .iter()
        .filter_map(|a| match a {
            smtp::AttachmentRef::File { token } => Some(token.clone()),
            smtp::AttachmentRef::Message { .. } => None,
        })
        .collect();
    state.attachments.forget(&tokens);
    let draft_refs = draft_refs.unwrap_or_default();

    // 后台：删除对应草稿；确认「已发送」里有这封，没有再补存，界面不用等
    let id = acct.id.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        for draft in draft_refs {
            if let Err(e) = remove_draft(&state, &draft).await {
                eprintln!("邮件已发送，但删除草稿失败：{e:#}");
            }
        }
        tokio::time::sleep(Duration::from_secs(4)).await;
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

/// Dock 图标上的未读数，同时更新托盘图标的提示文字。
/// Dock 角标只在 macOS 上设置：Linux 上这个调用走 D-Bus，没有会话总线时会卡住主线程。
#[tauri::command]
fn set_badge(app: AppHandle, count: i64) {
    #[cfg(target_os = "macos")]
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_badge_count(if count > 0 { Some(count) } else { None });
    }
    if let Some(tray) = app.tray_by_id(TRAY_ID) {
        let tip = if count > 0 {
            format!("MailBeacon · {count} 封未读")
        } else {
            "MailBeacon".into()
        };
        let _ = tray.set_tooltip(Some(tip));
    }
}

// ---------------------------------------------------------------------------
// 偏好：通知、托盘、开机启动
// ---------------------------------------------------------------------------

const TRAY_ID: &str = "main";

#[tauri::command]
fn get_prefs(app: AppHandle, state: State<'_, AppState>) -> prefs::Prefs {
    use tauri_plugin_autostart::ManagerExt;
    let mut prefs = state.prefs.lock().unwrap().clone();
    // 用户可能在系统设置里删掉了登录项，以系统为准
    if let Ok(enabled) = app.autolaunch().is_enabled() {
        prefs.autostart = enabled;
    }
    prefs
}

#[tauri::command]
fn save_prefs(
    app: AppHandle,
    state: State<'_, AppState>,
    prefs: prefs::Prefs,
) -> CmdResult<prefs::Prefs> {
    use tauri_plugin_autostart::ManagerExt;
    let prefs = prefs.validated().map_err(err)?;
    let db = state.db().map_err(|e| format!("设置无法保存：{e:#}"))?;
    let launcher = app.autolaunch();
    let was_enabled = launcher.is_enabled().unwrap_or(!prefs.autostart);
    let toggle = |on: bool| {
        if on {
            launcher.enable()
        } else {
            launcher.disable()
        }
    };
    let had_tray = app.tray_by_id(TRAY_ID).is_some();
    // 先把系统里的状态改好，全部成功才保存，失败就恢复原样
    if was_enabled != prefs.autostart {
        toggle(prefs.autostart).map_err(|e| format!("无法更改开机启动：{e}"))?;
    }
    let saved = apply_tray(&app, prefs.tray)
        .map_err(|e| format!("托盘图标设置失败：{e:#}"))
        .and_then(|()| db.save_prefs(&prefs).map_err(err));
    if let Err(e) = saved {
        if was_enabled != prefs.autostart {
            let _ = toggle(was_enabled);
        }
        let _ = apply_tray(&app, had_tray);
        return Err(e);
    }
    *state.prefs.lock().unwrap() = prefs.clone();
    Ok(prefs)
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 按设置显示或去掉托盘图标（macOS 上是菜单栏图标）。
///
/// Linux 上托盘靠 libayatana-appindicator，系统没装时加载库会直接 panic，
/// 这里兜住，当作「没有托盘」处理。
fn apply_tray(app: &AppHandle, enabled: bool) -> Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_tray(app, enabled)))
        .map_err(|_| anyhow!("系统不支持托盘图标（Linux 需要安装 libayatana-appindicator）"))?
        .map_err(Into::into)
}

fn build_tray(app: &AppHandle, enabled: bool) -> tauri::Result<()> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    if !enabled {
        app.remove_tray_by_id(TRAY_ID);
        return Ok(());
    }
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    let menu = MenuBuilder::new(app)
        .text("tray-show", "打开 MailBeacon")
        .text("tray-compose", "写邮件")
        .separator()
        .text("tray-quit", "退出 MailBeacon")
        .build()?;
    let mut tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .tooltip("MailBeacon")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "tray-show" => show_main_window(app),
            "tray-compose" => {
                show_main_window(app);
                let _ = app.emit("tray-compose", ());
            }
            "tray-quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// 开机自动启动时带这个参数：只在后台收信，不弹出窗口。
const HIDDEN_FLAG: &str = "--hidden";

fn launched_hidden(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|a| a == HIDDEN_FLAG)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 必须最先注册：再次打开 App（或开机启动后又手动打开）时只把已有窗口叫出来，
        // 不能出现两个实例同时收信、写同一个数据库。
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // 已经在运行时又被开机启动拉起：留在后台
            if !launched_hidden(args) {
                show_main_window(app);
            }
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![HIDDEN_FLAG]),
        ))
        .setup(|app| {
            // 主窗口在这里创建，好接管新窗口请求：邮件正文 iframe 是不能跑脚本的沙箱，
            // WebKit 里外层挂的点击监听收不到事件，链接只能靠 <base target="_blank">
            // 发出新窗口请求；wry 默认直接丢弃这种请求，点了就没反应。
            let conf = app
                .config()
                .app
                .windows
                .iter()
                .find(|w| w.label == "main")
                .cloned();
            let handle = app.handle().clone();
            tauri::WebviewWindowBuilder::from_config(
                app.handle(),
                &conf.context("缺少主窗口配置")?,
            )?
            .on_new_window(move |url, _| {
                if supported_link(url.as_str()) {
                    let _ = handle.opener().open_url(url.as_str(), None::<&str>);
                }
                tauri::webview::NewWindowResponse::Deny
            })
            .build()?;

            let app_menu = SubmenuBuilder::new(app, "MailBeacon")
                .about_with_text("关于 MailBeacon", None)
                .separator()
                .hide_with_text("隐藏 MailBeacon")
                .hide_others_with_text("隐藏其他窗口")
                .show_all_with_text("显示全部")
                .separator()
                .quit_with_text("退出 MailBeacon")
                .build()?;
            let file_menu = SubmenuBuilder::new(app, "文件")
                .close_window_with_text("关闭窗口")
                .separator()
                .quit_with_text("退出 MailBeacon")
                .build()?;
            let edit_menu = SubmenuBuilder::new(app, "编辑")
                .undo_with_text("撤销")
                .redo_with_text("重做")
                .separator()
                .cut_with_text("剪切")
                .copy_with_text("拷贝")
                .paste_with_text("粘贴")
                .select_all_with_text("全选")
                .build()?;
            let view_menu = SubmenuBuilder::new(app, "视图")
                .fullscreen_with_text("进入全屏")
                .build()?;
            let window_menu = SubmenuBuilder::new(app, "窗口")
                .minimize_with_text("最小化")
                .close_window_with_text("关闭窗口")
                .build()?;
            let help_menu = SubmenuBuilder::new(app, "帮助")
                .about_with_text("关于 MailBeacon", None)
                .build()?;
            let menu = MenuBuilder::new(app)
                .items(&[
                    &app_menu,
                    &file_menu,
                    &edit_menu,
                    &view_menu,
                    &window_menu,
                    &help_menu,
                ])
                .build()?;
            app.set_menu(menu)?;
            let dir = app.path().app_config_dir()?;
            let store = Store::load(dir)?;
            let drafts_dir = store.dir().join("drafts");
            let outbox_dir = store.dir().join("outbox");
            let db = contacts::Db::open(&store.dir().join("local.db")).map_err(|e| {
                eprintln!("本地数据库打不开，通讯录暂不可用：{e:#}");
                format!("通讯录暂不可用：{e:#}")
            });
            let mut prefs = db
                .as_ref()
                .ok()
                .and_then(|db| db.load_prefs().ok())
                .unwrap_or_default();
            if let Err(e) = apply_tray(app.handle(), prefs.tray) {
                eprintln!("托盘图标创建失败：{e:#}");
                // 本次运行按没有托盘处理（关闭窗口即退出），不改保存的设置
                prefs.tray = false;
            }
            let tray = prefs.tray;
            let saved_rules = match db.as_ref().map(|db| db.load_rules()) {
                Ok(Ok(rules)) => rules,
                Ok(Err(e)) => {
                    eprintln!("收信规则读取失败：{e:#}");
                    Vec::new()
                }
                Err(_) => Vec::new(),
            };
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
                pop3: Mutex::default(),
                attachments: drafts::AttachmentRegistry::default(),
                local_drafts: drafts::LocalDrafts::new(drafts_dir),
                outbox: outbox::Outbox::new(outbox_dir),
                outbox_wake: tokio::sync::Notify::new(),
                outbox_send: tokio::sync::Mutex::new(()),
                outbox_sent: Mutex::default(),
                db,
                recorded_senders: Mutex::default(),
                prefs: Mutex::new(prefs),
                rules: Mutex::new(saved_rules),
                rule_memory: Mutex::default(),
                no_thread_search: Mutex::default(),
                wake,
            });
            // 窗口创建时是隐藏的（tauri.conf.json），免得开机启动时闪一下。
            // 开机启动时留在后台；没有托盘又不是 macOS 时找不回窗口，照常显示。
            let stay_hidden = launched_hidden(std::env::args())
                && close_hides_window("main", tray, cfg!(target_os = "macos"));
            if !stay_hidden && let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
            }
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
            tauri::async_runtime::spawn(outbox_worker(app.handle().clone()));
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
            search_messages,
            cancel_search,
            get_previews,
            get_message,
            save_message,
            set_flag,
            move_messages,
            save_attachment,
            open_link,
            send_mail,
            save_draft,
            delete_draft,
            pick_attachments,
            draft_attachments,
            release_attachments,
            clean_html,
            pick_inline_image,
            contacts_suggest,
            contacts_forget,
            contacts_list,
            contact_save,
            contact_delete,
            groups_list,
            group_save,
            group_delete,
            group_members_set,
            contacts_import,
            contacts_export,
            set_badge,
            get_prefs,
            save_prefs,
            get_rules,
            save_rules,
            run_rules,
            thread_messages,
            templates_list,
            templates_save,
            send_receipt,
            ignore_receipt,
            schedule_mail,
            outbox_list,
            outbox_send_now,
            outbox_reschedule,
            outbox_cancel,
        ])
        .on_window_event(|window, event| {
            // 关闭窗口只隐藏、App 继续在后台收信：macOS 上从 Dock 找回，
            // 其他平台从托盘图标找回。⌘Q / 菜单「退出」/ 托盘「退出」才真正退出。
            // 没有托盘的 Windows/Linux 关闭即退出，免得进程在后台找不回来。
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // 以托盘图标实际存在为准：创建失败时关闭窗口就退出，免得找不回来
                    let tray = window.app_handle().tray_by_id(TRAY_ID).is_some();
                    if close_hides_window(window.label(), tray, cfg!(target_os = "macos")) {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
                // 拖进窗口的文件在这里登记成附件令牌，网页端拿不到路径。
                tauri::WindowEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) => {
                    let state = window.state::<AppState>();
                    let picked: Vec<_> = paths
                        .iter()
                        .filter_map(|path| state.attachments.register(path).ok())
                        .collect();
                    if !picked.is_empty() {
                        let _ = window.emit("attachments-dropped", picked);
                    }
                }
                _ => {}
            }
        })
        .build(tauri::generate_context!())
        .expect("启动 MailBeacon 失败")
        .run(|app, event| {
            // 点 Dock 图标时把隐藏的主窗口找回来。
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows: false,
                ..
            } = event
            {
                show_main_window(app);
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

/// 关闭主窗口时是否只隐藏：要有托盘图标或 Dock 图标能把窗口找回来。
fn close_hides_window(label: &str, tray: bool, dock: bool) -> bool {
    label == "main" && (tray || dock)
}

#[cfg(test)]
mod command_tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    #[test]
    fn supported_links_match_the_frontend_link_policy() {
        for url in [
            "https://example.com/report",
            "HTTP://example.com",
            "mailto:person@example.com",
            "mailto:?to=person@example.com&subject=hi",
            "tel:+8613812345678",
        ] {
            assert!(supported_link(url), "{url}");
        }
        for url in [
            "",
            "https://",
            "mailto:",
            "tel:",
            " https://example.com",
            "https://example.com\n",
            "javascript:alert(1)",
            "file:///tmp/private",
            "about:srcdoc#top",
            "tel:abc",
            "http://tauri.localhost/foo.html",
            "https://TAURI.localhost:443/#top",
            "http://tauri.localhost./",
            "http://a@tauri.localhost/",
        ] {
            assert!(!supported_link(url), "{url}");
        }
    }

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
    async fn opening_cached_message_reuses_body_and_marks_it_seen() {
        let cache = Mutex::new(BodyCache::default());
        cache.lock().unwrap().insert(
            ("a".into(), "INBOX".into(), 7, 1),
            Arc::new(b"same mail".to_vec()),
            42,
        );
        // A cached normal open must not issue UID FETCH BODY[] again. It still
        // marks the message seen, preserving get_message's existing behavior.
        let mut session =
            scripted_session(vec![selected(7), (String::new(), Some("OK seen"))]).await;
        let (raw, date, validity) = read_raw(&mut session, &cache, "a", "INBOX", 1, false, None)
            .await
            .unwrap();
        assert_eq!(raw.as_slice(), b"same mail");
        assert_eq!(date, 42);
        assert_eq!(validity, 7);
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
    fn closing_hides_only_the_main_window_and_only_when_it_can_be_reopened() {
        assert!(
            close_hides_window("main", true, false),
            "有托盘图标时关闭只隐藏"
        );
        assert!(
            close_hides_window("main", false, true),
            "macOS 可以从 Dock 找回"
        );
        assert!(
            !close_hides_window("main", false, false),
            "没有托盘也没有 Dock 时直接退出"
        );
        assert!(!close_hides_window("other", true, true));
    }

    #[test]
    fn scheduled_send_time_must_be_soon_but_not_too_far() {
        let now = now_ms();
        assert!(check_send_at(now + 30 * 1000).is_err(), "不到一分钟");
        assert!(check_send_at(now + 2 * MINUTE_MS).is_ok());
        assert!(check_send_at(now + 400 * 24 * 60 * MINUTE_MS).is_err());
    }

    #[test]
    fn hidden_launch_flag_is_recognised() {
        assert!(launched_hidden(
            ["mailbeacon", "--hidden"].map(String::from)
        ));
        assert!(!launched_hidden(["mailbeacon"].map(String::from)));
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
            recipients: Vec::new(),
            ..Default::default()
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
