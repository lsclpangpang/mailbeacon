use super::buffer::BufferInput;
use super::*;
fn input(id: &str) -> NewRecord {
    NewRecord {
        id: id.into(),
        account_id: "a".into(),
        from: "me@example.com".into(),
        subject: "s".into(),
        recipients: vec!["you@example.com".into()],
        message_id: format!("<{id}@example.com>"),
        endpoint: "receive-a".into(),
        scheduled_id: None,
        previous_id: None,
    }
}
fn meta(id: &str) -> BufferInput {
    BufferInput {
        request_id: id.into(),
        payload_hash: "payload".into(),
        compose_session_id: "session".into(),
        compose_revision: 3,
        deadline: 2000,
        sender_fingerprint: "smtp-a".into(),
        receive_epoch: 1,
        followup_seconds: Some(60),
        request_receipt: true,
    }
}
const RAW: &[u8] = b"From: me@example.com\r\nTo: you@example.com\r\nSubject: hi\r\n\r\nsent";
const DRAFT:&[u8]=b"From: me@example.com\r\nBcc: hidden@example.com\r\nDisposition-Notification-To: me@example.com\r\n\r\ndraft";
#[test]
fn send_buffer_deadline_cancel_and_old_begin_have_one_authority() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .enqueue_buffer(input("b1"), RAW, DRAFT, meta("request1"))
        .unwrap();
    assert_eq!(r.status, Status::Buffered);
    assert!(!s.begin(&r.id).unwrap());
    assert!(
        s.claim_buffered(&r.id, 1999, false, "smtp-a", None)
            .unwrap()
            .is_none()
    );
    let cancelled = s.cancel_buffered(&r.id).unwrap();
    assert_eq!(cancelled.status, Status::Cancelled);
    assert!(
        s.claim_buffered(&r.id, 2000, true, "smtp-a", Some(cancelled.version))
            .unwrap()
            .is_none()
    );
    assert_eq!(s.buffer_draft(&r.id).unwrap(), DRAFT);
    assert_eq!(s.cancel_buffered(&r.id).unwrap().version, cancelled.version);
}
#[test]
fn send_buffer_identity_and_restart_are_fail_closed() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    assert!(s.claim_buffered("b1", 2000, false, "smtp-b", None).is_err());
    assert_eq!(s.get("b1").unwrap().unwrap().status, Status::Paused);
    let mut second = meta("r2");
    second.compose_session_id = "session2".into();
    s.enqueue_buffer(input("b2"), RAW, DRAFT, second).unwrap();
    drop(s);
    let s = Store::open(d.path()).unwrap();
    assert_eq!(s.get("b2").unwrap().unwrap().status, Status::Paused);
    assert!(
        s.claim_buffered("b2", 9999, false, "smtp-a", None)
            .unwrap()
            .is_none()
    );
}
#[test]
fn send_buffer_publish_is_transactional_and_repeated_payload_conflicts() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let db = Connection::open(d.path().join("local.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_buffer BEFORE INSERT ON send_buffer BEGIN SELECT RAISE(ABORT,'full'); END;").unwrap();
    assert!(
        s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
            .is_err()
    );
    assert!(s.get("b1").unwrap().is_none());
    db.execute_batch("DROP TRIGGER fail_buffer").unwrap();
    let r = s
        .enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    assert_eq!(
        s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
            .unwrap()
            .id,
        r.id
    );
    let mut m = meta("r1");
    m.payload_hash = "other".into();
    assert!(s.enqueue_buffer(input("b1"), RAW, DRAFT, m).is_err());
}
#[tokio::test]
async fn send_buffer_acceptance_and_reminder_are_atomic_and_timestamp_immutable() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    let claim = s
        .claim_buffered("b1", 2000, false, "smtp-a", None)
        .unwrap()
        .unwrap();
    assert!(s.cancel_buffered("b1").is_err());
    let r = buffer::execute_claimed(&s, claim, || 3000, |_| async { Ok(()) })
        .await
        .unwrap();
    assert_eq!(r.accepted_at, Some(3000));
    let reminders = s.list_reminders(None).unwrap();
    assert_eq!(reminders.len(), 1);
    assert_eq!(reminders[0].due_at, 63000);
    s.copy_result("b1", Err("offline".into())).unwrap();
    drop(s);
    let s = Store::open(d.path()).unwrap();
    assert_eq!(s.get("b1").unwrap().unwrap().accepted_at, Some(3000));
    assert_eq!(s.list_reminders(None).unwrap().len(), 1);
}
#[tokio::test]
async fn send_buffer_failed_acceptance_transaction_never_leaves_an_active_reminder() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    let db = Connection::open(d.path().join("local.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_reminder BEFORE INSERT ON mail_reminders BEGIN SELECT RAISE(ABORT,'full'); END;").unwrap();
    let claim = s
        .claim_buffered("b1", 2000, false, "smtp-a", None)
        .unwrap()
        .unwrap();
    assert!(
        buffer::execute_claimed(&s, claim, || 3000, |_| async { Ok(()) })
            .await
            .is_err()
    );
    assert_eq!(s.get("b1").unwrap().unwrap().status, Status::Unknown);
    assert!(s.list_reminders(None).unwrap().is_empty());
}
#[test]
fn send_buffer_concurrent_cancel_and_claim_never_both_win() {
    use std::sync::{Arc, Barrier};
    for n in 0..24 {
        let d = tempfile::tempdir().unwrap();
        let s = Arc::new(Store::open(d.path()).unwrap());
        let id = format!("b{n}");
        s.enqueue_buffer(input(&id), RAW, DRAFT, meta(&id)).unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let (s2, b2, id2) = (s.clone(), barrier.clone(), id.clone());
        let t = std::thread::spawn(move || {
            b2.wait();
            s2.cancel_buffered(&id2).is_ok()
        });
        barrier.wait();
        let claimed = s
            .claim_buffered(&id, 2000, false, "smtp-a", None)
            .unwrap()
            .is_some();
        assert_ne!(claimed, t.join().unwrap());
    }
}
#[test]
fn send_buffer_tampered_draft_is_never_restored() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    std::fs::write(d.path().join("send-records/b1.draft"), b"other").unwrap();
    assert!(s.buffer_draft("b1").is_err());
}
#[test]
fn send_buffer_account_removal_cancels_unsubmitted_work_only() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    s.cancel_account_buffers("a").unwrap();
    assert_eq!(s.get("b1").unwrap().unwrap().status, Status::Cancelled);
    assert!(
        s.claim_buffered("b1", 2000, true, "smtp-a", None)
            .unwrap()
            .is_none()
    );
}
#[tokio::test]
async fn send_buffer_manual_retry_keeps_followup_intent_with_new_submission_identity() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    let claim = s
        .claim_buffered("b1", 2000, false, "smtp-a", None)
        .unwrap()
        .unwrap();
    buffer::execute_claimed(
        &s,
        claim,
        || 3000,
        |_| async { Err(SendError::Rejected("rejected".into())) },
    )
    .await
    .unwrap();
    s.prepare_resend("b1", input("b2"), RAW, "smtp-a", 1)
        .unwrap();
    execute(&s, "b2", |_| async { Ok(()) }).await.unwrap();
    let reminders = s.list_reminders(None).unwrap();
    assert_eq!(reminders.len(), 1);
    assert_eq!(reminders[0].send_record_id.as_deref(), Some("b2"));
}

