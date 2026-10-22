//! Smart-folder and attachment IPC. Every content operation validates the complete source.
use super::*;
use crate::{
    attachment_index::{self, AttachmentRef},
    attachment_preview,
    saved_views::{self, SearchSpec},
    search_index::SearchTarget,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    #[serde(flatten)]
    pub page: search_index::SearchPage,
    pub catalog_incomplete: Vec<String>,
}

fn resolve_targets(
    state: &AppState,
    spec: &SearchSpec,
) -> Result<(Vec<SearchTarget>, Vec<String>)> {
    spec.validate()?;
    let index = state.search_index()?;
    let mut incomplete = Vec::new();
    if spec.scope == "fixed" {
        for target in &spec.targets {
            let acct = state
                .configured_account(&target.account_id)
                .context("智能文件夹来源账户已删除，请修复范围")?;
            if acct.receive_epoch != target.receive_epoch {
                bail!("智能文件夹来源已更换接收服务器，请修复范围");
            }
            if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
                bail!("POP3不支持此持久搜索范围");
            }
            if index.has_folder_catalog(&target.account_id, target.receive_epoch)? {
                if !index.catalog_contains(target)? {
                    bail!("智能文件夹来源文件夹已失效，请修复范围");
                }
            } else {
                incomplete.push(format!("{}：文件夹列表尚未完整同步", acct.email));
            }
        }
        return Ok((spec.targets.clone(), incomplete));
    }
    let accounts = state.store.lock().unwrap().accounts.clone();
    let mut targets = Vec::new();
    for acct in accounts {
        if acct.receive_protocol == config::ReceiveProtocol::Pop3 {
            incomplete.push(format!("{}：POP3未纳入持久搜索范围", acct.email));
            continue;
        }
        let complete = index.has_folder_catalog(&acct.id, acct.receive_epoch)?;
        if !complete {
            incomplete.push(format!("{}：文件夹列表尚未完整同步", acct.email));
        }
        let folders = if complete {
            index.catalog_folders(&acct.id, acct.receive_epoch)?
        } else {
            index.saved_folders(&acct.id, acct.receive_epoch)?
        };
        for folder in folders {
            if !spec.include_excluded && matches!(folder.role.as_str(), "junk" | "trash") {
                continue;
            }
            targets.push(SearchTarget {
                account_id: acct.id.clone(),
                receive_epoch: acct.receive_epoch,
                folder: folder.path,
            });
        }
    }
    Ok((targets, incomplete))
}

/// Read saved definitions and explain invalid current scope without changing persisted rules.
#[tauri::command]
pub fn list_saved_views(state: State<'_, AppState>) -> CmdResult<Vec<saved_views::SavedView>> {
    let mut views = state
        .search_index()
        .and_then(|i| i.saved_views())
        .map_err(err)?;
    for view in &mut views {
        if view.repair_reason.is_none() {
            if let Ok(spec) = serde_json::from_value::<SearchSpec>(view.spec.clone()) {
                view.repair_reason = resolve_targets(&state, &spec).err().map(|e| e.to_string());
            }
        }
    }
    Ok(views)
}

/// Save the explicitly supplied condition set; transient query changes never call this implicitly.
#[tauri::command]
pub fn save_saved_view(
    state: State<'_, AppState>,
    id: Option<String>,
    name: String,
    position: i64,
    spec: serde_json::Value,
    expected_revision: Option<u64>,
) -> CmdResult<saved_views::SavedView> {
    let parsed: SearchSpec = serde_json::from_value(spec.clone()).map_err(|e| e.to_string())?;
    resolve_targets(&state, &parsed).map_err(err)?;
    state
        .search_index()
        .and_then(|i| i.save_view(id, name, position, spec, expected_revision))
        .map_err(err)
}

/// Delete only the named saved definition at the expected revision.
#[tauri::command]
pub fn delete_saved_view(
    state: State<'_, AppState>,
    id: String,
    expected_revision: u64,
) -> CmdResult<()> {
    state
        .search_index()
        .and_then(|i| i.delete_view(&id, expected_revision))
        .map_err(err)
}

/// Execute saved or temporary rules including empty-keyword queries and stable pagination.
#[tauri::command]
pub fn search_saved_view(
    state: State<'_, AppState>,
    app: AppHandle,
    id: Option<String>,
    spec: Option<SearchSpec>,
    cursor: Option<String>,
) -> CmdResult<SearchResponse> {
    let index = state.search_index().map_err(err)?;
    let spec = match spec {
        Some(s) => s,
        None => {
            let id = id.ok_or("请选择一个智能文件夹或明确查询条件")?;
            let view = index
                .saved_views()
                .map_err(err)?
                .into_iter()
                .find(|v| v.id == id)
                .ok_or("智能文件夹已删除")?;
            serde_json::from_value(view.spec).map_err(|e| format!("规则需要修复：{e}"))?
        }
    };
    let (targets, incomplete) = resolve_targets(&state, &spec).map_err(err)?;
    let request = spec.request(targets.clone(), cursor).map_err(err)?;
    let mut page = index.search_page(&request).map_err(err)?;
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
    Ok(SearchResponse {
        page,
        catalog_incomplete: incomplete,
    })
}

/// Enumerate metadata without fetching bodies; an empty explicit target list means all current IMAP accounts.
#[tauri::command]
pub fn list_attachments(
    state: State<'_, AppState>,
    mut input: attachment_index::Query,
) -> CmdResult<attachment_index::Page> {
    let spec = SearchSpec {
        scope: if input.targets.is_empty() {
            "all"
        } else {
            "fixed"
        }
        .into(),
        targets: input.targets.clone(),
        ..Default::default()
    };
    let (targets, incomplete) = resolve_targets(&state, &spec).map_err(err)?;
    input.targets = targets;
    let mut page = state
        .search_index()
        .and_then(|i| i.attachment_page(&input))
        .map_err(err)?;
    page.catalog_incomplete = incomplete;
    Ok(page)
}

