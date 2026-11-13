//! Settings → Reverse engineering (0.0.6 item 5; `docs/dev/0.0.6/item5-spec.md` WP9d): the
//! commands behind the tab. rea (with Node.js), Kuna and Ghidra (with Java) are optional,
//! pinned downloads like the ROCm pack (`reverse::install`); nothing is downloaded until the
//! owner clicks Download here and accepts the licences.
//!
//! - Every command is for the main window only and checks the calling window first.
//! - The decompiler choice is the settings key [`DECOMPILER_KEY`] (`"kuna"` or `"ghidra"`),
//!   written only here, never through `set_setting` (whose allow-list does not name it). The
//!   first decompiler installed becomes the choice; removing the chosen one switches to the
//!   other one if it is installed, else to none. At start the saved choice is read into the
//!   `reverse` server ([`load_saved_choice`]).
//! - A command that would stop rea while it works on a chat's file (removing rea or the
//!   decompiler it uses, switching the decompiler, clearing Kuna's saved analysis) answers
//!   `busy` unless `force` is set; the tab then asks "rea is working on a file in a chat.
//!   Stop it and continue?". A download that finishes never cuts a chat's call: it waits
//!   for rea to be free before it restarts rea.
//! - Each change calls `ReverseServer::refresh()`, restarts rea where the change touches
//!   what rea uses (which also ends every chat's open file, WP9g), and tells the Tools panel
//!   that the reverse tools may have changed (`mcp-changed`).
//! - Nothing here is a tool action: no tool and no model output can reach these commands,
//!   in any approval mode (D12).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use reverse::{
    AppleTools, Component, ComponentState, Decompiler, DownloadStage, Os, Platform, Progress,
    RestartReason, ReverseError, ReverseLayout, ReverseServer, ReverseStatus,
};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The owner's decompiler choice (`"kuna"` or `"ghidra"`; absent for none).
pub const DECOMPILER_KEY: &str = "reverseDecompiler";
/// Download progress, sent to the main window.
pub const PROGRESS_EVENT: &str = "reverse-progress";
/// At most one progress event this often, except when the part or the step changes.
const PROGRESS_EVERY: Duration = Duration::from_millis(300);
/// How long a finished download waits for rea to finish a chat's call before restarting it
/// (an open with Ghidra may take up to 16 minutes). When rea is still busy after that, it
/// is not restarted: its next start uses the new files by itself.
const RESTART_WAIT: Duration = Duration::from_secs(20 * 60);
/// How often that wait looks again.
const RESTART_POLL: Duration = Duration::from_millis(500);
/// `xcode-select`, which opens Apple's own installer for the command line tools (R7, Q3).
pub(crate) const XCODE_SELECT: &str = "/usr/bin/xcode-select";
/// How long `xcode-select --install` may take to hand over to Apple's installer.
const XCODE_SELECT_WAIT: Duration = Duration::from_secs(30);

/// A download in progress: its cancel token and the rows it installs (a decompiler's
/// download brings rea along when rea is missing).
struct Running {
    token: CancellationToken,
    rows: Vec<Component>,
}

/// Downloads in progress, by the row whose Download was clicked.
static DOWNLOADS: LazyLock<Mutex<HashMap<Component, Running>>> = LazyLock::new(Default::default);

/// One part's sizes on this computer (for the dialogs and the progress bars).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartSize {
    /// The row the part belongs to.
    pub component: Component,
    /// `rea`, `node`, `kuna`, `ghidra` or `java`.
    pub name: String,
    /// Bytes to download.
    pub download_bytes: u64,
    /// Bytes on disk once unpacked.
    pub unpacked_bytes: u64,
}

/// Everything the tab shows: the server's status, plus what the tab's wording depends on.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseTab {
    /// rows, decompiler in use, Apple's tools, rea's state, Kuna's saved analysis, memory.
    #[serde(flatten)]
    pub status: ReverseStatus,
    /// This computer's system: every text of the tab depends on what it offers.
    pub os: Os,
    /// The owner's choice, also when it cannot be used right now (broken, needs an update).
    pub chosen: Option<Decompiler>,
    /// Each part offered on this computer, in the lock's order.
    pub part_sizes: Vec<PartSize>,
}

/// The `reverse-progress` event.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseProgressDto {
    /// The row whose Download was clicked.
    pub component: Component,
    /// The part being fetched, checked or unpacked (empty in the last event).
    pub part: String,
    /// Bytes so far in this step, all parts of the download together while downloading.
    pub done: u64,
    /// Bytes in this step.
    pub total: u64,
    /// The step.
    pub stage: DownloadStage,
    /// Bytes so far of this part alone (0 while checking).
    pub part_done: u64,
    /// Bytes of this part alone (its download while downloading and checking, its unpacked
    /// size while unpacking).
    pub part_total: u64,
    /// The last event of this download.
    pub finished: bool,
    /// Why it failed, in plain words; `None` when it worked or was cancelled.
    pub error: Option<AppError>,
}

impl ReverseProgressDto {
    fn finished(component: Component, error: Option<AppError>) -> Self {
        Self {
            component,
            part: String::new(),
            done: 0,
            total: 0,
            stage: DownloadStage::Downloading,
            part_done: 0,
            part_total: 0,
            finished: true,
            error,
        }
    }
}

// ---------------------------------------------------------------------------------------
// The app's server.

/// The app's `reverse` server: the one `toolset::build_host` registered (WP9g). Only if that
/// did not happen (the app's data folder could not be named then) does the tab make one
/// over the same folders; it is set as the app's server, so there is still only one.
fn server(app: &AppHandle) -> AppResult<Arc<ReverseServer>> {
    if let Some(server) = reverse::handle() {
        return Ok(server);
    }
    let local = app
        .path()
        .app_local_data_dir()
        .map_err(|e| AppError::internal(format!("the app's data folder: {e}")))?;
    let own = ReverseServer::new(ReverseLayout::for_app(&local), Platform::current());
    reverse::server::set_handle(own);
    reverse::handle().ok_or_else(|| AppError::internal("the reverse server is not ready"))
}

/// What the tab shows now.
pub(crate) fn tab(server: &ReverseServer) -> ReverseTab {
    let installer = server.installer();
    let platform = installer.platform();
    let part_sizes = installer
        .lock()
        .parts
        .iter()
        .filter_map(|p| {
            let build = p.build_for(platform)?;
            Some(PartSize {
                component: p.component,
                name: p.name.clone(),
                download_bytes: build.file.size,
                unpacked_bytes: build.unpacked_bytes,
            })
        })
        .collect();
    ReverseTab {
        status: server.status(),
        os: platform.os(),
        chosen: server.chosen_decompiler(),
        part_sizes,
    }
}

/// Tells the Tools panel that the reverse tools' marks may have changed.
fn tools_changed(app: &AppHandle) {
    let _ = app.emit_to(crate::MAIN_WINDOW, "mcp-changed", "reverse");
}

// ---------------------------------------------------------------------------------------
// The decompiler choice.

/// Whether a row is on this computer (usable or not).
fn on_disk(state: &ComponentState) -> bool {
    matches!(
        state,
        ComponentState::Installed
            | ComponentState::Broken { .. }
            | ComponentState::NeedsUpdate { .. }
    )
}

/// Whether this build offers a row on this computer, else the row's reason (Kuna on
/// Windows, Ghidra on a Mac, a part a release build withholds for lack of its publisher's
/// checks).
fn offered(server: &ReverseServer, component: Component) -> Result<(), String> {
    let row = server.installer().status(component);
    if row.available {
        Ok(())
    } else {
        Err(row.reason.unwrap_or_default())
    }
}

/// The decompilers that may be the choice: on this computer (usable or not) and offered by
/// this build, Kuna first. Files of a decompiler this build does not offer (left by a debug
/// build, or withdrawn by a later lock) never make it the choice, so rea never starts with
/// it; its row can still be removed.
fn decompilers_on_disk(server: &ReverseServer) -> Vec<Decompiler> {
    [Decompiler::Kuna, Decompiler::Ghidra]
        .into_iter()
        .filter(|d| {
            on_disk(&server.installer().verify_installed(d.component()))
                && offered(server, d.component()).is_ok()
        })
        .collect()
}

