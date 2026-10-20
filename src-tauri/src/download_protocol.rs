//! IMAP 下载协议校验：只有匹配命令的 tagged OK 才能确认查询、原文或邮件消失。

use crate::{
    auto_download::Checkpoint,
    imap::{self, Folder, OP_TIMEOUT},
};
use anyhow::{Context, Result, anyhow, bail};
use async_imap::{
    Session,
    imap_proto::{
        AttributeValue, MailboxDatum, MailboxListData, NameAttribute, RequestId, Response,
        ResponseCode, Status as ImapStatus,
    },
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};

pub(crate) enum ArchiveData {
    ListedMailbox(MailboxListData<'static>),
    Uids(Vec<u32>),
    Validity(u32),
    Next(u32),
    Message {
        uid: Option<u32>,
        raw: Option<Vec<u8>>,
        date: i64,
        /// RFC822.SIZE
        size: Option<u32>,
    },
}

fn archive_data(response: &Response<'_>) -> Option<ArchiveData> {
    match response {
        Response::MailboxData(MailboxDatum::List(data)) => {
            Some(ArchiveData::ListedMailbox(data.clone().into_owned()))
        }
        Response::MailboxData(MailboxDatum::Search(uids)) => Some(ArchiveData::Uids(uids.clone())),
        Response::Data {
            status: ImapStatus::Ok,
            outcome,
        } => match outcome.code {
            Some(ResponseCode::UidValidity(value)) => Some(ArchiveData::Validity(value)),
            Some(ResponseCode::UidNext(value)) => Some(ArchiveData::Next(value)),
            _ => None,
        },
        Response::Fetch(_, attributes) => {
            let uid = attributes.iter().find_map(|attribute| {
                if let AttributeValue::Uid(value) = attribute {
                    Some(*value)
                } else {
                    None
                }
            });
            let raw = attributes.iter().find_map(|attribute| match attribute {
                AttributeValue::BodySection {
                    section: None,
                    index: None,
                    data: Some(raw),
                }
                | AttributeValue::Rfc822(Some(raw)) => Some(raw.as_ref().to_vec()),
                _ => None,
            });
            let date = attributes
                .iter()
                .find_map(|attribute| {
                    if let AttributeValue::InternalDate(value) = attribute {
                        chrono::DateTime::parse_from_str(value, "%d-%b-%Y %H:%M:%S %z")
                            .ok()
                            .map(|date| date.timestamp_millis())
                    } else {
                        None
                    }
                })
                .unwrap_or(0);
            let size = attributes.iter().find_map(|attribute| match attribute {
                AttributeValue::Rfc822Size(size) => Some(*size),
                _ => None,
            });
            Some(ArchiveData::Message {
                uid,
                raw,
                date,
                size,
            })
        }
        _ => None,
    }
}

// async-imap 的部分流式助手不验证命令末尾状态。归档必须先收到对应 tagged OK，
// 才能把 LIST/SEARCH/FETCH/EXAMINE 的数据视为成功，避免把拒绝或断线当空邮箱。
/// 超时或断线：命令的结果未知，这条连接也不能再用。
/// 其余错误（服务器回 NO/BAD、不支持某个功能等）只是这个命令没执行，连接照常可用。
pub(crate) fn connection_problem(e: &anyhow::Error) -> bool {
    e.chain().any(|c| {
        c.is::<std::io::Error>()
            || c.is::<tokio::time::error::Elapsed>()
            || matches!(
                c.downcast_ref::<async_imap::error::Error>(),
                Some(async_imap::error::Error::Io(_) | async_imap::error::Error::ConnectionLost)
            )
    })
}

pub(crate) async fn checked_command<T>(
    session: &mut Session<T>,
    command: &str,
) -> Result<Vec<ArchiveData>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let tag = session.run_command(command).await?;
    finish_command(session, tag).await
}

/// 读到 `tag` 的最终答复为止；只有 OK 才算成功，NO/BAD 报错。
pub(crate) async fn finish_command<T>(
    session: &mut Session<T>,
    tag: RequestId,
) -> Result<Vec<ArchiveData>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let mut responses = Vec::new();
    loop {
        let response = session.read_response().await?.ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "服务器未确认命令完成，连接已断开",
            )
        })?;
        match response.parsed() {
            Response::Done {
                tag: done,
                status,
                outcome,
            } => {
                if done != &tag {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "服务器返回了不匹配的命令完成标记",
                    )
                    .into());
                }
                return match status {
                    ImapStatus::Ok => Ok(responses),
                    ImapStatus::No => {
                        Err(async_imap::error::Error::No(format!("{outcome:?}")).into())
                    }
                    ImapStatus::Bad => {
                        Err(async_imap::error::Error::Bad(format!("{outcome:?}")).into())
                    }
                    _ => Err(async_imap::error::Error::ConnectionLost.into()),
                };
            }
            Response::Data {
                status: ImapStatus::Bye,
                ..
            } => return Err(async_imap::error::Error::ConnectionLost.into()),
            other => {
                if let Some(data) = archive_data(other) {
                    responses.push(data);
                }
            }
        }
    }
}

