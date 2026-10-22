//! 应用标识变更的一次性数据迁移。

use anyhow::{Context, Result, bail};
use ring::digest;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const LEGACY_ID: &str = "com.lsclpangpang.mailpulse";
const CURRENT_ID: &str = "com.lsclpangpang.mailbeacon";
const MARKER: &str = ".identity-migration.json";
const SOURCE_DIGEST: &str = ".identity-migration-source";
const FINISHED: &str = ".identity-migration-finished";
const LOCK: &str = ".mailbeacon-identity-migration.lock";
const STAGING_PREFIX: &str = ".mailbeacon-migrating-";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    BeforeCopy,
    FileCopied,
    Copied,
    Validated,
    Publishing,
}

/// 必须在 WebView 和任何配置/数据库初始化之前调用；失败时停止启动。
pub fn prepare(target: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    let web_profile = Some(
        dirs::home_dir()
            .context("找不到用户目录")?
            .join("Library/WebKit")
            .join(CURRENT_ID),
    );
    #[cfg(not(target_os = "macos"))]
    let web_profile: Option<PathBuf> = None;
    prepare_roots(target, web_profile.as_deref(), |phase| {
        if phase != Phase::FileCopied {
            ensure_legacy_stopped()?;
        }
        Ok(())
    })
}

fn prepare_roots(
    target: &Path,
    web_profile: Option<&Path>,
    mut check: impl FnMut(Phase) -> Result<()>,
) -> Result<()> {
    // Check on every launch, including when the one-time migration is already complete.
    check(Phase::BeforeCopy)?;
    directory_exists(target)?;
    let finished = target.join(FINISHED);
    reject_link(&finished)?;
    if finished.try_exists()? {
        if !finished.is_file() || finished.metadata()?.len() != 0 {
            bail!("整体迁移完成标记异常，请保留现有数据并检查备份");
        }
        return Ok(());
    }
    // A first attempt may have published config but failed before publishing WebKit.
    // Until both roots finish, never silently resume from a stale source snapshot.
    verify_pending_source(target)?;
    if let Some(profile) = web_profile {
        verify_pending_source(profile)?;
    }
    prepare_with(target, &mut check).context("邮箱数据迁移未完成")?;
    if let Some(profile) = web_profile {
        // WebKit keeps localStorage outside app_config_dir. Copy the complete profile,
        // including salt, origin metadata and SQLite journals, without decoding its private format.
        // Separate atomic markers allow retry if the app exits between the two directories.
        prepare_with(profile, check).context("界面偏好迁移未完成")?;
    }
    verify_pending_source(target)?;
    if let Some(profile) = web_profile {
        verify_pending_source(profile)?;
    }
    let migrated = target.join(MARKER).try_exists()?
        || match web_profile {
            Some(profile) => profile.join(MARKER).try_exists()?,
            None => false,
        };
    if migrated {
        // A reinstall can leave only WebKit data. Record its completion too, before
        // normal startup creates the new mailbox store, so old sources can be retired.
        if !directory_exists(target)? {
            fs::create_dir(target)?;
            make_private(target, true)?;
        }
        private_options()
            .create_new(true)
            .open(finished)?
            .sync_all()?;
        sync_directory(target)?;
    }
    Ok(())
}

fn verify_pending_source(target: &Path) -> Result<()> {
    if !directory_exists(target)? || !target.join(MARKER).try_exists()? {
        return Ok(());
    }
    let digest_path = target.join(SOURCE_DIGEST);
    reject_link(&digest_path)?;
    let expected = fs::read(digest_path)
        .context("首次迁移尚未全部完成，来源校验记录缺失，请保留新旧数据目录")?;
    let source = target.parent().context("数据目录无父目录")?.join(LEGACY_ID);
    directory_exists(&source)?;
    let mut current = BTreeMap::new();
    scan_tree(&source, &source, &mut current)?;
    if inventory_digest(&current) != expected {
        bail!(
            "首次迁移尚未全部完成，但旧数据已经变化。已停止启动，请保留新旧目录并核对草稿和待发邮件，不要再次使用旧版写入数据"
        );
    }
    Ok(())
}

