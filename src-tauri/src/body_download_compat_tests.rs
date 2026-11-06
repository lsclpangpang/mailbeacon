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
        for (data, completion) in std::iter::once((String::new(), Some("OK login"))).chain(replies)
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

fn response(uid: u32, total: usize, offset: Option<usize>, data: &str) -> String {
    let range = offset.map_or(String::new(), |offset| format!("<{offset}>"));
    format!(
        "* 1 FETCH (UID {uid} RFC822.SIZE {total} INTERNALDATE \"02-Jan-2025 03:04:05 +0000\" BODY[]{range} {{{}}}\r\n{data})\r\n",
        data.len()
    )
}

fn assert_full_fallback(commands: &Arc<Mutex<Vec<String>>>, peek: bool) {
    let commands = commands.lock().unwrap();
    assert_eq!(
        commands.len(),
        3,
        "login, one range, one independent full fetch"
    );
    assert!(commands[1].contains("<"), "the first fetch is partial");
    let body = if peek { "BODY.PEEK[]" } else { "BODY[]" };
    assert!(
        commands[2].contains(&format!("{body})")),
        "the fallback must explicitly request the entire message"
    );
    assert!(!commands[2].contains('<'));
    if !peek {
        assert!(!commands[1].contains("PEEK"));
        assert!(!commands[2].contains("PEEK"));
    }
}

#[tokio::test]
async fn appended_crlf_on_a_large_range_is_replaced_by_an_independent_full_message() {
    // Distinct bytes around the boundary catch both accidental CRLF insertion
    // and the corresponding two-byte skip on the next range.
    let raw = format!(
        "Subject: range compatibility\r\n\r\n{}\r\n",
        "0123456789abcdefghijklmnopqrstuvwxyz".repeat(3000)
    );
    let declared = raw.len() - 2;
    let partial = format!("{}\r\n", &raw[..CHUNK_BYTES]);
    let (mut s, commands) = session(vec![
        (response(7, declared, Some(0), &partial), Some("OK range")),
        (response(7, declared, None, &raw), Some("OK complete")),
    ])
    .await;
    let checkpoints = Mutex::new(Vec::new());
    let (downloaded, date) = fetch(&mut s, 7, true, None, |total, date, offset, bytes| {
        checkpoints
            .lock()
            .unwrap()
            .push((total, date, offset, bytes));
        async {}
    })
    .await
    .unwrap();
    assert_eq!(downloaded, raw.as_bytes());
    assert_eq!(date, 1_735_787_045_000);
    assert!(checkpoints.lock().unwrap().is_empty());
    assert_full_fallback(&commands, true);
}

#[tokio::test]
async fn small_message_fallback_preserves_the_complete_responses_trailing_crlf() {
    let (mut s, commands) = session(vec![
        (response(7, 4, Some(0), "mail\r\n"), Some("OK range")),
        (response(7, 4, None, "mail\r\n"), Some("OK complete")),
    ])
    .await;
    let (raw, _) = fetch(&mut s, 7, true, None, |_, _, _, _| async {})
        .await
        .unwrap();
    assert_eq!(raw, b"mail\r\n", "do not trim even a genuine trailing CRLF");
    assert_full_fallback(&commands, true);
}

#[tokio::test]
async fn short_range_uses_the_confirmed_full_response_instead_of_trusting_size_metadata() {
    // A full response's declared size may be too large, not just two bytes too
    // small. The complete literal and tagged OK establish the returned bytes.
    let (mut s, commands) = session(vec![
        (response(7, 10, Some(0), "mail"), Some("OK range")),
        (response(7, 10, None, "mail"), Some("OK complete")),
    ])
    .await;
    let checkpoints = Mutex::new(0);
    let (raw, _) = fetch(&mut s, 7, true, None, |_, _, _, _| {
        *checkpoints.lock().unwrap() += 1;
        async {}
    })
    .await
    .unwrap();
    assert_eq!(raw, b"mail");
    assert_eq!(*checkpoints.lock().unwrap(), 0);
    assert_full_fallback(&commands, true);
}

#[tokio::test]
async fn mismatched_resumed_range_restarts_with_a_full_response_without_joining_old_bytes() {
    let raw = "headtail\r\n";
    let (mut s, commands) = session(vec![
        (response(7, 8, Some(4), "tail\r\n"), Some("OK range")),
        (response(7, 8, None, raw), Some("OK complete")),
    ])
    .await;
    let checkpoints = Mutex::new(0);
    let resume = Resume {
        raw: b"head".to_vec(),
        date: 42,
        total: 8,
    };
    let (downloaded, date) = fetch(&mut s, 7, true, Some(resume), |_, _, _, _| {
        *checkpoints.lock().unwrap() += 1;
        async {}
    })
    .await
    .unwrap();
    assert_eq!(downloaded, raw.as_bytes());
    assert_eq!(
        date, 1_735_787_045_000,
        "the new full response supplies its date"
    );
    assert_eq!(*checkpoints.lock().unwrap(), 0);
    assert!(commands.lock().unwrap()[1].contains("<4.65536>"));
    assert_full_fallback(&commands, true);
}

