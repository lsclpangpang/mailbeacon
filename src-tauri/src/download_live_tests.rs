//! Explicitly opted-in, read-only IMAP diagnostics. Ordinary test runs never
//! access a real account. Successful downloads use the normal local body cache.
//! Set MAILBEACON_LIVE_DOWNLOAD=1 and explicit non-secret HOST, UID, and SIZE
//! values in MAILBEACON_LIVE_HOST / MAILBEACON_LIVE_UID / MAILBEACON_LIVE_SIZE.
//! Choose a multipart message with a readable text part and an attachment.
//! Credentials are loaded only from the existing local credential store.

use std::{
    future::pending,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Result, anyhow, ensure};
use tokio::sync::oneshot;

use crate::{
    BodyCache, body_download, body_store, config, credentials, download_body_report,
    download_protocol, imap, mail_ref, offline, text_first_view,
};

const LIVE_BUDGET: Duration = Duration::from_secs(20 * 60);
const PROGRESS_BYTES: u64 = 4 * 1024 * 1024;

struct LiveTarget {
    host: String,
    uid: u32,
    expected_bytes: u32,
}

/// A short real-server sample for the arrival downloader. Only a temporary
/// cache is cleaned; the user's existing body cache is never opened or removed.
#[tokio::test]
#[ignore = "real IMAP read-only sample; requires explicit DOWNLOAD/HOST/UID/SIZE and user authorization"]
async fn live_imap_background_checkpoint_and_local_retention() -> Result<()> {
    ensure!(
        std::env::var("MAILBEACON_LIVE_DOWNLOAD").as_deref() == Ok("1"),
        "live diagnostic is disabled"
    );
    let target = LiveTarget::from_env()?;
    let profile = profile_dir()?;
    let accounts: Vec<config::Account> = safe(
        serde_json::from_slice(&safe(
            std::fs::read(profile.join("accounts.json")),
            "configuration read",
        )?),
        "configuration decode",
    )?;
    let mut matching = accounts.into_iter().filter(|a| {
        a.imap_host.eq_ignore_ascii_case(&target.host)
            && a.receive_protocol == config::ReceiveProtocol::Imap
    });
    let account = matching
        .next()
        .ok_or_else(|| anyhow!("configured host unavailable"))?;
    ensure!(matching.next().is_none(), "configured host is ambiguous");
    let credentials = safe(credentials::Store::open(&profile), "credential store open")?;
    let password = safe(credentials.load(&account.id), "credential load")?
        .ok_or_else(|| anyhow!("credential unavailable"))?;
    let mut session = safe(imap::login(&account, &password).await, "read-only login")?;
    session.get_mut().set_download_timeout(true);
    let validity = examine(&mut session).await?;
    let root = safe(tempfile::tempdir(), "temporary cache")?;
    let store = body_store::BodyStore::new(root.path().join("bodies"), 64 * 1024 * 1024);
    let ticket = store.begin_write("sample", body_store::CacheOrigin::Background);
    let started = Instant::now();
    let result = body_download::fetch_controlled(
        &mut session,
        target.uid,
        true,
        None,
        Some(body_store::MAX_FILE),
        |total, date, offset, bytes| {
            let saved = total == target.expected_bytes
                && store
                    .append_resume_with_ticket(
                        &ticket, "INBOX", validity, target.uid, total, date, offset, &bytes,
                    )
                    .unwrap_or(false);
            async move {
                ensure!(saved, "sample checkpoint failed");
                Err(anyhow!("sample captured"))
            }
        },
    )
    .await;
    ensure!(
        result.is_err(),
        "sample should stop after its first confirmed chunk"
    );
    let sample = store
        .load_resume("sample", "INBOX", validity, target.uid)
        .ok_or_else(|| anyhow!("confirmed sample unavailable"))?;
    ensure!(
        sample.raw.len() == body_download::CHUNK_BYTES
            && sample.total == target.expected_bytes
            && sample.date > 0,
        "invalid confirmed sample"
    );
    let preview = safe(
        store.preview_cleanup("sample", Some(sample.date + 1)),
        "retention preview",
    )?;
    ensure!(preview.count == 1, "sample should be one pending mail");
    let report = safe(
        store.cleanup_with(
            "sample",
            Some(sample.date + 1),
            chrono::Utc::now().timestamp_millis(),
            |_| Ok(Default::default()),
        ),
        "temporary retention cleanup",
    )?;
    ensure!(
        report.removed_count == 1 && !store.background_allowed("sample", sample.date),
        "old background cache must stay cleared"
    );
    let explicit = store.begin_write("sample", body_store::CacheOrigin::Interactive);
    ensure!(
        safe(
            store.append_resume_with_ticket(
                &explicit,
                "INBOX",
                validity,
                target.uid,
                sample.total,
                sample.date,
                0,
                &sample.raw
            ),
            "explicit sample re-download"
        )?,
        "explicit download must remain possible"
    );
    ensure!(
        store.usage("sample").0 == 0,
        "a checkpoint must never be a complete body cache"
    );
    eprintln!(
        "live background_sample_bytes={} sample_seconds={:.3} retention_preview={} temporary_removed={} explicit_resume_allowed=true",
        sample.raw.len(),
        started.elapsed().as_secs_f64(),
        preview.count,
        report.removed_count
    );
    Ok(())
}

