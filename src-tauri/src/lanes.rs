//! 每个账号的两条按需连接：
//! - 命令连接：翻页、摘要、标记、移动等界面操作，串行排队；
//! - 读信连接：下载邮件原文。大邮件动辄几秒，不能排在一批摘要后面，也不能挡住翻页。
//!
//! 服务器不肯再开一条连接时（连接数超限等），读信暂时退回命令连接，过一阵再试。

use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    task::{self, Poll},
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use async_imap::Session;
use futures::future::BoxFuture;
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    sync::OwnedMutexGuard,
    time::timeout,
};

use crate::{download_protocol, imap};

/// IMAP 连接底下的字节流
pub trait Stream: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send> Stream for T {}

/// 服务器拒绝再开一条连接（连接数超限等）以后，这么久内读信都走命令连接
const READER_RETRY_AFTER: Duration = Duration::from_secs(10 * 60);
/// 网络问题导致读信连接没开出来：过一会儿就再试
const READER_RETRY_AFTER_NETWORK: Duration = Duration::from_secs(60);

pub struct Conn<T: Stream> {
    pub s: Session<T>,
    pub selected: Option<String>,
    pub last: Instant,
}

impl<T: Stream> Conn<T> {
    pub fn new(s: Session<T>) -> Self {
        Self {
            s,
            selected: None,
            last: Instant::now(),
        }
    }

    pub async fn select(&mut self, folder: &str) -> Result<()> {
        if self.selected.as_deref() != Some(folder) {
            self.selected = None;
            timeout(
                imap::OP_TIMEOUT,
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

type Slot<T> = Arc<tokio::sync::Mutex<Option<Conn<T>>>>;

pub struct Lanes<T: Stream> {
    cmd: Mutex<HashMap<String, Slot<T>>>,
    reader: Mutex<HashMap<String, Slot<T>>>,
    /// 读信连接开不出来的账号：到这个时间之前读信都走命令连接
    no_reader: Mutex<HashMap<String, Instant>>,
}

impl<T: Stream> Default for Lanes<T> {
    fn default() -> Self {
        Self {
            cmd: Mutex::default(),
            reader: Mutex::default(),
            no_reader: Mutex::default(),
        }
    }
}

fn slot<T: Stream>(map: &Mutex<HashMap<String, Slot<T>>>, id: &str) -> Slot<T> {
    map.lock()
        .unwrap()
        .entry(id.to_string())
        .or_default()
        .clone()
}

/// 拿到连接的独占使用权；没有连接或空闲太久已失效时重新登录。
async fn connected<T, C, Fut>(
    slot: Slot<T>,
    connect: &C,
) -> Result<OwnedMutexGuard<Option<Conn<T>>>>
where
    T: Stream,
    C: Fn() -> Fut,
    Fut: Future<Output = Result<Session<T>>>,
{
    let mut guard = slot.lock_owned().await;
    if let Some(c) = guard.as_mut() {
        // 空闲久了的连接可能已被服务器或 NAT 断开，先探活
        if c.last.elapsed() > Duration::from_secs(60)
            && !matches!(timeout(Duration::from_secs(5), c.s.noop()).await, Ok(Ok(_)))
        {
            *guard = None;
        }
    }
    if guard.is_none() {
        *guard = Some(Conn::new(connect().await?));
    }
    Ok(guard)
}

/// 在拿到的连接上执行；出错就丢弃连接，下次重新登录。
async fn run<T, X, R, F>(mut guard: OwnedMutexGuard<Option<Conn<T>>>, ctx: &X, f: F) -> Result<R>
where
    T: Stream,
    X: ?Sized,
    F: for<'c> FnOnce(&'c mut Conn<T>, &'c X) -> BoxFuture<'c, Result<R>>,
{
    let c = guard.as_mut().expect("connected() 保证有连接");
    let r = f(c, ctx).await;
    match &r {
        Ok(_) => c.last = Instant::now(),
        Err(_) => *guard = None,
    }
    r
}

impl<T: Stream> Lanes<T> {
    /// 在该账号的命令连接上执行一组操作。
    pub async fn with_cmd<X, R, C, Fut, F>(&self, id: &str, ctx: &X, connect: C, f: F) -> Result<R>
    where
        X: ?Sized,
        C: Fn() -> Fut,
        Fut: Future<Output = Result<Session<T>>>,
        F: for<'c> FnOnce(&'c mut Conn<T>, &'c X) -> BoxFuture<'c, Result<R>>,
    {
        let guard = connected(slot(&self.cmd, id), &connect).await?;
        run(guard, ctx, f).await
    }

    /// 在该账号的读信连接上执行；读信连接开不出来时退回命令连接。
    pub async fn with_reader<X, R, C, Fut, F>(
        &self,
        id: &str,
        ctx: &X,
        connect: C,
        f: F,
    ) -> Result<R>
    where
        X: ?Sized,
        C: Fn() -> Fut,
        Fut: Future<Output = Result<Session<T>>>,
        F: for<'c> FnOnce(&'c mut Conn<T>, &'c X) -> BoxFuture<'c, Result<R>>,
    {
        if !self.reader_available(id) {
            return self.with_cmd(id, ctx, connect, f).await;
        }
        let refused = match connected(slot(&self.reader, id), &connect).await {
            Ok(guard) => {
                self.no_reader.lock().unwrap().remove(id);
                return run(guard, ctx, f).await;
            }
            Err(e) => e,
        };
        // 登录时被拒（很多服务器连接数超限也回 NO，看起来和密码错一样）：
        // 命令连接能用，说明只是不让多开，先退回去；命令连接也不行就照常报错。
        let r = self.with_cmd(id, ctx, connect, f).await;
        if r.is_ok() {
            eprintln!("账号 {id} 的读信连接打不开，暂时改用命令连接：{refused:#}");
            let wait = if download_protocol::connection_problem(&refused) {
                READER_RETRY_AFTER_NETWORK
            } else {
                READER_RETRY_AFTER
            };
            self.no_reader
                .lock()
                .unwrap()
                .insert(id.to_string(), Instant::now() + wait);
        }
        r
    }

    /// 读信连接现在能不能用（开不出来时读信退回命令连接）
    pub fn reader_available(&self, id: &str) -> bool {
        !self
            .no_reader
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|until| Instant::now() < *until)
    }

    /// 删除账号：丢掉它的连接（进行中的操作结束后连接随之关闭）。
    pub fn remove_account(&self, id: &str) {
        self.cmd.lock().unwrap().remove(id);
        self.reader.lock().unwrap().remove(id);
        self.no_reader.lock().unwrap().remove(id);
    }
}

/// 每个账号「打开邮件」的序号。下载原文要排队，排到时发现用户已经点了别的邮件，
/// 就不必再下这一封（正在下载的那封不打断：打断会弄坏连接）。
#[derive(Default)]
pub struct Turns(Mutex<HashMap<String, u64>>);

impl Turns {
    /// 用户打开了一封邮件：领一个新序号
    pub fn next(&self, id: &str) -> u64 {
        let mut map = self.0.lock().unwrap();
        let n = map.entry(id.to_string()).or_default();
        *n += 1;
        *n
    }

    /// 当前序号（预取用：不算用户操作）
    pub fn current(&self, id: &str) -> u64 {
        self.0.lock().unwrap().get(id).copied().unwrap_or(0)
    }

    pub fn is_current(&self, id: &str, turn: u64) -> bool {
        self.current(id) == turn
    }
}

/// 数着收到了多少字节的连接，下载大邮件时给界面报进度
#[derive(Debug)]
pub struct Counting<S> {
    inner: S,
    received: Arc<AtomicU64>,
}

impl<S> Counting<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            received: Arc::default(),
        }
    }

