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
        match timeout(OP_TIMEOUT, checked_command(s, &format!("EXAMINE {quoted}"))).await {
            Ok(Ok(_)) => {}
            Ok(Err(e)) if !connection_problem(&e) => continue,
            Ok(Err(e)) => return Err(e),
            Err(_) => anyhow::bail!("打开文件夹超时"),
        }
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
        for mail in fetched {
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
