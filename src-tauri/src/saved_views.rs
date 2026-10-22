//! Persisted search definitions; scope and definition revisions never follow UI selection.
use crate::search_index::{SearchRequest, SearchTarget};
use anyhow::{Context, Result, bail};
use chrono::{Days, Local, TimeZone};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct SearchSpec {
    pub spec_version: u32,
    pub scope: String,
    pub targets: Vec<SearchTarget>,
    pub query: String,
    pub unread_only: bool,
    pub flagged_only: bool,
    pub attachment_only: bool,
    pub sender: String,
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub relative_days: Option<u32>,
    pub include_excluded: bool,
    pub sort: String,
}

impl Default for SearchSpec {
    fn default() -> Self {
        Self {
            spec_version: 1,
            scope: "fixed".into(),
            targets: vec![],
            query: String::new(),
            unread_only: false,
            flagged_only: false,
            attachment_only: false,
            sender: String::new(),
            after: None,
            before: None,
            relative_days: None,
            include_excluded: false,
            sort: "dateDesc".into(),
        }
    }
}

impl SearchSpec {
    /// Reject unknown rules rather than broadening a saved scope silently.
    pub fn validate(&self) -> Result<()> {
        if self.spec_version != 1 {
            bail!("智能文件夹规则版本不受支持，请修复条件");
        }
        if !matches!(self.sort.as_str(), "dateAsc" | "dateDesc") {
            bail!("不支持的排序规则");
        }
        match self.scope.as_str() {
            "fixed" if !self.targets.is_empty() && self.targets.len() <= 200 => {}
            "all" if self.targets.is_empty() => {}
            _ => bail!("请选择固定文件夹，或明确选择全部账户"),
        }
        if self.targets.iter().any(|t| {
            t.account_id.is_empty()
                || t.receive_epoch == 0
                || t.folder.is_empty()
                || t.folder.starts_with("__")
        }) {
            bail!("智能文件夹必须保存已确认的真实文件夹身份");
        }
        if self.after.zip(self.before).is_some_and(|(a, b)| a >= b) {
            bail!("日期范围起点必须早于终点");
        }
        if let Some(days) = self.relative_days {
            if !(1..=36500).contains(&days) || self.after.is_some() || self.before.is_some() {
                bail!("相对日期与固定日期不能同时设置");
            }
        }
        if self.query.len() > 4096 || self.sender.len() > 1024 {
            bail!("查询条件过长");
        }
        Ok(())
    }

    /// Resolve a relative date at query time using calendar midnights in the local timezone.
    pub fn request(
        &self,
        targets: Vec<SearchTarget>,
        cursor: Option<String>,
    ) -> Result<SearchRequest> {
        self.validate()?;
        let (after, before) = if let Some(days) = self.relative_days {
            let today = Local::now().date_naive();
            let start = today
                .checked_sub_days(Days::new((days - 1).into()))
                .context("日期超出范围")?;
            let end = today
                .checked_add_days(Days::new(1))
                .context("日期超出范围")?;
            let midnight = |date: chrono::NaiveDate| -> Result<i64> {
                Ok(Local
                    .from_local_datetime(&date.and_hms_opt(0, 0, 0).context("无效日期")?)
                    .earliest()
                    .context("此时区的日期边界不存在，请使用固定日期")?
                    .timestamp_millis())
            };
            (Some(midnight(start)?), Some(midnight(end)?))
        } else {
            (self.after, self.before)
        };
        Ok(SearchRequest {
            targets,
            query: self.query.clone(),
            unread_only: self.unread_only,
            flagged_only: self.flagged_only,
            attachment_only: self.attachment_only,
            sender: self.sender.clone(),
            after,
            before,
            page_size: 100,
            cursor,
            sort: self.sort.clone(),
        })
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedView {
    pub id: String,
    pub name: String,
    pub position: i64,
    pub revision: u64,
    /// Keep unrecognized future definitions intact so users can repair or export them.
    pub spec: serde_json::Value,
    pub repair_reason: Option<String>,
}

/// Independent idempotent schema; contacts retains ownership of user_version.
pub fn init(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS saved_views (
        id TEXT PRIMARY KEY, name TEXT NOT NULL, position INTEGER NOT NULL,
        revision INTEGER NOT NULL, spec TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS productivity_migrations (name TEXT PRIMARY KEY);
        INSERT OR IGNORE INTO productivity_migrations VALUES ('saved-views-v1');",
    )?;
    Ok(())
}

/// Read every definition, preserving even rules from a newer application version.
pub fn list(conn: &Connection) -> Result<Vec<SavedView>> {
    let mut stmt = conn
        .prepare("SELECT id,name,position,revision,spec FROM saved_views ORDER BY position,id")?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, i64>(2)?,
            r.get::<_, u64>(3)?,
            r.get::<_, String>(4)?,
        ))
    })?;
    let mut views = Vec::new();
    for row in rows {
        let (id, name, position, revision, json) = row?;
        let spec: serde_json::Value = serde_json::from_str(&json).context("智能文件夹定义损坏")?;
        let repair_reason = serde_json::from_value::<SearchSpec>(spec.clone())
            .map_err(anyhow::Error::from)
            .and_then(|s| s.validate())
            .err()
            .map(|e| e.to_string());
        views.push(SavedView {
            id,
            name,
            position,
            revision,
            spec,
            repair_reason,
        });
    }
    Ok(views)
}

