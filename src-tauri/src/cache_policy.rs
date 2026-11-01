//! Manual and scheduled local-cache cleanup. No mail-server operations are used.
use super::*;
use body_store::{CacheEntry, CleanupReport};
use std::collections::HashSet;

type Key = (String, u32, u32);
fn entry_key(e: &CacheEntry) -> Key {
    (e.folder_hash.clone(), e.uid_validity, e.uid)
}
fn source_key(s: &mail_ref::MessageRef) -> Key {
    (body_store::folder_hash(&s.folder), s.uid_validity, s.uid)
}

fn mail_counts(report: &mut CleanupReport, keys: &HashSet<Key>, failed: &HashSet<Key>) {
    report.count = keys.len() as u64;
    report.removed_count = keys.difference(failed).count() as u64;
    report.failed_count = failed.len() as u64;
}
fn cutoff(days: Option<u32>, now: i64) -> Result<Option<i64>> {
    match days {
        None => Ok(None),
        Some(days) if (1..=36_500).contains(&days) => {
            Ok(Some((now - i64::from(days) * 86_400_000).max(0)))
        }
        _ => bail!("清理期限请填 1～36500 天"),
    }
}
fn account(state: &AppState, id: &str, epoch: u64) -> Result<Account> {
    let account = state.configured_account(id)?;
    anyhow::ensure!(
        account.receive_epoch == epoch,
        "收信服务器已更新，请重新打开缓存设置"
    );
    anyhow::ensure!(
        account.receive_protocol == config::ReceiveProtocol::Imap,
        "此账号不使用 IMAP 原文缓存"
    );
    Ok(account)
}

fn automatic_policy_matches(current_days: u32, expected_days: Option<u32>) -> bool {
    expected_days.is_none_or(|expected| expected > 0 && current_days == expected)
}

