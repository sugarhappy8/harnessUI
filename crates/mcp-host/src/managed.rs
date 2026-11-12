//! A stdio MCP server the app owns and runs itself (rea; item5-spec.md WP9b, R9, R10), not
//! one of the user's servers.
//!
//! - **Start:** a fixed program, argument list, exact environment (nothing is captured or
//!   added: the caller passes the whole environment) and working folder, started through
//!   [`runtime::spawn::command_wrap`] like every other server (Job Object with kill-on-close
//!   and no console window on Windows, its own process group and the reaper on Unix). The
//!   handshake, the paged `tools/list` and the bounded stderr ring are the host's own
//!   ([`crate::connection`], [`crate::stdio`]); the first lifecycle tried is the legacy
//!   `initialize` (protocol 2025-11-25) and the one that worked is remembered. The whole
//!   start has its own limit ([`ManagedSpec::start_timeout`]). Stderr also goes to the exact
//!   log file the caller names (size-capped and rotated like the host's logs).
//! - **Calls:** [`ManagedServer::call`] refuses a name that is not in its [`ToolAllowList`]
//!   before anything is written to the server's standard input. Progress notifications from
//!   the server only keep a call alive (the per-call limit counts from the last sign of
//!   life, up to [`MAX_CALL_FACTOR`] times the limit in all); their text is never passed on.
//! - **Cancel** (the caller's token, or the per-call limit): the server is sent
//!   `notifications/cancelled` and given [`CANCEL_GRACE`] to answer the call. If it does, it
//!   keeps running. If not, it is stopped with [`STOP_AFTER_CANCEL`]: standard input
//!   closed, a wait, then its whole process tree killed. The next start begins afresh.
//!
//! The cancellation is sent as a plain notification on the wire rather than through rmcp's
//! request handle, because rmcp would drop the pending call (and any late answer) the moment
//! it sends one, and the answer is what tells the app the server ended the work itself.

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rmcp::model::{
    CallToolRequest, CallToolRequestParams, ClientNotification, ClientRequest, CustomNotification,
    Implementation, RequestId, ServerResult,
};
use rmcp::service::PeerRequestOptions;
use rmcp::{Peer, RoleClient, ServiceError};
use runtime::spawn::{SpawnSpec, StdioMode};
use serde_json::{Map, Value, json};
use tokio::sync::{Notify, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::builtin::{ProgressFn, ToolOutput};
use crate::connection::{self, Closed, Connection, Lifecycle, Target};
use crate::error::HostError;
use crate::result::map_call_result;
use crate::stderr::StderrLog;
use crate::stdio::Prepared;

/// After a cancel, how long the server has to answer the call before it is stopped.
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// How long a server stopped after an unanswered cancel gets to exit after its standard
/// input closes, before its tree is killed.
pub const STOP_AFTER_CANCEL: Duration = Duration::from_secs(3);

/// Progress keeps a call alive for at most this many times its limit in all.
pub const MAX_CALL_FACTOR: u32 = 4;

/// Progress ticks passed to the caller are at most this frequent.
const TICK_EVERY: Duration = Duration::from_secs(1);

/// Lines of the stderr tail quoted in a start-up error.
const TAIL_IN_ERRORS: usize = 10;

/// What the server is told it talks to.
const CLIENT_NAME: &str = "harnessUI";

/// The text of a call stopped by the caller, through [`ManagedServer::call`].
pub const CANCELLED_TEXT: &str = "cancelled: the user stopped this tool call before it finished";

/// How to start a managed server.
#[derive(Clone, Debug)]
pub struct ManagedSpec {
    /// Name used in logs (`reverse`).
    pub name: String,
    /// Absolute path of the program.
    pub program: PathBuf,
    /// Fixed argument list.
    pub args: Vec<String>,
    /// The whole environment (nothing else is inherited).
    pub env: runtime::EnvSpec,
    /// Working folder.
    pub cwd: PathBuf,
    /// Where stderr is logged (size-capped), if anywhere.
    pub log_file: Option<PathBuf>,
    /// How long the start and handshake may take.
    pub start_timeout: Duration,
}

/// The tool names a call may use; any other name is refused before the server hears of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolAllowList(BTreeSet<String>);

impl ToolAllowList {
    /// An allow-list of exactly these names.
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(names.into_iter().map(Into::into).collect())
    }
    /// Whether `name` may be called.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
    /// The names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
    /// How many names.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether it allows nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// How a call ended ([`ManagedServer::call_outcome`]).
#[derive(Debug, Clone, PartialEq)]
pub enum CallOutcome {
    /// The server answered: a result, or a tool error the model can read.
    Done(ToolOutput),
    /// The caller's token was cancelled (Stop).
    Cancelled {
        /// The server did not answer within [`CANCEL_GRACE`] and was stopped.
        stopped: bool,
    },
    /// Neither an answer nor progress came within the call's limit (or progress went on
    /// past [`MAX_CALL_FACTOR`] times it).
    TimedOut {
        /// The server did not answer within [`CANCEL_GRACE`] and was stopped.
        stopped: bool,
    },
}

/// How a server stopped by [`ManagedServer::stop_with`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// Nothing was running.
    NotRunning,
    /// It ended by itself within the grace (`exited with code 0`, …).
    Exited(String),
    /// It was still running after the grace, so its whole tree was killed.
    Killed,
}