#[test]
fn send_buffer_session_remains_owned_after_cancel_and_reopen() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    assert!(!s.owns_compose_session("session").unwrap());
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    assert!(s.owns_compose_session("session").unwrap());
    s.cancel_buffered("b1").unwrap();
    drop(s);
    let s = Store::open(d.path()).unwrap();
    assert!(s.owns_compose_session("session").unwrap());
    assert!(!s.owns_compose_session("different").unwrap());
}

#[test]
fn productivity_buffer_same_session_cannot_publish_a_second_request() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
        .unwrap();
    assert!(
        s.enqueue_buffer(input("b2"), RAW, DRAFT, meta("r2"))
            .is_err()
    );
    assert_eq!(s.list(None).unwrap().len(), 1);
}
#[test]
fn productivity_buffer_failed_publish_can_retry_regenerated_mime() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let db = Connection::open(d.path().join("local.db")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_buffer BEFORE INSERT ON send_buffer BEGIN SELECT RAISE(ABORT,'full'); END;").unwrap();
    assert!(
        s.enqueue_buffer(input("b1"), RAW, DRAFT, meta("r1"))
            .is_err()
    );
    db.execute_batch("DROP TRIGGER fail_buffer").unwrap();
    let new_raw = b"Message-ID: <regenerated@x>\r\n\r\nnew boundary";
    s.enqueue_buffer(input("b1"), new_raw, b"new draft boundary", meta("r1"))
        .unwrap();
    assert_eq!(s.raw("b1").unwrap(), new_raw);
}

#[test]
fn send_buffer_active_sends_include_old_paused_and_submitting_beyond_history_page() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.enqueue_buffer(input("old"), RAW, DRAFT, meta("r1"))
        .unwrap();
    for n in 0..101 {
        s.prepare(input(&format!("new-{n}")), RAW).unwrap();
    }
    drop(s);
    let s = Store::open(d.path()).unwrap();
    let active = s.active_sends().unwrap();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].id, "old");
    assert_eq!(active[0].status, Status::Paused);
    s.claim_buffered("old", 3000, true, "smtp-a", Some(active[0].version))
        .unwrap()
        .unwrap();
    assert_eq!(s.active_sends().unwrap()[0].status, Status::Submitting);
    s.finish_submission("old", Ok(()), 3001).unwrap();
    assert!(s.active_sends().unwrap().is_empty());
}
