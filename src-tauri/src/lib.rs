//! MailBeacon 邮件客户端。
//!
//! 每个账号三条 IMAP 连接：
//! - 同步连接（sync.rs）：常驻 IDLE 盯收件箱，新邮件实时推给界面；
//! - 命令连接（lanes.rs）：翻页、摘要、标记、移动等按需操作，空闲超过一分钟先探活再用；
//! - 读信连接（lanes.rs）：下载邮件原文，大邮件不挡住翻页和摘要。

mod archive_source;
mod auto_download;
mod body_store;
mod bulk_download;
mod cache_warm;
mod config;
mod contacts;
mod download_protocol;
mod drafts;
#[cfg(test)]
mod e2e;
mod export;
mod html_mail;
mod imap;
mod lanes;
mod mime;
mod outbox;
mod pop3;
mod prefs;
mod rules;
mod search_index;
mod smtp;
mod sync;
mod templates;
#[cfg(test)]
mod test_imap;
mod threads;

use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
use imap::{Folder, MessageView, OP_TIMEOUT, Preview, Summary};
use sync::Event;

type CmdResult<T> = std::result::Result<T, String>;

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

type Cmd = lanes::Conn<imap::ImapStream>;

/// UID 只有在同一 UIDVALIDITY 内才唯一，文件夹重建后不能复用旧原文。
type MsgKey = (String, String, u32, u32);
type CachedBody = (Arc<Vec<u8>>, i64);
const BODY_CACHE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Default)]
struct BodyCache {
    entries: VecDeque<(MsgKey, CachedBody)>,
    bytes: usize,
    /// 磁盘上的原文缓存（只给 IMAP 用），重启后看过的邮件不用重新下载
    disk: Option<Arc<body_store::BodyStore>>,
    /// 自动下载、全量下载存下来的邮件，只读地当缓存用
    archives: Option<Arc<archive_source::ArchiveSource>>,
}

impl BodyCache {
    fn with_disk(disk: Arc<body_store::BodyStore>) -> Self {
        Self {
            disk: Some(disk),
            ..Self::default()
        }
    }

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

/// 后台缓存预热任务（见 cache_warm.rs）。`generation` 防止已取消的旧任务改写新任务的进度。
struct WarmJob {
    generation: u64,
    progress: cache_warm::Progress,
    handle: Option<JoinHandle<()>>,
}

/// 本地搜索索引的后台任务状态：进度 + 最近一次检查时间。
/// 建完的进度保留在这里，30 秒内不重复去服务器枚举 UID。
struct SearchJob {
    progress: search_index::Progress,
    checked_at: Instant,
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
    /// 每个账号的命令连接和读信连接
    lanes: lanes::Lanes<imap::ImapStream>,
    /// 每个账号最近一次打开邮件的序号
    turns: Arc<lanes::Turns>,
    folders: Mutex<HashMap<String, Vec<Folder>>>,
    live: Mutex<HashMap<String, Live>>,
    /// 摘要请求（部分 FETCH）出错过的账号，不再请求，避免反复断线重连
    no_preview: Mutex<std::collections::HashSet<String>>,
    /// 邮件结构（BODYSTRUCTURE）解析不了的账号，不再先出文字
    no_text_first: Mutex<std::collections::HashSet<String>>,
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
    /// 本地搜索索引：邮件摘要和正文开头存在 local.db，搜索不再依赖服务器 IMAP SEARCH
    search_index: std::result::Result<Arc<search_index::SearchIndex>, String>,
    /// 每个「账号|文件夹」的索引进度（内存里，跨搜索复用）
    search_jobs: Arc<Mutex<HashMap<String, SearchJob>>>,
    /// 每个「账号|文件夹」的索引连续失败了几次（决定多久后自动再试），以及是否已经排了一次重试
    search_failures: Arc<Mutex<HashMap<String, (u32, bool)>>>,
    /// 打开没索引过的文件夹时，后台补建摘要索引的上次开始时间（防抖）
    list_index_jobs: Mutex<HashMap<String, Instant>>,
    /// 各文件夹这次会话里确认过的 UIDVALIDITY（账号|文件夹 → 值）：打开已缓存
    /// 的邮件时先用它读本地缓存，不用每次等服务器 SELECT
    body_validity: Mutex<HashMap<String, u32>>,
    /// 本机安装的 App（「打开方式」兜底）：第一次扫描后缓存，右键附件不用每次重扫
    installed_apps: Mutex<Option<Vec<OpenWithApp>>>,
    /// 同时只允许一个摘要索引补建任务：快速切文件夹时不会同时开好几条连接
    list_index_running: Arc<AtomicBool>,
    /// 每个账号的后台缓存预热：进度和正在跑的任务
    cache_warm: Arc<Mutex<HashMap<String, WarmJob>>>,
    wake: broadcast::Sender<()>,
    /// 界面「收取」按钮的请求，交给各账号的收信连接处理
    checks: sync::CheckMail,
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

    pub(crate) fn search_index(&self) -> Result<&Arc<search_index::SearchIndex>> {
        self.search_index.as_ref().map_err(|e| anyhow!("{e}"))
    }

    /// 把服务器上的最新摘要写进本地索引。索引还没建好时忽略（下次索引会补上）：
    /// 列表优先读本地索引，收信、改标记、移动之后要马上把变化写回去。
    pub(crate) fn index_upsert(
        &self,
        account_id: &str,
        folder: &str,
        uid_validity: u32,
        mails: &[Summary],
    ) {
        if mails.is_empty() {
            return;
        }
        let Ok(index) = self.search_index() else {
            return;
        };
        if let Err(e) = index.upsert_summaries(account_id, folder, uid_validity, mails) {
            eprintln!("更新本地索引失败（{folder}）：{e:#}");
        }
    }

    /// 用服务器上的已读/星标刷新本地索引里的邮件。
    pub(crate) fn index_flags(&self, account_id: &str, folder: &str, flags: &[(u32, bool, bool)]) {
        let Ok(index) = self.search_index() else {
            return;
        };
        if let Err(e) = index.update_flags(account_id, folder, flags) {
            eprintln!("刷新本地索引的标记失败（{folder}）：{e:#}");
        }
    }

    /// 界面改了某几封邮件的标记，同步到本地索引。
    pub(crate) fn index_set_flags(
        &self,
        account_id: &str,
        folder: &str,
        uids: &[u32],
        seen: Option<bool>,
        flagged: Option<bool>,
    ) {
        let Ok(index) = self.search_index() else {
            return;
        };
        if let Err(e) = index.set_flags(account_id, folder, uids, seen, flagged) {
            eprintln!("更新本地索引的标记失败（{folder}）：{e:#}");
        }
    }

    /// 邮件在服务器上被删除或移走，从本地索引里去掉。
    pub(crate) fn index_remove(&self, account_id: &str, folder: &str, uids: &[u32]) {
        let Ok(index) = self.search_index() else {
            return;
        };
        if let Err(e) = index.remove_uids(account_id, folder, uids) {
            eprintln!("清理本地索引失败（{folder}）：{e:#}");
        }
    }

    /// 有邮件移进了已索引的文件夹：忽略「刚检查过」的防抖，马上让后台补一次索引，
    /// 否则本地列表会一直缺着这几封。
    pub(crate) fn refresh_index_soon(&self, app: &AppHandle, account_id: &str, folder: &str) {
        if let Ok(mut jobs) = self.search_jobs.lock() {
            if let Some(job) = jobs.get_mut(&format!("{account_id}|{folder}")) {
                if !job.progress.running {
                    job.checked_at = Instant::now() - Duration::from_secs(31);
                }
            }
        }
        if let Err(e) = self.ensure_search_index(app, account_id, folder) {
            eprintln!("移动后刷新本地索引失败（{folder}）：{e:#}");
        }
    }

    /// 记下这次会话里服务器确认过的 UIDVALIDITY（收信连接 SELECT 到、或正常
    /// 打开邮件时确认到）。打开已缓存的邮件时用它直接读本地缓存。
    pub(crate) fn remember_body_validity(&self, account_id: &str, folder: &str, validity: u32) {
        if validity == 0 {
            return;
        }
        self.body_validity
            .lock()
            .unwrap()
            .insert(format!("{account_id}|{folder}"), validity);
    }

    fn known_body_validity(&self, account_id: &str, folder: &str) -> Option<u32> {
        self.body_validity
            .lock()
            .unwrap()
            .get(&format!("{account_id}|{folder}"))
            .copied()
    }

    /// 打开一个还没索引过的文件夹时，后台补建摘要索引（只摘要，不下正文）。
    /// 建完后列表就能像收件箱一样从本地一次显示全部，不再一页 100 封。
    fn ensure_list_index(&self, app: &AppHandle, account_id: &str, folder: &str) {
        // 收件箱启动时就按完整索引在跑，不用另外补
        if folder.eq_ignore_ascii_case("INBOX") {
            return;
        }
        let Ok(index) = self.search_index().cloned() else {
            return;
        };
        // 摘要已经有了（正文可能还没补）就不用再建
        let ready = index
            .progress(account_id, folder)
            .map(|p| p.complete || (p.total > 0 && p.indexed >= p.total))
            .unwrap_or(false);
        if ready {
            return;
        }
        // 同一时间只跑一个补建任务；正在跑就先让这次列表走服务器分页，
        // 下次打开这个文件夹再补
        if self.list_index_running.swap(true, Ordering::SeqCst) {
            return;
        }
        let key = format!("{account_id}|{folder}");
        {
            let mut jobs = self.list_index_jobs.lock().unwrap();
            // 5 分钟内试过就不重复连服务器（失败也一样，下次打开再试）
            if jobs
                .get(&key)
                .is_some_and(|started| started.elapsed() < Duration::from_secs(300))
            {
                return;
            }
            jobs.insert(key, Instant::now());
        }
        let (acct, pass) = match self.account(account_id) {
            Ok((acct, pass)) => (acct, pass),
            Err(_) => {
                self.list_index_running.store(false, Ordering::SeqCst);
                return;
            }
        };
        let running = self.list_index_running.clone();
        let (app, account_id, folder) = (app.clone(), account_id.to_string(), folder.to_string());
        tauri::async_runtime::spawn(async move {
            let connect = move || {
                let (acct, pass) = (acct.clone(), pass.clone());
                async move { imap::login(&acct, &pass).await }
            };
            match search_index::index_headers_resilient(connect, &index, &account_id, &folder).await
            {
                Ok(_) => {}
                Err(e) => eprintln!("补建摘要索引失败（{folder}）：{e:#}"),
            }
            running.store(false, Ordering::SeqCst);
            // 通知界面：这个文件夹现在可以改读本地列表了
            sync::emit(
                &app,
                Event::ListIndex {
                    account_id: &account_id,
                    folder: &folder,
                },
            );
        });
    }

    /// 这个文件夹的摘要索引是否已经够用来显示列表。
    fn headers_index_ready(&self, account_id: &str, folder: &str) -> bool {
        self.search_index()
            .ok()
            .and_then(|index| index.progress(account_id, folder).ok())
            .is_some_and(|progress| {
                progress.complete || (progress.total > 0 && progress.indexed >= progress.total)
            })
    }