/// One managed server process.
pub struct ManagedServer {
    spec: ManagedSpec,
    client: Implementation,
    log: Arc<StderrLog>,
    current: Mutex<Option<Arc<Connection>>>,
    lifecycle: Mutex<Option<Lifecycle>>,
    /// Grows at every stop, so a start that finishes after a stop does not survive it.
    stops: AtomicU64,
    /// Cancelled by a stop while a start runs.
    starting: Mutex<CancellationToken>,
    /// Serialises starts.
    start_lock: tokio::sync::Mutex<()>,
}

/// The server's answer to a call, or why there is none.
type Answer = Result<Result<ServerResult, ServiceError>, oneshot::error::RecvError>;

/// How a call's wait ended.
enum End {
    Answer(Box<Answer>),
    Cancelled,
    TimedOut,
}

impl ManagedServer {
    /// A server that is not started yet.
    pub fn new(spec: ManagedSpec) -> Self {
        let log = Arc::new(StderrLog::to_file(spec.log_file.clone()));
        Self {
            client: Implementation::new(CLIENT_NAME, env!("CARGO_PKG_VERSION")),
            log,
            current: Mutex::new(None),
            lifecycle: Mutex::new(None),
            stops: AtomicU64::new(0),
            starting: Mutex::new(CancellationToken::new()),
            start_lock: tokio::sync::Mutex::new(()),
            spec,
        }
    }

    /// Starts with this lifecycle first (one remembered from an earlier server).
    pub fn with_lifecycle(self, lifecycle: Option<Lifecycle>) -> Self {
        *lock(&self.lifecycle) = lifecycle;
        self
    }

    /// How it is started.
    pub fn spec(&self) -> &ManagedSpec {
        &self.spec
    }

    /// The lifecycle the last successful start used (tried first next time).
    pub fn lifecycle(&self) -> Option<Lifecycle> {
        *lock(&self.lifecycle)
    }

