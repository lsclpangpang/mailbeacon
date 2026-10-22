use super::*;

use std::{cell::Cell, fs, path::Path};

use rusqlite::{Connection, params};

fn database(dir: &Path) -> Connection {
    Connection::open(dir.join("local.db")).unwrap()
}

fn encrypted_record(dir: &Path, id: &str) -> (i64, Vec<u8>, Vec<u8>) {
    database(dir)
        .query_row(
            "SELECT version, nonce, ciphertext FROM account_credentials WHERE account_id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
}

fn cannot_load(dir: &Path, id: &str) -> bool {
    match Store::open(dir) {
        Err(_) => true,
        Ok(store) => store.load(id).is_err(),
    }
}

#[test]
fn credentials_round_trip_update_reopen_and_remove_independently() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.load("missing").unwrap(), None);
    store.save("a", "授权码-one").unwrap();
    store.save("b", "another-secret").unwrap();
    store.save("a", "replacement-secret").unwrap();
    assert_eq!(
        store.load("a").unwrap().as_deref(),
        Some("replacement-secret")
    );
    drop(store);

    let store = Store::open(dir.path()).unwrap();
    assert_eq!(
        store.load("a").unwrap().as_deref(),
        Some("replacement-secret")
    );
    assert_eq!(store.load("b").unwrap().as_deref(), Some("another-secret"));
    store.remove("a").unwrap();
    store.remove("a").unwrap();
    assert_eq!(store.load("a").unwrap(), None);
    assert_eq!(store.load("b").unwrap().as_deref(), Some("another-secret"));
    drop(store);
    assert_eq!(Store::open(dir.path()).unwrap().load("a").unwrap(), None);
}

#[test]
fn repeated_saves_use_fresh_nonces_and_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "same-secret").unwrap();
    let first = encrypted_record(dir.path(), "a");
    store.save("a", "same-secret").unwrap();
    let second = encrypted_record(dir.path(), "a");
    assert_ne!(first.1, second.1, "AEAD nonce must not be reused");
    assert_ne!(first.2, second.2);
    assert_eq!(store.load("a").unwrap().as_deref(), Some("same-secret"));
}

#[test]
fn ciphertext_is_bound_to_the_account_identity() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "secret-for-a").unwrap();
    store.save("b", "secret-for-b").unwrap();
    let (version, nonce, ciphertext) = encrypted_record(dir.path(), "a");
    database(dir.path())
        .execute(
            "UPDATE account_credentials SET version=?1, nonce=?2, ciphertext=?3 WHERE account_id='b'",
            params![version, nonce, ciphertext],
        )
        .unwrap();
    assert!(store.load("b").is_err());
    assert_eq!(store.load("a").unwrap().as_deref(), Some("secret-for-a"));
}

#[test]
fn tampered_ciphertext_fails_without_retrying_the_keychain() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "original-secret").unwrap();
    let (_, _, mut ciphertext) = encrypted_record(dir.path(), "a");
    ciphertext[0] ^= 0x80;
    database(dir.path())
        .execute(
            "UPDATE account_credentials SET ciphertext=?1 WHERE account_id='a'",
            [ciphertext],
        )
        .unwrap();
    let legacy_called = Cell::new(false);
    let result = store.load_or_migrate("a", || {
        legacy_called.set(true);
        Ok("old-keychain-secret".to_string())
    });
    assert!(result.is_err());
    assert!(!legacy_called.get());
    assert!(!format!("{:#}", result.unwrap_err()).contains("original-secret"));
}

#[test]
fn unsupported_versions_and_malformed_nonces_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("version", "version-secret").unwrap();
    store.save("nonce", "nonce-secret").unwrap();
    let conn = database(dir.path());
    conn.execute(
        "UPDATE account_credentials SET version=999 WHERE account_id='version'",
        [],
    )
    .unwrap();
    conn.execute(
        "UPDATE account_credentials SET nonce=?1 WHERE account_id='nonce'",
        [vec![0_u8; 3]],
    )
    .unwrap();
    assert!(store.load("version").is_err());
    assert!(store.load("nonce").is_err());
}

#[test]
fn missing_key_never_replaces_the_key_for_existing_ciphertext() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "cannot-regenerate-this-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    drop(store);
    let key = dir.path().join("secrets/credentials.key");
    fs::remove_file(&key).unwrap();
    assert!(cannot_load(dir.path(), "a"));
    assert!(
        !key.exists(),
        "must not silently generate a replacement key"
    );
    assert_eq!(encrypted_record(dir.path(), "a"), before);
}

#[test]
fn invalid_key_length_is_reported_without_overwriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "existing-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    drop(store);
    let key = dir.path().join("secrets/credentials.key");
    fs::write(&key, b"interrupted-key-write").unwrap();
    assert!(cannot_load(dir.path(), "a"));
    assert_eq!(fs::read(&key).unwrap(), b"interrupted-key-write");
    assert_eq!(encrypted_record(dir.path(), "a"), before);
}