#[tokio::test]
async fn invalid_full_fallbacks_never_publish_bytes_or_retry_again() {
    let full = response(7, 4, None, "mail\r\n");
    let cases = [
        ("refused", full.clone(), Some("NO denied")),
        ("unsupported", full.clone(), Some("BAD unsupported")),
        ("unconfirmed", full.clone(), None),
        (
            "wrong uid",
            response(8, 4, None, "mail\r\n"),
            Some("OK full"),
        ),
        (
            "still partial",
            response(7, 4, Some(0), "mail\r\n"),
            Some("OK full"),
        ),
        ("duplicate body", format!("{full}{full}"), Some("OK full")),
        (
            "conflicting metadata",
            format!("* 1 FETCH (UID 7 RFC822.SIZE 6)\r\n{full}"),
            Some("OK full"),
        ),
        (
            "missing body",
            "* 1 FETCH (UID 7 RFC822.SIZE 4)\r\n".into(),
            Some("OK full"),
        ),
        (
            "truncated literal",
            "* 1 FETCH (UID 7 RFC822.SIZE 4 BODY[] {6}\r\nmail".into(),
            None,
        ),
    ];
    for (label, reply, completion) in cases {
        let (mut s, commands) = session(vec![
            (response(7, 4, Some(0), "mail\r\n"), Some("OK range")),
            (reply, completion),
        ])
        .await;
        let checkpoints = Mutex::new(0);
        let result = fetch(&mut s, 7, true, None, |_, _, _, _| {
            *checkpoints.lock().unwrap() += 1;
            async {}
        })
        .await;
        assert!(result.is_err(), "{label} must be rejected");
        assert_eq!(*checkpoints.lock().unwrap(), 0, "{label}");
        assert_full_fallback(&commands, true);
    }
}

#[tokio::test]
async fn background_mismatches_do_not_start_an_unbounded_full_fetch() {
    for (declared, bytes) in [(4, "mail\r\n"), (5, "mail")] {
        let (mut s, commands) = session(vec![(
            response(7, declared, Some(0), bytes),
            Some("OK range"),
        )])
        .await;
        let checkpoints = Mutex::new(0);
        let result = fetch_controlled(
            &mut s,
            7,
            true,
            None,
            Some(32 * 1024 * 1024),
            |_, _, _, _| {
                *checkpoints.lock().unwrap() += 1;
                async { Ok(()) }
            },
        )
        .await;
        assert!(result.is_err());
        assert_eq!(*checkpoints.lock().unwrap(), 0);
        assert_eq!(commands.lock().unwrap().len(), 2, "login and one range");
    }
}

#[tokio::test]
async fn fallback_preserves_non_peek_fetch_semantics() {
    let (mut s, commands) = session(vec![
        (response(7, 4, Some(0), "mail\r\n"), Some("OK range")),
        (response(7, 4, None, "mail\r\n"), Some("OK complete")),
    ])
    .await;
    let (raw, _) = fetch(&mut s, 7, false, None, |_, _, _, _| async {})
        .await
        .unwrap();
    assert_eq!(raw, b"mail\r\n");
    assert_full_fallback(&commands, false);
}

#[tokio::test]
async fn unsupported_partial_fetch_can_recover_with_a_size_mismatched_full_response() {
    for status in ["NO partial unsupported", "BAD partial unsupported"] {
        let (mut s, commands) = session(vec![
            (String::new(), Some(status)),
            (response(7, 4, None, "mail\r\n"), Some("OK complete")),
        ])
        .await;
        let (raw, _) = fetch(&mut s, 7, true, None, |_, _, _, _| async {})
            .await
            .unwrap();
        assert_eq!(raw, b"mail\r\n");
        assert_full_fallback(&commands, true);
    }
}

#[tokio::test]
async fn unsolicited_whole_body_with_size_mismatch_is_rechecked_by_an_explicit_full_fetch() {
    let (mut s, commands) = session(vec![
        (response(7, 4, None, "mail\r\n"), Some("OK initial")),
        (response(7, 4, None, "mail\r\n"), Some("OK complete")),
    ])
    .await;
    let (raw, _) = fetch(&mut s, 7, true, None, |_, _, _, _| async {})
        .await
        .unwrap();
    assert_eq!(raw, b"mail\r\n");
    assert_full_fallback(&commands, true);
}
