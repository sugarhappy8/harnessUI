//! The host: one registry and one call path for built-in and external tools
//! (ARCHITECTURE.md 9.1, 9.2, 4.2, 13.3).
//!
//! From the registry upward (tool list, schema validation, describing a call for policy,
//! calling it, cancellation) there is no difference between a built-in server, connected
//! in memory by calling the [`BuiltinServer`] trait directly, and an external one reached
//! over stdio or Streamable HTTP through rmcp.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, Implementation, Tool};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use crate::builtin::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use crate::config::ServerConfig;
use crate::connection::{self, Connection, Lifecycle, Target};
use crate::describe::{self, ExternalTool, ToolPrefs};
use crate::error::HostError;
use crate::names::exposed_name;
use crate::result::map_call_result;
use crate::schema::{CompiledSchema, SchemaError, canonical_json};
use crate::secrets::SecretResolver;
use crate::stderr::StderrLog;
use crate::stdio;

/// Default per-call timeout for external tools.
pub const DEFAULT_CALL_TIMEOUT: Duration = Duration::from_secs(300);

/// Default start-up timeout for external servers (the SDK has none).
pub const DEFAULT_START_TIMEOUT: Duration = Duration::from_secs(60);

/// After a cancelled call to a stdio server, its process tree is killed this much later
/// and the server restarted on next use (4.2).
pub const KILL_AFTER_CANCEL: Duration = Duration::from_millis(500);

/// Longest external tool description kept (13.5).
pub const MAX_DESCRIPTION_CHARS: usize = 2000;

/// Capacity of the event channel; a slow subscriber misses events and re-reads state.
const EVENT_CAPACITY: usize = 256;

/// How the host starts and talks to servers.
#[derive(Clone)]
pub struct HostOptions {
    /// Variable names passed to stdio servers in addition to the runtime crate's minimal
    /// allowlist (user-editable pass-through list).
    pub env_allowlist_extra: Vec<String>,
    /// The login-shell environment the app captured at start-up (macOS). Used for `PATH`
    /// lookup and as the source of the allowlisted variables; never passed whole.
    pub captured_env: Option<BTreeMap<String, String>>,
    /// Folder for per-server stderr logs (`mcp-server-<name>.log`, size-capped).
    pub stderr_log_dir: Option<PathBuf>,
    /// Resolves `${secret:<name>}` placeholders (normally from the OS keychain).
    pub secret_resolver: SecretResolver,
    /// Per-call timeout for external tools.
    pub call_timeout: Duration,
    /// Start-up timeout for each connection attempt of an external server.
    pub start_timeout: Duration,
    /// Working folder for stdio servers whose config has no `cwd`; the home folder when
    /// unset.
    pub default_cwd: Option<PathBuf>,
    /// Client name sent to servers and used in the HTTP User-Agent (the app's productName).
    pub client_name: String,
    /// Client version sent to servers.
    pub client_version: String,
}

impl Default for HostOptions {
    fn default() -> Self {
        Self {
            env_allowlist_extra: Vec::new(),
            captured_env: None,
            stderr_log_dir: None,
            secret_resolver: Arc::new(|_: &str| None),
            call_timeout: DEFAULT_CALL_TIMEOUT,
            start_timeout: DEFAULT_START_TIMEOUT,
            default_cwd: None,
            client_name: env!("CARGO_PKG_NAME").to_owned(),
            client_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }
}

impl fmt::Debug for HostOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostOptions")
            .field("env_allowlist_extra", &self.env_allowlist_extra)
            .field(
                "captured_env",
                &self
                    .captured_env
                    .as_ref()
                    .map(|env| env.keys().collect::<Vec<_>>()),
            )
            .field("stderr_log_dir", &self.stderr_log_dir)
            .field("call_timeout", &self.call_timeout)
            .field("start_timeout", &self.start_timeout)
            .field("default_cwd", &self.default_cwd)
            .field("client_name", &self.client_name)
            .field("client_version", &self.client_version)
            .finish_non_exhaustive()
    }
}

/// What kind of server this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ServerKind {
    /// In the app's process.
    Builtin,
    /// A local process over stdin/stdout.
    Stdio,
    /// A remote server over Streamable HTTP.
    Http,
}

/// Where a server is in its life.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ServerState {
    /// Connecting (or restarting).
    Starting,
    /// Connected; its tools can be called.
    Ready,
    /// Could not start, or stopped unexpectedly.
    Failed {
        /// What went wrong, with the server's last stderr lines when there are any.
        reason: String,
    },
    /// Stopped on purpose (removed, or the app is quitting).
    Stopped,
}

/// One server as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    /// The server's name.
    pub name: String,
    /// Built-in, stdio or HTTP.
    pub kind: ServerKind,
    /// Starting / Ready / Failed / Stopped.
    pub state: ServerState,
    /// The negotiated protocol version, once connected.
    pub protocol_version: Option<String>,
    /// The lifecycle the server answered to, to persist and pass back to
    /// [`Host::add_external_with_lifecycle`].
    pub lifecycle: Option<Lifecycle>,
    /// The last lines of the server's stderr (about 50).
    pub stderr_tail: Vec<String>,
    /// Number of registered tools.
    pub tool_count: usize,
    /// A call was cancelled; the server's process is being replaced on next use.
    pub restart_on_next_use: bool,
    /// The process id of a running stdio server (on Unix also its process group), for
    /// diagnostics.
    pub pid: Option<u32>,
}

/// One tool in the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredTool {
    /// The name the model calls it by: the built-in name, or `<server>__<tool>`.
    pub exposed: String,
    /// The server's name.
    pub server: String,
    /// The tool's own name on its server.
    pub original: String,
    /// The tool as offered, under its exposed name. External descriptions are sanitised
    /// and capped at [`MAX_DESCRIPTION_CHARS`]; the schema is the server's full schema,
    /// except one refused for its size, which is replaced by `{"type": "object"}` (the
    /// tool is disabled either way). Offer llama-server [`Host::llama_schema`] instead.
    pub def: ToolDef,
    /// SHA-256 (hex) of the tool's original name, description, input schema (canonical
    /// JSON) and destructive hint. A tool whose fingerprint changed since the user last
    /// saw it starts disabled and loses its grants (9.2).
    pub fingerprint: String,
    /// Why the tool is disabled, when its schema was refused.
    pub schema_error: Option<String>,
}

/// Something changed; re-read [`Host::servers`] or [`Host::tools`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum HostEvent {
    /// A server's status changed.
    StatusChanged {
        /// The server's name.
        server: String,
    },
    /// A server's tools changed (added, removed, renamed or re-listed).
    ToolsChanged {
        /// The server's name.
        server: String,
    },
}

/// The MCP host. Cheap to clone; every clone is the same host.
#[derive(Clone)]
pub struct Host {
    inner: Arc<Inner>,
}

impl fmt::Debug for Host {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Host")
            .field("servers", &self.servers().len())
            .field("tools", &self.tools().len())
            .finish()
    }
}

struct Inner {
    opts: HostOptions,
    client_info: Implementation,
    events: broadcast::Sender<HostEvent>,
    registry: Mutex<Registry>,
    http: OnceLock<Result<reqwest::Client, HostError>>,
    shutting_down: AtomicBool,
    order: AtomicU64,
}

impl Inner {
    fn next_order(&self) -> u64 {
        self.order.fetch_add(1, Ordering::SeqCst)
    }

    fn emit(&self, event: HostEvent) {
        let _ = self.events.send(event);
    }

