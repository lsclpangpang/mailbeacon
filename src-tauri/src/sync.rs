//! 后台同步：每个账号保持一条常驻 IMAP 连接盯着收件箱，优先用 IDLE 让服务器主动推送。
//! 断线指数退避重连，系统睡眠唤醒后立即重连，密码错误直接停止（避免触发风控）。

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::{sync::broadcast, time::timeout};

use crate::{
    config::{Account, ReceiveProtocol},
    imap::{self, AuthFailed, FlagState, Imap, OP_TIMEOUT, Summary},
};

const INBOX: &str = "INBOX";
/// 支持 IDLE 时，最多等这么久就主动检查一次新邮件。
///
/// 服务器正常会立刻推送 EXISTS；但部分邮箱会保持 IDLE 连接却不及时推送，
/// 而 `wait_with_timeout` 的计时器又会被服务器的 `* OK` 保活不断重置，
/// 所以这里用一层不受保活影响的硬超时，保证新邮件最多落后这么久就会被
/// 主动检查到（列表和系统通知都走这条路径）。
const IDLE_FALLBACK: Duration = Duration::from_secs(30);
/// 兜底检查只拉新邮件；已读/星标/未读数每这么多次兜底做一次完整同步，
/// 避免每 30 秒都执行 FETCH FLAGS 和 SEARCH UNSEEN 造成命令量放大。
const IDLE_FULL_SYNC_EVERY: u32 = 10;
/// 不支持 IDLE 时的轮询间隔
const POLL: Duration = Duration::from_secs(15);
/// POP3 每次检查都要重新登录（会话内看不到新邮件），间隔放长以免触发登录风控
const POP3_POLL: Duration = Duration::from_secs(60);

/// 推给界面的事件，统一走 "mail" 这一个事件名
#[derive(Clone, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Event<'a> {
    #[serde(rename_all = "camelCase")]
    Status {
        account_id: &'a str,
        state: &'a str,
        detail: String,
    },
    #[serde(rename_all = "camelCase")]
    AutoDownload {
        account_id: &'a str,
        state: &'a str,
        detail: String,
        last_path: Option<String>,
        saved_count: u64,
    },
    #[serde(rename_all = "camelCase")]
    BulkDownload {
        account_id: &'a str,
        #[serde(flatten)]
        status: crate::bulk_download::Status,
    },
    /// 重连后的完整首屏，界面用它替换该账号收件箱
    #[serde(rename_all = "camelCase")]
    Inbox {
        account_id: &'a str,
        messages: Vec<Summary>,
    },
    #[serde(rename_all = "camelCase")]
    NewMail {
        account_id: &'a str,
        messages: Vec<Summary>,
    },
    /// 已加载范围内（uid >= min_uid）的最新状态；不在列表里的说明已在别处删除
    #[serde(rename_all = "camelCase")]
    Flags {
        account_id: &'a str,
        min_uid: u32,
        items: Vec<FlagState>,
    },
    #[serde(rename_all = "camelCase")]
    Unread { account_id: &'a str, count: usize },
    /// 收信规则没能执行（例如目标文件夹已被删除），界面提示用户修改规则
    #[serde(rename_all = "camelCase")]
    RuleError { account_id: &'a str, detail: String },
}

pub fn emit(app: &AppHandle, ev: Event) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        state.record(&ev);
        // 只传递触发信号；自动下载任务有独立的持久进度，不依赖首屏或通知列表。
        match &ev {
            Event::Inbox { account_id, .. } | Event::NewMail { account_id, .. } => {
                let _ = state.download_wake.send(account_id.to_string());
            }
            _ => {}
        }
    }
    let _ = app.emit("mail", ev);
}

pub async fn watch(app: AppHandle, acct: Account, pass: String, wake: broadcast::Sender<()>) {
    let mut cursor = Cursor::default();
    let mut backoff = 1u64;
    loop {
        let mut wake_rx = wake.subscribe();
        let started = Instant::now();
        emit(
            &app,
            Event::Status {
                account_id: &acct.id,
                state: "connecting",
                detail: String::new(),
            },
        );
        match run(&app, &acct, &pass, &mut cursor, &mut wake_rx).await {
            Ok(()) => {
                backoff = 1;
                continue;
            }
            Err(e) if e.is::<AuthFailed>() || e.is::<crate::pop3::AuthFailed>() => {
                emit(
                    &app,
                    Event::Status {
                        account_id: &acct.id,
                        state: "error",
                        detail: e.to_string(),
                    },
                );
                return;
            }
            Err(e) => emit(
                &app,
                Event::Status {
                    account_id: &acct.id,
                    state: "offline",
                    detail: format!("{e:#}"),
                },
            ),
        }
        // 用了一段时间才断开（多半是网络抖动）：立即重连；连续失败才逐步拉长间隔
        if started.elapsed() > Duration::from_secs(60) {
            backoff = 1;
            continue;
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(backoff)) => {}
            _ = wake_rx.recv() => {}
        }
        backoff = (backoff * 2).min(60);
    }
}

