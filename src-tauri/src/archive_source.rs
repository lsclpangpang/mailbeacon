//! 已下载的邮件（自动下载、全量下载保存的 .eml）当作只读缓存用。
//!
//! 只读：不加锁、不创建、不改也不删任何文件，连修改时间都不碰；下载的文件不计入
//! 缓存总量。任何错误都当作「没有」，调用方接着找服务器。磁盘操作都通过
//! [`ArchiveSource::within`] 放在阻塞线程里、限时跑：下载目录可能在网盘或睡着的移动硬盘上。

use crate::{
    auto_download, body_store,
    bulk_download::{find_account_directory, find_folder_directory},
    config::{Account, ReceiveProtocol},
};
use std::{
    collections::{BTreeSet, HashMap},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant, UNIX_EPOCH},
};

/// 没找到某个文件夹的下载目录时，这么久之内不再重新扫描父目录
const MISS_TTL: Duration = Duration::from_secs(30);
/// 读下载目录超时过的账号，这么久之内不再读（每次都等超时太慢）
const SLOW_TTL: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Dirs {
    /// 自动下载目录：只有收件箱
    auto: Option<PathBuf>,
    /// 全量下载时选的父目录：按账号、文件夹分子目录
    bulk: Vec<PathBuf>,
}

/// (账号, 全量下载父目录, IMAP 文件夹路径)
type FolderKey = (String, PathBuf, String);

#[derive(Default)]
pub struct ArchiveSource {
    dirs: Mutex<HashMap<String, Dirs>>,
    /// 找到过的文件夹子目录；目录没了就丢掉重新找
    folders: Mutex<HashMap<FolderKey, PathBuf>>,
    /// 最近没找到的，避免每打开一封邮件都扫描一遍父目录
    misses: Mutex<HashMap<FolderKey, Instant>>,
    /// 最近读下载目录超时的账号
    slow: Mutex<HashMap<String, Instant>>,
    #[cfg(test)]
    scans: std::sync::atomic::AtomicUsize,
}

impl ArchiveSource {
    /// 按账号配置登记它的下载目录（改了设置就再调一次）；POP3 账号不登记。
    pub fn register(&self, acct: &Account) {
        if acct.receive_protocol == ReceiveProtocol::Pop3 {
            self.remove_account(&acct.id);
            return;
        }
        // 相对路径（自动下载关着时可以随便填）不知道相对哪里，不用
        let absolute = |d: &str| Some(PathBuf::from(d.trim())).filter(|p| p.is_absolute());
        let auto = absolute(&acct.auto_download.directory);
        let bulk = acct
            .archive_dirs
            .iter()
            .filter_map(|d| absolute(d))
            .collect();
        self.set_dirs(&acct.id, auto, bulk);
    }

    pub fn set_dirs(&self, account_id: &str, auto: Option<PathBuf>, bulk: Vec<PathBuf>) {
        let dirs = Dirs { auto, bulk };
        let mut all = self.dirs.lock().unwrap();
        if all.get(account_id) == Some(&dirs) {
            return;
        }
        all.insert(account_id.to_string(), dirs);
        drop(all);
        self.forget_lookups(account_id);
    }

    pub fn remove_account(&self, account_id: &str) {
        self.dirs.lock().unwrap().remove(account_id);
        self.forget_lookups(account_id);
    }

    fn forget_lookups(&self, account_id: &str) {
        self.folders
            .lock()
            .unwrap()
            .retain(|k, _| k.0 != account_id);
        self.misses.lock().unwrap().retain(|k, _| k.0 != account_id);
        self.slow.lock().unwrap().remove(account_id);
    }

