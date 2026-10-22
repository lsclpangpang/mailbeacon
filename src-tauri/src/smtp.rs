//! 发信：SMTP 发出后，确认「已发送」里有这封邮件，没有就补存一份。
//! QQ/163 等是否自动保存取决于网页版设置，Gmail 一定会自动保存，所以按实际结果判断，避免重复。

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    address::Envelope,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Certificate, Tls, TlsParameters},
    },
};
#[cfg(test)]
use mailbeacon_parser::MessageParser;
use mailbeacon_parser::MimeHeaders;
use serde::Deserialize;

use crate::config::Account;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub account_id: String,
    pub to: String,
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub bcc: String,
    pub subject: String,
    pub body: String,
    #[serde(default)]
    pub html_body: Option<String>,
    #[serde(default)]
    pub quote_source: Option<QuoteSource>,
    #[serde(default)]
    pub quote_cids: Vec<String>,
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
    /// 要随信发送的附件：本地文件（选择文件时登记的令牌）或已有邮件里的附件
    #[serde(default)]
    pub attachments: Vec<AttachmentRef>,
    /// 请求已读回执（Disposition-Notification-To，RFC 8098）
    #[serde(default)]
    pub request_receipt: bool,
}

/// 请求已读回执的邮件头：回执发回发件人自己。
#[derive(Clone)]
struct DispositionNotificationTo(String);

impl lettre::message::header::Header for DispositionNotificationTo {
    fn name() -> lettre::message::header::HeaderName {
        lettre::message::header::HeaderName::new_from_ascii_str("Disposition-Notification-To")
    }

    fn parse(s: &str) -> std::result::Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self(s.to_string()))
    }

    fn display(&self) -> lettre::message::header::HeaderValue {
        lettre::message::header::HeaderValue::new(Self::name(), self.0.clone())
    }
}

/// 附件来源。网页端只拿得到令牌和邮件位置，拿不到也不能指定任意本地路径。
#[derive(Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AttachmentRef {
    /// 用户在文件对话框或拖拽中选中的文件
    #[serde(rename_all = "camelCase")]
    File { token: String },
    /// 已有邮件（转发的原邮件、重新打开的草稿）里的第 index 个附件
    #[serde(rename_all = "camelCase")]
    Message {
        account_id: String,
        folder: String,
        uid: u32,
        #[serde(default)]
        uid_validity: Option<u32>,
        index: usize,
    },
}

