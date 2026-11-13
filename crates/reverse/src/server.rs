//! The built-in tool server `reverse` (item5-spec.md WP9g `server.rs`, R10 to R13, R19,
//! R21): always the 37 allow-listed tools, `describe` through [`crate::describe`], and
//! `call` holding rea's lock while it checks the chat's folders, binding and result ids
//! again immediately before forwarding.
//!
//! One `Arc<ReverseServer>` per app: `toolset::build_host` (WP9g) builds it, registers it,
//! sets [`handle`] and manages [`ReverseHandle`] for the Settings commands (WP9d) and the
//! Tools panel (WP9h).
//!
//! **A call**, while holding rea's lock ([`crate::ReaGuard`]), in this order:
//! 1. a name outside [`allow::model_list`], or an action `describe` did not make, is refused
//!    before anything else (before the lock, even);
//! 2. if this chat's open file no longer passes the chat's sandbox (`ctx.chat`, read again
//!    for the call), rea is told `close_binary` and `reset_investigation` (no save path),
//!    the binding and the epoch's result ids are dropped, and a session call is refused;
//! 3. the call is described again, now, with the current session and what this computer
//!    offers, and must match what was approved: the same canonical arguments, the same
//!    file identity (or a folder walked again), the same decompiler, binding and result
//!    id. A session call also checks its file's identity (or walks its `.app` folder);
//! 4. rea is started if needed (a new start grows the store epoch and runs the live check
//!    of rea's tool list against the installed version's fixture);
//! 5. a session call whose file rea lost (an idle stop, a crash) opens it again first,
//!    without a card (R21); `inspect_analysis_view` needs its id in the current epoch;
//! 6. `open_binary` first clears rea when another chat used it (R12 step 6);
//! 7. the call is forwarded with the canonical arguments and the fixed `detail` only, the
//!    tool's time limit, the turn's Stop and the app's own progress line; the result's ids
//!    are recorded for this chat, the binding is updated, and the result is cleaned
//!    ([`crate::results`]).
//!
//! Housekeeping the app did for a call (`close_binary`, `reset_investigation`, a restart of
//! rea) is named on the first line of that call's result, which the audit row's result
//! summary keeps (the row's preview is written at approval time, before the call).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor as PathFlavor, PathSandbox};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::allow::{self, AllowedTool, ToolClass};
use crate::describe::{self, Availability, DescribeEnv, Payload, messages as m};
use crate::env::{self, Flavor};
use crate::install::Installer;
use crate::layout::ReverseLayout;
use crate::platform::{self, Os, Platform};
use crate::process::{ReaGuard, ReaProcess, RestartReason, RunConfig, STOP_GRACE, Started};
use crate::results::{self, Masker};
use crate::session::{Binding, ClosedReason, Session};
use crate::walk::{self, RealWalkFs, WalkFs};
use crate::{
    AppleTools, Component, ComponentState, Decompiler, FileIdentity, ReverseError, ReverseStatus,
    RunState,
};

/// The server's name in the registry and the Tools panel.
pub const SERVER_NAME: &str = "reverse";

/// Time limit of the app's own `close_binary` and `reset_investigation`.
const HOUSEKEEPING_TIMEOUT: Duration = Duration::from_secs(3 * 60);
/// How often the progress line says how long rea has been at it.
const PROGRESS_EVERY: Duration = Duration::from_secs(60);

/// What the model reads when `call` refuses (WP9g; section 6's messages are in
/// [`crate::describe::messages`]).
pub mod refusals {
    /// The action was not made by this server's `describe` for this tool.
    pub const NOT_DESCRIBED: &str =
        "This rea step was not checked by harnessUI, so it did not run.";
    /// A file or folder changed between the approval and the call.
    pub const FILE_CHANGED: &str = "The file changed after this step was approved, so rea did not read it. Run the step again.";
    /// The decompiler changed between the approval of an open and the call.
    pub const DECOMPILER_CHANGED: &str = "The decompiler was changed in Settings after this step was approved, so rea did not open the file. Run open_binary again.";
    /// The chat's open file changed (another open of the same chat) since the step was
    /// planned.
    pub const BINDING_CHANGED: &str = "The file open in rea for this chat changed after this step was planned. Run the step again.";
    /// The open file was replaced or changed on disk.
    pub const OPEN_FILE_CHANGED: &str = "The file open in rea for this chat was changed or replaced on disk, so rea closed it. Open it again with open_binary.";
    /// rea stopped during the call.
    pub const STOPPED: &str = "rea stopped during this step (it was restarted for a change in Settings, or it ended with a problem). It starts again the next time it is used.";
    /// rea did not start.
    pub const START_FAILED: &str =
        "rea could not start. The user can see what went wrong in Settings → Reverse engineering.";
    /// A file the app or rea runs is gone.
    pub const MISSING: &str = "Part of rea is missing, perhaps removed by antivirus software. The user can choose Repair in Settings → Reverse engineering.";
    /// The system refuses to open a file of the pack (not Windows).
    pub const BLOCKED: &str = "The system blocked part of rea. The user can allow harnessUI in their antivirus, then choose Repair in Settings → Reverse engineering.";
    /// The disk is full.
    pub const DISK_FULL: &str =
        "The disk is full, so rea could not start. The user needs to free some space.";
    /// rea answered with something that is not a result.
    pub const PROTOCOL: &str =
        "rea answered in a way harnessUI does not understand, so this step has no result.";
}

