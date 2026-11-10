//! The tool servers the app runs (ARCHITECTURE.md 9): the MCP host with the built-in
//! servers (files, terminal, git, skills) and the user's external servers.

use std::path::PathBuf;
use std::sync::Arc;

use mcp_host::{Host, HostEvent, HostOptions, Lifecycle, ServerConfig};
use tauri::{AppHandle, Emitter, Manager};

use crate::state::AppState;

/// Keychain key for a `${secret:<name>}` placeholder in an MCP server config.
pub fn mcp_secret_key(name: &str) -> String {
    format!("mcp-secret:{name}")
}

/// Where imported skills live: `<app data>/skills`.
pub fn skills_root(app: &AppHandle) -> Option<PathBuf> {
    app.path()
        .app_local_data_dir()
        .ok()
        .map(|d| d.join("skills"))
}

/// Setting: also read skills from `~/.agents/skills` (shared with other agents).
pub const SHARED_SKILLS: &str = "sharedSkillsFolder";

/// Builds the host with the built-in servers registered.
pub async fn build_host(
    app: &AppHandle,
    secrets: Arc<dyn store::secrets::SecretStore>,
    skills: Arc<tools::SkillsServer>,
) -> Host {
    let resolver_secrets = secrets.clone();
    let captured = captured_env().await;
    let opts = HostOptions {
        captured_env: captured.clone(),
        stderr_log_dir: app.path().app_log_dir().ok(),
        secret_resolver: Arc::new(move |name: &str| {
            resolver_secrets.get(&mcp_secret_key(name)).ok().flatten()
        }),
        default_cwd: app.path().home_dir().ok(),
        client_name: app.package_info().name.clone(),
        client_version: app.package_info().version.to_string(),
        ..HostOptions::default()
    };
    let host = Host::new(opts);
    host.add_builtin(Arc::new(tools::FilesServer::default()));
    host.add_builtin(Arc::new(tools::TerminalServer::new(
        tools::TerminalConfig::default(),
    )));
    host.add_builtin(Arc::new(tools::GitServer::new(git_config().await)));
    host.add_builtin(skills);
    host
}

/// System git, with the user's own credential helpers (global and system config only;
/// repository config never chooses a helper).
async fn git_config() -> tools::GitConfig {
    let path = std::env::var_os("PATH");
    let git = runtime::resolve_program("git", path.as_deref());
    let mut env: Vec<(String, String)> = Vec::new();
    for key in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "SystemRoot",
        "APPDATA",
        "LOCALAPPDATA",
        "SSH_AUTH_SOCK",
        "TMPDIR",
        "TEMP",
        "TMP",
    ] {
        if let Ok(v) = std::env::var(key) {
            env.push((key.to_string(), v));
        }
    }
    let credential_helpers = match &git {
        Some(g) => user_credential_helpers(g, &env).await,
        None => Vec::new(),
    };
    tools::GitConfig {
        git,
        env,
        credential_helpers,
    }
}

async fn user_credential_helpers(git: &std::path::Path, env: &[(String, String)]) -> Vec<String> {
    use tokio::io::AsyncReadExt;
    let mut spec_env = runtime::EnvSpec::empty();
    for (k, v) in env {
        spec_env = spec_env.set(k.clone(), v.clone());
    }
    // Run outside any repository so only system and global config are read.
    let spec = runtime::SpawnSpec::new(git, std::env::temp_dir())
        .args(["config", "--get-all", "credential.helper"])
        .env(spec_env)
        .stdin(runtime::StdioMode::Null)
        .stdout(runtime::StdioMode::Piped)
        .stderr(runtime::StdioMode::Null);
    let Ok(mut child) = runtime::spawn(spec) else {
        return Vec::new();
    };
    let mut out = String::new();
    let read = async {
        if let Some(mut stdout) = child.take_stdout() {
            let _ = stdout.read_to_string(&mut out).await;
        }
        child.wait().await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(5), read).await {
        Ok(Ok(status)) if status.success() => out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect(),
        Ok(_) => Vec::new(),
        Err(_) => {
            let _ = child.kill().await;
            Vec::new()
        }
    }
}

/// The login shell's environment on macOS (an app started from Finder gets a minimal
/// one, without Homebrew, nvm and the like on PATH). `None` elsewhere, and when the
/// shell does not answer within 10 s.
#[cfg(target_os = "macos")]
async fn captured_env() -> Option<std::collections::BTreeMap<String, String>> {
    use tokio::io::AsyncReadExt;
    let home = std::env::var("HOME").ok()?;
    let spec = runtime::SpawnSpec::new("/bin/zsh", &home)
        .args(["-lc", "/usr/bin/env -0"])
        .env(
            runtime::EnvSpec::minimal()
                .inherit("HOME")
                .inherit("USER")
                .inherit("LOGNAME")
                .inherit("SHELL")
                .inherit("TMPDIR"),
        )
        .stdin(runtime::StdioMode::Null)
        .stdout(runtime::StdioMode::Piped)
        .stderr(runtime::StdioMode::Null);
    let mut child = runtime::spawn(spec).ok()?;
    let mut out = Vec::new();
    let read = async {
        if let Some(mut stdout) = child.take_stdout() {
            let _ = stdout.read_to_end(&mut out).await;
        }
        child.wait().await
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), read).await {
        Ok(Ok(status)) if status.success() => {}
        Ok(_) => return None,
        Err(_) => {
            let _ = child.kill().await;
            tracing::warn!("the login shell did not report its environment in 10 s");
            return None;
        }
    }
    let env = parse_env0(&out);
    (!env.is_empty()).then_some(env)
}

