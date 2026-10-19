//! 账号配置：服务器信息存成 JSON，授权码存进系统钥匙串。

use std::{fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "MailPulse";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    /// exmail | qq | 163 | gmail | custom
    pub kind: String,
    pub email: String,
    /// 发信时显示的名字
    pub name: String,
    /// 新邮件、回复和转发时由编辑器插入的可编辑纯文本签名。
    #[serde(default)]
    pub signature: String,
    /// 每个账号独立的自动下载设置和已确认进度。
    #[serde(default)]
    pub auto_download: crate::auto_download::Settings,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
}

/// 「添加邮箱」表单
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAccount {
    pub kind: String,
    pub email: String,
    pub password: String,
    #[serde(default)]
    pub name: String,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
}

/// 常见邮箱的服务器地址：(IMAP, SMTP)，端口统一 993 / 465。
fn preset(kind: &str) -> Option<(&'static str, &'static str)> {
    match kind {
        "exmail" => Some(("imap.exmail.qq.com", "smtp.exmail.qq.com")),
        "qq" => Some(("imap.qq.com", "smtp.qq.com")),
        "163" => Some(("imap.163.com", "smtp.163.com")),
        "126" => Some(("imap.126.com", "smtp.126.com")),
        "gmail" => Some(("imap.gmail.com", "smtp.gmail.com")),
        _ => None,
    }
}

impl NewAccount {
    pub fn into_account(self, id: String) -> Result<(Account, String)> {
        let email = self.email.trim().to_string();
        if !email.contains('@') {
            bail!("邮箱地址格式不对");
        }
        let password = self.password.trim().to_string();
        if password.is_empty() {
            bail!("请填写授权码");
        }
        let (imap_host, smtp_host) = match preset(&self.kind) {
            Some((i, s)) => (i.to_string(), s.to_string()),
            None => (
                self.imap_host
                    .filter(|h| !h.trim().is_empty())
                    .ok_or_else(|| anyhow!("请填写 IMAP 服务器"))?,
                self.smtp_host
                    .filter(|h| !h.trim().is_empty())
                    .ok_or_else(|| anyhow!("请填写 SMTP 服务器"))?,
            ),
        };
        let name = match self.name.trim() {
            "" => email.split('@').next().unwrap_or(&email).to_string(),
            n => n.to_string(),
        };
        let acct = Account {
            id,
            kind: self.kind,
            email,
            name,
            signature: String::new(),
            auto_download: Default::default(),
            imap_host: imap_host.trim().to_string(),
            imap_port: self.imap_port.unwrap_or(993),
            smtp_host: smtp_host.trim().to_string(),
            smtp_port: self.smtp_port.unwrap_or(465),
        };
        Ok((acct, password))
    }
}

pub struct Store {
    path: PathBuf,
    pub accounts: Vec<Account>,
}

impl Store {
    pub fn load(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir)?;
        let path = dir.join("accounts.json");
        let accounts = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .with_context(|| format!("{} 格式错误", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, accounts })
    }

    pub fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(&self.accounts)?)?;
        file.sync_all()?;
        fs::rename(&tmp, &self.path)?;
        #[cfg(unix)]
        if let Some(parent) = self.path.parent() {
            // rename 已提交，之后的目录耐久性错误不能让调用方回滚成旧内存配置。
            if let Err(error) = fs::File::open(parent).and_then(|dir| dir.sync_all()) {
                eprintln!("账号配置已保存，但目录同步失败：{error}");
            }
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// 原子保存自动下载配置和进度；写盘失败时恢复内存状态。
    pub fn set_auto_download(
        &mut self,
        id: &str,
        settings: crate::auto_download::Settings,
    ) -> Result<Account> {
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let previous = std::mem::replace(&mut self.accounts[index].auto_download, settings);
        if let Err(error) = self.save() {
            self.accounts[index].auto_download = previous;
            return Err(error.context("保存自动下载设置或进度失败"));
        }
        Ok(self.accounts[index].clone())
    }

    /// 保存指定账号的签名；落盘失败时恢复原来的内存值。
    pub fn set_signature(&mut self, id: &str, signature: String) -> Result<Account> {
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let previous = std::mem::replace(&mut self.accounts[index].signature, signature);
        if let Err(error) = self.save() {
            self.accounts[index].signature = previous;
            return Err(error.context("保存签名失败"));
        }
        Ok(self.accounts[index].clone())
    }
}

pub fn save_password(id: &str, password: &str) -> Result<()> {
    keyring::Entry::new(KEYCHAIN_SERVICE, id)?
        .set_password(password)
        .context("写入钥匙串失败")
}

