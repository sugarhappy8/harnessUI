//! One connection to an external server: the rmcp client, the handshake routine and tool
//! listing (ARCHITECTURE.md 9.2 "Handshake", "Calls", "Server-initiated requests").
//!
//! **Handshake.** The SDK's automatic mode fails in two cases (servers on older SDKs exit
//! when the first message is not `initialize`, and a server slower than 10 s to start
//! breaks its fallback), so the host owns the routine: it starts with the legacy
//! `initialize` handshake pinned to protocol 2025-11-25, which every current SDK answers.
//! If the server rejects that with a JSON-RPC error, the server is restarted (or, for a
//! remote server, a new transport made) and connected with the 2026-07-28 `server/discover`
//! lifecycle. A remembered [`Lifecycle`] goes first instead, with the same fallback. Each
//! attempt has the start-up timeout, because the SDK has none.
//!
//! **Server-initiated requests.** The client advertises no capabilities and keeps rmcp's
//! defaults: sampling is refused, roots is an empty list, elicitation is declined. On
//! 2026-07-28 connections these arrive as `input_required` rounds, which
//! `RunningService::call_tool` drives through the same handler.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use http::{HeaderName, HeaderValue};
// Logging notifications are deprecated by SEP-2577 but servers still send them.
#[allow(deprecated)]
use rmcp::model::LoggingMessageNotificationParam;
use rmcp::model::{
    ClientCapabilities, ClientConfig, ErrorCode, Implementation, PaginatedRequestParams,
    ProgressNotificationParam, ProtocolVersion, SubscriptionFilter, Tool,
};
use rmcp::service::{ClientInitializeError, NotificationContext, RunningService};
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient, ServiceError};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use crate::builtin::ProgressFn;
use crate::config::redact_url;
use crate::error::HostError;
use crate::stderr::StderrLog;
use crate::stdio::{self, ChildProcess, ExitInfo, Prepared};

/// Most tools accepted from one server.
pub const MAX_TOOLS_PER_SERVER: usize = 500;

/// Most pages of `tools/list` followed.
const MAX_TOOL_PAGES: usize = 50;

/// Most bytes of tool definitions (serialised) accepted from one server; the SDK accepts
/// up to 16 MiB per page.
const MAX_TOOL_LIST_BYTES: usize = 16 * 1024 * 1024;

/// How long a stopping server gets to exit after its stdin closes, before its tree is
/// killed.
pub(crate) const STOP_GRACE: Duration = Duration::from_secs(1);

/// Stderr lines quoted in a start-up error.
const TAIL_IN_ERRORS: usize = 10;

/// Which lifecycle a server speaks. Remember it per server and pass it back to
/// [`crate::Host::add_external_with_lifecycle`] to skip a failed first attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Lifecycle {
    /// The `initialize` handshake, pinned to protocol 2025-11-25.
    Legacy,
    /// The 2026-07-28 `server/discover` lifecycle with per-request metadata.
    Modern,
}

impl Lifecycle {
    fn other(self) -> Self {
        match self {
            Self::Legacy => Self::Modern,
            Self::Modern => Self::Legacy,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Legacy => "initialize (2025-11-25)",
            Self::Modern => "2026-07-28",
        }
    }
}

/// State shared between a connection's handler (rmcp's task) and the host.
pub(crate) struct ConnShared {
    /// Signalled when the server says its tool list changed.
    pub refresh: Notify,
    calls: Mutex<Calls>,
    log: Arc<StderrLog>,
}

/// The calls in flight and where their progress goes. One lock covers both, so a
/// progress callback is never left in place while a second call is running.
#[derive(Default)]
struct Calls {
    in_flight: usize,
    progress: Option<ProgressFn>,
}

impl ConnShared {
    pub(crate) fn new(log: Arc<StderrLog>) -> Arc<Self> {
        Arc::new(Self {
            refresh: Notify::new(),
            calls: Mutex::new(Calls::default()),
            log,
        })
    }

    fn calls(&self) -> std::sync::MutexGuard<'_, Calls> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Marks a call as running. Progress notifications go to `progress` while it is the
    /// only call in flight on this connection: the SDK's call helper does not expose the
    /// request's progress token, so with several calls they could not be told apart.
    pub(crate) fn begin_call(&self, progress: Option<ProgressFn>) -> CallGuard<'_> {
        let mut calls = self.calls();
        calls.in_flight += 1;
        calls.progress = if calls.in_flight == 1 { progress } else { None };
        CallGuard { shared: self }
    }

    /// Where progress goes now, if anywhere.
    fn progress(&self) -> Option<ProgressFn> {
        self.calls().progress.clone()
    }
}

