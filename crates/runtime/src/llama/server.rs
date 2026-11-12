//! The llama-server lifecycle (ARCHITECTURE.md 7.3 and 7.4).
//!
//! ```text
//! Stopped → Starting (progress) → Ready → Stopping → Stopped
//!               ↓ failure                ↓ crash
//!            Failed (reason)        Restarting (max 3 in 5 min) → Ready | Failed
//! ```
//!
//! [`start`] spawns one server in managed-child mode and returns once it answers `/health`
//! and an authenticated `/props`. The [`Supervisor`] adds the restart and step-down policy
//! on top.

mod supervisor;

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use reqwest::StatusCode;
use serde::Serialize;
use tokio::process::ChildStdin;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

pub use supervisor::{
    Candidate, LocalStarter, RestartPolicy, Starter, Supervised, Supervisor, SupervisorExit,
    step_down,
};

use super::backend::{Backend, BackendKind};
use super::failure::{LoadFailure, classify};
use super::launch::{LaunchConfig, REDACTED, build_args, build_env_on};
use super::lines::{LineReader, Tail};
use super::props::Props;
use super::protocol::{ChildState, LogEvent, LogInfo, parse_child_line};
use super::stage::LoadStage;
use crate::spawn::{self, ManagedChild, SpawnSpec, StdioMode};

/// Lines of stderr kept for error reports.
pub const STDERR_TAIL_LINES: usize = 400;

/// How often `/health` is polled while the model loads.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Per-request timeout for the loopback health and props calls.
const LOOPBACK_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

/// How long `stop` waits after `SIGTERM` before killing the tree (Unix).
#[cfg(unix)]
const GRACEFUL_STOP: Duration = Duration::from_secs(3);

/// How long to wait for the output readers to drain after the process exited.
const READER_DRAIN: Duration = Duration::from_secs(2);

/// Largest `/props` body accepted (it carries the chat template).
const MAX_PROPS_BYTES: usize = 8 * 1024 * 1024;

/// What to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartSpec {
    /// The backend whose runtime directory and executable to use.
    pub backend: Backend,
    /// Model, context, device, port and key.
    pub launch: LaunchConfig,
    /// How long the model may take to load before the start is abandoned. Scale it with
    /// the model size.
    pub load_timeout: Duration,
}

/// The lifecycle state shown in the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", content = "failure", rename_all = "snake_case")]
pub enum ServerState {
    /// The process was started and the model is loading.
    Starting,
    /// The server answers requests.
    Ready,
    /// A stop was requested.
    Stopping,
    /// No server is running.
    Stopped,
    /// The start failed, or the restart policy gave up.
    Failed(LoadFailure),
}

/// Everything the runtime reports while a server starts, runs and stops. The receiver must
/// keep draining the channel: every stderr line is forwarded as [`ServerEvent::Log`].
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// A lifecycle transition.
    State(ServerState),
    /// Load progress from the managed-child protocol.
    Progress {
        /// The stage in progress (`text_model`, `mmproj_model`), when reported.
        stage: Option<String>,
        /// Progress of that stage, 0 to 1, when reported.
        value: Option<f32>,
    },
    /// One line of the server's output.
    Log(String),
    /// The server reported how many layers went to the GPU. `layers == 0` with a GPU
    /// backend means it silently ran on the CPU.
    Offloaded {
        /// Layers on the GPU.
        layers: u32,
        /// Layers in the model.
        total: u32,
    },
    /// The process exited.
    Exited {
        /// Exit code, or `None` when killed by a signal.
        code: Option<i32>,
    },
    /// The supervisor is restarting the server after an unexpected exit.
    Restarting {
        /// Which restart this is within the policy window (1-based).
        attempt: u32,
        /// How many restarts the window allows.
        max: u32,
    },
    /// The supervisor moved to the next backend down; the UI says which and why.
    SteppedDown {
        /// The backend that failed.
        from: BackendKind,
        /// The backend now in use.
        to: BackendKind,
        /// Why the previous backend failed.
        reason: LoadFailure,
    },
    /// A typed stage or transition that has no event of its own (0.0.5: the retries the
    /// supervisor adds, WP1a). The app shows it as the current [`LoadStage`].
    Stage(LoadStage),
}

