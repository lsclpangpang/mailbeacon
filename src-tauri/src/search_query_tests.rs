use super::*;

#[test]
fn exact_summary_reads_partial_catalog_without_rebinding_old_identity() {
    let index = SearchIndex::in_memory().unwrap();
    let mut mail = seeded("a", 2, "INBOX", 4, 1, "anchor");
    mail.message_id = "source@example.test".into();
    insert(&index, &mail);
    let source = mail.message_ref().unwrap();
    let stored = index.snapshot_summary(&source).unwrap();
    assert_eq!(stored.message_id, "source@example.test");
    assert_eq!(stored.message_ref().unwrap(), source);
    let mut old = source.clone();
    old.receive_epoch = 1;
    assert!(index.snapshot_summary(&old).is_err());
    index
        .record_snapshot(&crate::mail_ref::namespace("a", 2), "INBOX", 8)
        .unwrap();
    assert!(index.snapshot_summary(&source).is_err());
}

#[test]
fn productivity_migration_creates_persistent_views_without_taking_contacts_version() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.db");
    let connection = Connection::open(&path).unwrap();
    connection.pragma_update(None, "user_version", 77).unwrap();
    drop(connection);
    let index = SearchIndex::open(&path).unwrap();
    let count: i64 = index.lock().query_row("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('saved_views','attachment_metadata')", [], |r| r.get(0)).unwrap();
    assert_eq!(
        count, 2,
        "saved views and parsed attachment metadata must survive restart"
    );
    let version: i64 = index
        .lock()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 77);
}

#[test]
fn productivity_migration_failure_rolls_back_new_tables_and_preserves_existing_data() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.db");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("PRAGMA user_version=77;CREATE TABLE existing_user_data(value TEXT);INSERT INTO existing_user_data VALUES('keep');CREATE TABLE productivity_migrations(name TEXT PRIMARY KEY);CREATE TRIGGER fail_attachment_migration BEFORE INSERT ON productivity_migrations WHEN NEW.name='attachment-metadata-v1' BEGIN SELECT RAISE(ABORT,'injected migration fault');END;").unwrap();
    assert!(SearchIndex::open(&path).is_err());
    let count:i64=db.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name IN ('saved_views','attachment_metadata','attachment_sources')",[],|r|r.get(0)).unwrap();
    assert_eq!(count, 0);
    assert_eq!(
        db.query_row("SELECT value FROM existing_user_data", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "keep"
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        77
    );
}

#[test]
fn deleted_catalog_folder_is_not_rebound_to_stale_index_or_same_name_other_account() {
    let index = SearchIndex::in_memory().unwrap();
    let old = seeded("a", 1, "Archive", 1, 1, "old");
    insert(&index, &old);
    index
        .save_folders(
            "a",
            1,
            &[crate::imap::Folder {
                path: "Renamed".into(),
                name: "Archive".into(),
                role: "archive".into(),
            }],
        )
        .unwrap();
    index
        .save_folders(
            "b",
            1,
            &[crate::imap::Folder {
                path: "Archive".into(),
                name: "Archive".into(),
                role: "archive".into(),
            }],
        )
        .unwrap();
    assert!(!index.catalog_contains(&target("a", 1, "Archive")).unwrap());
    assert!(index.catalog_contains(&target("b", 1, "Archive")).unwrap());
    assert_eq!(
        index
            .catalog_folders("a", 1)
            .unwrap()
            .iter()
            .map(|f| f.path.as_str())
            .collect::<Vec<_>>(),
        vec!["Renamed"]
    );
}

#[test]
fn saved_search_can_sort_condition_only_results_oldest_first() {
    let index = SearchIndex::in_memory().unwrap();
    insert(&index, &seeded("a", 1, "INBOX", 1, 10, "first"));
    insert(&index, &seeded("a", 1, "INBOX", 2, 20, "second"));
    let request: SearchRequest = serde_json::from_value(serde_json::json!({
        "query":"", "targets":[{"accountId":"a","receiveEpoch":1,"folder":"INBOX"}],
        "sort":"dateAsc", "pageSize":10
    }))
    .unwrap();
    let page = index.search_page(&request).unwrap();
    assert_eq!(
        page.items.iter().map(|m| m.uid).collect::<Vec<_>>(),
        vec![1, 2]
    );
}

