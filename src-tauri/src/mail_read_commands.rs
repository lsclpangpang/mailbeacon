//! Search scope and snapshot entry points. Local reads never borrow a connection lane.
use super::*;
use crate::{
    mail_ref::MessageRef,
    search_index::{SearchRequest, SearchTarget},
};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchInput {
    pub scope: String,
    pub account_id: Option<String>,
    pub folder: String,
    pub include_excluded: bool,
    pub query: String,
    pub unread_only: bool,
    pub flagged_only: bool,
    pub attachment_only: bool,
    pub sender: String,
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub cursor: Option<String>,
}
impl Default for SearchInput {
    fn default() -> Self {
        Self {
            scope: "folder".into(),
            account_id: None,
            folder: "INBOX".into(),
            include_excluded: false,
            query: String::new(),
            unread_only: false,
            flagged_only: false,
            attachment_only: false,
            sender: String::new(),
            after: None,
            before: None,
            cursor: None,
        }
    }
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    #[serde(flatten)]
    page: search_index::SearchPage,
    catalog_incomplete: Vec<String>,
}

fn scoped_folders(
    scope: &str,
    folder: &str,
    include_excluded: bool,
    folders: &[Folder],
) -> Result<Vec<String>> {
    match scope {
        "folder" => {
            if matches!(folder, "__unread" | "__starred") {
                return Ok(vec!["INBOX".into()]);
            }
            if matches!(folder, "__sent" | "__drafts") {
                let role = if folder == "__sent" { "sent" } else { "drafts" };
                return Ok(folders
                    .iter()
                    .filter(|f| f.role == role)
                    .map(|f| f.path.clone())
                    .collect());
            }
            if folder.is_empty() || folder.starts_with("__") {
                bail!("请选择一个真实文件夹，或改为搜索全部邮箱");
            }
            Ok(vec![folder.into()])
        }
        "account" | "all" => Ok(folders
            .iter()
            .filter(|f| include_excluded || !matches!(f.role.as_str(), "junk" | "trash"))
            .map(|f| f.path.clone())
            .collect()),
        _ => bail!("搜索范围无效"),
    }
}

#[tauri::command]
pub fn search_mail(
    state: State<'_, AppState>,
    app: AppHandle,
    input: SearchInput,
) -> CmdResult<SearchResponse> {
    let index = state.search_index().map_err(err)?;
    if input.scope == "account" && input.account_id.is_none() {
        return Err("请先选择一个邮箱账户".into());
    }
    let accounts: Vec<_> = state
        .store
        .lock()
        .unwrap()
        .accounts
        .iter()
        .filter(|a| input.scope == "all" || input.account_id.as_ref().is_none_or(|id| id == &a.id))
        .cloned()
        .collect();
    let mut targets = Vec::new();
    let mut incomplete = Vec::new();
    for acct in accounts {
        if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
            incomplete.push(format!("{}：POP3 请在收件箱单独搜索", acct.email));
            continue;
        }
        let folders = state
            .folders
            .lock()
            .unwrap()
            .get(&acct.id)
            .cloned()
            .unwrap_or(
                index
                    .saved_folders(&acct.id, acct.receive_epoch)
                    .map_err(err)?,
            );
        if (input.scope != "folder" || matches!(input.folder.as_str(), "__sent" | "__drafts"))
            && !index
                .has_folder_catalog(&acct.id, acct.receive_epoch)
                .map_err(err)?
        {
            incomplete.push(format!("{}：文件夹列表尚未完整同步", acct.email));
        }
        if input.scope == "folder" && input.folder == "__drafts" {
            incomplete.push(format!(
                "{}：仅搜索服务器草稿；本地草稿可在草稿列表查看",
                acct.email
            ));
        }
        for folder in scoped_folders(
            &input.scope,
            &input.folder,
            input.include_excluded,
            &folders,
        )
        .map_err(err)?
        {
            targets.push(SearchTarget {
                account_id: acct.id.clone(),
                receive_epoch: acct.receive_epoch,
                folder,
            });
        }
    }
    let request = SearchRequest {
        query: input.query,
        targets: targets.clone(),
        unread_only: input.unread_only,
        flagged_only: input.flagged_only,
        attachment_only: input.attachment_only,
        sender: input.sender,
        after: input.after,
        before: input.before,
        cursor: input.cursor,
        page_size: 100,
        sort: "dateDesc".into(),
    };
    let mut page = index.search_page(&request).map_err(err)?;
    // Return the local page immediately. Network jobs only update coverage after completion.
    for target in &targets {
        if let Err(error) = state.ensure_search_index(&app, &target.account_id, &target.folder) {
            if let Some(c) = page
                .coverage
                .iter_mut()
                .find(|c| c.account_id == target.account_id && c.folder == target.folder)
            {
                c.error = Some(error.to_string());
            }
        }
    }
    enrich_local_bodies(&state, &app, targets);
    Ok(SearchResponse {
        page,
        catalog_incomplete: incomplete,
    })
}

