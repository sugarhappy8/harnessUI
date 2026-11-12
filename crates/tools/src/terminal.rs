//! The built-in `terminal` server (ARCHITECTURE.md 9.3, decision D5): `run_command`.
//!
//! - One command per call, in a working folder inside an approved folder or an enabled
//!   skill's folder.
//! - Windows: `powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass
//!   -EncodedCommand …` (Windows PowerShell 5.1, the only shell every machine has). The
//!   command is passed base64-encoded, so no quoting can change what runs; the wrapper
//!   forces UTF-8 output and returns the real exit code.
//! - macOS: `/bin/zsh -lc …` with the environment captured from a login shell.
//! - The environment drops variables whose names look like secrets.
//! - Output streams into the tool card and is capped at 1 MiB. A result always starts
//!   with the line `Exit code N (… s)` (or what ended the command instead). An output too
//!   long for the per-result cap is shown as its first lines, the lines in between that
//!   look like errors with their line numbers, and its last lines (`shorten`); the whole
//!   text goes to the app, which stores it for `read_output`. Default timeout 120 s. Stop
//!   kills the tree.
//! - A failure says what to do next (app 0.0.4): a program that is not there is named
//!   with close names on PATH, a timeout says how to allow more time, a working folder
//!   that is not allowed names the allowed ones.
//!
//! The classifier in `policy::command` decides what a grant can cover; it fails closed.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::command::{Parsed, Shell, classify, is_read_only, program_name};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{Action, Risk, Scope};
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;

use crate::{errors, shorten};

/// Output kept at most (8.4 raw limit).
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Default and maximum timeouts.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// Bytes of the end of the error stream kept apart, to show when later output buries it.
const STDERR_TAIL_BYTES: usize = 2048;
/// Lines of the end of the error stream shown at most.
const STDERR_TAIL_LINES: usize = 8;
/// Folders on PATH and names in them looked at when a program was not found.
const MAX_PATH_FOLDERS: usize = 200;
const MAX_PATH_NAMES: usize = 20_000;

/// Which shell runs commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalShell {
    /// Windows PowerShell 5.1 (or PowerShell 7 when `program` points at pwsh.exe).
    PowerShell { program: PathBuf },
    /// `/bin/zsh -lc` on macOS.
    Zsh,
    /// `/bin/sh -c`, for development on Linux.
    Sh,
}

impl TerminalShell {
    /// The platform default.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            let root = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
            TerminalShell::PowerShell {
                program: root.join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
            }
        } else if cfg!(target_os = "macos") {
            TerminalShell::Zsh
        } else {
            TerminalShell::Sh
        }
    }
    fn grammar(&self) -> Shell {
        match self {
            TerminalShell::PowerShell { .. } => Shell::PowerShell,
            _ => Shell::Posix,
        }
    }
}

/// Configuration the app supplies.
#[derive(Debug, Clone)]
pub struct TerminalConfig {
    pub shell: TerminalShell,
    /// The environment commands start from: the login-shell environment captured at
    /// startup on macOS, the process environment elsewhere.
    pub base_env: BTreeMap<String, String>,
    /// Variable names kept even though they look like secrets (user setting).
    pub pass_through: Vec<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            shell: TerminalShell::platform_default(),
            base_env: std::env::vars().collect(),
            pass_through: Vec::new(),
        }
    }
}

/// Whether a variable name looks like it holds a secret.
pub fn looks_secret(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    n.ends_with("_TOKEN")
        || n.ends_with("_KEY")
        || n.ends_with("_SECRET")
        || n.contains("PASSWORD")
        || n.ends_with("_PAT")
        || n == "TOKEN"
        || n.ends_with("_CREDENTIALS")
        || n.starts_with("AWS_SECRET")
}

/// The `terminal` server.
#[derive(Debug, Clone)]
pub struct TerminalServer {
    config: TerminalConfig,
    flavor: Flavor,
}

impl TerminalServer {
    pub fn new(config: TerminalConfig) -> Self {
        Self {
            config,
            flavor: Flavor::native(),
        }
    }

    fn env(&self) -> runtime::EnvSpec {
        let mut env = runtime::EnvSpec::empty();
        for (k, v) in &self.config.base_env {
            if !looks_secret(k)
                || self
                    .config
                    .pass_through
                    .iter()
                    .any(|p| p.eq_ignore_ascii_case(k))
            {
                env = env.set(k.clone(), v.clone());
            }
        }
        env
    }

    fn path_var(&self) -> Option<String> {
        self.config
            .base_env
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PATH"))
            .map(|(_, v)| v.clone())
    }

