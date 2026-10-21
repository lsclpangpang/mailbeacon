//! 应用偏好：开机启动、托盘图标、新邮件通知（内容、免打扰、静音账号、额外提醒的文件夹）。

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// 通知里显示多少内容。
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotifyContent {
    /// 发件人和主题
    #[default]
    Full,
    /// 只显示发件人
    Sender,
    /// 只提示有新邮件
    Hidden,
}

/// 一条系统通知的展示文案，以及点击后要打开的邮件定位。
/// `uid` 为 None 时只切到账号/文件夹（如「还有 N 封」汇总）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyPayload {
    pub title: String,
    pub body: String,
    pub account_id: String,
    pub folder: String,
    pub uid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    pub autostart: bool,
    pub tray: bool,
    pub notify: bool,
    pub notify_content: NotifyContent,
    pub dnd: bool,
    pub dnd_start: String,
    pub dnd_end: String,
    pub muted_accounts: Vec<String>,
    /// 账号 → 收件箱之外也要提醒的文件夹
    pub notify_folders: BTreeMap<String, Vec<String>>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            autostart: false,
            tray: true,
            notify: true,
            notify_content: NotifyContent::Full,
            dnd: false,
            dnd_start: "22:00".into(),
            dnd_end: "08:00".into(),
            muted_accounts: Vec::new(),
            notify_folders: BTreeMap::new(),
        }
    }
}

/// 本地时间是一天中的第几分钟（免打扰用）。
pub fn local_minutes() -> u32 {
    use chrono::Timelike;
    let now = chrono::Local::now();
    now.hour() * 60 + now.minute()
}

