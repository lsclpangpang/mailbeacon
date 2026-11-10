use super::*;
use crate::{rules, test_imap};

fn raw(subject: &str, days_ago: i64) -> String {
    format!(
        "From: sender@example.invalid\r\nSubject: {subject}\r\nX-Internal-Date: {}\r\n\r\nbody\r\n",
        (chrono::Utc::now() - chrono::Duration::days(days_ago)).format("%d-%b-%Y %H:%M:%S +0000")
    )
}

fn cursor() -> Cursor {
    Cursor {
        uid_validity: 3,
        ..Default::default()
    }
}

fn move_to(folder: &str) -> rules::Actions {
    rules::Actions {
        move_to: Some(folder.into()),
        ..Default::default()
    }
}

#[test]
fn changed_folder_events_carry_paths_without_reusing_the_source_message_uid() {
    let event = serde_json::to_value(Event::FolderChanged {
        account_id: "a",
        folder: "special",
    })
    .unwrap();
    assert_eq!(
        event,
        serde_json::json!({"type": "folderChanged", "accountId": "a", "folder": "special"})
    );
}

#[tokio::test]
async fn rule_moves_refresh_only_confirmed_targets_without_notification_opt_in() {
    let server = test_imap::state("MOVE UIDPLUS", &[("INBOX", 3), ("special", 7), ("Work", 9)]);
    let (first, rejected, third) = {
        let mut state = server.lock().unwrap();
        state.mailbox("special").next_uid = 700;
        (
            state.add("INBOX", &raw("first", 0), &[]),
            state.add("INBOX", &raw("rejected", 0), &[]),
            state.add("INBOX", &raw("third", 0), &[]),
        )
    };
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let batches = vec![
        (move_to("special"), vec![first]),
        (move_to("Missing"), vec![rejected]),
        (move_to("special"), vec![third]),
    ];
    let prefs = crate::prefs::Prefs::default();
    assert!(prefs.extra_folders("a").is_empty());
    let mut pending = PendingFolderChanges::default();
    let done = rules::execute_with_progress(&mut session, &batches, |actions, applied| {
        pending.record(actions, applied, 3);
    })
    .await
    .unwrap();
    assert_eq!(done.moved, vec![first, third]);
    assert_eq!(done.errors.len(), 1);
    let mut removals = Vec::new();
    let mut changed = Vec::new();
    pending
        .retry(
            |validity, uids| {
                removals.push((validity, uids.to_vec()));
                Ok(true)
            },
            |folder| {
                changed.push(folder.to_owned());
                Ok(true)
            },
        )
        .unwrap();
    assert_eq!(changed, vec!["special"]);
    let mut removed = removals[0].1.clone();
    removed.sort_unstable();
    assert_eq!(removals[0].0, 3);
    assert_eq!(removed, vec![first, third]);
    let state = server.lock().unwrap();
    let target = &state.mailboxes[1].messages;
    assert_eq!(
        target.iter().map(|mail| mail.uid).collect::<Vec<_>>(),
        vec![700, 701]
    );
    assert!(
        target
            .iter()
            .all(|mail| mail.uid != first && mail.uid != third),
        "target rows must be fetched with their actual destination UID"
    );
    assert_eq!(state.mailboxes[0].messages[0].uid, rejected);
}

#[tokio::test]
async fn confirmed_rule_moves_survive_later_disconnects_and_local_refresh_retries() {
    let server = test_imap::state("MOVE UIDPLUS", &[("INBOX", 3), ("special", 7), ("Work", 9)]);
    let (first, second) = {
        let mut state = server.lock().unwrap();
        (
            state.add("INBOX", &raw("first", 0), &[]),
            state.add("INBOX", &raw("second", 0), &[]),
        )
    };
    server.lock().unwrap().disconnect = vec![format!("UID MOVE {second} ")];
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let mut pending = PendingFolderChanges::default();
    let result = rules::execute_with_progress(
        &mut session,
        &[
            (move_to("special"), vec![first]),
            (move_to("Work"), vec![second]),
        ],
        |actions, applied| pending.record(actions, applied, 3),
    )
    .await;
    assert!(
        result.is_err(),
        "a disconnected batch must still trigger reconnection"
    );
    let mut removed = Vec::new();
    assert!(
        pending
            .retry(
                |_, uids| {
                    removed.extend_from_slice(uids);
                    Ok(true)
                },
                |_| anyhow::bail!("temporary local index write failure"),
            )
            .is_err()
    );
    assert_eq!(removed, vec![first]);
    let mut changed = Vec::new();
    pending
        .retry(
            |_, _| panic!("a successful source deletion must not run again"),
            |folder| {
                changed.push(folder.to_owned());
                Ok(true)
            },
        )
        .unwrap();
    assert_eq!(changed, vec!["special"]);
    pending
        .retry(
            |_, _| panic!("successful updates must leave no pending source"),
            |_| panic!("successful updates must leave no pending target"),
        )
        .unwrap();
    let commands = &server.lock().unwrap().commands;
    assert_eq!(
        commands
            .iter()
            .filter(|command| command.starts_with(&format!("UID MOVE {first} ")))
            .count(),
        1,
        "retrying local changes must not repeat a confirmed MOVE"
    );
}

