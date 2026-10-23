use super::*;

fn seeded(account: &str, epoch: u64, folder: &str, validity: u32, uid: u32, text: &str) -> Summary {
    serde_json::from_value(serde_json::json!({
        "accountId":account,"receiveEpoch":epoch,"folder":folder,"uidValidity":validity,
        "uid":uid,"fromName":"Test","fromAddr":"test@example.test","to":"me@example.test",
        "subject":text,"date":1,"seen":false,"flagged":false,"attachment":false,"recipients":[],"messageId":"<test@example.test>","inReplyTo":"","references":[]
    }))
    .unwrap()
}

fn insert(index: &SearchIndex, mail: &Summary) {
    index
        .upsert_summaries(
            &crate::mail_ref::namespace(&mail.account_id, mail.receive_epoch),
            &mail.folder,
            mail.uid_validity,
            &[mail.clone()],
        )
        .unwrap();
}

#[test]
fn sidebar_counts_never_turn_partial_or_missing_snapshots_into_zero_totals() {
    let index = SearchIndex::in_memory().unwrap();
    let missing = index.sidebar_folder_counts("a", 1, "INBOX").unwrap();
    assert_eq!(missing.total.value, None);
    assert_eq!(missing.starred.value, None);
    let mut mail = seeded("a", 1, "INBOX", 7, 1, "partial");
    mail.flagged = true;
    insert(&index, &mail);
    let partial = index.sidebar_folder_counts("a", 1, "INBOX").unwrap();
    assert_eq!(partial.total.value, Some(1));
    assert!(!partial.total.exact);
    assert_eq!(partial.starred.value, Some(1));
    assert!(!partial.starred.exact);
    assert_eq!(
        index
            .sidebar_folder_counts("a", 2, "INBOX")
            .unwrap()
            .total
            .value,
        None
    );
}

#[test]
fn sidebar_status_totals_work_without_fetching_any_summaries_and_reject_old_generations() {
    let index = SearchIndex::in_memory().unwrap();
    index
        .record_sidebar_status("a", 2, "Archive", 7, 321)
        .unwrap();
    let fresh = index.sidebar_folder_counts("a", 2, "Archive").unwrap();
    assert_eq!(fresh.total.value, Some(321));
    assert!(fresh.total.exact);
    assert_eq!(fresh.starred.value, None);
    index
        .record_sidebar_status("a", 2, "Archive", 8, 0)
        .unwrap();
    assert!(
        index
            .record_sidebar_status("a", 2, "Archive", 7, 321)
            .is_err()
    );
    let empty = index.sidebar_folder_counts("a", 2, "Archive").unwrap();
    assert_eq!(empty.total.value, Some(0));
    assert!(empty.total.exact);
    assert_eq!(
        index
            .sidebar_folder_counts("a", 1, "Archive")
            .unwrap()
            .total
            .value,
        None
    );
}

#[test]
fn sidebar_starred_counts_all_index_rows_beyond_the_first_page() {
    let index = SearchIndex::in_memory().unwrap();
    for uid in 1..=251 {
        let mut mail = seeded("a", 2, "INBOX", 7, uid, "starred");
        mail.flagged = true;
        insert(&index, &mail);
    }
    let key = crate::mail_ref::namespace("a", 2);
    index
        .save_meta(&key, "INBOX", 7, false, 251, 251, 0)
        .unwrap();
    let result = index.sidebar_folder_counts("a", 2, "INBOX").unwrap();
    assert_eq!(result.starred.value, Some(251));
    assert!(result.starred.exact);
    index.invalidate_folder(&key, "INBOX").unwrap();
    assert!(
        !index
            .sidebar_folder_counts("a", 2, "INBOX")
            .unwrap()
            .starred
            .exact
    );
    index.record_snapshot(&key, "INBOX", 8).unwrap();
    assert_eq!(
        index
            .sidebar_folder_counts("a", 2, "INBOX")
            .unwrap()
            .starred
            .value,
        None
    );
}

#[test]
fn sidebar_delayed_status_cannot_overwrite_a_new_local_mail_or_explicit_invalidation() {
    let index = SearchIndex::in_memory().unwrap();
    let token = index.sidebar_status_revision("a", 1, "INBOX").unwrap();
    insert(&index, &seeded("a", 1, "INBOX", 7, 1, "new"));
    assert!(
        !index
            .record_sidebar_status_if_current("a", 1, "INBOX", 7, 0, token)
            .unwrap()
    );
    assert_eq!(
        index
            .sidebar_folder_counts("a", 1, "INBOX")
            .unwrap()
            .total
            .value,
        Some(1)
    );
    let token = index.sidebar_status_revision("a", 1, "INBOX").unwrap();
    index.invalidate_folder("a", "INBOX").unwrap();
    assert!(
        !index
            .record_sidebar_status_if_current("a", 1, "INBOX", 7, 0, token)
            .unwrap()
    );
}