/// "HH:MM" → 一天中的分钟数。
fn minutes(value: &str) -> Option<u32> {
    let (h, m) = value.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

impl Prefs {
    /// 规范化并校验界面提交的设置。
    pub fn validated(mut self) -> Result<Self> {
        self.dnd_start = self.dnd_start.trim().to_string();
        self.dnd_end = self.dnd_end.trim().to_string();
        if minutes(&self.dnd_start).is_none() || minutes(&self.dnd_end).is_none() {
            bail!("免打扰时间格式应为 HH:MM");
        }
        if self.dnd && self.dnd_start == self.dnd_end {
            bail!("免打扰的开始和结束时间不能相同");
        }
        let mut muted = Vec::new();
        for id in self.muted_accounts.drain(..) {
            let id = id.trim().to_string();
            if !id.is_empty() && !muted.contains(&id) {
                muted.push(id);
            }
        }
        self.muted_accounts = muted;
        for folders in self.notify_folders.values_mut() {
            let mut kept: Vec<String> = Vec::new();
            for folder in folders.drain(..) {
                // 收件箱本来就会提醒
                if !folder.is_empty()
                    && !folder.eq_ignore_ascii_case("INBOX")
                    && !kept.contains(&folder)
                {
                    kept.push(folder);
                }
            }
            *folders = kept;
        }
        self.notify_folders.retain(|_, folders| !folders.is_empty());
        Ok(self)
    }

    /// 这个账号除收件箱外还要提醒的文件夹。
    pub fn extra_folders(&self, account_id: &str) -> &[String] {
        self.notify_folders
            .get(account_id)
            .map_or(&[], Vec::as_slice)
    }

    /// 账号被移除后清掉它的设置；有变化时返回 true。
    pub fn forget_account(&mut self, account_id: &str) -> bool {
        let before = self.muted_accounts.len();
        self.muted_accounts.retain(|a| a != account_id);
        let removed = self.notify_folders.remove(account_id).is_some();
        removed || before != self.muted_accounts.len()
    }

    /// `now` 是一天中的分钟数；开始晚于结束表示跨过午夜。
    pub fn quiet_at(&self, now: u32) -> bool {
        if !self.dnd {
            return false;
        }
        let (Some(start), Some(end)) = (minutes(&self.dnd_start), minutes(&self.dnd_end)) else {
            return false;
        };
        if start <= end {
            start <= now && now < end
        } else {
            now >= start || now < end
        }
    }

    /// 一批新邮件要弹出的通知（标题、正文）。`folder` 非空时注明来自哪个文件夹。
    /// 超过 3 封时前两封单独提醒，其余合成一条，避免刷屏。
    #[cfg(test)]
    pub fn notifications(
        &self,
        account_id: &str,
        folder: &str,
        mails: &[(String, String)],
        now: u32,
    ) -> Vec<(String, String)> {
        self.notifications_for(account_id, "", folder, mails, now)
    }

    /// 同上；`account` 是账号的显示名（邮箱地址），写在汇总通知里。
    pub fn notifications_for(
        &self,
        account_id: &str,
        account: &str,
        folder: &str,
        mails: &[(String, String)],
        now: u32,
    ) -> Vec<(String, String)> {
        let Some((first_from, first_subject)) = mails.first() else {
            return Vec::new();
        };
        if self
            .notification(account_id, first_from, first_subject, now)
            .is_none()
        {
            return Vec::new();
        }
        if self.notify_content == NotifyContent::Hidden {
            let body = if mails.len() == 1 {
                "你有新邮件".to_string()
            } else {
                format!("你有 {} 封新邮件", mails.len())
            };
            return vec![("MailBeacon".into(), body)];
        }
        let shown = if mails.len() > 3 { 2 } else { mails.len() };
        let mut out: Vec<_> = mails[..shown]
            .iter()
            .filter_map(|(from, subject)| self.notification(account_id, from, subject, now))
            .map(|(title, body)| {
                if folder.is_empty() {
                    (title, body)
                } else {
                    (format!("{title} · {folder}"), body)
                }
            })
            .collect();
        if mails.len() > shown {
            let title = if account.is_empty() {
                "MailBeacon".to_string()
            } else {
                format!("MailBeacon · {account}")
            };
            out.push((
                title,
                if folder.is_empty() {
                    format!("还有 {} 封新邮件", mails.len() - shown)
                } else {
                    format!("{folder} 里还有 {} 封新邮件", mails.len() - shown)
                },
            ));
        }
        out
    }

    /// 同上，但每条通知附带点击后打开的邮件定位（账号 / 文件夹 / uid）。
    /// `mails` 为 (from, subject, uid)；`folder` 为空时 payload.folder 为 "INBOX"。
    /// RED stub：尚未实现，故意返回错误结果，供 TDD 失败。
    pub fn notification_payloads(
        &self,
        account_id: &str,
        account_email: &str,
        folder: &str,
        mails: &[(String, String, u32)],
        now: u32,
    ) -> Vec<NotifyPayload> {
        let _ = (account_id, account_email, folder, mails, now, self);
        // Intentionally wrong until the open-from-notification feature is implemented.
        vec![NotifyPayload {
            title: "stub".into(),
            body: "stub".into(),
            account_id: String::new(),
            folder: String::new(),
            uid: Some(0),
        }]
    }

    /// 一封新邮件的通知标题和正文；不该通知时返回 None。
    pub fn notification(
        &self,
        account_id: &str,
        from: &str,
        subject: &str,
        now: u32,
    ) -> Option<(String, String)> {
        if !self.notify || self.muted_accounts.iter().any(|a| a == account_id) || self.quiet_at(now)
        {
            return None;
        }
        let from = if from.trim().is_empty() {
            "(未知发件人)"
        } else {
            from
        };
        let subject = if subject.trim().is_empty() {
            "(无主题)"
        } else {
            subject
        };
        Some(match self.notify_content {
            NotifyContent::Full => (from.to_string(), subject.to_string()),
            NotifyContent::Sender => ("新邮件".into(), format!("来自 {from}")),
            NotifyContent::Hidden => ("MailBeacon".into(), "你有新邮件".into()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_validation() {
        let p = Prefs::default();
        assert!(!p.autostart && p.tray && p.notify && !p.dnd);
        assert_eq!(p.notify_content, NotifyContent::Full);
        let mut bad = p.clone();
        bad.dnd_start = "25:00".into();
        assert!(bad.clone().validated().is_err());
        bad.dnd_start = "7:5".into();
        assert!(bad.validated().is_err());
        let mut ok = p.clone();
        ok.dnd_start = " 23:30 ".into();
        assert_eq!(ok.validated().unwrap().dnd_start, "23:30");
        // 旧版或手改的配置缺字段时用默认值
        let parsed: Prefs = serde_json::from_str(r#"{"notify":false}"#).unwrap();
        assert!(!parsed.notify && parsed.tray);
    }

    #[test]
    fn quiet_hours_wrap_past_midnight() {
        let mut p = Prefs::default();
        p.dnd = true;
        p.dnd_start = "22:00".into();
        p.dnd_end = "08:00".into();
        assert!(p.quiet_at(23 * 60));
        assert!(p.quiet_at(7 * 60 + 59));
        assert!(!p.quiet_at(8 * 60));
        assert!(!p.quiet_at(12 * 60));
        p.dnd_start = "12:00".into();
        p.dnd_end = "13:00".into();
        assert!(p.quiet_at(12 * 60 + 30));
        assert!(!p.quiet_at(13 * 60));
        p.dnd = false;
        assert!(!p.quiet_at(12 * 60 + 30));
    }

    #[test]
    fn notification_text_respects_content_mode_mute_and_quiet_hours() {
        let mut p = Prefs::default();
        let text = |p: &Prefs| p.notification("a", "张三", "季度报价", 10 * 60);
        assert_eq!(text(&p), Some(("张三".into(), "季度报价".into())));
        p.notify_content = NotifyContent::Sender;
        assert_eq!(text(&p), Some(("新邮件".into(), "来自 张三".into())));
        p.notify_content = NotifyContent::Hidden;
        assert_eq!(text(&p), Some(("MailBeacon".into(), "你有新邮件".into())));
        p.muted_accounts = vec!["a".into()];
        assert_eq!(text(&p), None);
        p.muted_accounts.clear();
        p.notify = false;
        assert_eq!(text(&p), None);
        p.notify = true;
        p.dnd = true;
        p.dnd_start = "09:00".into();
        p.dnd_end = "18:00".into();
        assert_eq!(text(&p), None);
        p.notify_content = NotifyContent::Full;
        p.dnd = false;
        assert_eq!(
            p.notification("a", "", "", 0),
            Some(("(未知发件人)".into(), "(无主题)".into()))
        );
    }

    #[test]
    fn validation_cleans_account_lists_and_never_lists_the_inbox_twice() {
        let mut p = Prefs::default();
        p.muted_accounts = vec!["a".into(), "a".into(), " ".into(), "b".into()];
        p.notify_folders.insert(
            "a".into(),
            vec!["INBOX".into(), "Work".into(), "Work".into(), "".into()],
        );
        p.notify_folders.insert("b".into(), vec!["inbox".into()]);
        let p = p.validated().unwrap();
        assert_eq!(p.muted_accounts, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(
            p.notify_folders.get("a").unwrap(),
            &vec!["Work".to_string()]
        );
        assert!(
            !p.notify_folders.contains_key("b"),
            "只剩收件箱的账号不保留"
        );
        assert_eq!(p.extra_folders("a"), &["Work".to_string()]);
        assert!(p.extra_folders("zzz").is_empty());
    }

    #[test]
    fn forgetting_an_account_drops_its_notification_settings() {
        let mut p = Prefs::default();
        p.muted_accounts = vec!["a".into(), "b".into()];
        p.notify_folders.insert("a".into(), vec!["Work".into()]);
        assert!(p.forget_account("a"));
        assert_eq!(p.muted_accounts, vec!["b".to_string()]);
        assert!(p.notify_folders.is_empty());
        assert!(!p.forget_account("a"), "没有变化时不需要保存");
    }

    #[test]
    fn many_new_mails_collapse_into_a_summary_notification() {
        let p = Prefs::default();
        let mail = |from: &str, subject: &str| (from.to_string(), subject.to_string());
        let one = p.notifications("a", "", &[mail("张三", "报价")], 600);
        assert_eq!(one, vec![("张三".to_string(), "报价".to_string())]);
        let many: Vec<_> = (0..5)
            .map(|i| mail(&format!("人{i}"), &format!("主题{i}")))
            .collect();
        let shown = p.notifications("a", "", &many, 600);
        assert_eq!(shown.len(), 3);
        assert_eq!(
            shown[2],
            ("MailBeacon".to_string(), "还有 3 封新邮件".to_string())
        );
        let labelled = p.notifications_for("a", "me@x.com", "工作", &many, 600);
        assert_eq!(
            labelled[2],
            (
                "MailBeacon · me@x.com".to_string(),
                "工作 里还有 3 封新邮件".to_string()
            ),
            "汇总通知说明是哪个账号、哪个文件夹"
        );
        // 额外提醒的文件夹在标题里注明来源
        let folder = p.notifications("a", "工作", &[mail("张三", "报价")], 600);
        assert_eq!(
            folder,
            vec![("张三 · 工作".to_string(), "报价".to_string())]
        );
        let mut hidden = p.clone();
        hidden.notify_content = NotifyContent::Hidden;
        assert_eq!(
            hidden.notifications("a", "", &many, 600),
            vec![("MailBeacon".to_string(), "你有 5 封新邮件".to_string())],
            "隐藏内容时只发一条汇总"
        );
        let mut muted = p.clone();
        muted.muted_accounts = vec!["a".into()];
        assert!(muted.notifications("a", "", &many, 600).is_empty());
    }

    #[test]
    fn quiet_hours_need_different_start_and_end_when_enabled() {
        let mut p = Prefs::default();
        p.dnd_start = "09:00".into();
        p.dnd_end = "09:00".into();
        assert!(p.clone().validated().is_ok(), "关闭时不检查");
        p.dnd = true;
        assert!(p.validated().is_err());
    }

    #[test]
    fn prefs_round_trip_through_the_local_database() {
        let db = crate::contacts::Db::open_in_memory().unwrap();
        assert_eq!(db.load_prefs().unwrap(), Prefs::default());
        let mut p = Prefs::default();
        p.autostart = true;
        p.notify_folders.insert("a".into(), vec!["Work".into()]);
        db.save_prefs(&p).unwrap();
        assert_eq!(db.load_prefs().unwrap(), p);
    }

    fn mail3(from: &str, subject: &str, uid: u32) -> (String, String, u32) {
        (from.to_string(), subject.to_string(), uid)
    }

    #[test]
    fn notification_payloads_attach_uid_and_open_target_per_mail() {
        let p = Prefs::default();
        let one = p.notification_payloads(
            "acct-1",
            "me@x.com",
            "",
            &[mail3("张三", "报价", 42)],
            600,
        );
        assert_eq!(
            one,
            vec![NotifyPayload {
                title: "张三".into(),
                body: "报价".into(),
                account_id: "acct-1".into(),
                folder: "INBOX".into(),
                uid: Some(42),
            }],
            "单封通知带 uid；空 folder 记为 INBOX"
        );

        let labelled = p.notification_payloads(
            "acct-1",
            "me@x.com",
            "工作",
            &[mail3("张三", "报价", 7)],
            600,
        );
        assert_eq!(labelled[0].title, "张三 · 工作");
        assert_eq!(labelled[0].folder, "工作");
        assert_eq!(labelled[0].uid, Some(7));
        assert_eq!(labelled[0].account_id, "acct-1");
    }

    #[test]
    fn notification_payloads_collapse_summary_has_no_uid() {
        let p = Prefs::default();
        let many: Vec<_> = (0..5)
            .map(|i| mail3(&format!("人{i}"), &format!("主题{i}"), 100 + i))
            .collect();
        let shown = p.notification_payloads("a", "me@x.com", "", &many, 600);
        assert_eq!(shown.len(), 3, "前两封单独 + 一条汇总");
        assert_eq!(shown[0].uid, Some(100));
        assert_eq!(shown[0].account_id, "a");
        assert_eq!(shown[0].folder, "INBOX");
        assert_eq!(shown[1].uid, Some(101));
        assert_eq!(
            shown[2],
            NotifyPayload {
                title: "MailBeacon · me@x.com".into(),
                body: "还有 3 封新邮件".into(),
                account_id: "a".into(),
                folder: "INBOX".into(),
                uid: None,
            },
            "汇总通知无 uid，点击只切到账号/文件夹"
        );

        let foldered = p.notification_payloads("a", "me@x.com", "工作", &many, 600);
        assert_eq!(foldered[2].body, "工作 里还有 3 封新邮件");
        assert_eq!(foldered[2].folder, "工作");
        assert_eq!(foldered[2].uid, None);
    }

    #[test]
    fn notification_payloads_hidden_content_still_opens_first_mail() {
        let mut p = Prefs::default();
        p.notify_content = NotifyContent::Hidden;
        let many: Vec<_> = (0..5)
            .map(|i| mail3(&format!("人{i}"), &format!("主题{i}"), 200 + i))
            .collect();
        let shown = p.notification_payloads("a", "me@x.com", "", &many, 600);
        assert_eq!(
            shown,
            vec![NotifyPayload {
                title: "MailBeacon".into(),
                body: "你有 5 封新邮件".into(),
                account_id: "a".into(),
                folder: "INBOX".into(),
                uid: Some(200),
            }],
            "隐藏内容仍带首封 uid，点击可打开一封信"
        );

        let one = p.notification_payloads(
            "a",
            "me@x.com",
            "归档",
            &[mail3("李四", "你好", 9)],
            600,
        );
        assert_eq!(one[0].body, "你有新邮件");
        assert_eq!(one[0].uid, Some(9));
        assert_eq!(one[0].folder, "归档");
    }

    #[test]
    fn notification_payloads_respect_mute_and_quiet_hours() {
        let p = Prefs::default();
        let mails = [mail3("张三", "报价", 1)];
        let mut muted = p.clone();
        muted.muted_accounts = vec!["a".into()];
        assert!(
            muted
                .notification_payloads("a", "me@x.com", "", &mails, 600)
                .is_empty()
        );
        let mut quiet = p.clone();
        quiet.dnd = true;
        quiet.dnd_start = "00:00".into();
        quiet.dnd_end = "23:59".into();
        assert!(
            quiet
                .notification_payloads("a", "me@x.com", "", &mails, 12 * 60)
                .is_empty()
        );
        let mut off = p.clone();
        off.notify = false;
        assert!(
            off.notification_payloads("a", "me@x.com", "", &mails, 600)
                .is_empty()
        );
        assert!(
            p.notification_payloads("a", "me@x.com", "", &[], 600)
                .is_empty(),
            "无新邮件不弹"
        );
    }
}
