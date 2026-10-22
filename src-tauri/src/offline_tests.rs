use super::*;
use crate::mail_ref::MessageRef;

#[tokio::test]
async fn cold_start_reads_exact_snapshot_and_attachments_without_a_network_client() {
    let root = tempfile::tempdir().unwrap();
    let db = root.path().join("local.db");
    let bodies = root.path().join("bodies");
    let source = MessageRef {
        account_id: "a".into(),
        receive_epoch: 1,
        folder: "INBOX".into(),
        uid_validity: 7,
        uid: 42,
    };
    let raw=b"From: Sender <sender@example.test>\r\nSubject: Offline\r\nMIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=x\r\n\r\n--x\r\nContent-Type: text/plain\r\n\r\nSaved body\r\n--x\r\nContent-Type: text/plain\r\nContent-Disposition: attachment; filename=a.txt\r\n\r\nSaved attachment\r\n--x--\r\n";
    {
        let index = search_index::SearchIndex::open(&db).unwrap();
        let summary = Summary {
            account_id: "a".into(),
            folder: "INBOX".into(),
            uid: 42,
            uid_validity: 7,
            ..Default::default()
        };
        index.upsert_summaries("a", "INBOX", 7, &[summary]).unwrap();
        index.record_snapshot("a", "INBOX", 7).unwrap();
        body_store::BodyStore::new(bodies.clone(), 1_000_000).put("a", "INBOX", 7, 42, raw, 123);
    }
    let index = search_index::SearchIndex::open(&db).unwrap();
    let cache = Mutex::new(BodyCache::with_disk(Arc::new(body_store::BodyStore::new(
        bodies, 1_000_000,
    ))));
    let read = offline::read_snapshot(&index, &cache, &source, 1)
        .await
        .unwrap();
    assert!(read.local_only);
    assert!(!read.read_marked);
    assert!(!index.snapshot_summary(&source).unwrap().seen);
    assert!(read.message.receipt_to.is_none());
    assert_eq!(read.source, source);
    assert!(read.message.text.contains("Saved body"));
    assert_eq!(read.message.attachments.len(), 1);
    assert!(
        offline::read_snapshot(
            &index,
            &cache,
            &MessageRef {
                receive_epoch: 2,
                ..source.clone()
            },
            2
        )
        .await
        .is_err()
    );
    assert!(
        offline::read_snapshot(
            &index,
            &cache,
            &MessageRef {
                uid_validity: 8,
                ..source.clone()
            },
            1
        )
        .await
        .is_err()
    );
    assert!(
        offline::read_snapshot(
            &index,
            &cache,
            &MessageRef {
                uid_validity: 0,
                ..source.clone()
            },
            1
        )
        .await
        .is_err()
    );
    assert!(
        offline::read_snapshot(&index, &cache, &source, 2)
            .await
            .is_err()
    );
}

fn disk_snapshot(
    root: &std::path::Path,
) -> (search_index::SearchIndex, Mutex<BodyCache>, MessageRef) {
    let index = search_index::SearchIndex::open(&root.join("local.db")).unwrap();
    let source = MessageRef {
        account_id: "a".into(),
        receive_epoch: 1,
        folder: "INBOX".into(),
        uid_validity: 7,
        uid: 42,
    };
    index
        .upsert_summaries(
            "a",
            "INBOX",
            7,
            &[Summary {
                account_id: "a".into(),
                folder: "INBOX".into(),
                uid: 42,
                uid_validity: 7,
                ..Default::default()
            }],
        )
        .unwrap();
    index.record_snapshot("a", "INBOX", 7).unwrap();
    let disk = Arc::new(body_store::BodyStore::new(root.join("bodies"), 1_000_000));
    disk.put(
        "a",
        "INBOX",
        7,
        42,
        b"From: sender@example.test\r\nSubject: Old generation\r\nDisposition-Notification-To: sender@example.test\r\n\r\nOriginal body",
        123,
    );
    (index, Mutex::new(BodyCache::with_disk(disk)), source)
}

