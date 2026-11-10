//! 规则 MOVE 前独立持久保留原文。配置账本授权消费，主 INBOX 扫描独立推进。

use super::*;
use std::{
    future::Future,
    io::{BufRead, BufReader, Read},
};

#[derive(Debug)]
struct LocalRuleMailError;

impl std::fmt::Display for LocalRuleMailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "规则邮件原文尚未完成本地保留")
    }
}
impl std::error::Error for LocalRuleMailError {}

/// 本地磁盘/筛选错误不代表 IMAP 连接断开；调用者保留源邮件并稍后重试规则。
pub(crate) fn rule_mail_local_error(error: &anyhow::Error) -> bool {
    error.is::<LocalRuleMailError>()
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Identity {
    version: u32,
    account: String,
    epoch: u64,
    generation: u64,
    validity: u32,
    uid: u32,
}

impl Identity {
    fn new(account: &Account, validity: u32, uid: u32) -> Self {
        Self {
            version: 1,
            account: account.id.clone(),
            epoch: account.receive_epoch,
            generation: account.auto_download.rule_generation,
            validity,
            uid,
        }
    }

    fn path(&self, config_dir: &Path) -> PathBuf {
        config_dir.join("rule-downloads").join(format!(
            "{}-e{}-g{}-v{}-u{}.mail",
            URL_SAFE_NO_PAD.encode(self.account.as_bytes()),
            self.epoch,
            self.generation,
            self.validity,
            self.uid
        ))
    }
}

fn same_connection(current: &Account, connection: &Account) -> bool {
    current.id == connection.id
        && current.receive_epoch == connection.receive_epoch
        && current.connection_revision == connection.connection_revision
}

fn admission(
    store: &Mutex<Store>,
    connection: &Account,
    validity: u32,
    uid: u32,
) -> Option<Identity> {
    let guard = store.lock().unwrap();
    let current = guard.get(&connection.id)?;
    let settings = &current.auto_download;
    (same_connection(current, connection)
        && settings.enabled
        && uid != 0
        && settings
            .checkpoint
            .is_some_and(|cp| cp.uid_validity == validity && uid > cp.last_uid)
        && settings.rule_pending.get(&uid) != Some(&true))
    .then(|| Identity::new(current, validity, uid))
}

/// 在仍 SELECT INBOX、执行规则 MOVE 之前调用；只要保留失败，调用者就必须保留源邮件。
pub(crate) async fn preserve_rule_mail(
    app: &AppHandle,
    connection: &Account,
    s: &mut Imap,
    validity: u32,
    uids: &[u32],
) -> Result<()> {
    preserve_messages(
        s,
        uids,
        |uid| admission(&app.state::<AppState>().store, connection, validity, uid),
        |identity, raw| {
            let app = app.clone();
            let connection = connection.clone();
            async move {
                let id = identity.account.clone();
                let gate = app
                    .state::<AppState>()
                    .download_gate(&connection.id)
                    .lock_owned()
                    .await;
                let handle = app.clone();
                let staged = tokio::task::spawn_blocking(move || {
                    let _gate = gate;
                    register(
                        &handle.state::<AppState>().store,
                        &connection,
                        &identity,
                        &raw,
                    )
                })
                .await
                .context("规则邮件原文保留任务中断")
                .and_then(|result| result)
                .context(LocalRuleMailError)?;
                if staged {
                    let _ = app.state::<AppState>().download_wake.send(id);
                }
                Ok(())
            }
        },
    )
    .await
}

async fn preserve_messages<T, A, W, F>(
    s: &mut async_imap::Session<T>,
    uids: &[u32],
    mut admitted: A,
    mut write: W,
) -> Result<()>
where
    T: AsyncRead + AsyncWrite + Unpin + std::fmt::Debug + Send,
    A: FnMut(u32) -> Option<Identity>,
    W: FnMut(Identity, Vec<u8>) -> F,
    F: Future<Output = Result<()>>,
{
    for &uid in uids {
        let Some(identity) = admitted(uid) else {
            continue;
        };
        // checked_raw_message only returns None after a completed exact UID SEARCH
        // confirms disappearance. Protocol/transport failures must block the MOVE.
        if let Some(raw) = raw_message(s, uid).await? {
            write(identity, raw).await?;
        }
    }
    Ok(())
}

fn can_register(account: &Account, connection: &Account, identity: &Identity) -> bool {
    let settings = &account.auto_download;
    same_connection(account, connection)
        && settings.enabled
        && Identity::new(account, identity.validity, identity.uid) == *identity
        && settings
            .checkpoint
            .is_some_and(|cp| cp.uid_validity == identity.validity && identity.uid > cp.last_uid)
        && settings.rule_pending.get(&identity.uid) != Some(&true)
}

/// Caller holds the account's download gate. File IO never holds the global
/// Store mutex: an archive on a slow disk must not stall another account.
fn register(
    store: &Mutex<Store>,
    connection: &Account,
    identity: &Identity,
    raw: &[u8],
) -> Result<bool> {
    let (config_dir, settings) = {
        let guard = store.lock().unwrap();
        let Some(account) = guard.get(&connection.id) else {
            return Ok(false);
        };
        if !can_register(account, connection, identity) {
            return Ok(false);
        }
        (guard.dir(), account.auto_download.clone())
    };
    if !settings.should_save(raw)? {
        return Ok(false);
    }
    publish(&config_dir, identity, raw)?;
    // An unregistered file is an orphan: it may be reused by this exact source
    // UID, but the consumer must never save it until this configuration commits.
    let mut guard = store.lock().unwrap();
    let Some(account) = guard.get(&connection.id) else {
        return Ok(false);
    };
    if !can_register(account, connection, identity) {
        return Ok(false);
    }
    let mut settings = account.auto_download.clone();
    settings.rule_pending.insert(identity.uid, false);
    guard.set_auto_download(&identity.account, settings)?;
    Ok(true)
}

fn read(config_dir: &Path, identity: &Identity) -> Result<Option<Vec<u8>>> {
    if !existing_plain_directory(&config_dir.join("rule-downloads"))? {
        return Ok(None);
    }
    let path = identity.path(config_dir);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("无法读取规则邮件保留队列"),
    };
    if !metadata.is_file() || crate::private_fs::is_link(&metadata) {
        bail!("规则邮件保留队列不是普通文件");
    }
    let bytes = fs::read(path).context("无法读取规则邮件保留原文")?;
    let separator = bytes
        .iter()
        .position(|byte| *byte == b'\n')
        .ok_or_else(|| anyhow!("规则邮件保留队列元数据缺失"))?;
    let actual: Identity =
        serde_json::from_slice(&bytes[..separator]).context("规则邮件保留队列元数据损坏")?;
    if actual != *identity {
        bail!("规则邮件保留队列身份不一致，已保留现有文件");
    }
    Ok(Some(bytes[separator + 1..].to_vec()))
}

