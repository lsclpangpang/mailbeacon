//! Order server snapshots against confirmed flag mutations without a time window.
use super::*;
use crate::mail_ref::{MessageRef, namespace};

#[derive(Default)]
pub(super) struct PendingFlags {
    pub seen: Option<(u64, bool)>,
    pub flagged: Option<(u64, bool)>,
}

impl SearchIndex {
    /// Capture before a server read; a confirmed mutation advances this generation.
    pub(crate) fn flags_revision(&self, account: &str, folder: &str) -> u64 {
        self.revisions
            .lock()
            .unwrap()
            .get(&(account.into(), folder.into()))
            .copied()
            .unwrap_or(0)
    }

    /// Apply a successful user mutation to its exact identity and return its generation.
    pub(crate) fn confirm_source_flags(
        &self,
        source: &MessageRef,
        seen: Option<bool>,
        flagged: Option<bool>,
    ) -> Result<u64> {
        source.validate()?;
        self.set_flags_at_validity(
            &namespace(&source.account_id, source.receive_epoch),
            &source.folder,
            &[source.uid],
            seen,
            flagged,
            Some(source.uid_validity),
        )
    }

    /// Reject a delayed FLAGS read atomically with its index update. False also means
    /// its corresponding UI event must not be published.
    pub(crate) fn update_flags_if_current(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        revision: u64,
        flags: &[(u32, bool, bool)],
    ) -> Result<bool> {
        let revisions = self.revisions.lock().unwrap();
        if validity == 0
            || revisions
                .get(&(account.into(), folder.into()))
                .copied()
                .unwrap_or(0)
                != revision
        {
            return Ok(false);
        }
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let current: Option<u32> = tx
            .query_row(
                "SELECT uid_validity FROM mail_index_meta WHERE account_id=?1 AND folder=?2",
                params![account, folder],
                |r| r.get(0),
            )
            .optional()?;
        if current.is_some_and(|current| current != validity) {
            return Ok(false);
        }
        for &(uid, seen, flagged) in flags {
            let json: Option<String> = tx.query_row("SELECT summary FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![account,folder,validity,uid],|r|r.get(0)).optional()?;
            let Some(json) = json else {
                if let Some(pending) = self.unindexed_flags.lock().unwrap().get_mut(&(
                    account.into(),
                    folder.into(),
                    validity,
                    uid,
                )) {
                    pending.seen = Some((revision, seen));
                    pending.flagged = Some((revision, flagged));
                }
                continue;
            };
            let mut summary: Summary = serde_json::from_str(&json)?;
            summary.seen = seen;
            summary.flagged = flagged;
            tx.execute("UPDATE mail_index SET summary=?5 WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![account,folder,validity,uid,serde_json::to_string(&summary)?])?;
        }
        tx.commit()?;
        Ok(true)
    }

    /// A stale generation must never be published. A cache write failure alone
    /// must not stop delivery of otherwise valid new mail.
    pub(crate) fn snapshot_summaries_for_publish(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        revision: Option<u64>,
        summaries: &mut [Summary],
    ) -> bool {
        match self.upsert_snapshot_summaries(account, folder, validity, revision, summaries) {
            Ok(current) => current,
            Err(error) => {
                eprintln!("更新本地摘要失败（{folder}）：{error:#}");
                // A competing generation change can also reject the write itself.
                !self
                    .folder_meta(account, folder)
                    .ok()
                    .flatten()
                    .is_some_and(|meta| meta.0 > validity)
            }
        }
    }

    /// A reconnect can contain new mail as well as old flags. Keep new messages,
    /// but merge confirmed flags from the same full identity when the read is old.
    pub(crate) fn upsert_snapshot_summaries(
        &self,
        account: &str,
        folder: &str,
        validity: u32,
        revision: Option<u64>,
        summaries: &mut [Summary],
    ) -> Result<bool> {
        let revisions = self.revisions.lock().unwrap();
        let current_revision = revisions
            .get(&(account.into(), folder.into()))
            .copied()
            .unwrap_or(0);
        if self
            .folder_meta(account, folder)?
            .is_some_and(|meta| meta.0 > validity)
        {
            return Ok(false);
        }
        if revision != Some(current_revision) {
            let conn = self.lock();
            for summary in summaries.iter_mut() {
                let old: Option<String> = conn.query_row("SELECT summary FROM mail_index WHERE account_id=?1 AND folder=?2 AND uid_validity=?3 AND uid=?4",params![account,folder,validity,summary.uid],|r|r.get(0)).optional()?;
                if let Some(old) = old {
                    let old: Summary = serde_json::from_str(&old)?;
                    summary.seen = old.seen;
                    summary.flagged = old.flagged;
                } else if let Some(pending) = self.unindexed_flags.lock().unwrap().get(&(
                    account.into(),
                    folder.into(),
                    validity,
                    summary.uid,
                )) {
                    if let Some((_, seen)) = pending
                        .seen
                        .filter(|(confirmed, _)| revision.is_none_or(|read| *confirmed > read))
                    {
                        summary.seen = seen;
                    }
                    if let Some((_, flagged)) = pending
                        .flagged
                        .filter(|(confirmed, _)| revision.is_none_or(|read| *confirmed > read))
                    {
                        summary.flagged = flagged;
                    }
                }
            }
        }
        self.record_snapshot(account, folder, validity)?;
        self.upsert_summaries(account, folder, validity, summaries)?;
        let mut pending = self.unindexed_flags.lock().unwrap();
        for summary in summaries {
            pending.remove(&(account.into(), folder.into(), validity, summary.uid));
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> MessageRef {
        MessageRef {
            account_id: "a".into(),
            receive_epoch: 2,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 1,
        }
    }
    fn seeded() -> (SearchIndex, String) {
        let index = SearchIndex::in_memory().unwrap();
        let source = source();
        let key = namespace(&source.account_id, source.receive_epoch);
        index
            .record_snapshot(&key, &source.folder, source.uid_validity)
            .unwrap();
        index
            .upsert_summaries(
                &key,
                &source.folder,
                source.uid_validity,
                &[Summary {
                    account_id: source.account_id,
                    receive_epoch: source.receive_epoch,
                    folder: source.folder.clone(),
                    uid_validity: source.uid_validity,
                    uid: source.uid,
                    ..Default::default()
                }],
            )
            .unwrap();
        (index, key)
    }

    #[test]
    fn delayed_watcher_flags_are_rejected_but_new_reads_can_accept_later_server_changes() {
        let (index, key) = seeded();
        let before = index.flags_revision(&key, "INBOX");
        let confirmed = index
            .confirm_source_flags(&source(), None, Some(true))
            .unwrap();
        assert!(confirmed > before);
        assert!(
            !index
                .update_flags_if_current(&key, "INBOX", 7, before, &[(1, false, false)])
                .unwrap()
        );
        assert!(index.snapshot_summary(&source()).unwrap().flagged);
        assert!(
            index
                .update_flags_if_current(&key, "INBOX", 7, confirmed, &[(1, true, false)])
                .unwrap()
        );
        let latest = index.snapshot_summary(&source()).unwrap();
        assert!(
            latest.seen && !latest.flagged,
            "a new server read must not be ignored forever"
        );
    }

    #[test]
    fn stale_reconnect_keeps_new_mail_and_preserves_flags_only_for_the_exact_identity() {
        let (index, key) = seeded();
        let before = index.flags_revision(&key, "INBOX");
        let old = index.snapshot_summary(&source()).unwrap();
        index
            .confirm_source_flags(&source(), None, Some(true))
            .unwrap();
        let mut batch = vec![
            old.clone(),
            Summary {
                uid: 2,
                subject: "new mail".into(),
                ..old
            },
        ];
        index
            .upsert_snapshot_summaries(&key, "INBOX", 7, Some(before), &mut batch)
            .unwrap();
        assert!(batch[0].flagged);
        assert!(!batch[1].flagged);
        assert_eq!(index.count(&key, "INBOX").unwrap(), 2);
        assert_eq!(batch[1].subject, "new mail");
        assert!(
            !index
                .update_flags_if_current(
                    &key,
                    "INBOX",
                    8,
                    index.flags_revision(&key, "INBOX"),
                    &[(1, false, false)]
                )
                .unwrap()
        );
        let other_epoch = namespace("a", 3);
        assert_eq!(index.flags_revision(&other_epoch, "INBOX"), 0);
        assert_eq!(index.flags_revision(&key, "Sent"), 0);
    }

    #[test]
    fn stale_first_summary_keeps_a_confirmation_made_before_the_message_was_indexed() {
        let index = SearchIndex::in_memory().unwrap();
        let source = source();
        let key = namespace(&source.account_id, source.receive_epoch);
        let before = index.flags_revision(&key, &source.folder);
        index
            .confirm_source_flags(&source, None, Some(true))
            .unwrap();
        let mut summaries = vec![Summary {
            account_id: source.account_id.clone(),
            receive_epoch: source.receive_epoch,
            folder: source.folder.clone(),
            uid_validity: source.uid_validity,
            uid: source.uid,
            ..Default::default()
        }];
        index
            .upsert_snapshot_summaries(
                &key,
                &source.folder,
                source.uid_validity,
                Some(before),
                &mut summaries,
            )
            .unwrap();
        assert!(
            summaries[0].flagged,
            "a confirmation must survive the first delayed summary for an online page"
        );
        assert!(index.snapshot_summary(&source).unwrap().flagged);
    }

    #[test]
    fn a_fresh_flags_read_supersedes_an_unindexed_confirmation_before_its_first_summary() {
        let index = SearchIndex::in_memory().unwrap();
        let source = source();
        let key = namespace(&source.account_id, source.receive_epoch);
        let before = index.flags_revision(&key, &source.folder);
        let revision = index
            .confirm_source_flags(&source, None, Some(true))
            .unwrap();
        assert!(
            index
                .update_flags_if_current(&key, &source.folder, 7, revision, &[(1, true, false)])
                .unwrap()
        );
        let mut summaries = vec![Summary {
            account_id: source.account_id.clone(),
            receive_epoch: source.receive_epoch,
            folder: source.folder.clone(),
            uid_validity: source.uid_validity,
            uid: source.uid,
            ..Default::default()
        }];
        index
            .upsert_snapshot_summaries(
                &key,
                &source.folder,
                source.uid_validity,
                Some(before),
                &mut summaries,
            )
            .unwrap();
        assert!(
            !summaries[0].flagged,
            "a newer accepted server read must replace an unindexed confirmation"
        );
        assert!(summaries[0].seen);
    }

    #[test]
    fn an_old_generation_snapshot_is_never_publishable_over_a_newer_index_generation() {
        let (index, key) = seeded();
        let mut old = vec![index.snapshot_summary(&source()).unwrap()];
        index.record_snapshot(&key, "INBOX", 8).unwrap();
        assert!(
            !index.snapshot_summaries_for_publish(&key, "INBOX", 7, Some(0), &mut old),
            "a rejected UIDVALIDITY 7 batch must never replace the UI generation 8"
        );
        assert_eq!(index.folder_meta(&key, "INBOX").unwrap().unwrap().0, 8);
    }

    #[test]
    fn confirmation_never_rewrites_a_reused_uid_from_another_validity() {
        let (index, _) = seeded();
        let changed = MessageRef {
            uid_validity: 8,
            ..source()
        };
        index
            .confirm_source_flags(&changed, None, Some(true))
            .unwrap();
        assert!(!index.snapshot_summary(&source()).unwrap().flagged);
    }
}