    /// 在阻塞线程里做 `f`，超过 `limit` 就不等了（返回 None），并且这个账号一段时间内
    /// 直接当没有，不再去读下载目录。后台那次读完就丢掉。读信用。
    pub async fn within<T: Send + 'static>(
        self: &Arc<Self>,
        account_id: &str,
        limit: Duration,
        f: impl FnOnce(&ArchiveSource) -> Option<T> + Send + 'static,
    ) -> Option<T> {
        if self.is_slow(account_id) {
            return None;
        }
        match self.limited(limit, f).await {
            Ok(found) => found,
            Err(_) => {
                eprintln!("读已下载的邮件超时，{} 秒内先不读", SLOW_TTL.as_secs());
                self.slow
                    .lock()
                    .unwrap()
                    .insert(account_id.to_string(), Instant::now());
                None
            }
        }
    }

    /// 同上，但超时不标记账号（后台预先缓存的大批扫描用：盘大不等于盘坏）。
    /// 账号刚因为读信超时被标记过时也不扫，免得在卡住的盘上再压一个线程。
    pub async fn scan_within<T: Send + 'static>(
        self: &Arc<Self>,
        account_id: &str,
        limit: Duration,
        f: impl FnOnce(&ArchiveSource) -> Option<T> + Send + 'static,
    ) -> Option<T> {
        if self.is_slow(account_id) {
            return None;
        }
        self.limited(limit, f).await.ok().flatten()
    }

    fn is_slow(&self, account_id: &str) -> bool {
        let slow_since = self.slow.lock().unwrap().get(account_id).copied();
        slow_since.is_some_and(|at| at.elapsed() < SLOW_TTL)
    }

    async fn limited<T: Send + 'static>(
        self: &Arc<Self>,
        limit: Duration,
        f: impl FnOnce(&ArchiveSource) -> Option<T> + Send + 'static,
    ) -> Result<Option<T>, tokio::time::error::Elapsed> {
        let me = self.clone();
        let done = tokio::time::timeout(limit, tokio::task::spawn_blocking(move || f(&me))).await?;
        Ok(done.ok().flatten())
    }

    /// 这个账号、这个文件夹可能存着邮件的下载目录（还没确认里面有没有这一封）
    fn folder_dirs(&self, account_id: &str, folder: &str) -> Vec<PathBuf> {
        let Some(dirs) = self.dirs.lock().unwrap().get(account_id).cloned() else {
            return Vec::new();
        };
        let mut out = Vec::new();
        // 文件名里不带文件夹：自动下载目录要是指到了别的文件夹的全量下载目录，不能当收件箱用
        if folder.eq_ignore_ascii_case("INBOX")
            && let Some(auto) = dirs.auto
            && fs::read(auto.join(".mailpulse-identity"))
                .map_or(true, |marker| marker.eq_ignore_ascii_case(b"folder:INBOX"))
        {
            out.push(auto);
        }
        for parent in dirs.bulk {
            if let Some(dir) = self.bulk_folder(account_id, parent, folder) {
                out.push(dir);
            }
        }
        out
    }

    fn bulk_folder(&self, account_id: &str, parent: PathBuf, folder: &str) -> Option<PathBuf> {
        let key = (account_id.to_string(), parent, folder.to_string());
        let cached = self.folders.lock().unwrap().get(&key).cloned();
        if let Some(dir) = cached {
            if fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
                return Some(dir);
            }
            self.folders.lock().unwrap().remove(&key);
        }
        if self
            .misses
            .lock()
            .unwrap()
            .get(&key)
            .is_some_and(|at| at.elapsed() < MISS_TTL)
        {
            return None;
        }
        #[cfg(test)]
        self.scans
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let found = find_account_directory(&key.1, account_id)
            .and_then(|root| find_folder_directory(&root, folder));
        match &found {
            Some(dir) => {
                self.misses.lock().unwrap().remove(&key);
                self.folders.lock().unwrap().insert(key, dir.clone());
            }
            None => {
                self.misses.lock().unwrap().insert(key, Instant::now());
            }
        }
        found
    }

    /// 在这个账号的下载目录里找这封邮件的原文和日期（文件修改时间，毫秒）。
    pub fn read(
        &self,
        account_id: &str,
        folder: &str,
        validity: u32,
        uid: u32,
    ) -> Option<(Vec<u8>, i64)> {
        for dir in self.folder_dirs(account_id, folder) {
            match read_one(&dir, account_id, validity, uid) {
                Ok(Some(found)) => return Some(found),
                Ok(None) => {}
                Err(e) => eprintln!("已下载的邮件不能用（{}）：{e:#}", dir.display()),
            }
        }
        None
    }

    /// `candidates` 里已下载、而且真能读到的 UID（预先缓存跳过用）。索引在但文件
    /// 被删了、太大或不是普通文件的不算，不然预先缓存会永远跳过它们。逐封确认要读
    /// 索引，盘慢时很费时间：超过 `budget` 就停，返回已经确认的，剩下的照常缓存。
    pub fn uids(
        &self,
        account_id: &str,
        folder: &str,
        validity: u32,
        candidates: &BTreeSet<u32>,
        budget: Duration,
    ) -> BTreeSet<u32> {
        let started = Instant::now();
        let readable = |dir: &Path, uid: u32| {
            auto_download::find_archived(dir, account_id, validity, uid)
                .ok()
                .flatten()
                .and_then(|path| fs::symlink_metadata(path).ok())
                .is_some_and(|m| m.len() <= MAX_READ)
        };
        // 先只看文件名（便宜），再从新到旧逐封确认：时间不够时，没确认的是旧邮件，
        // 而预先缓存正是从新邮件开始下的
        let mut listed: Vec<(u32, PathBuf)> = Vec::new();
        for dir in self.folder_dirs(account_id, folder) {
            for uid in auto_download::archived_uids(&dir, account_id, validity) {
                if candidates.contains(&uid) {
                    listed.push((uid, dir.clone()));
                }
            }
        }
        listed.sort_by_key(|(uid, _)| std::cmp::Reverse(*uid));
        let mut out = BTreeSet::new();
        for (uid, dir) in listed {
            if started.elapsed() >= budget {
                break;
            }
            if !out.contains(&uid) && readable(&dir, uid) {
                out.insert(uid);
            }
        }
        out
    }
}