/// Lines the app puts first on a result (they reach the audit row's result summary).
pub mod notes {
    /// rea was cleared before this call.
    pub const CLEARED: &str = "Before this step, harnessUI closed the file open in rea and cleared rea's earlier results (close_binary, reset_investigation).";
    /// rea was restarted before this call because clearing it failed.
    pub const RESTARTED: &str =
        "Before this step, harnessUI restarted rea to clear the file and results it held.";
}

/// The `reverse` server.
pub struct ReverseServer {
    layout: ReverseLayout,
    platform: Platform,
    installer: Arc<Installer>,
    process: Arc<ReaProcess>,
    decompiler: Mutex<Option<Decompiler>>,
    session: Mutex<Session>,
    live: Mutex<Live>,
    /// The next start is the first since the app started or an install changed (a longer
    /// start limit, R9/C16).
    first_start: AtomicBool,
}

/// What the live check of the running rea found.
#[derive(Clone, Debug)]
struct Live {
    /// The rea version it checked: an update or a removal makes it stale (the next start
    /// checks again).
    version: Option<String>,
    /// Allow-listed tools whose schema differs in the installed rea (or that it lacks).
    changed: Vec<String>,
    /// Whether `close_binary` and `reset_investigation` are as reviewed: without them no
    /// file may be opened (rea could not be cleared between chats).
    housekeeping_ok: bool,
}

impl Default for Live {
    fn default() -> Self {
        Self {
            version: None,
            changed: Vec::new(),
            housekeeping_ok: true,
        }
    }
}

/// Tauri managed state for the Settings commands.
pub struct ReverseHandle(pub Arc<ReverseServer>);

static HANDLE: OnceLock<Arc<ReverseServer>> = OnceLock::new();

/// The app's server, once `toolset::build_host` has set it.
pub fn handle() -> Option<Arc<ReverseServer>> {
    HANDLE.get().cloned()
}

/// Sets the app's server; only the first call counts (returns whether it did).
pub fn set_handle(server: Arc<ReverseServer>) -> bool {
    HANDLE.set(server).is_ok()
}

/// Why a call did not reach rea (or rea's answer could not be used): the text the model
/// reads.
type Refusal = String;

impl ReverseServer {
    /// The server for these folders on this platform (nothing starts).
    pub fn new(layout: ReverseLayout, platform: Platform) -> Arc<Self> {
        Self::build(Installer::new(layout.clone(), platform))
    }

    /// A server over an installer built elsewhere (tests: a local lock and fetcher). rea's
    /// process checks installs with the same installer.
    pub fn with_installer(installer: Installer) -> Arc<Self> {
        Self::build(installer)
    }

    fn build(installer: Installer) -> Arc<Self> {
        let layout = installer.layout().clone();
        let platform = installer.platform();
        let installer = Arc::new(installer);
        let process = Arc::new(ReaProcess::new(layout.clone(), platform));
        process.set_installer(installer.clone());
        Arc::new(Self {
            installer,
            process,
            layout,
            platform,
            decompiler: Mutex::new(None),
            session: Mutex::new(Session::new()),
            live: Mutex::new(Live::default()),
            first_start: AtomicBool::new(true),
        })
    }

    /// Downloads, removal and status of the rows (WP9d's commands).
    pub fn installer(&self) -> &Arc<Installer> {
        &self.installer
    }

    /// The rea process.
    pub fn process(&self) -> &Arc<ReaProcess> {
        &self.process
    }