#[test]
fn complete_mime_indexing_records_attachment_metadata_for_the_exact_source() {
    let index = SearchIndex::in_memory().unwrap();
    let mail = seeded("a", 1, "INBOX", 1, 10, "file");
    insert(&index, &mail);
    let raw = b"From: sender@example.test\r\nSubject: file\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nhello\r\n--x\r\nContent-Type: image/png\r\nContent-Disposition: attachment; filename=one.png\r\nContent-Transfer-Encoding: base64\r\n\r\naGVsbG8=\r\n--x--\r\n";
    index
        .index_full_body(&mail.message_ref().unwrap(), raw)
        .unwrap();
    let table: bool = index
        .lock()
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='attachment_metadata')",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        table,
        "full MIME parsing must feed durable attachment metadata"
    );
    let count: i64 = index.lock().query_row("SELECT COUNT(*) FROM attachment_metadata WHERE account_id='a' AND receive_epoch=1 AND folder='INBOX' AND uid_validity=7 AND uid=1", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn attachment_index_persists_paginates_and_invalidates_old_content_cursors() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.db");
    let raw=b"From: sender@example.test\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nbody\r\n--x\r\nContent-Type: application/pdf\r\nContent-Disposition: attachment; filename=same.pdf\r\n\r\n%PDF-1.4\r\n--x--\r\n";
    {
        let index = SearchIndex::open(&path).unwrap();
        for uid in 1..=205 {
            let mail = seeded("a", 1, "INBOX", uid, 42, "file");
            insert(&index, &mail);
            index
                .index_full_body(&mail.message_ref().unwrap(), raw)
                .unwrap();
        }
        let other = seeded("a", 2, "INBOX", 1, 42, "new endpoint");
        insert(&index, &other);
        index
            .index_full_body(&other.message_ref().unwrap(), raw)
            .unwrap();
    }
    let index = SearchIndex::open(&path).unwrap();
    let mut request = crate::attachment_index::Query {
        targets: vec![target("a", 1, "INBOX")],
        page_size: 37,
        ..Default::default()
    };
    let mut seen = std::collections::HashSet::new();
    loop {
        let page = index.attachment_page(&request).unwrap();
        assert_eq!(page.coverage[0].parsed_messages, 205);
        assert_eq!(page.coverage[0].attachment_count, 205);
        for item in &page.items {
            assert_eq!(item.reference.source.receive_epoch, 1);
            assert!(seen.insert(item.reference.source.uid));
        }
        if !page.has_more {
            break;
        }
        request.cursor = page.next_cursor;
    }
    assert_eq!(seen.len(), 205);
    request.cursor = None;
    let first = index.attachment_page(&request).unwrap();
    request.cursor = first.next_cursor;
    let source = seeded("a", 1, "INBOX", 1, 42, "file")
        .message_ref()
        .unwrap();
    let mut changed = raw.to_vec();
    changed.extend_from_slice(b"new raw");
    index.index_attachments(&source, &changed).unwrap();
    assert!(index.attachment_page(&request).is_err());
    index.remove_account("a").unwrap();
    request.cursor = None;
    assert!(index.attachment_page(&request).unwrap().items.is_empty());
    assert!(index.index_attachments(&source, raw).is_err());
}

#[test]
fn attachment_metadata_does_not_invent_unparsed_files_or_include_old_uidvalidity() {
    let index = SearchIndex::in_memory().unwrap();
    let mut mail = seeded("a", 1, "INBOX", 1, 1, "summary only");
    mail.attachment = true;
    insert(&index, &mail);
    let q = crate::attachment_index::Query {
        targets: vec![target("a", 1, "INBOX")],
        ..Default::default()
    };
    let page = index.attachment_page(&q).unwrap();
    assert!(page.items.is_empty());
    assert_eq!(page.coverage[0].indexed_messages, 1);
    assert_eq!(page.coverage[0].parsed_messages, 0);
    index.record_snapshot("a", "INBOX", 8).unwrap();
    assert!(
        index
            .index_attachments(&mail.message_ref().unwrap(), b"Subject: stale\r\n\r\nbody")
            .is_err()
    );
}

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

#[test]
fn account_removal_clears_every_epoch_and_blocks_late_index_and_catalog_writes() {
    let index = SearchIndex::in_memory().unwrap();
    let folders = vec![crate::imap::Folder {
        path: "INBOX".into(),
        name: "Inbox".into(),
        role: "inbox".into(),
    }];
    for epoch in [1, 2, 3] {
        insert(&index, &seeded("a", epoch, "INBOX", 1, 1, "needle"));
        index.save_folders("a", epoch, &folders).unwrap();
    }
    insert(&index, &seeded("ab", 2, "INBOX", 1, 1, "needle other"));
    index.remove_account("a").unwrap();
    for epoch in [1, 2, 3] {
        let storage = crate::mail_ref::namespace("a", epoch);
        assert!(
            index
                .search_page(&request(vec![target("a", epoch, "INBOX")]))
                .unwrap()
                .items
                .is_empty()
        );
        assert!(index.saved_folders("a", epoch).unwrap().is_empty());
        assert!(index.folder_validity(&storage, "INBOX").unwrap().is_none());
        assert!(
            index
                .upsert_summaries(
                    &storage,
                    "INBOX",
                    7,
                    &[seeded("a", epoch, "INBOX", 2, 2, "late")]
                )
                .is_err()
        );
        assert!(
            index
                .save_meta(&storage, "INBOX", 7, true, 2, 2, 2)
                .is_err()
        );
        assert!(index.save_folders("a", epoch, &folders).is_err());
    }
    assert!(
        index.save_folders("a", 4, &folders).is_err(),
        "a delayed future namespace must also be blocked"
    );
    assert_eq!(
        index
            .search_page(&request(vec![target("ab", 2, "INBOX")]))
            .unwrap()
            .items
            .len(),
        1
    );
}
