//! 邮件原文的磁盘缓存：看过的邮件重启后再打开不用重新下载，
//! 后台预热（cache_warm.rs）也把收件箱的邮件先存进来。
//!
//! 文件放在 `<数据目录>/bodies/<账号>/<文件夹>-<UIDVALIDITY>-<UID>.eml`，
//! 账号和文件夹名取哈希，文件开头 8 字节是 INTERNALDATE（毫秒）。
//! 每个账号有自己的总量上限，超过时按最近一次读写时间（mtime）从最旧的删起；
//! 后台预先缓存的邮件，mtime 记成它的收件时间。
//! 只给 IMAP 用：POP3 的编号只在本进程内有效，不能跨重启复用。

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use crate::cache_cleanup::{self, CleanupState};
pub use crate::cache_cleanup::{
    CacheEntry, CacheKind, CacheOrigin, CacheWriteTicket, CleanupReport,
};

/// 账号没设置时的总量上限
pub const DEFAULT_LIMIT: u64 = 2048 * 1024 * 1024;
/// 单封超过这个大小不落盘
pub const MAX_FILE: usize = 32 * 1024 * 1024;

pub struct BodyStore {
    dir: PathBuf,
    default_limit: u64,
    /// 账号 → 总量上限（来自账号设置）
    limits: Mutex<HashMap<String, u64>>,
    /// 账号 → 目前占用；第一次用到时扫描一次目录，之后跟着增删记账
    stats: Mutex<HashMap<String, Usage>>,
    /// 已删除的账号：还在路上的写入直接丢掉，不再建出它的目录
    removed: Mutex<HashSet<String>>,
    resumes: crate::download_resume::Store,
    /// Serializes retention changes with every ticketed publication, including
    /// derivatives written by a caller inside commit_with_ticket.
    cleanup: Mutex<HashMap<String, CleanupState>>,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    bytes: u64,
    count: usize,
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
pub(crate) fn folder_hash(folder: &str) -> String {
    if folder.eq_ignore_ascii_case("INBOX") {
        hash("INBOX")
    } else {
        hash(folder)
    }
}

/// 一个账号目录里写完的邮件（不含写到一半的临时文件）
fn files(dir: &Path) -> Vec<(PathBuf, u64, SystemTime)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.ends_with(".eml")))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then(|| {
                let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                (e.path(), meta.len(), modified)
            })
        })
        .collect()
}

/// 毫秒时间戳 → 文件时间；比现在还晚（服务器时钟偏快）的按现在算，没有日期（0）的返回 None
fn time_of(date_ms: i64) -> Option<SystemTime> {
    let ms = u64::try_from(date_ms).ok().filter(|ms| *ms > 0)?;
    Some((SystemTime::UNIX_EPOCH + Duration::from_millis(ms)).min(SystemTime::now()))
}

impl BodyStore {
    pub fn new(dir: PathBuf, default_limit: u64) -> Self {
        Self {
            dir,
            default_limit,
            limits: Mutex::default(),
            stats: Mutex::default(),
            removed: Mutex::default(),
            resumes: crate::download_resume::Store::default(),
            cleanup: Mutex::default(),
        }
    }

    fn account_dir(&self, account: &str) -> PathBuf {
        self.dir.join(hash(account))
    }

    pub fn begin_write(&self, account: &str, origin: CacheOrigin) -> CacheWriteTicket {
        let mut state = self.cleanup.lock().unwrap();
        let policy = state
            .entry(account.into())
            .or_insert_with(|| CleanupState::load(&self.account_dir(account), account));
        CacheWriteTicket {
            account: account.into(),
            generation: policy.generation,
            origin,
        }
    }

    pub fn background_allowed(&self, account: &str, date: i64) -> bool {
        if self.is_removed(account) {
            return false;
        }
        let mut state = self.cleanup.lock().unwrap();
        state
            .entry(account.into())
            .or_insert_with(|| CleanupState::load(&self.account_dir(account), account))
            .background_allowed(date)
    }

    pub fn ticket_valid(&self, ticket: &CacheWriteTicket, date: i64) -> bool {
        if self.is_removed(&ticket.account) {
            return false;
        }
        let mut state = self.cleanup.lock().unwrap();
        state
            .entry(ticket.account.clone())
            .or_insert_with(|| {
                CleanupState::load(&self.account_dir(&ticket.account), &ticket.account)
            })
            .ticket_valid(ticket, date)
    }

