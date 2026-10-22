use super::reminders::*;
use super::*;
fn source(epoch: u64, uid: u32) -> crate::mail_ref::MessageRef {
    crate::mail_ref::MessageRef {
        account_id: "a".into(),
        receive_epoch: epoch,
        folder: "INBOX".into(),
        uid_validity: 7,
        uid,
    }
}
#[test]
fn mail_reminders_snooze_is_persistent_versioned_and_notification_claim_is_once() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(
            source(1, 4),
            2000,
            None,
            None,
            1000,
            vec!["anchor@x".into()],
        )
        .unwrap();
    let changed = s
        .change_reminder(&r.id, r.version, "reschedule", Some(4000), 1001)
        .unwrap();
    assert!(
        s.change_reminder(&r.id, r.version, "cancel", None, 1002)
            .is_err()
    );
    assert!(s.tick_reminders(2000).unwrap().is_empty());
    assert_eq!(s.tick_reminders(4000).unwrap().len(), 1);
    assert!(s.tick_reminders(4001).unwrap().is_empty());
    drop(s);
    let s = Store::open(d.path()).unwrap();
    assert_eq!(s.list_reminders(None).unwrap()[0].state, ReminderState::Due);
    assert!(s.tick_reminders(5000).unwrap().is_empty());
    assert!(
        s.change_reminder(&r.id, changed.version, "cancel", None, 5001)
            .is_err()
    );
}
#[test]
fn mail_reminders_precise_anchor_excludes_self_and_automatic_responses() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(
            source(1, 4),
            5000,
            None,
            None,
            1000,
            vec!["anchor@x".into()],
        )
        .unwrap();
    for (n, (refs, from, automatic, epoch)) in [
        ("root@x", "you@x", false, 1),
        ("xanchor@x", "you@x", false, 1),
        ("anchor@x", "me@x", false, 1),
        ("anchor@x", "you@x", true, 1),
        ("anchor@x", "you@x", false, 2),
    ]
    .into_iter()
    .enumerate()
    {
        s.observe_reply(
            &ReplyEvidence {
                source: source(epoch, 8 + n as u32),
                message_id: "reply@x".into(),
                references: vec![refs.into()],
                sender: from.into(),
                automatic,
                observed_at: 2000,
                received_at: Some(2000),
            },
            &["me@x".into()],
        )
        .unwrap();
        assert_eq!(
            s.get_reminder(&r.id).unwrap().unwrap().state,
            ReminderState::Scheduled
        );
    }
    s.observe_reply(
        &ReplyEvidence {
            source: source(1, 99),
            message_id: "reply@x".into(),
            references: vec!["anchor@x".into()],
            sender: "you@x".into(),
            automatic: false,
            observed_at: 2000,
            received_at: Some(2000),
        },
        &["me@x".into()],
    )
    .unwrap();
    assert_eq!(
        s.get_reminder(&r.id).unwrap().unwrap().state,
        ReminderState::Due
    );
}
#[test]
fn mail_reminders_coverage_requires_post_deadline_round_and_all_folders() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(source(1, 4), 2000, None, None, 1000, vec![])
        .unwrap();
    let mut r = r;
    r.kind = ReminderKind::Followup;
    r.state = ReminderState::Checking;
    r.source = None;
    r.send_record_id = Some("sent".into());
    {
        let c = s.conn.lock().unwrap();
        write_reminder(&c, &r).unwrap();
    }
    let mut c = FolderCoverage {
        folder: "INBOX".into(),
        uid_validity: 7,
        scanned_uid: 9,
        upper_uid: 9,
        completed_at: 3000,
        exhausted: true,
        error: None,
    };
    assert!(
        s.finish_reminder_round(
            &r.id,
            r.version,
            1999,
            1,
            vec!["INBOX".into()],
            vec![c.clone()],
            3000
        )
        .is_err()
    );
    assert!(
        s.finish_reminder_round(
            &r.id,
            r.version,
            2001,
            1,
            vec!["INBOX".into(), "Archive".into()],
            vec![c.clone()],
            3000
        )
        .is_err()
    );
    c.exhausted = false;
    assert!(
        s.finish_reminder_round(
            &r.id,
            r.version,
            2001,
            1,
            vec!["INBOX".into()],
            vec![c],
            3000
        )
        .is_err()
    );
}
#[test]
fn mail_reminders_removed_account_and_old_epoch_cannot_resurrect() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(source(1, 4), 2000, None, None, 1000, vec![])
        .unwrap();
    s.invalidate_reminders("a", 1, false).unwrap();
    assert_eq!(
        s.get_reminder(&r.id).unwrap().unwrap().state,
        ReminderState::Orphaned
    );
    assert!(s.tick_reminders(9000).unwrap().is_empty());
}
#[tokio::test]
async fn mail_reminders_real_header_scan_excludes_receipts_and_detects_precise_reply() {
    let server = crate::test_imap::state("", &[("INBOX", 7), ("Archive", 9)]);
    {
        let mut server = server.lock().unwrap();
        server.add("INBOX","From: person@example.com\r\nMessage-ID: <reply@x>\r\nIn-Reply-To: <anchor@x>\r\nSubject: Re\r\n\r\nhi",&[]);
        server.add("Archive","From: person@example.com\r\nMessage-ID: <receipt@x>\r\nReferences: <anchor@x>\r\nAuto-Submitted: auto-replied\r\n\r\nreceipt",&[]);
    }
    let mut session = crate::test_imap::session(server.clone()).await;
    let mut found = vec![];
    let round = scan_reply_round(
        &mut session,
        "a",
        1,
        || false,
        |_, _| Ok(std::collections::HashSet::new()),
        |_, _, _| Ok(()),
        |e| {
            found.push(e);
            Ok(())
        },
    )
    .await
    .unwrap();
    assert_eq!(round.coverage.len(), 2);
    assert_eq!(found.len(), 2);
    assert!(
        found
            .iter()
            .any(|e| e.message_id == "reply@x" && !e.automatic)
    );
    assert!(
        found
            .iter()
            .any(|e| e.message_id == "receipt@x" && e.automatic)
    );
    assert!(
        server
            .lock()
            .unwrap()
            .commands
            .iter()
            .all(|c| !c.starts_with("UID STORE") && !c.starts_with("UID MOVE"))
    );
}
#[tokio::test]
async fn mail_reminders_cancelled_header_scan_never_claims_complete_coverage() {
    let server = crate::test_imap::state("", &[("INBOX", 7)]);
    let mut session = crate::test_imap::session(server).await;
    assert!(
        scan_reply_round(
            &mut session,
            "a",
            1,
            || true,
            |_, _| Ok(std::collections::HashSet::new()),
            |_, _, _| Ok(()),
            |_| Ok(())
        )
        .await
        .is_err()
    );
}
#[test]
fn mail_reminders_repeated_reply_is_idempotent_and_early_restore_does_not_renotify() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    s.set_snooze(source(1, 4), 5000, None, None, 1000, vec!["a@x".into()])
        .unwrap();
    let e = ReplyEvidence {
        source: source(1, 8),
        message_id: "r@x".into(),
        references: vec!["a@x".into()],
        sender: "other@x".into(),
        automatic: false,
        observed_at: 2000,
        received_at: Some(2000),
    };
    assert_eq!(s.observe_reply(&e, &[]).unwrap().len(), 1);
    assert!(
        s.observe_reply(&e, &[]).unwrap().is_empty(),
        "duplicate sync must not update a second time"
    );
    assert!(
        s.tick_reminders(5000).unwrap().is_empty(),
        "early restore already notified by reply event"
    );
}

