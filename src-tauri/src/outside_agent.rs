//! Coding apps (0.0.6 item 3, direction B, experimental and off by default, D5): the local
//! model hands a task to Claude Code or Codex installed on this computer, with the tools
//! `ask_claude_code {task, may_edit?}` and `ask_codex {task, may_edit?}` of the group
//! "Coding apps (experimental)" in the Tools panel.
//!
//! What must hold:
//!
//! - **It asks, except in Full access (D11 as amended for 0.0.6, D12).** In Ask first and
//!   Allow all every call shows a card with Allow once / Deny, after a grant, with or
//!   without untrusted content: the action has risk `AlwaysAsk`, keeps asking, reaches the
//!   network, and its scope ([`policy::Scope::CodingApp`]) is one no grant covers and the
//!   engine always asks for. In Full access nothing asks (the terminal could start the
//!   programs anyway), but the checks below still refuse before the policy engine runs.
//! - **The card states only what the command line confines** ([`card_lines`], tested
//!   against [`arguments`]), names the program by its absolute path, and says that the task
//!   goes online with the user's account.
//! - **Programs are resolved once**, when the user turns the group on
//!   ([`find_coding_apps`]): the file is located on `PATH` (the login `PATH` on macOS) or
//!   in `~/.local/bin`, never run. Its absolute path, real path and identity
//!   ([`FileIdentity`]: device and inode on Unix, volume serial number and file index on
//!   Windows; size and modified time on both) are saved under [`SETTINGS_KEY`]. Before each
//!   run the path must still lead to the same file with the same identity, or nothing runs.
//!   A program inside an approved folder is refused. On Windows the real `.exe` is used
//!   when there is one; an npm `.cmd` shim gets only the fixed words of the template.
//! - **The task goes on standard input**, never on the command line. The command line is
//!   the fixed template of [`arguments`]: the model chooses only `may_edit`, which picks
//!   between two fixed templates. No token that turns off a permission check
//!   ([`policy::command::coding_apps::BYPASS_TOKENS`]) can appear in it (tested).
//! - **The working folder** is the chat's working folder, and it must be approved.
//! - **Full access does not widen these checks.** In Full access the chat's tool context
//!   lists the whole computer as approved (`approvals::widen`); here "approved" keeps
//!   meaning the folders the user approved ([`ApprovedNow`], read from the store), so a
//!   program anywhere is not "inside an approved folder" and the working folder must still
//!   be one the user approved.
//! - **Helpers never get these tools** (`subagent.rs`), in any mode.
//! - **Secret environment variables stay stripped** (the apps use their own stored login),
//!   as in the terminal (`tools::terminal::looks_secret`).
//! - **Stop** ends the process group (Unix) or job (Windows). Processes the app started in
//!   a new session can outlive it (known limit, ARCHITECTURE 13.14).
//! - **The answer is untrusted tool output**, shortened like any other long result.
//!
//! The templates. Every Claude Code flag was checked against its documentation
//! (code.claude.com/docs/en/cli-reference and settings) and against Claude Code 2.1.322 on
//! the build machine (`claude --help`, and the strings of its binary, without starting a
//! session):
//!
//! - Claude Code: `claude -p --restricted --safe-mode --strict-mcp-config --permission-mode
//!   default --tools Read,Grep,Glob --settings {"disableAllHooks":true,"permissions":
//!   {"blockReadsOutsideWorkingDirectories":true}} --output-format stream-json --verbose`;
//!   with `may_edit`, `Edit,Write` are added to `--tools` and the permission mode is
//!   `acceptEdits`. `--verbose` is not in the specification's template: 2.1.322 refuses
//!   `--output-format stream-json` with `-p` without it (its binary holds the check
//!   `outputFormat==="stream-json"&&!verbose` and the message "When using --print,
//!   --output-format=stream-json requires --verbose"), and every stream-json example in the
//!   documentation pairs the two. `--restricted` needs 2.1.248 or later and the setting
//!   `permissions.blockReadsOutsideWorkingDirectories` 2.1.257 or later (both from the
//!   documentation; [`FLAG_SINCE`]); a known older version is not offered.
//!   `--permission-mode default`: 2.1.322's `--help` lists the mode as `manual`; the
//!   documentation and the binary keep `default` as the value (`manual` is mapped to it).
//!   `--restricted` also means Claude Code reads no user settings file, so only the
//!   environment can point it at the sharing port.
//! - Codex: `codex exec --json --sandbox read-only|workspace-write -c features.hooks=false -`
//!   in the working folder. Codex is not installed on the build machine; every flag is
//!   from its documentation (developers.openai.com/codex, CLI reference, non-interactive
//!   mode and hooks): `exec`, `--json` ("Print newline-delimited JSON events"), `--sandbox`
//!   with `read-only` and `workspace-write`, `-c key=value`, the feature key `hooks`
//!   (`[features] hooks = false`; `--enable` is documented as `-c features.<name>=true`) and
//!   `-` ("Use `-` to pipe the prompt from stdin"). None was run.

mod identity;
mod output;
#[cfg(test)]
mod tests;

pub use identity::FileIdentity;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tauri::{AppHandle, Manager, State};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The built-in server (the "Coding apps (experimental)" group of the Tools panel).
pub const SERVER_NAME: &str = "coding_apps";
/// The tool that asks Claude Code.
pub const CLAUDE_TOOL: &str = "ask_claude_code";
/// The tool that asks Codex.
pub const CODEX_TOOL: &str = "ask_codex";
/// Setting: the programs found when the group was last turned on ([`SavedSetup`]).
/// Written only by [`find_coding_apps`]; `set_setting` cannot write it.
pub const SETTINGS_KEY: &str = "codingApps";

/// What the panel says when neither program is found.
pub const NOT_INSTALLED: &str = "Claude Code and Codex are not installed on this computer.";
/// What a helper is told when it tries one of these tools.
pub const HELPER_REFUSAL: &str = "Helpers cannot ask Claude Code or Codex. Only the main chat can, and the user approves each request.";

