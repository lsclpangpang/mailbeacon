//! 收信规则：收件箱来了新邮件时，按发件人、收件人、主题、附件自动归类
//! （移到文件夹、标为已读、加星标）。规则在客户端执行，App 运行时生效。

use std::collections::{BTreeMap, VecDeque};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::imap::Summary;

pub const MAX_RULES: usize = 100;
/// 判断「新到」时给服务器和本机时钟留的余量。
pub const ARRIVAL_SLACK_MS: i64 = 10 * 60 * 1000;
pub const MAX_CONDITIONS: usize = 20;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Field {
    /// 发件人名字或地址
    #[default]
    From,
    /// 收件人或抄送
    To,
    Subject,
    /// 有没有附件（`Contains` 表示有，`NotContains` 表示没有）
    Attachment,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Op {
    #[default]
    Contains,
    NotContains,
    Equals,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Condition {
    pub field: Field,
    pub op: Op,
    pub value: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Actions {
    /// 移到这个文件夹（服务器上的原始路径）
    pub move_to: Option<String>,
    pub mark_read: bool,
    pub flag: bool,
}

impl Actions {
    fn is_empty(&self) -> bool {
        self.move_to.is_none() && !self.mark_read && !self.flag
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub account_id: String,
    /// true：所有条件都满足；false：满足任意一条
    pub match_all: bool,
    pub conditions: Vec<Condition>,
    pub actions: Actions,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            account_id: String::new(),
            match_all: true,
            conditions: Vec::new(),
            actions: Actions::default(),
        }
    }
}

fn contains(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

impl Condition {
    fn matches(&self, m: &Summary) -> bool {
        if self.field == Field::Attachment {
            return m.attachment == (self.op != Op::NotContains);
        }
        // 名字和地址分开比较，「等于」才能精确匹配地址
        let values: Vec<&str> = match self.field {
            Field::From => vec![&m.from_name, &m.from_addr],
            Field::To => std::iter::once(m.to.as_str())
                .chain(m.recipients.iter().map(String::as_str))
                .collect(),
            Field::Subject => vec![&m.subject],
            Field::Attachment => unreachable!(),
        };
        let hit = |v: &&str| match self.op {
            Op::Equals => v.trim().to_lowercase() == self.value.to_lowercase(),
            _ => contains(v, &self.value),
        };
        match self.op {
            Op::NotContains => !values.iter().any(hit),
            _ => values.iter().any(hit),
        }
    }
}

impl Rule {
    pub fn matches(&self, m: &Summary) -> bool {
        if !self.enabled || self.account_id != m.account_id || self.conditions.is_empty() {
            return false;
        }
        if self.match_all {
            self.conditions.iter().all(|c| c.matches(m))
        } else {
            self.conditions.iter().any(|c| c.matches(m))
        }
    }
}

/// 规范化并校验界面提交的整套规则。
pub fn validated(rules: Vec<Rule>) -> Result<Vec<Rule>> {
    if rules.len() > MAX_RULES {
        bail!("最多可以设置 {MAX_RULES} 条规则");
    }
    let mut out: Vec<Rule> = Vec::with_capacity(rules.len());
    for (index, mut rule) in rules.into_iter().enumerate() {
        rule.name = rule.name.trim().to_string();
        if rule.name.is_empty() {
            rule.name = format!("规则 {}", index + 1);
        }
        let label = rule.name.clone();
        rule.id = rule.id.trim().to_string();
        if rule.id.is_empty() || out.iter().any(|r| r.id == rule.id) {
            rule.id = format!("r{}-{index}", chrono::Utc::now().timestamp_millis());
        }
        if rule.account_id.trim().is_empty() {
            bail!("「{label}」没有选择邮箱账号");
        }
        if rule.conditions.is_empty() {
            bail!("「{label}」至少需要一个条件");
        }
        if rule.conditions.len() > MAX_CONDITIONS {
            bail!("「{label}」最多 {MAX_CONDITIONS} 个条件");
        }
        for c in &mut rule.conditions {
            c.value = c.value.trim().to_string();
            if c.field == Field::Attachment {
                c.value.clear();
                if c.op == Op::Equals {
                    c.op = Op::Contains;
                }
            } else if c.value.is_empty() {
                bail!("「{label}」有条件没有填写内容");
            } else if c.value.chars().count() > 200 {
                bail!("「{label}」的条件内容太长（最多 200 字）");
            }
        }
        rule.actions.move_to = rule
            .actions
            .move_to
            .take()
            .map(|f| f.trim().to_string())
            .filter(|f| !f.is_empty());
        if rule
            .actions
            .move_to
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("INBOX"))
        {
            bail!("「{label}」不能移到收件箱本身");
        }
        if rule.actions.is_empty() {
            bail!("「{label}」至少需要一个动作");
        }
        out.push(rule);
    }
    Ok(out)
}

/// 只对上次检查之后才到的邮件执行规则。
///
/// 移进收件箱的邮件（用户从规则的目标文件夹里拖回来、「不是垃圾邮件」、从已删除恢复）
/// 会拿到新的 UID，但服务器收件时间不变；不能再被规则移走，否则用户永远留不住它。
pub fn arrived_since(mails: &[Summary], since: i64) -> Vec<Summary> {
    mails
        .iter()
        .filter(|m| m.date >= since - ARRIVAL_SLACK_MS)
        .cloned()
        .collect()
}

type MailKey = String;

fn mail_key(m: &Summary) -> MailKey {
    crate::imap::mail_identity(m)
}

/// 还没见过更新的收件箱邮件时（收件箱是空的、刚启动），比启动时间早这么久的邮件不算新到。
const LOCAL_LOOKBACK_MS: i64 = 60 * 60 * 1000;
/// 记住规则最近移走的邮件数量上限。
const REMEMBER_MOVED: usize = 500;
/// 同一批新邮件因为连接问题连续失败这么多次后，跳过规则照常收信。
const MAX_ATTEMPTS: u32 = 2;

/// 同步连接上执行规则需要跨重连记住的东西。
#[derive(Default)]
pub struct Memory {
    /// 已经看过的收件箱邮件里最新的服务器收件时间（用服务器时钟，不受本机时钟影响）
    newest: i64,
    /// 启动时的本机时间。只在一开始定下，之后不随时间往后推，
    /// 断线（电脑睡眠）期间到达的邮件醒来后照样执行规则
    floor: i64,
    /// 规则移走过的邮件：用户又移回收件箱时不再动它
    moved: VecDeque<MailKey>,
    /// (这批邮件里最大的 UID, 已尝试次数)
    attempts: (u32, u32),
}

impl Memory {
    pub fn started(now: i64) -> Self {
        Self {
            floor: now - LOCAL_LOOKBACK_MS,
            ..Default::default()
        }
    }

    /// 记下这些收件箱邮件已经看过。`now` 是本机时间：收件时间写错到将来的邮件
    /// （例如导入的旧邮件）不能把参照推到将来，否则规则再也不会执行。
    pub fn saw(&mut self, mails: &[Summary], now: i64) {
        if let Some(max) = mails.iter().map(|m| m.date).max() {
            self.newest = self.newest.max(max.min(now));
        }
    }

    /// 该执行规则的邮件：上次之后才到的，而且不是规则移走后又被移回来的。
    pub fn candidates(&self, mails: &[Summary]) -> Vec<Summary> {
        arrived_since(mails, self.newest.max(self.floor))
            .into_iter()
            .filter(|m| !self.moved.contains(&mail_key(m)))
            .collect()
    }

    pub fn moved(&mut self, mails: &[Summary]) {
        for m in mails {
            self.moved.push_back(mail_key(m));
        }
        while self.moved.len() > REMEMBER_MOVED {
            self.moved.pop_front();
        }
    }

    /// 准备对以 `last_uid` 结尾的这批邮件执行规则；连续失败太多次时返回 false。
    pub fn attempt(&mut self, last_uid: u32) -> bool {
        if self.attempts.0 == last_uid {
            self.attempts.1 += 1;
        } else {
            self.attempts = (last_uid, 1);
        }
        self.attempts.1 <= MAX_ATTEMPTS
    }

    pub fn succeeded(&mut self) {
        self.attempts = (0, 0);
    }
}

/// 保存规则时要检查目标文件夹的规则：启用的、新加的或者改了目标的。
/// 目标没变的不拦：文件夹在服务器上被删了以后，用户还得能禁用或删掉其他规则。
pub fn targets_to_check<'a>(stored: &[Rule], rules: &'a [Rule]) -> Vec<&'a Rule> {
    rules
        .iter()
        .filter(|r| r.enabled && r.actions.move_to.is_some())
        .filter(|r| {
            !stored.iter().any(|o| {
                o.id == r.id
                    && o.account_id == r.account_id
                    && o.actions.move_to == r.actions.move_to
            })
        })
        .collect()
}

