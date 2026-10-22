use super::*;
fn fixture(
    dir: &std::path::Path,
) -> (
    config::Store,
    credentials::Store,
    crate::compose_recovery::ComposeRecovery,
    send_records::Store,
    String,
) {
    let credentials = credentials::Store::open(dir).unwrap();
    let mut accounts = config::Store::load(dir.into()).unwrap();
    let account:Account=serde_json::from_value(serde_json::json!({"id":"a","kind":"custom","email":"me@example.test","name":"Me","imapHost":"imap.example.test","imapPort":993,"smtpHost":"smtp.example.test","smtpPort":465})).unwrap();
    accounts
        .add_with_password(account, "test-secret", &credentials)
        .unwrap();
    let recovery = crate::compose_recovery::ComposeRecovery::new(dir.join("compose-recovery"));
    recovery
        .save(
            "editing",
            1,
            serde_json::json!({"accountId":"a","subject":"draft"}),
            serde_json::json!({"html":"body"}),
            vec![],
        )
        .unwrap();
    let sends = send_records::Store::open(dir).unwrap();
    sends
        .enqueue_buffer(
            NewRecord {
                id: "buffer".into(),
                account_id: "a".into(),
                from: "me@example.test".into(),
                subject: "pending".into(),
                recipients: vec!["you@example.test".into()],
                message_id: "<buffer@example.test>".into(),
                endpoint: "source".into(),
                scheduled_id: None,
                previous_id: None,
            },
            b"send raw",
            b"draft raw",
            buffer::BufferInput {
                request_id: "request".into(),
                payload_hash: "payload".into(),
                compose_session_id: "queued".into(),
                compose_revision: 1,
                deadline: 9000,
                sender_fingerprint: "smtp".into(),
                receive_epoch: 1,
                followup_seconds: None,
                request_receipt: false,
            },
        )
        .unwrap();
    let task = sends
        .set_snooze(
            mail_ref::MessageRef {
                account_id: "a".into(),
                receive_epoch: 1,
                folder: "INBOX".into(),
                uid_validity: 7,
                uid: 4,
            },
            9000,
            None,
            None,
            1000,
            vec!["source@x".into()],
        )
        .unwrap();
    (accounts, credentials, recovery, sends, task.id)
}
#[test]
fn account_removal_commit_failures_keep_drafts_buffers_and_reminders_live() {
    for credential_failure in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let (mut accounts, credentials, recovery, sends, id) = fixture(d.path());
        if credential_failure {
            rusqlite::Connection::open(d.path().join("local.db")).unwrap().execute_batch("CREATE TRIGGER refuse_credentials_delete BEFORE DELETE ON account_credentials BEGIN SELECT RAISE(ABORT,'blocked'); END;").unwrap();
        } else {
            std::fs::create_dir(d.path().join("accounts.json.tmp")).unwrap();
        }
        assert!(
            retire_account(
                "a",
                1,
                || accounts.remove_with_password("a", &credentials),
                &recovery,
                Some(&sends)
            )
            .is_err()
        );
        assert!(accounts.get("a").is_some());
        assert!(credentials.load("a").unwrap().is_some());
        assert!(
            recovery.restore("editing").is_ok(),
            "failed removal must preserve editable recovery"
        );
        assert_eq!(
            sends.get("buffer").unwrap().unwrap().status,
            Status::Buffered
        );
        assert_eq!(
            sends.get_reminder(&id).unwrap().unwrap().state,
            reminders::ReminderState::Scheduled
        );
    }
}
#[test]
fn account_removal_cleanup_failure_does_not_rollback_a_successful_removal() {
    let d = tempfile::tempdir().unwrap();
    let (mut accounts, credentials, recovery, sends, id) = fixture(d.path());
    rusqlite::Connection::open(d.path().join("local.db")).unwrap().execute_batch("CREATE TRIGGER refuse_buffer_cleanup BEFORE UPDATE ON send_records BEGIN SELECT RAISE(ABORT,'blocked'); END;").unwrap();
    let warnings = retire_account(
        "a",
        1,
        || accounts.remove_with_password("a", &credentials),
        &recovery,
        Some(&sends),
    )
    .expect("committed removal must remain successful");
    assert!(!warnings.is_empty());
    assert!(accounts.get("a").is_none());
    assert!(credentials.load("a").unwrap().is_none());
    assert!(recovery.restore("editing").is_err());
    assert_eq!(
        sends.get_reminder(&id).unwrap().unwrap().state,
        reminders::ReminderState::Cancelled,
        "other cleanup still runs after one component fails"
    );
}