/// The longest task, in characters.
pub const MAX_TASK_CHARS: usize = 60_000;
/// The longest a task may run.
pub const MAX_RUN: Duration = Duration::from_secs(30 * 60);
/// How often the card says how long it has been working.
const TICK: Duration = Duration::from_secs(60);
/// Bytes of the end of the error stream kept.
const STDERR_TAIL: usize = 8 * 1024;

/// The settings Claude Code gets on its command line: no hooks, and no reads outside the
/// working folder in any permission mode.
pub const CLAUDE_SETTINGS: &str =
    r#"{"disableAllHooks":true,"permissions":{"blockReadsOutsideWorkingDirectories":true}}"#;

/// One coding app.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodingApp {
    ClaudeCode,
    Codex,
}

impl CodingApp {
    pub const ALL: [CodingApp; 2] = [CodingApp::ClaudeCode, CodingApp::Codex];

    /// Its name for people.
    pub fn label(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// The company its account and service belong to.
    pub fn company(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Anthropic",
            Self::Codex => "OpenAI",
        }
    }

    /// The tool that asks it.
    pub fn tool(self) -> &'static str {
        match self {
            Self::ClaudeCode => CLAUDE_TOOL,
            Self::Codex => CODEX_TOOL,
        }
    }

    /// The program's file name, without an extension.
    pub fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::Codex => "codex",
        }
    }

    /// Its npm package, whose `package.json` gives the version of an npm install.
    fn package(self) -> &'static str {
        match self {
            Self::ClaudeCode => "@anthropic-ai/claude-code",
            Self::Codex => "@openai/codex",
        }
    }

    /// The name in [`policy::Scope::CodingApp`] and in JSON.
    pub fn key(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claudeCode",
            Self::Codex => "codex",
        }
    }

    /// The app a tool asks, if it is one of these tools.
    pub fn from_tool(tool: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.tool() == tool)
    }
}

/// Whether a registered tool is one of these tools.
pub fn is_tool(tool: &mcp_host::RegisteredTool) -> bool {
    tool.server == SERVER_NAME && CodingApp::from_tool(&tool.original).is_some()
}

/// Whether a tool name is one of these tools (built-in names are fixed).
pub fn is_tool_name(name: &str) -> bool {
    CodingApp::from_tool(name).is_some()
}

/// Whether an action is a call of one of these tools.
pub fn is_action(action: &policy::Action) -> bool {
    (action.server == SERVER_NAME && is_tool_name(&action.tool)) || action.scope.names_coding_app()
}

// ---- The templates ---------------------------------------------------------------------

/// The fixed command line (after the program) for one app. `may_edit` picks one of two
/// fixed templates; nothing the model writes is ever in it. The task goes on standard
/// input.
pub fn arguments(app: CodingApp, may_edit: bool) -> Vec<String> {
    let words: Vec<&str> = match app {
        CodingApp::ClaudeCode => vec![
            "-p",
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--permission-mode",
            if may_edit { "acceptEdits" } else { "default" },
            "--tools",
            if may_edit {
                "Read,Grep,Glob,Edit,Write"
            } else {
                "Read,Grep,Glob"
            },
            "--settings",
            CLAUDE_SETTINGS,
            "--output-format",
            "stream-json",
            "--verbose",
        ],
        CodingApp::Codex => vec![
            "exec",
            "--json",
            "--sandbox",
            if may_edit {
                "workspace-write"
            } else {
                "read-only"
            },
            "-c",
            "features.hooks=false",
            "-",
        ],
    };
    words.into_iter().map(str::to_owned).collect()
}

/// The flags every template must carry (tested against [`arguments`]).
pub fn required_flags(app: CodingApp) -> &'static [&'static str] {
    match app {
        CodingApp::ClaudeCode => &[
            "-p",
            "--restricted",
            "--safe-mode",
            "--strict-mcp-config",
            "--permission-mode",
            "--tools",
            "--settings",
            "--output-format",
            "--verbose",
        ],
        CodingApp::Codex => &["exec", "--json", "--sandbox", "-c", "-"],
    }
}

/// Whether both templates of an app carry every required flag and no token that turns off
/// a permission check ([`policy::command::coding_apps::BYPASS_TOKENS`]). A tool whose
/// template fails this is not offered, and nothing runs with it.
pub fn template_ok(app: CodingApp) -> bool {
    [false, true].into_iter().all(|may_edit| {
        let words = arguments(app, may_edit);
        required_flags(app)
            .iter()
            .all(|flag| words.iter().any(|w| w == flag))
            && policy::command::coding_apps::bypass_token(&words.join(" ")).is_none()
    })
}

/// A version as `major.minor.patch`.
pub type Version = (u32, u32, u32);

/// The oldest version whose documentation lists each flag or setting of the template,
/// where the documentation says. A version without the setting would drop it from
/// `--settings` without a word, so it counts like a missing flag. Flags not listed here
/// carry no version in the documentation and are present in Claude Code 2.1.322; Codex's
/// documentation gives no versions.
pub const FLAG_SINCE: &[(CodingApp, &str, Version)] = &[
    (CodingApp::ClaudeCode, "--restricted", (2, 1, 248)),
    (
        CodingApp::ClaudeCode,
        "permissions.blockReadsOutsideWorkingDirectories",
        (2, 1, 257),
    ),
];

