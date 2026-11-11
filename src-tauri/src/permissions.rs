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

/// The app's own folders, as Tauri resolves them on this OS.
struct AppDirs {
    local_data: Option<PathBuf>,
    data: Option<PathBuf>,
    /// Logs, diagnostics reports and MCP server logs. On macOS this is
    /// ~/Library/Logs/<id>, outside the data folders; elsewhere it is inside local data.
    logs: Option<PathBuf>,
    install: Option<PathBuf>,
}

impl AppDirs {
    fn forbidden(self) -> Vec<PathBuf> {
        [self.local_data, self.data, self.logs, self.install]
            .into_iter()
            .flatten()
            .collect()
    }
}

fn forbidden_folders(app: &AppHandle) -> Vec<PathBuf> {
    AppDirs {
        local_data: app.path().app_local_data_dir().ok(),
        data: app.path().app_data_dir().ok(),
        logs: app.path().app_log_dir().ok(),
        install: std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(Path::to_path_buf)),
    }
    .forbidden()
}

/// Whether approving `path` would let tools reach `forbidden`: it is inside it, or holds it.
fn overlaps(path: &Path, forbidden: &Path) -> bool {
    path.starts_with(forbidden) || forbidden.starts_with(path)
}

/// The folders tools may never reach (13.4 step 1): app data, logs, install folder, browser
/// profile.
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
        if overlaps(&path, &f) {
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

/// Sets the chat's working folder, which relative paths resolve against (ARCHITECTURE.md
/// 9.3). `None` goes back to the default, the first approved folder. Only a folder
/// approved for this chat, globally or for the chat itself, can be chosen.
#[tauri::command]
pub async fn set_chat_working_folder(
    state: State<'_, AppState>,
    chat_id: i64,
    path: Option<String>,
) -> AppResult<()> {
    set_working_folder(&state.store, store::ChatId(chat_id), path).await
}

async fn set_working_folder(
    store: &store::Store,
    chat: store::ChatId,
    path: Option<String>,
) -> AppResult<()> {
    if let Some(p) = &path {
        let approved = store.approved_folders_for_chat(chat).await?;
        if !approved.iter().any(|f| &f.path == p) {
            return Err(AppError::bad_request(
                "Only a folder approved for this chat can be its working folder.",
            ));
        }
    }
    store
        .update_chat(
            chat,
            store::ChatPatch {
                working_folder: Some(path),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn working_folder(store: &store::Store, chat: store::ChatId) -> Option<String> {
        store.get_chat(chat).await.unwrap().unwrap().working_folder
    }

    /// ARCHITECTURE.md 9.3: a chat's working folder can only be a folder approved for
    /// that chat, globally or for the chat itself.
    #[tokio::test]
    async fn the_working_folder_must_be_approved_for_the_chat() {
        let store = store::Store::open_in_memory().unwrap();
        let new_chat = || store::NewChat {
            title: "t".into(),
            model_ref: None,
            system_prompt: None,
        };
        let a = store.create_chat(new_chat()).await.unwrap();
        let b = store.create_chat(new_chat()).await.unwrap();
        store
            .add_approved_folder("/work/global".into(), None)
            .await
            .unwrap();
        store
            .add_approved_folder("/work/a".into(), Some(a))
            .await
            .unwrap();
        store
            .add_approved_folder("/work/b".into(), Some(b))
            .await
            .unwrap();

        set_working_folder(&store, a, Some("/work/global".into()))
            .await
            .unwrap();
        assert_eq!(
            working_folder(&store, a).await.as_deref(),
            Some("/work/global")
        );
        set_working_folder(&store, a, Some("/work/a".into()))
            .await
            .unwrap();
        assert_eq!(working_folder(&store, a).await.as_deref(), Some("/work/a"));

        // Another chat's folder, an unapproved folder, one inside an approved folder and
        // an empty path are refused, and the stored folder stays as it was.
        for refused in ["/work/b", "/elsewhere", "/work/a/sub", "/work/a/../b", ""] {
            let err = set_working_folder(&store, a, Some(refused.into()))
                .await
                .expect_err(refused);
            assert_eq!(err.code, "bad_request", "{refused}");
            assert_eq!(
                working_folder(&store, a).await.as_deref(),
                Some("/work/a"),
                "{refused} changed the working folder"
            );
        }

        // None goes back to the default (the first approved folder).
        set_working_folder(&store, a, None).await.unwrap();
        assert_eq!(working_folder(&store, a).await, None);
        // A chat that does not exist is an error.
        assert!(
            set_working_folder(&store, store::ChatId(999), None)
                .await
                .is_err()
        );
    }

    /// macOS keeps logs in ~/Library/Logs/<id>, outside the data folders: tools never
    /// reach them, and ~/Library/Logs itself cannot be approved.
    #[test]
    fn the_log_folder_is_forbidden() {
        let home = Path::new("/Users/me");
        let logs = home.join("Library/Logs/app.harness");
        let forbidden = AppDirs {
            local_data: Some(home.join("Library/Application Support/app.harness")),
            data: Some(home.join("Library/Application Support/app.harness")),
            logs: Some(logs.clone()),
            install: Some(PathBuf::from("/Applications/harnessUI.app/Contents/MacOS")),
        }
        .forbidden();
        assert!(forbidden.contains(&logs));
        let refused = |p: &Path| forbidden.iter().any(|f| overlaps(p, f));
        assert!(refused(&home.join("Library/Logs")));
        assert!(refused(&logs.join("harness.2026-11-10.log")));
        assert!(refused(&home.join("Library")));
        assert!(!refused(&home.join("Library/Logs/OtherApp")));
        assert!(!refused(&home.join("Documents")));
    }
}
