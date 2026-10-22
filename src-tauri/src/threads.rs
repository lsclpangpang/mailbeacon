//! 会话：用 Message-ID / In-Reply-To / References 把来回的邮件串起来。
//! 列表里的分组在界面上做；这里负责到收件箱、已发送等文件夹里找同一会话的其他邮件。

use anyhow::{Context, Result};
use tokio::time::timeout;

use crate::{
    download_protocol::{ArchiveData, checked_command, connection_problem},
    imap::{self, OP_TIMEOUT, Summary},
};

/// 祖先邮件太多时只找根邮件和最近这么多封。
const MAX_ANCESTORS: usize = 9;
/// 每个文件夹最多取这么多封同一会话的邮件。
const MAX_PER_FOLDER: usize = 50;

/// 能安全放进 SEARCH 引号里的 ID（ASCII 可见字符，不含引号和反斜杠）。
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 250
        && id
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

/// 会话涉及的 ID：根邮件、祖先（根加最近几封）、它自己。
struct Ids<'a> {
    root: &'a str,
    ancestors: Vec<&'a str>,
    own: Option<&'a str>,
}

fn ids(m: &Summary) -> Option<Ids<'_>> {
    let mut chain: Vec<&str> = m.references.iter().map(String::as_str).collect();
    if !m.in_reply_to.is_empty() && !chain.contains(&m.in_reply_to.as_str()) {
        chain.push(&m.in_reply_to);
    }
    chain.retain(|id| safe_id(id) && *id != m.message_id);
    let own = safe_id(&m.message_id).then_some(m.message_id.as_str());
    let root = chain.first().copied().or(own)?;
    let mut ancestors: Vec<&str> = Vec::new();
    if let Some(&first) = chain.first() {
        ancestors.push(first);
        let start = chain.len().saturating_sub(MAX_ANCESTORS).max(1);
        ancestors.extend(&chain[start..]);
    }
    Some(Ids {
        root,
        ancestors,
        own,
    })
}

/// 找同一会话邮件的 SEARCH 条件：它的祖先、引用了根邮件或它本身的邮件、直接回复它的邮件。
/// 没有可用的 Message-ID 时返回 None。
pub fn search_criteria(m: &Summary) -> Option<String> {
    let Ids {
        root,
        ancestors,
        own,
    } = ids(m)?;
    let mut criteria: Vec<String> = ancestors
        .iter()
        .map(|id| format!("HEADER Message-ID \"{id}\""))
        .collect();
    criteria.push(format!("HEADER References \"{root}\""));
    if let Some(own) = own {
        if own != root {
            criteria.push(format!("HEADER References \"{own}\""));
        }
        criteria.push(format!("HEADER In-Reply-To \"{own}\""));
        criteria.push(format!("HEADER Message-ID \"{own}\""));
    }
    criteria
        .into_iter()
        .reduce(|acc, next| format!("OR {acc} {next}"))
}

/// HEADER 搜索按子串匹配（`1@host` 也会命中 `21@host`），拿回来再按完整 ID 核对一遍。
fn really_related(m: &Summary, ids: &Ids<'_>) -> bool {
    let known = |id: &str| id == ids.root || ids.ancestors.contains(&id) || ids.own == Some(id);
    known(&m.message_id) || known(&m.in_reply_to) || m.references.iter().any(|r| known(r))
}

pub struct Found {
    /// 同一会话的邮件，按时间从早到晚
    pub mails: Vec<Summary>,
    /// 每个文件夹的服务器都拒绝了这种搜索（不支持 HEADER/OR）：以后不用再试
    pub search_refused: bool,
}