    /// 后台把其它文件夹的摘要索引也逐个建起来（只建摘要、不下正文）：
    /// 这样第一次打开已发送、归档、自定义文件夹也能直接读本地，不用等一页摘要
    /// 从服务器慢慢拉。一次只建一个文件夹，读信时让路；已经建好的跳过。
    fn start_header_index_sweep(&self, app: &AppHandle) {
        let accounts: Vec<String> = self
            .store
            .lock()
            .unwrap()
            .accounts
            .iter()
            .filter(|a| a.receive_protocol == config::ReceiveProtocol::Imap)
            .map(|a| a.id.clone())
            .collect();
        if accounts.is_empty() {
            return;
        }
        let handle = app.clone();
        tauri::async_runtime::spawn(async move {
            // 启动阶段收件箱同步、索引、缓存预热都在跑，过几分钟再慢慢扫其它文件夹
            tokio::time::sleep(Duration::from_secs(180)).await;
            for account_id in accounts {
                let Some(state) = handle.try_state::<AppState>() else {
                    return;
                };
                let folders = state
                    .with_cmd(&account_id, |c, _| {
                        Box::pin(async move { imap::list_folders(&mut c.s).await })
                    })
                    .await
                    .unwrap_or_else(|e| {
                        eprintln!("后台建索引：读取文件夹列表失败（{account_id}）：{e:#}");
                        Vec::new()
                    });
                for folder in folders {
                    let path = folder.path.clone();
                    if path.eq_ignore_ascii_case("INBOX") {
                        continue;
                    }
                    let Some(state) = handle.try_state::<AppState>() else {
                        return;
                    };
                    if state.headers_index_ready(&account_id, &path) {
                        continue;
                    }
                    // 用户正在读信就等一等，别和打开邮件抢带宽
                    loop {
                        let Some(state) = handle.try_state::<AppState>() else {
                            return;
                        };
                        if state.account(&account_id).is_err() {
                            return;
                        }
                        if state.turns.idle_for(&account_id) >= Duration::from_secs(15) {
                            break;
                        }
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    let Some(state) = handle.try_state::<AppState>() else {
                        return;
                    };
                    state.ensure_list_index(&handle, &account_id, &path);
                    // 等这个文件夹建完（最长 15 分钟；特别大的下次启动接着建）
                    let deadline = Instant::now() + Duration::from_secs(15 * 60);
                    while Instant::now() < deadline {
                        tokio::time::sleep(Duration::from_secs(2)).await;
                        let Some(state) = handle.try_state::<AppState>() else {
                            return;
                        };
                        if state.headers_index_ready(&account_id, &path) {
                            break;
                        }
                        // 任务没在跑：被防抖跳过或失败了，换下一个文件夹
                        if !state.list_index_running.load(Ordering::SeqCst) {
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            }
        });
    }

    fn body_store(&self) -> Option<Arc<body_store::BodyStore>> {
        self.bodies.lock().unwrap().disk.clone()
    }

    fn archives(&self) -> Option<Arc<archive_source::ArchiveSource>> {
        self.bodies.lock().unwrap().archives.clone()
    }

    /// 账号的下载目录改了（自动下载目录、全量下载目录）：重新登记给读信用
    fn refresh_archives(&self, account_id: &str) {
        let Some(archives) = self.archives() else {
            return;
        };
        match self.store.lock().unwrap().get(account_id) {
            Some(acct) => archives.register(acct),
            None => archives.remove_account(account_id),
        }
    }

    /// （重新）开始后台预热收件箱：`delay` 后开始，搜索索引在建时先等它建完，
    /// 之后每批都等用户不在读信时才下。`manual` 为界面上「立即更新缓存」：
    /// 不等索引，设置里关了预热也照样跑一遍。
    fn start_cache_warm(&self, app: &AppHandle, account_id: &str, delay: Duration, manual: bool) {
        let Some(acct) = self.store.lock().unwrap().get(account_id).cloned() else {
            return;
        };
        let Some(store) = self.body_store() else {
            return;
        };
        let archives = self.archives();
        let settings = acct.body_cache;
        let limit = settings.limit_bytes();
        // 先记上限：之后打开邮件的写入马上按新上限算（删超出部分在后台做）
        store.preset_limit(&acct.id, limit);
        let id = acct.id.clone();
        let run =
            acct.receive_protocol == config::ReceiveProtocol::Imap && (settings.enabled || manual);
        let progress = cache_warm::Progress {
            state: if run { "waiting" } else { "off" }.into(),
            ..Default::default()
        };
        static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
        let jobs = self.cache_warm.clone();
        let publish = {
            let (app, id, jobs) = (app.clone(), id.clone(), jobs.clone());
            move |p: cache_warm::Progress| {
                // 已被新任务取代：不改进度，也不推给界面
                match jobs.lock().unwrap().get_mut(&id) {
                    Some(job) if job.generation == generation => job.progress = p.clone(),
                    _ => return,
                }
                sync::emit(
                    &app,
                    Event::BodyCache {
                        account_id: &id,
                        progress: p,
                    },
                );
            }
        };
        let app = app.clone();
        let pass = self.passwords.lock().unwrap().get(&id).cloned();
        let task = {
            let (store, id) = (store.clone(), id.clone());
            async move {
                // 上限可能调小了：删掉超出的（磁盘操作，不占异步线程）
                let (pruned, limit_id) = (store.clone(), id.clone());
                let _ =
                    tokio::task::spawn_blocking(move || pruned.set_limit(&limit_id, limit)).await;
                publish(progress.with_usage(&store, &id, limit));
                if !run {
                    return;
                }
                // 没有授权码（钥匙串里读不到）就连不上服务器
                let Some(pass) = pass else {
                    publish(
                        cache_warm::Progress {
                            state: "error".into(),
                            detail: "缺少授权码，无法连接服务器".into(),
                            ..Default::default()
                        }
                        .with_usage(&store, &id, limit),
                    );
                    return;
                };
                tokio::time::sleep(delay).await;
                // 搜索索引还在建时先等它（最多半小时），别同时开太多连接
                let index_key = format!("{id}|INBOX");
                for _ in 0..if manual { 0 } else { 60 } {
                    let Some(state) = app.try_state::<AppState>() else {
                        return;
                    };
                    let indexing = state
                        .search_jobs
                        .lock()
                        .unwrap()
                        .get(&index_key)
                        .is_some_and(|j| j.progress.running);
                    if !indexing {
                        break;
                    }
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
                let idle = || {
                    let (app, id) = (app.clone(), id.clone());
                    async move {
                        while let Some(state) = app.try_state::<AppState>() {
                            if state.turns.idle_for(&id) >= cache_warm::IDLE_AFTER_OPEN {
                                break;
                            }
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                    }
                };
                // 下载超时过的邮件跨重试记住：先单独下，还不行就跳过
                let mut trouble = cache_warm::Trouble::default();
                let mut failures = 0;
                // 到目前为止最多处理到第几封：这次超过了才算有进展
                let mut best = 0;
                let mut reached = 0;
                loop {
                    let result = async {
                        let mut session = imap::login(&acct, &pass).await?;
                        let done = cache_warm::warm_folder(
                            &mut session,
                            store.clone(),
                            (&id, "INBOX"),
                            cache_warm::Plan {
                                limit,
                                max_mail: settings.max_mail_bytes(),
                                trouble: &mut trouble,
                                archives: archives.clone(),
                            },
                            idle,
                            |p| {
                                reached = reached.max(p.done);
                                publish(p.clone())
                            },
                        )
                        .await;
                        let _ = timeout(Duration::from_secs(5), session.logout()).await;
                        done
                    }
                    .await;
                    let Err(e) = result else {
                        return;
                    };
                    // 有进展就重新计数：长时间预热中偶尔断几次线不算失败
                    failures = if reached > best { 1 } else { failures + 1 };
                    best = best.max(reached);
                    // 断线、超时：等一分钟接着下（已下的不会重下），连续三次没进展就停
                    if failures < 3 && download_protocol::connection_problem(&e) {
                        eprintln!("缓存预热中断，一分钟后继续：{e:#}");
                        tokio::time::sleep(Duration::from_secs(60)).await;
                        continue;
                    }
                    eprintln!("缓存预热失败：{e:#}");
                    publish(
                        cache_warm::Progress {
                            state: "error".into(),
                            detail: format!("{e:#}"),
                            ..Default::default()
                        }
                        .with_usage(&store, &id, limit),
                    );
                    return;
                }
            }
        };
        // 换任务在一个锁里完成：并发的两次启动不会留下取消不了的旧任务
        let mut jobs = self.cache_warm.lock().unwrap();
        if let Some(old) = jobs.remove(&id)
            && let Some(handle) = old.handle
        {
            handle.abort();
        }
        jobs.insert(
            id,
            WarmJob {
                generation,
                progress: cache_warm::Progress {
                    state: if run { "waiting" } else { "off" }.into(),
                    ..Default::default()
                },
                handle: Some(tauri::async_runtime::spawn(task)),
            },
        );
    }

    /// 确保这个文件夹的本地搜索索引可用：需要时起一个后台任务增量补齐，
    /// 立刻返回当前进度。索引用独立的 IMAP 连接，不占用界面操作的命令连接。
    fn ensure_search_index(
        &self,
        app: &AppHandle,
        account_id: &str,
        folder: &str,
    ) -> Result<search_index::Progress> {
        let index = self.search_index()?.clone();
        let key = format!("{account_id}|{folder}");
        let previous = {
            let jobs = self.search_jobs.lock().unwrap();
            match jobs.get(&key) {
                Some(job) if job.progress.running => return Ok(job.progress.clone()),
                // 刚检查过：直接用上次结果，避免每敲一个字都重新枚举服务器
                Some(job) if job.checked_at.elapsed() < Duration::from_secs(30) => {
                    return Ok(job.progress.clone());
                }
                Some(job) => Some(job.progress.clone()),
                None => None,
            }
        };
        let persisted = index.progress(account_id, folder)?;
        let (indexed, total, body_indexed, body_total) = previous.as_ref().map_or(
            (
                persisted.indexed,
                persisted.total,
                persisted.body_indexed,
                persisted.body_total,
            ),
            |p| (p.indexed, p.total, p.body_indexed, p.body_total),
        );
        let initial = search_index::Progress {
            running: true,
            complete: false,
            indexed,
            total,
            body_indexed,
            body_total,
            error: None,
        };
        {
            // 并发搜索可能同时走到这里：只让一个任务真的去连服务器
            let mut jobs = self.search_jobs.lock().unwrap();
            if let Some(job) = jobs.get(&key) {
                if job.progress.running {
                    return Ok(job.progress.clone());
                }
            }
            jobs.insert(
                key.clone(),
                SearchJob {
                    progress: initial.clone(),
                    checked_at: Instant::now(),
                },
            );
        }
        let (acct, pass) = self.account(account_id)?;
        let jobs = self.search_jobs.clone();
        let failures = self.search_failures.clone();
        let app = app.clone();
        let account_id = account_id.to_string();
        let folder = folder.to_string();
        let task_initial = initial.clone();
        tauri::async_runtime::spawn(async move {
            let current = Arc::new(Mutex::new(task_initial));
            let report_jobs = jobs.clone();
            let report_current = current.clone();
            let report_app = app.clone();
            let report_account = account_id.clone();
            let report_folder = folder.clone();
            let report_key = key.clone();
            let report = move |p: &search_index::Progress| {
                *report_current.lock().unwrap() = p.clone();
                if let Ok(mut map) = report_jobs.lock() {
                    map.insert(
                        report_key.clone(),
                        SearchJob {
                            progress: p.clone(),
                            checked_at: Instant::now(),
                        },
                    );
                }
                sync::emit(
                    &report_app,
                    Event::SearchIndex {
                        account_id: &report_account,
                        folder: &report_folder,
                        indexing: p.clone(),
                    },
                );
            };
            // 邮件多、服务器慢时会超时：自动缩小批次、重新登录接着建，不用等下次搜索
            let connect = move || {
                let (acct, pass) = (acct.clone(), pass.clone());
                async move { imap::login(&acct, &pass).await }
            };
            let outcome =
                search_index::index_folder_resilient(connect, &index, &account_id, &folder, report)
                    .await;
            let mut final_progress = current.lock().unwrap().clone();
            final_progress.running = false;
            match outcome {
                Ok(done) => {
                    final_progress = done;
                    failures.lock().unwrap().remove(&key);
                }
                Err(e) => {
                    let error = format!("{e:#}");
                    // 每个文件夹同时只排一次重试，多次搜索失败不会叠出好几条重试
                    let scheduled = search_index::worth_retrying(&error).then(|| {
                        let mut map = failures.lock().unwrap();
                        let (n, pending) = map.entry(key.clone()).or_default();
                        *n += 1;
                        (!std::mem::replace(pending, true)).then_some(*n)
                    });
                    if let Some(Some(n)) = scheduled {
                        let wait = search_index::retry_delay(n);
                        eprintln!(
                            "搜索索引中断（第 {n} 次），{} 分钟后再试：{error}",
                            wait.as_secs() / 60
                        );
                        let (app, account_id, folder, key) =
                            (app.clone(), account_id.clone(), folder.clone(), key.clone());
                        let failures = failures.clone();
                        tauri::async_runtime::spawn(async move {
                            tokio::time::sleep(wait).await;
                            if let Some((_, pending)) = failures.lock().unwrap().get_mut(&key) {
                                *pending = false;
                            }
                            let Some(state) = app.try_state::<AppState>() else {
                                return;
                            };
                            // 账号已删除就不用再建了
                            if state.account(&account_id).is_ok()
                                && let Err(e) =
                                    state.ensure_search_index(&app, &account_id, &folder)
                            {
                                eprintln!("重新建搜索索引失败：{e:#}");
                            }
                        });
                    }
                    final_progress.error = Some(error);
                }
            }
            if let Ok(mut map) = jobs.lock() {
                map.insert(
                    key.clone(),
                    SearchJob {
                        progress: final_progress.clone(),
                        checked_at: Instant::now(),
                    },
                );
            }
            sync::emit(
                &app,
                Event::SearchIndex {
                    account_id: &account_id,
                    folder: &folder,
                    indexing: final_progress,
                },
            );
        });
        Ok(initial)
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

    /// 在该账号的命令连接上执行一组操作；出错就丢弃连接，下次重新登录。
    async fn with_cmd<T, F>(&self, id: &str, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(&'c mut Cmd, &'c Account) -> BoxFuture<'c, Result<T>>,
    {
        let (acct, pass) = self.account(id)?;
        let connect = || imap::login(&acct, &pass);
        self.lanes.with_cmd(id, &acct, connect, f).await
    }

    /// 下载原文用的读信连接，不和摘要、翻页抢一条连接。
    async fn with_reader<T, F>(&self, id: &str, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(&'c mut Cmd, &'c Account) -> BoxFuture<'c, Result<T>>,
    {
        let (acct, pass) = self.account(id)?;
        let connect = || imap::login(&acct, &pass);
        self.lanes.with_reader(id, &acct, connect, f).await
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
            | Event::RuleError { .. }
            | Event::SearchIndex { .. }
            | Event::ListIndex { .. }
            | Event::MessageProgress { .. }
            | Event::MessagePartial { .. }
            | Event::BodyCache { .. } => {}
        }
    }

    fn start_watcher(&self, app: &AppHandle, acct: Account, pass: String) {
        let id = acct.id.clone();
        self.start_downloader(app, id.clone());
        let checks = self.checks.register(&id);
        let handle = tauri::async_runtime::spawn(sync::watch(
            app.clone(),
            acct,
            pass,
            self.wake.clone(),
            checks,
        ));
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
    // 记下这个目录：下载下来的邮件以后直接当缓存读，重启后也记得
    let added = state
        .store
        .lock()
        .unwrap()
        .add_archive_dir(&account_id, &parent.display().to_string());
    match added {
        Ok(_) => state.refresh_archives(&account_id),
        Err(e) => eprintln!("记录全量下载目录失败：{e:#}"),
    }
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
    state.refresh_archives(&account_id);
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
    state.start_cache_warm(&app, &acct.id, Duration::from_secs(60), false);
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
    state.checks.remove(&account_id);
    if let Some(h) = state.watchers.lock().unwrap().remove(&account_id) {
        h.abort();
    }
    if let Some(h) = state.download_workers.lock().unwrap().remove(&account_id) {
        h.abort();
    }
    state.auto_downloads.lock().unwrap().remove(&account_id);
    state.bulk_downloads.lock().unwrap().remove(&account_id);
    state.lanes.remove_account(&account_id);
    state.refresh_archives(&account_id);
    if let Some(job) = state.cache_warm.lock().unwrap().remove(&account_id)
        && let Some(handle) = job.handle
    {
        handle.abort();
    }
    state.folders.lock().unwrap().remove(&account_id);
    state.live.lock().unwrap().remove(&account_id);
    state.passwords.lock().unwrap().remove(&account_id);
    let disk = {
        let mut bodies = state.bodies.lock().unwrap();
        bodies.remove_account(&account_id);
        bodies.disk.clone()
    };
    if let Some(disk) = disk {
        let id = account_id.clone();
        tauri::async_runtime::spawn_blocking(move || disk.remove_account(&id));
    }
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
    state.no_text_first.lock().unwrap().remove(&account_id);
    let job_prefix = format!("{account_id}|");
    state
        .search_jobs
        .lock()
        .unwrap()
        .retain(|key, _| !key.starts_with(&job_prefix));
    state
        .search_failures
        .lock()
        .unwrap()
        .retain(|key, _| !key.starts_with(&job_prefix));
    if let Ok(index) = state.search_index() {
        if let Err(e) = index.remove_account(&account_id) {
            eprintln!("清理账号搜索索引失败：{e:#}");
        }
    }
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

/// 列表一页。`complete` 表示这次返回的就是整个文件夹（来自本地索引），
/// 界面不用再显示「加载更早的邮件」。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MessagePage {
    items: Vec<Summary>,
    complete: bool,
    /// 后面还有更早的邮件（界面据此显示「加载更早」）；complete 时总为 false
    more: bool,
}

impl MessagePage {
    fn whole(items: Vec<Summary>) -> Self {
        Self {
            items,
            complete: true,
            more: false,
        }
    }

    fn partial(items: Vec<Summary>, more: bool) -> Self {
        Self {
            items,
            complete: false,
            more,
        }
    }
}

#[tauri::command]
async fn list_messages(
    state: State<'_, AppState>,
    app: AppHandle,
    account_id: String,
    folder: String,
    before: Option<u32>,
    flagged_only: Option<bool>,
    unread_only: Option<bool>,
) -> CmdResult<MessagePage> {
    let (flagged_only, unread_only) = (flagged_only == Some(true), unread_only == Some(true));
    if folder == drafts::LOCAL_DRAFTS {
        if before.is_some() || flagged_only || unread_only {
            return Ok(MessagePage::whole(Vec::new()));
        }
        return state
            .local_drafts
            .list(&account_id)
            .map(MessagePage::whole)
            .map_err(err);
    }
    if folder == outbox::OUTBOX {
        if before.is_some() || flagged_only || unread_only {
            return Ok(MessagePage::whole(Vec::new()));
        }
        let (acct, _) = state.account(&account_id).map_err(err)?;
        let sent = state.outbox_sent.lock().unwrap().clone();
        return Ok(MessagePage::whole(
            state
                .outbox
                .list(Some(&account_id))
                .map_err(err)?
                .iter()
                .filter(|i| !sent.contains(&(i.account_id.clone(), i.id)))
                .map(|i| i.summary(&acct.name, &acct.email))
                .collect(),
        ));
    }
    if state.is_pop3(&account_id).map_err(err)? {
        if flagged_only {
            return Ok(MessagePage::whole(Vec::new()));
        }
        let mut list = pop3_list(&state, &account_id, before).await.map_err(err)?;
        if unread_only {
            list.retain(|m| !m.seen);
        }
        let more = list.len() >= imap::PAGE;
        return Ok(MessagePage::partial(list, more));
    }
    // 摘要已经完整索引到本地时直接读本地库：几千封的收件箱不用再一页一页
    // 从服务器取，列表一次就能全显示。索引由后台任务增量维护，新邮件、改
    // 标记、移动/删除都会马上写回；索引还没建好时照旧走下面的服务器分页。
    if before.is_none() {
        if let Ok(Some(items)) = state
            .search_index()
            .and_then(|index| index.listed(&account_id, &folder))
        {
            if !(unread_only || flagged_only) {
                return Ok(MessagePage::whole(items));
            }
            // 「所有未读 / 已标星」要跟侧栏的实时未读数对得上：先用索引状态
            // 圈出候选（通常几百封），再用服务器最新标记确认一次，不必为整个
            // 文件夹（几万封）拉一遍标记；服务器取不到就先用索引状态。
            let mut candidates: Vec<Summary> = items
                .into_iter()
                .filter(|m| if unread_only { !m.seen } else { m.flagged })
                .collect();
            let uids: Vec<u32> = candidates.iter().map(|m| m.uid).collect();
            if !uids.is_empty() {
                let path = folder.clone();
                let flags = state
                    .with_cmd(&account_id, |c, _| {
                        Box::pin(async move {
                            c.select(&path).await?;
                            imap::fetch_flags(&mut c.s, &imap::uid_set(&uids)).await
                        })
                    })
                    .await;
                if let Ok(flags) = flags {
                    let updates: Vec<(u32, bool, bool)> =
                        flags.iter().map(|f| (f.uid, f.seen, f.flagged)).collect();
                    state.index_flags(&account_id, &folder, &updates);
                    let by_uid: HashMap<u32, imap::FlagState> =
                        flags.into_iter().map(|f| (f.uid, f)).collect();
                    candidates.retain(|m| {
                        by_uid
                            .get(&m.uid)
                            .is_some_and(|f| if unread_only { !f.seen } else { f.flagged })
                    });
                }
            }
            return Ok(MessagePage::whole(candidates));
        }
    }
    // 这个文件夹还没建过摘要索引：后台补一份（只摘要），下次打开就能读本地
    if before.is_none() {
        state.ensure_list_index(&app, &account_id, &folder);
    }
    // 只看星标、只看未读都在服务器上搜索，不局限于已加载的一页
    let criteria = if flagged_only {
        "FLAGGED"
    } else if unread_only {
        "UNSEEN"
    } else {
        ""
    };
    // 走到这里说明本地索引还没法直接列出这个文件夹：第一页先只取 40 封，
    // 界面不用为了一整页摘要等十几秒；后台摘要索引建好后会自动换成完整列表。
    let page = if before.is_none() { 40 } else { imap::PAGE };
    state
        .with_cmd(&account_id, |c, acct| {
            Box::pin(async move {
                c.select(&folder).await?;
                let (uids, more) =
                    imap::page_uids_matching_limited(&mut c.s, criteria, before, page).await?;
                if uids.is_empty() {
                    return Ok((Vec::new(), false));
                }
                let items =
                    imap::fetch_summaries(&mut c.s, &imap::uid_set(&uids), &acct.id, &folder)
                        .await?;
                Ok((items, more))
            })
        })
        .await
        .map(|(items, more)| MessagePage::partial(items, more))
        .map_err(err)
}

/// 搜索当前文件夹：IMAP 账号查本地搜索索引，POP3 账号走原有的整箱扫描。
///
/// 服务器 IMAP SEARCH 的文本条件不可靠：腾讯企业邮 / QQ 会忽略所有文本条件、
/// 直接返回整个文件夹（实测用不存在的关键词也能拿到全部 UID）；Coremail 虽支持
/// `CHARSET UTF-8`，但一次全文搜索要几十秒。所以文本搜索不再发给服务器，
/// 由后台任务把摘要和正文开头增量索引到本地，搜索时只查 local.db。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SearchResult {
    items: Vec<Summary>,
    /// 本地索引进度；POP3 不需要索引，返回 null
    indexing: Option<search_index::Progress>,
}

#[tauri::command]
async fn search_messages(
    state: State<'_, AppState>,
    app: AppHandle,
    account_id: String,
    folder: String,
    query: String,
) -> CmdResult<SearchResult> {
    let query = query.trim().to_string();
    if query.is_empty() {
        cancel_search(state.clone(), account_id.clone());
        return Ok(SearchResult {
            items: list_messages(state, app.clone(), account_id, folder, None, None, None)
                .await?
                .items,
            indexing: None,
        });
    }
    if folder == outbox::OUTBOX {
        // 定时发送不在任何 IMAP 文件夹里，直接在本地这份列表上过滤
        let items = list_messages(state, app.clone(), account_id, folder, None, None, None)
            .await?
            .items;
        let needle = query.to_lowercase();
        let items = items
            .into_iter()
            .filter(|m| {
                [&m.subject, &m.from_name, &m.from_addr, &m.to]
                    .iter()
                    .any(|text| text.to_lowercase().contains(&needle))
            })
            .collect();
        return Ok(SearchResult {
            items,
            indexing: None,
        });
    }
    if state.is_pop3(&account_id).map_err(err)? {
        // 先领取代次再排队等锁：排队期间清空关键词也能取消这次搜索。
        let generation = state.pop3_mailbox(&account_id).begin_search();
        let session = state.pop3_session(&account_id).await.map_err(err)?;
        let mut result = session.search(&query, generation).await.map_err(err)?;
        result.sort_by(|a, b| b.date.cmp(&a.date).then_with(|| b.uid.cmp(&a.uid)));
        return Ok(SearchResult {
            items: result,
            indexing: None,
        });
    }
    let indexing = state
        .ensure_search_index(&app, &account_id, &folder)
        .map_err(err)?;
    let mut items = state
        .search_index()
        .map_err(err)?
        .search(&account_id, &folder, &query)
        .map_err(err)?;
    if !items.is_empty() {
        // 索引里的已读/星标可能是旧的：结果通常不多，用服务器最新状态修正一次
        let uids: Vec<u32> = items.iter().map(|m| m.uid).collect();
        let flags = state
            .with_cmd(&account_id, |c, _| {
                Box::pin(async move {
                    c.select(&folder).await?;
                    imap::fetch_flags(&mut c.s, &imap::uid_set(&uids)).await
                })
            })
            .await;
        if let Ok(flags) = flags {
            let by_uid: HashMap<u32, imap::FlagState> =
                flags.into_iter().map(|f| (f.uid, f)).collect();
            for m in &mut items {
                if let Some(f) = by_uid.get(&m.uid) {
                    m.seen = f.seen;
                    m.flagged = f.flagged;
                }
            }
        }
    }
    Ok(SearchResult {
        items,
        indexing: Some(indexing),
    })
}

/// 停止该账号正在进行的 POP3 搜索（清空关键词或退出搜索时调用）。
/// 「收取」：让这个账号的收信连接立刻检查一次新邮件，返回收到几封。
/// 新邮件照常通过 "mail" 事件推给界面（和后台收到的一样提醒、执行收信规则）。
#[tauri::command]
async fn check_mail(state: State<'_, AppState>, account_id: String) -> CmdResult<usize> {
    // POP3 每次都要重新登录、IMAP 可能要等退出 IDLE，多给些时间
    state.checks.ask(&account_id, OP_TIMEOUT * 3).await
}

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

/// SELECT 文件夹并拿到它的 UIDVALIDITY。
async fn select_validity<T>(session: &mut async_imap::Session<T>, folder: &str) -> Result<u32>
where
    T: lanes::Stream,
{
    use download_protocol::{ArchiveData, checked_command, quoted_mailbox};
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(session, &format!("SELECT {}", quoted_mailbox(folder)?)),
    )
    .await
    .context("打开文件夹超时")??;
    responses
        .into_iter()
        .filter_map(|response| match response {
            ArchiveData::Validity(value) if value > 0 => Some(value),
            _ => None,
        })
        .last()
        .ok_or_else(|| anyhow!("服务器未提供 UIDVALIDITY，无法确认邮件身份"))
}

/// 先查内存，再查磁盘（磁盘上的读出来放进内存）。
async fn cached_body(cache: &Mutex<BodyCache>, key: &MsgKey) -> Option<CachedBody> {
    let (cached, disk) = {
        let cache = cache.lock().unwrap();
        (cache.get(key), cache.disk.clone())
    };
    if cached.is_some() {
        return cached;
    }
    let (account, folder, validity, uid) = key.clone();
    let from_disk =
        tokio::task::spawn_blocking(move || disk?.get(&account, &folder, validity, uid))
            .await
            .ok()
            .flatten();
    // 磁盘缓存里没有：看看下载目录里有没有（只读；只放进内存缓存，不占磁盘缓存）
    let (raw, date) = match from_disk {
        Some(found) => found,
        None => {
            let archives = cache.lock().unwrap().archives.clone()?;
            let (account, folder, validity, uid) = key.clone();
            archives
                .within(&account.clone(), ARCHIVE_READ_TIMEOUT, move |a| {
                    a.read(&account, &folder, validity, uid)
                })
                .await?
        }
    };
    let raw = Arc::new(raw);
    cache.lock().unwrap().insert(key.clone(), raw.clone(), date);
    Some((raw, date))
}

/// 下载目录可能在移动硬盘或网盘上：读得太慢就当没有，去服务器取
const ARCHIVE_READ_TIMEOUT: Duration = Duration::from_secs(2);
/// 设置页列目录、手动添加目录时最多等多久
const ARCHIVE_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// 下载整封原文（已 SELECT 好文件夹），存进内存和磁盘缓存。
async fn download_body<T>(
    session: &mut async_imap::Session<T>,
    cache: &Mutex<BodyCache>,
    key: MsgKey,
    peek: bool,
) -> Result<CachedBody>
where
    T: lanes::Stream,
{
    use download_protocol::{ArchiveData, checked_command};
    let uid = key.3;
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
                ..
            } if found == uid => Some((raw, date)),
            _ => None,
        })
        .ok_or_else(|| anyhow!("服务器没有返回邮件原文，邮件可能已在别处删除，请刷新后重试"))?;
    let raw = Arc::new(raw);
    let disk = cache.lock().unwrap().disk.clone();
    if let Some(disk) = disk {
        let (account, folder, validity, uid) = key.clone();
        let raw = raw.clone();
        // 后台写盘，不让用户等（偶尔还要清理旧缓存）
        tokio::task::spawn_blocking(move || disk.put(&account, &folder, validity, uid, &raw, date));
    }
    cache.lock().unwrap().insert(key, raw.clone(), date);
    Ok((raw, date))
}

/// 邮件大小（RFC822.SIZE）；服务器没给就是 None。
async fn message_size<T>(session: &mut async_imap::Session<T>, uid: u32) -> Result<Option<u32>>
where
    T: lanes::Stream,
{
    use download_protocol::{ArchiveData, checked_command};
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(session, &format!("UID FETCH {uid} (UID RFC822.SIZE)")),
    )
    .await
    .context("查询邮件大小超时")??;
    Ok(responses.into_iter().find_map(|response| match response {
        ArchiveData::Message {
            uid: Some(found),
            size,
            ..
        } if found == uid => size,
        _ => None,
    }))
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
    T: lanes::Stream,
{
    use download_protocol::checked_command;
    if uid == 0 {
        bail!("邮件位置无效，请刷新后重试");
    }
    let validity = select_validity(session, folder).await?;
    if expected_validity.is_some_and(|expected| expected != validity) {
        bail!("原邮件文件夹已发生变化，请重新打开原邮件后重试");
    }
    let key = (account_id.to_string(), folder.to_string(), validity, uid);
    if let Some((raw, date)) = cached_body(cache, &key).await {
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
    let (raw, date) = download_body(session, cache, key, peek).await?;
    Ok((raw, date, validity))
}

/// 多大的邮件才先出文字：小邮件整封下载也很快
const TEXT_FIRST_MIN: u32 = 256 * 1024;

/// 打开邮件过程中要告诉界面的事
enum Opened {
    /// 整封邮件的大小（进度的总量）
    Size(u32),
    /// 大邮件先出的文字版
    Partial(Box<MessageView>),
}

/// 先出文字这一步失败了（服务器不支持、回包解析不了、超时）：这个账号以后不再先出文字
#[derive(Debug)]
struct TextFirstFailed;

impl std::fmt::Display for TextFirstFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("先出文字失败")
    }
}

impl std::error::Error for TextFirstFailed {}

/// 查邮件大小和结构；大邮件（多是内嵌大图）只取正文那一部分，拼成一封没有图片和附件的
/// 邮件先给界面显示。返回（大小，文字版）。已 SELECT 好文件夹。
async fn text_first_view<T>(
    session: &mut async_imap::Session<T>,
    uid: u32,
    validity: u32,
) -> Result<(Option<u32>, Option<MessageView>)>
where
    T: lanes::Stream,
{
    use async_imap::imap_proto::{MessageSection, SectionPath};
    use futures::TryStreamExt;
    let fetches: Vec<_> = timeout(OP_TIMEOUT, async {
        session
            .uid_fetch(
                uid.to_string(),
                "(UID RFC822.SIZE INTERNALDATE BODYSTRUCTURE)",
            )
            .await?
            .try_collect()
            .await
    })
    .await
    .context("查询邮件结构超时")??;
    // async-imap 不检查结束状态：服务器回 NO/BAD 时这里是空的
    let Some(meta) = fetches.iter().find(|f| f.uid == Some(uid)) else {
        bail!("服务器没有返回邮件结构");
    };
    let size = meta.size;
    let date = meta
        .internal_date()
        .map(|d| d.timestamp_millis())
        .unwrap_or(0);
    let Some((path, octets)) = meta.bodystructure().and_then(imap::text_part) else {
        return Ok((size, None));
    };
    // 小邮件整封也很快；正文本身就占了大半封的，单独取也省不了多少
    let total = size.unwrap_or(0);
    if total <= TEXT_FIRST_MIN || octets > total / 2 {
        return Ok((size, None));
    }
    let section = path
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(".");
    let fetches: Vec<_> = timeout(OP_TIMEOUT, async {
        session
            .uid_fetch(
                uid.to_string(),
                format!("(UID BODY.PEEK[HEADER] BODY.PEEK[{section}.MIME] BODY.PEEK[{section}])"),
            )
            .await?
            .try_collect()
            .await
    })
    .await
    .context("下载正文超时")??;
    let Some(parts) = fetches.iter().find(|f| f.uid == Some(uid)) else {
        return Ok((size, None));
    };
    let (Some(header), Some(mime), Some(body)) = (
        parts.header(),
        parts.section(&SectionPath::Part(path.clone(), Some(MessageSection::Mime))),
        parts.section(&SectionPath::Part(path, None)),
    ) else {
        return Ok((size, None));
    };
    let raw = imap::assemble_text_message(header, mime, body);
    let mut view = imap::parse_message(&raw, date)?;
    view.uid_validity = Some(validity);
    view.partial = true;
    view.size = size;
    Ok((size, Some(view)))
}

/// 用户点开邮件时取原文（只读不标，PEEK）。和 read_raw 一样先查缓存；
/// 要下载时先报大小、大邮件先出文字（`text_first`），再下整封。
async fn read_opened<T>(
    session: &mut async_imap::Session<T>,
    cache: &Mutex<BodyCache>,
    key: (&str, &str, u32),
    text_first: bool,
    emit: impl Fn(Opened),
) -> Result<(Arc<Vec<u8>>, i64, u32)>
where
    T: lanes::Stream,
{
    let (account_id, folder, uid) = key;
    if uid == 0 {
        bail!("邮件位置无效，请刷新后重试");
    }
    let validity = select_validity(session, folder).await?;
    let key = (account_id.to_string(), folder.to_string(), validity, uid);
    if let Some((raw, date)) = cached_body(cache, &key).await {
        return Ok((raw, date, validity));
    }
    if text_first {
        let (size, view) = text_first_view(session, uid, validity)
            .await
            .map_err(|e| e.context(TextFirstFailed))?;
        if let Some(size) = size {
            emit(Opened::Size(size));
        }
        if let Some(view) = view {
            emit(Opened::Partial(Box::new(view)));
        }
    } else if let Some(size) = message_size(session, uid).await? {
        emit(Opened::Size(size));
    }
    let (raw, date) = download_body(session, cache, key, true).await?;
    Ok((raw, date, validity))
}

/// 预取：不大的邮件先下到缓存里（不标已读），返回这次有没有下载。
async fn prefetch_raw<T>(
    session: &mut async_imap::Session<T>,
    cache: &Mutex<BodyCache>,
    account_id: &str,
    folder: &str,
    uid: u32,
    max_size: u32,
) -> Result<bool>
where
    T: lanes::Stream,
{
    if uid == 0 {
        return Ok(false);
    }
    let validity = select_validity(session, folder).await?;
    let key = (account_id.to_string(), folder.to_string(), validity, uid);
    if cached_body(cache, &key).await.is_some() {
        return Ok(false);
    }
    if message_size(session, uid)
        .await?
        .is_none_or(|size| size > max_size)
    {
        return Ok(false);
    }
    download_body(session, cache, key, true).await?;
    Ok(true)
}

/// 已经点开了别的邮件，排队中的这封不用再下
#[derive(Debug)]
struct Superseded;

impl std::fmt::Display for Superseded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("已打开别的邮件")
    }
}

impl std::error::Error for Superseded {}

/// 用户点开的这封邮件：打开序号，以及给界面推进度和文字版用的句柄
struct Opening {
    turn: u64,
    app: AppHandle,
}

/// 在读信连接上取原文（引用、转发、附件用）。
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
        .with_reader(account_id, |c, acct| {
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

/// 用户点开的邮件：在读信连接上取原文，推送下载进度和大邮件的文字版。
/// 排到时用户已经点了别的邮件就不下载（Superseded）。
async fn open_raw(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    open: &Opening,
    text_first: bool,
) -> Result<(Arc<Vec<u8>>, i64, u32)> {
    let folder = folder.to_string();
    let cache = state.bodies.clone();
    let turns = state.turns.clone();
    let (turn, app) = (open.turn, open.app.clone());
    state
        .with_reader(account_id, |c, acct| {
            Box::pin(async move {
                if !turns.is_current(&acct.id, turn) {
                    // 不报错：报错会丢掉这条连接
                    return Ok(None);
                }
                c.selected = None;
                let received = c.s.get_ref().received();
                let total = Mutex::new(None);
                let emit = |opened| match opened {
                    Opened::Size(size) => *total.lock().unwrap() = Some(size),
                    Opened::Partial(view) => sync::emit(
                        &app,
                        Event::MessagePartial {
                            account_id: &acct.id,
                            folder: &folder,
                            uid,
                            view: &view,
                        },
                    ),
                };
                let report = |received| {
                    sync::emit(
                        &app,
                        Event::MessageProgress {
                            account_id: &acct.id,
                            folder: &folder,
                            uid,
                            received,
                            total: *total.lock().unwrap(),
                        },
                    )
                };
                let read =
                    read_opened(&mut c.s, &cache, (&acct.id, &folder, uid), text_first, emit);
                let result = lanes::with_progress(received, report, read).await?;
                c.selected = Some(folder);
                Ok(Some(result))
            })
        })
        .await?
        .ok_or_else(|| Superseded.into())
}

/// 保存账号的邮件缓存设置（总量、预热单封上限、是否预热），并按新设置重新预热。
#[tauri::command]
fn set_body_cache(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    settings: config::BodyCacheSettings,
) -> CmdResult<Account> {
    let acct = state
        .store
        .lock()
        .unwrap()
        .set_body_cache(&account_id, settings)
        .map_err(err)?;
    state.start_cache_warm(&app, &account_id, Duration::from_secs(3), false);
    Ok(acct)
}

/// 「立即更新缓存」：马上把收件箱里还没缓存的邮件补齐。
#[tauri::command]
async fn refresh_body_cache(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<cache_warm::Progress> {
    if state.is_pop3(&account_id).map_err(err)? {
        return Err("POP3 账号不缓存邮件原文".into());
    }
    state.start_cache_warm(&app, &account_id, Duration::ZERO, true);
    body_cache_status(state, account_id).await
}

/// 当缓存用的下载目录（设置页「邮件缓存」里列出来）
#[tauri::command]
async fn archive_status(
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Vec<archive_source::Location>> {
    let acct = state
        .store
        .lock()
        .unwrap()
        .get(&account_id)
        .cloned()
        .ok_or_else(|| "账号不存在".to_string())?;
    // 目录卡住（网盘、睡着的移动硬盘）时不等它：照样列出来，标成不在线
    let listed = acct.clone();
    match timeout(
        ARCHIVE_CHECK_TIMEOUT,
        tokio::task::spawn_blocking(move || archive_source::locations(&acct)),
    )
    .await
    {
        Ok(rows) => rows.map_err(|e| e.to_string()),
        Err(_) => Ok(archive_source::locations_offline(&listed)),
    }
}

/// 手动添加一个以前全量下载过的目录（只读使用）
#[tauri::command]
async fn add_archive_dir(
    state: State<'_, AppState>,
    account_id: String,
    directory: String,
) -> CmdResult<Vec<archive_source::Location>> {
    if state.is_pop3(&account_id).map_err(err)? {
        return Err("POP3 账号不使用已下载的邮件".into());
    }
    let id = account_id.clone();
    let parent = timeout(
        ARCHIVE_CHECK_TIMEOUT,
        tokio::task::spawn_blocking(move || archive_source::check_bulk_parent(&directory, &id)),
    )
    .await
    .map_err(|_| "读取这个目录太慢，请确认磁盘已连接后重试".to_string())?
    .map_err(|e| e.to_string())?
    .map_err(err)?;
    state
        .store
        .lock()
        .unwrap()
        .add_archive_dir(&account_id, &parent.display().to_string())
        .map_err(err)?;
    state.refresh_archives(&account_id);
    archive_status(state, account_id).await
}

/// 不再把这个目录当缓存用（不动里面的文件）
#[tauri::command]
async fn remove_archive_dir(
    state: State<'_, AppState>,
    account_id: String,
    path: String,
) -> CmdResult<Vec<archive_source::Location>> {
    state
        .store
        .lock()
        .unwrap()
        .remove_archive_dir(&account_id, &path)
        .map_err(err)?;
    state.refresh_archives(&account_id);
    archive_status(state, account_id).await
}

/// 缓存占用和预热进度（设置页打开时显示）
#[tauri::command]
async fn body_cache_status(
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<cache_warm::Progress> {
    let acct = state
        .store
        .lock()
        .unwrap()
        .get(&account_id)
        .cloned()
        .ok_or_else(|| "账号不存在".to_string())?;
    let store = state
        .body_store()
        .ok_or_else(|| "邮件缓存不可用".to_string())?;
    let progress = state
        .cache_warm
        .lock()
        .unwrap()
        .get(&account_id)
        .map(|job| job.progress.clone())
        .unwrap_or_else(|| cache_warm::Progress {
            state: "off".into(),
            ..Default::default()
        });
    // 第一次要扫描缓存目录：放到阻塞线程里
    let limit = acct.body_cache.limit_bytes();
    tokio::task::spawn_blocking(move || progress.with_usage(&store, &account_id, limit))
        .await
        .map_err(|e| e.to_string())
}

/// 预取多大以内的邮件：读信连接一次只能下一封，太大的会挡住用户接着点开的邮件
const PREFETCH_MAX: u32 = 512 * 1024;

/// 用户看完一封后，把列表里的下一封先下到缓存里（不标已读）。
/// 用户已经点开了别的邮件，就不再预取。
#[tauri::command]
async fn prefetch_message(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
) -> CmdResult<bool> {
    if folder == drafts::LOCAL_DRAFTS
        || folder == outbox::OUTBOX
        || state.is_pop3(&account_id).map_err(err)?
    {
        return Ok(false);
    }
    // 读信连接开不出来时预取会占用命令连接，挡住翻页和摘要，不如不预取
    if !state.lanes.reader_available(&account_id) {
        return Ok(false);
    }
    let turn = state.turns.current(&account_id);
    let cache = state.bodies.clone();
    let turns = state.turns.clone();
    state
        .with_reader(&account_id, |c, acct| {
            Box::pin(async move {
                if !turns.is_current(&acct.id, turn) {
                    return Ok(false);
                }
                c.selected = None;
                let done =
                    prefetch_raw(&mut c.s, &cache, &acct.id, &folder, uid, PREFETCH_MAX).await?;
                c.selected = Some(folder);
                Ok(done)
            })
        })
        .await
        .map_err(err)
}

/// 打开一封邮件（同时标记为已读）
#[tauri::command]
async fn get_message(
    app: AppHandle,
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
    // 原文在读信连接上只读不标（PEEK）；已读标记现在就在命令连接上排队，
    // 之后用户点「标为未读」一定排在它后面，不会被还在下载的这封邮件改回已读。
    // 标记放到后台任务里，不用等它（它可能排在一批摘要后面）。
    {
        let (app, account_id, folder) = (app.clone(), account_id.clone(), folder.clone());
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            if let Err(e) = store_flag(&state, &account_id, folder, &[uid], "\\Seen", true).await {
                eprintln!("标记已读失败：{e:#}");
            }
        });
    }
    // 原文已经缓存在本地（看过或后台预热过）而且这次会话里确认过 UIDVALIDITY：
    // 直接读缓存出邮件，不再等读信连接的 SELECT/重连。校验放到后台，文件夹
    // 真的重建过时把这一代的旧缓存丢掉，下次打开自然重新下载。
    if let Some(validity) = state.known_body_validity(&account_id, &folder) {
        let cache = state.bodies.clone();
        let key = (account_id.clone(), folder.clone(), validity, uid);
        if let Some((raw, date)) = cached_body(&cache, &key).await {
            let mut view = imap::parse_message(&raw, date).map_err(err)?;
            view.uid_validity = Some(validity);
            // 请求已读回执的邮件先不查服务器的 $MDNSent（那要再连一次）；用户
            // 点了「发送回执」也会再确认一次，不会因为缓存路径重复发送。
            state.record_sender(&account_id, &folder, uid, &view.from.name, &view.from.addr);
            let (app, account_id, folder) = (app.clone(), account_id.clone(), folder.clone());
            tauri::async_runtime::spawn(async move {
                let state = app.state::<AppState>();
                let f = folder.clone();
                let current = state
                    .with_reader(&account_id, |c, _| {
                        Box::pin(async move { select_validity(&mut c.s, &f).await })
                    })
                    .await;
                let Ok(current) = current else { return };
                state.remember_body_validity(&account_id, &folder, current);
                if current != validity {
                    eprintln!("文件夹 {folder} 的 UIDVALIDITY 变了，清掉旧的本地原文缓存");
                    state.bodies.lock().unwrap().retain(|((a, f, v, _), _)| {
                        !(a == &account_id && f == &folder && *v == validity)
                    });
                    if let Some(disk) = state.body_store() {
                        tokio::task::spawn_blocking(move || {
                            disk.remove(&account_id, &folder, None)
                        });
                    }
                }
            });
            return Ok(view);
        }
    }
    let open = Opening {
        turn: state.turns.next(&account_id),
        app: app.clone(),
    };
    let text_first = !state.no_text_first.lock().unwrap().contains(&account_id);
    let mut read = open_raw(&state, &account_id, &folder, uid, &open, text_first).await;
    // 先出文字只是锦上添花：失败了这次照常整封下载
    if let Err(e) = &read
        && e.downcast_ref::<TextFirstFailed>().is_some()
    {
        // 网络问题只是这一次没成，下次照样先出文字；服务器不支持、回包解析不了才不再试
        if !download_protocol::connection_problem(e) {
            eprintln!("账号 {account_id} 不再先出文字：{e:#}");
            state
                .no_text_first
                .lock()
                .unwrap()
                .insert(account_id.clone());
        }
        read = open_raw(&state, &account_id, &folder, uid, &open, false).await;
    }
    let (raw, date, validity) = read.map_err(err)?;
    state.remember_body_validity(&account_id, &folder, validity);
    let mut view = imap::parse_message(&raw, date).map_err(err)?;
    view.uid_validity = Some(validity);
    state.record_sender(&account_id, &folder, uid, &view.from.name, &view.from.addr);
    // 请求了已读回执的邮件：看看是不是已经回过（别的客户端也会打 $MDNSent 标记）
    if view.receipt_to.is_some() {
        let f = folder.clone();
        view.receipt_handled = state
            .with_reader(&account_id, |c, _| {
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
    store_flag(&state, &account_id, folder, &uids, flag, value)
        .await
        .map_err(err)
}

/// 在命令连接上改标记。同一账号的标记操作按请求先后执行。
async fn store_flag(
    state: &AppState,
    account_id: &str,
    folder: String,
    uids: &[u32],
    flag: &'static str,
    value: bool,
) -> Result<()> {
    let path = folder.clone();
    let set = imap::uid_set(uids);
    state
        .with_cmd(account_id, |c, _| {
            Box::pin(async move {
                c.select(&path).await?;
                let op = format!("{}FLAGS.SILENT ({flag})", if value { "+" } else { "-" });
                timeout(
                    OP_TIMEOUT,
                    download_protocol::checked_command(&mut c.s, &format!("UID STORE {set} {op}")),
                )
                .await??;
                Ok(())
            })
        })
        .await?;
    // 列表和搜索结果直接读本地索引，标记变了要马上写回去
    state.index_set_flags(
        account_id,
        &folder,
        uids,
        (flag == "\\Seen").then_some(value),
        (flag == "\\Flagged").then_some(value),
    );
    Ok(())
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
    app: AppHandle,
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
    // 本地索引里的列表立即去掉移走的邮件（目标文件夹不一定已经建过索引）
    state.index_remove(&account_id, &folder, &uids);

    let disk = {
        let mut bodies = state.bodies.lock().unwrap();
        bodies.retain(|((account, path, _, uid), _)| {
            account != &account_id
                || !((imap::mailbox_eq(path, &folder) && uids.contains(uid))
                    || target.as_deref().is_some_and(|t| imap::mailbox_eq(path, t)))
        });
        bodies.disk.clone()
    };
    // 磁盘上按 UIDVALIDITY+UID 存，目标文件夹里原有的不会被新邮件冒用，只删移走的
    if let Some(disk) = disk {
        disk.remove(&account_id, &folder, Some(&uids));
    }
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
        if target_inbox {
            state.refresh_index_soon(&app, &account_id, "INBOX");
        }
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
                // 首屏里可能有刚从别的文件夹移进来的邮件：一并写进本地索引，
                // 下次打开收件箱（列表读本地）就能看到
                if let Ok(index) = state.search_index() {
                    if let Ok(Some(validity)) = index.folder_validity(account_id, "INBOX") {
                        state.index_upsert(account_id, "INBOX", validity, &inbox);
                    }
                }
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
                    done.read,
                    done.flagged,
                ))
            })
        })
        .await
        .map_err(err)?;
    let (report, moved, read, flagged) = report;
    // 列表和搜索结果直接读本地索引：规则移走的要去掉，规则改过的标记要写回
    state.index_remove(
        &account_id,
        "INBOX",
        &moved.iter().map(|m| m.uid).collect::<Vec<_>>(),
    );
    state.index_set_flags(&account_id, "INBOX", &read, Some(true), None);
    state.index_set_flags(&account_id, "INBOX", &flagged, None, Some(true));
    // 用户再把它们移回收件箱时，同步连接上的规则不再动它们
    state.rule_memory(&account_id, |m| m.moved(&moved));
    if report.matched > 0 {
        let disk = {
            let mut bodies = state.bodies.lock().unwrap();
            bodies.retain(|((account, folder, _, _), _)| {
                account != &account_id || !imap::mailbox_eq(folder, "INBOX")
            });
            bodies.disk.clone()
        };
        if let Some(disk) = disk {
            disk.remove(
                &account_id,
                "INBOX",
                Some(&moved.iter().map(|m| m.uid).collect::<Vec<_>>()),
            );
        }
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

/// 附件缓存文件名：去掉路径分隔符和控制字符，限制长度。
fn safe_file_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return "附件".to_string();
    }
    // APFS 单个文件名上限 255 字节，超长时保留扩展名截断（中文按 UTF-8 字节算）
    fit_bytes(clean, 200)
}

/// 按 UTF-8 字节数截断文件名，尽量保留扩展名。
fn fit_bytes(name: &str, limit: usize) -> String {
    if name.len() <= limit {
        return name.to_string();
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 16 => (stem, format!(".{ext}")),
        _ => (name, String::new()),
    };
    let budget = limit.saturating_sub(ext.len());
    let mut out = String::new();
    for c in stem.chars() {
        if out.len() + c.len_utf8() > budget {
            break;
        }
        out.push(c);
    }
    out.push_str(&ext);
    out
}

/// 用系统默认方式打开文件；没有默认程序时让 macOS 弹出「未设定用来打开文稿…」的
/// 选择应用对话框（和 Finder 里双击同一个链路）。
#[cfg(target_os = "macos")]
fn open_with_system(path: &std::path::Path) -> Result<(), String> {
    use std::ffi::c_void;
    type CFTypeRef = *const c_void;

    #[repr(C, packed(2))]
    struct LSLaunchURLSpec {
        app_url: CFTypeRef,
        item_urls: CFTypeRef,
        pass_thru_params: CFTypeRef,
        launch_flags: u32,
        async_ref_con: CFTypeRef,
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFTypeRef,
            buffer: *const u8,
            buf_len: isize,
            is_directory: u8,
        ) -> CFTypeRef;
        fn CFArrayCreate(
            allocator: CFTypeRef,
            values: *const CFTypeRef,
            num_values: isize,
            callbacks: CFTypeRef,
        ) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSOpenFromURLSpec(spec: *const LSLaunchURLSpec, launched: *mut CFTypeRef) -> i32;
    }

