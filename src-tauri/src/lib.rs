//! MailPulse 邮件客户端。
//!
//! 每个账号两条 IMAP 连接：
//! - 同步连接（sync.rs）：常驻 IDLE 盯收件箱，新邮件实时推给界面；
//! - 命令连接（这里）：读信、翻页、标记、移动等按需操作，空闲超过一分钟先探活再用。

mod config;
#[cfg(test)]
mod e2e;
mod imap;
mod smtp;
mod sync;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use futures::{TryStreamExt, future::BoxFuture};
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
            timeout(OP_TIMEOUT, self.s.select(folder))
                .await
                .context("打开文件夹超时")?
                .with_context(|| format!("打开文件夹 {} 失败", imap::decode_mutf7(folder)))?;
            self.selected = Some(folder.to_string());
        }
        Ok(())
    }
}

type Slot = Arc<tokio::sync::Mutex<Option<Cmd>>>;
/// (账号, 文件夹, UID)
type MsgKey = (String, String, u32);

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
    slots: Mutex<HashMap<String, Slot>>,
    folders: Mutex<HashMap<String, Vec<Folder>>>,
    live: Mutex<HashMap<String, Live>>,
    /// 摘要请求（部分 FETCH）出错过的账号，不再请求，避免反复断线重连
    no_preview: Mutex<std::collections::HashSet<String>>,
    /// 最近打开的几封邮件原文，保存附件时不用再下载
    bodies: Mutex<VecDeque<(MsgKey, Arc<Vec<u8>>)>>,
    wake: broadcast::Sender<()>,
}

impl AppState {
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

    fn cache_body(&self, key: MsgKey, raw: Arc<Vec<u8>>) {
        let mut b = self.bodies.lock().unwrap();
        b.retain(|(k, _)| *k != key);
        b.push_back((key, raw));
        while b.len() > 30 {
            b.pop_front();
        }
    }

    fn cached_body(&self, key: &MsgKey) -> Option<Arc<Vec<u8>>> {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    }

    /// 同步任务推送事件时顺带记一份，界面（重新）加载时可以直接拿到。
    fn record(&self, ev: &Event) {
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
            Event::Flags { .. } => {}
        }
    }

    fn start_watcher(&self, app: &AppHandle, acct: Account, pass: String) {
        let id = acct.id.clone();
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
}

#[tauri::command]
fn snapshot(state: State<'_, AppState>) -> Snapshot {
    Snapshot {
        accounts: state.store.lock().unwrap().accounts.clone(),
        live: state.live.lock().unwrap().clone(),
    }
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
fn remove_account(state: State<'_, AppState>, account_id: String) -> CmdResult<()> {
    {
        let mut store = state.store.lock().unwrap();
        store.accounts.retain(|a| a.id != account_id);
        store.save().map_err(err)?;
    }
    if let Some(h) = state.watchers.lock().unwrap().remove(&account_id) {
        h.abort();
    }
    state.slots.lock().unwrap().remove(&account_id);
    state.folders.lock().unwrap().remove(&account_id);
    state.live.lock().unwrap().remove(&account_id);
    state.passwords.lock().unwrap().remove(&account_id);
    config::delete_password(&account_id);
    Ok(())
}

#[tauri::command]
async fn list_folders(state: State<'_, AppState>, account_id: String) -> CmdResult<Vec<Folder>> {
    state.folders(&account_id).await.map_err(err)
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

async fn fetch_raw(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    peek: bool,
) -> Result<(Arc<Vec<u8>>, i64)> {
    let key = (account_id.to_string(), folder.to_string(), uid);
    let folder = folder.to_string();
    let (raw, date) = state
        .with_cmd(account_id, |c, _| {
            Box::pin(async move {
                c.select(&folder).await?;
                let items = if peek {
                    "(UID INTERNALDATE BODY.PEEK[])"
                } else {
                    "(UID INTERNALDATE BODY[])"
                };
                let fetches = timeout(OP_TIMEOUT * 3, async {
                    c.s.uid_fetch(uid.to_string(), items)
                        .await?
                        .try_collect::<Vec<_>>()
                        .await
                })
                .await
                .context("下载邮件超时")??;
                let m = fetches
                    .iter()
                    .find(|m| m.uid == Some(uid))
                    .ok_or_else(|| anyhow!("邮件已不存在（可能已在别处删除）"))?;
                let raw = m
                    .body()
                    .ok_or_else(|| anyhow!("服务器没有返回邮件内容"))?
                    .to_vec();
                Ok((
                    raw,
                    m.internal_date().map(|d| d.timestamp_millis()).unwrap_or(0),
                ))
            })
        })
        .await?;
    let raw = Arc::new(raw);
    state.cache_body(key, raw.clone());
    Ok((raw, date))
}

/// 打开一封邮件（同时标记为已读）
#[tauri::command]
async fn get_message(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<MessageView> {
    let (raw, date) = fetch_raw(&state, &account_id, &folder, uid, false)
        .await
        .map_err(err)?;
    imap::parse_message(&raw, date).map_err(err)
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
                timeout(OP_TIMEOUT, async {
                    c.s.uid_store(imap::uid_set(&uids), op)
                        .await?
                        .try_collect::<Vec<_>>()
                        .await
                })
                .await??;
                Ok(())
            })
        })
        .await
        .map_err(err)
}

