//! IMAP 底层操作：连接登录、拉取邮件列表、文件夹、解析正文。

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use async_imap::{
    Client, Session,
    imap_proto::{BodyStructure, NameAttribute},
    types::Flag,
};
use futures::TryStreamExt;
use mailbeacon_parser::{Address, MessageParser, MimeHeaders};
use serde::Serialize;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    net::TcpStream,
    time::timeout,
};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls};

use crate::{
    config::Account,
    download_protocol::{ArchiveData, checked_command, quoted_mailbox},
};

/// 数着收到字节数的 TLS 连接（读信时报下载进度）
pub type ImapStream = crate::lanes::Counting<TlsStream<TcpStream>>;
pub type Imap = Session<ImapStream>;

/// 单个网络操作的超时：超过这个时间没响应就认为连接已失效。
pub const OP_TIMEOUT: Duration = Duration::from_secs(20);
/// 首屏和「加载更多」每次拉多少封
pub const PAGE: usize = 100;
/// 本地搜索索引里每封邮件最多保留的正文开头字符数
const SEARCH_TEXT_CHARS: usize = 2000;

/// 登录被拒绝：重试只会触发风控，这个账号直接停止。
#[derive(Debug)]
pub struct AuthFailed(pub String);

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

fn tls() -> TlsConnector {
    static TLS: OnceLock<TlsConnector> = OnceLock::new();
    TLS.get_or_init(|| {
        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        // 额外信任的 CA（自建邮件服务器、本地测试用）
        if let Some(pem) = extra_ca_pem() {
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
    })
    .clone()
}

/// 环境变量 MAILPULSE_EXTRA_CA 指向的 PEM 证书
pub fn extra_ca_pem() -> Option<Vec<u8>> {
    std::fs::read(std::env::var_os("MAILPULSE_EXTRA_CA")?).ok()
}

pub async fn login(acct: &Account, pass: &str) -> Result<Imap> {
    let host = acct.imap_host.as_str();
    let tcp = timeout(OP_TIMEOUT, TcpStream::connect((host, acct.imap_port)))
        .await
        .context("连接服务器超时")?
        .with_context(|| format!("无法连接 {host}"))?;
    tcp.set_nodelay(true)?;
    // TCP 保活：网络静默断开（切 Wi-Fi、NAT 超时）时，约 1 分钟内就能发现。
    let keepalive = socket2::TcpKeepalive::new()
        .with_time(Duration::from_secs(30))
        .with_interval(Duration::from_secs(10))
        .with_retries(3);
    socket2::SockRef::from(&tcp).set_tcp_keepalive(&keepalive)?;

    let name = rustls_pki_types::ServerName::try_from(host.to_string())?;
    let stream = timeout(OP_TIMEOUT, tls().connect(name, tcp))
        .await
        .context("TLS 握手超时")??;
    let mut client = Client::new(crate::lanes::Counting::new(stream));
    timeout(OP_TIMEOUT, client.read_response())
        .await
        .context("等待服务器问候超时")??
        .ok_or_else(|| anyhow!("服务器没有返回问候语"))?;

    let mut s = timeout(OP_TIMEOUT, client.login(&acct.email, pass))
        .await
        .context("登录超时")?
        .map_err(|(e, _)| AuthFailed(e.to_string()))?;

    // 163/126 要求登录后先发 ID 表明客户端身份，否则 SELECT 会报 "Unsafe Login"。
    let caps = timeout(OP_TIMEOUT, s.capabilities()).await??;
    if caps.has_str("ID") {
        let ident = [
            ("name", Some("MailBeacon")),
            ("version", Some(env!("CARGO_PKG_VERSION"))),
            ("vendor", Some("MailBeacon")),
        ];
        let _ = timeout(OP_TIMEOUT, s.id(ident)).await;
    }
    Ok(s)
}

pub async fn has_capability(s: &mut Imap, cap: &str) -> Result<bool> {
    Ok(timeout(OP_TIMEOUT, s.capabilities()).await??.has_str(cap))
}

#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub account_id: String,
    #[serde(default = "crate::mail_ref::initial_epoch")]
    pub receive_epoch: u64,
    /// Zero means an old summary has no proven mailbox generation; it is not a writable reference.
    #[serde(default)]
    pub uid_validity: u32,
    pub folder: String,
    pub uid: u32,
    pub from_name: String,
    pub from_addr: String,
    pub to: String,
    pub subject: String,
    /// 服务器收件时间，毫秒时间戳
    pub date: i64,
    pub seen: bool,
    pub flagged: bool,
    pub attachment: bool,
    /// 收件人和抄送的邮箱地址（小写），用于「@我」筛选
    pub recipients: Vec<String>,
    /// 会话用：Message-ID、In-Reply-To 和 References（都不带尖括号）
    pub message_id: String,
    pub in_reply_to: String,
    pub references: Vec<String>,
}

impl Default for Summary {
    fn default() -> Self {
        Self {
            account_id: String::new(),
            receive_epoch: 1,
            uid_validity: 0,
            folder: String::new(),
            uid: 0,
            from_name: String::new(),
            from_addr: String::new(),
            to: String::new(),
            subject: String::new(),
            date: 0,
            seen: false,
            flagged: false,
            attachment: false,
            recipients: Vec::new(),
            message_id: String::new(),
            in_reply_to: String::new(),
            references: Vec::new(),
        }
    }
}

impl Summary {
    /// Return a validated reference; legacy unverified summaries remain read-only.
    pub fn message_ref(&self) -> anyhow::Result<crate::mail_ref::MessageRef> {
        let source = crate::mail_ref::MessageRef {
            account_id: self.account_id.clone(),
            receive_epoch: self.receive_epoch,
            folder: self.folder.clone(),
            uid_validity: self.uid_validity,
            uid: self.uid,
        };
        source.validate()?;
        Ok(source)
    }
}

/// 同一封邮件在不同文件夹、不同 UID 下的身份：有 Message-ID 就用它，
/// 没有时退回到服务器收件时间、发件人和主题。
pub fn mail_identity(m: &Summary) -> String {
    if m.message_id.is_empty() {
        format!("{}|{}|{}", m.date, m.from_addr.to_lowercase(), m.subject)
    } else {
        format!("<{}>", m.message_id)
    }
}