    /// 这条连接到目前为止收到的字节数（一直累加）
    pub fn received(&self) -> Arc<AtomicU64> {
        self.received.clone()
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Counting<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let r = Pin::new(&mut self.inner).poll_read(cx, buf);
        let got = buf.filled().len() - before;
        self.received.fetch_add(got as u64, Ordering::Relaxed);
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Counting<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut task::Context<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// 下载进度多久报一次
pub const PROGRESS_EVERY: Duration = Duration::from_millis(300);

/// 执行 `work` 的同时，每隔一会儿报一次这期间连接收到了多少字节。
/// 很快就完成的（比如命中缓存）一次都不报。
pub async fn with_progress<R>(
    received: Arc<AtomicU64>,
    report: impl Fn(u64),
    work: impl Future<Output = R>,
) -> R {
    let start = received.load(Ordering::Relaxed);
    tokio::pin!(work);
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + PROGRESS_EVERY, PROGRESS_EVERY);
    loop {
        tokio::select! {
            r = &mut work => return r,
            _ = tick.tick() => report(received.load(Ordering::Relaxed) - start),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_imap;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::DuplexStream;

    fn server() -> test_imap::Shared {
        let s = test_imap::state("IMAP4rev1", &[("INBOX", 1)]);
        s.lock()
            .unwrap()
            .add("INBOX", "Subject: a\r\n\r\nbody\r\n", &[]);
        s
    }

    #[tokio::test]
    async fn reading_does_not_wait_for_a_busy_command_connection() {
        let lanes: Arc<Lanes<DuplexStream>> = Arc::default();
        let srv = server();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        // 命令连接被一批慢摘要占着
        let busy = {
            let (lanes, srv) = (lanes.clone(), srv.clone());
            tokio::spawn(async move {
                let connect = || test_imap::session(srv.clone());
                let connect = || async { Ok(connect().await) };
                lanes
                    .with_cmd("a", &(), connect, |_, _| {
                        Box::pin(async move {
                            started_tx.send(()).unwrap();
                            release_rx.await.unwrap();
                            Ok(())
                        })
                    })
                    .await
            })
        };
        started_rx.await.unwrap();
        let connect = || async { Ok(test_imap::session(srv.clone()).await) };
        let read = lanes.with_reader("a", &(), connect, |c, _| {
            Box::pin(async move { c.select("INBOX").await })
        });
        timeout(Duration::from_secs(2), read)
            .await
            .expect("读信不应等命令连接")
            .unwrap();
        release_tx.send(()).unwrap();
        busy.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn reader_falls_back_to_the_command_connection_when_it_cannot_log_in() {
        let lanes: Lanes<DuplexStream> = Lanes::default();
        let srv = server();
        let logins = AtomicUsize::new(0);
        // 第二条连接登录时被拒（连接数超限：服务器回 NO，login() 报成 AuthFailed），之后的都成功
        let connect = || {
            let n = logins.fetch_add(1, Ordering::SeqCst);
            let srv = srv.clone();
            async move {
                if n == 1 {
                    return Err(imap::AuthFailed("Too many simultaneous connections".into()).into());
                }
                Ok(test_imap::session(srv).await)
            }
        };
        lanes
            .with_cmd("a", &(), connect, |c, _| Box::pin(c.select("INBOX")))
            .await
            .unwrap();
        lanes
            .with_reader("a", &(), connect, |c, _| Box::pin(c.select("INBOX")))
            .await
            .expect("退回命令连接照样读信");
        assert_eq!(logins.load(Ordering::SeqCst), 2, "复用已有的命令连接");
        assert!(!lanes.reader_available("a"));
        assert!(lanes.reader_available("b"));
        // 一段时间内不再反复尝试读信连接
        lanes
            .with_reader("a", &(), connect, |c, _| Box::pin(c.select("INBOX")))
            .await
            .unwrap();
        assert_eq!(logins.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn wrong_password_is_reported_and_the_reader_is_tried_again_next_time() {
        let lanes: Lanes<DuplexStream> = Lanes::default();
        let logins = AtomicUsize::new(0);
        let connect = || {
            logins.fetch_add(1, Ordering::SeqCst);
            async {
                Err::<Session<DuplexStream>, _>(imap::AuthFailed("密码错误".into()).into())
            }
        };
        for round in 1..=2 {
            let r = lanes
                .with_reader("a", &(), connect, |c, _| Box::pin(c.select("INBOX")))
                .await;
            assert!(r.unwrap_err().downcast_ref::<imap::AuthFailed>().is_some());
            // 读信连接和命令连接都试过；两条都不行不算「不让多开」，下次照样先试读信连接
            assert_eq!(logins.load(Ordering::SeqCst), round * 2);
        }
    }

    #[test]
    fn a_newer_open_supersedes_older_ones_per_account() {
        let turns = Turns::default();
        let first = turns.next("a");
        assert!(turns.is_current("a", first));
        let other = turns.next("b");
        assert!(turns.is_current("a", first), "别的账号不影响");
        let second = turns.next("a");
        assert!(!turns.is_current("a", first));
        assert!(turns.is_current("a", second));
        assert!(turns.is_current("b", other));
        assert_eq!(turns.current("c"), 0);
    }

    #[tokio::test]
    async fn counting_stream_counts_what_the_server_sent() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (a, mut b) = tokio::io::duplex(1024);
        let mut a = Counting::new(a);
        let received = a.received();
        b.write_all(b"hello world").await.unwrap();
        let mut buf = [0; 5];
        a.read_exact(&mut buf).await.unwrap();
        a.write_all(b"sent bytes are not counted").await.unwrap();
        assert_eq!(received.load(Ordering::Relaxed), 5);
    }

    #[tokio::test(start_paused = true)]
    async fn progress_is_reported_while_a_download_runs_but_not_for_quick_ones() {
        let received = Arc::new(AtomicU64::new(1000));
        let reports = Mutex::new(Vec::new());
        let r = with_progress(
            received.clone(),
            |n| reports.lock().unwrap().push(n),
            async { "cached" },
        )
        .await;
        assert_eq!(r, "cached");
        assert!(reports.lock().unwrap().is_empty());

        let counter = received.clone();
        with_progress(
            received.clone(),
            |n| reports.lock().unwrap().push(n),
            async move {
                for _ in 0..3 {
                    counter.fetch_add(100, Ordering::Relaxed);
                    tokio::time::sleep(PROGRESS_EVERY).await;
                }
            },
        )
        .await;
        let got = reports.lock().unwrap().clone();
        assert!(!got.is_empty());
        assert!(got.windows(2).all(|w| w[0] <= w[1]), "{got:?}");
        assert!(got.iter().all(|n| *n <= 300), "只算这次下载的：{got:?}");
    }
}
