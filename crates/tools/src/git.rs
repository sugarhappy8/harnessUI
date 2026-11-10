//! The built-in `git` server (ARCHITECTURE.md 9.3): `git_status`, `git_diff`,
//! `git_log`, `git_commit`, `git_clone`, `git_pull`, `git_push`.
//!
//! A repository can make git run programs (hooks, a file-system monitor, external diff
//! and text-conversion drivers, a custom ssh command, credential helpers, filter and
//! merge drivers). A model with write access to the folder could set those up and then
//! call a harmless-looking `git_status`. So:
//!
//! - system git runs from fixed argument templates with typed parameters, a `--` before
//!   every path, ref or URL, and values starting with a dash rejected;
//! - every call switches off hooks, the file-system monitor, external diff, text
//!   conversion and the pager, forces the ssh command and the credential helpers to the
//!   user's own (global/system) ones, allows only https and ssh transports, and turns
//!   terminal prompts off;
//! - a repository whose own config defines filter or merge drivers is refused, because
//!   git has no switch to turn those off.
//!
//! `git_push` always asks, and its card shows the push URL resolved when it runs.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{Action, Risk, Scope};
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;

/// Output kept at most.
const OUTPUT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone)]
pub struct GitConfig {
    /// The git executable, or `None` when git is not installed.
    pub git: Option<PathBuf>,
    /// Environment for git (the captured login environment; secrets filtered by the caller).
    pub env: Vec<(String, String)>,
    /// The user's own credential helpers (from global/system config), re-applied after
    /// the repository's are reset.
    pub credential_helpers: Vec<String>,
}

/// The `git` server.
#[derive(Debug, Clone)]
pub struct GitServer {
    config: GitConfig,
    flavor: Flavor,
}

fn null_device() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// Rejects values git could read as options.
fn plain_value(name: &str, v: &str) -> Result<(), String> {
    if v.starts_with('-') {
        return Err(format!("{name} must not start with '-'"));
    }
    if v.contains('\0') || v.contains('\n') && name != "message" {
        return Err(format!("{name} contains a control character"));
    }
    Ok(())
}

/// Only https and ssh remotes (scp-like `user@host:path` counts as ssh).
fn check_remote_url(url: &str) -> Result<String, String> {
    plain_value("url", url)?;
    if let Ok(u) = url::Url::parse(url) {
        return match u.scheme() {
            "https" | "ssh" => Ok(u.host_str().unwrap_or("").to_ascii_lowercase()),
            other => Err(format!("{other}: URLs are not allowed; use https or ssh")),
        };
    }
    // scp-like: [user@]host:path, no "::" (transport syntax) and no local paths.
    if let Some((left, right)) = url.split_once(':')
        && !left.contains('/')
        && !right.starts_with(':')
        && !left.is_empty()
    {
        let host = left.rsplit('@').next().unwrap_or(left);
        if host.len() > 1 {
            return Ok(host.to_ascii_lowercase());
        }
    }
    Err("only https:// and ssh remotes are allowed".into())
}

impl GitServer {
    pub fn new(config: GitConfig) -> Self {
        Self {
            config,
            flavor: Flavor::native(),
        }
    }

    /// The hardening options placed before every git subcommand.
    fn safety_args(&self) -> Vec<String> {
        let mut a: Vec<String> = vec![
            "--no-pager".into(),
            "-c".into(),
            format!("core.hooksPath={}", null_device()),
            "-c".into(),
            "core.fsmonitor=false".into(),
            "-c".into(),
            "core.sshCommand=ssh".into(),
            "-c".into(),
            "core.askPass=".into(),
            "-c".into(),
            "diff.external=".into(),
            "-c".into(),
            "protocol.allow=never".into(),
            "-c".into(),
            "protocol.https.allow=always".into(),
            "-c".into(),
            "protocol.ssh.allow=always".into(),
            "-c".into(),
            "credential.helper=".into(),
        ];
        for h in &self.config.credential_helpers {
            a.push("-c".into());
            a.push(format!("credential.helper={h}"));
        }
        a
    }