    /// The callback runs under the same gate as cleanup. It may call
    /// received_date/cache_entries, but must not recursively acquire this gate.
    pub fn commit_with_ticket<T>(
        &self,
        ticket: &CacheWriteTicket,
        date: i64,
        commit: impl FnOnce() -> anyhow::Result<T>,
    ) -> anyhow::Result<Option<T>> {
        let mut state = self.cleanup.lock().unwrap();
        let policy = state.entry(ticket.account.clone()).or_insert_with(|| {
            CleanupState::load(&self.account_dir(&ticket.account), &ticket.account)
        });
        if self.is_removed(&ticket.account) || !policy.ticket_valid(ticket, date) {
            return Ok(None);
        }
        commit().map(Some)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn put_with_ticket(
        &self,
        ticket: &CacheWriteTicket,
        folder: &str,
        validity: u32,
        uid: u32,
        raw: &[u8],
        date: i64,
    ) -> anyhow::Result<bool> {
        Ok(self
            .commit_with_ticket(ticket, date, || {
                if raw.len() > MAX_FILE {
                    return Ok(false);
                }
                // A foreground read may have cached this message while a
                // background fetch was in flight. Preserve its recent-use mtime.
                if ticket.origin == CacheOrigin::Background
                    && self.path(&ticket.account, folder, validity, uid).is_file()
                {
                    return Ok(true);
                }
                self.write(
                    &ticket.account,
                    (folder, validity, uid),
                    raw,
                    date,
                    ticket.origin == CacheOrigin::Background,
                )?;
                Ok(!self.is_removed(&ticket.account)
                    && self.path(&ticket.account, folder, validity, uid).is_file())
            })?
            .unwrap_or(false))
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_resume_with_ticket(
        &self,
        ticket: &CacheWriteTicket,
        folder: &str,
        validity: u32,
        uid: u32,
        total: u32,
        date: i64,
        offset: usize,
        bytes: &[u8],
    ) -> anyhow::Result<bool> {
        Ok(self
            .commit_with_ticket(ticket, date, || {
                self.append_resume(
                    &ticket.account,
                    folder,
                    validity,
                    uid,
                    total,
                    date,
                    offset,
                    bytes,
                )?;
                Ok(true)
            })?
            .unwrap_or(false))
    }

    /// Read only the proven receive date, without touching mtime or loading MIME.
    /// This deliberately does not acquire the cleanup gate, so callbacks may use it.
    pub fn received_date(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> Option<i64> {
        Self::received_date_at(&self.path(account, folder, validity, uid))
    }

    fn received_date_at(path: &Path) -> Option<i64> {
        if cache_cleanup::regular_len(path)? < 8 {
            return None;
        }
        let mut header = [0u8; 8];
        fs::File::open(path).ok()?.read_exact(&mut header).ok()?;
        Some(i64::from_le_bytes(header))
    }

    /// Identity-bearing snapshot; callers already inside the gate may use it.
    pub fn cache_entries(&self, account: &str) -> anyhow::Result<Vec<CacheEntry>> {
        let dir = self.account_dir(account);
        let mut entries = Vec::new();
        match fs::symlink_metadata(&dir) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Ok(meta) if meta.is_dir() && !crate::private_fs::is_link(&meta) => {}
            _ => anyhow::bail!("邮件缓存目录不可读取"),
        }
        for entry in fs::read_dir(&dir)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some((folder_hash, uid_validity, uid)) = name
                .to_str()
                .and_then(|name| cache_cleanup::file_identity(name, ".eml"))
            else {
                continue;
            };
            let path = entry.path();
            let Some(bytes) = cache_cleanup::regular_len(&path) else {
                continue;
            };
            entries.push(CacheEntry {
                kind: CacheKind::Raw,
                account: account.into(),
                folder_hash,
                folder: None,
                uid_validity,
                uid,
                date: Self::received_date_at(&path).unwrap_or(0),
                bytes,
                paths: vec![path],
            });
        }
        entries.extend(
            self.resumes
                .cleanup_entries(&dir.join("pending"), account)?,
        );
        Ok(entries)
    }

    pub fn preview_cleanup(
        &self,
        account: &str,
        cutoff: Option<i64>,
    ) -> anyhow::Result<CleanupReport> {
        let _gate = self.cleanup.lock().unwrap();
        Ok(CleanupReport::preview(
            &self.cache_entries(account)?,
            cutoff,
        ))
    }

    pub fn cleanup_with(
        &self,
        account: &str,
        cutoff: Option<i64>,
        now_ms: i64,
        extra: impl FnOnce(&[CacheEntry]) -> anyhow::Result<CleanupReport>,
    ) -> anyhow::Result<CleanupReport> {
        anyhow::ensure!(
            now_ms > 0 && cutoff.is_none_or(|value| value >= 0),
            "缓存清理日期无效"
        );
        anyhow::ensure!(!self.is_removed(account), "账号缓存已移除");
        let mut state = self.cleanup.lock().unwrap();
        let policy = state
            .entry(account.into())
            .or_insert_with(|| CleanupState::load(&self.account_dir(account), account));
        // A failed policy publication performs no deletion and does not revoke
        // active tickets. After success a restart also preserves the floor.
        policy.persist_cleanup(
            &self.account_dir(account),
            account,
            cutoff.unwrap_or(now_ms),
        )?;
        let entries = self.cache_entries(account)?;
        let mut report = CleanupReport::preview(&entries, cutoff);
        // Derivative cleanup can still read original receive dates here.
        report.merge(&extra(&entries)?);
        for entry in entries
            .iter()
            .filter(|entry| entry.kind == CacheKind::Raw && entry.matches(cutoff))
        {
            report.merge(&cache_cleanup::remove_entry(entry));
        }
        report.merge(&self.resumes.cleanup_selected(&entries, cutoff));
        // Recompute on the next usage request, including partial deletion failures.
        self.stats.lock().unwrap().remove(account);
        Ok(report)
    }

    fn path(&self, account: &str, folder: &str, validity: u32, uid: u32) -> PathBuf {
        self.account_dir(account)
            .join(format!("{}-{validity}-{uid}.eml", folder_hash(folder)))
    }

    fn resume_path(&self, account: &str, folder: &str, validity: u32, uid: u32) -> PathBuf {
        self.account_dir(account)
            .join("pending")
            .join(format!("{}-{validity}-{uid}.partial", folder_hash(folder)))
    }

    pub(crate) fn load_resume(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> Option<crate::download_resume::Resume> {
        if self.is_removed(account) {
            return None;
        }
        self.resumes.load(
            &self.resume_path(account, folder, validity, uid),
            &crate::download_resume::Identity::new(account, folder, validity, uid),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn append_resume(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        total: u32,
        date: i64,
        offset: usize,
        bytes: &[u8],
    ) -> anyhow::Result<()> {
        if self.is_removed(account) {
            anyhow::bail!("账号的续传状态已移除");
        }
        self.resumes.append(
            &self.resume_path(account, folder, validity, uid),
            crate::download_resume::Identity::new(account, folder, validity, uid),
            total,
            date,
            offset,
            bytes,
        )
    }

    pub(crate) fn clear_resume(&self, account: &str, folder: &str, validity: u32, uid: u32) {
        self.resumes
            .clear(&self.resume_path(account, folder, validity, uid));
    }

    pub(crate) fn clear_resume_with_ticket(
        &self,
        ticket: &CacheWriteTicket,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> anyhow::Result<bool> {
        Ok(self
            .commit_with_ticket(ticket, i64::MAX, || {
                self.clear_resume(&ticket.account, folder, validity, uid);
                Ok(())
            })?
            .is_some())
    }

    /// Retire only this receiving namespace's pending downloads, keeping complete cache files.
    pub(crate) fn clear_resume_account(&self, account: &str) {
        self.resumes
            .remove_account(&self.account_dir(account).join("pending"), account);
    }

    fn limit(&self, account: &str) -> u64 {
        self.limits
            .lock()
            .unwrap()
            .get(account)
            .copied()
            .unwrap_or(self.default_limit)
    }

    /// 只记下账号的上限，不动磁盘（启动时用：必须赶在第一次写入之前，
    /// 不然会按默认上限把用户调大后存下的缓存删掉）。
    pub fn preset_limit(&self, account: &str, limit: u64) {
        self.limits
            .lock()
            .unwrap()
            .insert(account.to_string(), limit);
    }

    /// 设置账号的总量上限；现在就超了的话立刻删掉最久没用的。
    pub fn set_limit(&self, account: &str, limit: u64) {
        self.preset_limit(account, limit);
        let mut stats = self.stats.lock().unwrap();
        if self.usage_locked(&mut stats, account).bytes > limit {
            let left = self.prune(account, None, limit / 10 * 9);
            stats.insert(account.to_string(), left);
        }
    }

    fn usage_locked(&self, stats: &mut HashMap<String, Usage>, account: &str) -> Usage {
        *stats.entry(account.to_string()).or_insert_with(|| {
            let all = files(&self.account_dir(account));
            Usage {
                bytes: all.iter().map(|(_, len, _)| len).sum(),
                count: all.len(),
            }
        })
    }

    /// 已缓存的封数和字节数（第一次扫描目录，之后是记账的结果）
    pub fn usage(&self, account: &str) -> (usize, u64) {
        let u = self.usage_locked(&mut self.stats.lock().unwrap(), account);
        (u.count, u.bytes)
    }

    /// 还能再存多少字节
    pub fn room(&self, account: &str) -> u64 {
        let used = self
            .usage_locked(&mut self.stats.lock().unwrap(), account)
            .bytes;
        self.limit(account).saturating_sub(used)
    }

    #[cfg(test)]
    pub fn contains(&self, account: &str, folder: &str, validity: u32, uid: u32) -> bool {
        self.path(account, folder, validity, uid).is_file()
    }

    /// 这个文件夹（这一代 UIDVALIDITY）里已经缓存的 UID：读一次目录，不用每封去查
    pub fn cached_uids(&self, account: &str, folder: &str, validity: u32) -> HashSet<u32> {
        let prefix = format!("{}-{validity}-", folder_hash(folder));
        let Ok(entries) = fs::read_dir(self.account_dir(account)) else {
            return HashSet::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let name = e.file_name();
                name.to_str()?
                    .strip_prefix(&prefix)?
                    .strip_suffix(".eml")?
                    .parse()
                    .ok()
            })
            .collect()
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
        self.put_inner(account, folder, validity, uid, raw, date, false);
    }

    /// A completed download can discard its checkpoint only after cache storage succeeds.
    pub(crate) fn put_download(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        raw: &[u8],
        date: i64,
    ) -> bool {
        if raw.len() > MAX_FILE || self.is_removed(account) {
            return false;
        }
        match self.write(account, (folder, validity, uid), raw, date, false) {
            Ok(()) => {
                !self.is_removed(account) && self.path(account, folder, validity, uid).is_file()
            }
            Err(error) => {
                eprintln!("下载完成的邮件缓存写入失败，保留续传数据：{error:#}");
                false
            }
        }
    }

    /// 后台预先缓存的邮件：「最近使用」时间记成邮件的收件时间，
    /// 这样总量满了先删的是最老的邮件，而不是刚预先存下的新邮件。
    pub fn put_aged(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        raw: &[u8],
        date: i64,
    ) {
        self.put_inner(account, folder, validity, uid, raw, date, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn put_inner(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        raw: &[u8],
        date: i64,
        aged: bool,
    ) {
        if raw.len() > MAX_FILE || self.is_removed(account) {
            return;
        }
        // 预先缓存时已经有了（比如用户刚打开过）：别用旧的收件时间盖掉「刚用过」
        if aged && self.path(account, folder, validity, uid).is_file() {
            return;
        }
        if let Err(e) = self.write(account, (folder, validity, uid), raw, date, aged) {
            eprintln!("邮件原文写入本地缓存失败：{e:#}");
        }
    }

    fn write(
        &self,
        account: &str,
        (folder, validity, uid): (&str, u32, u32),
        raw: &[u8],
        date: i64,
        aged: bool,
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
        if aged {
            // 不知道收件时间的当作最老的，满了先删它
            let _ = f.set_modified(time_of(date).unwrap_or(SystemTime::UNIX_EPOCH));
        }
        drop(f);
        // 同一封邮件可能同时被写两次：替换和计数放在一个锁里，总量才不会多算
        let mut stats = self.stats.lock().unwrap();
        // 账号可能刚被删除：别再建出它的目录
        if self.is_removed(account) {
            let _ = fs::remove_file(&tmp);
            let _ = fs::remove_dir(dir);
            return Ok(());
        }
        let before = self.usage_locked(&mut stats, account);
        let replaced = fs::metadata(&path).map(|m| m.len()).ok();
        if let Err(e) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(e.into());
        }
        let now = Usage {
            bytes: before.bytes.saturating_sub(replaced.unwrap_or(0)) + raw.len() as u64 + 8,
            count: before.count + usize::from(replaced.is_none()),
        };
        let limit = self.limit(account);
        let now = if now.bytes > limit {
            self.prune(account, None, limit / 10 * 9)
        } else {
            now
        };
        stats.insert(account.to_string(), now);
        Ok(())
    }

    /// 从最久没用的删起，直到降到 `target`；`older_than` 限定只删比它旧的。返回剩下的占用。
    fn prune(&self, account: &str, older_than: Option<SystemTime>, target: u64) -> Usage {
        let mut all = files(&self.account_dir(account));
        all.sort_by_key(|(_, _, modified)| *modified);
        let mut left = Usage {
            bytes: all.iter().map(|(_, len, _)| len).sum(),
            count: all.len(),
        };
        for (path, len, modified) in all {
            if left.bytes <= target || older_than.is_some_and(|t| modified >= t) {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                left.bytes -= len;
                left.count -= 1;
            }
        }
        left
    }

    /// 后台预先缓存时腾地方：删掉比这封邮件（收件时间 `date`）更老的缓存，
    /// 直到放得下 `needed` 字节（再留 `reserve` 给打开的邮件）。返回是否放得下。
    pub fn make_room(&self, account: &str, needed: u64, reserve: u64, date: i64) -> bool {
        let limit = self.limit(account);
        let mut stats = self.stats.lock().unwrap();
        let used = self.usage_locked(&mut stats, account).bytes;
        let target = limit.saturating_sub(needed + reserve);
        if used <= target {
            return true;
        }
        // 不知道这封的收件时间：不知道哪些比它老，不删
        let Some(before) = time_of(date) else {
            return false;
        };
        let left = self.prune(account, Some(before), target);
        stats.insert(account.to_string(), left);
        left.bytes <= target
    }

    /// Remove only the damaged generation that was read. A concurrent healthy
    /// replacement wins; never delete user-managed export/archive files.
    pub fn remove_corrupt(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        uid: u32,
        expected: &[u8],
    ) {
        let path = self.path(account, folder, validity, uid);
        // Writes replace the file under this same lock.
        let mut stats = self.stats.lock().unwrap();
        let Ok(data) = fs::read(&path) else { return };
        if data.get(8..) != Some(expected) {
            return;
        }
        if fs::remove_file(&path).is_ok()
            && let Some(usage) = stats.get_mut(account)
        {
            usage.bytes = usage.bytes.saturating_sub(data.len() as u64);
            usage.count = usage.count.saturating_sub(1);
        }
    }

    /// 删掉某个文件夹里指定 UID 的原文（`uids` 为 None 时删整个文件夹）。
    pub fn remove(&self, account: &str, folder: &str, uids: Option<&[u32]>) {
        // A download may have finished its final checkpoint just before a
        // message deletion. Revoke its publication ticket under the same gate.
        let mut cleanup = self.cleanup.lock().unwrap();
        let policy = cleanup
            .entry(account.into())
            .or_insert_with(|| CleanupState::load(&self.account_dir(account), account));
        policy.generation = policy.generation.saturating_add(1);
        let prefix = format!("{}-", folder_hash(folder));
        self.resumes.remove(
            &self.account_dir(account).join("pending"),
            account,
            folder,
            &prefix,
            uids,
        );
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
                    && let Some(u) = self.stats.lock().unwrap().get_mut(account)
                {
                    u.bytes = u.bytes.saturating_sub(len);
                    u.count = u.count.saturating_sub(1);
                }
            }
        }
    }

    /// 删除账号时清掉它的全部缓存；之后还在路上的写入也会被丢掉
    pub fn remove_receive_account(&self, account: &str, last_epoch: u64) {
        let mut stats = self.stats.lock().unwrap();
        // Mark the public account before touching files, blocking every delayed epoch.
        self.removed.lock().unwrap().insert(account.to_string());
        for epoch in 1..=last_epoch.max(1) {
            self.clear_resume_account(&crate::mail_ref::namespace(account, epoch));
            let _ =
                fs::remove_dir_all(self.account_dir(&crate::mail_ref::namespace(account, epoch)));
        }
        stats.retain(|key, _| crate::mail_ref::storage_account(key) != account);
        self.limits
            .lock()
            .unwrap()
            .retain(|key, _| crate::mail_ref::storage_account(key) != account);
    }

    fn is_removed(&self, account: &str) -> bool {
        let removed = self.removed.lock().unwrap();
        removed.contains(account) || removed.contains(crate::mail_ref::storage_account(account))
    }
    pub fn remove_account(&self, account: &str) {
        let mut stats = self.stats.lock().unwrap();
        self.removed.lock().unwrap().insert(account.to_string());
        self.clear_resume_account(account);
        let _ = fs::remove_dir_all(self.account_dir(account));
        stats.remove(account);
        self.limits.lock().unwrap().remove(account);
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
    fn partial_downloads_are_not_complete_bodies_and_survive_reopening_only_at_their_identity() {
        let (root, s) = store(DEFAULT_LIMIT);
        let account = crate::mail_ref::namespace("a", 2);
        s.append_resume(&account, "INBOX", 7, 1, 20, 42, 0, b"first")
            .unwrap();
        assert!(s.get(&account, "INBOX", 7, 1).is_none());
        assert!(s.cached_uids(&account, "INBOX", 7).is_empty());
        assert_eq!(s.usage(&account), (0, 0));
        let reopened = BodyStore::new(root.path().join("bodies"), DEFAULT_LIMIT);
        assert_eq!(
            reopened.load_resume(&account, "inbox", 7, 1).unwrap().raw,
            b"first"
        );
        assert!(reopened.load_resume("a", "INBOX", 7, 1).is_none());
        assert!(reopened.load_resume(&account, "INBOX", 8, 1).is_none());
        assert!(reopened.load_resume(&account, "Archive", 7, 1).is_none());
        reopened.clear_resume(&account, "INBOX", 7, 1);
        assert!(reopened.load_resume(&account, "INBOX", 7, 1).is_none());
        reopened
            .append_resume(&account, "INBOX", 7, 1, 20, 42, 0, b"restart")
            .unwrap();
        assert_eq!(
            reopened.load_resume(&account, "INBOX", 7, 1).unwrap().raw,
            b"restart"
        );
    }

    #[test]
    fn completed_download_reports_only_successfully_stored_bodies() {
        let (_root, s) = store(DEFAULT_LIMIT);
        assert!(s.put_download("a", "INBOX", 7, 1, b"complete", 42));
        assert_eq!(s.get("a", "INBOX", 7, 1), Some((b"complete".to_vec(), 42)));
        fs::create_dir_all(s.path("a", "INBOX", 7, 2)).unwrap();
        assert!(!s.put_download("a", "INBOX", 7, 2, b"complete", 42));
        assert!(!s.put_download("a", "INBOX", 7, 3, &vec![0; MAX_FILE + 1], 42));
        s.remove_account("a");
        assert!(!s.put_download("a", "INBOX", 7, 1, b"late", 42));
    }

    #[test]
    fn removing_mail_or_a_folder_clears_all_matching_resume_generations_and_blocks_late_chunks() {
        let (_root, s) = store(DEFAULT_LIMIT);
        for validity in [7, 8] {
            for uid in [1, 2] {
                s.append_resume("a", "INBOX", validity, uid, 20, 42, 0, b"first")
                    .unwrap();
            }
        }
        s.remove("a", "inbox", Some(&[1]));
        assert!(s.load_resume("a", "INBOX", 8, 1).is_none());
        assert!(
            s.append_resume("a", "INBOX", 8, 1, 20, 42, 0, b"late")
                .is_err()
        );
        assert!(s.load_resume("a", "INBOX", 8, 2).is_some());
        s.remove("a", "INBOX", None);
        assert!(s.load_resume("a", "INBOX", 8, 2).is_none());
        assert!(
            s.append_resume("a", "INBOX", 8, 2, 20, 42, 0, b"late")
                .is_err()
        );
        s.append_resume("a", "INBOX", 9, 2, 20, 42, 0, b"new mailbox")
            .unwrap();
        assert_eq!(
            s.load_resume("a", "INBOX", 9, 2).unwrap().raw,
            b"new mailbox"
        );
    }

    #[test]
    fn endpoint_retirement_clears_pending_only_and_account_removal_blocks_every_epoch() {
        let (_root, s) = store(DEFAULT_LIMIT);
        let old = crate::mail_ref::namespace("a", 1);
        let new = crate::mail_ref::namespace("a", 2);
        s.put(&old, "INBOX", 7, 1, b"complete", 42);
        s.append_resume(&old, "INBOX", 7, 2, 20, 42, 0, b"first")
            .unwrap();
        s.clear_resume_account(&old);
        assert!(s.get(&old, "INBOX", 7, 1).is_some());
        assert!(s.load_resume(&old, "INBOX", 7, 2).is_none());
        assert!(
            s.append_resume(&old, "INBOX", 7, 2, 20, 42, 0, b"late")
                .is_err()
        );
        s.append_resume(&new, "INBOX", 7, 2, 20, 42, 0, b"new")
            .unwrap();
        s.remove_receive_account("a", 2);
        assert!(s.load_resume(&new, "INBOX", 7, 2).is_none());
        assert!(
            s.append_resume(&new, "INBOX", 7, 2, 20, 42, 0, b"late")
                .is_err()
        );
        assert!(
            s.append_resume(
                &crate::mail_ref::namespace("a", 3),
                "INBOX",
                7,
                2,
                20,
                42,
                0,
                b"late"
            )
            .is_err()
        );
    }

    #[test]
    fn corrupt_removal_is_generation_scoped_and_preserves_a_new_replacement() {
        let (_dir, store) = store(DEFAULT_LIMIT);
        store.put("a", "INBOX", 7, 1, &[0; 4], 0);
        store.put(
            "a",
            "INBOX",
            8,
            1,
            b"Subject: other generation\r\n\r\nbody",
            1,
        );
        store.put("a", "INBOX", 7, 2, b"Subject: other mail\r\n\r\nbody", 2);
        store.remove_corrupt("a", "INBOX", 7, 1, &[0; 4]);
        assert!(store.get("a", "INBOX", 7, 1).is_none());
        assert!(store.get("a", "INBOX", 8, 1).is_some());
        assert!(store.get("a", "INBOX", 7, 2).is_some());
        assert_eq!(store.usage("a").0, 2);
        let recovered = b"Subject: recovered\r\n\r\nbody";
        store.put("a", "INBOX", 7, 1, recovered, 3);
        store.remove_corrupt("a", "INBOX", 7, 1, &[0; 4]);
        assert_eq!(store.get("a", "INBOX", 7, 1).unwrap().0, recovered);
        assert_eq!(store.usage("a").0, 3);
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
    fn each_account_has_its_own_limit() {
        // 每封 100 + 8 字节
        let (_d, s) = store(250);
        s.set_limit("b", 1000);
        for uid in 1..=4 {
            s.put("a", "INBOX", 7, uid, &[0; 100], 0);
            s.put("b", "INBOX", 7, uid, &[0; 100], 0);
        }
        assert!(s.usage("a").1 <= 250, "a 用默认上限");
        assert_eq!(s.usage("b"), (4, 432), "b 的上限更大，一封都没删");
        assert_eq!(s.room("b"), 1000 - 432);
    }

    #[test]
    fn lowering_the_limit_frees_space_right_away() {
        let (_d, s) = store(DEFAULT_LIMIT);
        for uid in 1..=5 {
            s.put("a", "INBOX", 7, uid, &[0; 100], 0);
            age(&s, "INBOX", uid, 100 - uid as u64);
        }
        s.set_limit("a", 300);
        let (count, bytes) = s.usage("a");
        assert!(
            bytes <= 300 && count == bytes as usize / 108,
            "{count} {bytes}"
        );
        assert!(s.contains("a", "INBOX", 7, 5), "留下最近用过的");
        assert!(!s.contains("a", "INBOX", 7, 1));
    }

    #[test]
    fn usage_counts_only_finished_mail() {
        let (_d, s) = store(DEFAULT_LIMIT);
        assert_eq!(s.usage("a"), (0, 0));
        s.put("a", "INBOX", 7, 1, b"12", 0);
        s.put("a", "Sent", 7, 1, b"1234", 0);
        fs::write(s.account_dir("a").join(".tmp-1-1"), b"partial").unwrap();
        assert_eq!(s.usage("a"), (2, 2 + 8 + 4 + 8));
        assert!(s.contains("a", "INBOX", 7, 1));
        assert!(!s.contains("a", "INBOX", 8, 1));
    }

    #[test]
    fn a_preset_limit_is_used_by_the_first_write_without_scanning_first() {
        let (_d, s) = store(250);
        for uid in 1..=4 {
            s.put("a", "INBOX", 7, uid, &[0; 100], 0);
        }
        assert!(s.usage("a").1 <= 250);
        // 新进程：先记上限（用户调大过），第一次写入不能按默认的小上限删
        let s2 = BodyStore::new(s.dir.clone(), 250);
        s2.preset_limit("a", 10_000);
        s2.put("a", "INBOX", 7, 9, &[0; 100], 0);
        assert_eq!(s2.usage("a").0, s.usage("a").0 + 1);
    }

    #[test]
    fn pre_cached_mail_ages_by_its_received_date_and_makes_room_only_from_older_mail() {
        let (_d, s) = store(1000);
        let day = 86_400_000;
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        // 三封预先缓存的：收件时间 30、20、10 天前
        for (uid, days) in [(1, 30), (2, 20), (3, 10)] {
            s.put_aged("a", "INBOX", 7, uid, &[0; 200], now - days * day);
        }
        // 用户打开过的旧邮件：最近用过
        s.put("a", "Sent", 7, 1, &[0; 200], now - 400 * day);
        assert_eq!(s.cached_uids("a", "INBOX", 7), HashSet::from([1, 2, 3]));
        // 装一封 15 天前的：只能删比它老的（30 天前那封），够了就停
        assert!(s.make_room("a", 300, 0, now - 15 * day));
        assert!(!s.contains("a", "INBOX", 7, 1));
        assert!(s.contains("a", "INBOX", 7, 2) && s.contains("a", "Sent", 7, 1));
        // 再装一封 40 天前的：没有更老的可删，放不下
        assert!(!s.make_room("a", 600, 0, now - 40 * day));
        assert!(s.contains("a", "INBOX", 7, 2));
        assert_eq!(s.usage("a").0, 3, "记账跟着删除走");
    }

    #[test]
    fn mail_without_a_received_date_never_pushes_others_out() {
        let (_d, s) = store(1000);
        s.put("a", "INBOX", 7, 1, &[0; 400], 0);
        s.put_aged("a", "INBOX", 7, 2, &[0; 400], 0);
        assert!(!s.make_room("a", 400, 0, 0), "没有收件时间，不删别的");
        assert!(s.contains("a", "INBOX", 7, 1) && s.contains("a", "INBOX", 7, 2));
        // 它自己当作最老的：之后满了先删它
        s.put("a", "INBOX", 7, 3, &[0; 400], 0);
        assert!(!s.contains("a", "INBOX", 7, 2));
        assert!(s.contains("a", "INBOX", 7, 1) && s.contains("a", "INBOX", 7, 3));
    }

    #[test]
    fn writes_after_an_account_is_removed_are_dropped() {
        let (_d, s) = store(DEFAULT_LIMIT);
        s.put("a", "INBOX", 7, 1, b"1", 0);
        s.remove_account("a");
        s.put("a", "INBOX", 7, 2, b"2", 0);
        assert!(!s.account_dir("a").exists());
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
    #[test]
    fn removing_receiving_account_erases_old_epoch_disks_and_refuses_late_writes() {
        let (_d, s) = store(DEFAULT_LIMIT);
        for epoch in 1..=3 {
            s.put(
                &crate::mail_ref::namespace("a", epoch),
                "INBOX",
                7,
                1,
                b"body",
                1,
            );
        }
        s.put("ab", "INBOX", 7, 1, b"other", 1);
        s.remove_receive_account("a", 3);
        for epoch in 1..=4 {
            let key = crate::mail_ref::namespace("a", epoch);
            s.put(&key, "INBOX", 7, 2, b"late", 2);
            assert!(!s.account_dir(&key).exists());
        }
        assert_eq!(s.get("ab", "INBOX", 7, 1).unwrap().0, b"other");
    }
}
