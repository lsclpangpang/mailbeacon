//! 邮件原文的磁盘缓存：看过的邮件重启后再打开不用重新下载。
//!
//! 文件放在 `<数据目录>/bodies/<账号>/<文件夹>-<UIDVALIDITY>-<UID>.eml`，
//! 账号和文件夹名取哈希，文件开头 8 字节是 INTERNALDATE（毫秒）。
//! 总量超过上限时按最近一次读写时间（mtime）从最旧的删起。
//! 只给 IMAP 用：POP3 的编号只在本进程内有效，不能跨重启复用。

use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::SystemTime,
};

/// 默认总量上限
pub const DEFAULT_LIMIT: u64 = 512 * 1024 * 1024;
/// 单封超过这个大小不落盘
const MAX_FILE: usize = 32 * 1024 * 1024;

pub struct BodyStore {
    dir: PathBuf,
    limit: u64,
    /// 目前占用的字节数；第一次写入时统计
    total: Mutex<Option<u64>>,
}

/// FNV-1a：跨版本稳定，删账号时能找回同一个目录
fn hash(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// INBOX 不区分大小写，其余文件夹名原样
fn folder_hash(folder: &str) -> String {
    if folder.eq_ignore_ascii_case("INBOX") {
        hash("INBOX")
    } else {
        hash(folder)
    }
}

fn files(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let mut out = Vec::new();
    let Ok(accounts) = fs::read_dir(dir) else {
        return out;
    };
    for account in accounts.flatten() {
        let Ok(entries) = fs::read_dir(account.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_file() {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                out.push((entry.path(), meta.len(), modified));
            }
        }
    }
    out
}

impl BodyStore {
    pub fn new(dir: PathBuf, limit: u64) -> Self {
        Self {
            dir,
            limit,
            total: Mutex::new(None),
        }
    }

    fn account_dir(&self, account: &str) -> PathBuf {
        self.dir.join(hash(account))
    }

    fn path(&self, account: &str, folder: &str, validity: u32, uid: u32) -> PathBuf {
        self.account_dir(account)
            .join(format!("{}-{validity}-{uid}.eml", folder_hash(folder)))
    }

    /// 取出原文和日期；读到了就把它标成最近用过。
    pub fn get(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> Option<(Vec<u8>, i64)> {
        let path = self.path(account, folder, validity, uid);
        let mut data = fs::read(&path).ok()?;
        if data.len() < 8 {
            let _ = fs::remove_file(&path);
            return None;
        }
        let raw = data.split_off(8);
        let date = i64::from_le_bytes(data.try_into().ok()?);
        if let Ok(f) = fs::File::options().append(true).open(&path) {
            let _ = f.set_modified(SystemTime::now());
        }
        Some((raw, date))
    }

    /// 存一封原文（太大的不存）；超出总量就删掉最久没用的。
    pub fn put(&self, account: &str, folder: &str, validity: u32, uid: u32, raw: &[u8], date: i64) {
        if raw.len() > MAX_FILE {
            return;
        }
        if let Err(e) = self.write(account, folder, validity, uid, raw, date) {
            eprintln!("邮件原文写入本地缓存失败：{e:#}");
        }
    }

    fn write(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        raw: &[u8],
        date: i64,
    ) -> anyhow::Result<()> {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let path = self.path(account, folder, validity, uid);
        let dir = path.parent().expect("缓存文件总在账号目录里");
        // 邮件原文只给自己看：目录 0700、文件 0600
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(dir)?;
        // 先写临时文件再改名：写到一半退出也不会留下半封邮件
        let tmp = dir.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut f = options.open(&tmp)?;
        f.write_all(&date.to_le_bytes())?;
        f.write_all(raw)?;
        drop(f);
        // 同一封邮件可能同时被写两次：替换和计数放在一个锁里，总量才不会多算
        let mut total = self.total.lock().unwrap();
        let replaced = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        let now = match *total {
            Some(t) => t.saturating_sub(replaced) + raw.len() as u64 + 8,
            None => files(&self.dir).iter().map(|(_, len, _)| len).sum(),
        };
        *total = Some(if now > self.limit { self.prune() } else { now });
        Ok(())
    }

    /// 从最久没用的删起，直到降到上限的九成；返回剩下的字节数。
    fn prune(&self) -> u64 {
        let mut all = files(&self.dir);
        all.sort_by_key(|(_, _, modified)| *modified);
        let mut total: u64 = all.iter().map(|(_, len, _)| len).sum();
        let target = self.limit / 10 * 9;
        for (path, len, _) in all {
            if total <= target {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total -= len;
            }
        }
        total
    }

    /// 删掉某个文件夹里指定 UID 的原文（`uids` 为 None 时删整个文件夹）。
    pub fn remove(&self, account: &str, folder: &str, uids: Option<&[u32]>) {
        let prefix = format!("{}-", folder_hash(folder));
        let Ok(entries) = fs::read_dir(self.account_dir(account)) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(&prefix)) else {
                continue;
            };
            let uid = rest
                .strip_suffix(".eml")
                .and_then(|r| r.rsplit('-').next())
                .and_then(|u| u.parse::<u32>().ok());
            if uids.is_none_or(|list| uid.is_some_and(|u| list.contains(&u))) {
                let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
                if fs::remove_file(entry.path()).is_ok()
                    && let Some(total) = self.total.lock().unwrap().as_mut()
                {
                    *total = total.saturating_sub(len);
                }
            }
        }
    }

    /// 删除账号时清掉它的全部缓存
    pub fn remove_account(&self, account: &str) {
        let _ = fs::remove_dir_all(self.account_dir(account));
        *self.total.lock().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn store(limit: u64) -> (tempfile::TempDir, BodyStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = BodyStore::new(dir.path().join("bodies"), limit);
        (dir, store)
    }

    fn age(store: &BodyStore, folder: &str, uid: u32, secs: u64) {
        let f = fs::File::options()
            .append(true)
            .open(store.path("a", folder, 7, uid))
            .unwrap();
        f.set_modified(SystemTime::now() - Duration::from_secs(secs))
            .unwrap();
    }

    #[test]
    fn stored_mail_comes_back_with_its_date_only_for_the_same_generation() {
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, b"raw mail", 42);
        assert_eq!(s.get("a", "INBOX", 7, 1), Some((b"raw mail".to_vec(), 42)));
        assert_eq!(
            s.get("a", "inbox", 7, 1).map(|r| r.1),
            Some(42),
            "INBOX 不分大小写"
        );
        assert!(
            s.get("a", "INBOX", 8, 1).is_none(),
            "UIDVALIDITY 变了不能复用"
        );
        assert!(s.get("a", "Sent", 7, 1).is_none());
        assert!(s.get("b", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn oversize_mail_is_not_written() {
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, &vec![0; MAX_FILE + 1], 0);
        assert!(s.get("a", "INBOX", 7, 1).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn cached_mail_is_readable_only_by_the_user() {
        use std::os::unix::fs::PermissionsExt;
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, b"secret", 0);
        let path = s.path("a", "INBOX", 7, 1);
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path), 0o600);
        assert_eq!(mode(path.parent().unwrap()), 0o700);
    }

    #[test]
    fn least_recently_read_mail_is_evicted_first() {
        // 每封 100 + 8 字节，上限放得下两封
        let (_d, s) = store(250);
        s.put("a", "INBOX", 7, 1, &[1; 100], 0);
        s.put("a", "INBOX", 7, 2, &[2; 100], 0);
        age(&s, "INBOX", 1, 100);
        age(&s, "INBOX", 2, 50);
        // 读一下第 1 封：它变成最近用过的
        assert!(s.get("a", "INBOX", 7, 1).is_some());
        s.put("a", "INBOX", 7, 3, &[3; 100], 0);
        assert!(s.get("a", "INBOX", 7, 1).is_some());
        assert!(s.get("a", "INBOX", 7, 2).is_none(), "最久没用的被删");
        assert!(s.get("a", "INBOX", 7, 3).is_some());
    }

    #[test]
    fn moved_mail_and_removed_accounts_are_forgotten() {
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, b"1", 0);
        s.put("a", "INBOX", 7, 2, b"2", 0);
        s.put("a", "Trash", 7, 1, b"t", 0);
        s.put("b", "INBOX", 7, 1, b"b", 0);
        s.remove("a", "INBOX", Some(&[1]));
        assert!(s.get("a", "INBOX", 7, 1).is_none());
        assert!(s.get("a", "INBOX", 7, 2).is_some());
        s.remove("a", "Trash", None);
        assert!(s.get("a", "Trash", 7, 1).is_none());
        s.remove_account("a");
        assert!(s.get("a", "INBOX", 7, 2).is_none());
        assert!(s.get("b", "INBOX", 7, 1).is_some(), "别的账号不受影响");
    }

    #[test]
    fn a_truncated_file_is_ignored() {
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, b"x", 0);
        fs::write(s.path("a", "INBOX", 7, 1), b"abc").unwrap();
        assert!(s.get("a", "INBOX", 7, 1).is_none());
    }
}