async fn read_source(
    state: &AppState,
    source: &mail_ref::MessageRef,
    local_only: bool,
) -> Result<Arc<Vec<u8>>> {
    source.validate()?;
    let acct = state.configured_account(&source.account_id)?;
    if acct.receive_protocol != config::ReceiveProtocol::Imap {
        bail!("附件中心只支持已确认持久身份的IMAP邮件");
    }
    source.validate_current(acct.receive_epoch, source.uid_validity)?;
    state.search_index()?.snapshot_reference(source)?;
    let raw = raw_at_context(
        state,
        &source.account_id,
        &source.folder,
        source.uid,
        Some(source.uid_validity),
        Some(source.receive_epoch),
        local_only,
    )
    .await?;
    state.search_index()?.snapshot_reference(source)?;
    Ok(raw)
}

/// Explicitly enrich exactly one chosen message, reusing the offline cache before network.
#[tauri::command]
pub async fn index_message_attachments(
    state: State<'_, AppState>,
    source: mail_ref::MessageRef,
    local_only: Option<bool>,
) -> CmdResult<Vec<attachment_index::Item>> {
    let _gate = state.account_gate(&source.account_id).read_owned().await;
    let raw = read_source(&state, &source, local_only.unwrap_or(false))
        .await
        .map_err(err)?;
    let index = state.search_index().map_err(err)?.clone();
    tokio::task::spawn_blocking(move || {
        index.index_attachments(&source, &raw)?;
        attachment_index::parse(&source, &raw, 0)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(err)
}

async fn read_attachment(
    state: &AppState,
    item: &AttachmentRef,
    local_only: bool,
) -> Result<(String, String, Vec<u8>)> {
    let raw = read_source(state, &item.source, local_only).await?;
    let index = state.search_index()?.clone();
    let item = item.clone();
    tokio::task::spawn_blocking(move || {
        let result = attachment_index::verified_bytes(&item, &raw);
        if result.is_err() {
            let _ = index.index_attachments(&item.source, &raw);
        }
        result
    })
    .await
    .context("附件解析任务失败")?
}

/// Preview only verified PDF or raster images. Request registration precedes asynchronous reads.
#[tauri::command]
pub async fn preview_attachment(
    state: State<'_, AppState>,
    app: AppHandle,
    request_id: String,
    item: AttachmentRef,
    local_only: Option<bool>,
) -> CmdResult<attachment_preview::Preview> {
    let mut cancellation =
        attachment_preview::register(&request_id, &item.source.account_id).map_err(err)?;
    let result = async {
        let _gate = state
            .account_gate(&item.source.account_id)
            .read_owned()
            .await;
        if *cancellation.borrow() {
            bail!("预览已取消");
        }
        // A shared IMAP lane must drain its in-flight response before releasing the
        // connection. UI cancellation is immediate, but discarding this future mid-FETCH
        // would leave unread protocol bytes for the next command.
        let (name, _declared, data) = attachment_preview::drain_on_cancel(
            &mut cancellation,
            read_attachment(&state, &item, local_only.unwrap_or(false)),
        )
        .await?;
        if *cancellation.borrow() {
            bail!("预览已取消");
        }
        let kind = attachment_preview::content_kind(&data)?;
        if kind == "application/pdf" {
            attachment_preview::native_pdf(app, request_id.clone(), name, data, cancellation).await
        } else {
            attachment_preview::image(&request_id, name, &data)
        }
    }
    .await;
    if !matches!(result.as_ref(),Ok(p) if p.kind=="nativePdf") {
        attachment_preview::finish(&request_id);
    }
    result.map_err(err)
}

/// Closing or switching preview cancels pending content reads and terminates native PDF windows.
#[tauri::command]
pub fn cancel_attachment_preview(request_id: String) -> CmdResult<()> {
    attachment_preview::cancel(&request_id).map_err(err)
}

/// Open or export the exact bytes verified against the listed hash, never re-read by a stale index.
#[tauri::command]
pub async fn indexed_attachment_action(
    state: State<'_, AppState>,
    app: AppHandle,
    item: AttachmentRef,
    action: String,
    local_only: Option<bool>,
) -> CmdResult<String> {
    if !matches!(action.as_str(), "open" | "save") {
        return Err("附件操作无效".into());
    }
    let _gate = state
        .account_gate(&item.source.account_id)
        .read_owned()
        .await;
    let (name, _mime, data) = read_attachment(&state, &item, local_only.unwrap_or(false))
        .await
        .map_err(err)?;
    let dir = if action == "save" {
        dirs::download_dir()
            .or_else(dirs::home_dir)
            .ok_or("找不到下载文件夹")?
    } else {
        state
            .store
            .lock()
            .unwrap()
            .dir()
            .join("indexed-attachments")
            .join(&item.raw_hash)
            .join(item.part_id.to_string())
    };
    let path = tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        save_attachment_file(dir, &name, &data)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(err)?;
    if action == "open" {
        #[cfg(target_os = "macos")]
        {
            let opened = path.clone();
            tokio::task::spawn_blocking(move || open_with_system(&opened))
                .await
                .map_err(|e| e.to_string())??;
        }
        #[cfg(not(target_os = "macos"))]
        app.opener()
            .open_path(path.to_string_lossy(), None::<&str>)
            .map_err(|e| e.to_string())?;
    } else {
        let _ = app.opener().reveal_item_in_dir(&path);
    }
    Ok(path.to_string_lossy().into_owned())
}
