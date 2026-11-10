//! Settings → MCP servers, and the per-chat tools panel (ARCHITECTURE.md 9.2, 9.3, 13.4).
//!
//! - A pasted config is parsed here, in Rust; the webview only picks which servers to add.
//! - Every value in `env` and `headers`, and each credential found in `args` or a URL, is
//!   stored in the OS keychain; the saved config keeps `${secret:<name>}` placeholders.
//! - Tools of a new server start off (D10). A tool whose fingerprint changed starts off
//!   again and loses its grants (9.2).

use std::collections::BTreeSet;

use mcp_host::{ServerConfig, ServerKind, ServerState, ToolPrefs};
use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::toolset::mcp_secret_key;

/// Setting: the tools of the last chat whose tools the user changed (new chats start so).
const DEFAULT_TOOLS: &str = "defaultChatTools";

/// Built-in servers whose tools are on in a new chat.
const DEFAULT_ON_BUILTIN: &[&str] = &["files", "skills"];

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
    for e in entries.into_iter().filter(|e| names.contains(&e.name)) {
        if existing.contains(&e.name) {
            return Err(AppError::new(
                "mcp_exists",
                format!("A server named \"{}\" already exists.", e.name),
                "Remove the old one first, or rename this one in the JSON.",
                String::new(),
            ));
        }
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
        for secret in secret_names(&row.config) {
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

/// Tools that are on in a chat, given its stored preferences.
pub async fn enabled_for_chat(
    state: &AppState,
    chat: &store::Chat,
) -> Vec<mcp_host::RegisteredTool> {
    let list = chat_tool_list(state, chat).await;
    list.into_iter()
        .filter(|(_, d)| d.enabled_in_chat && d.available && d.schema_error.is_none())
        .map(|(t, _)| t)
        .collect()
}

async fn chat_tool_list(
    state: &AppState,
    chat: &store::Chat,
) -> Vec<(mcp_host::RegisteredTool, ToolDto)> {
    let prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    tool_list(state, Some(&prefs)).await
}

async fn tool_list(
    state: &AppState,
    prefs: Option<&crate::turns::ChatTools>,
) -> Vec<(mcp_host::RegisteredTool, ToolDto)> {
    let stored = state.store.list_mcp_servers().await.unwrap_or_default();
    // The user's last choice of tools is the default for new chats (9.3).
    let last_choice: Option<Vec<String>> =
        state.store.get_setting(DEFAULT_TOOLS).await.ok().flatten();
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
        let enabled_in_chat = match prefs.map(|p| &p.enabled) {
            Some(Some(list)) => list.contains(&t.exposed),
            Some(None) => match &last_choice {
                Some(list) => list.contains(&t.exposed),
                None if builtin => DEFAULT_ON_BUILTIN.contains(&t.server.as_str()),
                None => false,
            },
            None => false,
        };
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
    Ok(chat_tool_list(&state, &chat)
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
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let mut prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs).unwrap_or_default();
    prefs.thinking_off = !on;
    state
        .store
        .update_chat(
            chat_id,
            store::ChatPatch {
                tool_prefs: Some(serde_json::to_value(&prefs).map_err(AppError::internal)?),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}

/// One server's tools with the user's switches (Settings → MCP servers).
#[tauri::command]
pub async fn list_server_tools(
    state: State<'_, AppState>,
    server: String,
) -> AppResult<Vec<ToolDto>> {
    Ok(tool_list(&state, None)
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
    let known: BTreeSet<String> = state.host.tools().into_iter().map(|t| t.exposed).collect();
    let enabled: Vec<String> = enabled.into_iter().filter(|e| known.contains(e)).collect();
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let mut prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs).unwrap_or_default();
    // The last choice becomes the default for new chats (9.3).
    state.store.set_setting(DEFAULT_TOOLS, &enabled).await?;
    prefs.enabled = Some(enabled);
    state
        .store
        .update_chat(
            chat_id,
            store::ChatPatch {
                tool_prefs: Some(serde_json::to_value(&prefs).map_err(AppError::internal)?),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
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