    /// Starts the server (stopping a previous process first) and returns its tools (name
    /// and input schema).
    pub async fn start(&self) -> Result<Vec<(String, Value)>, HostError> {
        let _start = self.start_lock.lock().await;
        let prepared = self.prepare()?;
        if self.live().is_some() || lock(&self.current).is_some() {
            self.stop_with(connection::STOP_GRACE).await;
        }
        let stops = self.stops.load(Ordering::SeqCst);
        let cancel = CancellationToken::new();
        *lock(&self.starting) = cancel.clone();
        let first = self.lifecycle().unwrap_or(Lifecycle::Legacy);
        let limit = self.spec.start_timeout;
        let target = Target::Stdio(prepared);
        let work = async {
            let connection =
                connection::connect(&target, first, &self.client, &self.log, limit).await?;
            match connection::list_tools(&connection, limit).await {
                Ok(tools) => Ok((connection, tools)),
                Err(message) => {
                    connection.close().await;
                    Err(HostError::Connect(message))
                }
            }
        };
        let result = tokio::select! {
            result = tokio::time::timeout(limit, work) => match result {
                Ok(result) => result,
                Err(_) => Err(HostError::Connect(format!(
                    "the server did not finish starting within {} seconds{}",
                    limit.as_secs(),
                    self.tail_for_errors()
                ))),
            },
            _ = cancel.cancelled() => Err(HostError::NotReady("it was stopped while starting".into())),
        };
        let (connection, tools) = match result {
            Ok(found) => found,
            Err(error) => {
                self.log.note("the server did not start").await;
                return Err(error);
            }
        };
        let connection = Arc::new(connection);
        // Checked and stored under the lock `stop_with` takes after counting its stop, so a
        // stop from another thread either sees this connection or makes the check fail.
        let stopped_meanwhile = {
            let mut current = lock(&self.current);
            let stale = self.stops.load(Ordering::SeqCst) != stops;
            if !stale {
                *current = Some(connection.clone());
            }
            stale
        };
        if stopped_meanwhile {
            connection.close().await;
            return Err(HostError::NotReady("it was stopped while starting".into()));
        }
        *lock(&self.lifecycle) = Some(connection.lifecycle);
        self.watch_exit(&connection);
        self.log
            .note(&format!("ready with {} tools", tools.len()))
            .await;
        Ok(tools
            .into_iter()
            .map(|t| (t.name.to_string(), Value::Object((*t.input_schema).clone())))
            .collect())
    }

    /// The command, exactly as given. The program and working folder must be absolute; a
    /// Windows batch file is refused (it would need `cmd.exe`).
    fn prepare(&self) -> Result<Prepared, HostError> {
        let s = &self.spec;
        if !s.program.is_absolute() {
            return Err(HostError::InvalidConfig(format!(
                "the program \"{}\" must be an absolute path",
                policy::sanitize::one_line(&s.program.to_string_lossy(), 200)
            )));
        }
        if !s.cwd.is_absolute() || !s.cwd.is_dir() {
            return Err(HostError::InvalidConfig(format!(
                "the working folder \"{}\" does not exist",
                policy::sanitize::one_line(&s.cwd.to_string_lossy(), 200)
            )));
        }
        let program_text = s.program.to_string_lossy().into_owned();
        if crate::winquote::is_batch_file(&program_text) {
            return Err(HostError::InvalidConfig(
                "a batch file cannot be the app's own server".into(),
            ));
        }
        let spec = SpawnSpec::new(s.program.clone(), s.cwd.clone())
            .args(s.args.iter().map(OsString::from))
            .env(s.env.clone())
            .stdin(StdioMode::Piped)
            .stdout(StdioMode::Piped)
            .stderr(StdioMode::Piped);
        Ok(Prepared {
            spec,
            raw_args: None,
            program: s.program.clone(),
            command: program_text,
            search: OsString::new(),
        })
    }

    /// Notes an exit nobody asked for in the log (a crash after Ready).
    fn watch_exit(&self, connection: &Arc<Connection>) {
        let Some(process) = &connection.process else {
            return;
        };
        let mut exit = process.exit_watch();
        let stopping = connection.stopping.clone();
        let log = self.log.clone();
        connection.add_task(tokio::spawn(async move {
            let info = match exit.wait_for(Option::is_some).await {
                Ok(info) => info.clone(),
                Err(_) => return,
            };
            if let Some(info) = info
                && !stopping.load(Ordering::SeqCst)
            {
                log.note(&format!("the server {}", info.description)).await;
            }
        }));
    }