/// 移动到某类文件夹（role：trash / junk / archive / inbox），或直接指定目标文件夹（target）。
/// 已经在废纸篓里的再删就是彻底删除。
#[tauri::command]
async fn move_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uids: Vec<u32>,
    role: String,
    target: Option<String>,
) -> CmdResult<()> {
    let mut target = match target {
        Some(t) => Some(t),
        None => state
            .folder_by_role(&account_id, &role)
            .await
            .map_err(err)?,
    };
    if target.is_none() && role == "archive" {
        // Gmail 没有「归档」文件夹，移到「所有邮件」就是归档
        target = state
            .folder_by_role(&account_id, "all")
            .await
            .map_err(err)?;
    }
    let permanent = role == "trash" && target.as_deref().is_none_or(|t| t == folder);
    let target = match (target, permanent) {
        (_, true) => None,
        (Some(t), false) => Some(t),
        (None, false) => {
            return Err(match role.as_str() {
                "archive" => "这个邮箱没有归档文件夹",
                "junk" => "这个邮箱没有垃圾邮件文件夹",
                _ => "找不到目标文件夹",
            }
            .into());
        }
    };
    state
        .with_cmd(&account_id, |c, _| {
            Box::pin(async move {
                c.select(&folder).await?;
                imap::move_uids(&mut c.s, &imap::uid_set(&uids), target.as_deref()).await?;
                Ok(())
            })
        })
        .await
        .map_err(err)
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
) -> CmdResult<String> {
    let key = (account_id.clone(), folder.clone(), uid);
    let raw = match state.cached_body(&key) {
        Some(r) => r,
        None => {
            fetch_raw(&state, &account_id, &folder, uid, true)
                .await
                .map_err(err)?
                .0
        }
    };
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
    let (msg, message_id) = smtp::build(&acct, &draft).map_err(err)?;
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
            app.manage(AppState {
                store: Mutex::new(store),
                passwords: Mutex::new(passwords.clone()),
                watchers: Mutex::default(),
                slots: Mutex::default(),
                folders: Mutex::default(),
                live: Mutex::new(live),
                no_preview: Mutex::default(),
                bodies: Mutex::default(),
                wake,
            });
            // 仅 debug 构建：把 MAILPULSE_DEBUG_SCRIPT 指向的 JS 注入页面，用于本地自动化测试
            #[cfg(debug_assertions)]
            if let Ok(path) = std::env::var("MAILPULSE_DEBUG_SCRIPT") {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(3)).await;
                    if let (Ok(js), Some(w)) = (std::fs::read_to_string(&path), handle.get_webview_window("main")) {
                        let _ = w.eval(&js);
                    }
                });
            }
            let state = app.state::<AppState>();
            for a in accounts {
                if let Some(p) = passwords.remove(&a.id) {
                    state.start_watcher(app.handle(), a, p);
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            add_account,
            remove_account,
            list_folders,
            list_messages,
            get_previews,
            get_message,
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