/// 邮件头里的会话信息：(Message-ID, In-Reply-To, References)。
pub fn thread_headers(p: Option<&mailbeacon_parser::Message<'_>>) -> (String, String, Vec<String>) {
    let Some(p) = p else {
        return Default::default();
    };
    let text =
        |v: &mailbeacon_parser::HeaderValue<'_>| v.as_text().unwrap_or("").trim().to_string();
    let references = p
        .references()
        .as_text_list()
        .map(|list| {
            list.iter()
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect()
        })
        .unwrap_or_default();
    (
        p.message_id().unwrap_or("").trim().to_string(),
        text(p.in_reply_to()),
        references,
    )
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlagState {
    pub uid: u32,
    pub seen: bool,
    pub flagged: bool,
}

const SUMMARY_ITEMS: &str = "(UID FLAGS INTERNALDATE BODY.PEEK[HEADER])";

fn first_addr(a: Option<&Address>) -> (String, String) {
    a.and_then(|a| a.first())
        .map(|a| {
            (
                a.name().unwrap_or("").to_string(),
                a.address().unwrap_or("").to_string(),
            )
        })
        .unwrap_or_default()
}

fn addr_names(a: Option<&Address>) -> String {
    a.map(|a| {
        a.iter()
            .map(|x| x.name().or(x.address()).unwrap_or("").to_string())
            .collect::<Vec<_>>()
            .join("、")
    })
    .unwrap_or_default()
}

/// 按 UID 拉取邮件摘要，`set` 是 IMAP UID 集合，如 "120:*" 或 "3,5,9"。
pub async fn fetch_summaries<T>(
    s: &mut Session<T>,
    set: &str,
    acct_id: &str,
    folder: &str,
) -> Result<Vec<Summary>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let fetches = timeout(OP_TIMEOUT * 3, async {
        s.uid_fetch(set, SUMMARY_ITEMS)
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("拉取邮件列表超时")??;

    let parser = MessageParser::default();
    let mut out = Vec::with_capacity(fetches.len());
    for m in &fetches {
        let Some(uid) = m.uid else { continue };
        // 老邮件可能把 GBK 字节直接塞进邮件头（不是 RFC 2047 编码），先修再解析，
        // 否则发件人/主题会变成一串「�」
        let header_fix = m.header().map(crate::mime::fix_headers);
        let parsed = header_fix.as_ref().and_then(|h| parser.parse_headers(h));
        let p = parsed.as_ref();
        let mut seen = false;
        let mut flagged = false;
        let mut deleted = false;
        for f in m.flags() {
            match f {
                Flag::Seen => seen = true,
                Flag::Flagged => flagged = true,
                Flag::Deleted => deleted = true,
                _ => {}
            }
        }
        // 已标记删除、等待清除的邮件（如草稿的旧版本）其他客户端也不显示
        if deleted {
            continue;
        }
        let date = m
            .internal_date()
            .map(|d| d.timestamp_millis())
            .or_else(|| p.and_then(|p| p.date()).map(|d| d.to_timestamp() * 1000))
            .unwrap_or(0);
        out.push(build_summary(p, uid, seen, flagged, date, acct_id, folder));
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.uid));
    Ok(out)
}

/// 把解析好的邮件头（和已读/星标状态）变成列表/搜索用的 Summary。
fn build_summary(
    p: Option<&mailbeacon_parser::Message<'_>>,
    uid: u32,
    seen: bool,
    flagged: bool,
    date: i64,
    acct_id: &str,
    folder: &str,
) -> Summary {
    let (from_name, from_addr) = first_addr(p.and_then(|p| p.from()));
    let attachment = p.and_then(|p| p.content_type()).is_some_and(|ct| {
        ct.ctype().eq_ignore_ascii_case("multipart")
            && ct
                .subtype()
                .is_some_and(|s| s.eq_ignore_ascii_case("mixed"))
    });
    let (message_id, in_reply_to, references) = thread_headers(p);
    Summary {
        account_id: acct_id.to_string(),
        receive_epoch: 1,
        uid_validity: 0,
        folder: folder.to_string(),
        uid,
        from_name,
        from_addr,
        to: addr_names(p.and_then(|p| p.to())),
        subject: p.and_then(|p| p.subject()).unwrap_or("").to_string(),
        date,
        seen,
        flagged,
        attachment,
        recipients: recipient_addresses(p.and_then(|p| p.to()), p.and_then(|p| p.cc())),
        message_id,
        in_reply_to,
        references,
    }
}

/// 收件人和抄送里的邮箱地址，小写、去重。
pub fn recipient_addresses(to: Option<&Address<'_>>, cc: Option<&Address<'_>>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for list in [to, cc].into_iter().flatten() {
        for addr in list.iter().filter_map(|x| x.address()) {
            let addr = addr.trim().to_lowercase();
            if !addr.is_empty() && !out.contains(&addr) {
                out.push(addr);
            }
        }
    }
    out
}

pub async fn fetch_flags<T>(s: &mut Session<T>, set: &str) -> Result<Vec<FlagState>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let fetches = timeout(OP_TIMEOUT, async {
        s.uid_fetch(set, "(UID FLAGS)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("同步邮件状态超时")??;
    Ok(fetches
        .iter()
        .filter_map(|m| {
            let uid = m.uid?;
            let flags: Vec<_> = m.flags().collect();
            Some(FlagState {
                uid,
                seen: flags.iter().any(|f| matches!(f, Flag::Seen)),
                flagged: flags.iter().any(|f| matches!(f, Flag::Flagged)),
            })
        })
        .collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub uid: u32,
    pub text: String,
}

/// 列表里显示的正文摘要：只取每封邮件开头的一小段来解析，不下载整封。
pub async fn fetch_previews(s: &mut Imap, set: &str) -> Result<Vec<Preview>> {
    let fetches = timeout(OP_TIMEOUT * 3, async {
        s.uid_fetch(set, "(UID BODY.PEEK[]<0.16384>)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("拉取摘要超时")??;
    let parser = MessageParser::default();
    Ok(fetches
        .iter()
        .filter_map(|m| {
            let uid = m.uid?;
            let msg = parser.parse(m.body()?)?;
            let raw = msg.body_preview(400)?;
            let text: String = raw
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(140)
                .collect();
            Some(Preview { uid, text })
        })
        .collect())
}

/// 选中文件夹里所有未删除邮件的 UID（升序）。本地搜索索引靠它做增量枚举。
pub async fn all_uids<T>(s: &mut Session<T>) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    // 几万封的文件夹，有的服务器列出全部 UID 也要好一阵
    let responses = timeout(
        OP_TIMEOUT * 6,
        checked_command(s, "UID SEARCH ALL UNDELETED"),
    )
    .await
    .context("搜索邮件超时")??;
    let mut uids: Vec<u32> = responses
        .into_iter()
        .flat_map(|r| match r {
            ArchiveData::Uids(uids) => uids,
            _ => Vec::new(),
        })
        .collect();
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

/// 搜索索引正文阶段用：只取每封开头 16KB，解码成可搜索的正文文本。
///
/// 不设置 \\Seen。部分服务器对 `BODY[]<0.n>` 支持不好时解析不出正文，
/// 调用方会跳过这封（下次索引再补）。
pub async fn fetch_body_texts<T>(s: &mut Session<T>, set: &str) -> Result<Vec<Preview>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let fetches = timeout(OP_TIMEOUT * 3, async {
        s.uid_fetch(set, "(UID BODY.PEEK[]<0.16384>)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("拉取正文超时")??;
    let parser = MessageParser::default();
    Ok(fetches
        .iter()
        .filter_map(|m| {
            let uid = m.uid?;
            let msg = parser.parse(m.body()?)?;
            let raw = msg.body_preview(SEARCH_TEXT_CHARS * 2)?;
            let text: String = raw.split_whitespace().collect::<Vec<_>>().join(" ");
            Some(Preview {
                uid,
                text: text.chars().take(SEARCH_TEXT_CHARS).collect(),
            })
        })
        .collect())
}

pub async fn unseen_count(s: &mut Imap) -> Result<usize> {
    Ok(timeout(OP_TIMEOUT, s.uid_search("UNSEEN")).await??.len())
}

/// 已选中文件夹里 UID 小于 `before` 的最新一页（`before` 为空表示最新一页）。
pub async fn page_uids<T>(s: &mut Session<T>, before: Option<u32>) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    page_uids_matching(s, "", before).await
}

/// 同 `page_uids`，但只要满足 `criteria`（如 "UNSEEN"）的邮件；已标记删除的始终排除。
pub async fn page_uids_matching<T>(
    s: &mut Session<T>,
    criteria: &str,
    before: Option<u32>,
) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    Ok(page_uids_matching_limited(s, criteria, before, PAGE)
        .await?
        .0)
}

/// 同 `page_uids_matching`，但最多取 `limit` 封，并返回是否还有更早的邮件。
/// 没建过索引的文件夹第一次打开时用较小的 limit，界面先显示一部分，不用等一整页。
pub async fn page_uids_matching_limited<T>(
    s: &mut Session<T>,
    criteria: &str,
    before: Option<u32>,
    limit: usize,
) -> Result<(Vec<u32>, bool)>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let query = format!("UID SEARCH {criteria} UNDELETED");
    // checked_command 会检查服务器的最终答复：async-imap 的 uid_search 遇到 NO/BAD
    // 会当成空结果，不支持某个条件的服务器就会让文件夹看起来是空的。
    let responses = timeout(
        OP_TIMEOUT * 2,
        checked_command(s, &query.replace("  ", " ")),
    )
    .await
    .context("搜索邮件超时")??;
    let mut uids: Vec<u32> = responses
        .into_iter()
        .flat_map(|r| match r {
            ArchiveData::Uids(uids) => uids,
            _ => Vec::new(),
        })
        .filter(|u| before.is_none_or(|b| *u < b))
        .collect();
    uids.sort_unstable();
    let more = uids.len() > limit;
    let start = uids.len().saturating_sub(limit);
    Ok((uids.split_off(start), more))
}

#[derive(Debug, PartialEq, Eq)]
enum MoveStrategy {
    Move,
    CopyAndExpunge,
    Expunge,
}

fn move_strategy(has_move: bool, has_uidplus: bool, has_target: bool) -> Result<MoveStrategy> {
    match (has_move, has_uidplus, has_target) {
        (true, _, true) => Ok(MoveStrategy::Move),
        (_, true, true) => Ok(MoveStrategy::CopyAndExpunge),
        (_, true, false) => Ok(MoveStrategy::Expunge),
        _ => bail!(
            "服务器不支持安全地移动或彻底删除指定邮件（需要 MOVE 或 UIDPLUS），请使用邮箱网页版操作"
        ),
    }
}

/// 把已选中文件夹里的邮件移到 `target`；`target` 为空表示彻底删除。
/// 仅使用 MOVE 或 UID EXPUNGE，避免清除其他客户端标记删除的无关邮件。
pub async fn move_uids<T>(s: &mut Session<T>, set: &str, target: Option<&str>) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let caps = timeout(OP_TIMEOUT, s.capabilities()).await??;
    let strategy = move_strategy(
        caps.has_str("MOVE"),
        caps.has_str("UIDPLUS"),
        target.is_some(),
    )?;
    if let Some(t) = target {
        if strategy == MoveStrategy::Move {
            timeout(OP_TIMEOUT, s.uid_mv(set, t))
                .await
                .context("移动邮件超时，结果尚未确认，请刷新源文件夹和目标文件夹后检查")??;
            return Ok(());
        }
        timeout(OP_TIMEOUT, s.uid_copy(set, t))
            .await
            .context("复制邮件超时，结果尚未确认，请刷新源文件夹和目标文件夹后检查")??;
    }
    let remove_context = if target.is_some() {
        "邮件已复制到目标文件夹，但移除原邮件未完成，请刷新两边后确认，避免重复移动"
    } else {
        "彻底删除尚未完成，请刷新后确认"
    };
    // 流式 uid_store/uid_expunge 会丢弃 tagged NO/BAD；必须检查最终确认，
    // STORE 未成功时也不能继续 EXPUNGE 或向界面报告移动完成。
    timeout(
        OP_TIMEOUT,
        checked_command(s, &format!("UID STORE {set} +FLAGS.SILENT (\\Deleted)")),
    )
    .await
    .context(remove_context)?
    .context(remove_context)?;
    timeout(
        OP_TIMEOUT,
        checked_command(s, &format!("UID EXPUNGE {set}")),
    )
    .await
    .context(remove_context)?
    .context(remove_context)?;
    Ok(())
}

/// 已选中的 `folder` 里没有这个 Message-ID 的邮件，就补存一份（标为已读）。返回是否补存了。
pub async fn ensure_saved<T>(
    s: &mut Session<T>,
    folder: &str,
    message_id: &str,
    raw: &[u8],
) -> Result<bool>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(
            s,
            &format!(
                "UID SEARCH HEADER Message-ID {}",
                quoted_mailbox(message_id)?
            ),
        ),
    )
    .await??;
    let mut reported = false;
    let mut found = false;
    for response in responses {
        if let ArchiveData::Uids(uids) = response {
            reported = true;
            found |= !uids.is_empty();
        }
    }
    if !reported {
        bail!("服务器未返回已发送邮件查询结果，未补存以避免重复");
    }
    if found {
        return Ok(false);
    }
    timeout(OP_TIMEOUT, s.append(folder, Some("(\\Seen)"), None, raw))
        .await
        .context("保存到已发送超时")??;
    Ok(true)
}

pub fn uid_set(uids: &[u32]) -> String {
    uids.iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",")
}

// ---------------------------------------------------------------------------
// 文件夹
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    /// 服务器上的原始名字，作为 ID 使用
    pub path: String,
    /// 解码后的显示名
    pub name: String,
    /// inbox | sent | drafts | trash | junk | archive | all | other
    pub role: String,
}

fn validate_folder_name(name: &str, delimiter: Option<&str>) -> Result<String> {
    if name.chars().any(char::is_control) {
        bail!("文件夹名称不能包含换行、制表符等控制字符");
    }
    let name = name.trim();
    if name.is_empty() {
        bail!("请输入文件夹名称");
    }
    if let Some(delimiter) = delimiter.filter(|d| !d.is_empty()) {
        if name.contains(delimiter) {
            bail!("文件夹名称不能包含服务器的层级分隔符“{delimiter}”");
        }
    }
    Ok(name.to_string())
}

/// 创建服务器上的顶层文件夹，并用 LIST 回读确认其真实、可选择的路径。
pub async fn create_folder(s: &mut Imap, name: &str) -> Result<(Folder, Vec<Folder>)> {
    let mut roots = checked_names(s, "")
        .await
        .context("读取服务器文件夹分隔符失败")?;
    // 少数服务器对空 LIST 模式返回空列表，改从其实际顶层目录读取分隔符。
    if roots.is_empty() {
        roots = checked_names(s, "%").await.context("读取顶层文件夹失败")?;
    }
    let root = roots
        .iter()
        .find(|n| n.name.eq_ignore_ascii_case("INBOX"))
        .or_else(|| roots.first())
        .ok_or_else(|| anyhow!("服务器未返回文件夹层级信息，无法确认顶层文件夹名称"))?;
    let name = validate_folder_name(name, root.delimiter.as_deref())?;
    let path = encode_mutf7(&name);
    let existing = list_folders(s).await?;
    if existing.iter().any(|f| mailbox_eq(&f.path, &path)) {
        bail!("已经存在同名文件夹");
    }
    timeout(OP_TIMEOUT, s.create(&path))
        .await
        .context("创建文件夹超时，请刷新后确认是否已创建")?
        .context("创建文件夹失败")?;
    let folders = list_folders(s)
        .await
        .context("文件夹已创建，但读取服务器列表失败，请刷新后确认")?;
    let folder = folders
        .iter()
        .find(|f| mailbox_eq(&f.path, &path) || decode_mutf7(&f.path) == name)
        .cloned()
        .ok_or_else(|| anyhow!("服务器接受了创建请求，但未返回可选择的文件夹，请刷新后确认"))?;
    Ok((folder, folders))
}

/// INBOX 在 IMAP 中不区分大小写，其他文件夹按服务器返回的路径精确比较。
pub fn mailbox_eq(a: &str, b: &str) -> bool {
    a == b || (a.eq_ignore_ascii_case("INBOX") && b.eq_ignore_ascii_case("INBOX"))
}

/// 保留 LIST 的属性和分隔符，并拒绝失败命令携带的部分列表。
async fn checked_names<T>(
    session: &mut Session<T>,
    pattern: &str,
) -> Result<Vec<async_imap::imap_proto::MailboxListData<'static>>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let command = format!("LIST \"\" {}", quoted_mailbox(pattern)?);
    let responses = timeout(OP_TIMEOUT, checked_command(session, &command))
        .await
        .context("读取文件夹超时")??;
    Ok(responses
        .into_iter()
        .filter_map(|response| {
            if let ArchiveData::ListedMailbox(data) = response {
                Some(data)
            } else {
                None
            }
        })
        .collect())
}