    // kLSApplicationNotFoundErr：已经弹过「选择应用程序」对话框，不需要再报错
    const LS_APPLICATION_NOT_FOUND: i32 = -10814;
    const K_LS_LAUNCH_DEFAULTS: u32 = 0x0000_0001;
    const K_LS_LAUNCH_AND_DISPLAY_ERRORS: u32 = 0x0000_0040;

    use std::os::unix::ffi::OsStrExt;
    let bytes = path.as_os_str().as_bytes();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        );
        if url.is_null() {
            return Err("无法生成附件路径".into());
        }
        let items = CFArrayCreate(std::ptr::null(), &url, 1, std::ptr::null());
        if items.is_null() {
            CFRelease(url);
            return Err("无法生成打开请求".into());
        }
        let spec = LSLaunchURLSpec {
            app_url: std::ptr::null(),
            item_urls: items,
            pass_thru_params: std::ptr::null(),
            launch_flags: K_LS_LAUNCH_DEFAULTS | K_LS_LAUNCH_AND_DISPLAY_ERRORS,
            async_ref_con: std::ptr::null(),
        };
        let mut launched: CFTypeRef = std::ptr::null();
        let status = LSOpenFromURLSpec(&spec, &mut launched);
        if !launched.is_null() {
            CFRelease(launched);
        }
        CFRelease(items);
        CFRelease(url);
        if status == 0 || status == LS_APPLICATION_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("系统无法打开这个文件（错误码 {status}）"))
        }
    }
}