#[test]
fn sidebar_newer_enumeration_rejects_old_status_and_status_invalidates_old_enumeration() {
    let index = SearchIndex::in_memory().unwrap();
    index.record_snapshot("a", "INBOX", 7).unwrap();
    let token = index.sidebar_status_revision("a", 1, "INBOX").unwrap();
    index.save_meta("a", "INBOX", 7, false, 0, 321, 0).unwrap();
    assert!(
        !index
            .record_sidebar_status_if_current("a", 1, "INBOX", 7, 1, token)
            .unwrap()
    );
    let token = index.sidebar_status_revision("a", 1, "INBOX").unwrap();
    assert!(
        index
            .record_sidebar_status_if_current("a", 1, "INBOX", 7, 320, token)
            .unwrap()
    );
    assert_eq!(index.progress("a", "INBOX").unwrap().total, 0);
    assert_eq!(
        index
            .sidebar_folder_counts("a", 1, "INBOX")
            .unwrap()
            .total
            .value,
        Some(320)
    );
    assert_ne!(
        token,
        index.sidebar_status_revision("a", 1, "INBOX").unwrap()
    );
}

#[test]
fn sidebar_attachment_count_has_no_page_limit_and_requires_parsed_coverage() {
    let index = SearchIndex::in_memory().unwrap();
    insert(&index, &seeded("a", 2, "INBOX", 7, 1, "attachments"));
    let key = crate::mail_ref::namespace("a", 2);
    index.save_meta(&key, "INBOX", 7, false, 1, 1, 0).unwrap();
    for part in 0..125 {
        index.lock().execute("INSERT INTO attachment_metadata(account_id,receive_epoch,storage_account,folder,uid_validity,uid,part_id,filename,mime,sender,message_date,item) VALUES('a',2,?1,'INBOX',7,1,?2,'x','text/plain','test',0,'{}')",params![key,part]).unwrap();
    }
    let partial = index.sidebar_folder_counts("a", 2, "INBOX").unwrap();
    assert_eq!(partial.attachments.value, Some(125));
    assert!(!partial.attachments.exact);
    index
        .lock()
        .execute(
            "INSERT INTO attachment_sources VALUES(?1,'INBOX',7,1,'hash',?2)",
            params![key, crate::attachment_index::PARSER_VERSION],
        )
        .unwrap();
    let complete = index.sidebar_folder_counts("a", 2, "INBOX").unwrap();
    assert_eq!(complete.attachments.value, Some(125));
    assert!(complete.attachments.exact);
    index.record_snapshot(&key, "INBOX", 8).unwrap();
    assert_eq!(
        index
            .sidebar_folder_counts("a", 2, "INBOX")
            .unwrap()
            .attachments
            .value,
        None
    );
}

#[test]
fn sidebar_removed_accounts_reject_late_status_writes_in_every_epoch() {
    let index = SearchIndex::in_memory().unwrap();
    index.record_sidebar_status("a", 1, "INBOX", 7, 10).unwrap();
    index.record_sidebar_status("a", 2, "INBOX", 7, 20).unwrap();
    index.remove_account("a").unwrap();
    for epoch in [1, 2] {
        assert_eq!(
            index
                .sidebar_folder_counts("a", epoch, "INBOX")
                .unwrap()
                .total
                .value,
            None
        );
        assert!(
            index
                .record_sidebar_status("a", epoch, "INBOX", 7, 100)
                .is_err()
        );
    }
}

#[test]
fn sidebar_confirmed_batch_removal_updates_total_once_and_keeps_empty_exact() {
    let index = SearchIndex::in_memory().unwrap();
    for uid in [1, 2] {
        insert(&index, &seeded("a", 1, "INBOX", 7, uid, "mail"));
    }
    index.save_meta("a", "INBOX", 7, false, 2, 2, 0).unwrap();
    index.record_sidebar_status("a", 1, "INBOX", 7, 2).unwrap();
    let token = index.sidebar_status_revision("a", 1, "INBOX").unwrap();
    index.remove_uids("a", "INBOX", &[1, 1]).unwrap();
    let remaining = index.sidebar_folder_counts("a", 1, "INBOX").unwrap();
    assert_eq!(remaining.total.value, Some(1));
    assert!(remaining.total.exact);
    assert!(remaining.starred.exact);
    assert!(
        !index
            .record_sidebar_status_if_current("a", 1, "INBOX", 7, 2, token)
            .unwrap()
    );
    index.remove_uids("a", "INBOX", &[1]).unwrap();
    assert_eq!(
        index
            .sidebar_folder_counts("a", 1, "INBOX")
            .unwrap()
            .total
            .value,
        Some(1)
    );
    index.remove_uids("a", "INBOX", &[2]).unwrap();
    let empty = index.sidebar_folder_counts("a", 1, "INBOX").unwrap();
    assert_eq!(empty.total.value, Some(0));
    assert!(empty.total.exact);
}
