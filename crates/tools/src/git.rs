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
//! `git_push` always asks, and its card shows the push URL resolved when it runs. A user
//! name or password in a remote URL (often a token) is shown as `***` on cards, in errors
//! and in the output of clone, pull and push, and the push URL is kept out of the payload
//! key the agent stores as a result's source (9.2).
//!
//! Clone, pull and commit never write inside a skill folder (`ChatContext::skill_roots`):
//! skills are added only through the skills UI (9.3, 10).
//!
//! A failure says what to do next (app 0.0.4): [`advice`] reads what git said (git runs
//! with `LC_ALL=C`, so in English) or what the built-in library said, and adds one short
//! paragraph: not a repository, nothing to commit, merge conflicts with the files, a
//! sign-in the remote refused (credentials are the user's to set up), no branch checked
//! out. A long output is shown as its first and last lines, and the whole text goes to
//! the app, which stores it for `read_output`.
//!
//! When git is not installed (`GitConfig::git` is `None`) the same seven tools run on
//! libgit2 instead (`git_fallback`, `git_worker`), with the same actions and scopes:
//! local work in-process, network work (clone, the fetch of pull, push) in a worker
//! process Stop can kill. The fallback runs no hooks, has no Git LFS, reaches remotes
//! over https only, and authenticates to private repositories with a per-host token the
//! app supplies through [`TokenSource`].

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{Action, Risk, Scope};
use serde_json::{Map, Value, json};
use tokio::io::AsyncReadExt;
use tokio_util::sync::CancellationToken;

use crate::git_fallback::{self, RemoteRefusal, display_url, hide_credentials, mask_userinfo};
use crate::git_worker::{self, Job, WorkerCommand};
pub use crate::git_worker::{WORKER_FLAG, worker_main};
use crate::shorten;
use crate::skill_guard::{self, Reach};

/// Output kept at most.
const OUTPUT_LIMIT: usize = 1024 * 1024;
/// Time limit for clone, pull and push.
const NETWORK_TIMEOUT: Duration = Duration::from_secs(600);
/// Time limit for local operations.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(60);
/// The payload key of `git_push`'s approved push URLs. Deliberately not `url`, which the
/// agent reads as the result's source.
const PUSH_URL_KEY: &str = "pushUrl";

#[derive(Debug, Clone)]
pub struct GitConfig {
    /// The git executable, or `None` when git is not installed (the libgit2 fallback
    /// is used then).
    pub git: Option<PathBuf>,
    /// Environment for git (the captured login environment; secrets filtered by the caller).
    /// The fallback's worker gets it too, and reads `GIT_AUTHOR_*` / `GIT_COMMITTER_*`
    /// from it for commits.
    pub env: Vec<(String, String)>,
    /// The user's own credential helpers (from global/system config), re-applied after
    /// the repository's are reset.
    pub credential_helpers: Vec<String>,
}

/// Where the libgit2 fallback gets access tokens for private HTTPS repositories. The app
/// reads them from the keychain, one per host.
///
/// `host` is the remote's host in lowercase ASCII (punycode for internationalised names),
/// followed by `:port` when the URL names a non-default port: `github.com`,
/// `git.example.com:8443`. It is only ever the host of the URL the user approved, and it
/// is asked for only when that server answers 401. The token is sent as the password of
/// HTTP Basic authentication, with the user name from the URL or `x-access-token` (which
/// GitHub, GitLab, Gitea and Azure DevOps accept for personal access tokens). Called on
/// a blocking thread.
pub trait TokenSource: Send + Sync {
    /// The token stored for `host`, or `None`.
    fn token_for(&self, host: &str) -> Option<String>;
}

impl<F> TokenSource for F
where
    F: Fn(&str) -> Option<String> + Send + Sync,
{
    fn token_for(&self, host: &str) -> Option<String> {
        self(host)
    }
}

/// Which implementation runs the git tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitBackend {
    /// The system `git` executable.
    System,
    /// The built-in libgit2 fallback.
    Libgit2,
}

/// For the UI to show next to the git tools while the fallback is in use (9.3).
pub const FALLBACK_NOTICE: &str = "Git is not installed, so the git tools use the built-in Git library. It runs no hooks, has no Git LFS support and reaches remotes over https only. Private repositories need an access token, entered once per host.";

/// Shown when the fallback has no worker to run network operations in.
const FALLBACK_NO_WORKER: &str = "Git is not installed, and the built-in Git library cannot run clone, pull or push in this build (no worker process is configured). Install Git (on Windows from git-scm.com, on macOS with xcode-select --install) and restart the app.";

/// The `git` server.
#[derive(Clone)]
pub struct GitServer {
    config: GitConfig,
    flavor: Flavor,
    /// How to start the fallback's worker process.
    worker: Option<WorkerCommand>,
    /// Tokens for private HTTPS repositories (fallback only).
    tokens: Option<Arc<dyn TokenSource>>,
    /// Local-path remotes for the fallback. Only the crate's own tests set this; there is
    /// no public way to turn it on.
    allow_local_remotes: bool,
}