/// 应用选择器返回的路径必须是真实存在的 `.app` 目录。
fn is_mac_app(path: &std::path::Path) -> bool {
    path.is_dir()
        && path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// 出现在「打开方式」里的应用。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenWithApp {
    name: String,
    path: String,
}

fn open_with_app(path: &std::path::Path) -> Option<OpenWithApp> {
    let name = path.file_stem()?.to_string_lossy().to_string();
    if name.is_empty() {
        return None;
    }
    Some(OpenWithApp {
        name,
        path: path.display().to_string(),
    })
}

/// macOS 的 LaunchServices：系统认为能打开这个文件的 App（等同 Finder 的「打开方式」推荐）。
#[cfg(target_os = "macos")]
fn recommended_apps(file: &std::path::Path) -> Vec<OpenWithApp> {
    use std::ffi::{CStr, c_void};
    type CFTypeRef = *const c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFURLCreateFromFileSystemRepresentation(
            allocator: CFTypeRef,
            buffer: *const u8,
            buf_len: isize,
            is_directory: u8,
        ) -> CFTypeRef;
        fn CFURLGetFileSystemRepresentation(
            url: CFTypeRef,
            resolve: u8,
            buffer: *mut u8,
            max_len: isize,
        ) -> u8;
        fn CFArrayGetCount(array: CFTypeRef) -> isize;
        fn CFArrayGetValueAtIndex(array: CFTypeRef, index: isize) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
    }
    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        /// kLSRolesAll = 0xFFFFFFFF
        fn LSCopyApplicationURLsForURL(url: CFTypeRef, roles: u32) -> CFTypeRef;
    }

    use std::os::unix::ffi::OsStrExt;
    let bytes = file.as_os_str().as_bytes();
    let mut out = Vec::new();
    unsafe {
        let url = CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        );
        if url.is_null() {
            return out;
        }
        let apps = LSCopyApplicationURLsForURL(url, 0xFFFF_FFFF);
        CFRelease(url);
        if apps.is_null() {
            return out;
        }
        for i in 0..CFArrayGetCount(apps) {
            let item = CFArrayGetValueAtIndex(apps, i);
            if item.is_null() {
                continue;
            }
            let mut buf = vec![0u8; 4096];
            if CFURLGetFileSystemRepresentation(item, 1, buf.as_mut_ptr(), buf.len() as isize) != 0
            {
                if let Ok(path) = CStr::from_bytes_until_nul(&buf) {
                    let path = std::path::PathBuf::from(
                        String::from_utf8_lossy(path.to_bytes()).to_string(),
                    );
                    if is_mac_app(&path) {
                        if let Some(app) = open_with_app(&path) {
                            out.push(app);
                        }
                    }
                }
            }
        }
        CFRelease(apps);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

