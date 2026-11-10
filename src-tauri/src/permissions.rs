//! Approved folders and the audit log (ARCHITECTURE.md 9.3 Files, 13.4, 13.5, 13.7).
//!
//! Folders are added through a native folder picker opened from Rust, so a path is
//! never taken from the webview; approving a home folder or a drive root asks again.
//! The audit log is read-only here; it is exported through a native save dialog.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderDto {
    pub id: i64,
    pub path: String,
    /// `None` = approved for every chat.
    pub chat_id: Option<i64>,
}

#[tauri::command]
pub async fn list_approved_folders(
    state: State<'_, AppState>,
    chat_id: Option<i64>,
) -> AppResult<Vec<FolderDto>> {
    let rows = match chat_id {
        Some(id) => {
            state
                .store
                .approved_folders_for_chat(store::ChatId(id))
                .await?
        }
        None => state
            .store
            .list_approved_folders()
            .await?
            .into_iter()
            .filter(|f| f.chat_id.is_none())
            .collect(),
    };
    Ok(rows
        .into_iter()
        .map(|f| FolderDto {
            id: f.id.0,
            path: f.path,
            chat_id: f.chat_id.map(|c| c.0),
        })
        .collect())
}

/// Whether `path` is a home folder or a drive / volume root.
fn is_broad(app: &AppHandle, path: &Path) -> bool {
    let home = app
        .path()
        .home_dir()
        .ok()
        .and_then(|h| dunce::canonicalize(h).ok());
    let p = dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let is_root = p.parent().is_none()
        || p.parent()
            .is_some_and(|parent| parent.as_os_str().is_empty());
    let is_users_dir = p.file_name().is_some_and(|n| {
        let n = n.to_string_lossy().to_ascii_lowercase();
        n == "users" || n == "home"
    }) && p.parent().is_some_and(|pp| pp.parent().is_none());
    is_root || is_users_dir || home.as_deref() == Some(p.as_path())
}

fn forbidden_folders(app: &AppHandle) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(d) = app.path().app_local_data_dir() {
        out.push(d);
    }
    if let Ok(d) = app.path().app_data_dir() {
        out.push(d);
    }
    if let Some(d) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        out.push(d);
    }
    out
}

/// The folders tools may never reach (13.4 step 1): app data, install folder, browser profile.
pub fn forbidden_for_tools(app: &AppHandle) -> Vec<String> {
    forbidden_folders(app)
        .into_iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// Opens a folder picker and approves the chosen folder, globally or for one chat.
#[tauri::command]
pub async fn approve_folder(
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: Option<i64>,
) -> AppResult<Option<FolderDto>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Approve a folder")
        .pick_folder(move |p| {
            let _ = tx.send(p);
        });
    let Some(picked) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    let path = picked
        .into_path()
        .map_err(|e| AppError::internal(e.to_string()))?;
    let path = dunce::canonicalize(&path).unwrap_or(path);
    // The app's own folders can never be approved.
    for f in forbidden_folders(&app) {
        let f = dunce::canonicalize(&f).unwrap_or(f);
        if path.starts_with(&f) || f.starts_with(&path) {
            return Err(AppError::bad_request(
                "That folder contains the app's own data or program files, which tools can never reach. Pick a different folder.",
            ));
        }
    }
    if is_broad(&app, &path) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.dialog()
            .message(format!(
                "{} is your whole home folder or a drive. Tools will be able to read, and with your approval change, everything in it.\n\nApprove it anyway?",
                path.display()
            ))
            .title("Approve a broad folder?")
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancelCustom("Approve".into(), "Cancel".into()))
            .show(move |ok| {
                let _ = tx.send(ok);
            });
        if !rx.await.unwrap_or(false) {
            return Ok(None);
        }
    }
    let id = state
        .store
        .add_approved_folder(path.display().to_string(), chat_id.map(store::ChatId))
        .await?;
    tracing::info!(
        folder_id = id.0,
        global = chat_id.is_none(),
        "folder approved"
    );
    Ok(Some(FolderDto {
        id: id.0,
        path: path.display().to_string(),
        chat_id,
    }))
}

#[tauri::command]
pub async fn remove_approved_folder(state: State<'_, AppState>, id: i64) -> AppResult<()> {
    state
        .store
        .remove_approved_folder(store::FolderId(id))
        .await?;
    Ok(())
}

// ---- Audit log ------------------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditFilter {
    pub chat_id: Option<i64>,
    pub tool: Option<String>,
    pub decision: Option<String>,
    pub since: Option<i64>,
    pub limit: Option<u32>,
}

fn query(f: &AuditFilter) -> AppResult<store::AuditQuery> {
    let decision = match f.decision.as_deref().filter(|d| !d.is_empty()) {
        Some(d) => Some(
            d.parse::<store::AuditDecision>()
                .map_err(|_| AppError::bad_request("Unknown decision filter."))?,
        ),
        None => None,
    };
    Ok(store::AuditQuery {
        chat_id: f.chat_id.map(store::ChatId),
        server: None,
        tool: f.tool.clone().filter(|t| !t.is_empty()),
        decision,
        since: f.since,
        until: None,
        limit: f.limit.unwrap_or(500).min(10_000),
    })
}

#[tauri::command]
pub async fn list_audit(
    state: State<'_, AppState>,
    filter: AuditFilter,
) -> AppResult<Vec<store::AuditRow>> {
    Ok(state.store.list_audit(query(&filter)?).await?)
}

/// Exports the filtered audit log as JSON Lines or CSV through a native save dialog.
#[tauri::command]
pub async fn export_audit(
    app: AppHandle,
    state: State<'_, AppState>,
    filter: AuditFilter,
    format: String,
) -> AppResult<Option<String>> {
    let q = query(&AuditFilter {
        limit: Some(1_000_000),
        ..filter
    })?;
    let (body, ext) = match format.as_str() {
        "csv" => (state.store.export_audit_csv(q).await?, "csv"),
        _ => (state.store.export_audit_jsonl(q).await?, "jsonl"),
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export the audit log")
        .set_file_name(format!("audit-log.{ext}"))
        .add_filter(ext.to_uppercase(), &[ext])
        .save_file(move |p| {
            let _ = tx.send(p);
        });
    let Some(dest) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    let dest = dest
        .into_path()
        .map_err(|e| AppError::internal(e.to_string()))?;
    tokio::fs::write(&dest, body).await.map_err(|e| {
        AppError::new(
            "export_failed",
            "The audit log could not be saved.",
            "Pick another folder.",
            e.to_string(),
        )
    })?;
    Ok(Some(dest.display().to_string()))
}