#[test]
fn wrong_well_formed_key_does_not_decrypt_or_rewrite_records() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "existing-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    drop(store);
    let key = dir.path().join("secrets/credentials.key");
    let mut wrong_key = fs::read(&key).unwrap();
    assert_eq!(wrong_key.len(), 32);
    wrong_key[0] ^= 0x80;
    fs::write(&key, &wrong_key).unwrap();
    assert!(cannot_load(dir.path(), "a"));
    assert_eq!(fs::read(&key).unwrap(), wrong_key);
    assert_eq!(encrypted_record(dir.path(), "a"), before);
}

#[test]
fn successful_migration_is_persisted_and_never_reads_legacy_again() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let calls = Cell::new(0);
    let result = store
        .load_or_migrate("a", || {
            calls.set(calls.get() + 1);
            Ok("legacy-secret".to_string())
        })
        .unwrap();
    assert_eq!(result, "legacy-secret");
    assert_eq!(calls.get(), 1);
    assert_eq!(store.load("a").unwrap().as_deref(), Some("legacy-secret"));
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(
        reopened
            .load_or_migrate("a", || panic!("migration must not read legacy twice"))
            .unwrap(),
        "legacy-secret"
    );
}

#[test]
fn denied_legacy_access_leaves_no_partial_record_and_can_be_retried() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    assert!(
        store
            .load_or_migrate("a", || anyhow::bail!("legacy access denied"))
            .is_err()
    );
    assert_eq!(store.load("a").unwrap(), None);
    assert_eq!(
        store
            .load_or_migrate("a", || Ok("retry-secret".to_string()))
            .unwrap(),
        "retry-secret"
    );
}

#[test]
fn failed_local_write_cannot_report_a_successful_migration() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    database(dir.path())
        .execute_batch(
            "CREATE TRIGGER reject_credential_insert BEFORE INSERT ON account_credentials
             BEGIN SELECT RAISE(ABORT, 'simulated local write failure'); END;",
        )
        .unwrap();
    assert!(
        store
            .load_or_migrate("a", || Ok("legacy-still-intact".to_string()))
            .is_err()
    );
    assert_eq!(store.load("a").unwrap(), None);
    database(dir.path())
        .execute_batch("DROP TRIGGER reject_credential_insert")
        .unwrap();
    assert_eq!(
        store
            .load_or_migrate("a", || Ok("legacy-still-intact".to_string()))
            .unwrap(),
        "legacy-still-intact"
    );
}

#[test]
fn migration_verifies_the_persisted_ciphertext_before_reporting_success() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    database(dir.path())
        .execute_batch(
            "CREATE TRIGGER corrupt_credential_insert AFTER INSERT ON account_credentials
             BEGIN UPDATE account_credentials SET ciphertext=x'00' WHERE account_id=NEW.account_id; END;",
        )
        .unwrap();
    assert!(
        store
            .load_or_migrate("a", || Ok("legacy-must-remain-recoverable".to_string()))
            .is_err(),
        "a successful SQL insert alone does not prove a usable local credential"
    );
    assert_eq!(
        store.load("a").unwrap(),
        None,
        "verification failure must roll back the insert"
    );
    database(dir.path())
        .execute_batch("DROP TRIGGER corrupt_credential_insert")
        .unwrap();
    assert_eq!(
        store
            .load_or_migrate("a", || Ok("legacy-must-remain-recoverable".to_string()))
            .unwrap(),
        "legacy-must-remain-recoverable"
    );
}

#[test]
fn failed_update_and_delete_preserve_the_original_credential() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "original-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    let conn = database(dir.path());
    conn.execute_batch(
        "CREATE TRIGGER reject_credential_update BEFORE UPDATE ON account_credentials
         BEGIN SELECT RAISE(ABORT, 'simulated update failure'); END;
         CREATE TRIGGER reject_credential_delete BEFORE DELETE ON account_credentials
         BEGIN SELECT RAISE(ABORT, 'simulated delete failure'); END;",
    )
    .unwrap();
    assert!(store.save("a", "replacement-secret").is_err());
    assert!(store.remove("a").is_err());
    assert_eq!(store.load("a").unwrap().as_deref(), Some("original-secret"));
    assert_eq!(encrypted_record(dir.path(), "a"), before);
}