/// Ends a call started with [`ConnShared::begin_call`].
pub(crate) struct CallGuard<'a> {
    shared: &'a ConnShared,
}

impl Drop for CallGuard<'_> {
    fn drop(&mut self) {
        let mut calls = self.shared.calls();
        calls.in_flight = calls.in_flight.saturating_sub(1);
        calls.progress = None;
    }
}

/// The rmcp client handler for one connection.
#[derive(Clone)]
pub(crate) struct HostClient {
    info: ClientConfig,
    shared: Arc<ConnShared>,
}

// Logging notifications are deprecated by SEP-2577 but still sent by servers; they are
// kept for the server's diagnostics view.
#[allow(deprecated)]
impl ClientHandler for HostClient {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        self.shared.refresh.notify_one();
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        if let Some(progress) = self.shared.progress() {
            let amount = match params.total {
                Some(total) if total > 0.0 => format!("{:.0}%", params.progress / total * 100.0),
                _ => format!("{}", params.progress),
            };
            let line = match params.message {
                Some(message) => {
                    format!("{} ({amount})", policy::sanitize::one_line(&message, 200))
                }
                None => amount,
            };
            progress(line);
        }
    }

    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let data = match &params.data {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        self.shared.log.push(format!(
            "[log {:?}] {}",
            params.level,
            policy::sanitize::one_line(&data, 2000)
        ));
    }
}

/// Where a server lives.
pub(crate) enum Target {
    /// A local process.
    Stdio(Prepared),
    /// A remote Streamable HTTP endpoint (URL and headers already resolved).
    Http {
        client: reqwest::Client,
        url: String,
        headers: HashMap<HeaderName, HeaderValue>,
    },
}

/// How a stopped server process ended ([`Connection::close_with_grace`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Closed {
    /// It ended by itself within the grace.
    Exited(ExitInfo),
    /// It was still running after the grace, so its tree was killed.
    Killed,
}

/// A live connection.
pub(crate) struct Connection {
    pub service: RunningService<RoleClient, HostClient>,
    pub shared: Arc<ConnShared>,
    pub process: Option<ChildProcess>,
    pub lifecycle: Lifecycle,
    pub protocol: String,
    /// Set once the host stops this connection on purpose, so the exit is not reported
    /// as a failure.
    pub stopping: Arc<AtomicBool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}

impl Connection {
    fn new(
        service: RunningService<RoleClient, HostClient>,
        shared: Arc<ConnShared>,
        process: Option<ChildProcess>,
        lifecycle: Lifecycle,
    ) -> Self {
        let protocol = service
            .peer()
            .peer_info()
            .map(|info| info.protocol_version.as_str().to_owned())
            .unwrap_or_default();
        Self {
            service,
            shared,
            process,
            lifecycle,
            protocol,
            stopping: Arc::new(AtomicBool::new(false)),
            tasks: Mutex::new(Vec::new()),
        }
    }

    /// Ties a background task to this connection; it is aborted when the connection goes.
    pub(crate) fn add_task(&self, task: JoinHandle<()>) {
        self.tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(task);
    }

