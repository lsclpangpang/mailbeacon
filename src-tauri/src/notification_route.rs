//! Persistent, opaque notification links; a URL never contains mailbox contents.

#![cfg_attr(not(windows), allow(dead_code))]
use anyhow::{Context, Result};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

const TTL_SECONDS: u64 = 7 * 24 * 60 * 60;
const MAX_TICKETS: usize = 256;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub account_id: String,
    pub folder: String,
    pub uid: Option<u32>,
    pub receive_epoch: u64,
    pub uid_validity: Option<u32>,
}

#[derive(Deserialize, Serialize)]
struct Ticket {
    target: Target,
    created: u64,
}

pub struct Tickets {
    dir: PathBuf,
}
impl Tickets {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir: dir.join("notification-links"),
        }
    }
    /// Persist a random capability before showing a toast, so a click survives exit.
    pub fn issue(&self, target: Target, now: u64) -> Result<String> {
        crate::private_fs::create_private_dir(&self.dir)?;
        self.prune(now)?;
        let mut random = [0u8; 32];
        SystemRandom::new()
            .fill(&mut random)
            .map_err(|_| anyhow::anyhow!("通知标识生成失败"))?;
        let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
        crate::durable_fs::atomic_write(
            &self.dir.join(format!("{token}.json")),
            &serde_json::to_vec(&Ticket {
                target,
                created: now,
            })?,
        )?;
        Ok(format!("mailbeacon://notification/{token}"))
    }
    /// Only local, bounded and unexpired tickets can select a message.
    pub fn resolve(&self, url: &str, now: u64) -> Result<Option<Target>> {
        let Some(token) = token(url) else {
            return Ok(None);
        };
        let path = self.dir.join(format!("{token}.json"));
        let meta = match std::fs::symlink_metadata(&path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !meta.is_file() || crate::private_fs::is_link(&meta) || meta.len() > 8192 {
            return Ok(None);
        }
        let ticket: Ticket =
            serde_json::from_slice(&std::fs::read(&path)?).context("通知链接已损坏")?;
        if now < ticket.created || now - ticket.created > TTL_SECONDS {
            return Ok(None);
        }
        Ok(Some(ticket.target))
    }
    fn prune(&self, now: u64) -> Result<()> {
        let mut tickets = Vec::new();
        for entry in std::fs::read_dir(&self.dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".json") else {
                continue;
            };
            if !valid_token(stem) {
                continue;
            }
            let meta = entry.metadata()?;
            if !meta.is_file()
                || crate::private_fs::is_link(&std::fs::symlink_metadata(entry.path())?)
            {
                continue;
            }
            let created = meta
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            if now.saturating_sub(created) > TTL_SECONDS {
                std::fs::remove_file(entry.path())?;
            } else {
                tickets.push((created, entry.path()));
            }
        }
        tickets.sort_by_key(|(created, _)| *created);
        let remove = tickets.len().saturating_sub(MAX_TICKETS - 1);
        for (_, path) in tickets.into_iter().take(remove) {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }
}
fn valid_token(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|c| c.is_ascii_hexdigit())
}
fn token(url: &str) -> Option<&str> {
    let token = url.strip_prefix("mailbeacon://notification/")?;
    valid_token(token).then_some(token)
}

#[derive(Default)]
pub struct Pending {
    ready: bool,
    target: Option<Target>,
}
impl Pending {
    pub fn push(&mut self, target: Target) -> Option<Target> {
        if self.ready {
            Some(target)
        } else {
            self.target = Some(target);
            None
        }
    }
    pub fn ready(&mut self) -> Option<Target> {
        self.ready = true;
        self.target.take()
    }
}

fn xml_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t'))
        .collect::<String>()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
fn toast_xml(title: &str, body: &str, uri: &str) -> String {
    format!(
        "<toast activationType=\"protocol\" launch=\"{}\"><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual></toast>",
        xml_text(uri),
        xml_text(title),
        xml_text(body)
    )
}

/// Windows uses protocol activation instead of transient in-process toast callbacks.
#[cfg(windows)]
pub fn show(app_id: &str, title: &str, body: &str, uri: &str) -> Result<()> {
    use windows::{
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{ToastNotification, ToastNotificationManager},
        core::HSTRING,
    };
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(toast_xml(title, body, uri)))?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?.Show(&toast)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> Target {
        Target {
            account_id: "account1".into(),
            folder: "工作/收件箱".into(),
            uid: Some(42),
            receive_epoch: 3,
            uid_validity: Some(7),
        }
    }

    #[test]
    fn cold_start_resolves_only_local_unexpired_ticket() {
        let dir = tempfile::tempdir().unwrap();
        let store = Tickets::new(dir.path().to_owned());
        let url = store.issue(target(), 100).unwrap();
        assert!(!url.contains("account1") && !url.contains("工作"));
        let reopened = Tickets::new(dir.path().to_owned());
        assert_eq!(reopened.resolve(&url, 101).unwrap(), Some(target()));
        assert_eq!(reopened.resolve(&url, 100 + TTL_SECONDS + 1).unwrap(), None);
        assert_eq!(
            reopened
                .resolve(
                    &format!("mailbeacon://notification/{}", "a".repeat(64)),
                    101
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn untrusted_urls_cannot_select_a_mail_or_read_arbitrary_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Tickets::new(dir.path().to_owned());
        for url in [
            "https://notification/x",
            "mailbeacon://notification/../../accounts",
            "mailbeacon://other/abc",
            "mailbeacon://notification/abc?accountId=a",
            "mailbeacon://notification/abc#x",
        ] {
            assert_eq!(store.resolve(url, 100).unwrap(), None);
        }
    }

    #[test]
    fn pending_click_waits_for_webview_and_latest_click_wins() {
        let mut queue = Pending::default();
        assert_eq!(queue.push(target()), None);
        let mut newer = target();
        newer.uid = Some(43);
        assert_eq!(queue.push(newer.clone()), None);
        assert_eq!(queue.ready(), Some(newer));
        assert_eq!(queue.ready(), None);
        assert_eq!(queue.push(target()), Some(target()));
    }

    #[test]
    fn protocol_toast_escapes_text_and_has_persistent_activation() {
        let xml = toast_xml("A < B & C", "\"Hi\"", "mailbeacon://notification/abc");
        assert!(xml.contains("activationType=\"protocol\""));
        assert!(xml.contains("launch=\"mailbeacon://notification/abc\""));
        assert!(xml.contains("A &lt; B &amp; C"));
        assert!(!xml.contains("< B"));
    }
}