#[cfg(not(target_os = "macos"))]
fn recommended_apps(_file: &std::path::Path) -> Vec<OpenWithApp> {
    Vec::new()
}

/// 本机安装的 App（未知类型没有系统推荐时给用户兜底）。
fn installed_apps() -> Vec<OpenWithApp> {
    let mut dirs: Vec<PathBuf> = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/System/Applications"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs.push(home.join("Applications"));
    }
    let mut out = Vec::new();
    for dir in dirs {
        collect_apps(&dir, 0, &mut out);
    }
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn collect_apps(dir: &std::path::Path, depth: usize, out: &mut Vec<OpenWithApp>) {
    if depth > 1 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if is_mac_app(&path) {
            if let Some(app) = open_with_app(&path) {
                out.push(app);
            }
        } else if depth < 1 {
            collect_apps(&path, depth + 1, out);
        }
    }
}

/// 把附件写进本地缓存并返回路径：同一封邮件的同一个附件反复打开时不再重复下载、不堆文件。
async fn attachment_path(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
) -> Result<PathBuf> {
    let raw = raw_at(state, account_id, folder, uid, uid_validity).await?;
    // 每封邮件的每个附件一个子目录：文件名保持邮件里的原名，靠目录区分不同邮件
    let dir = state
        .store
        .lock()
        .unwrap()
        .dir()
        .join("attachments")
        .join(format!("{uid}-{index}"));
    // MIME 解析和写盘都不占异步线程
    tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let (name, data) = imap::attachment_bytes(&raw, index)?;
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("创建附件缓存目录失败：{}", dir.display()))?;
        let path = dir.join(safe_file_name(&name));
        let unchanged = std::fs::metadata(&path)
            .map(|meta| meta.len() == data.len() as u64)
            .unwrap_or(false);
        if !unchanged {
            std::fs::write(&path, &data)
                .with_context(|| format!("写入附件失败：{}", path.display()))?;
        }
        Ok(path)
    })
    .await
    .map_err(|e| anyhow!("写入附件失败：{e}"))?
}