    /// Calls `name`, which must be in `allow`; progress only keeps the call alive. A call
    /// stopped by `cancel` or past `timeout` comes back as an error result; use
    /// [`Self::call_outcome`] to tell those apart.
    pub async fn call(
        &self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<ToolOutput, HostError> {
        Ok(
            match self
                .call_outcome(allow, name, args, timeout, cancel, progress)
                .await?
            {
                CallOutcome::Done(output) => output,
                CallOutcome::Cancelled { .. } => ToolOutput::error(CANCELLED_TEXT),
                CallOutcome::TimedOut { .. } => ToolOutput::error(format!(
                    "the tool did not finish within {} seconds and was stopped",
                    timeout.as_secs()
                )),
            },
        )
    }

    /// [`Self::call`], saying how the call ended.
    ///
    /// - A name outside `allow` is refused with [`HostError::NotFound`] before anything is
    ///   sent; so is any call while the server is not running ([`HostError::NotReady`]).
    /// - `progress`, when given, receives an empty string (at most once a second) each time
    ///   the server reports progress: a sign of life to refresh the caller's own line. The
    ///   server's progress text is never passed on.
    /// - On cancel or timeout the server gets [`CANCEL_GRACE`] to answer, else it is stopped
    ///   (`stopped: true`); a server stopped this way is not running until the next
    ///   [`Self::start`].
    /// - A server that dies during the call gives [`HostError::NotReady`].
    pub async fn call_outcome(
        &self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<CallOutcome, HostError> {
        if !allow.contains(name) {
            return Err(HostError::NotFound(format!(
                "there is no tool named {}",
                policy::sanitize::one_line(name, 80)
            )));
        }
        if cancel.is_cancelled() {
            return Ok(CallOutcome::Cancelled { stopped: false });
        }
        let connection = self
            .live()
            .ok_or_else(|| HostError::NotReady("it is stopped".into()))?;
        let alive = Arc::new(Notify::new());
        let signal = alive.clone();
        // The host's handler formats the server's progress into a line; it is dropped here.
        let tick: ProgressFn = Arc::new(move |_server_text: String| signal.notify_one());
        let _call = connection.shared.begin_call(Some(tick));
        let request = ClientRequest::CallToolRequest(CallToolRequest::new(
            CallToolRequestParams::new(name.to_owned()).with_arguments(args),
        ));
        let peer = connection.service.peer().clone();
        let handle = peer
            .send_cancellable_request(request, PeerRequestOptions::no_options())
            .await
            .map_err(|_| HostError::NotReady("the connection to the server was lost".into()))?;
        let id = handle.id.clone();
        let mut rx = handle.rx;
        let total = timeout.saturating_mul(MAX_CALL_FACTOR);
        let idle = tokio::time::sleep(timeout);
        let hard = tokio::time::sleep(total);
        tokio::pin!(idle, hard);
        let mut last_tick: Option<Instant> = None;
        let end = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => break End::Cancelled,
                answer = &mut rx => break End::Answer(Box::new(answer)),
                _ = alive.notified() => {
                    idle.as_mut().reset(Instant::now() + timeout);
                    if let Some(progress) = &progress
                        && last_tick.is_none_or(|t| t.elapsed() >= TICK_EVERY)
                    {
                        progress(String::new());
                        last_tick = Some(Instant::now());
                    }
                }
                _ = &mut idle => break End::TimedOut,
                _ = &mut hard => break End::TimedOut,
            }
        };
        match end {
            End::Answer(answer) => match *answer {
                Ok(Ok(result)) => Ok(CallOutcome::Done(self.map_result(name, result))),
                Ok(Err(error)) if !connection::is_connection_lost(&error) => {
                    Ok(CallOutcome::Done(ToolOutput::error(format!(
                        "{} could not run {}: {}",
                        self.spec.name,
                        policy::sanitize::one_line(name, 80),
                        connection::describe_service_error(&error)
                    ))))
                }
                _ => Err(HostError::NotReady(
                    "the server stopped during the call".into(),
                )),
            },
            End::Cancelled => {
                let stopped = self
                    .give_up(&connection, &peer, &id, &mut rx, "stopped by the user")
                    .await;
                Ok(CallOutcome::Cancelled { stopped })
            }
            End::TimedOut => {
                let stopped = self
                    .give_up(&connection, &peer, &id, &mut rx, "the call took too long")
                    .await;
                Ok(CallOutcome::TimedOut { stopped })
            }
        }
    }