fn publish(config_dir: &Path, identity: &Identity, raw: &[u8]) -> Result<()> {
    let directory = index_directory(config_dir, "rule-downloads")?;
    crate::private_fs::create_private_dir(&directory)?;
    let path = identity.path(config_dir);
    if let Some(existing) = read(config_dir, identity)? {
        if existing != raw {
            bail!("相同规则邮件的保留原文不一致，已保留原文件");
        }
        crate::private_fs::protect_file(&path)?;
        fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)?
            .sync_all()?;
        return sync_directory(&directory);
    }
    let mut contents = serde_json::to_vec(identity)?;
    contents.reserve(1 + raw.len());
    contents.push(b'\n');
    contents.extend_from_slice(raw);
    match crate::durable_fs::atomic_create(&path, &contents) {
        Ok(_) => {}
        Err(error)
            if error.chain().any(|source| {
                source
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::AlreadyExists)
            }) =>
        {
            if read(config_dir, identity)?.as_deref() != Some(raw) {
                bail!("规则邮件保留队列内容冲突，已保留现有文件");
            }
        }
        Err(error) => return Err(error).context("保存规则邮件原文失败"),
    }
    sync_directory(&directory)
}

fn remove(config_dir: &Path, identity: &Identity) -> Result<()> {
    if read(config_dir, identity)?.is_some() {
        fs::remove_file(identity.path(config_dir)).context("清理已保存的规则邮件原文失败")?;
        sync_directory(&config_dir.join("rule-downloads"))?;
    }
    Ok(())
}

/// Only this account's well-formed, deterministic spool files are candidates.
/// Registration and cleanup hold the same download gate, so a newly published
/// file cannot be mistaken for an orphan before its ledger write completes.
fn cleanup(config_dir: &Path, id: &str, current: Option<&Account>) -> Result<()> {
    let directory = config_dir.join("rule-downloads");
    if !existing_plain_directory(&directory)? {
        return Ok(());
    }
    let prefix = format!("{}-e", URL_SAFE_NO_PAD.encode(id.as_bytes()));
    let mut removed = false;
    for entry in fs::read_dir(&directory)? {
        let path = entry?.path();
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(&prefix) && name.ends_with(".mail"))
        {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.is_file() || crate::private_fs::is_link(&metadata) {
            continue;
        }
        let mut header = Vec::new();
        // Metadata is small and precedes the raw bytes. Never parse a malformed
        // arbitrary file as an unbounded header while cleaning old generations.
        BufReader::new(fs::File::open(&path)?)
            .take(64 * 1024)
            .read_until(b'\n', &mut header)?;
        let Ok(identity) = serde_json::from_slice::<Identity>(&header) else {
            continue;
        };
        if identity.version != 1 || identity.account != id || identity.path(config_dir) != path {
            continue;
        }
        let referenced = current.is_some_and(|account| {
            let settings = &account.auto_download;
            settings.enabled
                && account.receive_epoch == identity.epoch
                && settings.rule_generation == identity.generation
                && settings.rule_pending.contains_key(&identity.uid)
        });
        if !referenced {
            fs::remove_file(&path).context("清理过期的规则邮件保留队列失败")?;
            removed = true;
        }
    }
    if removed {
        sync_directory(&directory)?;
    }
    Ok(())
}

