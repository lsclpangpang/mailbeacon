//! Atomic local-file publication across desktop platforms.

use anyhow::{Context, Result};
use std::{io::Write, path::Path};

/// Publish a new private file without ever replacing an existing destination.
/// The rename is durable before callers can commit data that depends on it.
pub fn atomic_create(path: &Path, data: &[u8]) -> Result<()> {
    publish(path, data, false)
}

/// Publish a complete private file, preserving the old file if replacement fails.
/// Windows uses write-through rename; opening a directory and calling sync_all on
/// it is a Unix operation and can fail *after* publishing the new Windows file.
pub fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    publish(path, data, true)
}

fn publish(path: &Path, data: &[u8], overwrite: bool) -> Result<()> {
    let dir = path.parent().context("本地文件路径无效")?;
    let mut temporary = tempfile::NamedTempFile::new_in(dir)?;
    crate::private_fs::protect_file(temporary.path())?;
    temporary.write_all(data)?;
    temporary.as_file().sync_all()?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_NORMAL, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
            SetFileAttributesW,
        };
        // std::fs handles long Windows paths; use its canonical extended form
        // for the native rename too (including UNC paths).
        let source = std::fs::canonicalize(temporary.path())?;
        let destination =
            std::fs::canonicalize(dir)?.join(path.file_name().context("本地文件名无效")?);
        let from: Vec<u16> = source.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        if to[..to.len() - 1].contains(&0) {
            anyhow::bail!("本地文件路径包含无效字符");
        }
        // NamedTempFile starts with FILE_ATTRIBUTE_TEMPORARY on Windows. Remove
        // that hint before publication so Windows treats this as persistent data.
        if unsafe { SetFileAttributesW(from.as_ptr(), FILE_ATTRIBUTE_NORMAL) } == 0 {
            return Err(std::io::Error::last_os_error()).context("准备持久化本地文件失败");
        }
        temporary.as_file().sync_all()?;
        // Both paths are in one directory/volume. No copy/delete fallback and no
        // delete-first window: failure leaves the original destination intact.
        let flags = MOVEFILE_WRITE_THROUGH
            | if overwrite {
                MOVEFILE_REPLACE_EXISTING
            } else {
                0
            };
        let moved = unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), flags) };
        if moved == 0 {
            return Err(std::io::Error::last_os_error()).context("保存本地文件失败，原文件已保留");
        }
    }
    #[cfg(not(windows))]
    {
        if overwrite {
            temporary.persist(path).map_err(|e| e.error)?;
        } else {
            temporary.persist_noclobber(path).map_err(|e| e.error)?;
        }
        #[cfg(unix)]
        std::fs::File::open(dir)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creation_never_overwrites_an_existing_private_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.key");
        atomic_create(&path, b"original key").unwrap();
        assert!(atomic_create(&path, b"replacement key").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"original key");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_TEMPORARY;
            crate::private_fs::assert_private_acl(&path);
            assert_eq!(
                std::fs::metadata(&path).unwrap().file_attributes() & FILE_ATTRIBUTE_TEMPORARY,
                0
            );
        }
    }

    #[test]
    fn concurrent_creations_publish_exactly_one_complete_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials.key");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
        let attempts: Vec<_> = (0..8u8)
            .map(|value| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    atomic_create(&path, &vec![value; 4096]).map(|_| value)
                })
            })
            .collect();
        let winners: Vec<_> = attempts
            .into_iter()
            .filter_map(|attempt| attempt.join().unwrap().ok())
            .collect();
        assert_eq!(winners.len(), 1);
        assert_eq!(std::fs::read(&path).unwrap(), vec![winners[0]; 4096]);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[test]
    fn replacement_reopens_complete_latest_bytes_in_a_unicode_path() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("恢复 邮件");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("草稿.json");
        atomic_write(&path, b"old snapshot").unwrap();
        let latest = vec![0xab; 256 * 1024];
        atomic_write(&path, &latest).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), latest);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_TEMPORARY;
            assert_eq!(
                std::fs::metadata(&path).unwrap().file_attributes() & FILE_ATTRIBUTE_TEMPORARY,
                0
            );
        }
    }

    #[test]
    fn failed_replacement_preserves_existing_destination_and_cleans_temp() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("occupied");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("existing"), b"keep").unwrap();
        assert!(atomic_write(&path, b"cannot replace a directory").is_err());
        assert_eq!(std::fs::read(path.join("existing")).unwrap(), b"keep");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn sharing_violation_preserves_previous_snapshot() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("locked.json");
        atomic_write(&path, b"previous").unwrap();
        let locked = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(&path)
            .unwrap();
        assert!(atomic_write(&path, b"next").is_err());
        drop(locked);
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
    }

    #[cfg(windows)]
    #[test]
    fn embedded_nul_cannot_redirect_the_replacement() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("original");
        atomic_write(&path, b"previous").unwrap();
        assert!(atomic_write(&root.path().join("original\0ignored"), b"next").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous");
    }

    #[cfg(windows)]
    #[test]
    fn replacement_supports_long_windows_directory_paths() {
        let root = tempfile::tempdir().unwrap();
        let mut dir = root.path().to_path_buf();
        for _ in 0..5 {
            dir.push("long-application-storage-folder-name-with-spaces 12345");
        }
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("draft.json");
        assert!(path.as_os_str().len() > 260);
        atomic_write(&path, b"old").unwrap();
        atomic_write(&path, b"new").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"new");
    }
}
