//! Local cache retention primitives. Dates are server INTERNALDATE milliseconds,
//! never filesystem modification times or the sender's Date header.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheOrigin {
    Interactive,
    Background,
}

#[derive(Clone, Debug)]
pub struct CacheWriteTicket {
    pub(crate) account: String,
    pub(crate) generation: u64,
    pub(crate) origin: CacheOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CacheKind {
    Raw,
    Pending,
}

#[derive(Clone, Debug)]
pub struct CacheEntry {
    pub kind: CacheKind,
    pub account: String,
    pub folder_hash: String,
    pub folder: Option<String>,
    pub uid_validity: u32,
    pub uid: u32,
    pub date: i64,
    pub bytes: u64,
    pub(crate) paths: Vec<PathBuf>,
}

impl CacheEntry {
    pub fn matches(&self, cutoff: Option<i64>) -> bool {
        cutoff.is_none_or(|cutoff| self.date > 0 && self.date < cutoff)
    }
}

#[derive(Clone, Default, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CleanupReport {
    pub count: u64,
    pub bytes: u64,
    pub unknown_date_count: u64,
    pub removed_count: u64,
    pub freed_bytes: u64,
    pub failed_count: u64,
}

impl CleanupReport {
    pub fn merge(&mut self, other: &Self) {
        self.count += other.count;
        self.bytes += other.bytes;
        self.unknown_date_count += other.unknown_date_count;
        self.removed_count += other.removed_count;
        self.freed_bytes += other.freed_bytes;
        self.failed_count += other.failed_count;
    }

    pub(crate) fn preview(entries: &[CacheEntry], cutoff: Option<i64>) -> Self {
        let mut report = Self::default();
        for entry in entries {
            report.unknown_date_count += u64::from(entry.date <= 0);
            if entry.matches(cutoff) {
                report.count += 1;
                report.bytes += entry.bytes;
            }
        }
        report
    }
}

#[derive(Serialize, Deserialize)]
struct Policy {
    version: u8,
    account: String,
    background_after_ms: i64,
}

pub(crate) struct CleanupState {
    pub generation: u64,
    // None is a corrupt/unreadable saved policy: background writes fail closed,
    // while an explicitly opened message can still be cached interactively.
    floor: Option<i64>,
}

impl CleanupState {
    pub fn load(dir: &Path, account: &str) -> Self {
        let path = dir.join(".cache-retention.json");
        let floor = match fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(0),
            Ok(meta)
                if meta.is_file() && !crate::private_fs::is_link(&meta) && meta.len() <= 65536 =>
            {
                fs::read(path)
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<Policy>(&bytes).ok())
                    .filter(|policy| {
                        policy.version == 1
                            && policy.account == account
                            && policy.background_after_ms >= 0
                    })
                    .map(|policy| policy.background_after_ms)
            }
            _ => None,
        };
        Self {
            generation: 0,
            floor,
        }
    }

    pub fn background_allowed(&self, date: i64) -> bool {
        self.floor
            .is_some_and(|floor| floor == 0 || (date > 0 && date >= floor))
    }

    pub fn ticket_valid(&self, ticket: &CacheWriteTicket, date: i64) -> bool {
        ticket.generation == self.generation
            && (ticket.origin == CacheOrigin::Interactive || self.background_allowed(date))
    }

    pub fn persist_cleanup(&mut self, dir: &Path, account: &str, floor: i64) -> Result<()> {
        ensure!(floor >= 0, "缓存清理日期无效");
        let previous = self.floor.context("本地缓存清理规则无法读取，未执行清理")?;
        let policy = Policy {
            version: 1,
            account: account.into(),
            background_after_ms: previous.max(floor),
        };
        let generation = self
            .generation
            .checked_add(1)
            .context("缓存清理代际超出范围")?;
        crate::private_fs::create_private_dir(dir)?;
        crate::durable_fs::atomic_write(
            &dir.join(".cache-retention.json"),
            &serde_json::to_vec(&policy)?,
        )?;
        self.floor = Some(policy.background_after_ms);
        self.generation = generation;
        Ok(())
    }
}

pub(crate) fn file_identity(name: &str, suffix: &str) -> Option<(String, u32, u32)> {
    let stem = name.strip_suffix(suffix)?;
    let (prefix, uid) = stem.rsplit_once('-')?;
    let (folder, validity) = prefix.rsplit_once('-')?;
    if folder.len() != 16 || !folder.bytes().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some((folder.into(), validity.parse().ok()?, uid.parse().ok()?))
}

/// Only direct regular files are eligible; never follow links outside AppData.
pub(crate) fn regular_len(path: &Path) -> Option<u64> {
    let meta = fs::symlink_metadata(path).ok()?;
    (meta.is_file() && !crate::private_fs::is_link(&meta)).then_some(meta.len())
}