    /// The program as an absolute path where it resolves (cmdlets and shell built-ins
    /// stay as names, marked so they never match a path grant by accident).
    fn resolve(&self, program: &str) -> String {
        let path = self.path_var();
        runtime::resolve_program(program, path.as_deref().map(std::ffi::OsStr::new))
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| format!("builtin:{}", program_name(program)))
    }

    fn sandbox(&self, chat: &ChatContext) -> PathSandbox {
        let mut folders = chat.approved_folders.clone();
        folders.extend(chat.skill_folders.iter().cloned());
        PathSandbox::new(
            self.flavor,
            &folders,
            &chat.forbidden_folders,
            &RealFs,
            &|p| volume_case(p),
        )
        .0
    }

    fn command_line(&self, command: &str) -> (PathBuf, Vec<String>) {
        match &self.config.shell {
            TerminalShell::PowerShell { program } => {
                let script = format!(
                    "$ErrorActionPreference='Continue'; [Console]::OutputEncoding=[System.Text.Encoding]::UTF8; $OutputEncoding=[System.Text.Encoding]::UTF8\n{command}\n$__ok=$?; $__code=$LASTEXITCODE; if (-not $__ok) {{ if ($__code) {{ exit $__code }} else {{ exit 1 }} }} else {{ exit 0 }}"
                );
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
                (
                    program.clone(),
                    vec![
                        "-NoLogo".into(),
                        "-NoProfile".into(),
                        "-NonInteractive".into(),
                        "-ExecutionPolicy".into(),
                        "Bypass".into(),
                        "-EncodedCommand".into(),
                        encoded,
                    ],
                )
            }
            TerminalShell::Zsh => (
                PathBuf::from("/bin/zsh"),
                vec!["-lc".into(), command.into()],
            ),
            TerminalShell::Sh => (PathBuf::from("/bin/sh"), vec!["-c".into(), command.into()]),
        }
    }
}

/// What a timed-out result says after the output.
fn timeout_note(seconds: u64) -> String {
    const NO_INPUT: &str =
        "A command that waits for input never finishes here: commands get no input.";
    if seconds >= MAX_TIMEOUT.as_secs() {
        format!(
            "[It was ended after {seconds} s, the longest a command may run. Run it in smaller steps. {NO_INPUT}]"
        )
    } else {
        format!(
            "[It was ended after the {seconds} s it was given. If it only needs more time, pass a larger timeout_seconds (at most {}). {NO_INPUT}]",
            MAX_TIMEOUT.as_secs()
        )
    }
}

/// Whether a failed command's output says that a program was not found: exit code 127
/// from a POSIX shell, PowerShell's own error otherwise.
fn says_not_found(code: Option<i32>, output: &str) -> bool {
    code == Some(127)
        || output.contains("CommandNotFoundException")
        || output.contains("is not recognized as")
}

/// The last lines of a command's error stream, from the bytes kept of its end.
fn last_lines(tail: &[u8], cut_at_start: bool) -> String {
    let text = String::from_utf8_lossy(tail);
    let mut lines: Vec<&str> = text.lines().collect();
    // The kept bytes may start in the middle of a line.
    if cut_at_start && lines.len() > 1 {
        lines.remove(0);
    }
    let from = lines.len().saturating_sub(STDERR_TAIL_LINES);
    lines[from..].join("\n")
}

fn arg_looks_like_path(a: &str) -> bool {
    !a.starts_with('-')
        && (a.contains('/') || a.contains('\\') || a.starts_with('.') || a.starts_with('~'))
}

#[async_trait]
impl BuiltinServer for TerminalServer {
    fn name(&self) -> &str {
        "terminal"
    }

