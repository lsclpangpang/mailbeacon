//! 账号配置：服务器信息存成 JSON，授权码加密存入本地数据库；钥匙串只用于旧版迁移。

use std::{fs, io::Write, path::PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "MailPulse";

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReceiveProtocol {
    #[default]
    Imap,
    Pop3,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub id: String,
    #[serde(default = "initial_revision")]
    pub receive_epoch: u64,
    #[serde(default = "initial_revision")]
    pub connection_revision: u64,
    #[serde(default)]
    pub previous_archive_dirs: Vec<String>,
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
    /// 邮件原文本地缓存：总量、后台预先下载的单封大小上限
    #[serde(default)]
    pub body_cache: BodyCacheSettings,
    /// 做过全量下载的父目录：里面已下载的邮件直接当缓存读（只读，不改不删）
    #[serde(default)]
    pub archive_dirs: Vec<String>,
    /// 收信协议；旧配置缺省为 IMAP。
    #[serde(default)]
    pub receive_protocol: ReceiveProtocol,
    /// POP3 接收服务器。仅 receiveProtocol=pop3 时使用。
    #[serde(default)]
    pub pop3_host: String,
    #[serde(default = "default_pop3_port")]
    pub pop3_port: u16,
    #[serde(default = "default_pop3_tls")]
    pub pop3_tls: bool,
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
}

pub(crate) fn initial_revision() -> u64 {
    1
}

/// 目录路径比较用：去掉首尾空白和末尾的分隔符（根目录除外）
fn normalize_dir(path: &str) -> String {
    let path = path.trim();
    let trimmed = path.trim_end_matches(['/', '\\']);
    // 根目录和盘符根（C:\）去掉分隔符就变了意思
    if trimmed.is_empty() || trimmed.ends_with(':') {
        path[..(trimmed.len() + 1).min(path.len())].to_string()
    } else {
        trimmed.to_string()
    }
}

/// 邮件原文本地缓存的设置（每个账号一份）
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BodyCacheSettings {
    /// 后台预先把收件箱的邮件下到本地
    pub enabled: bool,
    /// 这个账号的缓存总量上限（MB）
    pub limit_mb: u32,
    /// 后台预先下载时，单封邮件的大小上限（MB）；更大的等打开时再下
    pub max_mail_mb: u32,
}

impl Default for BodyCacheSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            limit_mb: 2048,
            max_mail_mb: 5,
        }
    }
}

impl BodyCacheSettings {
    /// 总量 64 MB～100 GB；单封 1～32 MB（更大的邮件不落盘），且不超过总量
    pub fn check(&self) -> Result<()> {
        if !(64..=102_400).contains(&self.limit_mb) {
            bail!("缓存总量请填 64～102400 MB");
        }
        if !(1..=32).contains(&self.max_mail_mb) {
            bail!("单封大小上限请填 1～32 MB");
        }
        if self.max_mail_mb > self.limit_mb {
            bail!("单封大小上限不能超过缓存总量");
        }
        Ok(())
    }

    pub fn limit_bytes(&self) -> u64 {
        u64::from(self.limit_mb) * 1024 * 1024
    }

    pub fn max_mail_bytes(&self) -> u32 {
        self.max_mail_mb * 1024 * 1024
    }
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
    pub receive_protocol: Option<ReceiveProtocol>,
    pub pop3_host: Option<String>,
    pub pop3_port: Option<u16>,
    pub pop3_tls: Option<bool>,
    pub imap_host: Option<String>,
    pub imap_port: Option<u16>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<u16>,
}