impl LiveTarget {
    fn from_env() -> Result<Self> {
        fn required(name: &'static str) -> Result<String> {
            let value = std::env::var(name).map_err(|_| {
                anyhow!("set {name} explicitly before enabling the live diagnostic")
            })?;
            ensure!(!value.trim().is_empty(), "{name} must not be empty");
            Ok(value)
        }
        let host = required("MAILBEACON_LIVE_HOST")?;
        ensure!(
            host == host.trim() && !host.chars().any(char::is_whitespace),
            "MAILBEACON_LIVE_HOST must be an exact configured host without whitespace"
        );
        let uid = required("MAILBEACON_LIVE_UID")?
            .parse::<u32>()
            .map_err(|_| anyhow!("MAILBEACON_LIVE_UID must be a positive u32"))?;
        ensure!(uid > 0, "MAILBEACON_LIVE_UID must be positive");
        let expected_bytes = required("MAILBEACON_LIVE_SIZE")?
            .parse::<u32>()
            .map_err(|_| anyhow!("MAILBEACON_LIVE_SIZE must be an exact byte count"))?;
        ensure!(
            expected_bytes > crate::TEXT_FIRST_MIN,
            "MAILBEACON_LIVE_SIZE must select a message eligible for text-first reading"
        );
        ensure!(
            expected_bytes as usize <= body_store::MAX_FILE,
            "MAILBEACON_LIVE_SIZE exceeds the complete-body cache limit"
        );
        Ok(Self {
            host,
            uid,
            expected_bytes,
        })
    }
}

// IMAP/parser error chains can contain a response or a server-supplied address.
// Keep diagnostic output to fixed stage names and numeric measurements only.
fn safe<T, E>(result: std::result::Result<T, E>, stage: &'static str) -> Result<T> {
    result.map_err(|_| anyhow!("live download diagnostic failed at {stage}"))
}

fn profile_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("MAILBEACON_LIVE_PROFILE") {
        ensure!(!path.is_empty(), "live profile override is empty");
        return Ok(PathBuf::from(path));
    }
    Ok(dirs::home_dir()
        .ok_or_else(|| anyhow!("home directory is unavailable"))?
        .join("Library/Application Support/com.lsclpangpang.mailbeacon"))
}

async fn examine(session: &mut imap::Imap) -> Result<u32> {
    let responses = safe(
        safe(
            tokio::time::timeout(
                imap::OP_TIMEOUT,
                download_protocol::checked_command(session, "EXAMINE \"INBOX\""),
            )
            .await,
            "EXAMINE timeout",
        )?,
        "EXAMINE",
    )?;
    responses
        .into_iter()
        .filter_map(|response| match response {
            download_protocol::ArchiveData::Validity(validity) if validity > 0 => Some(validity),
            _ => None,
        })
        .last()
        .ok_or_else(|| anyhow!("EXAMINE did not confirm UIDVALIDITY"))
}

fn validate_complete(raw: &[u8], date: i64, target: &LiveTarget) -> Result<()> {
    ensure!(
        raw.len() == target.expected_bytes as usize,
        "complete body size mismatch"
    );
    safe(
        offline::validate_cached_body(raw),
        "complete body validation",
    )?;
    let view = safe(imap::parse_message(raw, date), "complete MIME parsing")?;
    ensure!(
        !view.text.trim().is_empty()
            || view
                .html
                .as_ref()
                .is_some_and(|html| !html.trim().is_empty()),
        "complete mail contains no readable body"
    );
    ensure!(
        !view.attachments.is_empty(),
        "complete mail contains no attachment"
    );
    eprintln!(
        "live complete_bytes={} text_bytes={} html_bytes={} attachments={}",
        raw.len(),
        view.text.len(),
        view.html.as_ref().map_or(0, String::len),
        view.attachments.len()
    );
    Ok(())
}