pub(crate) fn remove_entry(entry: &CacheEntry) -> CleanupReport {
    let mut result = CleanupReport::default();
    let mut removed_any = false;
    let mut failed = false;
    for path in &entry.paths {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Ok(meta) if meta.is_file() && !crate::private_fs::is_link(&meta) => {
                match fs::remove_file(path) {
                    Ok(()) => {
                        result.freed_bytes += meta.len();
                        removed_any = true;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => {
                        result.failed_count += 1;
                        failed = true;
                    }
                }
            }
            _ => {
                result.failed_count += 1;
                failed = true;
            }
        }
    }
    result.removed_count = u64::from(removed_any && !failed);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body_store::{BodyStore, DEFAULT_LIMIT};
    use std::sync::{Arc, mpsc};

    fn store() -> (tempfile::TempDir, BodyStore) {
        let root = tempfile::tempdir().unwrap();
        let store = BodyStore::new(root.path().join("bodies"), DEFAULT_LIMIT);
        (root, store)
    }

    #[test]
    fn a_late_completion_cannot_erase_a_new_explicit_resume_checkpoint() {
        let (_root, store) = store();
        let old = store.begin_write("a", CacheOrigin::Interactive);
        store
            .put_with_ticket(&old, "INBOX", 7, 1, b"complete", 40)
            .unwrap();
        store
            .cleanup_with("a", None, 1000, |_| Ok(Default::default()))
            .unwrap();
        let new = store.begin_write("a", CacheOrigin::Interactive);
        store
            .append_resume_with_ticket(&new, "INBOX", 7, 1, 8, 40, 0, b"head")
            .unwrap();
        assert!(!store.clear_resume_with_ticket(&old, "INBOX", 7, 1).unwrap());
        assert_eq!(store.load_resume("a", "INBOX", 7, 1).unwrap().raw, b"head");
    }

    #[test]
    fn preview_uses_received_dates_and_cleanup_preserves_other_accounts_and_unknown_dates() {
        let (_root, store) = store();
        store.put("a", "INBOX", 7, 1, b"old", 40);
        store.put("a", "INBOX", 7, 2, b"new", 150);
        store.put("a", "INBOX", 7, 3, b"unknown", 0);
        store.put("b", "INBOX", 7, 1, b"other", 30);
        store
            .append_resume("a", "INBOX", 7, 4, 8, 50, 0, b"head")
            .unwrap();
        let old = store.begin_write("a", CacheOrigin::Interactive);
        let other = store.begin_write("b", CacheOrigin::Background);
        let preview = store.preview_cleanup("a", Some(100)).unwrap();
        assert_eq!(preview.count, 2);
        assert_eq!(preview.unknown_date_count, 1);
        assert_eq!(store.received_date("a", "INBOX", 7, 1), Some(40));
        assert!(
            store.ticket_valid(&old, 40),
            "preview must not revoke writers"
        );
        let removed = store
            .cleanup_with("a", Some(100), 1000, |entries| {
                assert_eq!(entries.len(), 4);
                assert_eq!(store.received_date("a", "INBOX", 7, 1), Some(40));
                Ok(CleanupReport::default())
            })
            .unwrap();
        assert_eq!(removed.count, 2);
        assert_eq!(removed.removed_count, 2);
        assert_eq!(removed.bytes, preview.bytes);
        assert_eq!(removed.freed_bytes, preview.bytes);
        assert_eq!(removed.failed_count, 0);
        assert!(store.get("a", "INBOX", 7, 1).is_none());
        assert!(store.load_resume("a", "INBOX", 7, 4).is_none());
        assert!(store.get("a", "INBOX", 7, 2).is_some());
        assert!(store.get("a", "INBOX", 7, 3).is_some());
        assert!(store.get("b", "INBOX", 7, 1).is_some());
        assert_eq!(store.usage("a").0, 2);
        assert!(!store.ticket_valid(&old, 150));
        assert!(store.ticket_valid(&other, 30));
    }

    #[test]
    fn cleanup_floor_survives_restart_and_explicit_reopen_can_cache_old_mail_again() {
        let (root, store) = store();
        store.put("a", "INBOX", 7, 1, b"old", 40);
        store
            .append_resume("a", "INBOX", 7, 2, 8, 40, 0, b"head")
            .unwrap();
        let stale = store.begin_write("a", CacheOrigin::Interactive);
        store
            .cleanup_with("a", Some(100), 1000, |_| Ok(CleanupReport::default()))
            .unwrap();
        assert!(
            !store
                .put_with_ticket(&stale, "INBOX", 7, 1, b"late", 40)
                .unwrap()
        );
        assert!(
            !store
                .append_resume_with_ticket(&stale, "INBOX", 7, 2, 8, 40, 0, b"late")
                .unwrap()
        );
        let interactive = store.begin_write("a", CacheOrigin::Interactive);
        assert!(
            store
                .put_with_ticket(&interactive, "INBOX", 7, 1, b"manual", 40)
                .unwrap()
        );
        assert!(
            store
                .append_resume_with_ticket(&interactive, "INBOX", 7, 2, 8, 40, 0, b"new!")
                .unwrap()
        );
        assert_eq!(store.load_resume("a", "INBOX", 7, 2).unwrap().raw, b"new!");
        drop(store);
        let reopened = BodyStore::new(root.path().join("bodies"), DEFAULT_LIMIT);
        assert!(!reopened.background_allowed("a", 99));
        assert!(!reopened.background_allowed("a", 0));
        assert!(reopened.background_allowed("a", 100));
        assert!(reopened.background_allowed("b", 40));
        let background = reopened.begin_write("a", CacheOrigin::Background);
        assert!(
            !reopened
                .put_with_ticket(&background, "INBOX", 7, 3, b"old", 40)
                .unwrap()
        );
        assert!(
            reopened
                .put_with_ticket(&background, "INBOX", 7, 3, b"new", 101)
                .unwrap()
        );
    }

    #[test]
    fn clear_all_removes_unknown_dates_but_allows_future_arrivals() {
        let (_root, store) = store();
        store.put("a", "INBOX", 7, 1, b"unknown", 0);
        store
            .append_resume("a", "INBOX", 7, 2, 8, 0, 0, b"head")
            .unwrap();
        let report = store
            .cleanup_with("a", None, 1000, |_| Ok(CleanupReport::default()))
            .unwrap();
        assert_eq!(report.unknown_date_count, 2);
        assert_eq!(report.removed_count, 2);
        assert!(!store.background_allowed("a", 999));
        assert!(!store.background_allowed("a", 0));
        assert!(store.background_allowed("a", 1000));
        let future = store.begin_write("a", CacheOrigin::Background);
        assert!(
            store
                .put_with_ticket(&future, "INBOX", 7, 3, b"future", 1001)
                .unwrap()
        );
    }

    #[test]
    fn policy_write_failure_deletes_nothing_and_keeps_existing_tickets_valid() {
        let (_root, store) = store();
        store.put("a", "INBOX", 7, 1, b"old", 40);
        let ticket = store.begin_write("a", CacheOrigin::Interactive);
        let entries = store.cache_entries("a").unwrap();
        let policy = entries[0].paths[0]
            .parent()
            .unwrap()
            .join(".cache-retention.json");
        fs::create_dir(&policy).unwrap();
        assert!(
            store
                .cleanup_with("a", None, 1000, |_| {
                    panic!("deletion callback must not run before durable policy publication")
                })
                .is_err()
        );
        assert!(store.get("a", "INBOX", 7, 1).is_some());
        assert!(store.ticket_valid(&ticket, 40));
    }

    #[test]
    fn a_writer_waiting_on_cleanup_cannot_publish_with_its_old_ticket() {
        let (_root, store) = store();
        let store = Arc::new(store);
        store.put("a", "INBOX", 7, 1, b"old", 40);
        let old = store.begin_write("a", CacheOrigin::Interactive);
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let cleaner = {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .cleanup_with("a", None, 1000, |_| {
                        entered_tx.send(()).unwrap();
                        release_rx.recv().unwrap();
                        Ok(CleanupReport::default())
                    })
                    .unwrap()
            })
        };
        entered_rx.recv().unwrap();
        let writer = {
            let store = store.clone();
            std::thread::spawn(move || {
                store
                    .put_with_ticket(&old, "INBOX", 7, 1, b"late", 40)
                    .unwrap()
            })
        };
        release_tx.send(()).unwrap();
        assert_eq!(cleaner.join().unwrap().removed_count, 1);
        assert!(!writer.join().unwrap());
        assert!(store.get("a", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn deleting_a_message_revokes_its_pending_final_publication() {
        let (_root, store) = store();
        let ticket = store.begin_write("a", CacheOrigin::Background);
        store
            .append_resume_with_ticket(&ticket, "INBOX", 7, 1, 4, 40, 0, b"mail")
            .unwrap();
        store.remove("a", "INBOX", Some(&[1]));
        assert!(
            !store
                .put_with_ticket(&ticket, "INBOX", 7, 1, b"mail", 40)
                .unwrap()
        );
        assert!(store.get("a", "INBOX", 7, 1).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_follow_symlinks_outside_its_account_cache() {
        use std::os::unix::fs::symlink;
        let (root, store) = store();
        store.put("a", "INBOX", 7, 1, b"old", 40);
        let entries = store.cache_entries("a").unwrap();
        let dir = entries[0].paths[0].parent().unwrap();
        let external = root.path().join("saved.eml");
        fs::write(&external, b"user archive").unwrap();
        symlink(
            &external,
            dir.join(format!(
                "{}-7-2.eml",
                crate::body_store::folder_hash("INBOX")
            )),
        )
        .unwrap();
        let report = store
            .cleanup_with("a", None, 1000, |_| Ok(CleanupReport::default()))
            .unwrap();
        assert_eq!(report.removed_count, 1);
        assert_eq!(fs::read(external).unwrap(), b"user archive");
    }
}