/// 额外提醒的文件夹最多看最后这么多封新邮件（一下子移进来很多时不逐封拉取）。
const FOLDER_NOTIFY_MAX: u32 = 50;
/// 服务器收件时间早于上次检查这么久的，是从别处移进来的旧邮件，不提醒。
const FOLDER_NOTIFY_SLACK_MS: i64 = 10 * 60 * 1000;
/// 记住最近提醒过的收件箱邮件，Gmail 标签里的同一封不再提醒。
const RECENT_INBOX: usize = 200;

/// 额外提醒的文件夹上次检查时的位置。
#[derive(Clone, Copy, Debug, PartialEq)]
struct FolderMark {
    validity: u32,
    next: u32,
    /// 上次确认这个位置的时间（毫秒）
    checked_at: i64,
}

/// 额外提醒的文件夹各自看到哪里了。
#[derive(Default)]
struct FolderCursors(HashMap<String, FolderMark>);

impl FolderCursors {
    /// 有新邮件时返回上次的位置，拉取成功后再 `commit`；
    /// 第一次看到（或文件夹重建）只记位置，不提醒已有的邮件。
    fn check(&mut self, folder: &str, validity: u32, next: u32, now: i64) -> Option<FolderMark> {
        match self.0.get_mut(folder) {
            Some(mark) if mark.validity == validity && next > mark.next => Some(*mark),
            Some(mark) if mark.validity == validity => {
                mark.checked_at = now;
                None
            }
            _ => {
                self.commit(folder, validity, next, now);
                None
            }
        }
    }

    fn commit(&mut self, folder: &str, validity: u32, next: u32, now: i64) {
        self.0.insert(
            folder.to_string(),
            FolderMark {
                validity,
                next,
                checked_at: now,
            },
        );
    }

    /// 只保留仍在提醒的文件夹。
    fn keep(&mut self, folders: &[String]) {
        self.0.retain(|f, _| folders.contains(f));
    }
}

/// 判断两个文件夹里是不是同一封邮件（Gmail 标签）。
type MailKey = String;

fn mail_key(m: &Summary) -> MailKey {
    imap::mail_identity(m)
}

/// 跨重连保留的状态，断线期间到达的邮件在重连后补上。
#[derive(Default)]
struct Cursor {
    uid_validity: u32,
    last_uid: u32,
    min_uid: u32,
    /// 连续兜底检查的次数，用于定期插入一次完整的标记/未读同步。
    idle_fallbacks: u32,
    pop3_initialized: bool,
    pop3_uidls: HashSet<String>,
    folders: FolderCursors,
    /// 最近提醒过的收件箱邮件
    recent_inbox: std::collections::VecDeque<MailKey>,
}

impl Cursor {
    fn remember_inbox(&mut self, mails: &[Summary]) {
        for m in mails {
            self.recent_inbox.push_back(mail_key(m));
        }
        while self.recent_inbox.len() > RECENT_INBOX {
            self.recent_inbox.pop_front();
        }
    }
}

/// 返回需要推给界面的摘要、当前全部 UIDL 和本地未读数。
/// `known` 为空时读取最新一页，否则只读取尚未见过的 UIDL。
async fn pop3_snapshot(
    app: &AppHandle,
    acct: &Account,
    pass: &str,
    known: Option<&HashSet<String>>,
) -> Result<(Vec<Summary>, HashSet<String>, usize)> {
    let shared = app.state::<crate::AppState>().pop3_mailbox(&acct.id);
    // 检查新邮件必须重新登录：已有的 POP3 会话看不到之后到达的邮件。
    let mut session = crate::pop3::Session::open(shared, acct, pass, true).await?;
    let all = session.ids();
    let current: HashSet<String> = all
        .iter()
        .filter_map(|id| session.uidl(*id).map(str::to_string))
        .collect();
    let ids: Vec<u32> = match known {
        None => all[all.len().saturating_sub(imap::PAGE)..].to_vec(),
        Some(known) => all
            .into_iter()
            .filter(|id| session.uidl(*id).is_some_and(|uidl| !known.contains(uidl)))
            .collect(),
    };
    let mut messages = session.summaries(&ids).await?;
    let unread = session.unread();
    session.close().await;
    messages.reverse();
    Ok((messages, current, unread))
}