pub async fn list_folders<T>(s: &mut Session<T>) -> Result<Vec<Folder>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let names = checked_names(s, "*").await?;
    let mut out = Vec::new();
    for n in &names {
        let attrs = &n.name_attributes;
        if attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect)) {
            continue;
        }
        let path = n.name.to_string();
        let decoded = decode_mutf7(&path);
        let leaf = match n.delimiter.as_deref() {
            Some(d) if !d.is_empty() => decoded.rsplit(d).next().unwrap_or(&decoded).to_string(),
            _ => decoded.clone(),
        };
        let role = folder_role(&path, &leaf, attrs);
        let name = match role {
            "inbox" => "收件箱",
            "sent" => "已发送",
            "drafts" => "草稿",
            "trash" => "已删除",
            "junk" => "垃圾邮件",
            "archive" => "归档",
            "all" => "所有邮件",
            _ => leaf.as_str(),
        }
        .to_string();
        out.push(Folder {
            path,
            name,
            role: role.to_string(),
        });
    }
    let rank = |r: &str| {
        ["inbox", "drafts", "sent", "archive", "all", "junk", "trash"]
            .iter()
            .position(|x| *x == r)
            .unwrap_or(9)
    };
    out.sort_by(|a, b| {
        rank(&a.role)
            .cmp(&rank(&b.role))
            .then_with(|| a.name.cmp(&b.name))
    });
    // 同一角色只保留一个（有的服务器同时有 "Sent" 和 "Sent Messages"）
    let mut seen_roles = std::collections::HashSet::new();
    for f in out.iter_mut() {
        if f.role != "other" && !seen_roles.insert(f.role.clone()) {
            f.role = "other".into();
        }
    }
    Ok(out)
}

