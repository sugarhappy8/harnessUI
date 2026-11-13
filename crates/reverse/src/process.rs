//! The one rea process of the app (item5-spec.md WP9b `process.rs`, R7, R9, R12, R15): every
//! call holds its lock ([`ReaGuard`]), so calls run one at a time and the checks of WP9g run
//! while nothing else can reach rea. Started on first use, stopped after [`IDLE_STOP`]
//! without a call, on [`ReaProcess::restart`] and when the app quits.
//!
//! - **Before a start** ([`ReaGuard::ensure_started`]), checked fresh each time: a Mac
//!   without Apple's command line tools never starts rea (R7: its `xcrun` would open Apple's
//!   install window); rea must be installed and usable (marker present, every program
//!   present with its size, not blocked, at or above its minimum version and one of the
//!   accepted rea versions), and so must the decompiler the configuration names; the program
//!   must be the pack's `node` and the arguments exactly `[<rea>/scripts/rea.mjs, "mcp"]`,
//!   both inside the reverse folder (R9). Then `reverse/tmp` is emptied, the folders the
//!   environment names are created and rea is started through [`mcp_host::ManagedServer`].
//!   On Linux with Ghidra, rea's temporary folder outside the reverse folder
//!   ([`ReverseLayout::run_tmp`]) is made afresh first, open to this user only (mode 0700),
//!   and checked to be a real folder of this user's.
//! - **Generation:** grows at every start, so WP9g can tell that rea lost the chat's open
//!   file (R21). A running rea whose configuration differs from the one asked for (another
//!   decompiler) is stopped and started again, never reused.
//! - **Stopping:** [`ReaProcess::stop`] does not wait for a call that runs: rea's standard
//!   input is closed at once, rea gets the grace to end Kuna or Java itself, then its tree is
//!   killed, and the call ends as [`ReverseError::Stopped`]. A start that finishes after a
//!   stop is undone. After [`RestartReason::Quit`] nothing starts again. After a stop (and
//!   the idle stop), the temporary folder outside the reverse folder is removed unless rea
//!   was started again meanwhile; it is also removed when the app starts
//!   ([`ReaProcess::new`]), in case the app ended without a stop.
//! - **A caller that stops waiting** (the agent loop drops a cancelled tool's future 250 ms
//!   after Stop): the call runs in a task of its own, so the managed server's cancel, its
//!   5 s grace and the stop after it still run to their end, and that call still counts as
//!   holding rea: [`ReaProcess::lock`] and the idle stop wait for it after taking the lock,
//!   and the state stays `Busy` until rea has answered or been stopped. A caller that goes
//!   away without cancelling cancels the call. A start whose caller stops waiting is no
//!   longer `Starting`: the half-started rea is stopped the same way, and holds rea until
//!   it has stopped.
//! - **State** for the tab: `Failed` carries the stderr tail with the reverse folder masked
//!   ([`crate::results::mask`]) and invisible characters made visible.
//!
//! Known limit: on macOS and Linux rea starts Kuna and Java in process groups of their own.
//! If the app itself crashes, those can outlive it until they notice the closed pipe. Stop,
//! the timeout, the idle stop and quitting all give rea time to end them first. When rea is
//! still running after that time its tree is killed, and on Linux so are the groups its own
//! children lead ([`mcp_host::ManagedServer`]); on macOS Kuna or Java can then outlive rea
//! the same way. On Windows the app's Job Object ends them in every case.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use mcp_host::{
    CallOutcome, HostError, Lifecycle, ManagedServer, ManagedSpec, ProgressFn, ToolAllowList,
    ToolOutput,
};
use serde_json::{Map, Value};
use tokio::sync::{Mutex, OwnedMutexGuard, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::install::Installer;
use crate::layout::ReverseLayout;
use crate::platform::{self, Platform};
use crate::{AppleTools, Component, ReverseError, RunState};

/// Stopped after this long without a call.
pub const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
/// Grace given to rea to end Kuna or Java itself before its tree is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(3);
/// rea's stderr log in the app's log folder ([`ReaProcess::set_log_dir`]); no user server's
/// `mcp-server-<name>.log` can be this file.
pub const LOG_FILE_NAME: &str = "mcp-reverse.log";

/// The managed server's name in its log.
const SERVER_NAME: &str = "reverse";
/// Lines of the stderr tail kept for "Show details".
const TAIL_LINES: usize = 20;
/// Longest "Show details" text.
const MAX_DETAILS_CHARS: usize = 8000;

/// How to start rea (built by [`crate::env::run_config`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// The pack's `node`.
    pub program: PathBuf,
    /// `[<rea>/scripts/rea.mjs, "mcp"]`.
    pub args: Vec<String>,
    /// The whole environment.
    pub env: runtime::EnvSpec,
    /// `reverse/work`.
    pub cwd: PathBuf,
    /// 180 s on the first start after an install or repair, 60 s after.
    pub start_timeout: Duration,
}

impl RunConfig {
    /// The same configuration, ignoring the start limit.
    fn same_as(&self, other: &RunConfig) -> bool {
        self.program == other.program
            && self.args == other.args
            && self.env == other.env
            && self.cwd == other.cwd
    }

    fn var(&self, name: &str) -> Option<&str> {
        self.env
            .set
            .iter()
            .rev()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Why rea is restarted (each ends every chat's open file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartReason {
    /// The owner chose the other decompiler.
    DecompilerChanged,
    /// rea or a decompiler was updated or repaired.
    Updated,
    /// rea or a decompiler was removed.
    Removed,
    /// The app quits.
    Quit,
}

/// What [`ReaGuard::ensure_started`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    /// Grows at each start of rea.
    pub generation: u64,
    /// rea's `tools/list`: name and input schema, for the live check (WP9c).
    pub tools: Vec<(String, Value)>,
    /// True when this call started rea.
    pub fresh: bool,
}

/// One rea process as seen from here: a [`ManagedServer`], or a test double.
#[async_trait]
pub(crate) trait Rea: Send + Sync {
    async fn start(&self) -> Result<Vec<(String, Value)>, HostError>;
    async fn call(
        &self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<CallOutcome, HostError>;
    async fn stop(&self, grace: Duration);
    /// Waits until every process this rea started has ended, also a half-started one whose
    /// start was dropped: the managed server kills that one at once, without waiting.
    async fn ended(&self);
    fn is_running(&self) -> bool;
    fn ended_unexpectedly(&self) -> bool;
    fn stderr_tail(&self) -> Vec<String>;
    fn lifecycle(&self) -> Option<Lifecycle>;
    /// The process id while it runs (tests only look at it).
    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        None
    }
}

#[async_trait]
impl Rea for ManagedServer {
    async fn start(&self) -> Result<Vec<(String, Value)>, HostError> {
        ManagedServer::start(self).await
    }
    async fn call(
        &self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<CallOutcome, HostError> {
        self.call_outcome(allow, name, args, timeout, cancel, progress)
            .await
    }
    async fn stop(&self, grace: Duration) {
        ManagedServer::stop(self, grace).await;
    }
    async fn ended(&self) {}
    fn is_running(&self) -> bool {
        ManagedServer::is_running(self)
    }
    fn ended_unexpectedly(&self) -> bool {
        ManagedServer::ended_unexpectedly(self)
    }
    fn stderr_tail(&self) -> Vec<String> {
        ManagedServer::stderr_tail(self)
    }
    fn lifecycle(&self) -> Option<Lifecycle> {
        ManagedServer::lifecycle(self)
    }
    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        ManagedServer::pid(self)
    }
}

/// Makes the server for one start.
pub(crate) type Launch = Arc<dyn Fn(ManagedSpec, Option<Lifecycle>) -> Arc<dyn Rea> + Send + Sync>;

fn launch_managed() -> Launch {
    Arc::new(|spec, lifecycle| -> Arc<dyn Rea> {
        Arc::new(ManagedServer::new(spec).with_lifecycle(lifecycle))
    })
}

/// The rea that runs now.
struct Running {
    server: Arc<dyn Rea>,
    config: RunConfig,
    generation: u64,
    tools: Vec<(String, Value)>,
}

#[derive(Default)]
struct Status {
    running: Option<Running>,
    /// The server being started, so a stop can cut the start short.
    starting: Option<Arc<dyn Rea>>,
    /// The call that runs (its number) and its chat (0 when the caller did not say).
    busy: Option<(u64, i64)>,
    /// Calls made so far, to number them.
    calls_made: u64,
    /// Why the last start failed or rea crashed ("Show details").
    failed: Option<String>,
    last_used: Option<Instant>,
    idle_task: Option<JoinHandle<()>>,
    quitting: bool,
    /// Grows at every stop: a start that finishes after one is undone.
    stops: u64,
    lifecycle: Option<Lifecycle>,
}

struct Shared {
    layout: ReverseLayout,
    platform: Platform,
    calls: Arc<Mutex<()>>,
    status: StdMutex<Status>,
    /// Calls whose work has not ended, even if their caller went away, and the stop of a
    /// start whose caller went away (at most one).
    in_flight: watch::Sender<u32>,
    generation: AtomicU64,
    installer: StdMutex<Arc<Installer>>,
    log_file: StdMutex<Option<PathBuf>>,
    launch: Launch,
    /// Where Apple's tools are looked for ("/"; tests use a fake tree).
    apple_root: PathBuf,
    /// The starts that prepared rea's temporary folder outside the reverse folder
    /// ([`ReverseLayout::run_tmp`]); held while it is prepared or removed, so a stop removes
    /// it only when no start has prepared it since the stop began.
    run_tmp_starts: StdMutex<u64>,
}

impl Shared {
    fn status(&self) -> std::sync::MutexGuard<'_, Status> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn installer(&self) -> Arc<Installer> {
        self.installer
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn run_tmp_starts(&self) -> u64 {
        *self
            .run_tmp_starts
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    /// Text from rea for the tab: the reverse folder (and rea's temporary folder outside it)
    /// masked, invisible characters made visible, capped.
    fn clean(&self, text: &str) -> String {
        let masked = crate::results::mask_layout(text, &self.layout);
        let visible = policy::sanitize::visible(&masked);
        let count = visible.chars().count();
        if count <= MAX_DETAILS_CHARS {
            return visible;
        }
        // Keep the end: the last lines say what went wrong.
        visible.chars().skip(count - MAX_DETAILS_CHARS).collect()
    }

    fn details(&self, server: &dyn Rea) -> String {
        let tail = server.stderr_tail();
        let start = tail.len().saturating_sub(TAIL_LINES);
        self.clean(&tail[start..].join("\n"))
    }

    /// Waits until no call is in flight (one whose caller went away may still be ending:
    /// rea's grace to answer the cancel, then its stop).
    async fn settled(&self) {
        let mut in_flight = self.in_flight.subscribe();
        let _ = in_flight.wait_for(|n| *n == 0).await;
    }
}

/// One call whose work has not ended; held by the call's own task. When it goes, the call
/// no longer makes rea busy and counts as rea's last use.
struct InFlight {
    shared: Arc<Shared>,
    number: u64,
}

impl InFlight {
    fn begin(shared: Arc<Shared>, number: u64) -> Self {
        shared.in_flight.send_modify(|n| *n += 1);
        Self { shared, number }
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        {
            let mut s = self.shared.status();
            if s.busy.is_some_and(|(number, _)| number == self.number) {
                s.busy = None;
            }
            s.last_used = Some(Instant::now());
        }
        self.shared
            .in_flight
            .send_modify(|n| *n = n.saturating_sub(1));
    }
}

/// Clears [`Status::starting`] on every way out of [`ReaGuard::ensure_started`], also when
/// its caller stops waiting (the agent loop drops a stopped tool's future 250 ms after
/// Stop). Only while `starting` is still this server: then the half-started server is also
/// stopped, in a task of its own that holds rea like a call that is still ending.
struct StartGuard {
    shared: Arc<Shared>,
    server: Arc<dyn Rea>,
}

impl Drop for StartGuard {
    fn drop(&mut self) {
        let ours = {
            let mut s = self.shared.status();
            let ours = s
                .starting
                .as_ref()
                .is_some_and(|starting| Arc::ptr_eq(starting, &self.server));
            if ours {
                s.starting = None;
            }
            ours
        };
        if !ours {
            // Started, failed, or taken by a stop that stops it itself.
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        // Number 0 is no call's, so it never clears another call's busy state.
        let flight = InFlight::begin(self.shared.clone(), 0);
        let server = self.server.clone();
        runtime.spawn(async move {
            let _flight = flight;
            server.stop(STOP_GRACE).await;
        });
    }
}

/// The one rea process.
pub struct ReaProcess {
    shared: Arc<Shared>,
}

/// Exclusive use of rea for one call (and the app's own housekeeping calls with it).
pub struct ReaGuard {
    generation: u64,
    chat: Option<i64>,
    shared: Arc<Shared>,
    _calls: OwnedMutexGuard<()>,
}

impl ReaProcess {
    /// Nothing runs until the first call. The app makes its one rea process when it starts:
    /// a temporary folder outside the reverse folder left by an app that ended without a stop
    /// is removed here.
    pub fn new(layout: ReverseLayout, platform: Platform) -> Self {
        Self::build(layout, platform, launch_managed(), PathBuf::from("/"))
    }