#[cfg(not(target_os = "macos"))]
async fn captured_env() -> Option<std::collections::BTreeMap<String, String>> {
    None
}

/// Parses `env -0` output (NUL-separated `NAME=value`).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn parse_env0(bytes: &[u8]) -> std::collections::BTreeMap<String, String> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter_map(|kv| kv.split_once('='))
        .filter(|(k, _)| !k.is_empty() && !k.contains(char::is_whitespace))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Reloads skills from disk into the skills server: the app's skills folder and, when
/// switched on, `~/.agents/skills`. Disabled skills are left out.
pub async fn reload_skills(app: &AppHandle) {
    let state = app.state::<AppState>();
    let mut roots = Vec::new();
    if let Some(root) = skills_root(app) {
        let _ = std::fs::create_dir_all(&root);
        roots.push(root);
    }
    let shared: bool = state
        .store
        .get_setting(SHARED_SKILLS)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    if shared && let Ok(home) = app.path().home_dir() {
        roots.push(home.join(".agents").join("skills"));
    }
    let disabled: Vec<String> = state
        .store
        .get_setting("disabledSkills")
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    let mut all = Vec::new();
    for root in roots {
        let (skills, errors) = tokio::task::spawn_blocking(move || tools::skills::load_all(&root))
            .await
            .unwrap_or_default();
        for (path, e) in errors {
            tracing::warn!("skipped a skill at {}: {e}", path.display());
        }
        for s in skills {
            if !all.iter().any(|x: &tools::Skill| x.name == s.name) {
                all.push(s);
            }
        }
    }
    all.retain(|s| !disabled.contains(&s.name));
    state.skills.set_enabled(all);
}

/// Starts the user's saved, enabled servers (each in the background).
pub async fn start_saved_servers(app: &AppHandle) {
    let state = app.state::<AppState>();
    let servers = match state.store.list_mcp_servers().await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("could not read MCP servers: {e}");
            return;
        }
    };
    for s in servers.into_iter().filter(|s| s.enabled) {
        let Ok(cfg) = serde_json::from_value::<ServerConfig>(s.config.clone()) else {
            tracing::warn!(server = %s.name, "stored MCP config is not readable; skipped");
            continue;
        };
        let lifecycle = s.protocol_era.as_deref().and_then(|e| {
            serde_json::from_value::<Lifecycle>(serde_json::Value::String(e.to_string())).ok()
        });
        let host = state.host.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = host
                .add_external_with_lifecycle(&s.name, cfg, lifecycle)
                .await
            {
                tracing::warn!(server = %s.name, "MCP server did not start: {e}");
            }
        });
    }
}

/// Forwards host changes to the main window and remembers each server's lifecycle.
pub fn forward_events(app: AppHandle, host: Host) {
    let mut rx = host.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(HostEvent::StatusChanged { server }) => {
                    remember_lifecycle(&app, &host, &server).await;
                    let _ = app.emit_to(crate::MAIN_WINDOW, "mcp-changed", server);
                }
                Ok(HostEvent::ToolsChanged { server }) => {
                    let _ = app.emit_to(crate::MAIN_WINDOW, "mcp-changed", server);
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = app.emit_to(crate::MAIN_WINDOW, "mcp-changed", String::new());
                }
                Err(_) => break,
            }
        }
    });
}

async fn remember_lifecycle(app: &AppHandle, host: &Host, server: &str) {
    let Some(status) = host.servers().into_iter().find(|s| s.name == server) else {
        return;
    };
    let Some(lifecycle) = status.lifecycle else {
        return;
    };
    let era = serde_json::to_value(lifecycle)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string));
    let state = app.state::<AppState>();
    if let Ok(list) = state.store.list_mcp_servers().await
        && let Some(row) = list.into_iter().find(|r| r.name == server)
        && row.protocol_era != era
    {
        let _ = state.store.set_mcp_protocol_era(row.id, era).await;
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn env0_is_parsed() {
        let env =
            super::parse_env0(b"PATH=/opt/homebrew/bin:/usr/bin\0A=b=c\0MULTI=line1\nline2\0\0");
        assert_eq!(
            env.get("PATH").map(String::as_str),
            Some("/opt/homebrew/bin:/usr/bin")
        );
        assert_eq!(env.get("A").map(String::as_str), Some("b=c"));
        assert_eq!(env.get("MULTI").map(String::as_str), Some("line1\nline2"));
        assert_eq!(env.len(), 3);
    }
}
