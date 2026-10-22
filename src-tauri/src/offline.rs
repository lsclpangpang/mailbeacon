//! Local snapshot reads deliberately have no IMAP or SMTP connection capability.
use super::*;
use crate::mail_ref::{MessageRef, namespace};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMessage {
    pub message: MessageView,
    pub source: MessageRef,
    pub local_only: bool,
    pub snapshot_at: Option<i64>,
}

pub async fn read_snapshot(
    index: &search_index::SearchIndex,
    cache: &Mutex<BodyCache>,
    source: &MessageRef,
    current_epoch: u64,
) -> Result<SnapshotMessage> {
    source.validate()?;
    if source.receive_epoch != current_epoch {
        bail!("该邮件来自更换收信服务器前的旧账户快照，请重新选择当前邮箱邮件");
    }
    let (snapshot_at, full) = index.snapshot_reference(source)?;
    let key = (
        namespace(&source.account_id, source.receive_epoch),
        source.folder.clone(),
        source.uid_validity,
        source.uid,
    );
    let Some((raw, date)) = cached_body(cache, &key).await else {
        bail!(if full {
            "此邮件的本地正文已清理或归档目录不在线；联网后可重新下载"
        } else {
            "本机只有邮件摘要，尚未保存完整正文；联网后打开邮件即可下载"
        });
    };
    let parsed = crate::mime::parse(&raw).context("本地邮件文件损坏，未从网络读取其他邮件")?;
    if parsed.from().is_none() && parsed.subject().is_none() && parsed.message_id().is_none() {
        bail!("本地邮件文件损坏，请联网重新下载");
    }
    drop(parsed);
    let mut message =
        imap::parse_message(&raw, date).context("本地邮件文件损坏，请联网重新下载")?;
    message.uid_validity = Some(source.uid_validity);
    // Offline reading never asks for or sends an MDN receipt.
    message.receipt_to = None;
    Ok(SnapshotMessage {
        message,
        source: source.clone(),
        local_only: true,
        snapshot_at,
    })
}
