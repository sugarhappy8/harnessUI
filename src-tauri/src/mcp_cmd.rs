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
/// on in a new chat. They let the model act widely (the whole screen; helpers that work on
/// their own; coding apps that work online, 0.0.6), so each chat gets its own decision.
const PER_CHAT_ONLY: &[&str] = &[
    computer::SERVER_NAME,
    crate::subagent::SERVER_NAME,
    crate::outside_agent::SERVER_NAME,
];

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

/// What the Tools panel says about a folder tool in a chat that is not in Full access.
const ONLY_IN_FULL_ACCESS: &str = "Works only when Full access is on in this chat.";

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

/// [0.0.6] Setting: the owner's saved servers that were renamed at start because a new
/// built-in server has their name. Settings → MCP says so once, until the owner closes
/// the note (which clears it).
pub const RENAMED_SERVERS: &str = "mcpRenamedServers";

/// One saved server renamed at start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct RenamedServer {
    /// The name it had.
    pub from: String,
    /// The name it has now.
    pub to: String,
}

/// A free name for the owner's server whose name a built-in server now has:
/// "<name> (yours)", then "<name> (yours 2)" and so on; never a taken name, and never one
/// that would share secrets with another server ([`prefix_clash`]).
fn free_name(name: &str, taken: &BTreeSet<String>) -> String {
    (1u32..)
        .map(|n| match n {
            1 => format!("{name} (yours)"),
            n => format!("{name} (yours {n})"),
        })
        .find(|c| !taken.contains(c) && prefix_clash(c, taken.iter().map(String::as_str)).is_none())
        .unwrap_or_else(|| format!("{name} (yours)"))
}

/// The config with the server's own secrets (the ones saved under `from`'s prefix) named
/// under `to`'s prefix instead, and each (old, new) secret name.
fn moved_secrets(from: &str, to: &str, config: &Value) -> (Value, Vec<(String, String)>) {
    let old_prefix = format!("{}.", secret_prefix(from));
    let new_prefix = format!("{}.", secret_prefix(to));
    let pairs: Vec<(String, String)> = own_secret_names(from, config)
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|old| {
            let new = format!("{new_prefix}{}", &old[old_prefix.len()..]);
            (old, new)
        })
        .collect();
    fn walk(v: &mut Value, pairs: &[(String, String)]) {
        match v {
            Value::String(s) => {
                for (old, new) in pairs {
                    let (old, new) = (
                        mcp_host::secrets::placeholder(old),
                        mcp_host::secrets::placeholder(new),
                    );
                    if s.contains(&old) {
                        *s = s.replace(&old, &new);
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(|x| walk(x, pairs)),
            Value::Object(o) => o.values_mut().for_each(|x| walk(x, pairs)),
            _ => {}
        }
    }
    let mut out = config.clone();
    walk(&mut out, &pairs);
    (out, pairs)
}

/// The name a tool of the saved server `from` is offered under once the server is called
/// `to`, for a name kept in a chat's choice of tools or in the last choice; `None` when
/// `name` is not one of `from`'s tools. `from` is a built-in server's name, too short to
/// be cut in a tool's name. `owners` are the saved servers' names now: a name that also
/// fits one of them, with more of its name (as "reverse__yours___x" fits "reverse
/// (yours)"), is that server's tool and stays as it is.
fn renamed_tool(name: &str, from: &str, to: &str, owners: &[String]) -> Option<String> {
    use mcp_host::names::{SEPARATOR, exposed_name, sanitize_part};
    let old = sanitize_part(from);
    let tool = name.strip_prefix(old.as_str())?.strip_prefix(SEPARATOR)?;
    if old.is_empty() || tool.is_empty() {
        return None;
    }
    // In a long name a server's part may be cut down to 16 characters (names.rs).
    let theirs = owners.iter().map(|o| sanitize_part(o)).any(|o| {
        (o.len().min(16)..=o.len())
            .filter(|&n| n >= old.len())
            .any(|n| {
                name.strip_prefix(&o[..n])
                    .is_some_and(|rest| rest.starts_with(SEPARATOR))
            })
    });
    if theirs {
        return None;
    }
    Some(exposed_name(to, tool, &|_| false))
}

/// `name` under its server's new name, when it is a tool of a renamed server.
fn renamed_name(name: &str, renamed: &[RenamedServer], owners: &[String]) -> Option<String> {
    renamed
        .iter()
        .find_map(|r| renamed_tool(name, &r.from, &r.to, owners))
}

/// A chat's stored tool choices ([`crate::turns::ChatTools`]) with the renamed servers'
/// tools under their new names: the tools that are on, and those the chat remembers as
/// shown and found. Everything else stays as stored. True when anything changed.
fn rename_tools_in_chat(prefs: &mut Value, renamed: &[RenamedServer], owners: &[String]) -> bool {
    let mut changed = false;
    for key in ["enabled", "shown", "found"] {
        let Some(Value::Array(items)) = prefs.get_mut(key) else {
            continue;
        };
        for item in items {
            if let Value::String(name) = item
                && let Some(new) = renamed_name(name, renamed, owners)
            {
                *name = new;
                changed = true;
            }
        }
    }
    changed
}

/// [0.0.6] Keeps the renamed servers' tools on where they were on. A chat's choice and the
/// last choice that new chats start with ([`DEFAULT_TOOLS`]) list tools by the name they
/// are offered under, which starts with the server's name. "Allow for this chat" answers
/// need nothing: they are kept in memory only and end when the app quits (D12), so none is
/// left from before this start, and the owner's server has not run yet.
async fn carry_tool_choices(store: &store::Store, renamed: &[RenamedServer], owners: &[String]) {
    let chats = store
        .list_chats(store::ListChats {
            limit: 0,
            include_archived: true,
            ..Default::default()
        })
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("could not read chats to keep a renamed server's tools on: {e}");
            Vec::new()
        });
    for summary in chats {
        // One change at a time to a chat's tool choices, as everywhere else.
        let _one_at_a_time = PREFS_WRITES.lock().await;
        let Ok(Some(chat)) = store.get_chat(summary.id).await else {
            continue;
        };
        let mut prefs = chat.tool_prefs;
        if rename_tools_in_chat(&mut prefs, renamed, owners)
            && let Err(e) = store
                .update_chat(
                    summary.id,
                    store::ChatPatch {
                        tool_prefs: Some(prefs),
                        ..Default::default()
                    },
                )
                .await
        {
            tracing::warn!(chat = summary.id.0, "could not keep a renamed server's tools on: {e}");
        }
    }
    if let Ok(Some(mut last)) = store.get_setting::<Vec<String>>(DEFAULT_TOOLS).await {
        let mut changed = false;
        for name in &mut last {
            if let Some(new) = renamed_name(name, renamed, owners) {
                *name = new;
                changed = true;
            }
        }
        if changed && let Err(e) = store.set_setting(DEFAULT_TOOLS, &last).await {
            tracing::warn!("could not keep a renamed server's tools in the last choice: {e}");
        }
    }
}

