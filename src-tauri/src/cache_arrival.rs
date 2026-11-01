//! Durable queue for genuinely new INBOX mail. Historical cache warming keeps
//! its own smaller size limit; arrivals use the complete-cache 32 MiB limit.
//! The cursor and pending identities are published together, so a restart never
//! skips a discovered message merely because its body was still downloading.

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager as _, async_runtime::JoinHandle};
use tokio::{sync::Notify, time::timeout};

use crate::{
    body_store::{BodyStore, CacheOrigin, MAX_FILE},
    config::{Account, ReceiveProtocol},
    download_protocol::{ArchiveData, checked_boundary, checked_command, checked_search},
    imap::{self, Imap, OP_TIMEOUT, Summary},
    mail_ref::MessageRef,
};

const INBOX: &str = "INBOX";
const POLL: Duration = Duration::from_secs(30);
const RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Pending {
    source: MessageRef,
    date: i64,
    #[serde(default)]
    attempts: u32,
    #[serde(default)]
    retry_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Queue {
    version: u8,
    account: String,
    epoch: u64,
    validity: u32,
    /// Discovery watermark, not a download-completion watermark.
    highwater: u32,
    pending: Vec<Pending>,
}

impl Queue {
    fn matches(&self, account: &Account, validity: u32) -> bool {
        self.account == account.id
            && self.epoch == account.receive_epoch
            && self.validity == validity
    }

    fn insert(&mut self, source: MessageRef, date: i64) {
        if source.validate().is_err()
            || source.account_id != self.account
            || source.receive_epoch != self.epoch
            || source.uid_validity != self.validity
            || source.folder != INBOX
            || self.pending.iter().any(|p| p.source == source)
        {
            return;
        }
        self.pending.push(Pending {
            source,
            date,
            attempts: 0,
            retry_at: 0,
        });
    }
}

struct Worker {
    wake: Arc<Notify>,
    task: JoinHandle<()>,
}

pub(crate) struct Manager {
    dir: PathBuf,
    queues: Mutex<HashMap<String, Queue>>,
    workers: Mutex<HashMap<String, Worker>>,
}

impl Manager {
    pub fn new(dir: PathBuf) -> Arc<Self> {
        Arc::new(Self {
            dir,
            queues: Mutex::default(),
            workers: Mutex::default(),
        })
    }

    fn path(&self, account: &str) -> PathBuf {
        // A stable, path-safe key. The embedded account is checked after loading.
        self.dir
            .join(format!("{}.json", crate::body_store::folder_hash(account)))
    }

    fn load(&self, account: &str) -> Result<Option<Queue>> {
        let path = self.path(account);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        ensure!(
            metadata.is_file()
                && !crate::private_fs::is_link(&metadata)
                && metadata.len() <= 64 * 1024 * 1024,
            "新邮件缓存队列文件无效"
        );
        let queue: Queue = serde_json::from_slice(&fs::read(path)?)?;
        ensure!(
            queue.version == 1 && queue.account == account && queue.epoch > 0 && queue.validity > 0,
            "新邮件缓存队列身份无效"
        );
        ensure!(
            queue.pending.iter().all(|p| p.source.validate().is_ok()
                && p.source.account_id == account
                && p.source.receive_epoch == queue.epoch
                && p.source.uid_validity == queue.validity
                && p.source.folder == INBOX),
            "新邮件缓存队列包含过期身份"
        );
        Ok(Some(queue))
    }

    fn save(&self, queue: &Queue) -> Result<()> {
        crate::private_fs::create_private_dir(&self.dir)?;
        crate::durable_fs::atomic_write(&self.path(&queue.account), &serde_json::to_vec(queue)?)
    }

    /// Called only after the sync connection has confirmed the mailbox identity.
    /// A first installation (or a new receiving identity) starts at today's
    /// boundary: its existing first page is history, not a batch of new mail.
    pub fn observe_boundary(&self, account: &Account, validity: u32, upper: u32) -> Result<()> {
        if account.receive_protocol != ReceiveProtocol::Imap || validity == 0 {
            return Ok(());
        }
        let mut queues = self.queues.lock().unwrap();
        let previous = match queues.get(&account.id) {
            Some(queue) => Some(queue.clone()),
            None => self.load(&account.id)?,
        };
        let queue = match previous {
            Some(mut queue) if queue.matches(account, validity) => {
                if !account.body_cache.enabled {
                    queue.highwater = queue.highwater.max(upper);
                }
                queue
            }
            Some(queue) if queue.epoch > account.receive_epoch => return Ok(()),
            _ => Queue {
                version: 1,
                account: account.id.clone(),
                epoch: account.receive_epoch,
                validity,
                highwater: upper,
                pending: Vec::new(),
            },
        };
        self.save(&queue)?;
        queues.insert(account.id.clone(), queue);
        drop(queues);
        self.wake(&account.id);
        Ok(())
    }

    /// The caller supplies only verified, post-rule arrivals, never a first page.
    /// Do not advance discovery here: a reconnect may have fetched only 100
    /// summaries while more than 100 messages arrived during shutdown.
    pub fn enqueue(&self, account: &Account, messages: &[Summary]) -> Result<()> {
        let mut queues = self.queues.lock().unwrap();
        let Some(old) = queues.get(&account.id) else {
            return Ok(());
        };
        if old.epoch != account.receive_epoch {
            return Ok(());
        }
        let mut queue = old.clone();
        for message in messages {
            let Ok(source) = message.message_ref() else {
                continue;
            };
            if !account.body_cache.enabled {
                if source.receive_epoch == queue.epoch && source.uid_validity == queue.validity {
                    queue.highwater = queue.highwater.max(source.uid);
                }
            } else {
                queue.insert(source, message.date);
            }
        }
        self.save(&queue)?;
        queues.insert(account.id.clone(), queue);
        drop(queues);
        self.wake(&account.id);
        Ok(())
    }

    fn snapshot(&self, account: &Account) -> Option<Queue> {
        self.queues
            .lock()
            .unwrap()
            .get(&account.id)
            .filter(|q| q.epoch == account.receive_epoch)
            .cloned()
    }

    fn discovered(
        &self,
        account: &Account,
        validity: u32,
        upper: u32,
        messages: &[(u32, i64)],
    ) -> Result<()> {
        let mut queues = self.queues.lock().unwrap();
        let Some(old) = queues
            .get(&account.id)
            .filter(|q| q.matches(account, validity))
        else {
            anyhow::bail!("邮箱身份已经更新");
        };
        let mut queue = old.clone();
        for &(uid, date) in messages {
            queue.insert(
                MessageRef {
                    account_id: account.id.clone(),
                    receive_epoch: account.receive_epoch,
                    folder: INBOX.into(),
                    uid_validity: validity,
                    uid,
                },
                date,
            );
        }
        queue.highwater = queue.highwater.max(upper);
        self.save(&queue)?;
        queues.insert(account.id.clone(), queue);
        Ok(())
    }

    fn finished(&self, source: &MessageRef, failed: bool) -> Result<()> {
        let mut queues = self.queues.lock().unwrap();
        let Some(old) = queues.get(&source.account_id) else {
            return Ok(());
        };
        let mut queue = old.clone();
        if failed {
            if let Some(p) = queue.pending.iter_mut().find(|p| &p.source == source) {
                p.attempts = p.attempts.saturating_add(1);
                p.retry_at = chrono::Utc::now().timestamp_millis()
                    + (60_000_i64 * 2_i64.pow(p.attempts.min(5) - 1)).min(900_000);
            }
        } else {
            queue.pending.retain(|p| &p.source != source);
        }
        self.save(&queue)?;
        queues.insert(source.account_id.clone(), queue);
        Ok(())
    }

    fn wake(&self, id: &str) {
        if let Some(worker) = self.workers.lock().unwrap().get(id) {
            worker.wake.notify_one();
        }
    }

    /// Idempotent: another arrival never aborts an in-progress large download.
    pub fn start(self: &Arc<Self>, app: &AppHandle, id: &str) {
        let mut workers = self.workers.lock().unwrap();
        if let Some(worker) = workers.get(id).filter(|w| !w.task.inner().is_finished()) {
            worker.wake.notify_one();
            return;
        }
        let wake = Arc::new(Notify::new());
        let manager = self.clone();
        let (app, id, worker_wake) = (app.clone(), id.to_string(), wake.clone());
        let worker_id = id.clone();
        let task = tauri::async_runtime::spawn(async move {
            manager.run(app, worker_id, worker_wake).await;
        });
        workers.insert(id, Worker { wake, task });
    }

    pub fn stop(&self, id: &str) {
        if let Some(worker) = self.workers.lock().unwrap().remove(id) {
            worker.task.abort();
        }
    }

    async fn run(self: Arc<Self>, app: AppHandle, id: String, wake: Arc<Notify>) {
        loop {
            let Some(state) = app.try_state::<crate::AppState>() else {
                return;
            };
            let Ok(account) = state.configured_account(&id) else {
                return;
            };
            if account.receive_protocol != ReceiveProtocol::Imap {
                return;
            }
            if !account.body_cache.enabled || self.snapshot(&account).is_none() {
                wait(&wake, POLL).await;
                continue;
            }
            let result = self.run_connected(&app, &account, &wake).await;
            if let Err(error) = result {
                eprintln!("新邮件后台缓存稍后重试：{error:#}");
                // New mail cannot reset connection-error backoff and hammer a
                // server which has refused this optional background connection.
                tokio::time::sleep(RETRY).await;
            }
        }
    }

    async fn run_connected(&self, app: &AppHandle, account: &Account, wake: &Notify) -> Result<()> {
        wait_idle(app, account).await?;
        let state = app.state::<crate::AppState>();
        let (login_account, password) = state.account(&account.id)?;
        ensure!(
            login_account.receive_epoch == account.receive_epoch
                && login_account.connection_revision == account.connection_revision,
            "邮箱配置已经更新"
        );
        let Some(store) = state.body_store() else {
            anyhow::bail!("本地缓存不可用");
        };
        let mut session = imap::login(account, &password).await?;
        session.get_mut().set_download_timeout(true);
        loop {
            wait_idle(app, account).await?;
            let boundary = checked_boundary(&mut session, INBOX).await?;
            current(app, account)?;
            let queue = self.snapshot(account).context("新邮件队列已更新")?;
            ensure!(
                queue.validity == boundary.uid_validity,
                "收件箱代际已更新，等待收信连接确认"
            );
            if boundary.last_uid > queue.highwater {
                let uids =
                    checked_search(&mut session, queue.highwater + 1, boundary.last_uid).await?;
                // Persist every confirmed UID before querying per-mail metadata.
                // A server omitting SIZE or INTERNALDATE must not make discovery
                // silently jump past a message that still needs downloading.
                let arrivals: Vec<_> = uids.into_iter().map(|uid| (uid, 0)).collect();
                current(app, account)?;
                self.discovered(account, boundary.uid_validity, boundary.last_uid, &arrivals)?;
            }
            let next = self.snapshot(account).and_then(|q| {
                q.pending
                    .into_iter()
                    .find(|p| p.retry_at <= chrono::Utc::now().timestamp_millis())
            });
            let Some(pending) = next else {
                // Close the optional connection while idle, freeing its slot for
                // other devices and foreground readers. Wake does not cancel work.
                drop(session);
                wait(wake, POLL).await;
                return Ok(());
            };
            let result = download(app, account, &store, &mut session, &pending).await;
            self.finished(&pending.source, result.is_err())?;
            result?;
        }
    }
}

async fn wait(wake: &Notify, delay: Duration) {
    tokio::select! { _ = wake.notified() => {}, _ = tokio::time::sleep(delay) => {} }
}

fn current(app: &AppHandle, account: &Account) -> Result<()> {
    let state = app.state::<crate::AppState>();
    let latest = state.configured_account(&account.id)?;
    ensure!(
        latest.connection_revision == account.connection_revision
            && latest.receive_epoch == account.receive_epoch
            && latest.body_cache.enabled,
        "邮箱配置已更新或后台缓存已关闭"
    );
    Ok(())
}

fn current_source(app: &AppHandle, account: &Account, source: &MessageRef) -> Result<()> {
    current(app, account)?;
    ensure!(
        app.state::<crate::AppState>()
            .known_body_validity(&account.id, INBOX)
            .is_none_or(|validity| validity == source.uid_validity),
        "收件箱代际已更新"
    );
    Ok(())
}

async fn wait_idle(app: &AppHandle, account: &Account) -> Result<()> {
    loop {
        current(app, account)?;
        let state = app.state::<crate::AppState>();
        if state.turns.idle_for(&account.id) >= crate::cache_warm::IDLE_AFTER_OPEN
            && !state.lanes.reader_busy(&account.id)
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

async fn metadata<T: crate::lanes::Stream>(
    session: &mut async_imap::Session<T>,
    uids: &[u32],
) -> Result<Vec<(u32, u32, i64)>> {
    if uids.is_empty() {
        return Ok(Vec::new());
    }
    let responses = timeout(
        OP_TIMEOUT,
        checked_command(
            session,
            &format!(
                "UID FETCH {} (UID RFC822.SIZE INTERNALDATE)",
                imap::uid_set(uids)
            ),
        ),
    )
    .await
    .context("查询新邮件大小超时")??;
    let wanted: HashSet<_> = uids.iter().copied().collect();
    let mut found: HashMap<u32, (Option<u32>, i64)> = HashMap::new();
    for response in responses {
        if let ArchiveData::Message {
            uid: Some(uid),
            size,
            date,
            ..
        } = response
        {
            if !wanted.contains(&uid) {
                continue;
            }
            let value = found.entry(uid).or_default();
            if let Some(size) = size {
                ensure!(
                    value.0.is_none_or(|previous| previous == size),
                    "服务器返回的邮件大小冲突"
                );
                value.0 = Some(size);
            }
            if date != 0 {
                value.1 = date;
            }
        }
    }
    ensure!(
        found.values().all(|(size, _)| size.is_some()),
        "服务器未确认新邮件大小"
    );
    let mut result: Vec<_> = found
        .into_iter()
        .filter_map(|(uid, (size, date))| size.map(|size| (uid, size, date)))
        .collect();
    result.sort_unstable_by_key(|m| m.0);
    Ok(result)
}

async fn download(
    app: &AppHandle,
    account: &Account,
    store: &Arc<BodyStore>,
    session: &mut Imap,
    pending: &Pending,
) -> Result<()> {
    let source = &pending.source;
    current_source(app, account, source)?;
    let namespace = crate::mail_ref::namespace(&account.id, account.receive_epoch);
    let cached = {
        let store = store.clone();
        let ns = namespace.clone();
        let validity = source.uid_validity;
        tokio::task::spawn_blocking(move || store.cached_uids(&ns, INBOX, validity)).await?
    };
    if cached.contains(&source.uid) {
        return Ok(());
    }
    let info = metadata(session, &[source.uid]).await?;
    let Some(&(_, size, date)) = info.first() else {
        ensure!(
            checked_search(session, source.uid, source.uid)
                .await?
                .is_empty(),
            "服务器没有返回仍存在邮件的缓存信息"
        );
        return Ok(());
    };
    if size as usize > MAX_FILE || !store.background_allowed(&namespace, date) {
        return Ok(());
    }
    let ticket = store.begin_write(&namespace, CacheOrigin::Background);
    let resume = {
        let store = store.clone();
        let ns = namespace.clone();
        let source = source.clone();
        let ticket = ticket.clone();
        tokio::task::spawn_blocking(move || {
            let resume = store.load_resume(&ns, INBOX, source.uid_validity, source.uid);
            if resume
                .as_ref()
                .is_some_and(|r| r.total != size || r.raw.len() > size as usize)
            {
                let _ =
                    store.clear_resume_with_ticket(&ticket, INBOX, source.uid_validity, source.uid);
                None
            } else {
                resume
            }
        })
        .await?
    };
    wait_idle(app, account).await?;
    let fetched = crate::body_download::fetch_controlled(
        session,
        source.uid,
        true,
        resume,
        Some(MAX_FILE),
        |total, received, offset, bytes| {
            let date = if received == 0 { date } else { received };
            let (store, ticket, source) = (store.clone(), ticket.clone(), source.clone());
            async move {
                current_source(app, account, &source)?;
                let gate = app.state::<crate::AppState>().account_gate(&account.id);
                let _guard = gate.read().await;
                current_source(app, account, &source)?;
                let written = tokio::task::spawn_blocking(move || {
                    store.append_resume_with_ticket(
                        &ticket,
                        INBOX,
                        source.uid_validity,
                        source.uid,
                        total,
                        date,
                        offset,
                        &bytes,
                    )
                })
                .await??;
                ensure!(written, "缓存已清理，停止旧的后台下载");
                drop(_guard);
                wait_idle(app, account).await
            }
        },
    )
    .await;
    // Keep confirmed ranges on connection failure. A rejected offset/size must
    // not strand this identity in a permanent retry against the same bad bytes.
    if let Err(error) = &fetched {
        if !crate::download_protocol::connection_problem(error)
            && current(app, account).is_ok()
            && store.ticket_valid(&ticket, date)
        {
            let (store, ticket, source) = (store.clone(), ticket.clone(), source.clone());
            tokio::task::spawn_blocking(move || {
                store.clear_resume_with_ticket(&ticket, INBOX, source.uid_validity, source.uid)
            })
            .await??;
        }
    }
    let (raw, received) = fetched?;
    let received = if received == 0 { date } else { received };
    // Recheck the selected generation and UID after downloading, before a
    // complete cache file becomes visible. BODY.PEEK never marks mail read.
    let boundary = checked_boundary(session, INBOX).await?;
    if boundary.uid_validity != source.uid_validity
        || checked_search(session, source.uid, source.uid)
            .await?
            .is_empty()
    {
        return Ok(());
    }
    let gate = app.state::<crate::AppState>().account_gate(&account.id);
    let _guard = gate.read().await;
    current_source(app, account, source)?;
    let (store, source) = (store.clone(), source.clone());
    tokio::task::spawn_blocking(move || publish_download(&store, &ticket, &source, &raw, received))
        .await??;
    Ok(())
}

fn publish_download(
    store: &BodyStore,
    ticket: &crate::body_store::CacheWriteTicket,
    source: &MessageRef,
    raw: &[u8],
    date: i64,
) -> Result<()> {
    if store.put_with_ticket(ticket, INBOX, source.uid_validity, source.uid, raw, date)? {
        store.clear_resume_with_ticket(ticket, INBOX, source.uid_validity, source.uid)?;
        return Ok(());
    }
    // Cleanup/deletion revokes all old tickets for the namespace. A newer mail
    // outside the cleanup range must stay queued and retry with a new ticket.
    ensure!(
        !store.background_allowed(&ticket.account, date)
            || store
                .cached_uids(&ticket.account, INBOX, source.uid_validity)
                .contains(&source.uid),
        "缓存写入被并发清理打断，将保留新邮件并重试"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    #[test]
    fn cleanup_of_older_mail_does_not_drop_a_new_arrivals_final_publication() {
        let root = tempfile::tempdir().unwrap();
        let store = BodyStore::new(root.path().join("bodies"), 1_000_000);
        let source = summary(11).message_ref().unwrap();
        let ticket = store.begin_write("test", CacheOrigin::Background);
        store
            .cleanup_with("test", Some(100), 300, |_| Ok(Default::default()))
            .unwrap();
        assert!(publish_download(&store, &ticket, &source, b"new mail", 200).is_err());
        assert!(store.cached_uids("test", INBOX, 7).is_empty());
        assert!(
            publish_download(&store, &ticket, &source, b"old mail", 50).is_ok(),
            "cleaned dates may leave the queue"
        );
        let ticket = store.begin_write("test", CacheOrigin::Background);
        publish_download(&store, &ticket, &source, b"new mail", 200).unwrap();
        assert_eq!(store.get("test", INBOX, 7, 11).unwrap().0, b"new mail");
    }

    async fn scripted(
        response: &'static str,
        status: &'static str,
    ) -> async_imap::Session<tokio::io::DuplexStream> {
        let (client, server) = tokio::io::duplex(8192);
        tokio::spawn(async move {
            let mut server = BufReader::new(server);
            for (data, status) in [("", "OK login"), (response, status)] {
                let mut command = String::new();
                server.read_line(&mut command).await.unwrap();
                let tag = command.split_whitespace().next().unwrap();
                server
                    .write_all(format!("{data}{tag} {status}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        async_imap::Client::new(client)
            .login("test", "pw")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn metadata_uses_only_confirmed_matching_uid_attributes() {
        let mut session = scripted(
            concat!(
                "* 1 FETCH (UID 7 FLAGS (\\Seen))\r\n",
                "* 2 FETCH (UID 8 RFC822.SIZE 999)\r\n",
                "* 1 FETCH (UID 7 RFC822.SIZE 29884416)\r\n",
                "* 1 FETCH (UID 7 INTERNALDATE \"02-Jan-2025 03:04:05 +0000\")\r\n"
            ),
            "OK metadata",
        )
        .await;
        let result = metadata(&mut session, &[7]).await.unwrap();
        assert_eq!(result, vec![(7, 29_884_416, 1_735_787_045_000)]);
        assert!(
            result[0].1 as usize <= MAX_FILE,
            "28.5 MiB arrivals bypass the history limit"
        );
        for (response, status) in [
            ("* 1 FETCH (UID 7 FLAGS (\\Seen))\r\n", "OK incomplete"),
            ("* 1 FETCH (UID 7 RFC822.SIZE 100)\r\n", "NO refused"),
        ] {
            let mut session = scripted(response, status).await;
            assert!(metadata(&mut session, &[7]).await.is_err());
        }
    }

    #[tokio::test]
    async fn new_arrival_does_not_abort_an_existing_worker() {
        let dir = tempfile::tempdir().unwrap();
        let account = account();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 10).unwrap();
        let task = tauri::async_runtime::spawn(std::future::pending::<()>());
        manager.workers.lock().unwrap().insert(
            account.id.clone(),
            Worker {
                wake: Arc::new(Notify::new()),
                task,
            },
        );
        manager.enqueue(&account, &[summary(11)]).unwrap();
        assert!(
            !manager.workers.lock().unwrap()[&account.id]
                .task
                .inner()
                .is_finished()
        );
        manager.stop(&account.id);
    }

    fn account() -> Account {
        serde_json::from_value(serde_json::json!({"id":"test", "kind":"custom", "email":"test@example.invalid",
            "name":"Test", "imapHost":"example.invalid", "imapPort":993, "smtpHost":"example.invalid", "smtpPort":465})).unwrap()
    }

    fn summary(uid: u32) -> Summary {
        Summary {
            account_id: "test".into(),
            receive_epoch: 1,
            uid_validity: 7,
            folder: INBOX.into(),
            uid,
            date: 1234,
            ..Summary::default()
        }
    }

    #[test]
    fn first_snapshot_is_a_baseline_and_restart_preserves_unfinished_new_mail() {
        let dir = tempfile::tempdir().unwrap();
        let account = account();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 800).unwrap();
        assert!(manager.snapshot(&account).unwrap().pending.is_empty());
        manager
            .enqueue(&account, &[summary(801), summary(801)])
            .unwrap();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 950).unwrap();
        let q = manager.snapshot(&account).unwrap();
        assert_eq!(
            q.highwater, 800,
            "latest first page must not skip shutdown arrivals"
        );
        assert_eq!(q.pending.len(), 1);
        assert_eq!(q.pending[0].source.uid, 801);
    }

    #[test]
    fn discovery_persists_every_offline_arrival_and_deduplicates_the_fresh_hook() {
        let dir = tempfile::tempdir().unwrap();
        let account = account();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 10).unwrap();
        manager.enqueue(&account, &[summary(250)]).unwrap();
        let arrivals: Vec<_> = (11..=350).map(|uid| (uid, 1234)).collect();
        manager.discovered(&account, 7, 350, &arrivals).unwrap();
        let restarted = Manager::new(dir.path().to_owned());
        restarted.observe_boundary(&account, 7, 350).unwrap();
        let q = restarted.snapshot(&account).unwrap();
        assert_eq!(q.pending.len(), 340);
        assert_eq!(q.highwater, 350);
    }

    #[test]
    fn epochs_and_uidvalidity_reset_baseline_without_reusing_pending_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = account();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 10).unwrap();
        manager.enqueue(&account, &[summary(11)]).unwrap();
        manager.observe_boundary(&account, 8, 100).unwrap();
        manager.enqueue(&account, &[summary(12)]).unwrap();
        assert!(manager.snapshot(&account).unwrap().pending.is_empty());
        account.receive_epoch = 2;
        manager.observe_boundary(&account, 7, 500).unwrap();
        assert_eq!(manager.snapshot(&account).unwrap().highwater, 500);
        assert!(manager.discovered(&account, 8, 600, &[(600, 1)]).is_err());
    }

    #[test]
    fn failed_publication_does_not_advance_the_memory_watermark() {
        let dir = tempfile::tempdir().unwrap();
        let account = account();
        let manager = Manager::new(dir.path().join("queue"));
        manager.observe_boundary(&account, 7, 10).unwrap();
        fs::remove_dir_all(&manager.dir).unwrap();
        fs::write(&manager.dir, b"blocked directory").unwrap();
        assert!(manager.discovered(&account, 7, 20, &[(20, 1)]).is_err());
        assert_eq!(manager.snapshot(&account).unwrap().highwater, 10);
    }

    #[test]
    fn disabled_arrivals_do_not_build_a_surprise_history_queue() {
        let dir = tempfile::tempdir().unwrap();
        let mut account = account();
        let manager = Manager::new(dir.path().to_owned());
        account.body_cache.enabled = false;
        manager.observe_boundary(&account, 7, 10).unwrap();
        manager.enqueue(&account, &[summary(11)]).unwrap();
        account.body_cache.enabled = true;
        manager.observe_boundary(&account, 7, 11).unwrap();
        let q = manager.snapshot(&account).unwrap();
        assert_eq!(q.highwater, 11);
        assert!(q.pending.is_empty());
    }

    #[test]
    fn failures_back_off_without_removing_the_restart_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let account = account();
        let manager = Manager::new(dir.path().to_owned());
        manager.observe_boundary(&account, 7, 10).unwrap();
        manager
            .enqueue(&account, &[summary(11), summary(12)])
            .unwrap();
        let source = summary(11).message_ref().unwrap();
        manager.finished(&source, true).unwrap();
        let q = manager.snapshot(&account).unwrap();
        assert_eq!(q.pending.len(), 2);
        assert!(q.pending[0].retry_at > chrono::Utc::now().timestamp_millis());
        assert_eq!(q.pending[1].retry_at, 0);
        manager.finished(&source, false).unwrap();
        assert_eq!(manager.snapshot(&account).unwrap().pending.len(), 1);
    }
}