pub fn load_password(id: &str) -> Result<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, id)?
        .get_password()
        .context("从钥匙串读取授权码失败")
}

pub fn delete_password(id: &str) {
    if let Ok(e) = keyring::Entry::new(KEYCHAIN_SERVICE, id) {
        let _ = e.delete_credential();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OLD_ACCOUNT: &str = r#"{"id":"test","kind":"qq","email":"test@example.com","name":"测试","imapHost":"imap.example.com","imapPort":993,"smtpHost":"smtp.example.com","smtpPort":465}"#;

    fn test_dir(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "mailpulse-config-{label}-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap()
        ))
    }

    #[test]
    fn automatic_download_defaults_off_and_persists_checkpoint() {
        let dir = test_dir("auto-download");
        let mut store = Store::load(dir.clone()).unwrap();
        store
            .accounts
            .push(serde_json::from_str(OLD_ACCOUNT).unwrap());
        assert!(!store.get("test").unwrap().auto_download.enabled);
        let settings = crate::auto_download::Settings {
            filter_mode: crate::auto_download::FilterMode::None,
            keywords: Vec::new(),
            enabled: true,
            directory: dir.display().to_string(),
            revision: 1,
            checkpoint: Some(crate::auto_download::Checkpoint {
                uid_validity: 42,
                last_uid: 100,
            }),
            saved_count: 0,
            last_path: None,
            ..Default::default()
        };
        store.set_auto_download("test", settings.clone()).unwrap();
        let loaded = Store::load(dir.clone()).unwrap();
        assert_eq!(loaded.get("test").unwrap().auto_download, settings);
        fs::create_dir(dir.join("accounts.json.tmp")).unwrap();
        assert!(store.set_auto_download("test", Default::default()).is_err());
        assert_eq!(store.get("test").unwrap().auto_download, settings);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn old_accounts_load_with_empty_signature_and_persist_changes() {
        let dir = test_dir("migration");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("accounts.json"), format!("[{OLD_ACCOUNT}]")).unwrap();
        let mut store = Store::load(dir.clone()).unwrap();
        assert_eq!(store.get("test").unwrap().signature, "");
        let updated = store
            .set_signature("test", "张三\n产品团队".into())
            .unwrap();
        assert_eq!(updated.email, "test@example.com");
        assert_eq!(updated.imap_host, "imap.example.com");
        assert_eq!(
            Store::load(dir.clone())
                .unwrap()
                .get("test")
                .unwrap()
                .signature,
            "张三\n产品团队"
        );
        store.set_signature("test", String::new()).unwrap();
        assert_eq!(
            Store::load(dir.clone())
                .unwrap()
                .get("test")
                .unwrap()
                .signature,
            ""
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn field_keyword_drafts_roundtrip_without_losing_legacy_rules_or_progress() {
        use crate::auto_download::{Checkpoint, KeywordScope, Settings};
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store
            .accounts
            .push(serde_json::from_str(OLD_ACCOUNT).unwrap());
        let mut settings: Settings = serde_json::from_value(serde_json::json!({
            "filterMode":"blacklist", "keywords":["legacy"], "keywordScope":"fields",
            "fieldKeywords":{"subject":["主题"],"sender":["Sender"],"recipient":["To"],"body":["正文"]},
            "checkpoint":{"uidValidity":7,"lastUid":200}, "savedCount":42
        })).unwrap();
        settings.normalize_filter_rules().unwrap();
        store.set_auto_download("test", settings.clone()).unwrap();
        let loaded = Store::load(dir.path().to_path_buf()).unwrap();
        let saved = &loaded.get("test").unwrap().auto_download;
        assert_eq!(saved, &settings);
        assert_eq!(saved.keyword_scope, KeywordScope::Fields);
        assert_eq!(saved.keywords, ["legacy"]);
        assert_eq!(
            saved.checkpoint,
            Some(Checkpoint {
                uid_validity: 7,
                last_uid: 200
            })
        );
        assert_eq!(saved.saved_count, 42);
    }

    #[test]
    fn failed_signature_write_restores_memory() {
        let dir = test_dir("rollback");
        let mut store = Store::load(dir.clone()).unwrap();
        store
            .accounts
            .push(serde_json::from_str(OLD_ACCOUNT).unwrap());
        fs::create_dir(dir.join("accounts.json.tmp")).unwrap();
        assert!(store.set_signature("test", "不能保存".into()).is_err());
        assert_eq!(store.get("test").unwrap().signature, "");
        assert!(store.set_signature("missing", "签名".into()).is_err());
        fs::remove_dir_all(dir).unwrap();
    }
}