/// Why [`start`] failed. Every variant can report a classified [`LoadFailure`] and the
/// stderr tail for **Copy details**.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The launch settings cannot be used (for example a path that is not valid Unicode).
    #[error("the launch settings are invalid: {0}")]
    InvalidConfig(String),
    /// The process could not be created.
    #[error("could not start llama-server: {0}")]
    Spawn(#[source] io::Error),
    /// The process exited before it was ready.
    #[error("llama-server exited while loading (exit code {exit_code:?}): {failure}")]
    Exited {
        /// The classified reason.
        failure: LoadFailure,
        /// Exit code, or `None` when killed by a signal.
        exit_code: Option<i32>,
        /// The last lines of stderr.
        stderr_tail: Vec<String>,
    },
    /// The model did not finish loading in time; the process tree was killed.
    #[error("llama-server did not become ready within {} s", after.as_secs())]
    Timeout {
        /// The timeout that elapsed.
        after: Duration,
        /// The reason classified from stderr so far (usually unknown).
        failure: LoadFailure,
        /// The last lines of stderr.
        stderr_tail: Vec<String>,
    },
    /// Another process answers on the chosen port (a wrong-key reply or a different model).
    #[error("another process answers on port {port}")]
    PortOwnedByOther {
        /// The port that was taken.
        port: u16,
        /// The last lines of stderr.
        stderr_tail: Vec<String>,
    },
    /// `/props` failed or returned something unusable.
    #[error("llama-server's /props reply was unusable: {message}")]
    Props {
        /// What went wrong.
        message: String,
        /// The last lines of stderr.
        stderr_tail: Vec<String>,
    },
}

impl StartError {
    /// The classified reason, for the UI message.
    pub fn failure(&self) -> LoadFailure {
        match self {
            Self::Exited { failure, .. } | Self::Timeout { failure, .. } => failure.clone(),
            Self::PortOwnedByOther { .. } => LoadFailure::PortInUse,
            Self::InvalidConfig(_) | Self::Spawn(_) | Self::Props { .. } => LoadFailure::Unknown,
        }
    }

    /// The last lines of stderr, when the process ran.
    pub fn stderr_tail(&self) -> &[String] {
        match self {
            Self::Exited { stderr_tail, .. }
            | Self::Timeout { stderr_tail, .. }
            | Self::PortOwnedByOther { stderr_tail, .. }
            | Self::Props { stderr_tail, .. } => stderr_tail,
            Self::InvalidConfig(_) | Self::Spawn(_) => &[],
        }
    }
}

/// A builder for the HTTP client used to talk to llama-server: no proxy (a system proxy
/// would otherwise receive local chat and its key) and no redirects (ARCHITECTURE.md 13.6).
pub fn loopback_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
}

/// Starts llama-server for `spec` and waits until it is ready.
///
/// The process runs with the backend's runtime directory as working directory, the
/// scrubbed environment from [`build_env`] and stdin kept open (managed-child mode exits
/// when stdin closes). Stdout is parsed for progress, stderr is kept in a ring buffer of
/// [`STDERR_TAIL_LINES`] lines and forwarded as events. Readiness is `/health` returning
/// 200, raced against process exit and `spec.load_timeout`, followed by an authenticated
/// `/props`. A wrong-key reply means another process owns the port.
///
/// Emits [`ServerState::Starting`], then [`ServerState::Ready`] or
/// [`ServerState::Failed`].
pub async fn start(
    spec: StartSpec,
    events: UnboundedSender<ServerEvent>,
) -> Result<RunningServer, StartError> {
    let _ = events.send(ServerEvent::State(ServerState::Starting));
    let result = start_inner(spec, events.clone()).await;
    match &result {
        Ok(_) => {
            let _ = events.send(ServerEvent::State(ServerState::Ready));
        }
        Err(error) => {
            let _ = events.send(ServerEvent::State(ServerState::Failed(error.failure())));
        }
    }
    result
}

enum LoadOutcome {
    Exited(Option<i32>),
    TimedOut,
    Healthy,
}