/// The choice to use: the saved one while it is on this computer, else the first one that
/// is (so the first decompiler installed becomes the choice, and removing the chosen one
/// switches to the other), else none.
pub(crate) fn resolve_choice(
    saved: Option<Decompiler>,
    on_disk: &[Decompiler],
) -> Option<Decompiler> {
    match saved {
        Some(d) if on_disk.contains(&d) => Some(d),
        _ => on_disk.first().copied(),
    }
}

/// The saved choice; a value that is not a decompiler is ignored.
async fn read_saved(store: &store::Store) -> Option<Decompiler> {
    store
        .get_setting::<Decompiler>(DECOMPILER_KEY)
        .await
        .ok()
        .flatten()
}

async fn save_choice(store: &store::Store, choice: Option<Decompiler>) -> AppResult<()> {
    match choice {
        Some(d) => store.set_setting(DECOMPILER_KEY, &d).await?,
        None => {
            store.delete_setting(DECOMPILER_KEY).await?;
        }
    }
    Ok(())
}

/// Brings the saved choice and the server in line with what is on this computer, and
/// returns the choice.
pub(crate) async fn reconcile(
    server: &ReverseServer,
    store: &store::Store,
) -> AppResult<Option<Decompiler>> {
    let saved = read_saved(store).await;
    let choice = resolve_choice(saved, &decompilers_on_disk(server));
    if choice != saved {
        save_choice(store, choice).await?;
    }
    server.set_decompiler(choice);
    Ok(choice)
}

/// At start: the owner's saved choice into the app's `reverse` server.
pub fn load_saved_choice(app: &AppHandle) {
    let Ok(server) = server(app) else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let store = state.store.clone();
    if let Err(e) = tauri::async_runtime::block_on(reconcile(&server, &store)) {
        tracing::warn!("the reverse engineering decompiler choice could not be read: {e}");
    }
}

// ---------------------------------------------------------------------------------------
// The rules shared by the commands.

/// "rea is working on a file in a chat. Stop it and continue?"
pub(crate) fn busy_error() -> AppError {
    AppError::new(
        "busy",
        "rea is working on a file in a chat.",
        "Stop it and continue?",
        String::new(),
    )
}

/// Refuses with [`busy_error`] while rea starts or works on a call, unless forced.
fn refuse_if_busy(server: &ReverseServer, force: bool) -> AppResult<()> {
    if !force && server.process().is_busy() {
        return Err(busy_error());
    }
    Ok(())
}

/// The owner accepted the licences shown in the download dialog.
pub(crate) fn consent(accept_license: bool) -> AppResult<()> {
    if accept_license {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "The licences must be accepted before anything is downloaded.",
        ))
    }
}

/// "A download of this part is already running."
fn download_running() -> AppError {
    AppError::new(
        "download_running",
        "A download of this part is already running.",
        "Wait for it to finish, or cancel it first.",
        String::new(),
    )
}

/// The rows a download of `component` installs: rea first when it is missing.
fn rows_of_download(server: &ReverseServer, component: Component) -> Vec<Component> {
    let rea_missing = matches!(
        server.installer().verify_installed(Component::Rea),
        ComponentState::NotInstalled | ComponentState::Partial { .. }
    );
    if component != Component::Rea && rea_missing {
        vec![Component::Rea, component]
    } else {
        vec![component]
    }
}

/// The row a part belongs to, as the tab names it.
fn row_label_of_part(part: &str) -> &str {
    match part {
        "rea" | "node" => "rea",
        "kuna" => "Kuna",
        "ghidra" | "java" => "Ghidra",
        other => other,
    }
}

/// A part, as the tab names it.
fn part_label(part: &str) -> &str {
    match part {
        "rea" => "rea",
        "node" => "Node.js",
        "kuna" => "Kuna",
        "ghidra" => "Ghidra",
        "java" => "Java",
        other => other,
    }
}

fn component_label(component: Component) -> &'static str {
    match component {
        Component::Rea => "rea",
        Component::Kuna => "Kuna",
        Component::Ghidra => "Ghidra",
    }
}