static LOCAL_INDEX: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);

fn enrich_local_bodies(state: &AppState, app: &AppHandle, targets: Vec<SearchTarget>) {
    let Ok(index) = state.search_index().cloned() else {
        return;
    };
    let cache = state.bodies.clone();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Ok(_permit) = LOCAL_INDEX.try_acquire() else {
            return;
        };
        let per_folder = (80 / targets.len().max(1)).max(1);
        let mut updated = false;
        for target in targets {
            let Ok(sources) = index.incomplete_references(&target, per_folder) else {
                continue;
            };
            for source in sources {
                let Some(state) = app.try_state::<AppState>() else {
                    return;
                };
                if state
                    .configured_account(&source.account_id)
                    .is_ok_and(|a| a.receive_epoch != source.receive_epoch)
                {
                    break;
                }
                // Reader work wins over the optional local index pass.
                if state.turns.idle_for(&source.account_id) < Duration::from_secs(2) {
                    break;
                }
                let key = (
                    mail_ref::namespace(&source.account_id, source.receive_epoch),
                    source.folder.clone(),
                    source.uid_validity,
                    source.uid,
                );
                if let Some((raw, _)) = cached_body(&cache, &key).await {
                    updated |= index.index_full_body(&source, &raw).is_ok();
                }
                let _ = index.mark_local_checked(&source);
            }
        }
        if updated {
            let _ = app.emit("mail", serde_json::json!({"type":"localIndex"}));
        }
    });
}

#[tauri::command]
pub fn list_saved_folders(
    state: State<'_, AppState>,
    account_id: String,
) -> CmdResult<Vec<Folder>> {
    let acct = state.configured_account(&account_id).map_err(err)?;
    if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
        return Ok(Vec::new());
    }
    state
        .search_index()
        .and_then(|i| i.saved_folders(&account_id, acct.receive_epoch))
        .map_err(err)
}

#[tauri::command]
pub fn list_saved_messages(
    state: State<'_, AppState>,
    account_id: String,
    folder: String,
    cursor: Option<String>,
) -> CmdResult<search_index::SnapshotPage> {
    let acct = state.configured_account(&account_id).map_err(err)?;
    if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
        return Err("POP3 邮件暂不支持跨重启离线列表".into());
    }
    state
        .search_index()
        .and_then(|index| {
            index.snapshot_page(
                &SearchTarget {
                    account_id,
                    receive_epoch: acct.receive_epoch,
                    folder,
                },
                cursor,
                100,
            )
        })
        .map_err(err)
}

#[tauri::command]
pub async fn get_message_ref(
    state: State<'_, AppState>,
    app: AppHandle,
    source: MessageRef,
    local_only: bool,
) -> CmdResult<offline::SnapshotMessage> {
    let _gate = state.account_gate(&source.account_id).read_owned().await;
    source.validate().map_err(err)?;
    let acct = state.configured_account(&source.account_id).map_err(err)?;
    source
        .validate_current(acct.receive_epoch, source.uid_validity)
        .map_err(err)?;
    if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
        return Err("此离线邮件路径只适用于 IMAP".into());
    }
    if local_only {
        return offline::read_snapshot(
            state.search_index().map_err(err)?,
            &state.bodies,
            &source,
            acct.receive_epoch,
        )
        .await
        .map_err(err);
    }
    let flag_index = state.search_index().ok().cloned();
    let read_revision = flag_index.as_ref().map(|index| {
        index.flags_revision(
            &mail_ref::namespace(&source.account_id, source.receive_epoch),
            &source.folder,
        )
    });
    let open = Opening {
        turn: state.turns.next(&source.account_id),
        app,
        expected_validity: Some(source.uid_validity),
    };
    let text_first = !state
        .no_text_first
        .lock()
        .unwrap()
        .contains(&source.account_id);
    let mut raw = open_raw(
        &state,
        &source.account_id,
        &source.folder,
        source.uid,
        &open,
        text_first,
    )
    .await;
    if raw
        .as_ref()
        .is_err_and(|e| e.downcast_ref::<TextFirstFailed>().is_some())
    {
        raw = open_raw(
            &state,
            &source.account_id,
            &source.folder,
            source.uid,
            &open,
            false,
        )
        .await;
    }
    let (raw, date, validity) = match raw {
        Ok(found) => found,
        Err(e) if download_protocol::connection_problem(&e) => {
            return offline::read_snapshot(
                state.search_index().map_err(err)?,
                &state.bodies,
                &source,
                acct.receive_epoch,
            )
            .await
            .map(|mut snapshot| {
                snapshot.sync_error = Some(format!("连接失败：{e:#}"));
                snapshot
            })
            .map_err(|local| format!("连接失败；{local:#}"));
        }
        Err(e) => return Err(err(e)),
    };
    source
        .validate_current(
            state
                .configured_account(&source.account_id)
                .map_err(err)?
                .receive_epoch,
            validity,
        )
        .map_err(err)?;
    state.remember_body_validity(&source.account_id, &source.folder, validity);
    let _ = state
        .search_index()
        .and_then(|i| i.index_full_body(&source, &raw));
    let mut message = imap::parse_message(&raw, date).map_err(err)?;
    message.uid_validity = Some(validity);
    // Marking read re-selects the exact source; failure never turns a local snapshot into a write.
    let marking = source.clone();
    let seen = state
        .with_cmd(&source.account_id, |c, acct| {
            Box::pin(async move {
                c.selected = None;
                automatic_read(
                    &mut c.s,
                    flag_index.as_deref(),
                    &marking,
                    acct.receive_epoch,
                    read_revision,
                )
                .await
            })
        })
        .await;
    Ok(completed_read(message, source, seen))
}