fn prepare_with(target: &Path, mut check: impl FnMut(Phase) -> Result<()>) -> Result<()> {
    if target.file_name().and_then(|s| s.to_str()) != Some(CURRENT_ID) {
        bail!("应用数据目录与 MailBeacon 标识不一致");
    }
    let parent = target.parent().context("找不到应用数据父目录")?;
    fs::create_dir_all(parent)?;
    let lock_path = parent.join(LOCK);
    reject_link(&lock_path)?;
    let lock = private_options().read(true).open(&lock_path)?;
    lock.try_lock()
        .context("另一个 MailBeacon 正在迁移，请稍后重新打开")?;
    cleanup_staging(parent)?;
    let source = parent.join(LEGACY_ID);
    let target_exists = directory_exists(target)?;
    if target_exists && target.join(MARKER).try_exists()? {
        reject_link(&target.join(MARKER))?;
        let marker: serde_json::Value = serde_json::from_slice(&fs::read(target.join(MARKER))?)?;
        if marker != migration_marker() {
            bail!("数据迁移标记无法识别，请保留两个数据目录并检查备份");
        }
        return Ok(());
    }
    if !directory_exists(&source)? || fs::read_dir(&source)?.next().is_none() {
        return Ok(());
    }
    ensure_empty_target(target)?;
    if [MARKER, SOURCE_DIGEST, FINISHED]
        .into_iter()
        .any(|name| source.join(name).exists())
    {
        bail!("旧目录含有迁移标记，请先确认要使用的数据副本");
    }
    check(Phase::BeforeCopy)?;
    let stage = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(parent)
        .context("无法创建迁移暂存目录，请检查磁盘空间和权限")?;
    make_private(stage.path(), true)?;
    // Identifies our abandoned staging directories after a kill or power failure.
    // This marker only becomes authoritative when the whole directory is published.
    let mut marker = private_options()
        .create_new(true)
        .open(stage.path().join(MARKER))?;
    marker.write_all(&serde_json::to_vec(&migration_marker())?)?;
    marker.sync_all()?;
    drop(marker);
    let mut copied = BTreeMap::new();
    copy_tree(&source, &source, stage.path(), &mut copied, &mut check)?;
    check(Phase::Copied)?;
    ensure_unchanged(&source, &copied)?;
    validate_database(stage.path())?;
    remap_accounts(stage.path(), &source, target)?;
    check(Phase::Validated)?;
    private_options()
        .create_new(true)
        .open(stage.path().join(SOURCE_DIGEST))?
        .write_all(&inventory_digest(&copied))?;
    sync_tree(stage.path())?;
    check(Phase::Publishing)?;
    ensure_unchanged(&source, &copied)?;
    ensure_empty_target(target)?;
    if directory_exists(target)? {
        // remove_dir refuses a directory populated by another writer in the meantime.
        fs::remove_dir(target).context("新目录出现数据，已停止迁移，未覆盖任何文件")?;
    }
    fs::rename(stage.path(), target).context("无法启用迁移数据，旧目录已保留")?;
    // Marker and data are published together. A crash here is safely recognized on retry.
    sync_directory(parent).context("新数据已启用，但目录同步失败，请重启检查；不要删除新目录")?;
    Ok(())
}

fn migration_marker() -> serde_json::Value {
    serde_json::json!({"version": 1, "from": LEGACY_ID, "to": CURRENT_ID})
}

fn cleanup_staging(parent: &Path) -> Result<()> {
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(STAGING_PREFIX)
            || !entry.file_type()?.is_dir()
        {
            continue;
        }
        let path = entry.path();
        if fs::read_dir(&path)?.next().is_none() {
            fs::remove_dir(path)?;
            continue;
        }
        let marker = path.join(MARKER);
        let Ok(meta) = fs::symlink_metadata(&marker) else {
            continue;
        };
        if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 512 {
            continue;
        }
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&fs::read(marker)?) else {
            continue;
        };
        if value == migration_marker() {
            fs::remove_dir_all(&path).context("无法清理上次中断的迁移副本，请检查磁盘权限")?;
        }
    }
    Ok(())
}

