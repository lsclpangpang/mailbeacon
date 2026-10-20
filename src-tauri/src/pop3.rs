//! Minimal POP3 receive support.
//!
//! POP3 has a single mailbox and no server-side read/flag/folder operations.  We
//! keep the protocol deliberately small: implicit TLS (normally port 995) or
//! plain TCP for local/test servers, USER/PASS authentication, UIDL/LIST and
//! RETR/TOP.  POP3 message numbers are renumbered whenever mail is deleted, so
//! the app never exposes them: [`Mailbox`] assigns each UIDL a local id that
//! stays fixed for the lifetime of the process, and [`Session`] maps it back to
//! the current message number on every connection.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, anyhow, bail};
use mail_parser::{Address, MessageParser, MimeHeaders};
use rustls_pki_types::ServerName;
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    net::TcpStream,
    sync::{Mutex, OwnedMutexGuard},
    time::timeout,
};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls};

use crate::{
    config::Account,
    imap::{MessageView, Preview, Summary},
};

const OP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_LINE: usize = 1024 * 1024;
const MAX_MESSAGE: usize = 64 * 1024 * 1024;
/// 搜索时未缓存的邮件用 TOP 读取邮件头和正文开头，每批之间让出连接。
const SEARCH_BATCH: usize = 50;
const SEARCH_TOP_LINES: u32 = 100;
/// 缓存并参与搜索的正文开头长度
const SEARCH_TEXT_CHARS: usize = 500;
/// 用完的连接保留这么久，紧接着的操作（列表→摘要→打开）不用重新登录
const CONN_IDLE: Duration = Duration::from_secs(10);
/// POP3 会话内看不到之后到达的邮件，复用的连接超过这个时长就重新登录
const CONN_MAX_AGE: Duration = Duration::from_secs(60);

trait MailIo: AsyncRead + AsyncWrite {}
impl<T: AsyncRead + AsyncWrite + ?Sized> MailIo for T {}
type BoxIo = Box<dyn MailIo + Send + Unpin>;

type TlsIo = TlsStream<TcpStream>;

fn tls() -> TlsConnector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    if let Some(pem) = crate::imap::extra_ca_pem() {
        use rustls_pki_types::{CertificateDer, pem::PemObject};
        for cert in CertificateDer::pem_slice_iter(&pem).flatten() {
            let _ = roots.add(cert);
        }
    }
    TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("TLS 配置")
        .with_root_certificates(roots)
        .with_no_client_auth(),
    ))
}

#[derive(Debug)]
pub struct AuthFailed(String);
impl std::fmt::Display for AuthFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "POP3 登录失败：{}（请确认填的是授权码/应用专用密码）",
            self.0
        )
    }
}
impl std::error::Error for AuthFailed {}

/// 单封邮件读取失败（服务器回 -ERR 或超过大小上限），连接本身仍可继续使用。
#[derive(Debug)]
pub struct MessageError(String);
impl std::fmt::Display for MessageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for MessageError {}

/// 邮箱被其他会话锁定、服务器临时故障等不是密码问题，可以稍后重试
/// （RFC 2449 的 [IN-USE]、[SYS/TEMP] 响应码，以及锁定邮箱时的常见措辞）。
/// 带 [AUTH] 的一律是认证失败；"blocked"、"try again later" 之类可能是风控，不重试。
fn is_transient_login_error(line: &str) -> bool {
    let upper = line.to_ascii_uppercase();
    if upper.contains("[AUTH]") {
        return false;
    }
    [
        "[IN-USE]",
        "[SYS/TEMP]",
        "MAILDROP",
        "ALREADY LOCKED",
        "UNABLE TO LOCK",
        "IN USE",
    ]
    .iter()
    .any(|word| upper.contains(word))
}

/// An authenticated POP3 connection.
pub struct Pop3 {
    stream: BufReader<BoxIo>,
}

impl std::fmt::Debug for Pop3 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pop3").finish_non_exhaustive()
    }
}

impl Pop3 {
    pub async fn login(account: &Account, password: &str) -> Result<Self> {
        let host = if account.pop3_host.trim().is_empty() {
            bail!("未配置 POP3 服务器")
        } else {
            account.pop3_host.trim()
        };
        let tcp = timeout(OP_TIMEOUT, TcpStream::connect((host, account.pop3_port)))
            .await
            .context("连接 POP3 服务器超时")?
            .with_context(|| format!("无法连接 POP3 服务器 {host}"))?;
        tcp.set_nodelay(true)?;
        let io: BoxIo = if account.pop3_tls {
            let name = ServerName::try_from(host.to_string())?;
            let stream: TlsIo = timeout(OP_TIMEOUT, tls().connect(name, tcp))
                .await
                .context("POP3 TLS 握手超时")??;
            Box::new(stream)
        } else {
            Box::new(tcp)
        };
        let mut this = Self {
            stream: BufReader::new(io),
        };
        let greeting = this.line().await?;
        if !greeting.starts_with("+OK") {
            bail!("POP3 服务器拒绝连接：{greeting}");
        }
        for command in [
            format!("USER {}", pop3_atom(&account.email)?),
            format!("PASS {}", pop3_pass(password)?),
        ] {
            let line = this.command_line(&command).await?;
            if line.starts_with("+OK") {
                continue;
            }
            if is_transient_login_error(&line) {
                bail!("POP3 邮箱暂时不可用：{line}");
            }
            return Err(AuthFailed(line).into());
        }
        Ok(this)
    }

    async fn line_bytes(&mut self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        let n = timeout(OP_TIMEOUT, self.stream.read_until(b'\n', &mut bytes))
            .await
            .context("POP3 读取超时")??;
        if n == 0 {
            bail!("POP3 服务器已断开连接");
        }
        if bytes.len() > MAX_LINE {
            bail!("POP3 响应行过长");
        }
        if bytes.ends_with(b"\n") {
            bytes.pop();
        }
        if bytes.ends_with(b"\r") {
            bytes.pop();
        }
        Ok(bytes)
    }

    async fn line(&mut self) -> Result<String> {
        String::from_utf8(self.line_bytes().await?).context("POP3 响应不是有效 UTF-8")
    }

    async fn command_line(&mut self, command: &str) -> Result<String> {
        self.stream.get_mut().write_all(command.as_bytes()).await?;
        self.stream.get_mut().write_all(b"\r\n").await?;
        self.stream.get_mut().flush().await?;
        self.line().await
    }

    async fn command_ok(&mut self, command: &str) -> Result<()> {
        let line = self.command_line(command).await?;
        if line.starts_with("+OK") {
            Ok(())
        } else {
            bail!("{line}")
        }
    }

