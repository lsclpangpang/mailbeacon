use super::*;

use std::{
    io::{BufRead, BufReader},
    process::{Command, Stdio},
};

const CHILD_MODE: &str = "MAILBEACON_MIGRATION_FAULT_CHILD";
const CHILD_ROOT: &str = "MAILBEACON_MIGRATION_FAULT_ROOT";
const CHILD_TEST: &str = "app_migration::fault_tests::migration_fault_child";
const CRASH_EXIT: i32 = 86;

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join(LEGACY_ID);
    let target = root.path().join(CURRENT_ID);
    fs::create_dir(&source).unwrap();
    fs::write(source.join("mail"), b"original mail").unwrap();
    (root, source, target)
}

fn child(mode: &str, root: &Path) -> Command {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
        .env(CHILD_MODE, mode)
        .env(CHILD_ROOT, root);
    command
}

#[test]
fn migration_fault_child() {
    let Ok(mode) = std::env::var(CHILD_MODE) else {
        return;
    };
    let root = PathBuf::from(std::env::var_os(CHILD_ROOT).unwrap());
    match mode.as_str() {
        "lock" => {
            let lock = private_options().read(true).open(root.join(LOCK)).unwrap();
            lock.try_lock().unwrap();
            println!("MIGRATION_LOCK_READY");
            std::io::stdout().flush().unwrap();
            let mut proceed = [0];
            std::io::stdin().read_exact(&mut proceed).unwrap();
            // Skip Rust destructors: the OS must release the process-owned lock.
            std::process::exit(CRASH_EXIT);
        }
        "hot-journal" => {
            let conn = rusqlite::Connection::open(root.join(LEGACY_ID).join("local.db")).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=DELETE;
                 PRAGMA synchronous=FULL;
                 PRAGMA cache_size=1;
                 PRAGMA cache_spill=ON;
                 CREATE TABLE example(id INTEGER PRIMARY KEY, value TEXT, payload BLOB);
                 WITH RECURSIVE numbers(n) AS (
                     VALUES(1) UNION ALL SELECT n+1 FROM numbers WHERE n<24
                 )
                 INSERT INTO example SELECT n, 'committed', zeroblob(8192) FROM numbers;
                 BEGIN IMMEDIATE;
                 UPDATE example SET value='uncommitted', payload=zeroblob(16384);",
            )
            .unwrap();
            // A small cache forces dirty pages out before this uncommitted transaction exits.
            std::process::exit(CRASH_EXIT);
        }
        "stage" => {
            prepare_with(&root.join(CURRENT_ID), |phase| {
                if phase == Phase::Publishing {
                    println!("MIGRATION_STAGE_READY");
                    std::io::stdout().flush()?;
                    let mut proceed = [0];
                    std::io::stdin().read_exact(&mut proceed)?;
                }
                Ok(())
            })
            .unwrap();
        }
        _ => panic!("unknown migration child mode"),
    }
}

#[test]
fn another_process_blocks_migration_and_its_abnormal_exit_releases_the_lock() {
    let (root, source, target) = fixture();
    let mut holder = child("lock", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(holder.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "lock child exited before readiness"
        );
        if line.contains("MIGRATION_LOCK_READY") {
            break;
        }
    }
    let blocked = prepare_with(&target, |_| {
        panic!("a blocked migration must not begin copying")
    });
    assert!(blocked.is_err());
    assert!(!target.exists());
    assert_eq!(fs::read(source.join("mail")).unwrap(), b"original mail");
    holder.stdin.take().unwrap().write_all(b"x").unwrap();
    assert_eq!(holder.wait().unwrap().code(), Some(CRASH_EXIT));
    prepare_with(&target, |_| Ok(())).unwrap();
    assert_eq!(fs::read(target.join("mail")).unwrap(), b"original mail");
    prepare_with(&target, |_| Ok(())).unwrap();
}

#[test]
fn hot_rollback_journal_is_recovered_only_in_the_migration_copy() {
    let (root, source, target) = fixture();
    let output = child("hot-journal", root.path()).output().unwrap();
    assert_eq!(
        output.status.code(),
        Some(CRASH_EXIT),
        "journal fixture failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let original_db = fs::read(source.join("local.db")).unwrap();
    let original_journal = fs::read(source.join("local.db-journal")).unwrap();
    // SQLite's hot-journal header proves this is not merely an empty leftover filename.
    assert_eq!(
        &original_journal[..8],
        &[0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7]
    );
    prepare_with(&target, |_| Ok(())).unwrap();
    let conn = rusqlite::Connection::open(target.join("local.db")).unwrap();
    let (total, committed): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), SUM(value='committed') FROM example",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((total, committed), (24, 24));
    assert_eq!(fs::read(source.join("local.db")).unwrap(), original_db);
    assert_eq!(
        fs::read(source.join("local.db-journal")).unwrap(),
        original_journal
    );
}

