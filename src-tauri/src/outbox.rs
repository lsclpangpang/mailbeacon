//! 定时发送：到时间之前，组好的邮件原文存在本地「定时发送」里，后台到点用 SMTP 发出。
//! MailBeacon 关着的时候发不了，下次打开时补发已经到时间的邮件。

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::{drafts::safe_account, imap::Summary};

/// 界面里的虚拟文件夹
pub const OUTBOX: &str = "__outbox";
/// 被服务器拒绝这么多次后不再自动重试，等用户处理（断网、超时不算次数）
pub const MAX_ATTEMPTS: u32 = 5;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Item {
    pub account_id: String,
    pub id: u32,
    /// 计划发送的时间（毫秒）
    pub send_at: i64,
    pub created: i64,
    pub subject: String,
    /// 显示用的收件人
    pub to: String,
    /// 信封上的收件人（含抄送、密送）
    pub recipients: Vec<String>,
    pub message_id: String,
    /// 被服务器拒绝的次数（到 MAX_ATTEMPTS 就停）
    pub attempts: u32,
    /// 一共试过几次（决定重试间隔）
    pub tries: u32,
    pub last_error: Option<String>,
    /// 失败后下次重试的时间
    pub next_try: i64,
    /// 正在交给 SMTP：发送前先记下，中途被关掉时重启后不自动重发
    pub sending: bool,
}

impl Item {
    pub fn due(&self, now: i64) -> bool {
        !self.sending && self.send_at <= now && self.next_try <= now && self.attempts < MAX_ATTEMPTS
    }

    /// 记下一次失败；下次重试的间隔逐渐拉长（1、5、15、60 分钟，之后每小时）。
    /// `rejected` 表示服务器明确拒收：这种才计入放弃的次数，断网、超时一直按小时重试。
    pub fn failed(&mut self, error: &str, now: i64, rejected: bool) {
        self.sending = false;
        self.tries += 1;
        if rejected {
            self.attempts += 1;
        }
        self.last_error = Some(error.to_string());
        let minutes = [1, 5, 15, 60][(self.tries as usize - 1).min(3)];
        self.next_try = now + minutes * 60 * 1000;
    }

    /// 在列表里显示成一封邮件；日期是计划发送的时间。
    pub fn summary(&self, from_name: &str, from_addr: &str) -> Summary {
        Summary {
            account_id: self.account_id.clone(),
            folder: OUTBOX.into(),
            uid: self.id,
            from_name: from_name.into(),
            from_addr: from_addr.into(),
            to: self.to.clone(),
            subject: self.subject.clone(),
            date: self.send_at,
            seen: true,
            message_id: self.message_id.trim_matches(['<', '>']).to_string(),
            ..Default::default()
        }
    }
}

pub struct Outbox {
    dir: PathBuf,
    /// 读写文件时串行，避免后台发送和界面操作同时改同一封
    lock: std::sync::Mutex<()>,
}