    async fn multiline(&mut self, command: &str) -> Result<Vec<u8>> {
        let first = self.command_line(command).await?;
        if !first.starts_with("+OK") {
            return Err(MessageError(first).into());
        }
        let mut out = Vec::new();
        let mut too_large = false;
        loop {
            let mut line = self.line_bytes().await?;
            if line == b"." {
                break;
            }
            // Dot-stuffing: a line beginning with two dots represents one dot.
            if line.first() == Some(&b'.') {
                line.remove(0);
            }
            // 超限后仍读完剩余行，让连接停在下一条命令的边界上。
            if too_large || out.len().saturating_add(line.len()).saturating_add(2) > MAX_MESSAGE {
                too_large = true;
                out.clear();
                continue;
            }
            out.extend_from_slice(&line);
            out.extend_from_slice(b"\r\n");
        }
        if too_large {
            return Err(MessageError("POP3 邮件超过 64 MB，已拒绝读取".into()).into());
        }
        Ok(out)
    }

    /// Return sequence number -> stable server UIDL.  Sequence numbers are
    /// sorted to keep UI order deterministic.
    pub async fn uidls(&mut self) -> Result<BTreeMap<u32, String>> {
        let data = self.multiline("UIDL").await?;
        let text = String::from_utf8(data).context("POP3 UIDL 响应编码错误")?;
        let mut out = BTreeMap::new();
        for line in text.lines() {
            let mut p = line.splitn(2, ' ');
            let Some(n) = p.next().and_then(|s| s.parse::<u32>().ok()) else {
                continue;
            };
            let Some(uid) = p.next().filter(|s| !s.is_empty()) else {
                continue;
            };
            out.insert(n, uid.to_string());
        }
        Ok(out)
    }

    pub async fn retrieve(&mut self, sequence: u32) -> Result<Vec<u8>> {
        if sequence == 0 {
            bail!("POP3 邮件序号无效");
        }
        self.multiline(&format!("RETR {sequence}")).await
    }

    /// 邮件头加正文开头若干行。
    pub async fn top(&mut self, sequence: u32, lines: u32) -> Result<Vec<u8>> {
        if sequence == 0 {
            bail!("POP3 邮件序号无效");
        }
        self.multiline(&format!("TOP {sequence} {lines}")).await
    }

    /// 序号 -> 字节数，用于在下载前拒绝超大邮件。
    pub async fn sizes(&mut self) -> Result<BTreeMap<u32, u64>> {
        let data = self.multiline("LIST").await?;
        let text = String::from_utf8(data).context("POP3 LIST 响应编码错误")?;
        Ok(text
            .lines()
            .filter_map(|line| {
                let mut p = line.split_whitespace();
                Some((p.next()?.parse().ok()?, p.next()?.parse().ok()?))
            })
            .collect())
    }

    pub async fn quit(mut self) -> Result<()> {
        let _ = timeout(OP_TIMEOUT, self.command_ok("QUIT")).await;
        Ok(())
    }
}

fn pop3_atom(value: &str) -> Result<String> {
    if value.is_empty() || value.bytes().any(|b| b == b'\r' || b == b'\n' || b <= 0x20) {
        bail!("POP3 登录参数包含非法字符");
    }
    Ok(value.to_string())
}

/// RFC 1939 的 PASS 参数是整行剩余部分，可以包含空格（如带空格的应用专用密码）。
fn pop3_pass(value: &str) -> Result<String> {
    if value.is_empty() || value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        bail!("POP3 登录参数包含非法字符");
    }
    Ok(value.to_string())
}

/// 一封邮件的列表信息，按 UIDL 持久化；`complete` 为 false 表示只根据 TOP 读到的开头生成。
#[derive(Clone, Serialize, Deserialize)]
struct Meta {
    from_name: String,
    from_addr: String,
    to: String,
    subject: String,
    date: i64,
    attachment: bool,
    /// 正文开头的纯文本（空白已合并），列表摘要和搜索都用它
    text: String,
    complete: bool,
    /// 收件人和抄送地址（旧版缓存里没有）
    #[serde(default)]
    recipients: Vec<String>,
    /// 会话信息（旧版缓存里没有）
    #[serde(default)]
    message_id: String,
    #[serde(default)]
    in_reply_to: String,
    #[serde(default)]
    references: Vec<String>,
}

/// 未读状态：默认已读，只记首次同步时最新一页和之后新到的邮件里还没读的。
/// 点开邮件时只改写这个小文件。
#[derive(Default, Serialize, Deserialize)]
struct ReadState {
    initialized: bool,
    unread: HashSet<String>,
    /// 与 known.json 里的 token 一致时，那份已知 UIDL 才可信
    #[serde(default)]
    known_token: u64,
}

/// 已知 UIDL 和 read.json 分开写；token 用来发现两者不同步（写入失败、文件损坏）。
#[derive(Default, Serialize, Deserialize)]
struct KnownState {
    token: u64,
    uidls: HashSet<String>,
}

struct Conn {
    pop: Pop3,
    /// 本次登录看到的 id -> 序号
    current: BTreeMap<u32, u32>,
    /// 序号 -> 字节数（LIST）
    sizes: BTreeMap<u32, u64>,
    opened: Instant,
    last_used: Instant,
}

/// 读取方式：整封、邮件头加正文开头、只有邮件头。
#[derive(Clone, Copy, PartialEq)]
enum Fetch {
    Full,
    Partial,
    Headers,
}

/// 一个 POP3 账号的状态，跨连接保留；未读状态、已知 UIDL 和列表缓存写入磁盘。
#[derive(Default)]
pub struct Mailbox {
    ids: HashMap<String, u32>,
    uidls: HashMap<u32, String>,
    next_id: u32,
    read: ReadState,
    /// 上次同步时服务器上的全部 UIDL，用来识别新邮件；只在邮箱变化时写盘
    known: HashSet<String>,
    /// known 与未读状态来自同一次保存；不可信时下次同步不据此判断新邮件
    known_trusted: bool,
    meta: HashMap<String, Meta>,
    /// 本进程内连邮件头都读不出来的邮件，不再反复尝试
    failed: HashSet<String>,
    /// 保存位置（不含扩展名）；为空时只保存在内存中
    path: Option<PathBuf>,
    read_dirty: bool,
    known_dirty: bool,
    meta_changes: usize,
    meta_saved: Option<Instant>,
    /// 用完后短暂保留的连接，紧接着的操作不用重新登录
    conn: Option<Conn>,
}

/// 状态文件在后台线程写入；每个文件按提交顺序编号，旧内容不会覆盖新内容。
#[derive(Default)]
struct Writer {
    next: AtomicU64,
    written: std::sync::Mutex<HashMap<PathBuf, u64>>,
}