fn folder_role(path: &str, leaf: &str, attrs: &[NameAttribute]) -> &'static str {
    if path.eq_ignore_ascii_case("INBOX") {
        return "inbox";
    }
    for a in attrs {
        match a {
            NameAttribute::Sent => return "sent",
            NameAttribute::Drafts => return "drafts",
            NameAttribute::Trash => return "trash",
            NameAttribute::Junk => return "junk",
            NameAttribute::Archive => return "archive",
            NameAttribute::All => return "all",
            _ => {}
        }
    }
    let l = leaf.to_lowercase();
    match l.as_str() {
        "sent" | "sent messages" | "sent items" | "sent mail" | "已发送" | "已发送邮件" => {
            "sent"
        }
        "drafts" | "draft" | "草稿箱" | "草稿" => "drafts",
        "trash" | "deleted" | "deleted messages" | "deleted items" | "已删除" | "已删除邮件" => {
            "trash"
        }
        "junk" | "spam" | "junk e-mail" | "垃圾邮件" | "垃圾箱" => "junk",
        "archive" | "归档" => "archive",
        "all mail" | "所有邮件" => "all",
        _ => "other",
    }
}

/// 将显示名称编码为 IMAP 修改版 UTF-7；包括非 BMP 字符的 UTF-16 代理对。
pub fn encode_mutf7(name: &str) -> String {
    use base64::{Engine, engine::general_purpose::STANDARD_NO_PAD};
    let mut out = String::new();
    let mut bytes = Vec::new();
    let flush = |out: &mut String, bytes: &mut Vec<u8>| {
        if !bytes.is_empty() {
            out.push('&');
            out.push_str(&STANDARD_NO_PAD.encode(&*bytes).replace('/', ","));
            out.push('-');
            bytes.clear();
        }
    };
    for c in name.chars() {
        if (' '..='~').contains(&c) {
            flush(&mut out, &mut bytes);
            if c == '&' {
                out.push_str("&-");
            } else {
                out.push(c);
            }
        } else {
            for unit in c.encode_utf16(&mut [0; 2]) {
                bytes.extend_from_slice(&unit.to_be_bytes());
            }
        }
    }
    flush(&mut out, &mut bytes);
    out
}

/// IMAP 文件夹名用的「修改版 UTF-7」（RFC 3501 5.1.3），中文文件夹名靠它编码。
pub fn decode_mutf7(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        let Some(j) = rest.find('-') else {
            out.push('&');
            break;
        };
        let chunk = &rest[..j];
        rest = &rest[j + 1..];
        if chunk.is_empty() {
            out.push('&');
            continue;
        }
        let mut bits = 0u32;
        let mut nbits = 0;
        let mut units = Vec::new();
        for c in chunk.bytes() {
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b',' => 63,
                _ => continue,
            } as u32;
            bits = (bits << 6) | v;
            nbits += 6;
            if nbits >= 16 {
                nbits -= 16;
                units.push(((bits >> nbits) & 0xffff) as u16);
            }
        }
        out.push_str(&String::from_utf16_lossy(&units));
    }
    out.push_str(rest);
    out
}

// ---------------------------------------------------------------------------
// 正文
// ---------------------------------------------------------------------------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub name: String,
    pub addr: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub index: usize,
    pub name: String,
    pub size: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageView {
    /// 服务器文件夹代际；离线解析时未知，由读取命令补充。
    pub uid_validity: Option<u32>,
    pub subject: String,
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    /// 只有自己存的草稿和已发送里才会有密送
    pub bcc: Vec<Person>,
    pub date: i64,
    pub html: Option<String>,
    /// 用于回复的原始 HTML，保留 cid 引用；前端展示前必须按邮件规则清理。
    pub reply_html: Option<String>,
    pub text: String,
    pub has_remote: bool,
    pub attachments: Vec<Attachment>,
    pub message_id: Option<String>,
    /// 重新打开回复草稿时保持会话关系
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    /// 发件人请求已读回执时回执要发到的地址（Disposition-Notification-To）
    pub receipt_to: Option<String>,
    /// 已经发过或拒绝过回执（服务器上有 $MDNSent 标记，别的客户端也认）
    pub receipt_handled: bool,
    /// 大邮件先出的文字版：只有正文，图片和附件还在下载
    pub partial: bool,
    /// 整封邮件的大小（字节），先出文字时给进度条用
    pub size: Option<u32>,
}

/// 大邮件先出文字时要单独取的正文部分：在邮件结构里的位置（如 [1, 2]）和大小。
/// 只看多部分邮件；优先 HTML，其次纯文本，跳过附件和转发的整封邮件。
pub fn text_part(structure: &BodyStructure<'_>) -> Option<(Vec<u32>, u32)> {
    fn find(bs: &BodyStructure<'_>, path: &mut Vec<u32>, subtype: &str) -> Option<(Vec<u32>, u32)> {
        match bs {
            BodyStructure::Text { common, other, .. } => {
                // 标成附件的，或者带文件名的（有的邮件程序附件不写 Content-Disposition）
                let attachment = common
                    .disposition
                    .as_ref()
                    .is_some_and(|d| d.ty.eq_ignore_ascii_case("attachment"))
                    || common.ty.params.iter().flatten().any(|(k, _)| {
                        k.eq_ignore_ascii_case("name") || k.eq_ignore_ascii_case("filename")
                    });
                (common.ty.ty.eq_ignore_ascii_case("text")
                    && common.ty.subtype.eq_ignore_ascii_case(subtype)
                    && !attachment)
                    .then(|| (path.clone(), other.octets))
            }
            BodyStructure::Multipart { bodies, .. } => {
                bodies.iter().enumerate().find_map(|(i, b)| {
                    path.push(i as u32 + 1);
                    let found = find(b, path, subtype);
                    path.pop();
                    found
                })
            }
            BodyStructure::Basic { .. } | BodyStructure::Message { .. } => None,
        }
    }
    if !matches!(structure, BodyStructure::Multipart { .. }) {
        return None;
    }
    find(structure, &mut Vec::new(), "html").or_else(|| find(structure, &mut Vec::new(), "plain"))
}

