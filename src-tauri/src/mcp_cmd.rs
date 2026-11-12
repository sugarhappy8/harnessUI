//! Settings → MCP servers, and the per-chat tools panel (ARCHITECTURE.md 9.2, 9.3, 13.4).
//!
//! - A pasted config is parsed here, in Rust; the webview only picks which servers to add.
//! - Every value in `env` and `headers`, and each credential found in `args` or a URL, is
//!   stored in the OS keychain; the saved config keeps `${secret:<name>}` placeholders.
//! - Tools of a new server start off (D10). A tool whose fingerprint changed starts off
//!   again and loses its grants (9.2).
//! - A new chat starts with the file, skill, artifact and web search tools, and with the
//!   browser tools when a browser is available and the model's context is at least 8,192
//!   tokens (D10 as the owner changed it on 2026-11-11, artifacts and search added in
//!   0.0.4). Every other built-in group starts off, the Subagents group (`run_subagent`)
//!   included. [`Selection`] is the one place that decides this, for the Tools panel and
//!   for the turn.
//! - The companion tools (`find_tools`, `call_tool`, `read_output`, `update_task`) are not
//!   switches: they come along whenever at least one other tool is on in the chat
//!   ([`companions_follow`]).
//! - Which tools are ON is decided here, by the user's switches only. Which of those a
//!   turn SHOWS the model is decided in `tool_select.rs`, which may change what a chat
//!   remembers as shown and found ([`change_selection_memory`]) and nothing else.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use mcp_host::{ServerConfig, ServerKind, ServerState, ToolPrefs};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::toolset::mcp_secret_key;

/// Setting: the tools of the last chat whose tools the user changed (new chats start so).
/// The name carries a version: the defaults changed on 2026-11-11 (Browser on when it
/// fits) and again in 0.0.4 (Artifacts and Web search on), and a last choice saved before
/// a change would keep every new chat on the old set without the user ever having decided
/// against the new groups. Old saved choices under [`EARLIER_DEFAULT_TOOLS`] are simply no
/// longer read. A chat with its own saved choice keeps it: its new groups start off.
const DEFAULT_TOOLS: &str = "defaultChatTools3";

/// The names the last choice was saved under before the defaults changed.
#[cfg(test)]
const EARLIER_DEFAULT_TOOLS: &[&str] = &["defaultChatTools", "defaultChatTools2"];

/// Groups that are switched on for one chat only: the saved last choice never turns them
/// on in a new chat. Both let the model act widely (the whole screen; helpers that work on
/// their own), so each chat gets its own decision.
const PER_CHAT_ONLY: &[&str] = &[computer::SERVER_NAME, crate::subagent::SERVER_NAME];

/// Built-in servers whose tools are on in every new chat (D10; artifacts and search
/// since 0.0.4).
const DEFAULT_ON_BUILTIN: &[&str] = &["files", "skills", "artifacts", "search"];

/// The built-in browser server's name.
const BROWSER_SERVER: &str = "browser";

/// The Browser group is also on in a new chat when a browser is available and the chat
/// model's context is at least this many tokens (D10 as the owner changed it on
/// 2026-11-11). Its ten tools cost about 1,400 tokens on every request, which a 4,096
/// window has no room for.
pub const BROWSER_DEFAULT_MIN_CONTEXT: u32 = 8192;

/// What is known about the chat model's context when a new chat's tools are decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownContext {
    /// No model is chosen for the chat.
    NoModel,
    /// A local model: the context it runs with when it is the loaded model, and the
    /// context setting the next load asks for.
    Local {
        loaded: Option<u32>,
        configured: u32,
    },
    /// A remote endpoint: the context it reports for the model, if any, and the
    /// `remoteContextLength` setting used when it reports none.
    Remote { reported: Option<u32>, setting: u32 },
}

impl KnownContext {
    /// The context size to decide on, in tokens.
    pub fn tokens(self) -> Option<u32> {
        match self {
            Self::NoModel => None,
            Self::Local { loaded, configured } => Some(loaded.unwrap_or(configured)),
            Self::Remote { reported, setting } => Some(reported.unwrap_or(setting)),
        }
    }
}

/// Whether a built-in server's tools are on in a new chat that has no saved choice.
fn default_on(server: &str, browser_available: bool, context: Option<u32>) -> bool {
    DEFAULT_ON_BUILTIN.contains(&server)
        || (server == BROWSER_SERVER
            && browser_available
            && context.is_some_and(|n| n >= BROWSER_DEFAULT_MIN_CONTEXT))
}

/// Which tools are on in the list being built. The Tools panel and the turn both go
/// through this, so they cannot disagree.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Selection<'a> {
    /// No chat (a server's list in Settings): nothing is "on in the chat".
    NoChat,
    /// The chat's own saved choice.
    Chat(&'a [String]),
    /// A chat without its own choice, once the user has changed tools in any chat: that
    /// last choice (9.3).
    LastChoice(&'a [String]),
    /// A chat without its own choice, and no last choice: the defaults.
    Defaults {
        browser_available: bool,
        /// The model's context in tokens, when it is known or can be assumed.
        context: Option<u32>,
    },
}

impl<'a> Selection<'a> {
    /// A chat's own choice wins, then the last choice, then the defaults.
    fn for_chat(
        own: Option<&'a [String]>,
        last_choice: Option<&'a [String]>,
        browser_available: bool,
        context: Option<u32>,
    ) -> Self {
        match (own, last_choice) {
            (Some(list), _) => Self::Chat(list),
            (None, Some(list)) => Self::LastChoice(list),
            (None, None) => Self::Defaults {
                browser_available,
                context,
            },
        }
    }

    /// Whether one tool is on.
    fn on(&self, exposed: &str, server: &str, builtin: bool) -> bool {
        match self {
            Self::NoChat => false,
            Self::Chat(list) => list.iter().any(|e| e == exposed),
            Self::LastChoice(list) => {
                !PER_CHAT_ONLY.contains(&server) && list.iter().any(|e| e == exposed)
            }
            Self::Defaults {
                browser_available,
                context,
            } => builtin && default_on(server, *browser_available, *context),
        }
    }
}

/// What a remote endpoint last reported as a model's context when the tool list asked
/// (`None`: it reports none).
#[derive(Debug, Clone, Copy)]
struct SeenContext {
    tokens: Option<u32>,
    at: Instant,
}

/// By model reference. In memory only.
static REMOTE_CONTEXT: Mutex<BTreeMap<String, SeenContext>> = Mutex::new(BTreeMap::new());

/// How long a report is used before the endpoint is asked again.
const CONTEXT_FRESH: Duration = Duration::from_secs(60);

/// How long the tool list waits for an endpoint to report its context.
const CONTEXT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);

fn seen_context(model_ref: &str) -> Option<SeenContext> {
    REMOTE_CONTEXT.lock().ok()?.get(model_ref).copied()
}