impl Writer {
    fn submit(self: &Arc<Self>, path: PathBuf, bytes: Vec<u8>) {
        let version = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        let writer = self.clone();
        let job = move || {
            let mut written = writer.written.lock().unwrap();
            if written.get(&path).is_some_and(|done| *done >= version) {
                return;
            }
            match write_bytes(&path, &bytes) {
                Ok(()) => {
                    written.insert(path, version);
                }
                Err(e) => eprintln!("保存 {} 失败：{e:#}", path.display()),
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => drop(handle.spawn_blocking(job)),
            Err(_) => job(),
        }
    }

    /// 删除文件，并让还在排队的写入全部作废。
    fn remove(&self, paths: &[PathBuf]) {
        let mut written = self.written.lock().unwrap();
        for path in paths {
            let _ = std::fs::remove_file(path);
            written.insert(path.clone(), u64::MAX);
        }
    }
}

/// 一个 POP3 账号的共享句柄。取消搜索和删除账号不需要等账号锁。
pub struct Handle {
    mailbox: Arc<Mutex<Mailbox>>,
    search_generation: AtomicU64,
    removed: AtomicBool,
    writer: Arc<Writer>,
}

pub type SharedMailbox = Arc<Handle>;

/// 账号 id 来自本地配置，这里仍只保留文件名安全的字符。
fn state_path(dir: &Path, account_id: &str) -> PathBuf {
    let name: String = account_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    dir.join("pop3").join(name)
}

fn state_files(base: &Path) -> [PathBuf; 3] {
    [
        base.with_extension("read.json"),
        base.with_extension("known.json"),
        base.with_extension("cache.json"),
    ]
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            eprintln!("{} 无法解析，已忽略：{e}", path.display());
            T::default()
        }),
        Err(_) => T::default(),
    }
}

fn write_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// 删除账号时清掉保存的 POP3 状态（账号没有加载过 POP3 句柄时用）。
pub fn remove_state(dir: &Path, account_id: &str) {
    for path in state_files(&state_path(dir, account_id)) {
        let _ = std::fs::remove_file(path);
    }
}

impl Handle {
    pub fn load(dir: &Path, account_id: &str) -> Self {
        let base = state_path(dir, account_id);
        let [read, known, cache] = state_files(&base);
        let read: ReadState = read_json(&read);
        let known: KnownState = read_json(&known);
        let mailbox = Mailbox {
            // 非空的 known 即使比 read.json 旧也可用（只有排在它之后的才算新邮件）；
            // 空集合只有 token 对得上才可信，否则可能是文件丢失或损坏。
            known_trusted: !known.uidls.is_empty()
                || (known.token != 0 && known.token == read.known_token),
            read,
            known: known.uidls,
            meta: read_json(&cache),
            path: Some(base),
            ..Mailbox::default()
        };
        Self::new(mailbox)
    }

    fn new(mailbox: Mailbox) -> Self {
        Self {
            mailbox: Arc::new(Mutex::new(mailbox)),
            search_generation: AtomicU64::new(0),
            removed: AtomicBool::new(false),
            writer: Arc::default(),
        }
    }

    /// 让正在进行的搜索在下一封邮件前停止。
    pub fn cancel_search(&self) {
        self.search_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// 在等账号锁之前领取搜索代次：排队期间的取消或更新的搜索都会让它作废。
    pub fn begin_search(&self) -> u64 {
        self.search_generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub async fn set_read(&self, ids: &[u32], value: bool) {
        let mut mailbox = self.mailbox.lock().await;
        mailbox.set_read(ids, value);
        mailbox.save(&self.writer, false);
    }

    /// 账号已删除：停止搜索，不再登录或写盘，删除状态文件。
    pub async fn forget(&self, dir: &Path, account_id: &str) {
        self.removed.store(true, Ordering::SeqCst);
        self.cancel_search();
        let conn = {
            let mut mailbox = self.mailbox.lock().await;
            mailbox.path = None;
            mailbox.conn.take()
        };
        self.writer
            .remove(&state_files(&state_path(dir, account_id)));
        if let Some(conn) = conn {
            let _ = conn.pop.quit().await;
        }
    }

    fn removed(&self) -> bool {
        self.removed.load(Ordering::SeqCst)
    }
}

impl Mailbox {
    /// 状态有变化时提交后台写入。列表缓存可能很大，攒够一批或隔一段时间才写，
    /// `force` 时立即写。
    fn save(&mut self, writer: &Arc<Writer>, force: bool) {
        let Some(base) = self.path.clone() else {
            return;
        };
        let [read, known, cache] = state_files(&base);
        if self.known_dirty {
            // 新 token 同时写进两个文件；任一写入失败，下次加载时就能发现。
            let token = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_nanos() as u64)
                .unwrap_or(1)
                .max(self.read.known_token + 1);
            let state = KnownState {
                token,
                uidls: std::mem::take(&mut self.known),
            };
            if let Ok(bytes) = serde_json::to_vec(&state) {
                writer.submit(known, bytes);
            }
            self.known = state.uidls;
            self.read.known_token = token;
            self.read_dirty = true;
            self.known_dirty = false;
        }
        if self.read_dirty {
            if let Ok(bytes) = serde_json::to_vec(&self.read) {
                writer.submit(read, bytes);
            }
            self.read_dirty = false;
        }
        let due = force
            || self.meta_changes >= 100
            || self
                .meta_saved
                .is_none_or(|saved| saved.elapsed() > Duration::from_secs(30));
        if self.meta_changes > 0 && due {
            if let Ok(bytes) = serde_json::to_vec(&self.meta) {
                writer.submit(cache, bytes);
            }
            self.meta_changes = 0;
            self.meta_saved = Some(Instant::now());
        }
    }

    /// 按序号顺序给新 UIDL 分配 id，返回当前 id -> 序号。
    fn sync(&mut self, uidls: &BTreeMap<u32, String>) -> BTreeMap<u32, u32> {
        let mut current = BTreeMap::new();
        for (sequence, uidl) in uidls {
            let id = match self.ids.get(uidl) {
                Some(id) => *id,
                None => {
                    self.next_id += 1;
                    self.ids.insert(uidl.clone(), self.next_id);
                    self.uidls.insert(self.next_id, uidl.clone());
                    self.next_id
                }
            };
            current.insert(id, *sequence);
        }
        let present: HashSet<String> = uidls.values().cloned().collect();
        if !self.read.initialized {
            // 首次同步：最新一页算未读，更早的旧邮件算已读。
            let backlog = uidls.len().saturating_sub(crate::imap::PAGE);
            self.read.unread = uidls.values().skip(backlog).cloned().collect();
            self.read.initialized = true;
            self.read_dirty = true;
            // 邮箱为空时也要写出 known.json 和 token，重启后才能识别期间到达的邮件。
            self.known_dirty = true;
        } else if self.known_trusted {
            // 新邮件总是排在最后。只有序号在所有已知邮件之后的未知 UIDL 才算新到，
            // 服务器暂时漏报后又出现的旧邮件不会被重新标成未读。
            let last_known = uidls
                .iter()
                .filter(|(_, uidl)| self.known.contains(*uidl))
                .map(|(sequence, _)| *sequence)
                .max()
                .unwrap_or(0);
            for (sequence, uidl) in uidls {
                if *sequence > last_known && !self.known.contains(uidl) {
                    self.read_dirty |= self.read.unread.insert(uidl.clone());
                }
            }
        }
        self.known_trusted = true;
        if self.known != present {
            self.known = present;
            self.known_dirty = true;
        }
        let known = &self.known;
        let unread = self.read.unread.len();
        self.read.unread.retain(|uidl| known.contains(uidl));
        self.read_dirty |= self.read.unread.len() != unread;
        let meta = self.meta.len();
        self.meta.retain(|uidl, _| known.contains(uidl));
        self.meta_changes += meta - self.meta.len();
        current
    }

    fn is_read(&self, id: u32) -> bool {
        self.uidls
            .get(&id)
            .is_none_or(|uidl| !self.read.unread.contains(uidl))
    }

    fn set_read(&mut self, ids: &[u32], value: bool) {
        for id in ids {
            let Some(uidl) = self.uidls.get(id) else {
                continue;
            };
            let changed = if value {
                self.read.unread.remove(uidl)
            } else {
                self.read.unread.insert(uidl.clone())
            };
            self.read_dirty |= changed;
        }
    }

    fn meta(&self, id: u32) -> Option<&Meta> {
        self.uidls.get(&id).and_then(|uidl| self.meta.get(uidl))
    }

    fn summary(&self, account_id: &str, id: u32) -> Option<Summary> {
        self.meta(id).map(|meta| Summary {
            account_id: account_id.into(),
            folder: "INBOX".into(),
            uid: id,
            from_name: meta.from_name.clone(),
            from_addr: meta.from_addr.clone(),
            to: meta.to.clone(),
            subject: meta.subject.clone(),
            date: meta.date,
            seen: self.is_read(id),
            flagged: false,
            attachment: meta.attachment,
            recipients: meta.recipients.clone(),
            message_id: meta.message_id.clone(),
            in_reply_to: meta.in_reply_to.clone(),
            references: meta.references.clone(),
        })
    }
}

/// 空闲 CONN_IDLE 后关闭保留的连接；QUIT 在锁外发送，不挡住其他操作。
async fn reap_idle(handle: SharedMailbox) {
    tokio::time::sleep(CONN_IDLE).await;
    let conn = {
        let mut mailbox = handle.mailbox.lock().await;
        let idle = mailbox
            .conn
            .as_ref()
            .is_some_and(|conn| conn.last_used.elapsed() >= CONN_IDLE);
        if idle { mailbox.conn.take() } else { None }
    };
    if let Some(conn) = conn {
        let _ = conn.pop.quit().await;
    }
}

/// 搜索被新的搜索、清空关键词或删除账号取消。
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("搜索已取消")
    }
}
impl std::error::Error for Cancelled {}

