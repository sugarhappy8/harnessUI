//! The browser tool in the app (ARCHITECTURE.md 9.3 "Browser", 13.3, D8): settings, the
//! approval cards for held navigations, start-up registration and shutdown, and the
//! one-click download of the agent's own browser on a Mac that has none.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use runtime::packs::PackError;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
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

/// The same server, for code that has no window state at hand (the chat's tool list).
static SERVER: OnceLock<Arc<browser::BrowserServer>> = OnceLock::new();

/// The download of the agent's own browser that is running, if one is (one at a time).
static DOWNLOAD: Mutex<Option<CancellationToken>> = Mutex::new(None);

/// Event: progress of the agent browser download ([`AgentBrowserProgress`]).
const PROGRESS_EVENT: &str = "agent-browser-progress";

/// Why the browser tools cannot work right now (no browser was found), if they cannot.
/// The tool list marks them unavailable with this reason.
pub fn unavailable_reason() -> Option<String> {
    SERVER.get()?.availability().err().map(|e| e.message)
}

/// The agent browser's profile folder (never the user's own profile).
pub fn profile_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|d| d.join("browser-profile"))
}

/// Where the agent's own browser is downloaded to: beside its profile, in the app's
/// local data folder (which no tool can reach, 13.4).
pub fn own_browser_dir(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|d| d.join("agent-browser"))
}

fn config_from(app: &AppHandle, s: &BrowserSettings) -> browser::BrowserConfig {
    let mut c = browser::BrowserConfig::new(
        profile_dir(app).unwrap_or_else(|| PathBuf::from("browser-profile")),
    );
    c.browser_path = s.path.as_ref().map(PathBuf::from);
    c.own_browser_dir = own_browser_dir(app);
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
    let server = Arc::new(browser::BrowserServer::new(
        config_from(app, &settings),
        Arc::new(AppNavigationApprover { app: app.clone() }),
    ));
    let _ = SERVER.set(server.clone());
    server
}

/// The agent's own browser, for Settings → Browser.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct OwnBrowserStatus {
    /// True where the download is offered (macOS with a pinned build for this processor).
    /// The page shows the download only when this is true; it never looks at the OS.
    pub offered: bool,
    /// "notInstalled", "partial", "downloading" or "installed".
    pub state: String,
    /// The installed version, when installed.
    pub version: Option<String>,
    /// The version a download would install.
    pub pinned_version: String,
    /// Size of the download in bytes (0 when none is offered).
    pub download_bytes: u64,
    /// Bytes already on disk from a stopped download.
    pub bytes_done: u64,
    /// True when the browser in use is this one.
    pub in_use: bool,
}

/// Pure: the status from what is on disk and what is running.
fn own_status(
    offered: Option<&browser::own::BrowserBuild>,
    pinned_version: &str,
    on_disk: &browser::own::OwnState,
    downloading: bool,
    in_use: bool,
) -> OwnBrowserStatus {
    use browser::own::OwnState;
    let (state, version, bytes_done) = match on_disk {
        OwnState::Installed(b) => ("installed", Some(b.version.clone()), 0),
        _ if downloading => ("downloading", None, 0),
        OwnState::Partial { bytes } => ("partial", None, *bytes),
        OwnState::NotInstalled => ("notInstalled", None, 0),
    };
    OwnBrowserStatus {
        offered: offered.is_some(),
        state: state.to_string(),
        version,
        pinned_version: pinned_version.to_string(),
        download_bytes: offered.map(|b| b.file.size).unwrap_or(0),
        bytes_done,
        in_use,
    }
}

fn downloading() -> bool {
    DOWNLOAD.lock().map(|d| d.is_some()).unwrap_or(false)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowserStatus {
    pub settings: BrowserSettings,
    /// The browser that would be used, if any.
    pub found: Option<String>,
    /// Its name ("Google Chrome", "Brave", "The agent's own browser (Chrome for Testing)").
    pub kind: Option<String>,
    /// Why there is none (message and next step).
    pub unavailable: Option<String>,
    pub running: bool,
    /// The browser the app can download for the agent.
    pub own: OwnBrowserStatus,
}

#[tauri::command]
pub async fn browser_status(
    app: AppHandle,
    state: State<'_, AppState>,
    handle: State<'_, BrowserHandle>,
) -> AppResult<BrowserStatus> {
    let settings: BrowserSettings = state.store.get_setting(SETTINGS).await?.unwrap_or_default();
    let availability = handle.0.availability();
    let in_use = availability
        .as_ref()
        .is_ok_and(|f| f.kind == browser::BrowserKind::Own);
    let (found, kind, unavailable) = match availability {
        Ok(f) => (
            Some(f.path.display().to_string()),
            Some(f.kind.label().to_string()),
            None,
        ),
        Err(e) => (None, None, Some(format!("{} {}", e.message, e.next_step))),
    };
    let offered = browser::own::offered_build();
    let on_disk = match own_browser_dir(&app) {
        Some(root) => {
            tauri::async_runtime::spawn_blocking(move || browser::own::state(&root, offered))
                .await
                .map_err(AppError::internal)?
        }
        None => browser::own::OwnState::NotInstalled,
    };
    Ok(BrowserStatus {
        settings,
        found,
        kind,
        unavailable,
        running: handle.0.is_running().await,
        own: own_status(
            offered,
            &browser::own::embedded().chrome_for_testing.version,
            &on_disk,
            downloading(),
            in_use,
        ),
    })
}

/// Progress of the agent browser download, sent to the main window.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBrowserProgress {
    pub done: u64,
    pub total: u64,
    /// True once the download is verified and is being unpacked.
    pub unpacking: bool,
    pub finished: bool,
    pub error: Option<AppError>,
}