/// 已读出内容、准备打包的附件。
#[derive(Clone)]
pub struct OutgoingAttachment {
    pub name: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

/// 附件总大小上限。QQ、163 等普通附件上限约 50 MB，再大服务器会拒收。
pub const MAX_ATTACHMENT_BYTES: usize = 50 * 1024 * 1024;

/// 发出去的邮件不带密送且必须有收件人；草稿保留密送、允许暂时没有收件人。
#[derive(Clone, Copy, PartialEq)]
pub enum Purpose {
    Send,
    Draft,
}

/// 按扩展名推断附件类型；不认识的一律 application/octet-stream。
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "heic" => "image/heic",
        "svg" => "image/svg+xml",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "htm" | "html" => "text/html",
        "md" => "text/markdown",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "rar" => "application/vnd.rar",
        "7z" => "application/x-7z-compressed",
        "gz" => "application/gzip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        "key" => "application/vnd.apple.keynote",
        "pages" => "application/vnd.apple.pages",
        "numbers" => "application/vnd.apple.numbers",
        "eml" => "message/rfc822",
        "ics" => "text/calendar",
        "vcf" => "text/vcard",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

/// 原邮件的位置独立于发件账号，切换发件人不会改变引用来源。
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuoteSource {
    pub account_id: String,
    pub folder: String,
    pub uid: u32,
    #[serde(default)]
    pub uid_validity: Option<u32>,
}

/// "张三 <a@x.com>, b@y.com；c@z.com" → 收件人列表
/// 按逗号、分号（含中文标点）和换行切分收件人；引号里的分隔符属于名字。
fn split_recipients(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ',' | ';' | '，' | '；' | '\n' if !quoted => {
                parts.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .collect()
}

fn parse_list(s: &str) -> Result<Vec<Mailbox>> {
    split_recipients(s)
        .into_iter()
        .map(|x| {
            x.parse::<Mailbox>()
                .map_err(|_| anyhow!("邮箱地址格式不对：{x}"))
        })
        .collect()
}

/// 草稿里所有收件人（收件、抄送、密送）的名字和地址，用于记录最近联系人。
pub fn recipients(d: &Draft) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for list in [&d.to, &d.cc, &d.bcc] {
        for m in parse_list(list)? {
            out.push((m.name.unwrap_or_default(), m.email.to_string()));
        }
    }
    Ok(out)
}

fn angle(id: &str) -> String {
    let id = id.trim().trim_matches(|c| c == '<' || c == '>');
    format!("<{id}>")
}

/// 构造邮件，返回（邮件，Message-ID）
#[cfg(test)]
pub fn build(acct: &Account, d: &Draft) -> Result<(Message, String)> {
    build_with_quote(acct, d, None)
}

#[cfg(test)]
pub fn build_with_quote(
    acct: &Account,
    d: &Draft,
    quote_raw: Option<&[u8]>,
) -> Result<(Message, String)> {
    compose(acct, d, quote_raw, Vec::new(), Purpose::Send)
}

/// 组装邮件：保留 HTML 正文，从原邮件复制引用中的内嵌图片，再附上 `attachments`。
/// 原邮件里没被引用的普通附件不会自动跟随回复发送。
pub fn compose(
    acct: &Account,
    d: &Draft,
    quote_raw: Option<&[u8]>,
    attachments: Vec<OutgoingAttachment>,
    purpose: Purpose,
) -> Result<(Message, String)> {
    let to = parse_list(&d.to)?;
    let cc = parse_list(&d.cc)?;
    let bcc = parse_list(&d.bcc)?;
    if purpose == Purpose::Send && to.is_empty() && cc.is_empty() && bcc.is_empty() {
        bail!("请填写收件人");
    }
    let total: usize = attachments.iter().map(|a| a.data.len()).sum();
    if total > MAX_ATTACHMENT_BYTES {
        bail!(
            "附件总大小 {:.1} MB，超过 {} MB 上限",
            total as f64 / 1024.0 / 1024.0,
            MAX_ATTACHMENT_BYTES / 1024 / 1024
        );
    }
    let domain = acct.email.split('@').nth(1).unwrap_or("mailpulse.local");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let message_id = format!("<{nanos:x}.{:x}@{domain}>", std::process::id());

    let from = Mailbox::new(
        Some(acct.name.clone()),
        acct.email.parse().context("发件地址格式不对")?,
    );
    let mut b = Message::builder()
        .from(from)
        .subject(d.subject.clone())
        .message_id(Some(message_id.clone()));
    if purpose == Purpose::Draft {
        b = b.keep_bcc();
        if to.is_empty() && cc.is_empty() && bcc.is_empty() {
            // 草稿只存不发，但 lettre 组装时需要信封；用发件人自己占位。
            let me: Address = acct.email.parse().context("发件地址格式不对")?;
            b = b.envelope(Envelope::new(Some(me.clone()), vec![me])?);
        }
    }
    for m in to {
        b = b.to(m);
    }
    for m in cc {
        b = b.cc(m);
    }
    for m in bcc {
        b = b.bcc(m);
    }
    if d.request_receipt {
        b = b.header(DispositionNotificationTo(format!("<{}>", acct.email)));
    }
    if let Some(irt) = d.in_reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
        b = b.in_reply_to(angle(irt));
        let mut refs: Vec<String> = d.references.iter().map(|r| angle(r)).collect();
        refs.push(angle(irt));
        refs.dedup();
        b = b.references(refs.join(" "));
    }
    let plain = d.body.replace("\r\n", "\n").replace('\n', "\r\n");
    let body = if let Some(html) = d.html_body.as_ref() {
        let mut alternative = MultiPart::alternative().singlepart(SinglePart::plain(plain));
        let mut images = quoted_images(&d.quote_cids, quote_raw)?;
        // 编辑器里插入的图片是 data: URL，换成 cid 内嵌图片，收件方客户端才都能显示
        let (html, inserted) = crate::html_mail::extract_data_images(html);
        let inserted_bytes: usize = inserted.iter().map(|i| i.data.len()).sum();
        if total + inserted_bytes > MAX_ATTACHMENT_BYTES {
            bail!(
                "正文图片和附件合计超过 {} MB 上限",
                MAX_ATTACHMENT_BYTES / 1024 / 1024
            );
        }
        for image in inserted {
            let content_type =
                ContentType::parse(&image.content_type).context("正文图片类型无效")?;
            images.push(Attachment::new_inline(image.cid).body(image.data, content_type));
        }
        let html = &html;
        if images.is_empty() {
            alternative = alternative.singlepart(SinglePart::html(html.clone()));
        } else {
            let mut related = MultiPart::related().singlepart(SinglePart::html(html.clone()));
            for image in images {
                related = related.singlepart(image);
            }
            alternative = alternative.multipart(related);
        }
        Body::Multi(alternative)
    } else {
        if !d.quote_cids.is_empty() {
            bail!("缺少带格式的引用正文，无法保留内嵌图片");
        }
        Body::Plain(plain)
    };
    let msg = match (body, attachments.is_empty()) {
        (Body::Plain(plain), true) => b.header(ContentType::TEXT_PLAIN).body(plain)?,
        (Body::Multi(multi), true) => b.multipart(multi)?,
        (body, false) => {
            let mut mixed = match body {
                Body::Plain(plain) => MultiPart::mixed().singlepart(SinglePart::plain(plain)),
                Body::Multi(multi) => MultiPart::mixed().multipart(multi),
            };
            for file in attachments {
                let content_type = ContentType::parse(&file.content_type)
                    .unwrap_or_else(|_| ContentType::parse("application/octet-stream").unwrap());
                mixed = mixed.singlepart(Attachment::new(file.name).body(file.data, content_type));
            }
            b.multipart(mixed)?
        }
    };
    Ok((msg, message_id))
}

enum Body {
    Plain(String),
    Multi(MultiPart),
}

