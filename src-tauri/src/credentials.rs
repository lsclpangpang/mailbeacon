//! 本地凭据：SQLite 只保存 AES-256-GCM 密文，随机密钥独立存放。
//! 旧钥匙串仅用于首次迁移；读取失败、损坏或版本不兼容都不会静默回退。

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use ring::{
    aead::{self, Aad, LessSafeKey, Nonce, UnboundKey},
    digest,
    rand::{SecureRandom, SystemRandom},
};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

const VERSION: i64 = 1;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 12;
const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS account_credentials (
    account_id TEXT PRIMARY KEY NOT NULL,
    version INTEGER NOT NULL,
    nonce BLOB NOT NULL,
    ciphertext BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS pending_account_updates (
 account_id TEXT PRIMARY KEY NOT NULL, account_json TEXT NOT NULL,
 version INTEGER NOT NULL, nonce BLOB NOT NULL, ciphertext BLOB NOT NULL
)";

/// 由后台独占的本地凭据存储；没有把明文或密钥暴露给前端的接口。
pub struct Store {
    conn: Mutex<Connection>,
    key: LessSafeKey,
    key_path: PathBuf,
    key_fingerprint: digest::Digest,
}

impl Store {
    /// 打开共享数据库并加载密钥。已有密文但密钥缺失时拒绝生成新密钥。
    pub fn open(dir: &Path) -> Result<Self> {
        fs::create_dir_all(dir).context("无法创建本地配置目录")?;
        let mut conn = Connection::open(dir.join("local.db")).context("无法打开本地凭据数据库")?;
        conn.busy_timeout(Duration::from_secs(5))?;
        // 沿用共享库日志模式；这里切换 WAL 会与并发初始化的锁升级冲突。
        // 凭据提交成功后必须落盘，不能依赖其他连接的同步设置。
        conn.pragma_update(None, "synchronous", "FULL")?;
        // 两个首次启动的实例串行初始化，避免分别生成不同密钥。
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(SCHEMA)?;
        let has_records: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM account_credentials) OR EXISTS(SELECT 1 FROM pending_account_updates)",
            [],
            |r| r.get(0),
        )?;
        let (key, key_fingerprint) = load_or_create_key(dir, has_records)?;
        tx.commit()?;
        Ok(Self {
            conn: Mutex::new(conn),
            key,
            key_path: dir.join("secrets/credentials.key"),
            key_fingerprint,
        })
    }

    fn connection(&self) -> Result<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| anyhow!("本地凭据存储状态异常，请重启应用"))
    }

    /// 返回指定账户的授权码；只有确实没有记录时才返回 None。
    pub fn load(&self, id: &str) -> Result<Option<String>> {
        let conn = self.connection()?;
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_account_updates WHERE account_id=?1)",
            [id],
            |row| row.get(0),
        )?;
        if pending {
            bail!("此邮箱的连接更新尚未恢复完成，已暂停凭据读取；请重试连接或检查本地恢复记录");
        }
        self.read(&conn, id)
    }

    fn read(&self, conn: &Connection, id: &str) -> Result<Option<String>> {
        let record: Option<(i64, Vec<u8>, Vec<u8>)> = conn
            .query_row(
                "SELECT version, nonce, ciphertext FROM account_credentials WHERE account_id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .context("读取本地授权码失败")?;
        let Some((version, nonce, mut ciphertext)) = record else {
            return Ok(None);
        };
        if version != VERSION {
            bail!("本地授权码格式来自不支持的版本，请升级应用");
        }
        let nonce: [u8; NONCE_LEN] = nonce
            .try_into()
            .map_err(|_| anyhow!("本地授权码数据损坏，请恢复配套备份"))?;
        let aad = account_aad(id);
        let plaintext = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&aad),
                &mut ciphertext,
            )
            .map_err(|_| anyhow!("本地授权码校验失败，请恢复数据库和密钥的配套备份"))?;
        let password = String::from_utf8(plaintext.to_vec()).context("本地授权码格式无效")?;
        Ok(Some(password))
    }

    /// 加密保存并回读验证；写入或验证失败时回滚，保留原记录。
    pub fn save(&self, id: &str, password: &str) -> Result<()> {
        if id.is_empty() || password.is_empty() {
            bail!("账户标识和授权码不能为空");
        }
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow!("无法生成授权码加密随机数"))?;
        let aad = account_aad(id);
        let mut ciphertext = password.as_bytes().to_vec();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&aad),
                &mut ciphertext,
            )
            .map_err(|_| anyhow!("加密本地授权码失败"))?;
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        // 进程仍持有旧密钥时，磁盘文件可能已被误删或替换；此时不能发布新凭据。
        let current_key = read_key_bytes(&self.key_path)?;
        if digest::digest(&digest::SHA256, &current_key).as_ref() != self.key_fingerprint.as_ref() {
            bail!("本地授权码密钥已变化，未保存新密码，请恢复配套密钥后重启应用");
        }
        tx.execute(
            "INSERT INTO account_credentials (account_id, version, nonce, ciphertext) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(account_id) DO UPDATE SET version=excluded.version, nonce=excluded.nonce, ciphertext=excluded.ciphertext",
            params![id, VERSION, nonce.as_slice(), ciphertext],
        ).context("保存本地授权码失败")?;
        if self.read(&tx, id)?.as_deref() != Some(password) {
            bail!("本地授权码保存后校验失败");
        }
        tx.commit().context("提交本地授权码失败")?;
        Ok(())
    }

    /// 将新配置和加密的新授权码一起写入恢复日志，尚不替换活动凭据。
    pub fn stage_account_update(
        &self,
        id: &str,
        account_json: &str,
        password: Option<&str>,
    ) -> Result<()> {
        let password = match password {
            Some(p) => p.to_string(),
            None => self
                .load(id)?
                .ok_or_else(|| anyhow!("没有已保存的授权码，请填写新授权码"))?,
        };
        if id.is_empty() || password.is_empty() {
            bail!("账户标识和授权码不能为空");
        }
        let current_key = read_key_bytes(&self.key_path)?;
        if digest::digest(&digest::SHA256, &current_key).as_ref() != self.key_fingerprint.as_ref() {
            bail!("本地授权码密钥已变化，请恢复配套密钥后重启应用");
        }
        let mut nonce = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce)
            .map_err(|_| anyhow!("无法生成加密随机数"))?;
        let mut ciphertext = password.into_bytes();
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce),
                Aad::from(&account_aad(id)),
                &mut ciphertext,
            )
            .map_err(|_| anyhow!("加密新授权码失败"))?;
        self.connection()?.execute(
            "INSERT INTO pending_account_updates (account_id,account_json,version,nonce,ciphertext) VALUES (?1,?2,?3,?4,?5)",
            params![id, account_json, VERSION, nonce.as_slice(), ciphertext],
        ).context("保存账户更新恢复记录失败，请先恢复上次更新")?;
        Ok(())
    }

    pub fn pending_account_updates(&self) -> Result<Vec<(String, String)>> {
        let conn = self.connection()?;
        let mut statement = conn.prepare(
            "SELECT account_id, account_json FROM pending_account_updates ORDER BY account_id",
        )?;
        Ok(statement
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn cancel_account_update(&self, id: &str) -> Result<()> {
        self.connection()?.execute(
            "DELETE FROM pending_account_updates WHERE account_id=?1",
            [id],
        )?;
        Ok(())
    }

    /// accounts.json 已持久化后，原子发布密文并删除恢复日志。
    pub fn finish_account_update(&self, id: &str) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let changed = tx.execute(
            "INSERT INTO account_credentials (account_id,version,nonce,ciphertext) SELECT account_id,version,nonce,ciphertext FROM pending_account_updates WHERE account_id=?1
             ON CONFLICT(account_id) DO UPDATE SET version=excluded.version,nonce=excluded.nonce,ciphertext=excluded.ciphertext", [id])?;
        if changed != 1 || self.read(&tx, id)?.is_none() {
            bail!("无法确认账户更新后的授权码");
        }
        tx.execute(
            "DELETE FROM pending_account_updates WHERE account_id=?1",
            [id],
        )?;
        tx.commit()
            .context("提交新授权码失败，更新将在重试或重启时恢复")?;
        Ok(())
    }

    /// 删除本地凭据；失败时返回错误，调用方保留账号配置。
    pub fn remove(&self, id: &str) -> Result<()> {
        let mut conn = self.connection()?;
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("DELETE FROM account_credentials WHERE account_id=?1", [id])
            .context("删除本地授权码失败")?;
        tx.execute(
            "DELETE FROM pending_account_updates WHERE account_id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// 本地缺少条目才读取旧钥匙串，确认密文已保存后再交给收发信连接使用。
    /// 保留旧钥匙串条目供回退旧版使用，后续启动不会再访问它。
    pub fn load_or_migrate(
        &self,
        id: &str,
        legacy: impl FnOnce() -> Result<String>,
    ) -> Result<String> {
        if let Some(password) = self.load(id)? {
            return Ok(password);
        }
        let password = legacy().context("旧授权码迁移未完成，请允许钥匙串访问后重启应用重试")?;
        self.save(id, &password)
            .context("旧授权码尚未迁移到本地，原钥匙串凭据已保留，请排除存储故障后重启重试")?;
        Ok(password)
    }
}

fn account_aad(id: &str) -> Vec<u8> {
    // 固定用途和格式版本，防止密文被替换到另一账户或另一种记录。
    let mut aad = b"MailBeacon/account-credential/v1\0".to_vec();
    aad.extend_from_slice(id.as_bytes());
    aad
}

fn load_or_create_key(dir: &Path, has_records: bool) -> Result<(LessSafeKey, digest::Digest)> {
    let secrets = dir.join("secrets");
    match fs::symlink_metadata(&secrets) {
        Ok(meta) if !meta.is_dir() || crate::private_fs::is_link(&meta) => {
            bail!("本地密钥目录无效，请恢复配套备份")
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if has_records {
                bail!("本地授权码密钥丢失，请恢复 secrets/credentials.key 的配套备份");
            }
            crate::private_fs::create_private_dir(&secrets).context("创建本地密钥目录失败")?;
            #[cfg(unix)]
            File::open(dir)?
                .sync_all()
                .context("同步本地密钥目录失败")?;
        }
        Err(e) => return Err(e).context("读取本地密钥目录失败"),
    }
    crate::private_fs::create_private_dir(&secrets)?;
    let path = secrets.join("credentials.key");
    match fs::symlink_metadata(&path) {
        Ok(meta) if !meta.is_file() || crate::private_fs::is_link(&meta) => {
            bail!("本地密钥文件无效，请恢复配套备份")
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if has_records {
                bail!("本地授权码密钥丢失，请恢复 secrets/credentials.key 的配套备份");
            }
            let mut key = [0u8; KEY_LEN];
            SystemRandom::new()
                .fill(&mut key)
                .map_err(|_| anyhow!("无法生成本地随机密钥"))?;
            // 写完且同步后再原子发布，崩溃不会留下半份密钥，也绝不覆盖现有文件。
            let mut temp =
                tempfile::NamedTempFile::new_in(&secrets).context("创建本地密钥临时文件失败")?;
            crate::private_fs::protect_file(temp.path())?;
            temp.write_all(&key)?;
            temp.as_file().sync_all()?;
            temp.persist_noclobber(&path)
                .map_err(|e| e.error)
                .context("保存本地随机密钥失败")?;
            #[cfg(unix)]
            File::open(&secrets)?
                .sync_all()
                .context("同步本地密钥失败")?;
        }
        Err(e) => return Err(e).context("读取本地密钥失败"),
    }
    let bytes = read_key_bytes(&path)?;
    let key =
        UnboundKey::new(&aead::AES_256_GCM, &bytes).map_err(|_| anyhow!("初始化本地加密失败"))?;
    Ok((
        LessSafeKey::new(key),
        digest::digest(&digest::SHA256, &bytes),
    ))
}

fn read_key_bytes(path: &Path) -> Result<Vec<u8>> {
    let meta =
        fs::symlink_metadata(path).context("读取本地授权码密钥失败，请恢复配套密钥后重启应用")?;
    if !meta.is_file() || crate::private_fs::is_link(&meta) {
        bail!("本地密钥文件无效，请恢复配套备份");
    }
    crate::private_fs::protect_file(path)?;
    let file = File::open(path).context("打开本地密钥失败")?;
    let mut bytes = Vec::new();
    file.take((KEY_LEN + 1) as u64).read_to_end(&mut bytes)?;
    if bytes.len() != KEY_LEN {
        bail!("本地授权码密钥损坏，请恢复配套备份，原文件未被覆盖");
    }
    Ok(bytes)
}

#[cfg(test)]
#[path = "credentials_tests.rs"]
mod tests;