/// 用邮件头和单独取来的正文部分拼成一封只有正文的邮件，交给 parse_message。
/// 原邮件头里的 Content-Type 等换成这一部分自己的 MIME 头。
pub fn assemble_text_message(header: &[u8], part_mime: &[u8], part_body: &[u8]) -> Vec<u8> {
    // 按字节处理：老邮件程序会直接用 GBK 写邮件头，转成字符串会弄坏
    let trim_cr = |line: &[u8]| line.strip_suffix(b"\r").unwrap_or(line).to_vec();
    let mut raw = Vec::new();
    let mut skipping = false;
    for line in header.split(|&b| b == b'\n').map(trim_cr) {
        if line.is_empty() {
            continue;
        }
        if !line.starts_with(b" ") && !line.starts_with(b"\t") {
            let name = line.split(|&b| b == b':').next().unwrap_or_default();
            let name = String::from_utf8_lossy(name).trim().to_ascii_lowercase();
            skipping = name.starts_with("content-") || name == "mime-version";
        }
        if !skipping {
            raw.extend_from_slice(&line);
            raw.extend_from_slice(b"\r\n");
        }
    }
    raw.extend_from_slice(b"MIME-Version: 1.0\r\n");
    let mime_end = part_mime
        .iter()
        .rposition(|&b| b != b'\r' && b != b'\n')
        .map_or(0, |i| i + 1);
    if mime_end > 0 {
        raw.extend_from_slice(&part_mime[..mime_end]);
        raw.extend_from_slice(b"\r\n");
    }
    raw.extend_from_slice(b"\r\n");
    raw.extend_from_slice(part_body);
    raw
}

/// RFC 3503：回执发过或用户拒绝后打上的标记，各客户端据此不再询问。
pub const MDN_SENT: &str = "$MDNSent";

/// 已选中文件夹里这封邮件有没有某个关键字标记。
pub async fn has_keyword<T>(s: &mut Session<T>, uid: u32, keyword: &str) -> Result<bool>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let fetches = timeout(OP_TIMEOUT, async {
        s.uid_fetch(uid.to_string(), "(FLAGS)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("读取邮件标记超时")??;
    Ok(fetches.iter().filter(|m| m.uid == Some(uid)).any(|m| {
        m.flags()
            .any(|f| matches!(f, Flag::Custom(k) if k.eq_ignore_ascii_case(keyword)))
    }))
}

/// 给已选中文件夹里的邮件加上关键字标记。
pub async fn add_keyword<T>(s: &mut Session<T>, uid: u32, keyword: &str) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    timeout(
        OP_TIMEOUT,
        checked_command(s, &format!("UID STORE {uid} +FLAGS.SILENT ({keyword})")),
    )
    .await
    .context("修改邮件标记超时")??;
    Ok(())
}

/// Disposition-Notification-To 里的邮箱地址。
fn receipt_address(msg: &mailbeacon_parser::Message<'_>) -> Option<String> {
    let raw = msg.header_raw("Disposition-Notification-To")?.trim();
    let addr = match (raw.rfind('<'), raw.rfind('>')) {
        (Some(a), Some(b)) if a < b => &raw[a + 1..b],
        _ => raw,
    }
    .trim();
    (addr.contains('@')
        && !addr
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || matches!(c, '<' | '>' | '"' | ',')))
    .then(|| addr.to_string())
}

fn people(a: Option<&Address>) -> Vec<Person> {
    a.map(|a| {
        a.iter()
            .map(|x| Person {
                name: x.name().unwrap_or("").to_string(),
                addr: x.address().unwrap_or("").to_string(),
            })
            .collect()
    })
    .unwrap_or_default()
}

pub fn parse_message(raw: &[u8], fallback_date: i64) -> Result<MessageView> {
    use base64::Engine;
    let raw = crate::mime::fix_headers(raw);
    let msg = crate::mime::parse(&raw)?;
    let real_html = msg.html_part(0).is_some_and(|p| p.is_text_html());
    let mut html = if real_html {
        msg.body_html(0).map(|h| h.into_owned())
    } else {
        None
    };
    let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();
    let reply_html = html.clone();

    let mut attachments = Vec::new();
    for (index, part) in msg.attachments().enumerate() {
        // 正文里引用的内嵌图片（cid:）直接换成 data URI，不作为附件列出
        if let (Some(cid), Some(h)) = (part.content_id(), html.as_mut()) {
            let cid = cid.trim_matches(|c| c == '<' || c == '>');
            let needle = format!("cid:{cid}");
            if h.contains(&needle) {
                let mime = part
                    .content_type()
                    .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream")))
                    .unwrap_or_else(|| "application/octet-stream".into());
                let data = base64::engine::general_purpose::STANDARD.encode(part.contents());
                *h = h.replace(&needle, &format!("data:{mime};base64,{data}"));
                continue;
            }
        }
        let name = part
            .attachment_name()
            .map(str::to_string)
            .or_else(|| {
                part.message()
                    .and_then(|m| m.subject())
                    .map(|s| format!("{s}.eml"))
            })
            .unwrap_or_else(|| format!("附件{}", index + 1));
        attachments.push(Attachment {
            index,
            name,
            size: part.len(),
        });
    }

    let has_remote = html.as_deref().is_some_and(|h| {
        let l = h.to_ascii_lowercase();
        l.contains("src=\"http")
            || l.contains("src='http")
            || l.contains("url(http")
            || l.contains("background=\"http")
    });
    let references = match msg.references().as_text_list() {
        Some(list) => list.iter().map(|s| s.to_string()).collect(),
        None => msg
            .references()
            .as_text()
            .map(|s| vec![s.to_string()])
            .unwrap_or_default(),
    };
    let from = people(msg.from()).into_iter().next().unwrap_or(Person {
        name: String::new(),
        addr: String::new(),
    });
    Ok(MessageView {
        uid_validity: None,
        subject: msg.subject().unwrap_or("").to_string(),
        from,
        to: people(msg.to()),
        cc: people(msg.cc()),
        bcc: people(msg.bcc()),
        date: msg
            .date()
            .map(|d| d.to_timestamp() * 1000)
            .unwrap_or(fallback_date),
        html,
        reply_html,
        text,
        has_remote,
        attachments,
        message_id: msg.message_id().map(str::to_string),
        in_reply_to: msg.in_reply_to().as_text().map(str::to_string),
        references,
        receipt_to: receipt_address(&msg),
        receipt_handled: false,
        partial: false,
        size: None,
    })
}