fn quoted_images(cids: &[String], raw: Option<&[u8]>) -> Result<Vec<SinglePart>> {
    if cids.is_empty() {
        return Ok(Vec::new());
    }
    let raw = raw.ok_or_else(|| anyhow!("无法读取引用邮件的内嵌图片，请重新打开原邮件后回复"))?;
    let raw = crate::mime::fix_headers(raw);
    let msg = crate::mime::parse(raw.as_ref()).context("无法解析引用邮件的内嵌图片")?;
    let mut images = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for cid in cids {
        if cid.is_empty()
            || cid
                .chars()
                .any(|c| c.is_control() || matches!(c, '<' | '>'))
        {
            bail!("原邮件的内嵌图片标识无效");
        }
        if !seen.insert(cid) {
            continue;
        }
        let mut matches = msg
            .attachments()
            .filter(|p| p.content_id() == Some(cid.as_str()));
        let part = matches
            .next()
            .ok_or_else(|| anyhow!("原邮件中的内嵌图片已不可用，请重新打开原邮件后回复"))?;
        if matches.next().is_some() {
            bail!("原邮件的内嵌图片标识重复，无法确定正确图片");
        }
        let mime = part
            .content_type()
            .filter(|ct| ct.ctype().eq_ignore_ascii_case("image"))
            .ok_or_else(|| anyhow!("引用包含不支持的内嵌内容，仅支持内嵌图片"))?;
        let content_type = ContentType::parse(&format!(
            "{}/{}",
            mime.ctype(),
            mime.subtype().unwrap_or("octet-stream")
        ))
        .context("原邮件的内嵌图片类型无效")?;
        images
            .push(Attachment::new_inline(cid.clone()).body(part.contents().to_vec(), content_type));
    }
    Ok(images)
}

/// 非 ASCII 文本编码成 RFC 2047 的 encoded-word，按字切开、每段一行，
/// 保证折行后每行不超过 78 个字符（第一行前面还有「Subject: 」之类的头名）。
fn encoded_words(s: &str) -> String {
    use base64::Engine;
    let mut words = Vec::new();
    let mut chunk = String::new();
    for c in s.chars() {
        if chunk.len() + c.len_utf8() > 39 {
            words.push(std::mem::take(&mut chunk));
        }
        chunk.push(c);
    }
    if !chunk.is_empty() {
        words.push(chunk);
    }
    words
        .iter()
        .map(|w| {
            format!(
                "=?utf-8?b?{}?=",
                base64::engine::general_purpose::STANDARD.encode(w.as_bytes())
            )
        })
        .collect::<Vec<_>>()
        .join("\r\n ")
}

/// 发件人显示名：ASCII 的用引号括起来（「Li, Jarvis」里的逗号不能被当成两个地址）。
fn display_name(name: &str) -> String {
    if name.is_ascii() && !name.chars().any(|c| c.is_control()) && name.len() <= 60 {
        format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        encoded_words(name)
    }
}

/// Message-ID 里不该有的空白和控制字符（折行的头会带进来）去掉。
fn clean_id(id: &str) -> String {
    id.trim_matches(['<', '>'])
        .chars()
        .filter(|c| !c.is_whitespace() && !c.is_control() && !matches!(c, '<' | '>'))
        .collect()
}

/// base64 正文按 76 个字符一行。
fn base64_lines(data: &[u8]) -> String {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(data);
    encoded
        .as_bytes()
        .chunks(76)
        .map(|c| String::from_utf8_lossy(c).into_owned())
        .collect::<Vec<_>>()
        .join("\r\n")
}

/// 已读回执（RFC 8098 的 multipart/report）：返回回执要发给的地址和整封原文。
/// 只在用户点了「发送回执」时才调用，从不自动发送。
pub fn receipt(acct: &Account, original: &crate::imap::MessageView) -> Result<(String, Vec<u8>)> {
    let to = original
        .receipt_to
        .clone()
        .ok_or_else(|| anyhow!("这封邮件没有请求已读回执"))?;
    let domain = acct.email.split('@').nth(1).unwrap_or("mailpulse.local");
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let boundary = format!("mdn-{nanos:x}");
    let now = chrono::Local::now();
    let sent = chrono::DateTime::from_timestamp_millis(original.date)
        .filter(|_| original.date > 0)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string()
        });
    let subject = if original.subject.trim().is_empty() {
        "(无主题)".to_string()
    } else {
        // 主题里的换行（服务器原样给的）不能带进新邮件头
        original
            .subject
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect()
    };
    let human = format!(
        "您{}发给 {} 的邮件「{}」已于 {} 被阅读。\r\n\r\n这只表示邮件在收件人的电脑上显示过，不代表内容已被阅读或理解。\r\n",
        sent.map(|d| format!("于 {d} ")).unwrap_or_default(),
        acct.email,
        subject,
        now.format("%Y-%m-%d %H:%M"),
    );
    let original_id = original
        .message_id
        .as_deref()
        .map(clean_id)
        .filter(|id| !id.is_empty());
    let mut raw = String::new();
    raw += &format!("From: {} <{}>\r\n", display_name(&acct.name), acct.email);
    raw += &format!("To: <{to}>\r\n");
    raw += &format!(
        "Subject: {}\r\n",
        encoded_words(&format!("已读：{subject}"))
    );
    raw += &format!("Date: {}\r\n", now.to_rfc2822());
    raw += &format!("Message-ID: <{nanos:x}.mdn@{domain}>\r\n");
    if let Some(id) = &original_id {
        raw += &format!("In-Reply-To: <{id}>\r\nReferences: <{id}>\r\n");
    }
    raw += "MIME-Version: 1.0\r\n";
    raw += &format!(
        "Content-Type: multipart/report; report-type=disposition-notification;\r\n boundary=\"{boundary}\"\r\n\r\n"
    );
    raw += &format!(
        "--{boundary}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n{}\r\n",
        base64_lines(human.as_bytes())
    );
    raw += &format!(
        "--{boundary}\r\nContent-Type: message/disposition-notification\r\n\r\nReporting-UA: MailBeacon\r\nFinal-Recipient: rfc822; {}\r\n{}Disposition: manual-action/MDN-sent-manually; displayed\r\n\r\n--{boundary}--\r\n",
        acct.email,
        original_id
            .map(|id| format!("Original-Message-ID: <{id}>\r\n"))
            .unwrap_or_default()
    );
    Ok((to, raw.into_bytes()))
}

