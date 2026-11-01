//! Interactive raw-message downloads. Only tagged-OK, identity-checked ranges
//! become resume checkpoints; partial data is never a complete body cache.

use anyhow::{Result, anyhow, bail, ensure};
use async_imap::Session;
use std::future::Future;

use crate::{
    download_protocol::{ArchiveData, checked_command},
    download_resume::Resume,
    lanes::Stream,
};

pub(crate) const CHUNK_BYTES: usize = 64 * 1024;

fn next_chunk_size(current: usize, elapsed: std::time::Duration) -> usize {
    if elapsed < std::time::Duration::from_secs(1) {
        (current * 2).min(1024 * 1024)
    } else if elapsed > std::time::Duration::from_secs(5) {
        (current / 2).max(CHUNK_BYTES)
    } else {
        current
    }
}

fn unsupported(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<async_imap::error::Error>(),
            Some(async_imap::error::Error::No(_) | async_imap::error::Error::Bad(_))
        )
    })
}

pub(crate) async fn fetch<T, F, Fut>(
    session: &mut Session<T>,
    uid: u32,
    peek: bool,
    resume: Option<Resume>,
    mut checkpoint: F,
) -> Result<(Vec<u8>, i64)>
where
    T: Stream,
    F: FnMut(u32, i64, usize, Vec<u8>) -> Fut,
    Fut: Future<Output = ()>,
{
    fetch_controlled(
        session,
        uid,
        peek,
        resume,
        None,
        |total, date, offset, bytes| {
            let saved = checkpoint(total, date, offset, bytes);
            async move {
                saved.await;
                Ok(())
            }
        },
    )
    .await
}