/// [0.0.6] Renames the owner's saved servers whose name a built-in server now has
/// ("reverse" and "coding_apps" were free before 0.0.6), before the saved servers start.
/// The row keeps its id, so its tool switches and its on/off switch stay; its secrets move
/// to the new name (when the keychain refuses, it keeps them under the old names, which
/// still work, and removing the server deletes them all the same:
/// [`removable_secret_names`]); its tools stay on in the chats and in the last choice
/// ([`carry_tool_choices`]). What was renamed is added to [`RENAMED_SERVERS`] for
/// Settings → MCP.
pub(crate) async fn rename_clashing_servers(
    store: &store::Store,
    secrets: &dyn store::secrets::SecretStore,
    builtin: &BTreeSet<String>,
) -> Vec<RenamedServer> {
    let rows = match store.list_mcp_servers().await {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!("could not read MCP servers to check their names: {e}");
            return Vec::new();
        }
    };
    let mut taken: BTreeSet<String> = rows
        .iter()
        .map(|r| r.name.clone())
        .chain(builtin.iter().cloned())
        .collect();
    let mut renamed = Vec::new();
    for row in rows.into_iter().filter(|r| builtin.contains(&r.name)) {
        let to = free_name(&row.name, &taken);
        let (moved, pairs) = moved_secrets(&row.name, &to, &row.config);
        // Each secret is copied before the row changes; if one cannot be, the config keeps
        // its old secret names (they still work) and nothing is deleted.
        let mut copied: Vec<String> = Vec::new();
        let mut all_copied = true;
        for (old, new) in &pairs {
            match secrets.get(&mcp_secret_key(old)) {
                Ok(Some(value)) => {
                    if secrets.set(&mcp_secret_key(new), &value).is_err() {
                        all_copied = false;
                        break;
                    }
                    copied.push(new.clone());
                }
                Ok(None) => {}
                Err(_) => {
                    all_copied = false;
                    break;
                }
            }
        }
        let undo = |copied: &[String]| {
            for new in copied {
                let _ = secrets.delete(&mcp_secret_key(new));
            }
        };
        if !all_copied {
            undo(&copied);
        }
        let config = if all_copied {
            moved
        } else {
            row.config.clone()
        };
        match store.rename_mcp_server(row.id, to.clone(), config).await {
            Ok(()) => {
                if all_copied {
                    for (old, _) in &pairs {
                        let _ = secrets.delete(&mcp_secret_key(old));
                    }
                }
                tracing::info!(from = %row.name, to = %to, "renamed a saved MCP server: a built-in server has its name");
                taken.insert(to.clone());
                renamed.push(RenamedServer { from: row.name, to });
            }
            Err(e) => {
                tracing::warn!(server = %row.name, "could not rename a saved MCP server: {e}");
                if all_copied {
                    undo(&copied);
                }
            }
        }
    }
    if !renamed.is_empty() {
        // The saved servers' names now (a built-in name left is a row not renamed).
        let owners: Vec<String> = taken.difference(builtin).cloned().collect();
        carry_tool_choices(store, &renamed, &owners).await;
        let mut note: Vec<RenamedServer> = store
            .get_setting(RENAMED_SERVERS)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        note.extend(renamed.iter().cloned());
        if let Err(e) = store.set_setting(RENAMED_SERVERS, &note).await {
            tracing::warn!("could not save the note about renamed MCP servers: {e}");
        }
    }
    renamed
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
    let row = state
        .store
        .list_mcp_servers()
        .await?
        .into_iter()
        .find(|r| r.name == name);
    let builtin = state.host.server_kind(&name) == Some(ServerKind::Builtin);
    if builtin && row.is_none() {
        return Err(AppError::bad_request(
            "Built-in tools cannot be removed; switch them off per chat instead.",
        ));
    }
    // Only an outside server is stopped; a saved row that has a built-in server's name
    // (from before that server existed) is removed from the list alone.
    if !builtin {
        let _ = state.host.remove(&name).await;
    }
    if row.is_some() {
        let builtin_names = crate::tool_catalog::builtin_servers(&state.host);
        delete_saved_server(&state.store, state.secrets.as_ref(), &builtin_names, &name).await?;
    }
    Ok(())
}