/// 账号已移除并持有其 download_gate 时，清理私有队列；保留用户下载目录。
pub(crate) fn cleanup_rule_mail(config_dir: &Path, id: &str) -> Result<()> {
    cleanup(config_dir, id, None)
}

#[derive(Default)]
pub(super) struct Drained {
    pub more: bool,
    revision: Option<u64>,
}

fn actionable(
    config_dir: &Path,
    identity: &Identity,
    complete: bool,
    checkpoint: Checkpoint,
) -> Result<bool> {
    Ok(!complete || checkpoint.last_uid >= identity.uid || plain_file(&identity.path(config_dir))?)
}

/// Consume only registered entries, including false entries already overtaken by
/// the INBOX scanner. This function never advances the INBOX checkpoint.
fn consume(store: &Mutex<Store>, id: &str, limit: usize) -> Result<Drained> {
    consume_with_save(store, id, limit, save_atomic)
}

fn consume_with_save(
    store: &Mutex<Store>,
    id: &str,
    limit: usize,
    mut save: impl FnMut(&Path, &str, Checkpoint, u32, &[u8]) -> Result<PathBuf>,
) -> Result<Drained> {
    let (config_dir, account) = {
        let guard = store.lock().unwrap();
        (guard.dir(), guard.get(id).cloned())
    };
    cleanup(&config_dir, id, account.as_ref())?;
    let Some(account) = account else {
        return Ok(Drained::default());
    };
    let mut settings = account.auto_download.clone();
    if !settings.enabled || settings.rule_pending.is_empty() {
        return Ok(Drained::default());
    }
    let checkpoint = settings
        .checkpoint
        .ok_or_else(|| anyhow!("规则邮件保留队列缺少收件箱身份"))?;
    let mut count = 0;
    for (uid, complete) in settings.rule_pending.clone() {
        let identity = Identity::new(&account, checkpoint.uid_validity, uid);
        if !actionable(&config_dir, &identity, complete, checkpoint)? {
            continue;
        }
        if count >= limit {
            return Ok(Drained {
                more: true,
                revision: (count > 0).then_some(settings.revision),
            });
        }
        count += 1;
        if !complete {
            let raw = read(&config_dir, &identity)?
                .ok_or_else(|| anyhow!("已登记的规则邮件保留原文缺失，请恢复本机数据后重试"))?;
            let path = if settings.should_save(&raw)? {
                Some(save(
                    Path::new(&settings.directory),
                    id,
                    checkpoint,
                    uid,
                    &raw,
                )?)
            } else {
                None
            };
            // Commit count and completion together before deleting raw. If this
            // fails, the existing archive is verified/reused on the next attempt.
            let Some(_) = mark_complete(store, &identity, path.as_deref())? else {
                return Ok(Drained::default());
            };
        }
        remove(&config_dir, &identity)?;
        let Some(current) = forget_complete(store, &identity)? else {
            return Ok(Drained::default());
        };
        settings = current;
    }
    Ok(Drained {
        more: false,
        revision: (count > 0).then_some(settings.revision),
    })
}

fn same_namespace(account: &Account, identity: &Identity) -> bool {
    account.auto_download.enabled
        && Identity::new(account, identity.validity, identity.uid) == *identity
        && account
            .auto_download
            .checkpoint
            .is_some_and(|cp| cp.uid_validity == identity.validity)
}

fn mark_complete(
    store: &Mutex<Store>,
    identity: &Identity,
    path: Option<&Path>,
) -> Result<Option<Settings>> {
    let mut guard = store.lock().unwrap();
    let Some(account) = guard.get(&identity.account) else {
        return Ok(None);
    };
    if !same_namespace(account, identity) {
        return Ok(None);
    }
    let mut settings = account.auto_download.clone();
    match settings.rule_pending.get(&identity.uid) {
        Some(false) => {}
        Some(true) => return Ok(Some(settings)),
        None => return Ok(None),
    }
    if let Some(path) = path {
        settings.saved_count = settings.saved_count.saturating_add(1);
        settings.last_path = Some(path.display().to_string());
    }
    settings.rule_pending.insert(identity.uid, true);
    guard.set_auto_download(&identity.account, settings.clone())?;
    Ok(Some(settings))
}