/// Reads `2.1.322`, `v2.1.322` or `2.1.322-beta.1`.
pub fn parse_version(text: &str) -> Option<Version> {
    let text = text.trim().trim_start_matches('v');
    let core = text.split(['-', '+', ' ']).next()?;
    let mut parts = core.split('.').map(|p| p.parse::<u32>().ok());
    let v = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// A flag or setting of the template that this version of the app does not have yet, if
/// its version is known and older than the documented first version of one: the newest
/// such requirement, so that updating to its version is enough for all of them.
pub fn missing_flag(app: CodingApp, version: Option<&str>) -> Option<(&'static str, Version)> {
    let version = parse_version(version?)?;
    FLAG_SINCE
        .iter()
        .filter(|(a, _, since)| *a == app && version < *since)
        .max_by_key(|(_, _, since)| *since)
        .map(|(_, flag, since)| (*flag, *since))
}

/// What the card says the app can do, from what the template confines and nothing more.
pub fn card_lines(app: CodingApp, may_edit: bool, folder: &str, windows: bool) -> Vec<String> {
    let mut lines = Vec::new();
    match (app, may_edit) {
        (CodingApp::ClaudeCode, false) => {
            lines.push(format!("It can only read files in {folder}."))
        }
        (CodingApp::ClaudeCode, true) => {
            lines.push(format!("It can read and change files only in {folder}."))
        }
        (CodingApp::Codex, false) => {
            lines.push("It can read files elsewhere on this computer that you can read.".into())
        }
        (CodingApp::Codex, true) => {
            lines.push("It can read files elsewhere on this computer that you can read.".into());
            lines.push(format!("It can change files in {folder}."));
            lines.push(
                "It can also change files in the temporary folder and run commands in its sandbox."
                    .into(),
            );
        }
    }
    if app == CodingApp::Codex && windows {
        lines.push("On Windows, Codex's sandbox has not been tried yet.".into());
    }
    lines
}

/// The line about where the task goes: online with the user's account, or, when the app is
/// set up to use the model shared by this app (WP5), to the local model.
pub fn online_line(app: CodingApp, uses_shared_model: bool) -> String {
    if uses_shared_model {
        format!(
            "{} is set up to use the model in harnessUI, so this task goes to your local model, not online.",
            app.label()
        )
    } else {
        format!(
            "It runs online with your {} account and uses your plan or credit. What it reads is sent to {}. It can take several minutes.",
            app.label(),
            app.company()
        )
    }
}

// ---- Finding the programs ----------------------------------------------------------------

/// Why a program cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Problem {
    NotFound,
    /// Its version is older than the first version with a flag the template needs.
    TooOld {
        version: String,
        flag: String,
        minimum: String,
    },
    /// It is inside a folder approved for the app's file tools.
    InsideApprovedFolder {
        folder: String,
    },
    /// It could not be looked at.
    Unreadable {
        reason: String,
    },
    /// It is not the file that was found when the group was turned on.
    Changed,
}

impl Problem {
    /// The problem in plain words.
    pub fn words(&self, app: CodingApp) -> String {
        let label = app.label();
        match self {
            Problem::NotFound => format!("{label} is not installed on this computer."),
            Problem::TooOld {
                version, minimum, ..
            } => format!(
                "{label} {version} is too old for this. Update {label} to {minimum} or later, then turn Coding apps off and on again."
            ),
            Problem::InsideApprovedFolder { folder } => format!(
                "{label} was found inside a folder you approved for this app ({folder}), so it is not used. A program there could have been put there by a project. Install {label} in its usual place."
            ),
            Problem::Unreadable { reason } => {
                format!("{label} was found but could not be checked ({reason}).")
            }
            Problem::Changed => format!(
                "{label} changed on this computer after Coding apps was turned on, perhaps by an update. Turn Coding apps off and on again in the Tools panel to use it."
            ),
        }
    }
}

/// One app as found when the group was turned on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedApp {
    pub app: CodingApp,
    /// The absolute path where it was found (on `PATH` or in `~/.local/bin`).
    pub path: Option<String>,
    /// The file that path leads to (links followed); this is what runs.
    pub program: Option<String>,
    pub identity: Option<FileIdentity>,
    /// Its version, when the files next to it say (npm's `package.json`, or a version as
    /// the file's name); never by running it.
    pub version: Option<String>,
    pub problem: Option<Problem>,
}

impl SavedApp {
    /// Found and usable as far as was known when it was found.
    pub fn usable(&self) -> bool {
        self.problem.is_none() && self.program.is_some() && self.identity.is_some()
    }
}

/// What [`SETTINGS_KEY`] holds.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSetup {
    /// When the programs were looked for (Unix ms); `None` before the first time.
    pub checked_at: Option<i64>,
    #[serde(default)]
    pub apps: Vec<SavedApp>,
}

impl SavedSetup {
    pub fn get(&self, app: CodingApp) -> Option<&SavedApp> {
        self.apps.iter().find(|a| a.app == app)
    }
}

/// Where an app is: on `path` (the `PATH` value to search; on Windows the real `.exe`
/// first, else the npm `.cmd` shim), else in `<home>/.local/bin`, where Claude Code's own
/// installer puts it (often missing from the `PATH` an app started from the Dock gets).
/// Only file names are looked at; nothing is run.
pub fn locate(
    app: CodingApp,
    path: Option<&OsStr>,
    home: Option<&Path>,
    windows: bool,
) -> Option<PathBuf> {
    let name = app.program();
    let search = path.filter(|p| !p.is_empty());
    let on_path = |n: &str| search.and_then(|p| runtime::resolve_program(n, Some(p)));
    let found = if windows {
        on_path(&format!("{name}.exe")).or_else(|| on_path(name))
    } else {
        on_path(name)
    };
    found.or_else(|| {
        let bin = home?.join(".local").join("bin");
        let candidates: Vec<PathBuf> = if windows {
            vec![bin.join(format!("{name}.exe"))]
        } else {
            vec![bin.join(name)]
        };
        candidates.into_iter().find_map(|c| {
            let text = c.to_str()?.to_owned();
            runtime::resolve_program(&text, None)
        })
    })
}