/// 第 `index` 个附件的文件名和内容
/// 取出第 index 个附件，连同类型一起，用于转发或重新打开草稿时原样附上。
pub fn attachment_part(raw: &[u8], index: usize) -> Result<crate::smtp::OutgoingAttachment> {
    let raw = crate::mime::fix_headers(raw);
    let msg = crate::mime::parse(&raw)?;
    let part = msg
        .attachments()
        .nth(index)
        .ok_or_else(|| anyhow!("附件不存在"))?;
    let name = part
        .attachment_name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("附件{}", index + 1));
    let content_type = part
        .content_type()
        .map(|ct| format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream")))
        .unwrap_or_else(|| crate::smtp::content_type_for(&name).to_string());
    Ok(crate::smtp::OutgoingAttachment {
        name,
        content_type,
        data: part.contents().to_vec(),
    })
}

pub fn attachment_bytes(raw: &[u8], index: usize) -> Result<(String, Vec<u8>)> {
    let raw = crate::mime::fix_headers(raw);
    let msg = crate::mime::parse(&raw)?;
    let part = msg
        .attachments()
        .nth(index)
        .ok_or_else(|| anyhow!("附件不存在"))?;
    let name = part
        .attachment_name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("附件{}", index + 1));
    Ok((name, part.contents().to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keywords_mark_receipts_as_handled_for_every_client() {
        let server = crate::test_imap::state("", &[("INBOX", 1)]);
        let uid = server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: x\r\n\r\nhi\r\n", &["\\Seen"]);
        let mut s = crate::test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        assert!(!has_keyword(&mut s, uid, MDN_SENT).await.unwrap());
        add_keyword(&mut s, uid, MDN_SENT).await.unwrap();
        assert!(has_keyword(&mut s, uid, MDN_SENT).await.unwrap());
        assert!(
            has_keyword(&mut s, uid, "$mdnsent").await.unwrap(),
            "关键字不区分大小写"
        );
    }

    async fn failing_session(
        failed_command: &str,
        failure: &str,
        prefix: &str,
    ) -> (
        Session<tokio::io::DuplexStream>,
        tokio::task::JoinHandle<Vec<String>>,
    ) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let failed_command = failed_command.to_string();
        let failure = failure.to_string();
        let prefix = prefix.to_string();
        let (client, server) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            let (reader, mut writer) = tokio::io::split(server);
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let mut commands = Vec::new();
            writer.write_all(b"* OK isolated test\r\n").await.unwrap();
            while reader.read_line(&mut line).await.unwrap() != 0 {
                let (tag, command) = line.trim_end().split_once(' ').unwrap();
                commands.push(command.to_string());
                let response = if command.starts_with(&failed_command) {
                    writer.write_all(prefix.as_bytes()).await.unwrap();
                    if failure == "EOF" {
                        break;
                    }
                    format!("{tag} {failure} test response\r\n")
                } else if command == "CAPABILITY" {
                    format!("* CAPABILITY IMAP4rev1 UIDPLUS\r\n{tag} OK capabilities\r\n")
                } else {
                    format!("{tag} OK complete\r\n")
                };
                writer.write_all(response.as_bytes()).await.unwrap();
                line.clear();
            }
            commands
        });
        let mut client = Client::new(client);
        client.read_response().await.unwrap();
        let session = client.login("test", "test").await.unwrap();
        (session, server)
    }

    #[tokio::test]
    async fn messages_flagged_deleted_are_hidden_from_lists() {
        let server = crate::test_imap::state("", &[("Drafts", 1)]);
        let raw = "From: a@x\r\nSubject: s\r\nMessage-ID: <m@x>\r\n\r\nbody\r\n";
        let keep = server.lock().unwrap().add("Drafts", raw, &["\\Draft"]);
        let gone = server
            .lock()
            .unwrap()
            .add("Drafts", raw, &["\\Draft", "\\Deleted"]);
        let mut s = crate::test_imap::session(server.clone()).await;
        timeout(OP_TIMEOUT, s.select("Drafts"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page_uids(&mut s, None).await.unwrap(), vec![keep]);
        let summaries = fetch_summaries(&mut s, &format!("{keep},{gone}"), "a", "Drafts")
            .await
            .unwrap();
        assert_eq!(
            summaries.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![keep]
        );
    }

    #[tokio::test]
    async fn unread_pages_and_summary_recipients() {
        let server = crate::test_imap::state("", &[("INBOX", 1)]);
        let mail = |n: u32| {
            format!(
                "From: a@x\r\nTo: \"Me\" <Me@Example.com>, other@x\r\nCc: c@x\r\nSubject: s{n}\r\n\r\nbody\r\n"
            )
        };
        let read = server.lock().unwrap().add("INBOX", &mail(1), &["\\Seen"]);
        let unread = server.lock().unwrap().add("INBOX", &mail(2), &[]);
        server
            .lock()
            .unwrap()
            .add("INBOX", &mail(3), &["\\Deleted"]);
        let mut s = crate::test_imap::session(server.clone()).await;
        timeout(OP_TIMEOUT, s.select("INBOX"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            page_uids_matching(&mut s, "UNSEEN", None).await.unwrap(),
            vec![unread]
        );
        assert_eq!(page_uids(&mut s, None).await.unwrap(), vec![read, unread]);
        server
            .lock()
            .unwrap()
            .add("INBOX", &mail(4), &["\\Flagged", "\\Deleted"]);
        let flagged = server
            .lock()
            .unwrap()
            .add("INBOX", &mail(5), &["\\Flagged"]);
        assert_eq!(
            page_uids_matching(&mut s, "FLAGGED", None).await.unwrap(),
            vec![flagged],
            "星标也排除已删除"
        );
        assert!(
            page_uids_matching(&mut s, "NOT DELETED", None)
                .await
                .is_err(),
            "测试服务器不认识的条件要报错，不能当作全部匹配"
        );
        let summaries = fetch_summaries(&mut s, &unread.to_string(), "a", "INBOX")
            .await
            .unwrap();
        assert_eq!(
            summaries[0].recipients,
            ["me@example.com", "other@x", "c@x"]
        );
    }

    #[test]
    fn view_keeps_in_reply_to_for_reopened_reply_drafts() {
        let raw = "From: a@x\r\nSubject: Re: s\r\nIn-Reply-To: <orig@x>\r\nReferences: <root@x> <orig@x>\r\n\r\nbody\r\n";
        let view = parse_message(raw.as_bytes(), 0).unwrap();
        assert_eq!(view.in_reply_to.as_deref(), Some("orig@x"));
    }

    #[test]
    fn attachment_part_keeps_name_type_and_bytes_and_view_keeps_bcc() {
        let raw = concat!(
            "From: a@example.com\r\nTo: b@example.com\r\nBcc: hidden@example.com\r\n",
            "Subject: s\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n",
            "--x\r\nContent-Type: text/plain\r\n\r\nhi\r\n",
            "--x\r\nContent-Type: application/pdf; name=\"=?UTF-8?B?5oql5Lu3LnBkZg==?=\"\r\n",
            "Content-Disposition: attachment; filename=\"=?UTF-8?B?5oql5Lu3LnBkZg==?=\"\r\n",
            "Content-Transfer-Encoding: base64\r\n\r\nJVBERg==\r\n--x--\r\n"
        );
        let part = attachment_part(raw.as_bytes(), 0).unwrap();
        assert_eq!(part.name, "报价.pdf");
        assert_eq!(part.content_type, "application/pdf");
        assert_eq!(part.data, b"%PDF");
        assert!(attachment_part(raw.as_bytes(), 1).is_err());
        let view = parse_message(raw.as_bytes(), 0).unwrap();
        assert_eq!(view.bcc[0].addr, "hidden@example.com");
    }

    #[tokio::test]
    async fn rejected_store_never_expunge_or_report_a_successful_move() {
        for failure in ["NO", "BAD", "EOF"] {
            let (mut session, server) = failing_session("UID STORE", failure, "").await;
            let result = move_uids(&mut session, "1", Some("Target")).await;
            drop(session);
            let commands = server.await.unwrap();
            assert!(result.is_err(), "{failure} must reject move");
            assert!(
                !commands.iter().any(|c| c.starts_with("UID EXPUNGE")),
                "must not expunge after rejected STORE"
            );
        }
    }

    #[tokio::test]
    async fn rejected_expunge_never_reports_a_successful_delete() {
        for failure in ["NO", "BAD", "EOF"] {
            let (mut session, server) = failing_session("UID EXPUNGE", failure, "").await;
            let result = move_uids(&mut session, "1", None).await;
            drop(session);
            server.await.unwrap();
            assert!(result.is_err(), "{failure} must reject permanent delete");
        }
    }

    #[tokio::test]
    async fn rejected_list_does_not_return_partial_or_empty_folders() {
        for failure in ["NO", "BAD", "EOF"] {
            for partial in ["", "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n"] {
                let (mut session, server) = failing_session("LIST", failure, partial).await;
                let result = list_folders(&mut session).await;
                drop(session);
                server.await.unwrap();
                assert!(
                    result.is_err(),
                    "{failure} must reject LIST, partial={partial:?}"
                );
            }
        }
    }

    #[tokio::test]
    async fn rejected_sent_search_never_appends_a_duplicate_copy() {
        for failure in ["NO", "BAD", "EOF"] {
            let (mut session, server) = failing_session("UID SEARCH", failure, "").await;
            let result = ensure_saved(
                &mut session,
                "Sent",
                "<test@example.com>",
                b"Subject: test\r\n\r\nbody",
            )
            .await;
            drop(session);
            let commands = server.await.unwrap();
            assert!(result.is_err());
            assert!(
                !commands.iter().any(|c| c.starts_with("APPEND")),
                "{failure} must not be mistaken for absent sent message"
            );
        }
    }

    #[tokio::test]
    async fn strict_list_keeps_roles_and_delimiters_and_omits_noselect() {
        let list = concat!(
            "* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n",
            "* LIST (\\Sent) \"/\" \"Mail/Sent\"\r\n",
            "* LIST (\\Noselect) \"/\" \"Mail\"\r\n",
            "* LIST () \"/\" \"Mail/Reports\"\r\n",
        );
        let (mut session, server) = failing_session("LIST", "OK", list).await;
        let folders = list_folders(&mut session).await.unwrap();
        drop(session);
        let commands = server.await.unwrap();
        assert!(commands.iter().any(|c| c == "LIST \"\" \"*\""));
        assert_eq!(folders.len(), 3);
        assert!(
            folders
                .iter()
                .any(|f| f.path == "Mail/Sent" && f.role == "sent")
        );
        assert!(
            folders
                .iter()
                .any(|f| f.path == "Mail/Reports" && f.name == "Reports")
        );
    }

    #[tokio::test]
    async fn strict_sent_search_preserves_existing_copy_and_requires_search_data() {
        for partial in ["* SEARCH 7\r\n", ""] {
            let (mut session, server) = failing_session("UID SEARCH", "OK", partial).await;
            let result = ensure_saved(
                &mut session,
                "Sent",
                "<test@example.com>",
                b"Subject: test\r\n\r\nbody",
            )
            .await;
            drop(session);
            let commands = server.await.unwrap();
            if partial.is_empty() {
                assert!(
                    result.is_err(),
                    "tagged OK without SEARCH data cannot prove absence"
                );
            } else {
                assert!(
                    !result.unwrap(),
                    "already saved mail must not be appended again"
                );
            }
            assert!(!commands.iter().any(|c| c.starts_with("APPEND")));
        }
    }

    #[tokio::test]
    async fn strict_move_and_delete_accept_confirmed_success() {
        for target in [Some("Target"), None] {
            let (mut session, server) = failing_session("never matches", "OK", "").await;
            move_uids(&mut session, "1", target).await.unwrap();
            drop(session);
            let commands = server.await.unwrap();
            assert!(commands.iter().any(|c| c == "UID EXPUNGE 1"));
            assert_eq!(
                commands.iter().any(|c| c.starts_with("UID COPY")),
                target.is_some()
            );
        }
    }

    #[test]
    fn reply_html_keeps_cid_while_reader_html_embeds_it() {
        let raw = b"From: sender@example.com\r\nSubject: inline\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=q\r\n\r\n--q\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p style=\"color:red\">hello<img src=\"cid:logo@example.com\"></p>\r\n--q\r\nContent-Type: image/png\r\nContent-ID: <logo@example.com>\r\nContent-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\naW1hZ2U=\r\n--q--\r\n";
        let view = parse_message(raw, 0).unwrap();
        let json = serde_json::to_value(view).unwrap();
        assert!(
            json["html"]
                .as_str()
                .unwrap()
                .contains("data:image/png;base64,aW1hZ2U=")
        );
        assert!(
            json["replyHtml"]
                .as_str()
                .is_some_and(|html| html.contains("cid:logo@example.com"))
        );
        assert_eq!(json["attachments"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn encodes_modified_utf7_for_ascii_chinese_ampersands_and_surrogates() {
        assert_eq!(encode_mutf7("已发送"), "&XfJT0ZAB-");
        assert_eq!(encode_mutf7("A&B"), "A&-B");
        for name in ["", "INBOX", "项目归档", "客户 & 合作 🤝", "𠮷", "a ~ z"] {
            assert_eq!(decode_mutf7(&encode_mutf7(name)), name);
        }
    }

    #[test]
    fn folder_names_must_be_nonempty_flat_and_without_controls() {
        assert_eq!(
            validate_folder_name("  项目 & 资料 📨  ", Some("/")).unwrap(),
            "项目 & 资料 📨"
        );
        for name in ["", "  ", "a\nb", "a\0b", "a\tb", "a/b"] {
            assert!(validate_folder_name(name, Some("/")).is_err(), "{name:?}");
        }
        assert!(validate_folder_name("a.b", Some(".")).is_err());
        assert!(validate_folder_name("a.b", Some("/")).is_ok());
        assert!(validate_folder_name("a/b", None).is_ok());
    }

    #[test]
    fn moves_never_use_mailbox_wide_expunge() {
        assert_eq!(
            move_strategy(true, false, true).unwrap(),
            MoveStrategy::Move
        );
        assert_eq!(
            move_strategy(false, true, true).unwrap(),
            MoveStrategy::CopyAndExpunge
        );
        assert_eq!(
            move_strategy(true, true, false).unwrap(),
            MoveStrategy::Expunge
        );
        assert!(move_strategy(false, false, true).is_err());
        assert!(move_strategy(true, false, false).is_err());
    }

    #[test]
    fn mutf7() {
        assert_eq!(decode_mutf7("&XfJT0ZAB-"), "已发送");
        assert_eq!(decode_mutf7("INBOX"), "INBOX");
        assert_eq!(decode_mutf7("&-"), "&");
        assert_eq!(decode_mutf7("Other/&g0l6Pw-"), "Other/草稿");
    }

    #[test]
    fn decodes_legacy_headers_for_list_and_reader() {
        // 固定的 RFC 2047 字节夹具：主题与正文可以使用不同字符集。
        // GB2312 用例来自出现乱码的通知主题，不含账号或预约信息。
        let cases = [
            (
                "=?gb2312?B?zqq908rcIEdlbml1cyBCYXIgzOyyxbDJt/7O8df2usPXvLG4?=",
                "为接受 Genius Bar 天才吧服务做好准备",
            ),
            (
                "=?GB2312?B?zOHQ0aO6?=\r\n\t=?GB2312?B?zqq908rcIEdlbml1cyBCYXIgzOyyxbDJt/7O8df2usPXvLG4?=",
                "提醒：为接受 Genius Bar 天才吧服务做好准备",
            ),
            ("=?Big5?B?wWPF6aSkpOW5d6z5tKO/9A==?=", "繁體中文預約提醒"),
            // 包含 GB18030 的四字节字符，避免只验证其与 GBK 重叠的部分。
            ("=?GB18030?B?1KTUvJU0sjXM4dDR?=", "预约𠮷提醒"),
            (
                "=?UTF-8?B?5o6o6YCB5rWL6K+V77ya5L2g5aW9IPCfkYs=?=",
                "推送测试：你好 👋",
            ),
        ];

        for (subject_header, expected_subject) in cases {
            let headers = format!(
                "From: =?GBK?Q?=D5=C5=C8=FD?= <sender@example.com>\r\n\
                 Subject: {subject_header}\r\n\
                 MIME-Version: 1.0\r\n\
                 Content-Type: text/plain; charset=utf-8\r\n\r\n"
            );
            // 列表只解析 IMAP 返回的邮件头。
            let summary = MessageParser::default()
                .parse_headers(headers.as_bytes())
                .unwrap();
            assert_eq!(summary.subject(), Some(expected_subject));
            assert_eq!(
                first_addr(summary.from()),
                ("张三".into(), "sender@example.com".into())
            );

            // 阅读页解析完整原文，不能依赖列表已经解码的主题。
            let raw = format!("{headers}正文中文正常");
            let view = parse_message(raw.as_bytes(), 0).unwrap();
            assert_eq!(view.subject, expected_subject);
            assert_eq!(view.from.name, "张三");
            assert_eq!(view.from.addr, "sender@example.com");
            assert_eq!(view.text, "正文中文正常");
        }
    }

    #[test]
    fn text_only_message_keeps_headers_and_uses_the_parts_own_encoding() {
        let header = b"From: =?UTF-8?B?5byg5LiJ?= <sender@example.com>\r\nSubject: talk\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related;\r\n\tboundary=\"b1\"\r\nTo: me@example.com\r\n\r\n";
        let mime =
            b"Content-Type: text/html; charset=gbk\r\nContent-Transfer-Encoding: base64\r\n\r\n";
        // GBK 编码的「正文」
        let body = b"PHA+1f3OxDxpbWcgc3JjPSJjaWQ6YSI+PC9wPg==\r\n";
        let raw = assemble_text_message(header, mime, body);
        let view = parse_message(&raw, 0).unwrap();
        assert_eq!(view.subject, "talk");
        assert_eq!(view.from.addr, "sender@example.com");
        assert_eq!(view.to[0].addr, "me@example.com");
        let html = view.html.unwrap();
        assert!(html.contains("正文"), "{html}");
        assert!(view.attachments.is_empty());
        assert!(
            !String::from_utf8_lossy(&raw).contains("boundary"),
            "原来的多部分头不能留下"
        );
    }

    #[test]
    fn raw_8bit_headers_survive_the_text_only_message() {
        // 老邮件程序直接用 GBK 写邮件头，不做 RFC 2047 编码
        let mut header = b"From: a@example.com\r\nSubject: ".to_vec();
        header.extend_from_slice(&[0xd5, 0xfd, 0xce, 0xc4]); // GBK「正文」
        header.extend_from_slice(b"\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n");
        let raw = assemble_text_message(&header, b"Content-Type: text/plain\r\n\r\n", b"hi");
        assert!(
            raw.windows(4).any(|w| w == [0xd5, 0xfd, 0xce, 0xc4]),
            "原样保留邮件头字节"
        );
        assert!(!raw.windows(8).any(|w| w == b"boundary"));
    }

    #[test]
    fn parses_html_with_inline_image() {
        let raw = b"From: =?UTF-8?B?5byg5LiJ?= <a@example.com>\r\nTo: b@example.com\r\nSubject: hi\r\nMessage-ID: <x@y>\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>hello</p><img src=\"cid:img1\">\r\n--b\r\nContent-Type: image/png\r\nContent-ID: <img1>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b--\r\n";
        let v = parse_message(raw, 0).unwrap();
        assert_eq!(v.from.name, "张三");
        assert!(
            v.html
                .as_deref()
                .unwrap()
                .contains("data:image/png;base64,")
        );
        assert!(v.attachments.is_empty());
        assert!(!v.has_remote);
        assert_eq!(v.message_id.as_deref(), Some("x@y"));
    }

    #[test]
    fn plain_text_has_no_html() {
        let raw = b"From: a@example.com\r\nSubject: t\r\n\r\nline1\r\nline2\r\n";
        let v = parse_message(raw, 7).unwrap();
        assert!(v.html.is_none());
        assert!(v.text.contains("line2"));
        assert_eq!(v.date, 7);
    }

    #[test]
    fn attachment_indices_stay_stable_when_inline_images_are_skipped() {
        let raw = b"From: a@example.com\r\nTo: me@example.com\r\nSubject: x\r\nMIME-Version: 1.0\r\nContent-Type: multipart/related; boundary=\"b\"\r\n\r\n--b\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<p>hi<img src=\"cid:img1\"></p>\r\n--b\r\nContent-Type: image/png\r\nContent-ID: <img1>\r\nContent-Transfer-Encoding: base64\r\n\r\niVBORw0KGgo=\r\n--b\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=\"manual.pdf\"\r\nContent-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQ=\r\n--b--\r\n";
        let v = parse_message(raw, 0).unwrap();
        // 内嵌图片转成 data URI 后不再列为附件，但附件自己的 index 要对得上
        assert_eq!(v.attachments.len(), 1);
        assert_eq!(v.attachments[0].name, "manual.pdf");
        let (name, data) = attachment_bytes(raw, v.attachments[0].index).unwrap();
        assert_eq!(name, "manual.pdf");
        assert_eq!(data, b"%PDF-1.4");
    }

    #[test]
    fn raw_gbk_headers_do_not_turn_into_replacement_characters() {
        let name = [0xd3, 0xca, 0xbc, 0xfe, 0xd6, 0xfa, 0xca, 0xd6]; // 邮件助手（GBK）
        let mut raw = b"From: \"".to_vec();
        raw.extend_from_slice(&name);
        raw.extend_from_slice(
            "\" <10000@exmail.weixin.qq.com>\r\nSubject: 企业微信邮箱登录提醒\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n正文"
                .as_bytes(),
        );
        let view = parse_message(&raw, 0).unwrap();
        assert_eq!(view.from.name, "邮件助手");
        assert_eq!(view.from.addr, "10000@exmail.weixin.qq.com");
        assert_eq!(view.text, "正文");
    }

    #[tokio::test]
    async fn limited_page_reports_whether_older_mail_exists() {
        let server = crate::test_imap::state("", &[("INBOX", 1)]);
        for n in 1..=5 {
            server
                .lock()
                .unwrap()
                .add("INBOX", &format!("Subject: m{n}\r\n\r\nbody"), &[]);
        }
        let mut s = crate::test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        let (uids, more) = page_uids_matching_limited(&mut s, "", None, 2)
            .await
            .unwrap();
        assert_eq!(uids, [4, 5], "取最新的一小页");
        assert!(more, "后面还有更早的");
        let (uids, more) = page_uids_matching_limited(&mut s, "", None, 5)
            .await
            .unwrap();
        assert_eq!(uids.len(), 5);
        assert!(!more, "整个文件夹都取完了");
        let (uids, more) = page_uids_matching_limited(&mut s, "", Some(4), 2)
            .await
            .unwrap();
        assert_eq!(uids, [2, 3], "继续往前翻页");
        assert!(more);
    }
}
