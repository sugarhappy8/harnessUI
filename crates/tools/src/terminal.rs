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
//! - Output streams into the tool card, is capped at 1 MiB, and the model sees the head
//!   and tail sized to the per-result cap. Default timeout 120 s. Stop kills the tree.
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

/// Output kept at most (8.4 raw limit).
pub const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Default and maximum timeouts.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);

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

fn arg_looks_like_path(a: &str) -> bool {
    !a.starts_with('-')
        && (a.contains('/') || a.contains('\\') || a.starts_with('.') || a.starts_with('~'))
}

fn head_tail(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let half = max_chars / 2;
    let mut a = half;
    while !text.is_char_boundary(a) {
        a -= 1;
    }
    let mut b = text.len() - half;
    while !text.is_char_boundary(b) {
        b += 1;
    }
    let omitted = b - a;
    format!(
        "{}\n[... {omitted} characters (about {} tokens) omitted ...]\n{}",
        &text[..a],
        omitted / 3,
        &text[b..]
    )
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
            return deny("No folder is approved for this chat, so commands have nowhere to run. Ask the user to approve one.".into());
        };
        let cwd = match sb.check(&cwd_arg, base.as_deref(), &RealFs) {
            Ok(c) if std::path::Path::new(&c.canonical).is_dir() => c,
            Ok(c) => return Described::Invalid(format!("{} is not a folder", c.canonical)),
            Err(e) => {
                let folders: Vec<String> =
                    sb.roots().iter().map(|r| r.canonical.to_string()).collect();
                return deny(format!("{e}. Commands may run in: {}.", folders.join(", ")));
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
                                return deny(format!("{} reads {a}: {e}", c.program));
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
            return ToolOutput::error(format!("the working folder changed after approval: {e}"));
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
            Err(e) => return ToolOutput::error(format!("the command could not start: {e}")),
        };
        let output = Arc::new(Mutex::new(Vec::<u8>::new()));
        let truncated = Arc::new(Mutex::new(false));
        let mut readers = Vec::new();
        let streams: Vec<Box<dyn tokio::io::AsyncRead + Unpin + Send>> = [
            child
                .take_stdout()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
            child
                .take_stderr()
                .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        ]
        .into_iter()
        .flatten()
        .collect();
        for mut stream in streams {
            let out = output.clone();
            let cut = truncated.clone();
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
        let cut_note = if truncated.lock().map(|c| *c).unwrap_or(false) {
            "\n[output beyond 1 MiB was not kept]"
        } else {
            ""
        };
        let full = format!("{header}\n{text}{cut_note}");
        let budget = ctx.chat.result_cap_tokens.max(500) * 3;
        let shown = head_tail(&full, budget);
        let failed = !matches!(&outcome, Ok(Ok(s)) if s.success());
        ToolOutput {
            content: vec![mcp_host::Content::Text {
                text: shown.clone(),
            }],
            is_error: failed,
            full_text: (shown.len() < full.len()).then_some(full),
            ui: Some(json!({ "command": command, "cwd": cwd })),
        }
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

    #[test]
    fn head_and_tail() {
        let t = "a".repeat(1000) + &"b".repeat(1000);
        let s = head_tail(&t, 200);
        assert!(s.starts_with("aaa") && s.ends_with("bbb") && s.contains("omitted"));
        assert_eq!(head_tail("short", 200), "short");
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
