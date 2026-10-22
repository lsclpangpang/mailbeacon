//! Stable receive identity. Persisted references identify snapshots, not live server validation.
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const fn initial_epoch() -> u64 {
    1
}

/// Full IMAP identity, including the account's receiving endpoint generation.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageRef {
    pub account_id: String,
    pub receive_epoch: u64,
    pub folder: String,
    pub uid_validity: u32,
    pub uid: u32,
}

impl MessageRef {
    /// Reject unknown identities rather than guessing which server mail they describe.
    pub fn validate(&self) -> Result<()> {
        if self.account_id.is_empty()
            || self.folder.is_empty()
            || self.receive_epoch == 0
            || self.uid_validity == 0
            || self.uid == 0
        {
            bail!("邮件身份尚未确认，请联网刷新后重试");
        }
        Ok(())
    }

    /// A live operation must match both the configured receive endpoint and SELECT result.
    pub fn validate_current(&self, receive_epoch: u64, uid_validity: u32) -> Result<()> {
        self.validate()?;
        if self.receive_epoch != receive_epoch || self.uid_validity != uid_validity {
            bail!("原邮件所属邮箱已发生变化，请刷新后重新选择；未操作其他邮件");
        }
        Ok(())
    }
}

/// Epoch 1 keeps existing caches usable. New endpoints get a disjoint storage namespace.
pub fn namespace(account_id: &str, receive_epoch: u64) -> String {
    if receive_epoch == 1 {
        account_id.to_string()
    } else {
        format!("{account_id}\u{1f}receive:{receive_epoch}")
    }
}

/// Download manifests retain the public account id; only the registry uses the epoch key.
pub fn storage_account(storage: &str) -> &str {
    storage
        .rsplit_once('\u{1f}')
        .filter(|(_, suffix)| suffix.starts_with("receive:"))
        .map_or(storage, |(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_or_changed_identity_never_authorizes_a_live_operation() {
        let source = MessageRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 42,
        };
        assert!(source.validate_current(1, 7).is_ok());
        assert!(source.validate_current(2, 7).is_err());
        assert!(source.validate_current(1, 8).is_err());
        assert!(
            MessageRef {
                uid_validity: 0,
                ..source.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            MessageRef {
                receive_epoch: 0,
                ..source.clone()
            }
            .validate()
            .is_err()
        );
        assert!(MessageRef { uid: 0, ..source }.validate().is_err());
    }

    #[test]
    fn new_receive_identity_does_not_share_legacy_storage() {
        assert_eq!(namespace("a", 1), "a");
        assert_ne!(namespace("a", 1), namespace("a", 2));
        assert_ne!(namespace("a", 2), namespace("b", 2));
    }

    #[test]
    fn legacy_summary_has_an_initial_epoch_but_no_invented_mailbox_generation() {
        let mail = crate::imap::Summary::default();
        let json = serde_json::to_value(&mail).unwrap();
        assert_eq!(json["receiveEpoch"], 1);
        assert_eq!(json["uidValidity"], 0);
        let restored: crate::imap::Summary = serde_json::from_value(serde_json::json!({
            "accountId":"a", "folder":"INBOX", "uid":4, "fromName":"", "fromAddr":"", "to":"", "subject":"", "date":0,
            "seen":false, "flagged":false, "attachment":false, "recipients":[], "messageId":"", "inReplyTo":"", "references":[]
        })).unwrap();
        let json = serde_json::to_value(restored).unwrap();
        assert_eq!(json["receiveEpoch"], 1);
        assert_eq!(json["uidValidity"], 0);
    }
}