/// The version of the program at `program` (links followed), when the files say:
/// `package.json` of its npm package in a folder above it (or, for a Windows shim, in
/// `node_modules` next to it), or a version as its file name (Claude Code's own installer
/// keeps `versions/2.1.322`).
pub fn version_near(app: CodingApp, program: &Path) -> Option<String> {
    let read = |file: PathBuf| -> Option<String> {
        let text = std::fs::read_to_string(file).ok()?;
        let json: Value = serde_json::from_str(&text).ok()?;
        (json.get("name")?.as_str()? == app.package())
            .then(|| json.get("version")?.as_str().map(str::to_owned))
            .flatten()
            .filter(|v| parse_version(v).is_some())
    };
    for dir in program.ancestors().skip(1).take(6) {
        if let Some(v) = read(dir.join("package.json")) {
            return Some(v);
        }
    }
    if let Some(dir) = program.parent() {
        let mut shim = dir.join("node_modules");
        for part in app.package().split('/') {
            shim = shim.join(part);
        }
        if let Some(v) = read(shim.join("package.json")) {
            return Some(v);
        }
    }
    let name = program.file_name()?.to_str()?;
    let name = name.strip_suffix(".exe").unwrap_or(name);
    parse_version(name).map(|_| name.to_owned())
}

/// Whether `path` is inside one of `folders` (by the path sandbox's own rules), and which.
fn inside_any(path: &str, folders: &[String]) -> Option<String> {
    if folders.is_empty() {
        return None;
    }
    let (sandbox, _) =
        PathSandbox::new(Flavor::native(), folders, &[], &RealFs, &|p| volume_case(p));
    let checked = sandbox.check(path, None, &RealFs).ok()?;
    sandbox
        .roots()
        .get(checked.root)
        .map(|r| r.canonical.to_string())
}

/// Looks for one app and describes what was found. `approved` is every approved folder.
pub fn find_app(
    app: CodingApp,
    path: Option<&OsStr>,
    home: Option<&Path>,
    approved: &[String],
) -> SavedApp {
    let mut saved = SavedApp {
        app,
        path: None,
        program: None,
        identity: None,
        version: None,
        problem: None,
    };
    let Some(found) = locate(app, path, home, cfg!(windows)) else {
        saved.problem = Some(Problem::NotFound);
        return saved;
    };
    saved.path = Some(found.display().to_string());
    let program = match dunce::canonicalize(&found) {
        Ok(p) => p,
        Err(e) => {
            saved.problem = Some(Problem::Unreadable {
                reason: e.to_string(),
            });
            return saved;
        }
    };
    saved.program = Some(program.display().to_string());
    match FileIdentity::of(&program) {
        Ok(identity) => saved.identity = Some(identity),
        Err(e) => {
            saved.problem = Some(Problem::Unreadable {
                reason: e.to_string(),
            });
            return saved;
        }
    }
    saved.version = version_near(app, &program);
    for candidate in [&found, &program] {
        if let Some(folder) = inside_any(&candidate.display().to_string(), approved) {
            saved.problem = Some(Problem::InsideApprovedFolder { folder });
            return saved;
        }
    }
    if let Some((flag, since)) = missing_flag(app, saved.version.as_deref()) {
        saved.problem = Some(Problem::TooOld {
            version: saved.version.clone().unwrap_or_default(),
            flag: flag.into(),
            minimum: format!("{}.{}.{}", since.0, since.1, since.2),
        });
    }
    saved
}

/// The same file as when it was found: the path still leads to the saved program, and
/// the program's identity is unchanged.
pub fn unchanged(saved: &SavedApp) -> Result<PathBuf, Problem> {
    let (Some(path), Some(program), Some(identity)) =
        (&saved.path, &saved.program, &saved.identity)
    else {
        return Err(saved.problem.clone().unwrap_or(Problem::NotFound));
    };
    let now = dunce::canonicalize(path).map_err(|_| Problem::Changed)?;
    if now != Path::new(program) {
        return Err(Problem::Changed);
    }
    match FileIdentity::of(&now) {
        Ok(current) if current == *identity => Ok(now),
        _ => Err(Problem::Changed),
    }
}

// ---- Whether an app uses the shared model ---------------------------------------------

/// Whether a URL points at this computer on `port` (where the app shares its model).
fn points_at(url: &str, port: u16) -> bool {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return false;
    };
    let local = matches!(
        parsed.host_str(),
        Some("127.0.0.1" | "localhost" | "[::1]" | "::1")
    );
    local && parsed.port_or_known_default() == Some(port)
}

/// Whether the app would send this task to the model the app shares on `port` (WP5)
/// rather than online: Claude Code through `ANTHROPIC_BASE_URL` in its environment (with
/// `--restricted` it reads no settings file), Codex through `OPENAI_BASE_URL` or the
/// provider chosen in its `config.toml`.
pub fn uses_shared_model(
    app: CodingApp,
    env: &BTreeMap<String, String>,
    home: Option<&Path>,
    port: Option<u16>,
) -> bool {
    let Some(port) = port else {
        return false;
    };
    let var = |name: &str| {
        env.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    match app {
        CodingApp::ClaudeCode => var("ANTHROPIC_BASE_URL").is_some_and(|u| points_at(u, port)),
        CodingApp::Codex => {
            if var("OPENAI_BASE_URL").is_some_and(|u| points_at(u, port)) {
                return true;
            }
            let dir = match var("CODEX_HOME") {
                Some(dir) if !dir.is_empty() => PathBuf::from(dir),
                _ => match home {
                    Some(home) => home.join(".codex"),
                    None => return false,
                },
            };
            codex_config_uses(&dir.join("config.toml"), port)
        }
    }
}

/// Whether Codex's `config.toml` chooses a provider whose `base_url` points at `port`.
fn codex_config_uses(file: &Path, port: u16) -> bool {
    let Ok(meta) = std::fs::metadata(file) else {
        return false;
    };
    if meta.len() > 1024 * 1024 {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return false;
    };
    let Ok(config) = text.parse::<toml::Table>() else {
        return false;
    };
    let Some(provider) = config.get("model_provider").and_then(|v| v.as_str()) else {
        return false;
    };
    config
        .get("model_providers")
        .and_then(|p| p.get(provider))
        .and_then(|p| p.get("base_url"))
        .and_then(|u| u.as_str())
        .is_some_and(|u| points_at(u, port))
}

// ---- The server --------------------------------------------------------------------------

/// Reads the port the app shares its model on, if sharing was ever set up.
pub type SharePort =
    Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<u16>> + Send>> + Send + Sync>;

/// The folders the user approved, read from the store when a call is checked.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovedNow {
    /// Every approved folder, of every chat: a program inside any of them is refused (also
    /// one approved after the programs were found).
    pub all: Vec<String>,
    /// The folders approved for the chat that asks (global plus its own, as stored),
    /// without the whole computer that Full access adds to its tool context. `None` when
    /// they could not be read.
    pub chat: Option<Vec<String>>,
}