async fn automatic_read<T: crate::lanes::Stream>(
    session: &mut async_imap::Session<T>,
    index: Option<&search_index::SearchIndex>,
    source: &MessageRef,
    epoch: u64,
    revision: Option<u64>,
) -> Result<bool> {
    // Called while the command lane is locked: a user mutation cannot pass this
    // check and then race the STORE or the corresponding index confirmation.
    // Conservatively skip if any operation invalidated this folder during the read.
    if let Some(index) = index {
        let current = index.flags_revision(
            &mail_ref::namespace(&source.account_id, source.receive_epoch),
            &source.folder,
        );
        if revision != Some(current) {
            return Ok(false);
        }
    }
    let validity = select_validity(session, &source.folder).await?;
    source.validate_current(epoch, validity)?;
    if !imap::fetch_flags(session, &source.uid.to_string())
        .await?
        .iter()
        .any(|m| m.uid == source.uid)
    {
        bail!("原邮件已删除，未标记其他邮件");
    }
    timeout(
        OP_TIMEOUT,
        download_protocol::checked_command(
            session,
            &format!("UID STORE {} +FLAGS.SILENT (\\Seen)", source.uid),
        ),
    )
    .await??;
    if let Some(index) = index {
        if let Err(error) = index.confirm_source_flags(source, Some(true), None) {
            eprintln!("更新自动已读索引失败：{error:#}");
        }
    }
    Ok(true)
}