async fn start_inner(
    spec: StartSpec,
    events: UnboundedSender<ServerEvent>,
) -> Result<RunningServer, StartError> {
    validate(&spec.launch)?;
    let client = loopback_client_builder()
        .timeout(LOOPBACK_REQUEST_TIMEOUT)
        .build()
        .map_err(|error| {
            StartError::InvalidConfig(format!(
                "could not create the loopback HTTP client: {error}"
            ))
        })?;

    let launch = &spec.launch;
    let process = SpawnSpec::new(&spec.backend.server, &spec.backend.dir)
        .args(build_args(launch))
        .env(build_env_on(spec.backend.base_env(), launch))
        .stdin(StdioMode::Piped)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped);
    let mut child = spawn::spawn(process).map_err(StartError::Spawn)?;
    let stdin = child.take_stdin();
    let tail = Arc::new(Mutex::new(Tail::new(STDERR_TAIL_LINES)));
    let log = Arc::new(Mutex::new(LogInfo::default()));
    let mut readers = Vec::with_capacity(2);
    if let Some(stdout) = child.take_stdout() {
        readers.push(tokio::spawn(read_stdout(stdout, events.clone())));
    }
    if let Some(stderr) = child.take_stderr() {
        readers.push(tokio::spawn(read_stderr(
            stderr,
            tail.clone(),
            log.clone(),
            events.clone(),
        )));
    }

    let mut server = RunningServer {
        child,
        stdin,
        port: launch.port,
        base_url: format!("http://127.0.0.1:{}", launch.port),
        api_key: launch.api_key.clone(),
        props: Props::default(),
        backend: spec.backend.clone(),
        tail,
        log,
        readers,
        events,
        exit: None,
    };

    let outcome = {
        let deadline = tokio::time::sleep(spec.load_timeout);
        tokio::pin!(deadline);
        tokio::select! {
            status = server.child.wait() => LoadOutcome::Exited(status.ok().and_then(|s| s.code())),
            () = &mut deadline => LoadOutcome::TimedOut,
            () = wait_healthy(&client, &server.base_url) => LoadOutcome::Healthy,
        }
    };

    match outcome {
        LoadOutcome::Exited(code) => {
            let stderr_tail = server.finish_after_exit(code).await;
            return Err(StartError::Exited {
                failure: classify(&stderr_tail, code),
                exit_code: code,
                stderr_tail,
            });
        }
        LoadOutcome::TimedOut => {
            let stderr_tail = server.kill_and_collect().await;
            return Err(StartError::Timeout {
                after: spec.load_timeout,
                failure: classify(&stderr_tail, None),
                stderr_tail,
            });
        }
        LoadOutcome::Healthy => {}
    }

    let props = match fetch_props(&client, &server.base_url, &server.api_key).await {
        Ok(props) => props,
        Err(PropsError::Unauthorized) => {
            let stderr_tail = server.kill_and_collect().await;
            return Err(StartError::PortOwnedByOther {
                port: server.port,
                stderr_tail,
            });
        }
        Err(PropsError::Other(message)) => {
            let stderr_tail = server.kill_and_collect().await;
            return Err(StartError::Props {
                message,
                stderr_tail,
            });
        }
    };

    // A server that answered with our key could still be a foreign one that has no key
    // set at all. Ours is alive and serves the model we asked for.
    if let Ok(Some(status)) = server.child.try_wait() {
        let code = status.code();
        let stderr_tail = server.finish_after_exit(code).await;
        return Err(StartError::Exited {
            failure: classify(&stderr_tail, code),
            exit_code: code,
            stderr_tail,
        });
    }
    if !same_model_file(&props.model_path, &launch.model) {
        let stderr_tail = server.kill_and_collect().await;
        return Err(StartError::PortOwnedByOther {
            port: server.port,
            stderr_tail,
        });
    }
    server.props = props;
    Ok(server)
}

fn validate(launch: &LaunchConfig) -> Result<(), StartError> {
    if launch.model.to_str().is_none() {
        return Err(StartError::InvalidConfig(
            "the model path is not valid Unicode".into(),
        ));
    }
    if launch.mmproj.as_ref().is_some_and(|p| p.to_str().is_none()) {
        return Err(StartError::InvalidConfig(
            "the projector path is not valid Unicode".into(),
        ));
    }
    if launch.port == 0 {
        return Err(StartError::InvalidConfig(
            "the port must be chosen by the app, not 0".into(),
        ));
    }
    if launch.api_key.is_empty()
        || launch
            .api_key
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(StartError::InvalidConfig(
            "the API key must be non-empty printable text".into(),
        ));
    }
    if launch.ctx == 0 {
        return Err(StartError::InvalidConfig(
            "the context length must be set explicitly".into(),
        ));
    }
    Ok(())
}