    fn map_result(&self, name: &str, result: ServerResult) -> ToolOutput {
        match result {
            ServerResult::CallToolResult(result) => map_call_result(result),
            ServerResult::InputRequiredResult(_) => ToolOutput::error(format!(
                "{} asked for more input during {}, which this app does not support",
                self.spec.name,
                policy::sanitize::one_line(name, 80)
            )),
            _ => ToolOutput::error(format!(
                "{} answered {} with something this app does not understand",
                self.spec.name,
                policy::sanitize::one_line(name, 80)
            )),
        }
    }

    /// Tells the server the call `id` is cancelled and waits [`CANCEL_GRACE`] for its
    /// answer. Returns whether the server had to be stopped.
    async fn give_up(
        &self,
        connection: &Arc<Connection>,
        peer: &Peer<RoleClient>,
        id: &RequestId,
        rx: &mut oneshot::Receiver<Result<ServerResult, ServiceError>>,
        reason: &str,
    ) -> bool {
        let deadline = Instant::now() + CANCEL_GRACE;
        // Sent as a plain notification: see the module documentation.
        let notification = ClientNotification::CustomNotification(CustomNotification::new(
            "notifications/cancelled",
            Some(json!({ "requestId": id, "reason": reason })),
        ));
        let _ = tokio::time::timeout_at(deadline, peer.send_notification(notification)).await;
        let answered = matches!(
            tokio::time::timeout_at(deadline, rx).await,
            Ok(Ok(Ok(_) | Err(ServiceError::McpError(_))))
        );
        if answered {
            self.log.note("the server ended the stopped call").await;
            return false;
        }
        self.log
            .note(&format!(
                "the server did not end the stopped call within {} seconds; stopping it",
                CANCEL_GRACE.as_secs()
            ))
            .await;
        self.stop_connection(connection, STOP_AFTER_CANCEL).await;
        true
    }

    /// Closes standard input, waits up to `grace`, then ends the whole process tree.
    pub async fn stop(&self, grace: Duration) {
        self.stop_with(grace).await;
    }

    /// [`Self::stop`], saying how the process ended.
    pub async fn stop_with(&self, grace: Duration) -> StopOutcome {
        self.stops.fetch_add(1, Ordering::SeqCst);
        lock(&self.starting).cancel();
        let connection = lock(&self.current).take();
        match connection {
            Some(connection) => self.close(&connection, grace).await,
            None => StopOutcome::NotRunning,
        }
    }

    /// Stops `connection` if it is still the current one (a newer start is left alone).
    async fn stop_connection(&self, connection: &Arc<Connection>, grace: Duration) {
        let current = {
            let mut current = lock(&self.current);
            if current.as_ref().is_some_and(|c| Arc::ptr_eq(c, connection)) {
                current.take()
            } else {
                None
            }
        };
        if let Some(connection) = current {
            self.close(&connection, grace).await;
        }
    }

    async fn close(&self, connection: &Arc<Connection>, grace: Duration) -> StopOutcome {
        let outcome = match connection.close_with_grace(grace).await {
            Some(Closed::Exited(exit)) => StopOutcome::Exited(exit.description),
            Some(Closed::Killed) => StopOutcome::Killed,
            None => StopOutcome::NotRunning,
        };
        let line = match &outcome {
            StopOutcome::Exited(how) => format!("stopped: the server {how}"),
            StopOutcome::Killed => format!(
                "stopped: the server was still running after {} seconds, so it was ended",
                grace.as_secs_f32()
            ),
            StopOutcome::NotRunning => "stopped".to_owned(),
        };
        self.log.note(&line).await;
        outcome
    }

    /// The last lines the server wrote to stderr, with the app's own notes (`--- … ---`).
    pub fn stderr_tail(&self) -> Vec<String> {
        self.log.tail()
    }

