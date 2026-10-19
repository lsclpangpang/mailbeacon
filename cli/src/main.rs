//! MailBeacon：收信速度原型。
//! 每个账号的每个文件夹保持一条常驻 IMAP 连接，优先用 IDLE 让服务器主动推送，
//! 新邮件一到就打印出来，并显示「服务器收到 → 本地显示」的延迟。

use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result, anyhow, bail};
use async_imap::{Client, Session};
use chrono::{DateTime, FixedOffset, Local};
use futures::TryStreamExt;
use serde::Deserialize;
use tokio::{net::TcpStream, sync::broadcast, time::timeout};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls};

type Imap = Session<TlsStream<TcpStream>>;

/// 单个网络操作的超时：超过这个时间没响应就认为连接已失效，直接重连。
const OP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Deserialize)]
struct Config {
    #[serde(default = "default_idle_refresh")]
    idle_refresh_secs: u64,
    #[serde(default = "default_poll")]
    poll_secs: u64,
    #[serde(rename = "account")]
    accounts: Vec<Account>,
}

fn default_idle_refresh() -> u64 {
    30
}
fn default_poll() -> u64 {
    15
}
fn default_kind() -> String {
    "custom".into()
}
fn default_folders() -> Vec<String> {
    vec!["INBOX".into()]
}
fn default_mode() -> String {
    "auto".into()
}

#[derive(Deserialize)]
struct Account {
    name: String,
    #[serde(default = "default_kind")]
    kind: String,
    user: String,
    password: Option<String>,
    password_cmd: Option<String>,
    host: Option<String>,
    port: Option<u16>,
    #[serde(default = "default_folders")]
    folders: Vec<String>,
    /// auto：支持 IDLE 就用 IDLE；poll：强制轮询（用于对比测试）
    #[serde(default = "default_mode")]
    mode: String,
}

impl Account {
    fn server(&self) -> Result<(String, u16)> {
        let preset = match self.kind.as_str() {
            "exmail" => Some("imap.exmail.qq.com"),
            "qq" => Some("imap.qq.com"),
            "163" => Some("imap.163.com"),
            "gmail" => Some("imap.gmail.com"),
            "custom" => None,
            k => bail!("[{}] 未知的 kind：{k}", self.name),
        };
        let host = self
            .host
            .clone()
            .or(preset.map(String::from))
            .ok_or_else(|| anyhow!("[{}] kind = \"custom\" 时必须填写 host", self.name))?;
        Ok((host, self.port.unwrap_or(993)))
    }

    fn secret(&self) -> Result<String> {
        if let Some(p) = &self.password {
            return Ok(p.clone());
        }
        let cmd = self
            .password_cmd
            .as_ref()
            .ok_or_else(|| anyhow!("[{}] 需要填写 password 或 password_cmd", self.name))?;
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .output()?;
        if !out.status.success() {
            bail!("[{}] password_cmd 执行失败", self.name);
        }
        Ok(String::from_utf8(out.stdout)?.trim().to_string())
    }
}

/// 一条监听任务：一个账号的一个文件夹。
struct Watch {
    tag: String,
    host: String,
    port: u16,
    user: String,
    pass: String,
    folder: String,
    force_poll: bool,
    idle_refresh: Duration,
    poll: Duration,
}

/// 登录被拒绝：重试只会触发风控，这个账号直接停止。
#[derive(Debug)]
struct AuthFailed(String);

impl std::fmt::Display for AuthFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "登录失败：{}（请确认填的是授权码/应用专用密码，并已开启 IMAP）",
            self.0
        )
    }
}

impl std::error::Error for AuthFailed {}

/// 跨重连保留的状态，断线期间到达的邮件在重连后补上。
#[derive(Default)]
struct Cursor {
    uid_validity: u32,
    last_uid: Option<u32>,
}

fn log(tag: &str, msg: impl AsRef<str>) {
    println!(
        "{}  [{}]  {}",
        Local::now().format("%H:%M:%S"),
        tag,
        msg.as_ref()
    );
}

async fn connect(
    host: &str,
    port: u16,
    tls: &TlsConnector,
) -> Result<Client<TlsStream<TcpStream>>> {
    let tcp = timeout(OP_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .context("TCP 连接超时")??;
    tcp.set_nodelay(true)?;
    // TCP 保活：网络静默断开（切 Wi-Fi、NAT 超时）时，内核约 1 分钟内就能发现并让读操作报错。
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10))
        .with_retries(3);
    socket2::SockRef::from(&tcp).set_tcp_keepalive(&keepalive)?;

    let name = rustls_pki_types::ServerName::try_from(host.to_string())?;
    let stream = timeout(OP_TIMEOUT, tls.connect(name, tcp))
        .await
        .context("TLS 握手超时")??;
    let mut client = Client::new(stream);
    timeout(OP_TIMEOUT, client.read_response())
        .await
        .context("等待服务器问候超时")??
        .ok_or_else(|| anyhow!("服务器没有返回问候语"))?;
    Ok(client)
}