    fn registry(&self) -> std::sync::MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[derive(Default)]
struct Registry {
    servers: BTreeMap<String, ServerSlot>,
    tools: BTreeMap<String, ToolEntry>,
}

#[derive(Clone)]
enum ServerSlot {
    Builtin {
        server: Arc<dyn BuiltinServer>,
        order: u64,
    },
    External(Arc<External>),
}

impl ServerSlot {
    fn order(&self) -> u64 {
        match self {
            Self::Builtin { order, .. } => *order,
            Self::External(ext) => ext.order,
        }
    }
}

#[derive(Clone)]
struct ToolEntry {
    exposed: String,
    server: String,
    original: String,
    def: ToolDef,
    fingerprint: String,
    schema: Result<CompiledSchema, String>,
    destructive_hint: Option<bool>,
    builtin: bool,
    order: (u64, usize),
}

/// What [`Host::lookup`] returns: enough to describe or call a tool without the registry
/// lock.
struct Found {
    original: String,
    schema: Result<CompiledSchema, String>,
    destructive_hint: Option<bool>,
    server: ServerSlot,
}

struct External {
    name: String,
    config: ServerConfig,
    kind: ServerKind,
    log: Arc<StderrLog>,
    status: Mutex<ExternalStatus>,
    slot: tokio::sync::Mutex<Option<Arc<Connection>>>,
    /// Cancelled when the server is removed or the host shuts down.
    stop: CancellationToken,
    order: u64,
}

struct ExternalStatus {
    state: ServerState,
    pid: Option<u32>,
    protocol: Option<String>,
    lifecycle: Option<Lifecycle>,
    remembered: Option<Lifecycle>,
    restart_pending: bool,
    generation: u64,
}

impl External {
    fn status(&self) -> std::sync::MutexGuard<'_, ExternalStatus> {
        self.status.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn generation(&self) -> u64 {
        self.status().generation
    }

    fn snapshot(&self, tool_count: usize) -> ServerStatus {
        let status = self.status();
        ServerStatus {
            name: self.name.clone(),
            kind: self.kind,
            state: status.state.clone(),
            protocol_version: status.protocol.clone(),
            lifecycle: status.lifecycle,
            stderr_tail: self.log.tail(),
            tool_count,
            restart_on_next_use: status.restart_pending,
            pid: status.pid,
        }
    }
}

impl Host {
    /// A host with no servers.
    pub fn new(opts: HostOptions) -> Host {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let client_info =
            Implementation::new(opts.client_name.clone(), opts.client_version.clone());
        Host {
            inner: Arc::new(Inner {
                opts,
                client_info,
                events,
                registry: Mutex::new(Registry::default()),
                http: OnceLock::new(),
                shutting_down: AtomicBool::new(false),
                order: AtomicU64::new(0),
            }),
        }
    }

    /// Events for the UI: status and tool-list changes.
    pub fn subscribe(&self) -> broadcast::Receiver<HostEvent> {
        self.inner.events.subscribe()
    }

    /// Registers a built-in server, connected in memory. Its tools keep their fixed names;
    /// an external tool that held one of those names is moved to a suffixed name. Adding a
    /// server with the name of a registered built-in replaces it.
    pub fn add_builtin(&self, server: Arc<dyn BuiltinServer>) {
        let name = server.name().to_owned();
        let defs = server.tools();
        let mut displaced_servers: Vec<String> = Vec::new();
        {
            let mut reg = self.inner.registry();
            if let Some(ServerSlot::External(_)) = reg.servers.get(&name) {
                tracing::warn!(server = %name, "a built-in server has the name of an external server; not added");
                return;
            }
            reg.tools.retain(|_, t| t.server != name);
            let order = self.inner.next_order();
            let mut displaced: Vec<ToolEntry> = Vec::new();
            for (index, def) in defs.into_iter().enumerate() {
                if let Some(existing) = reg.tools.get(&def.name) {
                    if existing.builtin {
                        tracing::warn!(tool = %def.name, server = %name, "two built-in tools share a name; the second is ignored");
                        continue;
                    }
                    if let Some(entry) = reg.tools.remove(&def.name) {
                        displaced.push(entry);
                    }
                }
                let schema = CompiledSchema::compile(&def.input_schema).map_err(|e| e.to_string());
                let fingerprint = fingerprint(
                    &def.name,
                    &def.description,
                    &def.input_schema,
                    def.destructive,
                );
                reg.tools.insert(
                    def.name.clone(),
                    ToolEntry {
                        exposed: def.name.clone(),
                        server: name.clone(),
                        original: def.name.clone(),
                        destructive_hint: def.destructive,
                        def,
                        fingerprint,
                        schema,
                        builtin: true,
                        order: (order, index),
                    },
                );
            }
            reg.servers
                .insert(name.clone(), ServerSlot::Builtin { server, order });
            for mut entry in displaced {
                let exposed = exposed_name(&entry.server, &entry.original, &|n| {
                    reg.tools.contains_key(n)
                });
                entry.exposed = exposed.clone();
                entry.def.name = exposed.clone();
                if !displaced_servers.contains(&entry.server) {
                    displaced_servers.push(entry.server.clone());
                }
                reg.tools.insert(exposed, entry);
            }
        }
        self.inner.emit(HostEvent::StatusChanged {
            server: name.clone(),
        });
        self.inner.emit(HostEvent::ToolsChanged { server: name });
        for server in displaced_servers {
            self.inner.emit(HostEvent::ToolsChanged { server });
        }
    }

    /// Adds an external server and starts it: Starting, then Ready or Failed(reason). The
    /// server stays registered when it fails, so [`restart`](Self::restart) can retry.
    pub async fn add_external(&self, name: &str, cfg: ServerConfig) -> Result<(), HostError> {
        self.add_external_with_lifecycle(name, cfg, None).await
    }

    /// [`add_external`](Self::add_external) with the lifecycle remembered from an earlier
    /// run ([`ServerStatus::lifecycle`]), which is tried first.
    pub async fn add_external_with_lifecycle(
        &self,
        name: &str,
        cfg: ServerConfig,
        remembered: Option<Lifecycle>,
    ) -> Result<(), HostError> {
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return Err(HostError::ShuttingDown);
        }
        let name = valid_server_name(name)?;
        let kind = match cfg {
            ServerConfig::Stdio { .. } => ServerKind::Stdio,
            ServerConfig::Http { .. } | ServerConfig::LegacySse { .. } => ServerKind::Http,
        };
        let ext = Arc::new(External {
            log: Arc::new(StderrLog::new(
                &name,
                self.inner.opts.stderr_log_dir.as_deref(),
            )),
            name: name.clone(),
            config: cfg,
            kind,
            status: Mutex::new(ExternalStatus {
                state: ServerState::Starting,
                pid: None,
                protocol: None,
                lifecycle: None,
                remembered,
                restart_pending: false,
                generation: 0,
            }),
            slot: tokio::sync::Mutex::new(None),
            stop: CancellationToken::new(),
            order: self.inner.next_order(),
        });
        {
            let mut reg = self.inner.registry();
            if reg.servers.contains_key(&name) {
                return Err(HostError::AlreadyExists(name));
            }
            reg.servers
                .insert(name.clone(), ServerSlot::External(ext.clone()));
        }
        self.inner.emit(HostEvent::StatusChanged { server: name });
        self.start(&ext, false).await
    }

    /// Restarts an external server (a no-op for a built-in one).
    pub async fn restart(&self, name: &str) -> Result<(), HostError> {
        match self.server_slot(name) {
            Some(ServerSlot::External(ext)) => self.start(&ext, false).await,
            Some(ServerSlot::Builtin { .. }) => Ok(()),
            None => Err(HostError::NotFound(name.to_owned())),
        }
    }