/// 规则的目标文件夹要在这个账号里存在，而且不能是收件箱、草稿、已发送。
/// `folders` 为空表示还没读到文件夹列表，只做基本检查。
pub fn check_target(rule: &Rule, folders: Option<&[crate::imap::Folder]>) -> Result<()> {
    let Some(target) = rule.actions.move_to.as_deref() else {
        return Ok(());
    };
    if target.starts_with("__") {
        bail!("「{}」不能移到本地草稿", rule.name);
    }
    let Some(folders) = folders else {
        return Ok(());
    };
    match folders.iter().find(|f| f.path == target) {
        None => bail!(
            "「{}」的目标文件夹「{}」不存在",
            rule.name,
            crate::imap::decode_mutf7(target)
        ),
        Some(f) if matches!(f.role.as_str(), "inbox" | "drafts" | "sent") => {
            bail!("「{}」不能移到「{}」", rule.name, f.name)
        }
        Some(_) => Ok(()),
    }
}

/// 一封邮件要执行的动作：按顺序叠加所有命中的规则，移动目标以第一条为准。
pub fn plan(rules: &[Rule], m: &Summary) -> Option<Actions> {
    let mut actions = Actions::default();
    for rule in rules.iter().filter(|r| r.matches(m)) {
        if actions.move_to.is_none() {
            actions.move_to = rule.actions.move_to.clone();
        }
        actions.mark_read |= rule.actions.mark_read;
        actions.flag |= rule.actions.flag;
    }
    // 已经是这个状态的就不用再改
    actions.mark_read &= !m.seen;
    actions.flag &= !m.flagged;
    (!actions.is_empty()).then_some(actions)
}