/// Also called by a turn with what the endpoint just reported, so the Tools panel decides
/// on the same number the turn used.
pub(crate) fn remember_context(model_ref: &str, tokens: Option<u32>) {
    if let Ok(mut map) = REMOTE_CONTEXT.lock() {
        // A handful of models at most; never let it grow without bound.
        if map.len() > 256 {
            map.clear();
        }
        map.insert(
            model_ref.to_string(),
            SeenContext {
                tokens,
                at: Instant::now(),
            },
        );
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerDto {
    pub id: Option<i64>,
    pub name: String,
    pub kind: ServerKind,
    pub state: ServerState,
    pub enabled: bool,
    pub builtin: bool,
    /// The command line or the redacted URL.
    pub summary: String,
    pub protocol_version: Option<String>,
    pub stderr_tail: Vec<String>,
    pub tool_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FindingDto {
    pub what: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPreview {
    pub name: String,
    pub kind: String,
    pub summary: String,
    pub exists: bool,
    pub findings: Vec<FindingDto>,
    pub env_keys: Vec<String>,
    pub header_keys: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDto {
    pub exposed: String,
    pub server: String,
    pub original: String,
    pub description: String,
    pub builtin: bool,
    pub enabled_in_chat: bool,
    /// External tools: the user switched it on in the server's list and has seen this
    /// version of it. Always true for built-in tools.
    pub available: bool,
    /// External tools: changed since the user last saw it.
    pub changed: bool,
    pub allow_any_args: bool,
    pub always_ask: bool,
    pub destructive: bool,
    pub ready: bool,
    pub schema_error: Option<String>,
    /// Rough cost of offering the tool, in tokens.
    pub token_estimate: u32,
    /// One of the tools that come along with any other tool (`find_tools`, `call_tool`,
    /// `read_output`, `update_task`). Not a switch: `enabled_in_chat` follows the others.
    pub companion: bool,
    /// On, and in the list the model gets whatever the next message says (adaptive tool
    /// selection, `tool_select.rs`). A tool that is on but not shown is "on request": the
    /// model finds it with `find_tools` and runs it with `call_tool`, or gets it in its
    /// list when a message is about its group. Always false for a tool that is not offered.
    pub shown: bool,
}

fn summary_of(cfg: &ServerConfig) -> (String, String) {
    match cfg {
        ServerConfig::Stdio { command, args, .. } => {
            let mut s = command.clone();
            for a in args {
                s.push(' ');
                if a.contains(' ') {
                    s.push('"');
                    s.push_str(a);
                    s.push('"');
                } else {
                    s.push_str(a);
                }
            }
            ("stdio".into(), s)
        }
        ServerConfig::Http { url, .. } => ("http".into(), mcp_host::redact_url(url)),
        ServerConfig::LegacySse { url, .. } => ("sse".into(), mcp_host::redact_url(url)),
    }
}

/// A prefix for this server's secret names, made of placeholder-safe characters.
fn secret_prefix(server: &str) -> String {
    let p: String = server
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(48)
        .collect();
    if p.is_empty() { "server".into() } else { p }
}

/// The existing server whose secrets `name` would share: secret names are made from
/// [`secret_prefix`], which maps different names ("a b", "a_b") to the same prefix.
fn prefix_clash<'a>(name: &str, existing: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let prefix = secret_prefix(name);
    existing
        .into_iter()
        .find(|other| *other != name && secret_prefix(other) == prefix)
}

fn safe_key(k: &str) -> String {
    k.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

fn is_placeholder(v: &str) -> bool {
    let names = mcp_host::secrets::placeholder_names(v);
    names.len() == 1 && v.trim() == mcp_host::secrets::placeholder(&names[0])
}

/// Moves secrets out of `cfg` into the keychain and returns the config to store.
fn secure_config(
    state: &AppState,
    server: &str,
    cfg: &ServerConfig,
    move_found: bool,
) -> AppResult<ServerConfig> {
    let prefix = secret_prefix(server);
    let mut stored: Vec<String> = Vec::new();
    let mut put = |name: &str, value: &str| -> AppResult<String> {
        state
            .secrets
            .set(&mcp_secret_key(name), value)
            .map_err(|e| {
                AppError::new(
                    "keychain",
                    "The secret could not be saved in the system keychain.",
                    "Check that the keychain is unlocked and try again.",
                    e.to_string(),
                )
            })?;
        stored.push(name.to_string());
        Ok(mcp_host::secrets::placeholder(name))
    };
    let mut cfg = cfg.clone();
    if move_found {
        let mut findings = mcp_host::scan_credentials(&cfg);
        for f in &mut findings {
            let name = format!("{prefix}.{}", f.suggested_name);
            f.placeholder = put(&name, &f.value)?;
            f.suggested_name = name;
        }
        cfg = mcp_host::apply_findings(&cfg, &findings);
    }
    match &mut cfg {
        ServerConfig::Stdio { env, .. } => {
            for (k, v) in env.iter_mut() {
                if !v.is_empty() && !is_placeholder(v) {
                    *v = put(&format!("{prefix}.env.{}", safe_key(k)), v)?;
                }
            }
        }
        ServerConfig::Http { headers, .. } | ServerConfig::LegacySse { headers, .. } => {
            for (k, v) in headers.iter_mut() {
                if !v.is_empty() && !is_placeholder(v) {
                    *v = put(&format!("{prefix}.header.{}", safe_key(k)), v)?;
                }
            }
        }
    }
    Ok(cfg)
}

/// A pasted config may not refer to saved secrets. Its own values are moved to the
/// keychain when it is added; a `${secret:…}` placeholder in it would instead read a
/// secret saved for any server and send it to this one (another server's token in this
/// server's header or URL). Stored configs keep the placeholders [`secure_config`] made.
fn refuse_secret_placeholders(entries: &[mcp_host::ServerEntry]) -> AppResult<()> {
    for e in entries {
        let value = serde_json::to_value(&e.config).map_err(AppError::internal)?;
        if !secret_names(&value).is_empty() {
            return Err(AppError::new(
                "mcp_config",
                format!("\"{}\" refers to a saved secret (${{secret:…}}).", e.name),
                "Paste the real value instead. It is moved to the system keychain when you add the server.",
                String::new(),
            ));
        }
    }
    Ok(())
}

/// The secrets a stored config refers to that belong to this server (the ones
/// [`secure_config`] saved under its prefix), so removing it never deletes another's:
/// prefixes are unique, because [`add_mcp_servers`] refuses a name whose prefix another
/// server already has ([`prefix_clash`]).
fn own_secret_names(server: &str, config: &Value) -> Vec<String> {
    let own = format!("{}.", secret_prefix(server));
    secret_names(config)
        .into_iter()
        .filter(|s| s.starts_with(&own))
        .collect()
}

/// Every `${secret:…}` name used in a stored config.
fn secret_names(config: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::String(s) => out.extend(mcp_host::secrets::placeholder_names(s)),
            Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            Value::Object(o) => o.values().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    walk(config, &mut out);
    out
}

#[tauri::command]
pub async fn list_mcp_servers(state: State<'_, AppState>) -> AppResult<Vec<McpServerDto>> {
    let stored = state.store.list_mcp_servers().await?;
    let live = state.host.servers();
    let mut out = Vec::new();
    for s in &live {
        let row = stored.iter().find(|r| r.name == s.name);
        let summary = row
            .and_then(|r| serde_json::from_value::<ServerConfig>(r.config.clone()).ok())
            .map(|c| summary_of(&c).1)
            .unwrap_or_else(|| {
                if s.kind == ServerKind::Builtin {
                    "Built in".into()
                } else {
                    String::new()
                }
            });
        out.push(McpServerDto {
            id: row.map(|r| r.id.0),
            name: s.name.clone(),
            kind: s.kind,
            state: s.state.clone(),
            enabled: row.map(|r| r.enabled).unwrap_or(true),
            builtin: s.kind == ServerKind::Builtin,
            summary,
            protocol_version: s.protocol_version.clone(),
            stderr_tail: s.stderr_tail.clone(),
            tool_count: s.tool_count,
        });
    }
    // Saved but switched off: not in the host.
    for r in stored
        .iter()
        .filter(|r| !live.iter().any(|s| s.name == r.name))
    {
        let cfg = serde_json::from_value::<ServerConfig>(r.config.clone()).ok();
        let (kind, summary) = cfg.as_ref().map(summary_of).unwrap_or_default();
        out.push(McpServerDto {
            id: Some(r.id.0),
            name: r.name.clone(),
            kind: if kind == "stdio" {
                ServerKind::Stdio
            } else {
                ServerKind::Http
            },
            state: ServerState::Stopped,
            enabled: r.enabled,
            builtin: false,
            summary,
            protocol_version: None,
            stderr_tail: Vec::new(),
            tool_count: 0,
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn preview_mcp_config(
    state: State<'_, AppState>,
    json: String,
) -> AppResult<Vec<ImportPreview>> {
    let entries = mcp_host::parse_config(&json).map_err(|e| {
        AppError::new(
            "mcp_config",
            "That configuration could not be read.",
            "Paste an mcpServers or servers JSON object.",
            e.to_string(),
        )
    })?;
    refuse_secret_placeholders(&entries)?;
    let names: BTreeSet<String> = state
        .host
        .servers()
        .into_iter()
        .map(|s| s.name)
        .chain(
            state
                .store
                .list_mcp_servers()
                .await?
                .into_iter()
                .map(|r| r.name),
        )
        .collect();
    Ok(entries
        .into_iter()
        .map(|e| {
            let (kind, summary) = summary_of(&e.config);
            let findings = mcp_host::scan_credentials(&e.config)
                .into_iter()
                .map(|f| FindingDto {
                    what: f.what.clone(),
                    name: f.suggested_name.clone(),
                })
                .collect();
            let (env_keys, header_keys) = match &e.config {
                ServerConfig::Stdio { env, .. } => (env.keys().cloned().collect(), Vec::new()),
                ServerConfig::Http { headers, .. } | ServerConfig::LegacySse { headers, .. } => {
                    (Vec::new(), headers.keys().cloned().collect())
                }
            };
            // Show the command line with the found credentials already replaced.
            let shown = mcp_host::apply_findings(&e.config, &mcp_host::scan_credentials(&e.config));
            let summary = if kind == "stdio" {
                summary_of(&shown).1
            } else {
                summary
            };
            ImportPreview {
                exists: names.contains(&e.name),
                name: e.name,
                kind,
                summary,
                findings,
                env_keys,
                header_keys,
            }
        })
        .collect())
}

/// Adds the chosen servers from a pasted config. The user has seen the "runs with your
/// permissions" warning (the UI shows it before this call).
#[tauri::command]
pub async fn add_mcp_servers(
    app: AppHandle,
    state: State<'_, AppState>,
    json: String,
    names: Vec<String>,
    move_credentials: bool,
) -> AppResult<()> {
    let entries = mcp_host::parse_config(&json).map_err(|e| {
        AppError::new(
            "mcp_config",
            "That configuration could not be read.",
            "Check the JSON.",
            e.to_string(),
        )
    })?;
    // Checked for every server in the paste before any is added.
    refuse_secret_placeholders(&entries)?;
    let existing: BTreeSet<String> = state
        .host
        .servers()
        .into_iter()
        .map(|s| s.name)
        .chain(
            state
                .store
                .list_mcp_servers()
                .await?
                .into_iter()
                .map(|r| r.name),
        )
        .collect();
    let chosen: Vec<_> = entries
        .into_iter()
        .filter(|e| names.contains(&e.name))
        .collect();
    // Checked for every chosen server before any is added, against the existing ones and
    // each other.
    for (i, e) in chosen.iter().enumerate() {
        if existing.contains(&e.name) {
            return Err(AppError::new(
                "mcp_exists",
                format!("A server named \"{}\" already exists.", e.name),
                "Remove the old one first, or rename this one in the JSON.",
                String::new(),
            ));
        }
        let others = existing
            .iter()
            .map(String::as_str)
            .chain(chosen[..i].iter().map(|c| c.name.as_str()));
        if let Some(other) = prefix_clash(&e.name, others) {
            return Err(AppError::new(
                "mcp_name_clash",
                format!(
                    "The names \"{}\" and \"{other}\" are too similar: the two servers would share saved secrets.",
                    e.name
                ),
                "Rename one of them in the JSON.",
                String::new(),
            ));
        }
    }
    for e in chosen {
        let cfg = secure_config(&state, &e.name, &e.config, move_credentials)?;
        let value = serde_json::to_value(&cfg).map_err(AppError::internal)?;
        let id = state
            .store
            .upsert_mcp_server(store::McpServerInput {
                name: e.name.clone(),
                config: value,
                enabled: true,
            })
            .await?;
        let shown = serde_json::to_value(&cfg).map_err(AppError::internal)?;
        let _ = state
            .store
            .acknowledge_mcp_server(id, shown, chrono::Utc::now().timestamp_millis())
            .await;
        let host = state.host.clone();
        let name = e.name.clone();
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(err) = host.add_external(&name, cfg).await {
                tracing::warn!(server = %name, "MCP server did not start: {err}");
            }
            let _ = tauri::Emitter::emit_to(&app, crate::MAIN_WINDOW, "mcp-changed", name);
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn remove_mcp_server(state: State<'_, AppState>, name: String) -> AppResult<()> {
    if state
        .host
        .servers()
        .iter()
        .any(|s| s.name == name && s.kind == ServerKind::Builtin)
    {
        return Err(AppError::bad_request(
            "Built-in tools cannot be removed; switch them off per chat instead.",
        ));
    }
    let _ = state.host.remove(&name).await;
    if let Some(row) = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|r| r.name == name)
    {
        for secret in own_secret_names(&name, &row.config) {
            let _ = state.secrets.delete(&mcp_secret_key(&secret));
        }
        state.store.delete_mcp_server(row.id).await?;
    }
    Ok(())
}

#[tauri::command]
pub async fn set_mcp_server_enabled(
    state: State<'_, AppState>,
    name: String,
    enabled: bool,
) -> AppResult<()> {
    let row = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|r| r.name == name)
        .ok_or_else(|| AppError::not_found("MCP server"))?;
    state
        .store
        .upsert_mcp_server(store::McpServerInput {
            name: row.name.clone(),
            config: row.config.clone(),
            enabled,
        })
        .await?;
    if enabled {
        let cfg: ServerConfig = serde_json::from_value(row.config).map_err(AppError::internal)?;
        let host = state.host.clone();
        tauri::async_runtime::spawn(async move {
            let _ = host.add_external(&name, cfg).await;
        });
    } else {
        let _ = state.host.remove(&name).await;
    }
    Ok(())
}

#[tauri::command]
pub async fn restart_mcp_server(state: State<'_, AppState>, name: String) -> AppResult<()> {
    let host = state.host.clone();
    tauri::async_runtime::spawn(async move {
        let _ = host.restart(&name).await;
    });
    Ok(())
}

/// The user's switches for one external tool.
pub async fn tool_prefs_for(state: &AppState, server: &str, original: &str) -> ToolPrefs {
    match stored_pref(state, server, original).await {
        Some(p) => ToolPrefs {
            allow_any_args: p.allow_any_args,
            always_ask: p.always_ask,
        },
        None => ToolPrefs::default(),
    }
}

async fn stored_pref(state: &AppState, server: &str, original: &str) -> Option<store::McpToolPref> {
    let row = state
        .store
        .list_mcp_servers()
        .await
        .ok()?
        .into_iter()
        .find(|r| r.name == server)?;
    state
        .store
        .mcp_tool_prefs(row.id)
        .await
        .ok()?
        .into_iter()
        .find(|p| p.tool_name == original)
}

/// Rough token cost of offering a tool: name, description and schema at ~3.5 chars/token.
fn token_estimate(t: &mcp_host::RegisteredTool) -> u32 {
    let chars =
        t.exposed.len() + t.def.description.len() + t.def.input_schema.to_string().len() + 40;
    (chars as f64 / 3.5).ceil() as u32
}

/// Tools that are on in a chat, given its stored preferences. `turn_context` is the
/// context, in tokens, the turn asking will run with; the defaults of a new chat depend
/// on it (see [`Selection`]).
pub async fn enabled_for_chat(
    state: &AppState,
    chat: &store::Chat,
    turn_context: Option<u32>,
) -> Vec<mcp_host::RegisteredTool> {
    let list = chat_tool_list(state, chat, turn_context).await;
    list.into_iter()
        .filter(|(_, d)| d.offered())
        .map(|(t, _)| t)
        .collect()
}

impl ToolDto {
    /// Whether the tool is offered to the model: on in the chat, and able to work.
    fn offered(&self) -> bool {
        self.enabled_in_chat && self.available && self.schema_error.is_none()
    }

    /// Marks a tool that cannot work right now; the Tools panel shows `reason`.
    fn mark_unavailable(&mut self, reason: &str) {
        self.available = false;
        self.schema_error.get_or_insert_with(|| reason.to_string());
    }
}

/// The companion rule (0.0.4): `find_tools`, `call_tool`, `read_output` and `update_task`
/// are on in a chat exactly when at least one other tool is offered in it. They are never
/// switched by the user, and a saved choice that names them changes nothing.
///
/// With "choose tools for each task" off (`auto` false) every tool that is on is shown,
/// so there is nothing to find and nothing to call through: `find_tools` and `call_tool`
/// are then not offered, and a turn's tool list is what it was before 0.0.4.
fn companions_follow(tools: &mut [&mut ToolDto], auto: bool) {
    let another_is_on = tools.iter().any(|d| !d.companion && d.offered());
    for dto in tools.iter_mut().filter(|d| d.companion) {
        let finder = dto.server == crate::tool_select::finder::SERVER_NAME;
        dto.enabled_in_chat = another_is_on && (auto || !finder);
    }
}

/// The groups that exist but have no tool on, in the order of the list: at least one of
/// their tools could be switched on in the Tools panel, and none is. The companions are
/// not groups of their own.
fn groups_that_are_off<'a>(tools: impl Iterator<Item = &'a ToolDto>) -> Vec<String> {
    // (server, a tool could be switched on, a tool is on)
    let mut groups: Vec<(String, bool, bool)> = Vec::new();
    for dto in tools.filter(|d| !d.companion) {
        let usable = dto.available && dto.schema_error.is_none();
        let on = dto.enabled_in_chat;
        match groups
            .iter_mut()
            .find(|(server, _, _)| *server == dto.server)
        {
            Some(group) => {
                group.1 |= usable;
                group.2 |= on;
            }
            None => groups.push((dto.server.clone(), usable, on)),
        }
    }
    groups
        .into_iter()
        .filter(|(_, usable, on)| *usable && !*on)
        .map(|(server, _, _)| server)
        .collect()
}

/// The groups that are switched off in a chat, for `find_tools`: it may say that a group
/// is off, and that the user can switch it on. It cannot switch it on.
pub(crate) async fn off_groups(state: &AppState, chat_id: store::ChatId) -> Vec<String> {
    let Ok(Some(chat)) = state.store.get_chat(chat_id).await else {
        return Vec::new();
    };
    let list = chat_tool_list(state, &chat, None).await;
    groups_that_are_off(list.iter().map(|(_, dto)| dto))
}

/// The names a user's choice of tools is saved with: registered tools only, and never a
/// companion (they are not switches; see [`companions_follow`]).
fn user_switches(
    enabled: Vec<String>,
    registered: &[mcp_host::RegisteredTool],
    builtin: &BTreeSet<String>,
) -> Vec<String> {
    let is_switch = |name: &str| {
        registered.iter().any(|t| {
            t.exposed == name && !crate::tool_select::is_companion_server(&t.server, builtin)
        })
    };
    enabled.into_iter().filter(|e| is_switch(e)).collect()
}

/// One change at a time to a chat's stored tool choices: each is read, changed and
/// written back, and a turn (remembering what it showed) may do so while the user changes
/// a switch. Without the lock, the later write would undo the earlier change.
static PREFS_WRITES: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Reads a chat's stored tool choices, changes them and writes them back, as one step.
async fn change_chat_prefs(
    state: &AppState,
    chat_id: store::ChatId,
    change: impl FnOnce(&mut crate::turns::ChatTools),
) -> AppResult<()> {
    let _one_at_a_time = PREFS_WRITES.lock().await;
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let mut prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    change(&mut prefs);
    let value = serde_json::to_value(&prefs).map_err(AppError::internal)?;
    if value == chat.tool_prefs {
        return Ok(());
    }
    state
        .store
        .update_chat(
            chat_id,
            store::ChatPatch {
                tool_prefs: Some(value),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}

/// Changes what a chat remembers about the tool selection: the tools it was shown, and
/// the tools that wait to be shown (`tool_select.rs`). This is all a turn can change in a
/// chat's tool choices. Which tools are on is not within its reach: only the user's
/// switches ([`set_chat_tools`]) change that.
pub(crate) async fn change_selection_memory(
    state: &AppState,
    chat_id: store::ChatId,
    change: impl FnOnce(&mut Vec<String>, &mut Vec<String>),
) -> AppResult<()> {
    change_chat_prefs(state, chat_id, |prefs| {
        change(&mut prefs.shown, &mut prefs.found)
    })
    .await
}

async fn chat_tool_list(
    state: &AppState,
    chat: &store::Chat,
    turn_context: Option<u32>,
) -> Vec<(mcp_host::RegisteredTool, ToolDto)> {
    let prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    // The user's last choice of tools is the default for new chats (9.3).
    let last_choice: Option<Vec<String>> =
        state.store.get_setting(DEFAULT_TOOLS).await.ok().flatten();
    // No browser found: the browser tools cannot work (and are not on by default).
    let no_browser = crate::browser_cmd::unavailable_reason();
    let needs_defaults = prefs.enabled.is_none() && last_choice.is_none();
    // The context only matters for the browser default, so it is only looked up then.
    let context = if needs_defaults && no_browser.is_none() {
        match turn_context {
            Some(n) => Some(n),
            None => context_for_defaults(state, chat).await.tokens(),
        }
    } else {
        None
    };
    let selection = Selection::for_chat(
        prefs.enabled.as_deref(),
        last_choice.as_deref(),
        no_browser.is_none(),
        context,
    );
    let mut list = tool_list(state, &selection).await;
    // Computer use: only for a model that can see images.
    let no_computer = computer_unavailable(state, chat).await;
    for (_, dto) in &mut list {
        let why_not = match dto.server.as_str() {
            computer::SERVER_NAME => no_computer.as_deref(),
            BROWSER_SERVER => no_browser.as_deref(),
            _ => None,
        };
        if let Some(reason) = why_not {
            dto.mark_unavailable(reason);
        }
    }
    // The companions follow the other tools, once it is known which of those can work.
    let setting = crate::tool_select::settings(state).await;
    let mut dtos: Vec<&mut ToolDto> = list.iter_mut().map(|(_, dto)| dto).collect();
    companions_follow(&mut dtos, setting.auto);
    // Which of the offered tools the next turn shows whatever the user writes.
    let offered: Vec<crate::tool_select::Candidate> = list
        .iter()
        .filter(|(_, dto)| dto.offered())
        .map(|(tool, dto)| crate::tool_select::Candidate::of(tool, dto.builtin))
        .collect();
    // With the model's context known, the list is also held to what fits in it.
    let known_context = match turn_context {
        Some(n) => Some(n),
        None => context_for_defaults(state, chat).await.tokens(),
    };
    let room = match known_context {
        Some(n) => Some(crate::tool_select::allowance(
            n,
            crate::tool_select::other_fixed(state, chat).await,
        )),
        None => None,
    };
    let shown = crate::tool_select::shown_before_a_message(
        &offered,
        &prefs,
        &crate::tool_select::skill_words(state),
        !state.skills.loaded_in(chat.id.0).is_empty(),
        setting,
        room,
    );
    for (_, dto) in &mut list {
        dto.shown = dto.offered() && shown.contains(&dto.exposed);
    }
    list
}

/// What is known about the context of the chat's model without starting a turn.
async fn context_for_defaults(state: &AppState, chat: &store::Chat) -> KnownContext {
    let Some(model) = chat.model_ref.as_deref() else {
        return KnownContext::NoModel;
    };
    if let Some(path) = model.strip_prefix("local:") {
        let status = state.runtime.status();
        let loaded = (status.model_path.as_deref() == Some(path))
            .then_some(status.n_ctx)
            .flatten();
        let configured = state
            .store
            .get_setting::<crate::dto::LocalModelSettings>(crate::settings::LOCAL_MODEL)
            .await
            .ok()
            .flatten()
            .unwrap_or_default()
            .ctx;
        return KnownContext::Local { loaded, configured };
    }
    if model.starts_with("remote:") {
        let stored: Option<Value> = state
            .store
            .get_setting(crate::settings::REMOTE_CONTEXT_LENGTH)
            .await
            .ok()
            .flatten();
        return KnownContext::Remote {
            reported: remote_context(state, model).await,
            setting: crate::settings::remote_context_length(stored.as_ref()),
        };
    }
    KnownContext::NoModel
}

/// The context a remote endpoint reports for a model, asked the way a turn asks: its
/// report from the last minute, else its report now (waiting a few seconds at most), else
/// its last report when it does not answer.
async fn remote_context(state: &AppState, model_ref: &str) -> Option<u32> {
    let seen = seen_context(model_ref);
    if let Some(s) = seen
        && s.at.elapsed() < CONTEXT_FRESH
    {
        return s.tokens;
    }
    let lookup = async {
        let (id, model) = model_ref.strip_prefix("remote:")?.split_once(':')?;
        let id: i64 = id.parse().ok()?;
        let (client, _) = crate::providers::client_for(state, store::ProviderId(id))
            .await
            .ok()?;
        client.context_length(model).await.ok()
    };
    let tokens = match tokio::time::timeout(CONTEXT_LOOKUP_TIMEOUT, lookup).await {
        // The endpoint answered: its report, or none (then the setting decides).
        Ok(Some(reported)) => reported,
        // No answer: keep what was seen before, and do not ask again for a minute.
        _ => seen.and_then(|s| s.tokens),
    };
    remember_context(model_ref, tokens);
    tokens
}

/// Why the computer tools cannot be offered in this chat, if they cannot: only a model
/// that cannot see images. There is nothing to set up per model; the rule and its tests
/// are in `computer_cmd::unavailable_reason`.
async fn computer_unavailable(state: &AppState, chat: &store::Chat) -> Option<String> {
    let model = chat.model_ref.as_deref()?;
    let status = state.runtime.status();
    crate::computer_cmd::unavailable_reason(model, status.model_path.as_deref(), status.vision)
        .map(str::to_string)
}

async fn tool_list(
    state: &AppState,
    selection: &Selection<'_>,
) -> Vec<(mcp_host::RegisteredTool, ToolDto)> {
    let stored = state.store.list_mcp_servers().await.unwrap_or_default();
    let mut out = Vec::new();
    for t in state.host.tools() {
        let builtin = !stored.iter().any(|r| r.name == t.server);
        let (available, changed, allow_any_args, always_ask) = if builtin {
            // The skill tools are offered only while at least one skill is enabled.
            (
                t.server != "skills" || !state.skills.enabled().is_empty(),
                false,
                false,
                false,
            )
        } else {
            match stored_pref(state, &t.server, &t.original).await {
                Some(p) => {
                    let same = p.fingerprint.as_deref() == Some(t.fingerprint.as_str());
                    (p.enabled && same, !same, p.allow_any_args, p.always_ask)
                }
                None => (false, false, false, false),
            }
        };
        let enabled_in_chat = selection.on(&t.exposed, &t.server, builtin);
        let dto = ToolDto {
            exposed: t.exposed.clone(),
            server: t.server.clone(),
            original: t.original.clone(),
            description: t.def.description.clone(),
            builtin,
            enabled_in_chat,
            available,
            changed,
            allow_any_args,
            always_ask,
            destructive: t.def.destructive.unwrap_or(false),
            ready: state.host.tool_ready(&t.exposed),
            schema_error: t.schema_error.clone(),
            token_estimate: token_estimate(&t),
            companion: builtin
                && crate::tool_select::COMPANION_SERVERS.contains(&t.server.as_str()),
            // Decided per chat, in `chat_tool_list`.
            shown: false,
        };
        out.push((t, dto));
    }
    out
}

#[tauri::command]
pub async fn list_chat_tools(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
) -> AppResult<Vec<ToolDto>> {
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    Ok(chat_tool_list(&state, &chat, None)
        .await
        .into_iter()
        .map(|(_, d)| d)
        .collect())
}

/// Thinking on or off for one chat.
#[tauri::command]
pub async fn set_chat_thinking(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    on: bool,
) -> AppResult<()> {
    change_chat_prefs(&state, chat_id, |prefs| prefs.thinking_off = !on).await
}

/// One server's tools with the user's switches (Settings → MCP servers).
#[tauri::command]
pub async fn list_server_tools(
    state: State<'_, AppState>,
    server: String,
) -> AppResult<Vec<ToolDto>> {
    Ok(tool_list(&state, &Selection::NoChat)
        .await
        .into_iter()
        .map(|(_, d)| d)
        .filter(|d| d.server == server)
        .collect())
}

#[tauri::command]
pub async fn set_chat_tools(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    enabled: Vec<String>,
) -> AppResult<()> {
    let enabled = user_switches(
        enabled,
        &state.host.tools(),
        &crate::tool_catalog::builtin_servers(&state.host),
    );
    if state.store.get_chat(chat_id).await?.is_none() {
        return Err(AppError::not_found("chat"));
    }
    // The last choice becomes the default for new chats (9.3).
    state.store.set_setting(DEFAULT_TOOLS, &enabled).await?;
    change_chat_prefs(&state, chat_id, |prefs| prefs.enabled = Some(enabled)).await
}

/// The user's switches for one external tool in its server's list. Turning a tool on
/// records the version the user saw; if it had changed, its grants are removed.
#[tauri::command]
pub async fn set_mcp_tool_pref(
    state: State<'_, AppState>,
    server: String,
    tool: String,
    enabled: bool,
    allow_any_args: bool,
    always_ask: bool,
) -> AppResult<()> {
    let row = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|r| r.name == server)
        .ok_or_else(|| AppError::not_found("MCP server"))?;
    let current = state
        .host
        .tools()
        .into_iter()
        .find(|t| t.server == server && t.original == tool)
        .ok_or_else(|| AppError::not_found("tool"))?;
    let old = state
        .store
        .mcp_tool_prefs(row.id)
        .await?
        .into_iter()
        .find(|p| p.tool_name == tool);
    let changed =
        old.as_ref().and_then(|p| p.fingerprint.as_deref()) != Some(current.fingerprint.as_str());
    let narrowed = old
        .as_ref()
        .is_some_and(|p| p.allow_any_args && !allow_any_args);
    if changed || narrowed {
        revoke_tool_grants(&state, &server, &tool).await;
    }
    state
        .store
        .upsert_mcp_tool_pref(store::McpToolPref {
            server_id: row.id,
            tool_name: tool,
            enabled,
            allow_any_args,
            always_ask,
            fingerprint: Some(current.fingerprint),
        })
        .await?;
    Ok(())
}

/// Removes every chat grant for one external tool.
async fn revoke_tool_grants(state: &AppState, server: &str, tool: &str) {
    crate::turns::revoke_external_tool(state, server, tool);
}

/// Revokes this chat's grants ("Forget what I allowed in this chat").
#[tauri::command]
pub async fn clear_chat_grants(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
) -> AppResult<()> {
    // [0.0.4 allow all] What the user allowed in this chat includes Allow all: forgetting
    // it puts the chat back to asking first.
    crate::approvals::ask_first(chat_id.0);
    crate::turns::save_grants(&state, chat_id.0, &BTreeSet::new()).await
}

/// The last lines a server wrote to stderr.
#[tauri::command]
pub async fn mcp_server_log(app: AppHandle, name: String) -> AppResult<Vec<String>> {
    let state = app.state::<AppState>();
    Ok(state
        .host
        .servers()
        .into_iter()
        .find(|s| s.name == name)
        .map(|s| s.stderr_tail)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    /// D10 as changed on 2026-11-11: files and skills always; the browser when one is
    /// available and the context is at least 8,192 tokens; nothing else. Since 0.0.4
    /// also artifacts and web search, always.
    #[test]
    fn a_new_chat_starts_with_files_skills_and_the_browser_when_it_fits() {
        for (available, context, browser_on) in [
            (true, Some(8192), true),
            (true, Some(32768), true),
            (true, Some(8191), false),
            (true, Some(4096), false),
            (true, None, false),
            (false, Some(8192), false),
            (false, Some(131_072), false),
            (false, None, false),
        ] {
            assert_eq!(
                default_on("browser", available, context),
                browser_on,
                "available {available}, context {context:?}"
            );
            assert!(default_on("files", available, context));
            assert!(default_on("skills", available, context));
            assert!(default_on("artifacts", available, context));
            assert!(default_on("search", available, context));
            for off in ["terminal", "git", "computer", "weather"] {
                assert!(!default_on(off, available, context), "{off}");
            }
            // The companions are not on by a default of their own: they follow the others.
            for companion in crate::tool_select::COMPANION_SERVERS {
                assert!(!default_on(companion, available, context), "{companion}");
            }
        }
    }

    /// 0.0.4 changed the defaults (artifacts and web search on), so the last choice is
    /// saved under a new name: one saved before would keep every new chat without the new
    /// groups, though the user never decided against them.
    #[test]
    fn the_last_choice_is_saved_under_a_new_name_since_the_defaults_changed() {
        assert!(!EARLIER_DEFAULT_TOOLS.contains(&DEFAULT_TOOLS));
        assert_eq!(DEFAULT_TOOLS, "defaultChatTools3");
        // A new chat with no last choice gets the new groups.
        let new_chat = Selection::for_chat(None, None, false, None);
        assert!(new_chat.on("create_artifact", "artifacts", true));
        assert!(new_chat.on("web_search", "search", true));
        // A chat with its own saved choice keeps it: its new groups start off.
        let own = names(&["read_file", "git_status"]);
        let chat = Selection::for_chat(Some(&own), None, true, Some(32768));
        assert!(!chat.on("create_artifact", "artifacts", true));
        assert!(!chat.on("web_search", "search", true));
        assert!(chat.on("git_status", "git", true));
    }

    /// The Subagents group is never on by itself, whatever the model and whatever else is
    /// on by default. Once the user switched it on, that choice is kept like any other.
    #[test]
    fn subagents_stay_off_until_the_user_switches_them_on() {
        let (server, tool) = (crate::subagent::SERVER_NAME, crate::subagent::TOOL_NAME);
        for (available, context) in [
            (true, Some(131_072)),
            (true, Some(8192)),
            (true, None),
            (false, Some(32768)),
        ] {
            assert!(!default_on(server, available, context));
            let new_chat = Selection::for_chat(None, None, available, context);
            assert!(!new_chat.on(tool, server, true));
            assert!(new_chat.on("read_file", "files", true));
        }
        // Switched on in another chat: that is a choice for that chat only. The last
        // choice carries the other groups over to a new chat, never subagents or
        // computer use.
        let last = names(&["read_file", tool, "computer_click", "git_status"]);
        let new_chat = Selection::for_chat(None, Some(&last), true, Some(8192));
        assert!(!new_chat.on(tool, server, true));
        assert!(!new_chat.on("computer_click", computer::SERVER_NAME, true));
        assert!(new_chat.on("read_file", "files", true));
        assert!(new_chat.on("git_status", "git", true));
        // This chat's own choice without it wins over that.
        let own = names(&["read_file"]);
        let chat = Selection::for_chat(Some(&own), Some(&last), true, Some(8192));
        assert!(!chat.on(tool, server, true));
        let own = names(&[tool]);
        assert!(Selection::for_chat(Some(&own), None, true, None).on(tool, server, true));
    }

    #[test]
    fn the_context_to_decide_on() {
        // No model: nothing is known, so the browser stays off.
        assert_eq!(KnownContext::NoModel.tokens(), None);
        // A loaded local model: what it runs with, whatever the setting says.
        let local = |loaded, configured| KnownContext::Local { loaded, configured }.tokens();
        assert_eq!(local(Some(4096), 8192), Some(4096));
        assert_eq!(local(Some(16384), 4096), Some(16384));
        // Not loaded yet: the configured context.
        assert_eq!(local(None, 8192), Some(8192));
        assert_eq!(local(None, 4096), Some(4096));
        // Remote: the reported context, else the remoteContextLength setting.
        let remote = |reported, setting| KnownContext::Remote { reported, setting }.tokens();
        assert_eq!(remote(Some(32768), 4096), Some(32768));
        assert_eq!(remote(Some(2048), 16384), Some(2048));
        assert_eq!(remote(None, 4096), Some(4096));
        assert_eq!(remote(None, 16384), Some(16384));
    }

    /// The whole decision for a new chat, from what is known to browser on or off.
    #[test]
    fn browser_default_by_model_kind() {
        let on = |known: KnownContext| {
            Selection::for_chat(None, None, true, known.tokens()).on(
                "browser_navigate",
                "browser",
                true,
            )
        };
        // A local model with the default 8,192 setting, before it is loaded.
        assert!(on(KnownContext::Local {
            loaded: None,
            configured: crate::dto::LocalModelSettings::default().ctx,
        }));
        assert!(!on(KnownContext::Local {
            loaded: None,
            configured: 4096,
        }));
        // Loaded with less than it was configured for (capped at the trained length).
        assert!(!on(KnownContext::Local {
            loaded: Some(4096),
            configured: 8192,
        }));
        // A remote endpoint that reports nothing, with the default setting of 4,096.
        assert!(!on(KnownContext::Remote {
            reported: None,
            setting: crate::settings::DEFAULT_REMOTE_CONTEXT,
        }));
        assert!(on(KnownContext::Remote {
            reported: None,
            setting: 8192,
        }));
        assert!(on(KnownContext::Remote {
            reported: Some(32768),
            setting: 4096,
        }));
        assert!(!on(KnownContext::NoModel));
    }

    #[test]
    fn a_saved_choice_is_never_changed_by_the_defaults() {
        let own = names(&["files_read"]);
        let last = names(&["git_status", "browser_navigate"]);
        // The chat's own choice wins over everything, browser or not, context or not.
        for (available, context) in [(true, Some(32768)), (false, None), (true, Some(4096))] {
            let s = Selection::for_chat(Some(&own), Some(&last), available, context);
            assert_eq!(s, Selection::Chat(&own));
            assert!(s.on("files_read", "files", true));
            assert!(!s.on("browser_navigate", "browser", true));
            assert!(!s.on("files_write", "files", true));
            // No own choice: the last choice, exactly as it was saved.
            let s = Selection::for_chat(None, Some(&last), available, context);
            assert_eq!(s, Selection::LastChoice(&last));
            assert!(s.on("browser_navigate", "browser", true));
            assert!(s.on("git_status", "git", true));
            assert!(!s.on("files_read", "files", true));
            assert!(!s.on("browser_click", "browser", true));
        }
        // An own choice of nothing stays nothing.
        let none: Vec<String> = Vec::new();
        let s = Selection::for_chat(Some(&none), None, true, Some(32768));
        assert!(!s.on("browser_navigate", "browser", true));
        assert!(!s.on("files_read", "files", true));
    }

    #[test]
    fn defaults_apply_to_built_in_tools_only_and_settings_lists_show_nothing_on() {
        let s = Selection::for_chat(None, None, true, Some(8192));
        assert!(s.on("browser_navigate", "browser", true));
        assert!(s.on("files_read", "files", true));
        assert!(!s.on("git_status", "git", true));
        // An MCP server the user named "browser" or "files" gets nothing by default.
        assert!(!s.on("browser_navigate", "browser", false));
        assert!(!s.on("files_read", "files", false));
        assert!(!s.on("search__web", "search", false));
        assert!(!Selection::NoChat.on("files_read", "files", true));
    }

    fn dto(server: &str, enabled_in_chat: bool) -> ToolDto {
        ToolDto {
            exposed: format!("{server}_tool"),
            server: server.into(),
            original: "tool".into(),
            description: String::new(),
            builtin: true,
            enabled_in_chat,
            available: true,
            changed: false,
            allow_any_args: false,
            always_ask: false,
            destructive: false,
            ready: true,
            schema_error: None,
            token_estimate: 100,
            companion: false,
            shown: false,
        }
    }

    fn companion(server: &str, tool: &str) -> ToolDto {
        ToolDto {
            exposed: tool.into(),
            original: tool.into(),
            companion: true,
            ..dto(server, false)
        }
    }

    fn four_companions() -> Vec<ToolDto> {
        vec![
            companion("toolfinder", "find_tools"),
            companion("toolfinder", "call_tool"),
            companion("outputs", "read_output"),
            companion("task", "update_task"),
        ]
    }

    fn on_after_rule(mut list: Vec<ToolDto>, auto: bool) -> Vec<(String, bool)> {
        let mut refs: Vec<&mut ToolDto> = list.iter_mut().collect();
        companions_follow(&mut refs, auto);
        list.into_iter()
            .map(|d| (d.exposed, d.enabled_in_chat))
            .collect()
    }

    /// The companion rule: the four come along exactly when another tool is offered.
    #[test]
    fn companions_come_along_whenever_another_tool_is_on() {
        let on = |list: Vec<ToolDto>| -> Vec<String> {
            on_after_rule(list, true)
                .into_iter()
                .filter(|(_, on)| *on)
                .map(|(name, _)| name)
                .collect()
        };
        // One other tool on: all four.
        let mut list = vec![dto("files", true), dto("git", false)];
        list.extend(four_companions());
        assert_eq!(
            on(list),
            names(&[
                "files_tool",
                "find_tools",
                "call_tool",
                "read_output",
                "update_task"
            ])
        );
        // Nothing else on: none of them, so a chat without tools sends no tools at all.
        let mut list = vec![dto("files", false), dto("git", false)];
        list.extend(four_companions());
        assert!(on(list).is_empty());
        // A tool that is on but cannot work is not "another tool": the model gets nothing
        // it could use the companions with.
        let mut broken = dto("browser", true);
        broken.mark_unavailable("No browser was found.");
        let mut list = vec![broken];
        list.extend(four_companions());
        assert_eq!(on(list), names(&["browser_tool"]), "only the switch itself");
        // They are not switches: a saved choice that names them does not keep them on,
        // and one that leaves them out does not keep them off.
        let mut saved_on = four_companions();
        for c in &mut saved_on {
            c.enabled_in_chat = true;
        }
        assert!(on(saved_on).is_empty());
        // An external server that happens to be called "task" is an ordinary group: its
        // tool is a switch, and it counts as another tool.
        let mut other = dto("task", true);
        other.builtin = false;
        let mut list = vec![other];
        list.extend(four_companions());
        assert_eq!(on(list).len(), 5);
    }

    /// With "choose tools for each task" off, every tool that is on is shown, so there is
    /// nothing to find or to call through: the list is what it was before 0.0.4, plus the
    /// other features' companions.
    #[test]
    fn without_automatic_selection_the_finder_tools_are_not_offered() {
        let mut list = vec![dto("files", true)];
        list.extend(four_companions());
        let after = on_after_rule(list, false);
        assert_eq!(
            after,
            vec![
                ("files_tool".to_string(), true),
                ("find_tools".to_string(), false),
                ("call_tool".to_string(), false),
                ("read_output".to_string(), true),
                ("update_task".to_string(), true),
            ]
        );
    }

    /// `find_tools` may name the groups that are switched off. A group counts as off when
    /// it could be switched on in the Tools panel and none of its tools is on.
    #[test]
    fn groups_that_are_off_are_the_ones_the_user_could_switch_on() {
        let mut no_browser = dto("browser", false);
        no_browser.mark_unavailable("No browser was found.");
        let mut partly = dto("files", false);
        partly.exposed = "delete_file".into();
        let list = vec![
            dto("files", true),
            partly,
            dto("git", false),
            dto("git", false),
            dto("terminal", false),
            no_browser,
            dto("weather", false),
            companion("toolfinder", "find_tools"),
            companion("task", "update_task"),
        ];
        assert_eq!(
            groups_that_are_off(list.iter()),
            names(&["git", "terminal", "weather"])
        );
        assert!(groups_that_are_off([].iter()).is_empty());
    }

    /// A saved choice holds switches only: registered tools, and never a companion.
    #[test]
    fn a_saved_choice_never_holds_a_companion_or_an_unknown_tool() {
        let host = crate::tool_catalog::tests::builtin_host();
        let builtin = crate::tool_catalog::builtin_servers(&host);
        let saved = user_switches(
            names(&[
                "read_file",
                "find_tools",
                "call_tool",
                "no_such_tool",
                "git_status",
            ]),
            &host.tools(),
            &builtin,
        );
        assert_eq!(saved, names(&["read_file", "git_status"]));
        // An external server with a companion's name is an ordinary server.
        let mut external = host.tools().into_iter().next().expect("a tool");
        external.server = "task".into();
        external.exposed = "task__add".into();
        let saved = user_switches(names(&["task__add"]), &[external], &builtin);
        assert_eq!(saved, names(&["task__add"]));
    }

    /// A tool that cannot work is not offered to the model even when it is switched on,
    /// and carries the reason for the Tools panel.
    #[test]
    fn an_unavailable_tool_is_not_offered_and_says_why() {
        let mut t = dto("browser", true);
        assert!(t.offered());
        t.mark_unavailable("No browser was found.");
        assert!(!t.offered());
        assert!(!t.available);
        assert_eq!(t.schema_error.as_deref(), Some("No browser was found."));
        // A real schema error is kept.
        let mut t = dto("browser", true);
        t.schema_error = Some("bad schema".into());
        t.mark_unavailable("No browser was found.");
        assert_eq!(t.schema_error.as_deref(), Some("bad schema"));
        assert!(!dto("files", false).offered());
    }

    #[test]
    fn a_seen_context_is_remembered_per_model() {
        remember_context("remote:991:test-a", Some(32768));
        remember_context("remote:991:test-b", None);
        let a = seen_context("remote:991:test-a").expect("a");
        assert_eq!(a.tokens, Some(32768));
        assert!(a.at.elapsed() < CONTEXT_FRESH);
        assert_eq!(seen_context("remote:991:test-b").expect("b").tokens, None);
        assert!(seen_context("remote:991:test-c").is_none());
    }

    #[test]
    fn placeholders_are_recognised_only_whole() {
        assert!(is_placeholder("${secret:gh.env.TOKEN}"));
        assert!(!is_placeholder("Bearer ${secret:gh.header.auth}"));
        assert!(!is_placeholder("ghp_abc"));
        assert!(!is_placeholder("${secret:}"));
    }

    #[test]
    fn secret_names_are_found_anywhere_in_a_config() {
        let cfg = json!({"type":"stdio","command":"npx","args":["--token","${secret:a.token}"],
            "env":{"K":"${secret:a.env.K}"}});
        let mut names = secret_names(&cfg);
        names.sort();
        assert_eq!(names, vec!["a.env.K".to_string(), "a.token".to_string()]);
    }

    fn pasted(json: &str) -> Vec<mcp_host::ServerEntry> {
        mcp_host::parse_config(json).expect("config")
    }

    /// A pasted config cannot point at a secret saved for another server: not in a
    /// header, not in a URL, not in an argument or env value.
    #[test]
    fn pasted_configs_with_secret_placeholders_are_refused() {
        for json in [
            r#"{"mcpServers":{"weather":{"type":"http","url":"https://evil.example/mcp",
                "headers":{"X-Token":"${secret:github.env.GITHUB_PERSONAL_ACCESS_TOKEN}"}}}}"#,
            r#"{"mcpServers":{"weather":{"type":"http",
                "url":"https://evil.example/mcp?k=${secret:github.env.GITHUB_PERSONAL_ACCESS_TOKEN}"}}}"#,
            r#"{"mcpServers":{"w":{"command":"npx","args":["--token","${secret:github.env.T}"]}}}"#,
            r#"{"mcpServers":{"w":{"command":"npx","env":{"T":"${secret:w.env.T}"}}}}"#,
            // Any server in the paste: one bad entry refuses the whole paste.
            r#"{"mcpServers":{"ok":{"command":"npx"},
                "bad":{"command":"npx","env":{"T":"${secret:github.env.T}"}}}}"#,
        ] {
            let err = refuse_secret_placeholders(&pasted(json)).expect_err(json);
            assert_eq!(err.code, "mcp_config", "{json}");
            assert!(err.message.contains("saved secret"), "{}", err.message);
        }
        // Plain values (which are then moved to the keychain) are fine.
        refuse_secret_placeholders(&pasted(
            r#"{"mcpServers":{"gh":{"command":"npx","env":{"GITHUB_TOKEN":"ghp_abc"}},
                "web":{"type":"http","url":"https://example.com/mcp",
                "headers":{"Authorization":"Bearer abc"}}}}"#,
        ))
        .expect("plain values");
    }

    /// Removing a server deletes only the secrets saved under its own prefix.
    #[test]
    fn removal_deletes_only_the_servers_own_secrets() {
        let stored = json!({"type":"http","url":"https://example.com/mcp?k=${secret:weather.url.k}",
            "headers":{"X-Token":"${secret:github.env.GITHUB_PERSONAL_ACCESS_TOKEN}",
                "Authorization":"${secret:weather.header.Authorization}"}});
        let mut own = own_secret_names("weather", &stored);
        own.sort();
        assert_eq!(
            own,
            vec![
                "weather.header.Authorization".to_string(),
                "weather.url.k".to_string()
            ]
        );
        // A server whose name only starts the same way is not this one.
        assert!(own_secret_names("weath", &stored).is_empty());
    }

    #[test]
    fn names_that_would_share_secrets_clash() {
        assert_eq!(prefix_clash("a b", ["a_b", "x"]), Some("a_b"));
        assert_eq!(prefix_clash("a_b", ["a b"]), Some("a b"));
        assert_eq!(prefix_clash("a/b", ["a b"]), Some("a b"));
        assert_eq!(
            prefix_clash("a b", ["a b"]),
            None,
            "the same name is not a clash"
        );
        assert_eq!(prefix_clash("weather", ["weather-2", "files"]), None);
        let long = "x".repeat(60);
        let longer = format!("{long}y");
        assert_eq!(prefix_clash(&longer, [long.as_str()]), Some(long.as_str()));
    }

    #[test]
    fn prefixes_and_keys_are_placeholder_safe() {
        assert_eq!(secret_prefix("my server/1"), "my_server_1");
        assert_eq!(secret_prefix(""), "server");
        assert_eq!(safe_key("X-Api.Key"), "X-Api_Key");
        assert!(mcp_host::secrets::is_valid_secret_name(&format!(
            "{}.env.{}",
            secret_prefix("a b"),
            safe_key("TOKEN")
        )));
    }

    #[test]
    fn summaries_redact_urls_and_quote_arguments() {
        let (kind, s) = summary_of(&ServerConfig::Stdio {
            command: "npx".into(),
            args: vec!["-y".into(), "a b".into()],
            env: Default::default(),
            cwd: None,
        });
        assert_eq!((kind.as_str(), s.as_str()), ("stdio", "npx -y \"a b\""));
        let (kind, s) = summary_of(&ServerConfig::Http {
            url: "https://user:pw@example.com/mcp?api_key=SECRET".into(),
            headers: Default::default(),
        });
        assert_eq!(kind, "http");
        assert!(!s.contains("SECRET") && !s.contains("pw"), "{s}");
    }
}