async fn run_live_diagnostic(target: LiveTarget) -> Result<()> {
    let target_uid = target.uid;
    let expected_bytes = target.expected_bytes;
    let profile = profile_dir()?;
    let accounts: Vec<config::Account> = safe(
        serde_json::from_slice(&safe(
            std::fs::read(profile.join("accounts.json")),
            "account configuration read",
        )?),
        "account configuration decode",
    )?;
    let mut matching = accounts.into_iter().filter(|account| {
        account.imap_host.eq_ignore_ascii_case(&target.host)
            && account.receive_protocol == config::ReceiveProtocol::Imap
    });
    let account = matching
        .next()
        .ok_or_else(|| anyhow!("configured IMAP host is unavailable"))?;
    ensure!(
        matching.next().is_none(),
        "configured IMAP host matches multiple accounts"
    );

    let credentials = safe(credentials::Store::open(&profile), "credential store open")?;
    let password = safe(credentials.load(&account.id), "credential read")?
        .ok_or_else(|| anyhow!("saved IMAP credential is unavailable"))?;
    let namespace = mail_ref::namespace(&account.id, account.receive_epoch);
    let disk = Arc::new(body_store::BodyStore::new(
        profile.join("bodies"),
        body_store::DEFAULT_LIMIT,
    ));
    disk.preset_limit(&namespace, account.body_cache.limit_bytes());
    let cache = Mutex::new(BodyCache::with_disk(disk.clone()));

    // First and second connections are strictly sequential. No SELECT, STORE,
    // APPEND, MOVE, DELETE, SMTP, or user-visible content output is permitted.
    let mut first = safe(imap::login(&account, &password).await, "first login")?;
    first.get_mut().set_download_timeout(true);
    let validity = examine(&mut first).await?;
    let key = (namespace.clone(), "INBOX".to_string(), validity, target_uid);

    let text_started = Instant::now();
    let (size, partial) = safe(
        text_first_view(&mut first, target_uid, validity).await,
        "text-first read",
    )?;
    ensure!(size == Some(expected_bytes), "target mail size changed");
    let partial = partial.ok_or_else(|| anyhow!("text-first read returned no partial view"))?;
    ensure!(partial.partial, "text-first result was not marked partial");
    ensure!(
        partial.uid_validity == Some(validity),
        "partial view mailbox generation mismatch"
    );
    ensure!(
        !partial.text.trim().is_empty()
            || partial
                .html
                .as_ref()
                .is_some_and(|html| !html.trim().is_empty()),
        "partial view contains no readable body"
    );
    eprintln!(
        "live partial_seconds={:.3} total_bytes={} partial_text_bytes={} partial_html_bytes={}",
        text_started.elapsed().as_secs_f64(),
        expected_bytes,
        partial.text.len(),
        partial.html.as_ref().map_or(0, String::len)
    );
    drop(partial);

    // Repeated diagnostics must not delete an already complete user cache.
    let existing = {
        let disk = disk.clone();
        let namespace = namespace.clone();
        safe(
            tokio::task::spawn_blocking(move || {
                disk.get(&namespace, "INBOX", validity, target_uid)
            })
            .await,
            "existing cache read",
        )?
    };
    if let Some((raw, date)) = existing {
        validate_complete(&raw, date, &target)?;
        drop(first);
        eprintln!("live already_cached_bytes={}", raw.len());
        return Ok(());
    }

    let prior = {
        let disk = disk.clone();
        let namespace = namespace.clone();
        safe(
            tokio::task::spawn_blocking(move || {
                disk.load_resume(&namespace, "INBOX", validity, target_uid)
            })
            .await,
            "existing checkpoint read",
        )?
    };
    let already_complete = prior
        .as_ref()
        .is_some_and(|resume| resume.raw.len() == expected_bytes as usize);
    let interrupted_at = if already_complete {
        // A previous successful network download may still await cache publication.
        expected_bytes as usize
    } else {
        let (confirmed_tx, confirmed_rx) = oneshot::channel::<Result<usize>>();
        let mut confirmed_tx = Some(confirmed_tx);
        let disk = disk.clone();
        let namespace = namespace.clone();
        let download = body_download::fetch(
            &mut first,
            target_uid,
            true,
            prior,
            move |total, date, offset, bytes| {
                let disk = disk.clone();
                let namespace = namespace.clone();
                let sender = confirmed_tx.take();
                async move {
                    let end = offset + bytes.len();
                    let saved = tokio::task::spawn_blocking(move || {
                        disk.append_resume(
                            &namespace, "INBOX", validity, target_uid, total, date, offset, &bytes,
                        )
                    })
                    .await;
                    let saved = safe(saved, "first checkpoint task")
                        .and_then(|result| safe(result, "first checkpoint commit"));
                    if let Some(sender) = sender {
                        let _ = sender.send(saved.map(|()| end));
                    }
                    // Cancellation cannot race a second FETCH: the callback
                    // remains pending after its first durable tagged-OK commit.
                    pending::<()>().await;
                }
            },
        );
        tokio::pin!(download);
        tokio::select! {
            confirmed = confirmed_rx => {
                safe(confirmed, "checkpoint notification")??
            }
            result = &mut download => {
                safe(result, "first range download")?;
                return Err(anyhow!("download finished without the interruption checkpoint"));
            }
        }
    };
    drop(first);
    ensure!(
        interrupted_at > 0,
        "interruption did not retain any confirmed bytes"
    );
    eprintln!("live interrupted_confirmed_bytes={interrupted_at}");

    let mut second = safe(imap::login(&account, &password).await, "second login")?;
    drop(password);
    second.get_mut().set_download_timeout(true);
    ensure!(
        examine(&mut second).await? == validity,
        "mailbox generation changed before resume"
    );
    let network_received = second.get_ref().received();
    let baseline = network_received.load(Ordering::Relaxed);
    let resumed = AtomicU64::new(0);
    let started = Instant::now();
    let result = {
        let download = download_body_report(&mut second, &cache, key.clone(), true, |offset| {
            resumed.store(offset, Ordering::Relaxed);
            eprintln!("live resume_offset={offset}");
        });
        tokio::pin!(download);
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        let mut next_progress = PROGRESS_BYTES;
        loop {
            tokio::select! {
                result = &mut download => break safe(result, "resumed complete download")?,
                _ = interval.tick() => {
                    let bytes = resumed.load(Ordering::Relaxed)
                        + network_received.load(Ordering::Relaxed).saturating_sub(baseline);
                    if bytes >= next_progress {
                        eprintln!(
                            "live progress_bytes={} total_bytes={} elapsed_seconds={:.1}",
                            bytes.min(u64::from(expected_bytes)),
                            expected_bytes,
                            started.elapsed().as_secs_f64()
                        );
                        next_progress = (bytes / PROGRESS_BYTES + 1) * PROGRESS_BYTES;
                    }
                }
            }
        }
    };
    drop(second);
    ensure!(
        resumed.load(Ordering::Relaxed) == interrupted_at as u64,
        "resumed download did not load the confirmed checkpoint"
    );
    let (raw, date) = result;
    validate_complete(&raw, date, &target)?;

    // download_body_report publishes on a blocking worker. Wait for that exact
    // result, rather than writing through a diagnostic-only cache path.
    safe(
        safe(
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    let saved = {
                        let disk = disk.clone();
                        let namespace = namespace.clone();
                        safe(
                            tokio::task::spawn_blocking(move || {
                                disk.get(&namespace, "INBOX", validity, target_uid)
                            })
                            .await,
                            "published cache read",
                        )?
                    };
                    if let Some((saved, _)) = saved {
                        ensure!(
                            saved.as_slice() == raw.as_slice(),
                            "published cache differs"
                        );
                        break Ok::<(), anyhow::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            })
            .await,
            "cache publication timeout",
        )?,
        "cache publication verification",
    )?;
    eprintln!(
        "live cached_bytes={} resumed_elapsed_seconds={:.3}",
        raw.len(),
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

#[tokio::test]
#[ignore = "real IMAP account; requires explicit DOWNLOAD/HOST/UID/SIZE settings and user authorization"]
async fn live_imap_large_mail_text_and_resumable_download() {
    if std::env::var("MAILBEACON_LIVE_DOWNLOAD").as_deref() != Ok("1") {
        return;
    }
    let target = LiveTarget::from_env().unwrap_or_else(|error| panic!("{error}"));
    match tokio::time::timeout(LIVE_BUDGET, run_live_diagnostic(target)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => panic!("{error}"),
        Err(_) => panic!("live download diagnostic exceeded its 20-minute budget"),
    }
}