/// Reads [`ApprovedNow`] for a chat (by its id).
pub type Approved =
    Arc<dyn Fn(i64) -> Pin<Box<dyn Future<Output = ApprovedNow> + Send>> + Send + Sync>;

/// The "Coding apps" server. It lists the tools of the apps that were found usable when
/// the group was last turned on; it is registered again each time they are looked for.
pub struct CodingAppsServer {
    saved: SavedSetup,
    /// The environment the apps start from (the login environment on macOS), before
    /// secret-looking variables are dropped.
    base_env: BTreeMap<String, String>,
    home: Option<PathBuf>,
    share_port: SharePort,
    approved: Approved,
    windows: bool,
    max_run: Duration,
    tick: Duration,
}

impl CodingAppsServer {
    pub fn new(
        saved: SavedSetup,
        base_env: BTreeMap<String, String>,
        home: Option<PathBuf>,
        share_port: SharePort,
    ) -> Self {
        Self {
            saved,
            base_env,
            home,
            share_port,
            approved: Arc::new(|_| Box::pin(async { ApprovedNow::default() })),
            windows: cfg!(windows),
            max_run: MAX_RUN,
            tick: TICK,
        }
    }

    /// Where to read the approved folders from. By default only the chat's tool context is
    /// known, and a chat in Full access is refused (its context lists the whole computer).
    pub fn with_approved(mut self, approved: Approved) -> Self {
        self.approved = approved;
        self
    }

    /// A server that lists nothing yet: the group before its programs were looked for.
    pub fn unchecked() -> Self {
        Self::new(
            SavedSetup::default(),
            BTreeMap::new(),
            None,
            Arc::new(|| Box::pin(async { None })),
        )
    }

    /// The environment of a run: the base environment without secret-looking variables.
    fn child_env(&self) -> runtime::EnvSpec {
        let mut env = runtime::EnvSpec::empty();
        for (k, v) in &self.base_env {
            if !tools::terminal::looks_secret(k) {
                env = env.set(k.clone(), v.clone());
            }
        }
        env
    }

    /// The working folder's sandbox: the folders the user approved for the chat (never the
    /// whole computer of Full access), without the app's own folders.
    fn sandbox(own: &[String], chat: &ChatContext) -> PathSandbox {
        PathSandbox::new(
            Flavor::native(),
            own,
            &chat.forbidden_folders,
            &RealFs,
            &|p| volume_case(p),
        )
        .0
    }

    /// The folders the user approved for the chat, and every approved folder. Outside Full
    /// access the chat's tool context holds the chat's folders as stored; in Full access it
    /// also lists the whole computer (`approvals::widen`), so they are read from the store.
    async fn folders(
        &self,
        app: CodingApp,
        chat: &ChatContext,
    ) -> Result<(Vec<String>, ApprovedNow), String> {
        let now = (self.approved)(chat.chat_id).await;
        let own = if chat.full_access {
            now.chat.clone().ok_or_else(|| {
                format!(
                    "The folders approved for this chat could not be read, so {} was not asked.",
                    app.label()
                )
            })?
        } else {
            chat.approved_folders.clone()
        };
        Ok((own, now))
    }
}

/// A server that lists both tools, as if both programs were found (for the tool catalog's
/// check of every registered tool). Its programs do not exist, so nothing could run.
#[cfg(test)]
pub(crate) fn listing_both() -> CodingAppsServer {
    let apps = CodingApp::ALL
        .into_iter()
        .map(|app| SavedApp {
            app,
            path: Some(format!("/nonexistent/{}", app.program())),
            program: Some(format!("/nonexistent/{}", app.program())),
            identity: Some(FileIdentity {
                device: 0,
                file: 0,
                size: 0,
                modified_ns: 0,
            }),
            version: None,
            problem: None,
        })
        .collect();
    CodingAppsServer::new(
        SavedSetup {
            checked_at: Some(0),
            apps,
        },
        BTreeMap::new(),
        None,
        Arc::new(|| Box::pin(async { None })),
    )
}

fn tool_def(app: CodingApp) -> ToolDef {
    let label = app.label();
    ToolDef {
        name: app.tool().into(),
        description: format!(
            "Ask {label}, a coding assistant installed on this computer, to do a task in the chat's working folder. It works online with the user's own account, and the user approves every request. It sees nothing of this chat: put everything it needs into task. Set may_edit to let it change files there. Its answer comes back as the result."
        ),
        input_schema: json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "minLength": 1,
                    "description": format!("The whole task for {label}.")
                },
                "may_edit": {
                    "type": "boolean",
                    "description": "Let it change files in the working folder (default: read only)."
                }
            },
            "required": ["task"],
            "additionalProperties": false
        }),
        destructive: Some(true),
        read_only: Some(false),
    }
}

/// What a described call carries to its run.
struct Planned {
    app: CodingApp,
    may_edit: bool,
    task: String,
    cwd: policy::CheckedPath,
}

impl CodingAppsServer {
    /// A refusal for the policy engine (step 2: no card).
    fn refused(app: CodingApp, task: &str, why: String) -> Described {
        Described::Action(policy::Action {
            server: SERVER_NAME.into(),
            tool: app.tool().into(),
            summary: format!(
                "Ask {}: {}",
                app.label(),
                policy::sanitize::one_line(task, 120)
            ),
            preview: why.clone(),
            scope: policy::Scope::CodingApp {
                app: app.key().into(),
                folder: String::new(),
            },
            risk: policy::Risk::AlwaysAsk,
            read_only: false,
            outbound: true,
            keep_asking: Some(policy::KeepAsking::Publish),
            network: true,
            hard_limit: Some(why),
            payload: Value::Null,
        })
    }