fn reject_link(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            bail!("迁移路径包含符号链接，已停止：{}", path.display())
        }
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

fn directory_exists(path: &Path) -> Result<bool> {
    reject_link(path)?;
    match fs::metadata(path) {
        Ok(meta) if meta.is_dir() => Ok(true),
        Ok(_) => bail!("预期数据目录的位置不是目录：{}", path.display()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e.into()),
    }
}

fn ensure_empty_target(target: &Path) -> Result<()> {
    if directory_exists(target)? && fs::read_dir(target)?.next().is_some() {
        bail!(
            "新旧数据目录同时存在，已停止迁移以免覆盖。请备份并确认保留哪份数据：{}",
            target.display()
        );
    }
    Ok(())
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
}

fn make_private(path: &Path, directory: bool) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            path,
            fs::Permissions::from_mode(if directory { 0o700 } else { 0o600 }),
        )?;
    }
    #[cfg(not(unix))]
    let _ = (path, directory);
    Ok(())
}

type Inventory = BTreeMap<PathBuf, Option<Vec<u8>>>;

fn inventory_digest(inventory: &Inventory) -> Vec<u8> {
    let mut hash = digest::Context::new(&digest::SHA256);
    for (path, content) in inventory {
        let name = path.as_os_str().as_encoded_bytes();
        hash.update(&(name.len() as u64).to_le_bytes());
        hash.update(name);
        hash.update(&[u8::from(content.is_some())]);
        if let Some(content) = content {
            hash.update(content);
        }
    }
    hash.finish().as_ref().to_vec()
}

fn file_digest(path: &Path, mut output: Option<&mut File>) -> Result<Vec<u8>> {
    let mut input = File::open(path)?;
    let mut hash = digest::Context::new(&digest::SHA256);
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
        if let Some(ref mut output) = output {
            output.write_all(&buffer[..n])?;
        }
    }
    Ok(hash.finish().as_ref().to_vec())
}

fn copy_tree(
    source: &Path,
    root: &Path,
    stage: &Path,
    inventory: &mut Inventory,
    check: &mut impl FnMut(Phase) -> Result<()>,
) -> Result<()> {
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?;
        let destination = stage.join(relative);
        let kind = entry.file_type()?;
        if kind.is_dir() {
            fs::create_dir(&destination)?;
            make_private(&destination, true)?;
            inventory.insert(relative.to_path_buf(), None);
            copy_tree(&path, root, stage, inventory, check)?;
        } else if kind.is_file() {
            let mut file = private_options().create_new(true).open(&destination)?;
            let hash = file_digest(&path, Some(&mut file))?;
            // BodyStore uses mtime as cache age; migrating must not make old mail look new.
            file.set_modified(fs::metadata(&path)?.modified()?)?;
            inventory.insert(relative.to_path_buf(), Some(hash));
            check(Phase::FileCopied)?;
        } else {
            bail!("旧数据含链接或特殊文件，已停止迁移：{}", path.display());
        }
    }
    Ok(())
}

fn scan_tree(directory: &Path, root: &Path, inventory: &mut Inventory) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let path = entry.path();
        let relative = path.strip_prefix(root)?.to_path_buf();
        let kind = entry.file_type()?;
        if kind.is_dir() {
            inventory.insert(relative, None);
            scan_tree(&path, root, inventory)?;
        } else if kind.is_file() {
            inventory.insert(relative, Some(file_digest(&path, None)?));
        } else {
            bail!("迁移期间旧数据出现链接或特殊文件");
        }
    }
    Ok(())
}

fn ensure_unchanged(source: &Path, copied: &Inventory) -> Result<()> {
    directory_exists(source)?;
    let mut current = BTreeMap::new();
    scan_tree(source, source, &mut current)?;
    if &current != copied {
        bail!("迁移期间旧数据发生变化，请退出旧版 MailBeacon 后重试");
    }
    Ok(())
}