impl std::fmt::Debug for GitServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GitServer")
            .field("config", &self.config)
            .field("flavor", &self.flavor)
            .field("worker", &self.worker)
            .field("tokens", &self.tokens.as_ref().map(|_| "<token source>"))
            .finish()
    }
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
pub(crate) fn check_remote_url(url: &str) -> Result<String, String> {
    plain_value("url", url)?;
    // git and the url crate read some URLs differently (`https://a.com\@b.com/x`: the url
    // crate sees a.com, git connects to b.com). Refuse anything they could disagree on, so
    // the host on the card and in the grant is the host git contacts.
    if url.contains('\\') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the URL contains a backslash, a space or a control character; write it as https://host/path".into());
    }
    if let Ok(u) = url::Url::parse(url) {
        return match u.scheme() {
            "https" | "ssh" => {
                let host = u.host_str().unwrap_or("").to_ascii_lowercase();
                match git_url_host(url) {
                    Some(git_host) if git_host == host => Ok(host),
                    _ => Err(format!(
                        "the URL is ambiguous (git would not read its host as {host}); write it as {}://{host}/path",
                        u.scheme()
                    )),
                }
            }
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

/// The host git connects to for a `scheme://` URL: in the authority (the part after `://`
/// up to the first `/`, `?` or `#`, as git's credential code and curl read it), the text
/// after the last `@`, without a port or brackets, lowercased.
fn git_url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let hostport = authority.rsplit('@').next().unwrap_or(authority);
    let host = if let Some(stripped) = hostport.strip_prefix('[') {
        stripped.split(']').next().unwrap_or(stripped).to_string()
    } else {
        hostport.split(':').next().unwrap_or(hostport).to_string()
    };
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

impl GitServer {
    pub fn new(config: GitConfig) -> Self {
        Self {
            config,
            flavor: Flavor::native(),
            worker: None,
            tokens: None,
            allow_local_remotes: false,
        }
    }

    /// The app's own executable, started with [`WORKER_FLAG`] for the fallback's network
    /// operations (`std::env::current_exe()`). Without it the fallback refuses clone,
    /// pull and push. Not used when system git is installed.
    pub fn with_worker(mut self, app_exe: impl Into<PathBuf>) -> Self {
        self.worker = Some(WorkerCommand::app(app_exe.into()));
        self
    }

    /// Per-host access tokens for private HTTPS repositories (fallback only).
    pub fn with_tokens(mut self, tokens: Arc<dyn TokenSource>) -> Self {
        self.tokens = Some(tokens);
        self
    }

    /// Which implementation runs the tools; the UI shows [`FALLBACK_NOTICE`] for
    /// [`GitBackend::Libgit2`].
    pub fn backend(&self) -> GitBackend {
        if self.config.git.is_some() {
            GitBackend::System
        } else {
            GitBackend::Libgit2
        }
    }

    fn fallback(&self) -> bool {
        self.config.git.is_none()
    }

    /// A remote URL as the card shows it, with any user name and password replaced by
    /// `***` (credentials never reach the card, the audit log or the chat, 9.2). System
    /// git: otherwise exactly as git will be given it, as before the fallback existed (git
    /// and the url crate can read a URL differently, and a card rewritten into the url
    /// crate's reading would hide that). Fallback: the URL is in standard form already
    /// (`fallback_remote`).
    fn card_url(&self, url: &str) -> String {
        if self.fallback() {
            display_url(url)
        } else {
            mask_userinfo(url)
        }
    }

    /// Skill folders are written only by the skills UI (9.3, 10): a clone or pull refuses a
    /// folder in a skill folder or holding one, and a commit (which writes the repository's
    /// git folder) a repository in one. The other tools do not write the working tree.
    fn skill_check(&self, tool: &str, path: &str, chat: &ChatContext) -> Result<(), String> {
        let reach = match tool {
            "git_clone" | "git_pull" => Reach::InsideOrAround,
            "git_commit" => Reach::Inside,
            _ => return Ok(()),
        };
        skill_guard::check(path, &chat.skill_roots, self.flavor, reach)
    }

    /// The host a clone grant covers, or why the URL is refused.
    fn remote_host(&self, url: &str) -> Result<String, RemoteRefusal> {
        if self.fallback() {
            git_fallback::fallback_remote(url, self.allow_local_remotes)
        } else {
            check_remote_url(url).map_err(RemoteRefusal::Blocked)
        }
    }

    /// The fallback worker's environment: the platform allowlist and proxy variables
    /// from the app, then the configured git environment.
    fn worker_env(&self) -> runtime::EnvSpec {
        let mut env = runtime::EnvSpec::minimal();
        for k in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "NO_PROXY",
            "no_proxy",
            "ALL_PROXY",
            "all_proxy",
        ] {
            env = env.inherit(k);
        }
        for (k, v) in &self.config.env {
            env = env.set(k.clone(), v.clone());
        }
        env
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

    /// Runs local libgit2 work on a blocking thread. Stop returns at once, and the work
    /// itself stops at its next check of the token (4.2); so does the timeout.
    async fn local<T, F>(cancel: &CancellationToken, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce(CancellationToken) -> Result<T, String> + Send + 'static,
    {
        let token = cancel.child_token();
        let inner = token.clone();
        let task = tokio::task::spawn_blocking(move || f(inner));
        tokio::select! {
            r = task => r.unwrap_or_else(|e| Err(format!("internal error: {e}"))),
            _ = cancel.cancelled() => Err(git_fallback::STOPPED.into()),
            _ = tokio::time::sleep(LOCAL_TIMEOUT) => {
                token.cancel();
                Err(format!("git did not finish within {} s", LOCAL_TIMEOUT.as_secs()))
            }
        }
    }

    /// Checks a fallback repository's working tree, git folder and shared git folder
    /// against the sandbox and returns the canonical working tree. Stricter than the
    /// system-git path: a `.git` file pointing outside the approved folders is refused,
    /// because commits would write there.
    fn contain_repo(
        &self,
        chat: &ChatContext,
        paths: &git_fallback::RepoPaths,
    ) -> Result<String, String> {
        let sb = self.sandbox(chat);
        let top = paths.workdir.display().to_string();
        let top_checked = sb.check(&top, None, &RealFs).map_err(|e| {
            format!("the repository root {top} is outside the approved folders ({e})")
        })?;
        for (what, p) in [
            ("git folder", &paths.gitdir),
            ("shared git folder", &paths.commondir),
        ] {
            let shown = p.display().to_string();
            sb.check(&shown, None, &RealFs).map_err(|e| {
                format!(
                    "the repository's {what} {shown} is outside the approved folders ({e}); the git tools refuse such repositories"
                )
            })?;
        }
        Ok(top_checked.canonical)
    }

    /// The fallback's repository root containing `folder`.
    async fn fallback_root(&self, chat: &ChatContext, folder: &str) -> Result<String, String> {
        let sb = self.sandbox(chat);
        let base = chat
            .working_folder
            .clone()
            .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()));
        let checked = sb
            .check(folder, base.as_deref(), &RealFs)
            .map_err(|e| e.to_string())?;
        let start = PathBuf::from(&checked.canonical);
        let paths = Self::local(&CancellationToken::new(), move |_| {
            git_fallback::discover(&start)
        })
        .await?;
        self.contain_repo(chat, &paths)
    }

    /// Re-checks the approved repository immediately before a fallback side effect.
    async fn fallback_recheck(&self, chat: &ChatContext, root: &Path) -> Result<(), String> {
        let at = root.to_path_buf();
        let paths = Self::local(&CancellationToken::new(), move |_| {
            git_fallback::paths_at(&at)
        })
        .await?;
        let now = self.contain_repo(chat, &paths)?;
        if Path::new(&now) != root {
            return Err(format!(
                "the repository at {} changed after approval; nothing was done",
                root.display()
            ));
        }
        Ok(())
    }

    /// The repository root containing `folder`, checked against the sandbox.
    async fn repo_root(&self, chat: &ChatContext, folder: &str) -> Result<String, String> {
        if self.fallback() {
            return self.fallback_root(chat, folder).await;
        }
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
            // Allow all: a push publishes in the user's name and keeps asking. Clone, pull
            // and push are the git tools that reach the network (the outbound ones).
            keep_asking: (tool == "git_push").then_some(policy::KeepAsking::Publish),
            network: outbound,
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
            keep_asking: None,
            network: false,
            hard_limit: Some(why),
            payload: Value::Null,
        })
    }

    async fn push_url(&self, root: &str, remote: &str) -> Result<String, String> {
        if self.fallback() {
            let (root, remote) = (PathBuf::from(root), remote.to_string());
            return Self::local(&CancellationToken::new(), move |_| {
                git_fallback::push_url(&root, &remote)
            })
            .await;
        }
        let cancel = tokio_util::sync::CancellationToken::new();
        let (code, out) = self
            .run(
                Path::new(root),
                // --all: git pushes to every push URL (or every URL when there is no
                // pushurl), so the card must show, and the call compare, all of them.
                &[
                    "remote".into(),
                    "get-url".into(),
                    "--push".into(),
                    "--all".into(),
                    "--".into(),
                    remote.into(),
                ],
                &cancel,
                Duration::from_secs(20),
            )
            .await?;
        if code != 0 {
            return Err(format!(
                "the remote {remote} does not exist in this repository. git remote -v (run_command) lists the remotes"
            ));
        }
        let urls: Vec<&str> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        if urls.is_empty() {
            return Err(format!("the remote {remote} has no URL"));
        }
        Ok(urls.join("\n"))
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
        if self.fallback()
            && self.worker.is_none()
            && matches!(tool, "git_clone" | "git_pull" | "git_push")
        {
            return Described::Invalid(FALLBACK_NO_WORKER.into());
        }
        if tool == "git_clone" {
            let (Some(url), Some(dest)) = (s(args, "url"), s(args, "destination")) else {
                return Described::Invalid("url and destination are required".into());
            };
            let host = match self.remote_host(url) {
                Ok(h) => h,
                Err(RemoteRefusal::Blocked(e)) => return Self::deny(tool, e),
                Err(RemoteRefusal::Unsupported(e)) => return Described::Invalid(e),
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
            if let Err(why) = self.skill_check(tool, &checked.canonical, chat) {
                return Self::deny(tool, why);
            }
            if checked.exists
                && std::fs::read_dir(&checked.canonical)
                    .map(|mut d| d.next().is_some())
                    .unwrap_or(true)
            {
                return Described::Invalid(format!(
                    "{} already exists and is not empty, so nothing can be cloned into it. Pass a destination that does not exist yet, or an empty folder.",
                    checked.canonical
                ));
            }
            let shown = self.card_url(url);
            return Self::action(
                tool,
                format!("Clone {shown}"),
                format!("git clone {shown}\ninto {}", checked.canonical),
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
            return Self::deny(
                tool,
                "No folder is approved for this chat, so there is no repository to work on. The user can approve a folder in the Tools panel.".into(),
            );
        };
        let root = match self.repo_root(chat, &folder).await {
            Ok(r) => r,
            Err(e) => return Self::deny(tool, with_advice(tool, e)),
        };
        if let Err(why) = self.skill_check(tool, &root, chat) {
            return Self::deny(tool, why);
        }
        // libgit2 never runs the programs a repository's config can name, so the
        // fallback has nothing to refuse here.
        if !self.fallback()
            && let Err(e) = self.check_repo_config(&root).await
        {
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
                // The fallback cannot push over ssh; say so before anyone approves. The
                // address comes from the repository's config, a file: an answer given
                // before the gate (an app note) does not quote it.
                if self.fallback() && self.remote_host(&url).is_err() {
                    return Described::Invalid(format!(
                        "cannot push to {remote}: Git is not installed, and the built-in Git library pushes over https only, to an address written in standard form (https://host/path, with no backslash or spaces), so that the server it connects to is the one shown for approval. The address of {remote} is not like that; git remote -v (run_command) shows it. Install Git, or give the remote an https address."
                    ));
                }
                let shown: Vec<String> = url.lines().map(|u| self.card_url(u)).collect();
                let first = shown.first().cloned().unwrap_or_default();
                let more = if shown.len() > 1 {
                    format!(" and {} more", shown.len() - 1)
                } else {
                    String::new()
                };
                let label = if shown.len() > 1 {
                    "push URLs (git pushes to every one)"
                } else {
                    "push URL"
                };
                Self::action(
                    tool,
                    format!("Push to {remote} ({first}{more})"),
                    format!(
                        "git push {remote}\n{label}:\n{}\nrepository: {root}",
                        shown.join("\n")
                    ),
                    Scope::GitWrite { repo: root.clone() },
                    Risk::AlwaysAsk,
                    false,
                    true,
                    // Not under "url": the agent stores that key as the result's source
                    // (with the chat, and in every later prompt), and a push URL read from
                    // the repository's config can hold a token.
                    json!({"root": root, "remote": remote, PUSH_URL_KEY: url}),
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
        // Output of the network tools can name a remote with its credentials (a URL from
        // the repository's config, or an error from git or libgit2): hide them.
        let shown = |text: String| {
            if matches!(tool, "git_clone" | "git_pull" | "git_push") {
                hide_credentials(&text)
            } else {
                text
            }
        };
        if self.fallback() {
            return match self.call_fallback(tool, args, action, &ctx).await {
                Ok(text) => {
                    let text = shown(text);
                    let note = (tool == "git_status")
                        .then(|| status_note(&text))
                        .flatten();
                    sized_output(tool, 0, text, note, &ctx)
                }
                Err(e) => ToolOutput::error(with_advice(tool, shown(e))),
            };
        }
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
        // The repository the call works in (none for a clone).
        let mut repo: Option<PathBuf> = None;
        let result = match tool {
            "git_clone" => {
                let (Some(url), Some(dest)) = (
                    p.get("url").and_then(Value::as_str),
                    p.get("dest").and_then(Value::as_str),
                ) else {
                    return ToolOutput::error("internal error: the approved action has no URL");
                };
                if let Err(why) = self.skill_check(tool, dest, &ctx.chat) {
                    return ToolOutput::error(why);
                }
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
                repo = Some(root.clone());
                // Re-check what was approved immediately before running.
                if let Err(e) = self.check_repo_config(&root.display().to_string()).await {
                    return ToolOutput::error(e);
                }
                if let Err(why) = self.skill_check(tool, &root.display().to_string(), &ctx.chat) {
                    return ToolOutput::error(why);
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
                            .get(PUSH_URL_KEY)
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        // The push URL resolved when it runs must be the one on the card.
                        match self.push_url(&root.display().to_string(), &remote).await {
                            Ok(now) if now == approved_url => {}
                            Ok(now) => {
                                let shown: Vec<String> = now.lines().map(mask_userinfo).collect();
                                return ToolOutput::error(format!(
                                    "the push URL changed after approval (now {}); nothing was pushed",
                                    shown.join(", ")
                                ));
                            }
                            Err(e) => return ToolOutput::error(e),
                        }
                        if approved_url.lines().any(|u| check_remote_url(u).is_err()) {
                            return ToolOutput::error(
                                "a push URL is not a plain https or ssh URL; nothing was pushed",
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
                let text = shown(text);
                let note = if code == 0 {
                    (tool == "git_status")
                        .then(|| status_note(&text))
                        .flatten()
                } else if mentions_conflicts(&text)
                    && let Some(root) = &repo
                {
                    // Which files: git's message does not always say.
                    let files = self.unmerged_files(root).await;
                    (!files.is_empty()).then(|| conflict_note(&files))
                } else {
                    None
                };
                sized_output(tool, code, text, note, &ctx)
            }
            Err(e) => ToolOutput::error(with_advice(tool, shown(e))),
        }
    }
}

/// Conflicted files named in one message at most.
const MAX_CONFLICTS_LISTED: usize = 10;

/// How to resolve conflicts, after the sentence that says where they are.
const RESOLVE: &str = "In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit.";

/// What to do about a failure, from what git (or the built-in library) said. One short
/// paragraph the model can act on, or `None` for a failure nothing useful is known about.
/// Nothing from `text` is repeated in it.
fn advice(tool: &str, text: &str) -> Option<String> {
    let lower = text.to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| lower.contains(n));
    let said = if has(&["not a git repository", "not inside a git repository"])
        && !has(&["does not appear to be a git repository"])
    {
        "Pass a folder inside a repository as repo. A new repository is made with git init (run_command) or git_clone.".to_string()
    } else if has(&[
        "authentication failed",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
        "permission denied (publickey",
        "needs a sign-in",
        "asked for a sign-in",
        "asked for a kind of sign-in",
    ]) {
        "The remote did not accept the sign-in. Credentials are the user's to set up (a credential helper, an access token or an SSH key); this app does not type passwords. Tell the user, and do not try again until they have.".to_string()
    } else if has(&["unmerged files", "unresolved conflict", "conflict (", "fix conflicts", "unmerged paths"]) {
        format!("There are merge conflicts. {RESOLVE} git_status lists the files.")
    } else if has(&["nothing to commit", "nothing added to commit", "no changes added to commit"]) {
        "Nothing was committed: there are no changes to commit. That is fine if the work was committed before; git_status shows the state.".to_string()
    } else if has(&["not currently on a branch", "head is detached", "head detached"]) {
        format!(
            "No branch is checked out (HEAD is detached), so {tool} has no branch to work on. Ask the user which branch to use; git switch (run_command) checks one out."
        )
    } else if has(&["no upstream branch", "no tracking information"]) {
        "The branch has no upstream on the remote yet. git_push and git_pull work on a branch that has one; the first push of a new branch is git push --set-upstream (run_command).".to_string()
    } else if has(&["not possible to fast-forward"]) {
        "The branch and its upstream have both changed, so it cannot be fast-forwarded. git_pull does nothing else: merging or rebasing is for the user to decide.".to_string()
    } else if has(&["[rejected]", "non-fast-forward", "fetch first"]) {
        "The remote has commits this branch does not have. Pull first (git_pull), then push again.".to_string()
    } else if has(&["would be overwritten by"]) {
        "Changes in the working tree are in the way. Commit them with git_commit first, then pull again.".to_string()
    } else if has(&["pathspec"]) && has(&["did not match"]) {
        "A path matches no file. Paths are relative to the repository's root folder; git_status lists the changed files.".to_string()
    } else if has(&["please tell me who you are", "author identity unknown", "committer identity unknown"]) {
        "Git does not know the author's name and e-mail. The user enters them in Settings, Git.".to_string()
    } else if has(&[
        "could not resolve host",
        "could not read from remote repository",
        "does not appear to be a git repository",
        "repository not found",
        "failed to connect",
        "connection timed out",
    ]) {
        "The remote could not be reached. Check the URL and the network connection.".to_string()
    } else {
        return None;
    };
    Some(said)
}

/// `message`, then the advice for it when there is some.
fn with_advice(tool: &str, message: String) -> String {
    match advice(tool, &message) {
        Some(more) => {
            let message = message.trim_end();
            let stop = if message.ends_with(['.', '!', '?', ')', ']']) {
                ""
            } else {
                "."
            };
            format!("{message}{stop} {more}")
        }
        None => message,
    }
}

/// Whether a failed command's output is about unresolved conflicts.
fn mentions_conflicts(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["unmerged files", "unresolved conflict", "conflict (", "fix conflicts", "unmerged paths"]
        .iter()
        .any(|n| lower.contains(n))
}

/// The files a status listing (`XY path`, as `git status --porcelain=v1` prints it) marks
/// as in conflict.
fn conflicted_in_status(status: &str) -> Vec<String> {
    status
        .lines()
        .filter_map(|line| {
            let (xy, path) = (line.get(..2)?, line.get(3..)?);
            matches!(xy, "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU").then(|| path.to_string())
        })
        .collect()
}

/// "Merge conflicts in N files: a, b." and how to resolve them. File names are data: each
/// is shown on one line with invisible characters made visible.
fn conflict_note(files: &[String]) -> String {
    let shown: Vec<String> = files
        .iter()
        .take(MAX_CONFLICTS_LISTED)
        .map(|f| policy::sanitize::one_line(f, 200))
        .collect();
    let more = match files.len().saturating_sub(shown.len()) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    format!(
        "Merge conflicts in {} file{}: {}{more}. {RESOLVE}",
        files.len(),
        if files.len() == 1 { "" } else { "s" },
        shown.join(", ")
    )
}

/// What a status listing needs said about it: conflicts, and a detached HEAD.
fn status_note(status: &str) -> Option<String> {
    let conflicts = conflicted_in_status(status);
    if !conflicts.is_empty() {
        return Some(conflict_note(&conflicts));
    }
    status
        .lines()
        .next()
        .is_some_and(|first| first.starts_with("## HEAD (no branch)"))
        .then(|| {
            "No branch is checked out (HEAD is detached): a commit made now belongs to no branch. Ask the user which branch to use; git switch (run_command) checks one out.".to_string()
        })
}

/// A tool result sized to the per-result cap, with the whole text kept for the app when
/// it is longer. `note` is what the tool adds about the output; a failure with no note
/// gets the advice for what git said.
fn sized_output(
    tool: &str,
    code: i32,
    text: String,
    note: Option<String>,
    ctx: &CallContext,
) -> ToolOutput {
    let budget = shorten::budget_bytes(ctx.chat.result_cap_tokens);
    let mut body = if text.trim().is_empty() {
        "(no output)".to_string()
    } else {
        text
    };
    let note = note.or_else(|| (code != 0).then(|| advice(tool, &body)).flatten());
    if let Some(note) = note {
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str(&format!("[{note}]"));
    }
    let full = format!("exit code {code}\n{body}");
    // A diff or a log is read from the top: most of the room goes to the first lines.
    match shorten::head_and_tail(&full, budget, 75) {
        Some(view) => shorten::shortened(view, full, code != 0, None),
        None => ToolOutput {
            is_error: code != 0,
            ..ToolOutput::text(full)
        },
    }
}

fn payload_str<'a>(p: &'a Value, k: &str) -> Option<&'a str> {
    p.get(k).and_then(Value::as_str)
}

impl GitServer {
    /// The files with unresolved conflicts in a repository (system git). Empty when they
    /// cannot be listed.
    async fn unmerged_files(&self, root: &Path) -> Vec<String> {
        let cancel = CancellationToken::new();
        let args: Vec<String> = ["diff", "--name-only", "--diff-filter=U", "--no-ext-diff", "--"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        match self.run(root, &args, &cancel, Duration::from_secs(20)).await {
            Ok((0, out)) => out
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Starts the worker for one network job.
    async fn run_worker(
        &self,
        job: Job,
        cwd: &Path,
        approved_url: &str,
        ctx: &CallContext,
    ) -> Result<String, String> {
        let Some(cmd) = &self.worker else {
            return Err(FALLBACK_NO_WORKER.into());
        };
        git_worker::run_job(git_worker::JobRun {
            cmd,
            env: self.worker_env(),
            cwd,
            job,
            token_host: git_fallback::host_key(approved_url),
            tokens: self.tokens.clone(),
            cancel: &ctx.cancel,
            timeout: NETWORK_TIMEOUT,
            progress: ctx.progress.clone(),
        })
        .await
    }

    /// The fallback's `call`: the same approved actions as the system-git path, run on
    /// libgit2. Each tool re-checks what was approved immediately before its side effect.
    async fn call_fallback(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &Action,
        ctx: &CallContext,
    ) -> Result<String, String> {
        let p = &action.payload;
        let cancel = &ctx.cancel;
        if tool == "git_clone" {
            let (Some(url), Some(dest)) = (payload_str(p, "url"), payload_str(p, "dest")) else {
                return Err("internal error: the approved action has no URL".into());
            };
            self.remote_host(url).map_err(|r| r.message().to_string())?;
            // The destination must still be inside the approved folders and empty.
            let checked = self
                .sandbox(&ctx.chat)
                .check(dest, None, &RealFs)
                .map_err(|e| e.to_string())?;
            if checked.canonical != dest {
                return Err(format!(
                    "{dest} changed after approval (it now resolves to {}); nothing was cloned",
                    checked.canonical
                ));
            }
            self.skill_check(tool, dest, &ctx.chat)?;
            let dest = PathBuf::from(dest);
            let parent = dest.parent().map(Path::to_path_buf).unwrap_or_default();
            std::fs::create_dir_all(&parent)
                .map_err(|e| format!("could not create {}: {e}", parent.display()))?;
            let job = Job::Clone {
                url: url.to_string(),
                dest: dest.display().to_string(),
            };
            let text = self.run_worker(job, &parent, url, ctx).await.map_err(|e| {
                if dest.exists() {
                    format!(
                        "{e}\nThe folder {} may hold a partial clone; delete it before cloning again.",
                        dest.display()
                    )
                } else {
                    e
                }
            })?;
            return Ok(format!("{text}\n{}", git_fallback::RESULT_NOTE));
        }
        let Some(root) = payload_str(p, "root").map(PathBuf::from) else {
            return Err("internal error: the approved action has no repository".into());
        };
        self.fallback_recheck(&ctx.chat, &root).await?;
        self.skill_check(tool, &root.display().to_string(), &ctx.chat)?;
        match tool {
            "git_status" => Self::local(cancel, move |c| git_fallback::status(&root, &c)).await,
            "git_diff" => {
                let staged = args.get("staged").and_then(Value::as_bool).unwrap_or(false);
                let path = s(args, "path").map(str::to_string);
                Self::local(cancel, move |c| {
                    git_fallback::diff(&root, staged, path.as_deref(), &c)
                })
                .await
            }
            "git_log" => {
                let n = args
                    .get("limit")
                    .and_then(Value::as_u64)
                    .unwrap_or(20)
                    .clamp(1, 100) as usize;
                let path = s(args, "path").map(str::to_string);
                Self::local(cancel, move |c| {
                    git_fallback::log(&root, n, path.as_deref(), &c)
                })
                .await
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
                let message = payload_str(p, "message").unwrap_or("").to_string();
                let env = self.config.env.clone();
                let text = Self::local(cancel, move |c| {
                    git_fallback::commit(&root, &message, &paths, &env, &c)
                })
                .await?;
                Ok(format!("{text}\n{}", git_fallback::RESULT_NOTE))
            }
            "git_pull" => {
                let at = root.clone();
                let url = Self::local(cancel, move |_| git_fallback::upstream_url(&at)).await?;
                self.remote_host(&url)
                    .map_err(|r| r.message().to_string())?;
                let job = Job::Fetch {
                    repo: root.display().to_string(),
                };
                let fetched = self.run_worker(job, &root, &url, ctx).await?;
                // The fetch can take minutes: check the repository again immediately
                // before the fast-forward writes to it.
                self.fallback_recheck(&ctx.chat, &root).await?;
                let merged =
                    Self::local(cancel, move |c| git_fallback::fast_forward(&root, &c)).await?;
                Ok(format!(
                    "{fetched}\n{merged}\n{}",
                    git_fallback::RESULT_NOTE
                ))
            }
            "git_push" => {
                let remote = payload_str(p, "remote").unwrap_or("origin").to_string();
                let approved_url = payload_str(p, PUSH_URL_KEY).unwrap_or("").to_string();
                // The push URL resolved now must be the one on the card; the worker
                // checks it once more on the object that pushes.
                let now = self.push_url(&root.display().to_string(), &remote).await?;
                if now != approved_url {
                    return Err(format!(
                        "the push URL changed after approval (now {}); nothing was pushed",
                        display_url(&now)
                    ));
                }
                self.remote_host(&approved_url)
                    .map_err(|r| format!("{}; nothing was pushed", r.message()))?;
                let job = Job::Push {
                    repo: root.display().to_string(),
                    remote,
                    expect_url: approved_url.clone(),
                };
                let text = self.run_worker(job, &root, &approved_url, ctx).await?;
                Ok(format!("{text}\n{}", git_fallback::RESULT_NOTE))
            }
            other => Err(format!("the git server has no tool named {other}")),
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

    const SAY_CREDENTIALS: &str = "The remote did not accept the sign-in. Credentials are the user's to set up (a credential helper, an access token or an SSH key); this app does not type passwords. Tell the user, and do not try again until they have.";

    /// What each failure tells the model to do next, from git's own words.
    #[test]
    fn failures_say_what_to_do_next() {
        let said = |tool: &str, text: &str| advice(tool, text);
        // Not a repository.
        assert_eq!(
            said("git_status", "fatal: not a git repository (or any of the parent directories): .git").as_deref(),
            Some("Pass a folder inside a repository as repo. A new repository is made with git init (run_command) or git_clone.")
        );
        assert_eq!(
            said("git_log", "/w/notes is not inside a git repository"),
            said("git_status", "fatal: not a git repository"),
        );
        // Nothing to commit.
        assert_eq!(
            said("git_commit", "On branch main\nnothing to commit, working tree clean").as_deref(),
            Some("Nothing was committed: there are no changes to commit. That is fine if the work was committed before; git_status shows the state.")
        );
        // Sign-in, in git's several wordings and the built-in library's.
        for text in [
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://example.com/r.git/'",
            "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
            "github.com needs a sign-in. Add an access token for github.com in the app's Git settings, then try again",
        ] {
            assert_eq!(said("git_push", text).as_deref(), Some(SAY_CREDENTIALS), "{text}");
        }
        // No branch.
        assert_eq!(
            said("git_pull", "fatal: You are not currently on a branch.").as_deref(),
            Some("No branch is checked out (HEAD is detached), so git_pull has no branch to work on. Ask the user which branch to use; git switch (run_command) checks one out.")
        );
        assert_eq!(
            said("git_push", "HEAD is detached (no branch is checked out); check out a branch first"),
            Some("No branch is checked out (HEAD is detached), so git_push has no branch to work on. Ask the user which branch to use; git switch (run_command) checks one out.".to_string())
        );
        // Conflicts, when the files are not known.
        assert_eq!(
            said("git_commit", "error: Committing is not possible because you have unmerged files.").as_deref(),
            Some("There are merge conflicts. In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit. git_status lists the files.")
        );
        // The rest of what comes up.
        assert_eq!(
            said("git_pull", "fatal: Not possible to fast-forward, aborting.").as_deref(),
            Some("The branch and its upstream have both changed, so it cannot be fast-forwarded. git_pull does nothing else: merging or rebasing is for the user to decide.")
        );
        assert_eq!(
            said("git_push", " ! [rejected]        main -> main (fetch first)").as_deref(),
            Some("The remote has commits this branch does not have. Pull first (git_pull), then push again.")
        );
        assert_eq!(
            said("git_push", "fatal: The current branch topic has no upstream branch.").as_deref(),
            Some("The branch has no upstream on the remote yet. git_push and git_pull work on a branch that has one; the first push of a new branch is git push --set-upstream (run_command).")
        );
        assert_eq!(
            said("git_pull", "error: Your local changes to the following files would be overwritten by merge:\n\ta.txt").as_deref(),
            Some("Changes in the working tree are in the way. Commit them with git_commit first, then pull again.")
        );
        assert_eq!(
            said("git_commit", "fatal: pathspec 'nope.txt' did not match any files").as_deref(),
            Some("A path matches no file. Paths are relative to the repository's root folder; git_status lists the changed files.")
        );
        assert_eq!(
            said("git_commit", "Author identity unknown\n\n*** Please tell me who you are.").as_deref(),
            Some("Git does not know the author's name and e-mail. The user enters them in Settings, Git.")
        );
        assert_eq!(
            said("git_clone", "fatal: unable to access 'https://nowhere.example/r.git/': Could not resolve host: nowhere.example").as_deref(),
            Some("The remote could not be reached. Check the URL and the network connection.")
        );
        assert_eq!(said("git_diff", "fatal: something nobody has seen before"), None);
    }

    #[test]
    fn a_status_listing_names_conflicts_and_a_detached_head() {
        let status = "## main...origin/main [ahead 1]\nUU src/a.rs\nAA b.txt\n M c.txt\nDU gone.txt\n?? new.txt\nUD \"with space.txt\"";
        assert_eq!(
            conflicted_in_status(status),
            ["src/a.rs", "b.txt", "gone.txt", "\"with space.txt\""]
        );
        assert_eq!(
            status_note(status).as_deref(),
            Some("Merge conflicts in 4 files: src/a.rs, b.txt, gone.txt, \"with space.txt\". In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit.")
        );
        assert_eq!(
            status_note("## main\nUU only.txt").as_deref(),
            Some("Merge conflicts in 1 file: only.txt. In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit.")
        );
        assert_eq!(
            status_note("## HEAD (no branch)\n M a.txt").as_deref(),
            Some("No branch is checked out (HEAD is detached): a commit made now belongs to no branch. Ask the user which branch to use; git switch (run_command) checks one out.")
        );
        assert_eq!(status_note("## main\n M a.txt\n?? b.txt"), None);
        // Many files: the first ten, and how many more.
        let many: String = (0..14).map(|i| format!("UU f{i}.txt\n")).collect();
        let note = status_note(&format!("## main\n{many}")).expect("a note");
        assert!(note.starts_with("Merge conflicts in 14 files: f0.txt, "), "{note}");
        assert!(note.contains("f9.txt and 4 more. In each file"), "{note}");
    }

    #[tokio::test]
    async fn a_folder_that_is_not_a_repository_says_what_to_pass() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        for tool in ["git_status", "git_commit"] {
            let why = go(&s, &chat(&root), tool, json!({"message": "m"}))
                .await
                .unwrap_err();
            assert_eq!(
                why,
                format!("{} is not inside a git repository. Pass a folder inside a repository as repo. A new repository is made with git init (run_command) or git_clone.", root.display())
            );
        }
    }

    #[tokio::test]
    async fn nothing_to_commit_conflicts_and_a_detached_head_are_explained() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        let c = chat(&root);
        let git = |args: &[&str]| {
            let s = s.clone();
            let root = root.clone();
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            async move {
                let cancel = CancellationToken::new();
                s.run(&root, &args, &cancel, Duration::from_secs(20)).await.unwrap()
            }
        };
        std::fs::write(root.join("a.txt"), "base\n").unwrap();
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "base"})).await.unwrap();
        assert!(!out.is_error, "{}", out.joined_text());

        // Nothing to commit.
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "again"})).await.unwrap();
        let text = out.joined_text();
        assert!(out.is_error && text.starts_with("exit code 1\n"), "{text}");
        assert!(
            text.ends_with("\n[Nothing was committed: there are no changes to commit. That is fine if the work was committed before; git_status shows the state.]"),
            "{text}"
        );

        // Two branches change the same line: the merge stops with a conflict.
        git(&["checkout", "-q", "-b", "other"]).await;
        std::fs::write(root.join("a.txt"), "theirs\n").unwrap();
        git(&["commit", "-q", "-a", "-m", "theirs"]).await;
        git(&["checkout", "-q", "main"]).await;
        std::fs::write(root.join("a.txt"), "ours\n").unwrap();
        git(&["commit", "-q", "-a", "-m", "ours"]).await;
        let (code, _) = git(&["merge", "other"]).await;
        assert_ne!(code, 0, "the merge was expected to stop");
        let (_, out) = go(&s, &c, "git_status", json!({})).await.unwrap();
        let text = out.joined_text();
        assert!(!out.is_error && text.contains("\nUU a.txt"), "{text}");
        assert!(
            text.ends_with("\n[Merge conflicts in 1 file: a.txt. In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit.]"),
            "{text}"
        );
        // A commit of another path is refused by git while a.txt is unresolved.
        std::fs::write(root.join("b.txt"), "b\n").unwrap();
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "b", "paths": ["b.txt"]})).await.unwrap();
        let text = out.joined_text();
        assert!(out.is_error, "{text}");
        assert!(
            text.ends_with("\n[Merge conflicts in 1 file: a.txt. In each file, replace the parts between <<<<<<< and >>>>>>> with the wanted text, then commit with git_commit.]"),
            "{text}"
        );
        git(&["merge", "--abort"]).await;

        // No branch checked out.
        git(&["checkout", "-q", "--detach"]).await;
        let (_, out) = go(&s, &c, "git_status", json!({})).await.unwrap();
        let text = out.joined_text();
        assert!(
            text.ends_with("\n[No branch is checked out (HEAD is detached): a commit made now belongs to no branch. Ask the user which branch to use; git switch (run_command) checks one out.]"),
            "{text}"
        );
        let (_, out) = go(&s, &c, "git_pull", json!({})).await.unwrap();
        let text = out.joined_text();
        assert!(out.is_error, "{text}");
        assert!(
            text.ends_with("\n[No branch is checked out (HEAD is detached), so git_pull has no branch to work on. Ask the user which branch to use; git switch (run_command) checks one out.]"),
            "{text}"
        );
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
        // git pushes to every push URL: the card names all of them.
        s.run(
            &root,
            &[
                "remote".into(),
                "set-url".into(),
                "--add".into(),
                "--push".into(),
                "origin".into(),
                "https://example.com/r.git".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        s.run(
            &root,
            &[
                "remote".into(),
                "set-url".into(),
                "--add".into(),
                "--push".into(),
                "origin".into(),
                "https://mirror.example/r.git".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
        let Described::Action(both) = s.describe("git_push", &a, &chat(&root)).await else {
            panic!()
        };
        assert!(
            both.preview.contains("https://example.com/r.git")
                && both.preview.contains("https://mirror.example/r.git"),
            "{}",
            both.preview
        );
        s.run(
            &root,
            &[
                "config".into(),
                "--unset-all".into(),
                "remote.origin.pushurl".into(),
            ],
            &cancel,
            Duration::from_secs(20),
        )
        .await
        .unwrap();
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

    /// The payload keys `source_of` in the agent reads for a result's source (stored with
    /// the chat and repeated in every later prompt).
    const SOURCE_KEYS: [&str; 5] = ["source", "path", "url", "canonicalPath", "canonical_path"];

    fn no_secret_in_source(act: &Action, secrets: &[&str]) {
        for key in SOURCE_KEYS {
            if let Some(v) = act.payload.get(key).and_then(Value::as_str) {
                for s in secrets {
                    assert!(!v.contains(s), "{key} holds {s}: {v}");
                }
            }
        }
    }

    /// Credentials in a push URL never reach the card, the audit preview, the result's
    /// source or an error message.
    #[tokio::test]
    async fn push_urls_never_show_credentials() {
        let Some(s) = server() else { return };
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        init_repo(&s, &root).await;
        let cancel = CancellationToken::new();
        let git = |args: &[&str]| {
            let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
            let s = s.clone();
            let root = root.clone();
            let cancel = cancel.clone();
            async move {
                s.run(&root, &args, &cancel, Duration::from_secs(20))
                    .await
                    .unwrap()
            }
        };
        git(&[
            "remote",
            "add",
            "origin",
            "https://user:ghp_secret1@example.com/r.git",
        ])
        .await;
        let a = json!({}).as_object().cloned().unwrap();
        let Described::Action(act) = s.describe("git_push", &a, &chat(&root)).await else {
            panic!()
        };
        assert!(
            act.preview.contains("https://***@example.com/r.git"),
            "{}",
            act.preview
        );
        assert!(!act.preview.contains("ghp_") && !act.summary.contains("ghp_"));
        assert!(!act.preview.contains("user:"), "{}", act.preview);
        no_secret_in_source(&act, &["ghp_"]);
        // A token used as the user name, on a second push URL.
        git(&[
            "remote",
            "set-url",
            "--add",
            "--push",
            "origin",
            "https://user:ghp_secret1@example.com/r.git",
        ])
        .await;
        git(&[
            "remote",
            "set-url",
            "--add",
            "--push",
            "origin",
            "https://ghp_tok2@mirror.example/r.git",
        ])
        .await;
        let Described::Action(both) = s.describe("git_push", &a, &chat(&root)).await else {
            panic!()
        };
        assert!(
            both.preview.contains("https://***@mirror.example/r.git"),
            "{}",
            both.preview
        );
        assert!(!both.preview.contains("ghp_") && !both.summary.contains("ghp_"));
        no_secret_in_source(&both, &["ghp_"]);
        // The URL changes after approval: the error names the new one without credentials.
        git(&["config", "--unset-all", "remote.origin.pushurl"]).await;
        git(&[
            "remote",
            "set-url",
            "origin",
            "https://ghp_new3@evil.example/r.git",
        ])
        .await;
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
        let text = out.joined_text();
        assert!(
            out.is_error && text.contains("changed after approval"),
            "{text}"
        );
        assert!(
            text.contains("evil.example") && !text.contains("ghp_"),
            "{text}"
        );
    }

    /// The system-git card shows the URL exactly as git will be given it, as before the
    /// fallback existed: git and the url crate can read a URL differently
    /// (`https://github.com\@evil.example/` reaches evil.example through git), and a
    /// card rewritten into the url crate's reading would hide that from the user. Only a
    /// user name and password are replaced by `***` (credentials are never shown, 9.2).
    #[tokio::test]
    async fn the_system_git_card_shows_the_url_as_given() {
        let s = GitServer::new(GitConfig {
            git: Some(PathBuf::from("/nonexistent/git")),
            env: vec![],
            credential_helpers: vec![],
        });
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        // A URL git reads differently from the url crate is refused outright (no card).
        let a = json!({"url": "https://github.com\\@evil.example/r.git", "destination": "c"});
        let Described::Action(act) = s
            .describe("git_clone", a.as_object().unwrap(), &chat(&root))
            .await
        else {
            panic!("not described")
        };
        assert!(
            act.hard_limit
                .as_deref()
                .is_some_and(|h| h.contains("backslash")),
            "{act:?}"
        );
        for (url, shown) in [
            (
                "https://user:pw@GitHub.com/r.git",
                "https://***@GitHub.com/r.git",
            ),
            (
                "https://ghp_x@GitHub.com/r.git",
                "https://***@GitHub.com/r.git",
            ),
            ("https://GitHub.com/r.git", "https://GitHub.com/r.git"),
        ] {
            let a = json!({"url": url, "destination": "c"});
            let Described::Action(act) = s
                .describe("git_clone", a.as_object().unwrap(), &chat(&root))
                .await
            else {
                panic!("{url} was not described")
            };
            assert!(act.hard_limit.is_none(), "{url}");
            assert!(
                act.preview.contains(&format!("git clone {shown}\n")),
                "{url}: {}",
                act.preview
            );
            assert_eq!(act.summary, format!("Clone {shown}"));
            // What runs is the URL the model wrote.
            assert_eq!(act.payload["url"], url);
        }
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
            // git connects to evil.example; the url crate would say github.com.
            "https://github.com\\@evil.example/r.git",
            "https://github.com /@evil.example/r.git",
        ] {
            assert!(check_remote_url(bad).is_err(), "{bad}");
        }
        assert_eq!(
            check_remote_url("https://user@GitHub.com:8443/a").unwrap(),
            "github.com"
        );
    }

    #[test]
    fn git_reads_the_host_after_the_last_at_sign() {
        assert_eq!(
            git_url_host("https://a.com\\@b.com/x").as_deref(),
            Some("b.com")
        );
        assert_eq!(git_url_host("ssh://git@[::1]:22/x").as_deref(), Some("::1"));
        assert_eq!(
            git_url_host("https://u:p@Host.example/x").as_deref(),
            Some("host.example")
        );
        // git and curl end the authority at the first '/', '?' or '#'; an '@' after that
        // is not a user name.
        for url in [
            "https://evil.example?@github.com/r.git",
            "https://evil.example#@github.com/r.git",
            "https://u:t@evil.example#@github.com/r.git",
        ] {
            assert_eq!(git_url_host(url).as_deref(), Some("evil.example"), "{url}");
        }
    }

    #[test]
    fn dash_values_are_rejected() {
        assert!(plain_value("path", "--output=/etc/x").is_err());
        assert!(plain_value("path", "src/main.rs").is_ok());
    }
}

/// The libgit2 fallback end to end, with `GitConfig::git = None`. Network operations run
/// in a real worker process: this test binary started again with the libtest arguments
/// that run only [`fallback_tests::worker_process_entry`], which turns it into the
/// worker. That entry is the only place local-path remotes are allowed.
#[cfg(test)]
mod fallback_tests {
    use super::*;
    use std::ffi::OsString;

    const TEST_WORKER_ENV: &str = "HARNESS_TOOLS_GIT_WORKER_TEST";

    /// A checked-out file's text with CRLF read as LF: Git for Windows sets
    /// `core.autocrlf=true` in its system config, and libgit2 honours it on checkout as git
    /// does.
    fn text(path: &std::path::Path) -> String {
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    /// Runs as an ordinary (empty) test, or as the worker when started by
    /// [`test_worker`].
    #[test]
    fn worker_process_entry() {
        if std::env::var_os(TEST_WORKER_ENV).is_some() {
            crate::git_worker::run(true);
        }
    }

    fn test_worker() -> WorkerCommand {
        WorkerCommand {
            program: std::env::current_exe().unwrap(),
            args: [
                "git::fallback_tests::worker_process_entry",
                "--exact",
                "--nocapture",
                "--test-threads=1",
                "--format=terse",
            ]
            .into_iter()
            .map(OsString::from)
            .collect(),
            env: vec![
                (TEST_WORKER_ENV.into(), "1".into()),
                // The stalled-clone test talks to 127.0.0.1 directly, never via a proxy.
                ("NO_PROXY".into(), "127.0.0.1,localhost".into()),
                ("no_proxy".into(), "127.0.0.1,localhost".into()),
            ],
        }
    }

    fn identity_env() -> Vec<(String, String)> {
        [
            ("GIT_AUTHOR_NAME", "T"),
            ("GIT_AUTHOR_EMAIL", "t@example.com"),
            ("GIT_COMMITTER_NAME", "T"),
            ("GIT_COMMITTER_EMAIL", "t@example.com"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
    }

    fn server() -> GitServer {
        let mut s = GitServer::new(GitConfig {
            git: None,
            env: identity_env(),
            credential_helpers: vec![],
        });
        s.worker = Some(test_worker());
        s.allow_local_remotes = true;
        s
    }

    fn chat(dir: &Path) -> ChatContext {
        ChatContext {
            chat_id: 1,
            approved_folders: vec![dir.display().to_string()],
            result_cap_tokens: 4000,
            ..Default::default()
        }
    }

    fn canonical_tempdir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn init(dir: &Path, bare: bool) {
        let mut opts = git2::RepositoryInitOptions::new();
        opts.initial_head("main").bare(bare);
        git2::Repository::init_opts(dir, &opts).unwrap();
    }

    fn call_ctx(c: &ChatContext) -> CallContext {
        CallContext {
            chat: c.clone(),
            cancel: CancellationToken::new(),
            progress: None,
        }
    }

    async fn describe(s: &GitServer, c: &ChatContext, tool: &str, a: &Value) -> Described {
        s.describe(tool, a.as_object().unwrap(), c).await
    }

    /// Describe, then run what was described (as if approved).
    async fn go(
        s: &GitServer,
        c: &ChatContext,
        tool: &str,
        a: Value,
    ) -> Result<(Action, ToolOutput), String> {
        match describe(s, c, tool, &a).await {
            Described::Invalid(w) => Err(w),
            Described::Action(act) if act.hard_limit.is_some() => Err(act.hard_limit.unwrap()),
            Described::Action(act) => {
                let out = s
                    .call(tool, a.as_object().unwrap(), &act, call_ctx(c))
                    .await;
                Ok((act, out))
            }
        }
    }

    async fn ok(s: &GitServer, c: &ChatContext, tool: &str, a: Value) -> (Action, String) {
        let (act, out) = go(s, c, tool, a).await.unwrap();
        assert!(!out.is_error, "{tool}: {}", out.joined_text());
        (act, out.joined_text())
    }

    #[test]
    fn the_backend_follows_git_presence() {
        assert_eq!(server().backend(), GitBackend::Libgit2);
        let sys = GitServer::new(GitConfig {
            git: Some(PathBuf::from("/usr/bin/git")),
            env: vec![],
            credential_helpers: vec![],
        });
        assert_eq!(sys.backend(), GitBackend::System);
        // Production servers never accept local-path remotes.
        assert!(!sys.allow_local_remotes);
        assert!(
            !GitServer::new(GitConfig {
                git: None,
                env: vec![],
                credential_helpers: vec![]
            })
            .allow_local_remotes
        );
    }

    #[tokio::test]
    async fn status_commit_log_and_diff() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let c = chat(&root);
        std::fs::write(root.join("a.txt"), "one\n").unwrap();

        let (act, out) = ok(&s, &c, "git_status", json!({})).await;
        assert_eq!(
            act.scope,
            Scope::GitRead {
                repo: root.display().to_string()
            }
        );
        assert!(act.read_only && !act.outbound && act.risk == Risk::Normal);
        assert!(out.contains("## No commits yet on main"), "{out}");
        assert!(out.contains("?? a.txt"), "{out}");

        // git log before the first commit is an error, as with git.
        let (_, out) = go(&s, &c, "git_log", json!({})).await.unwrap();
        assert!(out.is_error);

        let (act, out) = ok(&s, &c, "git_commit", json!({"message": "first\n\n\n"})).await;
        assert_eq!(
            act.scope,
            Scope::GitWrite {
                repo: root.display().to_string()
            }
        );
        assert!(!act.read_only && !act.outbound && act.risk == Risk::Normal);
        assert!(out.contains("[main (root-commit) "), "{out}");
        assert!(out.contains(" 1 file changed, 1 insertion(+)"), "{out}");
        assert!(out.contains("no hooks ran"), "{out}");

        let (_, out) = ok(&s, &c, "git_log", json!({})).await;
        assert!(out.contains(" T: first"), "{out}");

        std::fs::write(root.join("a.txt"), "two\n").unwrap();
        std::fs::write(root.join("b.txt"), "bee\n").unwrap();
        let (_, out) = ok(&s, &c, "git_status", json!({})).await;
        assert!(
            out.contains(" M a.txt") && out.contains("?? b.txt"),
            "{out}"
        );
        let (_, out) = ok(&s, &c, "git_diff", json!({})).await;
        assert!(out.contains("-one") && out.contains("+two"), "{out}");
        assert!(
            !out.contains("bee"),
            "untracked files are not in git diff: {out}"
        );

        // Commit only b.txt: a.txt stays modified.
        let (_, out) = ok(
            &s,
            &c,
            "git_commit",
            json!({"message": "add b", "paths": ["b.txt"]}),
        )
        .await;
        assert!(out.contains("[main "), "{out}");
        let (_, out) = ok(&s, &c, "git_status", json!({})).await;
        assert!(out.contains(" M a.txt") && !out.contains("b.txt"), "{out}");
        let (_, out) = ok(&s, &c, "git_log", json!({"path": "b.txt"})).await;
        assert!(out.contains("add b") && !out.contains("first"), "{out}");
        let (_, out) = ok(&s, &c, "git_log", json!({"limit": 1})).await;
        assert_eq!(out.lines().count(), 2, "{out}"); // "exit code 0" + one commit

        // Staged diff.
        let repo = git2::Repository::open(&root).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.txt")).unwrap();
        index.write().unwrap();
        let (_, out) = ok(&s, &c, "git_diff", json!({"staged": true})).await;
        assert!(out.contains("+two"), "{out}");

        // An unknown path and an empty commit are errors.
        let (_, out) = go(
            &s,
            &c,
            "git_commit",
            json!({"message": "x", "paths": ["nope.txt"]}),
        )
        .await
        .unwrap();
        assert!(out.is_error && out.joined_text().contains("did not match"));
        ok(&s, &c, "git_commit", json!({"message": "a two"})).await;
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "again"}))
            .await
            .unwrap();
        assert!(out.is_error && out.joined_text().contains("nothing to commit"));
    }

    #[tokio::test]
    async fn deletions_and_renames_are_committed() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let c = chat(&root);
        std::fs::write(root.join("old.txt"), "same content here\n").unwrap();
        ok(&s, &c, "git_commit", json!({"message": "one"})).await;
        std::fs::rename(root.join("old.txt"), root.join("new.txt")).unwrap();
        ok(&s, &c, "git_commit", json!({"message": "rename"})).await;
        let (_, out) = ok(&s, &c, "git_status", json!({})).await;
        assert!(
            !out.contains("old.txt") && !out.contains("new.txt"),
            "{out}"
        );
        let repo = git2::Repository::open(&root).unwrap();
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        assert!(tree.get_name("new.txt").is_some() && tree.get_name("old.txt").is_none());
    }

    #[tokio::test]
    async fn paths_managed_by_filter_drivers_are_never_committed() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let c = chat(&root);
        std::fs::write(
            root.join(".gitattributes"),
            "*.bin filter=lfs diff=lfs merge=lfs -text\n",
        )
        .unwrap();
        std::fs::write(root.join("big.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(root.join("ok.txt"), "fine\n").unwrap();
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "all"}))
            .await
            .unwrap();
        let text = out.joined_text();
        assert!(out.is_error, "{text}");
        assert!(
            text.contains("big.bin") && text.contains("Git LFS"),
            "{text}"
        );
        let repo = git2::Repository::open(&root).unwrap();
        assert!(repo.head().is_err(), "nothing may be committed");
        assert!(repo.index().unwrap().is_empty(), "nothing may stay staged");
        // Other paths can still be committed.
        let (_, out) = ok(
            &s,
            &c,
            "git_commit",
            json!({"message": "txt", "paths": ["ok.txt", ".gitattributes"]}),
        )
        .await;
        assert!(out.contains("2 files changed"), "{out}");
    }

    #[tokio::test]
    async fn hooks_never_run() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let marker = root.join("hook-ran");
        let hook = root.join(".git/hooks/pre-commit");
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::fs::write(root.join("a.txt"), "x").unwrap();
        ok(&s, &chat(&root), "git_commit", json!({"message": "m"})).await;
        assert!(!marker.exists(), "a repository hook ran");
    }

    #[tokio::test]
    async fn a_git_folder_outside_the_approved_folders_is_refused() {
        let s = server();
        let (_a, approved) = canonical_tempdir();
        let (_b, elsewhere) = canonical_tempdir();
        init(&elsewhere, false);
        let work = approved.join("repo");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(
            work.join(".git"),
            format!("gitdir: {}\n", elsewhere.join(".git").display()),
        )
        .unwrap();
        let why = go(
            &s,
            &chat(&approved),
            "git_status",
            json!({"repo": work.display().to_string()}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("outside the approved folders"), "{why}");
    }

    #[tokio::test]
    async fn push_always_asks_shows_the_url_and_rechecks_it() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let repo = git2::Repository::open(&root).unwrap();
        repo.remote("origin", "https://user:hunter2@example.com/r.git")
            .unwrap();
        let c = chat(&root);
        let Described::Action(act) = describe(&s, &c, "git_push", &json!({})).await else {
            panic!("push was not described")
        };
        assert_eq!(act.risk, Risk::AlwaysAsk);
        assert!(act.outbound);
        assert_eq!(
            act.scope,
            Scope::GitWrite {
                repo: root.display().to_string()
            }
        );
        assert!(
            act.preview.contains("https://***@example.com/r.git"),
            "{}",
            act.preview
        );
        assert!(!act.preview.contains("hunter2") && !act.summary.contains("hunter2"));
        assert!(!act.preview.contains("user:"), "{}", act.preview);
        // The result's source (stored with the chat, repeated in later prompts) is read
        // from these payload keys; none may hold the URL with its password.
        for key in ["source", "path", "url", "canonicalPath", "canonical_path"] {
            if let Some(v) = act.payload.get(key).and_then(Value::as_str) {
                assert!(!v.contains("hunter2"), "{key}: {v}");
            }
        }
        // pushInsteadOf in the repository's config changes the URL after approval:
        // nothing is pushed, and the error does not show the new URL's credentials.
        repo.config()
            .unwrap()
            .set_str(
                "url.https://ghp_evil@evil.example/.pushInsteadOf",
                "https://user:hunter2@example.com/",
            )
            .unwrap();
        let out = s.call("git_push", &Map::new(), &act, call_ctx(&c)).await;
        assert!(out.is_error);
        assert!(
            out.joined_text().contains("changed after approval"),
            "{}",
            out.joined_text()
        );
        assert!(out.joined_text().contains("evil.example"));
        assert!(
            !out.joined_text().contains("ghp_evil"),
            "{}",
            out.joined_text()
        );
    }

    /// Skill folders are written only by the skills UI: no clone into one (or into a
    /// folder that would hold one), and no commit or pull in a repository inside one.
    #[tokio::test]
    async fn the_git_tools_never_write_into_skill_folders() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        let skills = root.join("skills");
        let in_skill = skills.join("repo");
        std::fs::create_dir_all(&in_skill).unwrap();
        init(&in_skill, false);
        let around = root.join("dotfiles");
        std::fs::create_dir_all(around.join(".agents/skills")).unwrap();
        init(&around, false);
        let c = ChatContext {
            skill_roots: vec![
                skills.display().to_string(),
                around.join(".agents/skills").display().to_string(),
                root.join("home/.agents/skills").display().to_string(), // not there yet
            ],
            ..chat(&root)
        };
        let refused = |d: Described| match d {
            Described::Action(a) => a
                .hard_limit
                .is_some_and(|h| h.contains("Settings → Skills")),
            Described::Invalid(_) => false,
        };
        let url = "https://github.com/a/b.git";
        for dest in [
            skills.join("new"),
            root.join("home"),
            root.join("home/.agents"),
            root.join("home/.agents/skills/x"),
        ] {
            let a = json!({"url": url, "destination": dest.display().to_string()});
            assert!(
                refused(describe(&s, &c, "git_clone", &a).await),
                "clone into {}",
                dest.display()
            );
        }
        let repo = |p: &Path| json!({"repo": p.display().to_string()});
        for tool in ["git_commit", "git_pull"] {
            let mut a = repo(&in_skill);
            a["message"] = json!("m");
            assert!(refused(describe(&s, &c, tool, &a).await), "{tool}");
        }
        // A pull could write into a skill folder inside the repository.
        assert!(refused(describe(&s, &c, "git_pull", &repo(&around)).await));
        // Reading still works, and so does a clone elsewhere.
        let Described::Action(a) = describe(&s, &c, "git_status", &repo(&in_skill)).await else {
            panic!()
        };
        assert!(a.hard_limit.is_none(), "{a:?}");
        let a = json!({"url": url, "destination": root.join("work").display().to_string()});
        let Described::Action(a) = describe(&s, &c, "git_clone", &a).await else {
            panic!()
        };
        assert!(a.hard_limit.is_none(), "{a:?}");
    }

    /// A token used as the user name alone is hidden too.
    #[tokio::test]
    async fn push_cards_hide_a_token_used_as_the_user_name() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let repo = git2::Repository::open(&root).unwrap();
        repo.remote("origin", "https://ghp_x@example.com/r.git")
            .unwrap();
        let Described::Action(act) = describe(&s, &chat(&root), "git_push", &json!({})).await
        else {
            panic!("push was not described")
        };
        assert!(
            act.preview.contains("https://***@example.com/r.git"),
            "{}",
            act.preview
        );
        assert!(!act.preview.contains("ghp_x") && !act.summary.contains("ghp_x"));
        assert!(act.payload.get("url").is_none(), "{:?}", act.payload);
    }

    #[tokio::test]
    async fn ssh_needs_git_and_other_schemes_are_blocked() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        let c = chat(&root);
        let d = describe(
            &s,
            &c,
            "git_clone",
            &json!({"url": "git@github.com:a/b.git", "destination": "b"}),
        )
        .await;
        assert!(
            matches!(&d, Described::Invalid(w) if w.contains("https only")),
            "{d:?}"
        );
        let d = describe(
            &s,
            &c,
            "git_clone",
            &json!({"url": "http://example.com/a.git", "destination": "b"}),
        )
        .await;
        assert!(
            matches!(&d, Described::Action(a) if a.hard_limit.is_some()),
            "{d:?}"
        );
        let Described::Action(a) = describe(
            &s,
            &c,
            "git_clone",
            &json!({"url": "https://github.com/a/b.git", "destination": "b"}),
        )
        .await
        else {
            panic!()
        };
        assert_eq!(
            a.scope,
            Scope::GitClone {
                host: "github.com".into()
            }
        );
        assert!(a.outbound && a.risk == Risk::Normal && a.hard_limit.is_none());
        // A push remote over ssh is refused before anyone is asked.
        init(&root, false);
        git2::Repository::open(&root)
            .unwrap()
            .remote("origin", "git@github.com:a/b.git")
            .unwrap();
        let d = describe(&s, &c, "git_push", &json!({})).await;
        assert!(
            matches!(&d, Described::Invalid(w) if w.contains("https only")),
            "{d:?}"
        );
    }

    /// `https://github.com\@127.0.0.1:<port>/r.git`: the url crate reads the host as
    /// github.com (so a github.com clone grant would cover it and a github.com token
    /// would be handed over), while libgit2 connects to 127.0.0.1. The fallback refuses
    /// it before anyone is asked, re-checks it before running, and the worker never
    /// connects.
    #[tokio::test]
    async fn urls_libgit2_reads_differently_are_refused() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let s = server();
        let (_d, top) = canonical_tempdir();
        let c = chat(&top);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        let url = format!("https://github.com\\@127.0.0.1:{port}/r.git");
        let a = json!({"url": url, "destination": "c"});
        // An action approved for this URL (forged here) is refused before the worker
        // starts.
        let dest = top.join("c").display().to_string();
        let act = Action {
            server: "git".into(),
            tool: "git_clone".into(),
            summary: String::new(),
            preview: String::new(),
            scope: Scope::GitClone {
                host: "github.com".into(),
            },
            risk: Risk::Normal,
            read_only: false,
            outbound: true,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: json!({"url": url, "dest": dest}),
        };
        let out = s
            .call("git_clone", a.as_object().unwrap(), &act, call_ctx(&c))
            .await;
        assert!(out.is_error, "{}", out.joined_text());
        // Give a stray connection time to arrive before checking that none did.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            connections.load(Ordering::SeqCst),
            0,
            "the worker connected to 127.0.0.1 for a URL approved as github.com: {}",
            out.joined_text()
        );
        // And no card is offered for it.
        let d = describe(&s, &c, "git_clone", &a).await;
        assert!(
            matches!(&d, Described::Invalid(w) if w.contains("standard form"))
                || matches!(&d, Described::Action(act) if act.hard_limit.is_some()),
            "{d:?}"
        );
        // A remote configured with such a URL cannot be pushed to or pulled from.
        let work = top.join("work");
        std::fs::create_dir_all(&work).unwrap();
        init(&work, false);
        let repo = git2::Repository::open(&work).unwrap();
        repo.remote("origin", &url).unwrap();
        std::fs::write(work.join("f.txt"), "x\n").unwrap();
        let w = json!({"repo": work.display().to_string()});
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "m"}),
        )
        .await;
        let d = describe(&s, &c, "git_push", &w).await;
        assert!(
            matches!(&d, Described::Invalid(w) if w.contains("standard form") || w.contains("backslash")),
            "{d:?}"
        );
        let mut cfg = repo.config().unwrap();
        cfg.set_str("branch.main.remote", "origin").unwrap();
        cfg.set_str("branch.main.merge", "refs/heads/main").unwrap();
        let (_, out) = go(&s, &c, "git_pull", w.clone()).await.unwrap();
        assert!(
            out.is_error
                && (out.joined_text().contains("standard form")
                    || out.joined_text().contains("backslash")),
            "{}",
            out.joined_text()
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(
            connections.load(Ordering::SeqCst),
            0,
            "the worker connected"
        );
    }

    /// git refuses to fetch into the checked-out branch; libgit2 does not. A fetch
    /// refspec such as `+refs/heads/*:refs/heads/*` would move the branch under an
    /// unchanged working tree, and the next "commit all" would undo the remote's work.
    #[tokio::test]
    async fn pull_refuses_to_fetch_into_the_checked_out_branch() {
        let s = server();
        let (_d, top) = canonical_tempdir();
        let c = chat(&top);
        let bare = top.join("bare.git");
        init(&bare, true);
        let work = top.join("work");
        std::fs::create_dir_all(&work).unwrap();
        init(&work, false);
        git2::Repository::open(&work)
            .unwrap()
            .remote("origin", &bare.display().to_string())
            .unwrap();
        let w = json!({"repo": work.display().to_string()});
        std::fs::write(work.join("f.txt"), "v1\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "v1"}),
        )
        .await;
        ok(&s, &c, "git_push", w.clone()).await;
        ok(
            &s,
            &c,
            "git_clone",
            json!({"url": bare.display().to_string(), "destination": "clone"}),
        )
        .await;
        let clone = top.join("clone");
        let clone_repo = git2::Repository::open(&clone).unwrap();
        let before = clone_repo.refname_to_id("refs/heads/main").unwrap();
        clone_repo
            .config()
            .unwrap()
            .set_str("remote.origin.fetch", "+refs/heads/*:refs/heads/*")
            .unwrap();
        std::fs::write(work.join("f.txt"), "v2\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "v2"}),
        )
        .await;
        ok(&s, &c, "git_push", w.clone()).await;
        let (_, out) = go(
            &s,
            &c,
            "git_pull",
            json!({"repo": clone.display().to_string()}),
        )
        .await
        .unwrap();
        assert!(
            out.is_error && out.joined_text().contains("checked-out branch"),
            "{}",
            out.joined_text()
        );
        assert_eq!(
            clone_repo.refname_to_id("refs/heads/main").unwrap(),
            before,
            "the checked-out branch moved"
        );
        assert_eq!(text(&clone.join("f.txt")), "v1\n");
    }

    /// libgit2 takes the proxy from the repository's own config before the environment,
    /// and on Windows answers a proxy's NTLM/Negotiate challenge with the user's Windows
    /// credentials. A repository cannot route the fallback's traffic: pull and push
    /// refuse, and nothing connects to the proxy.
    #[tokio::test]
    async fn the_repository_cannot_choose_a_proxy() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let s = server();
        let (_d, top) = canonical_tempdir();
        let c = chat(&top);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let connections = Arc::new(AtomicUsize::new(0));
        let seen = connections.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                seen.fetch_add(1, Ordering::SeqCst);
                drop(conn);
            }
        });
        let work = top.join("work");
        std::fs::create_dir_all(&work).unwrap();
        init(&work, false);
        let repo = git2::Repository::open(&work).unwrap();
        repo.remote("origin", "https://example.com/r.git").unwrap();
        let w = json!({"repo": work.display().to_string()});
        std::fs::write(work.join("f.txt"), "x\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "m"}),
        )
        .await;
        let mut cfg = repo.config().unwrap();
        cfg.set_str("branch.main.remote", "origin").unwrap();
        cfg.set_str("branch.main.merge", "refs/heads/main").unwrap();
        for key in [
            "remote.origin.proxy",
            "http.https://example.com/.proxy",
            "http.proxy",
        ] {
            cfg.set_str(key, &format!("http://127.0.0.1:{port}"))
                .unwrap();
            for tool in ["git_push", "git_pull"] {
                let (_, out) = go(&s, &c, tool, w.clone()).await.unwrap();
                assert!(
                    out.is_error && out.joined_text().contains("proxy"),
                    "{key} {tool}: {}",
                    out.joined_text()
                );
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            assert_eq!(
                connections.load(Ordering::SeqCst),
                0,
                "{key}: the worker connected to the repository's proxy"
            );
            cfg.remove(key).unwrap();
        }
    }

    /// Filter drivers run only on file content, so removing a file a driver manages
    /// (an LFS-tracked file, say) can be committed without one, as with git.
    #[tokio::test]
    async fn deleting_a_filter_managed_file_can_be_committed() {
        let s = server();
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let c = chat(&root);
        std::fs::write(root.join(".gitattributes"), "*.bin filter=lfs -text\n").unwrap();
        // A pointer file committed earlier by real git (here: straight through libgit2).
        std::fs::write(
            root.join("big.bin"),
            "version https://git-lfs.github.com/spec/v1\n",
        )
        .unwrap();
        {
            let repo = git2::Repository::open(&root).unwrap();
            let mut index = repo.index().unwrap();
            index.add_path(Path::new(".gitattributes")).unwrap();
            index.add_path(Path::new("big.bin")).unwrap();
            index.write().unwrap();
            let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
            let sig = git2::Signature::now("T", "t@example.com").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "lfs", &tree, &[])
                .unwrap();
        }
        std::fs::remove_file(root.join("big.bin")).unwrap();
        let (_, out) = ok(&s, &c, "git_commit", json!({"message": "drop big.bin"})).await;
        assert!(out.contains("1 file changed"), "{out}");
        let repo = git2::Repository::open(&root).unwrap();
        let tree = repo.head().unwrap().peel_to_tree().unwrap();
        assert!(tree.get_name("big.bin").is_none());
        // Changing a managed file is still refused.
        std::fs::write(root.join("other.bin"), [1u8, 2, 3]).unwrap();
        let (_, out) = go(&s, &c, "git_commit", json!({"message": "add"}))
            .await
            .unwrap();
        assert!(
            out.is_error && out.joined_text().contains("other.bin"),
            "{}",
            out.joined_text()
        );
    }

    #[tokio::test]
    async fn without_a_worker_only_local_tools_run() {
        let s = GitServer::new(GitConfig {
            git: None,
            env: identity_env(),
            credential_helpers: vec![],
        });
        let (_d, root) = canonical_tempdir();
        init(&root, false);
        let c = chat(&root);
        ok(&s, &c, "git_status", json!({})).await;
        for (tool, a) in [
            ("git_pull", json!({})),
            ("git_push", json!({})),
            (
                "git_clone",
                json!({"url": "https://github.com/a/b.git", "destination": "b"}),
            ),
        ] {
            let d = describe(&s, &c, tool, &a).await;
            assert!(
                matches!(&d, Described::Invalid(w) if w.contains("no worker")),
                "{tool}: {d:?}"
            );
        }
    }

    #[tokio::test]
    async fn clone_pull_and_push_through_the_worker() {
        let s = server();
        let (_d, top) = canonical_tempdir();
        let c = chat(&top);
        let bare = top.join("bare.git");
        init(&bare, true);
        let work = top.join("work");
        std::fs::create_dir_all(&work).unwrap();
        init(&work, false);
        git2::Repository::open(&work)
            .unwrap()
            .remote("origin", &bare.display().to_string())
            .unwrap();
        let w = json!({"repo": work.display().to_string()});

        std::fs::write(work.join("f.txt"), "v1\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "v1"}),
        )
        .await;
        let (act, out) = ok(&s, &c, "git_push", w.clone()).await;
        assert_eq!(act.risk, Risk::AlwaysAsk);
        assert!(
            out.contains("[new branch]") && out.contains("main -> main"),
            "{out}"
        );
        let bare_repo = git2::Repository::open_bare(&bare).unwrap();
        assert!(bare_repo.find_reference("refs/heads/main").is_ok());

        let (act, out) = ok(
            &s,
            &c,
            "git_clone",
            json!({"url": bare.display().to_string(), "destination": "clone"}),
        )
        .await;
        assert_eq!(
            act.scope,
            Scope::GitClone {
                host: "local".into()
            }
        );
        assert!(
            out.contains("Cloned ") && out.contains("(branch main)"),
            "{out}"
        );
        let clone = top.join("clone");
        assert_eq!(text(&clone.join("f.txt")), "v1\n");
        // Cloning into a folder that is no longer empty is refused before anyone asks.
        let d = describe(
            &s,
            &c,
            "git_clone",
            &json!({"url": bare.display().to_string(), "destination": "clone"}),
        )
        .await;
        assert!(
            matches!(&d, Described::Invalid(w) if w.contains("not empty")),
            "{d:?}"
        );

        let cl = json!({"repo": clone.display().to_string()});
        let (act, out) = ok(&s, &c, "git_pull", cl.clone()).await;
        assert_eq!(
            act.scope,
            Scope::GitWrite {
                repo: clone.display().to_string()
            }
        );
        assert!(act.outbound);
        assert!(out.contains("Already up to date."), "{out}");

        std::fs::write(work.join("f.txt"), "v2\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "v2"}),
        )
        .await;
        let (_, out) = ok(&s, &c, "git_push", w.clone()).await;
        assert!(out.contains("..") && out.contains("main -> main"), "{out}");
        let (_, out) = ok(&s, &c, "git_status", w.clone()).await;
        assert!(
            out.contains("## main...origin/main") || out.contains("## main"),
            "{out}"
        );

        let (_, out) = ok(&s, &c, "git_pull", cl.clone()).await;
        assert!(out.contains("Fast-forward"), "{out}");
        assert_eq!(text(&clone.join("f.txt")), "v2\n");
        let (_, out) = ok(&s, &c, "git_status", cl.clone()).await;
        assert!(
            out.starts_with("exit code 0\n## main...origin/main"),
            "{out}"
        );
        assert!(!out.contains("[ahead") && !out.contains("[behind"), "{out}");

        // Diverged history: pull refuses and changes nothing.
        std::fs::write(clone.join("g.txt"), "local\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": clone.display().to_string(), "message": "local"}),
        )
        .await;
        std::fs::write(work.join("f.txt"), "v3\n").unwrap();
        ok(
            &s,
            &c,
            "git_commit",
            json!({"repo": work.display().to_string(), "message": "v3"}),
        )
        .await;
        ok(&s, &c, "git_push", w.clone()).await;
        let (_, out) = go(&s, &c, "git_pull", cl.clone()).await.unwrap();
        assert!(
            out.is_error && out.joined_text().contains("Not possible to fast-forward"),
            "{}",
            out.joined_text()
        );
        assert_eq!(text(&clone.join("f.txt")), "v2\n");
        // And a non-fast-forward push is rejected.
        let (_, out) = go(&s, &c, "git_push", cl.clone()).await.unwrap();
        assert!(out.is_error, "{}", out.joined_text());
    }

    /// A clone whose server accepts the connection and never answers the TLS handshake:
    /// libgit2 has no cancel for that, so Stop must kill the worker (4.2).
    #[tokio::test]
    async fn stop_kills_a_stalled_clone() {
        use std::io::Read;
        use std::sync::Mutex;
        let s = server();
        let (_d, top) = canonical_tempdir();
        let c = chat(&top);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let held: Arc<Mutex<Vec<std::net::TcpStream>>> = Arc::default();
        let keep = held.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming().flatten() {
                keep.lock().unwrap().push(conn);
            }
        });
        let a = json!({"url": format!("https://127.0.0.1:{port}/r.git"), "destination": "stalled"});
        let Described::Action(act) = describe(&s, &c, "git_clone", &a).await else {
            panic!("clone was not described")
        };
        assert!(act.hard_limit.is_none());
        let cancel = CancellationToken::new();
        let stopped_at: Arc<Mutex<Option<std::time::Instant>>> = Arc::default();
        let (stopper, watch, mark) = (cancel.clone(), held.clone(), stopped_at.clone());
        tokio::spawn(async move {
            // Press Stop once the worker is connected and waiting.
            while watch.lock().unwrap().is_empty() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            *mark.lock().unwrap() = Some(std::time::Instant::now());
            stopper.cancel();
        });
        let ctx = CallContext {
            chat: c.clone(),
            cancel,
            progress: None,
        };
        let out = tokio::time::timeout(
            Duration::from_secs(30),
            s.call("git_clone", a.as_object().unwrap(), &act, ctx),
        )
        .await
        .expect("the clone did not stop");
        let took = stopped_at
            .lock()
            .unwrap()
            .expect("Stop was pressed")
            .elapsed();
        assert!(out.is_error, "{}", out.joined_text());
        assert!(
            out.joined_text().contains(git_fallback::STOPPED),
            "{}",
            out.joined_text()
        );
        assert!(took < Duration::from_secs(1), "Stop took {took:?}");
        // The worker is gone: its end of the connection is closed.
        let mut conn = held.lock().unwrap().remove(0);
        conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut buf = [0u8; 4096];
        loop {
            match conn.read(&mut buf) {
                Ok(0) => break,
                Ok(_) => continue, // the ClientHello sent before the stall
                Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
                Err(e) => panic!("the worker still holds the connection: {e}"),
            }
        }
    }

    #[cfg(unix)]
    fn sh(script: &str) -> WorkerCommand {
        WorkerCommand {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env: vec![],
        }
    }

    #[cfg(unix)]
    async fn run_fake(
        cmd: &WorkerCommand,
        token_host: Option<&str>,
        tokens: Option<Arc<dyn TokenSource>>,
        cancel: &CancellationToken,
    ) -> Result<String, String> {
        let cwd = std::env::temp_dir();
        git_worker::run_job(git_worker::JobRun {
            cmd,
            env: runtime::EnvSpec::minimal(),
            cwd: &cwd,
            job: Job::Fetch { repo: "/r".into() },
            token_host: token_host.map(str::to_string),
            tokens,
            cancel,
            timeout: Duration::from_secs(20),
            progress: None,
        })
        .await
    }

    /// A fake worker that asks for a token for github.com and reports what it got.
    #[cfg(unix)]
    const ASKS_FOR_A_TOKEN: &str = r#"read job