#[tokio::test]
async fn watched_baselines_old_mail_and_rebuilt_folders_refresh_without_old_notifications() {
    let server = test_imap::state("IDLE", &[("INBOX", 3), ("special", 7)]);
    server
        .lock()
        .unwrap()
        .add("special", &raw("existing", 300), &[]);
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let mut cursor = cursor();
    let folders = vec!["special".into()];
    let mut changed = Vec::new();
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        changed.push(folder.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    assert!(
        news.is_empty(),
        "initial baseline must not announce existing mail"
    );
    assert_eq!(
        changed,
        vec!["special"],
        "baseline must invalidate a potentially old local list"
    );
    changed.clear();
    extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        changed.push(folder.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    assert!(
        changed.is_empty(),
        "an unchanged folder must not rebuild every 30 seconds"
    );

    server
        .lock()
        .unwrap()
        .add("special", &raw("moved old mail", 300), &[]);
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        changed.push(folder.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    assert!(
        news.is_empty(),
        "old mail still must not produce a notification"
    );
    assert_eq!(
        changed,
        vec!["special"],
        "notification filtering must not suppress list updates"
    );
    changed.clear();
    server.lock().unwrap().mailbox("special").validity = 8;
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        changed.push(folder.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    assert!(news.is_empty());
    assert_eq!(changed, vec!["special"]);
}

#[tokio::test]
async fn a_gmail_duplicate_still_refreshes_its_destination_list() {
    let server = test_imap::state("IDLE", &[("INBOX", 3), ("Label", 7)]);
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let mut cursor = cursor();
    let folders = vec!["Label".into()];
    extra_folder_updates(&mut session, "a", &folders, &mut cursor, |_| Ok(()))
        .await
        .unwrap();
    let mail = raw("same mail", 0);
    let inbox_uid = server.lock().unwrap().add("INBOX", &mail, &[]);
    let seen = imap::fetch_summaries(&mut session, &inbox_uid.to_string(), "a", INBOX)
        .await
        .unwrap();
    cursor.remember_inbox(&seen);
    server.lock().unwrap().add("Label", &mail, &[]);
    let mut changed = Vec::new();
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        changed.push(folder.to_owned());
        Ok(())
    })
    .await
    .unwrap();
    assert!(news.is_empty(), "the same mail must not notify twice");
    assert_eq!(changed, vec!["Label"]);
}

#[tokio::test]
async fn local_folder_update_failures_do_not_disconnect_a_healthy_mail_check() {
    let server = test_imap::state("IDLE", &[("INBOX", 3), ("special", 7), ("Work", 9)]);
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let mut cursor = cursor();
    let folders = vec!["special".into(), "Work".into()];
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |_| {
        anyhow::bail!("local index busy")
    })
    .await
    .expect("a local SQLite failure must not mark the IMAP connection offline");
    assert!(news.is_empty(), "a baseline must not announce old mail");
    assert_eq!(cursor.folders.0["special"].next, 1);
    assert_eq!(cursor.folders.0["Work"].next, 1);
    {
        let mut state = server.lock().unwrap();
        state.add("special", &raw("first", 0), &[]);
        state.add("Work", &raw("second", 0), &[]);
    }
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
        anyhow::ensure!(folder != "Work", "local index busy");
        Ok(())
    })
    .await
    .expect("new-mail detection must continue while the local cache is busy");
    assert_eq!(
        news.iter()
            .map(|(folder, _)| folder.as_str())
            .collect::<Vec<_>>(),
        vec!["special", "Work"],
        "local writes must not swallow either folder's notifications"
    );
    assert_eq!(cursor.folders.0["special"].next, 2);
    assert_eq!(cursor.folders.0["Work"].next, 2);
    let mut retried = Vec::new();
    cursor
        .folder_changes
        .retry(
            |_, _| panic!("retrying a watched folder must never execute a MOVE"),
            |folder| {
                retried.push(folder.to_owned());
                Ok(true)
            },
        )
        .unwrap();
    retried.sort();
    assert_eq!(retried, vec!["Work", "special"]);
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |_| {
        panic!("an unchanged folder should not restart local updates")
    })
    .await
    .unwrap();
    assert!(news.is_empty());
}

#[tokio::test]
async fn pending_local_updates_survive_a_later_real_imap_disconnect() {
    let server = test_imap::state("IDLE", &[("INBOX", 3), ("special", 7), ("Work", 9)]);
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let mut cursor = cursor();
    let folders = vec!["special".into(), "Work".into()];
    extra_folder_updates(&mut session, "a", &folders, &mut cursor, |_| Ok(()))
        .await
        .unwrap();
    {
        let mut state = server.lock().unwrap();
        state.add("special", &raw("first", 0), &[]);
        state.add("Work", &raw("second", 0), &[]);
        state.disconnect = vec!["EXAMINE \"Work\"".into()];
    }
    assert!(
        extra_folder_updates(&mut session, "a", &folders, &mut cursor, |folder| {
            anyhow::ensure!(folder != "special", "local index busy");
            Ok(())
        })
        .await
        .is_err(),
        "an actual IMAP disconnect must still trigger reconnection"
    );
    assert!(
        server
            .lock()
            .unwrap()
            .commands
            .iter()
            .any(|command| command == "EXAMINE \"Work\""),
        "the earlier local failure must not abort the server check"
    );
    assert_eq!(
        cursor.folders.0["special"].next, 1,
        "a real connection failure must keep unannounced notifications retryable"
    );
    let mut retried = Vec::new();
    cursor
        .folder_changes
        .retry(
            |_, _| panic!("watched-folder recovery has no source MOVE"),
            |folder| {
                retried.push(folder.to_owned());
                Ok(true)
            },
        )
        .unwrap();
    assert_eq!(
        retried,
        vec!["special"],
        "local work must survive the later network error"
    );
    server.lock().unwrap().disconnect.clear();
    let mut session = test_imap::session(server.clone()).await;
    session.select(INBOX).await.unwrap();
    let (news, _) = extra_folder_updates(&mut session, "a", &folders, &mut cursor, |_| Ok(()))
        .await
        .unwrap();
    assert_eq!(
        news.iter()
            .map(|(folder, _)| folder.as_str())
            .collect::<Vec<_>>(),
        vec!["special", "Work"]
    );
}