async fn run_pop3(
    app: &AppHandle,
    acct: &Account,
    pass: &str,
    cursor: &mut Cursor,
    wake_rx: &mut broadcast::Receiver<()>,
) -> Result<()> {
    let was_initialized = cursor.pop3_initialized;
    let known = cursor.pop3_uidls.clone();
    let (first, current, unread) = pop3_snapshot(
        app,
        acct,
        pass,
        if was_initialized { Some(&known) } else { None },
    )
    .await?;
    cursor.pop3_initialized = true;
    cursor.pop3_uidls = current;
    if was_initialized {
        if !first.is_empty() {
            notify(app, acct, "", &first);
            emit(
                app,
                Event::NewMail {
                    account_id: &acct.id,
                    messages: first,
                },
            );
        }
    } else {
        emit(
            app,
            Event::Inbox {
                account_id: &acct.id,
                messages: first,
            },
        );
    }
    emit(
        app,
        Event::Unread {
            account_id: &acct.id,
            count: unread,
        },
    );
    emit(
        app,
        Event::Status {
            account_id: &acct.id,
            state: "online",
            detail: format!("POP3 每 {} 秒检查一次", POP3_POLL.as_secs()),
        },
    );
    loop {
        tokio::select! {
            _ = tokio::time::sleep(POP3_POLL) => {}
            _ = wake_rx.recv() => return Ok(()),
        }
        let known = cursor.pop3_uidls.clone();
        let (fresh, current, unread) = pop3_snapshot(app, acct, pass, Some(&known)).await?;
        if !fresh.is_empty() {
            notify(app, acct, "", &fresh);
            emit(
                app,
                Event::NewMail {
                    account_id: &acct.id,
                    messages: fresh,
                },
            );
        }
        cursor.pop3_uidls = current;
        emit(
            app,
            Event::Unread {
                account_id: &acct.id,
                count: unread,
            },
        );
    }
}

/// 等待服务器的 IDLE 事件，但最多等 `fallback`。
///
/// async-imap 的 `wait_with_timeout` 会被服务器发来的任何一行（包括
/// `* OK Still here` 保活）重置计时器；只要服务器定期保活，兜底刷新就
/// 永远不会触发。这里再套一层硬超时，返回 `Ok(true)` 表示服务器有动静，
/// `Ok(false)` 表示到了兜底时间。
async fn wait_with_fallback<T, E, F>(wait: F, fallback: Duration) -> Result<bool>
where
    F: Future<Output = std::result::Result<T, E>>,
    E: Into<anyhow::Error>,
{
    match timeout(fallback, wait).await {
        Ok(Ok(_)) => Ok(true),
        Ok(Err(error)) => Err(error.into()),
        Err(_) => Ok(false),
    }
}