    /// The program to run, if it is still the one found and is not inside an approved
    /// folder (this chat's, its skill folders, or any other chat's).
    async fn program(
        &self,
        app: CodingApp,
        chat: &ChatContext,
        own: &[String],
        approved: &ApprovedNow,
    ) -> Result<PathBuf, String> {
        if !template_ok(app) {
            return Err(format!(
                "{} cannot be asked: the app's command line for it is missing a setting it needs.",
                app.label()
            ));
        }
        let Some(saved) = self.saved.get(app).filter(|s| s.usable()) else {
            let problem = self
                .saved
                .get(app)
                .and_then(|s| s.problem.clone())
                .unwrap_or(Problem::NotFound);
            return Err(problem.words(app));
        };
        let program = unchanged(saved).map_err(|p| p.words(app))?;
        let mut folders = own.to_vec();
        folders.extend(chat.skill_folders.iter().cloned());
        folders.extend(approved.all.iter().cloned());
        for candidate in [
            saved.path.as_deref().unwrap_or_default(),
            &program.display().to_string(),
        ] {
            if let Some(folder) = inside_any(candidate, &folders) {
                return Err(Problem::InsideApprovedFolder {
                    folder: policy::sanitize::visible(&folder),
                }
                .words(app));
            }
        }
        Ok(program)
    }

    /// The chat's working folder, checked: approved by the user (`own`) and a folder.
    fn working_folder(
        app: CodingApp,
        chat: &ChatContext,
        own: &[String],
    ) -> Result<policy::CheckedPath, String> {
        let sandbox = Self::sandbox(own, chat);
        let base = if own.is_empty() {
            None
        } else {
            chat.working_folder
                .clone()
                .or_else(|| sandbox.roots().first().map(|r| r.canonical.to_string()))
        };
        let Some(base) = base else {
            return Err(format!(
                "No folder is approved for this chat, so {} has nowhere to work. The user can approve a folder in the Tools panel.",
                app.label()
            ));
        };
        match sandbox.check(&base, None, &RealFs) {
            Ok(c) if Path::new(&c.canonical).is_dir() => Ok(c),
            Ok(c) => Err(format!(
                "The chat's working folder {} is not a folder that exists. The user can choose another one in the Tools panel.",
                policy::sanitize::visible(&c.canonical)
            )),
            Err(e) => Err(format!(
                "The chat's working folder is not approved ({e}). The user can approve it in the Tools panel."
            )),
        }
    }

    fn plan(
        &self,
        tool: &str,
        args: &Map<String, Value>,
    ) -> Result<(CodingApp, bool, String), String> {
        let app = CodingApp::from_tool(tool)
            .ok_or_else(|| format!("the Coding apps group has no tool named {tool}"))?;
        let task = args.get("task").and_then(Value::as_str).unwrap_or("");
        if task.trim().is_empty() {
            return Err(format!(
                "task is empty: write the whole task for {}",
                app.label()
            ));
        }
        let chars = task.chars().count();
        if chars > MAX_TASK_CHARS {
            return Err(format!(
                "task is too long ({chars} characters): keep it under {MAX_TASK_CHARS}"
            ));
        }
        let may_edit = args
            .get("may_edit")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        Ok((app, may_edit, task.to_owned()))
    }

    fn planned(action: &policy::Action) -> Option<Planned> {
        let p = &action.payload;
        Some(Planned {
            app: CodingApp::from_tool(&action.tool)?,
            may_edit: p.get("mayEdit")?.as_bool()?,
            task: p.get("task")?.as_str()?.to_owned(),
            cwd: policy::CheckedPath {
                canonical: p.get("cwd")?.as_str()?.to_owned(),
                root: p.get("root")?.as_u64()? as usize,
                exists: true,
                was_symlink: p.get("wasSymlink")?.as_bool()?,
            },
        })
    }
}

