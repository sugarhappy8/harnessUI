//! Updates that arrive on their own (ARCHITECTURE.md 16). 0.0.8 is the first version that
//! updates itself.
//!
//! - The updater plugin is registered only when the build carries an updater public key
//!   (`plugins.updater.pubkey`, added by the release build from
//!   `tauri.updater.conf.json`). Without it nothing is checked and every command here
//!   reports "not available".
//! - The automatic check is on unless the user turned it off in Settings (setting
//!   `updateAutoCheck`; never set means on). It runs once at start, after the skills and
//!   saved servers, and then every six hours while the app is open, counted on the wall
//!   clock so the hours a computer sleeps count too. A check that did not work (no network
//!   yet just after login, say) is tried again after 30 minutes; a download that did not
//!   work waits the whole round, so a release that never verifies is not fetched again and
//!   again. Each check asks github.com for the latest release's `latest.json`.
//! - A newer version is downloaded and its signature verified in the background, without
//!   asking. The verified package waits in memory until the user clicks *Restart and
//!   update*: nothing is ever installed on its own. The window hears about it from the
//!   `update-ready` event, and reads it from `update_status` too, because events are not
//!   replayed and the window may start listening after the check at start.
//! - Only one check or download runs at a time ([`Updates::busy`]). No request waits forever
//!   on a connection that went quiet: connecting, each read and the whole check have time
//!   limits, so a stalled check or download fails and the next round can run.
//! - The check, download and signature verification run in Rust; the webview has no
//!   updater permission. The plugin installs only a file signed with the updater key whose
//!   advertised version is newer than this one. `latest.json` is not signed, but `tauri build`
//!   writes the version into each update file's signature, and with `requireSignedVersion`
//!   on (tauri.updater.conf.json) the plugin refuses a signature whose version differs from
//!   the announced one or that has none, so an older signed build cannot be passed off as a
//!   newer one (ARCHITECTURE.md 16, SECURITY.md).
//! - On Windows the reply, llama-server and every tool child are stopped before the installer
//!   runs, because it cannot replace files a running process holds open. On macOS the new
//!   app is put in place first and the restart stops them, so an install that fails there
//!   has stopped nothing ([`STOP_BEFORE_INSTALL`]).

use std::cmp::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::OwnedMutexGuard;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Setting: check for updates on its own. On when the user never chose.
const AUTO_CHECK: &str = "updateAutoCheck";

/// How long the automatic check waits before it looks again, on the wall clock.
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);

/// How long the automatic check waits after a check that did not work.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// How often the automatic check wakes to see whether a round is due. The rounds are timed
/// on the wall clock, because the timer that sleeps here stops while the computer sleeps:
/// six hours of it can take days on a laptop that is mostly closed.
const WAKE_EVERY: Duration = Duration::from_secs(15 * 60);

/// How long an updater request may take to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long an updater request may wait for the next bytes. It starts again after every
/// read, so a slow download that keeps moving is never cut off; one that stops is.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a whole check may take: one small `latest.json`.
const CHECK_TIMEOUT: Duration = Duration::from_secs(2 * 60);

/// Whether the reply and every child process are stopped before the installer runs: only on
/// Windows, where the installer cannot replace files a running process holds open. On macOS
/// the new app is put in place while they run (a running program keeps its files until it
/// ends), and the restart after it stops them the way quitting does (lib.rs,
/// `RunEvent::Exit`), so an install that fails there leaves everything running.
const STOP_BEFORE_INSTALL: bool = cfg!(windows);

/// Sent to the main window whenever what the updater is doing changes ([`UpdateState`]).
const STATE_EVENT: &str = "update-state";

/// Sent to the main window when a newer version has been downloaded and verified and can be
/// installed ([`UpdateInfo`]).
const READY_EVENT: &str = "update-ready";

/// The hint for a Mac that runs harnessUI from a place it cannot replace itself in.
const MOVE_TO_APPLICATIONS: &str =
    "Move harnessUI to the Applications folder, open it from there, and try again.";

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