async fn run(
    app: &AppHandle,
    acct: &Account,
    pass: &str,
    cursor: &mut Cursor,
    wake_rx: &mut broadcast::Receiver<()>,
) -> Result<()> {
    if acct.receive_protocol == ReceiveProtocol::Pop3 {
        return run_pop3(app, acct, pass, cursor, wake_rx).await;
    }
    let mut s = imap::login(acct, pass).await?;
    let use_idle = imap::has_capability(&mut s, "IDLE").await?;
    let mb = timeout(OP_TIMEOUT, s.select(INBOX))
        .await?
        .context("打开收件箱失败")?;
    let validity = mb.uid_validity.unwrap_or(0);

    // 每次（重）连上都发一份完整首屏：顺带纠正断线期间在别处做的删除和已读
    let uids = imap::page_uids(&mut s, None).await?;
    let mut first = if uids.is_empty() {
        Vec::new()
    } else {
        imap::fetch_summaries(&mut s, &imap::uid_set(&uids), &acct.id, INBOX).await?
    };
    let newest = first.iter().map(|m| m.uid).max().unwrap_or(0);
    // 重连时，断线期间到达的邮件照样执行收信规则、发通知
    if cursor.uid_validity == validity && cursor.last_uid > 0 {
        let missed: Vec<_> = first
            .iter()
            .filter(|m| m.uid > cursor.last_uid)
            .cloned()
            .collect();
        let done = run_rules(app, acct, &mut s, &missed).await?;
        first = done.apply_to(first);
        // 被规则移走的不记：移到提醒的文件夹时由那边提醒
        let missed = done.apply_to(missed);
        cursor.remember_inbox(&missed);
        notify(app, acct, "", &missed);
    }
    rules_saw(app, &acct.id, &first);
    *cursor = Cursor {
        uid_validity: validity,
        last_uid: newest.max(mb.uid_next.unwrap_or(1).saturating_sub(1)),
        min_uid: uids.first().copied().unwrap_or(1),
        idle_fallbacks: 0,
        pop3_initialized: false,
        pop3_uidls: HashSet::new(),
        folders: std::mem::take(&mut cursor.folders),
        recent_inbox: std::mem::take(&mut cursor.recent_inbox),
    };
    emit(
        app,
        Event::Inbox {
            account_id: &acct.id,
            messages: first,
        },
    );
    emit(
        app,
        Event::Unread {
            account_id: &acct.id,
            count: imap::unseen_count(&mut s).await?,
        },
    );
    emit(
        app,
        Event::Status {
            account_id: &acct.id,
            state: "online",
            detail: if use_idle {
                "实时推送（约 30 秒兜底检查）".into()
            } else {
                format!("每 {} 秒检查一次", POLL.as_secs())
            },
        },
    );
    check_extra_folders(app, acct, &mut s, cursor).await?;

    loop {
        if use_idle {
            let mut idle = s.idle();
            timeout(OP_TIMEOUT, idle.init())
                .await
                .context("进入 IDLE 超时")??;
            let (wait, _stop) = idle.wait();
            let mut event = false;
            let woke = tokio::select! {
                // 服务器推送了变化，或者到了兜底时间，都退出 IDLE 主动同步一次。
                got = wait_with_fallback(wait, IDLE_FALLBACK) => { event = got?; false }
                _ = wake_rx.recv() => true,
            };
            if woke {
                return Ok(());
            }
            s = timeout(OP_TIMEOUT, idle.done())
                .await
                .context("退出 IDLE 超时，连接可能已失效")??;
            // 有服务器事件时做完整同步；纯兜底只查新邮件，定期再补一次完整同步。
            let full = event || {
                cursor.idle_fallbacks = cursor.idle_fallbacks.wrapping_add(1);
                cursor.idle_fallbacks % IDLE_FULL_SYNC_EVERY == 0
            };
            refresh(app, acct, &mut s, cursor, full).await?;
            check_extra_folders(app, acct, &mut s, cursor).await?;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = wake_rx.recv() => return Ok(()),
            }
            timeout(OP_TIMEOUT, s.noop()).await.context("NOOP 超时")??;
            refresh(app, acct, &mut s, cursor, true).await?;
            check_extra_folders(app, acct, &mut s, cursor).await?;
        }
    }
}

/// 服务器有动静（或定时兜底）后：拉新邮件；`full` 时再同步已读/星标和未读数。
///
/// 兜底检查通常只做一次新邮件查询（一条命令）；发现新邮件或到达完整同步的
/// 周期时才额外执行 FETCH FLAGS 和 SEARCH UNSEEN。
async fn refresh(
    app: &AppHandle,
    acct: &Account,
    s: &mut Imap,
    cursor: &mut Cursor,
    full: bool,
) -> Result<()> {
    // 服务器的主动通知（EXISTS/EXPUNGE 等）这里不逐条处理，清空避免通道堆满。
    while s.unsolicited_responses.try_recv().is_ok() {}

    let fresh: Vec<_> =
        imap::fetch_summaries(s, &format!("{}:*", cursor.last_uid + 1), &acct.id, INBOX)
            .await?
            .into_iter()
            // "N:*" 在没有新邮件时会返回最后一封，所以要按 UID 过滤
            .filter(|m| m.uid > cursor.last_uid)
            .collect();
    let has_fresh = !fresh.is_empty();
    if let Some(max) = fresh.iter().map(|m| m.uid).max() {
        // 规则执行成功（或只是被服务器拒绝）后才算处理过；断线时重连会再处理一次
        let done = run_rules(app, acct, s, &fresh).await?;
        rules_saw(app, &acct.id, &fresh);
        cursor.last_uid = max;
        let fresh = done.apply_to(fresh);
        cursor.remember_inbox(&fresh);
        notify(app, acct, "", &fresh);
        if !fresh.is_empty() {
            emit(
                app,
                Event::NewMail {
                    account_id: &acct.id,
                    messages: fresh,
                },
            );
        }
    }
    if !full && !has_fresh {
        return Ok(());
    }
    let items = imap::fetch_flags(s, &format!("{}:*", cursor.min_uid)).await?;
    emit(
        app,
        Event::Flags {
            account_id: &acct.id,
            min_uid: cursor.min_uid,
            items,
        },
    );
    emit(
        app,
        Event::Unread {
            account_id: &acct.id,
            count: imap::unseen_count(s).await?,
        },
    );
    Ok(())
}