fn default_pop3_port() -> u16 {
    995
}
fn default_pop3_tls() -> bool {
    true
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

fn pop3_preset(kind: &str) -> Option<&'static str> {
    match kind {
        "exmail" => Some("pop.exmail.qq.com"),
        "qq" => Some("pop.qq.com"),
        "163" => Some("pop.163.com"),
        "126" => Some("pop.126.com"),
        "gmail" => Some("pop.gmail.com"),
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
        let receive_protocol = self.receive_protocol.unwrap_or_default();
        let (imap_host, smtp_host) = match preset(&self.kind) {
            Some((i, s)) => (i.to_string(), s.to_string()),
            None => {
                let imap_host = match self.imap_host.filter(|h| !h.trim().is_empty()) {
                    Some(host) => host,
                    None if receive_protocol == ReceiveProtocol::Pop3 => String::new(),
                    None => return Err(anyhow!("请填写 IMAP 服务器")),
                };
                let smtp_host = self
                    .smtp_host
                    .filter(|h| !h.trim().is_empty())
                    .ok_or_else(|| anyhow!("请填写 SMTP 服务器"))?;
                (imap_host, smtp_host)
            }
        };
        let pop3_host = self
            .pop3_host
            .filter(|h| !h.trim().is_empty())
            .or_else(|| pop3_preset(&self.kind).map(str::to_string))
            .unwrap_or_default();
        if receive_protocol == ReceiveProtocol::Pop3 && pop3_host.is_empty() {
            bail!("请填写 POP3 服务器");
        }
        let name = match self.name.trim() {
            "" => email.split('@').next().unwrap_or(&email).to_string(),
            n => n.to_string(),
        };
        let acct = Account {
            id,
            receive_epoch: 1,
            connection_revision: 1,
            previous_archive_dirs: Vec::new(),
            kind: self.kind,
            email,
            name,
            signature: String::new(),
            auto_download: Default::default(),
            body_cache: Default::default(),
            archive_dirs: Vec::new(),
            receive_protocol,
            pop3_host: pop3_host.trim().to_string(),
            pop3_port: self.pop3_port.unwrap_or(995),
            pop3_tls: self.pop3_tls.unwrap_or(true),
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

    /// 配置目录，其他本地状态（如 POP3 已读记录）也放在这里。
    pub fn dir(&self) -> PathBuf {
        self.path.parent().map(PathBuf::from).unwrap_or_default()
    }

    pub fn get(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// 保存新账号及本地授权码；失败时恢复内存并尽力清理本次新增的凭据。
    /// JSON 和 SQLite 分别持久化，不承诺两者在断电时构成同一个原子事务。
    pub fn add_with_password(
        &mut self,
        account: Account,
        password: &str,
        credentials: &crate::credentials::Store,
    ) -> Result<()> {
        if self.accounts.iter().any(|a| a.id == account.id) {
            bail!("账号标识已存在，请重试添加");
        }
        if self
            .accounts
            .iter()
            .any(|a| a.email.eq_ignore_ascii_case(&account.email))
        {
            bail!("这个邮箱已经添加过了");
        }
        // 即使 JSON 中没有账号，也不能覆盖之前保存但尚未关联的授权码。
        if credentials.load(&account.id)?.is_some() {
            bail!("账号标识已有本地授权码，请重试添加");
        }
        let id = account.id.clone();
        let previous_len = self.accounts.len();
        let result = (|| {
            credentials.save(&id, password)?;
            if credentials.load(&id)?.as_deref() != Some(password) {
                bail!("本地授权码保存后校验失败");
            }
            self.accounts.push(account);
            self.save().context("保存账号配置失败")
        })();
        if let Err(error) = result {
            self.accounts.truncate(previous_len);
            if let Err(cleanup) = credentials.remove(&id) {
                return Err(error.context(format!(
                    "添加账号失败，且无法清理本次保存的授权码：{cleanup:#}"
                )));
            }
            return Err(error);
        }
        Ok(())
    }

    /// 移除账号及本地授权码；凭据删除失败时尝试恢复原账号配置。
    /// 调用方应在成功后停止后台任务和清理缓存，避免失败时留下半删除的运行状态。
    pub fn remove_with_password(
        &mut self,
        id: &str,
        credentials: &crate::credentials::Store,
    ) -> Result<()> {
        if self.get(id).is_none() {
            bail!("账号不存在");
        }
        let previous = self.accounts.clone();
        self.accounts.retain(|a| a.id != id);
        if let Err(error) = self.save() {
            self.accounts = previous;
            return Err(error.context("保存移除后的账号配置失败"));
        }
        if let Err(error) = credentials.remove(id) {
            self.accounts = previous;
            if let Err(rollback) = self.save() {
                return Err(error.context(format!(
                    "删除本地授权码失败，恢复账号配置也失败：{rollback:#}"
                )));
            }
            return Err(error.context("删除本地授权码失败，已恢复账号配置"));
        }
        Ok(())
    }

    /// 更新同一账户；恢复记录只含配置和独立加密密文，从不存明文授权码。
    pub fn update_connection(
        &mut self,
        next: Account,
        password: Option<&str>,
        credentials: &crate::credentials::Store,
    ) -> Result<()> {
        self.recover_account_updates(credentials)?;
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == next.id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let old = self.accounts[index].clone();
        if next.email != old.email || next.receive_protocol != old.receive_protocol {
            bail!("不能在修复时更换邮箱身份或协议");
        }
        if next.connection_revision
            != old
                .connection_revision
                .checked_add(1)
                .ok_or_else(|| anyhow!("配置版本已耗尽"))?
        {
            bail!("账户设置已更新，请重新打开后重试");
        }
        credentials.stage_account_update(&next.id, &serde_json::to_string(&next)?, password)?;
        self.accounts[index] = next.clone();
        if let Err(error) = self.save() {
            self.accounts[index] = old;
            credentials
                .cancel_account_update(&next.id)
                .context("保存失败，取消恢复记录也失败；请重启完成恢复")?;
            return Err(error.context("保存失败，仍保留原设置"));
        }
        credentials
            .finish_account_update(&next.id)
            .context("设置已写入，授权码提交尚未完成；请重试或重启恢复")
    }

    /// 启动时先完成中断的提交，再加载任何网络连接的凭据。
    pub fn recover_account_updates(
        &mut self,
        credentials: &crate::credentials::Store,
    ) -> Result<()> {
        for (id, json) in credentials.pending_account_updates()? {
            let next: Account = serde_json::from_str(&json).context("账户更新恢复记录损坏")?;
            let index = self
                .accounts
                .iter()
                .position(|a| a.id == id)
                .ok_or_else(|| anyhow!("恢复记录对应的账户不存在"))?;
            if next.id != id
                || next.email != self.accounts[index].email
                || next.receive_protocol != self.accounts[index].receive_protocol
            {
                bail!("账户更新恢复身份不匹配");
            }
            let old = std::mem::replace(&mut self.accounts[index], next);
            if let Err(error) = self.save() {
                self.accounts[index] = old;
                return Err(error.context("无法恢复账户配置"));
            }
            credentials.finish_account_update(&id)?;
        }
        Ok(())
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

    /// 保存指定账号的邮件缓存设置；不合理的值直接拒绝，落盘失败时恢复原来的内存值。
    pub fn set_body_cache(&mut self, id: &str, settings: BodyCacheSettings) -> Result<Account> {
        settings.check()?;
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let previous = std::mem::replace(&mut self.accounts[index].body_cache, settings);
        if let Err(error) = self.save() {
            self.accounts[index].body_cache = previous;
            return Err(error.context("保存缓存设置失败"));
        }
        Ok(self.accounts[index].clone())
    }

    /// 记下一个全量下载目录（已有就不重复）；落盘失败时恢复原来的内存值。
    /// 返回这次是否新加了。
    pub fn add_archive_dir(&mut self, id: &str, path: &str) -> Result<bool> {
        let path = normalize_dir(path);
        if path.is_empty() {
            bail!("目录不能为空");
        }
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let dirs = &mut self.accounts[index].archive_dirs;
        if dirs.iter().any(|d| normalize_dir(d) == path) {
            return Ok(false);
        }
        dirs.push(path);
        if let Err(error) = self.save() {
            self.accounts[index].archive_dirs.pop();
            return Err(error.context("保存下载目录失败"));
        }
        Ok(true)
    }

    /// 不再把这个目录当缓存用；只删记录，不碰磁盘上的文件。
    pub fn remove_archive_dir(&mut self, id: &str, path: &str) -> Result<Account> {
        let path = normalize_dir(path);
        let index = self
            .accounts
            .iter()
            .position(|a| a.id == id)
            .ok_or_else(|| anyhow!("账号不存在"))?;
        let previous = self.accounts[index].archive_dirs.clone();
        self.accounts[index]
            .archive_dirs
            .retain(|d| normalize_dir(d) != path);
        if let Err(error) = self.save() {
            self.accounts[index].archive_dirs = previous;
            return Err(error.context("保存下载目录失败"));
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

/// 只在旧账号尚无本地凭据时读取旧钥匙串；保留原条目供旧版回退使用。
pub fn load_legacy_password(id: &str) -> Result<String> {
    keyring::Entry::new(KEYCHAIN_SERVICE, id)?
        .get_password()
        .context("从旧版钥匙串迁移授权码失败，可重启后重试")
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

    fn credential_account(id: &str, email: &str) -> Account {
        let mut account: Account = serde_json::from_str(OLD_ACCOUNT).unwrap();
        account.id = id.into();
        account.email = email.into();
        account
    }

    fn block_credential_deletion(dir: &std::path::Path) {
        let conn = rusqlite::Connection::open(dir.join("local.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_credentials_delete BEFORE DELETE ON account_credentials
             BEGIN SELECT RAISE(ABORT, 'credential deletion blocked'); END;",
        )
        .unwrap();
    }

    #[test]
    fn account_credentials_add_and_remove_persist_together() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store
            .add_with_password(
                credential_account("new", "new@example.test"),
                "account-test-secret",
                &credentials,
            )
            .unwrap();
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .get("new")
                .is_some()
        );
        assert_eq!(
            credentials.load("new").unwrap().as_deref(),
            Some("account-test-secret")
        );
        let json = fs::read_to_string(dir.path().join("accounts.json")).unwrap();
        assert!(!json.contains("account-test-secret"));

        store.remove_with_password("new", &credentials).unwrap();
        assert!(store.accounts.is_empty());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .accounts
                .is_empty()
        );
        assert!(credentials.load("new").unwrap().is_none());
    }

    #[test]
    fn account_credentials_add_json_failure_restores_memory_and_removes_new_secret() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store.save().unwrap();
        fs::create_dir(dir.path().join("accounts.json.tmp")).unwrap();
        assert!(
            store
                .add_with_password(
                    credential_account("new", "new@example.test"),
                    "secret",
                    &credentials
                )
                .is_err()
        );
        assert!(store.accounts.is_empty());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .accounts
                .is_empty()
        );
        assert!(credentials.load("new").unwrap().is_none());
    }

    #[test]
    fn account_credentials_add_cleanup_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store.save().unwrap();
        fs::create_dir(dir.path().join("accounts.json.tmp")).unwrap();
        block_credential_deletion(dir.path());
        let error = store
            .add_with_password(
                credential_account("new", "new@example.test"),
                "secret",
                &credentials,
            )
            .unwrap_err();
        assert!(
            format!("{error:#}").contains("credential deletion blocked"),
            "{error:#}"
        );
        assert!(store.accounts.is_empty());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .accounts
                .is_empty()
        );
        assert_eq!(credentials.load("new").unwrap().as_deref(), Some("secret"));
    }

    #[test]
    fn account_credentials_add_database_failure_does_not_publish_account() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store.save().unwrap();
        let conn = rusqlite::Connection::open(dir.path().join("local.db")).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER refuse_credentials_insert BEFORE INSERT ON account_credentials
             BEGIN SELECT RAISE(ABORT, 'credential insert blocked'); END;",
        )
        .unwrap();
        assert!(
            store
                .add_with_password(
                    credential_account("new", "new@example.test"),
                    "secret",
                    &credentials,
                )
                .is_err()
        );
        assert!(store.accounts.is_empty());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .accounts
                .is_empty()
        );
        assert!(credentials.load("new").unwrap().is_none());
    }

    #[test]
    fn account_credentials_remove_database_failure_restores_account_json_and_memory() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store
            .add_with_password(
                credential_account("kept", "kept@example.test"),
                "secret",
                &credentials,
            )
            .unwrap();
        block_credential_deletion(dir.path());
        assert!(store.remove_with_password("kept", &credentials).is_err());
        assert!(store.get("kept").is_some());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .get("kept")
                .is_some()
        );
        assert_eq!(credentials.load("kept").unwrap().as_deref(), Some("secret"));
    }

    #[test]
    fn account_credentials_remove_json_failure_keeps_secret_and_account() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store
            .add_with_password(
                credential_account("kept", "kept@example.test"),
                "secret",
                &credentials,
            )
            .unwrap();
        fs::create_dir(dir.path().join("accounts.json.tmp")).unwrap();
        assert!(store.remove_with_password("kept", &credentials).is_err());
        assert!(store.get("kept").is_some());
        assert!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .get("kept")
                .is_some()
        );
        assert_eq!(credentials.load("kept").unwrap().as_deref(), Some("secret"));
    }

    #[test]
    fn account_credentials_add_rejects_duplicate_ids_emails_and_orphan_secret() {
        let dir = tempfile::tempdir().unwrap();
        let credentials = crate::credentials::Store::open(dir.path()).unwrap();
        let mut store = Store::load(dir.path().to_path_buf()).unwrap();
        store
            .add_with_password(
                credential_account("kept", "kept@example.test"),
                "original",
                &credentials,
            )
            .unwrap();
        for account in [
            credential_account("kept", "different@example.test"),
            credential_account("different", "KEPT@EXAMPLE.TEST"),
        ] {
            assert!(
                store
                    .add_with_password(account, "replacement", &credentials)
                    .is_err()
            );
        }
        credentials.save("orphan", "orphan-secret").unwrap();
        assert!(
            store
                .add_with_password(
                    credential_account("orphan", "orphan@example.test"),
                    "replacement",
                    &credentials
                )
                .is_err()
        );
        assert_eq!(store.accounts.len(), 1);
        assert_eq!(
            Store::load(dir.path().to_path_buf())
                .unwrap()
                .accounts
                .len(),
            1
        );
        assert_eq!(
            credentials.load("kept").unwrap().as_deref(),
            Some("original")
        );
        assert_eq!(
            credentials.load("orphan").unwrap().as_deref(),
            Some("orphan-secret")
        );
        assert!(credentials.load("different").unwrap().is_none());
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
    fn mail_cache_defaults_on_and_rejects_nonsense_sizes() {
        let dir = test_dir("body-cache");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("accounts.json"), format!("[{OLD_ACCOUNT}]")).unwrap();
        let mut store = Store::load(dir.clone()).unwrap();
        let cache = store.get("test").unwrap().body_cache;
        assert_eq!(
            cache,
            BodyCacheSettings {
                enabled: true,
                limit_mb: 2048,
                max_mail_mb: 5
            },
            "老配置默认开启：总量 2048 MB，单封 5 MB"
        );
        let wanted = BodyCacheSettings {
            enabled: false,
            limit_mb: 4096,
            max_mail_mb: 10,
        };
        store.set_body_cache("test", wanted).unwrap();
        assert_eq!(
            Store::load(dir.clone())
                .unwrap()
                .get("test")
                .unwrap()
                .body_cache,
            wanted
        );
        for bad in [
            BodyCacheSettings {
                limit_mb: 10,
                ..wanted
            },
            BodyCacheSettings {
                max_mail_mb: 0,
                ..wanted
            },
            BodyCacheSettings {
                max_mail_mb: 33,
                ..wanted
            },
            BodyCacheSettings {
                limit_mb: 100,
                max_mail_mb: 200,
                ..wanted
            },
        ] {
            assert!(store.set_body_cache("test", bad).is_err(), "{bad:?}");
        }
        assert_eq!(store.get("test").unwrap().body_cache, wanted, "出错时不改");
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

    #[test]
    fn archive_dirs_default_empty_and_persist_without_duplicates() {
        let dir = test_dir("archive-dirs");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("accounts.json"), format!("[{OLD_ACCOUNT}]")).unwrap();
        let mut store = Store::load(dir.clone()).unwrap();
        assert!(store.get("test").unwrap().archive_dirs.is_empty());
        assert!(store.add_archive_dir("test", "/data/mail").unwrap());
        assert!(!store.add_archive_dir("test", "/data/mail/").unwrap());
        assert!(store.add_archive_dir("test", "/data/other").unwrap());
        assert!(store.add_archive_dir("missing", "/data/mail").is_err());
        assert!(store.add_archive_dir("test", "  ").is_err());
        let loaded = Store::load(dir.clone()).unwrap();
        assert_eq!(
            loaded.get("test").unwrap().archive_dirs,
            vec!["/data/mail".to_string(), "/data/other".to_string()]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn archive_dir_changes_roll_back_on_save_failure() {
        let dir = test_dir("archive-rollback");
        let mut store = Store::load(dir.clone()).unwrap();
        store
            .accounts
            .push(serde_json::from_str(OLD_ACCOUNT).unwrap());
        store.add_archive_dir("test", "/data/mail").unwrap();
        fs::create_dir(dir.join("accounts.json.tmp")).unwrap();
        assert!(store.add_archive_dir("test", "/data/other").is_err());
        assert!(store.remove_archive_dir("test", "/data/mail").is_err());
        assert_eq!(
            store.get("test").unwrap().archive_dirs,
            vec!["/data/mail".to_string()]
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn removing_an_archive_dir_only_forgets_it() {
        let dir = test_dir("archive-remove");
        let mail = dir.join("mail");
        fs::create_dir_all(&mail).unwrap();
        fs::write(mail.join("a.eml"), b"x").unwrap();
        let mut store = Store::load(dir.clone()).unwrap();
        store
            .accounts
            .push(serde_json::from_str(OLD_ACCOUNT).unwrap());
        let path = mail.display().to_string();
        store.add_archive_dir("test", &path).unwrap();
        let acct = store
            .remove_archive_dir("test", &format!("{path}/"))
            .unwrap();
        assert!(acct.archive_dirs.is_empty());
        assert!(mail.join("a.eml").exists());
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn directory_paths_keep_roots_and_drive_roots() {
        assert_eq!(normalize_dir("/data/mail/"), "/data/mail");
        assert_eq!(normalize_dir("/"), "/");
        assert_eq!(normalize_dir("C:\\"), "C:\\");
        assert_eq!(normalize_dir("C:\\Mail\\"), "C:\\Mail");
    }
}