fn forget_complete(store: &Mutex<Store>, identity: &Identity) -> Result<Option<Settings>> {
    let mut guard = store.lock().unwrap();
    let Some(account) = guard.get(&identity.account) else {
        return Ok(None);
    };
    if !same_namespace(account, identity) {
        return Ok(None);
    }
    let mut settings = account.auto_download.clone();
    if settings.rule_pending.get(&identity.uid) == Some(&true)
        && settings
            .checkpoint
            .is_some_and(|cp| cp.last_uid >= identity.uid)
    {
        settings.rule_pending.remove(&identity.uid);
        guard.set_auto_download(&identity.account, settings.clone())?;
    }
    Ok(Some(settings))
}

pub(super) async fn consume_async(app: &AppHandle, id: &str) -> Result<Drained> {
    let gate = app.state::<AppState>().download_gate(id).lock_owned().await;
    let handle = app.clone();
    let id = id.to_string();
    let work_id = id.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _gate = gate;
        consume(&handle.state::<AppState>().store, &work_id, BATCH)
    })
    .await
    .context("规则邮件保存任务中断")??;
    if let Some(revision) = result.revision {
        emit_status(
            app,
            &id,
            revision,
            "waiting",
            "已处理规则邮件；等待新邮件".into(),
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_imap;

    const RAW: &[u8] = b"From: sender@example.com\r\nSubject: moved salary\r\n\r\noriginal\0\xff";

    fn fixture() -> (tempfile::TempDir, Mutex<Store>, Account) {
        let directory = tempfile::tempdir().unwrap();
        let archive = directory.path().join("archive");
        fs::create_dir(&archive).unwrap();
        let mut store = Store::load(directory.path().join("config")).unwrap();
        let mut account: Account = serde_json::from_str(
            r#"{"id":"rule-test","kind":"qq","email":"test@example.com","name":"test","imapHost":"imap.example.com","imapPort":993,"smtpHost":"smtp.example.com","smtpPort":465}"#,
        )
        .unwrap();
        account.auto_download = Settings {
            enabled: true,
            directory: archive.display().to_string(),
            revision: 1,
            rule_generation: 1,
            checkpoint: Some(Checkpoint {
                uid_validity: 7,
                last_uid: 10,
            }),
            ..Default::default()
        };
        store.accounts.push(account.clone());
        store.save().unwrap();
        (directory, Mutex::new(store), account)
    }

    fn settings(store: &Mutex<Store>) -> Settings {
        store.lock().unwrap().accounts[0].auto_download.clone()
    }

    fn identity(account: &Account, uid: u32) -> Identity {
        Identity::new(account, 7, uid)
    }

    fn stage(store: &Mutex<Store>, account: &Account, uid: u32, raw: &[u8]) -> Result<bool> {
        register(store, account, &identity(account, uid), raw)
    }

    fn originals(directory: &Path) -> Vec<PathBuf> {
        fs::read_dir(directory.join("archive"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|extension| extension == "eml"))
            .collect()
    }

    #[test]
    fn durable_restart_consumes_without_credentials_and_keeps_fence_until_inbox_catches_up() {
        let (directory, store, account) = fixture();
        assert!(stage(&store, &account, 11, RAW).unwrap());
        let config = directory.path().join("config");
        assert_eq!(
            read(&config, &identity(&account, 11)).unwrap().unwrap(),
            RAW
        );
        let before = settings(&store);
        assert_eq!(before.rule_pending.get(&11), Some(&false));
        assert_eq!(before.saved_count, 0);
        assert!(originals(directory.path()).is_empty());
        drop(store);

        // No credentials store or network session is involved in recovery.
        let restarted = Mutex::new(Store::load(config.clone()).unwrap());
        assert!(!consume(&restarted, &account.id, BATCH).unwrap().more);
        let saved = settings(&restarted);
        assert_eq!(saved.checkpoint, before.checkpoint);
        assert_eq!(saved.saved_count, 1);
        assert_eq!(saved.rule_pending.get(&11), Some(&true));
        assert!(read(&config, &identity(&account, 11)).unwrap().is_none());
        assert_eq!(fs::read(saved.last_path.unwrap()).unwrap(), RAW);

        assert!(
            commit(
                &restarted,
                &account.id,
                1,
                Checkpoint {
                    uid_validity: 7,
                    last_uid: 11,
                },
                Some((11, RAW))
            )
            .unwrap()
        );
        assert_eq!(settings(&restarted).saved_count, 1);
        assert_eq!(settings(&restarted).rule_pending.get(&11), Some(&true));
        consume(&restarted, &account.id, BATCH).unwrap();
        assert!(settings(&restarted).rule_pending.is_empty());
        assert_eq!(originals(directory.path()).len(), 1);
    }

    #[test]
    fn normal_commit_leaves_completion_until_spool_is_removed() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 11,
        };
        commit(&store, &account.id, 1, cp, Some((11, RAW))).unwrap();
        assert_eq!(settings(&store).rule_pending.get(&11), Some(&true));
        assert!(
            read(&directory.path().join("config"), &identity(&account, 11))
                .unwrap()
                .is_some()
        );
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 1);
        assert!(settings(&store).rule_pending.is_empty());
        assert_eq!(originals(directory.path()).len(), 1);
    }

    #[test]
    fn moved_pending_raw_is_saved_even_when_scanner_crossed_its_uid() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        let cp = Checkpoint {
            uid_validity: 7,
            last_uid: 25,
        };
        commit(&store, &account.id, 1, cp, None).unwrap();
        assert_eq!(settings(&store).rule_pending.get(&11), Some(&false));
        consume(&store, &account.id, BATCH).unwrap();
        let saved = settings(&store);
        assert_eq!(saved.saved_count, 1);
        assert_eq!(saved.checkpoint, Some(cp));
        assert!(saved.rule_pending.is_empty());
        assert_eq!(originals(directory.path()).len(), 1);
    }

    #[test]
    fn unregistered_orphan_is_not_consumed_and_failed_registration_is_retryable() {
        let (directory, store, account) = fixture();
        let blocked = directory.path().join("config/accounts.json.tmp");
        fs::create_dir(&blocked).unwrap();
        assert!(stage(&store, &account, 11, RAW).is_err());
        assert!(settings(&store).rule_pending.is_empty());
        assert!(
            read(&directory.path().join("config"), &identity(&account, 11))
                .unwrap()
                .is_some()
        );
        fs::remove_dir(blocked).unwrap();
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 0);
        assert!(originals(directory.path()).is_empty());
        assert!(stage(&store, &account, 11, RAW).unwrap());
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 1);
    }

    #[test]
    fn failed_completion_write_retains_raw_and_retries_existing_archive_once() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        let config = directory.path().join("config");
        let blocked = config.join("accounts.json.tmp");
        fs::create_dir(&blocked).unwrap();
        assert!(consume(&store, &account.id, BATCH).is_err());
        assert_eq!(settings(&store).saved_count, 0);
        assert_eq!(settings(&store).rule_pending.get(&11), Some(&false));
        assert!(read(&config, &identity(&account, 11)).unwrap().is_some());
        assert_eq!(originals(directory.path()).len(), 1);
        fs::remove_dir(blocked).unwrap();
        drop(store);
        let restarted = Mutex::new(Store::load(config).unwrap());
        consume(&restarted, &account.id, BATCH).unwrap();
        consume(&restarted, &account.id, BATCH).unwrap();
        assert_eq!(settings(&restarted).saved_count, 1);
        assert_eq!(originals(directory.path()).len(), 1);
    }

    #[test]
    fn archive_failure_keeps_pending_raw_for_later_local_retry() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        let archive = directory.path().join("archive");
        fs::remove_dir(&archive).unwrap();
        fs::write(&archive, b"unavailable destination").unwrap();
        assert!(consume(&store, &account.id, BATCH).is_err());
        assert_eq!(settings(&store).rule_pending.get(&11), Some(&false));
        assert_eq!(settings(&store).saved_count, 0);
        assert!(
            read(&directory.path().join("config"), &identity(&account, 11))
                .unwrap()
                .is_some()
        );
        fs::remove_file(&archive).unwrap();
        fs::create_dir(archive).unwrap();
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 1);
    }

    #[test]
    fn repeated_uid_reuses_identical_spool_and_rejects_conflicting_raw() {
        let (directory, store, account) = fixture();
        assert!(stage(&store, &account, 11, RAW).unwrap());
        assert!(stage(&store, &account, 11, RAW).unwrap());
        assert!(stage(&store, &account, 11, b"conflicting original").is_err());
        assert_eq!(settings(&store).rule_pending.len(), 1);
        assert_eq!(
            read(&directory.path().join("config"), &identity(&account, 11))
                .unwrap()
                .unwrap(),
            RAW
        );
        consume(&store, &account.id, BATCH).unwrap();
        assert!(!stage(&store, &account, 11, RAW).unwrap());
        assert_eq!(settings(&store).saved_count, 1);
    }

    #[test]
    fn inactive_draft_and_credential_revision_keep_accepted_spool_consumable() {
        let (_directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        {
            let mut guard = store.lock().unwrap();
            let current = &mut guard.accounts[0];
            current.connection_revision += 1;
            current.auto_download.revision += 1;
            // None is the active filter: this draft has no effect on admission.
            current.auto_download.field_keywords.body = vec!["unused draft".into()];
            assert_eq!(
                current.auto_download.rule_generation,
                account.auto_download.rule_generation
            );
            guard.save().unwrap();
        }
        assert!(admission(&store, &account, 7, 12).is_none());
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 1);
        assert_eq!(settings(&store).checkpoint.unwrap().last_uid, 10);
    }

    #[test]
    fn new_generation_or_epoch_rejects_old_raw_and_cleans_only_its_obsolete_spool() {
        for change_epoch in [false, true] {
            let (directory, store, account) = fixture();
            let old_identity = admission(&store, &account, 7, 11).unwrap();
            stage(&store, &account, 11, RAW).unwrap();
            let config = directory.path().join("config");
            let unrelated = config.join("rule-downloads/unrelated.mail");
            fs::write(&unrelated, b"not a task").unwrap();
            {
                let mut guard = store.lock().unwrap();
                let current = &mut guard.accounts[0];
                if change_epoch {
                    current.receive_epoch += 1;
                } else {
                    current.auto_download.rule_generation += 1;
                }
                current.auto_download.rule_pending.clear();
                guard.save().unwrap();
            }
            assert!(!register(&store, &account, &old_identity, RAW).unwrap());
            consume(&store, &account.id, BATCH).unwrap();
            assert!(!old_identity.path(&config).exists());
            assert!(unrelated.exists());
            assert_eq!(settings(&store).saved_count, 0);
            assert!(originals(directory.path()).is_empty());
        }
    }

    #[test]
    fn account_removal_cleans_its_spool_and_preserves_archives_and_other_accounts() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        consume(&store, &account.id, BATCH).unwrap();
        stage(&store, &account, 12, RAW).unwrap();
        let config = directory.path().join("config");
        let mut other = account.clone();
        other.id = "other-account".into();
        publish(&config, &identity(&other, 13), RAW).unwrap();
        store.lock().unwrap().accounts.clear();
        cleanup_rule_mail(&config, &account.id).unwrap();
        assert!(!identity(&account, 12).path(&config).exists());
        assert!(identity(&other, 13).path(&config).exists());
        assert_eq!(originals(directory.path()).len(), 1);
    }

    #[test]
    fn missing_checkpoint_does_not_delete_referenced_preserved_original() {
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        store.lock().unwrap().accounts[0].auto_download.checkpoint = None;
        assert!(consume(&store, &account.id, BATCH).is_err());
        assert!(
            identity(&account, 11)
                .path(&directory.path().join("config"))
                .exists()
        );
        assert_eq!(settings(&store).rule_pending.get(&11), Some(&false));
    }

    #[test]
    fn disabled_old_uid_and_wrong_validity_do_not_admit_rule_download() {
        let (_directory, store, account) = fixture();
        assert!(admission(&store, &account, 7, 10).is_none());
        assert!(admission(&store, &account, 9, 11).is_none());
        store.lock().unwrap().accounts[0].auto_download.enabled = false;
        assert!(admission(&store, &account, 7, 11).is_none());
        assert!(!stage(&store, &account, 11, RAW).unwrap());
    }

    #[test]
    fn filter_rejection_does_not_spool_and_parse_error_is_local_failure() {
        let (directory, store, account) = fixture();
        {
            let mut guard = store.lock().unwrap();
            let settings = &mut guard.accounts[0].auto_download;
            settings.filter_mode = FilterMode::Whitelist;
            settings.keywords = vec!["wanted".into()];
        }
        assert!(!stage(&store, &account, 11, RAW).unwrap());
        assert!(!directory.path().join("config/rule-downloads").exists());
        let malformed = b"Subject: wanted\r\nContent-Type: text/plain\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%%";
        let error = stage(&store, &account, 11, malformed)
            .context(LocalRuleMailError)
            .unwrap_err();
        assert!(rule_mail_local_error(&error));
        assert!(settings(&store).rule_pending.is_empty());
    }

    #[test]
    fn bounded_consumer_skips_retained_completion_fences_and_eventually_drains() {
        let (_directory, store, account) = fixture();
        for uid in 11..16 {
            stage(&store, &account, uid, RAW).unwrap();
        }
        assert!(consume(&store, &account.id, 2).unwrap().more);
        assert_eq!(settings(&store).saved_count, 2);
        assert!(consume(&store, &account.id, 2).unwrap().more);
        assert_eq!(settings(&store).saved_count, 4);
        assert!(!consume(&store, &account.id, 2).unwrap().more);
        assert_eq!(settings(&store).saved_count, 5);
        assert!(!consume(&store, &account.id, 2).unwrap().more);
        assert_eq!(settings(&store).saved_count, 5);
        assert_eq!(settings(&store).rule_pending.len(), 5);
    }

    #[test]
    fn slow_archive_file_lock_does_not_block_configuration_for_other_accounts() {
        let (directory, store, account) = fixture();
        let store = std::sync::Arc::new(store);
        stage(&store, &account, 11, RAW).unwrap();
        let index = index_directory(&directory.path().join("archive"), ".mailpulse-index").unwrap();
        let lock = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(index.join(".lock"))
            .unwrap();
        lock.lock().unwrap();
        let (saving, ready) = std::sync::mpsc::channel();
        let work_store = store.clone();
        let worker = std::thread::spawn(move || {
            consume_with_save(
                &work_store,
                &account.id,
                BATCH,
                |directory, id, checkpoint, uid, raw| {
                    saving.send(()).unwrap();
                    save_atomic(directory, id, checkpoint, uid, raw)
                },
            )
        });
        ready.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(
            !worker.is_finished(),
            "archive lock must block the consumer"
        );
        let configuration_available = store.try_lock().is_ok();
        lock.unlock().unwrap();
        worker.join().unwrap().unwrap();
        assert!(
            configuration_available,
            "one account waiting for an archive file lock must not hold the global Store mutex"
        );
    }

    #[cfg(unix)]
    #[test]
    fn spool_is_private_and_symlink_redirection_is_rejected() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (directory, store, account) = fixture();
        stage(&store, &account, 11, RAW).unwrap();
        let config = directory.path().join("config");
        let spool = identity(&account, 11).path(&config);
        assert_eq!(
            fs::metadata(&spool).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(config.join("rule-downloads"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        fs::remove_file(&spool).unwrap();
        let unrelated = directory.path().join("unrelated");
        fs::write(&unrelated, RAW).unwrap();
        symlink(&unrelated, &spool).unwrap();
        assert!(stage(&store, &account, 11, RAW).is_err());
        assert!(consume(&store, &account.id, BATCH).is_err());
        assert_eq!(fs::read(unrelated).unwrap(), RAW);
    }

    async fn selected_server() -> (
        test_imap::Shared,
        async_imap::Session<tokio::io::DuplexStream>,
    ) {
        let server = test_imap::state("IMAP4rev1 MOVE UIDPLUS", &[("INBOX", 7), ("special", 91)]);
        {
            let mut guard = server.lock().unwrap();
            for _ in 0..10 {
                guard.add("INBOX", "Subject: historical\r\n\r\nold", &[]);
            }
            guard.add("INBOX", "Subject: moved salary\r\n\r\noriginal", &[]);
            for _ in 0..100 {
                guard.add("special", "Subject: historical target\r\n\r\nold", &[]);
            }
        }
        let mut session = test_imap::session(server.clone()).await;
        session.select("INBOX").await.unwrap();
        (server, session)
    }

    #[tokio::test]
    async fn pre_move_body_peek_preserves_source_uid_even_when_target_uid_differs() {
        let (directory, store, account) = fixture();
        let (server, mut session) = selected_server().await;
        preserve_messages(
            &mut session,
            &[11],
            |uid| admission(&store, &account, 7, uid),
            |identity, raw| {
                let store = &store;
                let account = &account;
                async move {
                    register(store, account, &identity, &raw)
                        .context(LocalRuleMailError)
                        .map(|_| ())
                }
            },
        )
        .await
        .unwrap();
        download_protocol::checked_command(&mut session, "UID MOVE 11 \"special\"")
            .await
            .unwrap();
        {
            let mut guard = server.lock().unwrap();
            assert!(
                guard
                    .mailbox("INBOX")
                    .messages
                    .iter()
                    .all(|mail| mail.uid != 11)
            );
            assert_eq!(guard.mailbox("special").messages.last().unwrap().uid, 101);
            assert!(
                guard
                    .commands
                    .iter()
                    .any(|command| command.contains("BODY.PEEK[]")),
                "preservation must not mark a new message read"
            );
            assert!(
                guard
                    .commands
                    .iter()
                    .all(|command| !command.starts_with("EXAMINE "))
            );
        }
        consume(&store, &account.id, BATCH).unwrap();
        assert_eq!(settings(&store).saved_count, 1);
        assert_eq!(
            fs::read(&originals(directory.path())[0]).unwrap(),
            b"Subject: moved salary\r\n\r\noriginal"
        );
        assert!(
            find_archived(&directory.path().join("archive"), &account.id, 7, 11)
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn failed_body_fetch_never_reaches_move_or_registers_partial_raw() {
        for disconnect in [false, true] {
            let (_directory, store, account) = fixture();
            let (server, mut session) = selected_server().await;
            {
                let mut guard = server.lock().unwrap();
                if disconnect {
                    guard.disconnect.push("UID FETCH 11 ".into());
                } else {
                    guard.fail.push("UID FETCH 11 ".into());
                }
            }
            let result = preserve_messages(
                &mut session,
                &[11],
                |uid| admission(&store, &account, 7, uid),
                |identity, raw| {
                    let store = &store;
                    let account = &account;
                    async move {
                        register(store, account, &identity, &raw)
                            .context(LocalRuleMailError)
                            .map(|_| ())
                    }
                },
            )
            .await;
            if result.is_ok() {
                download_protocol::checked_command(&mut session, "UID MOVE 11 \"special\"")
                    .await
                    .unwrap();
            }
            let error = result.unwrap_err();
            assert!(!rule_mail_local_error(&error));
            assert!(settings(&store).rule_pending.is_empty());
            let mut guard = server.lock().unwrap();
            assert!(
                guard
                    .mailbox("INBOX")
                    .messages
                    .iter()
                    .any(|mail| mail.uid == 11)
            );
            assert!(
                guard
                    .commands
                    .iter()
                    .all(|command| !command.starts_with("UID MOVE "))
            );
        }
    }

    #[tokio::test]
    async fn local_stage_io_failure_blocks_move_and_keeps_connection_usable() {
        let (directory, store, account) = fixture();
        let (server, mut session) = selected_server().await;
        fs::create_dir(directory.path().join("config/accounts.json.tmp")).unwrap();
        let error = preserve_messages(
            &mut session,
            &[11],
            |uid| admission(&store, &account, 7, uid),
            |identity, raw| {
                let store = &store;
                let account = &account;
                async move {
                    register(store, account, &identity, &raw)
                        .context(LocalRuleMailError)
                        .map(|_| ())
                }
            },
        )
        .await
        .unwrap_err();
        assert!(rule_mail_local_error(&error));
        assert!(
            download_protocol::connection_problem(&error),
            "the local marker must take precedence even when fs IO resembles a network error"
        );
        download_protocol::checked_command(&mut session, "NOOP")
            .await
            .unwrap();
        let mut guard = server.lock().unwrap();
        assert!(
            guard
                .mailbox("INBOX")
                .messages
                .iter()
                .any(|mail| mail.uid == 11)
        );
        assert!(
            guard
                .commands
                .iter()
                .all(|command| !command.starts_with("UID MOVE "))
        );
    }

    #[tokio::test]
    async fn settings_and_connection_are_rechecked_after_body_fetch() {
        for change in [
            "generation",
            "connection",
            "epoch",
            "checkpoint",
            "disabled",
        ] {
            let (_directory, store, account) = fixture();
            let (server, mut session) = selected_server().await;
            let (ready, waiting) = tokio::sync::oneshot::channel();
            let (resume, resumed) = tokio::sync::oneshot::channel();
            server.lock().unwrap().pause_reply = Some(("UID FETCH 11 ".into(), ready, resumed));
            let preserve = preserve_messages(
                &mut session,
                &[11],
                |uid| admission(&store, &account, 7, uid),
                |identity, raw| {
                    let store = &store;
                    let account = &account;
                    async move {
                        register(store, account, &identity, &raw)
                            .context(LocalRuleMailError)
                            .map(|_| ())
                    }
                },
            );
            let mutate = async {
                waiting.await.unwrap();
                {
                    let mut guard = store.lock().unwrap();
                    let current = &mut guard.accounts[0];
                    match change {
                        "generation" => current.auto_download.rule_generation += 1,
                        "connection" => current.connection_revision += 1,
                        "epoch" => current.receive_epoch += 1,
                        "checkpoint" => {
                            current.auto_download.checkpoint.as_mut().unwrap().last_uid = 11
                        }
                        "disabled" => current.auto_download.enabled = false,
                        _ => unreachable!(),
                    }
                    guard.save().unwrap();
                }
                resume.send(()).unwrap();
            };
            let (result, ()) = tokio::join!(preserve, mutate);
            result.unwrap();
            assert!(
                settings(&store).rule_pending.is_empty(),
                "stale raw admitted across {change} change"
            );
            assert_eq!(settings(&store).saved_count, 0);
        }
    }
}