/// 对收件箱的新邮件执行收信规则；服务器拒绝的原因推给界面，不影响收信；断线时返回错误。
async fn run_rules(
    app: &AppHandle,
    acct: &Account,
    s: &mut Imap,
    mails: &[Summary],
) -> Result<crate::rules::Applied> {
    let Some(state) = app.try_state::<crate::AppState>() else {
        return Ok(Default::default());
    };
    let candidates = state.rule_memory(&acct.id, |m| m.candidates(mails));
    let batches = crate::rules::batches(&state.rules.lock().unwrap(), &candidates);
    if batches.is_empty() {
        return Ok(Default::default());
    }
    // 同一批邮件反复因为连接问题失败时跳过规则，免得账号一直连不上
    let last = mails.iter().map(|m| m.uid).max().unwrap_or(0);
    if !state.rule_memory(&acct.id, |m| m.attempt(last)) {
        emit(
            app,
            Event::RuleError {
                account_id: &acct.id,
                detail: "收信规则多次因为连接问题没能执行，这几封新邮件已跳过规则".into(),
            },
        );
        return Ok(Default::default());
    }
    let done = crate::rules::execute(s, &batches).await?;
    let moved: Vec<_> = candidates
        .iter()
        .filter(|m| done.moved.contains(&m.uid))
        .cloned()
        .collect();
    state.rule_memory(&acct.id, |m| {
        m.succeeded();
        m.moved(&moved);
    });
    for detail in &done.errors {
        emit(
            app,
            Event::RuleError {
                account_id: &acct.id,
                detail: detail.clone(),
            },
        );
    }
    Ok(done)
}

/// 记下收件箱里已经看过的邮件，之后只对更新的邮件执行规则。
fn rules_saw(app: &AppHandle, account_id: &str, mails: &[Summary]) {
    if let Some(state) = app.try_state::<crate::AppState>() {
        let now = chrono::Utc::now().timestamp_millis();
        state.rule_memory(account_id, |m| m.saw(mails, now));
    }
}

/// 设置里勾选的其他文件夹（例如规则移走的邮件）也提醒新邮件。
/// 检查期间收件箱来了新邮件时马上同步一次。
async fn check_extra_folders(
    app: &AppHandle,
    acct: &Account,
    s: &mut Imap,
    cursor: &mut Cursor,
) -> Result<()> {
    let state = app.state::<crate::AppState>();
    let folders = state.prefs.lock().unwrap().extra_folders(&acct.id).to_vec();
    let (news, inbox_next) = extra_folder_news(s, &acct.id, &folders, cursor).await?;
    for (folder, fresh) in news {
        let label = state
            .folders
            .lock()
            .unwrap()
            .get(&acct.id)
            .and_then(|list| {
                list.iter()
                    .find(|f| f.path == folder)
                    .map(|f| f.name.clone())
            })
            .unwrap_or_else(|| imap::decode_mutf7(&folder));
        notify(app, acct, &label, &fresh);
    }
    if let Some(next) = inbox_next {
        refresh(app, acct, s, cursor, true).await?;
        // 中间的 UID 可能已经被删掉，拉不到；跳过去，免得以后每次都以为有新邮件
        cursor.last_uid = cursor.last_uid.max(next - 1);
    }
    Ok(())
}

