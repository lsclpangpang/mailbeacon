//! Disposable extracted attachments. Each new directory has one exact message owner.
//! User exports and downloaded .eml archives are deliberately outside these roots.
use crate::{
    body_store::BodyStore,
    mail_ref::{MessageRef, namespace},
};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Serialize, Deserialize)]
struct Owner {
    source: MessageRef,
    received_date: i64,
}

fn owner(path: &Path) -> Option<Owner> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || crate::private_fs::is_link(&metadata) || metadata.len() > 64 * 1024 {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(crate) fn save(
    root: &Path,
    source: &MessageRef,
    date: i64,
    part: usize,
    name: &str,
    data: &[u8],
) -> Result<PathBuf> {
    let key = crate::attachment_index::raw_hash(&serde_json::to_vec(source)?);
    let cache_root = root.join("attachment-cache");
    // Validate the parent before creating any children. Checking only the final
    // directory would follow a linked cache root into an unrelated user folder.
    crate::private_fs::create_private_dir(&cache_root)?;
    let dir = cache_root.join(key);
    crate::private_fs::create_private_dir(&dir)?;
    let owner_file = dir.join("owner.json");
    let previous = owner(&owner_file);
    let date = if date > 0 {
        date
    } else {
        previous
            .filter(|o| o.source == *source)
            .map_or(0, |o| o.received_date)
    };
    crate::durable_fs::atomic_write(
        &owner_file,
        &serde_json::to_vec(&Owner {
            source: source.clone(),
            received_date: date,
        })?,
    )?;
    let dir = dir.join(part.to_string());
    crate::private_fs::create_private_dir(&dir)?;
    let path = dir.join(crate::safe_file_name(name));
    if !fs::read(&path).is_ok_and(|old| old == data) {
        crate::durable_fs::atomic_write(&path, data)?;
    }
    Ok(path)
}

pub(crate) struct Candidate {
    pub source: MessageRef,
    pub date: i64,
    pub path: PathBuf,
    pub bytes: u64,
}

// Never follow symlinks or junctions while counting/deleting application caches.
fn tree_bytes(path: &Path) -> Option<u64> {
    let meta = fs::symlink_metadata(path).ok()?;
    if crate::private_fs::is_link(&meta) {
        return None;
    }
    if meta.is_file() {
        return Some(meta.len());
    }
    if !meta.is_dir() {
        return None;
    }
    let mut bytes = 0;
    for entry in fs::read_dir(path).ok()? {
        bytes += tree_bytes(&entry.ok()?.path())?;
    }
    Some(bytes)
}

/// Build this list before removing raw files: legacy directories have no timestamp
/// manifest, so only the original raw INTERNALDATE can prove their age.
pub(crate) fn candidates(root: &Path, storage: &str, disk: &BodyStore) -> Vec<Candidate> {
    let mut result = Vec::new();
    let cache_root = root.join("attachment-cache");
    let safe_root = fs::symlink_metadata(&cache_root)
        .is_ok_and(|m| m.is_dir() && !crate::private_fs::is_link(&m));
    if safe_root && let Ok(entries) = fs::read_dir(&cache_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(bytes) = tree_bytes(&path) else {
                continue;
            };
            let Some(owner) = owner(&path.join("owner.json")) else {
                continue;
            };
            let Ok(identity) = serde_json::to_vec(&owner.source) else {
                continue;
            };
            if path.file_name().and_then(|name| name.to_str())
                != Some(crate::attachment_index::raw_hash(&identity).as_str())
            {
                continue;
            }
            if namespace(&owner.source.account_id, owner.source.receive_epoch) == storage {
                result.push(Candidate {
                    source: owner.source,
                    date: owner.received_date,
                    path,
                    bytes,
                });
            }
        }
    }
    result.extend(legacy_candidates(root, storage, disk));
    result
}

