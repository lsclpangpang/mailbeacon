//! 账号配置：服务器信息存成 JSON，授权码存进系统钥匙串。

use std::{fs, path::PathBuf};

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
        fs::write(&tmp, serde_json::to_vec_pretty(&self.accounts)?)?;
        fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
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