/// Compares the model the server reports with the one requested, by file name, so a
/// different spelling of the same path still matches.
fn same_model_file(reported: &str, requested: &Path) -> bool {
    let reported = Path::new(reported);
    reported == requested
        || (reported.file_name().is_some() && reported.file_name() == requested.file_name())
}

async fn wait_healthy(client: &reqwest::Client, base_url: &str) {
    let url = format!("{base_url}/health");
    loop {
        if let Ok(response) = client.get(&url).send().await
            && response.status() == StatusCode::OK
        {
            return;
        }
        tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
    }
}

enum PropsError {
    Unauthorized,
    Other(String),
}

async fn fetch_props(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
) -> Result<Props, PropsError> {
    let mut response = client
        .get(format!("{base_url}/props"))
        .bearer_auth(api_key)
        .send()
        .await
        .map_err(|error| PropsError::Other(error.to_string()))?;
    match response.status() {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => return Err(PropsError::Unauthorized),
        status if !status.is_success() => return Err(PropsError::Other(format!("HTTP {status}"))),
        _ => {}
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| PropsError::Other(error.to_string()))?
    {
        if body.len() + chunk.len() > MAX_PROPS_BYTES {
            return Err(PropsError::Other("the reply is too large".into()));
        }
        body.extend_from_slice(&chunk);
    }
    parse_props(&body).map_err(PropsError::Other)
}

/// Parses a `/props` body and checks the one number everything else budgets against.
fn parse_props(body: &[u8]) -> Result<Props, String> {
    let props: Props =
        serde_json::from_slice(body).map_err(|error| format!("unexpected JSON: {error}"))?;
    if props.n_ctx == 0 {
        // A missing or renamed `default_generation_settings.n_ctx` would otherwise give the
        // agent a context budget of zero.
        return Err("the reply has no allocated context size (n_ctx)".into());
    }
    Ok(props)
}

async fn read_stdout(stdout: tokio::process::ChildStdout, events: UnboundedSender<ServerEvent>) {
    let mut reader = LineReader::new(stdout);
    while let Some(line) = reader.next_line().await {
        let event = match parse_child_line(&line) {
            Some(ChildState::Loading {
                current, progress, ..
            }) => ServerEvent::Progress {
                stage: current,
                value: progress,
            },
            // Readiness is decided by /health and /props; the state line is not an event.
            Some(ChildState::Ready { .. }) => continue,
            Some(ChildState::Sleeping | ChildState::Unknown(_)) | None => ServerEvent::Log(line),
        };
        let _ = events.send(event);
    }
}

async fn read_stderr(
    stderr: tokio::process::ChildStderr,
    tail: Arc<Mutex<Tail>>,
    log: Arc<Mutex<LogInfo>>,
    events: UnboundedSender<ServerEvent>,
) {
    let mut reader = LineReader::new(stderr);
    while let Some(line) = reader.next_line().await {
        let event = log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .scan(&line);
        if let Some(LogEvent::Offloaded { layers, total }) = event {
            let _ = events.send(ServerEvent::Offloaded { layers, total });
        }
        tail.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(line.clone());
        let _ = events.send(ServerEvent::Log(line));
    }
}

/// A llama-server that answered `/health` and `/props`.
///
/// Dropping it kills the process tree. Use [`stop`](Self::stop) for an orderly shutdown.
/// `Debug` shows the API key as `<redacted>`.
pub struct RunningServer {
    child: ManagedChild,
    /// Kept open for the server's lifetime: managed-child mode exits when stdin closes.
    stdin: Option<ChildStdin>,
    port: u16,
    base_url: String,
    api_key: String,
    props: Props,
    backend: Backend,
    tail: Arc<Mutex<Tail>>,
    log: Arc<Mutex<LogInfo>>,
    readers: Vec<JoinHandle<()>>,
    events: UnboundedSender<ServerEvent>,
    /// Set once the exit was observed: the exit code (or `None` for a signal).
    exit: Option<Option<i32>>,
}