    /// Removes a server and its tools; an external server is stopped (its process tree
    /// killed after a short grace period).
    pub async fn remove(&self, name: &str) -> Result<(), HostError> {
        let slot = {
            let mut reg = self.inner.registry();
            let slot = reg
                .servers
                .remove(name)
                .ok_or_else(|| HostError::NotFound(name.to_owned()))?;
            reg.tools.retain(|_, t| t.server != name);
            slot
        };
        if let ServerSlot::External(ext) = slot {
            self.stop_external(&ext).await;
        }
        self.inner.emit(HostEvent::StatusChanged {
            server: name.to_owned(),
        });
        self.inner.emit(HostEvent::ToolsChanged {
            server: name.to_owned(),
        });
        Ok(())
    }

    /// Stops every external server, killing their process trees, and refuses new ones.
    pub async fn shutdown(&self) {
        self.inner.shutting_down.store(true, Ordering::SeqCst);
        let externals: Vec<Arc<External>> = self
            .inner
            .registry()
            .servers
            .values()
            .filter_map(|slot| match slot {
                ServerSlot::External(ext) => Some(ext.clone()),
                ServerSlot::Builtin { .. } => None,
            })
            .collect();
        let mut stopping = tokio::task::JoinSet::new();
        for ext in externals {
            let host = self.clone();
            stopping.spawn(async move {
                host.stop_external(&ext).await;
                ext.name.clone()
            });
        }
        while let Some(done) = stopping.join_next().await {
            if let Ok(server) = done {
                self.inner.emit(HostEvent::StatusChanged { server });
            }
        }
    }

    /// Every server, built-in ones first, in the order they were added.
    pub fn servers(&self) -> Vec<ServerStatus> {
        let reg = self.inner.registry();
        let mut slots: Vec<(&String, &ServerSlot)> = reg.servers.iter().collect();
        slots.sort_by_key(|(_, slot)| (!matches!(slot, ServerSlot::Builtin { .. }), slot.order()));
        slots
            .into_iter()
            .map(|(name, slot)| {
                let tool_count = reg.tools.values().filter(|t| &t.server == name).count();
                match slot {
                    ServerSlot::Builtin { .. } => ServerStatus {
                        name: name.clone(),
                        kind: ServerKind::Builtin,
                        state: ServerState::Ready,
                        protocol_version: None,
                        lifecycle: None,
                        stderr_tail: Vec::new(),
                        tool_count,
                        restart_on_next_use: false,
                        pid: None,
                    },
                    ServerSlot::External(ext) => ext.snapshot(tool_count),
                }
            })
            .collect()
    }

    /// Every registered tool, in a fixed order: by server (in the order added), then in
    /// the order the server lists them. Disabled tools are included with their reason.
    pub fn tools(&self) -> Vec<RegisteredTool> {
        let reg = self.inner.registry();
        let mut entries: Vec<&ToolEntry> = reg.tools.values().collect();
        entries.sort_by_key(|t| (!t.builtin, t.order));
        entries
            .into_iter()
            .map(|t| RegisteredTool {
                exposed: t.exposed.clone(),
                server: t.server.clone(),
                original: t.original.clone(),
                def: t.def.clone(),
                fingerprint: t.fingerprint.clone(),
                schema_error: t.schema.as_ref().err().cloned(),
            })
            .collect()
    }

    /// Whether a call to `exposed` could run now: the tool exists, is not disabled, and
    /// its server is built in or Ready (for the policy engine's `server_ready`).
    pub fn tool_ready(&self, exposed: &str) -> bool {
        let Some(found) = self.lookup(exposed) else {
            return false;
        };
        found.schema.is_ok()
            && match &found.server {
                ServerSlot::Builtin { .. } => true,
                ServerSlot::External(ext) => matches!(ext.status().state, ServerState::Ready),
            }
    }

    /// Validates arguments against the tool's full schema (8.3 step 5).
    pub fn validate(&self, exposed: &str, args: &Value) -> Result<(), Vec<String>> {
        match self.lookup(exposed) {
            None => Err(vec![format!("there is no tool named \"{exposed}\"")]),
            Some(found) => match &found.schema {
                Ok(schema) => schema.validate(args),
                Err(reason) => Err(vec![format!("the tool is disabled: {reason}")]),
            },
        }
    }

    /// The simplified schema to offer llama-server for this tool (8.3 step 1); `None` for
    /// an unknown or disabled tool.
    pub fn llama_schema(&self, exposed: &str) -> Option<Value> {
        self.lookup(exposed)?.schema.ok().map(|s| s.llama().clone())
    }

    /// Turns a call into an action for the policy engine. Built-in tools use their
    /// server's describer; external tools the generic one (server, tool, full arguments;
    /// exact-arguments scope unless `prefs.allow_any_args`; risk from the destructive hint
    /// and `prefs.always_ask` only).
    pub async fn describe(
        &self,
        exposed: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
        prefs: ToolPrefs,
    ) -> Described {
        let Some(found) = self.lookup(exposed) else {
            return Described::Invalid(format!("there is no tool named \"{exposed}\""));
        };
        if let Err(reason) = &found.schema {
            return Described::Invalid(format!("the tool {exposed} is disabled: {reason}"));
        }
        match found.server {
            ServerSlot::Builtin { server, .. } => {
                server.describe(&found.original, args, chat).await
            }
            ServerSlot::External(ext) => Described::Action(describe::describe_external(
                ExternalTool {
                    server: &ext.name,
                    exposed,
                    original: &found.original,
                    destructive_hint: found.destructive_hint,
                },
                args,
                prefs,
            )),
        }
    }

    /// Runs an approved call. Built-in tools are called directly. External tools go
    /// through rmcp with the per-call timeout and `ctx.cancel`: on cancel the request is
    /// abandoned (rmcp sends the cancel notification) and, for a stdio server, its process
    /// tree is killed [`KILL_AFTER_CANCEL`] later and the server restarted on next use,
    /// because a cancelled call returns nothing and the server's state is unknown (4.2).
    /// Failures of every kind come back as error results the model can read.
    pub async fn call(
        &self,
        exposed: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let Some(found) = self.lookup(exposed) else {
            return ToolOutput::error(format!("there is no tool named \"{exposed}\""));
        };
        if let Err(reason) = &found.schema {
            return ToolOutput::error(format!("the tool {exposed} is disabled: {reason}"));
        }
        match &found.server {
            ServerSlot::Builtin { server, .. } => {
                server.call(&found.original, args, action, ctx).await
            }
            ServerSlot::External(ext) => {
                self.call_external(ext, exposed, &found, args, action, ctx)
                    .await
            }
        }
    }