#[test]
fn added_or_removed_source_entries_prevent_publication_and_allow_a_clean_retry() {
    for phase in [Phase::Copied, Phase::Publishing] {
        for mutation in [
            "add-file",
            "remove-file",
            "add-directory",
            "remove-directory",
        ] {
            let (_root, source, target) = fixture();
            fs::write(source.join("removed"), b"remove me").unwrap();
            fs::create_dir(source.join("empty")).unwrap();
            let result = prepare_with(&target, |at| {
                if at == phase {
                    match mutation {
                        "add-file" => fs::write(source.join("new"), b"new mail")?,
                        "remove-file" => fs::remove_file(source.join("removed"))?,
                        "add-directory" => fs::create_dir(source.join("new-directory"))?,
                        _ => fs::remove_dir(source.join("empty"))?,
                    }
                }
                Ok(())
            });
            assert!(result.is_err(), "{phase:?}: {mutation}");
            assert!(!target.exists(), "{phase:?}: {mutation}");
            assert_eq!(fs::read(source.join("mail")).unwrap(), b"original mail");
            prepare_with(&target, |_| Ok(())).unwrap();
            for name in ["mail", "removed", "empty", "new", "new-directory"] {
                assert_eq!(
                    source.join(name).exists(),
                    target.join(name).exists(),
                    "{name}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn swapping_the_source_root_for_an_identical_symlink_still_aborts() {
    use std::os::unix::fs::symlink;

    for phase in [Phase::Copied, Phase::Publishing] {
        let (root, source, target) = fixture();
        let preserved = root.path().join("preserved-source");
        let outside = root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("mail"), b"original mail").unwrap();
        let result = prepare_with(&target, |at| {
            if at == phase {
                fs::rename(&source, &preserved)?;
                symlink(&outside, &source)?;
            }
            Ok(())
        });
        assert!(result.is_err(), "{phase:?}");
        assert!(!target.exists());
        assert!(
            fs::symlink_metadata(&source)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(preserved.join("mail")).unwrap(), b"original mail");
        assert_eq!(fs::read(outside.join("mail")).unwrap(), b"original mail");
        assert_eq!(fs::read_dir(&outside).unwrap().count(), 1);
    }
}

#[test]
fn invalid_completion_markers_cannot_bypass_conflicting_target_protection() {
    let mut changed_version = migration_marker();
    changed_version["version"] = serde_json::json!(2);
    let mut changed_source = migration_marker();
    changed_source["from"] = serde_json::json!("unrelated.application");
    let mut changed_target = migration_marker();
    changed_target["to"] = serde_json::json!(LEGACY_ID);
    for marker in [
        b"{".to_vec(),
        b"null".to_vec(),
        serde_json::to_vec(&changed_version).unwrap(),
        serde_json::to_vec(&changed_source).unwrap(),
        serde_json::to_vec(&changed_target).unwrap(),
    ] {
        let (_root, source, target) = fixture();
        fs::create_dir(&target).unwrap();
        fs::write(target.join("draft"), b"existing new draft").unwrap();
        fs::write(target.join(MARKER), &marker).unwrap();
        assert!(prepare_with(&target, |_| Ok(())).is_err());
        assert_eq!(
            fs::read(target.join("draft")).unwrap(),
            b"existing new draft"
        );
        assert_eq!(fs::read(target.join(MARKER)).unwrap(), marker);
        assert!(!target.join("mail").exists());
        assert_eq!(fs::read(source.join("mail")).unwrap(), b"original mail");
    }
}

#[test]
fn a_completed_copy_cannot_be_reused_as_an_unmarked_legacy_source() {
    let (_root, source, target) = fixture();
    let marker = serde_json::to_vec(&migration_marker()).unwrap();
    fs::write(source.join(MARKER), &marker).unwrap();
    assert!(prepare_with(&target, |_| Ok(())).is_err());
    assert!(!target.exists());
    assert_eq!(fs::read(source.join(MARKER)).unwrap(), marker);
    assert_eq!(fs::read(source.join("mail")).unwrap(), b"original mail");
}

#[test]
fn migration_preserves_cached_mail_modification_times_and_bytes() {
    use std::time::{Duration, UNIX_EPOCH};

    let (_root, source, target) = fixture();
    let cached = source.join("bodies/account/INBOX");
    fs::create_dir_all(&cached).unwrap();
    for (name, seconds, bytes) in [
        ("old.eml", 1_600_000_000, b"older cached mail".as_slice()),
        ("new.eml", 1_700_000_000, b"newer cached mail".as_slice()),
    ] {
        let path = cached.join(name);
        fs::write(&path, bytes).unwrap();
        let file = OpenOptions::new().write(true).open(&path).unwrap();
        file.set_times(
            fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds)),
        )
        .unwrap();
    }
    prepare_with(&target, |_| Ok(())).unwrap();
    for name in ["old.eml", "new.eml"] {
        let relative = Path::new("bodies/account/INBOX").join(name);
        assert_eq!(
            fs::metadata(target.join(&relative))
                .unwrap()
                .modified()
                .unwrap(),
            fs::metadata(source.join(&relative))
                .unwrap()
                .modified()
                .unwrap(),
            "cache eviction order must survive migration: {name}"
        );
        assert_eq!(
            fs::read(target.join(&relative)).unwrap(),
            fs::read(source.join(&relative)).unwrap()
        );
    }
}

#[test]
fn killed_migration_reclaims_only_owned_staging_directories_after_obtaining_the_lock() {
    let (root, source, target) = fixture();
    let mut migrating = child("stage", root.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(migrating.stdout.take().unwrap());
    loop {
        let mut line = String::new();
        assert!(
            output.read_line(&mut line).unwrap() > 0,
            "migration child exited before staging completed"
        );
        if line.contains("MIGRATION_STAGE_READY") {
            break;
        }
    }
    let stages: Vec<_> = fs::read_dir(root.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".mailbeacon-migrating-")
        })
        .collect();
    assert_eq!(stages.len(), 1);
    let stage = &stages[0];
    let marker = serde_json::to_vec(&migration_marker()).unwrap();
    assert_eq!(fs::read(stage.join(MARKER)).unwrap(), marker);
    let empty_stage = root.path().join(".mailbeacon-migrating-empty");
    fs::create_dir(&empty_stage).unwrap();

    let unrelated = root.path().join("unrelated-directory");
    fs::create_dir(&unrelated).unwrap();
    fs::write(unrelated.join(MARKER), &marker).unwrap();
    fs::write(unrelated.join("keep"), b"unrelated data").unwrap();
    let unmarked = root.path().join(".mailbeacon-migrating-unmarked");
    fs::create_dir(&unmarked).unwrap();
    fs::write(unmarked.join("keep"), b"unknown ownership").unwrap();
    let invalid = root.path().join(".mailbeacon-migrating-invalid");
    fs::create_dir(&invalid).unwrap();
    fs::write(invalid.join(MARKER), b"{invalid}").unwrap();
    let other_marker = root.path().join(".mailbeacon-migrating-other-version");
    fs::create_dir(&other_marker).unwrap();
    let mut changed_marker = migration_marker();
    changed_marker["version"] = serde_json::json!(999);
    let changed_marker = serde_json::to_vec(&changed_marker).unwrap();
    fs::write(other_marker.join(MARKER), &changed_marker).unwrap();

    #[cfg(unix)]
    let (linked_root, linked_marker) = {
        use std::os::unix::fs::symlink;
        let linked_root = root.path().join(".mailbeacon-migrating-symlink");
        symlink(&unrelated, &linked_root).unwrap();
        let linked_marker = root.path().join(".mailbeacon-migrating-linked-marker");
        fs::create_dir(&linked_marker).unwrap();
        symlink(unrelated.join(MARKER), linked_marker.join(MARKER)).unwrap();
        fs::write(linked_marker.join("keep"), b"linked ownership").unwrap();
        (linked_root, linked_marker)
    };

    assert!(prepare_with(&target, |_| Ok(())).is_err());
    assert!(stage.exists(), "a live migration must not be reclaimed");
    assert!(empty_stage.exists(), "cleanup requires the migration lock");
    assert!(!target.exists());
    migrating.kill().unwrap();
    assert!(!migrating.wait().unwrap().success());
    assert!(stage.exists(), "kill must bypass temporary-directory Drop");
    assert_eq!(fs::read(stage.join("mail")).unwrap(), b"original mail");

    prepare_with(&target, |_| Ok(())).unwrap();
    assert!(!stage.exists(), "reclaim the interrupted owned copy");
    assert!(!empty_stage.exists(), "reclaim an empty staging directory");
    assert_eq!(fs::read(target.join("mail")).unwrap(), b"original mail");
    assert_eq!(fs::read(source.join("mail")).unwrap(), b"original mail");
    assert_eq!(fs::read(unrelated.join("keep")).unwrap(), b"unrelated data");
    assert_eq!(fs::read(unrelated.join(MARKER)).unwrap(), marker);
    assert_eq!(
        fs::read(unmarked.join("keep")).unwrap(),
        b"unknown ownership"
    );
    assert_eq!(fs::read(invalid.join(MARKER)).unwrap(), b"{invalid}");
    assert_eq!(fs::read(other_marker.join(MARKER)).unwrap(), changed_marker);
    #[cfg(unix)]
    {
        assert!(
            fs::symlink_metadata(linked_root)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(
            fs::symlink_metadata(linked_marker.join(MARKER))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read(linked_marker.join("keep")).unwrap(),
            b"linked ownership"
        );
    }
}
