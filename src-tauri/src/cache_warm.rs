//! 后台预热邮件缓存：把收件箱的邮件原文先下到本地（body_store.rs），
//! 打开历史邮件时就不用再等下载。
//!
//! - 从最新的往旧的下，超过单封大小上限的跳过（等打开时再下）；
//! - 只填到缓存总量的九成，留出余地给打开的邮件；满了只删比新邮件更老的缓存腾地方；
//! - 每批下载前先等用户空闲（`idle`），读信时让路；
//! - 用自己的连接，不占读信和命令连接。

use std::{collections::HashSet, sync::Arc, time::Duration};

use anyhow::{Context, Result};
use async_imap::Session;
use serde::Serialize;
use tokio::time::timeout;

use crate::{
    body_store::BodyStore,
    download_protocol::{ArchiveData, checked_command},
    imap::{self, OP_TIMEOUT},
    lanes,
};

/// 一批最多下多少封、多少字节：小邮件合起来省往返，大邮件一封一批
const BATCH_MAILS: usize = 20;
const BATCH_BYTES: u64 = 1024 * 1024;
/// 预热最多用到总量的这么多（百分比），剩下的留给打开的邮件
const FILL_PERCENT: u64 = 90;
/// 一次问多少封邮件的大小
const SIZE_CHUNK: usize = 500;

/// 预热进度，推给设置页显示
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// off（没开）| waiting（等空闲）| running | done | full（到了总量上限）| error
    pub state: String,
    /// 收件箱里不超过单封上限的邮件，已经缓存了几封
    pub done: u32,
    pub total: u32,
    /// 这个账号的缓存（所有文件夹，包括打开过的）
    pub cached_count: u32,
    pub cached_bytes: u64,
    pub limit_bytes: u64,
    pub detail: String,
}

impl Progress {
    pub fn with_usage(mut self, store: &BodyStore, account: &str, limit: u64) -> Self {
        let (count, bytes) = store.usage(account);
        self.cached_count = count as u32;
        self.cached_bytes = bytes;
        self.limit_bytes = limit;
        self
    }
}

/// 文件夹里所有邮件的（UID，大小，收件时间），按 UID 从新到旧
async fn sizes<T: lanes::Stream>(session: &mut Session<T>) -> Result<Vec<(u32, u32, i64)>> {
    let uids = imap::all_uids(session).await?;
    let mut out = Vec::with_capacity(uids.len());
    for chunk in uids.chunks(SIZE_CHUNK) {
        let responses = timeout(
            OP_TIMEOUT * 3,
            checked_command(
                session,
                &format!(
                    "UID FETCH {} (UID RFC822.SIZE INTERNALDATE)",
                    imap::uid_set(chunk)
                ),
            ),
        )
        .await
        .context("查询邮件大小超时")??;
        out.extend(responses.into_iter().filter_map(|r| match r {
            ArchiveData::Message {
                uid: Some(uid),
                size: Some(size),
                date,
                ..
            } => Some((uid, size, date)),
            _ => None,
        }));
    }
    out.sort_unstable_by_key(|&(uid, _, _)| std::cmp::Reverse(uid));
    Ok(out)
}

/// 下载一批的超时：按大小放宽（至少 60 秒，按 16 KB/s 估算），慢线路上的大邮件也有机会下完
fn batch_timeout(bytes: u64) -> Duration {
    (OP_TIMEOUT * 3).max(Duration::from_secs(bytes / (16 * 1024)))
}

/// 下载超时过的邮件（跨重试保留）：`slow` 以后单独一封一批下，单独下还超时的放进 `skip` 不再尝试
#[derive(Default)]
pub struct Trouble {
    pub slow: HashSet<u32>,
    pub skip: HashSet<u32>,
}

/// 这一轮怎么下：总量、单封上限、下载超时过的邮件
pub struct Plan<'a> {
    pub limit: u64,
    pub max_mail: u32,
    pub trouble: &'a mut Trouble,
}