    async fn call_external(
        &self,
        ext: &Arc<External>,
        exposed: &str,
        found: &Found,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if ctx.cancel.is_cancelled() {
            return cancelled_output();
        }
        if !describe::action_matches(action, &ext.name, exposed, &found.original, args) {
            return ToolOutput::error(
                "this call does not match the action that was approved, so it was not run",
            );
        }
        if let Ok(schema) = &found.schema
            && let Err(problems) = schema.validate(&Value::Object(args.clone()))
        {
            return ToolOutput::error(format!("invalid arguments: {}", problems.join("; ")));
        }
        // Waiting for the connection can take as long as a start-up (a restart after an
        // earlier cancel, or one the user started), so Stop must end this wait too (4.2).
        // Starts run in their own task and are not cut short by giving up here.
        let found_connection = tokio::select! {
            biased;
            _ = ctx.cancel.cancelled() => return cancelled_output(),
            found = self.connection_for_call(ext) => found,
        };
        let (connection, generation) = match found_connection {
            Ok(found) => found,
            Err(error) => {
                return ToolOutput::error(format!("{} is not available: {error}", ext.name));
            }
        };
        let params =
            CallToolRequestParams::new(found.original.clone()).with_arguments(args.clone());
        let timeout = self.inner.opts.call_timeout;
        let outcome = {
            let _call = connection.shared.begin_call(ctx.progress.clone());
            tokio::select! {
                biased;
                _ = ctx.cancel.cancelled() => Outcome::Cancelled,
                result = tokio::time::timeout(timeout, connection.service.call_tool(params)) => match result {
                    Ok(Ok(result)) => Outcome::Done(result),
                    Ok(Err(error)) => Outcome::Failed(error),
                    Err(_) => Outcome::TimedOut,
                },
            }
        };
        match outcome {
            Outcome::Done(result) => map_call_result(result),
            Outcome::Failed(error) => {
                if connection::is_connection_lost(&error) && connection.process.is_none() {
                    self.set_failed(ext, "the connection to the server was lost".into());
                }
                ToolOutput::error(format!(
                    "{} could not run {}: {}",
                    ext.name,
                    found.original,
                    connection::describe_service_error(&error)
                ))
            }
            Outcome::Cancelled => {
                self.retire_after_cancel(ext, &connection, generation);
                cancelled_output()
            }
            Outcome::TimedOut => {
                self.retire_after_cancel(ext, &connection, generation);
                ToolOutput::error(format!(
                    "the tool did not finish within {} seconds and was stopped",
                    timeout.as_secs()
                ))
            }
        }
    }

    /// After a cancelled or timed-out call: a stdio server is killed shortly after and
    /// replaced on next use; a remote one is left alone. `generation` is the start the
    /// call's connection belongs to: when the server was restarted meanwhile, only the old
    /// process is killed and the new one is left running.
    fn retire_after_cancel(
        &self,
        ext: &Arc<External>,
        connection: &Arc<Connection>,
        generation: u64,
    ) {
        if connection.process.is_none() {
            return;
        }
        let marked = {
            let mut status = ext.status();
            let current = status.generation == generation;
            if current {
                status.restart_pending = true;
            }
            current
        };
        if marked {
            self.inner.emit(HostEvent::StatusChanged {
                server: ext.name.clone(),
            });
        }
        connection.stopping.store(true, Ordering::SeqCst);
        let connection = connection.clone();
        tokio::spawn(async move {
            tokio::time::sleep(KILL_AFTER_CANCEL).await;
            connection.kill_now();
        });
    }

    /// The live connection for a call and the start it belongs to, restarting the server
    /// first if a cancelled call left it marked.
    async fn connection_for_call(
        &self,
        ext: &Arc<External>,
    ) -> Result<(Arc<Connection>, u64), HostError> {
        let pending = ext.status().restart_pending;
        if pending {
            self.start(ext, true).await?;
        }
        let slot = ext.slot.lock().await;
        let (state, generation) = {
            let status = ext.status();
            (status.state.clone(), status.generation)
        };
        match (slot.as_ref(), state) {
            (Some(connection), ServerState::Ready) => Ok((connection.clone(), generation)),
            (_, ServerState::Failed { reason }) => Err(HostError::NotReady(reason)),
            (_, ServerState::Starting) => Err(HostError::NotReady("it is still starting".into())),
            _ => Err(HostError::NotReady("it is stopped".into())),
        }
    }

    /// Connects (or reconnects) an external server and registers its tools. With
    /// `only_if_pending`, does nothing unless a cancelled call marked it for restart (so
    /// concurrent calls restart it once).
    ///
    /// The work runs in its own task: a caller that stops waiting (a call the user
    /// stopped, a dropped UI command) never leaves the server half-started in Starting.
    async fn start(&self, ext: &Arc<External>, only_if_pending: bool) -> Result<(), HostError> {
        let host = self.clone();
        let ext = ext.clone();
        match tokio::spawn(async move { host.start_now(&ext, only_if_pending).await }).await {
            Ok(result) => result,
            Err(error) => Err(HostError::Connect(format!(
                "starting the server was interrupted: {error}"
            ))),
        }
    }

    async fn start_now(&self, ext: &Arc<External>, only_if_pending: bool) -> Result<(), HostError> {
        let mut slot = ext.slot.lock().await;
        if only_if_pending && !ext.status().restart_pending {
            return Ok(());
        }
        if self.inner.shutting_down.load(Ordering::SeqCst) {
            return Err(HostError::ShuttingDown);
        }
        if ext.stop.is_cancelled() {
            return Err(HostError::NotFound(ext.name.clone()));
        }
        if let Some(old) = slot.take() {
            if ext.status().restart_pending {
                old.kill_now();
                if let Some(process) = &old.process {
                    let _ = process.wait_exit(Duration::from_secs(2)).await;
                }
            } else {
                old.close().await;
            }
        }
        let generation = {
            let mut status = ext.status();
            status.generation += 1;
            status.state = ServerState::Starting;
            status.pid = None;
            status.restart_pending = false;
            status.generation
        };
        self.inner.emit(HostEvent::StatusChanged {
            server: ext.name.clone(),
        });
        let result = tokio::select! {
            result = self.connect_and_list(ext) => result,
            // Removed or shut down while starting.
            _ = ext.stop.cancelled() => Err(if self.inner.shutting_down.load(Ordering::SeqCst) {
                HostError::ShuttingDown
            } else {
                HostError::NotFound(ext.name.clone())
            }),
        };
        match result {
            Ok((connection, tools)) => {
                let connection = Arc::new(connection);
                let still_registered = self.register_external_tools_async(&ext.name, tools).await;
                if !still_registered {
                    connection.close().await;
                    return Err(HostError::NotFound(ext.name.clone()));
                }
                {
                    let mut status = ext.status();
                    status.state = ServerState::Ready;
                    status.pid = connection.process.as_ref().and_then(|p| p.pid());
                    status.protocol = Some(connection.protocol.clone());
                    status.lifecycle = Some(connection.lifecycle);
                    status.remembered = Some(connection.lifecycle);
                }
                // After Ready is set: a process that already died is then reported as
                // Failed by the exit watcher instead of being overwritten with Ready.
                self.spawn_connection_tasks(ext, &connection, generation);
                *slot = Some(connection);
                drop(slot);
                self.inner.emit(HostEvent::StatusChanged {
                    server: ext.name.clone(),
                });
                self.inner.emit(HostEvent::ToolsChanged {
                    server: ext.name.clone(),
                });
                Ok(())
            }
            Err(error) => {
                drop(slot);
                // A start cut short by remove or shutdown ends as Stopped, not Failed.
                if !ext.stop.is_cancelled() {
                    self.set_failed(ext, error.to_string());
                }
                Err(error)
            }
        }
    }

    async fn connect_and_list(
        &self,
        ext: &Arc<External>,
    ) -> Result<(Connection, Vec<rmcp::model::Tool>), HostError> {
        let opts = &self.inner.opts;
        let mut target = self.target_for(ext)?;
        if let Target::Stdio(prepared) = &mut target {
            stdio::check_developer_tools(prepared).await?;
        }
        let first = ext.status().remembered.unwrap_or(Lifecycle::Legacy);
        let connected = connection::connect(
            &target,
            first,
            &self.inner.client_info,
            &ext.log,
            opts.start_timeout,
        )
        .await;
        let connection = match connected {
            Ok(connection) => connection,
            Err(HostError::AuthRequired) => return Err(HostError::AuthRequired),
            Err(_) if matches!(ext.config, ServerConfig::LegacySse { .. }) => {
                return Err(HostError::LegacySseUnsupported);
            }
            Err(error) => return Err(error),
        };
        match connection::list_tools(&connection, opts.start_timeout).await {
            Ok(tools) => Ok((connection, tools)),
            Err(message) => {
                connection.close().await;
                Err(HostError::Connect(message))
            }
        }
    }