/// Whether the automatic check is on: what the user chose, and on when they never chose.
fn auto_check_on(stored: Option<bool>) -> bool {
    stored.unwrap_or(true)
}

/// The saved choice. A setting that cannot be read counts as never chosen.
async fn auto_check_setting(state: &AppState) -> bool {
    auto_check_on(state.store.get_setting(AUTO_CHECK).await.ok().flatten())
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    pub version: String,
    pub date: Option<String>,
    pub notes: Option<String>,
}

impl UpdateInfo {
    fn of(u: &Update) -> Self {
        Self {
            version: u.version.clone(),
            date: u.date.map(|d| d.to_string()),
            notes: u.body.clone(),
        }
    }
}

/// What the updater is doing, as the window shows it (src/lib/types.ts `UpdateState`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateState {
    /// A check is running.
    pub checking: bool,
    /// A check finished since the app started, whether it found a newer version or not.
    pub checked: bool,
    /// The version being downloaded and verified.
    pub downloading: Option<String>,
    /// The newer version the last check since start found (automatic or *Check now*).
    pub available: Option<UpdateInfo>,
    /// Downloaded and verified: *Restart and update* installs it.
    pub ready: Option<UpdateInfo>,
    /// Why the last check or download did not work. The next one tries again.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    pub configured: bool,
    pub auto_check: bool,
    pub current_version: String,
    #[serde(flatten)]
    pub state: UpdateState,
}

/// What the updater is doing right now. A downloaded package is kept beside it, in
/// [`Inner::ready`], because a later check can run while one waits to be installed.
#[derive(Debug, Clone, Default, PartialEq)]
enum Activity {
    #[default]
    Idle,
    Checking,
    Downloading {
        version: String,
    },
    /// The last check or download did not work; the next one tries again.
    Failed {
        message: String,
    },
}

/// A downloaded update whose signature verified, with the plugin's description of it, which
/// installs it.
struct Package {
    update: Update,
    bytes: Vec<u8>,
}

/// The updater's memory for this run of the app. Generic over the package so the rules can
/// be tested without a real download.
struct Inner<P> {
    activity: Activity,
    checked: bool,
    available: Option<UpdateInfo>,
    /// The newest verified package, until it is installed or the app quits. Shared, so
    /// installing can use it while it stays visible here.
    ready: Option<(UpdateInfo, Arc<P>)>,
}

impl<P> Default for Inner<P> {
    fn default() -> Self {
        Self {
            activity: Activity::Idle,
            checked: false,
            available: None,
            ready: None,
        }
    }
}

impl<P> Inner<P> {
    fn start_check(&mut self) {
        self.activity = Activity::Checking;
    }

    /// A check answered: the newer version it found, or `None` when there is none. A package
    /// already waiting stays: it is signed and newer than this app.
    fn checked(&mut self, found: Option<UpdateInfo>) {
        self.activity = Activity::Idle;
        self.checked = true;
        self.available = found;
    }

    fn failed(&mut self, message: String) {
        self.activity = Activity::Failed { message };
    }

    /// Whether `found` is worth downloading: newer than this app and than the package
    /// already waiting.
    fn wants(&self, current: &str, found: &str) -> bool {
        should_download(current, self.ready_version(), found)
    }

    fn start_download(&mut self, version: String) {
        self.activity = Activity::Downloading { version };
    }

    /// A download verified. It becomes the package to install unless the same or a newer
    /// one already waits; says whether it did.
    fn downloaded(&mut self, info: UpdateInfo, package: P) -> bool {
        self.activity = Activity::Idle;
        let newer = self
            .ready_version()
            .is_none_or(|ready| compare_versions(&info.version, ready) == Some(Ordering::Greater));
        if newer {
            self.ready = Some((info, Arc::new(package)));
        }
        newer
    }

    fn ready_version(&self) -> Option<&str> {
        self.ready.as_ref().map(|(info, _)| info.version.as_str())
    }

    fn ready_package(&self) -> Option<Arc<P>> {
        self.ready.as_ref().map(|(_, p)| Arc::clone(p))
    }

