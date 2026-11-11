//! The tool servers the app runs (ARCHITECTURE.md 9): the MCP host with the built-in
//! servers (files, terminal, git, skills, subagents) and the user's external servers.

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
    pass_through: Vec<String>,
    identity: crate::git_cmd::GitIdentity,
) -> Host {
    let resolver_secrets = secrets.clone();
    let captured = captured_env().await;
    let _ = CAPTURED.set(captured.clone());
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
    host.add_builtin(Arc::new(tools::TerminalServer::new(terminal_config(
        captured.as_ref(),
        pass_through,
    ))));
    let base = git_config(captured.as_ref()).await;
    let _ = GIT_BASE.set(base.clone());
    let git = crate::git_cmd::server(base, secrets.clone(), &identity);
    record_backend(&git);
    host.add_builtin(Arc::new(git));
    host.add_builtin(skills);
    // Lists `run_subagent` (the "Subagents" group, off in a new chat); the turn runs it.
    host.add_builtin(Arc::new(crate::subagent::SubagentsServer));
    host
}

/// System git, with the user's own credential helpers (global and system config only;
/// repository config never chooses a helper).
/// The git configuration found at start (before the commit identity), to rebuild the
/// git server when the identity changes.
static GIT_BASE: std::sync::OnceLock<tools::GitConfig> = std::sync::OnceLock::new();
/// Whether the git tools use the built-in library (true) or system git.
static GIT_FALLBACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn record_backend(git: &tools::GitServer) {
    GIT_FALLBACK.store(
        git.backend() == tools::GitBackend::Libgit2,
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// Which git the tools use.
pub fn git_backend() -> tools::GitBackend {
    if GIT_FALLBACK.load(std::sync::atomic::Ordering::Relaxed) {
        tools::GitBackend::Libgit2
    } else {
        tools::GitBackend::System
    }
}

/// Rebuilds the git server with a new commit identity.
pub async fn rebuild_git(state: &AppState, identity: &crate::git_cmd::GitIdentity) {
    let Some(base) = GIT_BASE.get().cloned() else {
        return;
    };
    let git = crate::git_cmd::server(base, state.secrets.clone(), identity);
    record_backend(&git);
    state.host.add_builtin(Arc::new(git));
}

/// The captured login environment (macOS), kept to rebuild the terminal server.
static CAPTURED: std::sync::OnceLock<Option<std::collections::BTreeMap<String, String>>> =
    std::sync::OnceLock::new();

/// The terminal's pass-through list.
#[tauri::command]
pub async fn get_terminal_pass_through(
    state: tauri::State<'_, AppState>,
) -> crate::error::AppResult<Vec<String>> {
    Ok(state
        .store
        .get_setting(TERMINAL_PASS_THROUGH)
        .await?
        .unwrap_or_default())
}

/// Replaces the terminal's pass-through list and rebuilds the terminal server.
#[tauri::command]
pub async fn set_terminal_pass_through(
    state: tauri::State<'_, AppState>,
    names: Vec<String>,
) -> crate::error::AppResult<Vec<String>> {
    let mut clean: Vec<String> = names
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect();
    if let Some(bad) = clean
        .iter()
        .find(|n| n.len() > 128 || !n.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err(crate::error::AppError::bad_request(format!(
            "\"{bad}\" is not a variable name (letters, digits and _ only)."
        )));
    }
    clean.sort();
    clean.dedup();
    state
        .store
        .set_setting(TERMINAL_PASS_THROUGH, &clean)
        .await?;
    let captured = CAPTURED.get().cloned().flatten();
    state
        .host
        .add_builtin(Arc::new(tools::TerminalServer::new(terminal_config(
            captured.as_ref(),
            clean.clone(),
        ))));
    Ok(clean)
}

/// Setting: environment variables the terminal passes through even though their names
/// look secret (9.3, "Environment").
pub const TERMINAL_PASS_THROUGH: &str = "terminalPassThrough";

/// The terminal's configuration: the captured login environment on macOS, the app's own
/// elsewhere, and the user's pass-through list.
pub fn terminal_config(
    captured: Option<&std::collections::BTreeMap<String, String>>,
    pass_through: Vec<String>,
) -> tools::TerminalConfig {
    let mut config = tools::TerminalConfig::default();
    if let Some(env) = captured {
        config.base_env = env.clone();
    }
    config.pass_through = pass_through;
    config
}

/// The variable from the captured login environment (macOS), else the app's own.
fn captured_var(
    captured: Option<&std::collections::BTreeMap<String, String>>,
    key: &str,
) -> Option<String> {
    captured
        .and_then(|c| c.get(key).cloned())
        .or_else(|| std::env::var(key).ok())
}

/// The system git the tools run, if any. On macOS without the developer tools, Apple's
/// `/usr/bin/git` is a stub that opens an installer dialog, so it is never run (9.2, 9.3):
/// a git found elsewhere in `PATH` (Homebrew) is used instead, else none, and the git
/// tools use the built-in library.
async fn system_git(path: Option<&str>) -> Option<PathBuf> {
    let search = path.map(std::ffi::OsStr::new);
    let found = runtime::resolve_program("git", search);
    let stub = is_git_stub(found.as_deref(), cfg!(target_os = "macos"));
    // The developer-tools check runs `xcode-select`, so only for the stub on macOS.
    let stub_without_tools = stub && !mcp_host::developer_tools_installed().await;
    choose_git(found, stub_without_tools, || {
        search.and_then(|s| mcp_host::alternative_to_stub("git", s))
    })
}

/// Whether `found` is Apple's git stub. Only on macOS: elsewhere `/usr/bin/git` is git.
fn is_git_stub(found: Option<&std::path::Path>, macos: bool) -> bool {
    macos && found.is_some_and(mcp_host::is_developer_tools_stub)
}

/// `found`, unless it is Apple's stub and the developer tools are missing: then another
/// git in `PATH` that is not a stub, else none.
fn choose_git(
    found: Option<PathBuf>,
    stub_without_tools: bool,
    alternative: impl FnOnce() -> Option<PathBuf>,
) -> Option<PathBuf> {
    if !stub_without_tools {
        return found;
    }
    alternative().filter(|g| !mcp_host::is_developer_tools_stub(g))
}

async fn git_config(
    captured: Option<&std::collections::BTreeMap<String, String>>,
) -> tools::GitConfig {
    let path = captured_var(captured, "PATH");
    let git = system_git(path.as_deref()).await;
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
        if let Some(v) = captured_var(captured, key) {
            env.push((key.to_string(), v));
        }
    }
    // No usable system git (or only Apple's stub): nothing is run to ask for helpers.
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
    use std::path::PathBuf;

    /// macOS without the developer tools: Apple's /usr/bin/git is never used.
    #[test]
    fn apples_git_stub_is_never_used_without_the_developer_tools() {
        let stub = PathBuf::from("/usr/bin/git");
        let brew = PathBuf::from("/opt/homebrew/bin/git");
        assert!(super::is_git_stub(Some(&stub), true));
        assert!(
            !super::is_git_stub(Some(&stub), false),
            "elsewhere /usr/bin/git is the real git"
        );
        assert!(!super::is_git_stub(Some(&brew), true));
        assert!(!super::is_git_stub(None, true));

        // No other git: none, so the tools use the built-in library.
        assert_eq!(super::choose_git(Some(stub.clone()), true, || None), None);
        // A Homebrew git later in PATH is used instead.
        assert_eq!(
            super::choose_git(Some(stub.clone()), true, || Some(brew.clone())),
            Some(brew.clone())
        );
        // Another stub is never picked as the alternative.
        assert_eq!(
            super::choose_git(Some(stub.clone()), true, || Some(stub.clone())),
            None
        );
        // With the developer tools, /usr/bin/git is the real git; no search is made.
        assert_eq!(
            super::choose_git(Some(stub.clone()), false, || panic!("not searched")),
            Some(stub)
        );
        assert_eq!(
            super::choose_git(Some(brew.clone()), false, || panic!("not searched")),
            Some(brew)
        );
    }

    /// Without a git, no credential helper is asked for (nothing is run).
    #[tokio::test]
    async fn no_git_means_no_credential_helper_probe() {
        let empty = std::env::temp_dir().join("harniss-no-git-here");
        let captured =
            std::collections::BTreeMap::from([("PATH".to_string(), empty.display().to_string())]);
        let config = super::git_config(Some(&captured)).await;
        assert_eq!(config.git, None);
        assert!(config.credential_helpers.is_empty());
    }

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