    fn env(&self) -> runtime::EnvSpec {
        let mut env = runtime::EnvSpec::empty();
        for (k, v) in &self.config.env {
            env = env.set(k.clone(), v.clone());
        }
        env.set("GIT_TERMINAL_PROMPT", "0")
            .set("GIT_PAGER", "cat")
            .set("GIT_OPTIONAL_LOCKS", "0")
            .set("LC_ALL", "C")
    }

    async fn run(
        &self,
        cwd: &Path,
        args: &[String],
        cancel: &tokio_util::sync::CancellationToken,
        timeout: Duration,
    ) -> Result<(i32, String), String> {
        let Some(git) = &self.config.git else {
            return Err(GIT_MISSING.into());
        };
        let mut all = self.safety_args();
        all.extend(args.iter().cloned());
        let spec = runtime::SpawnSpec::new(git, cwd)
            .args(all)
            .env(self.env())
            .stdin(runtime::StdioMode::Null)
            .stdout(runtime::StdioMode::Piped)
            .stderr(runtime::StdioMode::Piped);
        let mut child = runtime::spawn(spec).map_err(|e| format!("git could not start: {e}"))?;
        let mut out = child.take_stdout();
        let mut err = child.take_stderr();
        let read = async {
            let mut o = Vec::new();
            let mut e = Vec::new();
            if let Some(s) = out.as_mut() {
                let _ = s.take(OUTPUT_LIMIT as u64).read_to_end(&mut o).await;
            }
            if let Some(s) = err.as_mut() {
                let _ = s.take(64 * 1024).read_to_end(&mut e).await;
            }
            (o, e)
        };
        let result = tokio::select! {
            r = async { let (o, e) = read.await; (o, e, child.wait().await) } => Ok(r),
            _ = cancel.cancelled() => Err("Stopped by the user".to_string()),
            _ = tokio::time::sleep(timeout) => Err(format!("git did not finish within {} s", timeout.as_secs())),
        };
        match result {
            Ok((o, e, Ok(status))) => {
                let mut text = String::from_utf8_lossy(&o).into_owned();
                let stderr = String::from_utf8_lossy(&e);
                if !stderr.trim().is_empty() {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(stderr.trim_end());
                }
                Ok((status.code().unwrap_or(-1), text))
            }
            Ok((_, _, Err(e))) => Err(e.to_string()),
            Err(why) => {
                let _ = child.kill().await;
                Err(why)
            }
        }
    }

    fn sandbox(&self, chat: &ChatContext) -> PathSandbox {
        PathSandbox::new(
            self.flavor,
            &chat.approved_folders,
            &chat.forbidden_folders,
            &RealFs,
            &|p| volume_case(p),
        )
        .0
    }