    fn abort_tasks(&self) {
        for task in self
            .tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            task.abort();
        }
    }

    /// Whether the server offers tools at all.
    pub(crate) fn has_tools(&self) -> bool {
        self.service
            .peer()
            .peer_info()
            .is_none_or(|info| info.capabilities.tools.is_some())
    }

    /// Whether the server says it sends tool-list changes.
    pub(crate) fn announces_list_changes(&self) -> bool {
        self.service
            .peer()
            .peer_info()
            .and_then(|info| {
                info.capabilities
                    .tools
                    .as_ref()
                    .and_then(|t| t.list_changed)
            })
            .unwrap_or(false)
    }

    /// Stops the connection: closes the transport (for a process, its stdin), gives a
    /// process [`STOP_GRACE`] to exit and then kills its whole tree.
    pub(crate) async fn close(&self) {
        self.close_with_grace(STOP_GRACE).await;
    }

    /// [`Self::close`] with its own `grace` (rea's managed server gives 3 s, so rea can
    /// end the programs it started itself; item5-spec.md WP9b). Returns how the process
    /// ended, when there is one: on its own within the grace, or killed after it.
    pub(crate) async fn close_with_grace(&self, grace: Duration) -> Option<Closed> {
        self.stopping.store(true, Ordering::SeqCst);
        self.abort_tasks();
        self.service.cancellation_token().cancel();
        let process = self.process.as_ref()?;
        if let Some(exit) = process.wait_exit(grace).await {
            return Some(Closed::Exited(exit));
        }
        process.kill();
        let _ = process.wait_exit(Duration::from_secs(2)).await;
        Some(Closed::Killed)
    }

    /// Kills the connection at once (after a cancelled call, 4.2).
    pub(crate) fn kill_now(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.abort_tasks();
        self.service.cancellation_token().cancel();
        if let Some(process) = &self.process {
            process.kill();
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::SeqCst);
        self.abort_tasks();
        // The running service cancels itself on drop, and the process handle kills the
        // tree when it goes.
    }
}

/// Connects to `target`, trying `first` and then the other lifecycle when the server
/// rejects the first handshake. See the module documentation.
pub(crate) async fn connect(
    target: &Target,
    first: Lifecycle,
    client: &Implementation,
    log: &Arc<StderrLog>,
    start_timeout: Duration,
) -> Result<Connection, HostError> {
    match attempt(target, first, client, log, start_timeout).await {
        Ok(connection) => Ok(connection),
        Err(Attempt::Init(error, _)) if should_fall_back(first, &error) => {
            let other = first.other();
            let restart = if matches!(target, Target::Stdio(_)) {
                "restarting the server"
            } else {
                "reconnecting"
            };
            log.note(&format!(
                "the server did not accept the {} handshake ({}); {restart} with the {} lifecycle",
                first.label(),
                describe_init_error(&error),
                other.label()
            ))
            .await;
            attempt(target, other, client, log, start_timeout)
                .await
                .map_err(|e| e.into_host_error(log, start_timeout))
        }
        Err(e) => Err(e.into_host_error(log, start_timeout)),
    }
}

/// Whether an error from a `tried` handshake means the server speaks the other lifecycle.
fn should_fall_back(tried: Lifecycle, error: &ClientInitializeError) -> bool {
    match tried {
        // A modern-only server answers `initialize` with a JSON-RPC error.
        Lifecycle::Legacy => matches!(
            error,
            ClientInitializeError::JsonRpcError(_) | ClientInitializeError::ExpectedInitResult(_)
        ),
        // A legacy server answers `server/discover` with an error, or (rmcp 2.x and
        // older) exits because the first message was not `initialize`.
        Lifecycle::Modern => matches!(
            error,
            ClientInitializeError::JsonRpcError(_)
                | ClientInitializeError::ConnectionClosed(_)
                | ClientInitializeError::UncorrelatedErrorResponse { .. }
                | ClientInitializeError::ExpectedInitResult(_)
                | ClientInitializeError::ExpectedInitResponse(_)
        ),
    }
}

enum Attempt {
    Spawn(std::io::Error),
    Timeout,
    Init(Box<ClientInitializeError>, Option<ExitInfo>),
}

impl Attempt {
    fn into_host_error(self, log: &StderrLog, start_timeout: Duration) -> HostError {
        let tail = stderr_tail(log);
        match self {
            Self::Spawn(error) => HostError::Spawn(error.to_string()),
            Self::Timeout => HostError::Connect(format!(
                "the server did not finish starting within {} seconds{tail}",
                start_timeout.as_secs()
            )),
            Self::Init(error, _) if error.is_authorization_required() => HostError::AuthRequired,
            Self::Init(error, exit) => {
                let message = match exit {
                    Some(exit) => format!(
                        "the server {} before completing the handshake",
                        exit.description
                    ),
                    None => describe_init_error(&error),
                };
                HostError::Connect(format!("{message}{tail}"))
            }
        }
    }
}

