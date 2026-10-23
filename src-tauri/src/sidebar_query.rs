//! Generation-scoped sidebar aggregates; no page limit and no network reads.
use super::*;
use crate::{
    mail_ref::namespace,
    sidebar_counts::{Count, FolderCounts},
};

impl SearchIndex {
    pub fn sidebar_status_revision(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
    ) -> Result<(u64, u64)> {
        let key = namespace(account, epoch);
        let revisions = self.revisions.lock().unwrap();
        let revision = self
            .lock()
            .query_row(
                "SELECT revision FROM mail_sidebar_revision WHERE account_id=?1 AND folder=?2",
                params![key, folder],
                |r| r.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);
        Ok((
            revisions.get(&(key, folder.into())).copied().unwrap_or(0),
            revision,
        ))
    }

    pub fn record_sidebar_status_if_current(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
        validity: u32,
        total: u32,
        expected: (u64, u64),
    ) -> Result<bool> {
        self.write_sidebar_status(account, epoch, folder, validity, total, Some(expected))
    }

    #[cfg(test)]
    pub fn record_sidebar_status(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
        validity: u32,
        total: u32,
    ) -> Result<()> {
        self.write_sidebar_status(account, epoch, folder, validity, total, None)
            .map(|_| ())
    }

    fn write_sidebar_status(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
        validity: u32,
        total: u32,
        expected: Option<(u64, u64)>,
    ) -> Result<bool> {
        if validity == 0 {
            anyhow::bail!("服务器未确认文件夹代际");
        }
        let key = namespace(account, epoch);
        let mut revisions = self.revisions.lock().unwrap();
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let db_revision = tx
            .query_row(
                "SELECT revision FROM mail_sidebar_revision WHERE account_id=?1 AND folder=?2",
                params![key, folder],
                |r| r.get::<_, u64>(0),
            )
            .optional()?
            .unwrap_or(0);
        let current_revision = (
            revisions
                .get(&(key.clone(), folder.into()))
                .copied()
                .unwrap_or(0),
            db_revision,
        );
        if expected.is_some_and(|v| v != current_revision) {
            return Ok(false);
        }
        let old: Option<(u32,u32,bool)> = tx
            .query_row(
                "SELECT uid_validity,total,complete FROM mail_index_meta WHERE account_id=?1 AND folder=?2",
                params![key, folder],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            )
            .optional()?;
        anyhow::ensure!(
            old.is_none_or(|v| v.0 <= validity),
            "旧文件夹统计任务已过期，请重新同步"
        );
        let changed = old.is_some_and(|v| v.0 != validity);
        let changed_total = old.is_some_and(|v| (v.1 > 0 || v.2) && v.1 != total);
        if changed || changed_total {
            *revisions.entry((key.clone(), folder.into())).or_default() += 1;
        }
        if changed {
            tx.execute(
                "DELETE FROM mail_index WHERE account_id=?1 AND folder=?2",
                params![key, folder],
            )?;
            tx.execute(
                "DELETE FROM mail_index_meta WHERE account_id=?1 AND folder=?2",
                params![key, folder],
            )?;
        }
        let at = chrono::Utc::now().timestamp_millis();
        if old.is_none() || changed {
            tx.execute("INSERT INTO mail_index_meta(account_id,folder,uid_validity,complete,indexed,total,body_indexed,updated_at) VALUES(?1,?2,?3,0,0,0,0,?4)",params![key,folder,validity,at])?;
        } else if changed_total {
            tx.execute(
                "UPDATE mail_index_meta SET complete=0,total=0 WHERE account_id=?1 AND folder=?2",
                params![key, folder],
            )?;
        }
        tx.execute("INSERT INTO mail_sidebar_status(account_id,folder,uid_validity,total,updated_at) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(account_id,folder) DO UPDATE SET uid_validity=excluded.uid_validity,total=excluded.total,updated_at=excluded.updated_at", params![key,folder,validity,total,at])?;
        tx.commit()?;
        Ok(true)
    }

    pub fn sidebar_folder_counts(
        &self,
        account: &str,
        epoch: u64,
        folder: &str,
    ) -> Result<FolderCounts> {
        let key = namespace(account, epoch);
        let conn = self.lock();
        let meta: Option<(u32,bool,u64,u64,i64)> = conn.query_row("SELECT uid_validity,complete,total,indexed,updated_at FROM mail_index_meta WHERE account_id=?1 AND folder=?2 AND uid_validity>0", params![key,folder], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        let Some((validity, complete, total, recorded_indexed, at)) = meta else {
            return Ok(FolderCounts::unknown(folder));
        };
        let status: Option<(u64,i64)> = conn.query_row("SELECT total,updated_at FROM mail_sidebar_status WHERE account_id=?1 AND folder=?2 AND uid_validity=?3", params![key,folder,validity], |r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (indexed,flagged,parsed):(u64,u64,u64) = conn.query_row("SELECT COUNT(*),COALESCE(SUM(json_extract(summary,'$.flagged')=1),0),COALESCE(SUM(EXISTS(SELECT 1 FROM attachment_sources s WHERE s.storage_account=i.account_id AND s.folder=i.folder AND s.uid_validity=i.uid_validity AND s.uid=i.uid AND s.parser_version=?4)),0) FROM mail_index i WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity=?3", params![key,folder,validity,crate::attachment_index::PARSER_VERSION], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let attachments:u64 = conn.query_row("SELECT COUNT(*) FROM attachment_metadata a JOIN mail_index i ON a.storage_account=i.account_id AND a.folder=i.folder AND a.uid_validity=i.uid_validity AND a.uid=i.uid WHERE i.account_id=?1 AND i.folder=?2 AND i.uid_validity=?3",params![key,folder,validity],|r|r.get(0))?;
        let headers_complete = (complete || (total > 0 && recorded_indexed == total))
            && indexed == total
            && status.is_none_or(|s| s.0 == total);
        let total_count = if let Some((value, snapshot_at)) = status {
            Count::exact(value, Some(snapshot_at))
        } else if (total > 0 || complete) && indexed <= total {
            Count::exact(total, Some(at))
        } else {
            Count::partial(indexed, Some(at))
        };
        let empty = status.is_some_and(|s| s.0 == 0) || (complete && total == 0 && indexed == 0);
        let contradicted = status.is_some_and(|s| s.0 < indexed);
        Ok(FolderCounts {
            folder: folder.into(),
            uid_validity: Some(validity),
            total: total_count,
            starred: if empty {
                Count::exact(0, status.map(|s| s.1).or(Some(at)))
            } else if contradicted {
                Count::unknown()
            } else if headers_complete {
                Count::exact(flagged, Some(at))
            } else {
                Count::partial(flagged, Some(at))
            },
            attachments: if empty {
                Count::exact(0, status.map(|s| s.1).or(Some(at)))
            } else if contradicted {
                Count::unknown()
            } else if headers_complete && parsed == indexed {
                Count::exact(attachments, Some(at))
            } else {
                Count::partial(attachments, Some(at))
            },
        })
    }
}