/// Save only an explicit user revision; stale editors cannot overwrite newer definitions.
pub fn save(
    conn: &mut Connection,
    id: Option<String>,
    name: String,
    position: i64,
    spec: serde_json::Value,
    expected_revision: Option<u64>,
) -> Result<SavedView> {
    let parsed: SearchSpec = serde_json::from_value(spec.clone()).context("智能文件夹规则无效")?;
    parsed.validate()?;
    let name = name.trim().to_string();
    if name.is_empty() || name.chars().count() > 120 || position < 0 {
        bail!("智能文件夹名称或顺序无效");
    }
    let tx = conn.transaction()?;
    let (id, revision) = if let Some(id) = id {
        let current: Option<u64> = tx
            .query_row("SELECT revision FROM saved_views WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .optional()?;
        let current = current.context("智能文件夹已被删除")?;
        if expected_revision != Some(current) {
            bail!("智能文件夹已被更新，请刷新后重试");
        }
        let revision = current.checked_add(1).context("修订号溢出")?;
        tx.execute(
            "UPDATE saved_views SET name=?2,position=?3,revision=?4,spec=?5 WHERE id=?1",
            params![id, name, position, revision, serde_json::to_string(&spec)?],
        )?;
        (id, revision)
    } else {
        if expected_revision.is_some() {
            bail!("新智能文件夹不能携带旧修订号");
        }
        let id = crate::send_records::new_id()?;
        tx.execute(
            "INSERT INTO saved_views VALUES(?1,?2,?3,1,?4)",
            params![id, name, position, serde_json::to_string(&spec)?],
        )?;
        (id, 1)
    };
    tx.commit()?;
    Ok(SavedView {
        id,
        name,
        position,
        revision,
        spec,
        repair_reason: None,
    })
}

/// Delete a definition only; this module has no mail deletion capability.
pub fn delete(conn: &Connection, id: &str, expected_revision: u64) -> Result<()> {
    if conn.execute(
        "DELETE FROM saved_views WHERE id=?1 AND revision=?2",
        params![id, expected_revision],
    )? != 1
    {
        bail!("智能文件夹已改变，请刷新后重试");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn spec() -> serde_json::Value {
        serde_json::json!({"specVersion":1,"scope":"fixed","targets":[{"accountId":"a","receiveEpoch":2,"folder":"INBOX"}],"attachmentOnly":true})
    }
    #[test]
    fn saved_views_survive_reopen_and_stale_updates_do_not_replace_scope() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.db");
        let mut db = Connection::open(&path).unwrap();
        init(&db).unwrap();
        let view = save(&mut db, None, "Attachments".into(), 0, spec(), None).unwrap();
        drop(db);
        let mut db = Connection::open(&path).unwrap();
        init(&db).unwrap();
        let found = list(&db).unwrap().remove(0);
        assert_eq!(found.spec["targets"][0]["receiveEpoch"], 2);
        let changed = save(
            &mut db,
            Some(view.id.clone()),
            "Renamed".into(),
            1,
            spec(),
            Some(1),
        )
        .unwrap();
        assert_eq!(changed.revision, 2);
        assert!(
            save(
                &mut db,
                Some(view.id.clone()),
                "stale".into(),
                0,
                spec(),
                Some(1)
            )
            .is_err()
        );
        assert!(delete(&db, &view.id, 1).is_err());
        delete(&db, &view.id, 2).unwrap();
        assert!(list(&db).unwrap().is_empty());
    }
    #[test]
    fn future_rules_are_preserved_but_cannot_be_executed_or_silently_saved() {
        let mut db = Connection::open_in_memory().unwrap();
        init(&db).unwrap();
        let mut future = spec();
        future["specVersion"] = 9.into();
        db.execute(
            "INSERT INTO saved_views VALUES('old','Future',0,1,?1)",
            [future.to_string()],
        )
        .unwrap();
        let view = list(&db).unwrap().remove(0);
        assert_eq!(view.spec, future);
        assert!(view.repair_reason.is_some());
        assert!(save(&mut db, None, "future".into(), 1, future, None).is_err());
    }
    #[test]
    fn fixed_and_dynamic_scopes_cannot_be_conflated() {
        let mut s: SearchSpec = serde_json::from_value(spec()).unwrap();
        s.scope = "all".into();
        assert!(s.validate().is_err());
        s.targets.clear();
        assert!(s.validate().is_ok());
        s.scope = "fixed".into();
        assert!(s.validate().is_err());
    }
}