/// 把邮件头里的 Date 换成真正发出的时间（定时邮件是提前组好的）。
pub fn with_date<Tz: chrono::TimeZone>(raw: &[u8], at: &chrono::DateTime<Tz>) -> Vec<u8>
where
    Tz::Offset: std::fmt::Display,
{
    replace_header(raw, "Date", &at.to_rfc2822())
}

/// 显式重新发送创建新的邮件身份，保留 MIME 正文的原始字节。
pub fn with_message_id(raw: &[u8], id: &str) -> Vec<u8> {
    replace_header(raw, "Message-ID", id)
}

fn replace_header(raw: &[u8], name: &str, value: &str) -> Vec<u8> {
    let prefix = format!("{name}:");
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map_or(raw.len(), |p| p + 2);
    let (head, body) = raw.split_at(end);
    let head = String::from_utf8_lossy(head);
    let mut out = String::new();
    let mut skipping = false;
    let mut replaced = false;
    for line in head.split_inclusive("\r\n") {
        let folded = line.starts_with(' ') || line.starts_with('\t');
        if folded && skipping {
            continue;
        }
        skipping = false;
        if !folded
            && line.len() >= prefix.len()
            && line.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
        {
            skipping = true;
            if !replaced {
                out += &format!("{name}: {value}\r\n");
                replaced = true;
            }
            continue;
        }
        out += line;
    }
    if !replaced {
        out = format!("{name}: {value}\r\n{out}");
    }
    let mut bytes = out.into_bytes();
    bytes.extend_from_slice(body);
    bytes
}

/// 服务器明确拒收（5xx：收件人不存在、内容被拒等），重试也没用。
#[derive(Debug)]
pub struct Rejected(pub String);

impl std::fmt::Display for Rejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

pub fn rejected(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<Rejected>())
}

/// 直接发出一封已经组好的原文（已读回执、定时发送）。
pub async fn send_raw(acct: &Account, pass: &str, to: &[String], raw: &[u8]) -> Result<()> {
    let from: Address = acct
        .email
        .parse()
        .map_err(|_| Rejected("发件地址格式不对".into()))?;
    let to: Vec<Address> = to
        .iter()
        .map(|a| {
            a.parse()
                .map_err(|_| anyhow::Error::from(Rejected(format!("收件地址格式不对：{a}"))))
        })
        .collect::<Result<_>>()?;
    let envelope = Envelope::new(Some(from), to)?;
    transport(acct, pass)?
        .send_raw(&envelope, raw)
        .await
        .map_err(send_error)?;
    Ok(())
}

#[derive(Debug)]
struct NotAccepted(String);
impl std::fmt::Display for NotAccepted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for NotAccepted {}

/// 仅明确的 SMTP 拒绝可以作为未提交处理；传输断线保守待确认。
pub fn not_accepted(e: &anyhow::Error) -> bool {
    rejected(e) || e.chain().any(|c| c.is::<NotAccepted>())
}

fn send_error(e: lettre::transport::smtp::Error) -> anyhow::Error {
    if e.is_permanent() && e.to_string().contains("auth") {
        Rejected(format!("发信登录失败，请确认授权码正确并已开启 SMTP：{e}")).into()
    } else if e.is_permanent() {
        Rejected(format!("发送失败：{e}")).into()
    } else if e.is_transient() {
        NotAccepted(format!("服务器暂未接受邮件：{e}")).into()
    } else {
        anyhow!("发送结果尚未确认：{e}")
    }
}

pub async fn send(acct: &Account, pass: &str, msg: Message) -> Result<()> {
    transport(acct, pass)?.send(msg).await.map_err(send_error)?;
    Ok(())
}

/// 仅连通性、TLS、认证和 NOOP；不会提交邮件。
pub async fn test_connection(acct: &Account, pass: &str) -> Result<()> {
    let ok = transport(acct, pass)?
        .test_connection()
        .await
        .map_err(send_error)?;
    anyhow::ensure!(ok, "SMTP 连接检测未得到成功响应");
    Ok(())
}