/// 用系统默认程序打开附件（附件会先缓存到本地）。
#[tauri::command]
async fn open_attachment(
    _app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
) -> CmdResult<String> {
    let path = attachment_path(&state, &account_id, &folder, uid, index, uid_validity)
        .await
        .map_err(err)?;
    // macOS 走 LaunchServices：没有默认程序时由系统弹「选择应用程序」对话框
    #[cfg(target_os = "macos")]
    {
        let open_path = path.clone();
        tokio::task::spawn_blocking(move || open_with_system(&open_path))
            .await
            .map_err(|e| format!("打开附件失败：{e}"))??;
    }
    #[cfg(not(target_os = "macos"))]
    _app.opener()
        .open_path(path.display().to_string(), None::<&str>)
        .map_err(|e| format!("打开附件失败：{e}"))?;
    Ok(path.display().to_string())
}

/// 「打开方式」列表：系统推荐优先，没有推荐时给已安装的 App 兜底。
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenWithChoices {
    path: String,
    recommended: Vec<OpenWithApp>,
    installed: Vec<OpenWithApp>,
}

#[tauri::command]
async fn attachment_open_with(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
) -> CmdResult<OpenWithChoices> {
    let path = attachment_path(&state, &account_id, &folder, uid, index, uid_validity)
        .await
        .map_err(err)?;
    // LaunchServices 查询可能要几十到几百毫秒，别占异步线程
    let probe = path.clone();
    let recommended = tokio::task::spawn_blocking(move || recommended_apps(&probe))
        .await
        .unwrap_or_default();
    // 系统有推荐应用时不用扫 /Applications；没有推荐（例如 .lic）才用兜底列表，
    // 兜底列表第一次算完会缓存在内存里
    let installed = if recommended.is_empty() {
        installed_apps_cached(&state).await
    } else {
        Vec::new()
    };
    Ok(OpenWithChoices {
        recommended,
        installed,
        path: path.display().to_string(),
    })
}

/// 本机安装的 App 列表（「打开方式」兜底）：第一次扫描后缓存在内存里。
async fn installed_apps_cached(state: &AppState) -> Vec<OpenWithApp> {
    if let Some(apps) = state.installed_apps.lock().unwrap().clone() {
        return apps;
    }
    let apps = tokio::task::spawn_blocking(installed_apps)
        .await
        .unwrap_or_default();
    *state.installed_apps.lock().unwrap() = Some(apps.clone());
    apps
}

/// 用指定的应用打开附件。
#[tauri::command]
async fn open_attachment_with_app(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
    app: String,
) -> CmdResult<()> {
    let path = attachment_path(&state, &account_id, &folder, uid, index, uid_validity)
        .await
        .map_err(err)?;
    let file = path.display().to_string();
    let app_path = PathBuf::from(&app);
    if !is_mac_app(&app_path) {
        return Err(format!("选择的不是 macOS 应用：{app}"));
    }
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let opened = std::process::Command::new("/usr/bin/open")
            .arg("-a")
            .arg(&app)
            .arg("--")
            .arg(&file)
            .output()
            .map_err(|e| format!("打开附件失败：{e}"))?;
        if !opened.status.success() {
            let detail = String::from_utf8_lossy(&opened.stderr).trim().to_string();
            return Err(format!("打开附件失败：{detail}"));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("打开附件失败：{e}"))??;
    Ok(())
}

