//! 端到端测试：连一台真实的 IMAP/SMTP 服务器跑完整流程。默认跳过，需要时：
//!
//! ```sh
//! MAILPULSE_E2E=localhost:3993:3465 MAILPULSE_EXTRA_CA=ca.pem \
//!   cargo test -p mailpulse-app e2e -- --ignored --nocapture
//! ```
//! 服务器上要有 me@localhost / bob@localhost 两个账号，密码都是 pw（GreenMail 即可）。

use std::time::{Duration, Instant};

use async_imap::extensions::idle::IdleResponse;
use tokio::time::timeout;

use crate::{
    config::Account,
    imap::{self, OP_TIMEOUT},
    smtp,
};

fn account(user: &str) -> Account {
    let spec = std::env::var("MAILPULSE_E2E").expect("需要 MAILPULSE_E2E=host:imap端口:smtp端口");
    let mut it = spec.split(':');
    let host = it.next().unwrap().to_string();
    Account {
        id: user.into(),
        kind: "custom".into(),
        email: format!("{user}@localhost"),
        name: format!("{user} 测试"),
        imap_host: host.clone(),
        imap_port: it.next().unwrap().parse().unwrap(),
        smtp_host: host,
        smtp_port: it.next().unwrap().parse().unwrap(),
    }
}

const HTML_WITH_ATTACHMENT: &str = "From: =?UTF-8?B?5byg5LiJ?= <zhang@example.com>\r\n\
To: me@localhost\r\n\
Subject: =?UTF-8?B?5ZGo5oql77ya56ys5LiJ5a2j5bqm?=\r\n\
Message-ID: <weekly-1@example.com>\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/mixed; boundary=\"outer\"\r\n\
\r\n\
--outer\r\n\
Content-Type: text/html; charset=utf-8\r\n\
\r\n\
<html><head><style>p{color:red}</style></head><body><p>\u{4f60}\u{597d}\u{ff0c}\u{8fd9}\u{662f}\u{5468}\u{62a5}\u{6b63}\u{6587}</p><img src=\"https://tracker.example.com/p.gif\"></body></html>\r\n\
--outer\r\n\
Content-Type: application/pdf; name=\"report.pdf\"\r\n\
Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
JVBERi0xLjQKJcfsj6IK\r\n\
--outer--\r\n";