    fn target_for(&self, ext: &External) -> Result<Target, HostError> {
        let opts = &self.inner.opts;
        match &ext.config {
            ServerConfig::Stdio {
                command,
                args,
                env,
                cwd,
            } => Ok(Target::Stdio(stdio::prepare(
                command,
                args,
                env,
                cwd.as_deref(),
                opts,
            )?)),
            ServerConfig::Http { url, headers } | ServerConfig::LegacySse { url, headers } => {
                let client = self.http_client()?;
                let resolver = opts.secret_resolver.as_ref();
                Ok(Target::Http {
                    client,
                    url: crate::http::resolve_url(url, resolver)?,
                    headers: crate::http::resolve_headers(headers, resolver)?,
                })
            }
        }
    }

    fn http_client(&self) -> Result<reqwest::Client, HostError> {
        self.inner
            .http
            .get_or_init(|| {
                let agent = format!(
                    "{}/{}",
                    self.inner.opts.client_name, self.inner.opts.client_version
                );
                crate::http::build_client(self.inner.opts.call_timeout, &agent)
            })
            .clone()
    }

    /// Background work tied to one connection: re-listing tools when the server says they
    /// changed, the 2026-07-28 listen stream, and noticing a process that exits on its own.
    fn spawn_connection_tasks(
        &self,
        ext: &Arc<External>,
        connection: &Arc<Connection>,
        generation: u64,
    ) {
        let weak_host: Weak<Inner> = Arc::downgrade(&self.inner);
        let weak_ext = Arc::downgrade(ext);
        let weak_connection = Arc::downgrade(connection);
        let shared = connection.shared.clone();
        connection.add_task(tokio::spawn(async move {
            loop {
                shared.refresh.notified().await;
                let (Some(inner), Some(ext), Some(connection)) = (
                    weak_host.upgrade(),
                    weak_ext.upgrade(),
                    weak_connection.upgrade(),
                ) else {
                    return;
                };
                Host { inner }
                    .refresh_tools(&ext, &connection, generation)
                    .await;
            }
        }));
        if let Some(task) = connection::listen_for_tool_changes(connection) {
            connection.add_task(task);
        }
        if let Some(process) = &connection.process {
            let mut exit = process.exit_watch();
            let stopping = connection.stopping.clone();
            let weak_host: Weak<Inner> = Arc::downgrade(&self.inner);
            let weak_ext = Arc::downgrade(ext);
            connection.add_task(tokio::spawn(async move {
                let info = match exit.wait_for(Option::is_some).await {
                    Ok(info) => info.clone(),
                    Err(_) => return,
                };
                if stopping.load(Ordering::SeqCst) {
                    return;
                }
                let (Some(inner), Some(ext), Some(info)) =
                    (weak_host.upgrade(), weak_ext.upgrade(), info)
                else {
                    return;
                };
                if ext.generation() != generation {
                    return;
                }
                ext.log
                    .note(&format!("the server {}", info.description))
                    .await;
                let tail = ext.log.tail_text(10);
                let reason = if tail.is_empty() {
                    format!("the server {} unexpectedly", info.description)
                } else {
                    format!(
                        "the server {} unexpectedly\n\nLast output from the server:\n{tail}",
                        info.description
                    )
                };
                Host { inner }.set_failed(&ext, reason);
            }));
        }
    }

    async fn refresh_tools(&self, ext: &Arc<External>, connection: &Connection, generation: u64) {
        match connection::list_tools(connection, self.inner.opts.start_timeout).await {
            Ok(tools) => {
                if ext.generation() != generation {
                    return;
                }
                if self.register_external_tools_async(&ext.name, tools).await {
                    self.inner.emit(HostEvent::ToolsChanged {
                        server: ext.name.clone(),
                    });
                }
            }
            Err(message) => ext.log.push(format!(
                "--- refreshing the tool list failed: {message} ---"
            )),
        }
    }

    /// Replaces a server's tools, keeping each surviving tool's exposed name. Returns
    /// false when the server is no longer registered.
    #[cfg(test)]
    fn register_external_tools(&self, server: &str, tools: Vec<Tool>) -> bool {
        self.insert_external_tools(server, prepare_external_tools(tools))
    }

    /// Replaces a server's tools, keeping each surviving tool's exposed name. The schemas
    /// are compiled on a blocking thread, outside the registry lock. Returns false when
    /// the server is no longer registered.
    async fn register_external_tools_async(&self, server: &str, tools: Vec<Tool>) -> bool {
        let prepared =
            match tokio::task::spawn_blocking(move || prepare_external_tools(tools)).await {
                Ok(prepared) => prepared,
                Err(error) => {
                    tracing::warn!(server, %error, "preparing an MCP server's tools failed");
                    return false;
                }
            };
        self.insert_external_tools(server, prepared)
    }

    fn insert_external_tools(&self, server: &str, tools: Vec<PreparedTool>) -> bool {
        let mut reg = self.inner.registry();
        if !reg.servers.contains_key(server) {
            return false;
        }
        let previous: HashMap<String, String> = reg
            .tools
            .values()
            .filter(|t| t.server == server)
            .map(|t| (t.original.clone(), t.exposed.clone()))
            .collect();
        reg.tools.retain(|_, t| t.server != server);
        let order = self.inner.next_order();
        // Surviving tools keep their names, so they are placed first: a new tool whose
        // sanitised name happens to equal one of them must not take it (and with it the
        // grants and toggles the app keyed on that name).
        let mut names: HashMap<String, String> = HashMap::new();
        for tool in &tools {
            if let Some(name) = previous.get(&tool.original)
                && !reg.tools.contains_key(name)
                && !names.values().any(|taken| taken == name)
            {
                names.insert(tool.original.clone(), name.clone());
            }
        }
        for (index, tool) in tools.into_iter().enumerate() {
            let exposed = match names.get(&tool.original) {
                Some(name) => name.clone(),
                None => exposed_name(server, &tool.original, &|n| {
                    reg.tools.contains_key(n) || names.values().any(|taken| taken == n)
                }),
            };
            reg.tools.insert(
                exposed.clone(),
                ToolEntry {
                    exposed: exposed.clone(),
                    server: server.to_owned(),
                    original: tool.original,
                    def: ToolDef {
                        name: exposed,
                        description: tool.description,
                        input_schema: tool.input_schema,
                        destructive: tool.destructive,
                        read_only: tool.read_only,
                    },
                    fingerprint: tool.fingerprint,
                    schema: tool.schema,
                    destructive_hint: tool.destructive,
                    builtin: false,
                    order: (order, index),
                },
            );
        }
        true
    }

    async fn stop_external(&self, ext: &Arc<External>) {
        ext.stop.cancel();
        let mut slot = ext.slot.lock().await;
        if let Some(connection) = slot.take() {
            connection.close().await;
        }
        drop(slot);
        let mut status = ext.status();
        status.state = ServerState::Stopped;
        status.pid = None;
        status.restart_pending = false;
        status.generation += 1;
    }

    fn set_failed(&self, ext: &External, reason: String) {
        ext.status().state = ServerState::Failed { reason };
        self.inner.emit(HostEvent::StatusChanged {
            server: ext.name.clone(),
        });
    }

    fn server_slot(&self, name: &str) -> Option<ServerSlot> {
        self.inner.registry().servers.get(name).cloned()
    }

    fn lookup(&self, exposed: &str) -> Option<Found> {
        let reg = self.inner.registry();
        let entry = reg.tools.get(exposed)?;
        let server = reg.servers.get(&entry.server)?.clone();
        Some(Found {
            original: entry.original.clone(),
            schema: entry.schema.clone(),
            destructive_hint: entry.destructive_hint,
            server,
        })
    }
}

