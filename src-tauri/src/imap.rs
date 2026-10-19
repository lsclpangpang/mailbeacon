//! IMAP 底层操作：连接登录、拉取邮件列表、文件夹、解析正文。

use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};

use anyhow::{Context, Result, anyhow};
use async_imap::{Client, Session, imap_proto::NameAttribute, types::Flag};
use futures::TryStreamExt;
use mail_parser::{Address, MessageParser, MimeHeaders};
use serde::Serialize;
use tokio::{net::TcpStream, time::timeout};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls};

use crate::config::Account;

pub type Imap = Session<TlsStream<TcpStream>>;

/// 单个网络操作的超时：超过这个时间没响应就认为连接已失效。
pub const OP_TIMEOUT: Duration = Duration::from_secs(20);
/// 首屏和「加载更多」每次拉多少封
pub const PAGE: usize = 100;

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
    let mut client = Client::new(stream);
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
            ("name", Some("MailPulse")),
            ("version", Some(env!("CARGO_PKG_VERSION"))),
            ("vendor", Some("MailPulse")),
        ];
        let _ = timeout(OP_TIMEOUT, s.id(ident)).await;
    }
    Ok(s)
}

pub async fn has_capability(s: &mut Imap, cap: &str) -> Result<bool> {
    Ok(timeout(OP_TIMEOUT, s.capabilities()).await??.has_str(cap))
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Summary {
    pub account_id: String,
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
pub async fn fetch_summaries(
    s: &mut Imap,
    set: &str,
    acct_id: &str,
    folder: &str,
) -> Result<Vec<Summary>> {
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
        let parsed = m.header().and_then(|h| parser.parse_headers(h));
        let p = parsed.as_ref();
        let (from_name, from_addr) = first_addr(p.and_then(|p| p.from()));
        let mut seen = false;
        let mut flagged = false;
        for f in m.flags() {
            match f {
                Flag::Seen => seen = true,
                Flag::Flagged => flagged = true,
                _ => {}
            }
        }
        let date = m
            .internal_date()
            .map(|d| d.timestamp_millis())
            .or_else(|| p.and_then(|p| p.date()).map(|d| d.to_timestamp() * 1000))
            .unwrap_or(0);
        let attachment = p.and_then(|p| p.content_type()).is_some_and(|ct| {
            ct.ctype().eq_ignore_ascii_case("multipart")
                && ct
                    .subtype()
                    .is_some_and(|s| s.eq_ignore_ascii_case("mixed"))
        });
        out.push(Summary {
            account_id: acct_id.to_string(),
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
        });
    }
    out.sort_by_key(|m| std::cmp::Reverse(m.uid));
    Ok(out)
}

pub async fn fetch_flags(s: &mut Imap, set: &str) -> Result<Vec<FlagState>> {
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

pub async fn unseen_count(s: &mut Imap) -> Result<usize> {
    Ok(timeout(OP_TIMEOUT, s.uid_search("UNSEEN")).await??.len())
}

/// 已选中文件夹里 UID 小于 `before` 的最新一页（`before` 为空表示最新一页）。
pub async fn page_uids(s: &mut Imap, before: Option<u32>) -> Result<Vec<u32>> {
    let mut uids: Vec<u32> = timeout(OP_TIMEOUT * 2, s.uid_search("ALL"))
        .await
        .context("搜索邮件超时")??
        .into_iter()
        .filter(|u| before.is_none_or(|b| *u < b))
        .collect();
    uids.sort_unstable();
    let start = uids.len().saturating_sub(PAGE);
    Ok(uids.split_off(start))
}

/// 把已选中文件夹里的邮件移到 `target`；`target` 为空表示彻底删除。
/// 服务器支持 MOVE 就直接移动，否则复制 + 标记删除 + 清除。
pub async fn move_uids(s: &mut Imap, set: &str, target: Option<&str>) -> Result<()> {
    if let Some(t) = target {
        if has_capability(s, "MOVE").await? {
            timeout(OP_TIMEOUT, s.uid_mv(set, t))
                .await
                .context("移动邮件超时")??;
            return Ok(());
        }
        timeout(OP_TIMEOUT, s.uid_copy(set, t))
            .await
            .context("复制邮件超时")??;
    }
    timeout(OP_TIMEOUT, async {
        s.uid_store(set, "+FLAGS.SILENT (\\Deleted)")
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("删除邮件超时")??;
    if has_capability(s, "UIDPLUS").await? {
        timeout(OP_TIMEOUT, async {
            s.uid_expunge(set).await?.try_collect::<Vec<_>>().await
        })
        .await??;
    } else {
        timeout(OP_TIMEOUT, async {
            s.expunge().await?.try_collect::<Vec<_>>().await
        })
        .await??;
    }
    Ok(())
}

/// 已选中的 `folder` 里没有这个 Message-ID 的邮件，就补存一份（标为已读）。返回是否补存了。
pub async fn ensure_saved(
    s: &mut Imap,
    folder: &str,
    message_id: &str,
    raw: &[u8],
) -> Result<bool> {
    let found = timeout(
        OP_TIMEOUT,
        s.uid_search(format!("HEADER Message-ID \"{message_id}\"")),
    )
    .await??;
    if !found.is_empty() {
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

pub async fn list_folders(s: &mut Imap) -> Result<Vec<Folder>> {
    let names = timeout(OP_TIMEOUT, async {
        s.list(Some(""), Some("*"))
            .await?
            .try_collect::<Vec<_>>()
            .await
    })
    .await
    .context("读取文件夹超时")??;
    let mut out = Vec::new();
    for n in &names {
        let attrs = n.attributes();
        if attrs.iter().any(|a| matches!(a, NameAttribute::NoSelect)) {
            continue;
        }
        let path = n.name().to_string();
        let decoded = decode_mutf7(&path);
        let leaf = match n.delimiter() {
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
    pub subject: String,
    pub from: Person,
    pub to: Vec<Person>,
    pub cc: Vec<Person>,
    pub date: i64,
    pub html: Option<String>,
    pub text: String,
    pub has_remote: bool,
    pub attachments: Vec<Attachment>,
    pub message_id: Option<String>,
    pub references: Vec<String>,
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
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow!("无法解析这封邮件"))?;
    let real_html = msg.html_part(0).is_some_and(|p| p.is_text_html());
    let mut html = if real_html {
        msg.body_html(0).map(|h| h.into_owned())
    } else {
        None
    };
    let text = msg.body_text(0).map(|t| t.into_owned()).unwrap_or_default();

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
        subject: msg.subject().unwrap_or("").to_string(),
        from,
        to: people(msg.to()),
        cc: people(msg.cc()),
        date: msg
            .date()
            .map(|d| d.to_timestamp() * 1000)
            .unwrap_or(fallback_date),
        html,
        text,
        has_remote,
        attachments,
        message_id: msg.message_id().map(str::to_string),
        references,
    })
}

/// 第 `index` 个附件的文件名和内容
pub fn attachment_bytes(raw: &[u8], index: usize) -> Result<(String, Vec<u8>)> {
    let msg = MessageParser::default()
        .parse(raw)
        .ok_or_else(|| anyhow!("无法解析这封邮件"))?;
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

    #[test]
    fn mutf7() {
        assert_eq!(decode_mutf7("&XfJT0ZAB-"), "已发送");
        assert_eq!(decode_mutf7("INBOX"), "INBOX");
        assert_eq!(decode_mutf7("&-"), "&");
        assert_eq!(decode_mutf7("Other/&g0l6Pw-"), "Other/草稿");
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
}