/// 设置页里的一行：一个可以当缓存用的下载目录
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub path: String,
    /// auto（自动下载，只有收件箱）| bulk（全量下载）
    pub kind: &'static str,
    /// 目录现在能不能读到（移动硬盘拔了就是 false）
    pub online: bool,
    /// 自动下载目录要到自动下载设置里改，这里不能移除
    pub removable: bool,
}

/// 这个账号所有当缓存用的下载目录（磁盘操作，放在阻塞线程里跑）
pub fn locations(acct: &Account) -> Vec<Location> {
    let mut out = locations_offline(acct);
    for row in &mut out {
        let path = Path::new(&row.path);
        row.online = path.is_absolute()
            && if row.kind == "auto" {
                fs::metadata(path).is_ok_and(|m| m.is_dir())
            } else {
                find_account_directory(path, &acct.id).is_some()
            };
    }
    out
}

/// 同上，但不碰磁盘，全都标成不在线（目录读不动时用）
pub fn locations_offline(acct: &Account) -> Vec<Location> {
    if acct.receive_protocol == ReceiveProtocol::Pop3 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let auto = acct.auto_download.directory.trim();
    if !auto.is_empty() {
        out.push(Location {
            path: auto.to_string(),
            kind: "auto",
            online: false,
            removable: false,
        });
    }
    for dir in &acct.archive_dirs {
        out.push(Location {
            path: dir.clone(),
            kind: "bulk",
            online: false,
            removable: true,
        });
    }
    out
}

/// 用户手动添加的全量下载目录：必须是这个账号做过全量下载的父目录（选成了账号目录
/// 本身也行，换成它的上一级）。只读检查，不写任何东西。返回规范化后的父目录。
pub fn check_bulk_parent(directory: &str, account_id: &str) -> anyhow::Result<PathBuf> {
    let path = Path::new(directory.trim());
    if !path.is_absolute() {
        anyhow::bail!("请选择有效的本地绝对路径");
    }
    let path = fs::canonicalize(path).map_err(|_| anyhow::anyhow!("目录不存在或无法访问"))?;
    if find_account_directory(&path, account_id).is_some() {
        return Ok(path);
    }
    if let Some(parent) = path.parent()
        && find_account_directory(parent, account_id).as_deref() == Some(path.as_path())
    {
        return Ok(parent.to_path_buf());
    }
    anyhow::bail!("这个目录里没有这个账号全量下载的邮件")
}