    fn state(&self) -> UpdateState {
        UpdateState {
            checking: self.activity == Activity::Checking,
            checked: self.checked,
            downloading: match &self.activity {
                Activity::Downloading { version } => Some(version.clone()),
                _ => None,
            },
            available: self.available.clone(),
            ready: self.ready.as_ref().map(|(info, _)| info.clone()),
            error: match &self.activity {
                Activity::Failed { message } => Some(message.clone()),
                _ => None,
            },
        }
    }
}

/// The updater's state for this run of the app (managed by Tauri, see lib.rs).
#[derive(Default)]
pub struct Updates {
    inner: Mutex<Inner<Package>>,
    /// Held for a whole check and the download it starts, so two never run at once.
    busy: Arc<tokio::sync::Mutex<()>>,
}

impl Updates {
    fn with<T>(&self, f: impl FnOnce(&mut Inner<Package>) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    pub fn state(&self) -> UpdateState {
        self.with(|i| i.state())
    }

    /// The right to check and download, unless a check or download is already running.
    fn try_start(&self) -> Option<OwnedMutexGuard<()>> {
        Arc::clone(&self.busy).try_lock_owned().ok()
    }

    /// The right to check and download, after the check or download already running ends.
    async fn wait_start(&self) -> OwnedMutexGuard<()> {
        Arc::clone(&self.busy).lock_owned().await
    }
}

/// Orders two versions the way the updater plugin does, with `semver::Version` ("1.2.10"
/// after "1.2.9", "1.0.0-beta" before "1.0.0", and build metadata after "+" counts too: the
/// plugin offers "0.0.8+2" to 0.0.8, so it is downloaded too). `None` when either is not a
/// version.
fn compare_versions(a: &str, b: &str) -> Option<Ordering> {
    let parse = |v: &str| semver::Version::parse(v.trim()).ok();
    Some(parse(a)?.cmp(&parse(b)?))
}

/// Whether a version a check found should be downloaded: only when it is newer than this
/// app and than the package already waiting. The same or an older one is skipped.
fn should_download(current: &str, ready: Option<&str>, found: &str) -> bool {
    let newer_than = |v: &str| compare_versions(found, v) == Some(Ordering::Greater);
    newer_than(current) && ready.is_none_or(newer_than)
}

/// Tells the main window what the updater is doing now.
fn announce(app: &AppHandle) {
    if let Some(updates) = app.try_state::<Updates>() {
        let _ = app.emit_to(crate::MAIN_WINDOW, STATE_EVENT, updates.state());
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

fn no_newer_version() -> AppError {
    AppError::bad_request("There is no newer version.")
}

#[tauri::command]
pub async fn update_status(app: AppHandle, state: State<'_, AppState>) -> AppResult<UpdateStatus> {
    Ok(UpdateStatus {
        configured: configured(app.config()),
        auto_check: auto_check_setting(&state).await,
        current_version: app.package_info().version.to_string(),
        state: app
            .try_state::<Updates>()
            .map(|u| u.state())
            .unwrap_or_default(),
    })
}

#[tauri::command]
pub async fn set_update_auto_check(
    app: AppHandle,
    state: State<'_, AppState>,
    on: bool,
) -> AppResult<()> {
    state.store.set_setting(AUTO_CHECK, &on).await?;
    if on {
        // Look now rather than at the next round, hours away.
        tauri::async_runtime::spawn(async move {
            automatic_check(&app).await;
        });
    }
    Ok(())
}

fn check_failed(details: String) -> AppError {
    AppError::new(
        "update_check",
        "Could not check for updates.",
        "Check your internet connection and try again.",
        details,
    )
}

async fn find(app: &AppHandle) -> AppResult<Option<Update>> {
    if !configured(app.config()) {
        return Err(not_configured());
    }
    // Without limits a connection that goes quiet (a proxy that stalls, a network that
    // changed while the laptop slept) would hold the check, or the download, for good, and
    // with it [`Updates::busy`]. The plugin keeps this client setting in the `Update` it
    // returns, so the download gets the same limits.
    let updater = app
        .updater_builder()
        .configure_client(|client| {
            client
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
        })
        .build()
        .map_err(|e| AppError::internal(e.to_string()))?;
    match tokio::time::timeout(CHECK_TIMEOUT, updater.check()).await {
        Ok(result) => result.map_err(|e| check_failed(e.to_string())),
        Err(_) => Err(check_failed(format!(
            "the check took longer than {} seconds",
            CHECK_TIMEOUT.as_secs()
        ))),
    }
}

/// Asks for the latest release and records what it found, or why it failed. The caller holds
/// [`Updates::busy`].
async fn check(app: &AppHandle, updates: &Updates) -> AppResult<Option<Update>> {
    updates.with(|i| i.start_check());
    announce(app);
    let result = find(app).await;
    match &result {
        Ok(found) => updates.with(|i| i.checked(found.as_ref().map(UpdateInfo::of))),
        Err(e) => updates.with(|i| i.failed(e.message.clone())),
    }
    announce(app);
    result
}

/// Downloads `update` and verifies its signature, unless the same or a newer version already
/// waits. `_busy` is the right the check that found it took; it is given back when this
/// ends. Says whether a new package is now waiting.
async fn fetch(app: &AppHandle, update: Update, _busy: OwnedMutexGuard<()>) -> AppResult<bool> {
    let updates = app.state::<Updates>();
    let current = app.package_info().version.to_string();
    if !updates.with(|i| i.wants(&current, &update.version)) {
        return Ok(false);
    }
    updates.with(|i| i.start_download(update.version.clone()));
    announce(app);
    // `download` checks the signature against the build's public key before it returns.
    match update.download(|_, _| {}, || {}).await {
        Ok(bytes) => {
            let info = UpdateInfo::of(&update);
            let ready = updates.with(|i| i.downloaded(info.clone(), Package { update, bytes }));
            announce(app);
            if ready {
                tracing::info!("update {} downloaded and verified", info.version);
                let _ = app.emit_to(crate::MAIN_WINDOW, READY_EVENT, info);
            }
            Ok(ready)
        }
        Err(e) => {
            let err = AppError::new(
                "update_download",
                "The update could not be downloaded or its signature did not verify.",
                "Try again later, or download the new version from GitHub.",
                e.to_string(),
            );
            updates.with(|i| i.failed(err.message.clone()));
            announce(app);
            Err(err)
        }
    }
}

/// One round of the automatic check. Skipped when the user turned it off, when this build
/// cannot update itself, and while a check or download is already running. A failure is
/// written to the log once and the next round tries again; nothing is shown for it but the
/// status line in Settings. Says whether the check worked. A skipped round counts as
/// working, since there is nothing to try again, and so does a failed download: trying it
/// again soon would fetch the whole package again each time, also when its signature never
/// verifies.
async fn automatic_check(app: &AppHandle) -> bool {
    if !configured(app.config()) || !auto_check_setting(&app.state::<AppState>()).await {
        return true;
    }
    let updates = app.state::<Updates>();
    let Some(busy) = updates.try_start() else {
        return true;
    };
    match check(app, &updates).await {
        Ok(Some(update)) => {
            if let Err(e) = fetch(app, update, busy).await {
                tracing::warn!("update download failed: {} ({})", e.message, e.details);
            }
            true
        }
        Ok(None) => true,
        Err(e) => {
            tracing::info!("update check failed: {} ({})", e.message, e.details);
            false
        }
    }
}

/// Whether the next automatic round is due, given when the last one ended and whether its
/// check worked: six hours after one that worked, 30 minutes after one that did not, on the
/// wall clock. The first round is due at once, and so is the next one when the clock was set
/// back past the last.
fn round_due(last: Option<(SystemTime, bool)>, now: SystemTime) -> bool {
    let Some((ended, worked)) = last else {
        return true;
    };
    let wait = if worked { CHECK_EVERY } else { RETRY_AFTER };
    match now.duration_since(ended) {
        Ok(gone) => gone >= wait,
        Err(_) => true,
    }
}

/// The automatic check: once now (lib.rs starts this after the skills and saved servers),
/// then every six hours while the app is open, or 30 minutes after a check that did not
/// work. Each round reads the setting again.
pub async fn check_regularly(app: AppHandle) {
    let mut last = None;
    loop {
        if round_due(last, SystemTime::now()) {
            let worked = automatic_check(&app).await;
            last = Some((SystemTime::now(), worked));
        }
        tokio::time::sleep(WAKE_EVERY).await;
    }
}

/// *Check now*: the same check as the automatic one, whatever the setting. A newer version
/// it finds is then downloaded and verified in the background, and the window hears from
/// `update-ready` when it can be installed.
#[tauri::command]
pub async fn check_for_update(app: AppHandle) -> AppResult<Option<UpdateInfo>> {
    if !configured(app.config()) {
        return Err(not_configured());
    }
    let updates = app.state::<Updates>();
    let Some(busy) = updates.try_start() else {
        // The automatic check, or the download it started, is running; the window hears how
        // it ends from `update-state`.
        return Ok(updates.state().available);
    };
    let found = check(&app, &updates).await?;
    let info = found.as_ref().map(UpdateInfo::of);
    if let Some(update) = found {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = fetch(&app, update, busy).await {
                tracing::warn!("update download failed: {} ({})", e.message, e.details);
            }
        });
    }
    Ok(info)
}

/// The package to install: the one waiting, or else one checked for and downloaded now,
/// after the check or download already running (if any) ends.
async fn package_to_install(app: &AppHandle, updates: &Updates) -> AppResult<Arc<Package>> {
    if let Some(package) = updates.with(|i| i.ready_package()) {
        return Ok(package);
    }
    let busy = updates.wait_start().await;
    // The download that was running may have just finished.
    if let Some(package) = updates.with(|i| i.ready_package()) {
        return Ok(package);
    }
    let update = check(app, updates).await?.ok_or_else(no_newer_version)?;
    fetch(app, update, busy).await?;
    updates
        .with(|i| i.ready_package())
        .ok_or_else(no_newer_version)
}

/// Whether harnessUI runs from a place it cannot replace itself in: the disk image it came on,
/// or the read-only copy macOS makes of an app opened straight from the Downloads folder (App
/// Translocation). Judged from the error the install gave and the app's own path.
fn read_only_place(error: &str, exe: &str) -> bool {
    error.contains("Read-only file system")
        || error.contains("os error 30")
        || error.contains("not on the same mount point")
        || exe.starts_with("/Volumes/")
        || exe.contains("/AppTranslocation/")
}

/// The error for an install that did not work, with the hint that fits this computer.
fn install_error(os: &str, error: &str, exe: &str) -> AppError {
    let hint = match os {
        "macos" if read_only_place(error, exe) => MOVE_TO_APPLICATIONS,
        // Not recognised, but this is the usual cause on a Mac.
        "macos" => {
            "If harnessUI is not in the Applications folder, move it there, open it from there, \
             and try again. Otherwise download the new version from GitHub."
        }
        // Everything was stopped for the installer, and the updater let go of what the window
        // had open just before it started the installer.
        "windows" => {
            "Restart harnessUI, then try again. If it still does not work, download the new \
             version from GitHub."
        }
        _ => "Download the new version from GitHub.",
    };
    AppError::new(
        "update_install",
        "The update could not be installed.",
        hint,
        error,
    )
}

/// On a Mac, refuses to start an install that cannot work, with the hint that fits: when
/// macOS runs the app from a read-only copy (App Translocation) or the folder that holds the
/// app cannot be written to at all (the disk image it came on).
#[cfg(target_os = "macos")]
fn refuse_read_only_place() -> AppResult<()> {
    let Ok(exe) = std::env::current_exe() else {
        return Ok(());
    };
    let text = exe.to_string_lossy().into_owned();
    // …/harnessUI.app/Contents/MacOS/<binary>: the folder that holds harnessUI.app. A program
    // that is not inside an app (a development build) is not checked.
    if !text.contains(".app/Contents/MacOS/") {
        return Ok(());
    }
    let Some(folder) = exe.ancestors().nth(4) else {
        return Ok(());
    };
    let read_only = text.contains("/AppTranslocation/") || {
        // A probe folder that is removed at once. Only a read-only file system counts: a folder
        // the user may not write to is installed into after asking for their password.
        let probe = folder.join(format!(".harnessui-update-check-{}", std::process::id()));
        match std::fs::create_dir(&probe) {
            Ok(()) => {
                let _ = std::fs::remove_dir(&probe);
                false
            }
            Err(e) => e.kind() == std::io::ErrorKind::ReadOnlyFilesystem,
        }
    };
    if read_only {
        return Err(AppError::new(
            "update_install",
            "The update could not be installed.",
            MOVE_TO_APPLICATIONS,
            format!("harnessUI runs from a read-only place: {text}"),
        ));
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn refuse_read_only_place() -> AppResult<()> {
    Ok(())
}

/// Before the Windows installer runs: stops the reply, the browser, the keys computer use
/// holds, every tool server and llama-server. The model is unloaded the way removing it
/// does, so if the install fails the header does not go on showing a model that is gone.
async fn stop_for_install(app: &AppHandle, state: &AppState) {
    state.generation.cancel();
    if let Some(browser) = app.try_state::<crate::browser_cmd::BrowserHandle>() {
        browser.0.shutdown().await;
    }
    if let Some(c) = app.try_state::<crate::computer_cmd::ComputerHandle>()
        && let Some(server) = c.0.clone()
    {
        server.release_all();
    }
    state.host.shutdown().await;
    state.runtime.unload().await;
}

/// After a Windows install that failed once everything was stopped: shows the window again
/// (the updater hides it just before it starts the installer) and starts the saved tool
/// servers again. The browser starts again when a tool needs it, and the model when the user
/// picks it. The error still says to restart harnessUI ([`install_error`]).
async fn start_again_after_failed_install(app: &AppHandle, state: &AppState) {
    if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.set_focus();
    }
    state.host.resume();
    crate::toolset::start_saved_servers(app).await;
}

/// *Restart and update*: installs the downloaded update (or downloads it first when none
/// waits), then restarts the app.
#[tauri::command]
pub async fn install_update(app: AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    if !configured(app.config()) {
        return Err(not_configured());
    }
    refuse_read_only_place()?;
    let updates = app.state::<Updates>();
    let package = package_to_install(&app, &updates).await?;
    if STOP_BEFORE_INSTALL {
        stop_for_install(&app, &state).await;
    }
    // `install` checks the signature again first. On Windows it starts the installer, which
    // opens the new version when it is done, and ends this process itself: nothing after
    // this line runs there. On macOS it puts the new app in place of this one and returns.
    if let Err(e) = package.update.install(&package.bytes) {
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let err = install_error(std::env::consts::OS, &e.to_string(), &exe);
        tracing::warn!("update install failed: {}", err.details);
        if STOP_BEFORE_INSTALL {
            start_again_after_failed_install(&app, &state).await;
        }
        // The package stays waiting, so the user can try again.
        return Err(err);
    }
    // macOS: the reply ends here, and the restart stops everything else the way quitting
    // does (lib.rs, `RunEvent::Exit`).
    state.generation.cancel();
    app.restart();
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

    /// The automatic check is on for everyone who never turned it off.
    #[test]
    fn the_automatic_check_is_on_unless_turned_off() {
        assert!(auto_check_on(None));
        assert!(auto_check_on(Some(true)));
        assert!(!auto_check_on(Some(false)));
    }

    #[test]
    fn versions_are_ordered_like_semver() {
        use Ordering::*;
        assert_eq!(compare_versions("0.0.9", "0.0.8"), Some(Greater));
        assert_eq!(compare_versions("0.0.10", "0.0.9"), Some(Greater));
        assert_eq!(compare_versions("0.1.0", "0.0.99"), Some(Greater));
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Some(Equal));
        // Build metadata counts, as in the plugin's own comparison.
        assert_eq!(compare_versions("1.0.0+build.5", "1.0.0"), Some(Greater));
        assert_eq!(compare_versions("1.0.0+2", "1.0.0+2"), Some(Equal));
        assert_eq!(compare_versions("1.0.0-beta", "1.0.0"), Some(Less));
        assert_eq!(
            compare_versions("1.0.0-beta.2", "1.0.0-beta.10"),
            Some(Less)
        );
        assert_eq!(compare_versions("1.0.0-alpha", "1.0.0-alpha.1"), Some(Less));
        assert_eq!(compare_versions("1.0.0-1", "1.0.0-alpha"), Some(Less));
        assert_eq!(compare_versions("0.0.7", "0.0.8"), Some(Less));
        assert_eq!(compare_versions("1.0", "1.0.0"), None);
        assert_eq!(compare_versions("1.0.0.0", "1.0.0"), None);
        assert_eq!(compare_versions("v1.0.0", "1.0.0"), None);
    }

    /// A found version is downloaded only when it is newer than this app and than the one
    /// already waiting.
    #[test]
    fn only_a_newer_version_is_downloaded() {
        assert!(should_download("0.0.8", None, "0.0.9"));
        assert!(!should_download("0.0.8", None, "0.0.8"));
        assert!(!should_download("0.0.8", None, "0.0.7"));
        assert!(should_download("0.0.8", Some("0.0.9"), "0.0.10"));
        assert!(!should_download("0.0.8", Some("0.0.9"), "0.0.9"));
        assert!(!should_download("0.0.8", Some("0.0.10"), "0.0.9"));
        assert!(!should_download("0.0.8", None, "not a version"));
        // The plugin offers a rebuild with build metadata as an update; it is downloaded.
        assert!(should_download("0.0.8", None, "0.0.8+2"));
    }

    /// The automatic rounds: the first at once, then six hours after one whose check worked
    /// and 30 minutes after one whose check did not, on the wall clock.
    #[test]
    fn rounds_are_timed_on_the_wall_clock_and_a_failure_is_retried_sooner() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let minutes = |m: u64| Duration::from_secs(m * 60);
        assert!(round_due(None, t));
        assert!(!round_due(Some((t, true)), t + minutes(30)));
        assert!(!round_due(Some((t, true)), t + minutes(6 * 60 - 1)));
        assert!(round_due(Some((t, true)), t + minutes(6 * 60)));
        assert!(!round_due(Some((t, false)), t + minutes(29)));
        assert!(round_due(Some((t, false)), t + minutes(30)));
        // A clock set back past the last round.
        assert!(round_due(Some((t, true)), t - minutes(1)));
        // The loop wakes often enough to notice a round that came due while asleep.
        assert!(WAKE_EVERY < RETRY_AFTER);
    }