#[async_trait]
impl BuiltinServer for CodingAppsServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        CodingApp::ALL
            .into_iter()
            .filter(|app| template_ok(*app) && self.saved.get(*app).is_some_and(SavedApp::usable))
            .map(tool_def)
            .collect()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let (app, may_edit, task) = match self.plan(tool, args) {
            Ok(planned) => planned,
            Err(why) => return Described::Invalid(why),
        };
        let (own, approved) = match self.folders(app, chat).await {
            Ok(f) => f,
            Err(why) => return Self::refused(app, &task, why),
        };
        let program = match self.program(app, chat, &own, &approved).await {
            Ok(p) => p,
            Err(why) => return Self::refused(app, &task, why),
        };
        let cwd = match Self::working_folder(app, chat, &own) {
            Ok(c) => c,
            Err(why) => return Self::refused(app, &task, why),
        };
        let shared = uses_shared_model(
            app,
            &self.base_env,
            self.home.as_deref(),
            (self.share_port)().await,
        );
        let folder = policy::sanitize::visible(&cwd.canonical);
        let mut preview = card_lines(app, may_edit, &folder, self.windows);
        preview.push(online_line(app, shared));
        preview.push("Experimental.".into());
        preview.push(format!(
            "Program: {}",
            policy::sanitize::visible(&program.display().to_string())
        ));
        let preview = format!(
            "{}\n\nTask:\n{}",
            preview.join("\n"),
            policy::sanitize::visible(&task)
        );
        Described::Action(policy::Action {
            server: SERVER_NAME.into(),
            tool: app.tool().into(),
            summary: format!(
                "Ask {}: {}",
                app.label(),
                policy::sanitize::one_line(&task, 120)
            ),
            preview,
            scope: policy::Scope::CodingApp {
                app: app.key().into(),
                folder: cwd.canonical.clone(),
            },
            risk: policy::Risk::AlwaysAsk,
            read_only: false,
            outbound: true,
            // It sends in the user's name and spends their plan or credit.
            keep_asking: Some(policy::KeepAsking::Publish),
            network: true,
            hard_limit: None,
            payload: json!({
                "app": app.key(),
                "mayEdit": may_edit,
                "task": task,
                "cwd": cwd.canonical,
                "root": cwd.root,
                "wasSymlink": cwd.was_symlink,
                "program": program.display().to_string(),
            }),
        })
    }

    async fn call(
        &self,
        tool: &str,
        _args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if ctx.cancel.is_cancelled() {
            return ToolOutput::error("cancelled");
        }
        let Some(planned) = Self::planned(action).filter(|p| p.app.tool() == tool) else {
            return ToolOutput::error(
                "internal error: the approved action has no task; nothing was run",
            );
        };
        let app = planned.app;
        // Everything is checked again immediately before the run (13.4 step 5).
        let (own, approved) = match self.folders(app, &ctx.chat).await {
            Ok(f) => f,
            Err(why) => return ToolOutput::error(format!("{why} Nothing was run.")),
        };
        let program = match self.program(app, &ctx.chat, &own, &approved).await {
            Ok(p) => p,
            Err(why) => return ToolOutput::error(format!("{why} Nothing was run.")),
        };
        if action.payload.get("program").and_then(Value::as_str)
            != Some(program.display().to_string().as_str())
        {
            return ToolOutput::error(format!("{} Nothing was run.", Problem::Changed.words(app)));
        }
        if let Err(e) = Self::sandbox(&own, &ctx.chat).recheck(&planned.cwd, &RealFs) {
            return ToolOutput::error(format!(
                "The working folder changed after the request was approved ({e}), so nothing was run."
            ));
        }
        let spec = runtime::SpawnSpec::new(&program, &planned.cwd.canonical)
            .args(arguments(app, planned.may_edit))
            .env(self.child_env())
            .stdin(runtime::StdioMode::Piped)
            .stdout(runtime::StdioMode::Piped)
            .stderr(runtime::StdioMode::Piped);
        let budget = tools::shorten::budget_bytes(ctx.chat.result_cap_tokens);
        run(
            app,
            spec,
            planned.task,
            ctx,
            self.max_run,
            self.tick,
            budget,
        )
        .await
    }
}

/// How a run ended.
enum Ended {
    Exited(std::io::Result<std::process::ExitStatus>),
    Stopped,
    TimedOut,
    SignedOut,
}

/// Starts the program, hands it the task on standard input and turns what it prints into
/// the result.
async fn run(
    app: CodingApp,
    spec: runtime::SpawnSpec,
    task: String,
    ctx: CallContext,
    max_run: Duration,
    tick: Duration,
    budget: usize,
) -> ToolOutput {
    let label = app.label();
    let program = spec.program.display().to_string();
    let mut child = match runtime::spawn(spec) {
        Ok(c) => c,
        Err(e) => {
            return ToolOutput::error(format!(
                "{label} could not be started ({e}). Nothing was run. The user can turn Coding apps off and on again in the Tools panel."
            ));
        }
    };
    // The task, then the end of input.
    if let Some(mut stdin) = child.take_stdin() {
        tokio::spawn(async move {
            let _ = stdin.write_all(task.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let parsed = Arc::new(Mutex::new(output::Parsed::new(app)));
    let signed_out = Arc::new(tokio::sync::Notify::new());
    let progress = ctx.progress.clone();
    let mut readers = Vec::new();
    if let Some(mut stdout) = child.take_stdout() {
        let parsed = parsed.clone();
        let signed_out = signed_out.clone();
        let progress = progress.clone();
        readers.push(tokio::spawn(async move {
            let mut lines = output::Lines::default();
            let mut buf = vec![0u8; 16 * 1024];
            loop {
                match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        for line in lines.push(&buf[..n]) {
                            let said = parsed.lock().map(|mut p| p.line(&line)).unwrap_or_default();
                            if let (Some(p), Some(text)) = (&progress, said.progress) {
                                p(text);
                            }
                            if said.signed_out {
                                signed_out.notify_one();
                            }
                        }
                    }
                }
            }
            if let Some(line) = lines.finish()
                && let Ok(mut p) = parsed.lock()
            {
                p.line(&line);
            }
        }));
    }
    let stderr_tail = Arc::new(Mutex::new(Vec::<u8>::new()));
    if let Some(mut stderr) = child.take_stderr() {
        let tail = stderr_tail.clone();
        readers.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 8 * 1024];
            loop {
                match stderr.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut t) = tail.lock() {
                            t.extend_from_slice(&buf[..n]);
                            if t.len() > STDERR_TAIL {
                                let drop = t.len() - STDERR_TAIL;
                                t.drain(..drop);
                            }
                        }
                    }
                }
            }
        }));
    }
    let started = Instant::now();
    let ticker = progress.clone().map(|p| {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tick);
            loop {
                interval.tick().await;
                let minutes = started.elapsed().as_secs() / 60;
                p(format!("{label} is working… {minutes} min · Stop\n"));
            }
        })
    });
    let ended = tokio::select! {
        status = child.wait() => Ended::Exited(status),
        _ = ctx.cancel.cancelled() => Ended::Stopped,
        _ = tokio::time::sleep(max_run) => Ended::TimedOut,
        _ = signed_out.notified() => Ended::SignedOut,
    };
    if !matches!(ended, Ended::Exited(_)) {
        // The whole process group (Unix) or job (Windows).
        let _ = child.kill().await;
    }
    if let Some(t) = ticker {
        t.abort();
    }
    for r in readers {
        let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
    }
    let stderr = stderr_tail
        .lock()
        .map(|t| String::from_utf8_lossy(&t).into_owned())
        .unwrap_or_default();
    let parsed = parsed
        .lock()
        .map(|p| p.clone())
        .unwrap_or_else(|_| output::Parsed::new(app));
    let minutes = started.elapsed().as_secs_f64() / 60.0;
    tracing::info!(app = label, program = %program, minutes, "a coding app finished");
    output::result(app, &ended_kind(&ended), &parsed, &stderr, budget)
}