/// 在文件管理器里显示附件（附件会先缓存到本地）。/// 在文件管理器里显示附件（附件会先缓存到本地）。
#[tauri::command]
async fn reveal_attachment(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    uid: u32,
    index: usize,
    uid_validity: Option<u32>,
) -> CmdResult<String> {
    let path = attachment_path(&state, &account_id, &folder, uid, index, uid_validity)
        .await
        .map_err(err)?;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| format!("在文件管理器中显示失败：{e}"))?;
    Ok(path.display().to_string())
}

/// 把附件写进「下载」：同名同内容的文件已经存在时直接复用，不再生成 (2)、(3)… 副本。
fn save_attachment_file(dir: PathBuf, name: &str, data: &[u8]) -> Result<PathBuf> {
    let name = safe_file_name(name);
    let existing = dir.join(&name);
    if let Ok(meta) = std::fs::metadata(&existing) {
        if meta.len() == data.len() as u64 && std::fs::read(&existing).is_ok_and(|old| old == data)
        {
            return Ok(existing);
        }
    }
    let path = unique_path(dir, &name);
    std::fs::write(&path, data).with_context(|| format!("保存失败：{}", path.display()))?;
    Ok(path)
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
    let path = save_attachment_file(dir, &name, &data).map_err(err)?;
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
/// 原文已经缓存在本地时直接读（附件、引用图片、下载 .eml 都不用先向服务器
/// 确认 UIDVALIDITY）。UI 传进来的、或这次会话里确认过的 UIDVALIDITY 为准。
async fn cached_raw(
    state: &AppState,
    account_id: &str,
    folder: &str,
    uid: u32,
    uid_validity: Option<u32>,
) -> Option<(Arc<Vec<u8>>, i64, u32)> {
    let validity = uid_validity
        .filter(|value| *value > 0)
        .or_else(|| state.known_body_validity(account_id, folder))?;
    let cache = state.bodies.clone();
    let key = (account_id.to_string(), folder.to_string(), validity, uid);
    let (raw, date) = cached_body(&cache, &key).await?;
    Some((raw, date, validity))
}

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
    // 已经缓存过就直接读本地：附件、引用图片这些操作不用再等一次 SELECT
    if let Some((raw, _, _)) = cached_raw(state, account_id, folder, uid, uid_validity).await {
        return Ok(raw);
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
            let bodies_dir = store.dir().join("bodies");
            // 每个账号的缓存上限要在第一次写入之前记上，不然会按默认上限删掉用户调大后存下的缓存
            let body_store = body_store::BodyStore::new(bodies_dir, body_store::DEFAULT_LIMIT);
            for a in &store.accounts {
                body_store.preset_limit(&a.id, a.body_cache.limit_bytes());
            }
            // 已下载的邮件（自动下载、全量下载）只读地当缓存用
            let archives = Arc::new(archive_source::ArchiveSource::default());
            for a in &store.accounts {
                archives.register(a);
            }
            let db = contacts::Db::open(&store.dir().join("local.db")).map_err(|e| {
                eprintln!("本地数据库打不开，通讯录暂不可用：{e:#}");
                format!("通讯录暂不可用：{e:#}")
            });
            let search_index = search_index::SearchIndex::open(&store.dir().join("local.db"))
                .map(Arc::new)
                .map_err(|e| {
                    eprintln!("搜索索引打不开，搜索暂不可用：{e:#}");
                    format!("搜索索引暂不可用：{e:#}")
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
                lanes: lanes::Lanes::default(),
                turns: Arc::default(),
                folders: Mutex::default(),
                live: Mutex::new(live),
                no_preview: Mutex::default(),
                no_text_first: Mutex::default(),
                bodies: Arc::new(Mutex::new(BodyCache {
                    archives: Some(archives),
                    ..BodyCache::with_disk(Arc::new(body_store))
                })),
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
                search_index,
                search_jobs: Arc::default(),
                search_failures: Arc::default(),
                list_index_jobs: Mutex::default(),
                body_validity: Mutex::default(),
                installed_apps: Mutex::default(),
                list_index_running: Arc::default(),
                cache_warm: Arc::default(),
                wake,
                checks: sync::CheckMail::default(),
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
            // 「打开方式」的兜底应用列表：启动后后台扫一遍，右键附件不用等
            {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    if let Some(state) = handle.try_state::<AppState>() {
                        let _ = installed_apps_cached(&state).await;
                    }
                });
            }
            let state = app.state::<AppState>();
            let index_accounts: Vec<String> = accounts
                .iter()
                .filter(|a| a.receive_protocol == config::ReceiveProtocol::Imap)
                .map(|a| a.id.clone())
                .collect();
            for a in accounts {
                if let Some(p) = passwords.remove(&a.id) {
                    state.start_watcher(app.handle(), a, p);
                } else {
                    state.start_downloader(app.handle(), a.id);
                }
            }
            // 后台把收件箱的搜索索引慢慢建起来：腾讯服务器对每封邮件都有固定延迟
            // （摘要约 0.15 秒、正文约 0.7 秒），大邮箱首次要跑不少时间，启动后就
            // 分阶段跑，界面不用等它；之后每次只补新增邮件。
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_secs(5)).await;
                for id in index_accounts {
                    let Some(state) = handle.try_state::<AppState>() else {
                        return;
                    };
                    if let Err(e) = state.ensure_search_index(&handle, &id, "INBOX") {
                        eprintln!("启动搜索索引失败：{e:#}");
                    }
                    // 启动阶段同步、摘要、索引都在跑：缓存预热一分钟后再开始
                    state.start_cache_warm(&handle, &id, Duration::from_secs(60), false);
                }
            });
            // 其它文件夹的摘要索引也在后台慢慢建，第一次点开时就不用等服务器分页
            state.start_header_index_sweep(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            snapshot,
            add_account,
            remove_account,
            save_signature,
            save_auto_download,
            choose_download_directory,
            archive_status,
            add_archive_dir,
            remove_archive_dir,
            start_bulk_download,
            cancel_bulk_download,
            list_folders,
            create_folder,
            list_messages,
            search_messages,
            cancel_search,
            check_mail,
            get_previews,
            get_message,
            prefetch_message,
            set_body_cache,
            refresh_body_cache,
            body_cache_status,
            save_message,
            set_flag,
            move_messages,
            save_attachment,
            open_attachment,
            attachment_open_with,
            open_attachment_with_app,
            reveal_attachment,
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
    async fn mail_read_before_a_restart_opens_from_disk_and_is_still_marked_seen() {
        let dir = tempfile::tempdir().unwrap();
        let disk = || {
            Arc::new(body_store::BodyStore::new(
                dir.path().to_path_buf(),
                body_store::DEFAULT_LIMIT,
            ))
        };
        let cache = Mutex::new(BodyCache::with_disk(disk()));
        let mut session = scripted_session(vec![
            selected(7),
            (body_response("big mail"), Some("OK fetched")),
        ])
        .await;
        read_raw(&mut session, &cache, "a", "INBOX", 1, true, None)
            .await
            .unwrap();
        // 写盘在后台进行
        for _ in 0..200 {
            if disk().get("a", "INBOX", 7, 1).is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // 重启：内存缓存是空的，磁盘上的还在。不能再 FETCH，但要补标已读
        let cache = Mutex::new(BodyCache::with_disk(disk()));
        let mut session =
            scripted_session(vec![selected(7), (String::new(), Some("OK seen"))]).await;
        let (raw, date, _) = read_raw(&mut session, &cache, "a", "INBOX", 1, false, None)
            .await
            .unwrap();
        assert_eq!(raw.as_slice(), b"big mail");
        assert_eq!(date, 1_735_787_045_000);
        assert!(
            cache
                .lock()
                .unwrap()
                .get(&("a".into(), "INBOX".into(), 7, 1))
                .is_some()
        );
        // 文件夹重建（UIDVALIDITY 变了）就不能再用
        let mut session = scripted_session(vec![
            selected(8),
            (body_response("other mail"), Some("OK fetched")),
        ])
        .await;
        let (raw, _, _) = read_raw(&mut session, &cache, "a", "INBOX", 1, true, None)
            .await
            .unwrap();
        assert_eq!(raw.as_slice(), b"other mail");
    }

    fn size_response(size: u32) -> (String, Option<&'static str>) {
        (
            format!("* 1 FETCH (UID 1 RFC822.SIZE {size})\r\n"),
            Some("OK sized"),
        )
    }

    #[tokio::test]
    async fn prefetch_downloads_small_mail_into_the_cache_without_marking_it_seen() {
        let cache = Mutex::new(BodyCache::default());
        let mut session = scripted_session(vec![
            selected(7),
            size_response(8),
            (body_response("next one"), Some("OK fetched")),
        ])
        .await;
        assert!(
            prefetch_raw(&mut session, &cache, "a", "INBOX", 1, 1024)
                .await
                .unwrap()
        );
        let (raw, _) = cache
            .lock()
            .unwrap()
            .get(&("a".into(), "INBOX".into(), 7, 1))
            .unwrap();
        assert_eq!(raw.as_slice(), b"next one");
        // 第二次：已经在缓存里，什么都不下
        let mut session = scripted_session(vec![selected(7)]).await;
        assert!(
            !prefetch_raw(&mut session, &cache, "a", "INBOX", 1, 1024)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn prefetch_skips_large_mail() {
        let cache = Mutex::new(BodyCache::default());
        // 只回 SELECT 和大小；再发 FETCH 原文就会断线报错
        let mut session = scripted_session(vec![selected(7), size_response(5_000_000)]).await;
        assert!(
            !prefetch_raw(&mut session, &cache, "a", "INBOX", 1, 1024)
                .await
                .unwrap()
        );
        assert!(cache.lock().unwrap().entries.is_empty());
    }

    /// 常见的带图通知邮件：正文 HTML + 文字版，后面跟着大图
    const RELATED: &str = r#"((("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "BASE64" 120 2 NIL NIL NIL)("TEXT" "HTML" ("CHARSET" "gbk") NIL NIL "BASE64" 2000 30 NIL NIL NIL) "ALTERNATIVE" ("BOUNDARY" "b2") NIL NIL)("IMAGE" "PNG" ("NAME" "a.png") "<a>" NIL "BASE64" 880000 NIL ("INLINE" ("FILENAME" "a.png")) NIL) "RELATED" ("BOUNDARY" "b1") NIL NIL)"#;

    async fn text_part_of(structure: &str) -> Option<(Vec<u32>, u32)> {
        use futures::TryStreamExt;
        let mut session = scripted_session(vec![(
            format!("* 1 FETCH (UID 1 BODYSTRUCTURE {structure})\r\n"),
            Some("OK fetched"),
        )])
        .await;
        let fetches: Vec<_> = session
            .uid_fetch("1", "(UID BODYSTRUCTURE)")
            .await
            .unwrap()
            .try_collect()
            .await
            .unwrap();
        imap::text_part(fetches[0].bodystructure().unwrap())
    }

    #[tokio::test]
    async fn the_html_body_part_is_found_inside_nested_multiparts() {
        assert_eq!(text_part_of(RELATED).await, Some((vec![1, 2], 2000)));
        // 带附件、只有纯文本
        let mixed = r#"(("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "7BIT" 300 10 NIL NIL NIL)("APPLICATION" "PDF" ("NAME" "a.pdf") NIL NIL "BASE64" 900000 NIL ("ATTACHMENT" ("FILENAME" "a.pdf")) NIL) "MIXED" ("BOUNDARY" "m") NIL NIL)"#;
        assert_eq!(text_part_of(mixed).await, Some((vec![1], 300)));
        // 作为附件的 HTML 不算正文
        // 没有 Content-Disposition、只带文件名的 HTML 也是附件
        let named = r#"(("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "7BIT" 300 10 NIL NIL NIL)("TEXT" "HTML" ("NAME" "report.html") NIL NIL "BASE64" 9000 10 NIL NIL NIL) "MIXED" ("BOUNDARY" "m") NIL NIL)"#;
        assert_eq!(text_part_of(named).await, Some((vec![1], 300)));
        let attached = r#"(("TEXT" "HTML" ("CHARSET" "utf-8") NIL NIL "7BIT" 300 10 NIL ("ATTACHMENT" ("FILENAME" "a.html")) NIL)("TEXT" "PLAIN" ("CHARSET" "utf-8") NIL NIL "7BIT" 50 2 NIL NIL NIL) "MIXED" ("BOUNDARY" "m") NIL NIL)"#;
        assert_eq!(text_part_of(attached).await, Some((vec![2], 50)));
        // 单部分邮件、只有附件：没有可单独取的正文
        let single =
            r#"("TEXT" "HTML" ("CHARSET" "utf-8") NIL NIL "BASE64" 900000 30 NIL NIL NIL)"#;
        assert_eq!(text_part_of(single).await, None);
        let only_files = r#"(("IMAGE" "PNG" NIL NIL NIL "BASE64" 900000 NIL NIL NIL) "MIXED" ("BOUNDARY" "m") NIL NIL)"#;
        assert_eq!(text_part_of(only_files).await, None);
    }

    fn structure_response(size: u32, structure: &str) -> (String, Option<&'static str>) {
        (
            format!(
                "* 1 FETCH (UID 1 RFC822.SIZE {size} INTERNALDATE \"02-Jan-2025 03:04:05 +0000\" BODYSTRUCTURE {structure})\r\n"
            ),
            Some("OK fetched"),
        )
    }

    /// 打开邮件，记下推给界面的东西：("size", 大小) 或 ("partial", 主题)
    async fn open_and_watch(
        replies: Vec<(String, Option<&'static str>)>,
        cache: &Mutex<BodyCache>,
        text_first: bool,
    ) -> (Result<(Arc<Vec<u8>>, i64, u32)>, Vec<String>) {
        let seen = Mutex::new(Vec::new());
        let mut session = scripted_session(replies).await;
        let r = read_opened(&mut session, cache, ("a", "INBOX", 1), text_first, |o| {
            seen.lock().unwrap().push(match o {
                Opened::Size(n) => format!("size {n}"),
                Opened::Partial(v) => {
                    assert!(v.partial);
                    assert_eq!(v.uid_validity, Some(7));
                    assert_eq!(v.date, 1_735_787_045_000);
                    format!(
                        "partial {} {:?} {}",
                        v.subject,
                        v.size,
                        v.html.unwrap().contains("正文")
                    )
                }
            })
        })
        .await;
        (r, seen.into_inner().unwrap())
    }

    #[tokio::test]
    async fn large_mail_shows_its_text_before_the_images_arrive() {
        let cache = Mutex::new(BodyCache::default());
        let header = "From: sender@example.com\r\nSubject: talk\r\nContent-Type: multipart/related; boundary=\"b1\"\r\n\r\n";
        let mime =
            "Content-Type: text/html; charset=gbk\r\nContent-Transfer-Encoding: base64\r\n\r\n";
        let body = "PHA+1f3OxDxpbWcgc3JjPSJjaWQ6YSI+PC9wPg==\r\n";
        let parts = format!(
            "* 1 FETCH (UID 1 BODY[HEADER] {{{}}}\r\n{header} BODY[1.2.MIME] {{{}}}\r\n{mime} BODY[1.2] {{{}}}\r\n{body})\r\n",
            header.len(),
            mime.len(),
            body.len()
        );
        let (r, seen) = open_and_watch(
            vec![
                selected(7),
                structure_response(900_000, RELATED),
                (parts, Some("OK fetched")),
                (body_response("whole mail"), Some("OK fetched")),
            ],
            &cache,
            true,
        )
        .await;
        assert_eq!(
            r.unwrap().0.as_slice(),
            b"whole mail",
            "文字版之后照常下整封"
        );
        assert_eq!(seen, ["size 900000", "partial talk Some(900000) true"]);
        assert!(
            cache
                .lock()
                .unwrap()
                .get(&("a".into(), "INBOX".into(), 7, 1))
                .is_some()
        );
    }

    #[tokio::test]
    async fn small_mail_only_reports_its_size_and_cached_mail_costs_one_select() {
        let cache = Mutex::new(BodyCache::default());
        let (r, seen) = open_and_watch(
            vec![
                selected(7),
                structure_response(40_000, RELATED),
                (body_response("small"), Some("OK fetched")),
            ],
            &cache,
            true,
        )
        .await;
        assert_eq!(r.unwrap().0.as_slice(), b"small");
        assert_eq!(seen, ["size 40000"]);
        // 已经下载过：只 SELECT 一次，什么都不推
        let (r, seen) = open_and_watch(vec![selected(7)], &cache, true).await;
        assert_eq!(r.unwrap().0.as_slice(), b"small");
        assert!(seen.is_empty());
    }

    #[tokio::test]
    async fn without_text_first_the_size_still_comes_first() {
        let cache = Mutex::new(BodyCache::default());
        let (r, seen) = open_and_watch(
            vec![
                selected(7),
                size_response(123),
                (body_response("plain"), Some("OK fetched")),
            ],
            &cache,
            false,
        )
        .await;
        assert_eq!(r.unwrap().0.as_slice(), b"plain");
        assert_eq!(seen, ["size 123"]);
    }

    #[tokio::test]
    async fn a_server_that_rejects_bodystructure_is_reported_as_a_text_first_failure() {
        let cache = Mutex::new(BodyCache::default());
        let (r, seen) = open_and_watch(
            vec![selected(7), (String::new(), Some("BAD unsupported"))],
            &cache,
            true,
        )
        .await;
        let e = r.unwrap_err();
        assert!(e.downcast_ref::<TextFirstFailed>().is_some(), "{e:#}");
        assert!(seen.is_empty());
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

    #[test]
    fn open_with_app_uses_bundle_name() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Demo Editor.app");
        std::fs::create_dir(&app).unwrap();
        let entry = open_with_app(&app).unwrap();
        assert_eq!(entry.name, "Demo Editor");
        assert!(entry.path.ends_with("Demo Editor.app"));
    }

    #[test]
    fn mac_app_choice_must_be_an_app_bundle() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Demo.app");
        std::fs::create_dir(&app).unwrap();
        assert!(is_mac_app(&app));
        let file = dir.path().join("notes.txt");
        std::fs::write(&file, b"x").unwrap();
        assert!(!is_mac_app(&file), "普通文件不是应用");
        assert!(!is_mac_app(&dir.path().join("missing.app")));
    }

    #[test]
    fn saving_the_same_attachment_twice_does_not_create_copies() {
        let dir = tempfile::tempdir().unwrap();
        let first =
            save_attachment_file(dir.path().to_path_buf(), "手册.pdf", b"pdf-bytes").unwrap();
        let again =
            save_attachment_file(dir.path().to_path_buf(), "手册.pdf", b"pdf-bytes").unwrap();
        assert_eq!(first, again, "同名同内容直接复用，不再生成 (2)");
        let files: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(files.len(), 1);

        // 同名但内容不同的另一个附件不能被覆盖，也不能复用
        let other =
            save_attachment_file(dir.path().to_path_buf(), "手册.pdf", b"other-bytes").unwrap();
        assert_ne!(other, first);
        assert!(other.to_string_lossy().contains("(2)"));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 2);
    }

    #[test]
    fn attachment_cache_file_names_are_safe() {
        // 正常附件名原样保留，不能加 uid 之类的后缀/前缀
        assert_eq!(
            safe_file_name("优维科技员工手册UW-HR-2024-005-A5.0.pdf"),
            "优维科技员工手册UW-HR-2024-005-A5.0.pdf"
        );
        assert_eq!(safe_file_name("../x/y:z.pdf"), ".._x_y_z.pdf");
        assert_eq!(safe_file_name("   "), "附件");

        // 超长名字按字节截断，扩展名要留着
        let long = format!("{}.pdf", "很长的名字".repeat(40));
        let fitted = safe_file_name(&long);
        assert!(fitted.len() <= 200, "{}", fitted.len());
        assert!(fitted.ends_with(".pdf"));
    }

    /// 下载目录里存着 UID 1、2 的缓存（内存和磁盘缓存都是空的）
    fn with_downloads(dir: &std::path::Path) -> (Mutex<BodyCache>, Arc<body_store::BodyStore>) {
        let auto = dir.join("downloads");
        std::fs::create_dir(&auto).unwrap();
        for uid in [1, 2] {
            let cp = auto_download::Checkpoint {
                uid_validity: 7,
                last_uid: uid,
            };
            let raw = format!("Subject: Saved {uid}\r\nFrom: a@example.com\r\n\r\nsaved");
            auto_download::save_atomic(&auto, "a", cp, uid, raw.as_bytes()).unwrap();
        }
        let archives = Arc::new(archive_source::ArchiveSource::default());
        archives.set_dirs("a", Some(auto), vec![]);
        let disk = Arc::new(body_store::BodyStore::new(
            dir.join("bodies"),
            body_store::DEFAULT_LIMIT,
        ));
        let mut cache = BodyCache::with_disk(disk.clone());
        cache.archives = Some(archives);
        (Mutex::new(cache), disk)
    }

    #[tokio::test]
    async fn cached_body_falls_back_to_downloaded_mail() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, disk) = with_downloads(dir.path());
        let key = |uid| ("a".to_string(), "INBOX".to_string(), 7, uid);
        // 不连服务器（秒开、附件、引用的快路径都只调这个）
        let (raw, date) = cached_body(&cache, &key(1)).await.unwrap();
        assert!(raw.starts_with(b"Subject: Saved 1"));
        assert!(date > 0);
        // 磁盘缓存里有同一封时用缓存的
        disk.put("a", "INBOX", 7, 2, b"Subject: cached\r\n\r\nx", 5);
        assert_eq!(
            cached_body(&cache, &key(2)).await.unwrap().0.as_slice(),
            b"Subject: cached\r\n\r\nx"
        );
        assert!(cached_body(&cache, &key(3)).await.is_none());
        let other = ("a".to_string(), "INBOX".to_string(), 8, 1);
        assert!(cached_body(&cache, &other).await.is_none());
    }

    #[tokio::test]
    async fn opening_a_downloaded_mail_needs_only_select() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, _disk) = with_downloads(dir.path());
        // 只回 SELECT：再问大小或下载原文就会断线报错
        let (r, seen) = open_and_watch(vec![selected(7)], &cache, true).await;
        let (raw, _, validity) = r.unwrap();
        assert!(raw.starts_with(b"Subject: Saved 1"));
        assert_eq!(validity, 7);
        assert!(seen.is_empty());
    }

    #[tokio::test]
    async fn downloaded_mail_goes_to_memory_cache_only() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, disk) = with_downloads(dir.path());
        let key = ("a".to_string(), "INBOX".to_string(), 7, 1);
        cached_body(&cache, &key).await.unwrap();
        assert!(cache.lock().unwrap().get(&key).is_some());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(disk.get("a", "INBOX", 7, 1).is_none());
        assert_eq!(disk.usage("a"), (0, 0));
    }

    #[tokio::test]
    async fn quote_and_attachment_paths_use_downloaded_mail() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, _disk) = with_downloads(dir.path());
        let mut session = scripted_session(vec![selected(7)]).await;
        let (raw, _, _) = read_raw(&mut session, &cache, "a", "INBOX", 2, true, Some(7))
            .await
            .unwrap();
        assert!(raw.starts_with(b"Subject: Saved 2"));
    }

    #[tokio::test]
    async fn prefetch_skips_downloaded_mail() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, _disk) = with_downloads(dir.path());
        let mut session = scripted_session(vec![selected(7)]).await;
        assert!(
            !prefetch_raw(&mut session, &cache, "a", "INBOX", 1, 1024)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn opening_a_downloaded_mail_still_marks_it_seen() {
        let dir = tempfile::tempdir().unwrap();
        let (cache, _disk) = with_downloads(dir.path());
        let mut session =
            scripted_session(vec![selected(7), (String::new(), Some("OK stored"))]).await;
        let (raw, _, _) = read_raw(&mut session, &cache, "a", "INBOX", 1, false, None)
            .await
            .unwrap();
        assert!(raw.starts_with(b"Subject: Saved 1"));
        // 脚本只有 SELECT 和一条「OK stored」：没发 STORE 的话，下面这条 NOOP 会拿到那条回应而成功
        assert!(session.noop().await.is_err(), "命中下载的邮件也要补标已读");
    }
}