    /// The folders.
    pub fn layout(&self) -> &ReverseLayout {
        &self.layout
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        self.session.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn live(&self) -> Live {
        self.live.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// The last live check, while it is about the rea installed now: after an update (or a
    /// removal) the marks of the old version no longer apply, and the next start checks
    /// the new one.
    fn live_now(&self) -> Live {
        let live = self.live();
        if live.changed.is_empty() && live.housekeeping_ok {
            return live;
        }
        let installed = self
            .installer
            .usable(Component::Rea)
            .ok()
            .map(|r| r.version);
        if installed.is_some() && installed == live.version {
            live
        } else {
            Live::default()
        }
    }

    /// Re-reads the install state (after installs, removals, a switch, Apple's tools): the
    /// next start of rea gets the longer limit of a first start, and a chat's open file
    /// whose decompiler is no longer the one rea uses is closed ("closed after a change in
    /// Settings").
    pub fn refresh(&self) {
        self.first_start.store(true, Ordering::SeqCst);
        let current = self.decompiler();
        let mut session = self.session();
        if session
            .binding()
            .is_some_and(|b| Some(b.view.decompiler) != current)
        {
            session.settings_changed();
        }
    }

    /// The decompiler the owner chose (`reverseDecompiler`), whether installed or not.
    pub fn chosen_decompiler(&self) -> Option<Decompiler> {
        *self.decompiler.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets the owner's choice (WP9d reads the setting at start and on each switch). A
    /// different choice ends the chat's open file ("closed after a change in Settings").
    pub fn set_decompiler(&self, decompiler: Option<Decompiler>) {
        let old = std::mem::replace(
            &mut *self.decompiler.lock().unwrap_or_else(|e| e.into_inner()),
            decompiler,
        );
        if old != decompiler {
            self.session().settings_changed();
        }
    }

    /// The decompiler rea uses: the choice, when it is installed and usable.
    pub fn decompiler(&self) -> Option<Decompiler> {
        self.chosen_decompiler()
            .filter(|d| self.installer.usable(d.component()).is_ok())
    }

    /// Stops rea for a Settings change or the app quitting (it starts again on the next
    /// call, except after [`RestartReason::Quit`]). Every chat's open file ends: its chat
    /// reads "closed after a change in Settings".
    pub async fn restart(&self, reason: RestartReason) {
        self.session().settings_changed();
        if reason != RestartReason::Quit {
            self.first_start.store(true, Ordering::SeqCst);
        }
        self.process.restart(reason).await;
    }

    /// What this computer offers right now.
    pub fn availability(&self) -> Availability {
        let rea = self.installer.state(Component::Rea);
        let needs_update = matches!(rea, ComponentState::NeedsUpdate { .. })
            || self.chosen_decompiler().is_some_and(|d| {
                matches!(
                    self.installer.state(d.component()),
                    ComponentState::NeedsUpdate { .. }
                )
            });
        Availability {
            rea_installed: !matches!(
                rea,
                ComponentState::NotInstalled
                    | ComponentState::Partial { .. }
                    | ComponentState::Downloading { .. }
            ),
            decompiler: self.decompiler(),
            apple_tools: platform::apple_tools(self.platform),
            needs_update,
            changed_tools: self.live_now().changed,
        }
    }

    /// Everything the tab shows.
    pub fn status(&self) -> ReverseStatus {
        ReverseStatus {
            components: self.installer.statuses(),
            decompiler: self.decompiler(),
            apple_tools: platform::apple_tools(self.platform),
            running: self.process.state(),
            cache_bytes: self.installer.cache_bytes(),
            memory_gb: platform::memory_gb(),
        }
    }

    /// Why a tool cannot run here yet (the Tools panel's mark), or `None` when it can.
    pub fn unavailable_reason(&self, tool: &str) -> Option<String> {
        let a = self.availability();
        if !a.rea_installed {
            return Some("Not set up yet. Set it up in Settings → Reverse engineering.".into());
        }
        if tool == "open_binary" {
            return None;
        }
        if a.apple_tools == AppleTools::Missing {
            return Some(
                "Needs Apple's command line tools. Install them in Settings → Reverse engineering."
                    .into(),
            );
        }
        if a.needs_update {
            return Some("Needs an update in Settings → Reverse engineering.".into());
        }
        if a.changed_tools.iter().any(|t| t == tool) {
            return Some(
                "Changed in the installed rea. Update rea in Settings → Reverse engineering."
                    .into(),
            );
        }
        let needs_decompiler = allow::class_of(tool) == Some(ToolClass::Session);
        if a.decompiler.is_none() && needs_decompiler {
            return Some(
                "Needs a decompiler. Install one in Settings → Reverse engineering.".into(),
            );
        }
        None
    }

    // ---- The call --------------------------------------------------------------------

    fn path_flavor(&self) -> PathFlavor {
        match self.platform.os() {
            Os::Windows => PathFlavor::Windows,
            _ => PathFlavor::Posix,
        }
    }

    /// The chat's sandbox as `describe` builds it (in Full access the caller widened the
    /// folders to the whole computer; the top of `near`'s drive is added in case it did
    /// not).
    fn sandbox(&self, chat: &ChatContext, near: &str) -> PathSandbox {
        let flavor = self.path_flavor();
        let mut approved = chat.approved_folders.clone();
        if chat.full_access
            && let Some(top) = top_of(near, flavor)
            && !approved.contains(&top)
        {
            approved.push(top);
        }
        let fs = RealWalkFs;
        PathSandbox::new(flavor, &approved, &chat.forbidden_folders, &fs, &|p| {
            fs.case_of(p)
        })
        .0
    }

    /// Whether a canonical path still passes this chat's sandbox now (and exists).
    fn reachable(&self, canonical: &str, chat: &ChatContext) -> bool {
        if walk::is_network_or_device(canonical, self.path_flavor()) {
            return false;
        }
        self.sandbox(chat, canonical)
            .check(canonical, None, &RealWalkFs)
            .is_ok_and(|c| c.exists)
    }

    /// Whether the open file is still the one that was opened: the same identity (and not
    /// a link now), or for a `.app` folder, a walk that passes again.
    fn intact(&self, b: &Binding, chat: &ChatContext) -> bool {
        let canonical = b.view.canonical.as_str();
        if b.folder {
            let sandbox = self.sandbox(chat, canonical);
            return RealWalkFs.kind(canonical) == Some(walk::EntryKind::Dir)
                && walk::walk_folder(&RealWalkFs, canonical, &sandbox, chat.full_access).is_ok();
        }
        RealWalkFs.kind(canonical) == Some(walk::EntryKind::File)
            && FileIdentity::of(std::path::Path::new(canonical))
                .ok()
                .as_ref()
                == Some(&b.view.identity)
    }

    fn run_config(&self, decompiler: Option<Decompiler>) -> RunConfig {
        env::run_config(
            &self.layout,
            &self.installer.installation(),
            decompiler,
            Flavor::of(self.platform),
            self.first_start.load(Ordering::SeqCst),
        )
    }

    /// Starts rea if needed; after a new start, a new store epoch and the live check.
    async fn start(&self, guard: &mut ReaGuard, config: &RunConfig) -> Result<Started, Refusal> {
        let started = guard
            .ensure_started(config.clone())
            .await
            .map_err(|e| self.start_refusal(&e))?;
        if started.fresh {
            self.first_start.store(false, Ordering::SeqCst);
            self.session().rea_started();
            let version = self
                .installer
                .installation()
                .rea
                .map(|r| r.version)
                .unwrap_or_default();
            let check = allow::live_check(&version, &started.tools);
            if !check.changed.is_empty() || !check.missing.is_empty() {
                tracing::warn!(
                    changed = ?check.changed,
                    missing = ?check.missing,
                    "rea's tools differ from the reviewed list; those are switched off"
                );
            }
            *self.live.lock().unwrap_or_else(|e| e.into_inner()) = Live {
                version: Some(version),
                changed: check.switched_off(),
                housekeeping_ok: check.housekeeping_ok(),
            };
        }
        Ok(started)
    }

    /// Tells rea `close_binary` and `reset_investigation` (no save path). When rea does not
    /// confirm the reset, it is stopped and started again instead: it keeps its results in
    /// memory, so a new process holds none. Either way the session is cleared (`reason`
    /// for the chat that held the file, if another one).
    async fn clear(
        &self,
        guard: &mut ReaGuard,
        config: &RunConfig,
        chat: i64,
        reason: ClosedReason,
        cancel: &CancellationToken,
        notes: &mut Vec<String>,
    ) -> Result<(), Refusal> {
        let app = allow::app_list();
        let _ = guard
            .call(
                &app,
                "close_binary",
                Map::new(),
                HOUSEKEEPING_TIMEOUT,
                cancel,
                None,
            )
            .await;
        let reset = if cancel.is_cancelled() {
            None
        } else {
            guard
                .call(
                    &app,
                    "reset_investigation",
                    Map::new(),
                    HOUSEKEEPING_TIMEOUT,
                    cancel,
                    None,
                )
                .await
                .ok()
        };
        if reset.as_ref().is_some_and(|r| !r.is_error) {
            self.session().cleared(reason, chat);
            notes.push(notes::CLEARED.to_string());
            return Ok(());
        }
        // Not confirmed: whatever rea holds goes with its process.
        self.process.stop(STOP_GRACE).await;
        if cancel.is_cancelled() {
            return Err(mcp_host::managed::CANCELLED_TEXT.to_string());
        }
        self.session().cleared(reason, chat);
        self.start(guard, config).await?;
        notes.push(notes::RESTARTED.to_string());
        Ok(())
    }

    /// Forwards one model call with its time limit, the turn's Stop and the app's own
    /// progress line.
    async fn forward(
        &self,
        guard: &mut ReaGuard,
        t: &AllowedTool,
        args: Map<String, Value>,
        ctx: &CallContext,
        line: String,
    ) -> Result<ToolOutput, Refusal> {
        let _ticker = Ticker::start(ctx.progress.clone(), line);
        guard
            .call(
                &allow::model_list(),
                t.name,
                args,
                t.timeout,
                &ctx.cancel,
                None,
            )
            .await
            .map_err(|e| self.call_refusal(&e, t.timeout))
    }

    fn start_refusal(&self, e: &ReverseError) -> Refusal {
        match e {
            ReverseError::AppleToolsMissing => m::APPLE_TOOLS.into(),
            ReverseError::NotInstalled { .. } => m::NOT_SET_UP.into(),
            ReverseError::NeedsUpdate { .. } => m::NEEDS_UPDATE.into(),
            ReverseError::Missing { .. } => refusals::MISSING.into(),
            ReverseError::Blocked { .. } if self.platform.os() == Os::Windows => {
                results::failures::WINDOWS_BLOCKED_LINE.into()
            }
            ReverseError::Blocked { .. } => refusals::BLOCKED.into(),
            ReverseError::Stopped => refusals::STOPPED.into(),
            ReverseError::Cancelled => mcp_host::managed::CANCELLED_TEXT.into(),
            ReverseError::DiskFull { .. } => refusals::DISK_FULL.into(),
            _ => refusals::START_FAILED.into(),
        }
    }

    fn call_refusal(&self, e: &ReverseError, limit: Duration) -> Refusal {
        match e {
            ReverseError::Cancelled => mcp_host::managed::CANCELLED_TEXT.into(),
            ReverseError::Timeout => format!(
                "rea did not finish within {} minutes, so this step was stopped. Try a smaller step.",
                limit.as_secs().div_ceil(60)
            ),
            ReverseError::Stopped => refusals::STOPPED.into(),
            ReverseError::NotAllowed { tool } => {
                format!(
                    "there is no tool named {}",
                    policy::sanitize::one_line(tool, 80)
                )
            }
            ReverseError::Protocol { .. } => refusals::PROTOCOL.into(),
            other => self.start_refusal(other),
        }
    }

    /// The call, holding rea's lock from the first check to the last change of the
    /// session. Returns rea's raw result (cleaned by the caller) or the refusal.
    async fn run(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: &CallContext,
        notes: &mut Vec<String>,
    ) -> Result<ToolOutput, Refusal> {
        let chat = ctx.chat.chat_id;
        // 1. The allow-list, and an action this server made for this tool.
        let Some(t) = allow::get(tool).filter(|_| allow::model_list().contains(tool)) else {
            return Err(format!(
                "there is no tool named {}",
                policy::sanitize::one_line(tool, 80)
            ));
        };
        let Some(payload) = Payload::of(action, tool) else {
            return Err(refusals::NOT_DESCRIBED.into());
        };
        let mut guard = self.process.lock().await;
        guard.set_chat(chat);
        if ctx.cancel.is_cancelled() {
            return Err(mcp_host::managed::CANCELLED_TEXT.into());
        }
        let availability = self.availability();
        let config = self.run_config(availability.decompiler);

        // 2. This chat's open file must still be in its folders.
        let held = self.session().binding_of(chat).cloned();
        if let Some(b) = held
            && !self.reachable(&b.view.canonical, &ctx.chat)
        {
            self.drop_binding(&mut guard, &b, &config, chat, &ctx.cancel, notes)
                .await?;
            if t.class == ToolClass::Session {
                return Err(m::NO_LONGER_REACHABLE.into());
            }
        }

        // 3. Described again, now: it must be what was approved.
        let snapshot = self.session().snapshot_for(chat);
        let now = describe::describe_on(
            self.platform,
            tool,
            args,
            &ctx.chat,
            &DescribeEnv {
                fs: &RealWalkFs,
                session: &snapshot,
                availability: &availability,
            },
        );
        let fresh = match now {
            Described::Invalid(why) => return Err(why),
            Described::Action(a) => match (&a.hard_limit, Payload::of(&a, tool)) {
                (Some(why), _) => return Err(why.clone()),
                (None, Some(p)) => p,
                (None, None) => return Err(refusals::NOT_DESCRIBED.into()),
            },
        };
        match t.class {
            ToolClass::Open | ToolClass::FileCheck => {
                if t.class == ToolClass::Open && fresh.decompiler != payload.decompiler {
                    return Err(refusals::DECOMPILER_CHANGED.into());
                }
                if fresh.args != payload.args || fresh.target != payload.target {
                    return Err(refusals::FILE_CHANGED.into());
                }
            }
            ToolClass::Session => {
                if fresh.binding != payload.binding || fresh.args != payload.args {
                    return Err(refusals::BINDING_CHANGED.into());
                }
                let b = self
                    .session()
                    .binding_of(chat)
                    .cloned()
                    .ok_or_else(|| m::NO_FILE_OPEN.to_string())?;
                if t.name == "close_binary" {
                    // Closing reads nothing, so a changed file does not matter; a rea that
                    // no longer holds the file (an idle stop, a crash) is neither started
                    // nor made to open it again just to close it.
                    if !self.holds(&b) {
                        self.session().closed_by_model();
                        return Ok(ToolOutput::text(closed_line(&b.view.canonical, self.path_flavor())));
                    }
                } else if !self.intact(&b, &ctx.chat) {
                    self.drop_binding(&mut guard, &b, &config, chat, &ctx.cancel, notes)
                        .await?;
                    return Err(refusals::OPEN_FILE_CHANGED.into());
                }
            }
            ToolClass::View => {
                if fresh.evidence != payload.evidence || fresh.args != payload.args {
                    return Err(m::UNKNOWN_ID.into());
                }
            }
        }

        // 4. rea, started if needed.
        let started = self.start(&mut guard, &config).await?;
        let live = self.live();
        if live.changed.iter().any(|c| c == tool) {
            return Err(m::CHANGED_TOOL.into());
        }

        match t.class {
            ToolClass::Open => {
                if !live.housekeeping_ok {
                    return Err(m::CHANGED_TOOL.into());
                }
                let target = payload
                    .target
                    .clone()
                    .ok_or_else(|| refusals::NOT_DESCRIBED.to_string())?;
                let decompiler = payload
                    .decompiler
                    .ok_or_else(|| describe::no_decompiler_message(self.platform).to_string())?;
                // 6. Clear rea when another chat used it (a rea started for this call
                // holds nothing yet).
                let must_clear = !started.fresh
                    && self
                        .session()
                        .must_clear_before_open(chat, started.generation);
                if must_clear {
                    self.clear(
                        &mut guard,
                        &config,
                        chat,
                        ClosedReason::OtherChat,
                        &ctx.cancel,
                        notes,
                    )
                    .await?;
                }
                let name = shown_name(&target.canonical, self.path_flavor());
                let line = format!("rea is reading {name} with {}…", decompiler.label());
                let result = self
                    .forward(&mut guard, t, payload.args.clone(), ctx, line)
                    .await;
                let generation = guard.generation();
                let mut session = self.session();
                match &result {
                    Ok(out) if !out.is_error => {
                        session.bind(
                            chat,
                            target.canonical.clone(),
                            target.identity.clone(),
                            decompiler,
                            generation,
                            payload.args.clone(),
                        );
                        session.record(chat, &target.canonical, &results::evidence_ids(out));
                    }
                    _ => session.open_failed(chat, generation),
                }
                result
            }
            ToolClass::FileCheck => {
                let target = payload
                    .target
                    .clone()
                    .ok_or_else(|| refusals::NOT_DESCRIBED.to_string())?;
                let name = shown_name(&target.canonical, self.path_flavor());
                let result = self
                    .forward(
                        &mut guard,
                        t,
                        payload.args.clone(),
                        ctx,
                        format!("rea is checking {name}…"),
                    )
                    .await;
                if let Ok(out) = &result
                    && !out.is_error
                {
                    self.session()
                        .record(chat, &target.canonical, &results::evidence_ids(out));
                }
                result
            }
            ToolClass::Session => {
                // 5. rea lost the file (an idle stop, a crash): open it again first.
                let mut b = self
                    .session()
                    .binding_of(chat)
                    .cloned()
                    .ok_or_else(|| m::NO_FILE_OPEN.to_string())?;
                if b.view.generation != guard.generation() {
                    match self
                        .reopen(&mut guard, b, &config, started.fresh, ctx, notes)
                        .await?
                    {
                        Reopened::Bound(again) => b = again,
                        // rea's own error, cleaned by the caller, after the app's line.
                        Reopened::Failed(out) => return Ok(out),
                    }
                }
                let name = shown_name(&b.view.canonical, self.path_flavor());
                let line = format!("rea is reading {name} with {}…", b.view.decompiler.label());
                let result = self
                    .forward(&mut guard, t, payload.args.clone(), ctx, line)
                    .await;
                let mut session = self.session();
                if t.name == "close_binary" {
                    session.closed_by_model();
                } else if let Ok(out) = &result
                    && !out.is_error
                {
                    session.record(chat, &b.view.canonical, &results::evidence_ids(out));
                }
                result
            }
            ToolClass::View => {
                let evidence = payload
                    .evidence
                    .clone()
                    .ok_or_else(|| m::UNKNOWN_ID.to_string())?;
                // A start of rea since the description took the result with it.
                let current = self
                    .session()
                    .evidence(&evidence.id)
                    .is_some_and(|e| e.chat_id == chat && e.canonical == evidence.canonical);
                if !current {
                    return Err(m::UNKNOWN_ID.into());
                }
                let name = shown_name(&evidence.canonical, self.path_flavor());
                let line = match availability.decompiler {
                    Some(d) => format!("rea is reading {name} with {}…", d.label()),
                    None => format!("rea is checking {name}…"),
                };
                let result = self
                    .forward(&mut guard, t, payload.args.clone(), ctx, line)
                    .await;
                if let Ok(out) = &result
                    && !out.is_error
                {
                    self.session()
                        .record(chat, &evidence.canonical, &results::evidence_ids(out));
                }
                result
            }
        }
    }

    /// Whether the running rea still holds the binding's file: the same process (no start
    /// since the open) and running now.
    fn holds(&self, b: &Binding) -> bool {
        self.process.generation() == b.view.generation
            && matches!(
                self.process.state(),
                RunState::Ready | RunState::Busy { .. }
            )
    }

    /// Ends this chat's binding whose file left the chat's folders or changed: when rea
    /// still holds it (running, same generation), `close_binary` and
    /// `reset_investigation` first.
    async fn drop_binding(
        &self,
        guard: &mut ReaGuard,
        b: &Binding,
        config: &RunConfig,
        chat: i64,
        cancel: &CancellationToken,
        notes: &mut Vec<String>,
    ) -> Result<(), Refusal> {
        if self.holds(b) {
            // The chat itself holds the file: no other chat is told anything.
            self.clear(guard, config, chat, ClosedReason::OtherChat, cancel, notes)
                .await
        } else {
            self.session().dropped();
            Ok(())
        }
    }

    /// Opens the chat's file again in a rea that lost it (R21), after the same checks as
    /// any session call (done by the caller); clears rea first if another chat's results
    /// are in it.
    async fn reopen(
        &self,
        guard: &mut ReaGuard,
        b: Binding,
        config: &RunConfig,
        fresh: bool,
        ctx: &CallContext,
        notes: &mut Vec<String>,
    ) -> Result<Reopened, Refusal> {
        let chat = ctx.chat.chat_id;
        let name = shown_name(&b.view.canonical, self.path_flavor());
        if let Some(progress) = &ctx.progress {
            progress(format!(
                "rea is opening {name} again with {}…\n",
                b.view.decompiler.label()
            ));
        }
        let others = !fresh && {
            let session = self.session();
            session.last_open().is_some_and(|(c, _)| c != chat) || session.others_hold_ids(chat)
        };
        if others {
            self.clear(
                guard,
                config,
                chat,
                ClosedReason::OtherChat,
                &ctx.cancel,
                notes,
            )
            .await?;
        }
        let open = allow::get("open_binary").ok_or_else(|| refusals::NOT_DESCRIBED.to_string())?;
        let line = format!("rea is reading {name} with {}…", b.view.decompiler.label());
        let _ticker = Ticker::start(ctx.progress.clone(), line);
        let result = guard
            .call(
                &allow::app_list(),
                open.name,
                b.open_args.clone(),
                open.timeout,
                &ctx.cancel,
                None,
            )
            .await;
        let generation = guard.generation();
        let mut session = self.session();
        match result {
            Ok(out) if !out.is_error => {
                let identity = (!b.folder).then(|| b.view.identity.clone());
                session.bind(
                    chat,
                    b.view.canonical.clone(),
                    identity,
                    b.view.decompiler,
                    generation,
                    b.open_args.clone(),
                );
                session.record(chat, &b.view.canonical, &results::evidence_ids(&out));
                session
                    .binding_of(chat)
                    .cloned()
                    .map(Reopened::Bound)
                    .ok_or_else(|| m::NO_FILE_OPEN.to_string())
            }
            Ok(out) => {
                session.open_failed(chat, generation);
                notes.push(format!(
                    "rea could not open {} again, so this step did not run. Open it again with open_binary.",
                    walk::shown(&b.view.canonical)
                ));
                Ok(Reopened::Failed(out))
            }
            Err(e) => {
                session.open_failed(chat, generation);
                Err(self.call_refusal(&e, open.timeout))
            }
        }
    }
}

/// How opening the chat's file again went.
enum Reopened {
    /// Open again: the new binding.
    Bound(Binding),
    /// rea refused: its error result.
    Failed(ToolOutput),
}

/// A file's name as the app's own lines show it: hidden and control characters made
/// visible, on one line, at most 120 characters.
fn shown_name(canonical: &str, flavor: PathFlavor) -> String {
    policy::sanitize::one_line(walk::file_name(canonical, flavor), 120)
}

/// What the model reads after `close_binary` when rea had already let go of the file.
fn closed_line(canonical: &str, flavor: PathFlavor) -> String {
    format!(
        "{} is closed in rea for this chat.",
        shown_name(canonical, flavor)
    )
}

/// The top of the drive holding `path`: `/`, or `C:\`.
fn top_of(path: &str, flavor: PathFlavor) -> Option<String> {
    match flavor {
        PathFlavor::Posix => Some("/".into()),
        PathFlavor::Windows => {
            let b = path.as_bytes();
            (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
                .then(|| format!("{}:\\", (b[0] as char).to_ascii_uppercase()))
        }
    }
}

/// The app's progress line ("rea is reading x with Kuna… 2 min · Stop"), at once and then
/// every minute, until dropped.
struct Ticker(Option<tokio::task::JoinHandle<()>>);

impl Ticker {
    fn start(progress: Option<mcp_host::ProgressFn>, line: String) -> Self {
        Self(progress.map(|p| {
            tokio::spawn(async move {
                let started = tokio::time::Instant::now();
                let mut every = tokio::time::interval(PROGRESS_EVERY);
                loop {
                    every.tick().await;
                    let minutes = started.elapsed().as_secs() / 60;
                    p(format!("{line} {minutes} min · Stop\n"));
                }
            })
        }))
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        if let Some(task) = self.0.take() {
            task.abort();
        }
    }
}

#[async_trait]
impl BuiltinServer for ReverseServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        allow::tool_defs()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let session = self.session().snapshot_for(chat.chat_id);
        let availability = self.availability();
        describe::describe_on(
            self.platform,
            tool,
            args,
            chat,
            &DescribeEnv {
                fs: &RealWalkFs,
                session: &session,
                availability: &availability,
            },
        )
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let mut notes = Vec::new();
        let output = match self.run(tool, args, action, &ctx, &mut notes).await {
            Ok(output) => output,
            Err(refusal) => ToolOutput::error(refusal),
        };
        // Built for each call: the folder's resolved and 8.3 spellings exist only once
        // something is installed in it.
        let masker = Masker::new(self.layout.root(), self.platform);
        results::clean_with(output, &masker, self.platform, &notes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_installed_marks_every_tool_not_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let s = ReverseServer::new(ReverseLayout::new(dir.path()), Platform::LinuxX64);
        for tool in ["open_binary", "procedure_pseudo_code", "inspect_pe_imports"] {
            assert_eq!(
                s.unavailable_reason(tool).as_deref(),
                Some("Not set up yet. Set it up in Settings → Reverse engineering.")
            );
        }
        let status = s.status();
        assert_eq!(status.components.len(), 3);
        assert!(
            status
                .components
                .iter()
                .all(|c| c.state == ComponentState::NotInstalled)
        );
        assert_eq!(s.decompiler(), None);
        assert_eq!(s.name(), "reverse");
    }

    #[test]
    fn the_top_of_a_drive() {
        assert_eq!(top_of("/a/b", PathFlavor::Posix).as_deref(), Some("/"));
        assert_eq!(
            top_of(r"d:\x\y", PathFlavor::Windows).as_deref(),
            Some(r"D:\")
        );
        assert_eq!(top_of(r"\\server\share", PathFlavor::Windows), None);
    }
}