fn transport(acct: &Account, pass: &str) -> Result<AsyncSmtpTransport<Tokio1Executor>> {
    // 25/587/2525 是 STARTTLS 的惯用端口，其余（465 等）一律直接 TLS
    let implicit_tls = !matches!(acct.smtp_port, 25 | 587 | 2525);
    let mut builder = if implicit_tls {
        AsyncSmtpTransport::<Tokio1Executor>::relay(&acct.smtp_host)?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&acct.smtp_host)?
    };
    if let Some(pem) = crate::imap::extra_ca_pem() {
        let params = TlsParameters::builder(acct.smtp_host.clone())
            .add_root_certificate(Certificate::from_pem(&pem)?)
            .build_rustls()?;
        builder = builder.tls(if implicit_tls {
            Tls::Wrapper(params)
        } else {
            Tls::Required(params)
        });
    }
    Ok(builder
        .port(acct.smtp_port)
        .credentials(Credentials::new(acct.email.clone(), pass.to_string()))
        .timeout(Some(Duration::from_secs(30)))
        .build())
}

#[cfg(test)]
pub(crate) const RICH_REPLY_SOURCE: &str = concat!(
    "From: sender@example.com\r\nTo: bob@localhost\r\nSubject: Original report\r\n",
    "Message-ID: <original-rich@example.com>\r\nMIME-Version: 1.0\r\n",
    "Content-Type: multipart/mixed; boundary=outer\r\n\r\n",
    "--outer\r\nContent-Type: multipart/related; boundary=related\r\n\r\n",
    "--related\r\nContent-Type: text/html; charset=utf-8\r\n\r\n",
    "<table style=\"border:1px solid red\"><tr><td><strong>原文格式</strong></td>",
    "<td><img src=\"cid:logo@example.com\"></td></tr></table>",
    "<a href=\"https://example.com/report\">查看报告</a>\r\n",
    "--related\r\nContent-Type: image/png\r\nContent-ID: <logo@example.com>\r\n",
    "Content-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\n",
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aB9sAAAAASUVORK5CYII=\r\n",
    "--related\r\nContent-Type: image/png\r\nContent-ID: <unused@example.com>\r\n",
    "Content-Disposition: inline\r\nContent-Transfer-Encoding: base64\r\n\r\n",
    "dW51c2Vk\r\n--related--\r\n",
    "--outer\r\nContent-Type: application/pdf; name=private.pdf\r\n",
    "Content-ID: <private@example.com>\r\nContent-Disposition: attachment; filename=private.pdf\r\n",
    "Content-Transfer-Encoding: base64\r\n\r\nJVBERi0xLjQK\r\n--outer--\r\n",
);

#[cfg(test)]
mod tests {
    use super::*;

    fn acct() -> Account {
        Account {
            receive_epoch: 1,
            connection_revision: 1,
            previous_archive_dirs: Vec::new(),
            id: "a".into(),
            kind: "qq".into(),
            email: "me@qq.com".into(),
            name: "我".into(),
            signature: String::new(),
            auto_download: Default::default(),
            body_cache: Default::default(),
            archive_dirs: Vec::new(),
            receive_protocol: crate::config::ReceiveProtocol::Imap,
            pop3_host: String::new(),
            pop3_port: 995,
            pop3_tls: true,
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 465,
        }
    }

    #[test]
    fn builds_reply() {
        let d = Draft {
            account_id: "a".into(),
            to: "张三 <z@example.com>， b@example.com".into(),
            cc: String::new(),
            bcc: String::new(),
            subject: "Re: 你好".into(),
            body: "收到\n谢谢".into(),
            html_body: None,
            quote_source: None,
            quote_cids: vec![],
            in_reply_to: Some("orig@example.com".into()),
            references: vec!["root@example.com".into()],
            attachments: vec![],
            request_receipt: false,
        };
        let mut account = acct();
        account.signature = "不应由 SMTP 自动重复追加的签名".into();
        let (m, id) = build(&account, &d).unwrap();
        let raw = String::from_utf8(m.formatted()).unwrap();
        assert_eq!(
            crate::imap::parse_message(raw.as_bytes(), 0)
                .unwrap()
                .text
                .trim()
                .replace("\r\n", "\n"),
            d.body
        );
        assert!(raw.contains("In-Reply-To: <orig@example.com>"));
        assert!(raw.contains("References: <root@example.com> <orig@example.com>"));
        assert!(raw.contains(&format!("Message-ID: {id}")));
        assert!(id.ends_with("@qq.com>"));
    }

    fn receipt_draft() -> Draft {
        Draft {
            account_id: "a".into(),
            to: "b@example.com".into(),
            cc: String::new(),
            bcc: String::new(),
            subject: "报价".into(),
            body: "您好".into(),
            html_body: None,
            quote_source: None,
            quote_cids: vec![],
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
            request_receipt: false,
        }
    }