/// 预热一个文件夹，返回最后的进度（state 为 done 或 full）。
/// `idle` 在每批下载前调用，等到用户不在读信时才返回。
pub async fn warm_folder<T, I, F, R>(
    session: &mut Session<T>,
    store: Arc<BodyStore>,
    (account, folder): (&str, &str),
    plan: Plan<'_>,
    mut idle: I,
    mut report: R,
) -> Result<Progress>
where
    T: lanes::Stream,
    I: FnMut() -> F,
    F: Future<Output = ()>,
    R: FnMut(&Progress),
{
    let Plan {
        limit,
        max_mail,
        trouble,
    } = plan;
    let validity = crate::select_validity(session, folder).await?;
    let wanted: Vec<(u32, u32, i64)> = sizes(session)
        .await?
        .into_iter()
        .filter(|(uid, size, _)| *size <= max_mail && !trouble.skip.contains(uid))
        .collect();
    // 读目录、第一次统计占用都是磁盘操作，不占异步线程
    let cached = {
        let (store, account, folder) = (store.clone(), account.to_string(), folder.to_string());
        tokio::task::spawn_blocking(move || {
            store.usage(&account);
            store.cached_uids(&account, &folder, validity)
        })
        .await?
    };
    let pending: Vec<(u32, u32, i64)> = wanted
        .iter()
        .copied()
        .filter(|(uid, _, _)| !cached.contains(uid))
        .collect();
    let mut progress = Progress {
        state: "running".into(),
        done: (wanted.len() - pending.len()) as u32,
        total: wanted.len() as u32,
        ..Default::default()
    }
    .with_usage(&store, account, limit);
    report(&progress);
    // 给打开的邮件留出余地：预热只填到总量的九成
    let reserve = limit - limit / 100 * FILL_PERCENT;
    let mut rest = &pending[..];
    while !rest.is_empty() {
        // 这一批：不超过 20 封、1 MB（第一封再大也单独成一批），也不超过剩下的空间
        let room = store.room(account).saturating_sub(reserve);
        let mut take = 0;
        let mut bytes = 0u64;
        while take < rest.len() && take < BATCH_MAILS {
            // 每封落盘还多 8 字节日期
            let next = u64::from(rest[take].1) + 8;
            let alone = trouble.slow.contains(&rest[take].0);
            if bytes + next > room || (take > 0 && (bytes + next > BATCH_BYTES || alone)) {
                break;
            }
            bytes += next;
            take += 1;
            if alone {
                break;
            }
        }
        if take == 0 {
            // 满了：删掉比这封更老的缓存腾地方（越新的邮件越值得留）；腾不出来就到此为止
            let (_, size, date) = rest[0];
            let made = {
                let (store, account) = (store.clone(), account.to_string());
                tokio::task::spawn_blocking(move || {
                    store.make_room(&account, u64::from(size) + 8, reserve, date)
                })
                .await?
            };
            if made {
                continue;
            }
            progress.state = "full".into();
            break;
        }
        let (batch, after) = rest.split_at(take);
        idle().await;
        let uids: Vec<u32> = batch.iter().map(|(uid, _, _)| *uid).collect();
        let fetched = timeout(
            batch_timeout(bytes),
            checked_command(
                session,
                &format!(
                    "UID FETCH {} (UID INTERNALDATE BODY.PEEK[])",
                    imap::uid_set(&uids)
                ),
            ),
        )
        .await
        .context("下载邮件超时");
        let responses = match fetched {
            Ok(Ok(responses)) => responses,
            // 断线、服务器拒绝：不怪这几封邮件，交给外面重试
            Ok(Err(e)) => return Err(e),
            Err(e) => {
                // 一批下不完：以后这几封单独下；单独一封还下不完就跳过它（打开时照常下载），
                // 别让它卡住整个预热
                if let [uid] = uids[..] {
                    trouble.skip.insert(uid);
                } else {
                    trouble.slow.extend(&uids);
                }
                return Err(e);
            }
        };
        let mails: Vec<(u32, Vec<u8>, i64)> = responses
            .into_iter()
            .filter_map(|r| match r {
                ArchiveData::Message {
                    uid: Some(uid),
                    raw: Some(raw),
                    date,
                    ..
                } if uids.contains(&uid) => Some((uid, raw, date)),
                _ => None,
            })
            .collect();
        // 在别处删掉的邮件服务器不再返回，算处理过了
        progress.done += batch.len() as u32;
        let (store2, account2, folder2) = (store.clone(), account.to_string(), folder.to_string());
        tokio::task::spawn_blocking(move || {
            for (uid, raw, date) in mails {
                store2.put_aged(&account2, &folder2, validity, uid, &raw, date);
            }
        })
        .await?;
        progress = progress.with_usage(&store, account, limit);
        report(&progress);
        rest = after;
    }
    if progress.state == "running" {
        progress.state = "done".into();
    }
    let progress = progress.with_usage(&store, account, limit);
    report(&progress);
    Ok(progress)
}

