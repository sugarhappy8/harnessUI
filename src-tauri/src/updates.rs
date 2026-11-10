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
    /// The newer version the last check since start found (automatic or *Check now*).
    pub available: Option<UpdateInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub notes: Option<String>,
}

impl UpdateInfo {
    fn of(u: &tauri_plugin_updater::Update) -> Self {
        Self {
            version: u.version.clone(),
            date: u.date.map(|d| d.to_string()),
            notes: u.body.clone(),
        }
    }
}

/// The answer of the last successful check since start. The startup check can finish
/// before the window listens for `update-available` (events are not replayed), so the
/// window also reads it from `update_status`.
#[derive(Default)]
pub struct LastFound(std::sync::Mutex<Option<UpdateInfo>>);

impl LastFound {
    /// Records a check's answer: the newer version, or `None` when there is none.
    pub fn set(&self, found: Option<UpdateInfo>) {
        if let Ok(mut g) = self.0.lock() {
            *g = found;
        }
    }

    pub fn get(&self) -> Option<UpdateInfo> {
        self.0.lock().ok().and_then(|g| g.clone())
    }
}

/// Records a successful check's answer, if the app manages a [`LastFound`].
fn remember(app: &AppHandle, found: Option<UpdateInfo>) {
    if let Some(last) = app.try_state::<LastFound>() {
        last.set(found);
    }
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
        available: app.try_state::<LastFound>().and_then(|l| l.get()),
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
    let found = find(&app).await?.as_ref().map(UpdateInfo::of);
    remember(&app, found.clone());
    Ok(found)
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
/// a newer version (and keeps it for `update_status`). Never installs on its own.
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
            let info = UpdateInfo::of(&u);
            remember(&app, Some(info.clone()));
            let _ = app.emit_to(crate::MAIN_WINDOW, "update-available", info);
        }
        Ok(None) => remember(&app, None),
        Err(e) => tracing::info!("update check failed: {}", e.message),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(version: &str) -> UpdateInfo {
        UpdateInfo {
            version: version.into(),
            date: None,
            notes: Some("Fixes.".into()),
        }
    }

    /// The last check's answer is kept: a found version until a later check says there is
    /// none; a failed check (which records nothing) leaves it as it was.
    #[test]
    fn the_last_found_update_is_kept() {
        let last = LastFound::default();
        assert_eq!(last.get(), None);
        last.set(Some(info("1.2.0")));
        assert_eq!(last.get(), Some(info("1.2.0")));
        last.set(Some(info("1.3.0")));
        assert_eq!(last.get().map(|i| i.version), Some("1.3.0".to_string()));
        last.set(None);
        assert_eq!(last.get(), None);
    }

    /// `update_status` as the window reads it (src/lib/types.ts `UpdateStatus`).
    #[test]
    fn the_status_carries_the_available_update() {
        let mut status = UpdateStatus {
            configured: true,
            auto_check: true,
            current_version: "1.0.0".into(),
            available: Some(info("1.2.0")),
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(
            v["available"],
            serde_json::json!({"version": "1.2.0", "date": null, "notes": "Fixes."})
        );
        assert_eq!(v["currentVersion"], "1.0.0");
        status.available = None;
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["available"], serde_json::Value::Null);
    }
}