/// 持续监听，出错后指数退避重连；系统唤醒时立即重连。
async fn watch(w: Watch, tls: TlsConnector, wake: broadcast::Sender<()>) {
    let mut cursor = Cursor::default();
    let mut backoff = 1u64;
    loop {
        let mut wake_rx = wake.subscribe();
        let started = Instant::now();
        match run(&w, &tls, &mut cursor, &mut wake_rx).await {
            // Ok 表示需要立刻重连（例如刚从睡眠唤醒）
            Ok(()) => {
                backoff = 1;
                continue;
            }
            Err(e) if e.is::<AuthFailed>() => {
                log(&w.tag, format!("✗ {e}，已停止该账号"));
                return;
            }
            Err(e) => log(&w.tag, format!("⚠ 连接中断：{e:#}")),
        }
        // 用了一段时间才断开（多半是网络抖动）：立即重连一次；连续失败才逐步拉长间隔
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

async fn run(
    w: &Watch,
    tls: &TlsConnector,
    cursor: &mut Cursor,
    wake_rx: &mut broadcast::Receiver<()>,
) -> Result<()> {
    let t0 = Instant::now();
    let client = connect(&w.host, w.port, tls).await?;
    let mut s = timeout(OP_TIMEOUT, client.login(&w.user, &w.pass))
        .await
        .context("登录超时")?
        .map_err(|(e, _)| AuthFailed(e.to_string()))?;

    let caps = timeout(OP_TIMEOUT, s.capabilities()).await??;
    // 163/126 要求登录后先发 ID 表明客户端身份，否则 SELECT 会报 "Unsafe Login"。
    if caps.has_str("ID") {
        let ident = [
            ("name", Some("mailpulse")),
            ("version", Some(env!("CARGO_PKG_VERSION"))),
            ("vendor", Some("mailpulse")),
        ];
        let _ = timeout(OP_TIMEOUT, s.id(ident)).await;
    }

    let mb = timeout(OP_TIMEOUT, s.select(&w.folder))
        .await?
        .with_context(|| format!("打开文件夹 {} 失败", w.folder))?;
    let validity = mb.uid_validity.unwrap_or(0);
    if cursor.last_uid.is_none() || cursor.uid_validity != validity {
        let last = match mb.uid_next {
            Some(n) => n.saturating_sub(1),
            None => timeout(OP_TIMEOUT, s.uid_search("ALL"))
                .await??
                .into_iter()
                .max()
                .unwrap_or(0),
        };
        *cursor = Cursor {
            uid_validity: validity,
            last_uid: Some(last),
        };
    }

    let use_idle = !w.force_poll && caps.has_str("IDLE");
    log(
        &w.tag,
        format!(
            "✓ 已连接（{} ms，{}封邮件），模式：{}",
            t0.elapsed().as_millis(),
            mb.exists,
            if use_idle {
                format!("IDLE 推送（每 {}s 兜底检查）", w.idle_refresh.as_secs())
            } else {
                format!("轮询（每 {}s）", w.poll.as_secs())
            }
        ),
    );
    // 补上断线期间到达的邮件
    fetch_new(&mut s, w, cursor).await?;

    if use_idle {
        loop {
            let mut idle = s.idle();
            timeout(OP_TIMEOUT, idle.init())
                .await
                .context("进入 IDLE 超时")??;
            let (wait, _stop) = idle.wait();
            let woke = tokio::select! {
                // 硬超时不受服务器 `* OK` 保活影响，保证兜底检查按时触发。
                r = timeout(w.idle_refresh, wait) => {
                    if let Ok(got) = r { got?; }
                    false
                }
                _ = wake_rx.recv() => true,
            };
            if woke {
                log(&w.tag, "↻ 检测到系统唤醒，立即重连");
                return Ok(());
            }
            s = timeout(OP_TIMEOUT, idle.done())
                .await
                .context("退出 IDLE 超时，连接可能已失效")??;
            fetch_new(&mut s, w, cursor).await?;
        }
    } else {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(w.poll) => {}
                _ = wake_rx.recv() => {
                    log(&w.tag, "↻ 检测到系统唤醒，立即重连");
                    return Ok(());
                }
            }
            timeout(OP_TIMEOUT, s.noop()).await.context("NOOP 超时")??;
            fetch_new(&mut s, w, cursor).await?;
        }
    }
}