pub(crate) fn quoted_mailbox(folder: &str) -> Result<String> {
    if folder.chars().any(char::is_control) {
        bail!("服务器文件夹名称包含无效控制字符");
    }
    Ok(format!(
        "\"{}\"",
        folder.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

pub(crate) async fn checked_folders<T>(session: &mut Session<T>) -> Result<Vec<Folder>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let responses = timeout(OP_TIMEOUT, checked_command(session, "LIST \"\" \"*\""))
        .await
        .context("读取文件夹超时")??;
    let mut folders = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for response in responses {
        if let ArchiveData::ListedMailbox(data) = response {
            if !data
                .name_attributes
                .iter()
                .any(|attr| matches!(attr, NameAttribute::NoSelect))
                && seen.insert(data.name.to_string())
            {
                folders.push(Folder {
                    path: data.name.to_string(),
                    name: imap::decode_mutf7(&data.name),
                    role: "other".into(),
                });
            }
        }
    }
    folders.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(folders)
}

pub(crate) async fn checked_boundary<T>(
    session: &mut Session<T>,
    folder: &str,
) -> Result<Checkpoint>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let command = format!("EXAMINE {}", quoted_mailbox(folder)?);
    let responses = timeout(OP_TIMEOUT, checked_command(session, &command))
        .await
        .context("读取文件夹超时")??;
    let mut validity = None;
    let mut next = None;
    for response in responses {
        match response {
            ArchiveData::Validity(value) => validity = Some(value),
            ArchiveData::Next(value) => next = Some(value),
            _ => {}
        }
    }
    Ok(Checkpoint {
        uid_validity: validity
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow!("服务器未提供 UIDVALIDITY"))?,
        last_uid: next
            .filter(|value| *value > 0)
            .ok_or_else(|| anyhow!("服务器未提供 UIDNEXT"))?
            - 1,
    })
}

pub(crate) async fn checked_search<T>(
    session: &mut Session<T>,
    start: u32,
    end: u32,
) -> Result<Vec<u32>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let command = format!("UID SEARCH UID {start}:{end}");
    let responses = timeout(OP_TIMEOUT, checked_command(session, &command))
        .await
        .context("查询邮件超时")??;
    let mut uids = Vec::new();
    let mut reported = false;
    for response in responses {
        if let ArchiveData::Uids(found) = response {
            reported = true;
            uids.extend(
                found
                    .iter()
                    .copied()
                    .filter(|uid| *uid >= start && *uid <= end),
            );
        }
    }
    if !reported {
        bail!("服务器未返回邮件查询结果");
    }
    uids.sort_unstable();
    uids.dedup();
    Ok(uids)
}

pub(crate) async fn checked_raw_message<T>(
    session: &mut Session<T>,
    uid: u32,
) -> Result<Option<Vec<u8>>>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    let command = format!("UID FETCH {uid} (UID BODY.PEEK[])");
    let responses = timeout(OP_TIMEOUT * 3, checked_command(session, &command))
        .await
        .context("下载邮件超时")??;
    let mut reported_uid = false;
    for response in responses {
        if let ArchiveData::Message {
            uid: Some(found),
            raw,
            ..
        } = response
        {
            if found == uid {
                reported_uid = true;
                if let Some(raw) = raw {
                    return Ok(Some(raw));
                }
            }
        }
    }
    if reported_uid {
        bail!("服务器没有返回完整邮件原文");
    }
    if checked_search(session, uid, uid).await?.contains(&uid) {
        bail!("邮件仍存在，但服务器没有返回原文，请重新下载");
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_internal_date_keeps_ui_milliseconds() {
        let (_, response) = Response::parse(b"* 1 FETCH (UID 11 INTERNALDATE \"02-Jan-2025 03:04:05 +0000\" BODY[] {4}\r\nmail)\r\n").unwrap();
        match archive_data(&response).unwrap() {
            ArchiveData::Message { uid, date, raw, .. } => {
                assert_eq!(uid, Some(11));
                assert_eq!(date, 1_735_787_045_000);
                assert_eq!(raw.as_deref(), Some(b"mail".as_slice()));
            }
            _ => panic!("expected message response"),
        }
    }
}
