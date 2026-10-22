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