impl RunningServer {
    /// `http://127.0.0.1:<port>`.
    pub fn base_url(&self) -> String {
        self.base_url.clone()
    }

    /// The port the server listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The per-launch API key, to send as `Authorization: Bearer <key>`.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// What `/props` reported after the start: the allocated context, template
    /// capabilities and modalities.
    pub fn props(&self) -> &Props {
        &self.props
    }

    /// The backend the server runs on.
    pub fn backend(&self) -> &Backend {
        &self.backend
    }

    /// The server's process id.
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// What the log has said so far (model path, offload counts).
    pub fn log_info(&self) -> LogInfo {
        self.log
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The last [`STDERR_TAIL_LINES`] lines of stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        self.tail
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .to_vec()
    }

    /// Waits until the process exits on its own and returns its exit code (`None` when it
    /// was killed by a signal). Emits [`ServerEvent::Exited`] once. Returns at once if the
    /// exit was already observed.
    pub async fn wait_exit(&mut self) -> Option<i32> {
        if let Some(code) = self.exit {
            return code;
        }
        let code = self
            .child
            .wait()
            .await
            .ok()
            .and_then(|status| status.code());
        self.finish_after_exit(code).await;
        code
    }

    /// Stops the server and waits for it to exit. On Unix the process group gets `SIGTERM`
    /// and up to three seconds before the tree is killed; on Windows the tree is killed at
    /// once (the server keeps no state worth a graceful stop). Emits
    /// [`ServerState::Stopping`], [`ServerEvent::Exited`] and [`ServerState::Stopped`].
    pub async fn stop(mut self) {
        let _ = self.events.send(ServerEvent::State(ServerState::Stopping));
        if self.exit.is_none() {
            #[cfg(unix)]
            {
                let terminated = self.child.signal(libc::SIGTERM).is_ok()
                    && tokio::time::timeout(GRACEFUL_STOP, self.child.wait())
                        .await
                        .is_ok();
                if !terminated {
                    let _ = self.child.kill().await;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = self.child.kill().await;
            }
            let code = self
                .child
                .wait()
                .await
                .ok()
                .and_then(|status| status.code());
            self.finish_after_exit(code).await;
        }
        let _ = self.events.send(ServerEvent::State(ServerState::Stopped));
    }

    /// The exit was observed: close stdin, let the readers drain, emit the event once and
    /// return the stderr tail.
    async fn finish_after_exit(&mut self, code: Option<i32>) -> Vec<String> {
        if self.exit.is_none() {
            self.exit = Some(code);
            self.stdin = None;
            self.drain_readers().await;
            let _ = self.events.send(ServerEvent::Exited { code });
        }
        self.stderr_tail()
    }

    async fn kill_and_collect(&mut self) -> Vec<String> {
        let _ = self.child.kill().await;
        let code = self
            .child
            .wait()
            .await
            .ok()
            .and_then(|status| status.code());
        self.finish_after_exit(code).await
    }

    async fn drain_readers(&mut self) {
        for reader in &mut self.readers {
            if tokio::time::timeout(READER_DRAIN, &mut *reader)
                .await
                .is_err()
            {
                reader.abort();
            }
        }
        self.readers.clear();
    }
}

impl std::fmt::Debug for RunningServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunningServer")
            .field("pid", &self.child.id())
            .field("base_url", &self.base_url)
            .field("api_key", &REDACTED)
            .field("backend", &self.backend.kind)
            .field("n_ctx", &self.props.n_ctx)
            .field("exit", &self.exit)
            .finish_non_exhaustive()
    }
}