    fn build(
        layout: ReverseLayout,
        platform: Platform,
        launch: Launch,
        apple_root: PathBuf,
    ) -> Self {
        if let Some(run_tmp) = layout.run_tmp() {
            remove_private_dir(run_tmp);
        }
        Self {
            shared: Arc::new(Shared {
                installer: StdMutex::new(Arc::new(Installer::new(layout.clone(), platform))),
                layout,
                platform,
                calls: Arc::new(Mutex::new(())),
                status: StdMutex::new(Status::default()),
                in_flight: watch::Sender::new(0),
                generation: AtomicU64::new(0),
                log_file: StdMutex::new(None),
                launch,
                apple_root,
                run_tmp_starts: StdMutex::new(0),
            }),
        }
    }

    /// Checks installs with this installer instead of the app's own (a server built over a
    /// test lock: `ReverseServer::with_installer` passes its installer here).
    pub fn set_installer(&self, installer: Arc<Installer>) {
        *self
            .shared
            .installer
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = installer;
    }

    /// Writes rea's stderr to [`LOG_FILE_NAME`] in `dir` (the app's log folder) from the
    /// next start on; `None` keeps it in memory only.
    pub fn set_log_dir(&self, dir: Option<&Path>) {
        *self
            .shared
            .log_file
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = dir.map(|d| d.join(LOG_FILE_NAME));
    }

    /// The folders rea runs in.
    pub fn layout(&self) -> &ReverseLayout {
        &self.shared.layout
    }

    /// The platform.
    pub fn platform(&self) -> Platform {
        self.shared.platform
    }

    /// Waits for rea to be free and holds it. Free means no guard holds it and no call is
    /// still ending (one whose caller stopped waiting: rea's grace, then its stop).
    pub async fn lock(&self) -> ReaGuard {
        let calls = self.shared.calls.clone().lock_owned().await;
        self.shared.settled().await;
        ReaGuard {
            generation: self.generation(),
            chat: None,
            shared: self.shared.clone(),
            _calls: calls,
        }
    }

    /// rea's process id while it runs.
    #[cfg(test)]
    fn pid(&self) -> Option<u32> {
        let s = self.shared.status();
        s.running.as_ref().and_then(|r| r.server.pid())
    }

    /// Grows at each start of rea.
    pub fn generation(&self) -> u64 {
        self.shared.generation.load(Ordering::SeqCst)
    }

    /// Whether rea is starting, a call holds it, or a stopped call is still ending (WP9d
    /// answers `busy` then, unless forced).
    pub fn is_busy(&self) -> bool {
        self.shared.status().starting.is_some()
            || self.shared.calls.try_lock().is_err()
            || *self.shared.in_flight.borrow() > 0
    }

    /// For the tab's status line.
    pub fn state(&self) -> RunState {
        let s = self.shared.status();
        if s.starting.is_some() {
            return RunState::Starting;
        }
        if let Some(running) = &s.running {
            if running.server.is_running() {
                return match s.busy {
                    Some((_, chat_id)) => RunState::Busy { chat_id },
                    None => RunState::Ready,
                };
            }
            if running.server.ended_unexpectedly() {
                return RunState::Failed {
                    reason: self.shared.details(running.server.as_ref()),
                };
            }
            // Stopped by the managed server itself after a call it did not end.
            return RunState::Stopped;
        }
        match &s.failed {
            Some(reason) => RunState::Failed {
                reason: reason.clone(),
            },
            None => RunState::Stopped,
        }
    }

    /// Stops rea (it starts again on the next call, except after [`RestartReason::Quit`]).
    pub async fn restart(&self, reason: RestartReason) {
        if reason == RestartReason::Quit {
            self.shared.status().quitting = true;
        }
        self.stop(STOP_GRACE).await;
    }

    /// Closes rea's standard input, waits up to `grace`, then ends its tree. A call that
    /// runs ends as [`ReverseError::Stopped`]; a start that runs is cut short.
    pub async fn stop(&self, grace: Duration) {
        stop_shared(&self.shared, grace).await;
    }
}

async fn stop_shared(shared: &Arc<Shared>, grace: Duration) {
    let starts = shared.run_tmp_starts();
    let (running, starting) = {
        let mut s = shared.status();
        s.stops += 1;
        s.failed = None;
        if let Some(task) = s.idle_task.take() {
            task.abort();
        }
        // Taken: the start that runs ends as stopped, and the tab no longer says Starting.
        (s.running.take(), s.starting.take())
    };
    if let Some(server) = starting {
        server.stop(grace).await;
    }
    if let Some(running) = running {
        running.server.stop(grace).await;
    }
    remove_run_tmp_after_stop(shared, starts).await;
}

/// After a stop: removes rea's temporary folder outside the reverse folder
/// ([`ReverseLayout::run_tmp`]), unless rea runs or is starting, or a start has prepared it
/// since `starts`. Best effort: the next start empties it, and so does the next app start.
async fn remove_run_tmp_after_stop(shared: &Arc<Shared>, starts: u64) {
    let Some(run_tmp) = shared.layout.run_tmp().map(Path::to_path_buf) else {
        return;
    };
    let shared = shared.clone();
    let _ = tokio::task::spawn_blocking(move || {
        let now = shared
            .run_tmp_starts
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let idle = {
            let s = shared.status();
            s.running.is_none() && s.starting.is_none()
        };
        if *now == starts && idle {
            remove_private_dir(&run_tmp);
        }
    })
    .await;
}

impl ReaGuard {
    /// Says which chat the call belongs to, for the tab's "rea is working on a file…" state.
    pub fn set_chat(&mut self, chat_id: i64) {
        self.chat = Some(chat_id);
    }