#[test]
fn mail_reminders_historical_reply_discovered_late_does_not_wake_snooze() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(source(1, 4), 5000, None, None, 1000, vec!["a@x".into()])
        .unwrap();
    let e = ReplyEvidence {
        source: source(1, 8),
        message_id: "old@x".into(),
        references: vec!["a@x".into()],
        sender: "other@x".into(),
        automatic: false,
        observed_at: 2000,
        received_at: Some(500),
    };
    assert!(s.observe_reply(&e, &[]).unwrap().is_empty());
    assert_eq!(
        s.get_reminder(&r.id).unwrap().unwrap().state,
        ReminderState::Scheduled
    );
}

#[test]
fn mail_reminders_source_generation_or_deleted_uid_never_rebinds() {
    for (validity, live) in [(8, vec![4]), (7, vec![5])] {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path()).unwrap();
        let r = s
            .set_snooze(source(1, 4), 5000, None, None, 1000, vec![])
            .unwrap();
        assert_eq!(
            s.reconcile_snooze_sources("a", 1, "INBOX", validity, &live)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.get_reminder(&r.id).unwrap().unwrap().state,
            ReminderState::Orphaned
        );
        assert!(s.tick_reminders(9000).unwrap().is_empty());
    }
}
#[test]
fn mail_reminders_removed_account_cannot_be_rescheduled() {
    let d = tempfile::tempdir().unwrap();
    let s = Store::open(d.path()).unwrap();
    let r = s
        .set_snooze(source(1, 4), 5000, None, None, 1000, vec![])
        .unwrap();
    s.invalidate_reminders("a", 1, true).unwrap();
    let r = s.get_reminder(&r.id).unwrap().unwrap();
    assert!(
        s.change_reminder(&r.id, r.version, "reschedule", Some(8000), 2000)
            .is_err()
    );
}

#[tokio::test]
async fn productivity_reminder_later_folder_scan_invalidates_earlier_generation() {
    let server = crate::test_imap::state("", &[("Archive", 7), ("INBOX", 9)]);
    server
        .lock()
        .unwrap()
        .add("INBOX", "From: you@x\r\nMessage-ID: <r@x>\r\n\r\nbody", &[]);
    let mut session = crate::test_imap::session(server.clone()).await;
    let result = scan_reply_round(
        &mut session,
        "a",
        1,
        || false,
        |_, _| Ok(std::collections::HashSet::new()),
        |_, _, _| Ok(()),
        |_| {
            server.lock().unwrap().mailbox("Archive").validity = 8;
            Ok(())
        },
    )
    .await;
    assert!(
        result.is_err(),
        "earlier folder generation changed before whole round completion"
    );
}
