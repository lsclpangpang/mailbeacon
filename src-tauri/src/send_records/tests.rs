use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn input(id: &str) -> NewRecord {
    NewRecord {
        id: id.into(),
        account_id: "a".into(),
        from: "me@example.com".into(),
        subject: "报告".into(),
        recipients: vec!["you@example.com".into()],
        message_id: "<original@example.com>".into(),
        endpoint: "imap.example.com:993:me@example.com".into(),
        scheduled_id: None,
        previous_id: None,
    }
}
const RAW: &[u8] = b"From: me@example.com\r\nTo: you@example.com\r\nMessage-ID: <original@example.com>\r\nSubject: report\r\n\r\nbody\r\n";

#[test]
fn orphaned_spool_cannot_be_overwritten_or_create_a_mismatched_record() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.persist_file("orphan", "eml", RAW).unwrap();
    assert!(
        store
            .prepare(input("orphan"), b"different content")
            .is_err()
    );
    assert_eq!(std::fs::read(store.dir.join("orphan.eml")).unwrap(), RAW);
    assert!(store.get("orphan").unwrap().is_none());
    store.prepare(input("orphan"), RAW).unwrap();
    assert_eq!(store.raw("orphan").unwrap(), RAW);
    #[cfg(windows)]
    {
        crate::private_fs::assert_private_acl(&store.dir);
        crate::private_fs::assert_private_acl(&store.dir.join("orphan.eml"));
    }
}

#[tokio::test]
async fn accepted_timestamp_is_persistent_and_copy_updates_cannot_change_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("accepted-clock"), RAW).unwrap();
    let record = execute(&store, "accepted-clock", |_| async { Ok(()) })
        .await
        .unwrap();
    let accepted = serde_json::to_value(&record).unwrap()["acceptedAt"].as_i64();
    assert!(
        accepted.is_some_and(|at| at > 0),
        "SMTP acceptance needs its own immutable persisted timestamp"
    );
    store
        .copy_result(&record.id, Err("offline".into()))
        .unwrap();
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(
        serde_json::to_value(reopened.get(&record.id).unwrap().unwrap()).unwrap()["acceptedAt"]
            .as_i64(),
        accepted
    );
}

#[test]
fn persistent_raw_and_record_do_not_change_shared_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
    db.pragma_update(None, "user_version", 3).unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.get("s1").unwrap().unwrap().status, Status::Prepared);
    assert_eq!(store.raw("s1").unwrap(), RAW);
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn identity_cannot_be_reused_with_different_content_and_raw_is_verified() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    assert!(store.prepare(input("s1"), b"different").is_err());
    std::fs::write(dir.path().join("send-records/s1.eml"), b"damaged").unwrap();
    assert!(store.raw("s1").is_err());
    assert!(store.prepare(input("../escape"), RAW).is_err());
}

#[tokio::test]
async fn acceptance_is_persistent_and_repeat_submission_never_calls_transport() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    let calls = AtomicUsize::new(0);
    for _ in 0..2 {
        let result = execute(&store, "s1", |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(result.status, Status::Accepted);
    }
    store
        .copy_result("s1", Err("没有已发送文件夹".into()))
        .unwrap();
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    let record = store.get("s1").unwrap().unwrap();
    assert_eq!(record.status, Status::Accepted);
    assert_eq!(record.copy_status, CopyStatus::Failed);
    assert_eq!(store.pending_copies().unwrap().len(), 1);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(store.raw("s1").unwrap(), RAW);
}

#[tokio::test]
async fn interrupted_and_ambiguous_submissions_are_never_automatically_resent() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("interrupted"), RAW).unwrap();
    assert!(store.begin("interrupted").unwrap());
    store.prepare(input("lost-reply"), RAW).unwrap();
    let record = execute(&store, "lost-reply", |_| async {
        Err(SendError::Unknown("连接中断".into()))
    })
    .await
    .unwrap();
    assert_eq!(record.status, Status::Unknown);
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    for id in ["interrupted", "lost-reply"] {
        let record = execute(&store, id, |_| async { panic!("unknown must not send") })
            .await
            .unwrap();
        assert_eq!(record.status, Status::Unknown);
    }
    assert!(store.pending_copies().unwrap().is_empty());
}

#[tokio::test]
async fn damaged_raw_and_write_failure_prevent_any_transport_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    std::fs::remove_file(dir.path().join("send-records/s1.eml")).unwrap();
    assert!(
        execute(&store, "s1", |_| async {
            panic!("missing raw must not send")
        })
        .await
        .is_err()
    );
    let blocked = dir.path().join("blocked");
    std::fs::write(&blocked, b"not a directory").unwrap();
    assert!(Store::open(&blocked).is_err());
}