/// The secrets removing a saved server deletes: the ones it owns ([`own_secret_names`]),
/// and the ones under a built-in server's secret prefix that it refers to. Only a server
/// renamed at start can have those ([`rename_clashing_servers`] leaves them under the old
/// name when the keychain refuses): no other saved server can have a built-in server's
/// name or its secret prefix, because [`add_mcp_servers`] refuses both. A secret another
/// saved server refers to (`others`, their stored configs) is never deleted.
fn removable_secret_names(
    server: &str,
    config: &Value,
    builtin: &BTreeSet<String>,
    others: &[&Value],
) -> Vec<String> {
    let prefixes: Vec<String> = std::iter::once(server)
        .chain(builtin.iter().map(String::as_str))
        .map(|name| format!("{}.", secret_prefix(name)))
        .collect();
    let theirs: BTreeSet<String> = others.iter().flat_map(|c| secret_names(c)).collect();
    secret_names(config)
        .into_iter()
        .filter(|s| prefixes.iter().any(|p| s.starts_with(p)) && !theirs.contains(s))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Deletes a saved server's row and its secrets ([`removable_secret_names`]). False when
/// no saved server has `name`.
async fn delete_saved_server(
    store: &store::Store,
    secrets: &dyn store::secrets::SecretStore,
    builtin: &BTreeSet<String>,
    name: &str,
) -> AppResult<bool> {
    let rows = store.list_mcp_servers().await?;
    let Some(row) = rows.iter().find(|r| r.name == name) else {
        return Ok(false);
    };
    let others: Vec<&Value> = rows
        .iter()
        .filter(|r| r.name != name)
        .map(|r| &r.config)
        .collect();
    for secret in removable_secret_names(name, &row.config, builtin, &others) {
        let _ = secrets.delete(&mcp_secret_key(&secret));
    }
    store.delete_mcp_server(row.id).await?;
    Ok(true)
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
    // Only an outside server is started or stopped: a saved row with a built-in server's
    // name never touches the built-in one.
    let builtin = state.host.server_kind(&name) == Some(ServerKind::Builtin);
    if builtin {
        return Ok(());
    }
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
    // [0.0.5 full access] The tools that create, move and delete folders work only while the
    // chat is in Full access; otherwise the Tools panel says so and no turn offers them.
    let full_access = crate::approvals::modes().full_access(chat.id.0);
    for (_, dto) in &mut list {
        let why_not = match dto.server.as_str() {
            computer::SERVER_NAME => no_computer.as_deref(),
            BROWSER_SERVER => no_browser.as_deref(),
            "files" if !full_access && tools::files::organize::is_tool(&dto.original) => {
                Some(ONLY_IN_FULL_ACCESS)
            }
            _ => None,
        };
        if let Some(reason) = why_not {
            dto.mark_unavailable(reason);
        }
    }
    // [0.0.6 item 5, WP9h] Reverse engineering: its tools are always listed (R19); the ones
    // that cannot run here yet say why (not set up, Apple's tools, an update, a decompiler).
    mark_reverse(
        list.iter_mut().map(|(_, dto)| dto),
        reverse::handle().as_deref(),
    );
    // The companions follow the other tools, once it is known which of those can work.
    let setting = crate::tool_select::settings(state).await;
    let mut dtos: Vec<&mut ToolDto> = list.iter_mut().map(|(_, dto)| dto).collect();
    companions_follow(&mut dtos, setting.auto);
    // [0.0.5 full access] The folder tools come with `write_file`, and only in Full access.
    // They follow it instead of being switches of their own, so a chat whose tool list was
    // saved before they existed has them as soon as Full access is on.
    let write_on = list
        .iter()
        .any(|(_, d)| d.server == "files" && d.original == "write_file" && d.enabled_in_chat);
    for (_, dto) in &mut list {
        if dto.server == "files" && tools::files::organize::is_tool(&dto.original) {
            dto.companion = true;
            dto.enabled_in_chat = write_on && full_access;
        }
    }
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
        let configured = crate::load_settings::configured_ctx(state, path).await;
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

/// [0.0.6 item 5, WP9h] What the Tools panel says about a reverse tool when the app has no
/// `reverse` server to ask (it is then not registered either; kept so a missing server
/// can never leave its tools offered). The same words as a server with nothing installed.
const REVERSE_NOT_SET_UP: &str = "Not set up yet. Set it up in Settings → Reverse engineering.";

/// [0.0.6 item 5, WP9h] Marks the built-in `reverse` server's tools that cannot run here yet
/// with the reason the Tools panel shows (`ReverseServer::unavailable_reason`: not set up;
/// Apple's command line tools; an update; a decompiler; a tool that changed in the installed
/// rea). A marked tool is not offered to the model, but stays switched on by name, so a
/// group the owner switched on also covers tools that become usable later (R19).
///
/// Whether the group is on in a new chat is not decided here: `reverse` is not in
/// [`DEFAULT_ON_BUILTIN`] (the owner's answer Q4 = B), so it starts off until the owner
/// turns it on in a chat's Tools panel, and then new chats follow the last choice like any
/// other group (it is not [`PER_CHAT_ONLY`]: its tools only read files).
fn mark_reverse<'a>(
    tools: impl IntoIterator<Item = &'a mut ToolDto>,
    server: Option<&reverse::ReverseServer>,
) {
    for dto in tools {
        if !dto.builtin || dto.server != reverse::server::SERVER_NAME {
            continue;
        }
        let reason = match server {
            Some(s) => s.unavailable_reason(&dto.original),
            None => Some(REVERSE_NOT_SET_UP.to_string()),
        };
        if let Some(reason) = reason {
            dto.mark_unavailable(&reason);
        }
    }
}

/// [0.0.6 item 5, WP9h] Whether the Reverse engineering group is switched on in this chat:
/// any tool of the built-in `reverse` server, by the chat's own choice or the last choice.
/// Never by default (the owner's answer Q4 = B), so no context is looked up. While it is
/// off, a program dropped on the message box comes with a note saying to turn it on
/// (`reverse_drop.rs`).
pub(crate) async fn reverse_switched_on(state: &AppState, chat: &store::Chat) -> bool {
    let prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    let last_choice: Option<Vec<String>> =
        state.store.get_setting(DEFAULT_TOOLS).await.ok().flatten();
    let selection = Selection::for_chat(
        prefs.enabled.as_deref(),
        last_choice.as_deref(),
        false,
        None,
    );
    reverse_group_on(&selection, &tools_with_kind(&state.host))
}

/// Each registered tool as (exposed name, server, built in). "Built in" is the server's
/// slot in the host, never whether a saved server has that name.
fn tools_with_kind(host: &mcp_host::Host) -> Vec<(String, String, bool)> {
    let builtin = crate::tool_catalog::builtin_servers(host);
    host.tools()
        .into_iter()
        .map(|t| {
            let is_builtin = builtin.contains(&t.server);
            (t.exposed, t.server, is_builtin)
        })
        .collect()
}

/// Whether one of the built-in `reverse` server's tools (exposed name, server, built in) is
/// on in `selection`.
fn reverse_group_on(selection: &Selection<'_>, tools: &[(String, String, bool)]) -> bool {
    tools.iter().any(|(exposed, server, builtin)| {
        *builtin
            && server == reverse::server::SERVER_NAME
            && selection.on(exposed, server, *builtin)
    })
}

async fn tool_list(
    state: &AppState,
    selection: &Selection<'_>,
) -> Vec<(mcp_host::RegisteredTool, ToolDto)> {
    // Built in by the server's slot in the host (a saved row may have a built-in name).
    let builtin_servers = crate::tool_catalog::builtin_servers(&state.host);
    let mut out = Vec::new();
    for t in state.host.tools() {
        let builtin = builtin_servers.contains(&t.server);
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
    change_chat_prefs(&state, chat_id, |prefs| {
        prefs.thinking_off = !on;
        prefs.thinking_chosen = true; // [0.0.4 profile] the user's choice beats a model profile
    })
    .await
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

    /// The Coding apps group (0.0.6) is off in a new chat, and switching it on in one chat
    /// never carries over to the next through the last choice.
    #[test]
    fn coding_apps_stay_off_until_the_user_switches_them_on() {
        let server = crate::outside_agent::SERVER_NAME;
        for tool in [
            crate::outside_agent::CLAUDE_TOOL,
            crate::outside_agent::CODEX_TOOL,
        ] {
            for (available, context) in [(true, Some(131_072)), (false, None)] {
                assert!(!default_on(server, available, context));
                assert!(
                    !Selection::for_chat(None, None, available, context).on(tool, server, true)
                );
            }
            let last = names(&["read_file", tool]);
            let new_chat = Selection::for_chat(None, Some(&last), true, Some(8192));
            assert!(!new_chat.on(tool, server, true));
            assert!(new_chat.on("read_file", "files", true));
            let own = names(&[tool]);
            assert!(Selection::for_chat(Some(&own), None, true, None).on(tool, server, true));
        }
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
        // A local model before it is loaded: the context its next load asks for (0.0.5:
        // its own setting, else the defaults, else what Automatic picks now). A fixed
        // 8,192, or Automatic at its cap, turns the browser on; Automatic at its floor on
        // a computer short of memory does not.
        assert!(on(KnownContext::Local {
            loaded: None,
            configured: 8192,
        }));
        assert!(on(KnownContext::Local {
            loaded: None,
            configured: models::fit::AUTO_CAP_CTX as u32,
        }));
        assert!(!on(KnownContext::Local {
            loaded: None,
            configured: models::fit::AUTO_FLOOR_CTX as u32,
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

    // ---- [0.0.6 item 5, WP9h] The Reverse engineering group ------------------------------

    /// The reasons the Tools panel shows for reverse tools (`ReverseServer::
    /// unavailable_reason`). `ToolsPanel.tsx` recognises the first two by their opening
    /// words, so they are pinned here as well.
    const NOT_SET_UP: &str = "Not set up yet. Set it up in Settings → Reverse engineering.";
    const NEEDS_DECOMPILER: &str =
        "Needs a decompiler. Install one in Settings → Reverse engineering.";
    const NEEDS_UPDATE: &str = "Needs an update in Settings → Reverse engineering.";
    const NEEDS_APPLE_TOOLS: &str =
        "Needs Apple's command line tools. Install them in Settings → Reverse engineering.";

    struct Scratch(std::path::PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(name: &str) -> Scratch {
        let dir = std::env::temp_dir().join(format!("mcp-cmd-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// A part on disk the way the installer leaves it: its folder with `.complete` (no
    /// programs recorded, so nothing else is checked).
    fn put_part(root: &std::path::Path, component: &str, part: &str, version: &str) {
        let dir = root.join(format!("{part}-{version}"));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = json!({
            "component": component,
            "part": part,
            "version": version,
            "archiveSha256": [],
            "programs": [],
            "diskBytes": 1_000_000,
        });
        std::fs::write(dir.join(".complete"), marker.to_string()).unwrap();
    }

    fn put_rea(root: &std::path::Path, version: &str) {
        put_part(root, "rea", "rea", version);
        put_part(root, "rea", "node", "24.22.3");
    }

    /// The app's `reverse` server over `root` on `platform` (the embedded lock; the
    /// development rules, so Kuna is offered on Linux).
    fn reverse_server(
        root: &std::path::Path,
        platform: reverse::Platform,
    ) -> std::sync::Arc<reverse::ReverseServer> {
        reverse::ReverseServer::with_installer(
            reverse::Installer::new(reverse::ReverseLayout::new(root), platform)
                .with_release_rules(false),
        )
    }

    /// The 37 reverse tools as the Tools panel lists them, all switched on or all off.
    fn reverse_dtos(on: bool) -> Vec<ToolDto> {
        reverse::allow::MODEL_TOOLS
            .iter()
            .map(|name| ToolDto {
                exposed: name.to_string(),
                original: name.to_string(),
                ..dto(reverse::server::SERVER_NAME, on)
            })
            .collect()
    }

    /// The reverse tools after the Tools panel's marks, as (name, reason).
    fn reverse_marks(server: Option<&reverse::ReverseServer>) -> Vec<(String, Option<String>)> {
        let mut list = reverse_dtos(true);
        mark_reverse(list.iter_mut(), server);
        for d in &list {
            // A marked tool is never offered; an unmarked one (switched on here) is.
            assert_eq!(d.offered(), d.schema_error.is_none(), "{}", d.exposed);
            assert_eq!(d.available, d.schema_error.is_none(), "{}", d.exposed);
        }
        list.into_iter()
            .map(|d| (d.exposed, d.schema_error))
            .collect()
    }

    fn is_session(name: &str) -> bool {
        reverse::allow::class_of(name) == Some(reverse::ToolClass::Session)
    }

    /// Q4 = B: the group is off in a new chat, whatever is installed and however much the
    /// model can remember; once the owner turns it on in a chat, new chats follow that last
    /// choice (it is not a per-chat-only group), and turning it off again carries over too.
    #[test]
    fn reverse_stays_off_in_a_new_chat_until_the_owner_turns_it_on() {
        let server = reverse::server::SERVER_NAME;
        assert!(!DEFAULT_ON_BUILTIN.contains(&server));
        assert!(!PER_CHAT_ONLY.contains(&server));
        for (available, context) in [
            (true, Some(4096)),
            (true, Some(8192)),
            (true, Some(131_072)),
            (false, Some(32768)),
            (true, None),
        ] {
            assert!(!default_on(server, available, context));
            let new_chat = Selection::for_chat(None, None, available, context);
            for tool in reverse::allow::MODEL_TOOLS {
                assert!(!new_chat.on(tool, server, true), "{tool}");
            }
            assert!(new_chat.on("read_file", "files", true));
        }
        // Turned on in a chat: that list is the last choice, so a new chat starts with it.
        let mut last = names(&["read_file"]);
        last.extend(reverse::allow::MODEL_TOOLS.iter().map(|t| t.to_string()));
        let new_chat = Selection::for_chat(None, Some(&last), true, Some(4096));
        for tool in reverse::allow::MODEL_TOOLS {
            assert!(new_chat.on(tool, server, true), "{tool}");
        }
        // Turned off again in a chat: the next last choice leaves it out.
        let last = names(&["read_file"]);
        let new_chat = Selection::for_chat(None, Some(&last), true, Some(32768));
        assert!(!new_chat.on("open_binary", server, true));
        // A last choice saved before item 5 existed (any older list) leaves it off.
        let older = names(&["read_file", "web_search", "create_artifact"]);
        let new_chat = Selection::for_chat(None, Some(&older), true, Some(32768));
        assert!(!new_chat.on("open_binary", server, true));
    }

    /// Nothing installed: every one of the 37 tools says so (the panel shows it once on the
    /// group with "Set up reverse engineering"), and none is offered even when switched on.
    /// With no server to ask, the same.
    #[test]
    fn reverse_tools_say_when_rea_is_not_set_up() {
        let dir = scratch("not-set-up");
        let s = reverse_server(&dir.0, reverse::Platform::LinuxX64);
        for marks in [reverse_marks(Some(&s)), reverse_marks(None)] {
            assert_eq!(marks.len(), 37);
            for (tool, why) in marks {
                assert_eq!(why.as_deref(), Some(NOT_SET_UP), "{tool}");
            }
        }
        assert_eq!(
            s.unavailable_reason("open_binary").as_deref(),
            Some(REVERSE_NOT_SET_UP)
        );
    }

    /// rea without a decompiler: opening a file and the file checks work; the tools that
    /// work on an open file say they need a decompiler. With Kuna installed and chosen,
    /// every tool works.
    #[test]
    fn reverse_tools_say_when_a_decompiler_is_needed() {
        let dir = scratch("no-decompiler");
        put_rea(&dir.0, "6.9.0");
        let s = reverse_server(&dir.0, reverse::Platform::LinuxX64);
        let marks = reverse_marks(Some(&s));
        let session = marks.iter().filter(|(t, _)| is_session(t)).count();
        assert_eq!(session, 22, "the tools that work on the open file");
        for (tool, why) in &marks {
            if is_session(tool) {
                assert_eq!(why.as_deref(), Some(NEEDS_DECOMPILER), "{tool}");
            } else {
                assert_eq!(why, &None, "{tool}");
            }
        }
        for tool in ["close_binary", "binary_session", "procedure_pseudo_code"] {
            assert!(
                marks
                    .iter()
                    .any(|(t, w)| t == tool && w.as_deref() == Some(NEEDS_DECOMPILER))
            );
        }
        put_part(&dir.0, "kuna", "kuna", "1.990");
        s.set_decompiler(Some(reverse::Decompiler::Kuna));
        assert!(reverse_marks(Some(&s)).iter().all(|(_, why)| why.is_none()));
    }

    /// An installed rea this app has no reviewed allow-list for (or below its minimum):
    /// every tool but `open_binary` says it needs an update; `open_binary` stays offered
    /// so a request to open a program gets the plain reason from `describe`.
    #[test]
    fn reverse_tools_say_when_rea_needs_an_update() {
        let dir = scratch("needs-update");
        put_rea(&dir.0, "6.8.0");
        let s = reverse_server(&dir.0, reverse::Platform::LinuxX64);
        for (tool, why) in reverse_marks(Some(&s)) {
            if tool == "open_binary" {
                assert_eq!(why, None);
            } else {
                assert_eq!(why.as_deref(), Some(NEEDS_UPDATE), "{tool}");
            }
        }
    }

    /// A Mac without Apple's command line tools: every tool but `open_binary` says so.
    /// (Checked where this computer has no such tools, which is every computer but a Mac
    /// that has them.)
    #[test]
    fn reverse_tools_say_when_apple_tools_are_missing() {
        let mac = reverse::Platform::MacosArm64;
        if reverse::platform::apple_tools(mac) != reverse::AppleTools::Missing {
            return;
        }
        let dir = scratch("apple-tools");
        put_rea(&dir.0, "6.9.0");
        put_part(&dir.0, "kuna", "kuna", "1.990");
        let s = reverse_server(&dir.0, mac);
        s.set_decompiler(Some(reverse::Decompiler::Kuna));
        for (tool, why) in reverse_marks(Some(&s)) {
            if tool == "open_binary" {
                assert_eq!(why, None);
            } else {
                assert_eq!(why.as_deref(), Some(NEEDS_APPLE_TOOLS), "{tool}");
            }
        }
    }

    /// The group switched on while the decompiler tools could not run saves every name
    /// (the Tools panel sends them all; a saved choice keeps registered tools that cannot
    /// run yet), so installing a decompiler later leaves the group fully on in that chat.
    #[test]
    fn installing_a_decompiler_later_keeps_a_chats_reverse_group_fully_on() {
        let host = crate::tool_catalog::tests::builtin_host();
        let builtin = crate::tool_catalog::builtin_servers(&host);
        let all: Vec<String> = reverse::allow::MODEL_TOOLS
            .iter()
            .map(|t| t.to_string())
            .collect();
        let saved = user_switches(all.clone(), &host.tools(), &builtin);
        assert_eq!(saved, all, "every reverse tool is a switch, usable or not");

        let dir = scratch("later");
        put_rea(&dir.0, "6.9.0");
        let s = reverse_server(&dir.0, reverse::Platform::LinuxX64);
        let chat = Selection::for_chat(Some(&saved), None, true, Some(4096));
        let listed = |s: &reverse::ReverseServer| {
            let mut list = reverse_dtos(false);
            for d in &mut list {
                d.enabled_in_chat = chat.on(&d.exposed, &d.server, true);
            }
            mark_reverse(list.iter_mut(), Some(s));
            list
        };
        let before = listed(&s);
        assert!(before.iter().all(|d| d.enabled_in_chat));
        assert_eq!(before.iter().filter(|d| d.offered()).count(), 37 - 22);
        put_part(&dir.0, "kuna", "kuna", "1.990");
        s.set_decompiler(Some(reverse::Decompiler::Kuna));
        let after = listed(&s);
        assert!(
            after.iter().all(|d| d.offered()),
            "fully on once Kuna is installed"
        );
    }

    /// Only the built-in server's tools are marked: an external server the user happened
    /// to call "reverse" is left to its own switches.
    #[test]
    fn reverse_marks_touch_only_the_built_in_server() {
        let mut external = dto(reverse::server::SERVER_NAME, true);
        external.builtin = false;
        let mut files = dto("files", true);
        mark_reverse([&mut external, &mut files], None);
        assert!(external.offered() && files.offered());
    }

    /// The marks are made before the companions decide whether another tool is on, so a
    /// chat whose only switched-on group cannot run gets no companions either.
    #[test]
    fn reverse_marks_come_before_the_companions() {
        let source = include_str!("mcp_cmd.rs");
        let body = source
            .split("async fn chat_tool_list(")
            .nth(1)
            .expect("chat_tool_list")
            .split("\nasync fn ")
            .next()
            .unwrap();
        let marks = body.find("mark_reverse(").expect("marks the reverse tools");
        let companions = body.find("companions_follow(").expect("companions");
        assert!(marks < companions);
        assert!(body.contains("reverse::handle()"));
    }

    /// The group counts as on in a chat when one of the built-in `reverse` tools is on there,
    /// by the chat's own choice or the last choice; never by default (Q4 = B), and an
    /// outside server of the same name does not count.
    #[test]
    fn whether_the_reverse_group_is_on_in_a_chat() {
        let reverse = reverse::server::SERVER_NAME.to_string();
        let tools = vec![
            ("open_binary".to_string(), reverse.clone(), true),
            ("procedure_pseudo_code".to_string(), reverse.clone(), true),
            ("read_file".to_string(), "files".to_string(), true),
        ];
        let own_on = names(&["read_file", "open_binary"]);
        let own_off = names(&["read_file"]);
        assert!(reverse_group_on(&Selection::Chat(&own_on), &tools));
        assert!(!reverse_group_on(&Selection::Chat(&own_off), &tools));
        assert!(reverse_group_on(&Selection::LastChoice(&own_on), &tools));
        assert!(!reverse_group_on(&Selection::LastChoice(&own_off), &tools));
        for context in [None, Some(4_096), Some(8_192), Some(131_072)] {
            for browser_available in [false, true] {
                let defaults = Selection::Defaults {
                    browser_available,
                    context,
                };
                assert!(!reverse_group_on(&defaults, &tools), "{context:?}");
            }
        }
        assert!(!reverse_group_on(&Selection::NoChat, &tools));
        // An outside server that happens to be called "reverse" is not the group.
        let outside = vec![("open_binary".to_string(), reverse, false)];
        assert!(!reverse_group_on(&Selection::Chat(&own_on), &outside));
        assert!(!reverse_group_on(&Selection::Chat(&own_on), &[]));
    }

    /// [0.0.6] Data from 0.0.5: the owner saved servers called "reverse" and "coding_apps",
    /// names the app's own servers now have. At start they get free names; each keeps its
    /// row (so its tool switches and on/off switch) and its secrets, which move to the new
    /// name; the note for Settings → MCP lists them; other servers are left alone, and a
    /// second start changes nothing.
    #[tokio::test]
    async fn saved_servers_from_0_0_5_with_a_built_in_name_are_renamed() {
        use store::secrets::SecretStore;
        let store = store::Store::open_in_memory().expect("store");
        let secrets = store::secrets::MemorySecretStore::new();
        let reverse_config = json!({
            "type": "http",
            "url": "https://rev.example/mcp",
            "headers": { "Authorization": "Bearer ${secret:reverse.Authorization}", "X-Other": "${secret:shared}" }
        });
        let reverse_id = store
            .upsert_mcp_server(store::McpServerInput {
                name: "reverse".into(),
                config: reverse_config.clone(),
                enabled: true,
            })
            .await
            .unwrap();
        store
            .upsert_mcp_tool_pref(store::McpToolPref {
                server_id: reverse_id,
                tool_name: "lookup".into(),
                enabled: true,
                allow_any_args: false,
                always_ask: true,
                fingerprint: Some("fp".into()),
            })
            .await
            .unwrap();
        secrets
            .set(&mcp_secret_key("reverse.Authorization"), "token-1")
            .unwrap();
        secrets.set(&mcp_secret_key("shared"), "keep").unwrap();
        let apps_id = store
            .upsert_mcp_server(store::McpServerInput {
                name: "coding_apps".into(),
                config: json!({ "type": "stdio", "command": "npx", "args": ["apps-mcp"] }),
                enabled: false,
            })
            .await
            .unwrap();
        // The first free name is taken already.
        store
            .upsert_mcp_server(store::McpServerInput {
                name: "coding_apps (yours)".into(),
                config: json!({ "command": "uvx", "args": ["x"] }),
                enabled: true,
            })
            .await
            .unwrap();
        let fetch_id = store
            .upsert_mcp_server(store::McpServerInput {
                name: "fetch".into(),
                config: json!({ "command": "uvx", "args": ["mcp-server-fetch"] }),
                enabled: true,
            })
            .await
            .unwrap();

        let host = crate::tool_catalog::tests::builtin_host();
        let builtin = crate::tool_catalog::builtin_servers(&host);
        assert!(builtin.contains("reverse") && builtin.contains("coding_apps"));
        let renamed = rename_clashing_servers(&store, &secrets, &builtin).await;
        assert_eq!(
            renamed,
            vec![
                RenamedServer {
                    from: "coding_apps".into(),
                    to: "coding_apps (yours 2)".into()
                },
                RenamedServer {
                    from: "reverse".into(),
                    to: "reverse (yours)".into()
                },
            ]
        );

        let rev = store.get_mcp_server(reverse_id).await.unwrap().unwrap();
        assert_eq!(rev.name, "reverse (yours)");
        assert!(rev.enabled);
        assert_eq!(
            rev.config,
            json!({
                "type": "http",
                "url": "https://rev.example/mcp",
                "headers": { "Authorization": "Bearer ${secret:reverse__yours_.Authorization}", "X-Other": "${secret:shared}" }
            })
        );
        assert!(serde_json::from_value::<ServerConfig>(rev.config.clone()).is_ok());
        assert_eq!(
            secrets
                .get(&mcp_secret_key("reverse__yours_.Authorization"))
                .unwrap()
                .as_deref(),
            Some("token-1")
        );
        assert_eq!(
            secrets
                .get(&mcp_secret_key("reverse.Authorization"))
                .unwrap(),
            None,
            "the old secret name is gone"
        );
        assert_eq!(
            secrets.get(&mcp_secret_key("shared")).unwrap().as_deref(),
            Some("keep"),
            "a secret the server does not own is left alone"
        );
        assert_eq!(
            own_secret_names(&rev.name, &rev.config),
            vec!["reverse__yours_.Authorization".to_string()],
            "removing it later deletes its secret"
        );
        let prefs = store.mcp_tool_prefs(reverse_id).await.unwrap();
        assert_eq!(prefs.len(), 1);
        assert!(prefs[0].always_ask && prefs[0].enabled);
        let apps = store.get_mcp_server(apps_id).await.unwrap().unwrap();
        assert_eq!(apps.name, "coding_apps (yours 2)");
        assert!(!apps.enabled);
        assert_eq!(
            store.get_mcp_server(fetch_id).await.unwrap().unwrap().name,
            "fetch"
        );
        let note: Vec<RenamedServer> = store.get_setting(RENAMED_SERVERS).await.unwrap().unwrap();
        assert_eq!(note, renamed);

        // A second start renames nothing and keeps the note as it is.
        assert!(
            rename_clashing_servers(&store, &secrets, &builtin)
                .await
                .is_empty()
        );
        let note: Vec<RenamedServer> = store.get_setting(RENAMED_SERVERS).await.unwrap().unwrap();
        assert_eq!(note.len(), 2);
        // No saved name is a built-in name any more.
        let names: BTreeSet<String> = store
            .list_mcp_servers()
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.name)
            .collect();
        assert!(names.is_disjoint(&builtin), "{names:?}");
    }

    /// A free name is never taken and never shares secrets with another server.
    #[test]
    fn a_free_name_is_not_taken_and_shares_no_secrets() {
        let taken = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            free_name("reverse", &taken(&["reverse"])),
            "reverse (yours)"
        );
        assert_eq!(
            free_name("reverse", &taken(&["reverse", "reverse (yours)"])),
            "reverse (yours 2)"
        );
        // "reverse__yours_" has the same secret prefix as "reverse (yours)".
        assert_eq!(
            free_name("reverse", &taken(&["reverse", "reverse__yours_"])),
            "reverse (yours 2)"
        );
    }

    /// [0.0.6] Whether a tool is built in comes from its server's slot in the host, so the
    /// built-in `reverse` group counts even if a saved server once had that name; the
    /// Tools list and the group check never decide it from saved names.
    #[test]
    fn built_in_comes_from_the_host_not_from_saved_names() {
        let host = crate::tool_catalog::tests::builtin_host();
        let tools = tools_with_kind(&host);
        assert!(
            tools
                .iter()
                .filter(|(_, server, _)| server == reverse::server::SERVER_NAME
                    || server == crate::outside_agent::SERVER_NAME)
                .all(|(_, _, builtin)| *builtin)
        );
        assert!(tools.iter().all(|(_, _, builtin)| *builtin));
        let on = names(&["read_file", "open_binary"]);
        assert!(reverse_group_on(&Selection::Chat(&on), &tools));

        let source = include_str!("mcp_cmd.rs");
        for start in ["async fn tool_list(", "async fn reverse_switched_on("] {
            let body = source
                .split(start)
                .nth(1)
                .expect(start)
                .split("\n}\n")
                .next()
                .unwrap();
            assert!(
                body.contains("builtin_servers") || body.contains("tools_with_kind("),
                "{start}"
            );
            assert!(
                !body.contains("list_mcp_servers"),
                "{start} decides from saved names"
            );
        }
    }

    /// [0.0.6] A renamed server's tools stay on where they were on: in each chat's own
    /// choice (and in what the chat remembers as shown and found), and in the last choice
    /// new chats start with. A tool of another server whose name starts alike stays as it is.
    #[tokio::test]
    async fn a_renamed_servers_tools_stay_on_in_chats_and_in_new_chats() {
        let store = store::Store::open_in_memory().expect("store");
        let secrets = store::secrets::MemorySecretStore::new();
        for (name, command) in [
            ("reverse", "rev-mcp"),
            ("coding_apps", "apps-mcp"),
            ("coding_apps (yours)", "other-mcp"),
        ] {
            store
                .upsert_mcp_server(store::McpServerInput {
                    name: name.into(),
                    config: json!({ "command": command }),
                    enabled: true,
                })
                .await
                .unwrap();
        }
        let free = |_: &str| false;
        // A name long enough to be shortened, before and after.
        let long_tool = "t".repeat(60);
        let old_long = mcp_host::names::exposed_name("reverse", &long_tool, &free);
        let other_long = mcp_host::names::exposed_name("coding_apps (yours)", &long_tool, &free);
        let chat = store
            .create_chat(store::NewChat {
                title: "with tools".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store
            .update_chat(
                chat,
                store::ChatPatch {
                    tool_prefs: Some(json!({
                        "enabled": ["reverse__lookup", "read_file", "coding_apps__yours___y", old_long, other_long],
                        "shown": ["reverse__lookup"],
                        "found": ["coding_apps__x"],
                        "thinkingOff": true,
                    })),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let untouched = store
            .create_chat(store::NewChat {
                title: "no choice".into(),
                ..Default::default()
            })
            .await
            .unwrap();
        store
            .set_setting(
                DEFAULT_TOOLS,
                &names(&["reverse__lookup", "coding_apps__x", "read_file"]),
            )
            .await
            .unwrap();

        let host = crate::tool_catalog::tests::builtin_host();
        let builtin = crate::tool_catalog::builtin_servers(&host);
        let renamed = rename_clashing_servers(&store, &secrets, &builtin).await;
        let to: Vec<&str> = renamed.iter().map(|r| r.to.as_str()).collect();
        assert_eq!(to, ["coding_apps (yours 2)", "reverse (yours)"]);

        // The names the host gives these tools under the new server names.
        let lookup = mcp_host::names::exposed_name("reverse (yours)", "lookup", &free);
        assert_eq!(lookup, "reverse__yours___lookup");
        let x = mcp_host::names::exposed_name("coding_apps (yours 2)", "x", &free);
        let new_long = mcp_host::names::exposed_name("reverse (yours)", &long_tool, &free);
        assert_ne!(new_long, old_long);

        let stored = store.get_chat(chat).await.unwrap().unwrap().tool_prefs;
        let prefs: crate::turns::ChatTools = serde_json::from_value(stored).unwrap();
        let enabled = prefs.enabled.expect("the chat keeps its own choice");
        assert_eq!(
            enabled,
            vec![
                lookup.clone(),
                "read_file".to_string(),
                "coding_apps__yours___y".to_string(),
                new_long,
                other_long,
            ]
        );
        assert!(Selection::Chat(&enabled).on(&lookup, "reverse (yours)", false));
        assert_eq!(prefs.shown, vec![lookup.clone()]);
        assert_eq!(prefs.found, vec![x.clone()]);
        assert!(prefs.thinking_off, "the rest of the chat's choices stay");
        assert_eq!(
            store.get_chat(untouched).await.unwrap().unwrap().tool_prefs,
            json!({})
        );
        let last: Vec<String> = store.get_setting(DEFAULT_TOOLS).await.unwrap().unwrap();
        assert_eq!(last, vec![lookup, x, "read_file".to_string()]);
    }

    /// Only the renamed server's own tools get the new name, and removing a server never
    /// deletes a secret another saved server refers to.
    #[test]
    fn only_the_renamed_servers_tools_and_secrets_follow_it() {
        let owners = names(&["reverse (yours)", "reverse_", "reverse x"]);
        let map = |name: &str| renamed_tool(name, "reverse", "reverse (yours 2)", &owners);
        assert_eq!(map("reverse__lookup").as_deref(), Some("reverse__yours_2___lookup"));
        // A name the host made unique stays unique.
        assert_eq!(map("reverse__lookup_2").as_deref(), Some("reverse__yours_2___lookup_2"));
        for theirs in [
            "reverse__yours___lookup",
            "reverse___lookup",
            "reverse_x__lookup",
            "reverse__",
            "reverse",
            "read_file",
            "reversed__lookup",
        ] {
            assert_eq!(map(theirs), None, "{theirs}");
        }
        // Each length of name the host may give, cut or not.
        for len in [1, 40, 47, 48, 55, 56, 60, 70] {
            let tool = "t".repeat(len);
            let free = |_: &str| false;
            let old = mcp_host::names::exposed_name("reverse", &tool, &free);
            let new = mcp_host::names::exposed_name("reverse (yours 2)", &tool, &free);
            assert_eq!(map(&old), Some(new), "{len}");
            let long_owner = mcp_host::names::exposed_name("reverse (yours)", &tool, &free);
            assert_eq!(map(&long_owner), None, "{len}");
        }

        let builtin: BTreeSet<String> = ["reverse".to_string()].into();
        let config = json!({ "headers": {
            "A": "${secret:reverse.A}",
            "B": "${secret:reverse__yours_.B}",
            "C": "${secret:reverse.shared}",
            "D": "${secret:fetch.D}",
        }});
        let other = json!({ "env": { "S": "${secret:reverse.shared}" } });
        assert_eq!(
            removable_secret_names("reverse (yours)", &config, &builtin, &[&other]),
            names(&["reverse.A", "reverse__yours_.B"])
        );
    }

    /// A keychain that refuses to read or to write while told to (locked, or the owner
    /// clicked Deny).
    #[derive(Default)]
    struct RefusingKeychain {
        inner: store::secrets::MemorySecretStore,
        refuse_get: std::sync::atomic::AtomicBool,
        refuse_set: std::sync::atomic::AtomicBool,
    }

    impl store::secrets::SecretStore for RefusingKeychain {
        fn get(&self, key: &str) -> Result<Option<String>, store::secrets::SecretError> {
            if self.refuse_get.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(store::secrets::SecretError::NoAccess("locked".into()));
            }
            self.inner.get(key)
        }
        fn set(&self, key: &str, value: &str) -> Result<(), store::secrets::SecretError> {
            if self.refuse_set.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(store::secrets::SecretError::NoAccess("locked".into()));
            }
            self.inner.set(key, value)
        }
        fn delete(&self, key: &str) -> Result<(), store::secrets::SecretError> {
            self.inner.delete(key)
        }
    }

    /// [0.0.6] The keychain refused while a saved server was renamed at start: the server
    /// keeps its secrets under their old names (so it still works), and removing it later
    /// deletes them all the same. Another server's secret stays.
    #[tokio::test]
    async fn a_server_renamed_while_the_keychain_refused_leaves_no_secret_when_removed() {
        use std::sync::atomic::Ordering;
        use store::secrets::SecretStore;
        let host = crate::tool_catalog::tests::builtin_host();
        let builtin = crate::tool_catalog::builtin_servers(&host);
        for refuse_get in [true, false] {
            let store = store::Store::open_in_memory().expect("store");
            let keychain = RefusingKeychain::default();
            keychain
                .set(&mcp_secret_key("reverse.Authorization"), "token-1")
                .unwrap();
            keychain
                .set(&mcp_secret_key("fetch.env.KEY"), "other")
                .unwrap();
            let config = json!({
                "type": "http",
                "url": "https://rev.example/mcp",
                "headers": { "Authorization": "Bearer ${secret:reverse.Authorization}" }
            });
            for (name, config) in [
                ("reverse", config.clone()),
                (
                    "fetch",
                    json!({ "command": "uvx", "env": { "KEY": "${secret:fetch.env.KEY}" } }),
                ),
            ] {
                store
                    .upsert_mcp_server(store::McpServerInput {
                        name: name.into(),
                        config,
                        enabled: true,
                    })
                    .await
                    .unwrap();
            }
            let refusal = if refuse_get {
                &keychain.refuse_get
            } else {
                &keychain.refuse_set
            };
            refusal.store(true, Ordering::SeqCst);
            let renamed = rename_clashing_servers(&store, &keychain, &builtin).await;
            refusal.store(false, Ordering::SeqCst);
            assert_eq!(
                renamed,
                vec![RenamedServer {
                    from: "reverse".into(),
                    to: "reverse (yours)".into()
                }]
            );
            let row = store
                .list_mcp_servers()
                .await
                .unwrap()
                .into_iter()
                .find(|r| r.name == "reverse (yours)")
                .expect("renamed");
            assert_eq!(row.config, config, "it keeps the secret names that work");
            assert_eq!(
                keychain
                    .get(&mcp_secret_key("reverse.Authorization"))
                    .unwrap()
                    .as_deref(),
                Some("token-1")
            );

            // Later the owner removes it; the keychain works again by then.
            assert!(
                delete_saved_server(&store, &keychain, &builtin, "reverse (yours)")
                    .await
                    .unwrap()
            );
            assert_eq!(
                keychain
                    .get(&mcp_secret_key("reverse.Authorization"))
                    .unwrap(),
                None,
                "refuse_get {refuse_get}: its secret is deleted"
            );
            assert_eq!(keychain.inner.len(), 1, "only the other server's secret is left");
            assert_eq!(
                keychain
                    .get(&mcp_secret_key("fetch.env.KEY"))
                    .unwrap()
                    .as_deref(),
                Some("other")
            );
            assert!(
                store
                    .list_mcp_servers()
                    .await
                    .unwrap()
                    .iter()
                    .all(|r| r.name == "fetch")
            );
        }
    }
}