fn legacy_candidates(root: &Path, storage: &str, disk: &BodyStore) -> Vec<Candidate> {
    // Legacy indexed paths are shared by raw hash; do not remove one when any
    // other account/source still owns it. Missing ownership evidence is retained.
    let Ok(conn) = rusqlite::Connection::open_with_flags(
        root.join("local.db"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    ) else {
        return Vec::new();
    };
    let Ok(mut statement) = conn.prepare("SELECT m.account_id,m.receive_epoch,m.folder,m.uid_validity,m.uid,m.part_id,s.raw_hash,m.storage_account FROM attachment_metadata m JOIN attachment_sources s ON s.storage_account=m.storage_account AND s.folder=m.folder AND s.uid_validity=m.uid_validity AND s.uid=m.uid") else { return Vec::new(); };
    let Ok(rows) = statement.query_map([], |r| {
        Ok((
            MessageRef {
                account_id: r.get(0)?,
                receive_epoch: r.get(1)?,
                folder: r.get(2)?,
                uid_validity: r.get(3)?,
                uid: r.get(4)?,
            },
            r.get::<_, usize>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, String>(7)?,
        ))
    }) else {
        return Vec::new();
    };
    let rows: Vec<_> = rows.filter_map(Result::ok).collect();
    let mut indexed: HashMap<PathBuf, Vec<(MessageRef, i64, String)>> = HashMap::new();
    let mut result = Vec::new();
    let mut visited = HashSet::new();
    for (source, part, hash, owner) in rows {
        if namespace(&source.account_id, source.receive_epoch) != owner
            || source.validate().is_err()
        {
            continue;
        }
        // Only lowercase SHA256 values can become path components.
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            continue;
        }
        let date = disk
            .received_date(&owner, &source.folder, source.uid_validity, source.uid)
            .unwrap_or(0);
        let indexed_root = root.join("indexed-attachments");
        let hash_dir = indexed_root.join(hash);
        if [indexed_root, hash_dir.clone()].iter().all(|p| {
            fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !crate::private_fs::is_link(&m))
        }) {
            indexed
                .entry(hash_dir.join(part.to_string()))
                .or_default()
                .push((source.clone(), date, owner.clone()));
        }
        if owner != storage {
            continue;
        }
        if !fs::symlink_metadata(root.join("attachments"))
            .is_ok_and(|m| m.is_dir() && !crate::private_fs::is_link(&m))
        {
            continue;
        }
        let identity = format!(
            "{}|{}|{}|{}|{}|{}",
            source.account_id,
            source.receive_epoch,
            source.folder,
            source.uid_validity,
            source.uid,
            part
        );
        let path = root
            .join("attachments")
            .join(crate::attachment_index::raw_hash(identity.as_bytes()));
        if visited.insert(path.clone())
            && let Some(bytes) = tree_bytes(&path)
        {
            result.push(Candidate {
                source,
                date,
                path,
                bytes,
            });
        }
    }
    for (path, owners) in indexed {
        if owners.iter().any(|(_, _, owner)| owner != storage) {
            continue;
        }
        // Date of the newest owner controls cleanup of a shared path. Unknown
        // dates are preserved by age-based cleanup even if other owners are old.
        let date = if owners.iter().any(|(_, date, _)| *date <= 0) {
            0
        } else {
            owners.iter().map(|(_, date, _)| *date).max().unwrap_or(0)
        };
        if let (Some((source, _, _)), Some(bytes)) = (owners.into_iter().next(), tree_bytes(&path))
        {
            result.push(Candidate {
                source,
                date,
                path,
                bytes,
            });
        }
    }
    result
}

pub(crate) fn matches(date: i64, cutoff: Option<i64>) -> bool {
    cutoff.is_none_or(|before| date > 0 && date < before)
}

