//! 后台同步：每个账号保持一条常驻 IMAP 连接盯着收件箱，优先用 IDLE 让服务器主动推送。
//! 断线指数退避重连，系统睡眠唤醒后立即重连，密码错误直接停止（避免触发风控）。

use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_notification::NotificationExt;
use tokio::{sync::broadcast, time::timeout};

use crate::{
    config::Account,
    imap::{self, AuthFailed, FlagState, Imap, OP_TIMEOUT, Summary},
};

const INBOX: &str = "INBOX";
/// 支持 IDLE 时，每隔多久重发一次 IDLE（顺便兜底同步一次）
const IDLE_REFRESH: Duration = Duration::from_secs(300);
/// 不支持 IDLE 时的轮询间隔
const POLL: Duration = Duration::from_secs(15);

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
            Err(e) if e.is::<AuthFailed>() => {
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
}

async fn run(
    app: &AppHandle,
    acct: &Account,
    pass: &str,
    cursor: &mut Cursor,
    wake_rx: &mut broadcast::Receiver<()>,
) -> Result<()> {
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
                "实时推送".into()
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
            let (wait, _stop) = idle.wait_with_timeout(IDLE_REFRESH);
            let woke = tokio::select! {
                r = wait => { r?; false }
                _ = wake_rx.recv() => true,
            };
            if woke {
                return Ok(());
            }
            s = timeout(OP_TIMEOUT, idle.done())
                .await
                .context("退出 IDLE 超时，连接可能已失效")??;
        } else {
            tokio::select! {
                _ = tokio::time::sleep(POLL) => {}
                _ = wake_rx.recv() => return Ok(()),
            }
            timeout(OP_TIMEOUT, s.noop()).await.context("NOOP 超时")??;
        }
        refresh(app, acct, &mut s, cursor).await?;
    }
}

/// 服务器有动静（或定时兜底）后：拉新邮件、同步已读/星标/删除、更新未读数。
async fn refresh(app: &AppHandle, acct: &Account, s: &mut Imap, cursor: &mut Cursor) -> Result<()> {
    // 服务器的主动通知（EXISTS/EXPUNGE 等）这里不逐条处理，清空避免通道堆满。
    while s.unsolicited_responses.try_recv().is_ok() {}

    let fresh: Vec<_> =
        imap::fetch_summaries(s, &format!("{}:*", cursor.last_uid + 1), &acct.id, INBOX)
            .await?
            .into_iter()
            // "N:*" 在没有新邮件时会返回最后一封，所以要按 UID 过滤
            .filter(|m| m.uid > cursor.last_uid)
            .collect();
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