enum Outcome {
    Done(rmcp::model::CallToolResult),
    Failed(rmcp::ServiceError),
    Cancelled,
    TimedOut,
}

/// An external tool checked and compiled, ready to be named and registered.
struct PreparedTool {
    original: String,
    description: String,
    input_schema: Value,
    fingerprint: String,
    schema: Result<CompiledSchema, String>,
    destructive: Option<bool>,
    read_only: Option<bool>,
}

/// Fingerprints, sanitises and compiles a server's tools (without the registry lock: this
/// is the slow part). Tools without a name and repeated names are dropped. A schema
/// refused for its size is replaced in the definition by an empty object schema, so the
/// registry never holds or hands out megabytes for a tool that is disabled anyway.
fn prepare_external_tools(tools: Vec<Tool>) -> Vec<PreparedTool> {
    let mut seen: HashSet<String> = HashSet::new();
    tools
        .into_iter()
        .filter(|tool| !tool.name.is_empty() && seen.insert(tool.name.to_string()))
        .map(|tool| {
            let original = tool.name.to_string();
            let raw_description = tool.description.as_deref().unwrap_or_default();
            let mut input_schema = Value::Object((*tool.input_schema).clone());
            let destructive = tool.annotations.as_ref().and_then(|a| a.destructive_hint);
            let read_only = tool.annotations.as_ref().and_then(|a| a.read_only_hint);
            let fingerprint = fingerprint(&original, raw_description, &input_schema, destructive);
            let compiled = CompiledSchema::compile(&input_schema);
            if matches!(compiled, Err(SchemaError::TooLarge)) {
                input_schema = serde_json::json!({"type": "object"});
            }
            let schema = compiled.map_err(|e| e.to_string());
            PreparedTool {
                original,
                description: cap_description(raw_description),
                input_schema,
                fingerprint,
                schema,
                destructive,
                read_only,
            }
        })
        .collect()
}

fn cancelled_output() -> ToolOutput {
    ToolOutput::error(
        "cancelled: the user stopped this tool call before it finished; the server may have done part of the work",
    )
}

fn valid_server_name(name: &str) -> Result<String, HostError> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.chars().count() > crate::config::MAX_SERVER_NAME_LEN
        || trimmed.chars().any(char::is_control)
    {
        return Err(HostError::InvalidName(policy::sanitize::one_line(name, 60)));
    }
    Ok(trimmed.to_owned())
}

/// A description safe to show and offer: invisible characters escaped, at most
/// [`MAX_DESCRIPTION_CHARS`] characters.
fn cap_description(text: &str) -> String {
    let visible = policy::sanitize::visible(text.trim());
    if visible.chars().count() <= MAX_DESCRIPTION_CHARS {
        return visible;
    }
    let cut: String = visible.chars().take(MAX_DESCRIPTION_CHARS - 1).collect();
    format!("{cut}…")
}