    /// The repository root containing `folder`, checked against the sandbox.
    async fn repo_root(&self, chat: &ChatContext, folder: &str) -> Result<String, String> {
        let sb = self.sandbox(chat);
        let base = chat
            .working_folder
            .clone()
            .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()));
        let checked = sb
            .check(folder, base.as_deref(), &RealFs)
            .map_err(|e| e.to_string())?;
        let cancel = tokio_util::sync::CancellationToken::new();
        let (code, out) = self
            .run(
                Path::new(&checked.canonical),
                &["rev-parse".into(), "--show-toplevel".into()],
                &cancel,
                Duration::from_secs(20),
            )
            .await?;
        if code != 0 {
            return Err(format!(
                "{} is not inside a git repository",
                checked.canonical
            ));
        }
        let top = out.lines().next().unwrap_or("").trim().to_string();
        let top_checked = sb.check(&top, None, &RealFs).map_err(|e| {
            format!("the repository root {top} is outside the approved folders ({e})")
        })?;
        Ok(top_checked.canonical)
    }

    /// Refuses repositories whose own config defines programs git would run.
    async fn check_repo_config(&self, root: &str) -> Result<(), String> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let args: Vec<String> = ["config", "--local", "--name-only", "--get-regexp", r"^(filter\..*\.(clean|smudge|process)|merge\..*\.driver|diff\..*\.(command|textconv))$"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (code, out) = self
            .run(Path::new(root), &args, &cancel, Duration::from_secs(20))
            .await?;
        if code == 0 && !out.trim().is_empty() {
            return Err(format!(
                "this repository's own config tells git to run programs ({}). The git tools refuse such repositories; run git yourself in the terminal if you trust it.",
                out.lines().take(3).collect::<Vec<_>>().join(", ")
            ));
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn action(
        tool: &str,
        summary: String,
        preview: String,
        scope: Scope,
        risk: Risk,
        read_only: bool,
        outbound: bool,
        payload: Value,
    ) -> Described {
        Described::Action(Action {
            server: "git".into(),
            tool: tool.into(),
            summary: policy::sanitize::one_line(&summary, 200),
            preview: policy::sanitize::visible(&preview),
            scope,
            risk,
            read_only,
            outbound,
            hard_limit: None,
            payload,
        })
    }

    fn deny(tool: &str, why: String) -> Described {
        Described::Action(Action {
            server: "git".into(),
            tool: tool.into(),
            summary: tool.into(),
            preview: String::new(),
            scope: Scope::GitRead {
                repo: String::new(),
            },
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            hard_limit: Some(why),
            payload: Value::Null,
        })
    }

    async fn push_url(&self, root: &str, remote: &str) -> Result<String, String> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let (code, out) = self
            .run(
                Path::new(root),
                &[
                    "remote".into(),
                    "get-url".into(),
                    "--push".into(),
                    "--".into(),
                    remote.into(),
                ],
                &cancel,
                Duration::from_secs(20),
            )
            .await?;
        if code != 0 {
            return Err(format!("the remote {remote} does not exist"));
        }
        Ok(out.lines().next().unwrap_or("").trim().to_string())
    }
}

/// Shown when git is not installed.
pub const GIT_MISSING: &str = "Git is not installed on this computer. Install Git (on Windows from git-scm.com, on macOS with xcode-select --install) and restart the app.";

fn s<'a>(args: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    args.get(k).and_then(Value::as_str)
}

#[async_trait]
impl BuiltinServer for GitServer {
    fn name(&self) -> &str {
        "git"
    }