#[tauri::command]
pub(crate) async fn preview_body_cache_cleanup(
    state: State<'_, AppState>,
    account_id: String,
    receive_epoch: u64,
    older_than_days: Option<u32>,
) -> CmdResult<CleanupReport> {
    let _gate = state.account_gate(&account_id).read_owned().await;
    let acct = account(&state, &account_id, receive_epoch).map_err(err)?;
    let before = cutoff(older_than_days, chrono::Utc::now().timestamp_millis()).map_err(err)?;
    let storage = mail_ref::namespace(&account_id, acct.receive_epoch);
    let root = state.store.lock().unwrap().dir().to_path_buf();
    let bodies = state.bodies.clone();
    let disk = state.body_store().ok_or("邮件缓存不可用")?;
    tokio::task::spawn_blocking(move || -> Result<CleanupReport> {
        let bodies = bodies.lock().unwrap();
        let mut report = disk.preview_cleanup(&storage, before)?;
        let mut keys = HashSet::new();
        let mut unknown = HashSet::new();
        for entry in disk.cache_entries(&storage)? {
            if entry.date <= 0 {
                unknown.insert(entry_key(&entry));
            }
            if entry.matches(before) {
                keys.insert(entry_key(&entry));
            }
        }
        for row in cache_attachments::candidates(&root, &storage, &disk) {
            if row.date <= 0 {
                unknown.insert(source_key(&row.source));
            }
            if cache_attachments::matches(row.date, before) {
                keys.insert(source_key(&row.source));
                report.bytes += row.bytes;
            }
        }
        for ((namespace, folder, validity, uid), (_, date)) in &bodies.entries {
            if namespace != &storage {
                continue;
            }
            let key = (body_store::folder_hash(folder), *validity, *uid);
            if *date <= 0 {
                unknown.insert(key.clone());
            }
            if cache_attachments::matches(*date, before) {
                keys.insert(key);
            }
        }
        report.count = keys.len() as u64;
        report.unknown_date_count = unknown.len() as u64;
        Ok(report)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(err)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Cleared {
    account_id: String,
    receive_epoch: u64,
}

async fn clear(
    app: &AppHandle,
    state: &AppState,
    id: &str,
    epoch: u64,
    days: Option<u32>,
    expected_auto_days: Option<u32>,
) -> Result<CleanupReport> {
    let _gate = state.account_gate(id).read_owned().await;
    let current = account(state, id, epoch)?;
    // The worker may have waited for an account operation after reading its
    // policy. Disabling or changing that policy revokes the queued cleanup.
    if !automatic_policy_matches(current.body_cache.auto_cleanup_days, expected_auto_days) {
        return Ok(CleanupReport::default());
    }
    let now = chrono::Utc::now().timestamp_millis();
    let before = cutoff(days, now)?;
    let storage = mail_ref::namespace(id, epoch);
    let root = state.store.lock().unwrap().dir().to_path_buf();
    let bodies = state.bodies.clone();
    let disk = state.body_store().context("邮件缓存不可用")?;
    let report = tokio::task::spawn_blocking(move || -> Result<CleanupReport> {
        // All memory publications lock bodies before checking their disk ticket.
        // Holding it through the cleanup closes the memory/disk publication race.
        let mut bodies = bodies.lock().unwrap();
        let mut keys = HashSet::new();
        let mut failed = HashSet::new();
        let mut unknown = HashSet::new();
        let mut report = disk.cleanup_with(&storage, before, now, |entries| {
            for entry in entries {
                if entry.date <= 0 {
                    unknown.insert(entry_key(entry));
                }
                if entry.matches(before) {
                    keys.insert(entry_key(entry));
                }
            }
            let mut extra = CleanupReport::default();
            for row in cache_attachments::candidates(&root, &storage, &disk) {
                let key = source_key(&row.source);
                if row.date <= 0 {
                    unknown.insert(key.clone());
                }
                if !cache_attachments::matches(row.date, before) {
                    continue;
                }
                keys.insert(key.clone());
                extra.count += 1;
                extra.bytes += row.bytes;
                if cache_attachments::remove(&row).is_ok() {
                    extra.removed_count += 1;
                    extra.freed_bytes += row.bytes;
                } else {
                    extra.failed_count += 1;
                    failed.insert(key);
                }
            }
            Ok(extra)
        })?;
        for entry in disk.cache_entries(&storage)? {
            // A failed pending-data deletion may already have removed its JSON
            // date metadata. It is still a retained candidate, even though it
            // now appears as unknown-date and no longer matches an age cutoff.
            let key = entry_key(&entry);
            if keys.contains(&key) {
                failed.insert(key);
            }
        }
        bodies.retain(|((namespace, folder, validity, uid), (_, date))| {
            if namespace != &storage {
                return true;
            }
            let key = (body_store::folder_hash(folder), *validity, *uid);
            if *date <= 0 {
                unknown.insert(key.clone());
            }
            if cache_attachments::matches(*date, before) {
                keys.insert(key);
                false
            } else {
                true
            }
        });
        mail_counts(&mut report, &keys, &failed);
        report.unknown_date_count = unknown.len() as u64;
        Ok(report)
    })
    .await??;
    let _ = app.emit(
        "body-cache-cleared",
        Cleared {
            account_id: id.into(),
            receive_epoch: epoch,
        },
    );
    Ok(report)
}

#[tauri::command]
pub(crate) async fn clear_body_cache(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    receive_epoch: u64,
    older_than_days: Option<u32>,
) -> CmdResult<CleanupReport> {
    clear(
        &app,
        &state,
        &account_id,
        receive_epoch,
        older_than_days,
        None,
    )
    .await
    .map_err(err)
}

/// Run immediately on startup/settings changes, then once an hour. A wakeup is
/// only a request to re-read settings; disabled accounts never trigger cleanup.
pub(crate) async fn worker(app: AppHandle) {
    loop {
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        let accounts = state.store.lock().unwrap().accounts.clone();
        for acct in accounts {
            if acct.receive_protocol != config::ReceiveProtocol::Imap
                || acct.body_cache.auto_cleanup_days == 0
            {
                continue;
            }
            // Recheck after each await: settings may have changed while another
            // account was being cleaned. Do not use a stale automatic policy.
            let Ok(current) = state.configured_account(&acct.id) else {
                continue;
            };
            if current.receive_epoch != acct.receive_epoch
                || current.body_cache.auto_cleanup_days != acct.body_cache.auto_cleanup_days
            {
                continue;
            }
            if let Err(error) = clear(
                &app,
                &state,
                &acct.id,
                acct.receive_epoch,
                Some(acct.body_cache.auto_cleanup_days),
                Some(acct.body_cache.auto_cleanup_days),
            )
            .await
            {
                eprintln!("自动清理邮件缓存失败：{error:#}");
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(3600)) => {},
            _ = state.cache_cleanup_wake.notified() => {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn several_failed_files_for_one_message_are_reported_as_one_failed_mail() {
        let key = (body_store::folder_hash("INBOX"), 7, 1);
        let mut keys = HashSet::new();
        let mut failed = HashSet::new();
        let mut report = CleanupReport::default();
        for _ in 0..3 {
            // raw body, partial checkpoint and extracted attachment
            report.failed_count += 1;
            keys.insert(key.clone());
            failed.insert(key.clone());
        }
        keys.insert((body_store::folder_hash("INBOX"), 7, 2));
        mail_counts(&mut report, &keys, &failed);
        assert_eq!(
            (report.count, report.removed_count, report.failed_count),
            (2, 1, 1)
        );
    }
    #[test]
    fn retention_cutoff_is_server_received_age_and_rejects_invalid_days() {
        let now = 2_000_000_000_000;
        let date = cutoff(Some(30), now).unwrap();
        assert_eq!(date, Some(now - 30 * 86_400_000));
        assert!(!cache_attachments::matches(date.unwrap(), date));
        assert!(cache_attachments::matches(date.unwrap() - 1, date));
        assert!(!cache_attachments::matches(0, date));
        assert!(cache_attachments::matches(0, cutoff(None, now).unwrap()));
        assert!(cutoff(Some(0), now).is_err());
        assert!(cutoff(Some(36_501), now).is_err());
    }

    #[test]
    fn disabling_or_changing_an_automatic_policy_revokes_its_queued_cleanup() {
        assert!(!automatic_policy_matches(0, Some(30)));
        assert!(!automatic_policy_matches(90, Some(30)));
        assert!(!automatic_policy_matches(0, Some(0)));
        assert!(automatic_policy_matches(30, Some(30)));
        // Explicit manual cleanup remains independent of automatic settings.
        assert!(automatic_policy_matches(0, None));
        assert!(automatic_policy_matches(90, None));
    }
}