impl Drop for RunningServer {
    fn drop(&mut self) {
        for reader in &self.readers {
            reader.abort();
        }
        // `child` kills the process tree when it drops.
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::llama::backend::BackendSource;
    use crate::llama::launch::GpuUse;

    fn launch() -> LaunchConfig {
        LaunchConfig {
            model: PathBuf::from("/m/model.gguf"),
            mmproj: None,
            ctx: 2048,
            device: None,
            gpu: GpuUse::Off,
            cache_ram_mib: 256,
            reasoning_budget: None,
            port: 40000,
            api_key: "key".into(),
            hip_visible_devices: None,
            ..LaunchConfig::default()
        }
    }

    #[test]
    fn validate_rejects_unusable_settings() {
        assert!(validate(&launch()).is_ok());
        assert!(matches!(
            validate(&LaunchConfig {
                port: 0,
                ..launch()
            }),
            Err(StartError::InvalidConfig(_))
        ));
        assert!(matches!(
            validate(&LaunchConfig { ctx: 0, ..launch() }),
            Err(StartError::InvalidConfig(_))
        ));
        assert!(matches!(
            validate(&LaunchConfig {
                api_key: String::new(),
                ..launch()
            }),
            Err(StartError::InvalidConfig(_))
        ));
        assert!(matches!(
            validate(&LaunchConfig {
                api_key: "a b".into(),
                ..launch()
            }),
            Err(StartError::InvalidConfig(_))
        ));
    }

    #[test]
    fn props_without_a_context_size_are_rejected() {
        let fixture = include_bytes!("../../tests/fixtures/props.json");
        assert_eq!(parse_props(fixture).expect("fixture").n_ctx, 4096);
        assert!(parse_props(b"{}").is_err());
        assert!(parse_props(br#"{"default_generation_settings":{"n_ctx":0}}"#).is_err());
        assert!(parse_props(b"not json").is_err());
    }

    #[test]
    fn model_file_comparison() {
        assert!(same_model_file("/m/model.gguf", Path::new("/m/model.gguf")));
        assert!(same_model_file("model.gguf", Path::new("/m/model.gguf")));
        assert!(!same_model_file(
            "/other/x.gguf",
            Path::new("/m/model.gguf")
        ));
        assert!(!same_model_file("", Path::new("/m/model.gguf")));
    }

    #[test]
    fn start_error_reports_failure_and_tail() {
        let tail = vec!["line".to_owned()];
        let exited = StartError::Exited {
            failure: LoadFailure::OutOfMemory,
            exit_code: Some(1),
            stderr_tail: tail.clone(),
        };
        assert_eq!(exited.failure(), LoadFailure::OutOfMemory);
        assert_eq!(exited.stderr_tail(), tail.as_slice());
        let port = StartError::PortOwnedByOther {
            port: 1,
            stderr_tail: vec![],
        };
        assert_eq!(port.failure(), LoadFailure::PortInUse);
        let spawn = StartError::Spawn(io::Error::from(io::ErrorKind::NotFound));
        assert_eq!(spawn.failure(), LoadFailure::Unknown);
        assert!(spawn.stderr_tail().is_empty());
        assert!(
            spawn
                .to_string()
                .starts_with("could not start llama-server")
        );
    }

    #[tokio::test]
    async fn missing_executable_is_a_spawn_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let backend = Backend {
            kind: BackendKind::Cpu,
            dir: dir.path().to_path_buf(),
            server: dir.path().join("no-such-llama-server"),
            fit_params: None,
            build: "test".into(),
            source: BackendSource::DevOverride,
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let spec = StartSpec {
            backend,
            launch: launch(),
            load_timeout: Duration::from_secs(5),
        };
        let error = start(spec, tx).await.expect_err("must fail");
        assert!(matches!(error, StartError::Spawn(_)), "{error:?}");
        assert_eq!(
            rx.recv().await,
            Some(ServerEvent::State(ServerState::Starting))
        );
        assert_eq!(
            rx.recv().await,
            Some(ServerEvent::State(ServerState::Failed(
                LoadFailure::Unknown
            )))
        );
    }

    #[test]
    fn events_serialise_with_a_type_tag() {
        let event = ServerEvent::SteppedDown {
            from: BackendKind::Rocm,
            to: BackendKind::Vulkan,
            reason: LoadFailure::OutOfMemory,
        };
        let json = serde_json::to_value(&event).expect("json");
        assert_eq!(json["type"], "stepped_down");
        assert_eq!(json["data"]["from"], "rocm");
        let state = serde_json::to_value(ServerEvent::State(ServerState::Ready)).expect("json");
        assert_eq!(state["data"]["state"], "ready");
    }
}