    fn tools(&self) -> Vec<ToolDef> {
        let shell_note = match self.config.shell {
            TerminalShell::PowerShell { .. } => {
                "Runs in Windows PowerShell 5.1: use PowerShell syntax; `&&` and `||` are not available, use `;` or separate calls."
            }
            TerminalShell::Zsh => "Runs in zsh on macOS.",
            TerminalShell::Sh => "Runs in /bin/sh.",
        };
        vec![ToolDef {
            name: "run_command".into(),
            description: format!(
                "Run one shell command in an approved folder and return its output and exit code. {shell_note} Nothing interactive: commands that wait for input will time out."
            ),
            input_schema: json!({"type":"object","properties":{
                "command": {"type":"string","minLength":1,"description":"The command line"},
                "cwd": {"type":"string","description":"Working folder (default: the chat's working folder)"},
                "timeout_seconds": {"type":"integer","minimum":1,"maximum":600}
            },"required":["command"],"additionalProperties":false}),
            destructive: None,
            read_only: Some(false),
        }]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        if tool != "run_command" {
            return Described::Invalid(format!("the terminal has no tool named {tool}"));
        }
        let Some(command) = args
            .get("command")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Described::Invalid("command is required".into());
        };
        let sb = self.sandbox(chat);
        let base = chat
            .working_folder
            .clone()
            .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()));
        let cwd_arg = args
            .get("cwd")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(base.clone());
        let shown_cmd = policy::sanitize::visible(&command);
        let deny = |why: String| {
            Described::Action(Action {
                server: "terminal".into(),
                tool: tool.into(),
                summary: policy::sanitize::one_line(&command, 200),
                preview: shown_cmd.clone(),
                scope: Scope::Command {
                    program: String::new(),
                    args: vec![],
                    cwd: String::new(),
                },
                risk: Risk::AlwaysAsk,
                read_only: false,
                outbound: true,
                keep_asking: None,
                network: true,
                hard_limit: Some(why),
                payload: Value::Null,
            })
        };
        let Some(cwd_arg) = cwd_arg else {
            return deny("No folder is approved for this chat, so commands have nowhere to run. The user can approve a folder in the Tools panel.".into());
        };
        let folders = || -> String {
            let folders: Vec<String> = sb.roots().iter().map(|r| r.canonical.to_string()).collect();
            folders.join(", ")
        };
        let cwd = match sb.check(&cwd_arg, base.as_deref(), &RealFs) {
            Ok(c) if std::path::Path::new(&c.canonical).is_dir() => c,
            Ok(c) => {
                let shown = policy::sanitize::visible(&c.canonical);
                let instead = match &base {
                    Some(base) => format!(
                        "leave cwd out to run in {}",
                        policy::sanitize::visible(base)
                    ),
                    None => "leave cwd out".to_string(),
                };
                return Described::Invalid(if c.exists {
                    format!(
                        "cwd {shown} is a file, not a folder. Pass a folder as cwd, or {instead}."
                    )
                } else {
                    format!(
                        "cwd {shown} does not exist. Pass a folder that exists as cwd, or {instead}."
                    )
                });
            }
            Err(e) => {
                return deny(format!(
                    "{e}. Commands may run in: {}. Pass one of these folders, or a folder inside one, as cwd; the user can approve another folder in the Tools panel.",
                    folders()
                ));
            }
        };
        let verdict = classify(self.config.shell.grammar(), &command);
        let mut preview = format!(
            "$ {shown_cmd}\nin {}",
            policy::sanitize::visible(&cwd.canonical)
        );
        let scope = match &verdict.parsed {
            Parsed::Plain(cmds) => {
                let mut parts = Vec::new();
                for c in cmds {
                    let program = self.resolve(&c.program);
                    preview.push_str(&format!(
                        "\nprogram: {}",
                        policy::sanitize::visible(&program)
                    ));
                    if is_read_only(c) {
                        // Read-only programs are granted per program; their path arguments
                        // are held to approved folders.
                        for a in c.args.iter().filter(|a| arg_looks_like_path(a)) {
                            if let Err(e) = sb.check(a, Some(&cwd.canonical), &RealFs) {
                                return deny(format!(
                                    "{} reads {a}: {e}. Commands may read files in: {}. The user can approve another folder in the Tools panel.",
                                    c.program,
                                    folders()
                                ));
                            }
                        }
                        parts.push(Scope::ReadOnlyProgram { program });
                    } else {
                        parts.push(Scope::Command {
                            program,
                            args: c.args.clone(),
                            cwd: cwd.canonical.clone(),
                        });
                    }
                }
                if parts.len() == 1 {
                    parts.remove(0)
                } else {
                    Scope::Commands { parts }
                }
            }
            Parsed::Complex { .. } => Scope::Command {
                program: "complex".into(),
                args: vec![command.clone()],
                cwd: cwd.canonical.clone(),
            },
        };
        if let Some(why) = &verdict.always_ask {
            preview = format!("Always asks: {why}.\n\n{preview}");
        }
        let timeout = args
            .get("timeout_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_TIMEOUT.as_secs())
            .min(MAX_TIMEOUT.as_secs());
        Described::Action(Action {
            server: "terminal".into(),
            tool: tool.into(),
            summary: format!("Run: {}", policy::sanitize::one_line(&command, 160)),
            preview,
            scope,
            risk: if verdict.always_ask.is_some() {
                Risk::AlwaysAsk
            } else {
                Risk::Normal
            },
            read_only: verdict.read_only,
            outbound: true,
            // Allow all: the classifier says whether the command still asks (deletes,
            // destroys, publishes, hides what it runs) and whether it can reach the network.
            keep_asking: verdict.keep_asking.as_ref().map(|(kind, _)| *kind),
            network: verdict.network,
            hard_limit: None,
            payload: json!({
                "cwd": cwd.canonical,
                "root": cwd.root,
                "exists": cwd.exists,
                "wasSymlink": cwd.was_symlink,
                "command": command,
                "timeoutSeconds": timeout,
            }),
        })
    }

    async fn call(
        &self,
        tool: &str,
        _args: &Map<String, Value>,
        action: &Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if tool != "run_command" {
            return ToolOutput::error(format!("the terminal has no tool named {tool}"));
        }
        if ctx.cancel.is_cancelled() {
            return ToolOutput::error("cancelled");
        }
        let p = &action.payload;
        let (Some(command), Some(cwd)) = (
            p.get("command").and_then(Value::as_str),
            p.get("cwd").and_then(Value::as_str),
        ) else {
            return ToolOutput::error("internal error: the approved action has no command");
        };
        // Re-check the working folder immediately before running (13.4 step 5).
        let checked = policy::CheckedPath {
            canonical: cwd.to_string(),
            root: p.get("root").and_then(Value::as_u64).unwrap_or(0) as usize,
            exists: true,
            was_symlink: p
                .get("wasSymlink")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        if let Err(e) = self.sandbox(&ctx.chat).recheck(&checked, &RealFs) {
            return ToolOutput::error(format!(
                "The working folder changed after the command was approved ({e}), so it was not run. Call run_command again."
            ));
        }
        let timeout = Duration::from_secs(
            p.get("timeoutSeconds")
                .and_then(Value::as_u64)
                .unwrap_or(120),
        );
        let (program, args) = self.command_line(command);
        let spec = runtime::SpawnSpec::new(&program, cwd)
            .args(args)
            .env(self.env())
            .stdin(runtime::StdioMode::Null)
            .stdout(runtime::StdioMode::Piped)
            .stderr(runtime::StdioMode::Piped);
        let mut child = match runtime::spawn(spec) {
            Ok(c) => c,
            Err(e) => {
                return ToolOutput::error(format!(
                    "The shell ({}) could not be started, so the command did not run: {e}. This is a problem with this computer's setup, not with the command; tell the user.",
                    program.display()
                ));
            }
        };
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let truncated = Arc::new(Mutex::new(false));
        // The end of the error stream, kept apart: (bytes, bytes were dropped before them).
        let stderr_end = Arc::new(Mutex::new((Vec::<u8>::new(), false)));
        let mut readers = Vec::new();
        let streams: Vec<(bool, Box<dyn tokio::io::AsyncRead + Unpin + Send>)> = [
            child.take_stdout().map(|s| {
                (
                    false,
                    Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                )
            }),
            child.take_stderr().map(|s| {
                (
                    true,
                    Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>,
                )
            }),
        ]
        .into_iter()
        .flatten()
        .collect();
        for (is_stderr, mut stream) in streams {
            let out = output.clone();
            let cut = truncated.clone();
            let stderr_end = stderr_end.clone();
            let progress = ctx.progress.clone();
            readers.push(tokio::spawn(async move {
                let mut buf = vec![0u8; 16 * 1024];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if let Some(p) = &progress {
                                p(String::from_utf8_lossy(&buf[..n]).into_owned());
                            }
                            if is_stderr && let Ok(mut end) = stderr_end.lock() {
                                end.0.extend_from_slice(&buf[..n]);
                                if end.0.len() > STDERR_TAIL_BYTES {
                                    let drop = end.0.len() - STDERR_TAIL_BYTES;
                                    end.0.drain(..drop);
                                    end.1 = true;
                                }
                            }
                            if let Ok(mut o) = out.lock() {
                                let room = OUTPUT_LIMIT.saturating_sub(o.len());
                                if room < n
                                    && let Ok(mut c) = cut.lock()
                                {
                                    *c = true;
                                }
                                o.extend_from_slice(&buf[..n.min(room)]);
                            }
                        }
                    }
                }
            }));
        }
        let started = Instant::now();
        let outcome = tokio::select! {
            status = child.wait() => Ok(status),
            _ = ctx.cancel.cancelled() => Err("cancelled"),
            _ = tokio::time::sleep(timeout) => Err("timeout"),
        };
        if outcome.is_err() {
            let _ = child.kill().await;
        }
        for r in readers {
            let _ = tokio::time::timeout(Duration::from_secs(2), r).await;
        }
        let bytes = output.lock().map(|o| o.clone()).unwrap_or_default();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        let header = match &outcome {
            Ok(Ok(status)) => match status.code() {
                Some(code) => format!(
                    "Exit code {code} ({:.1} s)",
                    started.elapsed().as_secs_f64()
                ),
                None => "The command was ended by a signal".into(),
            },
            Ok(Err(e)) => format!("Could not wait for the command: {e}"),
            Err("cancelled") => "Stopped by the user".into(),
            Err(_) => format!(
                "Timed out after {} s; the command was ended",
                timeout.as_secs()
            ),
        };
        let failed = !matches!(&outcome, Ok(Ok(s)) if s.success());
        let timed_out = matches!(&outcome, Err(why) if *why != "cancelled");
        // The whole result: the header line, the output, then what the app adds about it.
        let mut full = format!("{header}\n{text}");
        let mut note = |line: &str| {
            if !full.ends_with('\n') {
                full.push('\n');
            }
            full.push_str(line);
        };
        if timed_out && text.trim().is_empty() {
            note("(it printed nothing)");
        }
        if truncated.lock().map(|c| *c).unwrap_or(false) {
            note("[output beyond 1 MiB was not kept]");
        }
        if timed_out {
            note(&timeout_note(timeout.as_secs()));
        }
        let code = match &outcome {
            Ok(Ok(status)) => status.code(),
            _ => None,
        };
        if failed
            && says_not_found(code, &text)
            && let Some(line) = self.not_found_note(command, &text)
        {
            note(&line);
        }
        let budget = shorten::budget_bytes(ctx.chat.result_cap_tokens);
        let ui = Some(json!({ "command": command, "cwd": cwd }));
        // A failed command also shows the end of its error stream, wherever that ended up.
        let stderr_tail = stderr_end
            .lock()
            .map(|end| last_lines(&end.0, end.1))
            .unwrap_or_default();
        let stderr_tail = (failed && !stderr_tail.trim().is_empty()).then_some(stderr_tail);
        match shorten::command_view(&full, budget, stderr_tail.as_deref()) {
            Some(view) => shorten::shortened(view, full, failed, ui),
            None => ToolOutput {
                content: vec![mcp_host::Content::Text { text: full }],
                is_error: failed,
                full_text: None,
                ui,
            },
        }
    }
}

