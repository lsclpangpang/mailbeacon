use super::*;

fn seeded(account: &str, epoch: u64, folder: &str, uid: u32, date: i64, text: &str) -> Summary {
    Summary {
        account_id: account.into(),
        receive_epoch: epoch,
        uid_validity: 7,
        folder: folder.into(),
        uid,
        date,
        subject: text.into(),
        ..Default::default()
    }
}
fn target(account: &str, epoch: u64, folder: &str) -> SearchTarget {
    SearchTarget {
        account_id: account.into(),
        receive_epoch: epoch,
        folder: folder.into(),
    }
}
fn insert(index: &SearchIndex, mail: &Summary) {
    let account = crate::mail_ref::namespace(&mail.account_id, mail.receive_epoch);
    index
        .upsert_summaries(&account, &mail.folder, 7, &[mail.clone()])
        .unwrap();
    index
        .save_meta(&account, &mail.folder, 7, false, 1, 1, 0)
        .unwrap();
}
fn request(targets: Vec<SearchTarget>) -> SearchRequest {
    SearchRequest {
        targets,
        query: "needle".into(),
        page_size: 37,
        ..Default::default()
    }
}

#[test]
fn scoped_results_are_epoch_bound_and_paginate_more_than_two_thousand_equal_dates() {
    let index = SearchIndex::in_memory().unwrap();
    for uid in 1..=2105 {
        insert(&index, &seeded("a", 1, "INBOX", uid, 42, "needle"));
    }
    insert(
        &index,
        &seeded("a", 2, "INBOX", 9999, 42, "needle new endpoint"),
    );
    insert(&index, &seeded("b", 1, "Sent", 4, 42, "needle"));
    insert(&index, &seeded("b", 1, "Trash", 5, 42, "needle"));
    let mut q = request(vec![target("a", 1, "INBOX"), target("b", 1, "Sent")]);
    let mut found = Vec::new();
    loop {
        let page = index.search_page(&q).unwrap();
        found.extend(
            page.items
                .iter()
                .map(|m| (m.account_id.clone(), m.folder.clone(), m.uid)),
        );
        assert!(
            page.items
                .iter()
                .all(|m| m.receive_epoch == 1 && m.uid_validity == 7)
        );
        if !page.has_more {
            assert!(page.next_cursor.is_none());
            break;
        }
        q.cursor = page.next_cursor;
    }
    assert_eq!(found.len(), 2106);
    assert_eq!(
        found.iter().collect::<std::collections::HashSet<_>>().len(),
        2106
    );
}

#[test]
fn cursor_rejects_new_queries_or_changed_index_instead_of_losing_rows() {
    let index = SearchIndex::in_memory().unwrap();
    insert(&index, &seeded("a", 1, "INBOX", 1, 1, "needle"));
    insert(&index, &seeded("a", 1, "INBOX", 2, 2, "needle"));
    let mut q = request(vec![target("a", 1, "INBOX")]);
    q.page_size = 1;
    q.cursor = index.search_page(&q).unwrap().next_cursor;
    let mut other = q.clone();
    other.query = "different".into();
    assert!(index.search_page(&other).is_err());
    insert(&index, &seeded("a", 1, "INBOX", 3, 3, "needle"));
    assert!(index.search_page(&q).is_err());
}

#[test]
fn full_cached_body_is_searchable_after_prefix_and_skipped_is_never_full() {
    let index = SearchIndex::in_memory().unwrap();
    let mail = seeded("a", 1, "INBOX", 1, 1, "long mail");
    insert(&index, &mail);
    let mut raw = b"From: sender@example.test\r\nSubject: long mail\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n".to_vec();
    raw.extend(vec![b'x'; 20_000]);
    raw.extend_from_slice(b" unique-tail-needle");
    index
        .index_full_body(&mail.message_ref().unwrap(), &raw)
        .unwrap();
    let mut q = request(vec![target("a", 1, "INBOX")]);
    q.query = "unique-tail-needle".into();
    let page = index.search_page(&q).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.coverage[0].full, 1);
    let other = seeded("a", 1, "INBOX", 2, 2, "second");
    insert(&index, &other);
    index.skip_body("a", "INBOX", 7, 2).unwrap();
    let page = index.search_page(&q).unwrap();
    assert_eq!(page.coverage[0].full, 1);
    assert_eq!(page.coverage[0].skipped, 1);
    assert!(
        index
            .index_full_body(
                &crate::mail_ref::MessageRef {
                    uid_validity: 8,
                    ..mail.message_ref().unwrap()
                },
                &raw
            )
            .is_err()
    );
}

#[test]
fn offline_snapshot_returns_partial_headers_and_persisted_folder_catalog() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.db");
    {
        let index = SearchIndex::open(&path).unwrap();
        insert(&index, &seeded("a", 1, "Archive", 1, 1, "needle"));
        index.save_meta("a", "Archive", 7, false, 1, 99, 0).unwrap();
        index
            .save_folders(
                "a",
                1,
                &[crate::imap::Folder {
                    path: "Archive".into(),
                    name: "归档".into(),
                    role: "archive".into(),
                }],
            )
            .unwrap();
    }
    let index = SearchIndex::open(&path).unwrap();
    assert_eq!(index.saved_folders("a", 1).unwrap()[0].name, "归档");
    assert!(index.saved_folders("a", 2).unwrap().is_empty());
    let page = index
        .snapshot_page(&target("a", 1, "Archive"), None, 20)
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.coverage[0].total, Some(99));
    assert_eq!(page.coverage[0].indexed, 1);
}

#[test]
fn late_old_generation_summary_cannot_replace_reconnected_mailbox() {
    let index = SearchIndex::in_memory().unwrap();
    let mut mail = seeded("a", 1, "INBOX", 42, 1, "old");
    insert(&index, &mail);
    index.record_snapshot("a", "INBOX", 8).unwrap();
    mail.uid_validity = 8;
    mail.subject = "new".into();
    index
        .upsert_summaries("a", "INBOX", 8, &[mail.clone()])
        .unwrap();
    assert!(
        index
            .upsert_summaries("a", "INBOX", 7, &[seeded("a", 1, "INBOX", 42, 1, "old")])
            .is_err()
    );
    index.save_meta("a", "INBOX", 7, true, 1, 1, 1).unwrap();
    let page = index
        .snapshot_page(&target("a", 1, "INBOX"), None, 100)
        .unwrap();
    assert_eq!(page.items[0].uid_validity, 8);
    assert_eq!(page.items[0].subject, "new");
}