    /// The generation this guard last saw (at the lock, or at its start of rea).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Starts rea with `config` unless it runs already with the same configuration. See the
    /// module documentation for what is checked first.
    pub async fn ensure_started(&mut self, config: RunConfig) -> Result<Started, ReverseError> {
        let shared = self.shared.clone();
        if shared.status().quitting {
            return Err(ReverseError::Stopped);
        }
        {
            let s = shared.status();
            if let Some(running) = &s.running
                && running.server.is_running()
                && running.config.same_as(&config)
            {
                self.generation = running.generation;
                return Ok(Started {
                    generation: running.generation,
                    tools: running.tools.clone(),
                    fresh: false,
                });
            }
        }
        check_before_start(&shared, &config)?;
        let old = {
            let mut s = shared.status();
            if let Some(task) = s.idle_task.take() {
                task.abort();
            }
            let old = s.running.take();
            if let Some(old) = &old
                && old.server.ended_unexpectedly()
            {
                s.failed = Some(shared.details(old.server.as_ref()));
            }
            old
        };
        if let Some(old) = old
            && old.server.is_running()
        {
            // Another configuration (a decompiler switch that skipped restart()).
            old.server.stop(STOP_GRACE).await;
        }
        let folders = crate::env::folders(&config, &shared.layout);
        let tmp = shared.layout.tmp();
        // rea's temporary folder outside the reverse folder, when this configuration uses it.
        let run_tmp = shared
            .layout
            .run_tmp()
            .filter(|r| config.var("TMPDIR").map(Path::new) == Some(*r))
            .map(Path::to_path_buf);
        let prepared = tokio::task::spawn_blocking({
            let shared = shared.clone();
            move || {
                let mut starts = shared
                    .run_tmp_starts
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                *starts += 1;
                prepare_folders(&tmp, run_tmp.as_deref(), &folders)
            }
        })
        .await;
        match prepared {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(ReverseError::from(e)),
            Err(e) => {
                return Err(ReverseError::Io {
                    message: e.to_string(),
                });
            }
        }
        let spec = ManagedSpec {
            name: SERVER_NAME.into(),
            program: config.program.clone(),
            args: config.args.clone(),
            env: config.env.clone(),
            cwd: config.cwd.clone(),
            log_file: shared
                .log_file
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            start_timeout: config.start_timeout,
        };
        let (server, stops) = {
            let mut s = shared.status();
            let server = (shared.launch)(spec, s.lifecycle);
            s.starting = Some(server.clone());
            (server, s.stops)
        };
        let _starting = StartGuard {
            shared: shared.clone(),
            server: server.clone(),
        };
        let result = server.start().await;
        // Decided under the lock, which is released before anything is awaited.
        let after = {
            let mut s = shared.status();
            s.starting = None;
            match result {
                Ok(tools) if s.stops == stops && !s.quitting => {
                    let generation = shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
                    s.lifecycle = server.lifecycle().or(s.lifecycle);
                    s.failed = None;
                    s.last_used = Some(Instant::now());
                    s.running = Some(Running {
                        server: server.clone(),
                        config,
                        generation,
                        tools: tools.clone(),
                    });
                    s.idle_task = Some(spawn_idle_stop(Arc::downgrade(&shared), generation));
                    Ok(Started {
                        generation,
                        tools,
                        fresh: true,
                    })
                }
                // Stopped (or the app quit) while it started: undone below.
                Ok(_) => Err(None),
                Err(_) if s.stops != stops => Err(Some(ReverseError::Stopped)),
                Err(error) => {
                    let mut message = shared.clean(&error.to_string());
                    let details = shared.details(server.as_ref());
                    if !details.is_empty() && !message.contains(&details) {
                        message = format!("{message}\n\n{details}");
                    }
                    s.failed = Some(message.clone());
                    Err(Some(ReverseError::StartFailed { message }))
                }
            }
        };
        match after {
            Ok(started) => {
                self.generation = started.generation;
                Ok(started)
            }
            Err(Some(error)) => Err(error),
            Err(None) => {
                server.stop(STOP_GRACE).await;
                Err(ReverseError::Stopped)
            }
        }
    }

    /// Calls `name`, which must be in `allow` (refused before anything reaches rea). rea's
    /// result comes back as it is (WP9g cleans it); a call stopped by `cancel` is
    /// [`ReverseError::Cancelled`], one past its limit [`ReverseError::Timeout`], and a rea
    /// that is not running or stops during the call [`ReverseError::Stopped`].
    ///
    /// The call runs in a task of its own: when this future is dropped, the call goes on
    /// until rea answers or the managed server has stopped it, and until then it holds rea
    /// for the next [`ReaProcess::lock`]. Dropping this future before the call ends cancels
    /// the call (without cancelling `cancel` itself).
    pub async fn call(
        &mut self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<ToolOutput, ReverseError> {
        if !allow.contains(name) {
            return Err(ReverseError::NotAllowed {
                tool: name.to_string(),
            });
        }
        if cancel.is_cancelled() {
            return Err(ReverseError::Cancelled);
        }
        let (server, number) = {
            let mut s = self.shared.status();
            let server = match &s.running {
                Some(running) if running.server.is_running() => running.server.clone(),
                _ => return Err(ReverseError::Stopped),
            };
            s.calls_made += 1;
            let number = s.calls_made;
            s.busy = Some((number, self.chat.unwrap_or(0)));
            (server, number)
        };
        let token = cancel.child_token();
        let _caller_gone = token.clone().drop_guard();
        let flight = InFlight::begin(self.shared.clone(), number);
        let allow = allow.clone();
        let tool = name.to_owned();
        let task = tokio::spawn(async move {
            let _flight = flight;
            server
                .call(&allow, &tool, args, timeout, &token, progress)
                .await
        });
        let outcome = match task.await {
            Ok(outcome) => outcome,
            Err(_) => Err(HostError::NotReady("the call ended unexpectedly".into())),
        };
        match outcome {
            Ok(CallOutcome::Done(output)) => Ok(output),
            Ok(CallOutcome::Cancelled { .. }) => Err(ReverseError::Cancelled),
            Ok(CallOutcome::TimedOut { .. }) => Err(ReverseError::Timeout),
            Err(HostError::NotFound(_)) => Err(ReverseError::NotAllowed {
                tool: name.to_string(),
            }),
            Err(HostError::NotReady(_)) => Err(ReverseError::Stopped),
            Err(other) => Err(ReverseError::Protocol {
                message: self.shared.clean(&other.to_string()),
            }),
        }
    }
}

impl Drop for ReaGuard {
    fn drop(&mut self) {
        // Holding rea counts as using it.
        self.shared.status().last_used = Some(Instant::now());
    }
}

/// The checks of the module documentation, before anything starts.
fn check_before_start(shared: &Shared, config: &RunConfig) -> Result<(), ReverseError> {
    if platform::apple_tools_at(shared.platform, &shared.apple_root) == AppleTools::Missing {
        return Err(ReverseError::AppleToolsMissing);
    }
    let installer = shared.installer();
    installer.usable(Component::Rea)?;
    match config.var("REA_ANALYSIS_PROVIDER") {
        Some("kuna") => {
            installer.usable(Component::Kuna)?;
        }
        Some("ghidra") if config.var("GHIDRA_INSTALL_DIR").is_some() => {
            installer.usable(Component::Ghidra)?;
        }
        Some("ghidra") => {}
        // Never rea's automatic search (R10).
        _ => {
            return Err(ReverseError::StartFailed {
                message: "rea's decompiler setting is missing".into(),
            });
        }
    }
    if config.program.as_os_str().is_empty() {
        return Err(ReverseError::NotInstalled {
            component: Component::Rea,
        });
    }
    let root = shared.layout.root();
    let in_pack = |p: &Path| p.is_absolute() && p.starts_with(root) && p.is_file();
    let exact = config.args.len() == 2
        && config.args[1] == "mcp"
        && in_pack(Path::new(&config.args[0]))
        && Path::new(&config.args[0]).ends_with(Path::new("scripts").join("rea.mjs"));
    if !in_pack(&config.program) || !exact || !config.cwd.starts_with(root) {
        return Err(ReverseError::Missing { part: "rea".into() });
    }
    Ok(())
}

/// Empties `reverse/tmp` (best effort: a file another program still holds is left), makes
/// rea's temporary folder outside the reverse folder afresh when the configuration uses one
/// (`run_tmp`, [`prepare_private_dir`]) and creates the folders rea's environment names.
fn prepare_folders(tmp: &Path, run_tmp: Option<&Path>, folders: &[PathBuf]) -> std::io::Result<()> {
    // Does not follow links: a link inside is removed, not its target.
    match std::fs::remove_dir_all(tmp) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(error = %e, "could not empty rea's temporary folder"),
    }
    std::fs::create_dir_all(tmp)?;
    if let Some(run_tmp) = run_tmp {
        prepare_private_dir(run_tmp)?;
    }
    for folder in folders {
        std::fs::create_dir_all(folder)?;
    }
    Ok(())
}

/// Makes `dir` afresh for rea: whatever is there is removed without following links, then
/// the folder is created (its parent must exist) open to this user only, mode 0700, and
/// checked to be a real folder of this user's with that mode. Its parent is the system's
/// own private runtime folder, so only this user's programs could put something else there;
/// if one did, rea does not start.
fn prepare_private_dir(dir: &Path) -> std::io::Result<()> {
    remove_private_dir(dir);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        std::fs::DirBuilder::new().mode(0o700).create(dir)?;
        // Whatever the umask: exactly 0700.
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::DirBuilder::new().create(dir)?;
    }
    if cfg!(unix) && !crate::layout::private_folder(dir) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "rea's temporary folder could not be made private",
        ));
    }
    Ok(())
}

/// Removes `dir` and everything in it without following links: a link or a file in its
/// place is removed itself. Best effort (a warning in the log).
fn remove_private_dir(dir: &Path) {
    let removed = match std::fs::symlink_metadata(dir) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(dir),
        Ok(_) => std::fs::remove_file(dir),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    if let Err(e) = removed {
        tracing::warn!(error = %e, "could not remove rea's private temporary folder");
    }
}