impl TerminalServer {
    /// The names of the programs on PATH (without `.exe` and the like on Windows).
    fn programs_on_path(&self) -> Vec<String> {
        let Some(path) = self.path_var() else {
            return Vec::new();
        };
        let windows = matches!(self.config.shell, TerminalShell::PowerShell { .. });
        let mut names = Vec::new();
        for folder in std::env::split_paths(&path)
            .filter(|f| f.is_absolute())
            .take(MAX_PATH_FOLDERS)
        {
            let Ok(entries) = std::fs::read_dir(&folder) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                if names.len() >= MAX_PATH_NAMES {
                    return names;
                }
                let name = entry.file_name().to_string_lossy().into_owned();
                let name = match name.rsplit_once('.') {
                    Some((stem, ext))
                        if windows
                            && ["exe", "cmd", "bat", "com", "ps1"]
                                .iter()
                                .any(|e| ext.eq_ignore_ascii_case(e)) =>
                    {
                        stem.to_string()
                    }
                    _ => name,
                };
                names.push(name);
            }
        }
        names
    }

    /// For a command that failed because a program was not found: which one, and what is
    /// on PATH that is close to it. `None` when the command line cannot be broken down, or
    /// the output does not name a program that is missing.
    fn not_found_note(&self, command: &str, output: &str) -> Option<String> {
        let verdict = classify(self.config.shell.grammar(), command);
        let Parsed::Plain(commands) = &verdict.parsed else {
            return None;
        };
        let path = self.path_var();
        let missing = commands
            .iter()
            .map(|c| c.program.as_str())
            .find(|program| {
                !program.is_empty()
                    && !program.contains(['/', '\\'])
                    && runtime::resolve_program(program, path.as_deref().map(std::ffi::OsStr::new))
                        .is_none()
                    && output.lines().any(|line| {
                        let lower = line.to_lowercase();
                        line.contains(program)
                            && (lower.contains("not found") || lower.contains("not recognized"))
                    })
            })?;
        let shown = policy::sanitize::one_line(missing, 80);
        let close =
            errors::similar_names(missing, self.programs_on_path(), errors::MAX_SIMILAR_NAMES);
        let close: Vec<String> = close
            .iter()
            .map(|name| policy::sanitize::one_line(name, 80))
            .collect();
        Some(if close.is_empty() {
            format!(
                "[\"{shown}\" was not found: no program with that name is on PATH, and nothing on PATH has a similar name. Check the spelling, or ask the user to install it.]"
            )
        } else {
            format!(
                "[\"{shown}\" was not found: no program with that name is on PATH. Close names on PATH: {}. Check the spelling, or ask the user to install it.]",
                close.join(", ")
            )
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn chat(dir: &std::path::Path) -> ChatContext {
        ChatContext {
            chat_id: 1,
            approved_folders: vec![dir.display().to_string()],
            working_folder: None,
            forbidden_folders: vec![],
            skill_folders: vec![],
            skill_roots: vec![],
            result_cap_tokens: 2000,
            full_access: false,
        }
    }

    fn server(extra_env: &[(&str, &str)]) -> TerminalServer {
        let mut base: BTreeMap<String, String> = std::env::vars()
            .filter(|(k, _)| k == "PATH" || k == "HOME")
            .collect();
        for (k, v) in extra_env {
            base.insert((*k).into(), (*v).into());
        }
        TerminalServer::new(TerminalConfig {
            shell: TerminalShell::Sh,
            base_env: base,
            pass_through: vec![],
        })
    }

    async fn describe(s: &TerminalServer, c: &ChatContext, args: Value) -> Action {
        match s
            .describe("run_command", args.as_object().unwrap(), c)
            .await
        {
            Described::Action(a) => a,
            Described::Invalid(w) => panic!("{w}"),
        }
    }

    #[tokio::test]
    async fn runs_a_command_and_reports_the_exit_code() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("hello.txt"), "hi").unwrap();
        let s = server(&[]);
        let c = chat(&root);
        let a = describe(&s, &c, json!({"command": "ls"})).await;
        assert!(a.read_only);
        assert_eq!(a.risk, Risk::Normal);
        assert!(matches!(a.scope, Scope::ReadOnlyProgram { .. }));
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c.clone(),
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        let t = out.joined_text();
        assert!(t.starts_with("Exit code 0"), "{t}");
        assert!(t.contains("hello.txt"));

        let a = describe(&s, &c, json!({"command": "cat missing-file"})).await;
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c,
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        assert!(out.is_error);
        assert!(out.joined_text().starts_with("Exit code 1"));
    }

    #[tokio::test]
    async fn secrets_are_not_in_the_environment() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = server(&[
            ("GITHUB_TOKEN", "sekrit"),
            ("PLAIN_VAR", "visible"),
            ("DB_PASSWORD", "pw"),
        ]);
        let c = chat(&root);
        let a = describe(&s, &c, json!({"command": "env"})).await;
        assert_eq!(a.risk, Risk::AlwaysAsk, "env is a wrapper");
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c,
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        let t = out.joined_text();
        assert!(t.contains("PLAIN_VAR=visible"), "{t}");
        assert!(!t.contains("sekrit"));
        assert!(!t.contains("DB_PASSWORD"));
    }

    #[tokio::test]
    async fn stop_and_timeout_kill_the_command_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = server(&[]);
        let c = chat(&root);
        let a = describe(&s, &c, json!({"command": "sleep 30"})).await;
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            c2.cancel();
        });
        let t0 = Instant::now();
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c.clone(),
                    cancel,
                    progress: None,
                },
            )
            .await;
        assert!(t0.elapsed() < Duration::from_secs(2), "{:?}", t0.elapsed());
        assert!(out.joined_text().contains("Stopped by the user"));

        let a = describe(&s, &c, json!({"command": "sleep 30", "timeout_seconds": 1})).await;
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c,
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        assert!(out.joined_text().contains("Timed out after 1 s"));
    }

    #[tokio::test]
    async fn working_folder_and_read_paths_are_sandboxed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("ok")).unwrap();
        let s = server(&[]);
        let c = chat(&root.join("ok"));
        let a = describe(&s, &c, json!({"command": "ls", "cwd": "/"})).await;
        assert!(a.hard_limit.is_some());
        let a = describe(&s, &c, json!({"command": "cat ../secret.txt"})).await;
        assert!(a.hard_limit.is_some(), "{:?}", a.preview);
        let a = describe(&s, &c, json!({"command": "cargo build"})).await;
        assert!(a.hard_limit.is_none());
        assert!(matches!(a.scope, Scope::Command { .. }));
        let a = describe(&s, &c, json!({"command": "ls && git status"})).await;
        assert!(matches!(a.scope, Scope::Commands { .. }));
        let a = describe(&s, &c, json!({"command": "echo $(whoami)"})).await;
        assert_eq!(a.risk, Risk::AlwaysAsk);
    }

    /// The file and git tools do not write in skill folders; the terminal still runs there
    /// (an enabled skill's folder is a valid working directory).
    #[tokio::test]
    async fn commands_still_run_in_skill_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let skills = root.join("skills");
        std::fs::create_dir_all(skills.join("pdf")).unwrap();
        std::fs::write(skills.join("pdf/SKILL.md"), "x").unwrap();
        std::fs::create_dir_all(root.join("work")).unwrap();
        let s = server(&[]);
        let c = ChatContext {
            skill_folders: vec![skills.join("pdf").display().to_string()],
            skill_roots: vec![skills.display().to_string()],
            ..chat(&root.join("work"))
        };
        let cwd = skills.join("pdf").display().to_string();
        let a = describe(&s, &c, json!({"command": "ls", "cwd": cwd})).await;
        assert!(a.hard_limit.is_none(), "{a:?}");
        let out = s
            .call(
                "run_command",
                &Map::new(),
                &a,
                CallContext {
                    chat: c,
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        assert!(!out.is_error, "{out:?}");
        assert!(out.joined_text().contains("SKILL.md"));
    }

    async fn run(s: &TerminalServer, c: &ChatContext, args: Value) -> ToolOutput {
        let a = describe(s, c, args).await;
        assert!(a.hard_limit.is_none(), "{:?}", a.hard_limit);
        s.call(
            "run_command",
            &Map::new(),
            &a,
            CallContext {
                chat: c.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await
    }

    /// A long output that failed: the exit code, the first lines, the line in the middle
    /// that looks like an error with its number, the last lines, and the whole text for
    /// the app to store.
    #[tokio::test]
    async fn a_long_failing_output_shows_its_errors_by_line_number() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = server(&[]);
        let mut c = chat(&root);
        c.result_cap_tokens = 500;
        let out = run(
            &s,
            &c,
            json!({"command": "seq 1 1500; echo 'error: the build broke'; seq 1501 3000; exit 3"}),
        )
        .await;
        assert!(out.is_error);
        let text = out.joined_text();
        assert!(text.starts_with("Exit code 3 ("), "{text}");
        assert!(text.contains("\n1\n2\n3\n"), "{text}");
        // The header is line 1 of the stored text, so the echo is line 1502.
        let middle = text
            .split_once("\n[Lines ")
            .and_then(|(_, rest)| rest.split_once("[The output continues at line "))
            .map(|(middle, _)| middle)
            .unwrap_or_else(|| panic!("{text}"));
        assert!(
            middle.ends_with(
                " are not shown. One of them looks like an error:]\n1502: error: the build broke\n"
            ),
            "{middle}"
        );
        assert!(text.ends_with("\n2999\n3000"), "{text}");
        assert!(text.len() + 200 <= 1500, "{}", text.len());
        let full = out.full_text.clone().expect("the whole output");
        assert_eq!(crate::shorten::lines(&full).len(), 3002);
        assert_eq!(crate::shorten::lines(&full)[1501], "error: the build broke");
        let mut ui = out.ui.clone();
        let note = crate::shorten::Shown::take(&mut ui).expect("a note for the app");
        assert!(note.lines > 20 && note.next.is_some(), "{note:?}");
        assert_eq!(
            ui.as_ref()
                .and_then(|u| u.get("command"))
                .and_then(Value::as_str)
                .map(|c| c.starts_with("seq 1 1500")),
            Some(true)
        );

        // A long output that worked is shaped the same way and is not an error.
        let out = run(&s, &c, json!({"command": "seq 1 3000"})).await;
        assert!(!out.is_error);
        let text = out.joined_text();
        assert!(
            text.starts_with("Exit code 0 (") && text.ends_with("\n3000"),
            "{text}"
        );
        assert!(
            text.contains(" are not shown.]\n") && out.full_text.is_some(),
            "{text}"
        );
        // A short one is shown whole, with nothing to store.
        let out = run(&s, &c, json!({"command": "echo hello; exit 4"})).await;
        assert!(out.is_error && out.full_text.is_none());
        let text = out.joined_text();
        assert!(
            text.starts_with("Exit code 4 (") && text.ends_with("s)\nhello\n"),
            "{text:?}"
        );
        assert_eq!(
            out.ui,
            Some(json!({"command": "echo hello; exit 4", "cwd": root.display().to_string()}))
        );
    }

    #[tokio::test]
    async fn the_end_of_the_error_stream_is_shown_when_output_buries_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = server(&[]);
        let mut c = chat(&root);
        c.result_cap_tokens = 500;
        let out = run(
            &s,
            &c,
            json!({"command": "echo 'make: *** [all] Error 2' >&2; sleep 0.2; seq 1 3000; exit 2"}),
        )
        .await;
        let text = out.joined_text();
        assert!(text.starts_with("Exit code 2 ("), "{text}");
        assert!(
            text.ends_with(
                "\n3000\n[The last lines of its error output (stderr):]\nmake: *** [all] Error 2"
            ),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_program_that_is_not_there_is_named_with_close_names_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in ["mytool-build", "mytool-test", "unrelated"] {
            std::fs::write(bin.join(name), "#!/bin/sh\n").unwrap();
        }
        let path = format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        );
        let s = server(&[("PATH", &path)]);
        let c = chat(&root);
        let out = run(&s, &c, json!({"command": "mytool-biuld --release"})).await;
        assert!(out.is_error);
        let text = out.joined_text();
        assert!(text.starts_with("Exit code 127 ("), "{text}");
        assert_eq!(
            text.lines().last(),
            Some(
                "[\"mytool-biuld\" was not found: no program with that name is on PATH. Close names on PATH: mytool-build. Check the spelling, or ask the user to install it.]"
            ),
            "{text}"
        );
        // Nothing close: say so.
        let out = run(&s, &c, json!({"command": "zzqqxxyy-tool"})).await;
        assert_eq!(
            out.joined_text().lines().last(),
            Some(
                "[\"zzqqxxyy-tool\" was not found: no program with that name is on PATH, and nothing on PATH has a similar name. Check the spelling, or ask the user to install it.]"
            ),
        );
        // A program that is there and fails gets no such line.
        let out = run(&s, &c, json!({"command": "ls no-such-file-here"})).await;
        assert!(
            out.is_error && !out.joined_text().contains("was not found: no program"),
            "{}",
            out.joined_text()
        );
    }

    #[tokio::test]
    async fn a_timeout_says_how_long_it_ran_what_it_printed_and_how_to_allow_more() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let s = server(&[]);
        let c = chat(&root);
        let out = run(
            &s,
            &c,
            json!({"command": "echo step one; sleep 30", "timeout_seconds": 1}),
        )
        .await;
        assert!(out.is_error);
        assert_eq!(
            out.joined_text(),
            "Timed out after 1 s; the command was ended\nstep one\n[It was ended after the 1 s it was given. If it only needs more time, pass a larger timeout_seconds (at most 600). A command that waits for input never finishes here: commands get no input.]"
        );
        let out = run(&s, &c, json!({"command": "sleep 30", "timeout_seconds": 1})).await;
        assert_eq!(
            out.joined_text(),
            "Timed out after 1 s; the command was ended\n(it printed nothing)\n[It was ended after the 1 s it was given. If it only needs more time, pass a larger timeout_seconds (at most 600). A command that waits for input never finishes here: commands get no input.]"
        );
        assert_eq!(
            timeout_note(600),
            "[It was ended after 600 s, the longest a command may run. Run it in smaller steps. A command that waits for input never finishes here: commands get no input.]"
        );
    }

    #[tokio::test]
    async fn a_working_folder_that_is_not_allowed_names_the_allowed_ones() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("ok")).unwrap();
        std::fs::write(root.join("ok/file.txt"), "x").unwrap();
        let s = server(&[]);
        let ok = root.join("ok").display().to_string();
        let c = chat(&root.join("ok"));
        let a = describe(&s, &c, json!({"command": "ls", "cwd": "/"})).await;
        assert_eq!(
            a.hard_limit,
            Some(format!(
                "the path is outside the approved folders. Commands may run in: {ok}. Pass one of these folders, or a folder inside one, as cwd; the user can approve another folder in the Tools panel."
            ))
        );
        let a = describe(&s, &c, json!({"command": "cat ../secret.txt"})).await;
        assert_eq!(
            a.hard_limit,
            Some(format!(
                "cat reads ../secret.txt: the path is outside the approved folders. Commands may read files in: {ok}. The user can approve another folder in the Tools panel."
            ))
        );
        let invalid = |args: Value| {
            let s = s.clone();
            let c = c.clone();
            async move {
                match s
                    .describe("run_command", args.as_object().unwrap(), &c)
                    .await
                {
                    Described::Invalid(why) => why,
                    Described::Action(a) => panic!("{a:?}"),
                }
            }
        };
        assert_eq!(
            invalid(json!({"command": "ls", "cwd": "file.txt"})).await,
            format!(
                "cwd {ok}/file.txt is a file, not a folder. Pass a folder as cwd, or leave cwd out to run in {ok}."
            )
        );
        assert_eq!(
            invalid(json!({"command": "ls", "cwd": "nowhere"})).await,
            format!(
                "cwd {ok}/nowhere does not exist. Pass a folder that exists as cwd, or leave cwd out to run in {ok}."
            )
        );
        let none = ChatContext {
            approved_folders: vec![],
            ..c.clone()
        };
        let a = describe(&s, &none, json!({"command": "ls"})).await;
        assert_eq!(
            a.hard_limit.as_deref(),
            Some(
                "No folder is approved for this chat, so commands have nowhere to run. The user can approve a folder in the Tools panel."
            )
        );
    }

    #[test]
    fn secret_names() {
        for n in [
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "DB_PASSWORD",
            "my_secret",
            "GH_PAT",
        ] {
            assert!(looks_secret(n), "{n}");
        }
        for n in ["PATH", "HOME", "LANG", "KEYBOARD_LAYOUT"] {
            assert!(!looks_secret(n), "{n}");
        }
    }

    /// The same three things the old byte-based helper was held to, on the line-based view
    /// that replaced it: the start, the end, and a marker for what is missing.
    #[test]
    fn head_and_tail() {
        let t = "a".repeat(5000) + &"b".repeat(5000);
        let s = shorten::command_view(&t, 1500, None)
            .expect("too long")
            .text;
        assert!(s.starts_with("aaa") && s.ends_with("bbb") && s.contains("not shown"));
        assert!(s.len() <= 1500);
        assert_eq!(shorten::command_view("short", 1500, None), None);
    }
}

#[cfg(test)]
mod encoding_tests {
    use super::*;

    #[test]
    fn powershell_command_is_base64_utf16() {
        let s = TerminalServer::new(TerminalConfig {
            shell: TerminalShell::PowerShell {
                program: PathBuf::from("powershell.exe"),
            },
            base_env: BTreeMap::new(),
            pass_through: vec![],
        });
        let (_, args) = s.command_line("Write-Output \"a\"; exit 3");
        let encoded = args.last().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        let units: Vec<u16> = bytes
            .chunks(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        let script = String::from_utf16(&units).unwrap();
        assert!(script.contains("Write-Output \"a\"; exit 3"));
        assert!(script.contains("OutputEncoding"));
        assert!(args.contains(&"-EncodedCommand".to_string()));
        assert!(args.contains(&"-NoProfile".to_string()));
    }
}