/// 持有账号锁的一次 POP3 操作：同一账号同一时刻只有一个会话在用连接，
/// 避免服务器因为邮箱被锁定而拒绝并发登录。
pub struct Session {
    handle: SharedMailbox,
    /// 只在搜索分批让出锁的瞬间为空
    guard: Option<OwnedMutexGuard<Mailbox>>,
    account: Account,
    password: String,
    /// 搜索时的代次；其他操作为空
    search: Option<u64>,
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(mut mailbox) = self.guard.take() else {
            return;
        };
        mailbox.save(&self.handle.writer, false);
        let keep = mailbox
            .conn
            .as_mut()
            .map(|conn| conn.last_used = Instant::now());
        drop(mailbox);
        if keep.is_some() {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(reap_idle(self.handle.clone()));
            }
        }
    }
}

impl Session {
    /// `fresh` 为 true 时总是重新登录（检查新邮件）；否则可复用最近的连接。
    pub async fn open(
        handle: SharedMailbox,
        account: &Account,
        password: &str,
        fresh: bool,
    ) -> Result<Self> {
        let guard = handle.mailbox.clone().lock_owned().await;
        let mut session = Self {
            handle,
            guard: Some(guard),
            account: account.clone(),
            password: password.to_string(),
            search: None,
        };
        session.connect(fresh, false).await?;
        Ok(session)
    }

    fn mailbox(&self) -> &Mailbox {
        self.guard.as_ref().expect("POP3 会话持有账号锁")
    }

    fn mailbox_mut(&mut self) -> &mut Mailbox {
        self.guard.as_mut().expect("POP3 会话持有账号锁")
    }

    fn check_cancelled(&self) -> Result<()> {
        let superseded = self.search.is_some_and(|generation| {
            self.handle.search_generation.load(Ordering::SeqCst) != generation
        });
        if superseded || self.handle.removed() {
            return Err(Cancelled.into());
        }
        Ok(())
    }

    /// `ignore_age` 用于搜索：搜索不需要看到新邮件，连接不必因为用久了而重新登录。
    async fn connect(&mut self, fresh: bool, ignore_age: bool) -> Result<()> {
        if self.handle.removed() {
            bail!("账号已删除");
        }
        let reusable = self.mailbox().conn.as_ref().is_some_and(|conn| {
            conn.last_used.elapsed() < CONN_IDLE
                && (ignore_age || conn.opened.elapsed() < CONN_MAX_AGE)
        });
        if reusable && !fresh {
            return Ok(());
        }
        if let Some(old) = self.mailbox_mut().conn.take() {
            let _ = old.pop.quit().await;
        }
        let mut pop = Pop3::login(&self.account, &self.password).await?;
        let uidls = pop.uidls().await?;
        // 服务器不支持 LIST 时没有大小信息；其他错误（超时、断线）说明连接
        // 可能停在半条响应上，必须丢弃，否则后续命令会读到错位的回复。
        let sizes = match pop.sizes().await {
            Ok(sizes) => sizes,
            Err(e) if e.is::<MessageError>() => BTreeMap::new(),
            Err(e) => return Err(e),
        };
        let current = self.mailbox_mut().sync(&uidls);
        let now = Instant::now();
        self.mailbox_mut().conn = Some(Conn {
            pop,
            current,
            sizes,
            opened: now,
            last_used: now,
        });
        Ok(())
    }

    /// 让排队的其他操作（如打开邮件）先用连接，再取回锁继续。
    async fn yield_turn(&mut self) -> Result<()> {
        if let Some(mut mailbox) = self.guard.take() {
            mailbox.save(&self.handle.writer, false);
            if let Some(conn) = mailbox.conn.as_mut() {
                conn.last_used = Instant::now();
            }
        }
        tokio::task::yield_now().await;
        self.guard = Some(self.handle.mailbox.clone().lock_owned().await);
        self.check_cancelled()?;
        self.connect(false, true).await
    }

    fn current(&self) -> Option<&BTreeMap<u32, u32>> {
        self.mailbox().conn.as_ref().map(|conn| &conn.current)
    }

    /// 当前邮箱里的 id，从旧到新。
    pub fn ids(&self) -> Vec<u32> {
        self.current()
            .map(|current| current.keys().copied().collect())
            .unwrap_or_default()
    }

    pub fn uidl(&self, id: u32) -> Option<&str> {
        self.mailbox().uidls.get(&id).map(String::as_str)
    }