/// 每次检查收件箱后用 STATUS 看一眼各文件夹的 UIDNEXT（每个文件夹一条命令）；
/// 只有真的变了才 EXAMINE 进去拉新邮件摘要，最后切回收件箱继续 IDLE。
///
/// 只提醒真正新到的邮件：从别处移进来的旧邮件（收件时间早于上次检查）和
/// 刚在收件箱提醒过的同一封（Gmail 标签）都跳过。出错时直接返回，由外层重连，
/// 位置不前移，重连后再试。第二个返回值是收件箱的 UIDNEXT（仅当检查期间来了新邮件）。
async fn extra_folder_news<T>(
    s: &mut async_imap::Session<T>,
    account_id: &str,
    folders: &[String],
    cursor: &mut Cursor,
) -> Result<(Vec<(String, Vec<Summary>)>, Option<u32>)>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    cursor.folders.keep(folders);
    let mut left_inbox = false;
    let mut news = Vec::new();
    for folder in folders {
        let now = chrono::Utc::now().timestamp_millis();
        let status = match timeout(OP_TIMEOUT, s.status(folder, "(UIDNEXT UIDVALIDITY)"))
            .await
            .context("检查文件夹超时")?
        {
            Ok(status) => status,
            // 文件夹被删或改名：跳过，连接本身没问题
            Err(async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let (Some(validity), Some(next)) = (status.uid_validity, status.uid_next) else {
            eprintln!(
                "文件夹 {} 的 STATUS 回复里没有 UIDNEXT/UIDVALIDITY，跳过新邮件提醒",
                imap::decode_mutf7(folder)
            );
            continue;
        };
        let Some(mark) = cursor.folders.check(folder, validity, next, now) else {
            continue;
        };
        left_inbox = true;
        timeout(OP_TIMEOUT, s.examine(folder))
            .await
            .context("打开文件夹超时")??;
        let from = mark.next.max(next.saturating_sub(FOLDER_NOTIFY_MAX));
        let since = mark.checked_at - FOLDER_NOTIFY_SLACK_MS;
        let fresh: Vec<_> = imap::fetch_summaries(s, &format!("{from}:*"), account_id, folder)
            .await?
            .into_iter()
            // "N:*" 在没有新邮件时会返回最后一封
            .filter(|m| m.uid >= from && m.date >= since)
            .filter(|m| !cursor.recent_inbox.contains(&mail_key(m)))
            .collect();
        cursor.folders.commit(folder, validity, next, now);
        if !fresh.is_empty() {
            news.push((folder.clone(), fresh));
        }
    }
    let mut inbox_next = None;
    if left_inbox {
        let mb = timeout(OP_TIMEOUT, s.select(INBOX))
            .await
            .context("切回收件箱超时")?
            .context("切回收件箱失败")?;
        if mb.uid_validity.unwrap_or(0) != cursor.uid_validity {
            anyhow::bail!("收件箱已重建，重新同步");
        }
        // 不在收件箱期间到达的邮件，IDLE 不会再推送
        inbox_next = mb.uid_next.filter(|&n| n > cursor.last_uid + 1);
    }
    Ok((news, inbox_next))
}

/// 窗口不在前台时，按通知设置给未读的新邮件弹系统通知。`folder` 为空表示收件箱。
fn notify(app: &AppHandle, acct: &Account, folder: &str, msgs: &[Summary]) {
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    let mails: Vec<_> = msgs
        .iter()
        .filter(|m| !m.seen)
        .map(|m| {
            let from = if m.from_name.is_empty() {
                &m.from_addr
            } else {
                &m.from_name
            };
            (from.clone(), m.subject.clone())
        })
        .collect();
    let Some(state) = app.try_state::<crate::AppState>() else {
        return;
    };
    let shown = state.prefs.lock().unwrap().notifications_for(
        &acct.id,
        &acct.email,
        folder,
        &mails,
        crate::prefs::local_minutes(),
    );
    for (title, body) in shown {
        let _ = app.notification().builder().title(title).body(body).show();
    }
}

/// 墙上时钟突然跳了很多，说明电脑刚从睡眠中醒来，旧连接大概率已失效。
pub fn spawn_wake_detector(tx: broadcast::Sender<()>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut prev = SystemTime::now();
        loop {
            tick.tick().await;
            let now = SystemTime::now();
            if now
                .duration_since(prev)
                .map_or(true, |d| d > Duration::from_secs(20))
            {
                let _ = tx.send(());
            }
            prev = now;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_folder_cursor_moves_only_when_committed() {
        let mut seen = FolderCursors::default();
        assert!(
            seen.check("Work", 7, 10, 1_000).is_none(),
            "第一次只记位置，不提醒旧邮件"
        );
        assert!(seen.check("Work", 7, 10, 2_000).is_none());
        let mark = seen.check("Work", 7, 13, 3_000).unwrap();
        assert_eq!(
            (mark.next, mark.checked_at),
            (10, 2_000),
            "没变化的检查也刷新时间"
        );
        // 没提交（例如拉取失败）时，下次还从原位置开始
        assert_eq!(seen.check("Work", 7, 13, 4_000).unwrap().next, 10);
        seen.commit("Work", 7, 13, 4_000);
        assert!(seen.check("Work", 7, 13, 5_000).is_none());
        assert!(
            seen.check("Work", 8, 2, 6_000).is_none(),
            "文件夹重建后重新记位置"
        );
        assert_eq!(seen.check("Work", 8, 3, 7_000).unwrap().next, 2);
        seen.keep(&["Other".to_string()]);
        assert!(
            seen.check("Work", 8, 9, 8_000).is_none(),
            "取消提醒后再加回来不补发旧邮件"
        );
    }

    fn raw(from: &str, subject: &str) -> String {
        raw_at(from, subject, &chrono::Utc::now())
    }

    fn raw_at(from: &str, subject: &str, at: &chrono::DateTime<chrono::Utc>) -> String {
        format!(
            "From: {from}\r\nSubject: {subject}\r\nX-Internal-Date: {}\r\n\r\nbody\r\n",
            at.format("%d-%b-%Y %H:%M:%S +0000")
        )
    }

    fn inbox_cursor() -> Cursor {
        Cursor {
            uid_validity: 3,
            last_uid: 0,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn extra_folders_report_only_new_mail_and_return_to_the_inbox() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Work", 7), ("Bills", 9)]);
        {
            let mut st = server.lock().unwrap();
            st.add("Work", &raw("old@x.com", "旧邮件"), &[]);
        }
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let mut cursor = inbox_cursor();
        let folders = vec!["Work".to_string(), "Gone".to_string()];
        // 第一次只记位置；不存在的文件夹跳过，连接照常可用
        let (news, _) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert!(news.is_empty());
        {
            let mut st = server.lock().unwrap();
            st.add("Work", &raw("boss@x.com", "季度报价"), &[]);
            st.add("Work", &raw("me@x.com", "已读的"), &["\\Seen"]);
            st.commands.clear();
        }
        let (news, inbox_next) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert_eq!(inbox_next, None);
        assert_eq!(news.len(), 1);
        assert_eq!(news[0].0, "Work");
        let subjects: Vec<_> = news[0].1.iter().map(|m| m.subject.as_str()).collect();
        assert_eq!(
            subjects,
            vec!["已读的", "季度报价"],
            "新的在前；已读过滤留给通知层"
        );
        assert!(
            news[0]
                .1
                .iter()
                .all(|m| m.folder == "Work" && m.account_id == "a")
        );
        let commands = server.lock().unwrap().commands.clone();
        assert!(
            commands.iter().any(|c| c.starts_with("EXAMINE")),
            "{commands:?}"
        );
        assert_eq!(
            commands.last().map(String::as_str),
            Some("SELECT \"INBOX\""),
            "{commands:?}"
        );
        // 没有变化时只发 STATUS，不离开收件箱
        server.lock().unwrap().commands.clear();
        let (news, _) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert!(news.is_empty());
        let commands = server.lock().unwrap().commands.clone();
        assert!(
            commands.iter().all(|c| c.starts_with("STATUS")),
            "{commands:?}"
        );
    }

    #[tokio::test]
    async fn old_mail_moved_into_a_watched_folder_is_not_announced() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Work", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let mut cursor = inbox_cursor();
        let folders = vec!["Work".to_string()];
        extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        let last_year = chrono::Utc::now() - chrono::Duration::days(300);
        {
            let mut st = server.lock().unwrap();
            for i in 0..80 {
                st.add(
                    "Work",
                    &raw_at("old@x.com", &format!("旧 {i}"), &last_year),
                    &[],
                );
            }
            st.add("Work", &raw("new@x.com", "新到的"), &[]);
            st.commands.clear();
        }
        let (news, _) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        let subjects: Vec<_> = news
            .iter()
            .flat_map(|(_, m)| m)
            .map(|m| m.subject.as_str())
            .collect();
        assert_eq!(subjects, vec!["新到的"], "移进来的旧邮件不提醒");
        let commands = server.lock().unwrap().commands.clone();
        let fetch = commands
            .iter()
            .find(|c| c.starts_with("UID FETCH"))
            .unwrap();
        assert!(
            fetch.starts_with("UID FETCH 32:*"),
            "最多只看最后 50 封：{fetch}"
        );
    }

    #[tokio::test]
    async fn mail_already_announced_from_the_inbox_is_not_repeated_for_a_label() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Label", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let mut cursor = inbox_cursor();
        let folders = vec!["Label".to_string()];
        extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        let mail = raw("boss@x.com", "同一封");
        server.lock().unwrap().add("Label", &mail, &[]);
        // 同一封邮件刚在收件箱提醒过（Gmail 标签就是这样）
        let inbox = test_imap::state("", &[("INBOX", 3)]);
        inbox.lock().unwrap().add("INBOX", &mail, &[]);
        let mut other = test_imap::session(inbox).await;
        other.select(INBOX).await.unwrap();
        let seen = imap::fetch_summaries(&mut other, "1", "a", INBOX)
            .await
            .unwrap();
        cursor.remember_inbox(&seen);
        let (news, _) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert!(news.is_empty(), "不重复提醒");
    }

    #[tokio::test]
    async fn failed_fetch_keeps_the_folder_position_for_the_next_try() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Work", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let mut cursor = inbox_cursor();
        let folders = vec!["Work".to_string()];
        extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        {
            let mut st = server.lock().unwrap();
            st.add("Work", &raw("boss@x.com", "季度报价"), &[]);
            st.fail = vec!["EXAMINE".into()];
        }
        assert!(
            extra_folder_news(&mut s, "a", &folders, &mut cursor)
                .await
                .is_err()
        );
        // 重连后再试
        server.lock().unwrap().fail.clear();
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let (news, _) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert_eq!(news.len(), 1, "上次没拉到的邮件不能丢");
    }

    #[tokio::test]
    async fn inbox_mail_arriving_during_the_folder_check_is_reported() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Work", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        s.select(INBOX).await.unwrap();
        let mut cursor = inbox_cursor();
        let folders = vec!["Work".to_string()];
        extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        {
            let mut st = server.lock().unwrap();
            st.add("Work", &raw("boss@x.com", "季度报价"), &[]);
            st.add("INBOX", &raw("x@y.com", "收件箱新邮件"), &[]);
        }
        let (_, inbox_next) = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        assert_eq!(inbox_next, Some(2), "切回收件箱时发现新邮件要马上同步");
    }

    #[tokio::test]
    async fn rebuilt_inbox_after_checking_folders_forces_a_resync() {
        use crate::test_imap;
        let server = test_imap::state("IDLE", &[("INBOX", 3), ("Work", 7)]);
        let mut s = test_imap::session(server.clone()).await;
        let mut cursor = inbox_cursor();
        let folders = vec!["Work".to_string()];
        extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .unwrap();
        {
            let mut st = server.lock().unwrap();
            st.add("Work", &raw("boss@x.com", "新"), &[]);
            st.mailbox("INBOX").validity = 4;
        }
        let error = extra_folder_news(&mut s, "a", &folders, &mut cursor)
            .await
            .err()
            .expect("收件箱重建后必须报错");
        assert!(error.to_string().contains("重新同步"), "{error:#}");
    }

    /// 服务器只发保活、从不推送 EXISTS 时，兜底必须按时结束等待。
    #[tokio::test]
    async fn idle_wait_falls_back_when_no_server_event_arrives() {
        let started = Instant::now();
        let got = wait_with_fallback(
            std::future::pending::<std::result::Result<(), anyhow::Error>>(),
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert!(!got);
        assert!(started.elapsed() >= Duration::from_millis(20));
    }

    /// 内层等待再慢（例如被服务器保活反复续期），也不能推迟兜底硬超时。
    #[tokio::test]
    async fn idle_wait_fallback_wins_over_slower_inner_wait() {
        let started = Instant::now();
        let got = wait_with_fallback(
            async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<(), anyhow::Error>(())
            },
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert!(!got);
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[tokio::test]
    async fn idle_wait_returns_server_event_before_fallback() {
        let got = wait_with_fallback(
            async { Ok::<_, anyhow::Error>("exists") },
            Duration::from_secs(30),
        )
        .await
        .unwrap();
        assert!(got);
    }

    /// 连接错误不能被兜底吞掉，必须交给外层重连。
    #[tokio::test]
    async fn idle_wait_propagates_connection_errors() {
        let error = wait_with_fallback::<(), _, _>(
            async { Err::<(), _>(anyhow::anyhow!("连接断开")) },
            Duration::from_secs(30),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("连接断开"));
    }
}