#[tokio::test]
async fn local_disk_read_keeps_unread_state_and_suppresses_receipts() {
    let root = tempfile::tempdir().unwrap();
    let (index, cache, source) = disk_snapshot(root.path());
    let before = index.snapshot_reference(&source).unwrap();
    let read = offline::read_snapshot(&index, &cache, &source, 1)
        .await
        .unwrap();
    assert_eq!(read.message.subject, "Old generation");
    assert!(read.local_only);
    assert!(!read.read_marked);
    assert!(read.message.receipt_to.is_none());
    assert!(!index.snapshot_summary(&source).unwrap().seen);
    assert_eq!(index.snapshot_reference(&source).unwrap(), before);
}

#[tokio::test]
async fn existing_disk_file_is_rejected_after_index_generation_changes() {
    let root = tempfile::tempdir().unwrap();
    let (index, cache, source) = disk_snapshot(root.path());
    index.record_snapshot("a", "INBOX", 8).unwrap();
    index
        .upsert_summaries(
            "a",
            "INBOX",
            8,
            &[Summary {
                account_id: "a".into(),
                folder: "INBOX".into(),
                uid: 42,
                uid_validity: 8,
                ..Default::default()
            }],
        )
        .unwrap();
    let disk = cache.lock().unwrap().disk.clone().unwrap();
    assert!(disk.get("a", "INBOX", 7, 42).is_some());
    disk.put(
        "a",
        "INBOX",
        8,
        42,
        b"From: other@example.test\r\nSubject: New generation\r\n\r\nDifferent body",
        456,
    );
    assert!(
        offline::read_snapshot(&index, &cache, &source, 1)
            .await
            .is_err()
    );
    let current = MessageRef {
        uid_validity: 8,
        ..source
    };
    let read = offline::read_snapshot(&index, &cache, &current, 1)
        .await
        .unwrap();
    assert_eq!(read.message.subject, "New generation");
    assert_eq!(read.source, current);
    assert!(!read.read_marked);
}

#[test]
fn index_generation_change_while_disk_read_is_pending_rejects_old_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (index, cache, source) = disk_snapshot(root.path());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        // Hold the only blocking worker so the real cache read yields after its
        // initial identity check and before any file bytes have been read.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let blocker = tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let mut reading = Box::pin(offline::read_snapshot(&index, &cache, &source, 1));
        assert!(futures::poll!(reading.as_mut()).is_pending());
        index.record_snapshot("a", "INBOX", 8).unwrap();
        release_tx.send(()).unwrap();
        let result = reading.await;
        blocker.await.unwrap();
        assert!(
            result.is_err(),
            "a superseded generation must not be returned as a valid local snapshot"
        );
    });
}

#[tokio::test]
async fn missing_or_corrupted_local_body_is_not_another_mail_or_an_online_retry() {
    let root = tempfile::tempdir().unwrap();
    let index = search_index::SearchIndex::open(&root.path().join("local.db")).unwrap();
    let source = MessageRef {
        account_id: "a".into(),
        receive_epoch: 1,
        folder: "INBOX".into(),
        uid_validity: 7,
        uid: 1,
    };
    index
        .upsert_summaries(
            "a",
            "INBOX",
            7,
            &[Summary {
                account_id: "a".into(),
                folder: "INBOX".into(),
                uid: 1,
                ..Default::default()
            }],
        )
        .unwrap();
    index.record_snapshot("a", "INBOX", 7).unwrap();
    let cache = Mutex::new(BodyCache::default());
    assert!(
        offline::read_snapshot(&index, &cache, &source, 1)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("摘要")
    );
    cache.lock().unwrap().insert(
        ("a".into(), "INBOX".into(), 7, 2),
        Arc::new(b"Subject: other\r\n\r\nother".to_vec()),
        0,
    );
    assert!(
        offline::read_snapshot(&index, &cache, &source, 1)
            .await
            .is_err()
    );
    cache
        .lock()
        .unwrap()
        .insert(("a".into(), "INBOX".into(), 7, 1), Arc::new(vec![0; 4]), 0);
    assert!(
        offline::read_snapshot(&index, &cache, &source, 1)
            .await
            .is_err()
    );
}
