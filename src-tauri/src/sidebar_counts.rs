//! Local sidebar snapshots and an explicitly requested, throttled STATUS refresh.
use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Count {
    pub value: Option<u64>,
    pub exact: bool,
    pub snapshot_at: Option<i64>,
}
impl Count {
    pub fn unknown() -> Self {
        Self {
            value: None,
            exact: false,
            snapshot_at: None,
        }
    }
    pub fn exact(value: u64, at: Option<i64>) -> Self {
        Self {
            value: Some(value),
            exact: true,
            snapshot_at: at,
        }
    }
    pub fn partial(value: u64, at: Option<i64>) -> Self {
        Self {
            value: (value > 0).then_some(value),
            exact: false,
            snapshot_at: at,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderCounts {
    pub folder: String,
    pub uid_validity: Option<u32>,
    #[serde(rename = "count")]
    pub total: Count,
    pub starred: Count,
    pub attachments: Count,
}
impl FolderCounts {
    pub fn unknown(folder: &str) -> Self {
        Self {
            folder: folder.into(),
            uid_validity: None,
            total: Count::unknown(),
            starred: Count::unknown(),
            attachments: Count::unknown(),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountCounts {
    pub account_id: String,
    pub receive_epoch: u64,
    pub folders: Vec<FolderCounts>,
    pub starred: Count,
    pub attachments: Count,
    pub send_records: Count,
    pub local_drafts: Count,
    pub outbox: Count,
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub accounts: Vec<AccountCounts>,
}
pub struct Refresh {
    pub at: Instant,
    pub running: bool,
}

fn combine<'a>(counts: impl Iterator<Item = &'a Count>, complete: bool) -> Count {
    let rows: Vec<_> = counts.collect();
    let value = rows.iter().filter_map(|c| c.value).sum();
    let exact = complete && rows.iter().all(|c| c.exact && c.value.is_some());
    let at = rows.iter().filter_map(|c| c.snapshot_at).min();
    if exact {
        Count::exact(value, at)
    } else {
        Count::partial(value, at)
    }
}

fn snapshot_for(state: &AppState, selected: Option<&str>) -> Result<Snapshot> {
    let accounts: Vec<_> = state
        .store
        .lock()
        .unwrap()
        .accounts
        .iter()
        .filter(|a| selected.is_none_or(|id| a.id == id))
        .cloned()
        .collect();
    let mut results = Vec::new();
    for account in accounts {
        let index = state.search_index().ok();
        let catalog = state
            .folders
            .lock()
            .unwrap()
            .get(&account.id)
            .cloned()
            .or_else(|| {
                index.and_then(|i| i.catalog_folders(&account.id, account.receive_epoch).ok())
            })
            .unwrap_or_default();
        let catalog_complete = index.is_some_and(|i| {
            i.has_folder_catalog(&account.id, account.receive_epoch)
                .unwrap_or(false)
        });
        let mut folders = Vec::new();
        for folder in &catalog {
            if folder.path.starts_with("__") {
                continue;
            }
            folders.push(
                index
                    .and_then(|i| {
                        i.sidebar_folder_counts(&account.id, account.receive_epoch, &folder.path)
                            .ok()
                    })
                    .unwrap_or_else(|| FolderCounts::unknown(&folder.path)),
            );
        }
        let starred = folders
            .iter()
            .find(|f| f.folder.eq_ignore_ascii_case("INBOX"))
            .map(|f| f.starred.clone())
            .unwrap_or_else(Count::unknown);
        let attachments = combine(
            folders
                .iter()
                .filter(|f| {
                    catalog.iter().any(|c| {
                        c.path == f.folder
                            && !matches!(c.role.as_str(), "junk" | "trash" | "drafts")
                    })
                })
                .map(|f| &f.attachments),
            catalog_complete,
        );
        let send_records = state
            .sending()
            .and_then(|s| s.count(&account.id))
            .map(|v| Count::exact(v, None))
            .unwrap_or_else(|_| Count::unknown());
        let local_drafts = state
            .local_drafts
            .count(&account.id)
            .map(|v| Count::exact(v, None))
            .unwrap_or_else(|_| Count::unknown());
        let sent = state.outbox_sent.lock().unwrap().clone();
        let outbox = state
            .outbox
            .list(Some(&account.id))
            .map(|items| {
                Count::exact(
                    items
                        .iter()
                        .filter(|i| !sent.contains(&(i.account_id.clone(), i.id)))
                        .count() as u64,
                    None,
                )
            })
            .unwrap_or_else(|_| Count::unknown());
        folders.push(FolderCounts {
            folder: drafts::LOCAL_DRAFTS.into(),
            total: local_drafts.clone(),
            ..FolderCounts::unknown(drafts::LOCAL_DRAFTS)
        });
        folders.push(FolderCounts {
            folder: outbox::OUTBOX.into(),
            total: outbox.clone(),
            ..FolderCounts::unknown(outbox::OUTBOX)
        });
        results.push(AccountCounts {
            account_id: account.id,
            receive_epoch: account.receive_epoch,
            folders,
            starred,
            attachments,
            send_records,
            local_drafts,
            outbox,
        });
    }
    Ok(Snapshot { accounts: results })
}

#[tauri::command]
pub fn get_sidebar_counts(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> CmdResult<Snapshot> {
    snapshot_for(&state, account_id.as_deref()).map_err(err)
}

pub(crate) async fn status<T>(
    session: &mut async_imap::Session<T>,
    folder: &str,
) -> Result<(u32, u32)>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    use crate::download_protocol::{ArchiveData, checked_command, quoted_mailbox};
    let command = format!("STATUS {} (MESSAGES UIDVALIDITY)", quoted_mailbox(folder)?);
    let rows = timeout(OP_TIMEOUT, checked_command(session, &command))
        .await
        .context("读取邮件数量超时")??;
    rows.into_iter()
        .find_map(|row| match row {
            ArchiveData::MailboxStatus {
                folder: name,
                messages: Some(count),
                validity: Some(validity),
            } if imap::mailbox_eq(&name, folder) && validity > 0 => Some((validity, count)),
            _ => None,
        })
        .ok_or_else(|| anyhow!("服务器未返回完整的文件夹数量和代际"))
}

static COUNT_NETWORK: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

#[tauri::command]
pub async fn refresh_sidebar_counts(
    state: State<'_, AppState>,
    app: AppHandle,
    account_id: String,
    force: Option<bool>,
) -> CmdResult<Snapshot> {
    let account = state.configured_account(&account_id).map_err(err)?;
    if account.receive_protocol == config::ReceiveProtocol::Pop3 {
        return snapshot_for(&state, Some(&account_id)).map_err(err);
    }
    let key = mail_ref::namespace(&account.id, account.receive_epoch);
    {
        let mut jobs = state.sidebar_count_jobs.lock().unwrap();
        if jobs.get(&key).is_some_and(|j| {
            j.running || (force != Some(true) && j.at.elapsed() < Duration::from_secs(30))
        }) {
            return snapshot_for(&state, Some(&account_id)).map_err(err);
        }
        jobs.insert(
            key.clone(),
            Refresh {
                at: Instant::now(),
                running: true,
            },
        );
    }
    let outcome = async {
        let _permit = COUNT_NETWORK.acquire().await?;
        let (current, password) = state.account(&account_id)?;
        anyhow::ensure!(
            current.receive_epoch == account.receive_epoch,
            "收信服务器已更改，请重新读取数量"
        );
        let index = state.search_index()?.clone();
        let mut session = imap::login(&current, &password).await?;
        let folders = imap::list_folders(&mut session).await?;
        {
            let _gate = state.account_gate(&account_id).read_owned().await;
            anyhow::ensure!(
                state.configured_account(&account_id)?.receive_epoch == account.receive_epoch,
                "收信服务器已更改，请重新读取数量"
            );
            index.save_folders(&account.id, account.receive_epoch, &folders)?;
            state
                .folders
                .lock()
                .unwrap()
                .insert(account.id.clone(), folders.clone());
        }
        let mut first_error = None;
        for folder in folders {
            let revision =
                index.sidebar_status_revision(&account.id, account.receive_epoch, &folder.path)?;
            match status(&mut session, &folder.path).await {
                Ok((validity, total)) => {
                    let _gate = state.account_gate(&account_id).read_owned().await;
                    anyhow::ensure!(
                        state.configured_account(&account_id)?.receive_epoch
                            == account.receive_epoch,
                        "收信服务器已更改，请重新读取数量"
                    );
                    index.record_sidebar_status_if_current(
                        &account.id,
                        account.receive_epoch,
                        &folder.path,
                        validity,
                        total,
                        revision,
                    )?;
                }
                Err(error) => {
                    let disconnected = download_protocol::connection_problem(&error);
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                    if disconnected {
                        break;
                    }
                }
            }
        }
        let _ = timeout(Duration::from_secs(2), session.logout()).await;
        if let Some(error) = first_error {
            return Err(error);
        }
        Ok(())
    }
    .await;
    if let Some(job) = state.sidebar_count_jobs.lock().unwrap().get_mut(&key) {
        job.running = false;
        job.at = Instant::now();
    }
    // Aggregate once for the whole round, including folders fetched before a later failure.
    // This avoids repeatedly scanning every folder and reading local draft MIME for each STATUS.
    let snapshot = snapshot_for(&state, Some(&account_id)).map_err(err)?;
    let _ = app.emit("sidebarCounts", snapshot.clone());
    outcome.map_err(err)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    async fn response(
        data: &str,
        completion: Option<&str>,
    ) -> async_imap::Session<tokio::io::DuplexStream> {
        let (client, server) = tokio::io::duplex(4096);
        let (data, completion) = (data.to_string(), completion.map(str::to_string));
        tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut reader = BufReader::new(read);
            write.write_all(b"* OK test\r\n").await.unwrap();
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let tag = line.split_whitespace().next().unwrap();
            write
                .write_all(format!("{tag} OK login\r\n").as_bytes())
                .await
                .unwrap();
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert!(line.contains("STATUS \"Archive\" (MESSAGES UIDVALIDITY)"));
            write.write_all(data.as_bytes()).await.unwrap();
            if let Some(done) = completion {
                let tag = line.split_whitespace().next().unwrap();
                write
                    .write_all(format!("{tag} {done}\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        let mut client = async_imap::Client::new(client);
        client.read_response().await.unwrap();
        client
            .login("test", "pw")
            .await
            .map_err(|(e, _)| e)
            .unwrap()
    }

    #[tokio::test]
    async fn sidebar_status_requires_messages_validity_and_tagged_success() {
        let mut good = response(
            "* STATUS Archive (MESSAGES 450 UIDVALIDITY 7)\r\n",
            Some("OK status"),
        )
        .await;
        assert_eq!(status(&mut good, "Archive").await.unwrap(), (7, 450));
        for (data, done) in [
            ("* STATUS Archive (UIDVALIDITY 7)\r\n", Some("OK status")),
            (
                "* STATUS Archive (MESSAGES 450 UIDVALIDITY 7)\r\n",
                Some("NO denied"),
            ),
            ("* STATUS Archive (MESSAGES 450 UIDVALIDITY 7)\r\n", None),
            (
                "* STATUS Other (MESSAGES 0 UIDVALIDITY 7)\r\n",
                Some("OK status"),
            ),
        ] {
            let mut session = response(data, done).await;
            assert!(status(&mut session, "Archive").await.is_err());
        }
    }

    #[test]
    fn sidebar_unknown_scope_never_becomes_an_exact_zero() {
        assert_eq!(combine(std::iter::empty(), false), Count::unknown());
        let counts = [Count::exact(4, None), Count::unknown()];
        assert_eq!(combine(counts.iter(), true), Count::partial(4, None));
        let counts = [Count::exact(0, None), Count::exact(3, None)];
        assert_eq!(combine(counts.iter(), true), Count::exact(3, None));
    }
}