    fn tools(&self) -> Vec<ToolDef> {
        let repo = json!({"type":"string","description":"A folder inside the repository (default: the working folder)"});
        let def = |name: &str,
                   desc: &str,
                   props: Value,
                   required: Vec<&str>,
                   read_only: bool,
                   destructive: bool| ToolDef {
            name: name.into(),
            description: desc.into(),
            input_schema: json!({"type":"object","properties":props,"required":required,"additionalProperties":false}),
            destructive: Some(destructive),
            read_only: Some(read_only),
        };
        vec![
            def(
                "git_status",
                "Show the branch and changed files.",
                json!({"repo": repo}),
                vec![],
                true,
                false,
            ),
            def(
                "git_diff",
                "Show changes. staged=true shows what is staged for commit.",
                json!({"repo": repo, "staged": {"type":"boolean"}, "path": {"type":"string"}}),
                vec![],
                true,
                false,
            ),
            def(
                "git_log",
                "Show recent commits.",
                json!({"repo": repo, "limit": {"type":"integer","minimum":1,"maximum":100}, "path": {"type":"string"}}),
                vec![],
                true,
                false,
            ),
            def(
                "git_commit",
                "Stage the given paths (or all changes when paths is empty) and commit them.",
                json!({"repo": repo, "message": {"type":"string","minLength":1}, "paths": {"type":"array","items":{"type":"string"}}}),
                vec!["message"],
                false,
                false,
            ),
            def(
                "git_clone",
                "Clone an https or ssh repository into a folder inside an approved folder.",
                json!({"url": {"type":"string"}, "destination": {"type":"string"}}),
                vec!["url", "destination"],
                false,
                false,
            ),
            def(
                "git_pull",
                "Fast-forward the current branch from its upstream.",
                json!({"repo": repo}),
                vec![],
                false,
                false,
            ),
            def(
                "git_push",
                "Push the current branch to a remote. Always asks.",
                json!({"repo": repo, "remote": {"type":"string"}}),
                vec![],
                false,
                true,
            ),
        ]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        if self.config.git.is_none() {
            return Described::Invalid(GIT_MISSING.into());
        }
        if tool == "git_clone" {
            let (Some(url), Some(dest)) = (s(args, "url"), s(args, "destination")) else {
                return Described::Invalid("url and destination are required".into());
            };
            let host = match check_remote_url(url) {
                Ok(h) => h,
                Err(e) => return Self::deny(tool, e),
            };
            let sb = self.sandbox(chat);
            let base = chat
                .working_folder
                .clone()
                .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()));
            let checked = match sb.check(dest, base.as_deref(), &RealFs) {
                Ok(c) => c,
                Err(e) => return Self::deny(tool, e.to_string()),
            };
            if checked.exists
                && std::fs::read_dir(&checked.canonical)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(true)
            {
                return Described::Invalid(format!(
                    "{} already exists and is not empty",
                    checked.canonical
                ));
            }
            return Self::action(
                tool,
                format!("Clone {url}"),
                format!("git clone {url}\ninto {}", checked.canonical),
                Scope::GitClone { host },
                Risk::Normal,
                false,
                true,
                json!({"url": url, "dest": checked.canonical}),
            );
        }
        let folder = s(args, "repo")
            .map(str::to_string)
            .or_else(|| chat.working_folder.clone())
            .or_else(|| chat.approved_folders.first().cloned());
        let Some(folder) = folder else {
            return Self::deny(tool, "No folder is approved for this chat.".into());
        };
        let root = match self.repo_root(chat, &folder).await {
            Ok(r) => r,
            Err(e) => return Self::deny(tool, e),
        };
        if let Err(e) = self.check_repo_config(&root).await {
            return Self::deny(tool, e);
        }
        let read = |summary: String| {
            Self::action(
                tool,
                summary.clone(),
                summary,
                Scope::GitRead { repo: root.clone() },
                Risk::Normal,
                true,
                false,
                json!({"root": root}),
            )
        };
        match tool {
            "git_status" => read(format!("git status in {root}")),
            "git_diff" => {
                if let Some(p) = s(args, "path")
                    && let Err(e) = plain_value("path", p)
                {
                    return Described::Invalid(e);
                }
                read(format!("git diff in {root}"))
            }
            "git_log" => read(format!("git log in {root}")),
            "git_commit" => {
                let message = s(args, "message").unwrap_or("").to_string();
                let paths: Vec<String> = args
                    .get("paths")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                for p in &paths {
                    if let Err(e) = plain_value("path", p) {
                        return Described::Invalid(e);
                    }
                }
                let what = if paths.is_empty() {
                    "all changes".to_string()
                } else {
                    paths.join(", ")
                };
                Self::action(
                    tool,
                    format!("Commit {what} in {root}"),
                    format!("git commit in {root}\nfiles: {what}\nmessage:\n{message}"),
                    Scope::GitWrite { repo: root.clone() },
                    Risk::Normal,
                    false,
                    false,
                    json!({"root": root, "message": message, "paths": paths}),
                )
            }
            "git_pull" => Self::action(
                tool,
                format!("git pull --ff-only in {root}"),
                format!("git pull --ff-only in {root}"),
                Scope::GitWrite { repo: root.clone() },
                Risk::Normal,
                false,
                true,
                json!({"root": root}),
            ),
            "git_push" => {
                let remote = s(args, "remote").unwrap_or("origin").to_string();
                if let Err(e) = plain_value("remote", &remote) {
                    return Described::Invalid(e);
                }
                let url = match self.push_url(&root, &remote).await {
                    Ok(u) => u,
                    Err(e) => return Described::Invalid(e),
                };
                Self::action(
                    tool,
                    format!("Push to {remote} ({url})"),
                    format!("git push {remote}\npush URL: {url}\nrepository: {root}"),
                    Scope::GitWrite { repo: root.clone() },
                    Risk::AlwaysAsk,
                    false,
                    true,
                    json!({"root": root, "remote": remote, "url": url}),
                )
            }
            other => Described::Invalid(format!("the git server has no tool named {other}")),
        }
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let p = &action.payload;
        let timeout =
            Duration::from_secs(if matches!(tool, "git_clone" | "git_pull" | "git_push") {
                600
            } else {
                60
            });
        let cancel = ctx.cancel.clone();
        let run = |cwd: PathBuf, a: Vec<String>| {
            let cancel = cancel.clone();
            async move { self.run(&cwd, &a, &cancel, timeout).await }
        };
        let result = match tool {
            "git_clone" => {
                let (Some(url), Some(dest)) = (
                    p.get("url").and_then(Value::as_str),
                    p.get("dest").and_then(Value::as_str),
                ) else {
                    return ToolOutput::error("internal error: the approved action has no URL");
                };
                let dest = PathBuf::from(dest);
                let parent = dest.parent().map(Path::to_path_buf).unwrap_or_default();
                if let Err(e) = std::fs::create_dir_all(&parent) {
                    return ToolOutput::error(format!(
                        "could not create {}: {e}",
                        parent.display()
                    ));
                }
                run(
                    parent,
                    vec![
                        "clone".into(),
                        "--no-recurse-submodules".into(),
                        "--".into(),
                        url.into(),
                        dest.display().to_string(),
                    ],
                )
                .await
            }
            _ => {
                let Some(root) = p.get("root").and_then(Value::as_str).map(PathBuf::from) else {
                    return ToolOutput::error(
                        "internal error: the approved action has no repository",
                    );
                };
                // Re-check what was approved immediately before running.
                if let Err(e) = self.check_repo_config(&root.display().to_string()).await {
                    return ToolOutput::error(e);
                }
                match tool {
                    "git_status" => {
                        run(
                            root,
                            vec!["status".into(), "--porcelain=v1".into(), "--branch".into()],
                        )
                        .await
                    }
                    "git_diff" => {
                        let mut a = vec![
                            "diff".into(),
                            "--no-ext-diff".into(),
                            "--no-textconv".into(),
                            "--no-color".into(),
                        ];
                        if args.get("staged").and_then(Value::as_bool).unwrap_or(false) {
                            a.push("--cached".into());
                        }
                        a.push("--".into());
                        if let Some(path) = s(args, "path") {
                            a.push(path.into());
                        }
                        run(root, a).await
                    }
                    "git_log" => {
                        let n = args
                            .get("limit")
                            .and_then(Value::as_u64)
                            .unwrap_or(20)
                            .clamp(1, 100);
                        let mut a = vec![
                            "log".into(),
                            "--no-color".into(),
                            format!("-n{n}"),
                            "--date=short".into(),
                            "--pretty=format:%h %ad %an: %s".into(),
                            "--".into(),
                        ];
                        if let Some(path) = s(args, "path") {
                            a.push(path.into());
                        }
                        run(root, a).await
                    }
                    "git_commit" => {
                        let paths: Vec<String> = p
                            .get("paths")
                            .and_then(Value::as_array)
                            .map(|a| {
                                a.iter()
                                    .filter_map(|v| v.as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        let message = p
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let mut add = vec!["add".into()];
                        if paths.is_empty() {
                            add.push("--all".into());
                        }
                        add.push("--".into());
                        add.extend(paths);
                        match run(root.clone(), add).await {
                            Ok((0, _)) => {
                                run(
                                    root,
                                    vec![
                                        "commit".into(),
                                        "--no-verify".into(),
                                        format!("--message={message}"),
                                    ],
                                )
                                .await
                            }
                            other => other,
                        }
                    }
                    "git_pull" => {
                        run(
                            root,
                            vec![
                                "pull".into(),
                                "--ff-only".into(),
                                "--no-recurse-submodules".into(),
                            ],
                        )
                        .await
                    }
                    "git_push" => {
                        let remote = p
                            .get("remote")
                            .and_then(Value::as_str)
                            .unwrap_or("origin")
                            .to_string();
                        let approved_url = p
                            .get("url")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        // The push URL resolved when it runs must be the one on the card.
                        match self.push_url(&root.display().to_string(), &remote).await {
                            Ok(now) if now == approved_url => {}
                            Ok(now) => {
                                return ToolOutput::error(format!(
                                    "the push URL changed after approval (now {now}); nothing was pushed"
                                ));
                            }
                            Err(e) => return ToolOutput::error(e),
                        }
                        if check_remote_url(&approved_url).is_err() {
                            return ToolOutput::error(
                                "the push URL is not https or ssh; nothing was pushed",
                            );
                        }
                        run(
                            root,
                            vec!["push".into(), "--no-verify".into(), "--".into(), remote],
                        )
                        .await
                    }
                    other => {
                        return ToolOutput::error(format!(
                            "the git server has no tool named {other}"
                        ));
                    }
                }
            }
        };
        match result {
            Ok((code, text)) => {
                let budget = ctx.chat.result_cap_tokens.max(500) * 3;
                let body = if text.trim().is_empty() {
                    "(no output)".to_string()
                } else {
                    text
                };
                let full = format!("exit code {code}\n{body}");
                let shown = if full.len() > budget {
                    let mut cut = budget;
                    while !full.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    format!(
                        "{}\n[... output cut to fit; {} characters omitted ...]",
                        &full[..cut],
                        full.len() - cut
                    )
                } else {
                    full.clone()
                };
                ToolOutput {
                    is_error: code != 0,
                    full_text: (shown.len() < full.len()).then_some(full),
                    ..ToolOutput::text(shown)
                }
            }
            Err(e) => ToolOutput::error(e),
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn server() -> Option<GitServer> {
        let path = std::env::var("PATH").ok();
        let git = runtime::resolve_program("git", path.as_deref().map(std::ffi::OsStr::new))?;
        let env: Vec<(String, String)> = std::env::vars()
            .filter(|(k, _)| k == "PATH" || k == "HOME")
            .chain([
                ("GIT_AUTHOR_NAME".into(), "T".into()),
                ("GIT_AUTHOR_EMAIL".into(), "t@example.com".into()),
                ("GIT_COMMITTER_NAME".into(), "T".into()),
                ("GIT_COMMITTER_EMAIL".into(), "t@example.com".into()),
            ])
            .collect();
        Some(GitServer::new(GitConfig {
            git: Some(git),
            env,
            credential_helpers: vec![],
        }))
    }

    fn chat(dir: &Path) -> ChatContext {
        ChatContext {
            chat_id: 1,
            approved_folders: vec![dir.display().to_string()],
            result_cap_tokens: 2000,
            ..Default::default()
        }
    }

    async fn go(
        s: &GitServer,
        c: &ChatContext,
        tool: &str,
        a: Value,
    ) -> Result<(Action, ToolOutput), String> {
        let a = a.as_object().cloned().unwrap();
        match s.describe(tool, &a, c).await {
            Described::Invalid(w) => Err(w),
            Described::Action(act) if act.hard_limit.is_some() => Err(act.hard_limit.unwrap()),
            Described::Action(act) => {
                let out = s
                    .call(
                        tool,
                        &a,
                        &act,
                        CallContext {
                            chat: c.clone(),
                            cancel: CancellationToken::new(),
                            progress: None,
                        },
                    )
                    .await;
                Ok((act, out))
            }
        }
    }

    async fn init_repo(s: &GitServer, dir: &Path) {
        let cancel = CancellationToken::new();
        s.run(
            dir,
            &["init".into(), "-q".into(), "-b".into(), "main".into()],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn status_commit_log_and_diff() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        std::fs::write(root.join("a.txt"), "one\n").unwrap();
        let c = chat(&root);
        let (act, out) = go(&s, &c, "git_status", json!({})).await.unwrap();
        assert_eq!(
            act.scope,
            Scope::GitRead {
                repo: root.display().to_string()
            }
        );
        assert!(
            out.joined_text().contains("?? a.txt"),
            "{}",
            out.joined_text()
        );
        let (act, out) = go(&s, &c, "git_commit", json!({"message": "first"}))
            .await
            .unwrap();
        assert_eq!(
            act.scope,
            Scope::GitWrite {
                repo: root.display().to_string()
            }
        );
        assert!(!out.is_error, "{}", out.joined_text());
        let (_, out) = go(&s, &c, "git_log", json!({})).await.unwrap();
        assert!(out.joined_text().contains("first"));
        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        let (_, out) = go(&s, &c, "git_diff", json!({})).await.unwrap();
        assert!(out.joined_text().contains("+two"));
    }

    #[tokio::test]
    async fn hooks_never_run() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        let marker = root.join("hook-ran");
        let hook = root.join(".git/hooks/pre-commit");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch {}\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::write(root.join("a.txt"), "x").unwrap();
        let (_, out) = go(&s, &chat(&root), "git_commit", json!({"message": "m"}))
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.joined_text());
        assert!(!marker.exists(), "a repository hook ran");
    }

    #[tokio::test]
    async fn repositories_with_filter_drivers_are_refused() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        let cancel = CancellationToken::new();
        s.run(
            &root,
            &[
                "config".into(),
                "filter.evil.clean".into(),
                "touch pwned".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let why = go(&s, &chat(&root), "git_status", json!({}))
            .await
            .unwrap_err();
        assert!(why.contains("run programs"), "{why}");
    }

    #[tokio::test]
    async fn push_always_asks_and_shows_the_url() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        let cancel = CancellationToken::new();
        s.run(
            &root,
            &[
                "remote".into(),
                "add".into(),
                "origin".into(),
                "https://example.com/r.git".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let a = json!({}).as_object().cloned().unwrap();
        let Described::Action(act) = s.describe("git_push", &a, &chat(&root)).await else {
            panic!()
        };
        assert_eq!(act.risk, Risk::AlwaysAsk);
        assert!(act.preview.contains("https://example.com/r.git"));
        // The URL changes after approval: nothing is pushed.
        s.run(
            &root,
            &[
                "remote".into(),
                "set-url".into(),
                "origin".into(),
                "https://evil.example/r.git".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let out = s
            .call(
                "git_push",
                &a,
                &act,
                CallContext {
                    chat: chat(&root),
                    cancel: CancellationToken::new(),
                    progress: None,
                },
            )
            .await;
        assert!(out.is_error);
        assert!(out.joined_text().contains("changed after approval"));
    }

    #[test]
    fn remote_urls() {
        assert_eq!(
            check_remote_url("https://github.com/a/b.git").unwrap(),
            "github.com"
        );
        assert_eq!(
            check_remote_url("git@github.com:a/b.git").unwrap(),
            "github.com"
        );
        assert_eq!(check_remote_url("ssh://git@host:22/x").unwrap(), "host");
        for bad in [
            "file:///etc",
            "/local/path",
            "ext::sh -c touch",
            "-uploadpack=evil",
            "http://plain.example/x",
            "C:\\repo",
            "git://x/y",
        ] {
            assert!(check_remote_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn dash_values_are_rejected() {
        assert!(plain_value("path", "--output=/etc/x").is_err());
        assert!(plain_value("path", "src/main.rs").is_ok());
    }
}