/// Background callers can stop between confirmed chunks (cleanup, account
/// changes, foreground reading), and enforce their own complete-file limit.
/// A limit also keeps ranges at 64 KiB so foreground reads get a frequent turn.
pub(crate) async fn fetch_controlled<T, F, Fut>(
    session: &mut Session<T>,
    uid: u32,
    peek: bool,
    resume: Option<Resume>,
    max_bytes: Option<usize>,
    mut checkpoint: F,
) -> Result<(Vec<u8>, i64)>
where
    T: Stream,
    F: FnMut(u32, i64, usize, Vec<u8>) -> Fut,
    Fut: Future<Output = Result<()>>,
{
    let Resume {
        mut raw,
        mut date,
        total: mut expected_total,
    } = resume.unwrap_or(Resume {
        raw: Vec::new(),
        date: 0,
        total: 0,
    });
    ensure!(
        max_bytes.is_none_or(|max| raw.len() <= max && expected_total as usize <= max),
        "邮件超过后台缓存的单封大小上限"
    );
    let body = if peek { "BODY.PEEK[]" } else { "BODY[]" };
    let mut chunk_bytes = CHUNK_BYTES;
    loop {
        let offset = raw.len();
        let started = std::time::Instant::now();
        let command = format!(
            "UID FETCH {uid} (UID RFC822.SIZE INTERNALDATE {body}<{offset}.{chunk_bytes}>)"
        );
        // The production reader's Counting stream enforces a *no-data* timeout.
        // A large but steadily progressing message has no fixed wall-clock limit.
        let responses = match checked_command(session, &command).await {
            Ok(responses) => responses,
            Err(error) if offset == 0 && max_bytes.is_none() && unsupported(&error) => {
                // Older servers may reject partial FETCH. Retry only once with
                // a full response, whose tagged status is still checked.
                checked_command(
                    session,
                    &format!("UID FETCH {uid} (UID RFC822.SIZE INTERNALDATE {body})"),
                )
                .await?
            }
            Err(error) => return Err(error),
        };
        let mut chunk = None;
        let mut complete = None;
        let mut response_size = None;
        let mut response_date = 0;
        for response in responses {
            // A server may return the requested attributes in multiple FETCH
            // responses. Only metadata for this UID and this confirmed command
            // can describe its body; unsolicited updates for other UIDs cannot.
            let (found, size, received_date) = match &response {
                ArchiveData::Chunk {
                    uid, size, date, ..
                }
                | ArchiveData::Message {
                    uid, size, date, ..
                } => (*uid, *size, *date),
                _ => continue,
            };
            if found != Some(uid) {
                continue;
            }
            if let Some(size) = size {
                ensure!(
                    response_size.is_none_or(|previous| previous == size),
                    "服务器返回了相互冲突的邮件大小，未拼接下载数据"
                );
                response_size = Some(size);
            }
            if response_date == 0 && received_date != 0 {
                response_date = received_date;
            }
            match response {
                ArchiveData::Chunk {
                    raw: bytes,
                    offset: returned,
                    ..
                } => {
                    ensure!(
                        chunk.is_none() && complete.is_none(),
                        "服务器重复返回邮件内容，未拼接下载数据"
                    );
                    chunk = Some((bytes, returned as usize));
                }
                ArchiveData::Message {
                    raw: Some(bytes), ..
                } => {
                    ensure!(
                        chunk.is_none() && complete.is_none(),
                        "服务器重复返回邮件内容，未拼接下载数据"
                    );
                    complete = Some(bytes);
                }
                _ => {}
            }
        }
        if let Some(bytes) = complete {
            ensure!(offset == 0, "服务器没有按指定位置续传，请重试从头下载");
            ensure!(
                response_size.is_none_or(|size| size as usize == bytes.len()),
                "服务器返回的邮件大小不完整"
            );
            ensure!(
                max_bytes.is_none_or(|max| bytes.len() <= max),
                "邮件超过后台缓存的单封大小上限"
            );
            return Ok((bytes, response_date));
        }
        let (bytes, returned) = chunk
            .ok_or_else(|| anyhow!("服务器没有返回邮件原文，邮件可能已在别处删除，请刷新后重试"))?;
        let total = response_size.ok_or_else(|| anyhow!("服务器未确认邮件大小，无法安全续传"))?;
        ensure!(
            max_bytes.is_none_or(|max| total as usize <= max),
            "邮件超过后台缓存的单封大小上限"
        );
        ensure!(
            returned == offset,
            "服务器返回的续传位置不一致，请重试从头下载"
        );
        ensure!(
            expected_total == 0 || expected_total == total,
            "邮件大小已发生变化，请重试从头下载"
        );
        ensure!(offset <= total as usize, "续传位置超过邮件大小，请重新下载");
        let wanted = chunk_bytes.min(total as usize - offset);
        ensure!(
            bytes.len() == wanted,
            "服务器返回了不完整的邮件分段，请重试继续下载"
        );
        if offset == 0 {
            date = response_date;
            expected_total = total;
        }
        // A zero-length final probe validates an already complete checkpoint
        // after an interrupted cache publication.
        if bytes.is_empty() {
            ensure!(raw.len() == total as usize, "服务器没有返回邮件内容");
            return Ok((raw, date));
        }
        raw.extend_from_slice(&bytes);
        checkpoint(total, date, offset, bytes).await?;
        // Keep small checkpoints on a slow connection without adding hundreds
        // of command round trips to a fast one. Never exceed 1 MiB per range.
        if max_bytes.is_none() {
            chunk_bytes = next_chunk_size(chunk_bytes, started.elapsed());
        }
        if raw.len() == total as usize {
            return Ok((raw, date));
        }
        if raw.len() > total as usize {
            bail!("邮件内容超过服务器声明的大小");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

    async fn session(
        replies: Vec<(String, Option<&'static str>)>,
    ) -> (Session<DuplexStream>, Arc<Mutex<Vec<String>>>) {
        let (client, server) = tokio::io::duplex(8192);
        let commands = Arc::new(Mutex::new(Vec::new()));
        let observed = commands.clone();
        tokio::spawn(async move {
            let mut server = BufReader::new(server);
            for (data, completion) in
                std::iter::once((String::new(), Some("OK login"))).chain(replies)
            {
                let mut command = String::new();
                if server.read_line(&mut command).await.unwrap() == 0 {
                    return;
                }
                let tag = command.split_whitespace().next().unwrap().to_owned();
                observed.lock().unwrap().push(command);
                server.write_all(data.as_bytes()).await.unwrap();
                if let Some(completion) = completion {
                    server
                        .write_all(format!("{tag} {completion}\r\n").as_bytes())
                        .await
                        .unwrap();
                } else {
                    return;
                }
            }
        });
        (
            async_imap::Client::new(client)
                .login("test", "pw")
                .await
                .unwrap(),
            commands,
        )
    }

    fn chunk(uid: u32, total: usize, offset: usize, data: &str) -> String {
        format!(
            "* 1 FETCH (UID {uid} RFC822.SIZE {total} INTERNALDATE \"02-Jan-2025 03:04:05 +0000\" BODY[]<{offset}> {{{}}}\r\n{data})\r\n",
            data.len()
        )
    }

    #[test]
    fn fast_links_grow_ranges_and_slow_links_keep_bounded_checkpoints() {
        use std::time::Duration;
        assert_eq!(
            next_chunk_size(CHUNK_BYTES, Duration::from_millis(100)),
            2 * CHUNK_BYTES
        );
        assert_eq!(next_chunk_size(1024 * 1024, Duration::ZERO), 1024 * 1024);
        assert_eq!(
            next_chunk_size(1024 * 1024, Duration::from_secs(10)),
            512 * 1024
        );
        assert_eq!(
            next_chunk_size(CHUNK_BYTES, Duration::from_secs(10)),
            CHUNK_BYTES
        );
    }

    #[tokio::test]
    async fn background_cancellation_stops_before_requesting_the_next_range() {
        let first = "a".repeat(CHUNK_BYTES);
        let (mut s, commands) = session(vec![(
            chunk(7, CHUNK_BYTES + 4, 0, &first),
            Some("OK chunk"),
        )])
        .await;
        let saved = Mutex::new(Vec::new());
        let result = fetch_controlled(
            &mut s,
            7,
            true,
            None,
            Some(32 * 1024 * 1024),
            |_, _, _, bytes| {
                saved.lock().unwrap().extend(bytes);
                async { anyhow::bail!("cleanup cancelled this write ticket") }
            },
        )
        .await;
        assert!(result.unwrap_err().to_string().contains("cancelled"));
        assert_eq!(saved.lock().unwrap().len(), CHUNK_BYTES);
        assert_eq!(
            commands.lock().unwrap().len(),
            2,
            "only login and first range"
        );
    }

    #[tokio::test]
    async fn background_large_arrivals_keep_small_yield_points_and_reject_over_limit() {
        let first = "a".repeat(CHUNK_BYTES);
        let total = CHUNK_BYTES * 2 + 4;
        let (mut s, commands) = session(vec![
            (chunk(7, total, 0, &first), Some("OK chunk")),
            (chunk(7, total, CHUNK_BYTES, &first), Some("OK chunk")),
            (chunk(7, total, CHUNK_BYTES * 2, "tail"), Some("OK chunk")),
        ])
        .await;
        let (raw, _) = fetch_controlled(
            &mut s,
            7,
            true,
            None,
            Some(32 * 1024 * 1024),
            |_, _, _, _| async { Ok(()) },
        )
        .await
        .unwrap();
        assert_eq!(raw.len(), total);
        assert!(commands.lock().unwrap()[2].contains("<65536.65536>"));
        let (mut s, _) = session(vec![(
            chunk(7, 33 * 1024 * 1024, 0, &first),
            Some("OK chunk"),
        )])
        .await;
        let saved = Mutex::new(0);
        assert!(
            fetch_controlled(
                &mut s,
                7,
                true,
                None,
                Some(32 * 1024 * 1024),
                |_, _, _, _| {
                    *saved.lock().unwrap() += 1;
                    async { Ok(()) }
                }
            )
            .await
            .is_err()
        );
        assert_eq!(*saved.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn background_does_not_fallback_to_an_uninterruptible_full_body_request() {
        let (mut s, commands) =
            session(vec![(String::new(), Some("NO partial unsupported"))]).await;
        assert!(
            fetch_controlled(
                &mut s,
                7,
                true,
                None,
                Some(32 * 1024 * 1024),
                |_, _, _, _| async { Ok(()) }
            )
            .await
            .is_err()
        );
        assert_eq!(commands.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn confirmed_ranges_are_joined_once_and_checkpointed_at_exact_offsets() {
        let first = "a".repeat(CHUNK_BYTES);
        let tail = "tail";
        let total = first.len() + tail.len();
        let (mut s, commands) = session(vec![
            (chunk(7, total, 0, &first), Some("OK chunk")),
            (chunk(7, total, first.len(), tail), Some("OK chunk")),
        ])
        .await;
        let saved = Mutex::new(Vec::new());
        let (raw, _) = fetch(&mut s, 7, true, None, |_, _, offset, bytes| {
            saved.lock().unwrap().push((offset, bytes.len()));
            async {}
        })
        .await
        .unwrap();
        assert_eq!(raw, format!("{first}{tail}").into_bytes());
        assert_eq!(
            *saved.lock().unwrap(),
            vec![(0, CHUNK_BYTES), (CHUNK_BYTES, 4)]
        );
        assert!(commands.lock().unwrap()[2].contains("BODY.PEEK[]<65536."));
    }

    #[tokio::test]
    async fn disconnect_keeps_only_tagged_ok_bytes_and_retry_starts_at_checkpoint() {
        let first = "a".repeat(CHUNK_BYTES);
        let total = CHUNK_BYTES + 4;
        let (mut s, _) = session(vec![
            (chunk(7, total, 0, &first), Some("OK chunk")),
            (chunk(7, total, CHUNK_BYTES, "tail"), None),
        ])
        .await;
        let saved = Mutex::new(Vec::new());
        let error = fetch(&mut s, 7, true, None, |_, _, _, bytes| {
            saved.lock().unwrap().extend(bytes);
            async {}
        })
        .await
        .unwrap_err();
        assert!(crate::download_protocol::connection_problem(&error));
        assert_eq!(saved.lock().unwrap().len(), CHUNK_BYTES);
        let (mut s, commands) = session(vec![(
            chunk(7, total, CHUNK_BYTES, "tail"),
            Some("OK resumed"),
        )])
        .await;
        let resume = Resume {
            raw: saved.into_inner().unwrap(),
            date: 42,
            total: total as u32,
        };
        let (raw, date) = fetch(&mut s, 7, true, Some(resume), |_, _, _, _| async {})
            .await
            .unwrap();
        assert_eq!(raw, format!("{first}tail").into_bytes());
        assert_eq!(date, 42);
        assert!(commands.lock().unwrap()[1].contains("<65536.65536>"));
    }

    #[tokio::test]
    async fn refused_or_mismatched_ranges_never_publish_a_checkpoint() {
        for (response, status) in [
            (chunk(7, 4, 0, "mail"), "NO denied"),
            (chunk(8, 4, 0, "mail"), "OK done"),
            (chunk(7, 4, 1, "mail"), "OK done"),
            (chunk(7, 5, 0, "mail"), "OK done"),
        ] {
            // An initial NO gets one full-fetch fallback, which is also refused.
            let (mut s, _) = session(vec![
                (response, Some(status)),
                (String::new(), Some("NO denied")),
            ])
            .await;
            let saved = Mutex::new(0);
            assert!(
                fetch(&mut s, 7, true, None, |_, _, _, _| {
                    *saved.lock().unwrap() += 1;
                    async {}
                })
                .await
                .is_err()
            );
            assert_eq!(*saved.lock().unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn resumed_response_must_confirm_the_original_size_and_offset() {
        for response in [chunk(7, 9, 4, "tail"), chunk(7, 8, 0, "tail")] {
            let (mut s, _) = session(vec![(response, Some("OK done"))]).await;
            let resume = Resume {
                raw: b"head".to_vec(),
                date: 0,
                total: 8,
            };
            assert!(
                fetch(&mut s, 7, true, Some(resume), |_, _, _, _| async {})
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn split_metadata_for_the_same_uid_describes_ranges_and_full_bodies_in_any_order() {
        let expected_date = 1_735_787_045_000;
        for partial in [true, false] {
            for metadata_first in [true, false] {
                let metadata = concat!(
                    "* 1 FETCH (UID 7 RFC822.SIZE 4)\r\n",
                    "* 1 FETCH (UID 7 INTERNALDATE \"02-Jan-2025 03:04:05 +0000\")\r\n",
                );
                let body = if partial {
                    "* 1 FETCH (UID 7 BODY[]<0> {4}\r\nmail)\r\n"
                } else {
                    "* 1 FETCH (UID 7 BODY[] {4}\r\nmail)\r\n"
                };
                // Other mail's metadata must neither replace this size/date
                // nor be treated as a contradictory response for this UID.
                let unrelated = "* 2 FETCH (UID 8 RFC822.SIZE 999 INTERNALDATE \"03-Jan-2025 03:04:05 +0000\")\r\n";
                let response = if metadata_first {
                    format!("{unrelated}{metadata}{body}")
                } else {
                    format!("{unrelated}{body}{metadata}")
                };
                let (mut s, _) = session(vec![(response, Some("OK fetched"))]).await;
                let saved = Mutex::new(Vec::new());
                let (raw, date) = fetch(&mut s, 7, true, None, |total, date, offset, bytes| {
                    saved.lock().unwrap().push((total, date, offset, bytes));
                    async {}
                })
                .await
                .unwrap();
                assert_eq!(raw, b"mail");
                assert_eq!(date, expected_date);
                if partial {
                    assert_eq!(
                        *saved.lock().unwrap(),
                        vec![(4, expected_date, 0, b"mail".to_vec())]
                    );
                } else {
                    assert!(saved.lock().unwrap().is_empty());
                }
            }
        }
    }

    #[tokio::test]
    async fn conflicting_same_uid_metadata_is_rejected_before_checkpointing() {
        for metadata_first in [true, false] {
            let metadata = "* 1 FETCH (UID 7 RFC822.SIZE 5)\r\n";
            let body = chunk(7, 4, 0, "mail");
            let response = if metadata_first {
                format!("{metadata}{body}")
            } else {
                format!("{body}{metadata}")
            };
            let (mut s, _) = session(vec![(response, Some("OK fetched"))]).await;
            let saved = Mutex::new(0);
            let result = fetch(&mut s, 7, true, None, |_, _, _, _| {
                *saved.lock().unwrap() += 1;
                async {}
            })
            .await;
            assert!(result.is_err());
            assert_eq!(*saved.lock().unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn split_metadata_from_another_uid_cannot_supply_a_missing_range_size() {
        let response = concat!(
            "* 2 FETCH (UID 8 RFC822.SIZE 4)\r\n",
            "* 1 FETCH (UID 7 BODY[]<0> {4}\r\nmail)\r\n",
        );
        let (mut s, _) = session(vec![(response.into(), Some("OK fetched"))]).await;
        let saved = Mutex::new(0);
        let result = fetch(&mut s, 7, true, None, |_, _, _, _| {
            *saved.lock().unwrap() += 1;
            async {}
        })
        .await;
        assert!(result.is_err());
        assert_eq!(*saved.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn server_without_partial_fetch_can_return_a_confirmed_full_message() {
        let (mut s, commands) = session(vec![
            (String::new(), Some("BAD partial not supported")),
            (
                "* 1 FETCH (UID 7 RFC822.SIZE 4 BODY[] {4}\r\nmail)\r\n".into(),
                Some("OK full"),
            ),
        ])
        .await;
        let (raw, _) = fetch(&mut s, 7, true, None, |_, _, _, _| async {})
            .await
            .unwrap();
        assert_eq!(raw, b"mail");
        assert!(!commands.lock().unwrap()[2].contains("<0."));
    }
}