fn download_error(e: &PackError) -> AppError {
    match e {
        PackError::ChecksumMismatch { .. } | PackError::SizeMismatch { .. } => AppError::new(
            "checksum_mismatch",
            "The downloaded browser does not match its pinned checksum, so it was not installed.",
            "Try again. If it keeps failing, install Google Chrome or Microsoft Edge instead.",
            e.to_string(),
        ),
        PackError::Network { .. } => AppError::new(
            "network",
            "The browser download was interrupted.",
            "Check your connection and try again; it continues where it stopped.",
            e.to_string(),
        ),
        PackError::DiskFull => AppError::new(
            "disk_full",
            "The disk is full.",
            "Free about 600 MB and try again.",
            e.to_string(),
        ),
        PackError::BlockedHost(_) => AppError::new(
            "blocked_host",
            "The download was redirected to a host the app does not trust.",
            "Install Google Chrome or Microsoft Edge instead, and report this.",
            e.to_string(),
        ),
        _ => AppError::new(
            "agent_browser_failed",
            "The browser could not be installed.",
            "Try again, or install Google Chrome or Microsoft Edge instead.",
            e.to_string(),
        ),
    }
}

/// Tells the main window that what the browser tools can do has changed (the Tools panel
/// re-reads its list on this event).
fn tools_changed(app: &AppHandle) {
    let _ = app.emit_to(crate::MAIN_WINDOW, "mcp-changed", "browser");
}

/// Starts the download of the agent's own browser (macOS only). Returns at once; progress
/// arrives as `agent-browser-progress` events. A second call while one runs does nothing.
#[tauri::command]
pub async fn download_agent_browser(app: AppHandle) -> AppResult<()> {
    let Some(build) = browser::own::offered_build() else {
        return Err(AppError::bad_request(
            "A browser download is not offered on this computer. Install Google Chrome or Microsoft Edge instead.",
        ));
    };
    let root = own_browser_dir(&app)
        .ok_or_else(|| AppError::internal("the app's data folder could not be found"))?;
    let token = CancellationToken::new();
    {
        let mut slot = DOWNLOAD.lock().map_err(AppError::internal)?;
        if slot.is_some() {
            return Ok(());
        }
        *slot = Some(token.clone());
    }
    tauri::async_runtime::spawn(async move {
        let emit = |p: AgentBrowserProgress| {
            let _ = app.emit_to(crate::MAIN_WINDOW, PROGRESS_EVENT, p);
        };
        let last = Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(1));
        let progress = |p: browser::own::OwnProgress| {
            let Ok(mut t) = last.lock() else { return };
            if t.elapsed() < std::time::Duration::from_millis(300) && !p.unpacking {
                return;
            }
            *t = std::time::Instant::now();
            emit(AgentBrowserProgress {
                done: p.done,
                total: p.total,
                unpacking: p.unpacking,
                finished: false,
                error: None,
            });
        };
        let result =
            browser::own::install(browser::own::embedded(), build, &root, &token, &progress).await;
        if let Ok(mut slot) = DOWNLOAD.lock() {
            *slot = None;
        }
        let error = match &result {
            Ok(_) | Err(PackError::Cancelled) => None,
            Err(e) => {
                tracing::warn!("the agent browser download failed: {e}");
                Some(download_error(e))
            }
        };
        if result.is_ok() {
            tools_changed(&app);
        }
        emit(AgentBrowserProgress {
            done: 0,
            total: build.file.size,
            unpacking: false,
            finished: true,
            error,
        });
    });
    Ok(())
}

/// Stops the download; what is already downloaded is kept for the next attempt.
#[tauri::command]
pub async fn cancel_agent_browser_download() -> AppResult<()> {
    if let Ok(slot) = DOWNLOAD.lock()
        && let Some(token) = slot.as_ref()
    {
        token.cancel();
    }
    Ok(())
}