/// SHA-256 over the tool's identity: each field length-prefixed so no two different
/// tools hash the same input.
pub(crate) fn fingerprint(
    name: &str,
    description: &str,
    schema: &Value,
    destructive: Option<bool>,
) -> String {
    let mut hasher = Sha256::new();
    let destructive = match destructive {
        Some(true) => "true",
        Some(false) => "false",
        None => "unset",
    };
    for field in [name, description, &canonical_json(schema), destructive] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    /// A built-in server with two tools that records what it was asked.
    struct FakeFiles {
        calls: Mutex<Vec<(String, Value)>>,
    }

    #[async_trait]
    impl BuiltinServer for FakeFiles {
        fn name(&self) -> &str {
            "files"
        }

        fn tools(&self) -> Vec<ToolDef> {
            vec![
                ToolDef {
                    name: "read_file".into(),
                    description: "Read a file".into(),
                    input_schema: json!({
                        "type": "object",
                        "properties": {"path": {"type": "string"}},
                        "required": ["path"]
                    }),
                    destructive: None,
                    read_only: Some(true),
                },
                ToolDef {
                    name: "bad_schema".into(),
                    description: "Has a remote ref".into(),
                    input_schema: json!({"type": "object", "properties": {"a": {"$ref": "https://x/y"}}}),
                    destructive: None,
                    read_only: None,
                },
            ]
        }

        async fn describe(
            &self,
            tool: &str,
            args: &Map<String, Value>,
            chat: &ChatContext,
        ) -> Described {
            Described::Action(policy::Action {
                server: "files".into(),
                tool: tool.into(),
                summary: format!(
                    "read {}",
                    args.get("path").and_then(Value::as_str).unwrap_or("?")
                ),
                preview: String::new(),
                scope: policy::Scope::FileRead {
                    folder: chat.working_folder.clone().unwrap_or_default(),
                },
                risk: policy::Risk::Normal,
                read_only: true,
                outbound: false,
                keep_asking: None,
                network: false,
                hard_limit: None,
                payload: Value::Null,
            })
        }

        async fn call(
            &self,
            tool: &str,
            args: &Map<String, Value>,
            _action: &policy::Action,
            _ctx: CallContext,
        ) -> ToolOutput {
            self.calls
                .lock()
                .unwrap()
                .push((tool.to_owned(), Value::Object(args.clone())));
            ToolOutput::text(format!(
                "contents of {}",
                args["path"].as_str().unwrap_or("?")
            ))
        }
    }

    fn ctx() -> CallContext {
        CallContext {
            chat: ChatContext::default(),
            cancel: CancellationToken::new(),
            progress: None,
        }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap()
    }

    #[tokio::test]
    async fn built_in_servers_work_through_the_host() {
        let host = Host::new(HostOptions::default());
        let mut events = host.subscribe();
        let files = Arc::new(FakeFiles {
            calls: Mutex::new(Vec::new()),
        });
        host.add_builtin(files.clone());
        assert_eq!(
            events.recv().await.unwrap(),
            HostEvent::StatusChanged {
                server: "files".into()
            }
        );
        let servers = host.servers();
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].kind, ServerKind::Builtin);
        assert_eq!(servers[0].state, ServerState::Ready);
        assert_eq!(servers[0].tool_count, 2);

        let tools = host.tools();
        assert_eq!(tools[0].exposed, "read_file");
        assert_eq!(tools[0].def.name, "read_file");
        assert_eq!(tools[0].schema_error, None);
        assert_eq!(tools[0].fingerprint.len(), 64);
        assert!(
            tools[1]
                .schema_error
                .as_deref()
                .unwrap()
                .contains("external document")
        );
        assert!(host.tool_ready("read_file"));
        assert!(!host.tool_ready("bad_schema"));
        assert!(!host.tool_ready("nope"));

        assert_eq!(host.validate("read_file", &json!({"path": "a"})), Ok(()));
        assert_eq!(
            host.validate("read_file", &json!({})),
            Err(vec![
                "arguments: missing required field \"path\"".to_owned()
            ])
        );
        assert!(host.validate("bad_schema", &json!({})).is_err());
        assert!(host.validate("nope", &json!({})).is_err());
        assert_eq!(
            host.llama_schema("read_file").unwrap()["properties"]["path"],
            json!({"type": "string"})
        );
        assert_eq!(host.llama_schema("bad_schema"), None);

        let a = args(json!({"path": "notes.txt"}));
        let chat = ChatContext {
            working_folder: Some("/w".into()),
            ..Default::default()
        };
        let Described::Action(action) = host
            .describe("read_file", &a, &chat, ToolPrefs::default())
            .await
        else {
            panic!("an action")
        };
        assert_eq!(action.summary, "read notes.txt");
        assert_eq!(
            action.scope,
            policy::Scope::FileRead {
                folder: "/w".into()
            }
        );
        let out = host.call("read_file", &a, &action, ctx()).await;
        assert_eq!(out.joined_text(), "contents of notes.txt");
        assert_eq!(files.calls.lock().unwrap().len(), 1);

        assert!(matches!(
            host.describe("bad_schema", &a, &chat, ToolPrefs::default())
                .await,
            Described::Invalid(_)
        ));
        assert!(host.call("bad_schema", &a, &action, ctx()).await.is_error);
        assert!(host.call("nope", &a, &action, ctx()).await.is_error);
    }

    #[tokio::test]
    async fn external_tools_are_registered_under_sanitised_names() {
        let host = Host::new(HostOptions::default());
        host.add_builtin(Arc::new(FakeFiles {
            calls: Mutex::new(Vec::new()),
        }));
        // Register an external server's tools directly (no process needed for naming).
        let ext = Arc::new(External {
            name: "my server".into(),
            config: ServerConfig::Http {
                url: "https://h.example".into(),
                headers: BTreeMap::new(),
            },
            kind: ServerKind::Http,
            log: Arc::new(StderrLog::new("my server", None)),
            status: Mutex::new(ExternalStatus {
                state: ServerState::Ready,
                pid: None,
                protocol: None,
                lifecycle: None,
                remembered: None,
                restart_pending: false,
                generation: 1,
            }),
            slot: tokio::sync::Mutex::new(None),
            stop: CancellationToken::new(),
            order: 99,
        });
        host.inner
            .registry()
            .servers
            .insert("my server".into(), ServerSlot::External(ext));
        let schema = Arc::new(
            json!({"type": "object", "properties": {"q": {"type": "string"}}})
                .as_object()
                .cloned()
                .unwrap(),
        );
        let mut tool =
            rmcp::model::Tool::new("search.web", "Search \u{202E}the web", schema.clone());
        tool.annotations = Some(
            rmcp::model::ToolAnnotations::new()
                .destructive(true)
                .read_only(true),
        );
        let dup = rmcp::model::Tool::new("search web", "same sanitised name", schema.clone());
        assert!(host.register_external_tools("my server", vec![tool.clone(), dup]));
        let tools = host.tools();
        let names: Vec<&str> = tools.iter().map(|t| t.exposed.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "read_file",
                "bad_schema",
                "my_server__search_web",
                "my_server__search_web_2"
            ]
        );
        let search = &tools[2];
        assert_eq!(search.original, "search.web");
        assert_eq!(search.def.name, "my_server__search_web");
        assert_eq!(search.def.description, "Search \\u{202E}the web");
        assert_eq!(search.def.destructive, Some(true));
        // A re-listing keeps existing names, even when the order changes.
        let renamed = rmcp::model::Tool::new("search web", "changed", schema.clone());
        assert!(host.register_external_tools("my server", vec![renamed, tool]));
        let again = host.tools();
        let search_again = again.iter().find(|t| t.original == "search.web").unwrap();
        assert_eq!(search_again.exposed, "my_server__search_web");
        assert_eq!(search_again.fingerprint, search.fingerprint);
        let other = again.iter().find(|t| t.original == "search web").unwrap();
        assert_eq!(other.exposed, "my_server__search_web_2");
        assert_ne!(
            other.fingerprint, tools[3].fingerprint,
            "the description changed"
        );

        // The generic describer is used for external tools.
        let a = args(json!({"q": "rust"}));
        let Described::Action(action) = host
            .describe(
                "my_server__search_web",
                &a,
                &ChatContext::default(),
                ToolPrefs::default(),
            )
            .await
        else {
            panic!("an action")
        };
        assert_eq!(action.risk, policy::Risk::AlwaysAsk);
        assert!(action.outbound && !action.read_only);
        assert_eq!(
            action.scope,
            policy::Scope::ExternalExact {
                server: "my server".into(),
                tool: "my_server__search_web".into(),
                args: r#"{"q":"rust"}"#.into()
            }
        );
        // Not connected: the call fails readably, and a mismatched action is refused first.
        let out = host.call("my_server__search_web", &a, &action, ctx()).await;
        assert!(out.is_error);
        assert!(
            out.joined_text().contains("not available"),
            "{}",
            out.joined_text()
        );
        let other_args = args(json!({"q": "go"}));
        let out = host
            .call("my_server__search_web", &other_args, &action, ctx())
            .await;
        assert!(
            out.joined_text().contains("does not match the action"),
            "{}",
            out.joined_text()
        );
    }

    /// An external server registered without a process, for registry tests.
    fn unconnected(host: &Host, name: &str) {
        let ext = Arc::new(External {
            name: name.into(),
            config: ServerConfig::Http {
                url: "https://h.example".into(),
                headers: BTreeMap::new(),
            },
            kind: ServerKind::Http,
            log: Arc::new(StderrLog::new(name, None)),
            status: Mutex::new(ExternalStatus {
                state: ServerState::Ready,
                pid: None,
                protocol: None,
                lifecycle: None,
                remembered: None,
                restart_pending: false,
                generation: 1,
            }),
            slot: tokio::sync::Mutex::new(None),
            stop: CancellationToken::new(),
            order: 0,
        });
        host.inner
            .registry()
            .servers
            .insert(name.into(), ServerSlot::External(ext));
    }

    fn exposed_of(host: &Host, original: &str) -> String {
        host.tools()
            .into_iter()
            .find(|t| t.original == original)
            .map(|t| t.exposed)
            .unwrap_or_else(|| panic!("no tool {original}"))
    }

    #[tokio::test]
    async fn a_new_tool_never_takes_a_surviving_tools_name() {
        let host = Host::new(HostOptions::default());
        unconnected(&host, "s");
        let schema = Arc::new(Map::new());
        let tool = |name: &'static str| rmcp::model::Tool::new(name, "", schema.clone());
        assert!(host.register_external_tools("s", vec![tool("a.b")]));
        assert_eq!(exposed_of(&host, "a.b"), "s__a_b");
        // A new tool with the same sanitised name, listed before the old one.
        assert!(host.register_external_tools("s", vec![tool("a b"), tool("a.b")]));
        assert_eq!(exposed_of(&host, "a.b"), "s__a_b");
        assert_eq!(exposed_of(&host, "a b"), "s__a_b_2");
        // Once the old tool is gone, its name is free again.
        assert!(host.register_external_tools("s", vec![tool("a b")]));
        assert_eq!(exposed_of(&host, "a b"), "s__a_b_2");
        assert!(host.register_external_tools("s", vec![tool("a b"), tool("a?b")]));
        assert_eq!(exposed_of(&host, "a?b"), "s__a_b");
    }

    #[tokio::test]
    async fn an_oversized_schema_is_not_kept() {
        let host = Host::new(HostOptions::default());
        unconnected(&host, "s");
        let mut properties = Map::new();
        for i in 0..20_000 {
            properties.insert(format!("field_{i}"), json!({"type": "string"}));
        }
        let mut huge = Map::new();
        huge.insert("type".into(), json!("object"));
        huge.insert("properties".into(), Value::Object(properties));
        let tool = rmcp::model::Tool::new("big", "", Arc::new(huge.clone()));
        assert!(
            host.register_external_tools_async("s", vec![tool.clone()])
                .await
        );
        let registered = &host.tools()[0];
        assert!(
            registered
                .schema_error
                .as_deref()
                .is_some_and(|e| e.contains("larger than")),
            "{:?}",
            registered.schema_error
        );
        assert_eq!(registered.def.input_schema, json!({"type": "object"}));
        // The fingerprint still covers the real schema.
        assert_eq!(
            registered.fingerprint,
            fingerprint("big", "", &Value::Object(huge), None)
        );
        assert!(!host.tool_ready("s__big"));
        assert_eq!(host.llama_schema("s__big"), None);
    }

    #[tokio::test]
    async fn an_approval_does_not_carry_over_to_another_tool_under_the_same_name() {
        let host = Host::new(HostOptions::default());
        unconnected(&host, "s");
        let schema = Arc::new(Map::new());
        let tool = |name: &'static str| rmcp::model::Tool::new(name, "", schema.clone());
        assert!(host.register_external_tools("s", vec![tool("a.b")]));
        let a = args(json!({"q": 1}));
        let Described::Action(action) = host
            .describe("s__a_b", &a, &ChatContext::default(), ToolPrefs::default())
            .await
        else {
            panic!("an action")
        };
        // The server re-lists: "a.b" is gone and "a b" now sits under the exposed name
        // the card was shown for.
        assert!(host.register_external_tools("s", vec![tool("a b")]));
        assert_eq!(exposed_of(&host, "a b"), "s__a_b");
        let out = host.call("s__a_b", &a, &action, ctx()).await;
        assert!(
            out.joined_text().contains("does not match the action"),
            "{}",
            out.joined_text()
        );
    }

    #[tokio::test]
    async fn a_built_in_displaces_an_external_tool_with_its_name() {
        struct Clash;
        #[async_trait]
        impl BuiltinServer for Clash {
            fn name(&self) -> &str {
                "clash"
            }
            fn tools(&self) -> Vec<ToolDef> {
                vec![ToolDef {
                    name: "ext__tool".into(),
                    description: String::new(),
                    input_schema: json!({"type": "object"}),
                    destructive: None,
                    read_only: None,
                }]
            }
            async fn describe(
                &self,
                _: &str,
                _: &Map<String, Value>,
                _: &ChatContext,
            ) -> Described {
                Described::Invalid("unused".into())
            }
            async fn call(
                &self,
                _: &str,
                _: &Map<String, Value>,
                _: &policy::Action,
                _: CallContext,
            ) -> ToolOutput {
                ToolOutput::text("built-in")
            }
        }
        let host = Host::new(HostOptions::default());
        let ext = Arc::new(External {
            name: "ext".into(),
            config: ServerConfig::Http {
                url: "https://h.example".into(),
                headers: BTreeMap::new(),
            },
            kind: ServerKind::Http,
            log: Arc::new(StderrLog::new("ext", None)),
            status: Mutex::new(ExternalStatus {
                state: ServerState::Ready,
                pid: None,
                protocol: None,
                lifecycle: None,
                remembered: None,
                restart_pending: false,
                generation: 1,
            }),
            slot: tokio::sync::Mutex::new(None),
            stop: CancellationToken::new(),
            order: 0,
        });
        host.inner
            .registry()
            .servers
            .insert("ext".into(), ServerSlot::External(ext));
        let schema = Arc::new(Map::new());
        host.register_external_tools("ext", vec![rmcp::model::Tool::new("tool", "", schema)]);
        assert_eq!(host.tools()[0].exposed, "ext__tool");
        host.add_builtin(Arc::new(Clash));
        let tools = host.tools();
        let builtin = tools.iter().find(|t| t.server == "clash").unwrap();
        assert_eq!(builtin.exposed, "ext__tool");
        let moved = tools.iter().find(|t| t.server == "ext").unwrap();
        assert_eq!(moved.exposed, "ext__tool_2");
        assert_eq!(moved.def.name, "ext__tool_2");
    }

    #[tokio::test]
    async fn adding_and_removing_validates_names() {
        let host = Host::new(HostOptions::default());
        let cfg = ServerConfig::Stdio {
            command: "definitely-not-a-real-program-xyz".into(),
            args: vec![],
            env: BTreeMap::new(),
            cwd: None,
        };
        assert!(matches!(
            host.add_external("  ", cfg.clone()).await,
            Err(HostError::InvalidName(_))
        ));
        let err = host.add_external("missing", cfg.clone()).await.unwrap_err();
        assert!(matches!(err, HostError::CommandNotFound { .. }), "{err:?}");
        let status = &host.servers()[0];
        assert!(
            matches!(&status.state, ServerState::Failed { reason } if reason.contains("was not found"))
        );
        assert!(matches!(
            host.add_external("missing", cfg).await,
            Err(HostError::AlreadyExists(_))
        ));
        host.remove("missing").await.unwrap();
        assert!(host.servers().is_empty());
        assert!(matches!(
            host.remove("missing").await,
            Err(HostError::NotFound(_))
        ));
        assert!(matches!(
            host.restart("missing").await,
            Err(HostError::NotFound(_))
        ));
        host.shutdown().await;
        let cfg = ServerConfig::Http {
            url: "https://h.example".into(),
            headers: BTreeMap::new(),
        };
        assert_eq!(
            host.add_external("late", cfg).await,
            Err(HostError::ShuttingDown)
        );
    }

    #[tokio::test]
    async fn missing_secrets_fail_the_start_readably() {
        let host = Host::new(HostOptions::default());
        let cfg = ServerConfig::Http {
            url: "https://h.example/mcp".into(),
            headers: BTreeMap::from([(
                "Authorization".to_owned(),
                "Bearer ${secret:gh}".to_owned(),
            )]),
        };
        let err = host.add_external("remote", cfg).await.unwrap_err();
        assert_eq!(err.to_string(), "the secret \"gh\" is not in the keychain");
        assert!(matches!(
            &host.servers()[0].state,
            ServerState::Failed { .. }
        ));
    }

    #[test]
    fn fingerprints_cover_every_field() {
        let schema = json!({"type": "object"});
        let base = fingerprint("t", "d", &schema, None);
        assert_ne!(base, fingerprint("t2", "d", &schema, None));
        assert_ne!(base, fingerprint("t", "d2", &schema, None));
        assert_ne!(
            base,
            fingerprint("t", "d", &json!({"type": "object", "x": 1}), None)
        );
        assert_ne!(base, fingerprint("t", "d", &schema, Some(true)));
        assert_ne!(
            fingerprint("ab", "c", &schema, None),
            fingerprint("a", "bc", &schema, None)
        );
        assert_eq!(base, fingerprint("t", "d", &schema, None));
    }

    #[test]
    fn descriptions_are_capped_and_made_visible() {
        let long = "x".repeat(MAX_DESCRIPTION_CHARS + 10);
        let capped = cap_description(&long);
        assert_eq!(capped.chars().count(), MAX_DESCRIPTION_CHARS);
        assert!(capped.ends_with('…'));
        assert_eq!(cap_description(" a\u{200B}b "), "a\\u{200B}b");
    }

    #[test]
    fn the_host_is_send_sync_and_its_futures_are_send() {
        fn assert_send_sync<T: Send + Sync>() {}
        fn assert_send<T: Send>(_: &T) {}
        assert_send_sync::<Host>();
        let host = Host::new(HostOptions::default());
        let a = Map::new();
        let action = policy::Action {
            server: String::new(),
            tool: String::new(),
            summary: String::new(),
            preview: String::new(),
            scope: policy::Scope::Skill,
            risk: policy::Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Value::Null,
        };
        let chat = ChatContext::default();
        assert_send(&host.call("x", &a, &action, ctx()));
        assert_send(&host.describe("x", &a, &chat, ToolPrefs::default()));
        assert_send(&host.add_external(
            "x",
            ServerConfig::Http {
                url: String::new(),
                headers: BTreeMap::new(),
            },
        ));
        assert_send(&host.shutdown());
    }
}