/// Gigabytes with one decimal, rounded up ("You need 1.2 GB free.").
fn gigabytes_up(bytes: u64) -> String {
    let tenths = bytes.div_ceil(100_000_000);
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Every problem in the tab's plain words (item5-spec.md WP9d "Errors").
pub(crate) fn map_error(e: &ReverseError) -> AppError {
    let details = e.to_string();
    let (code, message, next): (&str, String, String) = match e {
        ReverseError::Mismatch { .. } => (
            "mismatch",
            "The download doesn't match the copy harnessUI expects, so it wasn't installed.".into(),
            "Try again. If it keeps failing, report it.".into(),
        ),
        ReverseError::Interrupted { .. } => (
            "interrupted",
            "The download was interrupted.".into(),
            "Check your connection and try again; it continues where it stopped.".into(),
        ),
        ReverseError::CantReach { host } => (
            "cant_reach",
            format!("Can't reach {host}."),
            "Check your internet connection and try again.".into(),
        ),
        ReverseError::ServerError { host, status } => (
            "server_error",
            format!("{host} didn't send the file (error {status})."),
            "Try again later. If it keeps failing, report it.".into(),
        ),
        ReverseError::DiskFull { need_bytes } if *need_bytes > 0 => (
            "disk_full",
            "The disk is full.".into(),
            format!("You need {} GB free.", gigabytes_up(*need_bytes)),
        ),
        ReverseError::DiskFull { .. } => (
            "disk_full",
            "The disk is full.".into(),
            "Free some space and try again; it continues where it stopped.".into(),
        ),
        ReverseError::PathTooLong { .. } => (
            "path_too_long",
            "The folder harnessUI keeps its files in has too long a path for Ghidra.".into(),
            "This is rare; please report it.".into(),
        ),
        ReverseError::NotNtfs => (
            "not_local_drive",
            "Ghidra can't be installed, because the folder harnessUI keeps its files in is not on this PC's own drive.".into(),
            "This is rare; please report it.".into(),
        ),
        ReverseError::BlockedHost { .. } => (
            "blocked_host",
            "The download was redirected to a server harnessUI does not trust.".into(),
            "Report this.".into(),
        ),
        ReverseError::Missing { part } => (
            "missing",
            format!(
                "Part of {} is missing, perhaps removed by antivirus software.",
                row_label_of_part(part)
            ),
            // The banner's Repair button is the next step, as on the row's own line.
            String::new(),
        ),
        ReverseError::Blocked { part } => (
            "blocked",
            format!("Your antivirus blocked part of {}.", row_label_of_part(part)),
            "Allow harnessUI in it, then choose Repair.".into(),
        ),
        ReverseError::NeedsUpdate { part } => (
            "needs_update",
            format!("This version of {} has a known problem.", part_label(part)),
            "Update it to keep using it.".into(),
        ),
        ReverseError::Archive { part, .. } => (
            "archive",
            format!(
                "The download of {} was damaged, so it wasn't installed.",
                part_label(part)
            ),
            "Try again. If it keeps failing, report it.".into(),
        ),
        ReverseError::NotAvailable { reason } => {
            ("not_available", reason.clone(), String::new())
        }
        ReverseError::NotInstalled { component } => (
            "not_installed",
            format!("{} is not installed.", component_label(*component)),
            "Download it first.".into(),
        ),
        ReverseError::Busy => {
            let e = download_running();
            return AppError { details, ..e };
        }
        ReverseError::Cancelled => ("cancelled", "The download was cancelled.".into(), String::new()),
        ReverseError::AppleToolsMissing => (
            "apple_tools_missing",
            "On this Mac rea needs Apple's command line tools first.".into(),
            "Install them in step 1 above.".into(),
        ),
        ReverseError::Io { .. } => (
            "io",
            "harnessUI could not write or remove its reverse engineering files.".into(),
            "Make sure the disk is not full, then try again.".into(),
        ),
        ReverseError::NotAllowed { .. }
        | ReverseError::StartFailed { .. }
        | ReverseError::Timeout
        | ReverseError::Stopped
        | ReverseError::Protocol { .. } => (
            "rea_failed",
            "rea could not do that.".into(),
            "Try again. If it keeps happening, copy the details into a bug report.".into(),
        ),
    };
    AppError::new(code, message, next, details)
}

/// The error a finished download reports: none when it worked or the owner cancelled it.
pub(crate) fn finish_error(result: &Result<(), ReverseError>) -> Option<AppError> {
    match result {
        Ok(()) | Err(ReverseError::Cancelled) => None,
        Err(e) => Some(map_error(e)),
    }
}

// ---------------------------------------------------------------------------------------
// Downloads.

/// Turns the installer's progress into events: per-part numbers, at most one event every
/// [`PROGRESS_EVERY`] unless the part or the step changes.
#[derive(Default)]
struct Tracker {
    last: Option<(String, DownloadStage, Instant)>,
    /// Where each part's bytes start in the download's running total.
    bases: HashMap<String, u64>,
}

impl Tracker {
    fn next(&mut self, p: &Progress, sizes: &HashMap<String, u64>) -> Option<ReverseProgressDto> {
        let size = sizes.get(&p.part).copied().unwrap_or(0);
        let (part_done, part_total) = match p.stage {
            DownloadStage::Downloading => {
                // The installer reports each part's start first (its running total so far).
                let base = *self.bases.entry(p.part.clone()).or_insert(p.done);
                (p.done.saturating_sub(base).min(size), size)
            }
            DownloadStage::Checking => (0, size),
            DownloadStage::Unpacking => (p.done.min(p.total), p.total),
        };
        let now = Instant::now();
        let emit = match &self.last {
            None => true,
            Some((part, stage, at)) => {
                *part != p.part || *stage != p.stage || now.duration_since(*at) >= PROGRESS_EVERY
            }
        };
        if !emit {
            return None;
        }
        self.last = Some((p.part.clone(), p.stage, now));
        Some(ReverseProgressDto {
            component: p.component,
            part: p.part.clone(),
            done: p.done,
            total: p.total,
            stage: p.stage,
            part_done,
            part_total,
            finished: false,
            error: None,
        })
    }
}

/// Downloads, checks and unpacks a row (rea first when it is missing), sending progress.
pub(crate) async fn run_download(
    server: &ReverseServer,
    component: Component,
    token: &CancellationToken,
    emit: &(dyn Fn(ReverseProgressDto) + Send + Sync),
) -> Result<(), ReverseError> {
    let installer = server.installer();
    let platform = installer.platform();
    let sizes: HashMap<String, u64> = installer
        .lock()
        .parts
        .iter()
        .filter_map(|p| Some((p.name.clone(), p.build_for(platform)?.file.size)))
        .collect();
    let tracker = Mutex::new(Tracker::default());
    let progress = |p: Progress| {
        let next = tracker
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next(&p, &sizes);
        if let Some(dto) = next {
            emit(dto);
        }
    };
    installer
        .download(component, token, &progress)
        .await
        .map(|_| ())
}

/// After a row was installed, repaired or updated: the choice (the first decompiler
/// installed becomes it), the server's state, and the restart rea needs, if any (rea itself,
/// or the decompiler rea uses, changed).
pub(crate) async fn after_install(
    server: &ReverseServer,
    store: &store::Store,
    component: Component,
) -> AppResult<Option<RestartReason>> {
    let before = server.chosen_decompiler();
    let choice = reconcile(server, store).await?;
    server.refresh();
    let touches_rea = match component {
        Component::Rea => true,
        other => choice.map(Decompiler::component) == Some(other),
    };
    Ok(touches_rea.then_some(if choice != before {
        RestartReason::DecompilerChanged
    } else {
        RestartReason::Updated
    }))
}

/// Whether a download of `component` replaces or retires files rea uses now: an Update or a
/// Repair of rea itself or of the decompiler rea uses.
pub(crate) fn replaces_files_rea_uses(server: &ReverseServer, component: Component) -> bool {
    let uses = component == Component::Rea
        || server.chosen_decompiler().map(Decompiler::component) == Some(component);
    if !uses {
        return false;
    }
    match server.installer().state(component) {
        ComponentState::Broken { .. } | ComponentState::NeedsUpdate { .. } => true,
        ComponentState::Installed => server.installer().status(component).update.is_some(),
        _ => false,
    }
}

/// Stops rea when it runs and is idle, the way the idle stop does: while holding rea, so no
/// chat's call is cut, and a chat's open file opens again by itself on its next call (R21).
/// Before an Update or a Repair, so rea's files are not open while they are replaced or the
/// old version is deleted (Windows refuses to delete files a running program holds). When
/// rea is busy it is left alone: the download goes on and rea restarts once it is free.
/// Returns whether rea was stopped.
pub(crate) async fn stop_if_idle(server: &ReverseServer) -> bool {
    if server.process().state() != reverse::RunState::Ready {
        return false;
    }
    let Ok(_guard) = tokio::time::timeout(Duration::from_millis(50), server.process().lock()).await
    else {
        return false;
    };
    if server.process().state() != reverse::RunState::Ready {
        return false;
    }
    server.process().stop(reverse::process::STOP_GRACE).await;
    true
}

/// Restarts rea once it is free (a chat's call is never cut by a download finishing).
/// Gives up after `wait`; returns whether it restarted.
pub(crate) async fn restart_when_free(
    server: Arc<ReverseServer>,
    reason: RestartReason,
    wait: Duration,
) -> bool {
    let start = Instant::now();
    while server.process().is_busy() {
        if start.elapsed() >= wait {
            return false;
        }
        tokio::time::sleep(RESTART_POLL).await;
    }
    server.restart(reason).await;
    true
}

// ---------------------------------------------------------------------------------------
// Removing, switching, clearing.

/// Removes a row; stops rea first when rea uses it (rea itself, or the chosen decompiler).
pub(crate) async fn remove_row(
    server: &ReverseServer,
    store: &store::Store,
    component: Component,
    force: bool,
) -> AppResult<()> {
    let rows: Vec<Component> = match component {
        Component::Rea => Component::ALL.to_vec(),
        other => vec![other],
    };
    if rows.iter().any(|c| {
        matches!(
            server.installer().state(*c),
            ComponentState::Downloading { .. }
        )
    }) {
        return Err(download_running());
    }
    let uses = component == Component::Rea
        || server.chosen_decompiler().map(Decompiler::component) == Some(component);
    if uses {
        refuse_if_busy(server, force)?;
        // On Windows rea's files stay open while it runs.
        server.restart(RestartReason::Removed).await;
    }
    let removed = server.installer().remove(component).await;
    reconcile(server, store).await?;
    server.refresh();
    removed.map_err(|e| map_error(&e))
}

/// Switches the decompiler rea uses (restarting rea, which ends every chat's open file).
pub(crate) async fn switch_decompiler(
    server: &ReverseServer,
    store: &store::Store,
    decompiler: Decompiler,
    force: bool,
) -> AppResult<()> {
    if let Err(reason) = offered(server, decompiler.component()) {
        return Err(map_error(&ReverseError::NotAvailable { reason }));
    }
    if !on_disk(&server.installer().verify_installed(decompiler.component())) {
        return Err(AppError::new(
            "not_installed",
            format!("{} is not installed.", decompiler.label()),
            format!("Download {} first.", decompiler.label()),
            String::new(),
        ));
    }
    if server.chosen_decompiler() == Some(decompiler) {
        return Ok(());
    }
    refuse_if_busy(server, force)?;
    save_choice(store, Some(decompiler)).await?;
    server.set_decompiler(Some(decompiler));
    server.refresh();
    server.restart(RestartReason::DecompilerChanged).await;
    Ok(())
}

/// Deletes Kuna's saved analysis; stops rea first when it runs with Kuna.
pub(crate) async fn clear_kuna_cache(server: &ReverseServer, force: bool) -> AppResult<()> {
    if server.chosen_decompiler() == Some(Decompiler::Kuna) {
        refuse_if_busy(server, force)?;
        let running = !matches!(
            server.process().state(),
            reverse::RunState::Stopped | reverse::RunState::Failed { .. }
        );
        if running {
            // Kuna may have the folder open; what it saved is gone, as after a removal.
            server.restart(RestartReason::Removed).await;
        }
    }
    server
        .installer()
        .clear_cache()
        .await
        .map_err(|e| map_error(&e))
}

/// `xcode-select --install`: fixed words, through `crates/runtime`.
pub(crate) fn apple_tools_spec() -> runtime::spawn::SpawnSpec {
    runtime::spawn::SpawnSpec::new(XCODE_SELECT, "/")
        .arg("--install")
        .stdout(runtime::spawn::StdioMode::Null)
        .stderr(runtime::spawn::StdioMode::Null)
}

// ---------------------------------------------------------------------------------------
// Commands (main window only).

/// Everything the tab shows; Apple's command line tools are checked fresh (files only).
#[tauri::command]
pub async fn reverse_status(window: tauri::WebviewWindow, app: AppHandle) -> AppResult<ReverseTab> {
    crate::commands::only_main_window(window.label())?;
    let server = server(&app)?;
    let tab = tab(&server);
    if apple_tools_changed(tab.status.apple_tools) {
        // Installed (or removed) while the tab was open: the Tools panel's marks follow.
        server.refresh();
        tools_changed(&app);
    }
    Ok(tab)
}

/// What the last status said about Apple's command line tools.
static LAST_APPLE_TOOLS: Mutex<Option<AppleTools>> = Mutex::new(None);

/// Whether Apple's tools changed since the last status (not on the first one).
fn apple_tools_changed(now: AppleTools) -> bool {
    let mut last = LAST_APPLE_TOOLS.lock().unwrap_or_else(|e| e.into_inner());
    let changed = last.is_some_and(|before| before != now);
    *last = Some(now);
    changed
}

/// Starts a download (rea first when a decompiler is chosen and rea is missing). Returns at
/// once; progress arrives as `reverse-progress` events. Refused without the owner's consent
/// to the licences shown in the dialog. A second click while it runs does nothing.
#[tauri::command]
pub async fn reverse_download(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    component: Component,
    accept_license: bool,
) -> AppResult<()> {
    crate::commands::only_main_window(window.label())?;
    consent(accept_license)?;
    let server = server(&app)?;
    if let Err(reason) = offered(&server, component) {
        return Err(map_error(&ReverseError::NotAvailable { reason }));
    }
    let rows = rows_of_download(&server, component);
    let token = CancellationToken::new();
    {
        let mut map = DOWNLOADS.lock().map_err(AppError::internal)?;
        if map.contains_key(&component) {
            return Ok(());
        }
        if map
            .values()
            .any(|r| r.rows.iter().any(|c| rows.contains(c)))
        {
            return Err(download_running());
        }
        map.insert(
            component,
            Running {
                token: token.clone(),
                rows,
            },
        );
    }
    let store = state.store.clone();
    tauri::async_runtime::spawn(async move {
        let emit = |p: ReverseProgressDto| {
            let _ = app.emit_to(crate::MAIN_WINDOW, PROGRESS_EVENT, p);
        };
        if replaces_files_rea_uses(&server, component) {
            stop_if_idle(&server).await;
        }
        let result = run_download(&server, component, &token, &emit).await;
        if let Ok(mut map) = DOWNLOADS.lock() {
            map.remove(&component);
        }
        if let Err(e) = &result
            && !matches!(e, ReverseError::Cancelled)
        {
            tracing::warn!(?component, "a reverse engineering download failed: {e}");
        }
        let mut error = finish_error(&result);
        let mut restart = None;
        if result.is_ok() {
            match after_install(&server, &store, component).await {
                Ok(r) => restart = r,
                Err(e) => error = Some(e),
            }
        } else {
            server.refresh();
        }
        tools_changed(&app);
        emit(ReverseProgressDto::finished(component, error));
        if let Some(reason) = restart
            && !restart_when_free(server, reason, RESTART_WAIT).await
        {
            tracing::info!("rea stayed busy; it uses the new files from its next start");
        }
    });
    Ok(())
}

/// Stops a download; what is already downloaded is kept ("Paused at …"). Cancelling rea's
/// row also stops a decompiler's download that brings rea along.
#[tauri::command]
pub async fn reverse_cancel(window: tauri::WebviewWindow, component: Component) -> AppResult<()> {
    crate::commands::only_main_window(window.label())?;
    if let Ok(map) = DOWNLOADS.lock() {
        for running in map.values() {
            if running.rows.contains(&component) {
                running.token.cancel();
            }
        }
    }
    Ok(())
}

/// Deletes a paused download (Remove on "Paused at …").
#[tauri::command]
pub async fn reverse_discard_partial(
    window: tauri::WebviewWindow,
    app: AppHandle,
    component: Component,
) -> AppResult<ReverseTab> {
    crate::commands::only_main_window(window.label())?;
    let server = server(&app)?;
    server
        .installer()
        .discard_partial(component)
        .await
        .map_err(|e| map_error(&e))?;
    server.refresh();
    tools_changed(&app);
    Ok(tab(&server))
}

/// Removes a row (rea removes everything). Answers `busy` while rea works, unless forced.
#[tauri::command]
pub async fn reverse_remove(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    component: Component,
    force: bool,
) -> AppResult<ReverseTab> {
    crate::commands::only_main_window(window.label())?;
    let server = server(&app)?;
    let result = remove_row(&server, &state.store, component, force).await;
    tools_changed(&app);
    result.map(|()| tab(&server))
}

/// Chooses the decompiler rea uses. Answers `busy` while rea works, unless forced.
#[tauri::command]
pub async fn reverse_set_decompiler(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    decompiler: Decompiler,
    force: bool,
) -> AppResult<ReverseTab> {
    crate::commands::only_main_window(window.label())?;
    let server = server(&app)?;
    switch_decompiler(&server, &state.store, decompiler, force).await?;
    tools_changed(&app);
    Ok(tab(&server))
}

/// Deletes Kuna's saved analysis. Answers `busy` while rea works with Kuna, unless forced.
#[tauri::command]
pub async fn reverse_clear_cache(
    window: tauri::WebviewWindow,
    app: AppHandle,
    force: bool,
) -> AppResult<ReverseTab> {
    crate::commands::only_main_window(window.label())?;
    let server = server(&app)?;
    clear_kuna_cache(&server, force).await?;
    Ok(tab(&server))
}

/// macOS only: opens Apple's own installer for the command line tools. The tab checks
/// again every few seconds and turns to "Installed" by itself.
#[tauri::command]
pub async fn reverse_install_apple_tools(
    window: tauri::WebviewWindow,
    app: AppHandle,
) -> AppResult<()> {
    crate::commands::only_main_window(window.label())?;
    let platform = server(&app)?.installer().platform();
    if platform.os() != Os::Macos {
        return Err(AppError::bad_request(
            "Apple's command line tools are needed only on a Mac.",
        ));
    }
    if reverse::platform::apple_tools(platform) == AppleTools::Present {
        return Ok(());
    }
    let mut child = runtime::spawn::spawn(apple_tools_spec()).map_err(|e| {
        AppError::new(
            "apple_tools",
            "Apple's installer could not be opened.",
            "Try again in a moment.",
            e.to_string(),
        )
    })?;
    // xcode-select only asks the system to show Apple's installer, then ends.
    let _ = tokio::time::timeout(XCODE_SELECT_WAIT, child.wait()).await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use reverse::{Installer, RunState};

    const COMMANDS: [&str; 8] = [
        "reverse_status",
        "reverse_download",
        "reverse_cancel",
        "reverse_discard_partial",
        "reverse_remove",
        "reverse_set_decompiler",
        "reverse_clear_cache",
        "reverse_install_apple_tools",
    ];

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("reverse-cmd-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// A server for Linux over `root` (the embedded lock; debug rules, so Kuna is offered).
    fn linux_server(root: &std::path::Path) -> Arc<ReverseServer> {
        ReverseServer::with_installer(
            Installer::new(ReverseLayout::new(root), Platform::LinuxX64)
                .with_release_rules(false)
                .with_free_space(Arc::new(|_| Some(u64::MAX))),
        )
    }

    fn server_with_fetch(root: &std::path::Path, cache: &std::path::Path) -> Arc<ReverseServer> {
        ReverseServer::with_installer(
            Installer::new(ReverseLayout::new(root), Platform::LinuxX64)
                .with_release_rules(false)
                .with_free_space(Arc::new(|_| Some(u64::MAX)))
                .with_fetch(Arc::new(reverse::DirFetch::new(cache))),
        )
    }

    /// Puts a part on disk the way the installer leaves it: its folder with `.complete`
    /// last (no programs recorded, so nothing else is checked).
    fn put_part(root: &std::path::Path, component: &str, part: &str, version: &str) {
        let dir = root.join(format!("{part}-{version}"));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = serde_json::json!({
            "component": component,
            "part": part,
            "version": version,
            "archiveSha256": [],
            "programs": [],
            "diskBytes": 1_000_000,
        });
        std::fs::write(dir.join(".complete"), marker.to_string()).unwrap();
    }

    fn put_rea(root: &std::path::Path) {
        put_part(root, "rea", "rea", "6.9.0");
        put_part(root, "rea", "node", "24.22.3");
    }
    fn put_kuna(root: &std::path::Path) {
        put_part(root, "kuna", "kuna", "1.990");
    }
    fn put_ghidra(root: &std::path::Path) {
        put_part(root, "ghidra", "ghidra", "12.1.4");
        put_part(root, "ghidra", "java", "21.0.13+9");
    }

    async fn saved(store: &store::Store) -> Option<serde_json::Value> {
        store.get_setting(DECOMPILER_KEY).await.unwrap()
    }

    /// Every command of this module checks the calling window first, like its neighbours,
    /// and is registered, listed and granted to the main window.
    #[test]
    fn every_command_is_for_the_main_window_only() {
        let source = include_str!("reverse_cmd.rs");
        let source = source.split("#[cfg(test)]").next().unwrap();
        let commands: Vec<&str> = source.split("#[tauri::command]").skip(1).collect();
        assert_eq!(commands.len(), COMMANDS.len());
        for (command, name) in commands.iter().zip(COMMANDS) {
            assert!(command.contains(&format!("pub async fn {name}(")), "{name}");
            let body = command.split_once(") -> AppResult").unwrap().1;
            let body = body.split_once('{').unwrap().1;
            let first = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap();
            assert_eq!(
                first, "crate::commands::only_main_window(window.label())?;",
                "{name}"
            );
        }
        let capabilities = include_str!("../capabilities/main.json");
        let list = include_str!("../commands.list");
        let lib = include_str!("lib.rs");
        for name in COMMANDS {
            assert!(
                capabilities.contains(&format!("\"allow-{}\"", name.replace('_', "-"))),
                "{name}"
            );
            assert!(list.lines().any(|l| l.trim() == name), "{name}");
            assert!(lib.contains(&format!("reverse_cmd::{name},")), "{name}");
        }
    }

    #[test]
    fn a_download_needs_the_owners_consent() {
        let e = consent(false).unwrap_err();
        assert_eq!(e.code, "bad_request");
        assert_eq!(
            e.message,
            "The licences must be accepted before anything is downloaded."
        );
        assert!(consent(true).is_ok());
        // The command checks it before it looks at anything else.
        let source = include_str!("reverse_cmd.rs");
        let download = source
            .split("pub async fn reverse_download(")
            .nth(1)
            .unwrap();
        let body = download.split_once(") -> AppResult<()> {").unwrap().1;
        let lines: Vec<&str> = body
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .take(2)
            .collect();
        assert_eq!(lines[1], "consent(accept_license)?;");
    }

    /// D12: the choice is never on the generic setting command's list, so no tool and no
    /// model output reaches it; only `reverse_set_decompiler` writes it.
    #[test]
    fn the_choice_is_not_writable_through_set_setting() {
        let models = include_str!("models_cmd.rs");
        let list = models
            .split("const ALLOWED: &[&str] = &[")
            .nth(1)
            .and_then(|s| s.split("];").next())
            .expect("the generic setting command's list");
        assert!(!list.contains(DECOMPILER_KEY), "{list}");
        assert!(!list.to_ascii_lowercase().contains("reverse"), "{list}");
        assert!(!list.contains("DECOMPILER"), "{list}");
        // Only `save_choice` writes the key.
        let source = include_str!("reverse_cmd.rs");
        let source = source.split("#[cfg(test)]").next().unwrap();
        assert_eq!(source.matches("set_setting(DECOMPILER_KEY").count(), 1);
    }

    #[test]
    fn the_choice_follows_what_is_on_this_computer() {
        use Decompiler::{Ghidra, Kuna};
        assert_eq!(resolve_choice(None, &[]), None);
        assert_eq!(resolve_choice(Some(Kuna), &[]), None);
        assert_eq!(resolve_choice(None, &[Ghidra]), Some(Ghidra));
        assert_eq!(resolve_choice(None, &[Kuna, Ghidra]), Some(Kuna));
        assert_eq!(resolve_choice(Some(Ghidra), &[Kuna, Ghidra]), Some(Ghidra));
        assert_eq!(resolve_choice(Some(Kuna), &[Ghidra]), Some(Ghidra));
    }

    #[tokio::test]
    async fn switching_refuses_a_decompiler_that_is_not_installed() {
        let dir = scratch("switch-missing");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        reconcile(&server, &store).await.unwrap();
        let e = switch_decompiler(&server, &store, Decompiler::Ghidra, true)
            .await
            .unwrap_err();
        assert_eq!(e.code, "not_installed");
        assert_eq!(e.message, "Ghidra is not installed.");
        assert_eq!(saved(&store).await, Some(serde_json::json!("kuna")));
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Kuna));
    }

    /// A server for Linux with the release rules on, over a lock in which every part has
    /// its publisher checks except Ghidra's, pinned on first download only: a release build
    /// withholds Ghidra ("Not available in this version of harnessUI.").
    fn release_server_without_ghidra(root: &std::path::Path) -> Arc<ReverseServer> {
        let mut lock = reverse::lock::embedded().clone();
        for p in &mut lock.parts {
            for b in &mut p.builds {
                b.verified_by = reverse::lock::release_checks(&p.name)
                    .unwrap()
                    .iter()
                    .map(|k| format!("{k}{}", if k.ends_with(':') { "x@y" } else { "" }))
                    .collect();
                if b.verified_by.is_empty() || p.name == "ghidra" {
                    b.verified_by = vec!["first-download-only".into()];
                }
            }
        }
        ReverseServer::with_installer(
            Installer::new(ReverseLayout::new(root), Platform::LinuxX64)
                .with_lock(Arc::new(lock))
                .with_release_rules(true)
                .with_free_space(Arc::new(|_| Some(u64::MAX))),
        )
    }

    /// A decompiler this build does not offer is never the choice, even with its files on
    /// disk (left by a debug build, or withdrawn by a later lock), and switching to it is
    /// refused with the row's reason: rea never starts with it.
    #[tokio::test]
    async fn a_decompiler_this_build_does_not_offer_is_never_the_choice() {
        let dir = scratch("withheld");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        let server = release_server_without_ghidra(&dir.0);
        let ghidra = server.installer().status(Component::Ghidra);
        assert!(!ghidra.available);
        assert_eq!(
            ghidra.reason.as_deref(),
            Some("Not available in this version of harnessUI.")
        );
        assert!(server.installer().status(Component::Kuna).available);
        let store = store::Store::open_in_memory().unwrap();
        // A saved choice of the withheld one moves to the offered one at start.
        store.set_setting(DECOMPILER_KEY, "ghidra").await.unwrap();
        assert_eq!(
            reconcile(&server, &store).await.unwrap(),
            Some(Decompiler::Kuna)
        );
        assert_eq!(saved(&store).await, Some(serde_json::json!("kuna")));
        assert_eq!(server.decompiler(), Some(Decompiler::Kuna));
        // Switching to it is refused with the row's own reason; nothing changes.
        let e = switch_decompiler(&server, &store, Decompiler::Ghidra, true)
            .await
            .unwrap_err();
        assert_eq!(e.code, "not_available");
        assert_eq!(e.message, "Not available in this version of harnessUI.");
        assert_eq!(saved(&store).await, Some(serde_json::json!("kuna")));
        assert_eq!(server.decompiler(), Some(Decompiler::Kuna));
        // Without the offered one, there is no choice at all.
        remove_row(&server, &store, Component::Kuna, false)
            .await
            .unwrap();
        assert_eq!(server.chosen_decompiler(), None);
        assert_eq!(server.decompiler(), None);
        assert_eq!(saved(&store).await, None);
        // The withheld row's files can still be removed, to free the space.
        remove_row(&server, &store, Component::Ghidra, false)
            .await
            .unwrap();
        assert!(!dir.0.join("ghidra-12.1.4").exists());
        assert!(!dir.0.join("java-21.0.13+9").exists());
        assert!(dir.0.join("rea-6.9.0").exists());
    }

    #[tokio::test]
    async fn switching_answers_busy_while_rea_works_unless_forced() {
        let dir = scratch("switch-busy");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        assert_eq!(
            reconcile(&server, &store).await.unwrap(),
            Some(Decompiler::Kuna)
        );
        // A chat's call holds rea.
        let guard = server.process().lock().await;
        assert!(server.process().is_busy());
        let e = switch_decompiler(&server, &store, Decompiler::Ghidra, false)
            .await
            .unwrap_err();
        assert_eq!(e.code, "busy");
        assert_eq!(e.message, "rea is working on a file in a chat.");
        assert_eq!(saved(&store).await, Some(serde_json::json!("kuna")));
        // "Stop and continue": the call ends (here: soon after) and the switch goes on.
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(guard);
        });
        switch_decompiler(&server, &store, Decompiler::Ghidra, true)
            .await
            .unwrap();
        release.await.unwrap();
        assert_eq!(saved(&store).await, Some(serde_json::json!("ghidra")));
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Ghidra));
        assert_eq!(server.decompiler(), Some(Decompiler::Ghidra));
        // The same choice again changes nothing, busy or not.
        let _guard = server.process().lock().await;
        switch_decompiler(&server, &store, Decompiler::Ghidra, false)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_first_decompiler_installed_becomes_the_choice() {
        let dir = scratch("first");
        put_rea(&dir.0);
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        assert_eq!(
            after_install(&server, &store, Component::Rea)
                .await
                .unwrap(),
            Some(RestartReason::Updated)
        );
        assert_eq!(server.chosen_decompiler(), None);
        assert_eq!(saved(&store).await, None);

        put_ghidra(&dir.0);
        assert_eq!(
            after_install(&server, &store, Component::Ghidra)
                .await
                .unwrap(),
            Some(RestartReason::DecompilerChanged)
        );
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Ghidra));
        assert_eq!(saved(&store).await, Some(serde_json::json!("ghidra")));

        // A second decompiler does not take over, and rea need not restart for it.
        put_kuna(&dir.0);
        assert_eq!(
            after_install(&server, &store, Component::Kuna)
                .await
                .unwrap(),
            None
        );
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Ghidra));
        // A repair or update of the chosen one restarts rea.
        assert_eq!(
            after_install(&server, &store, Component::Ghidra)
                .await
                .unwrap(),
            Some(RestartReason::Updated)
        );
    }

    #[tokio::test]
    async fn the_saved_choice_is_read_at_start() {
        let dir = scratch("start");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        store.set_setting(DECOMPILER_KEY, "ghidra").await.unwrap();
        assert_eq!(
            reconcile(&server, &store).await.unwrap(),
            Some(Decompiler::Ghidra)
        );
        assert_eq!(server.decompiler(), Some(Decompiler::Ghidra));
        // A value that is not a decompiler is ignored (never `auto`, never another program).
        store.set_setting(DECOMPILER_KEY, "hopper").await.unwrap();
        assert_eq!(
            reconcile(&server, &store).await.unwrap(),
            Some(Decompiler::Kuna)
        );
        assert_eq!(saved(&store).await, Some(serde_json::json!("kuna")));
        // A choice whose files were deleted by hand follows what is there.
        store.set_setting(DECOMPILER_KEY, "ghidra").await.unwrap();
        std::fs::remove_dir_all(dir.0.join("ghidra-12.1.4")).unwrap();
        std::fs::remove_dir_all(dir.0.join("java-21.0.13+9")).unwrap();
        assert_eq!(
            reconcile(&server, &store).await.unwrap(),
            Some(Decompiler::Kuna)
        );
    }

    #[tokio::test]
    async fn removing_the_chosen_decompiler_switches_to_the_other_or_none() {
        let dir = scratch("remove");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        std::fs::create_dir_all(dir.0.join("cache/kuna")).unwrap();
        std::fs::write(dir.0.join("cache/kuna/project"), b"saved").unwrap();
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        reconcile(&server, &store).await.unwrap();
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Kuna));

        remove_row(&server, &store, Component::Kuna, false)
            .await
            .unwrap();
        assert!(!dir.0.join("kuna-1.990").exists());
        assert!(!dir.0.join("cache/kuna").exists());
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Ghidra));
        assert_eq!(saved(&store).await, Some(serde_json::json!("ghidra")));

        remove_row(&server, &store, Component::Ghidra, false)
            .await
            .unwrap();
        assert_eq!(server.chosen_decompiler(), None);
        assert_eq!(saved(&store).await, None);
        assert!(dir.0.join("rea-6.9.0").exists());

        remove_row(&server, &store, Component::Rea, false)
            .await
            .unwrap();
        assert!(!dir.0.join("rea-6.9.0").exists());
        assert!(!dir.0.join("node-24.22.3").exists());
        let t = tab(&server);
        assert!(
            t.status
                .components
                .iter()
                .all(|c| c.state == ComponentState::NotInstalled)
        );
    }

    #[tokio::test]
    async fn removing_answers_busy_only_when_it_would_stop_rea() {
        let dir = scratch("remove-busy");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        reconcile(&server, &store).await.unwrap();
        let guard = server.process().lock().await;
        // Ghidra is not what rea uses: no question.
        remove_row(&server, &store, Component::Ghidra, false)
            .await
            .unwrap();
        for c in [Component::Kuna, Component::Rea] {
            let e = remove_row(&server, &store, c, false).await.unwrap_err();
            assert_eq!(e.code, "busy", "{c:?}");
        }
        assert!(dir.0.join("kuna-1.990").exists());
        assert!(dir.0.join("rea-6.9.0").exists());
        drop(guard);
        remove_row(&server, &store, Component::Kuna, false)
            .await
            .unwrap();
        assert!(!dir.0.join("kuna-1.990").exists());
    }

    #[tokio::test]
    async fn clearing_kunas_saved_analysis_asks_while_rea_works_with_kuna() {
        let dir = scratch("cache");
        put_rea(&dir.0);
        put_kuna(&dir.0);
        let kuna_cache = dir.0.join("cache/kuna");
        std::fs::create_dir_all(&kuna_cache).unwrap();
        std::fs::write(kuna_cache.join("project"), vec![0u8; 4096]).unwrap();
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        reconcile(&server, &store).await.unwrap();
        assert_eq!(tab(&server).status.cache_bytes, 4096);
        let guard = server.process().lock().await;
        assert_eq!(
            clear_kuna_cache(&server, false).await.unwrap_err().code,
            "busy"
        );
        assert!(kuna_cache.exists());
        clear_kuna_cache(&server, true).await.unwrap();
        assert!(!kuna_cache.exists());
        drop(guard);
        assert_eq!(tab(&server).status.cache_bytes, 0);
    }

    #[tokio::test]
    async fn a_finished_download_waits_for_rea_to_be_free() {
        let dir = scratch("restart");
        let server = linux_server(&dir.0);
        let guard = server.process().lock().await;
        assert!(
            !restart_when_free(
                server.clone(),
                RestartReason::Updated,
                Duration::from_millis(50)
            )
            .await
        );
        drop(guard);
        assert!(
            restart_when_free(
                server.clone(),
                RestartReason::Updated,
                Duration::from_millis(50)
            )
            .await
        );
        assert_eq!(server.process().state(), RunState::Stopped);
    }

    /// An Update or a Repair of what rea uses stops an idle rea first (its files may be open);
    /// a fresh download, or a row rea does not use, leaves rea alone.
    #[tokio::test]
    async fn only_an_update_or_repair_of_what_rea_uses_stops_rea_first() {
        let dir = scratch("replaces");
        let server = linux_server(&dir.0);
        let store = store::Store::open_in_memory().unwrap();
        for c in Component::ALL {
            assert!(!replaces_files_rea_uses(&server, c), "{c:?} not installed");
        }
        put_rea(&dir.0);
        put_kuna(&dir.0);
        put_ghidra(&dir.0);
        reconcile(&server, &store).await.unwrap();
        assert_eq!(server.chosen_decompiler(), Some(Decompiler::Kuna));
        for c in Component::ALL {
            assert!(
                !replaces_files_rea_uses(&server, c),
                "{c:?} installed, current"
            );
        }
        // A file of each decompiler is gone: only the one rea uses counts.
        for (part, version, program) in
            [("kuna", "1.990", "kuna"), ("java", "21.0.13+9", "bin/java")]
        {
            let marker = dir.0.join(format!("{part}-{version}/.complete"));
            let mut m: serde_json::Value =
                serde_json::from_str(&std::fs::read_to_string(&marker).unwrap()).unwrap();
            m["programs"] = serde_json::json!([{ "path": program, "size": 5 }]);
            std::fs::write(&marker, m.to_string()).unwrap();
        }
        assert!(matches!(
            server.installer().state(Component::Ghidra),
            ComponentState::Broken { .. }
        ));
        assert!(replaces_files_rea_uses(&server, Component::Kuna));
        assert!(!replaces_files_rea_uses(&server, Component::Ghidra));
        // An outdated part of rea.
        std::fs::remove_dir_all(dir.0.join("node-24.22.3")).unwrap();
        put_part(&dir.0, "rea", "node", "24.0.0");
        assert!(matches!(
            server.installer().state(Component::Rea),
            ComponentState::NeedsUpdate { .. }
        ));
        assert!(replaces_files_rea_uses(&server, Component::Rea));
        // rea is not running: nothing to stop, and nothing waits, even while a call holds it.
        assert!(!stop_if_idle(&server).await);
        let _guard = server.process().lock().await;
        assert!(!stop_if_idle(&server).await);
    }

    #[tokio::test]
    async fn a_download_reports_each_part_and_ends_with_the_plain_error() {
        let dir = scratch("download");
        let root = dir.0.join("reverse");
        let cache = dir.0.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let server = server_with_fetch(&root, &cache);
        let lock = server.installer().lock().clone();
        let mut sizes = HashMap::new();
        // Files with the pinned names but the wrong bytes: fetched, then refused.
        for name in ["rea", "node"] {
            let build = lock
                .part(name)
                .unwrap()
                .build_for(Platform::LinuxX64)
                .unwrap();
            std::fs::write(cache.join(&build.file.name), b"not the pinned file").unwrap();
            sizes.insert(name, build.file.size);
        }
        let events = Mutex::new(Vec::<ReverseProgressDto>::new());
        let token = CancellationToken::new();
        let result = run_download(&server, Component::Rea, &token, &|p| {
            events.lock().unwrap().push(p)
        })
        .await;
        assert_eq!(result, Err(ReverseError::Mismatch { part: "rea".into() }));
        let events = events.into_inner().unwrap();
        for name in ["rea", "node"] {
            let first = events
                .iter()
                .find(|e| e.part == name && e.stage == DownloadStage::Downloading)
                .unwrap_or_else(|| panic!("no event for {name}: {events:?}"));
            assert_eq!(first.part_total, sizes[name]);
            assert_eq!(first.component, Component::Rea);
            assert!(!first.finished);
        }
        assert!(
            events
                .iter()
                .any(|e| e.part == "rea" && e.stage == DownloadStage::Checking)
        );
        let error = finish_error(&result).unwrap();
        assert_eq!(
            error.message,
            "The download doesn't match the copy harnessUI expects, so it wasn't installed."
        );
        assert_eq!(
            error.next_step,
            "Try again. If it keeps failing, report it."
        );
        assert!(!root.join("rea-6.9.0").exists());

        // The last event carries the error and nothing else.
        let last = serde_json::to_value(ReverseProgressDto::finished(Component::Rea, Some(error)))
            .unwrap();
        assert_eq!(last["finished"], true);
        assert_eq!(last["component"], "rea");
        assert_eq!(last["error"]["code"], "mismatch");
    }

    /// A host that answers every file with an error status, like the real download does
    /// (`reverse::install`'s `from_pack_error`): the host is the file's own.
    struct RefusingFetch(u16);

    #[async_trait::async_trait]
    impl reverse::Fetch for RefusingFetch {
        async fn fetch(
            &self,
            file: &runtime::PackFile,
            _dest: &std::path::Path,
            _hosts: &'static [&'static str],
            _cancel: &CancellationToken,
            _on_bytes: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<(), ReverseError> {
            let host = reqwest::Url::parse(&file.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            Err(ReverseError::ServerError {
                host,
                status: self.0,
            })
        }
    }

    /// The tab's last event names the server that refused the file and its error number in
    /// plain words; the technical line stays behind "Copy details".
    #[tokio::test]
    async fn a_server_that_refuses_a_file_is_named_with_its_error_number() {
        for (status, component, host) in [
            (404, Component::Rea, "registry.npmjs.org"),
            (503, Component::Ghidra, "github.com"),
        ] {
            let dir = scratch("refused");
            let root = dir.0.join("reverse");
            if component != Component::Rea {
                put_rea(&root);
            }
            let server = ReverseServer::with_installer(
                Installer::new(ReverseLayout::new(&root), Platform::LinuxX64)
                    .with_release_rules(false)
                    .with_free_space(Arc::new(|_| Some(u64::MAX)))
                    .with_fetch(Arc::new(RefusingFetch(status))),
            );
            let token = CancellationToken::new();
            let result = run_download(&server, component, &token, &|_| {}).await;
            assert_eq!(
                result,
                Err(ReverseError::ServerError {
                    host: host.into(),
                    status,
                })
            );
            let error = finish_error(&result).unwrap();
            let last =
                serde_json::to_value(ReverseProgressDto::finished(component, Some(error))).unwrap();
            assert_eq!(last["finished"], true);
            assert_eq!(
                last["error"],
                serde_json::json!({
                    "code": "server_error",
                    "message": format!("{host} didn't send the file (error {status})."),
                    "nextStep": "Try again later. If it keeps failing, report it.",
                    "details": format!("{host} answered HTTP {status}"),
                })
            );
        }
    }

    /// An update of Node.js or Java alone names that part (the tab says "Update available:
    /// Node.js 24.22.2 → 24.22.3"), never the row's main part at a version it does not have.
    #[test]
    fn the_tab_names_the_part_an_update_is_for() {
        let dir = scratch("part-update");
        put_part(&dir.0, "rea", "rea", "6.9.0");
        put_part(&dir.0, "rea", "node", "24.22.2");
        put_part(&dir.0, "ghidra", "ghidra", "12.1.4");
        put_part(&dir.0, "ghidra", "java", "21.0.12+7");
        let server = linux_server(&dir.0);
        let v = serde_json::to_value(tab(&server)).unwrap();
        let row = |name: &str| {
            v["components"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["component"] == name)
                .unwrap()
                .clone()
        };
        let lock = server.installer().lock().clone();
        let size = |part: &str| {
            lock.part(part)
                .unwrap()
                .build_for(Platform::LinuxX64)
                .unwrap()
                .file
                .size
        };
        let rea = row("rea");
        assert_eq!(rea["state"]["kind"], "installed", "{rea}");
        assert_eq!(
            rea["update"],
            serde_json::json!({
                "part": "node",
                "from": "24.22.2",
                "to": "24.22.3",
                "downloadBytes": size("node"),
            })
        );
        let ghidra = row("ghidra");
        assert_eq!(ghidra["state"]["kind"], "installed", "{ghidra}");
        assert_eq!(
            ghidra["update"],
            serde_json::json!({
                "part": "java",
                "from": "21.0.12+7",
                "to": "21.0.13+9",
                "downloadBytes": size("java"),
            })
        );
    }

    #[tokio::test]
    async fn a_cancelled_download_ends_without_an_error_and_stays_paused() {
        let dir = scratch("cancel");
        let root = dir.0.join("reverse");
        let cache = dir.0.join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let server = server_with_fetch(&root, &cache);
        let build = server
            .installer()
            .lock()
            .part("rea")
            .unwrap()
            .build_for(Platform::LinuxX64)
            .unwrap()
            .clone();
        std::fs::write(cache.join(&build.file.name), vec![7u8; 4 << 20]).unwrap();
        // Part of it came before (a paused download), then the owner clicked Cancel.
        let downloads = root.join(".downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(
            downloads.join(reverse::install::part_file_name(&build.file)),
            vec![7u8; 1 << 20],
        )
        .unwrap();
        let token = CancellationToken::new();
        token.cancel();
        let result = run_download(&server, Component::Rea, &token, &|_| {}).await;
        assert_eq!(result, Err(ReverseError::Cancelled));
        assert!(finish_error(&result).is_none());
        assert_eq!(
            server.installer().state(Component::Rea),
            ComponentState::Partial { bytes: 1 << 20 }
        );
    }

    #[test]
    fn progress_events_carry_each_parts_own_numbers() {
        let sizes: HashMap<String, u64> = [("rea".to_string(), 100), ("node".to_string(), 50)]
            .into_iter()
            .collect();
        let mut t = Tracker::default();
        let p = |part: &str, done, total, stage| Progress {
            component: Component::Kuna,
            part: part.into(),
            done,
            total,
            stage,
        };
        let e = t
            .next(&p("rea", 0, 150, DownloadStage::Downloading), &sizes)
            .unwrap();
        assert_eq!((e.part_done, e.part_total), (0, 100));
        // Within the interval, same part and step: not sent.
        assert!(
            t.next(&p("rea", 40, 150, DownloadStage::Downloading), &sizes)
                .is_none()
        );
        // The next part starts where the first ended.
        let e = t
            .next(&p("node", 100, 150, DownloadStage::Downloading), &sizes)
            .unwrap();
        assert_eq!((e.part_done, e.part_total), (0, 50));
        t.last = None;
        let e = t
            .next(&p("node", 130, 150, DownloadStage::Downloading), &sizes)
            .unwrap();
        assert_eq!((e.part_done, e.part_total), (30, 50));
        let e = t
            .next(&p("rea", 0, 150, DownloadStage::Checking), &sizes)
            .unwrap();
        assert_eq!((e.part_done, e.part_total), (0, 100));
        let e = t
            .next(&p("rea", 70, 300, DownloadStage::Unpacking), &sizes)
            .unwrap();
        assert_eq!((e.part_done, e.part_total), (70, 300));
        assert_eq!(e.component, Component::Kuna);
    }

    #[test]
    fn every_problem_reads_in_plain_words() {
        let cases: Vec<(ReverseError, &str, &str, &str)> = vec![
            (
                ReverseError::Mismatch { part: "rea".into() },
                "mismatch",
                "The download doesn't match the copy harnessUI expects, so it wasn't installed.",
                "Try again. If it keeps failing, report it.",
            ),
            (
                ReverseError::Interrupted {
                    part: "node".into(),
                },
                "interrupted",
                "The download was interrupted.",
                "Check your connection and try again; it continues where it stopped.",
            ),
            (
                ReverseError::CantReach {
                    host: "nodejs.org".into(),
                },
                "cant_reach",
                "Can't reach nodejs.org.",
                "Check your internet connection and try again.",
            ),
            (
                ReverseError::DiskFull {
                    need_bytes: 1_150_000_000,
                },
                "disk_full",
                "The disk is full.",
                "You need 1.2 GB free.",
            ),
            (
                ReverseError::DiskFull { need_bytes: 0 },
                "disk_full",
                "The disk is full.",
                "Free some space and try again; it continues where it stopped.",
            ),
            (
                ReverseError::PathTooLong {
                    chars: 260,
                    max: 250,
                },
                "path_too_long",
                "The folder harnessUI keeps its files in has too long a path for Ghidra.",
                "This is rare; please report it.",
            ),
            (
                ReverseError::BlockedHost {
                    host: "example.com".into(),
                },
                "blocked_host",
                "The download was redirected to a server harnessUI does not trust.",
                "Report this.",
            ),
            (
                ReverseError::Missing {
                    part: "node".into(),
                },
                "missing",
                "Part of rea is missing, perhaps removed by antivirus software.",
                // The banner's Repair button is the next step (item5-spec.md WP9d Errors).
                "",
            ),
            (
                ReverseError::Blocked {
                    part: "java".into(),
                },
                "blocked",
                "Your antivirus blocked part of Ghidra.",
                "Allow harnessUI in it, then choose Repair.",
            ),
            (
                ReverseError::Blocked { part: "rea".into() },
                "blocked",
                "Your antivirus blocked part of rea.",
                "Allow harnessUI in it, then choose Repair.",
            ),
            (
                ReverseError::ServerError {
                    host: "github.com".into(),
                    status: 404,
                },
                "server_error",
                "github.com didn't send the file (error 404).",
                "Try again later. If it keeps failing, report it.",
            ),
            (
                ReverseError::NeedsUpdate {
                    part: "java".into(),
                },
                "needs_update",
                "This version of Java has a known problem.",
                "Update it to keep using it.",
            ),
            (
                ReverseError::NotAvailable {
                    reason: "rea can't use Kuna on Windows yet.".into(),
                },
                "not_available",
                "rea can't use Kuna on Windows yet.",
                "",
            ),
            (
                ReverseError::Busy,
                "download_running",
                "A download of this part is already running.",
                "Wait for it to finish, or cancel it first.",
            ),
            (
                ReverseError::Archive {
                    part: "ghidra".into(),
                    message: "x".into(),
                },
                "archive",
                "The download of Ghidra was damaged, so it wasn't installed.",
                "Try again. If it keeps failing, report it.",
            ),
            (
                ReverseError::NotNtfs,
                "not_local_drive",
                "Ghidra can't be installed, because the folder harnessUI keeps its files in is not on this PC's own drive.",
                "This is rare; please report it.",
            ),
            (
                ReverseError::AppleToolsMissing,
                "apple_tools_missing",
                "On this Mac rea needs Apple's command line tools first.",
                "Install them in step 1 above.",
            ),
        ];
        for (e, code, message, next) in cases {
            let a = map_error(&e);
            assert_eq!(a.code, code, "{e:?}");
            assert_eq!(a.message, message, "{e:?}");
            assert_eq!(a.next_step, next, "{e:?}");
            assert_eq!(a.details, e.to_string(), "{e:?}");
        }
        assert!(finish_error(&Err(ReverseError::Cancelled)).is_none());
        assert!(finish_error(&Ok(())).is_none());
    }

    #[test]
    fn the_tab_reads_these_shapes() {
        let dir = scratch("shape");
        let server = linux_server(&dir.0);
        let v = serde_json::to_value(tab(&server)).unwrap();
        for key in [
            "components",
            "decompiler",
            "appleTools",
            "running",
            "cacheBytes",
            "memoryGb",
            "os",
            "chosen",
            "partSizes",
        ] {
            assert!(v.get(key).is_some(), "{key}: {v}");
        }
        assert_eq!(v["os"], "linux");
        assert_eq!(v["appleTools"], "notNeeded");
        assert_eq!(v["running"], serde_json::json!({"kind": "stopped"}));
        assert_eq!(v["components"][0]["component"], "rea");
        assert_eq!(v["components"][0]["state"]["kind"], "notInstalled");
        let parts: Vec<&str> = v["partSizes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["name"].as_str().unwrap())
            .collect();
        assert_eq!(parts, ["rea", "node", "kuna", "ghidra", "java"]);
        assert!(v["partSizes"][0]["downloadBytes"].as_u64().unwrap() > 0);
    }

    #[test]
    fn a_change_of_apples_tools_is_noticed_once() {
        // The first status only records what it saw.
        *LAST_APPLE_TOOLS.lock().unwrap() = None;
        assert!(!apple_tools_changed(AppleTools::Missing));
        assert!(!apple_tools_changed(AppleTools::Missing));
        assert!(apple_tools_changed(AppleTools::Present));
        assert!(!apple_tools_changed(AppleTools::Present));
    }

    #[test]
    fn apples_installer_is_opened_with_fixed_words() {
        let spec = apple_tools_spec();
        assert_eq!(
            spec.program,
            std::path::PathBuf::from("/usr/bin/xcode-select")
        );
        assert_eq!(spec.args, vec![std::ffi::OsString::from("--install")]);
        assert_eq!(spec.stdin, runtime::spawn::StdioMode::Null);
    }

    #[test]
    fn a_decompilers_download_brings_rea_when_it_is_missing() {
        let dir = scratch("rows");
        let server = linux_server(&dir.0);
        assert_eq!(
            rows_of_download(&server, Component::Kuna),
            [Component::Rea, Component::Kuna]
        );
        assert_eq!(rows_of_download(&server, Component::Rea), [Component::Rea]);
        put_rea(&dir.0);
        assert_eq!(
            rows_of_download(&server, Component::Ghidra),
            [Component::Ghidra]
        );
    }
}