    /// The state the window reads as it moves from a check to a package that waits.
    #[test]
    fn a_check_and_its_download_move_the_state_along() {
        let mut inner = Inner::<&str>::default();
        assert_eq!(inner.state(), UpdateState::default());
        inner.start_check();
        assert!(inner.state().checking);
        inner.checked(Some(info("0.0.9")));
        let s = inner.state();
        assert!(!s.checking && s.checked);
        assert_eq!(s.available, Some(info("0.0.9")));
        assert!(inner.wants("0.0.8", "0.0.9"));
        inner.start_download("0.0.9".into());
        assert_eq!(inner.state().downloading.as_deref(), Some("0.0.9"));
        assert!(inner.downloaded(info("0.0.9"), "package 0.0.9"));
        let s = inner.state();
        assert_eq!(s.downloading, None);
        assert_eq!(s.ready, Some(info("0.0.9")));
        assert_eq!(inner.ready_package().as_deref(), Some(&"package 0.0.9"));
        // The same version again is not downloaded a second time.
        assert!(!inner.wants("0.0.8", "0.0.9"));
    }

    /// A newer version replaces the package that waits; the same or an older one does not.
    #[test]
    fn a_newer_package_replaces_the_waiting_one() {
        let mut inner = Inner::<&str>::default();
        assert!(inner.downloaded(info("0.0.9"), "a"));
        assert!(!inner.downloaded(info("0.0.9"), "b"));
        assert!(!inner.downloaded(info("0.0.8"), "c"));
        assert_eq!(inner.ready_package().as_deref(), Some(&"a"));
        assert!(inner.downloaded(info("0.0.10"), "d"));
        assert_eq!(inner.ready_package().as_deref(), Some(&"d"));
        assert_eq!(
            inner.state().ready.map(|i| i.version),
            Some("0.0.10".into())
        );
    }

