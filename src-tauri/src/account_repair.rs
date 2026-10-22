//! 原账户连接修复：明确账户身份、配置版本和持久化恢复边界。

use crate::config::{Account, ReceiveProtocol};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Update {
    pub expected_revision: Option<u64>,
    pub name: Option<String>,
    password: Option<String>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub pop3_host: Option<String>,
    pub pop3_port: Option<u16>,
    pub pop3_tls: Option<bool>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
}

impl Update {
    pub fn password(&self) -> Option<&str> {
        self.password
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
    }
    pub fn apply(&self, old: &Account) -> Result<Account> {
        if self
            .expected_revision
            .is_some_and(|r| r != old.connection_revision)
        {
            bail!("账户设置已更新，请重新打开连接设置后重试");
        }
        let mut next = old.clone();
        if let Some(name) = &self.name {
            next.name = name.trim().to_string();
        }
        for (input, field) in [
            (&self.imap_host, &mut next.imap_host),
            (&self.pop3_host, &mut next.pop3_host),
            (&self.smtp_host, &mut next.smtp_host),
        ] {
            if let Some(host) = input {
                let host = host.trim();
                if host.is_empty()
                    || host.chars().any(|c| {
                        c.is_whitespace() || c.is_control() || matches!(c, '/' | '\\' | '@')
                    })
                {
                    bail!("服务器地址请填写有效主机名，不含协议或路径");
                }
                *field = host.to_string();
            }
        }
        for (input, field) in [
            (self.imap_port, &mut next.imap_port),
            (self.pop3_port, &mut next.pop3_port),
            (self.smtp_port, &mut next.smtp_port),
        ] {
            if let Some(port) = input {
                if port == 0 {
                    bail!("端口请填写 1～65535");
                }
                *field = port;
            }
        }
        if let Some(tls) = self.pop3_tls {
            next.pop3_tls = tls;
        }
        next.connection_revision = old
            .connection_revision
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("连接配置版本已耗尽"))?;
        let changed = match old.receive_protocol {
            ReceiveProtocol::Imap => {
                !old.imap_host.eq_ignore_ascii_case(&next.imap_host)
                    || old.imap_port != next.imap_port
            }
            ReceiveProtocol::Pop3 => {
                !old.pop3_host.eq_ignore_ascii_case(&next.pop3_host)
                    || old.pop3_port != next.pop3_port
                    || old.pop3_tls != next.pop3_tls
            }
        };
        if changed {
            next.receive_epoch = old
                .receive_epoch
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("接收身份版本已耗尽"))?;
            for path in old
                .archive_dirs
                .iter()
                .chain(std::iter::once(&old.auto_download.directory))
            {
                if !path.is_empty() && !next.previous_archive_dirs.contains(path) {
                    next.previous_archive_dirs.push(path.clone());
                }
            }
            next.archive_dirs.clear();
            next.auto_download.enabled = false;
            next.auto_download.directory.clear();
            next.auto_download.checkpoint = None;
            next.auto_download.last_path = None;
            next.auto_download.saved_count = 0;
            next.auto_download.revision = next
                .auto_download
                .revision
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("下载配置版本已耗尽"))?;
        }
        Ok(next)
    }
}

