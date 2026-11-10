//! The browser tool in the app (ARCHITECTURE.md 9.3 "Browser", 13.3, D8): settings, the
//! approval cards for held navigations, start-up registration and shutdown.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Setting key for [`BrowserSettings`].
const SETTINGS: &str = "browserSettings";

/// The user's browser settings.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowserSettings {
    /// A browser executable chosen in Settings, tried first.
    #[serde(default)]
    pub path: Option<String>,
    /// Use a loopback debugging port instead of a pipe (for sites that refuse automated
    /// browsers; leaves an unauthenticated local port open while the browser runs).
    #[serde(default)]
    pub loopback_port: bool,
    /// Let pages load from private networks (routers, NAS, intranet).
    #[serde(default)]
    pub allow_private_networks: bool,
    /// Sites in strict mode: background data sends are held like form submissions.
    #[serde(default)]
    pub strict_sites: Vec<String>,
}

/// The browser server, kept to reconfigure and shut it down.
pub struct BrowserHandle(pub Arc<browser::BrowserServer>);

/// The agent browser's profile folder (never the user's own profile).
pub fn profile_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|d| d.join("browser-profile"))
}

fn config_from(app: &AppHandle, s: &BrowserSettings) -> browser::BrowserConfig {
    let mut c = browser::BrowserConfig::new(
        profile_dir(app).unwrap_or_else(|| PathBuf::from("browser-profile")),
    );
    c.browser_path = s.path.as_ref().map(PathBuf::from);
    c.connection = if s.loopback_port {
        browser::ConnectionMode::LoopbackPort
    } else {
        browser::ConnectionMode::Pipe
    };
    c.allow_private_networks = s.allow_private_networks;
    c.strict_sites = s.strict_sites.clone();
    c
}

/// Held navigations become always-ask cards in the chat (13.3, "Approvals raised during a
/// call"); Deny, Stop and the timeout fail the request.
struct AppNavigationApprover {
    app: AppHandle,
}

#[async_trait]
impl browser::NavigationApprover for AppNavigationApprover {
    async fn approve(
        &self,
        chat_id: i64,
        request: browser::HeldNavigation,
        cancel: CancellationToken,
    ) -> bool {
        let tool = request
            .during_tool
            .clone()
            .unwrap_or_else(|| "browser_navigate".into());
        crate::turns::ask_during_call(
            &self.app,
            crate::turns::DuringCall {
                chat_id,
                server: "browser".into(),
                tool,
                summary: request.summary.clone(),
                preview: request.preview.clone(),
            },
            cancel,
            // The browser's own approval timeout fires `cancel` first.
            std::time::Duration::from_secs(3600),
        )
        .await
    }
}

/// Creates the browser server from the saved settings (it starts a browser on first use).
pub async fn build(app: &AppHandle, store: &store::Store) -> Arc<browser::BrowserServer> {
    let settings: BrowserSettings = store
        .get_setting(SETTINGS)
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    Arc::new(browser::BrowserServer::new(
        config_from(app, &settings),
        Arc::new(AppNavigationApprover { app: app.clone() }),
    ))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub settings: BrowserSettings,
    /// The browser that would be used, if any.
    pub found: Option<String>,
    pub kind: Option<String>,
    /// Why there is none (message and next step).
    pub unavailable: Option<String>,
    pub running: bool,
}

#[tauri::command]
pub async fn browser_status(
    state: State<'_, AppState>,
    handle: State<'_, BrowserHandle>,
) -> AppResult<BrowserStatus> {
    let settings: BrowserSettings = state.store.get_setting(SETTINGS).await?.unwrap_or_default();
    let (found, kind, unavailable) = match handle.0.availability() {
        Ok(f) => (
            Some(f.path.display().to_string()),
            Some(format!("{:?}", f.kind)),
            None,
        ),
        Err(e) => (None, None, Some(format!("{} {}", e.message, e.next_step))),
    };
    Ok(BrowserStatus {
        settings,
        found,
        kind,
        unavailable,
        running: handle.0.is_running().await,
    })
}

#[tauri::command]
pub async fn set_browser_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: State<'_, BrowserHandle>,
    mut settings: BrowserSettings,
) -> AppResult<()> {
    settings.strict_sites = settings
        .strict_sites
        .iter()
        .filter_map(|s| url::Url::parse(s.trim()).ok())
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .filter_map(|u| policy::sanitize::origin(&u))
        .collect();
    // The path is only ever set through the native picker below.
    let saved: BrowserSettings = state.store.get_setting(SETTINGS).await?.unwrap_or_default();
    settings.path = saved.path;
    state.store.set_setting(SETTINGS, &settings).await?;
    handle.0.reconfigure(config_from(&app, &settings)).await;
    Ok(())
}

/// Picks a browser executable in a native dialog (or clears the choice).
#[tauri::command]
pub async fn choose_browser_path(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: State<'_, BrowserHandle>,
    clear: bool,
) -> AppResult<Option<String>> {
    let mut settings: BrowserSettings =
        state.store.get_setting(SETTINGS).await?.unwrap_or_default();
    if clear {
        settings.path = None;
    } else {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.dialog()
            .file()
            .set_title("Choose Chrome, Edge or Chromium")
            .pick_file(move |p| {
                let _ = tx.send(p);
            });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(settings.path);
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::internal(e.to_string()))?;
        if browser::find_browser(Some(&path))
            .map(|f| f.path)
            .ok()
            .as_deref()
            != Some(path.as_path())
        {
            return Err(AppError::bad_request(
                "That file is not a Chrome, Edge or Chromium browser.",
            ));
        }
        settings.path = Some(path.display().to_string());
    }
    state.store.set_setting(SETTINGS, &settings).await?;
    handle.0.reconfigure(config_from(&app, &settings)).await;
    Ok(settings.path)
}