/// Deletes the agent's own browser and any partial download. Its profile (cookies and
/// logins of the agent's browser) is kept.
#[tauri::command]
pub async fn remove_agent_browser(
    app: AppHandle,
    handle: State<'_, BrowserHandle>,
) -> AppResult<()> {
    if downloading() {
        return Err(AppError::bad_request(
            "The browser is still downloading. Stop the download first.",
        ));
    }
    let Some(root) = own_browser_dir(&app) else {
        return Ok(());
    };
    // Close the browser first if it is the one being deleted.
    if handle
        .0
        .availability()
        .is_ok_and(|f| f.kind == browser::BrowserKind::Own)
    {
        handle.0.shutdown().await;
    }
    browser::own::remove(&root)
        .await
        .map_err(|e| download_error(&e))?;
    tools_changed(&app);
    Ok(())
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
            .set_title("Choose Chrome, Edge, Brave, Vivaldi or Chromium")
            .pick_file(move |p| {
                let _ = tx.send(p);
            });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(settings.path);
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::internal(e.to_string()))?;
        if browser::find_browser(Some(&path), None)
            .map(|f| f.path)
            .ok()
            .as_deref()
            != Some(path.as_path())
        {
            return Err(AppError::bad_request(
                "That file could not be used as a browser. Choose Chrome, Edge, Brave, Vivaldi or Chromium.",
            ));
        }
        settings.path = Some(path.display().to_string());
    }
    state.store.set_setting(SETTINGS, &settings).await?;
    handle.0.reconfigure(config_from(&app, &settings)).await;
    tools_changed(&app);
    Ok(settings.path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use browser::own::{BrowserBuild, InstalledBrowser, OwnState};

    fn build() -> BrowserBuild {
        browser::own::embedded()
            .build_for("macos-aarch64")
            .expect("pinned build")
            .clone()
    }

    fn installed() -> OwnState {
        OwnState::Installed(InstalledBrowser {
            version: "1.2.3.4".into(),
            exe: PathBuf::from("/x/1.2.3.4/app"),
            dir: PathBuf::from("/x/1.2.3.4"),
        })
    }

    #[test]
    fn own_browser_status_follows_disk_and_download() {
        let b = build();
        let s = own_status(Some(&b), "9.9", &OwnState::NotInstalled, false, false);
        assert!(s.offered);
        assert_eq!(s.state, "notInstalled");
        assert_eq!(s.download_bytes, b.file.size);
        assert_eq!(s.pinned_version, "9.9");
        assert_eq!(s.version, None);

        let s = own_status(
            Some(&b),
            "9.9",
            &OwnState::Partial { bytes: 7 },
            false,
            false,
        );
        assert_eq!((s.state.as_str(), s.bytes_done), ("partial", 7));
        let s = own_status(
            Some(&b),
            "9.9",
            &OwnState::Partial { bytes: 7 },
            true,
            false,
        );
        assert_eq!(s.state, "downloading");
        let s = own_status(Some(&b), "9.9", &OwnState::NotInstalled, true, false);
        assert_eq!(s.state, "downloading");

        let s = own_status(Some(&b), "9.9", &installed(), false, true);
        assert_eq!(s.state, "installed");
        assert_eq!(s.version.as_deref(), Some("1.2.3.4"));
        assert!(s.in_use);
    }

    /// Where no download is offered (Windows, Linux) the page gets `offered: false` and a
    /// zero size, and an installed build is still reported so it can be removed.
    #[test]
    fn no_offer_means_no_download_but_an_installed_build_still_shows() {
        let s = own_status(None, "9.9", &OwnState::NotInstalled, false, false);
        assert!(!s.offered);
        assert_eq!(s.download_bytes, 0);
        assert_eq!(s.state, "notInstalled");
        let s = own_status(None, "9.9", &installed(), false, false);
        assert_eq!(s.state, "installed");
        assert!(!s.offered);
    }

    #[test]
    fn the_status_is_camel_case_for_the_page() {
        let v = serde_json::to_value(own_status(
            Some(&build()),
            "9.9",
            &OwnState::NotInstalled,
            false,
            false,
        ))
        .unwrap();
        for key in [
            "offered",
            "state",
            "version",
            "pinnedVersion",
            "downloadBytes",
            "bytesDone",
            "inUse",
        ] {
            assert!(v.get(key).is_some(), "{key} missing from {v}");
        }
    }

    #[test]
    fn download_errors_say_what_to_do() {
        let e = download_error(&PackError::ChecksumMismatch { file: "x".into() });
        assert_eq!(e.code, "checksum_mismatch");
        assert!(e.message.contains("was not installed"));
        let e = download_error(&PackError::BlockedHost("evil.example".into()));
        assert_eq!(e.code, "blocked_host");
        let e = download_error(&PackError::Archive {
            file: "x".into(),
            message: "the link \"a\" points outside the archive".into(),
        });
        assert_eq!(e.code, "agent_browser_failed");
    }
}