/// Collect server-confirmed thread members and preserve their readable identities.
pub async fn collect_indexed<T>(
    s: &mut async_imap::Session<T>,
    account_id: &str,
    folders: &[String],
    m: &Summary,
    receive_epoch: u64,
    index: &crate::search_index::SearchIndex,
) -> Result<Found>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mut found = collect(s, account_id, folders, m).await?;
    let storage = crate::mail_ref::namespace(account_id, receive_epoch);
    let mut grouped = std::collections::BTreeMap::<(String, u32), Vec<Summary>>::new();
    for mail in &mut found.mails {
        mail.receive_epoch = receive_epoch;
        crate::mail_ref::MessageRef {
            account_id: mail.account_id.clone(),
            receive_epoch,
            folder: mail.folder.clone(),
            uid_validity: mail.uid_validity,
            uid: mail.uid,
        }
        .validate()?;
        grouped
            .entry((mail.folder.clone(), mail.uid_validity))
            .or_default()
            .push(mail.clone());
    }
    for ((folder, validity), mails) in grouped {
        // A thread is only a subset of the folder. Upsert creates a missing
        // snapshot, but never replaces an existing generation or coverage state.
        index.upsert_summaries(&storage, &folder, validity, &mails)?;
    }
    Ok(found)
}

/// 在这些文件夹里找和 `m` 同一会话的邮件。
/// 同一封出现在多个文件夹时（Gmail 标签）只留排在前面的文件夹里那份。
/// 服务器拒绝搜索的文件夹跳过；只有超时、断线才报错。
/// 会改变已选中的文件夹，调用方要记得之后重新选择。
pub async fn collect<T>(
    s: &mut async_imap::Session<T>,
    account_id: &str,
    folders: &[String],
    m: &Summary,
) -> Result<Found>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let (Some(criteria), Some(ids)) = (search_criteria(m), ids(m)) else {
        return Ok(Found {
            mails: Vec::new(),
            search_refused: false,
        });
    };
    let mut out: Vec<Summary> = Vec::new();
    let (mut searched, mut refused) = (0, 0);
    for folder in folders {
        // 文件夹不存在或打不开就跳过，不影响其他文件夹
        let quoted = crate::download_protocol::quoted_mailbox(folder)?;
        let validity =
            match timeout(OP_TIMEOUT, checked_command(s, &format!("EXAMINE {quoted}"))).await {
                Ok(Ok(responses)) => responses
                    .into_iter()
                    .find_map(|r| match r {
                        ArchiveData::Validity(v) => Some(v),
                        _ => None,
                    })
                    .unwrap_or(0),
                Ok(Err(e)) if !connection_problem(&e) => continue,
                Ok(Err(e)) => return Err(e),
                Err(_) => anyhow::bail!("打开文件夹超时"),
            };
        searched += 1;
        let found = match timeout(
            OP_TIMEOUT,
            checked_command(s, &format!("UID SEARCH UNDELETED {criteria}")),
        )
        .await
        .context("查找会话邮件超时")?
        {
            Ok(found) => found,
            Err(e) if !connection_problem(&e) => {
                refused += 1;
                continue;
            }
            Err(e) => return Err(e),
        };
        let mut uids: Vec<u32> = found
            .into_iter()
            .flat_map(|r| match r {
                ArchiveData::Uids(uids) => uids,
                _ => Vec::new(),
            })
            .collect();
        uids.sort_unstable();
        let uids = &uids[uids.len().saturating_sub(MAX_PER_FOLDER)..];
        if uids.is_empty() {
            continue;
        }
        let fetched = match imap::fetch_summaries(s, &imap::uid_set(uids), account_id, folder).await
        {
            Ok(fetched) => fetched,
            Err(e) if !connection_problem(&e) => continue,
            Err(e) => return Err(e),
        };
        for mut mail in fetched {
            mail.uid_validity = validity;
            let duplicate =
                !mail.message_id.is_empty() && out.iter().any(|x| x.message_id == mail.message_id);
            if !duplicate && really_related(&mail, &ids) {
                out.push(mail);
            }
        }
    }
    out.sort_by_key(|m| (m.date, m.uid));
    Ok(Found {
        mails: out,
        search_refused: searched > 0 && refused == searched,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{imap, test_imap};

    #[tokio::test]
    async fn discovered_thread_member_remains_readable_and_previewable_offline() {
        let root = tempfile::tempdir().unwrap();
        let index = crate::search_index::SearchIndex::open(&root.path().join("local.db")).unwrap();
        let server = test_imap::state("", &[("INBOX", 7), ("Sent", 9)]);
        server.lock().unwrap().add(
            "INBOX",
            &raw("a@x", "", "", "boss@x.com", "Question", 1),
            &[],
        );
        let reply = b"Message-ID: <b@x>\r\nIn-Reply-To: <a@x>\r\nReferences: <a@x>\r\nFrom: me@example.test\r\nSubject: Re: Question\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nSaved reply\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=reply.txt\r\n\r\nSaved attachment\r\n--x--\r\n";
        server
            .lock()
            .unwrap()
            .add("Sent", std::str::from_utf8(reply).unwrap(), &[]);
        let mut session = test_imap::session(server).await;
        let found = collect_indexed(
            &mut session,
            "acct",
            &["INBOX".into(), "Sent".into()],
            &summary("a@x", "", &[]),
            2,
            &index,
        )
        .await
        .unwrap();
        let sent = found
            .mails
            .iter()
            .find(|mail| mail.folder == "Sent")
            .unwrap();
        let source = crate::mail_ref::MessageRef {
            account_id: "acct".into(),
            receive_epoch: 2,
            folder: "Sent".into(),
            uid_validity: 9,
            uid: sent.uid,
        };
        let stored = index.snapshot_summary(&source).expect("server-discovered thread members need a persistent snapshot before returning to the reader");
        assert_eq!(stored.message_id, "b@x");
        assert_eq!(sent.receive_epoch, 2);
        assert!(
            !index
                .progress(&crate::mail_ref::namespace("acct", 2), "Sent")
                .unwrap()
                .complete,
            "finding one conversation must not claim the folder is fully indexed"
        );
        index.index_full_body(&source, reply).unwrap();
        index.index_attachments(&source, reply).unwrap();
        let disk = std::sync::Arc::new(crate::body_store::BodyStore::new(
            root.path().join("bodies"),
            1_000_000,
        ));
        disk.put(
            &crate::mail_ref::namespace("acct", 2),
            "Sent",
            9,
            sent.uid,
            reply,
            123,
        );
        let cache = std::sync::Mutex::new(crate::BodyCache::with_disk(disk));
        let local = crate::offline::read_snapshot(&index, &cache, &source, 2)
            .await
            .unwrap();
        assert!(local.message.text.contains("Saved reply"));
        assert_eq!(local.message.attachments.len(), 1);
        assert!(local.local_only);
    }

    #[tokio::test]
    async fn stale_thread_search_cannot_replace_a_newer_folder_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let index = crate::search_index::SearchIndex::open(&root.path().join("local.db")).unwrap();
        let storage = crate::mail_ref::namespace("acct", 2);
        let newer = Summary {
            account_id: "acct".into(),
            receive_epoch: 2,
            folder: "Sent".into(),
            uid: 1,
            uid_validity: 10,
            message_id: "new@x".into(),
            ..Default::default()
        };
        index
            .upsert_summaries(&storage, "Sent", 10, &[newer])
            .unwrap();
        let server = test_imap::state("", &[("Sent", 9)]);
        server.lock().unwrap().add(
            "Sent",
            &raw("old@x", "", "", "me@example.test", "Old", 1),
            &[],
        );
        let mut session = test_imap::session(server).await;
        assert!(
            collect_indexed(
                &mut session,
                "acct",
                &["Sent".into()],
                &summary("old@x", "", &[]),
                2,
                &index
            )
            .await
            .is_err()
        );
        let current = crate::mail_ref::MessageRef {
            account_id: "acct".into(),
            receive_epoch: 2,
            folder: "Sent".into(),
            uid_validity: 10,
            uid: 1,
        };
        assert_eq!(
            index.snapshot_summary(&current).unwrap().message_id,
            "new@x"
        );
        assert!(
            index
                .snapshot_summary(&crate::mail_ref::MessageRef {
                    receive_epoch: 1,
                    ..current
                })
                .is_err()
        );
    }

    #[tokio::test]
    async fn thread_lookup_preserves_existing_complete_index_and_full_body() {
        let root = tempfile::tempdir().unwrap();
        let index = crate::search_index::SearchIndex::open(&root.path().join("local.db")).unwrap();
        let server = test_imap::state("", &[("Sent", 9)]);
        let mail = raw("b@x", "a@x", "a@x", "me@example.test", "Re: Question", 1);
        server.lock().unwrap().add("Sent", &mail, &[]);
        let mut session = test_imap::session(server).await;
        let progress =
            crate::search_index::index_folder(&mut session, &index, "acct", "Sent", |_| {})
                .await
                .unwrap();
        assert!(progress.complete);
        let source = crate::mail_ref::MessageRef {
            account_id: "acct".into(),
            receive_epoch: 1,
            folder: "Sent".into(),
            uid_validity: 9,
            uid: 1,
        };
        index.index_full_body(&source, mail.as_bytes()).unwrap();
        let before = index.progress("acct", "Sent").unwrap();
        collect_indexed(
            &mut session,
            "acct",
            &["Sent".into()],
            &summary("a@x", "", &[]),
            1,
            &index,
        )
        .await
        .unwrap();
        assert_eq!(index.progress("acct", "Sent").unwrap(), before);
        assert!(
            index.snapshot_reference(&source).unwrap().1,
            "thread summary refresh must not downgrade full body coverage"
        );
    }

    fn raw(id: &str, reply_to: &str, refs: &str, from: &str, subject: &str, day: u32) -> String {
        let mut head = format!(
            "Message-ID: <{id}>\r\nFrom: {from}\r\nSubject: {subject}\r\nX-Internal-Date: {day:02}-Oct-2026 08:00:00 +0000\r\n"
        );
        if !reply_to.is_empty() {
            head += &format!("In-Reply-To: <{reply_to}>\r\n");
        }
        if !refs.is_empty() {
            let refs: Vec<String> = refs.split(' ').map(|r| format!("<{r}>")).collect();
            head += &format!("References: {}\r\n", refs.join(" "));
        }
        format!("{head}\r\nbody\r\n")
    }

    #[tokio::test]
    async fn summaries_carry_the_threading_headers() {
        let server = test_imap::state("", &[("INBOX", 1)]);
        server.lock().unwrap().add(
            "INBOX",
            &raw("c@x", "b@x", "a@x b@x", "z@x.com", "Re: 报价", 3),
            &[],
        );
        let mut s = test_imap::session(server).await;
        s.select("INBOX").await.unwrap();
        let got = imap::fetch_summaries(&mut s, "1", "acct", "INBOX")
            .await
            .unwrap();
        assert_eq!(got[0].message_id, "c@x");
        assert_eq!(got[0].in_reply_to, "b@x");
        assert_eq!(
            got[0].references,
            vec!["a@x".to_string(), "b@x".to_string()]
        );
    }

    fn summary(id: &str, reply_to: &str, refs: &[&str]) -> Summary {
        Summary {
            message_id: id.into(),
            in_reply_to: reply_to.into(),
            references: refs.iter().map(|r| r.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn search_finds_ancestors_and_replies_by_id() {
        let q = search_criteria(&summary("c@x", "b@x", &["a@x", "b@x"])).unwrap();
        assert_eq!(
            q,
            "OR OR OR OR OR HEADER Message-ID \"a@x\" HEADER Message-ID \"b@x\" HEADER References \"a@x\" HEADER References \"c@x\" HEADER In-Reply-To \"c@x\" HEADER Message-ID \"c@x\""
        );
        // 新会话的第一封：只找回复它的邮件和它自己
        let q = search_criteria(&summary("a@x", "", &[])).unwrap();
        assert_eq!(
            q,
            "OR OR HEADER References \"a@x\" HEADER In-Reply-To \"a@x\" HEADER Message-ID \"a@x\""
        );
        assert!(
            search_criteria(&summary("", "", &[])).is_none(),
            "没有 ID 时不搜索"
        );
        // 带引号、换行或非 ASCII 的 ID 不拼进命令
        let q = search_criteria(&summary("ok@x", "bad\"id", &["坏@x", "a\r\nb"])).unwrap();
        assert!(
            !q.contains("bad") && !q.contains('坏') && !q.contains('\r'),
            "{q}"
        );
    }

    #[test]
    fn many_references_only_search_the_most_recent_ancestors() {
        let refs: Vec<String> = (0..30).map(|i| format!("r{i}@x")).collect();
        let refs: Vec<&str> = refs.iter().map(String::as_str).collect();
        let q = search_criteria(&summary("me@x", "r29@x", &refs)).unwrap();
        assert!(q.contains("\"r0@x\""), "根邮件一定要找");
        assert!(q.contains("\"r29@x\""));
        assert!(!q.contains("\"r5@x\""), "中间太多的祖先不逐个找");
    }

    #[tokio::test]
    async fn a_server_that_refuses_header_search_keeps_the_connection() {
        let server = test_imap::state("", &[("INBOX", 1), ("Sent", 2)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", &raw("a@x", "", "", "boss@x.com", "报价", 1), &[]);
        server.lock().unwrap().fail = vec!["UID SEARCH".into()];
        let mut s = test_imap::session(server.clone()).await;
        let folders = vec!["INBOX".to_string(), "Sent".into()];
        let found = collect(&mut s, "acct", &folders, &summary("a@x", "", &[]))
            .await
            .unwrap();
        assert!(found.mails.is_empty());
        assert!(found.search_refused, "记下服务器不支持，下次不再搜");
        assert!(s.noop().await.is_ok(), "连接还能继续用");
    }

    #[tokio::test]
    async fn deleted_mail_and_look_alike_ids_are_not_part_of_the_conversation() {
        let server = test_imap::state("", &[("INBOX", 1)]);
        {
            let mut st = server.lock().unwrap();
            st.add("INBOX", &raw("1@host", "", "", "a@x.com", "原邮件", 1), &[]);
            st.add(
                "INBOX",
                &raw("r@host", "1@host", "1@host", "b@x.com", "Re: 原邮件", 2),
                &[],
            );
            // 子串相同但不是同一个 ID
            st.add("INBOX", &raw("21@host", "", "", "c@x.com", "别的", 3), &[]);
            st.add(
                "INBOX",
                &raw("x@host", "21@host", "21@host", "d@x.com", "Re: 别的", 4),
                &[],
            );
            // 已标记删除、等待清除
            st.add(
                "INBOX",
                &raw("gone@host", "1@host", "1@host", "e@x.com", "Re: 原邮件", 5),
                &["\\Deleted"],
            );
        }
        let mut s = test_imap::session(server).await;
        let found = collect(
            &mut s,
            "acct",
            &["INBOX".to_string()],
            &summary("1@host", "", &[]),
        )
        .await
        .unwrap();
        let ids: Vec<_> = found.mails.iter().map(|m| m.message_id.as_str()).collect();
        assert_eq!(ids, vec!["1@host", "r@host"]);
        assert!(!found.search_refused);
    }

    #[tokio::test]
    async fn conversation_spans_folders_and_skips_unrelated_mail() {
        let server = test_imap::state("", &[("INBOX", 1), ("Sent", 2), ("Archive", 3)]);
        {
            let mut st = server.lock().unwrap();
            st.add("INBOX", &raw("a@x", "", "", "boss@x.com", "报价", 1), &[]);
            st.add(
                "Sent",
                &raw("b@x", "a@x", "a@x", "me@x.com", "Re: 报价", 2),
                &[],
            );
            st.add(
                "INBOX",
                &raw("c@x", "b@x", "a@x b@x", "boss@x.com", "Re: 报价", 3),
                &[],
            );
            st.add("INBOX", &raw("other@x", "", "", "x@y.com", "无关", 4), &[]);
            // 同一封在两个文件夹里各有一份（例如 Gmail 标签）
            st.add("Archive", &raw("a@x", "", "", "boss@x.com", "报价", 1), &[]);
        }
        let mut s = test_imap::session(server).await;
        s.select("INBOX").await.unwrap();
        let inbox = imap::fetch_summaries(&mut s, "1:*", "acct", "INBOX")
            .await
            .unwrap();
        let open: Vec<_> = inbox
            .into_iter()
            .filter(|m| m.message_id == "c@x")
            .collect();
        let folders = vec![
            "INBOX".to_string(),
            "Sent".into(),
            "Archive".into(),
            "Missing".into(),
        ];
        let got = collect(&mut s, "acct", &folders, &open[0])
            .await
            .unwrap()
            .mails;
        let ids: Vec<_> = got
            .iter()
            .map(|m| (m.message_id.as_str(), m.folder.as_str()))
            .collect();
        assert_eq!(
            ids,
            vec![("a@x", "INBOX"), ("b@x", "Sent"), ("c@x", "INBOX")],
            "按时间排，重复的留第一个文件夹的"
        );
    }
}