/// 用单独的连接取摘要：解析失败会弄坏连接。
/// GreenMail 的部分 FETCH 回包写成 `BODY[]<0>{n}`（少了空格，不合 RFC 3501），解析必然失败，跳过。
async fn preview(acct: &Account, uid: u32) -> Option<String> {
    let mut s = imap::login(acct, "pw").await.unwrap();
    s.select("INBOX").await.unwrap();
    match imap::fetch_previews(&mut s, &uid.to_string()).await {
        Ok(v) => Some(v.into_iter().next().expect("应有摘要").text),
        Err(e) if format!("{e:#}").contains("BODY[]<0>{") => {
            println!("（服务器部分 FETCH 回包不合规范，跳过摘要检查）");
            None
        }
        Err(e) => panic!("取摘要失败：{e:#}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_full_flow() {
    let me = account("me");
    let bob = account("bob");

    // ---- 登录 + 文件夹 ----
    let mut s = imap::login(&me, "pw").await.expect("登录");
    for f in ["Sent", "Trash", "Archive"] {
        let _ = s.create(f).await;
    }
    let folders = imap::list_folders(&mut s).await.unwrap();
    let role = |r: &str| folders.iter().find(|f| f.role == r).map(|f| f.path.clone());
    println!(
        "文件夹：{:?}",
        folders
            .iter()
            .map(|f| (&f.path, &f.name, &f.role))
            .collect::<Vec<_>>()
    );
    assert_eq!(role("inbox").as_deref(), Some("INBOX"));
    assert_eq!(role("sent").as_deref(), Some("Sent"));
    assert_eq!(role("trash").as_deref(), Some("Trash"));

    // 密码错误要识别成 AuthFailed（同步任务据此停止重试）
    let bad = imap::login(&me, "wrong").await.expect_err("错误密码应失败");
    assert!(bad.is::<imap::AuthFailed>(), "{bad:#}");

    // ---- IDLE 推送延迟：bob 发信给 me，看 me 的 IDLE 多久被唤醒 ----
    assert!(
        imap::has_capability(&mut s, "IDLE").await.unwrap(),
        "服务器应支持 IDLE"
    );
    s.select("INBOX").await.unwrap();
    let before = imap::page_uids(&mut s, None).await.unwrap();
    let last = before.last().copied().unwrap_or(0);

    let mut idle = s.idle();
    idle.init().await.unwrap();
    let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(30));

    let draft = smtp::Draft {
        account_id: "bob".into(),
        to: "我 <me@localhost>".into(),
        cc: String::new(),
        bcc: String::new(),
        subject: "推送测试：你好 👋".into(),
        body: "第一行\n第二行 https://example.com/x".into(),
        in_reply_to: None,
        references: vec![],
    };
    let (msg, mid) = smtp::build(&bob, &draft).unwrap();
    let raw_sent = msg.formatted();
    let t0 = Instant::now();
    smtp::send(&bob, "pw", msg).await.expect("SMTP 发送");
    let sent_at = t0.elapsed();
    let woke = wait.await.unwrap();
    let push = t0.elapsed();
    println!("SMTP 发送耗时 {sent_at:?}，IDLE 在发信开始后 {push:?} 被唤醒（{woke:?}）");
    assert!(
        matches!(woke, IdleResponse::NewData(_)),
        "应由新邮件唤醒，而不是超时"
    );
    assert!(push < Duration::from_secs(5));
    let mut s = idle.done().await.unwrap();

    // ---- 新邮件摘要 ----
    let fresh: Vec<_> = imap::fetch_summaries(&mut s, &format!("{}:*", last + 1), "me", "INBOX")
        .await
        .unwrap()
        .into_iter()
        .filter(|m| m.uid > last)
        .collect();
    assert_eq!(fresh.len(), 1);
    let m = &fresh[0];
    assert_eq!(m.subject, "推送测试：你好 👋");
    assert_eq!(m.from_name, "bob 测试");
    assert_eq!(m.from_addr, "bob@localhost");
    assert!(!m.seen);
    assert!(m.date > 0);
    if let Some(pv) = preview(&me, m.uid).await {
        assert_eq!(pv, "第一行 第二行 https://example.com/x");
    }
    assert!(imap::unseen_count(&mut s).await.unwrap() >= 1);

    // ---- HTML + 附件邮件（直接 APPEND 进收件箱）----
    s.append("INBOX", None, None, HTML_WITH_ATTACHMENT.as_bytes())
        .await
        .unwrap();
    s.noop().await.unwrap();
    let all = imap::page_uids(&mut s, None).await.unwrap();
    let html_uid = *all.last().unwrap();
    let sums = imap::fetch_summaries(&mut s, &html_uid.to_string(), "me", "INBOX")
        .await
        .unwrap();
    assert_eq!(sums[0].subject, "周报：第三季度");
    assert_eq!(sums[0].from_name, "张三");
    assert!(sums[0].attachment, "multipart/mixed 应显示附件图标");
    if let Some(pv) = preview(&me, html_uid).await {
        assert!(pv.contains("这是周报正文"), "HTML 摘要：{pv:?}");
        assert!(!pv.contains("color"), "摘要不应包含 CSS：{pv:?}");
    }

    // 打开邮件：BODY[] 会把它标为已读
    let f = s
        .uid_fetch(html_uid.to_string(), "(UID INTERNALDATE BODY[])")
        .await
        .unwrap();
    use futures::TryStreamExt;
    let f: Vec<_> = f.try_collect().await.unwrap();
    let raw = f[0].body().unwrap().to_vec();
    let v = imap::parse_message(&raw, 0).unwrap();
    assert!(v.html.as_deref().unwrap().contains("这是周报正文"));
    assert!(v.has_remote, "应识别出远程图片");
    assert_eq!(v.attachments.len(), 1);
    assert_eq!(v.attachments[0].name, "report.pdf");
    let (name, bytes) = imap::attachment_bytes(&raw, 0).unwrap();
    assert_eq!(name, "report.pdf");
    assert!(bytes.starts_with(b"%PDF-1.4"));
    let flags = imap::fetch_flags(&mut s, &html_uid.to_string())
        .await
        .unwrap();
    assert!(flags[0].seen, "打开后应为已读");

    // ---- 星标 / 未读 ----
    let _: Vec<_> = s
        .uid_store(m.uid.to_string(), "+FLAGS.SILENT (\\Flagged)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let flagged: Vec<u32> = s.uid_search("FLAGGED").await.unwrap().into_iter().collect();
    assert_eq!(flagged, vec![m.uid]);
    let _: Vec<_> = s
        .uid_store(html_uid.to_string(), "-FLAGS.SILENT (\\Seen)")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    assert!(
        !imap::fetch_flags(&mut s, &html_uid.to_string())
            .await
            .unwrap()[0]
            .seen
    );

    // ---- 移到废纸篓，再从废纸篓彻底删除 ----
    imap::move_uids(&mut s, &html_uid.to_string(), Some("Trash"))
        .await
        .unwrap();
    assert!(
        !imap::page_uids(&mut s, None)
            .await
            .unwrap()
            .contains(&html_uid),
        "应已离开收件箱"
    );
    s.select("Trash").await.unwrap();
    let trash = imap::page_uids(&mut s, None).await.unwrap();
    assert_eq!(trash.len(), 1);
    imap::move_uids(&mut s, &trash[0].to_string(), None)
        .await
        .unwrap();
    assert!(
        imap::page_uids(&mut s, None).await.unwrap().is_empty(),
        "废纸篓里应已彻底删除"
    );

    // ---- 发信后补存到「已发送」，且不会重复保存 ----
    s.select("Sent").await.unwrap();
    assert!(
        imap::ensure_saved(&mut s, "Sent", &mid, &raw_sent)
            .await
            .unwrap(),
        "第一次应补存"
    );
    s.noop().await.unwrap();
    assert!(
        !imap::ensure_saved(&mut s, "Sent", &mid, &raw_sent)
            .await
            .unwrap(),
        "第二次应发现已存在"
    );
    let sent = imap::page_uids(&mut s, None).await.unwrap();
    let sums = imap::fetch_summaries(&mut s, &imap::uid_set(&sent), "me", "Sent")
        .await
        .unwrap();
    assert_eq!(sums.len(), 1);
    assert!(sums[0].seen);
    assert_eq!(sums[0].to, "我");

    // ---- 回复：In-Reply-To/References 正确，对方收到 ----
    let reply = smtp::Draft {
        account_id: "me".into(),
        to: "bob@localhost".into(),
        cc: String::new(),
        bcc: String::new(),
        subject: "Re: 推送测试".into(),
        body: "收到".into(),
        in_reply_to: Some(mid.clone()),
        references: vec![],
    };
    let (msg, _) = smtp::build(&me, &reply).unwrap();
    smtp::send(&me, "pw", msg).await.unwrap();
    let mut b = imap::login(&bob, "pw").await.unwrap();
    b.select("INBOX").await.unwrap();
    let mut got = Vec::new();
    for _ in 0..20 {
        got = imap::page_uids(&mut b, None).await.unwrap();
        if !got.is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        b.noop().await.unwrap();
    }
    let f: Vec<_> = b
        .uid_fetch(got.last().unwrap().to_string(), "BODY.PEEK[]")
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let v = imap::parse_message(f[0].body().unwrap(), 0).unwrap();
    assert_eq!(v.subject, "Re: 推送测试");
    assert_eq!(
        v.references,
        vec![mid.trim_matches(|c| c == '<' || c == '>').to_string()]
    );
    assert_eq!(v.text.trim(), "收到");

    let _ = timeout(OP_TIMEOUT, s.logout()).await;
    let _ = timeout(OP_TIMEOUT, b.logout()).await;
}

/// 在符合 RFC 的服务器（如 Dovecot）上验证摘要、文件夹识别和 IDLE。
/// MAILPULSE_E2E_IMAP=host:端口，账号 me@localhost / pass。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_imap_previews() {
    let spec = std::env::var("MAILPULSE_E2E_IMAP").expect("需要 MAILPULSE_E2E_IMAP=host:port");
    let (host, port) = spec.split_once(':').unwrap();
    let acct = Account {
        id: "dc".into(),
        kind: "custom".into(),
        email: "me@localhost".into(),
        name: "me".into(),
        imap_host: host.into(),
        imap_port: port.parse().unwrap(),
        smtp_host: String::new(),
        smtp_port: 465,
    };
    let mut s = imap::login(&acct, "pass").await.unwrap();
    // 中文文件夹名走修改版 UTF-7
    for f in ["Sent", "Trash", "&UXZO1mWHTvZZOQ-"] {
        let _ = s.create(f).await;
    }
    let folders = imap::list_folders(&mut s).await.unwrap();
    println!(
        "文件夹：{:?}",
        folders
            .iter()
            .map(|f| (&f.path, &f.name, &f.role))
            .collect::<Vec<_>>()
    );
    assert!(
        folders
            .iter()
            .any(|f| f.name == "其他文件夹" && f.role == "other")
    );
    assert!(folders.iter().any(|f| f.path == "Sent" && f.role == "sent"));

    // 另一条连接在 IDLE，这边 APPEND，看推送
    let mut watcher = imap::login(&acct, "pass").await.unwrap();
    watcher.select("INBOX").await.unwrap();
    let mut idle = watcher.idle();
    idle.init().await.unwrap();
    let (wait, _stop) = idle.wait_with_timeout(Duration::from_secs(20));

    let plain = "From: =?UTF-8?B?5byg5LiJ?= <z@example.com>\r\nSubject: plain\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n\u{4f60}\u{597d}\u{ff0c}\r\n\u{8fd9}\u{662f}\u{6b63}\u{6587}\r\n";
    let t0 = Instant::now();
    s.append("INBOX", None, None, plain.as_bytes())
        .await
        .unwrap();
    s.append("INBOX", None, None, HTML_WITH_ATTACHMENT.as_bytes())
        .await
        .unwrap();
    let woke = wait.await.unwrap();
    println!("APPEND 后 {:?} 收到推送：{woke:?}", t0.elapsed());
    assert!(matches!(woke, IdleResponse::NewData(_)));
    let _ = idle.done().await.unwrap();

    s.select("INBOX").await.unwrap();
    let uids = imap::page_uids(&mut s, None).await.unwrap();
    let last2 = &uids[uids.len() - 2..];
    let pv = imap::fetch_previews(&mut s, &imap::uid_set(last2))
        .await
        .expect("Dovecot 上摘要应能正常解析");
    println!(
        "摘要：{:?}",
        pv.iter().map(|p| (&p.uid, &p.text)).collect::<Vec<_>>()
    );
    let get = |u: u32| {
        pv.iter()
            .find(|p| p.uid == u)
            .map(|p| p.text.clone())
            .unwrap()
    };
    assert_eq!(get(last2[0]), "你好， 这是正文");
    assert!(get(last2[1]).contains("这是周报正文"));
    assert!(!get(last2[1]).contains("color"));
    let sums = imap::fetch_summaries(&mut s, &imap::uid_set(last2), "dc", "INBOX")
        .await
        .unwrap();
    assert_eq!(sums.len(), 2);
    assert!(sums.iter().all(|m| !m.seen), "取摘要不应把邮件标为已读");
}