    #[test]
    fn a_read_receipt_request_asks_receipts_to_go_back_to_the_sender() {
        let raw = |d: &Draft| String::from_utf8(build(&acct(), d).unwrap().0.formatted()).unwrap();
        assert!(!raw(&receipt_draft()).contains("Disposition-Notification-To"));
        let mut d = receipt_draft();
        d.request_receipt = true;
        let raw = raw(&d);
        let line = raw
            .lines()
            .find(|l| l.starts_with("Disposition-Notification-To:"))
            .expect("要有回执请求头");
        assert!(line.contains("<me@qq.com>"), "{line}");
        let view = crate::imap::parse_message(raw.as_bytes(), 0).unwrap();
        assert_eq!(view.receipt_to.as_deref(), Some("me@qq.com"));
    }

    #[test]
    fn a_receipt_is_a_disposition_notification_report() {
        let original = crate::imap::parse_message(
            concat!(
                "From: 王总 <boss@x.com>\r\nTo: me@qq.com\r\nSubject: 报价\r\n",
                "Message-ID: <orig@x.com>\r\nDate: Mon, 19 Oct 2026 08:00:00 +0000\r\n",
                "Disposition-Notification-To: 王总 <boss@x.com>\r\n\r\nhi\r\n"
            )
            .as_bytes(),
            0,
        )
        .unwrap();
        assert_eq!(original.receipt_to.as_deref(), Some("boss@x.com"));
        let (to, raw) = receipt(&acct(), &original).unwrap();
        assert_eq!(to, "boss@x.com");
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.subject(), Some("已读：报价"));
        let ct = parsed.content_type().unwrap();
        assert_eq!(ct.ctype(), "multipart");
        assert_eq!(ct.subtype(), Some("report"));
        assert_eq!(
            ct.attribute("report-type"),
            Some("disposition-notification")
        );
        assert!(parsed.body_text(0).unwrap().contains("报价"));
        let report = parsed
            .parts
            .iter()
            .find(|p| {
                p.content_type()
                    .is_some_and(|c| c.subtype() == Some("disposition-notification"))
            })
            .expect("要有机器可读的回执部分");
        let text = String::from_utf8_lossy(report.contents());
        assert!(text.contains("Original-Message-ID: <orig@x.com>"), "{text}");
        assert!(
            text.contains("Final-Recipient: rfc822; me@qq.com"),
            "{text}"
        );
        assert!(
            text.contains("Disposition: manual-action/MDN-sent-manually; displayed"),
            "{text}"
        );
        assert_eq!(parsed.in_reply_to().as_text(), Some("orig@x.com"));
    }

    #[test]
    fn the_date_header_is_set_to_the_actual_send_time() {
        let raw = b"From: a@x.com\r\nDate: Mon, 19 Oct 2026 22:00:00\r\n +0800\r\nSubject: x\r\n\r\nDate: in the body stays\r\n";
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-20T08:00:00+08:00").unwrap();
        let out = String::from_utf8(with_date(raw, &at)).unwrap();
        assert!(
            out.contains("Date: Tue, 20 Oct 2026 08:00:00 +0800\r\nSubject: x"),
            "{out}"
        );
        assert!(!out.contains("22:00"), "折行的旧值也去掉：{out}");
        assert!(
            out.ends_with("\r\n\r\nDate: in the body stays\r\n"),
            "正文不动"
        );
    }

    #[test]
    fn receipts_keep_mime_lines_short_and_quote_names() {
        let mut me = acct();
        me.name = "Li, Jarvis".into();
        let long = "季度报价".repeat(60);
        let original = crate::imap::parse_message(
            format!("From: boss@x.com\r\nSubject: {long}\r\nMessage-ID: <a\r\n b@x.com>\r\nDisposition-Notification-To: boss@x.com\r\n\r\nhi\r\n").as_bytes(),
            0,
        )
        .unwrap();
        let (_, raw) = receipt(&me, &original).unwrap();
        let text = String::from_utf8(raw.clone()).unwrap();
        assert!(
            text.split("\r\n").all(|l| l.len() <= 78),
            "MIME 每行不超过 78 个字符"
        );
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.subject().unwrap(), format!("已读：{long}"));
        let from = parsed.from().unwrap().first().unwrap();
        assert_eq!(from.address(), Some("me@qq.com"));
        assert_eq!(from.name(), Some("Li, Jarvis"));
        assert!(parsed.body_text(0).unwrap().contains("季度报价"));
        assert!(!text.contains("a\r\n b@x.com"), "ID 里的空白去掉");
    }

    #[test]
    fn permanent_smtp_rejections_are_told_apart_from_connection_problems() {
        let e: anyhow::Error = Rejected("550 mailbox unavailable".into()).into();
        assert!(rejected(&e));
        assert!(!rejected(&anyhow!("连接超时")));
    }

    #[test]
    fn rejects_bad_address() {
        let mut d = Draft {
            account_id: "a".into(),
            to: "not-an-address".into(),
            cc: String::new(),
            bcc: String::new(),
            subject: String::new(),
            body: String::new(),
            html_body: None,
            quote_source: None,
            quote_cids: vec![],
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
            request_receipt: false,
        };
        assert!(build(&acct(), &d).is_err());
        d.to = String::new();
        assert!(build(&acct(), &d).is_err());
    }

    #[test]
    fn rich_reply_preserves_html_and_plain_fallback() {
        let html = "<div>收到，谢谢</div><blockquote><table style=\"border:1px solid red\"><tr><td><strong>原文</strong></td></tr></table><a href=\"https://example.com\">链接</a></blockquote>";
        let d: Draft = serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": "reader@example.com", "subject": "Re: 周报",
            "body": "收到，谢谢\n\n> 原文\n> 链接", "htmlBody": html,
            "inReplyTo": "original@example.com", "references": []
        }))
        .unwrap();
        let (message, _) = build(&acct(), &d).unwrap();
        let raw = message.formatted();
        let parsed = mailbeacon_parser::MessageParser::default()
            .parse(&raw)
            .unwrap();
        assert!(parsed.html_part(0).is_some(), "回复必须包含真实 HTML 正文");
        assert_eq!(parsed.body_html(0).unwrap().trim(), html);
        assert_eq!(
            parsed.body_text(0).unwrap().trim().replace("\r\n", "\n"),
            d.body
        );
        assert_eq!(parsed.in_reply_to().as_text(), Some("original@example.com"));
        assert!(String::from_utf8_lossy(&raw).contains("multipart/alternative"));
    }

    fn inline_reply() -> Draft {
        let source = crate::imap::parse_message(RICH_REPLY_SOURCE.as_bytes(), 0).unwrap();
        serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": "reader@example.com", "subject": "Re: Original report",
            "body": "收到，谢谢\n\n> 原文格式", "htmlBody": format!("<div>收到，谢谢</div><blockquote>{}</blockquote>", source.reply_html.unwrap()),
            "quoteSource": {"accountId":"original-account", "folder":"INBOX", "uid":7},
            "quoteCids": ["logo@example.com", "logo@example.com"],
            "inReplyTo": "original-rich@example.com", "references": []
        })).unwrap()
    }

    #[test]
    fn rich_reply_embeds_only_referenced_images_with_original_bytes() {
        let d = inline_reply();
        let (message, _) =
            build_with_quote(&acct(), &d, Some(RICH_REPLY_SOURCE.as_bytes())).unwrap();
        let raw = message.formatted();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let source = MessageParser::default()
            .parse(RICH_REPLY_SOURCE.as_bytes())
            .unwrap();
        assert_eq!(
            parsed.body_html(0).unwrap().trim(),
            d.html_body.as_ref().unwrap().trim()
        );
        assert_eq!(
            parsed.attachments().count(),
            1,
            "普通附件、无引用图片、重复 CID 均不多带"
        );
        let image = parsed.attachments().next().unwrap();
        assert_eq!(image.content_id(), Some("logo@example.com"));
        assert_eq!(
            image.contents(),
            source
                .attachments()
                .find(|p| p.content_id() == Some("logo@example.com"))
                .unwrap()
                .contents()
        );
        let serialized = String::from_utf8_lossy(&raw);
        assert!(serialized.contains("multipart/alternative"));
        assert!(serialized.contains("multipart/related"));
        assert!(!serialized.contains("data:image"));
        assert!(!serialized.contains("private.pdf"));
    }

    fn plain_draft(to: &str) -> Draft {
        serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": to, "bcc": "secret@example.com",
            "subject": "报价单", "body": "请查收附件", "inReplyTo": null
        }))
        .unwrap()
    }

    fn file(name: &str, data: &[u8]) -> OutgoingAttachment {
        OutgoingAttachment {
            name: name.into(),
            content_type: content_type_for(name).into(),
            data: data.to_vec(),
        }
    }

    #[test]
    fn attachments_round_trip_with_unicode_names_and_exact_bytes() {
        let d = plain_draft("reader@example.com");
        let files = vec![
            file("2026年报价单.pdf", b"%PDF-1.4 binary\x00\xff"),
            file("photo.JPG", &[0xff, 0xd8, 0xff, 0x00]),
        ];
        let (message, _) = compose(&acct(), &d, None, files, Purpose::Send).unwrap();
        let raw = message.formatted();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(parsed.body_text(0).unwrap().trim(), "请查收附件");
        let names: Vec<_> = parsed
            .attachments()
            .map(|p| p.attachment_name().unwrap().to_string())
            .collect();
        assert_eq!(names, ["2026年报价单.pdf", "photo.JPG"]);
        let first = parsed.attachments().next().unwrap();
        assert_eq!(first.contents(), b"%PDF-1.4 binary\x00\xff");
        assert_eq!(first.content_type().unwrap().subtype(), Some("pdf"));
        assert!(String::from_utf8_lossy(&raw).contains("multipart/mixed"));
        assert!(
            !String::from_utf8_lossy(&raw).contains("secret@example.com"),
            "发出的邮件不能泄露密送"
        );
    }

    #[test]
    fn attachments_combine_with_rich_reply_and_inline_images() {
        let d = inline_reply();
        let files = vec![file("清单.xlsx", b"PK\x03\x04")];
        let (message, _) = compose(
            &acct(),
            &d,
            Some(RICH_REPLY_SOURCE.as_bytes()),
            files,
            Purpose::Send,
        )
        .unwrap();
        let raw = message.formatted();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed.body_html(0).unwrap().trim(),
            d.html_body.as_ref().unwrap().trim()
        );
        let parts: Vec<_> = parsed
            .attachments()
            .map(|p| {
                (
                    p.content_id().map(str::to_string),
                    p.attachment_name().map(str::to_string),
                )
            })
            .collect();
        assert!(
            parts.contains(&(Some("logo@example.com".into()), None))
                || parts
                    .iter()
                    .any(|(cid, _)| cid.as_deref() == Some("logo@example.com"))
        );
        assert!(
            parts
                .iter()
                .any(|(_, name)| name.as_deref() == Some("清单.xlsx"))
        );
    }

    #[test]
    fn attachment_total_size_is_limited() {
        let d = plain_draft("reader@example.com");
        let big = vec![
            file("a.bin", &vec![0u8; MAX_ATTACHMENT_BYTES / 2 + 1]),
            file("b.bin", &vec![0u8; MAX_ATTACHMENT_BYTES / 2 + 1]),
        ];
        let err = compose(&acct(), &d, None, big, Purpose::Send).unwrap_err();
        assert!(format!("{err:#}").contains("附件"));
    }

    #[test]
    fn drafts_keep_bcc_and_allow_missing_recipients() {
        let mut empty = plain_draft("");
        empty.bcc.clear();
        assert!(compose(&acct(), &empty, None, vec![], Purpose::Send).is_err());
        assert!(compose(&acct(), &empty, None, vec![], Purpose::Draft).is_ok());
        let d = plain_draft("");
        let (message, _) = compose(
            &acct(),
            &d,
            None,
            vec![file("a.txt", b"hi")],
            Purpose::Draft,
        )
        .unwrap();
        let raw = message.formatted();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        assert_eq!(
            parsed.bcc().unwrap().first().unwrap().address(),
            Some("secret@example.com")
        );
        assert_eq!(parsed.attachments().count(), 1);
        assert_eq!(parsed.subject(), Some("报价单"));
    }

    #[test]
    fn images_inserted_in_the_editor_are_sent_as_inline_parts() {
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aB9sAAAAASUVORK5CYII=";
        let d: Draft = serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": "r@example.com", "subject": "图", "body": "看图",
            "htmlBody": format!("<p>看图</p><img src=\"data:image/png;base64,{png}\">"), "inReplyTo": null
        }))
        .unwrap();
        let (message, _) =
            compose(&acct(), &d, None, vec![file("a.txt", b"x")], Purpose::Send).unwrap();
        let raw = message.formatted();
        let parsed = MessageParser::default().parse(&raw).unwrap();
        let html = parsed.body_html(0).unwrap();
        assert!(!html.contains("data:image"));
        let inline = parsed
            .attachments()
            .find(|p| p.content_id().is_some())
            .expect("内嵌图片");
        assert!(html.contains(&format!("cid:{}", inline.content_id().unwrap())));
        assert!(inline.contents().starts_with(b"\x89PNG"));
        assert!(
            parsed
                .attachments()
                .any(|p| p.attachment_name() == Some("a.txt"))
        );
    }

    #[test]
    fn recipient_lists_respect_quoted_names_and_report_addresses() {
        let d: Draft = serde_json::from_value(serde_json::json!({
            "accountId": "a", "to": "\"张三, 研发\" <z@example.com>， b@example.com",
            "cc": "李四 <l@example.com>；", "bcc": "s@example.com", "subject": "s", "body": "b", "inReplyTo": null
        }))
        .unwrap();
        let (message, _) = compose(&acct(), &d, None, vec![], Purpose::Send).unwrap();
        let raw = String::from_utf8(message.formatted()).unwrap();
        assert!(raw.contains("z@example.com") && raw.contains("b@example.com"));
        assert_eq!(
            recipients(&d).unwrap(),
            vec![
                ("张三, 研发".to_string(), "z@example.com".to_string()),
                (String::new(), "b@example.com".to_string()),
                ("李四".to_string(), "l@example.com".to_string()),
                (String::new(), "s@example.com".to_string()),
            ]
        );
    }

    #[test]
    fn guesses_common_content_types() {
        assert_eq!(content_type_for("a.PDF"), "application/pdf");
        assert_eq!(content_type_for("x.jpeg"), "image/jpeg");
        assert_eq!(
            content_type_for("表格.xlsx"),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
        );
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }

    #[test]
    fn rich_reply_rejects_missing_images_and_non_image_attachments() {
        let mut d = inline_reply();
        assert!(build(&acct(), &d).is_err(), "读取原文失败不得静默丢掉图片");
        d.quote_cids = vec!["missing@example.com".into()];
        assert!(build_with_quote(&acct(), &d, Some(RICH_REPLY_SOURCE.as_bytes())).is_err());
        d.quote_cids = vec!["private@example.com".into()];
        assert!(build_with_quote(&acct(), &d, Some(RICH_REPLY_SOURCE.as_bytes())).is_err());
        d.quote_cids = vec!["bad\r\nContent-ID: injected".into()];
        assert!(build_with_quote(&acct(), &d, Some(RICH_REPLY_SOURCE.as_bytes())).is_err());
    }
}