/// The recent stderr lines for an error message, without the host's own notes.
fn stderr_tail(log: &StderrLog) -> String {
    let lines: Vec<String> = log
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

async fn attempt(
    target: &Target,
    lifecycle: Lifecycle,
    client: &Implementation,
    log: &Arc<StderrLog>,
    start_timeout: Duration,
) -> Result<Connection, Attempt> {
    let shared = ConnShared::new(log.clone());
    let handler = HostClient {
        // For the legacy handshake the version must be pinned: the default offers
        // 2026-07-28 in `initialize`, which some servers accept into a hybrid session.
        info: ClientConfig::new(ClientCapabilities::default(), client.clone())
            .with_protocol_version(ProtocolVersion::LATEST_WITH_INITIALIZE),
        shared: shared.clone(),
    };
    let mode = match lifecycle {
        Lifecycle::Legacy => ClientLifecycleMode::Initialize,
        Lifecycle::Modern => ClientLifecycleMode::Discover {
            preferred_versions: vec![ProtocolVersion::V_2026_07_28],
        },
    };
    match target {
        Target::Stdio(prepared) => {
            log.note(&format!(
                "starting {} ({} handshake)",
                prepared.program.display(),
                lifecycle.label()
            ))
            .await;
            let (process, stdout, stdin) =
                stdio::spawn(prepared, log.clone()).map_err(Attempt::Spawn)?;
            let transport = AsyncRwTransport::<RoleClient, _, _>::new(stdout, stdin);
            match tokio::time::timeout(start_timeout, handler.serve_with_lifecycle(transport, mode))
                .await
            {
                Ok(Ok(service)) => Ok(Connection::new(service, shared, Some(process), lifecycle)),
                Ok(Err(error)) => {
                    // Let a crashing server finish so its exit code and last words are kept.
                    let exit = process.wait_exit(Duration::from_millis(500)).await;
                    process.kill();
                    let exit = match exit {
                        Some(exit) => Some(exit),
                        None => {
                            let _ = process.wait_exit(Duration::from_secs(2)).await;
                            None
                        }
                    };
                    process.wait_stderr(Duration::from_millis(500)).await;
                    Err(Attempt::Init(Box::new(error), exit))
                }
                Err(_) => {
                    process.kill();
                    let _ = process.wait_exit(Duration::from_secs(2)).await;
                    process.wait_stderr(Duration::from_millis(500)).await;
                    Err(Attempt::Timeout)
                }
            }
        }
        Target::Http {
            client: http,
            url,
            headers,
        } => {
            log.note(&format!(
                "connecting to {} ({} handshake)",
                redact_url(url),
                lifecycle.label()
            ))
            .await;
            let config = StreamableHttpClientTransportConfig::with_uri(url.as_str())
                .custom_headers(headers.clone())
                .max_sse_event_size(crate::http::MAX_SSE_EVENT_BYTES);
            let transport = StreamableHttpClientTransport::with_client(http.clone(), config);
            match tokio::time::timeout(start_timeout, handler.serve_with_lifecycle(transport, mode))
                .await
            {
                Ok(Ok(service)) => Ok(Connection::new(service, shared, None, lifecycle)),
                Ok(Err(error)) => Err(Attempt::Init(Box::new(error), None)),
                Err(_) => Err(Attempt::Timeout),
            }
        }
    }
}

/// Lists every tool, following pages up to [`MAX_TOOL_PAGES`] and keeping at most
/// [`MAX_TOOLS_PER_SERVER`] tools and [`MAX_TOOL_LIST_BYTES`], each page with `timeout`.
pub(crate) async fn list_tools(
    connection: &Connection,
    timeout: Duration,
) -> Result<Vec<Tool>, String> {
    if !connection.has_tools() {
        return Ok(Vec::new());
    }
    let mut tools = Vec::new();
    let mut bytes = 0usize;
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_TOOL_PAGES {
        let params = PaginatedRequestParams::default().with_cursor(cursor.take());
        let page = tokio::time::timeout(timeout, connection.service.list_tools(Some(params)))
            .await
            .map_err(|_| {
                format!(
                    "listing tools timed out after {} seconds",
                    timeout.as_secs()
                )
            })?
            .map_err(|e| format!("could not list tools: {}", describe_service_error(&e)))?;
        for tool in page.tools {
            bytes += serde_json::to_vec(&tool).map_or(0, |v| v.len());
            if bytes > MAX_TOOL_LIST_BYTES {
                connection.shared.log.push(format!(
                    "--- the tool list is larger than {} MiB; only the first {} tools are used ---",
                    MAX_TOOL_LIST_BYTES / (1024 * 1024),
                    tools.len()
                ));
                return Ok(tools);
            }
            tools.push(tool);
        }
        if tools.len() >= MAX_TOOLS_PER_SERVER {
            tools.truncate(MAX_TOOLS_PER_SERVER);
            connection.shared.log.push(format!(
                "--- only the first {MAX_TOOLS_PER_SERVER} tools are used ---"
            ));
            break;
        }
        match page.next_cursor {
            Some(next) if !next.is_empty() => cursor = Some(next),
            _ => break,
        }
    }
    Ok(tools)
}

/// On 2026-07-28 connections, list-changed notifications only arrive on a
/// `subscriptions/listen` stream. Opens one and forwards each to the refresh signal.
pub(crate) fn listen_for_tool_changes(connection: &Connection) -> Option<JoinHandle<()>> {
    if connection.lifecycle != Lifecycle::Modern || !connection.announces_list_changes() {
        return None;
    }
    let peer = connection.service.peer().clone();
    let shared = connection.shared.clone();
    Some(tokio::spawn(async move {
        let filter = SubscriptionFilter::builder().tools_list_changed().build();
        let Ok(mut subscription) = peer.listen(filter).await else {
            return;
        };
        while let Ok(Some(_)) = subscription.next().await {
            shared.refresh.notify_one();
        }
    }))
}

/// A readable explanation of a handshake failure, with URLs redacted.
pub(crate) fn describe_init_error(error: &ClientInitializeError) -> String {
    let text = match error {
        ClientInitializeError::ConnectionClosed(_) => {
            "the server closed the connection before completing the handshake".to_owned()
        }
        ClientInitializeError::JsonRpcError(data) => {
            format!("the server rejected the handshake: {}", data.message)
        }
        ClientInitializeError::NoCompatibleProtocolVersion {
            server_supported, ..
        } => format!(
            "the server supports no protocol version this app speaks (it offers: {})",
            server_supported
                .iter()
                .map(ProtocolVersion::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        ClientInitializeError::TransportError { error, context } => {
            format!("connection error ({context}): {error}")
        }
        ClientInitializeError::Cancelled => "the connection attempt was cancelled".to_owned(),
        ClientInitializeError::LegacyFallbackFailed { fallback, .. } => {
            return describe_init_error(fallback);
        }
        other => other.to_string(),
    };
    // Redact before cutting: a cut inside a URL would leave part of its credentials in
    // a form the redaction no longer recognises.
    policy::sanitize::one_line(&redact_urls_in(&text), 400)
}

/// A readable explanation of a request failure, with URLs redacted.
pub(crate) fn describe_service_error(error: &ServiceError) -> String {
    let text = match error {
        ServiceError::McpError(data)
            if data.code == ErrorCode::METHOD_NOT_FOUND && data.message.contains("sampling") =>
        {
            "the server asked to use the model (sampling), which this app does not allow".to_owned()
        }
        ServiceError::McpError(data) => {
            format!("the server returned an error: {}", data.message)
        }
        ServiceError::TransportClosed | ServiceError::TransportSend(_) => {
            "the connection to the server was lost".to_owned()
        }
        ServiceError::Timeout { timeout } => {
            format!("timed out after {} seconds", timeout.as_secs())
        }
        ServiceError::Cancelled { .. } => "the request was cancelled".to_owned(),
        ServiceError::InputRequiredRoundsExceeded { .. } => {
            "the server kept asking for more input without finishing".to_owned()
        }
        ServiceError::UnexpectedResponse => {
            "the server sent a response this app does not understand".to_owned()
        }
        other => other.to_string(),
    };
    // Redact before cutting: a cut inside a URL would leave part of its credentials in
    // a form the redaction no longer recognises.
    policy::sanitize::one_line(&redact_urls_in(&text), 400)
}

/// Whether the error means the connection is gone.
pub(crate) fn is_connection_lost(error: &ServiceError) -> bool {
    matches!(
        error,
        ServiceError::TransportClosed | ServiceError::TransportSend(_)
    )
}

/// Replaces every `http://` or `https://` URL in `text` with its redacted form, so error
/// messages from the HTTP stack never carry credentials or query strings.
pub(crate) fn redact_urls_in(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    loop {
        let next = ["https://", "http://"]
            .iter()
            .filter_map(|scheme| rest.find(scheme))
            .min();
        let Some(start) = next else {
            out.push_str(rest);
            return out;
        };
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let end = after
            .find(|c: char| c.is_whitespace() || matches!(c, ')' | '"' | '\'' | '>' | ','))
            .unwrap_or(after.len());
        out.push_str(&redact_url(&after[..end]));
        rest = &after[end..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ErrorData;

    #[test]
    fn falls_back_only_on_rejection_signals() {
        let rejected = ClientInitializeError::JsonRpcError(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "initialize",
            None,
        ));
        let closed = ClientInitializeError::ConnectionClosed("discover response".into());
        assert!(should_fall_back(Lifecycle::Legacy, &rejected));
        assert!(!should_fall_back(Lifecycle::Legacy, &closed));
        assert!(should_fall_back(Lifecycle::Modern, &rejected));
        assert!(should_fall_back(Lifecycle::Modern, &closed));
        assert!(!should_fall_back(
            Lifecycle::Legacy,
            &ClientInitializeError::Cancelled
        ));
    }

    #[test]
    fn urls_in_messages_are_redacted() {
        assert_eq!(
            redact_urls_in(
                "error sending request for url (https://u:p@h.example/mcp?token=abc): timed out"
            ),
            "error sending request for url (https://h.example/mcp): timed out"
        );
        assert_eq!(redact_urls_in("no urls here"), "no urls here");
        assert_eq!(
            redact_urls_in("a http://x/?k=1 b https://y/z?q=2"),
            "a http://x/ b https://y/z"
        );
    }

    #[test]
    fn a_cut_never_leaves_part_of_a_credential() {
        // Messages are cut at 400 characters (399 and an ellipsis). Place the URL so that
        // the cut falls inside its password, after the given prefix.
        let message = |prefix: &str| {
            let url = "https://user:supersecretpassword@h.example/mcp?token=abcdef";
            let pad = 393 - prefix.chars().count() - "https://user:".len();
            format!("{} {url}", "x".repeat(pad - 1))
        };
        let error = ServiceError::McpError(ErrorData::new(
            ErrorCode::INTERNAL_ERROR,
            message("the server returned an error: "),
            None,
        ));
        let text = describe_service_error(&error);
        assert!(text.contains("https://h.example/"), "{text}");
        assert!(!text.contains("supers"), "{text}");
        assert!(!text.contains("abcdef"), "{text}");
        let init = ClientInitializeError::JsonRpcError(ErrorData::new(
            ErrorCode::INVALID_REQUEST,
            message("the server rejected the handshake: "),
            None,
        ));
        let text = describe_init_error(&init);
        assert!(!text.contains("supers"), "{text}");
        assert!(text.chars().count() <= 400);
    }

    #[test]
    fn service_errors_read_naturally() {
        let sampling = ServiceError::McpError(ErrorData::new(
            ErrorCode::METHOD_NOT_FOUND,
            "sampling/createMessage",
            None,
        ));
        assert_eq!(
            describe_service_error(&sampling),
            "the server asked to use the model (sampling), which this app does not allow"
        );
        assert_eq!(
            describe_service_error(&ServiceError::TransportClosed),
            "the connection to the server was lost"
        );
        assert!(is_connection_lost(&ServiceError::TransportClosed));
        assert!(!is_connection_lost(&ServiceError::UnexpectedResponse));
    }

    #[test]
    fn progress_goes_to_the_only_call_in_flight() {
        let shared = ConnShared::new(Arc::new(StderrLog::new("s", None)));
        let seen = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink = seen.clone();
        let progress: ProgressFn = Arc::new(move |line| sink.lock().unwrap().push(line));
        {
            let _one = shared.begin_call(Some(progress.clone()));
            assert!(shared.progress().is_some());
            {
                let _two = shared.begin_call(Some(progress.clone()));
                assert!(shared.progress().is_none());
            }
            // The first call does not get progress back: it could not be told apart.
            assert!(shared.progress().is_none());
        }
        assert!(shared.progress().is_none());
        assert_eq!(shared.calls().in_flight, 0);
        // A later call on its own gets progress again.
        let _three = shared.begin_call(Some(progress));
        assert!(shared.progress().is_some());
    }
}