/// Stops rea [`IDLE_STOP`] after its last use. Never during a call: it waits for the lock,
/// and stops while holding it.
fn spawn_idle_stop(weak: Weak<Shared>, generation: u64) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let deadline = {
                let Some(shared) = weak.upgrade() else {
                    return;
                };
                let s = shared.status();
                match (&s.running, s.last_used) {
                    (Some(running), Some(last)) if running.generation == generation => {
                        last + IDLE_STOP
                    }
                    _ => return,
                }
            };
            tokio::time::sleep_until(deadline).await;
            let Some(shared) = weak.upgrade() else {
                return;
            };
            let _calls = shared.calls.clone().lock_owned().await;
            shared.settled().await;
            let running = {
                let mut s = shared.status();
                let current = s
                    .running
                    .as_ref()
                    .is_some_and(|r| r.generation == generation);
                if !current {
                    return;
                }
                if s.last_used.is_some_and(|t| t + IDLE_STOP > Instant::now()) {
                    continue;
                }
                s.idle_task = None;
                s.running.take()
            };
            // No start while the lock is held.
            let starts = shared.run_tmp_starts();
            if let Some(running) = running {
                running.server.stop(STOP_GRACE).await;
            }
            remove_run_tmp_after_stop(&shared, starts).await;
            return;
        }
    })
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;
    use crate::Decompiler;
    use crate::env::{Flavor, run_config};
    use crate::lock;

    #[tokio::test]
    async fn the_guard_refuses_names_outside_the_allow_list() {
        let p = ReaProcess::new(ReverseLayout::new("/tmp/reverse"), Platform::LinuxX64);
        let mut g = p.lock().await;
        let allow = ToolAllowList::new(["open_binary"]);
        let e = g
            .call(
                &allow,
                "instrument_with_frida",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::NotAllowed {
                tool: "instrument_with_frida".into()
            }
        );
        // An allowed name with nothing running is not sent anywhere either.
        let e = g
            .call(
                &allow,
                "open_binary",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e, ReverseError::Stopped);
    }

    /// A rea that runs nothing, records what it is asked and can crash.
    #[derive(Default)]
    struct FakeRea {
        spec: StdMutex<Option<ManagedSpec>>,
        running: AtomicBool,
        crashed: AtomicBool,
        fail_start: bool,
        /// `start` never ends (it waits until its caller stops waiting for it).
        start_hangs: bool,
        /// Its process goes on ending after a start is dropped or a stop, until `end_now`.
        ends_late: bool,
        has_ended: AtomicBool,
        end: tokio::sync::Notify,
        starts: AtomicU64,
        calls: StdMutex<Vec<String>>,
        stops: StdMutex<Vec<Duration>>,
        /// Calls wait until cancelled or stopped.
        hang: bool,
        /// With `hang`: after a cancel, rea goes on this long, then stops itself (rea 6.9.0
        /// never answers a cancelled call, so the managed server stops it after its grace).
        cancel_takes: Option<Duration>,
        /// A call's token was cancelled.
        cancelled: AtomicBool,
        stopped: tokio::sync::Notify,
    }

    #[async_trait]
    impl Rea for FakeRea {
        async fn start(&self) -> Result<Vec<(String, Value)>, HostError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if self.start_hangs {
                std::future::pending::<()>().await;
            }
            if self.fail_start {
                return Err(HostError::Connect(
                    "the server exited with code 1 before completing the handshake".into(),
                ));
            }
            self.running.store(true, Ordering::SeqCst);
            Ok(vec![(
                "open_binary".into(),
                serde_json::json!({"type": "object"}),
            )])
        }
        async fn call(
            &self,
            allow: &ToolAllowList,
            name: &str,
            _args: Map<String, Value>,
            _timeout: Duration,
            cancel: &CancellationToken,
            _progress: Option<ProgressFn>,
        ) -> Result<CallOutcome, HostError> {
            assert!(allow.contains(name));
            if !self.running.load(Ordering::SeqCst) {
                return Err(HostError::NotReady("stopped".into()));
            }
            self.calls.lock().unwrap().push(name.to_owned());
            if self.hang {
                tokio::select! {
                    _ = cancel.cancelled() => {}
                    _ = self.stopped.notified() => return Err(HostError::NotReady("stopped".into())),
                }
                self.cancelled.store(true, Ordering::SeqCst);
                let Some(takes) = self.cancel_takes else {
                    return Ok(CallOutcome::Cancelled { stopped: false });
                };
                tokio::time::sleep(takes).await;
                self.running.store(false, Ordering::SeqCst);
                return Ok(CallOutcome::Cancelled { stopped: true });
            }
            Ok(CallOutcome::Done(ToolOutput::text(format!("ran {name}"))))
        }
        async fn stop(&self, grace: Duration) {
            self.stops.lock().unwrap().push(grace);
            self.running.store(false, Ordering::SeqCst);
            self.stopped.notify_waiters();
        }
        async fn ended(&self) {
            while self.ends_late && !self.has_ended.load(Ordering::SeqCst) {
                // Registered when made, so an `end_now` from here on is not missed.
                let end = self.end.notified();
                if self.has_ended.load(Ordering::SeqCst) {
                    break;
                }
                end.await;
            }
        }
        fn is_running(&self) -> bool {
            self.running.load(Ordering::SeqCst) && !self.crashed.load(Ordering::SeqCst)
        }
        fn ended_unexpectedly(&self) -> bool {
            self.crashed.load(Ordering::SeqCst)
        }
        fn stderr_tail(&self) -> Vec<String> {
            vec![format!(
                "Error: cannot open /data/reverse/rea-6.9.0/x\u{202E}y in {}",
                self.spec
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|s| s.cwd.display().to_string())
                    .unwrap_or_default()
            )]
        }
        fn lifecycle(&self) -> Option<Lifecycle> {
            Some(Lifecycle::Legacy)
        }
    }

    /// A reverse folder with rea (and optionally Kuna) installed as the embedded lock pins
    /// them, every launched server recorded.
    struct Bench {
        _dir: tempfile::TempDir,
        layout: ReverseLayout,
        launched: Arc<StdMutex<Vec<Arc<FakeRea>>>>,
        process: ReaProcess,
    }

    fn write_part(root: &Path, part: &str, programs: &[&str]) {
        let files: Vec<(&str, &[u8])> = programs.iter().map(|r| (*r, r.as_bytes())).collect();
        write_part_with(root, part, &files);
    }

    /// [`write_part`] with each program's content.
    fn write_part_with(root: &Path, part: &str, programs: &[(&str, &[u8])]) {
        let p = lock::embedded().part(part).expect("part in the lock");
        let dir = root.join(&p.dir);
        let mut recorded = Vec::new();
        for (rel, content) in programs {
            let file = dir.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, content).unwrap();
            recorded.push(serde_json::json!({"path": rel, "size": content.len()}));
        }
        let marker = serde_json::json!({
            "component": p.component,
            "part": p.name,
            "version": p.version,
            "archiveSha256": [],
            "programs": recorded,
            "diskBytes": 1,
        });
        std::fs::write(dir.join(".complete"), marker.to_string()).unwrap();
    }

    fn bench_with(
        platform: Platform,
        kuna: bool,
        apple_root: PathBuf,
        fake: fn() -> FakeRea,
    ) -> Bench {
        let dir = tempfile::tempdir().unwrap();
        let layout = ReverseLayout::new(dir.path().join("reverse"));
        write_part(layout.root(), "rea", &["scripts/rea.mjs"]);
        write_part(layout.root(), "node", &["bin/node"]);
        if kuna {
            write_part(layout.root(), "kuna", &["kuna"]);
        }
        let launched: Arc<StdMutex<Vec<Arc<FakeRea>>>> = Arc::default();
        let record = launched.clone();
        let launch: Launch = Arc::new(move |spec, _lifecycle| -> Arc<dyn Rea> {
            let rea = Arc::new(fake());
            *rea.spec.lock().unwrap() = Some(spec);
            record.lock().unwrap().push(rea.clone());
            rea
        });
        let process = ReaProcess::build(layout.clone(), platform, launch, apple_root);
        process.set_installer(Arc::new(
            Installer::new(layout.clone(), platform).with_release_rules(false),
        ));
        Bench {
            _dir: dir,
            layout,
            launched,
            process,
        }
    }

    fn bench(kuna: bool) -> Bench {
        bench_with(
            Platform::LinuxX64,
            kuna,
            PathBuf::from("/"),
            FakeRea::default,
        )
    }

    impl Bench {
        fn config(&self, decompiler: Option<Decompiler>) -> RunConfig {
            let installation = self.process.shared.installer().installation();
            run_config(
                &self.layout,
                &installation,
                decompiler,
                Flavor::Linux,
                false,
            )
        }
        fn last(&self) -> Arc<FakeRea> {
            self.launched
                .lock()
                .unwrap()
                .last()
                .cloned()
                .expect("launched")
        }
        fn count(&self) -> usize {
            self.launched.lock().unwrap().len()
        }
    }

    fn allow() -> ToolAllowList {
        ToolAllowList::new(["open_binary", "list_procedures"])
    }

    async fn run_call(
        process: &ReaProcess,
        config: RunConfig,
        name: &str,
    ) -> Result<ToolOutput, ReverseError> {
        let mut g = process.lock().await;
        g.ensure_started(config).await?;
        g.call(
            &allow(),
            name,
            Map::new(),
            Duration::from_secs(5),
            &CancellationToken::new(),
            None,
        )
        .await
    }

    #[tokio::test]
    async fn starts_once_and_reuses_the_running_rea() {
        let b = bench(true);
        let config = b.config(Some(Decompiler::Kuna));
        std::fs::create_dir_all(b.layout.tmp().join("old")).unwrap();
        std::fs::write(b.layout.tmp().join("old/leftover"), b"x").unwrap();
        assert_eq!(b.process.state(), RunState::Stopped);
        let mut g = b.process.lock().await;
        let first = g.ensure_started(config.clone()).await.unwrap();
        assert!(first.fresh);
        assert_eq!(first.generation, 1);
        assert_eq!(first.tools[0].0, "open_binary");
        let again = g.ensure_started(config.clone()).await.unwrap();
        assert!(!again.fresh);
        assert_eq!(again.generation, 1);
        assert_eq!(g.generation(), 1);
        let out = g
            .call(
                &allow(),
                "list_procedures",
                Map::new(),
                Duration::from_secs(5),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap();
        assert_eq!(out.joined_text(), "ran list_procedures");
        drop(g);
        assert_eq!(b.count(), 1);
        assert_eq!(b.process.generation(), 1);
        assert_eq!(b.process.state(), RunState::Ready);
        // The managed server got exactly the configuration, the log file and the limit.
        let spec = b.last().spec.lock().unwrap().clone().unwrap();
        assert_eq!(spec.program, config.program);
        assert_eq!(spec.args, config.args);
        assert_eq!(spec.env, config.env);
        assert_eq!(spec.cwd, b.layout.work());
        assert_eq!(spec.start_timeout, Duration::from_secs(60));
        assert_eq!(spec.name, "reverse");
        // reverse/tmp was emptied; the folders the environment names exist.
        assert!(b.layout.tmp().is_dir());
        assert!(!b.layout.tmp().join("old").exists());
        for f in [
            b.layout.home(),
            b.layout.work(),
            b.layout.kuna_cache(),
            b.layout.home().join(".config"),
        ] {
            assert!(f.is_dir(), "{}", f.display());
        }
    }

    /// A Linux bench with rea, Ghidra and Java installed and a private runtime folder
    /// (`<temp>/run`, mode 0700) holding rea's temporary folder outside the reverse folder,
    /// with a file left in it by an earlier run.
    #[cfg(unix)]
    fn bench_ghidra_with_run_tmp() -> (Bench, PathBuf) {
        bench_ghidra_with_run_tmp_using(FakeRea::default)
    }

    /// [`bench_ghidra_with_run_tmp`] with each rea made by `fake`.
    #[cfg(unix)]
    fn bench_ghidra_with_run_tmp_using(fake: fn() -> FakeRea) -> (Bench, PathBuf) {
        let (dir, layout, run_tmp) = ghidra_layout_with_run_tmp();
        let launched: Arc<StdMutex<Vec<Arc<FakeRea>>>> = Arc::default();
        let record = launched.clone();
        let launch: Launch = Arc::new(move |spec, _lifecycle| -> Arc<dyn Rea> {
            let rea = Arc::new(fake());
            *rea.spec.lock().unwrap() = Some(spec);
            record.lock().unwrap().push(rea.clone());
            rea
        });
        let process = ReaProcess::build(
            layout.clone(),
            Platform::LinuxX64,
            launch,
            PathBuf::from("/"),
        );
        process.set_installer(Arc::new(
            Installer::new(layout.clone(), Platform::LinuxX64).with_release_rules(false),
        ));
        let bench = Bench {
            _dir: dir,
            layout,
            launched,
            process,
        };
        (bench, run_tmp)
    }

    /// The reverse folder of [`bench_ghidra_with_run_tmp`], and rea's temporary folder.
    #[cfg(unix)]
    fn ghidra_layout_with_run_tmp() -> (tempfile::TempDir, ReverseLayout, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        std::fs::create_dir(&run).unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o700)).unwrap();
        let run_tmp = run.join("app-reverse");
        std::fs::create_dir(&run_tmp).unwrap();
        std::fs::write(run_tmp.join("left-by-a-crash"), b"x").unwrap();
        let layout =
            ReverseLayout::new(dir.path().join("reverse")).with_run_tmp(Some(run_tmp.clone()));
        write_part(layout.root(), "rea", &["scripts/rea.mjs"]);
        write_part(layout.root(), "node", &["bin/node"]);
        write_part(
            layout.root(),
            "ghidra",
            &[
                "support/analyzeHeadless",
                "Ghidra/Features/Decompiler/os/linux_x86_64/decompile",
                "Ghidra/application.properties",
            ],
        );
        write_part(layout.root(), "java", &["bin/java", "bin/javac"]);
        (dir, layout, run_tmp)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn with_ghidra_the_runtime_folder_is_made_private_at_each_start_and_removed_at_each_stop()
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let (b, run_tmp) = bench_ghidra_with_run_tmp();
        // The app's start removed what an earlier run left.
        assert!(!run_tmp.exists(), "left from an earlier run");
        let config = b.config(Some(Decompiler::Ghidra));
        assert_eq!(config.var("TMPDIR"), run_tmp.to_str());
        // Something else in its place (a link to another folder): removed, never followed.
        let elsewhere = run_tmp.parent().unwrap().with_file_name("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("keep"), b"x").unwrap();
        std::os::unix::fs::symlink(&elsewhere, &run_tmp).unwrap();
        let mut g = b.process.lock().await;
        g.ensure_started(config.clone()).await.unwrap();
        let meta = std::fs::symlink_metadata(&run_tmp).unwrap();
        assert!(meta.is_dir(), "not a real folder");
        assert_eq!(meta.permissions().mode() & 0o777, 0o700);
        // SAFETY: geteuid has no preconditions and cannot fail.
        assert_eq!(meta.uid(), unsafe { libc::geteuid() });
        assert!(elsewhere.join("keep").exists(), "the link was followed");
        assert_eq!(std::fs::read_dir(&run_tmp).unwrap().count(), 0);
        drop(g);
        // rea writes Ghidra's session there; a stop removes it.
        std::fs::create_dir(run_tmp.join("rea-ghidra-AbC123")).unwrap();
        std::fs::write(run_tmp.join("rea-ghidra-AbC123/copy"), b"x").unwrap();
        b.process.stop(STOP_GRACE).await;
        assert!(!run_tmp.exists(), "still there after the stop");
        // So does a restart, and quitting.
        run_call(&b.process, config.clone(), "open_binary")
            .await
            .unwrap();
        assert!(run_tmp.is_dir());
        b.process.restart(RestartReason::Updated).await;
        assert!(!run_tmp.exists(), "still there after a restart");
        run_call(&b.process, config.clone(), "open_binary")
            .await
            .unwrap();
        b.process.restart(RestartReason::Quit).await;
        assert!(!run_tmp.exists(), "still there after quitting");
        // Kuna, or no decompiler, never uses it.
        for d in [None, Some(Decompiler::Kuna)] {
            let other = b.config(d);
            assert_eq!(other.var("TMPDIR"), b.layout.tmp().to_str(), "{d:?}");
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_stop_removes_the_runtime_folder_only_when_no_start_came_since() {
        let (b, run_tmp) = bench_ghidra_with_run_tmp();
        let config = b.config(Some(Decompiler::Ghidra));
        let shared = b.process.shared.clone();
        let mut g = b.process.lock().await;
        g.ensure_started(config.clone()).await.unwrap();
        // While rea runs: kept.
        remove_run_tmp_after_stop(&shared, shared.run_tmp_starts()).await;
        assert!(run_tmp.is_dir(), "removed while rea runs");
        drop(g);
        b.process.stop(STOP_GRACE).await;
        assert!(!run_tmp.exists());
        // A start prepared it after the stop began (the count moved on): kept.
        std::fs::create_dir(&run_tmp).unwrap();
        let before = shared.run_tmp_starts();
        *shared.run_tmp_starts.lock().unwrap() += 1;
        remove_run_tmp_after_stop(&shared, before).await;
        assert!(run_tmp.is_dir(), "removed under a newer start");
        remove_run_tmp_after_stop(&shared, shared.run_tmp_starts()).await;
        assert!(!run_tmp.exists());
    }

    #[cfg(unix)]
    #[tokio::test(start_paused = true)]
    async fn the_idle_stop_removes_the_runtime_folder_too() {
        let (b, run_tmp) = bench_ghidra_with_run_tmp();
        run_call(
            &b.process,
            b.config(Some(Decompiler::Ghidra)),
            "open_binary",
        )
        .await
        .unwrap();
        assert!(run_tmp.is_dir());
        tokio::time::sleep(IDLE_STOP + Duration::from_secs(60)).await;
        assert_eq!(b.process.state(), RunState::Stopped);
        // The removal runs on a blocking thread; give it a moment (real time).
        for _ in 0..400 {
            if !run_tmp.exists() {
                break;
            }
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!run_tmp.exists(), "still there after the idle stop");
    }

    #[tokio::test]
    async fn the_log_goes_to_mcp_reverse_log_in_the_log_folder() {
        let b = bench(false);
        let logs = b.layout.root().join("logs");
        b.process.set_log_dir(Some(&logs));
        run_call(&b.process, b.config(None), "open_binary")
            .await
            .unwrap();
        let spec = b.last().spec.lock().unwrap().clone().unwrap();
        assert_eq!(spec.log_file, Some(logs.join("mcp-reverse.log")));
    }

    #[tokio::test]
    async fn restart_stops_rea_and_the_next_start_raises_the_generation() {
        let b = bench(true);
        run_call(&b.process, b.config(Some(Decompiler::Kuna)), "open_binary")
            .await
            .unwrap();
        let first = b.last();
        b.process.restart(RestartReason::DecompilerChanged).await;
        assert_eq!(*first.stops.lock().unwrap(), vec![STOP_GRACE]);
        assert_eq!(b.process.state(), RunState::Stopped);
        assert_eq!(b.process.generation(), 1);
        let mut g = b.process.lock().await;
        let s = g.ensure_started(b.config(None)).await.unwrap();
        assert!(s.fresh);
        assert_eq!(s.generation, 2);
        drop(g);
        assert_eq!(b.process.generation(), 2);
        assert_eq!(b.count(), 2);
    }

    #[tokio::test]
    async fn another_configuration_is_never_served_by_the_running_rea() {
        let b = bench(true);
        run_call(&b.process, b.config(Some(Decompiler::Kuna)), "open_binary")
            .await
            .unwrap();
        let kuna = b.last();
        let mut g = b.process.lock().await;
        let s = g.ensure_started(b.config(None)).await.unwrap();
        assert!(s.fresh && s.generation == 2);
        assert_eq!(kuna.stops.lock().unwrap().len(), 1);
        assert!(!kuna.is_running());
        let env = &b.last().spec.lock().unwrap().clone().unwrap().env;
        assert!(
            env.set
                .iter()
                .any(|(k, v)| k == "REA_ANALYSIS_PROVIDER" && v == "ghidra")
        );
        assert!(!env.set.iter().any(|(k, _)| k == "REA_KUNA_COMMAND"));
    }

    #[tokio::test]
    async fn nothing_starts_without_a_usable_rea() {
        // Not installed at all.
        let dir = tempfile::tempdir().unwrap();
        let layout = ReverseLayout::new(dir.path().join("reverse"));
        let b = bench(false);
        let empty = ReaProcess::build(
            layout.clone(),
            Platform::LinuxX64,
            b.process.shared.launch.clone(),
            PathBuf::from("/"),
        );
        empty.set_installer(Arc::new(
            Installer::new(layout.clone(), Platform::LinuxX64).with_release_rules(false),
        ));
        let installation = empty.shared.installer().installation();
        let config = run_config(&layout, &installation, None, Flavor::Linux, false);
        let e = empty.lock().await.ensure_started(config).await.unwrap_err();
        assert_eq!(
            e,
            ReverseError::NotInstalled {
                component: Component::Rea
            }
        );
        // The marker is missing: the part counts as not there.
        let b = bench(false);
        let config = b.config(None);
        let rea_dir = b
            .layout
            .part_dir(&lock::embedded().part("rea").unwrap().dir);
        std::fs::remove_file(rea_dir.join(".complete")).unwrap();
        let e = b
            .process
            .lock()
            .await
            .ensure_started(config.clone())
            .await
            .unwrap_err();
        assert!(
            matches!(
                e,
                ReverseError::Missing { .. } | ReverseError::NotInstalled { .. }
            ),
            "{e:?}"
        );
        // Broken: a program is gone.
        let b = bench(false);
        let config = b.config(None);
        std::fs::remove_file(
            b.layout
                .part_dir(&lock::embedded().part("node").unwrap().dir)
                .join("bin/node"),
        )
        .unwrap();
        let e = b
            .process
            .lock()
            .await
            .ensure_started(config)
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::Missing {
                part: "node".into()
            }
        );
        // NeedsUpdate: only an older rea, below the lock's minimum, is on disk.
        let b = bench(false);
        let config = b.config(None);
        let pinned = lock::embedded().part("rea").unwrap();
        let old_dir = b.layout.root().join("rea-6.8.0");
        std::fs::rename(b.layout.part_dir(&pinned.dir), &old_dir).unwrap();
        let marker = std::fs::read_to_string(old_dir.join(".complete"))
            .unwrap()
            .replace("6.9.0", "6.8.0");
        std::fs::write(old_dir.join(".complete"), marker).unwrap();
        let e = b
            .process
            .lock()
            .await
            .ensure_started(config)
            .await
            .unwrap_err();
        assert_eq!(e, ReverseError::NeedsUpdate { part: "rea".into() });
        // A decompiler the configuration names must be usable too.
        let b = bench(true);
        let config = b.config(Some(Decompiler::Kuna));
        std::fs::remove_file(
            b.layout
                .part_dir(&lock::embedded().part("kuna").unwrap().dir)
                .join("kuna"),
        )
        .unwrap();
        let e = b
            .process
            .lock()
            .await
            .ensure_started(config)
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::Missing {
                part: "kuna".into()
            }
        );
        assert_eq!(b.count(), 0, "nothing was launched");
    }

    #[tokio::test]
    async fn only_node_with_rea_mcp_from_the_pack_starts() {
        let b = bench(false);
        let good = b.config(None);
        let outside = tempfile::NamedTempFile::new().unwrap();
        let mut variants = Vec::new();
        let mut c = good.clone();
        c.program = outside.path().to_path_buf();
        variants.push(c);
        let mut c = good.clone();
        c.args.push("--http".into());
        variants.push(c);
        let mut c = good.clone();
        c.args[1] = "setup".into();
        variants.push(c);
        let mut c = good.clone();
        c.args[0] = outside.path().to_string_lossy().into_owned();
        variants.push(c);
        let mut c = good.clone();
        c.env = c.env.set("REA_ANALYSIS_PROVIDER", "auto");
        variants.push(c);
        for config in variants {
            let e = b
                .process
                .lock()
                .await
                .ensure_started(config.clone())
                .await
                .unwrap_err();
            assert!(
                matches!(
                    e,
                    ReverseError::Missing { .. } | ReverseError::StartFailed { .. }
                ),
                "{config:?}: {e:?}"
            );
        }
        assert_eq!(b.count(), 0);
        b.process.lock().await.ensure_started(good).await.unwrap();
        assert_eq!(b.count(), 1);
    }

    #[tokio::test]
    async fn a_mac_without_apple_tools_never_starts_rea() {
        let fake_root = tempfile::tempdir().unwrap();
        let b = bench_with(
            Platform::MacosArm64,
            true,
            fake_root.path().to_path_buf(),
            FakeRea::default,
        );
        let installation = b.process.shared.installer().installation();
        let config = run_config(
            &b.layout,
            &installation,
            Some(Decompiler::Kuna),
            Flavor::Macos,
            false,
        );
        let e = b
            .process
            .lock()
            .await
            .ensure_started(config.clone())
            .await
            .unwrap_err();
        assert_eq!(e, ReverseError::AppleToolsMissing);
        assert_eq!(b.count(), 0);
        // Checked fresh: once the tools are there, rea starts without an app restart.
        let swiftc = fake_root
            .path()
            .join("Library/Developer/CommandLineTools/usr/bin/swiftc");
        std::fs::create_dir_all(swiftc.parent().unwrap()).unwrap();
        std::fs::write(&swiftc, b"").unwrap();
        b.process.lock().await.ensure_started(config).await.unwrap();
        assert_eq!(b.count(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn rea_stops_after_15_minutes_without_a_call() {
        let b = bench(false);
        run_call(&b.process, b.config(None), "open_binary")
            .await
            .unwrap();
        let rea = b.last();
        tokio::time::sleep(Duration::from_secs(14 * 60)).await;
        assert!(rea.is_running());
        // A call resets the clock.
        run_call(&b.process, b.config(None), "list_procedures")
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_secs(14 * 60)).await;
        assert!(rea.is_running(), "stopped 14 minutes after the last call");
        tokio::time::sleep(Duration::from_secs(2 * 60)).await;
        assert!(
            !rea.is_running(),
            "still running 16 minutes after the last call"
        );
        assert_eq!(*rea.stops.lock().unwrap(), vec![STOP_GRACE]);
        assert_eq!(b.process.state(), RunState::Stopped);
        // The next call starts a new rea with a new generation.
        let mut g = b.process.lock().await;
        let s = g.ensure_started(b.config(None)).await.unwrap();
        assert!(s.fresh && s.generation == 2);
    }

    #[tokio::test(start_paused = true)]
    async fn the_idle_stop_never_cuts_a_call_short() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            hang: true,
            ..FakeRea::default()
        });
        let config = b.config(None);
        let mut g = b.process.lock().await;
        g.ensure_started(config).await.unwrap();
        let cancel = CancellationToken::new();
        let trigger = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(20 * 60)).await;
            trigger.cancel();
        });
        // A call that runs for 20 minutes holds rea; the idle stop waits for it.
        let e = g
            .call(
                &allow(),
                "open_binary",
                Map::new(),
                Duration::from_secs(3600),
                &cancel,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e, ReverseError::Cancelled);
        let rea = b.last();
        assert!(rea.is_running());
        assert!(rea.stops.lock().unwrap().is_empty());
        drop(g);
        tokio::time::sleep(Duration::from_secs(14 * 60)).await;
        assert!(rea.is_running());
        tokio::time::sleep(Duration::from_secs(2 * 60)).await;
        assert!(!rea.is_running());
    }

    #[tokio::test]
    async fn the_state_follows_rea() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            hang: true,
            ..FakeRea::default()
        });
        let mut g = b.process.lock().await;
        g.set_chat(42);
        g.ensure_started(b.config(None)).await.unwrap();
        let process = &b.process;
        let cancel = CancellationToken::new();
        let names = allow();
        let call = g.call(
            &names,
            "open_binary",
            Map::new(),
            Duration::from_secs(60),
            &cancel,
            None,
        );
        let watch = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            assert_eq!(process.state(), RunState::Busy { chat_id: 42 });
            assert!(process.is_busy());
            cancel.cancel();
        };
        let (result, ()) = tokio::join!(call, watch);
        assert_eq!(result.unwrap_err(), ReverseError::Cancelled);
        assert_eq!(process.state(), RunState::Ready);
        drop(g);
        assert!(!process.is_busy());
        // A crash: Failed with the tail, the reverse folder masked and hidden characters
        // made visible.
        b.last().crashed.store(true, Ordering::SeqCst);
        let RunState::Failed { reason } = process.state() else {
            panic!("{:?}", process.state())
        };
        assert!(reason.contains("[harnessUI's rea folder]"), "{reason}");
        assert!(
            !reason.contains(&b.layout.root().display().to_string()),
            "{reason}"
        );
        assert!(!reason.contains('\u{202E}'), "{reason}");
        // The next use starts again.
        let mut g = b.process.lock().await;
        assert!(g.ensure_started(b.config(None)).await.unwrap().fresh);
        drop(g);
        assert_eq!(process.state(), RunState::Ready);
    }

    #[tokio::test]
    async fn a_failed_start_is_reported_with_details_and_tried_again_next_time() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            fail_start: true,
            ..FakeRea::default()
        });
        let e = b
            .process
            .lock()
            .await
            .ensure_started(b.config(None))
            .await
            .unwrap_err();
        let ReverseError::StartFailed { message } = e else {
            panic!("{e:?}")
        };
        assert!(
            message.contains("before completing the handshake"),
            "{message}"
        );
        assert!(message.contains("[harnessUI's rea folder]"), "{message}");
        assert!(matches!(b.process.state(), RunState::Failed { .. }));
        assert_eq!(b.process.generation(), 0);
        let _ = b.process.lock().await.ensure_started(b.config(None)).await;
        assert_eq!(b.count(), 2, "tried again on the next use");
    }

    #[tokio::test]
    async fn a_stop_ends_a_running_call_and_quit_prevents_new_starts() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            hang: true,
            ..FakeRea::default()
        });
        let mut g = b.process.lock().await;
        g.ensure_started(b.config(None)).await.unwrap();
        let process = &b.process;
        let cancel = CancellationToken::new();
        let names = allow();
        let call = g.call(
            &names,
            "open_binary",
            Map::new(),
            Duration::from_secs(60),
            &cancel,
            None,
        );
        // The stop does not wait for the lock the call holds.
        let stop = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            process.restart(RestartReason::Quit).await;
        };
        let (result, ()) = tokio::join!(call, stop);
        assert_eq!(result.unwrap_err(), ReverseError::Stopped);
        assert_eq!(b.last().stops.lock().unwrap().clone(), vec![STOP_GRACE]);
        let e = g.ensure_started(b.config(None)).await.unwrap_err();
        assert_eq!(e, ReverseError::Stopped);
        assert_eq!(b.count(), 1);
    }

    /// How the agent loop waits for a tool (`crates/agent/src/turn/runner.rs`): until it
    /// ends, or, once the turn is cancelled, 250 ms more; then the tool's future is dropped.
    async fn like_the_agent_loop<F: std::future::Future>(
        call: F,
        cancel: &CancellationToken,
    ) -> Option<F::Output> {
        let mut call = std::pin::pin!(call);
        tokio::select! {
            biased;
            out = &mut call => Some(out),
            () = cancel.cancelled() => {
                tokio::time::timeout(Duration::from_millis(250), &mut call).await.ok()
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_call_dropped_after_stop_holds_rea_until_rea_is_done_with_it() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            hang: true,
            cancel_takes: Some(Duration::from_secs(8)),
            ..FakeRea::default()
        });
        let names = allow();
        {
            let mut g = b.process.lock().await;
            g.set_chat(7);
            g.ensure_started(b.config(None)).await.unwrap();
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                trigger.cancel();
            });
            let call = g.call(
                &names,
                "open_binary",
                Map::new(),
                Duration::from_secs(60),
                &cancel,
                None,
            );
            let out = like_the_agent_loop(call, &cancel).await;
            assert!(out.is_none(), "{out:?}");
        }
        let rea = b.last();
        assert!(rea.cancelled.load(Ordering::SeqCst));
        // rea still works on the stopped call: the tab says so, and nobody gets rea.
        assert_eq!(b.process.state(), RunState::Busy { chat_id: 7 });
        assert!(b.process.is_busy());
        let early = tokio::time::timeout(Duration::from_secs(5), b.process.lock()).await;
        assert!(
            early.is_err(),
            "another caller got rea while it still worked on the stopped call"
        );
        let g = tokio::time::timeout(Duration::from_secs(10), b.process.lock())
            .await
            .expect("rea is free once the stopped call has ended");
        assert!(!rea.is_running());
        assert_eq!(b.process.state(), RunState::Stopped);
        drop(g);
        assert!(!b.process.is_busy());
        // The next call starts a new rea.
        let mut g = b.process.lock().await;
        let s = g.ensure_started(b.config(None)).await.unwrap();
        assert!(s.fresh && s.generation == 2);
    }

    #[tokio::test]
    async fn a_call_whose_caller_goes_away_is_cancelled_and_no_longer_busy() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            hang: true,
            ..FakeRea::default()
        });
        let names = allow();
        let cancel = CancellationToken::new();
        let mut g = b.process.lock().await;
        g.set_chat(9);
        g.ensure_started(b.config(None)).await.unwrap();
        // The caller stops waiting without cancelling its token.
        let gave_up = tokio::time::timeout(
            Duration::from_millis(100),
            g.call(
                &names,
                "open_binary",
                Map::new(),
                Duration::from_secs(60),
                &cancel,
                None,
            ),
        )
        .await;
        assert!(gave_up.is_err(), "{gave_up:?}");
        drop(g);
        let rea = b.last();
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline
            && (b.process.state() != RunState::Ready || !rea.cancelled.load(Ordering::SeqCst))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(rea.cancelled.load(Ordering::SeqCst), "rea was never told");
        assert!(
            !cancel.is_cancelled(),
            "the caller's own token is left alone"
        );
        assert_eq!(b.process.state(), RunState::Ready);
        assert!(!b.process.is_busy());
        assert!(rea.is_running());
    }

    /// Waits until the tab would say "rea is starting…".
    async fn until_starting(process: &ReaProcess) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while process.state() != RunState::Starting {
            assert!(Instant::now() < deadline, "rea never got to Starting");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[tokio::test]
    async fn a_start_dropped_like_the_agent_loop_leaves_rea_stopped_and_free() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            start_hangs: true,
            ..FakeRea::default()
        });
        {
            let mut g = b.process.lock().await;
            g.set_chat(7);
            let cancel = CancellationToken::new();
            // Stop (or deleting the chat) while rea starts.
            let stop = async {
                until_starting(&b.process).await;
                cancel.cancel();
            };
            let start = like_the_agent_loop(g.ensure_started(b.config(None)), &cancel);
            let (out, ()) = tokio::join!(start, stop);
            assert!(out.is_none(), "{out:?}");
        }
        let rea = b.last();
        assert_eq!(b.process.state(), RunState::Stopped);
        // The half-started rea is stopped, and then rea is free for the next caller.
        let g = tokio::time::timeout(Duration::from_secs(5), b.process.lock())
            .await
            .expect("rea is free once the dropped start is undone");
        assert_eq!(*rea.stops.lock().unwrap(), vec![STOP_GRACE]);
        drop(g);
        assert!(!b.process.is_busy());
        assert_eq!(b.process.state(), RunState::Stopped);
        assert_eq!(b.process.generation(), 0);
    }

    #[tokio::test]
    async fn a_restart_ends_a_start_that_runs() {
        let b = bench_with(Platform::LinuxX64, false, PathBuf::from("/"), || FakeRea {
            start_hangs: true,
            ..FakeRea::default()
        });
        let mut g = b.process.lock().await;
        let config = b.config(None);
        let start = tokio::spawn(async move { g.ensure_started(config).await });
        until_starting(&b.process).await;
        // The restart from Settings (Remove, a decompiler switch, an update).
        b.process.restart(RestartReason::Updated).await;
        let rea = b.last();
        assert_eq!(*rea.stops.lock().unwrap(), vec![STOP_GRACE]);
        assert_eq!(b.process.state(), RunState::Stopped);
        // Its caller goes away later: the rea the restart stopped is left alone.
        start.abort();
        assert!(start.await.unwrap_err().is_cancelled());
        assert_eq!(b.process.state(), RunState::Stopped);
        assert!(!b.process.is_busy());
        assert_eq!(*rea.stops.lock().unwrap(), vec![STOP_GRACE]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_dropped_start_holds_rea_and_its_runtime_folder_until_its_rea_has_ended() {
        let (b, run_tmp) = bench_ghidra_with_run_tmp_using(|| FakeRea {
            start_hangs: true,
            ends_late: true,
            ..FakeRea::default()
        });
        let config = b.config(Some(Decompiler::Ghidra));
        {
            let mut g = b.process.lock().await;
            let cancel = CancellationToken::new();
            let stop = async {
                until_starting(&b.process).await;
                cancel.cancel();
            };
            let start = like_the_agent_loop(g.ensure_started(config), &cancel);
            let (out, ()) = tokio::join!(start, stop);
            assert!(out.is_none(), "{out:?}");
        }
        let rea = b.last();
        let deadline = Instant::now() + Duration::from_secs(5);
        while rea.stops.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "the half-started rea was never stopped");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        // Its process is still ending: rea is busy (not Starting) and its folder stays.
        assert_eq!(b.process.state(), RunState::Stopped);
        assert!(b.process.is_busy());
        assert!(run_tmp.is_dir());
        // A restart meanwhile leaves the folder too: the half-started rea may still use it.
        b.process.restart(RestartReason::Updated).await;
        assert!(
            run_tmp.is_dir(),
            "removed while the half-started rea could still run"
        );
        let early = tokio::time::timeout(Duration::from_millis(200), b.process.lock()).await;
        assert!(
            early.is_err(),
            "another caller got rea while the half-started rea still ran"
        );
        assert!(b.process.is_busy());
        // Once it has ended, rea is free and the folder that start made is gone.
        rea.has_ended.store(true, Ordering::SeqCst);
        rea.end.notify_waiters();
        let g = tokio::time::timeout(Duration::from_secs(5), b.process.lock())
            .await
            .expect("rea is free once the half-started rea has ended");
        assert!(!run_tmp.exists(), "the dropped start's folder is still there");
        assert_eq!(*rea.stops.lock().unwrap(), vec![STOP_GRACE]);
        drop(g);
        assert!(!b.process.is_busy());
        assert_eq!(b.process.state(), RunState::Stopped);
    }

    /// A Ghidra bench ([`ghidra_layout_with_run_tmp`]) started through the real managed
    /// server, whose `node` is a shell script that writes its process id to the returned
    /// file and then never answers rea's handshake.
    #[cfg(target_os = "linux")]
    fn silent_managed_rea() -> (tempfile::TempDir, ReaProcess, RunConfig, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let (dir, layout, run_tmp) = ghidra_layout_with_run_tmp();
        let pid_file = dir.path().join("rea.pid");
        let script = format!(
            "#!/bin/sh\necho $$ > '{}'\nexec /bin/sleep 1000\n",
            pid_file.display()
        );
        write_part_with(layout.root(), "node", &[("bin/node", script.as_bytes())]);
        let node = layout
            .part_dir(&lock::embedded().part("node").unwrap().dir)
            .join("bin/node");
        std::fs::set_permissions(&node, std::fs::Permissions::from_mode(0o755)).unwrap();
        let process = ReaProcess::build(
            layout.clone(),
            Platform::LinuxX64,
            launch_managed(),
            PathBuf::from("/"),
        );
        process.set_installer(Arc::new(
            Installer::new(layout.clone(), Platform::LinuxX64).with_release_rules(false),
        ));
        let installation = process.shared.installer().installation();
        let config = run_config(
            &layout,
            &installation,
            Some(Decompiler::Ghidra),
            Flavor::Linux,
            false,
        );
        assert_eq!(config.var("TMPDIR"), run_tmp.to_str());
        (dir, process, config, run_tmp, pid_file)
    }

    /// Waits until the script of [`silent_managed_rea`] has written its process id.
    #[cfg(target_os = "linux")]
    async fn until_pid(file: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(pid) = std::fs::read_to_string(file)
                .ok()
                .and_then(|t| t.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "rea's process never started");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_start_dropped_with_the_managed_server_holds_rea_until_its_process_has_ended() {
        let (_dir, process, config, run_tmp, pid_file) = silent_managed_rea();
        let pid = {
            let mut g = process.lock().await;
            let cancel = CancellationToken::new();
            let stop = async {
                let pid = until_pid(&pid_file).await;
                assert_eq!(process.state(), RunState::Starting);
                cancel.cancel();
                pid
            };
            let start = like_the_agent_loop(g.ensure_started(config), &cancel);
            let (out, pid) = tokio::join!(start, stop);
            assert!(out.is_none(), "{out:?}");
            pid
        };
        // Dropping the start killed the half-started rea at once. Nobody gets rea before
        // that process has ended, and then the folder that start made is gone too.
        assert_eq!(process.state(), RunState::Stopped);
        let g = tokio::time::timeout(Duration::from_secs(10), process.lock())
            .await
            .expect("rea is free once the half-started rea has ended");
        assert!(
            !pid_alive(pid),
            "another caller got rea while the half-started rea {pid} still ran"
        );
        assert!(!run_tmp.exists(), "the dropped start's folder is still there");
        drop(g);
        assert!(!process.is_busy());
        assert_eq!(process.state(), RunState::Stopped);
    }

    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_restart_during_a_managed_start_ends_once_the_half_started_rea_has_ended() {
        let (_dir, process, config, run_tmp, pid_file) = silent_managed_rea();
        let process = Arc::new(process);
        let starter = process.clone();
        let file = pid_file.clone();
        let start = tokio::spawn(async move {
            let mut g = starter.lock().await;
            let out = g.ensure_started(config).await;
            let pid = until_pid(&file).await;
            (out, pid_alive(pid))
        });
        let pid = until_pid(&pid_file).await;
        until_starting(&process).await;
        process.restart(RestartReason::Updated).await;
        // The restart returns once the half-started rea has ended, and removes its folder.
        assert!(
            !pid_alive(pid),
            "the restart returned while the half-started rea {pid} still ran"
        );
        assert!(!run_tmp.exists());
        let (out, alive) = start.await.unwrap();
        assert_eq!(out.unwrap_err(), ReverseError::Stopped);
        assert!(!alive, "the start ended while its rea still ran");
        assert!(!process.is_busy());
        assert_eq!(process.state(), RunState::Stopped);
    }

    #[test]
    fn the_futures_are_send() {
        // `BuiltinServer::call` runs them on a multi-threaded runtime.
        fn assert_send<T: Send>(_: &T) {}
        let b = bench(false);
        let lock = b.process.lock();
        assert_send(&lock);
        assert_send(&b.process.stop(STOP_GRACE));
        assert_send(&b.process.restart(RestartReason::Updated));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut g = runtime.block_on(lock);
        let cancel = CancellationToken::new();
        let allow = allow();
        assert_send(&g.ensure_started(b.config(None)));
        assert_send(&g.call(
            &allow,
            "open_binary",
            Map::new(),
            Duration::from_secs(1),
            &cancel,
            None,
        ));
        fn assert_sync<T: Send + Sync>() {}
        assert_sync::<ReaProcess>();
        assert_sync::<ReaGuard>();
    }

    /// The real rea, Node.js and Kuna from a cache filled by `scripts/pin_reverse.py
    /// --download-to` (`REVERSE_PACK_CACHE`): installed through the real installer, started
    /// through this process manager with the environment of `env.rs`. Skipped quietly
    /// without the cache; WP9e's `tests/real_rea.rs` goes further (decompiling, network).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn with_the_real_rea_from_the_pack_cache() {
        let Some(cache) = std::env::var_os("REVERSE_PACK_CACHE") else {
            eprintln!("REVERSE_PACK_CACHE is not set; skipped");
            return;
        };
        let platform = Platform::current();
        let dir = tempfile::tempdir().unwrap();
        let layout = ReverseLayout::new(dir.path().join("reverse"));
        let installer = Arc::new(
            Installer::new(layout.clone(), platform)
                .with_fetch(Arc::new(crate::install::DirFetch::new(PathBuf::from(
                    cache,
                ))))
                .with_release_rules(false),
        );
        let with_kuna =
            platform::availability_with(lock::embedded(), Component::Kuna, platform, false).0;
        let row = if with_kuna {
            Component::Kuna
        } else {
            Component::Rea
        };
        installer
            .download(row, &CancellationToken::new(), &|_| {})
            .await
            .expect("installs from the cache");
        let process = ReaProcess::new(layout.clone(), platform);
        process.set_installer(installer.clone());
        process.set_log_dir(Some(&dir.path().join("logs")));
        let decompiler = with_kuna.then_some(Decompiler::Kuna);
        let config = run_config(
            &layout,
            &installer.installation(),
            decompiler,
            Flavor::current(),
            true,
        );
        let begun = std::time::Instant::now();
        let mut g = process.lock().await;
        let started = g.ensure_started(config.clone()).await.expect("rea starts");
        eprintln!(
            "real rea: started in {:.1} s with {} tools",
            begun.elapsed().as_secs_f64(),
            started.tools.len()
        );
        let names: Vec<&str> = started.tools.iter().map(|(n, _)| n.as_str()).collect();
        for wanted in [
            "open_binary",
            "list_procedures",
            "procedure_pseudo_code",
            "inspect_pe_imports",
            "inspect_analysis_view",
        ] {
            assert!(names.contains(&wanted), "{wanted} missing from {names:?}");
        }
        // The families left out of REA_MCP_TOOL_FAMILIES are not listed.
        for gone in ["instrument_with_frida", "adb_shell", "start_frida_session"] {
            assert!(!names.contains(&gone), "{gone} is listed");
        }
        assert_eq!(process.state(), RunState::Ready);
        let allow = ToolAllowList::new(["binary_session", "open_binary"]);
        let out = g
            .call(
                &allow,
                "binary_session",
                serde_json::json!({"detail": "summary"})
                    .as_object()
                    .cloned()
                    .unwrap(),
                Duration::from_secs(60),
                &CancellationToken::new(),
                None,
            )
            .await
            .expect("binary_session answers");
        eprintln!(
            "real rea: binary_session: {}",
            out.joined_text().chars().take(300).collect::<String>()
        );
        // Stop during an open: rea gets the grace, then it is stopped if it never answers.
        if let Some(target) = ["/bin/ls", "/usr/bin/ls"]
            .iter()
            .map(Path::new)
            .find(|p| p.is_file())
        {
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                trigger.cancel();
            });
            let begun = std::time::Instant::now();
            let stopped = g
                .call(
                    &allow,
                    "open_binary",
                    serde_json::json!({"path": target})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    Duration::from_secs(900),
                    &cancel,
                    None,
                )
                .await;
            eprintln!(
                "real rea: Stop during open_binary -> {stopped:?} after {:.1} s; state {:?}",
                begun.elapsed().as_secs_f64(),
                process.state()
            );
            assert!(
                matches!(stopped, Err(ReverseError::Cancelled) | Ok(_)),
                "{stopped:?}"
            );
        }
        drop(g);
        // Started again (if the Stop ended it); with Kuna, a whole open works with this
        // environment (REA_KUNA_COMMAND, its cache and home inside the reverse folder).
        let mut g = process.lock().await;
        g.ensure_started(config.clone())
            .await
            .expect("rea starts again");
        let target = ["/bin/ls", "/usr/bin/ls"]
            .iter()
            .map(Path::new)
            .find(|p| p.is_file());
        if let (true, Some(target)) = (with_kuna, target) {
            let allow = ToolAllowList::new(["open_binary", "list_procedures"]);
            let begun = std::time::Instant::now();
            let opened = g
                .call(
                    &allow,
                    "open_binary",
                    serde_json::json!({"path": target})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    Duration::from_secs(600),
                    &CancellationToken::new(),
                    None,
                )
                .await
                .expect("open_binary answers");
            eprintln!(
                "real rea: open_binary {} with Kuna in {:.1} s, error: {}: {}",
                target.display(),
                begun.elapsed().as_secs_f64(),
                opened.is_error,
                opened.joined_text().chars().take(300).collect::<String>()
            );
            assert!(!opened.is_error, "{opened:?}");
            let listed = g
                .call(
                    &allow,
                    "list_procedures",
                    serde_json::json!({"detail": "summary"})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    Duration::from_secs(300),
                    &CancellationToken::new(),
                    None,
                )
                .await
                .expect("list_procedures answers");
            eprintln!(
                "real rea: list_procedures: {}",
                listed.joined_text().chars().take(300).collect::<String>()
            );
            assert!(!listed.is_error, "{listed:?}");
            assert!(layout.kuna_cache().is_dir());
        }
        drop(g);
        // Stop the way the agent loop does it, on a file Kuna takes long over
        // (`REVERSE_SLOW_BINARY`, e.g. a 120 MB library): 250 ms after Stop the call's future
        // is dropped. rea never answers a cancelled call, so it must still be stopped after
        // its grace, Kuna with it, and nobody may use rea before that.
        if let (true, Some(slow)) = (with_kuna, std::env::var_os("REVERSE_SLOW_BINARY")) {
            let slow = PathBuf::from(slow);
            let allow = ToolAllowList::new(["open_binary", "list_procedures"]);
            let mut g = process.lock().await;
            g.set_chat(7);
            g.ensure_started(config.clone()).await.expect("rea runs");
            let opened = g
                .call(
                    &allow,
                    "open_binary",
                    serde_json::json!({"path": slow})
                        .as_object()
                        .cloned()
                        .unwrap(),
                    Duration::from_secs(600),
                    &CancellationToken::new(),
                    None,
                )
                .await
                .expect("open_binary answers");
            assert!(!opened.is_error, "{opened:?}");
            let rea_pid = process.pid().expect("rea runs");
            let cancel = CancellationToken::new();
            let trigger = cancel.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(4)).await;
                trigger.cancel();
            });
            let begun = std::time::Instant::now();
            let call = g.call(
                &allow,
                "list_procedures",
                serde_json::json!({"detail": "summary"})
                    .as_object()
                    .cloned()
                    .unwrap(),
                Duration::from_secs(900),
                &cancel,
                None,
            );
            let out = like_the_agent_loop(call, &cancel).await;
            drop(g);
            let busy = pack_processes(layout.root());
            eprintln!(
                "real rea: list_procedures dropped after {:.1} s: {:?}; state {:?}; pack processes {busy:?}",
                begun.elapsed().as_secs_f64(),
                out.as_ref().map(|o| o.as_ref().map(|t| t.is_error)),
                process.state()
            );
            if out.is_none() {
                assert_eq!(process.state(), RunState::Busy { chat_id: 7 });
                assert!(process.is_busy());
                let g = process.lock().await;
                let freed = begun.elapsed();
                let left = pack_processes(layout.root());
                eprintln!(
                    "real rea: the next caller got rea {:.1} s after the call began; rea {rea_pid} alive {}; pack processes {left:?}; state {:?}",
                    freed.as_secs_f64(),
                    pid_alive(rea_pid),
                    process.state()
                );
                assert!(freed >= Duration::from_secs(4) + mcp_host::CANCEL_GRACE);
                assert!(!pid_alive(rea_pid), "rea {rea_pid} survived the stop");
                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !pack_processes(layout.root()).is_empty()
                    && std::time::Instant::now() < deadline
                {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                let left = pack_processes(layout.root());
                assert!(left.is_empty(), "still running from the pack: {left:?}");
                assert!(!matches!(process.state(), RunState::Busy { .. }));
                drop(g);
            }
        }
        let begun = std::time::Instant::now();
        process.stop(STOP_GRACE).await;
        eprintln!("real rea: stop took {:.1} s", begun.elapsed().as_secs_f64());
        let log = std::fs::read_to_string(dir.path().join("logs").join(LOG_FILE_NAME))
            .unwrap_or_default();
        let notes: Vec<&str> = log.lines().filter(|l| l.starts_with("--- ")).collect();
        eprintln!("real rea: log notes {notes:#?}");
        assert!(!log.is_empty(), "nothing in the log file");
        assert_eq!(process.state(), RunState::Stopped);
        // Everything rea wrote stays in the reverse folder: its home was used.
        assert!(layout.home().is_dir() && layout.work().is_dir());
    }

    /// Processes whose program is inside `root` (rea's `node`, Kuna). Linux only.
    fn pack_processes(root: &Path) -> Vec<(u32, PathBuf)> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                let exe = std::fs::read_link(e.path().join("exe")).ok()?;
                (exe.starts_with(root) && pid_alive(pid)).then_some((pid, exe))
            })
            .collect()
    }

    /// Whether a process runs (a zombie counts as gone). Linux only.
    fn pid_alive(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
            stat.rsplit_once(')')
                .is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z'))
        })
    }

    #[tokio::test]
    async fn calls_take_turns() {
        let b = bench(false);
        let config = b.config(None);
        let mut first = b.process.lock().await;
        first.ensure_started(config.clone()).await.unwrap();
        let second = tokio::time::timeout(Duration::from_millis(100), b.process.lock()).await;
        assert!(
            second.is_err(),
            "a second caller got rea while the first held it"
        );
        drop(first);
        let second = tokio::time::timeout(Duration::from_secs(1), b.process.lock()).await;
        assert!(second.is_ok());
    }
}