    /// A failed check is shown until the next one works, and never drops the package that
    /// waits; a check that finds nothing newer keeps it too.
    #[test]
    fn a_failure_keeps_the_waiting_package() {
        let mut inner = Inner::<&str>::default();
        inner.downloaded(info("0.0.9"), "a");
        inner.start_check();
        inner.failed("Could not check for updates.".into());
        let s = inner.state();
        assert_eq!(s.error.as_deref(), Some("Could not check for updates."));
        assert!(!s.checking);
        assert_eq!(s.ready, Some(info("0.0.9")));
        inner.start_check();
        inner.checked(None);
        let s = inner.state();
        assert_eq!(s.error, None);
        assert_eq!(s.available, None);
        assert_eq!(s.ready, Some(info("0.0.9")));
    }

    /// `update_status` as the window reads it (src/lib/types.ts `UpdateStatus`).
    #[test]
    fn the_status_carries_the_state_in_camel_case() {
        let mut status = UpdateStatus {
            configured: true,
            auto_check: true,
            current_version: "0.0.8".into(),
            state: UpdateState {
                checking: false,
                checked: true,
                downloading: Some("0.0.10".into()),
                available: Some(info("0.0.10")),
                ready: Some(info("0.0.9")),
                error: Some("Could not check for updates.".into()),
            },
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "configured": true,
                "autoCheck": true,
                "currentVersion": "0.0.8",
                "checking": false,
                "checked": true,
                "downloading": "0.0.10",
                "available": {"version": "0.0.10", "date": null, "notes": "Fixes."},
                "ready": {"version": "0.0.9", "date": null, "notes": "Fixes."},
                "error": "Could not check for updates.",
            })
        );
        status.state = UpdateState::default();
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["available"], serde_json::Value::Null);
        assert_eq!(v["ready"], serde_json::Value::Null);
        assert_eq!(v["downloading"], serde_json::Value::Null);
        assert_eq!(v["checked"], false);
    }

    /// On a Mac, an install that fails because the app runs from the disk image or a
    /// translocated copy tells the user to move it to Applications.
    #[test]
    fn a_mac_install_failure_says_to_move_the_app() {
        let app = "/Applications/harnessUI.app/Contents/MacOS/app";
        let dmg = "/Volumes/harnessUI/harnessUI.app/Contents/MacOS/app";
        let moved =
            "/private/var/folders/x/AppTranslocation/1234/d/harnessUI.app/Contents/MacOS/app";
        let ro = "Read-only file system (os error 30)";
        for (error, exe) in [(ro, app), ("some other failure", dmg), ("x", moved)] {
            let e = install_error("macos", error, exe);
            assert_eq!(e.code, "update_install");
            assert_eq!(e.next_step, MOVE_TO_APPLICATIONS, "{error} {exe}");
            assert_eq!(e.details, error);
        }
        let e = install_error(
            "macos",
            "temp directory is not on the same mount point as the app",
            app,
        );
        assert_eq!(e.next_step, MOVE_TO_APPLICATIONS);
        // Not recognised: the hint still names the usual cause.
        let e = install_error("macos", "permission denied", app);
        assert!(
            e.next_step.contains("Applications folder"),
            "{}",
            e.next_step
        );
        // Windows: never about the Applications folder. Everything was stopped for the
        // installer, so it says to restart first.
        let e = install_error("windows", ro, r"C:\Users\a\AppData\Local\harnessUI\app.exe");
        assert!(!e.next_step.contains("Applications"), "{}", e.next_step);
        assert!(
            e.next_step.starts_with("Restart harnessUI"),
            "{}",
            e.next_step
        );
        assert!(e.next_step.contains("download the new version from GitHub"));
    }
}