fn validate_database(stage: &Path) -> Result<()> {
    let path = stage.join("local.db");
    if !path.try_exists()? {
        return Ok(());
    }
    // Recover WAL / hot rollback journal only in the copy, never mutate the source.
    let conn =
        rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)
            .context("迁移副本的数据库打不开，请恢复完整备份")?;
    let integrity: String = conn.query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if integrity != "ok" {
        bail!("数据库完整性检查失败，请恢复完整备份");
    }
    let has_credentials: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='account_credentials')", [], |r| r.get(0))?;
    if has_credentials {
        let ids = conn
            .prepare("SELECT account_id FROM account_credentials")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !ids.is_empty() {
            let credentials = crate::credentials::Store::open(stage)?;
            for id in ids {
                if credentials.load(&id)?.is_none() {
                    bail!("本地授权码校验失败");
                }
            }
        }
    }
    let busy: i64 = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
    if busy != 0 {
        bail!("迁移副本数据库仍被占用，请退出其他实例后重试");
    }
    conn.close().map_err(|(_, e)| e)?;
    Ok(())
}

fn remap_accounts(stage: &Path, source: &Path, target: &Path) -> Result<()> {
    let path = stage.join("accounts.json");
    if !path.try_exists()? {
        return Ok(());
    }
    let data = fs::read(&path)?;
    serde_json::from_slice::<Vec<crate::config::Account>>(&data)
        .context("旧账户配置无法读取，已停止迁移")?;
    // Preserve unknown fields; only rewrite structured paths, never mail bodies or signatures.
    let mut accounts: serde_json::Value = serde_json::from_slice(&data)?;
    for account in accounts.as_array_mut().context("账户配置格式错误")? {
        if let Some(download) = account.get_mut("autoDownload") {
            for key in ["directory", "lastPath"] {
                if let Some(path) = download.get_mut(key) {
                    remap_path(path, source, target);
                }
            }
        }
        if let Some(dirs) = account
            .get_mut("archiveDirs")
            .and_then(|d| d.as_array_mut())
        {
            for path in dirs {
                remap_path(path, source, target);
            }
        }
    }
    private_options()
        .truncate(true)
        .open(path)?
        .write_all(&serde_json::to_vec_pretty(&accounts)?)?;
    Ok(())
}

fn remap_path(value: &mut serde_json::Value, source: &Path, target: &Path) {
    let Some(text) = value.as_str() else {
        return;
    };
    let path = Path::new(text);
    // Ambiguous traversal paths are left as user-specified external paths.
    if path.components().any(|part| part == Component::ParentDir) {
        return;
    }
    if let Ok(relative) = path.strip_prefix(source) {
        *value = serde_json::Value::String(target.join(relative).to_string_lossy().into_owned());
    }
}

fn sync_tree(directory: &Path) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else {
            make_private(&entry.path(), false)?;
            File::open(entry.path())?.sync_all()?;
        }
    }
    sync_directory(directory)
}

fn sync_directory(directory: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(directory)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = directory;
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_legacy_stopped() -> Result<()> {
    use objc2_app_kit::NSRunningApplication;
    use objc2_foundation::NSString;
    use std::os::unix::net::UnixStream;
    let apps = NSRunningApplication::runningApplicationsWithBundleIdentifier(&NSString::from_str(
        LEGACY_ID,
    ));
    if apps.iter().any(|app| !app.isTerminated()) {
        bail!("旧版 MailBeacon 仍在运行，请从菜单退出旧版后重新打开新版");
    }
    // Also detect an unpackaged development instance. A stale socket alone is harmless.
    match UnixStream::connect("/tmp/com_lsclpangpang_mailpulse_si.sock") {
        Ok(_) => bail!("旧版 MailBeacon 仍在运行，请退出旧版后重新打开新版"),
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            Ok(())
        }
        Err(e) => Err(e).context("无法确认旧版 MailBeacon 是否已退出，已停止启动"),
    }
}

#[cfg(not(target_os = "macos"))]
fn ensure_legacy_stopped() -> Result<()> {
    // Published desktop releases currently target macOS only.
    Ok(())
}