/// 同样动作的邮件合成一批，按批发命令；顺序固定，便于测试。
pub fn batches(rules: &[Rule], mails: &[Summary]) -> Vec<(Actions, Vec<u32>)> {
    let mut groups: BTreeMap<(Option<String>, bool, bool), Vec<u32>> = BTreeMap::new();
    for m in mails {
        if let Some(a) = plan(rules, m) {
            groups
                .entry((a.move_to, a.mark_read, a.flag))
                .or_default()
                .push(m.uid);
        }
    }
    groups
        .into_iter()
        .map(|((move_to, mark_read, flag), uids)| {
            (
                Actions {
                    move_to,
                    mark_read,
                    flag,
                },
                uids,
            )
        })
        .collect()
}

/// 执行结果：哪些邮件被移走、标为已读、加了星标，以及失败的原因。
#[derive(Debug, Default)]
pub struct Applied {
    pub moved: Vec<u32>,
    pub read: Vec<u32>,
    pub flagged: Vec<u32>,
    pub errors: Vec<String>,
}

impl Applied {
    /// 把结果套到摘要上：移走的去掉，其余更新已读/星标状态。
    pub fn apply_to(&self, mails: Vec<Summary>) -> Vec<Summary> {
        mails
            .into_iter()
            .filter(|m| !self.moved.contains(&m.uid))
            .map(|mut m| {
                m.seen |= self.read.contains(&m.uid);
                m.flagged |= self.flagged.contains(&m.uid);
                m
            })
            .collect()
    }
}

/// 服务器拒绝或不支持（不是超时、断线）：这一批没执行，连接照常可用。
fn refused(e: &anyhow::Error) -> bool {
    !crate::download_protocol::connection_problem(e)
}

