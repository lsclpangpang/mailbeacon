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
                let category =
                    if e.is::<crate::imap::AuthFailed>() || e.is::<crate::pop3::AuthFailed>() {
                        "authorization"
                    } else {
                        "connection"
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
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("old-secret"));
        drop(store);
        let mut reloaded = config::Store::load(dir.path().into()).unwrap();
        reloaded.recover_account_updates(&secrets).unwrap();
        assert_eq!(reloaded.get("a").unwrap().smtp_port, 587);
        assert_eq!(secrets.load("a").unwrap().as_deref(), Some("new-secret"));
        assert!(secrets.pending_account_updates().unwrap().is_empty());
        let db = std::fs::read(dir.path().join("local.db")).unwrap();
        assert!(!db.windows(10).any(|w| w == b"new-secret"));
    }
}