#[cfg(test)]
#[path = "app_migration_fault_tests.rs"]
mod fault_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let old = root.path().join(LEGACY_ID);
        let new = root.path().join(CURRENT_ID);
        fs::create_dir(&old).unwrap();
        (root, old, new)
    }

    fn migrate(target: &Path) -> Result<()> {
        prepare_with(target, |_| Ok(()))
    }

    #[test]
    fn resumes_web_profile_migration_after_config_was_already_published() {
        let (root, old, new) = fixture();
        fs::write(old.join("prefs.json"), b"{}").unwrap();
        migrate(&new).unwrap();
        let web_root = root.path().join("WebKit");
        let old_web = web_root.join(LEGACY_ID);
        let new_web = web_root.join(CURRENT_ID);
        fs::create_dir_all(old_web.join("WebsiteData/Default/origin/LocalStorage")).unwrap();
        for name in [
            "WebsiteData/Default/salt",
            "WebsiteData/Default/origin/LocalStorage/localstorage.sqlite3-wal",
        ] {
            fs::write(old_web.join(name), b"profile data").unwrap();
        }
        prepare_roots(&new, Some(&new_web), |_| Ok(())).unwrap();
        assert_eq!(
            fs::read(new_web.join("WebsiteData/Default/salt")).unwrap(),
            b"profile data"
        );
        assert_eq!(
            fs::read(
                new_web.join("WebsiteData/Default/origin/LocalStorage/localstorage.sqlite3-wal")
            )
            .unwrap(),
            b"profile data"
        );
        fs::write(
            old_web.join("WebsiteData/Default/salt"),
            b"older copy changed",
        )
        .unwrap();
        prepare_roots(&new, Some(&new_web), |_| Ok(())).unwrap();
        assert_eq!(
            fs::read(new_web.join("WebsiteData/Default/salt")).unwrap(),
            b"profile data"
        );
    }

    #[test]
    fn partial_multi_directory_migration_rejects_later_legacy_edits() {
        let (root, old, new) = fixture();
        fs::write(old.join("draft.eml"), b"original draft").unwrap();
        migrate(&new).unwrap();
        fs::write(old.join("draft.eml"), b"new draft in old app").unwrap();
        let new_web = root.path().join("WebKit").join(CURRENT_ID);
        let result = prepare_roots(&new, Some(&new_web), |_| Ok(()));
        assert!(
            result.is_err(),
            "must not silently use an outdated already-published config copy"
        );
        assert_eq!(
            fs::read(old.join("draft.eml")).unwrap(),
            b"new draft in old app"
        );
        assert_eq!(fs::read(new.join("draft.eml")).unwrap(), b"original draft");
    }

    #[test]
    fn web_only_migration_finishes_and_no_longer_depends_on_legacy_data() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join("Application Support").join(CURRENT_ID);
        let old_web = root.path().join("WebKit").join(LEGACY_ID);
        let new_web = root.path().join("WebKit").join(CURRENT_ID);
        fs::create_dir_all(&old_web).unwrap();
        fs::write(old_web.join("preferences"), b"old preferences").unwrap();
        prepare_roots(&new, Some(&new_web), |_| Ok(())).unwrap();
        let _store = crate::config::Store::load(new.clone()).unwrap();
        let _credentials = crate::credentials::Store::open(&new).unwrap();
        fs::write(new_web.join("preferences"), b"new preferences").unwrap();
        fs::remove_dir_all(&old_web).unwrap();
        prepare_roots(&new, Some(&new_web), |_| Ok(())).unwrap();
        assert_eq!(
            fs::read(new_web.join("preferences")).unwrap(),
            b"new preferences"
        );
    }

    fn account_fixture(old: &Path, external: &Path) -> serde_json::Value {
        serde_json::json!([{
            "id": "test-account", "kind": "custom", "email": "test@example.invalid",
            "name": "Migration Test", "imapHost": "imap.example.invalid", "imapPort": 993,
            "smtpHost": "smtp.example.invalid", "smtpPort": 465,
            "autoDownload": {"directory": old.join("downloads"), "lastPath": old.join("downloads/mail.eml")},
            "archiveDirs": [old.join("archives"), external],
            "futureSetting": {"preserve": true}
        }])
    }

    #[test]
    fn fresh_install_does_not_claim_a_migration() {
        let root = tempfile::tempdir().unwrap();
        let new = root.path().join(CURRENT_ID);
        migrate(&new).unwrap();
        assert!(!new.join(MARKER).exists());
    }

    #[test]
    fn copies_credentials_and_mail_once_and_only_remaps_internal_setting_paths() {
        let (_root, old, new) = fixture();
        let external = old.parent().unwrap().join("user-downloads");
        let original = serde_json::to_vec(&account_fixture(&old, &external)).unwrap();
        fs::write(old.join("accounts.json"), &original).unwrap();
        for name in [
            "drafts/a/1.eml",
            "outbox/a/2.eml",
            "outbox/a/2.json",
            "bodies/a/mail",
            "future/data",
        ] {
            fs::create_dir_all(old.join(name).parent().unwrap()).unwrap();
            fs::write(old.join(name), b"unchanged mail content").unwrap();
        }
        let creds = crate::credentials::Store::open(&old).unwrap();
        creds.save("test-account", "test-only-secret").unwrap();
        // Orphan credentials must be validated and preserved too.
        creds.save("orphan-account", "test-only-orphan").unwrap();
        drop(creds);
        let key = fs::read(old.join("secrets/credentials.key")).unwrap();
        let db = fs::read(old.join("local.db")).unwrap();
        migrate(&new).unwrap();
        assert_eq!(fs::read(old.join("accounts.json")).unwrap(), original);
        assert_eq!(fs::read(old.join("local.db")).unwrap(), db);
        assert_eq!(fs::read(new.join("secrets/credentials.key")).unwrap(), key);
        assert_eq!(
            crate::credentials::Store::open(&new)
                .unwrap()
                .load("test-account")
                .unwrap()
                .as_deref(),
            Some("test-only-secret")
        );
        let settings: serde_json::Value =
            serde_json::from_slice(&fs::read(new.join("accounts.json")).unwrap()).unwrap();
        assert_eq!(
            settings[0]["autoDownload"]["directory"],
            new.join("downloads").to_str().unwrap()
        );
        assert_eq!(
            settings[0]["autoDownload"]["lastPath"],
            new.join("downloads/mail.eml").to_str().unwrap()
        );
        assert_eq!(
            settings[0]["archiveDirs"][0],
            new.join("archives").to_str().unwrap()
        );
        assert_eq!(settings[0]["archiveDirs"][1], external.to_str().unwrap());
        assert_eq!(settings[0]["futureSetting"]["preserve"], true);
        assert_eq!(
            fs::read(new.join("outbox/a/2.json")).unwrap(),
            b"unchanged mail content"
        );
        assert_eq!(
            fs::read(new.join("future/data")).unwrap(),
            b"unchanged mail content"
        );
        fs::write(old.join("future/data"), b"old app changed").unwrap();
        migrate(&new).unwrap();
        assert_eq!(
            fs::read(new.join("future/data")).unwrap(),
            b"unchanged mail content"
        );
    }

    #[test]
    fn refuses_conflicting_target_but_can_use_an_empty_directory() {
        let (_root, old, new) = fixture();
        fs::write(old.join("prefs.json"), b"{}").unwrap();
        fs::create_dir(&new).unwrap();
        fs::write(new.join("keep"), b"new data").unwrap();
        assert!(migrate(&new).is_err());
        assert_eq!(fs::read(new.join("keep")).unwrap(), b"new data");
        fs::remove_file(new.join("keep")).unwrap();
        migrate(&new).unwrap();
        assert!(new.join("prefs.json").exists());
    }

    #[test]
    fn rejects_corrupt_database_config_and_missing_or_wrong_key() {
        for broken in [
            "database",
            "accounts",
            "missing-key",
            "short-key",
            "wrong-key",
        ] {
            let (_root, old, new) = fixture();
            let creds = crate::credentials::Store::open(&old).unwrap();
            creds.save("orphan", "test-only-secret").unwrap();
            drop(creds);
            match broken {
                "database" => fs::write(old.join("local.db"), b"broken").unwrap(),
                "accounts" => fs::write(old.join("accounts.json"), b"{").unwrap(),
                "missing-key" => fs::remove_file(old.join("secrets/credentials.key")).unwrap(),
                "short-key" => fs::write(old.join("secrets/credentials.key"), b"short").unwrap(),
                _ => fs::write(old.join("secrets/credentials.key"), [0; 32]).unwrap(),
            }
            let source = fs::read(old.join("local.db")).unwrap();
            assert!(migrate(&new).is_err(), "{broken}");
            assert!(!new.exists(), "{broken}");
            assert_eq!(fs::read(old.join("local.db")).unwrap(), source);
        }
    }

    #[test]
    fn copies_committed_wal_and_supports_pre_local_credentials_database() {
        let (_root, old, new) = fixture();
        let conn = rusqlite::Connection::open(old.join("local.db")).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE example(value TEXT); INSERT INTO example VALUES('committed in WAL');").unwrap();
        assert!(old.join("local.db-wal").metadata().unwrap().len() > 0);
        // Keep the connection open only to retain a crash-like WAL fixture; it performs no writes.
        migrate(&new).unwrap();
        let migrated = rusqlite::Connection::open(new.join("local.db")).unwrap();
        let value: String = migrated
            .query_row("SELECT value FROM example", [], |r| r.get(0))
            .unwrap();
        assert_eq!(value, "committed in WAL");
        assert!(!new.join("secrets/credentials.key").exists());
    }

    #[test]
    fn interruption_or_source_change_does_not_publish_and_retry_succeeds() {
        for phase in [
            Phase::BeforeCopy,
            Phase::FileCopied,
            Phase::Copied,
            Phase::Validated,
            Phase::Publishing,
        ] {
            let (_root, old, new) = fixture();
            fs::write(old.join("data"), b"original").unwrap();
            assert!(
                prepare_with(&new, |at| {
                    if at == phase {
                        anyhow::bail!("injected interruption")
                    }
                    Ok(())
                })
                .is_err()
            );
            assert!(!new.exists(), "{phase:?}");
            assert_eq!(fs::read(old.join("data")).unwrap(), b"original");
            migrate(&new).unwrap();
        }
        for phase in [Phase::Copied, Phase::Publishing] {
            let (_root, old, new) = fixture();
            fs::write(old.join("data"), b"original").unwrap();
            assert!(
                prepare_with(&new, |at| {
                    if at == phase {
                        fs::write(old.join("data"), b"replaced")?;
                    }
                    Ok(())
                })
                .is_err()
            );
            assert!(!new.exists());
        }
    }

    #[test]
    fn refuses_target_data_created_during_copy() {
        let (_root, old, new) = fixture();
        fs::write(old.join("data"), b"original").unwrap();
        assert!(
            prepare_with(&new, |phase| {
                if phase == Phase::Publishing {
                    fs::create_dir_all(&new)?;
                    fs::write(new.join("keep"), b"concurrent")?;
                }
                Ok(())
            })
            .is_err()
        );
        assert_eq!(fs::read(new.join("keep")).unwrap(), b"concurrent");
        assert!(!new.join(MARKER).exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_and_publishes_private_files() {
        use std::os::unix::{fs::PermissionsExt, fs::symlink};
        for link in ["source", "nested", "target"] {
            let (root, old, new) = fixture();
            let outside = root.path().join("outside");
            fs::create_dir(&outside).unwrap();
            fs::write(outside.join("keep"), b"outside").unwrap();
            fs::write(old.join("data"), b"original").unwrap();
            match link {
                "source" => {
                    fs::remove_dir_all(&old).unwrap();
                    symlink(&outside, &old).unwrap();
                }
                "nested" => symlink(&outside, old.join("link")).unwrap(),
                _ => symlink(&outside, &new).unwrap(),
            }
            assert!(migrate(&new).is_err());
            assert_eq!(fs::read(outside.join("keep")).unwrap(), b"outside");
        }
        let (_root, old, new) = fixture();
        fs::write(old.join("data"), b"private").unwrap();
        migrate(&new).unwrap();
        assert_eq!(new.metadata().unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(
            new.join("data").metadata().unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