#[tokio::test]
async fn accepted_but_bookkeeping_failure_stays_unknown_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_accept BEFORE UPDATE ON send_records WHEN json_extract(NEW.record, '$.status')='accepted' BEGIN SELECT RAISE(ABORT,'disk failed'); END;").unwrap();
    assert!(execute(&store, "s1", |_| async { Ok(()) }).await.is_err());
    drop(store);
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.get("s1").unwrap().unwrap().status, Status::Unknown);
    assert_eq!(store.raw("s1").unwrap(), RAW);
}

#[test]
fn sent_copy_is_never_written_for_unconfirmed_smtp_submission() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    assert!(store.copy_result("s1", Ok(())).is_err());
}

#[test]
fn explicit_resend_has_a_new_message_id_and_preserves_binary_body() {
    let raw = b"Message-ID: <old@x>\r\n\told-fold\r\nSubject: test\r\n\r\n\xff\x00\xfe";
    let rewritten = crate::smtp::with_message_id(raw, "<new@x>");
    assert!(rewritten.starts_with(b"Message-ID: <new@x>\r\nSubject: test\r\n"));
    assert!(rewritten.ends_with(b"\r\n\r\n\xff\x00\xfe"));
}

#[tokio::test]
async fn explicit_new_attempt_prevents_a_prepared_original_from_later_sending() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("original"), RAW).unwrap();
    let mut next = input("new-attempt");
    next.previous_id = Some("original".into());
    store.prepare(next, RAW).unwrap();
    let result = execute(&store, "original", |_| async {
        panic!("superseded draft must not send")
    })
    .await
    .unwrap();
    assert_eq!(result.status, Status::Rejected);
}

#[tokio::test]
async fn concurrent_submissions_call_transport_once() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.prepare(input("s1"), RAW).unwrap();
    let calls = AtomicUsize::new(0);
    let (first, second) = tokio::join!(
        execute(&store, "s1", |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            tokio::task::yield_now().await;
            Ok(())
        }),
        execute(&store, "s1", |_| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
    );
    assert_eq!(first.unwrap().status, Status::Accepted);
    assert!(matches!(
        second.unwrap().status,
        Status::Submitting | Status::Accepted
    ));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn legacy_interrupted_schedule_remains_unknown_even_when_rescheduled() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let outbox = crate::outbox::Outbox::new(dir.path().join("outbox"));
    let item = outbox
        .add(
            crate::outbox::Item {
                account_id: "a".into(),
                sending: true,
                message_id: "<original@example.com>".into(),
                recipients: vec!["you@example.com".into()],
                ..Default::default()
            },
            RAW,
            RAW,
        )
        .unwrap();
    let account: crate::config::Account = serde_json::from_value(serde_json::json!({"id":"a","kind":"custom","email":"me@example.com","name":"Me","imapHost":"imap.example.com","imapPort":993,"smtpHost":"smtp.example.com","smtpPort":465})).unwrap();
    // 导入未知记录必须在 INSERT 时就是 unknown，不能先 prepared 再 UPDATE。
    store.conn.lock().unwrap().execute_batch("CREATE TRIGGER stop_updates BEFORE UPDATE ON send_records BEGIN SELECT RAISE(FAIL, 'interrupted before update'); END;").unwrap();
    crate::sending::recover_outbox(&store, &outbox, &[account]).unwrap();
    outbox
        .modify("a", item.id, |i| {
            i.attempts = 0;
            i.send_at = 0;
        })
        .unwrap();
    let records = store.list(None).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].status, Status::Unknown);
    assert!(!store.begin(&records[0].id).unwrap());
}

#[test]
fn changing_schedule_cannot_erase_an_uncertain_submission_even_if_import_failed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let outbox = crate::outbox::Outbox::new(dir.path().join("outbox"));
    let item = outbox
        .add(
            crate::outbox::Item {
                account_id: "a".into(),
                recipients: vec!["you@example.com".into()],
                ..Default::default()
            },
            RAW,
            RAW,
        )
        .unwrap();
    assert!(crate::sending::ensure_reschedulable(&store, &item).is_ok());
    let sending = outbox.modify("a", item.id, |i| i.sending = true).unwrap();
    assert!(crate::sending::ensure_reschedulable(&store, &sending).is_err());
    std::fs::remove_file(dir.path().join(format!("outbox/a/{}.eml", item.id))).unwrap();
    outbox.recover_interrupted().unwrap();
    let interrupted = outbox.get("a", item.id).unwrap();
    assert!(crate::sending::ensure_reschedulable(&store, &interrupted).is_err());
}