#[test]
fn concurrent_first_open_uses_one_durable_key_for_every_store() {
    use std::sync::{Arc, Barrier};

    let dir = tempfile::tempdir().unwrap();
    let barrier = Arc::new(Barrier::new(4));
    let handles: Vec<_> = (0..4)
        .map(|index| {
            let barrier = barrier.clone();
            let path = dir.path().to_path_buf();
            std::thread::spawn(move || {
                barrier.wait();
                let store = Store::open(&path).unwrap();
                store
                    .save(&format!("account-{index}"), &format!("secret-{index}"))
                    .unwrap();
            })
        })
        .collect();
    for handle in handles {
        handle.join().unwrap();
    }
    let store = Store::open(dir.path()).unwrap();
    for index in 0..4 {
        assert_eq!(
            store.load(&format!("account-{index}")).unwrap(),
            Some(format!("secret-{index}"))
        );
    }
}

#[test]
fn losing_the_key_while_running_rejects_new_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "original-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    let key = dir.path().join("secrets/credentials.key");
    fs::remove_file(&key).unwrap();

    assert!(store.save("a", "unrecoverable-update").is_err());
    assert!(store.save("b", "unrecoverable-insert").is_err());
    assert_eq!(encrypted_record(dir.path(), "a"), before);
    assert_eq!(store.load("b").unwrap(), None);
    assert!(!key.exists());
}

#[test]
fn replacing_the_key_while_running_rejects_new_writes() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "original-secret").unwrap();
    let before = encrypted_record(dir.path(), "a");
    let key = dir.path().join("secrets/credentials.key");
    let mut replacement = fs::read(&key).unwrap();
    replacement[0] ^= 0x80;
    fs::write(&key, &replacement).unwrap();

    assert!(store.save("a", "unrecoverable-update").is_err());
    assert!(store.save("b", "unrecoverable-insert").is_err());
    assert_eq!(encrypted_record(dir.path(), "a"), before);
    assert_eq!(store.load("b").unwrap(), None);
    assert_eq!(fs::read(&key).unwrap(), replacement);
}

fn assert_no_plaintext_files(dir: &Path, plaintext: &[u8]) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            assert_no_plaintext_files(&path, plaintext);
        } else {
            let bytes = fs::read(&path).unwrap();
            assert!(
                !bytes
                    .windows(plaintext.len())
                    .any(|window| window == plaintext),
                "plaintext credential was written into {}",
                path.display()
            );
        }
    }
}

#[test]
fn database_journal_and_key_files_never_contain_plaintext_credentials() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    let secret = "never-store-plaintext-credential-886382480f56";
    store.save("a", secret).unwrap();
    assert_no_plaintext_files(dir.path(), secret.as_bytes());
    store.remove("a").unwrap();
    drop(store);
    assert_no_plaintext_files(dir.path(), secret.as_bytes());
}

#[cfg(unix)]
#[test]
fn key_file_and_directory_are_accessible_only_to_the_current_user() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "permission-test-secret").unwrap();
    let key = dir.path().join("secrets/credentials.key");
    assert_eq!(fs::read(&key).unwrap().len(), 32);
    assert_eq!(
        fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        fs::metadata(key.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[cfg(windows)]
#[test]
fn local_key_and_directory_use_protected_user_and_system_acls() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "windows-private-secret").unwrap();
    let secrets = dir.path().join("secrets");
    let key = secrets.join("credentials.key");
    crate::private_fs::assert_private_acl(&secrets);
    crate::private_fs::assert_private_acl(&key);
    drop(store);
    let reopened = Store::open(dir.path()).unwrap();
    assert_eq!(
        reopened.load("a").unwrap().as_deref(),
        Some("windows-private-secret")
    );
}

#[cfg(windows)]
#[test]
fn local_key_directory_rejects_junction_redirect() {
    let dir = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    crate::private_fs::test_junction(outside.path(), &dir.path().join("secrets"));
    assert!(Store::open(dir.path()).is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}

#[test]
fn credential_table_coexists_with_the_existing_local_database() {
    let dir = tempfile::tempdir().unwrap();
    database(dir.path())
        .execute_batch(
            "CREATE TABLE prefs (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO prefs VALUES ('theme', 'dark');
             PRAGMA user_version=3;",
        )
        .unwrap();
    let store = Store::open(dir.path()).unwrap();
    store.save("a", "coexistence-secret").unwrap();
    let conn = database(dir.path());
    assert_eq!(
        conn.query_row("SELECT value FROM prefs WHERE key='theme'", [], |r| r
            .get::<_, String>(0))
            .unwrap(),
        "dark"
    );
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
            .unwrap(),
        3
    );
}

#[test]
fn interrupted_initial_key_write_is_not_silently_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("secrets")).unwrap();
    let key = dir.path().join("secrets/credentials.key");
    fs::write(&key, []).unwrap();
    assert!(Store::open(dir.path()).is_err());
    assert_eq!(fs::read(&key).unwrap(), Vec::<u8>::new());
}

#[test]
fn invalid_database_is_preserved_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("local.db");
    let contents = b"not-a-sqlite-database";
    fs::write(&path, contents).unwrap();
    assert!(Store::open(dir.path()).is_err());
    assert_eq!(fs::read(path).unwrap(), contents);
}
