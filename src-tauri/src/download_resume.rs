//! Confirmed IMAP ranges, kept separately from complete cached messages.
//!
//! Data is synced before an atomic metadata replacement commits its length.
//! A crash can leave an extra data tail, but readers never return that tail.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};

pub(crate) const MAX_BYTES: usize = 128 * 1024 * 1024;
const MAX_PENDING: usize = 3;

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resume {
    pub raw: Vec<u8>,
    pub date: i64,
    pub total: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct Identity {
    account: String,
    folder: String,
    validity: u32,
    uid: u32,
}

impl Identity {
    pub fn new(account: &str, folder: &str, validity: u32, uid: u32) -> Self {
        Self {
            account: account.into(),
            folder: normalized_folder(folder),
            validity,
            uid,
        }
    }
}

fn normalized_folder(folder: &str) -> String {
    if folder.eq_ignore_ascii_case("INBOX") {
        "INBOX".into()
    } else {
        folder.into()
    }
}

#[derive(Serialize, Deserialize)]
struct Commit {
    version: u8,
    identity: Identity,
    total: u32,
    date: i64,
    committed: usize,
}

#[derive(Default)]
struct State {
    accounts: HashSet<String>,
    messages: HashSet<Identity>,
    /// Downloads may be awaiting their first confirmed range and have no file yet.
    active: HashMap<PathBuf, Identity>,
}

impl State {
    fn removed(&self, identity: &Identity) -> bool {
        self.accounts.contains(&identity.account) || self.messages.contains(identity)
    }
}

/// One lock covers append/eviction/removal so a late confirmed chunk cannot
/// resurrect a message, folder, or receiving endpoint removed in this process.
#[derive(Default)]
pub(crate) struct Store(Mutex<State>);

fn metadata_path(path: &Path) -> PathBuf {
    path.with_extension("partial.json")
}

fn clear(path: &Path) {
    // Forget the commit first: an interrupted cleanup can only leave ignored data.
    let _ = fs::remove_file(metadata_path(path));
    let _ = fs::remove_file(path);
}

fn commit(path: &Path) -> Option<Commit> {
    let meta_path = metadata_path(path);
    let metadata = fs::symlink_metadata(&meta_path).ok()?;
    if !metadata.is_file() || crate::private_fs::is_link(&metadata) || metadata.len() > 64 * 1024 {
        return None;
    }
    let saved: Commit = serde_json::from_slice(&fs::read(meta_path).ok()?).ok()?;
    let data = fs::symlink_metadata(path).ok()?;
    (saved.version == 1
        && saved.identity.validity > 0
        && saved.identity.uid > 0
        && saved.total > 0
        && saved.total as usize <= MAX_BYTES
        && saved.committed > 0
        && saved.committed <= saved.total as usize
        && data.is_file()
        && !crate::private_fs::is_link(&data)
        && data.len() >= saved.committed as u64
        && data.len() <= MAX_BYTES as u64)
        .then_some(saved)
}

impl Store {
    pub fn load(&self, path: &Path, identity: &Identity) -> Option<Resume> {
        let mut state = self.0.lock().unwrap();
        if state.removed(identity) || identity.validity == 0 || identity.uid == 0 {
            return None;
        }
        state.active.insert(path.to_path_buf(), identity.clone());
        let saved = commit(path)?;
        if saved.identity != *identity {
            return None;
        }
        let mut raw = Vec::with_capacity(saved.committed);
        fs::File::open(path)
            .ok()?
            .take(saved.committed as u64)
            .read_to_end(&mut raw)
            .ok()?;
        if raw.len() != saved.committed {
            return None;
        }
        Some(Resume {
            raw,
            date: saved.date,
            total: saved.total,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn append(
        &self,
        path: &Path,
        identity: Identity,
        total: u32,
        date: i64,
        offset: usize,
        bytes: &[u8],
    ) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        if state.removed(&identity) {
            bail!("邮件的续传状态已移除");
        }
        let end = offset
            .checked_add(bytes.len())
            .context("续传位置超出范围")?;
        if identity.validity == 0
            || identity.uid == 0
            || total == 0
            || total as usize > MAX_BYTES
            || end > total as usize
            || bytes.is_empty()
        {
            bail!("续传邮件大小或位置无效");
        }
        if offset > 0 {
            let saved = commit(path).context("续传状态不存在或不完整，请重新下载")?;
            if saved.identity != identity
                || saved.total != total
                || saved.date != date
                || saved.committed != offset
            {
                bail!("续传邮件身份、大小或位置已变化");
            }
        }
        state.active.insert(path.to_path_buf(), identity.clone());
        let dir = path.parent().context("续传目录无效")?;
        crate::private_fs::create_private_dir(dir)?;
        prune(dir, path, end, MAX_BYTES, MAX_PENDING)?;
        if fs::symlink_metadata(path).is_ok() {
            crate::private_fs::protect_file(path)?;
        }
        if offset == 0 && fs::symlink_metadata(metadata_path(path)).is_ok() {
            // A deliberate restart may change total/date. Retire the old commit
            // durably before replacing bytes it used to describe.
            crate::durable_fs::atomic_write(&metadata_path(path), b"")?;
        }
        let mut options = fs::OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path)?;
        crate::private_fs::protect_file(path)?;
        // Discard only uncommitted bytes, then append the already-confirmed range.
        file.set_len(offset as u64)?;
        file.seek(SeekFrom::Start(offset as u64))?;
        file.write_all(bytes)?;
        file.sync_all()?;
        let saved = Commit {
            version: 1,
            identity,
            total,
            date,
            committed: end,
        };
        crate::durable_fs::atomic_write(&metadata_path(path), &serde_json::to_vec(&saved)?)?;
        Ok(())
    }

    pub fn clear(&self, path: &Path) {
        let mut state = self.0.lock().unwrap();
        state.active.remove(path);
        clear(path);
    }

    pub fn remove(
        &self,
        dir: &Path,
        account: &str,
        folder: &str,
        filename_prefix: &str,
        uids: Option<&[u32]>,
    ) {
        let mut state = self.0.lock().unwrap();
        let folder = normalized_folder(folder);
        let active: Vec<_> = state
            .active
            .iter()
            .filter(|(_, identity)| {
                identity.account == account
                    && identity.folder == folder
                    && uids.is_none_or(|uids| uids.contains(&identity.uid))
            })
            .map(|(path, identity)| (path.clone(), identity.clone()))
            .collect();
        // Retire in-flight identities even when no checkpoint directory exists.
        for (path, identity) in active {
            state.messages.insert(identity);
            state.active.remove(&path);
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut paths = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(rest) = name
                .to_str()
                .and_then(|name| name.strip_prefix(filename_prefix))
            else {
                continue;
            };
            let uid = rest
                .strip_suffix(".partial")
                .or_else(|| rest.strip_suffix(".partial.json"))
                .and_then(|rest| rest.rsplit('-').next())
                .and_then(|uid| uid.parse::<u32>().ok());
            if uids.is_none_or(|uids| uid.is_some_and(|uid| uids.contains(&uid))) {
                paths.push(entry.path());
            }
        }
        // Record complete identities before removing either side of each pair.
        // New UIDVALIDITY values must remain free to start fresh checkpoints.
        for path in &paths {
            if let Some(saved) = commit(path)
                && saved.identity.account == account
                && saved.identity.folder == folder
            {
                state.messages.insert(saved.identity);
            }
        }
        for path in paths {
            let _ = fs::remove_file(path);
        }
    }

    pub fn remove_account(&self, dir: &Path, account: &str) {
        let mut state = self.0.lock().unwrap();
        state.accounts.insert(account.to_owned());
        state.messages.retain(|saved| saved.account != account);
        state.active.retain(|_, saved| saved.account != account);
        let _ = fs::remove_dir_all(dir);
    }
}

/// Bound confirmed checkpoints independently of the normal complete-body cache.
fn prune(
    dir: &Path,
    current: &Path,
    incoming: usize,
    max_bytes: usize,
    max_count: usize,
) -> Result<()> {
    let mut others = Vec::new();
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path == current
            || path
                .extension()
                .is_none_or(|extension| extension != "partial")
        {
            continue;
        }
        let Some(saved) = commit(&path) else {
            clear(&path);
            if fs::symlink_metadata(&path).is_ok() {
                bail!("无法清理无效的邮件续传数据");
            }
            continue;
        };
        // A crashed append's tail is not retained or charged as useful progress.
        fs::OpenOptions::new()
            .write(true)
            .open(&path)?
            .set_len(saved.committed as u64)?;
        let modified = fs::metadata(metadata_path(&path))?
            .modified()
            .unwrap_or(SystemTime::UNIX_EPOCH);
        others.push((path, saved.committed, modified));
    }
    others.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
    let mut bytes: usize = incoming + others.iter().map(|(_, size, _)| size).sum::<usize>();
    let mut count = 1 + others.len();
    for (path, size, _) in others {
        if bytes <= max_bytes && count <= max_count {
            break;
        }
        clear(&path);
        if path.exists() {
            bail!("无法清理较早的邮件续传数据");
        }
        bytes -= size;
        count -= 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(uid: u32) -> Identity {
        Identity::new("account", "INBOX", 7, uid)
    }

    fn path(dir: &Path, uid: u32) -> PathBuf {
        dir.join(format!("folder-7-{uid}.partial"))
    }

    #[test]
    fn restart_recovers_only_confirmed_bytes_and_next_append_overwrites_crash_tail() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("account/pending");
        let file = path(&dir, 1);
        Store::default()
            .append(&file, key(1), 12, 42, 0, b"confirmed")
            .unwrap();
        fs::OpenOptions::new()
            .append(true)
            .open(&file)
            .unwrap()
            .write_all(b"BAD")
            .unwrap();
        let reopened = Store::default();
        assert_eq!(
            reopened.load(&file, &key(1)).unwrap(),
            Resume {
                raw: b"confirmed".to_vec(),
                date: 42,
                total: 12
            }
        );
        reopened.append(&file, key(1), 12, 42, 9, b"new").unwrap();
        assert_eq!(reopened.load(&file, &key(1)).unwrap().raw, b"confirmednew");
    }

    #[test]
    fn missing_torn_or_inconsistent_metadata_never_becomes_progress() {
        let root = tempfile::tempdir().unwrap();
        let file = path(root.path(), 1);
        let store = Store::default();
        fs::write(&file, b"unconfirmed").unwrap();
        assert!(store.load(&file, &key(1)).is_none());
        fs::write(metadata_path(&file), b"{\"version\":1,").unwrap();
        assert!(store.load(&file, &key(1)).is_none());
        store
            .append(&file, key(1), 20, 42, 0, b"confirmed")
            .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&file)
            .unwrap()
            .set_len(2)
            .unwrap();
        assert!(store.load(&file, &key(1)).is_none());
        assert!(store.append(&file, key(1), 20, 42, 9, b"next").is_err());
    }

    #[test]
    fn source_total_date_and_offset_must_match_before_appending() {
        let root = tempfile::tempdir().unwrap();
        let file = path(root.path(), 1);
        let store = Store::default();
        store.append(&file, key(1), 20, 42, 0, b"first").unwrap();
        for other in [
            Identity::new("other", "INBOX", 7, 1),
            Identity::new("account", "Archive", 7, 1),
            Identity::new("account", "INBOX", 8, 1),
            key(2),
        ] {
            assert!(store.load(&file, &other).is_none());
            assert!(store.append(&file, other, 20, 42, 5, b"next").is_err());
        }
        for (total, date, offset) in [(21, 42, 5), (20, 43, 5), (20, 42, 4)] {
            assert!(
                store
                    .append(&file, key(1), total, date, offset, b"next")
                    .is_err()
            );
        }
        assert_eq!(store.load(&file, &key(1)).unwrap().raw, b"first");
        assert!(
            store
                .load(&file, &Identity::new("account", "inbox", 7, 1))
                .is_some()
        );
    }

    #[test]
    fn oldest_pending_items_are_evicted_by_count_and_bytes() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::default();
        for uid in 1..=4 {
            store
                .append(
                    &path(root.path(), uid),
                    key(uid),
                    100,
                    42,
                    0,
                    &[uid as u8; 10],
                )
                .unwrap();
            fs::File::open(metadata_path(&path(root.path(), uid)))
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(uid as u64))
                .unwrap();
        }
        assert!(store.load(&path(root.path(), 1), &key(1)).is_none());
        assert!(store.load(&path(root.path(), 2), &key(2)).is_some());
        prune(root.path(), &path(root.path(), 4), 10, 20, MAX_PENDING).unwrap();
        assert!(store.load(&path(root.path(), 2), &key(2)).is_none());
        assert!(store.load(&path(root.path(), 3), &key(3)).is_some());
        assert!(
            store
                .append(
                    &path(root.path(), 9),
                    key(9),
                    MAX_BYTES as u32 + 1,
                    42,
                    0,
                    b"x"
                )
                .is_err()
        );
        assert!(
            store
                .append(&path(root.path(), 9), key(9), 2, 42, 0, b"too long")
                .is_err()
        );
    }

    #[test]
    fn removed_mail_and_accounts_cannot_be_recreated_by_delayed_chunks() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("pending");
        let store = Store::default();
        for uid in 1..=2 {
            store
                .append(&path(&dir, uid), key(uid), 10, 42, 0, b"first")
                .unwrap();
        }
        store.remove(&dir, "account", "inbox", "folder-", Some(&[1]));
        assert!(store.load(&path(&dir, 1), &key(1)).is_none());
        assert!(
            store
                .append(&path(&dir, 1), key(1), 10, 42, 0, b"late")
                .is_err()
        );
        assert!(store.load(&path(&dir, 2), &key(2)).is_some());
        store.remove_account(&dir, "account");
        assert!(!dir.exists());
        assert!(
            store
                .append(&path(&dir, 2), key(2), 10, 42, 0, b"late")
                .is_err()
        );
    }

    #[test]
    fn deleting_before_the_first_checkpoint_blocks_late_bytes_but_not_a_new_generation() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("pending");
        let file = path(&dir, 1);
        let store = Store::default();
        assert!(store.load(&file, &key(1)).is_none());
        assert!(!dir.exists());
        store.remove(&dir, "account", "inbox", "folder-", Some(&[1]));
        assert!(store.append(&file, key(1), 10, 42, 0, b"late").is_err());
        assert!(!dir.exists());

        let new_identity = Identity::new("account", "INBOX", 8, 1);
        let new_file = dir.join("folder-8-1.partial");
        assert!(store.load(&new_file, &new_identity).is_none());
        store
            .append(&new_file, new_identity.clone(), 10, 42, 0, b"fresh")
            .unwrap();
        assert_eq!(store.load(&new_file, &new_identity).unwrap().raw, b"fresh");
        assert!(store.load(&file, &key(1)).is_none());
    }

    #[test]
    fn clearing_a_finished_download_forgets_active_identity_without_retiring_it() {
        let root = tempfile::tempdir().unwrap();
        let file = path(root.path(), 1);
        let store = Store::default();
        assert!(store.load(&file, &key(1)).is_none());
        store.clear(&file);
        store.remove(root.path(), "account", "INBOX", "folder-", Some(&[1]));
        store.append(&file, key(1), 10, 42, 0, b"fresh").unwrap();
        assert_eq!(store.load(&file, &key(1)).unwrap().raw, b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn pending_storage_is_private_and_rejects_redirected_data() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("account/pending");
        let file = path(&dir, 1);
        let store = Store::default();
        store.append(&file, key(1), 20, 42, 0, b"private").unwrap();
        assert_eq!(
            fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        for path in [&file, &metadata_path(&file)] {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let outside = root.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        fs::remove_file(&file).unwrap();
        symlink(&outside, &file).unwrap();
        assert!(store.load(&file, &key(1)).is_none());
        assert!(store.append(&file, key(1), 20, 42, 0, b"no").is_err());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    }
}
