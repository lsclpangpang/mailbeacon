//! 后台同步：每个账号保持一条常驻 IMAP 连接盯着收件箱，优先用 IDLE 让服务器主动推送。
//! 断线指数退避重连，系统睡眠唤醒后立即重连，密码错误直接停止（避免触发风控）。

use std::{
    collections::HashSet,
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
            notify(app, acct, &first);
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
            notify(app, acct, &fresh);
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
    let first = if uids.is_empty() {
        Vec::new()
    } else {
        imap::fetch_summaries(&mut s, &imap::uid_set(&uids), &acct.id, INBOX).await?
    };
    let newest = first.iter().map(|m| m.uid).max().unwrap_or(0);
    // 重连时，断线期间到达的邮件照样发通知
    if cursor.uid_validity == validity && cursor.last_uid > 0 {
        let missed: Vec<_> = first
            .iter()
            .filter(|m| m.uid > cursor.last_uid)
            .cloned()
            .collect();
        notify(app, acct, &missed);
    }
    *cursor = Cursor {
        uid_validity: validity,
        last_uid: newest.max(mb.uid_next.unwrap_or(1).saturating_sub(1)),
        min_uid: uids.first().copied().unwrap_or(1),
        idle_fallbacks: 0,
        pop3_initialized: false,
        pop3_uidls: HashSet::new(),
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
        } else {
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = wake_rx.recv() => return Ok(()),
            }
            timeout(OP_TIMEOUT, s.noop()).await.context("NOOP 超时")??;
            refresh(app, acct, &mut s, cursor, true).await?;
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
        cursor.last_uid = max;
        notify(app, acct, &fresh);
        emit(
            app,
            Event::NewMail {
                account_id: &acct.id,
                messages: fresh,
            },
        );
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

/// 窗口不在前台时，新邮件弹系统通知
fn notify(app: &AppHandle, _acct: &Account, msgs: &[Summary]) {
    let focused = app
        .get_webview_window("main")
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if focused {
        return;
    }
    for m in msgs.iter().filter(|m| !m.seen).take(3) {
        let from = if m.from_name.is_empty() {
            &m.from_addr
        } else {
            &m.from_name
        };
        let subject = if m.subject.is_empty() {
            "(无主题)"
        } else {
            &m.subject
        };
        let _ = app
            .notification()
            .builder()
            .title(from)
            .body(subject)
            .show();
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