printf '%s\n' 'stray output is ignored'
printf '%s\n' '@@harness-git-worker@@ {"kind":"need_token","host":"github.com"}'
read reply
case "$reply" in *s3cret*) r=got;; *'"token":null'*) r=none;; *) r=other;; esac
printf '@@harness-git-worker@@ {"kind":"done","text":"%s"}\n' "$r""#;

    #[cfg(unix)]
    #[tokio::test]
    async fn tokens_reach_the_worker_only_for_the_approved_host() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let asked = Arc::new(AtomicUsize::new(0));
        let counter = asked.clone();
        let tokens: Arc<dyn TokenSource> = Arc::new(move |host: &str| {
            counter.fetch_add(1, Ordering::SeqCst);
            (host == "github.com").then(|| "s3cret".to_string())
        });
        let cmd = sh(ASKS_FOR_A_TOKEN);
        let cancel = CancellationToken::new();
        let got = run_fake(&cmd, Some("github.com"), Some(tokens.clone()), &cancel).await;
        assert_eq!(got.as_deref(), Ok("got"));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // The call was approved for another host: the keychain is not even read.
        let got = run_fake(&cmd, Some("gitlab.com"), Some(tokens.clone()), &cancel).await;
        assert_eq!(got.as_deref(), Ok("none"));
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // No token source at all.
        let got = run_fake(&cmd, Some("github.com"), None, &cancel).await;
        assert_eq!(got.as_deref(), Ok("none"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_kills_the_worker_at_once() {
        let (_d, dir) = canonical_tempdir();
        let pid_file = dir.join("pid");
        let cmd = sh(&format!(
            "echo $$ > '{}'; exec sleep 30",
            pid_file.display()
        ));
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let started = std::time::Instant::now();
        let watch = pid_file.clone();
        tokio::spawn(async move {
            while !watch.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            stopper.cancel();
        });
        let got = run_fake(&cmd, None, None, &cancel).await;
        assert_eq!(got, Err(git_fallback::STOPPED.to_string()));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        let pid: i32 = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only checks whether the process exists.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "the worker survived Stop");
    }

    /// The fetch of a pull can take minutes. If the repository's git folder is moved
    /// outside the approved folders meanwhile (here by a fake worker), the in-process
    /// fast-forward that follows must not write there.
    #[cfg(unix)]
    #[tokio::test]
    async fn pull_rechecks_the_repository_before_the_fast_forward() {
        let (_a, approved) = canonical_tempdir();
        let (_b, elsewhere) = canonical_tempdir();
        let work = approved.join("work");
        std::fs::create_dir_all(&work).unwrap();
        init(&work, false);
        let repo = git2::Repository::open(&work).unwrap();
        repo.remote("origin", "https://example.com/r.git").unwrap();
        let mut cfg = repo.config().unwrap();
        cfg.set_str("branch.main.remote", "origin").unwrap();
        cfg.set_str("branch.main.merge", "refs/heads/main").unwrap();
        // main at v1, and an "already fetched" origin/main at v2: a real fast-forward.
        let sig = git2::Signature::now("T", "t@example.com").unwrap();
        let commit = |parents: &[&git2::Commit<'_>], content: &str| {
            let blob = repo.blob(content.as_bytes()).unwrap();
            let mut tb = repo.treebuilder(None).unwrap();
            tb.insert("f.txt", blob, 0o100644).unwrap();
            let tree = repo.find_tree(tb.write().unwrap()).unwrap();
            repo.commit(None, &sig, &sig, content, &tree, parents)
                .unwrap()
        };
        let v1 = repo.find_commit(commit(&[], "v1\n")).unwrap();
        let v2 = commit(&[&v1], "v2\n");
        repo.reference("refs/heads/main", v1.id(), true, "v1")
            .unwrap();
        repo.reference("refs/remotes/origin/main", v2, true, "v2")
            .unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .unwrap();
        let moved = elsewhere.join("moved.git");
        let mut s = server();
        s.worker = Some(sh(&format!(
            "read job; mv .git '{m}'; printf 'gitdir: %s\\n' '{m}' > .git; printf '%s\\n' '@@harness-git-worker@@ {{\"kind\":\"done\",\"text\":\"fetched\"}}'",
            m = moved.display()
        )));
        let (_, out) = go(
            &s,
            &chat(&approved),
            "git_pull",
            json!({"repo": work.display().to_string()}),
        )
        .await
        .unwrap();
        assert!(moved.exists(), "the fake worker did not run");
        assert!(
            out.is_error && out.joined_text().contains("outside the approved folders"),
            "{}",
            out.joined_text()
        );
        let moved_repo = git2::Repository::open(&moved).unwrap();
        assert_eq!(
            moved_repo.refname_to_id("refs/heads/main").unwrap(),
            v1.id(),
            "the fast-forward wrote outside the approved folders"
        );
        assert_eq!(text(&work.join("f.txt")), "v1\n");
    }

    /// Nothing is started once Stop has been pressed.
    #[tokio::test]
    async fn no_worker_starts_after_stop() {
        let cmd = WorkerCommand {
            program: PathBuf::from("/nonexistent/worker"),
            args: vec![],
            env: vec![],
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let cwd = std::env::temp_dir();
        let got = git_worker::run_job(git_worker::JobRun {
            cmd: &cmd,
            env: runtime::EnvSpec::minimal(),
            cwd: &cwd,
            job: Job::Fetch { repo: "/r".into() },
            token_host: None,
            tokens: None,
            cancel: &cancel,
            timeout: Duration::from_secs(20),
            progress: None,
        })
        .await;
        assert_eq!(got, Err(git_fallback::STOPPED.to_string()));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_worker_that_dies_is_reported() {
        let cmd = sh("read job; echo 'boom' >&2; exit 3");
        let err = run_fake(&cmd, None, None, &CancellationToken::new())
            .await
            .unwrap_err();
        assert!(err.contains("exit code 3") && err.contains("boom"), "{err}");
    }
}