    /// Whether the process runs.
    pub fn is_running(&self) -> bool {
        self.live().is_some()
    }

    /// Whether the process ended without being asked to (a crash after Ready).
    pub fn ended_unexpectedly(&self) -> bool {
        lock(&self.current).as_ref().is_some_and(|c| {
            !c.stopping.load(Ordering::SeqCst)
                && c.process
                    .as_ref()
                    .is_some_and(|p| p.exit_watch().borrow().is_some())
        })
    }

    /// The process id (on Unix also its process group), while it runs.
    pub fn pid(&self) -> Option<u32> {
        self.live()?.process.as_ref()?.pid()
    }

    /// The current connection if its process still runs.
    fn live(&self) -> Option<Arc<Connection>> {
        let current = lock(&self.current);
        let connection = current.as_ref()?;
        let exited = connection
            .process
            .as_ref()
            .is_none_or(|p| p.exit_watch().borrow().is_some());
        (!exited && !connection.stopping.load(Ordering::SeqCst)).then(|| connection.clone())
    }

    fn tail_for_errors(&self) -> String {
        let lines: Vec<String> = self
            .log
            .tail()
            .into_iter()
            .filter(|l| !l.starts_with("--- "))
            .collect();
        if lines.is_empty() {
            return String::new();
        }
        let start = lines.len().saturating_sub(TAIL_IN_ERRORS);
        format!(
            "\n\nLast output from the server:\n{}",
            lines[start..].join("\n")
        )
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(program: PathBuf) -> ManagedSpec {
        ManagedSpec {
            name: "reverse".into(),
            program,
            args: vec!["rea.mjs".into(), "mcp".into()],
            env: runtime::EnvSpec::default(),
            cwd: std::env::temp_dir(),
            log_file: None,
            start_timeout: Duration::from_secs(60),
        }
    }

    #[tokio::test]
    async fn a_name_outside_the_allow_list_is_refused() {
        let server = ManagedServer::new(spec(PathBuf::from("/nonexistent/node")));
        let allow = ToolAllowList::new(["open_binary"]);
        let e = server
            .call(
                &allow,
                "start_gdb_session",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(e, HostError::NotFound(_)), "{e}");
        assert!(allow.contains("open_binary") && allow.len() == 1);
        // An allowed name on a server that is not running is not sent anywhere.
        let e = server
            .call_outcome(
                &allow,
                "open_binary",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(e, HostError::NotReady(_)), "{e}");
        assert!(!server.is_running() && !server.ended_unexpectedly());
        assert_eq!(server.pid(), None);
        assert_eq!(
            server.stop_with(Duration::ZERO).await,
            StopOutcome::NotRunning
        );
    }

    #[tokio::test]
    async fn relative_programs_and_missing_folders_are_refused_before_spawning() {
        let server = ManagedServer::new(spec(PathBuf::from("node")));
        assert!(matches!(
            server.start().await,
            Err(HostError::InvalidConfig(m)) if m.contains("absolute")
        ));
        let mut s = spec(std::env::temp_dir().join("node"));
        s.cwd = std::env::temp_dir().join("harnessui-no-such-folder-for-managed-tests");
        assert!(matches!(
            ManagedServer::new(s).start().await,
            Err(HostError::InvalidConfig(m)) if m.contains("does not exist")
        ));
        let s = spec(std::env::temp_dir().join("server.cmd"));
        assert!(matches!(
            ManagedServer::new(s).start().await,
            Err(HostError::InvalidConfig(m)) if m.contains("batch")
        ));
    }

    #[tokio::test]
    async fn a_call_cancelled_before_it_starts_sends_nothing() {
        let server = ManagedServer::new(spec(PathBuf::from("/nonexistent/node")));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let allow = ToolAllowList::new(["open_binary"]);
        let out = server
            .call_outcome(
                &allow,
                "open_binary",
                Map::new(),
                Duration::from_secs(1),
                &cancel,
                None,
            )
            .await
            .unwrap();
        assert_eq!(out, CallOutcome::Cancelled { stopped: false });
    }
}