fn ended_kind(ended: &Ended) -> output::End {
    match ended {
        Ended::Exited(Ok(status)) => output::End::Exited(status.code()),
        Ended::Exited(Err(e)) => output::End::WaitFailed(e.to_string()),
        Ended::Stopped => output::End::Stopped,
        Ended::TimedOut => output::End::TimedOut,
        Ended::SignedOut => output::End::SignedOut,
    }
}

// ---- Commands (main window only) -----------------------------------------------------

/// One app as the Tools panel shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAppStatus {
    pub app: CodingApp,
    pub label: String,
    pub tool: String,
    /// The absolute path of the program that runs.
    pub path: Option<String>,
    pub version: Option<String>,
    pub usable: bool,
    /// Why it cannot be used, in plain words.
    pub problem: Option<String>,
}

/// What the Tools panel shows for the group.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodingAppsStatus {
    /// The programs were looked for at least once.
    pub checked: bool,
    pub apps: Vec<CodingAppStatus>,
    /// [`NOT_INSTALLED`] when neither was found.
    pub message: Option<String>,
}

/// The status of a saved setup, with each program checked again (looked at, not run).
pub fn status_of(saved: &SavedSetup) -> CodingAppsStatus {
    let apps: Vec<CodingAppStatus> = CodingApp::ALL
        .into_iter()
        .map(|app| {
            let entry = saved.get(app);
            let problem = match entry {
                None => saved.checked_at.map(|_| Problem::NotFound),
                Some(e) if e.usable() => unchanged(e).err(),
                Some(e) => Some(e.problem.clone().unwrap_or(Problem::NotFound)),
            };
            CodingAppStatus {
                app,
                label: app.label().into(),
                tool: app.tool().into(),
                path: entry.and_then(|e| e.program.clone()),
                version: entry.and_then(|e| e.version.clone()),
                usable: entry.is_some() && problem.is_none(),
                problem: problem.map(|p| p.words(app)),
            }
        })
        .collect();
    let none_found = saved.checked_at.is_some()
        && CodingApp::ALL.into_iter().all(|app| {
            saved
                .get(app)
                .is_none_or(|e| matches!(e.problem, Some(Problem::NotFound)))
        });
    CodingAppsStatus {
        checked: saved.checked_at.is_some(),
        apps,
        message: none_found.then(|| NOT_INSTALLED.to_owned()),
    }
}

/// The environment the apps start from: the login environment captured on macOS, the
/// app's own elsewhere.
fn base_env() -> BTreeMap<String, String> {
    crate::toolset::captured_env_now().unwrap_or_else(|| std::env::vars().collect())
}

fn home_of(env: &BTreeMap<String, String>) -> Option<PathBuf> {
    let var = |name: &str| {
        env.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// The server for a saved setup, reading the sharing port from the app's settings.
fn server(app: &AppHandle, saved: SavedSetup) -> CodingAppsServer {
    let env = base_env();
    let home = home_of(&env);
    let handle = app.clone();
    let share_port: SharePort = Arc::new(move || {
        let handle = handle.clone();
        Box::pin(async move {
            let state = handle.try_state::<AppState>()?;
            state
                .store
                .get_setting::<crate::share::ShareSettings>(crate::share::SETTINGS_KEY)
                .await
                .ok()
                .flatten()
                .map(|s| s.port)
        })
    });
    let handle = app.clone();
    let approved: Approved = Arc::new(move |chat_id| {
        let handle = handle.clone();
        Box::pin(async move {
            let Some(state) = handle.try_state::<AppState>() else {
                return ApprovedNow::default();
            };
            let paths = |rows: Vec<store::ApprovedFolder>| -> Vec<String> {
                rows.into_iter().map(|f| f.path).collect()
            };
            ApprovedNow {
                all: state
                    .store
                    .list_approved_folders()
                    .await
                    .map(paths)
                    .unwrap_or_default(),
                chat: state
                    .store
                    .approved_folders_for_chat(store::ChatId(chat_id))
                    .await
                    .map(paths)
                    .ok(),
            }
        })
    });
    CodingAppsServer::new(saved, env, home, share_port).with_approved(approved)
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

async fn saved_setup(state: &AppState) -> SavedSetup {
    state
        .store
        .get_setting::<SavedSetup>(SETTINGS_KEY)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// At start: registers the server with the programs found when the group was last turned
/// on, so a chat that has the group on can use them.
pub async fn register_saved(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let saved = saved_setup(&state).await;
    if saved.checked_at.is_some() {
        state.host.add_builtin(Arc::new(server(app, saved)));
    }
}

/// What the Tools panel shows for the group. Nothing is looked for and nothing is run.
#[tauri::command]
pub async fn coding_apps_status(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<CodingAppsStatus> {
    crate::commands::only_main_window(window.label())?;
    Ok(status_of(&saved_setup(&state).await))
}

/// Looks for Claude Code and Codex (the user turned the group on): locates the files,
/// never runs them, saves what was found and offers the tools of the usable ones.
#[tauri::command]
pub async fn find_coding_apps(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<CodingAppsStatus> {
    crate::commands::only_main_window(window.label())?;
    let env = base_env();
    let home = home_of(&env);
    let path = env
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
        .map(|(_, v)| v.clone());
    let approved: Vec<String> = state
        .store
        .list_approved_folders()
        .await?
        .into_iter()
        .map(|f| f.path)
        .collect();
    let found = tokio::task::spawn_blocking(move || {
        CodingApp::ALL
            .into_iter()
            .map(|a| {
                find_app(
                    a,
                    path.as_deref().map(OsStr::new),
                    home.as_deref(),
                    &approved,
                )
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(AppError::internal)?;
    let saved = SavedSetup {
        checked_at: Some(now_ms()),
        apps: found,
    };
    state.store.set_setting(SETTINGS_KEY, &saved).await?;
    state
        .host
        .add_builtin(Arc::new(server(&app, saved.clone())));
    Ok(status_of(&saved))
}