/// 恢复只提交连接字段；中断之后单独保存的签名、通知等配置不能倒退。
pub(crate) fn recover_configuration(current: &Account, pending: &Account) -> Result<Account> {
    if current.connection_revision == pending.connection_revision {
        let matches = current.name == pending.name
            && current.imap_host == pending.imap_host
            && current.imap_port == pending.imap_port
            && current.pop3_host == pending.pop3_host
            && current.pop3_port == pending.pop3_port
            && current.pop3_tls == pending.pop3_tls
            && current.smtp_host == pending.smtp_host
            && current.smtp_port == pending.smtp_port
            && current.receive_epoch == pending.receive_epoch;
        anyhow::ensure!(matches, "账户更新恢复配置不匹配；保留原记录等待修复");
        return Ok(current.clone());
    }
    anyhow::ensure!(
        current.connection_revision.checked_add(1) == Some(pending.connection_revision),
        "账户更新恢复版本不匹配；未覆盖较新设置"
    );
    let input = Update {
        name: Some(pending.name.clone()),
        imap_host: Some(pending.imap_host.clone()),
        imap_port: Some(pending.imap_port),
        pop3_host: Some(pending.pop3_host.clone()),
        pop3_port: Some(pending.pop3_port),
        pop3_tls: Some(pending.pop3_tls),
        smtp_host: Some(pending.smtp_host.clone()),
        smtp_port: Some(pending.smtp_port),
        ..Update::default()
    };
    let mut input = input;
    // 未使用的接收协议字段在旧账户中可以为空。
    match current.receive_protocol {
        ReceiveProtocol::Imap => {
            input.pop3_host = None;
            input.pop3_port = None;
            input.pop3_tls = None;
        }
        ReceiveProtocol::Pop3 => {
            input.imap_host = None;
            input.imap_port = None;
        }
    }
    let next = input.apply(current)?;
    anyhow::ensure!(
        next.receive_epoch == pending.receive_epoch,
        "账户更新恢复接收身份不匹配"
    );
    Ok(next)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub ok: bool,
    pub category: &'static str,
    pub detail: String,
}
impl Probe {
    pub fn from_result(result: Result<()>) -> Self {
        match result {
            Ok(()) => Self {
                ok: true,
                category: "ok",
                detail: "连接及认证成功".into(),
            },
            Err(e) => {
                let smtp = e.downcast_ref::<lettre::transport::smtp::Error>();
                let category = if e.is::<crate::imap::AuthFailed>()
                    || e.is::<crate::pop3::AuthFailed>()
                    || smtp.and_then(|error| error.status()).is_some_and(|code| {
                        matches!(code.to_string().as_str(), "530" | "534" | "535" | "538")
                    }) {
                    "authorization"
                } else if smtp.is_some_and(|error| error.is_tls())
                    || e.chain()
                        .any(|error| error.is::<tokio_rustls::rustls::Error>())
                {
                    "tls"
                } else if smtp.is_some_and(|error| error.is_timeout())
                    || e.chain().any(|error| {
                        error.is::<std::io::Error>() || error.is::<tokio::time::error::Elapsed>()
                    })
                {
                    "network"
                } else if e.is::<rustls_pki_types::InvalidDnsNameError>() {
                    "configuration"
                } else {
                    "unknown"
                };
                Self {
                    ok: false,
                    category,
                    detail: format!("{e:#}"),
                }
            }
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionTest {
    pub receive: Probe,
    pub send: Probe,
}

pub async fn test(account: &Account, pass: &str) -> ConnectionTest {
    let receive = async {
        match account.receive_protocol {
            ReceiveProtocol::Imap => {
                let mut session = crate::imap::login(account, pass).await?;
                let _ =
                    tokio::time::timeout(std::time::Duration::from_secs(5), session.logout()).await;
            }
            ReceiveProtocol::Pop3 => {
                let session = crate::pop3::Pop3::login(account, pass).await?;
                let _ = session.quit().await;
            }
        }
        Ok(())
    };
    let (receive, send) = tokio::join!(receive, crate::smtp::test_connection(account, pass));
    ConnectionTest {
        receive: Probe::from_result(receive),
        send: Probe::from_result(send),
    }
}

impl crate::AppState {
    async fn stop_account_connections(&self, id: &str) {
        self.checks.remove(id);
        let handles = [
            self.watchers.lock().unwrap().remove(id),
            self.download_workers.lock().unwrap().remove(id),
        ];
        for handle in handles.into_iter().flatten() {
            handle.abort();
            let _ = handle.await;
        }
        let control = self
            .bulk_downloads
            .lock()
            .unwrap()
            .get(id)
            .map(|task| task.control.clone());
        if let Some(control) = control {
            control.cancel();
            control.wait_for_writes().await;
        }
        let warm = self
            .cache_warm
            .lock()
            .unwrap()
            .remove(id)
            .and_then(|job| job.handle);
        if let Some(handle) = warm {
            handle.abort();
            let _ = handle.await;
        }
        self.lanes.remove_account(id);
        // POP3 的本机编号仅存在 Handle 中。普通重连保留映射，避免旧列表串到另一封。
        // 真正改变接收端点时，提交后的 invalidate_receive_identity 会隔离旧 Handle。
        let pop3 = self.pop3.lock().unwrap().get(id).cloned();
        if let Some(pop3) = pop3 {
            pop3.disconnect().await;
        }
    }
}

#[tauri::command]
pub(crate) async fn test_account_connection(
    state: tauri::State<'_, crate::AppState>,
    account_id: String,
    input: Update,
) -> crate::CmdResult<ConnectionTest> {
    let _gate = state.account_gate(&account_id).read_owned().await;
    let old = state
        .store
        .lock()
        .unwrap()
        .get(&account_id)
        .cloned()
        .ok_or("账号不存在")?;
    let candidate = input.apply(&old).map_err(crate::err)?;
    let password = match input.password() {
        Some(p) => p.to_string(),
        None => state
            .credentials()
            .map_err(crate::err)?
            .load(&account_id)
            .map_err(crate::err)?
            .ok_or("请填写新授权码")?,
    };
    Ok(test(&candidate, &password).await)
}

#[tauri::command]
pub(crate) async fn update_account_connection(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    account_id: String,
    input: Update,
) -> crate::CmdResult<Account> {
    let _gate = state.account_gate(&account_id).write_owned().await;
    let _downloads = state.download_gate(&account_id).lock_owned().await;
    let old = state
        .store
        .lock()
        .unwrap()
        .get(&account_id)
        .cloned()
        .ok_or("账号不存在")?;
    input.apply(&old).map_err(crate::err)?;
    let credentials = state.credentials().map_err(crate::err)?;
    state.stop_account_connections(&account_id).await;
    // 即使提交完成后的读取失败，也不能把新配置与旧内存凭据拼在一起。
    state.passwords.lock().unwrap().remove(&account_id);
    let saved = {
        let mut store = state.store.lock().unwrap();
        store
            .recover_account_update(&account_id, credentials)
            .and_then(|()| {
                let current = store
                    .get(&account_id)
                    .ok_or_else(|| anyhow::anyhow!("账号不存在"))?;
                let next = input.apply(current)?;
                store.update_connection(next.clone(), input.password(), credentials)?;
                Ok(next)
            })
    };
    // 恢复记录可能先于本次请求完成；失败分支也要撤销旧接收身份。
    let current = state.store.lock().unwrap().get(&account_id).cloned();
    if let Some(current) = &current {
        if current.receive_epoch != old.receive_epoch {
            state.invalidate_receive_identity(
                &account_id,
                old.receive_epoch,
                current.receive_epoch,
            );
            state.auto_downloads.lock().unwrap().insert(
                account_id.clone(),
                crate::auto_download::Status::from_settings(&current.auto_download),
            );
            state.bulk_downloads.lock().unwrap().remove(&account_id);
            state.refresh_archives(&account_id);
        }
    }
    let account = match saved {
        Ok(account) => account,
        Err(error) => {
            // 未完成的提交不能混用新配置和旧凭据。只在没有恢复记录时恢复旧连接。
            let pending = credentials
                .pending_account_updates()
                .map(|pending| pending.iter().any(|(id, _)| id == &account_id))
                .unwrap_or(true);
            if !pending {
                let current = state.store.lock().unwrap().get(&account_id).cloned();
                if let (Some(current), Ok(Some(pass))) = (current, credentials.load(&account_id)) {
                    state
                        .passwords
                        .lock()
                        .unwrap()
                        .insert(account_id.clone(), pass.clone());
                    state.start_watcher(&app, current, pass);
                    state.start_cache_warm(
                        &app,
                        &account_id,
                        std::time::Duration::from_secs(10),
                        false,
                    );
                }
            } else {
                state.passwords.lock().unwrap().remove(&account_id);
            }
            return Err(crate::err(error));
        }
    };
    let pass = credentials
        .load(&account_id)
        .map_err(crate::err)?
        .ok_or("设置已保存，但无法读取新授权码；请重试连接")?;
    state
        .passwords
        .lock()
        .unwrap()
        .insert(account_id.clone(), pass.clone());
    state.start_watcher(&app, account.clone(), pass);
    state.start_cache_warm(&app, &account_id, std::time::Duration::from_secs(10), false);
    Ok(account)
}

#[tauri::command]
pub(crate) async fn reconnect_account(
    app: tauri::AppHandle,
    state: tauri::State<'_, crate::AppState>,
    account_id: String,
) -> crate::CmdResult<Account> {
    update_account_connection(app, state, account_id, Update::default()).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config, credentials};

    fn original() -> config::Account {
        serde_json::from_value(serde_json::json!({
            "id":"a", "kind":"custom", "email":"demo@example.test", "name":"Demo",
            "imapHost":"imap.example.test", "imapPort":993,
            "smtpHost":"smtp.example.test", "smtpPort":465,
            "signature":"Keep signature", "archiveDirs":["/old-export"]
        }))
        .unwrap()
    }

    #[test]
    fn probe_does_not_mislabel_network_tls_or_unknown_errors_as_bad_password() {
        assert_eq!(
            Probe::from_result(Err(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "offline"
            )
            .into()))
            .category,
            "network"
        );
        assert_eq!(
            Probe::from_result(Err(tokio_rustls::rustls::Error::General(
                "bad certificate".into()
            )
            .into()))
            .category,
            "tls"
        );
        assert_eq!(
            Probe::from_result(Err(anyhow::anyhow!("unknown auth processing error"))).category,
            "unknown"
        );
    }

    #[test]
    fn password_or_smtp_update_keeps_receive_identity_and_user_data() {
        let old = original();
        let input: Update =
            serde_json::from_value(serde_json::json!({"password":"new-secret", "smtpPort":587}))
                .unwrap();
        let new = input.apply(&old).unwrap();
        assert_eq!(new.id, old.id);
        assert_eq!(new.receive_epoch, 1);
        assert_eq!(new.connection_revision, 2);
        assert_eq!(new.signature, old.signature);
        assert_eq!(new.archive_dirs, old.archive_dirs);
        assert_eq!(new.smtp_port, 587);
    }

    #[test]
    fn receiving_endpoint_change_isolates_archive_identity_without_deleting_files() {
        let mut old = original();
        old.auto_download.enabled = true;
        old.auto_download.directory = "/auto-export".into();
        let input: Update =
            serde_json::from_value(serde_json::json!({"imapHost":"other.example.test"})).unwrap();
        let new = input.apply(&old).unwrap();
        assert_eq!(new.receive_epoch, 2);
        assert!(new.archive_dirs.is_empty());
        assert!(!new.auto_download.enabled);
        assert!(new.auto_download.directory.is_empty());
        assert!(new.previous_archive_dirs.contains(&"/old-export".into()));
        assert!(new.previous_archive_dirs.contains(&"/auto-export".into()));
        assert_eq!(new.signature, old.signature);
    }

    #[test]
    fn invalid_parameters_and_identity_replacement_are_rejected() {
        assert!(
            serde_json::from_value::<Update>(serde_json::json!({"email":"other@example.test"}))
                .is_err()
        );
        for value in [
            serde_json::json!({"imapPort":0}),
            serde_json::json!({"imapHost":"bad host"}),
        ] {
            let input: Update = serde_json::from_value(value).unwrap();
            assert!(input.apply(&original()).is_err());
        }
    }

    #[test]
    fn update_persists_existing_id_and_blank_password_keeps_secret() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        let input: Update =
            serde_json::from_value(serde_json::json!({"name":"Changed", "password":""})).unwrap();
        let next = input.apply(store.get("a").unwrap()).unwrap();
        store
            .update_connection(next, input.password(), &secrets)
            .unwrap();
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("old-secret"));
        assert_eq!(
            config::Store::load(dir.path().into())
                .unwrap()
                .get("a")
                .unwrap()
                .name,
            "Changed"
        );
    }

    #[test]
    fn json_write_failure_preserves_old_configuration_and_secret() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        std::fs::create_dir(dir.path().join("accounts.json.tmp")).unwrap();
        let input: Update =
            serde_json::from_value(serde_json::json!({"password":"new-secret", "smtpPort":587}))
                .unwrap();
        assert!(
            store
                .update_connection(
                    input.apply(store.get("a").unwrap()).unwrap(),
                    input.password(),
                    &secrets
                )
                .is_err()
        );
        assert_eq!(store.get("a").unwrap().smtp_port, 465);
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("old-secret"));
        assert!(secrets.pending_account_updates().unwrap().is_empty());
    }

    #[test]
    fn credential_publish_failure_retains_recovery_record_until_restart() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
        db.execute_batch("CREATE TRIGGER reject_publish BEFORE UPDATE ON account_credentials BEGIN SELECT RAISE(ABORT, 'simulated publish failure'); END;").unwrap();
        let input: Update =
            serde_json::from_value(serde_json::json!({"password":"new-secret", "smtpPort":587}))
                .unwrap();
        assert!(
            store
                .update_connection(
                    input.apply(store.get("a").unwrap()).unwrap(),
                    input.password(),
                    &secrets
                )
                .is_err()
        );
        assert!(secrets.load("a").is_err(), "待恢复时不能读旧密码");
        assert_eq!(secrets.pending_account_updates().unwrap().len(), 1);
        db.execute_batch("DROP TRIGGER reject_publish").unwrap();
        let mut restarted = config::Store::load(dir.path().into()).unwrap();
        restarted.recover_account_updates(&secrets).unwrap();
        assert_eq!(restarted.get("a").unwrap().smtp_port, 587);
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("new-secret"));
        assert!(secrets.pending_account_updates().unwrap().is_empty());
    }

    #[test]
    fn repair_one_account_does_not_publish_another_accounts_pending_update() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        let mut other = original();
        other.id = "b".into();
        other.email = "b@example.test".into();
        store
            .add_with_password(other.clone(), "b-old", &secrets)
            .unwrap();
        let b_update: Update =
            serde_json::from_value(serde_json::json!({"password":"b-new", "smtpPort":587}))
                .unwrap();
        secrets
            .stage_account_update(
                "b",
                &serde_json::to_string(&b_update.apply(&other).unwrap()).unwrap(),
                b_update.password(),
            )
            .unwrap();
        let next = Update::default().apply(store.get("a").unwrap()).unwrap();
        store.update_connection(next, None, &secrets).unwrap();
        assert_eq!(store.get("b").unwrap().smtp_port, 465);
        assert!(secrets.load("b").is_err(), "B 的待恢复记录必须隔离");
        assert_eq!(secrets.pending_account_updates().unwrap().len(), 1);
    }

    #[test]
    fn recovery_keeps_unrelated_settings_changed_after_an_interrupted_update() {
        for config_was_published in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let secrets = credentials::Store::open(dir.path()).unwrap();
            let mut store = config::Store::load(dir.path().into()).unwrap();
            store
                .add_with_password(original(), "old-secret", &secrets)
                .unwrap();
            let input: Update = serde_json::from_value(
                serde_json::json!({"password":"new-secret", "smtpPort":587}),
            )
            .unwrap();
            let next = input.apply(store.get("a").unwrap()).unwrap();
            if config_was_published {
                let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
                db.execute_batch("CREATE TRIGGER reject_publish BEFORE UPDATE ON account_credentials BEGIN SELECT RAISE(ABORT, 'simulated publish failure'); END;").unwrap();
                assert!(
                    store
                        .update_connection(next, input.password(), &secrets)
                        .is_err()
                );
                db.execute_batch("DROP TRIGGER reject_publish").unwrap();
            } else {
                secrets
                    .stage_account_update(
                        "a",
                        &serde_json::to_string(&next).unwrap(),
                        input.password(),
                    )
                    .unwrap();
            }
            store
                .set_signature("a", "Updated separately".into())
                .unwrap();
            store.recover_account_updates(&secrets).unwrap();
            assert_eq!(store.get("a").unwrap().signature, "Updated separately");
            assert_eq!(store.get("a").unwrap().smtp_port, 587);
            assert_eq!(secrets.load("a").unwrap().as_deref(), Some("new-secret"));
        }
    }

    #[test]
    fn staging_failure_does_not_change_config_or_active_secret() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
        db.execute_batch("CREATE TRIGGER reject_stage BEFORE INSERT ON pending_account_updates BEGIN SELECT RAISE(ABORT, 'simulated stage failure'); END;").unwrap();
        let input: Update =
            serde_json::from_value(serde_json::json!({"password":"new-secret", "smtpPort":587}))
                .unwrap();
        assert!(
            store
                .update_connection(
                    input.apply(store.get("a").unwrap()).unwrap(),
                    input.password(),
                    &secrets
                )
                .is_err()
        );
        assert_eq!(
            config::Store::load(dir.path().into())
                .unwrap()
                .get("a")
                .unwrap()
                .smtp_port,
            465
        );
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("old-secret"));
    }

    #[test]
    fn interrupted_update_recovers_matching_configuration_and_encrypted_secret() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "old-secret", &secrets)
            .unwrap();
        let input: Update =
            serde_json::from_value(serde_json::json!({"password":"new-secret", "smtpPort":587}))
                .unwrap();
        let next = input.apply(store.get("a").unwrap()).unwrap();
        secrets
            .stage_account_update(
                "a",
                &serde_json::to_string(&next).unwrap(),
                input.password(),
            )
            .unwrap();
        assert!(secrets.load("a").is_err(), "提交未完成时不加载旧密码");
        drop(store);
        let mut reloaded = config::Store::load(dir.path().into()).unwrap();
        reloaded.recover_account_updates(&secrets).unwrap();
        assert_eq!(reloaded.get("a").unwrap().smtp_port, 587);
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("new-secret"));
        assert!(secrets.pending_account_updates().unwrap().is_empty());
        let db = std::fs::read(dir.path().join("local.db")).unwrap();
        assert!(!db.windows(10).any(|w| w == b"new-secret"));
    }
    #[test]
    fn orphaned_pending_update_does_not_disable_the_other_accounts() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "a-old", &secrets)
            .unwrap();
        let mut b = original();
        b.id = "b".into();
        b.email = "b@example.test".into();
        store.add_with_password(b, "b-old", &secrets).unwrap();
        let next = Update::default().apply(store.get("a").unwrap()).unwrap();
        secrets
            .stage_account_update("a", &serde_json::to_string(&next).unwrap(), Some("a-new"))
            .unwrap();
        // 移除的 JSON 已提交，但在删除 SQLite 凭据与 pending 前退出。
        store.accounts.retain(|a| a.id != "a");
        store.save().unwrap();
        let mut restarted = config::Store::load(dir.path().into()).unwrap();
        restarted.recover_account_updates(&secrets).unwrap();
        assert!(secrets.pending_account_updates().unwrap().is_empty());
        assert_eq!(secrets.load("b").unwrap().as_deref(), Some("b-old"));
        assert!(restarted.get("a").is_none());
    }

    #[test]
    fn broken_pending_update_isolated_and_never_falls_back_to_old_password() {
        let dir = tempfile::tempdir().unwrap();
        let secrets = credentials::Store::open(dir.path()).unwrap();
        let mut store = config::Store::load(dir.path().into()).unwrap();
        store
            .add_with_password(original(), "a-old", &secrets)
            .unwrap();
        let mut b = original();
        b.id = "b".into();
        b.email = "b@example.test".into();
        store.add_with_password(b, "b-old", &secrets).unwrap();
        for id in ["a", "b"] {
            let input: Update =
                serde_json::from_value(serde_json::json!({"smtpPort":587})).unwrap();
            let next = input.apply(store.get(id).unwrap()).unwrap();
            secrets
                .stage_account_update(
                    id,
                    &serde_json::to_string(&next).unwrap(),
                    Some("new-secret"),
                )
                .unwrap();
        }
        let db = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
        db.execute(
            "UPDATE pending_account_updates SET account_json='{' WHERE account_id='a'",
            [],
        )
        .unwrap();
        store.recover_account_updates(&secrets).unwrap();
        assert!(
            secrets.load("a").is_err(),
            "未完成恢复的账户不能读取旧授权码"
        );
        assert!(store.recover_account_update("a", &secrets).is_err());
        assert_eq!(store.get("b").unwrap().smtp_port, 587);
        assert_eq!(secrets.load("b").unwrap().as_deref(), Some("new-secret"));
        assert_eq!(secrets.pending_account_updates().unwrap().len(), 1);
    }
}
