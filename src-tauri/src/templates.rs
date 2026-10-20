//! 邮件模板：常用的主题和正文，写信时一键插入。存在本地数据库里。

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const MAX_TEMPLATES: usize = 100;
/// 正文（HTML）上限；模板里一般是文字，内嵌大图片应该用附件
pub const MAX_HTML_BYTES: usize = 512 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Template {
    pub id: String,
    pub name: String,
    pub subject: String,
    pub html: String,
}

/// 规范化并校验界面提交的整套模板。
pub fn validated(list: Vec<Template>) -> Result<Vec<Template>> {
    if list.len() > MAX_TEMPLATES {
        bail!("最多可以保存 {MAX_TEMPLATES} 个模板");
    }
    let mut out: Vec<Template> = Vec::with_capacity(list.len());
    for (index, mut t) in list.into_iter().enumerate() {
        t.name = t.name.trim().to_string();
        if t.name.is_empty() {
            bail!("模板需要一个名称");
        }
        if t.name.chars().count() > 60 {
            bail!("模板名称最多 60 个字");
        }
        if out.iter().any(|o| o.name == t.name) {
            bail!("已经有名为「{}」的模板", t.name);
        }
        if t.html.len() > MAX_HTML_BYTES {
            bail!("模板「{}」的正文太长", t.name);
        }
        if t.subject.trim().is_empty() && t.html.trim().is_empty() {
            bail!("模板「{}」没有内容", t.name);
        }
        t.id = t.id.trim().to_string();
        if t.id.is_empty() || out.iter().any(|o| o.id == t.id) {
            t.id = format!("t{}-{index}", chrono::Utc::now().timestamp_millis());
        }
        out.push(t);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(id: &str, name: &str) -> Template {
        Template {
            id: id.into(),
            name: name.into(),
            subject: "报价".into(),
            html: "<div>您好</div>".into(),
        }
    }

    #[test]
    fn validation_names_and_ids_templates() {
        let got = validated(vec![t("", " 报价回复 "), t("x", " ")]).unwrap_err();
        assert!(got.to_string().contains("名称"), "{got}");
        let got = validated(vec![t("", " 报价回复 "), t("", "会议邀请")]).unwrap();
        assert_eq!(got[0].name, "报价回复");
        assert!(!got[0].id.is_empty() && got[0].id != got[1].id);
        let kept = validated(vec![t("keep", "A")]).unwrap();
        assert_eq!(kept[0].id, "keep", "已有的 id 不变");
    }

    #[test]
    fn duplicate_names_and_oversized_templates_are_rejected() {
        assert!(validated(vec![t("a", "周报"), t("b", " 周报")]).is_err());
        let mut big = t("a", "大");
        big.html = "x".repeat(MAX_HTML_BYTES + 1);
        assert!(validated(vec![big]).is_err());
        assert!(validated(vec![t("", "x"); MAX_TEMPLATES + 1]).is_err());
        let mut empty = t("a", "空");
        empty.subject.clear();
        empty.html = "  ".into();
        assert!(
            validated(vec![empty]).is_err(),
            "主题和正文都空的模板没有意义"
        );
    }

    #[test]
    fn templates_round_trip_and_survive_corrupt_data() {
        let db = crate::contacts::Db::open_in_memory().unwrap();
        assert!(db.load_templates().unwrap().is_empty());
        let list = validated(vec![t("", "报价回复")]).unwrap();
        db.save_templates(&list).unwrap();
        assert_eq!(db.load_templates().unwrap(), list);
        db.save_raw_pref("templates", "[oops").unwrap();
        assert!(db.load_templates().unwrap().is_empty());
        assert_eq!(
            db.raw_pref("templates.corrupt").unwrap().as_deref(),
            Some("[oops")
        );
    }
}
