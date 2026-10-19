//! 发信：SMTP 发出后，确认「已发送」里有这封邮件，没有就补存一份。
//! QQ/163 等是否自动保存取决于网页版设置，Gmail 一定会自动保存，所以按实际结果判断，避免重复。

use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use lettre::{
    AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::{
        authentication::Credentials,
        client::{Certificate, Tls, TlsParameters},
    },
};
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
    pub in_reply_to: Option<String>,
    #[serde(default)]
    pub references: Vec<String>,
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
    let msg = b
        .header(ContentType::TEXT_PLAIN)
        .body(d.body.replace('\n', "\r\n"))?;
    Ok((msg, message_id))
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
mod tests {
    use super::*;

    fn acct() -> Account {
        Account {
            id: "a".into(),
            kind: "qq".into(),
            email: "me@qq.com".into(),
            name: "我".into(),
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
            in_reply_to: Some("orig@example.com".into()),
            references: vec!["root@example.com".into()],
        };
        let (m, id) = build(&acct(), &d).unwrap();
        let raw = String::from_utf8(m.formatted()).unwrap();
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
            in_reply_to: None,
            references: vec![],
        };
        assert!(build(&acct(), &d).is_err());
        d.to = String::new();
        assert!(build(&acct(), &d).is_err());
    }
}