fn completed_read(
    mut message: MessageView,
    source: MessageRef,
    seen: Result<bool>,
) -> offline::SnapshotMessage {
    let read_marked = matches!(seen, Ok(true));
    let sync_error = seen.err().map(|error| format!("{error:#}"));
    if !read_marked {
        // A skipped auto-read remains a live body; a validation failure becomes
        // read-only below. Neither is a confirmed read receipt.
        message.receipt_to = None;
    }
    offline::SnapshotMessage {
        message,
        source,
        local_only: sync_error.is_some(),
        read_marked,
        snapshot_at: None,
        sync_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn late_automatic_read_does_not_undo_an_explicit_unread_mutation() {
        let index = search_index::SearchIndex::in_memory().unwrap();
        let source = MessageRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 1,
        };
        let account = mail_ref::namespace(&source.account_id, source.receive_epoch);
        let server = crate::test_imap::state("IMAP4rev1", &[("INBOX", 7)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: original\r\n\r\nbody", &[]);
        let read_revision = index.flags_revision(&account, "INBOX");
        index
            .confirm_source_flags(&source, Some(false), None)
            .unwrap();
        let mut session = crate::test_imap::session(server.clone()).await;
        let marked = automatic_read(&mut session, Some(&index), &source, 1, Some(read_revision))
            .await
            .unwrap();
        assert!(
            !marked,
            "a body read started before U must not mark its message read later"
        );
        assert!(
            !server.lock().unwrap().mailbox("INBOX").messages[0]
                .flags
                .iter()
                .any(|flag| flag == "\\Seen")
        );
    }

    #[tokio::test]
    async fn current_automatic_read_marks_seen_and_confirms_index_before_releasing_command_lane() {
        let index = search_index::SearchIndex::in_memory().unwrap();
        let source = MessageRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 1,
        };
        let account = mail_ref::namespace(&source.account_id, source.receive_epoch);
        let server = crate::test_imap::state("IMAP4rev1", &[("INBOX", 7)]);
        server
            .lock()
            .unwrap()
            .add("INBOX", "Subject: original\r\n\r\nbody", &[]);
        let revision = index.flags_revision(&account, "INBOX");
        let mut session = crate::test_imap::session(server.clone()).await;
        assert!(
            automatic_read(&mut session, Some(&index), &source, 1, Some(revision))
                .await
                .unwrap()
        );
        assert!(
            server.lock().unwrap().mailbox("INBOX").messages[0]
                .flags
                .iter()
                .any(|flag| flag == "\\Seen")
        );
        assert_eq!(index.flags_revision(&account, "INBOX"), revision + 1);
        // A skipped auto-read is still a validated live body, but is not a read receipt.
        let message = imap::parse_message(b"From: sender@example.test\r\nSubject: saved\r\nDisposition-Notification-To: sender@example.test\r\n\r\nSaved body", 0).unwrap();
        let completed = completed_read(message, source, Ok(false));
        assert!(!completed.local_only);
        assert!(!completed.read_marked);
        assert!(completed.sync_error.is_none());
        assert!(completed.message.receipt_to.is_none());
    }

    #[test]
    fn failed_live_validation_keeps_the_visible_body_read_only() {
        for detail in [
            "原邮件已删除，未标记其他邮件",
            "原邮件文件夹已发生变化，请刷新后重新选择",
            "标记邮件已读超时",
        ] {
            let message = imap::parse_message(
                b"From: sender@example.test\r\nSubject: saved\r\nDisposition-Notification-To: sender@example.test\r\n\r\nSaved body",
                0,
            ).unwrap();
            let source = MessageRef {
                account_id: "a".into(),
                receive_epoch: 1,
                folder: "INBOX".into(),
                uid_validity: 7,
                uid: 42,
            };
            let result = completed_read(message, source, Err(anyhow!(detail)));
            assert!(
                result.local_only,
                "failed validation must not unlock actions: {detail}"
            );
            assert!(!result.read_marked);
            assert!(result.message.receipt_to.is_none());
            assert!(result.message.text.contains("Saved body"));
            assert_eq!(result.sync_error.as_deref(), Some(detail));
        }
    }

    #[test]
    fn successful_live_validation_enables_actions_and_reports_read_confirmation() {
        let message = imap::parse_message(
            b"From: sender@example.test\r\nSubject: saved\r\nDisposition-Notification-To: sender@example.test\r\n\r\nSaved body",
            0,
        ).unwrap();
        let source = MessageRef {
            account_id: "a".into(),
            receive_epoch: 1,
            folder: "INBOX".into(),
            uid_validity: 7,
            uid: 42,
        };
        let result = completed_read(message, source, Ok(true));
        assert!(!result.local_only);
        assert!(result.read_marked);
        assert!(result.message.receipt_to.is_some());
        assert!(result.sync_error.is_none());
    }
    #[test]
    fn direct_trash_scope_is_allowed_but_cross_folder_excludes_it() {
        let folders = vec![
            Folder {
                path: "INBOX".into(),
                name: "收件箱".into(),
                role: "inbox".into(),
            },
            Folder {
                path: "Trash".into(),
                name: "垃圾篓".into(),
                role: "trash".into(),
            },
        ];
        assert_eq!(
            scoped_folders("folder", "Trash", false, &folders).unwrap(),
            vec!["Trash"]
        );
        assert_eq!(
            scoped_folders("all", "INBOX", false, &folders).unwrap(),
            vec!["INBOX"]
        );
        assert_eq!(
            scoped_folders("account", "INBOX", true, &folders)
                .unwrap()
                .len(),
            2
        );
        assert!(scoped_folders("folder", "__all", false, &folders).is_err());
    }
    #[test]
    fn virtual_search_uses_the_same_inbox_or_role_folders_as_the_visible_view() {
        let folders = vec![
            Folder {
                path: "INBOX".into(),
                name: "Inbox".into(),
                role: "inbox".into(),
            },
            Folder {
                path: "Out".into(),
                name: "Sent".into(),
                role: "sent".into(),
            },
            Folder {
                path: "Work".into(),
                name: "Drafts".into(),
                role: "drafts".into(),
            },
        ];
        assert_eq!(
            scoped_folders("folder", "__unread", false, &folders).unwrap(),
            vec!["INBOX"]
        );
        assert_eq!(
            scoped_folders("folder", "__sent", false, &folders).unwrap(),
            vec!["Out"]
        );
        assert_eq!(
            scoped_folders("folder", "__drafts", false, &folders).unwrap(),
            vec!["Work"]
        );
    }
}