    pub fn unread(&self) -> usize {
        self.ids()
            .into_iter()
            .filter(|id| !self.mailbox().is_read(*id))
            .count()
    }

    pub fn set_read(&mut self, ids: &[u32], value: bool) {
        self.mailbox_mut().set_read(ids, value);
    }

    /// 读取一封邮件；邮件已不在服务器、超过大小上限或单封读取失败时返回 MessageError。
    /// 连接层面的错误会丢弃连接，下次重新登录。
    async fn fetch(&mut self, id: u32, how: Fetch) -> Result<Vec<u8>> {
        let mailbox = self.mailbox_mut();
        let conn = mailbox
            .conn
            .as_mut()
            .ok_or_else(|| anyhow!("POP3 连接已断开"))?;
        let sequence = conn
            .current
            .get(&id)
            .copied()
            .ok_or_else(|| MessageError("POP3 邮件已从服务器删除，请刷新后重试".into()))?;
        let too_large = conn
            .sizes
            .get(&sequence)
            .is_some_and(|size| *size > MAX_MESSAGE as u64);
        let result = match how {
            Fetch::Full if too_large => {
                Err(MessageError("POP3 邮件超过 64 MB，已拒绝读取".into()).into())
            }
            Fetch::Full => conn.pop.retrieve(sequence).await,
            Fetch::Headers => conn.pop.top(sequence, 0).await,
            Fetch::Partial => match conn.pop.top(sequence, SEARCH_TOP_LINES).await {
                // 不支持 TOP 的服务器：小邮件退回整封读取
                Err(e) if e.is::<MessageError>() && !too_large => conn.pop.retrieve(sequence).await,
                other => other,
            },
        };
        match &result {
            Ok(_) => conn.last_used = Instant::now(),
            Err(e) if e.is::<MessageError>() => {}
            Err(_) => mailbox.conn = None,
        }
        result
    }

    fn remember(&mut self, id: u32, raw: &[u8], how: Fetch) -> Result<()> {
        let summary = summary_from_raw(raw, &self.account.id, id, 0)?;
        // 只读到开头时 MIME 结构不完整，按邮件头判断是否可能带附件。
        let attachment = summary.attachment || (how != Fetch::Full && looks_like_attachment(raw));
        let meta = Meta {
            from_name: summary.from_name,
            from_addr: summary.from_addr,
            to: summary.to,
            subject: summary.subject,
            date: summary.date,
            attachment,
            text: if how == Fetch::Headers {
                String::new()
            } else {
                text_from_raw(raw, SEARCH_TEXT_CHARS)
            },
            // 只能读邮件头的邮件（太大或 RETR 失败）不再尝试整封读取
            complete: how != Fetch::Partial,
            recipients: summary.recipients,
            message_id: summary.message_id,
            in_reply_to: summary.in_reply_to,
            references: summary.references,
        };
        let mailbox = self.mailbox_mut();
        if let Some(uidl) = mailbox.uidls.get(&id).cloned() {
            mailbox.meta.insert(uidl, meta);
            mailbox.meta_changes += 1;
        }
        Ok(())
    }

    /// 读取整封邮件原文，并顺带补全列表信息。
    pub async fn raw(&mut self, id: u32) -> Result<Vec<u8>> {
        let raw = self.fetch(id, Fetch::Full).await?;
        let complete = self.mailbox().meta(id).is_some_and(|meta| meta.complete);
        if !complete {
            let _ = self.remember(id, &raw, Fetch::Full);
        }
        Ok(raw)
    }

