//! Opt-in updates (ARCHITECTURE.md 16).
//!
//! - The updater plugin is registered only when the build carries an updater public key
//!   (`plugins.updater.pubkey`, added by the release build from
//!   `tauri.updater.conf.json`). Without it every command here reports "not available".
//! - Nothing is checked unless the user clicks *Check now* or turns on automatic checks.
//! - The check, download and signature verification run in Rust; the webview has no
//!   updater permission. The plugin refuses an update that is not newer than this
//!   version, so an old signed installer cannot be passed off as an update.
//! - Before installing, llama-server and every tool child are stopped: on Windows the
//!   installer cannot replace files a running process holds open.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::UpdaterExt;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Setting: check for updates when the app starts.
const AUTO_CHECK: &str = "updateAutoCheck";

/// Whether this build has an updater key.
pub fn configured(config: &tauri::Config) -> bool {
    config
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .is_some_and(|k| !k.trim().is_empty())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub configured: bool,
    pub auto_check: bool,
    pub current_version: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub notes: Option<String>,
}

fn not_configured() -> AppError {
    AppError::new(
        "updates_unavailable",
        "Updates are not available in this build.",
        "Download new versions from the project's GitHub releases.",
        "This build has no updater signing key.",
    )
}

#[tauri::command]
pub async fn update_status(app: AppHandle, state: State<'_, AppState>) -> AppResult<UpdateStatus> {
    Ok(UpdateStatus {
        configured: configured(app.config()),
        auto_check: state
            .store
            .get_setting(AUTO_CHECK)
            .await
            .ok()
            .flatten()
            .unwrap_or(false),
        current_version: app.package_info().version.to_string(),
    })
}

#[tauri::command]
pub async fn set_update_auto_check(state: State<'_, AppState>, on: bool) -> AppResult<()> {
    state.store.set_setting(AUTO_CHECK, &on).await?;
    Ok(())
}

async fn find(app: &AppHandle) -> AppResult<Option<tauri_plugin_updater::Update>> {
    if !configured(app.config()) {
        return Err(not_configured());
    }
    let updater = app
        .updater()
        .map_err(|e| AppError::internal(e.to_string()))?;
    updater.check().await.map_err(|e| {
        AppError::new(
            "update_check",
            "Could not check for updates.",
            "Check your internet connection and try again.",
            e.to_string(),
        )
    })
}

#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> AppResult<Option<UpdateInfo>> {
    Ok(find(&app).await?.map(|u| UpdateInfo {
        version: u.version.clone(),
        date: u.date.map(|d| d.to_string()),
        notes: u.body.clone(),
    }))
}

/// Downloads, verifies and installs the update, then restarts the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    let Some(update) = find(&app).await? else {
        return Err(AppError::bad_request("There is no newer version."));
    };
    let bytes = update.download(|_, _| {}, || {}).await.map_err(|e| {
        AppError::new(
            "update_download",
            "The update could not be downloaded or its signature did not verify.",
            "Try again later, or download the new version from GitHub.",
            e.to_string(),
        )
    })?;
    // Stop every child before the installer runs.
    if let Ok(mut g) = state.generation.lock()
        && let Some(t) = g.take()
    {
        t.cancel();
    }
    if let Some(browser) = app.try_state::<crate::browser_cmd::BrowserHandle>() {
        browser.0.shutdown().await;
    }
    if let Some(c) = app.try_state::<crate::computer_cmd::ComputerHandle>()
        && let Some(server) = c.0.clone()
    {
        server.release_all();
    }
    state.host.shutdown().await;
    state.runtime.shutdown().await;
    update.install(bytes).map_err(|e| {
        AppError::new(
            "update_install",
            "The update could not be installed.",
            "Download the new version from GitHub.",
            e.to_string(),
        )
    })?;
    app.restart();
}

/// At start-up, when the user turned automatic checks on: tells the main window about
/// a newer version. Never installs on its own.
pub async fn auto_check(app: AppHandle) {
    let state = app.state::<AppState>();
    let on: bool = state
        .store
        .get_setting(AUTO_CHECK)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    if !on || !configured(app.config()) {
        return;
    }
    match find(&app).await {
        Ok(Some(u)) => {
            let _ = app.emit_to(
                crate::MAIN_WINDOW,
                "update-available",
                UpdateInfo {
                    version: u.version.clone(),
                    date: u.date.map(|d| d.to_string()),
                    notes: u.body.clone(),
                },
            );
        }
        Ok(None) => {}
        Err(e) => tracing::info!("update check failed: {}", e.message),
    }
}
