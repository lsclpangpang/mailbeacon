//! 发信：SMTP 发出后，确认「已发送」里有这封邮件，没有就补存一份。
//! QQ/163 等是否自动保存取决于网页版设置，Gmail 一定会自动保存，所以按实际结果判断，避免重复。

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Attachment, Mailbox, MultiPart, SinglePart, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Certificate, Tls, TlsParameters},
    },
};
#[cfg(test)]
use mail_parser::MessageParser;
use mail_parser::MimeHeaders;
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
fn parse_list(s: &str) -> Result<Vec<Mailbox>> {
    s.split([',', ';', '，', '；', '\n'])
        .map(str::trim)
        .filter(|x| !x.is_empty())
        .map(|x| {
            x.parse::<Mailbox>()
                .map_err(|_| anyhow!("邮箱地址格式不对：{x}"))
        })
        .collect()
}

fn angle(id: &str) -> String {
    let id = id.trim().trim_matches(|c| c == '<' || c == '>');
    format!("<{id}>")
}

/// 构造邮件，返回（邮件，Message-ID）
pub fn build(acct: &Account, d: &Draft) -> Result<(Message, String)> {
    build_with_quote(acct, d, None)
}

/// 保留 HTML 正文，并从原邮件复制引用中的内嵌图片；普通附件不会跟随回复发送。
pub fn build_with_quote(
    acct: &Account,
    d: &Draft,
    quote_raw: Option<&[u8]>,
) -> Result<(Message, String)> {
    let to = parse_list(&d.to)?;
    let cc = parse_list(&d.cc)?;
    let bcc = parse_list(&d.bcc)?;
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        bail!("请填写收件人");
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
    for m in to {
        b = b.to(m);
    }
    for m in cc {
        b = b.cc(m);
    }
    for m in bcc {
        b = b.bcc(m);
    }
    if let Some(irt) = d.in_reply_to.as_deref().filter(|s| !s.trim().is_empty()) {
        b = b.in_reply_to(angle(irt));
        let mut refs: Vec<String> = d.references.iter().map(|r| angle(r)).collect();
        refs.push(angle(irt));
        refs.dedup();
        b = b.references(refs.join(" "));
    }
    let plain = d.body.replace("\r\n", "\n").replace('\n', "\r\n");
    let msg = if let Some(html) = d.html_body.as_ref() {
        let mut alternative = MultiPart::alternative().singlepart(SinglePart::plain(plain));
        let images = quoted_images(&d.quote_cids, quote_raw)?;
        if images.is_empty() {
            alternative = alternative.singlepart(SinglePart::html(html.clone()));
        } else {
            let mut related = MultiPart::related().singlepart(SinglePart::html(html.clone()));
            for image in images {
                related = related.singlepart(image);
            }
            alternative = alternative.multipart(related);
        }
        b.multipart(alternative)?
    } else {
        if !d.quote_cids.is_empty() {
            bail!("缺少带格式的引用正文，无法保留内嵌图片");
        }
        b.header(ContentType::TEXT_PLAIN).body(plain)?
    };
    Ok((msg, message_id))
}

fn quoted_images(cids: &[String], raw: Option<&[u8]>) -> Result<Vec<SinglePart>> {
    if cids.is_empty() {
        return Ok(Vec::new());
    }
    let raw = raw.ok_or_else(|| anyhow!("无法读取引用邮件的内嵌图片，请重新打开原邮件后回复"))?;
    let msg = crate::mime::parse(raw).context("无法解析引用邮件的内嵌图片")?;
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

pub async fn send(acct: &Account, pass: &str, msg: Message) -> Result<()> {
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
    let transport = builder
        .port(acct.smtp_port)
        .credentials(Credentials::new(acct.email.clone(), pass.to_string()))
        .timeout(Some(Duration::from_secs(30)))
        .build();
    transport.send(msg).await.map_err(|e| {
        if e.is_permanent() && e.to_string().contains("auth") {
            anyhow!("发信登录失败，请确认授权码正确并已开启 SMTP：{e}")
        } else {
            anyhow!("发送失败：{e}")
        }
    })?;
    Ok(())
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
            id: "a".into(),
            kind: "qq".into(),
            email: "me@qq.com".into(),
            name: "我".into(),
            signature: String::new(),
            auto_download: Default::default(),
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
        let parsed = mail_parser::MessageParser::default().parse(&raw).unwrap();
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