/// 下载的原文最大读多少：和磁盘缓存单封上限一样（测试里调小）
const MAX_READ: u64 = if cfg!(test) {
    4096
} else {
    body_store::MAX_FILE as u64
};

fn read_one(
    dir: &Path,
    account_id: &str,
    validity: u32,
    uid: u32,
) -> anyhow::Result<Option<(Vec<u8>, i64)>> {
    let Some(path) = auto_download::find_archived(dir, account_id, validity, uid)? else {
        return Ok(None);
    };
    // 只打开一次：类型和大小都按打开的这个文件看。打开前后是不是同一个文件（dev/ino）
    // 也要核对，检查完被换成符号链接或大文件也读不到
    let before = fs::symlink_metadata(&path)?;
    if !before.is_file() {
        return Ok(None);
    }
    let file = fs::File::open(&path)?;
    let metadata = file.metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if (metadata.dev(), metadata.ino()) != (before.dev(), before.ino()) {
            anyhow::bail!("邮件文件在读取时被替换：{}", path.display());
        }
    }
    if !metadata.is_file() || metadata.len() > MAX_READ {
        return Ok(None);
    }
    let mut raw = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_READ + 1).read_to_end(&mut raw)?;
    if raw.len() as u64 > MAX_READ {
        return Ok(None);
    }
    // 被改坏、不像邮件的文件不用
    let looks_like_mail = crate::mime::parse(&raw).is_ok_and(|m| {
        m.from().is_some()
            || m.date().is_some()
            || m.subject().is_some()
            || m.message_id().is_some()
    });
    if !looks_like_mail {
        anyhow::bail!("不是有效的邮件：{}", path.display());
    }
    let date = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    Ok(Some((raw, date)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auto_download::{Checkpoint, save_atomic};
    use crate::imap::Folder;
    use std::sync::{Arc, atomic::Ordering};

    const FOREVER: Duration = Duration::from_secs(60);

    fn all() -> BTreeSet<u32> {
        (1..100).collect()
    }

    fn mail(uid: u32) -> Vec<u8> {
        format!("Subject: Mail {uid}\r\nFrom: a@example.com\r\n\r\nbody {uid}").into_bytes()
    }

    fn save(dir: &Path, account: &str, validity: u32, uid: u32) -> PathBuf {
        let cp = Checkpoint {
            uid_validity: validity,
            last_uid: uid,
        };
        save_atomic(dir, account, cp, uid, &mail(uid)).unwrap()
    }

    fn account(id: &str) -> Account {
        serde_json::from_value(serde_json::json!({
            "id": id, "kind":"custom", "email":"a@example.com", "name":"A",
            "imapHost":"localhost", "imapPort":993, "smtpHost":"localhost", "smtpPort":465
        }))
        .unwrap()
    }

    /// 用全量下载的规则在 parent 下建出账号/文件夹目录
    fn bulk_folder(parent: &Path, account_id: &str, folder: &str) -> PathBuf {
        let root = crate::bulk_download::account_directory(parent, &account(account_id)).unwrap();
        crate::bulk_download::stable_folder_directory(
            &root,
            &Folder {
                path: folder.into(),
                name: folder.into(),
                role: "other".into(),
            },
        )
        .unwrap()
    }

    fn snapshot(dir: &Path) -> Vec<(PathBuf, u64, std::time::SystemTime)> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for entry in fs::read_dir(&d).unwrap() {
                let path = entry.unwrap().path();
                let m = fs::symlink_metadata(&path).unwrap();
                if m.is_dir() {
                    stack.push(path.clone());
                }
                out.push((path, m.len(), m.modified().unwrap()));
            }
        }
        out.sort();
        out
    }

    #[test]
    fn reads_inbox_mail_from_the_auto_download_directory() {
        let auto = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        let (raw, date) = source.read("a", "INBOX", 7, 1).unwrap();
        assert_eq!(raw, mail(1));
        assert!(date > 0);
    }

    #[test]
    fn auto_download_directory_is_only_used_for_inbox() {
        let auto = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        assert!(source.read("a", "Archive", 7, 1).is_none());
    }

    #[test]
    fn reads_any_folder_from_a_bulk_directory() {
        let parent = tempfile::tempdir().unwrap();
        let archive = bulk_folder(parent.path(), "a", "Archive");
        save(&archive, "a", 9, 3);
        let source = ArchiveSource::default();
        source.set_dirs("a", None, vec![parent.path().into()]);
        assert_eq!(source.read("a", "Archive", 9, 3).unwrap().0, mail(3));
        assert!(source.read("a", "INBOX", 9, 3).is_none());
        assert!(source.read("a", "Archive", 8, 3).is_none());
    }

    #[test]
    fn register_follows_account_settings_and_skips_pop3() {
        let auto = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        save(&bulk_folder(parent.path(), "a", "Sent"), "a", 4, 2);
        let mut acct = account("a");
        acct.auto_download.directory = auto.path().display().to_string();
        acct.archive_dirs = vec![parent.path().display().to_string()];
        let source = ArchiveSource::default();
        source.register(&acct);
        assert!(source.read("a", "INBOX", 7, 1).is_some());
        assert!(source.read("a", "Sent", 4, 2).is_some());
        acct.receive_protocol = ReceiveProtocol::Pop3;
        source.register(&acct);
        assert!(source.read("a", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn unparsable_file_is_ignored() {
        let auto = tempfile::tempdir().unwrap();
        let path = save(auto.path(), "a", 7, 1);
        fs::write(&path, b"\x00\x01\x02 garbage without headers").unwrap();
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        assert!(source.read("a", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn missing_directory_or_file_is_just_a_miss() {
        let auto = tempfile::tempdir().unwrap();
        let path = save(auto.path(), "a", 7, 1);
        let source = ArchiveSource::default();
        source.set_dirs(
            "a",
            Some(auto.path().join("gone")),
            vec![auto.path().join("also-gone")],
        );
        assert!(source.read("a", "INBOX", 7, 1).is_none());
        assert!(source.uids("a", "INBOX", 7, &all(), FOREVER).is_empty());
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        fs::remove_file(path).unwrap();
        assert!(source.read("a", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn reading_never_touches_the_files() {
        let auto = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        save(&bulk_folder(parent.path(), "a", "Archive"), "a", 9, 3);
        let before = (snapshot(auto.path()), snapshot(parent.path()));
        std::thread::sleep(Duration::from_millis(20));
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![parent.path().into()]);
        source.read("a", "INBOX", 7, 1).unwrap();
        source.read("a", "Archive", 9, 3).unwrap();
        source.read("a", "Missing", 9, 3);
        source.uids("a", "INBOX", 7, &all(), FOREVER);
        source.uids("a", "Archive", 9, &all(), FOREVER);
        assert_eq!((snapshot(auto.path()), snapshot(parent.path())), before);
    }

    #[test]
    fn oversize_file_is_not_read() {
        let auto = tempfile::tempdir().unwrap();
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 1,
        };
        let mut big = mail(1);
        big.resize(MAX_READ as usize + 1, b'x');
        save_atomic(auto.path(), "a", cp, 1, &big).unwrap();
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        assert!(source.read("a", "INBOX", 7, 1).is_none());
    }

    #[test]
    fn folder_directory_lookups_are_cached_and_dropped_when_stale() {
        let parent = tempfile::tempdir().unwrap();
        let archive = bulk_folder(parent.path(), "a", "Archive");
        save(&archive, "a", 9, 3);
        save(&archive, "a", 9, 4);
        let source = ArchiveSource::default();
        source.set_dirs("a", None, vec![parent.path().into()]);
        source.read("a", "Archive", 9, 3).unwrap();
        source.read("a", "Archive", 9, 4).unwrap();
        assert_eq!(source.scans.load(Ordering::Relaxed), 1);
        // 没下载过的文件夹：一段时间内不反复扫描
        source.read("a", "Sent", 9, 3);
        source.read("a", "Sent", 9, 4);
        assert_eq!(source.scans.load(Ordering::Relaxed), 2);
        // 目录被删：缓存项失效，重新找也找不到
        fs::remove_dir_all(&archive).unwrap();
        assert!(source.read("a", "Archive", 9, 3).is_none());
        assert_eq!(source.scans.load(Ordering::Relaxed), 3);
        assert!(source.folders.lock().unwrap().is_empty());
    }

    #[test]
    fn changing_directories_forgets_earlier_misses() {
        let parent = tempfile::tempdir().unwrap();
        let source = ArchiveSource::default();
        source.set_dirs("a", None, vec![parent.path().into()]);
        assert!(source.read("a", "Archive", 9, 3).is_none());
        // 之后才做了全量下载，并登记了另一个目录
        let other = tempfile::tempdir().unwrap();
        save(&bulk_folder(other.path(), "a", "Archive"), "a", 9, 3);
        source.set_dirs("a", None, vec![parent.path().into(), other.path().into()]);
        assert!(source.read("a", "Archive", 9, 3).is_some());
    }

    #[test]
    fn uids_unions_all_directories() {
        let auto = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        save(auto.path(), "a", 7, 2);
        let inbox = bulk_folder(parent.path(), "a", "INBOX");
        save(&inbox, "a", 7, 2);
        save(&inbox, "a", 7, 5);
        save(&inbox, "b", 7, 6);
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![parent.path().into()]);
        assert_eq!(
            source.uids("a", "INBOX", 7, &all(), FOREVER),
            BTreeSet::from([1, 2, 5])
        );
    }

    #[test]
    fn locations_list_auto_and_bulk_directories_with_online_state() {
        let auto = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        bulk_folder(parent.path(), "a", "INBOX");
        let mut acct = account("a");
        acct.auto_download.directory = auto.path().display().to_string();
        acct.archive_dirs = vec![parent.path().display().to_string(), "/no/such/disk".into()];
        let rows = locations(&acct);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            (rows[0].kind, rows[0].online, rows[0].removable),
            ("auto", true, false)
        );
        assert_eq!(
            (rows[1].kind, rows[1].online, rows[1].removable),
            ("bulk", true, true)
        );
        assert_eq!(rows[2].path, "/no/such/disk");
        assert!(!rows[2].online);
        // 没设自动下载目录就不列；POP3 账号什么都不列
        acct.auto_download.directory.clear();
        assert_eq!(locations(&acct).len(), 2);
        // 读不动目录时：照样列出来，都标成不在线
        assert!(locations_offline(&acct).iter().all(|r| !r.online));
        assert_eq!(locations_offline(&acct).len(), 2);
        acct.receive_protocol = ReceiveProtocol::Pop3;
        assert!(locations(&acct).is_empty());
    }

    #[test]
    fn a_directory_is_accepted_only_if_it_holds_this_accounts_download() {
        let parent = tempfile::tempdir().unwrap();
        bulk_folder(parent.path(), "a", "INBOX");
        let accepted = check_bulk_parent(&parent.path().display().to_string(), "a").unwrap();
        assert_eq!(accepted, fs::canonicalize(parent.path()).unwrap());
        // 选成了账号目录本身也认：换成它的上一级
        let root = find_account_directory(parent.path(), "a").unwrap();
        assert_eq!(
            check_bulk_parent(&root.display().to_string(), "a").unwrap(),
            accepted
        );
        assert!(check_bulk_parent(&parent.path().display().to_string(), "b").is_err());
        assert!(check_bulk_parent("relative/dir", "a").is_err());
        assert!(check_bulk_parent("/no/such/disk", "a").is_err());
        // 只读检查：不在目录里写任何东西
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 1);
    }

    #[test]
    fn unregistered_or_removed_account_reads_nothing() {
        let auto = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        let source = ArchiveSource::default();
        assert!(source.read("a", "INBOX", 7, 1).is_none());
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        source.remove_account("a");
        assert!(source.read("a", "INBOX", 7, 1).is_none());
        assert!(source.uids("a", "INBOX", 7, &all(), FOREVER).is_empty());
    }

    #[test]
    fn uids_count_only_mail_that_can_actually_be_read() {
        let auto = tempfile::tempdir().unwrap();
        save(auto.path(), "a", 7, 1);
        let gone = save(auto.path(), "a", 7, 2);
        fs::remove_file(gone).unwrap();
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 3,
        };
        let mut big = mail(3);
        big.resize(MAX_READ as usize + 1, b'x');
        save_atomic(auto.path(), "a", cp, 3, &big).unwrap();
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        assert_eq!(
            source.uids("a", "INBOX", 7, &all(), FOREVER),
            BTreeSet::from([1])
        );
    }

    #[tokio::test]
    async fn a_slow_directory_is_skipped_for_a_while() {
        let source = Arc::new(ArchiveSource::default());
        let started = Instant::now();
        let slow = source
            .within("a", Duration::from_millis(50), |_| {
                std::thread::sleep(Duration::from_millis(500));
                Some(1)
            })
            .await;
        assert_eq!(slow, None);
        assert!(started.elapsed() < Duration::from_millis(400));
        // 刚超时过：这个账号先不读下载目录，直接当没有
        assert_eq!(
            source
                .within("a", Duration::from_secs(1), |_| Some(2))
                .await,
            None
        );
        // 后台扫描也先不扫
        assert_eq!(
            source
                .scan_within("a", Duration::from_secs(1), |_| Some(5))
                .await,
            None
        );
        // 别的账号不受影响
        assert_eq!(
            source
                .within("b", Duration::from_secs(1), |_| Some(3))
                .await,
            Some(3)
        );
        // 改了目录设置就重新试
        source.set_dirs("a", Some("/x".into()), vec![]);
        assert_eq!(
            source
                .within("a", Duration::from_secs(1), |_| Some(4))
                .await,
            Some(4)
        );
    }

    #[test]
    fn relative_directories_are_ignored() {
        let mut acct = account("a");
        acct.auto_download.directory = "mail".into();
        acct.archive_dirs = vec!["downloads".into()];
        let source = ArchiveSource::default();
        source.register(&acct);
        assert_eq!(source.dirs.lock().unwrap().get("a"), Some(&Dirs::default()));
    }

    #[test]
    fn auto_directory_pointing_at_another_folders_download_is_not_used_for_inbox() {
        let parent = tempfile::tempdir().unwrap();
        let sent = bulk_folder(parent.path(), "a", "Sent");
        save(&sent, "a", 1, 5);
        let inbox = bulk_folder(parent.path(), "a", "INBOX");
        save(&inbox, "a", 1, 6);
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(sent), vec![]);
        assert!(source.read("a", "INBOX", 1, 5).is_none());
        assert!(source.uids("a", "INBOX", 1, &all(), FOREVER).is_empty());
        source.set_dirs("a", Some(inbox), vec![]);
        assert!(source.read("a", "INBOX", 1, 6).is_some());
        assert!(
            source.read("a", "Inbox", 1, 6).is_some(),
            "收件箱的名字不分大小写"
        );
    }

    #[test]
    fn uids_check_only_candidates_and_stop_when_out_of_time() {
        let auto = tempfile::tempdir().unwrap();
        for uid in 1..=4 {
            save(auto.path(), "a", 7, uid);
        }
        let source = ArchiveSource::default();
        source.set_dirs("a", Some(auto.path().into()), vec![]);
        // 已经缓存过的不用再确认
        assert_eq!(
            source.uids("a", "INBOX", 7, &BTreeSet::from([2, 3, 9]), FOREVER),
            BTreeSet::from([2, 3])
        );
        // 时间用完了：只返回已经确认的（这里一个都没有），剩下的照常从服务器缓存
        assert!(
            source
                .uids("a", "INBOX", 7, &all(), Duration::ZERO)
                .is_empty()
        );
    }

    #[tokio::test]
    async fn a_slow_scan_gives_up_without_blocking_reads() {
        let source = Arc::new(ArchiveSource::default());
        let slow = source
            .scan_within("a", Duration::from_millis(50), |_| {
                std::thread::sleep(Duration::from_millis(500));
                Some(1)
            })
            .await;
        assert_eq!(slow, None);
        // 读信不受影响
        assert_eq!(
            source
                .within("a", Duration::from_secs(1), |_| Some(2))
                .await,
            Some(2)
        );
    }
}