    /// 已缓存的邮件不再下载；读不出来的邮件退回只读邮件头，仍失败就跳过并记下。
    /// 返回是否访问了网络。
    async fn ensure(&mut self, ids: &[u32], partial: bool) -> Result<bool> {
        let mut fetched_any = false;
        for &id in ids {
            self.check_cancelled()?;
            let mailbox = self.mailbox();
            let Some(uidl) = mailbox.uidls.get(&id) else {
                continue;
            };
            let cached = mailbox.meta.get(uidl);
            if cached.is_some_and(|meta| meta.complete || partial) || mailbox.failed.contains(uidl)
            {
                continue;
            }
            fetched_any = true;
            let how = if partial { Fetch::Partial } else { Fetch::Full };
            let mut result = self.fetch(id, how).await.map(|raw| (raw, how));
            if matches!(&result, Err(e) if e.is::<MessageError>()) {
                result = self
                    .fetch(id, Fetch::Headers)
                    .await
                    .map(|raw| (raw, Fetch::Headers));
            }
            match result {
                Ok((raw, how)) => {
                    if let Err(e) = self.remember(id, &raw, how) {
                        eprintln!("POP3 邮件 {id} 无法解析，已跳过：{e:#}");
                    }
                }
                Err(e) if e.is::<MessageError>() => {
                    eprintln!("POP3 邮件 {id} 读取失败，已跳过：{e:#}");
                    let mailbox = self.mailbox_mut();
                    if let Some(uidl) = mailbox.uidls.get(&id).cloned() {
                        mailbox.failed.insert(uidl);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(fetched_any)
    }

    pub async fn summaries(&mut self, ids: &[u32]) -> Result<Vec<Summary>> {
        self.ensure(ids, false).await?;
        let mailbox = self.mailbox();
        Ok(ids
            .iter()
            .filter_map(|id| mailbox.summary(&self.account.id, *id))
            .collect())
    }

    pub async fn previews(&mut self, ids: &[u32]) -> Result<Vec<Preview>> {
        // 摘要只需要正文开头，搜索时用 TOP 读到的内容就够了。
        self.ensure(ids, true).await?;
        let mailbox = self.mailbox();
        Ok(ids
            .iter()
            .filter_map(|id| {
                mailbox.meta(*id).map(|meta| Preview {
                    uid: *id,
                    text: meta.text.chars().take(140).collect(),
                })
            })
            .collect())
    }

    /// 从新到旧搜索整个邮箱，匹配发件人、收件人、主题和正文开头。
    /// 未缓存的邮件分批用 TOP 读取（结果写入磁盘缓存），每批之间让出连接；
    /// 新的搜索、清空关键词或删除账号都会让这次搜索停止。
    pub async fn search(mut self, query: &str, generation: u64) -> Result<Vec<Summary>> {
        self.search = Some(generation);
        self.check_cancelled()?;
        let needle = query.to_lowercase();
        let ids: Vec<u32> = self.ids().into_iter().rev().collect();
        let mut found = Vec::new();
        for batch in ids.chunks(SEARCH_BATCH) {
            let fetched = self.ensure(batch, true).await?;
            let mailbox = self.mailbox();
            found.extend(
                batch
                    .iter()
                    .filter(|id| {
                        mailbox.meta(**id).is_some_and(|meta| {
                            [
                                &meta.from_name,
                                &meta.from_addr,
                                &meta.to,
                                &meta.subject,
                                &meta.text,
                            ]
                            .iter()
                            .any(|field| field.to_lowercase().contains(&needle))
                        })
                    })
                    .filter_map(|id| mailbox.summary(&self.account.id, *id)),
            );
            if fetched {
                self.yield_turn().await?;
            }
        }
        let writer = self.handle.writer.clone();
        self.mailbox_mut().save(&writer, true);
        Ok(found)
    }

    pub async fn close(self) {}
}

/// multipart/mixed 通常带附件（只看邮件头时的近似判断）。
fn looks_like_attachment(raw: &[u8]) -> bool {
    MessageParser::default()
        .parse_headers(raw)
        .and_then(|msg| {
            msg.content_type().map(|ct| {
                ct.ctype().eq_ignore_ascii_case("multipart")
                    && ct
                        .subtype()
                        .is_some_and(|sub| sub.eq_ignore_ascii_case("mixed"))
            })
        })
        .unwrap_or(false)
}

/// Parse a POP3 message into the same read view used by IMAP.
pub fn parse_message(raw: &[u8], fallback_date: i64) -> Result<MessageView> {
    crate::imap::parse_message(raw, fallback_date)
}

/// Build a list summary from a complete RETR result. POP3 has no flags, so
/// messages are always reported as unread until the UI reads them locally.
pub fn summary_from_raw(
    raw: &[u8],
    account_id: &str,
    sequence: u32,
    fallback_date: i64,
) -> Result<Summary> {
    let parser = MessageParser::default();
    let msg = parser
        .parse_headers(raw)
        .ok_or_else(|| anyhow!("无法解析 POP3 邮件头"))?;
    let first = |a: Option<&Address<'_>>| {
        a.and_then(|a| a.first())
            .map(|x| {
                (
                    x.name().unwrap_or("").to_string(),
                    x.address().unwrap_or("").to_string(),
                )
            })
            .unwrap_or_default()
    };
    let (from_name, from_addr) = first(msg.from());
    let to = msg
        .to()
        .map(|a| {
            a.iter()
                .map(|x| x.name().or(x.address()).unwrap_or("").to_string())
                .collect::<Vec<_>>()
                .join("、")
        })
        .unwrap_or_default();
    let attachment = crate::mime::parse(raw)
        .map(|m| m.attachments().next().is_some())
        .unwrap_or(false);
    let date = msg
        .date()
        .map(|d| d.to_timestamp() * 1000)
        .unwrap_or(fallback_date);
    let (message_id, in_reply_to, references) = crate::imap::thread_headers(Some(&msg));
    Ok(Summary {
        account_id: account_id.into(),
        folder: "INBOX".into(),
        uid: sequence,
        from_name,
        from_addr,
        to,
        subject: msg.subject().unwrap_or("").to_string(),
        date,
        seen: false,
        flagged: false,
        attachment,
        recipients: crate::imap::recipient_addresses(msg.to(), msg.cc()),
        message_id,
        in_reply_to,
        references,
    })
}

/// 正文开头的纯文本，空白合并为单个空格。
fn text_from_raw(raw: &[u8], chars: usize) -> String {
    MessageParser::default()
        .parse(raw)
        .and_then(|msg| msg.body_preview(chars * 2).map(|text| text.into_owned()))
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(chars)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn pop3_summaries_carry_recipient_addresses() {
        let raw = b"From: a@x\r\nTo: Me <ME@x.com>\r\nCc: c@x\r\nSubject: s\r\n\r\nbody\r\n";
        let s = summary_from_raw(raw, "acct", 1, 0).unwrap();
        assert_eq!(s.recipients, ["me@x.com", "c@x"]);
    }

    #[test]
    fn rejects_crlf_in_auth_atoms() {
        assert!(pop3_atom("user\r\nPASS x").is_err());
        assert!(pop3_atom(" ").is_err());
        assert!(pop3_pass("abc\r\nDELE 1").is_err());
        assert_eq!(
            pop3_pass("abcd efgh ijkl mnop").unwrap(),
            "abcd efgh ijkl mnop"
        );
    }

    #[test]
    fn locked_mailbox_is_not_an_auth_failure() {
        assert!(is_transient_login_error(
            "-ERR [IN-USE] maildrop already locked"
        ));
        assert!(is_transient_login_error("-ERR unable to lock maildrop"));
        assert!(!is_transient_login_error("-ERR [AUTH] invalid password"));
        assert!(!is_transient_login_error(
            "-ERR Login fail. Please using weixin token"
        ));
        assert!(!is_transient_login_error("-ERR account blocked"));
        assert!(!is_transient_login_error(
            "-ERR Authentication failed, please try again later"
        ));
        assert!(!is_transient_login_error("-ERR [AUTH] mailbox in use"));
    }

    #[test]
    fn local_ids_survive_renumbering() {
        let mut mailbox = Mailbox::default();
        let first = mailbox.sync(&BTreeMap::from([
            (1, "a".to_string()),
            (2, "b".to_string()),
            (3, "c".to_string()),
        ]));
        assert_eq!(first, BTreeMap::from([(1, 1), (2, 2), (3, 3)]));
        // "a" 被删除后服务器重新编号：c 仍是 id 3，只是序号变成 2。
        let second = mailbox.sync(&BTreeMap::from([
            (1, "b".to_string()),
            (2, "c".to_string()),
            (3, "d".to_string()),
        ]));
        assert_eq!(second, BTreeMap::from([(2, 1), (3, 2), (4, 3)]));
    }

    #[test]
    fn unread_ignores_backlog_and_tracks_local_reads() {
        let mut mailbox = Mailbox::default();
        let uidls: BTreeMap<u32, String> = (1..=150).map(|n| (n, format!("u{n}"))).collect();
        mailbox.sync(&uidls);
        assert!(mailbox.is_read(50));
        assert!(!mailbox.is_read(51));
        mailbox.set_read(&[150], true);
        assert!(mailbox.is_read(150));
        let unread = (1..=150).filter(|id| !mailbox.is_read(*id)).count();
        assert_eq!(unread, 99);
    }

    #[test]
    fn new_mail_after_first_sync_is_unread() {
        let mut mailbox = Mailbox::default();
        mailbox.sync(&BTreeMap::from([(1, "a".to_string())]));
        mailbox.set_read(&[1], true);
        let current = mailbox.sync(&BTreeMap::from([
            (1, "a".to_string()),
            (2, "b".to_string()),
        ]));
        assert!(mailbox.is_read(1));
        assert!(!mailbox.is_read(*current.keys().last().unwrap()));
    }

    #[test]
    fn read_state_and_cache_survive_restart_by_uidl() {
        let dir = std::env::temp_dir().join(format!("mailbeacon-pop3-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let uidls: BTreeMap<u32, String> = (1..=3).map(|n| (n, format!("u{n}"))).collect();
        {
            let handle = Handle::load(&dir, "acct/1");
            let mut mailbox = handle.mailbox.try_lock().unwrap();
            mailbox.sync(&uidls);
            mailbox.set_read(&[3], true);
            mailbox.meta.insert(
                "u2".into(),
                Meta {
                    from_name: "张三".into(),
                    from_addr: "z@example.com".into(),
                    to: String::new(),
                    subject: "合同".into(),
                    date: 1,
                    attachment: false,
                    text: "正文".into(),
                    complete: true,
                    recipients: Vec::new(),
                    message_id: String::new(),
                    in_reply_to: String::new(),
                    references: Vec::new(),
                },
            );
            mailbox.meta_changes = 1;
            mailbox.save(&handle.writer, true);
        }
        // 重启后服务器重新编号：u1 已删除，u3 变成序号 2。
        let handle = Handle::load(&dir, "acct/1");
        let mut mailbox = handle.mailbox.try_lock().unwrap();
        let current = mailbox.sync(&BTreeMap::from([
            (1, "u2".to_string()),
            (2, "u3".to_string()),
        ]));
        let id_of = |uidl: &str| *current.keys().find(|id| mailbox.uidls[id] == uidl).unwrap();
        assert!(mailbox.is_read(id_of("u3")));
        assert!(!mailbox.is_read(id_of("u2")));
        assert_eq!(
            mailbox.summary("acct/1", id_of("u2")).unwrap().subject,
            "合同"
        );
        drop(mailbox);
        handle
            .writer
            .remove(&state_files(&state_path(&dir, "acct/1")));
        assert!(
            !state_path(&dir, "acct/1")
                .with_extension("read.json")
                .exists()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 最小 POP3 服务器：messages 是 (UIDL, 原文, LIST 声明的大小)；
    /// TOP 每次延迟 `delay`，用来模拟慢速搜索。
    struct MockServer {
        port: u16,
        retr: Arc<std::sync::atomic::AtomicUsize>,
        top: Arc<std::sync::atomic::AtomicUsize>,
    }

    async fn mock_server(messages: Vec<(String, String, u64)>, delay: Duration) -> MockServer {
        use std::sync::atomic::AtomicUsize;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let retr = Arc::new(AtomicUsize::new(0));
        let top = Arc::new(AtomicUsize::new(0));
        let (retr_count, top_count) = (retr.clone(), top.clone());
        let messages = Arc::new(messages);
        tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let (messages, retr, top) =
                    (messages.clone(), retr_count.clone(), top_count.clone());
                tokio::spawn(async move {
                    let (read, mut write) = socket.into_split();
                    let mut lines = BufReader::new(read).lines();
                    write.write_all(b"+OK ready\r\n").await.unwrap();
                    while let Ok(Some(line)) = lines.next_line().await {
                        let parts: Vec<&str> = line.split(' ').collect();
                        let n = |i: usize| {
                            parts
                                .get(i)
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(0)
                        };
                        let reply = match parts[0] {
                            "USER" | "PASS" => "+OK\r\n".to_string(),
                            "QUIT" => {
                                let _ = write.write_all(b"+OK\r\n").await;
                                return;
                            }
                            "UIDL" => {
                                let mut r = String::from("+OK\r\n");
                                for (i, (uidl, _, _)) in messages.iter().enumerate() {
                                    r += &format!("{} {uidl}\r\n", i + 1);
                                }
                                r + ".\r\n"
                            }
                            "LIST" => {
                                let mut r = String::from("+OK\r\n");
                                for (i, (_, _, size)) in messages.iter().enumerate() {
                                    r += &format!("{} {size}\r\n", i + 1);
                                }
                                r + ".\r\n"
                            }
                            "RETR" | "TOP" => {
                                if parts[0] == "TOP" {
                                    top.fetch_add(1, Ordering::SeqCst);
                                    tokio::time::sleep(delay).await;
                                } else {
                                    retr.fetch_add(1, Ordering::SeqCst);
                                }
                                match messages.get(n(1).wrapping_sub(1)) {
                                    Some((_, raw, _)) => {
                                        let body = if parts[0] == "TOP" && n(2) == 0 {
                                            raw.split("\r\n\r\n").next().unwrap().to_string()
                                                + "\r\n\r\n"
                                        } else {
                                            raw.clone()
                                        };
                                        format!("+OK\r\n{body}.\r\n")
                                    }
                                    None => "-ERR no such message\r\n".into(),
                                }
                            }
                            _ => "-ERR unknown\r\n".into(),
                        };
                        if write.write_all(reply.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
        MockServer { port, retr, top }
    }

    fn mock_account(port: u16) -> Account {
        Account {
            id: "pop".into(),
            kind: "custom".into(),
            email: "me@localhost".into(),
            name: "测试".into(),
            signature: String::new(),
            auto_download: Default::default(),
            body_cache: Default::default(),
            receive_protocol: crate::config::ReceiveProtocol::Pop3,
            pop3_host: "127.0.0.1".into(),
            pop3_port: port,
            pop3_tls: false,
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
        }
    }

    fn mail(n: usize) -> String {
        format!(
            "From: a{n}@example.com\r\nSubject: subject {n}\r\nContent-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Type: text/plain\r\n\r\nbody {n}\r\n--b--\r\n"
        )
    }

    #[tokio::test]
    async fn oversize_message_is_listed_from_headers_without_download() {
        let server = mock_server(
            vec![
                ("small".into(), mail(1), 100),
                ("huge".into(), mail(2), 70 * 1024 * 1024),
            ],
            Duration::ZERO,
        )
        .await;
        let account = mock_account(server.port);
        let handle: SharedMailbox = Arc::new(Handle::new(Mailbox::default()));
        for _ in 0..2 {
            let mut session = Session::open(handle.clone(), &account, "pw", true)
                .await
                .unwrap();
            let ids = session.ids();
            let summaries = session.summaries(&ids).await.unwrap();
            assert_eq!(summaries.len(), 2);
            assert!(summaries[1].attachment, "按邮件头判断出附件");
            let err = session.raw(ids[1]).await.unwrap_err();
            assert!(err.is::<MessageError>());
        }
        // 小邮件只整封下载一次；超大邮件从不 RETR，只读一次邮件头。
        assert_eq!(server.retr.load(Ordering::SeqCst), 1);
        assert_eq!(server.top.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn search_is_cancelled_and_does_not_block_opening_mail() {
        let messages = (1..=200).map(|n| (format!("u{n}"), mail(n), 100)).collect();
        let server = mock_server(messages, Duration::from_millis(10)).await;
        let account = mock_account(server.port);
        let handle: SharedMailbox = Arc::new(Handle::new(Mailbox::default()));
        let generation = handle.begin_search();
        let session = Session::open(handle.clone(), &account, "pw", false)
            .await
            .unwrap();
        let search = tokio::spawn(session.search("subject 1", generation));
        tokio::time::sleep(Duration::from_millis(100)).await;
        // 搜索还在进行，打开邮件最多等一批。
        let started = Instant::now();
        let mut session = Session::open(handle.clone(), &account, "pw", false)
            .await
            .unwrap();
        session.raw(1).await.unwrap();
        drop(session);
        // 按顺序判断而不是只看耗时：打开邮件时搜索最多跑完两批，且尚未结束。
        let tops_when_opened = server.top.load(Ordering::SeqCst);
        assert!(tops_when_opened <= 2 * SEARCH_BATCH, "{tops_when_opened}");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!search.is_finished());
        handle.cancel_search();
        let Err(err) = search.await.unwrap() else {
            panic!("搜索应被取消");
        };
        assert!(err.is::<Cancelled>());
        assert!(server.top.load(Ordering::SeqCst) < 200);
    }

    #[tokio::test]
    async fn cancel_while_waiting_for_lock_is_not_lost() {
        let messages = (1..=20).map(|n| (format!("u{n}"), mail(n), 100)).collect();
        let server = mock_server(messages, Duration::ZERO).await;
        let account = mock_account(server.port);
        let handle: SharedMailbox = Arc::new(Handle::new(Mailbox::default()));
        // 另一个操作占着账号锁，搜索在排队；排队期间用户清空了关键词。
        let busy = Session::open(handle.clone(), &account, "pw", false)
            .await
            .unwrap();
        let generation = handle.begin_search();
        let waiting = {
            let (handle, account) = (handle.clone(), account.clone());
            tokio::spawn(async move {
                let session = Session::open(handle, &account, "pw", false).await?;
                session.search("x", generation).await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        handle.cancel_search();
        drop(busy);
        let Err(err) = waiting.await.unwrap() else {
            panic!("排队中的搜索应被取消");
        };
        assert!(err.is::<Cancelled>());
        assert_eq!(server.top.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn untrusted_known_set_does_not_mark_everything_unread() {
        let dir =
            std::env::temp_dir().join(format!("mailbeacon-pop3-known-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let uidls: BTreeMap<u32, String> = (1..=150).map(|n| (n, format!("u{n}"))).collect();
        {
            let handle = Handle::load(&dir, "a");
            let mut mailbox = handle.mailbox.try_lock().unwrap();
            mailbox.sync(&uidls);
            mailbox.save(&handle.writer, true);
        }
        // known.json 损坏：重启后不能把 1..=50 这些旧邮件全部当成新邮件。
        std::fs::write(
            state_path(&dir, "a").with_extension("known.json"),
            b"{broken",
        )
        .unwrap();
        let handle = Handle::load(&dir, "a");
        let mut mailbox = handle.mailbox.try_lock().unwrap();
        let current = mailbox.sync(&uidls);
        let unread = current.keys().filter(|id| !mailbox.is_read(**id)).count();
        assert_eq!(unread, 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mail_arriving_while_closed_after_empty_first_sync_is_unread() {
        let dir =
            std::env::temp_dir().join(format!("mailbeacon-pop3-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        {
            let handle = Handle::load(&dir, "a");
            let mut mailbox = handle.mailbox.try_lock().unwrap();
            mailbox.sync(&BTreeMap::new());
            mailbox.save(&handle.writer, true);
        }
        let handle = Handle::load(&dir, "a");
        let mut mailbox = handle.mailbox.try_lock().unwrap();
        let current = mailbox.sync(&BTreeMap::from([(1, "new".to_string())]));
        assert!(!mailbox.is_read(*current.keys().next().unwrap()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mail_missing_from_one_uidl_listing_is_not_new_when_it_returns() {
        let mut mailbox = Mailbox::default();
        let all: BTreeMap<u32, String> = (1..=3).map(|n| (n, format!("u{n}"))).collect();
        mailbox.sync(&all);
        mailbox.set_read(&[1, 2, 3], true);
        // 服务器有一次漏报了 u2
        mailbox.sync(&BTreeMap::from([
            (1, "u1".to_string()),
            (2, "u3".to_string()),
        ]));
        mailbox.sync(&all);
        assert!(mailbox.is_read(2));
        // 真正的新邮件排在最后，仍然是未读
        let mut more = all.clone();
        more.insert(4, "u4".into());
        let current = mailbox.sync(&more);
        assert!(!mailbox.is_read(*current.keys().last().unwrap()));
    }

    #[tokio::test]
    async fn removed_account_stops_logging_in() {
        let server = mock_server(vec![("u1".into(), mail(1), 100)], Duration::ZERO).await;
        let account = mock_account(server.port);
        let dir = std::env::temp_dir().join(format!("mailbeacon-pop3-rm-{}", std::process::id()));
        let handle: SharedMailbox = Arc::new(Handle::load(&dir, "pop"));
        drop(
            Session::open(handle.clone(), &account, "pw", false)
                .await
                .unwrap(),
        );
        handle.forget(&dir, "pop").await;
        assert!(
            Session::open(handle.clone(), &account, "pw", false)
                .await
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn oversize_or_missing_message_keeps_connection_usable() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut b = [0u8; 64];
            let _ = server.read(&mut b).await.unwrap();
            server.write_all(b"-ERR no such message\r\n").await.unwrap();
            let _ = server.read(&mut b).await.unwrap();
            server.write_all(b"+OK\r\nok\r\n.\r\n").await.unwrap();
        });
        let mut p = Pop3 {
            stream: BufReader::new(Box::new(client)),
        };
        let e = p.retrieve(1).await.unwrap_err();
        assert!(e.is::<MessageError>());
        assert_eq!(p.retrieve(2).await.unwrap(), b"ok\r\n");
    }

    #[tokio::test]
    async fn parses_uidl_and_dot_stuffing() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            server.write_all(b"+OK ready\r\n").await.unwrap();
            let mut b = [0u8; 64];
            let _ = server.read(&mut b).await.unwrap();
            server
                .write_all(b"+OK\r\n1 uid-a\r\n2 uid-b\r\n.\r\n")
                .await
                .unwrap();
            let _ = server.read(&mut b).await.unwrap();
            server.write_all(b"+OK\r\n..dot\r\n.\r\n").await.unwrap();
        });
        let mut p = Pop3 {
            stream: BufReader::new(Box::new(client)),
        };
        assert_eq!(p.line().await.unwrap(), "+OK ready");
        assert_eq!(p.uidls().await.unwrap().get(&2).unwrap(), "uid-b");
        assert_eq!(p.multiline("RETR 1").await.unwrap(), b".dot\r\n");
    }

    #[tokio::test]
    async fn preserves_binary_message_lines() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            server.write_all(b"+OK\r\n").await.unwrap();
            let mut command = [0u8; 32];
            let _ = server.read(&mut command).await.unwrap();
            server.write_all(b"+OK\r\n\xff\x00\r\n.\r\n").await.unwrap();
        });
        let mut p = Pop3 {
            stream: BufReader::new(Box::new(client)),
        };
        assert_eq!(p.line().await.unwrap(), "+OK");
        assert_eq!(p.multiline("RETR 1").await.unwrap(), b"\xff\x00\r\n");
    }
}