/// 用户最近一次打开邮件后至少过了这么久，才继续预热
pub const IDLE_AFTER_OPEN: Duration = Duration::from_secs(15);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_imap;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    fn mail(n: u32, body_len: usize) -> String {
        format!("Subject: m{n}\r\n\r\n{}", "x".repeat(body_len))
    }

    /// 收件箱里 UID 1..=n 的邮件，正文长度由 `len` 决定
    fn server(lens: &[usize]) -> test_imap::Shared {
        let s = test_imap::state("IMAP4rev1", &[("INBOX", 7)]);
        for (i, len) in lens.iter().enumerate() {
            s.lock()
                .unwrap()
                .add("INBOX", &mail(i as u32 + 1, *len), &[]);
        }
        s
    }

    fn fetched_bodies(srv: &test_imap::Shared) -> Vec<String> {
        srv.lock()
            .unwrap()
            .commands
            .iter()
            .filter(|c| c.contains("BODY.PEEK[]"))
            .cloned()
            .collect()
    }

    async fn warm(
        srv: &test_imap::Shared,
        store: &Arc<BodyStore>,
        limits: (u64, u32),
    ) -> (Progress, Vec<Progress>, usize) {
        let mut session = test_imap::session(srv.clone()).await;
        let reports = Mutex::new(Vec::new());
        let idles = AtomicUsize::new(0);
        let mut trouble = Trouble::default();
        let done = warm_folder(
            &mut session,
            store.clone(),
            ("a", "INBOX"),
            Plan {
                limit: limits.0,
                max_mail: limits.1,
                trouble: &mut trouble,
            },
            || {
                idles.fetch_add(1, Ordering::SeqCst);
                async {}
            },
            |p| reports.lock().unwrap().push(p.clone()),
        )
        .await
        .unwrap();
        (done, reports.into_inner().unwrap(), idles.into_inner())
    }

    fn store() -> (tempfile::TempDir, Arc<BodyStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(BodyStore::new(dir.path().join("bodies"), 1 << 30));
        (dir, store)
    }

    #[tokio::test]
    async fn caches_the_inbox_skipping_mail_over_the_size_limit() {
        let (_d, store) = store();
        // 第 2 封太大
        let srv = server(&[100, 5000, 100]);
        let (done, reports, idles) = warm(&srv, &store, (1 << 30, 1000)).await;
        assert_eq!(done.state, "done");
        assert_eq!((done.done, done.total), (2, 2));
        assert_eq!(done.cached_count, 2);
        assert!(store.contains("a", "INBOX", 7, 1) && store.contains("a", "INBOX", 7, 3));
        assert!(!store.contains("a", "INBOX", 7, 2), "超过单封上限的不下");
        assert_eq!(idles, 1, "小邮件一批下完，下载前先等空闲");
        assert!(
            fetched_bodies(&srv)
                .iter()
                .all(|c| !c.contains(" 2 ") && !c.contains(",2,"))
        );
        assert_eq!(reports.first().unwrap().state, "running");
        let (cached, from) = store.get("a", "INBOX", 7, 3).unwrap();
        assert!(String::from_utf8(cached).unwrap().contains("Subject: m3"));
        assert_ne!(from, 0, "带上服务器收件时间");
    }

    #[tokio::test]
    async fn running_again_only_fetches_what_is_missing() {
        let (_d, store) = store();
        let srv = server(&[100, 100]);
        warm(&srv, &store, (1 << 30, 1000)).await;
        srv.lock().unwrap().add("INBOX", &mail(3, 100), &[]);
        srv.lock().unwrap().commands.clear();
        let (done, _, _) = warm(&srv, &store, (1 << 30, 1000)).await;
        assert_eq!((done.done, done.total), (3, 3));
        assert_eq!(
            fetched_bodies(&srv),
            ["UID FETCH 3 (UID INTERNALDATE BODY.PEEK[])"]
        );
    }

    #[tokio::test]
    async fn newest_mail_is_cached_first_and_it_stops_before_the_limit() {
        let (_d, store) = store();
        // 每封约 10 KB；总量只够放下 2 封（再留一成余地）
        let srv = server(&[10_000; 5]);
        let one = 10_000 + 20 + 8;
        let limit = one * 2 + one * 2 / 10 + 1000;
        store.set_limit("a", limit);
        // 每封单独一批
        let (done, _, _) = warm(&srv, &store, (limit, 20_000)).await;
        assert_eq!(done.state, "full");
        let cached: Vec<u32> = (1..=5)
            .filter(|u| store.contains("a", "INBOX", 7, *u))
            .collect();
        assert!(
            !cached.is_empty() && cached.iter().all(|u| *u >= 4),
            "从最新的开始：{cached:?}"
        );
        assert!(store.usage("a").1 <= limit);
    }

    #[tokio::test]
    async fn large_mail_goes_in_its_own_batch_and_waits_for_idle_each_time() {
        let (_d, store) = store();
        let srv = server(&[600_000, 600_000, 100]);
        let (done, reports, idles) = warm(&srv, &store, (1 << 30, 1 << 20)).await;
        assert_eq!(done.state, "done");
        assert_eq!(idles, 2, "两封大邮件放不进一批 1 MB");
        assert_eq!(fetched_bodies(&srv).len(), 2);
        let steps: Vec<u32> = reports.iter().map(|p| p.done).collect();
        assert!(steps.windows(2).all(|w| w[0] <= w[1]), "{steps:?}");
    }

    fn dated(n: u32, date: &str) -> String {
        format!(
            "X-Internal-Date: {date}\r\nSubject: m{n}\r\n\r\n{}",
            "x".repeat(10_000)
        )
    }

    #[tokio::test]
    async fn when_full_newer_mail_replaces_older_cached_mail() {
        let (_d, store) = store();
        let srv = test_imap::state("IMAP4rev1", &[("INBOX", 7)]);
        for (n, date) in [
            (1, "01-Jan-2020 00:00:00 +0000"),
            (2, "01-Jan-2021 00:00:00 +0000"),
        ] {
            srv.lock().unwrap().add("INBOX", &dated(n, date), &[]);
        }
        // 放得下两封，再留一成余地
        let one = 10_100;
        let limit = one * 2 + one * 2 / 9 + 500;
        store.set_limit("a", limit);
        let (done, _, _) = warm(&srv, &store, (limit, 20_000)).await;
        assert_eq!(done.state, "done");
        srv.lock()
            .unwrap()
            .add("INBOX", &dated(3, "01-Jan-2022 00:00:00 +0000"), &[]);
        let (done, _, _) = warm(&srv, &store, (limit, 20_000)).await;
        assert_eq!(done.state, "done", "删掉更老的腾出地方");
        assert!(store.contains("a", "INBOX", 7, 3), "新邮件存进来了");
        assert!(!store.contains("a", "INBOX", 7, 1), "最老的让位");
        assert!(store.contains("a", "INBOX", 7, 2));
    }

    #[tokio::test(start_paused = true)]
    async fn a_mail_that_times_out_is_retried_alone_then_skipped() {
        let (_d, store) = store();
        let srv = server(&[100, 100]);
        srv.lock().unwrap().stall_bodies = vec![2];
        let mut trouble = Trouble::default();
        let mut outcomes = Vec::new();
        for _ in 0..3 {
            let mut session = test_imap::session(srv.clone()).await;
            let r = warm_folder(
                &mut session,
                store.clone(),
                ("a", "INBOX"),
                Plan {
                    limit: 1 << 30,
                    max_mail: 1000,
                    trouble: &mut trouble,
                },
                || async {},
                |_| {},
            )
            .await;
            outcomes.push(r.map(|p| (p.state, p.done, p.total)).ok());
        }
        // 第一次两封一批超时；第二次单独下第 2 封又超时；第三次跳过它，只下第 1 封
        assert_eq!(outcomes[0], None);
        assert_eq!(outcomes[1], None);
        assert_eq!(outcomes[2], Some(("done".to_string(), 1, 1)));
        assert!(trouble.skip.contains(&2) && !trouble.skip.contains(&1));
        assert!(store.contains("a", "INBOX", 7, 1));
    }
}