/// 在已选中的文件夹里执行规则：先改标记再移动（标记跟着邮件走）。
/// 没执行成功的批次只记下原因、继续下一批，不能让一条坏规则卡住收信；
/// 超时或断线时返回错误，由调用方丢掉这条连接、重连后再执行。
pub async fn execute<T>(
    s: &mut async_imap::Session<T>,
    batches: &[(Actions, Vec<u32>)],
) -> Result<Applied>
where
    T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + std::fmt::Debug + Send,
{
    use anyhow::Context;
    let mut done = Applied::default();
    for (actions, uids) in batches {
        if uids.is_empty() {
            continue;
        }
        let set = crate::imap::uid_set(uids);
        let flags: Vec<&str> = [(actions.mark_read, "\\Seen"), (actions.flag, "\\Flagged")]
            .into_iter()
            .filter_map(|(on, flag)| on.then_some(flag))
            .collect();
        if !flags.is_empty() {
            let stored = tokio::time::timeout(
                crate::imap::OP_TIMEOUT,
                crate::download_protocol::checked_command(
                    s,
                    &format!("UID STORE {set} +FLAGS.SILENT ({})", flags.join(" ")),
                ),
            )
            .await
            .context("修改邮件标记超时")
            .and_then(|r| r);
            match stored {
                Ok(_) => {
                    if actions.mark_read {
                        done.read.extend(uids);
                    }
                    if actions.flag {
                        done.flagged.extend(uids);
                    }
                }
                Err(e) if refused(&e) => {
                    done.errors.push(format!("收信规则没能修改邮件标记：{e:#}"))
                }
                Err(e) => return Err(e),
            }
        }
        if let Some(target) = &actions.move_to {
            match crate::imap::move_uids(s, &set, Some(target)).await {
                Ok(()) => done.moved.extend(uids),
                Err(e) if refused(&e) => done.errors.push(format!(
                    "收信规则没能把邮件移到「{}」：{e:#}",
                    crate::imap::decode_mutf7(target)
                )),
                Err(e) => return Err(e),
            }
        }
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mail(uid: u32, from: (&str, &str), subject: &str) -> Summary {
        Summary {
            account_id: "a".into(),
            folder: "INBOX".into(),
            uid,
            from_name: from.0.into(),
            from_addr: from.1.into(),
            to: "我 <me@x.com>".into(),
            subject: subject.into(),
            date: 0,
            seen: false,
            flagged: false,
            attachment: false,
            recipients: vec!["me@x.com".into(), "team@x.com".into()],
            ..Default::default()
        }
    }

    fn cond(field: Field, op: Op, value: &str) -> Condition {
        Condition {
            field,
            op,
            value: value.into(),
        }
    }

    fn rule(conditions: Vec<Condition>, actions: Actions) -> Rule {
        Rule {
            id: "r1".into(),
            name: "规则".into(),
            account_id: "a".into(),
            conditions,
            actions,
            ..Default::default()
        }
    }

    fn move_to(folder: &str) -> Actions {
        Actions {
            move_to: Some(folder.into()),
            ..Default::default()
        }
    }

    #[test]
    fn conditions_match_case_insensitively_by_field() {
        let m = mail(1, ("GitHub", "noreply@GitHub.com"), "[GitHub] New SSH key");
        let hit = |c: Condition| rule(vec![c], move_to("Work")).matches(&m);
        assert!(hit(cond(Field::From, Op::Contains, "github.com")));
        assert!(hit(cond(Field::From, Op::Contains, "github")));
        assert!(hit(cond(Field::From, Op::Equals, "noreply@github.com")));
        assert!(
            !hit(cond(Field::From, Op::Equals, "github.com")),
            "等于要整段相同"
        );
        assert!(hit(cond(Field::Subject, Op::Contains, "ssh KEY")));
        assert!(!hit(cond(Field::Subject, Op::NotContains, "ssh")));
        assert!(
            hit(cond(Field::To, Op::Contains, "team@")),
            "抄送也算收件人"
        );
        assert!(hit(cond(Field::To, Op::Equals, "me@x.com")));
        assert!(!hit(cond(Field::To, Op::NotContains, "team@")));
        assert!(!hit(cond(Field::Attachment, Op::Contains, "")));
        assert!(hit(cond(Field::Attachment, Op::NotContains, "")));
    }

    #[test]
    fn all_or_any_and_disabled_or_other_account_rules() {
        let m = mail(1, ("张三", "zs@corp.com"), "发票 2026-10");
        let mut r = rule(
            vec![
                cond(Field::From, Op::Contains, "corp.com"),
                cond(Field::Subject, Op::Contains, "合同"),
            ],
            move_to("Bills"),
        );
        assert!(!r.matches(&m), "全部满足");
        r.match_all = false;
        assert!(r.matches(&m), "任意一条");
        r.enabled = false;
        assert!(!r.matches(&m));
        r.enabled = true;
        r.account_id = "b".into();
        assert!(!r.matches(&m), "只对自己的账号生效");
    }

    #[test]
    fn matching_rules_stack_and_the_first_move_wins() {
        let m = mail(1, ("Bank", "bill@bank.com"), "账单");
        let rules = vec![
            rule(
                vec![cond(Field::From, Op::Contains, "bank")],
                move_to("Bills"),
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "账单")],
                Actions {
                    move_to: Some("Other".into()),
                    mark_read: true,
                    flag: true,
                },
            ),
        ];
        assert_eq!(
            plan(&rules, &m),
            Some(Actions {
                move_to: Some("Bills".into()),
                mark_read: true,
                flag: true,
            })
        );
        let mut read = m.clone();
        read.seen = true;
        read.flagged = true;
        assert_eq!(
            plan(&rules, &read).unwrap(),
            move_to("Bills"),
            "已读、已加星的不重复设置"
        );
        assert_eq!(plan(&rules, &mail(2, ("x", "x@y.com"), "hi")), None);
    }

    #[test]
    fn mails_with_the_same_actions_are_batched() {
        let rules = vec![
            rule(
                vec![cond(Field::From, Op::Contains, "bank")],
                move_to("Bills"),
            ),
            rule(
                vec![cond(Field::Subject, Op::Contains, "周报")],
                Actions {
                    mark_read: true,
                    ..Default::default()
                },
            ),
        ];
        let mails = vec![
            mail(1, ("Bank", "a@bank.com"), "账单"),
            mail(2, ("同事", "c@corp.com"), "周报"),
            mail(3, ("Bank", "b@bank.com"), "账单"),
            mail(4, ("x", "x@y.com"), "hi"),
        ];
        let got = batches(&rules, &mails);
        assert_eq!(
            got,
            vec![
                (
                    Actions {
                        mark_read: true,
                        ..Default::default()
                    },
                    vec![2]
                ),
                (move_to("Bills"), vec![1, 3]),
            ]
        );
    }

    #[test]
    fn validation_requires_account_condition_and_action() {
        let ok = rule(
            vec![cond(Field::Subject, Op::Contains, " 发票 ")],
            move_to(" Bills "),
        );
        let got = validated(vec![ok.clone()]).unwrap();
        assert_eq!(got[0].conditions[0].value, "发票");
        assert_eq!(got[0].actions.move_to.as_deref(), Some("Bills"));

        let mut blank = ok.clone();
        blank.name = " ".into();
        blank.id = String::new();
        let got = validated(vec![blank.clone(), blank]).unwrap();
        assert_eq!(got[0].name, "规则 1");
        assert!(!got[0].id.is_empty());
        assert_ne!(got[0].id, got[1].id, "新规则各有各的 id");

        let mut no_account = ok.clone();
        no_account.account_id = String::new();
        assert!(validated(vec![no_account]).is_err());
        let mut no_cond = ok.clone();
        no_cond.conditions.clear();
        assert!(validated(vec![no_cond]).is_err());
        let mut empty_value = ok.clone();
        empty_value.conditions[0].value = "  ".into();
        assert!(validated(vec![empty_value]).is_err());
        let mut no_action = ok.clone();
        no_action.actions = Actions {
            move_to: Some(" ".into()),
            ..Default::default()
        };
        assert!(validated(vec![no_action]).is_err());
        let mut to_inbox = ok.clone();
        to_inbox.actions = move_to("inbox");
        assert!(validated(vec![to_inbox]).is_err());
        // 附件条件不需要填写内容
        let attachment = rule(
            vec![cond(Field::Attachment, Op::Equals, "x")],
            move_to("Files"),
        );
        let got = validated(vec![attachment]).unwrap();
        assert_eq!(
            got[0].conditions[0],
            cond(Field::Attachment, Op::Contains, "")
        );
        assert!(validated(vec![ok; MAX_RULES + 1]).is_err());
    }

    fn raw(from: &str, subject: &str) -> String {
        format!("From: {from}\r\nSubject: {subject}\r\n\r\nbody\r\n")
    }

    #[tokio::test]
    async fn executing_batches_sets_flags_moves_mail_and_reports_failures() {
        use crate::test_imap;
        let server = test_imap::state("MOVE UIDPLUS", &[("INBOX", 1), ("Bills", 2)]);
        let (bill, note, other) = {
            let mut st = server.lock().unwrap();
            (
                st.add("INBOX", &raw("bill@bank.com", "账单"), &[]),
                st.add("INBOX", &raw("c@corp.com", "周报"), &[]),
                st.add("INBOX", &raw("x@y.com", "hi"), &[]),
            )
        };
        let mut s = test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        let batches = vec![
            (
                Actions {
                    move_to: Some("Bills".into()),
                    mark_read: true,
                    flag: false,
                },
                vec![bill],
            ),
            (
                Actions {
                    flag: true,
                    ..Default::default()
                },
                vec![note],
            ),
            (move_to("Missing"), vec![other]),
        ];
        let done = execute(&mut s, &batches).await.unwrap();
        assert_eq!(done.moved, vec![bill]);
        assert_eq!(done.read, vec![bill]);
        assert_eq!(done.flagged, vec![note]);
        assert_eq!(done.errors.len(), 1, "{:?}", done.errors);
        assert!(done.errors[0].contains("Missing"), "{:?}", done.errors);
        let st = server.lock().unwrap();
        let inbox: Vec<u32> = st.mailboxes[0].messages.iter().map(|m| m.uid).collect();
        assert_eq!(inbox, vec![note, other]);
        let moved = &st.mailboxes[1].messages[0];
        assert!(moved.flags.iter().any(|f| f == "\\Seen"), "先标已读再移动");
        assert!(
            st.mailboxes[0].messages[0]
                .flags
                .iter()
                .any(|f| f == "\\Flagged")
        );

        let mails = vec![
            mail(bill, ("Bank", "bill@bank.com"), "账单"),
            mail(note, ("同事", "c@corp.com"), "周报"),
            mail(other, ("x", "x@y.com"), "hi"),
        ];
        let left = done.apply_to(mails);
        assert_eq!(
            left.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![note, other]
        );
        assert!(left[0].flagged && !left[0].seen);
    }

    #[test]
    fn only_mail_that_arrived_since_the_last_check_is_sorted() {
        let now = 10 * 60 * 60 * 1000;
        let mut fresh = mail(1, ("Bank", "a@bank.com"), "账单");
        fresh.date = now;
        // 用户把旧邮件从「账单」移回收件箱：UID 是新的，收件时间是旧的
        let mut moved_back = mail(2, ("Bank", "b@bank.com"), "旧账单");
        moved_back.date = now - 3 * 24 * 60 * 60 * 1000;
        let mut just_before = mail(3, ("Bank", "c@bank.com"), "账单");
        just_before.date = now - 2 * 60 * 1000;
        let got = arrived_since(&[fresh, moved_back, just_before], now - 60 * 1000);
        assert_eq!(
            got.iter().map(|m| m.uid).collect::<Vec<_>>(),
            vec![1, 3],
            "留几分钟余量"
        );
    }

    #[test]
    fn move_targets_are_checked_against_the_account_folders() {
        let folders = vec![
            crate::imap::Folder {
                path: "INBOX".into(),
                name: "收件箱".into(),
                role: "inbox".into(),
            },
            crate::imap::Folder {
                path: "Drafts".into(),
                name: "草稿".into(),
                role: "drafts".into(),
            },
            crate::imap::Folder {
                path: "Bills".into(),
                name: "账单".into(),
                role: "other".into(),
            },
        ];
        let ok = rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("Bills"),
        );
        assert!(check_target(&ok, Some(&folders)).is_ok());
        assert!(check_target(&ok, None).is_ok(), "还没读到文件夹列表时不拦");
        let drafts = rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("Drafts"),
        );
        assert!(check_target(&drafts, Some(&folders)).is_err());
        let missing = rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("Gone"),
        );
        let error = check_target(&missing, Some(&folders))
            .unwrap_err()
            .to_string();
        assert!(error.contains("Gone"), "{error}");
        let local = rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("__local_drafts"),
        );
        assert!(check_target(&local, None).is_err());
    }

    #[tokio::test]
    async fn refused_commands_are_reported_but_a_broken_connection_stops_execution() {
        use crate::test_imap;
        let server = test_imap::state("MOVE UIDPLUS", &[("INBOX", 1), ("Bills", 2)]);
        let uid = server
            .lock()
            .unwrap()
            .add("INBOX", &raw("bill@bank.com", "账单"), &[]);
        server.lock().unwrap().fail = vec!["UID STORE".into()];
        let mut s = test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        let read = Actions {
            mark_read: true,
            ..Default::default()
        };
        let done = execute(&mut s, &[(read.clone(), vec![uid])]).await.unwrap();
        assert!(
            done.read.is_empty() && done.errors.len() == 1,
            "{:?}",
            done.errors
        );
        // 网络断了：结果未知，必须报错让外层重连，而不是当成普通失败继续用这条连接
        server.lock().unwrap().fail.clear();
        server.lock().unwrap().disconnect = vec!["UID MOVE".into()];
        assert!(
            execute(&mut s, &[(move_to("Bills"), vec![uid])])
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn servers_without_move_get_copy_and_expunge_of_only_those_mails() {
        use crate::test_imap;
        let server = test_imap::state("UIDPLUS", &[("INBOX", 1), ("Bills", 2)]);
        let (bill, keep) = {
            let mut st = server.lock().unwrap();
            (
                st.add("INBOX", &raw("bill@bank.com", "账单"), &[]),
                // 别的客户端标了删除但还没清除的邮件不能被顺带清掉
                st.add("INBOX", &raw("x@y.com", "别动"), &["\\Deleted"]),
            )
        };
        let mut s = test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        let done = execute(&mut s, &[(move_to("Bills"), vec![bill])])
            .await
            .unwrap();
        assert_eq!(done.moved, vec![bill]);
        let st = server.lock().unwrap();
        assert_eq!(
            st.mailboxes[0]
                .messages
                .iter()
                .map(|m| m.uid)
                .collect::<Vec<_>>(),
            vec![keep]
        );
        assert_eq!(st.mailboxes[1].messages.len(), 1);
    }

    #[test]
    fn corrupt_saved_rules_are_backed_up_instead_of_overwritten() {
        let db = crate::contacts::Db::open_in_memory().unwrap();
        db.save_raw_pref("rules", "{not json").unwrap();
        assert!(db.load_rules().unwrap().is_empty());
        assert_eq!(
            db.raw_pref("rules.corrupt").unwrap().as_deref(),
            Some("{not json")
        );
        let rules = vec![rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("Bills"),
        )];
        db.save_rules(&rules).unwrap();
        assert_eq!(db.load_rules().unwrap(), rules);
        assert_eq!(
            db.raw_pref("rules.corrupt").unwrap().as_deref(),
            Some("{not json"),
            "备份一直留着"
        );
    }

    #[tokio::test]
    async fn a_server_that_cannot_move_safely_is_reported_without_dropping_the_connection() {
        use crate::test_imap;
        let server = test_imap::state("", &[("INBOX", 1), ("Bills", 2)]);
        let uid = server
            .lock()
            .unwrap()
            .add("INBOX", &raw("bill@bank.com", "账单"), &[]);
        let mut s = test_imap::session(server.clone()).await;
        s.select("INBOX").await.unwrap();
        let done = execute(&mut s, &[(move_to("Bills"), vec![uid])])
            .await
            .unwrap();
        assert!(done.moved.is_empty());
        assert_eq!(done.errors.len(), 1, "{:?}", done.errors);
        assert!(s.noop().await.is_ok(), "连接还能继续用");
    }

    #[test]
    fn memory_uses_the_server_clock_and_skips_mail_the_user_moved_back() {
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let mut memory = Memory::started(99 * day);
        let at = |uid: u32, date: i64| {
            let mut m = mail(uid, ("Bank", "a@bank.com"), &format!("账单 {uid}"));
            m.date = date;
            m
        };
        // 第一次连上：记下服务器上最新邮件的时间
        memory.saw(&[at(1, 100 * day), at(2, 99 * day)], now);
        let fresh = at(3, 100 * day + 1000);
        let moved_back_old = at(4, 50 * day);
        let got = memory.candidates(&[fresh.clone(), moved_back_old]);
        assert_eq!(got.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![3]);
        // 规则把 3 移走后用户马上又移回来：新 UID，同一封邮件，不再动它
        memory.moved(&[fresh.clone()]);
        let mut back = fresh.clone();
        back.uid = 9;
        assert!(memory.candidates(&[back]).is_empty());
    }

    #[test]
    fn an_empty_inbox_or_a_restart_does_not_resort_old_mail_moved_back() {
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        // 刚启动、收件箱是空的：参照是启动时间
        let memory = Memory::started(now);
        let mut old = mail(1, ("Bank", "a@bank.com"), "上周的账单");
        old.date = now - 5 * day;
        let mut fresh = mail(2, ("Bank", "a@bank.com"), "新账单");
        fresh.date = now - 60 * 1000;
        let got = memory.candidates(&[old, fresh]);
        assert_eq!(got.iter().map(|m| m.uid).collect::<Vec<_>>(), vec![2]);
    }

    #[test]
    fn mail_that_arrived_during_a_long_disconnect_is_still_sorted() {
        let hour = 60 * 60 * 1000;
        let t0 = 1000 * hour;
        let mut memory = Memory::started(t0);
        let last = Summary {
            date: t0,
            ..mail(1, ("x", "x@y.com"), "睡前最后一封")
        };
        memory.saw(&[last], t0);
        // 电脑睡了一夜：凌晨到的邮件，早上醒来才看到
        let night = Summary {
            date: t0 + 4 * hour,
            ..mail(2, ("Bank", "a@bank.com"), "账单")
        };
        assert_eq!(memory.candidates(&[night]).len(), 1);
    }

    #[test]
    fn look_alike_alerts_are_told_apart_by_message_id() {
        let now = 100 * 24 * 60 * 60 * 1000;
        let mut memory = Memory::started(now);
        let alert = |uid: u32, id: &str| Summary {
            message_id: id.into(),
            date: now,
            ..mail(uid, ("CI", "ci@x.com"), "构建失败")
        };
        memory.moved(&[alert(1, "a1@ci")]);
        assert_eq!(
            memory.candidates(&[alert(2, "a2@ci")]).len(),
            1,
            "同一秒的同名告警是另一封邮件"
        );
        assert!(memory.candidates(&[alert(3, "a1@ci")]).is_empty());
    }

    #[test]
    fn a_bogus_future_date_does_not_switch_rules_off() {
        let day = 24 * 60 * 60 * 1000;
        let now = 100 * day;
        let mut memory = Memory::started(now);
        let mut future = mail(1, ("x", "x@y.com"), "导入的旧邮件");
        future.date = now + 365 * day;
        memory.saw(&[future], now);
        let mut fresh = mail(2, ("Bank", "a@bank.com"), "新账单");
        fresh.date = now + 60 * 1000;
        assert_eq!(memory.candidates(&[fresh]).len(), 1);
    }

    #[test]
    fn memory_gives_up_on_rules_after_repeated_failures_for_the_same_mail() {
        let mut memory = Memory::default();
        assert!(memory.attempt(10));
        assert!(memory.attempt(10));
        assert!(
            !memory.attempt(10),
            "同一批邮件连续失败两次后跳过规则，照常收信"
        );
        assert!(memory.attempt(11), "新的一批重新计数");
        memory.succeeded();
        assert!(memory.attempt(11));
    }

    #[test]
    fn only_new_or_changed_enabled_targets_are_checked_on_save() {
        let stored = vec![Rule {
            id: "r1".into(),
            ..rule(
                vec![cond(Field::From, Op::Contains, "bank")],
                move_to("Gone"),
            )
        }];
        let mut same = stored.clone();
        same[0].name = "改了名字".into();
        assert!(
            targets_to_check(&stored, &same).is_empty(),
            "目标没变（哪怕已不存在）不拦，免得连禁用都存不了"
        );
        let mut disabled = stored.clone();
        disabled[0].actions.move_to = Some("Other".into());
        disabled[0].enabled = false;
        assert!(targets_to_check(&stored, &disabled).is_empty());
        let mut changed = stored.clone();
        changed[0].actions.move_to = Some("Other".into());
        assert_eq!(targets_to_check(&stored, &changed).len(), 1);
        let fresh = vec![rule(
            vec![cond(Field::From, Op::Contains, "x")],
            move_to("New"),
        )];
        assert_eq!(targets_to_check(&stored, &fresh).len(), 1, "新规则要检查");
    }

    #[test]
    fn rules_round_trip_through_the_local_database() {
        let db = crate::contacts::Db::open_in_memory().unwrap();
        assert!(db.load_rules().unwrap().is_empty());
        let rules = vec![rule(
            vec![cond(Field::From, Op::Contains, "bank")],
            move_to("Bills"),
        )];
        db.save_rules(&rules).unwrap();
        assert_eq!(db.load_rules().unwrap(), rules);
    }
}