async fn fetch_new(s: &mut Imap, w: &Watch, cursor: &mut Cursor) -> Result<()> {
    // 服务器的主动通知（EXISTS/EXPUNGE 等）这里用不到，清空避免通道堆满。
    while s.unsolicited_responses.try_recv().is_ok() {}

    let last = cursor.last_uid.unwrap_or(0);
    let query = format!("{}:*", last + 1);
    let mut msgs = timeout(OP_TIMEOUT, async {
        s.uid_fetch(&query, "(UID INTERNALDATE BODY.PEEK[HEADER])")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("拉取新邮件超时")??;
    let now = Local::now();

    // "N:*" 在没有新邮件时会返回最后一封，所以要按 UID 过滤。
    msgs.retain(|m| m.uid.is_some_and(|u| u > last));
    msgs.sort_by_key(|m| m.uid);
    for m in &msgs {
        let parsed = m
            .header()
            .and_then(|h| mail_parser::MessageParser::default().parse_headers(h));
        let subject = parsed
            .as_ref()
            .and_then(|p| p.subject())
            .unwrap_or("(无主题)");
        let from = parsed
            .as_ref()
            .and_then(|p| p.from())
            .and_then(|a| a.first())
            .map(|a| match (a.name(), a.address()) {
                (Some(n), Some(addr)) => format!("{n} <{addr}>"),
                (_, Some(addr)) => addr.to_string(),
                (Some(n), None) => n.to_string(),
                _ => "?".into(),
            })
            .unwrap_or_else(|| "?".into());
        log(
            &w.tag,
            format!(
                "✉  {from} — {subject}   ·  延迟 {}",
                latency(m.internal_date(), now)
            ),
        );
    }
    if let Some(max) = msgs.iter().filter_map(|m| m.uid).max() {
        cursor.last_uid = Some(max);
    }
    Ok(())
}

/// 服务器收件时间（INTERNALDATE，精确到秒）到本地显示的间隔。
fn latency(received: Option<DateTime<FixedOffset>>, now: DateTime<Local>) -> String {
    let Some(r) = received else {
        return "未知".into();
    };
    let ms = now.signed_duration_since(r).num_milliseconds();
    if ms < 1000 {
        "<1s".into()
    } else if ms < 120_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{} 分钟（可能是断线期间补收的）", ms / 60_000)
    }
}

/// 墙上时钟突然跳了很多，说明电脑刚从睡眠中醒来，旧连接大概率已失效。
fn spawn_wake_detector(tx: broadcast::Sender<()>) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut prev = SystemTime::now();
        loop {
            tick.tick().await;
            let now = SystemTime::now();
            let jumped = now
                .duration_since(prev)
                .map_or(true, |d| d > Duration::from_secs(20));
            if jumped {
                let _ = tx.send(());
            }
            prev = now;
        }
    });
}

#[tokio::main]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "accounts.toml".into());
    let text = std::fs::read_to_string(&path).with_context(|| {
        format!("读取 {path} 失败。先执行：cp accounts.example.toml accounts.toml 并填写账号")
    })?;
    let cfg: Config = toml::from_str(&text).with_context(|| format!("{path} 格式错误"))?;

    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // 额外信任的 CA（自建邮件服务器、本地测试用）
    if let Ok(ca) = std::env::var("MAILPULSE_EXTRA_CA") {
        use rustls_pki_types::{CertificateDer, pem::PemObject};
        for cert in CertificateDer::pem_file_iter(&ca).with_context(|| format!("读取 {ca} 失败"))?
        {
            roots.add(cert?)?;
        }
    }
    let tls = TlsConnector::from(Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ));

    let (wake_tx, _) = broadcast::channel(4);
    spawn_wake_detector(wake_tx.clone());

    for acct in &cfg.accounts {
        let (host, port) = acct.server()?;
        let pass = acct.secret()?;
        for folder in &acct.folders {
            let tag = if acct.folders.len() > 1 {
                format!("{}/{}", acct.name, folder)
            } else {
                acct.name.clone()
            };
            let w = Watch {
                tag,
                host: host.clone(),
                port,
                user: acct.user.clone(),
                pass: pass.clone(),
                folder: folder.clone(),
                force_poll: acct.mode == "poll",
                idle_refresh: Duration::from_secs(cfg.idle_refresh_secs),
                poll: Duration::from_secs(cfg.poll_secs),
            };
            tokio::spawn(watch(w, tls.clone(), wake_tx.clone()));
        }
    }

    println!(
        "MailBeacon 正在监听 {} 个账号，按 Ctrl+C 退出\n",
        cfg.accounts.len()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