pub(crate) fn remove(candidate: &Candidate) -> Result<()> {
    tree_bytes(&candidate.path).context("附件缓存路径已变化，已保留")?;
    fs::remove_dir_all(&candidate.path).context("清理附件缓存失败")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn source(account: &str, uid: u32) -> MessageRef {
        MessageRef {
            account_id: account.into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid,
        }
    }
    #[test]
    fn cleanup_has_exact_owner_and_keeps_exports_and_newer_mail() {
        let root = tempfile::tempdir().unwrap();
        let disk = BodyStore::new(root.path().join("bodies"), 1_000_000);
        let old = save(root.path(), &source("a", 1), 100, 0, "old.txt", b"old").unwrap();
        let new = save(root.path(), &source("a", 2), 300, 0, "new.txt", b"new").unwrap();
        let other = save(root.path(), &source("b", 1), 100, 0, "other.txt", b"other").unwrap();
        let exported = root.path().join("user-export.eml");
        fs::write(&exported, b"archive").unwrap();
        let found = candidates(root.path(), "a", &disk);
        assert_eq!(found.len(), 2);
        for row in found.iter().filter(|row| matches(row.date, Some(200))) {
            remove(row).unwrap();
        }
        assert!(!old.exists());
        assert!(new.exists());
        assert!(other.exists());
        assert!(exported.exists());
        assert!(!matches(0, Some(200)));
        assert!(matches(0, None));
        assert!(!matches(200, Some(200)));
    }
    #[test]
    fn repeated_open_reuses_file_and_preserves_proven_date() {
        let root = tempfile::tempdir().unwrap();
        let disk = BodyStore::new(root.path().join("bodies"), 1_000_000);
        let first = save(root.path(), &source("a", 1), 100, 0, "file.txt", b"one").unwrap();
        assert_eq!(
            first,
            save(root.path(), &source("a", 1), 0, 0, "file.txt", b"one").unwrap()
        );
        assert_eq!(candidates(root.path(), "a", &disk)[0].date, 100);
    }
    #[test]
    fn legacy_shared_files_are_not_deleted_for_another_account() {
        let root = tempfile::tempdir().unwrap();
        let disk = BodyStore::new(root.path().join("bodies"), 1_000_000);
        let conn = rusqlite::Connection::open(root.path().join("local.db")).unwrap();
        conn.execute_batch("CREATE TABLE attachment_metadata(account_id TEXT,receive_epoch INTEGER,folder TEXT,uid_validity INTEGER,uid INTEGER,part_id INTEGER,storage_account TEXT); CREATE TABLE attachment_sources(storage_account TEXT,folder TEXT,uid_validity INTEGER,uid INTEGER,raw_hash TEXT);").unwrap();
        let hash = crate::attachment_index::raw_hash(b"same contents");
        for account in ["a", "b"] {
            conn.execute(
                "INSERT INTO attachment_metadata VALUES(?1,1,'INBOX',7,1,0,?1)",
                [account],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO attachment_sources VALUES(?1,'INBOX',7,1,?2)",
                rusqlite::params![account, hash],
            )
            .unwrap();
            disk.put(account, "INBOX", 7, 1, b"raw", 100);
        }
        let shared = root
            .path()
            .join("indexed-attachments")
            .join(&hash)
            .join("0");
        fs::create_dir_all(&shared).unwrap();
        fs::write(shared.join("file.txt"), b"keep").unwrap();
        let identity = "a|1|INBOX|7|1|0";
        let single = root
            .path()
            .join("attachments")
            .join(crate::attachment_index::raw_hash(identity.as_bytes()));
        fs::create_dir_all(&single).unwrap();
        fs::write(single.join("file.txt"), b"old").unwrap();
        let found = candidates(root.path(), "a", &disk);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].date, 100);
        remove(&found[0]).unwrap();
        assert!(shared.join("file.txt").exists());
    }
    #[cfg(unix)]
    #[test]
    fn linked_cache_directory_is_never_traversed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let disk = BodyStore::new(root.path().join("bodies"), 1_000_000);
        save(
            outside.path(),
            &source("a", 1),
            100,
            0,
            "private.txt",
            b"keep",
        )
        .unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("attachment-cache"),
            root.path().join("attachment-cache"),
        )
        .unwrap();
        // A linked parent must also be rejected, not only the final child.
        assert!(candidates(root.path(), "a", &disk).is_empty());
        assert!(save(root.path(), &source("a", 2), 100, 0, "new.txt", b"reject").is_err());
        assert_eq!(
            fs::read_dir(outside.path().join("attachment-cache"))
                .unwrap()
                .count(),
            1
        );
    }

    #[test]
    fn mismatched_owner_manifest_cannot_reassign_another_directory() {
        let root = tempfile::tempdir().unwrap();
        let disk = BodyStore::new(root.path().join("bodies"), 1_000_000);
        let path = save(root.path(), &source("a", 1), 100, 0, "file.txt", b"keep").unwrap();
        let owner_file = path.parent().unwrap().parent().unwrap().join("owner.json");
        fs::write(
            owner_file,
            serde_json::to_vec(&Owner {
                source: source("b", 1),
                received_date: 100,
            })
            .unwrap(),
        )
        .unwrap();
        assert!(candidates(root.path(), "b", &disk).is_empty());
        assert!(path.exists());
    }
}