/// 原子写入：先写临时文件再改名，断电时不会留下半个文件。
fn write(path: &std::path::Path, data: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

impl Outbox {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            lock: Default::default(),
        }
    }

    fn account_dir(&self, account_id: &str) -> Result<PathBuf> {
        Ok(self.dir.join(safe_account(account_id)?))
    }

    fn file(&self, account_id: &str, id: u32, ext: &str) -> Result<PathBuf> {
        Ok(self.account_dir(account_id)?.join(format!("{id}.{ext}")))
    }

    /// 存一封定时邮件：发出用的原文，以及取消定时后转成草稿用的原文（保留密送）。
    pub fn add(&self, mut item: Item, send_raw: &[u8], draft_raw: &[u8]) -> Result<Item> {
        let _guard = self.lock.lock().unwrap();
        let dir = self.account_dir(&item.account_id)?;
        std::fs::create_dir_all(&dir)?;
        // 编号只增不减，取消或发出后也不复用；计数文件坏了也不能和现有的重号
        let counter = dir.join("next_id");
        let recorded: u32 = std::fs::read_to_string(&counter)
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(1);
        let existing = std::fs::read_dir(&dir)?
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.split('.').next()?.parse::<u32>().ok()
            })
            .max()
            .unwrap_or(0);
        let next = recorded.max(existing + 1);
        item.id = next;
        if item.created == 0 {
            item.created = chrono::Utc::now().timestamp_millis();
        }
        write(&counter, (next + 1).to_string().as_bytes())?;
        write(&dir.join(format!("{next}.eml")), send_raw)?;
        write(&dir.join(format!("{next}.draft")), draft_raw)?;
        write(
            &dir.join(format!("{next}.json")),
            &serde_json::to_vec(&item)?,
        )?;
        Ok(item)
    }

    pub fn get(&self, account_id: &str, id: u32) -> Result<Item> {
        let data = std::fs::read(self.file(account_id, id, "json")?)
            .map_err(|_| anyhow!("这封定时邮件已经发出或取消"))?;
        Ok(serde_json::from_slice(&data)?)
    }

    pub fn raw(&self, account_id: &str, id: u32) -> Result<Vec<u8>> {
        std::fs::read(self.file(account_id, id, "eml")?)
            .map_err(|_| anyhow!("这封定时邮件已经发出或取消"))
    }

    pub fn draft_raw(&self, account_id: &str, id: u32) -> Result<Vec<u8>> {
        std::fs::read(self.file(account_id, id, "draft")?)
            .map_err(|_| anyhow!("这封定时邮件已经发出或取消"))
    }

    /// 在锁里读出最新的内容再修改、写回，不会覆盖别处刚做的改动（例如发送中改了时间）。
    pub fn modify(&self, account_id: &str, id: u32, f: impl FnOnce(&mut Item)) -> Result<Item> {
        let _guard = self.lock.lock().unwrap();
        let path = self.file(account_id, id, "json")?;
        let data = std::fs::read(&path).map_err(|_| anyhow!("这封定时邮件已经发出或取消"))?;
        let mut item: Item = serde_json::from_slice(&data)?;
        f(&mut item);
        write(&path, &serde_json::to_vec(&item)?)?;
        Ok(item)
    }

    /// 启动时：上次发送到一半被关掉的邮件可能已经发出了，不自动重发，等用户看过「已发送」再决定。
    pub fn recover_interrupted(&self) -> Result<usize> {
        let mut n = 0;
        for item in self.list(None)?.into_iter().filter(|i| i.sending) {
            self.modify(&item.account_id, item.id, |i| {
                i.sending = false;
                i.attempts = MAX_ATTEMPTS;
                i.last_error = Some(
                    "上次发送时 MailBeacon 被关闭，邮件可能已经发出。请先看看「已发送」，确实没发出再点「立即发送」".into(),
                );
            })?;
            n += 1;
        }
        Ok(n)
    }

    pub fn remove(&self, account_id: &str, id: u32) -> Result<()> {
        let _guard = self.lock.lock().unwrap();
        // 先删描述文件：中途失败时剩下的原文不会再被当成待发邮件
        for ext in ["json", "eml", "draft"] {
            match std::fs::remove_file(self.file(account_id, id, ext)?) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }

    /// 某个账号（或全部账号）的定时邮件，按计划时间从早到晚。
    pub fn list(&self, account_id: Option<&str>) -> Result<Vec<Item>> {
        let accounts: Vec<String> = match account_id {
            Some(a) => vec![safe_account(a)?.to_string()],
            None => match std::fs::read_dir(&self.dir) {
                Ok(entries) => entries
                    .flatten()
                    .filter(|e| e.path().is_dir())
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => return Err(e.into()),
            },
        };
        let mut out = Vec::new();
        for account in accounts {
            let Ok(entries) = std::fs::read_dir(self.account_dir(&account)?) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_none_or(|e| e != "json") {
                    continue;
                }
                match std::fs::read(&path)
                    .ok()
                    .and_then(|d| serde_json::from_slice::<Item>(&d).ok())
                {
                    Some(item) if item.account_id == account => out.push(item),
                    _ => eprintln!("跳过损坏的定时邮件：{}", path.display()),
                }
            }
        }
        out.sort_by_key(|i| (i.send_at, i.id));
        Ok(out)
    }

    /// 已经到时间、该发出的邮件。
    pub fn due(&self, now: i64) -> Result<Vec<Item>> {
        Ok(self
            .list(None)?
            .into_iter()
            .filter(|i| i.due(now))
            .collect())
    }

    pub fn remove_account(&self, account_id: &str) {
        let _guard = self.lock.lock().unwrap();
        if let Ok(dir) = self.account_dir(account_id) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outbox() -> (Outbox, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Outbox::new(dir.path().to_path_buf()), dir)
    }

    fn item(account: &str, send_at: i64) -> Item {
        Item {
            account_id: account.into(),
            send_at,
            subject: "周报".into(),
            to: "老板".into(),
            recipients: vec!["boss@x.com".into()],
            message_id: "<m1@x.com>".into(),
            ..Default::default()
        }
    }

    #[test]
    fn scheduled_mail_is_kept_with_both_send_and_draft_copies() {
        let (o, dir) = outbox();
        let a = o.add(item("a", 1000), b"send", b"draft").unwrap();
        let b = o.add(item("a", 500), b"send2", b"draft2").unwrap();
        assert_eq!((a.id, b.id), (1, 2));
        assert_eq!(o.raw("a", 1).unwrap(), b"send");
        assert_eq!(o.draft_raw("a", 1).unwrap(), b"draft");
        let ids: Vec<_> = o.list(Some("a")).unwrap().iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![2, 1], "按计划时间排");
        assert!(o.list(Some("b")).unwrap().is_empty());
        // 重新打开（重启）后还在
        let again = Outbox::new(dir.path().to_path_buf());
        assert_eq!(again.get("a", 1).unwrap().send_at, 1000);
        again.remove("a", 1).unwrap();
        assert!(again.get("a", 1).is_err());
        let c = again.add(item("a", 1), b"s", b"d").unwrap();
        assert_eq!(c.id, 3, "编号不复用，界面上不会把新邮件当成旧的");
    }

    #[test]
    fn due_mail_respects_retry_backoff_and_gives_up_eventually() {
        let (o, _dir) = outbox();
        o.add(item("a", 1000), b"s", b"d").unwrap();
        o.add(item("b", 5000), b"s", b"d").unwrap();
        assert!(o.due(999).unwrap().is_empty());
        let due = o.due(1000).unwrap();
        assert_eq!(due.len(), 1);
        let it = o
            .modify("a", due[0].id, |i| i.failed("连接超时", 1000, true))
            .unwrap();
        let due_a = |now| {
            o.due(now)
                .unwrap()
                .into_iter()
                .filter(|i| i.account_id == "a")
                .count()
        };
        assert_eq!(due_a(1000 + 59_000), 0, "一分钟后再试");
        assert_eq!(due_a(1000 + 60_000), 1);
        for _ in 1..MAX_ATTEMPTS {
            o.modify("a", it.id, |i| i.failed("连接超时", 0, true))
                .unwrap();
        }
        assert!(
            o.due(i64::MAX / 2)
                .unwrap()
                .iter()
                .all(|i| i.account_id == "b"),
            "失败太多次就不再自动重试"
        );
        assert_eq!(
            o.get("a", 1).unwrap().last_error.as_deref(),
            Some("连接超时")
        );
    }

    #[test]
    fn a_failure_never_overwrites_a_newer_send_time() {
        let (o, _dir) = outbox();
        let it = o.add(item("a", 1000), b"s", b"d").unwrap();
        // 发送进行中用户改了时间，随后这次发送失败
        o.modify("a", it.id, |i| i.send_at = 9_999_999).unwrap();
        o.modify("a", it.id, |i| i.failed("连接超时", 1000, false))
            .unwrap();
        let now = o.get("a", it.id).unwrap();
        assert_eq!(now.send_at, 9_999_999);
        assert_eq!(now.last_error.as_deref(), Some("连接超时"));
    }

    #[test]
    fn connection_problems_keep_retrying_hourly_but_rejections_give_up() {
        let mut it = item("a", 0);
        for _ in 0..20 {
            it.failed("网络不可用", 0, false);
        }
        assert!(it.attempts < MAX_ATTEMPTS, "离线不算失败次数");
        assert_eq!(it.next_try, 60 * 60 * 1000, "最长一小时重试一次");
        let mut it = item("a", 0);
        for _ in 0..MAX_ATTEMPTS {
            it.failed("收件人被拒绝", 0, true);
        }
        assert!(!it.due(i64::MAX / 2));
    }

    #[test]
    fn mail_being_sent_is_not_sent_twice_and_an_interrupted_send_waits_for_the_user() {
        let (o, dir) = outbox();
        let it = o.add(item("a", 1000), b"s", b"d").unwrap();
        o.modify("a", it.id, |i| i.sending = true).unwrap();
        assert!(o.due(5000).unwrap().is_empty(), "正在发送的不再取出");
        // 发送途中程序被关掉：重启后不自动重发，让用户决定
        let again = Outbox::new(dir.path().to_path_buf());
        assert_eq!(again.recover_interrupted().unwrap(), 1);
        let held = again.get("a", it.id).unwrap();
        assert!(!held.sending && !held.due(i64::MAX / 2));
        assert!(held.last_error.unwrap().contains("可能已经发出"));
    }

    #[test]
    fn a_damaged_id_counter_never_reuses_a_pending_id() {
        let (o, dir) = outbox();
        o.add(item("a", 1), b"one", b"d").unwrap();
        o.add(item("a", 2), b"two", b"d").unwrap();
        std::fs::write(dir.path().join("a").join("next_id"), "").unwrap();
        let c = o.add(item("a", 3), b"three", b"d").unwrap();
        assert_eq!(c.id, 3);
        assert_eq!(o.raw("a", 1).unwrap(), b"one");
    }

    #[test]
    fn shown_as_a_mail_in_the_scheduled_folder() {
        let s = item("a", 1234).summary("我", "me@x.com");
        assert_eq!(s.folder, OUTBOX);
        assert_eq!(s.date, 1234);
        assert_eq!(s.to, "老板");
        assert_eq!(s.message_id, "m1@x.com");
    }

    #[test]
    fn bad_account_ids_and_broken_files_are_handled() {
        let (o, dir) = outbox();
        assert!(o.add(item("../x", 1), b"s", b"d").is_err());
        o.add(item("a", 1), b"s", b"d").unwrap();
        std::fs::write(dir.path().join("a").join("9.json"), "{broken").unwrap();
        assert_eq!(o.list(Some("a")).unwrap().len(), 1, "坏文件跳过");
        o.remove_account("a");
        assert!(o.list(None).unwrap().is_empty());
    }
}
