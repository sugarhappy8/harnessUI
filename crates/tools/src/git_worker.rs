//! libgit2 network operations in a worker process (ARCHITECTURE.md 4, 4.2 and 9.3).
//!
//! When system git is missing, `git_clone`, the fetch half of `git_pull` and `git_push`
//! run through libgit2. A stalled transfer has no cancel inside libgit2, so they run in
//! the app's own binary started with [`WORKER_FLAG`] through the runtime spawn module,
//! and Stop or the timeout kills the worker's process tree.
//!
//! Protocol, one JSON object per line:
//!
//! - parent → worker (stdin): one [`Job`], then one [`TokenReply`] for every
//!   `need_token` the worker sends;
//! - worker → parent (stdout): [`WorkerMsg`] lines, each carrying [`LINE_PREFIX`] so any
//!   stray output is ignored; the last one is `done` or `failed`.
//!
//! The access token never appears on a command line or in the environment. The worker
//! asks for it only when a server answers 401, and names the host it is talking to; the
//! parent hands a token over only for the host of the URL the user approved. Off-site
//! redirects are refused (`RemoteRedirect::None`), because the WinHTTP transport on
//! Windows names the original URL to the credential callback even after a redirect.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mcp_host::ProgressFn;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::git::TokenSource;
use crate::git_fallback::{self, display_url, fallback_remote, host_key};

/// Hidden command-line flag that turns the app binary into the git worker.
pub const WORKER_FLAG: &str = "--internal-git-worker";

/// Marks protocol lines on the worker's stdout.
pub(crate) const LINE_PREFIX: &str = "@@harniss-git-worker@@ ";
/// Longest protocol line read in either direction.
const MAX_LINE: usize = 256 * 1024;
/// Stderr kept for the error message when the worker dies.
const STDERR_TAIL: usize = 16 * 1024;
/// Progress lines sent at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(300);

/// One network operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub(crate) enum Job {
    /// Clone `url` into `dest` (absolute; must not exist or be empty).
    Clone { url: String, dest: String },
    /// Fetch the upstream remote of the current branch of the repository at `repo`.
    Fetch { repo: String },
    /// Push the current branch to `remote`, whose push URL must still be `expect_url`.
    Push {
        repo: String,
        remote: String,
        expect_url: String,
    },
}

/// Worker → parent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum WorkerMsg {
    Progress { text: String },
    NeedToken { host: String },
    Done { text: String },
    Failed { text: String },
}

/// Parent → worker, answering `need_token`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TokenReply {
    pub token: Option<String>,
}

impl std::fmt::Debug for TokenReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenReply")
            .field("token", &self.token.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

/// How to start the worker: the app binary with [`WORKER_FLAG`] in production.
#[derive(Debug, Clone)]
pub(crate) struct WorkerCommand {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Extra variables for the worker (tests only use this).
    pub env: Vec<(String, String)>,
}

impl WorkerCommand {
    pub(crate) fn app(app_exe: PathBuf) -> Self {
        Self {
            program: app_exe,
            args: vec![WORKER_FLAG.into()],
            env: Vec::new(),
        }
    }
}

/// Longest text one message carries, in bytes. JSON escapes a control character as six
/// bytes, so even the worst case stays under [`MAX_LINE`].
const MAX_TEXT: usize = 32 * 1024;

/// `text` cut to [`MAX_TEXT`] bytes at a character boundary.
fn capped(text: &str) -> String {
    if text.len() <= MAX_TEXT {
        return text.to_string();
    }
    let mut cut = MAX_TEXT;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}\n[... cut ...]", &text[..cut])
}

/// One protocol line as the worker writes it. Texts are capped so that a long message (a
/// server's error text can be up to a pkt-line long) still fits one line the parent reads.
pub(crate) fn encode_line(msg: &WorkerMsg) -> String {
    let msg = match msg {
        WorkerMsg::Progress { text } => WorkerMsg::Progress { text: capped(text) },
        WorkerMsg::Done { text } => WorkerMsg::Done { text: capped(text) },
        WorkerMsg::Failed { text } => WorkerMsg::Failed { text: capped(text) },
        WorkerMsg::NeedToken { host } => WorkerMsg::NeedToken { host: capped(host) },
    };
    let json = serde_json::to_string(&msg).unwrap_or_else(|_| {
        r#"{"kind":"failed","text":"internal error: unencodable message"}"#.into()
    });
    format!("{LINE_PREFIX}{json}\n")
}

/// Parses a line from the worker's stdout; anything without the prefix is ignored. The
/// prefix may follow other text on the same line (the test harness prints a test name
/// without a newline before the worker starts).
pub(crate) fn decode_line(line: &str) -> Option<WorkerMsg> {
    let at = line.find(LINE_PREFIX)?;
    serde_json::from_str(line[at + LINE_PREFIX.len()..].trim_end()).ok()
}

/// Whether the token the worker asked for may be handed over: only for the host the
/// user approved (the host of the clone URL, the push URL or the upstream's URL).
pub(crate) fn may_send_token(requested: &str, approved: Option<&str>) -> bool {
    match approved {
        Some(a) => !a.is_empty() && a.eq_ignore_ascii_case(requested),
        None => false,
    }
}

// ---------------------------------------------------------------------------------------
// Parent side.
// ---------------------------------------------------------------------------------------

/// What the parent needs to run one job.
pub(crate) struct JobRun<'a> {
    pub cmd: &'a WorkerCommand,
    pub env: runtime::EnvSpec,
    pub cwd: &'a Path,
    pub job: Job,
    /// The only host a token may be handed over for.
    pub token_host: Option<String>,
    pub tokens: Option<Arc<dyn TokenSource>>,
    pub cancel: &'a CancellationToken,
    pub timeout: Duration,
    pub progress: Option<ProgressFn>,
}

/// Runs one job in a worker process and returns its result text. Stop and the timeout
/// kill the worker's process tree (Job Object / process group) right away.
pub(crate) async fn run_job(run: JobRun<'_>) -> Result<String, String> {
    if run.cancel.is_cancelled() {
        // Nothing starts once Stop has been pressed.
        return Err(git_fallback::STOPPED.to_string());
    }
    let mut env = run.env;
    for (k, v) in &run.cmd.env {
        env = env.set(k.clone(), v.clone());
    }
    let spec = runtime::SpawnSpec::new(&run.cmd.program, run.cwd)
        .args(run.cmd.args.iter().cloned())
        .env(env)
        .stdin(runtime::StdioMode::Piped)
        .stdout(runtime::StdioMode::Piped)
        .stderr(runtime::StdioMode::Piped);
    let mut child =
        runtime::spawn(spec).map_err(|e| format!("the git worker could not start: {e}"))?;
    let (Some(mut stdin), Some(stdout)) = (child.take_stdin(), child.take_stdout()) else {
        let _ = child.kill().await;
        return Err("the git worker could not start: no pipes".into());
    };
    let stderr_task = child.take_stderr().map(|mut s| {
        tokio::spawn(async move {
            let mut kept = Vec::new();
            let _ = (&mut s)
                .take(STDERR_TAIL as u64)
                .read_to_end(&mut kept)
                .await;
            let _ = tokio::io::copy(&mut s, &mut tokio::io::sink()).await;
            kept
        })
    });
    let job_line = match serde_json::to_string(&run.job) {
        Ok(j) => j + "\n",
        Err(e) => {
            let _ = child.kill().await;
            return Err(format!("internal error: {e}"));
        }
    };
    let token_host = run.token_host;
    let tokens = run.tokens;
    let progress = run.progress;
    let work = async {
        let mut refused: Option<String> = None;
        if stdin.write_all(job_line.as_bytes()).await.is_err() || stdin.flush().await.is_err() {
            return (None, refused, child.wait().await);
        }
        let mut reader = tokio::io::BufReader::new(stdout);
        let mut outcome: Option<Result<String, String>> = None;
        loop {
            let mut buf = Vec::new();
            match (&mut reader)
                .take(MAX_LINE as u64)
                .read_until(b'\n', &mut buf)
                .await
            {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let Some(msg) = decode_line(&String::from_utf8_lossy(&buf)) else {
                continue;
            };
            match msg {
                WorkerMsg::Progress { text } => {
                    if let Some(p) = &progress {
                        p(policy::sanitize::one_line(&text, 300));
                    }
                }
                WorkerMsg::NeedToken { host } => {
                    let token = if may_send_token(&host, token_host.as_deref()) {
                        match &tokens {
                            Some(source) => {
                                let source = source.clone();
                                let h = host.clone();
                                tokio::task::spawn_blocking(move || source.token_for(&h))
                                    .await
                                    .ok()
                                    .flatten()
                                    .filter(|t| !t.is_empty())
                            }
                            None => None,
                        }
                    } else {
                        refused = Some(host);
                        None
                    };
                    let mut line = serde_json::to_string(&TokenReply { token })
                        .unwrap_or_else(|_| r#"{"token":null}"#.into());
                    line.push('\n');
                    if stdin.write_all(line.as_bytes()).await.is_err()
                        || stdin.flush().await.is_err()
                    {
                        break;
                    }
                }
                WorkerMsg::Done { text } => outcome = Some(Ok(text)),
                WorkerMsg::Failed { text } => outcome = Some(Err(text)),
            }
        }
        (outcome, refused, child.wait().await)
    };
    let result = tokio::select! {
        r = work => Ok(r),
        _ = run.cancel.cancelled() => Err(git_fallback::STOPPED.to_string()),
        _ = tokio::time::sleep(run.timeout) => Err(format!(
            "git did not finish within {} s",
            run.timeout.as_secs()
        )),
    };
    let (outcome, refused, status) = match result {
        Ok(r) => r,
        Err(why) => {
            // Kills the whole tree at once; no graceful wait (4.2).
            let _ = child.kill().await;
            return Err(why);
        }
    };
    let refused_note = |text: String| match &refused {
        Some(h) => format!(
            "{text}\n(The server at {h} asked for a sign-in, but this call was approved for {}; no token was sent.)",
            token_host.as_deref().unwrap_or("no host")
        ),
        None => text,
    };
    match outcome {
        Some(Ok(text)) => Ok(text),
        Some(Err(text)) => Err(refused_note(text)),
        None => {
            let tail = match stderr_task {
                Some(t) => t.await.unwrap_or_default(),
                None => Vec::new(),
            };
            let tail = String::from_utf8_lossy(&tail);
            let code = status
                .ok()
                .and_then(|s| s.code())
                .map_or_else(|| "killed".to_string(), |c| c.to_string());
            let mut msg = format!("the git worker stopped unexpectedly (exit code {code})");
            if !tail.trim().is_empty() {
                msg.push_str(": ");
                msg.push_str(&policy::sanitize::one_line(tail.trim(), 500));
            }
            Err(refused_note(msg))
        }
    }
}

// ---------------------------------------------------------------------------------------
// Worker side.
// ---------------------------------------------------------------------------------------

/// The worker's entry point (`app --internal-git-worker`): reads one job from stdin,
/// runs it with libgit2 and reports on stdout. Never returns. Exit code 0 on success,
/// 1 when the job failed, 2 when no valid job arrived.
pub fn worker_main() -> ! {
    run(false)
}

/// `allow_local` permits local-path remotes; only the crate's own tests pass `true`.
pub(crate) fn run(allow_local: bool) -> ! {
    let code = match read_stdin_line()
        .and_then(|l| serde_json::from_str::<Job>(l.trim()).map_err(|e| format!("bad job: {e}")))
    {
        Ok(job) => match execute(&job, allow_local) {
            Ok(text) => {
                emit(&WorkerMsg::Done { text });
                0
            }
            Err(text) => {
                emit(&WorkerMsg::Failed { text });
                1
            }
        },
        Err(text) => {
            emit(&WorkerMsg::Failed { text });
            2
        }
    };
    std::process::exit(code)
}

fn emit(msg: &WorkerMsg) {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(encode_line(msg).as_bytes());
    let _ = out.flush();
}

fn read_stdin_line() -> Result<String, String> {
    use std::io::{BufRead, Read};
    let stdin = std::io::stdin();
    let mut lock = stdin.lock();
    let mut buf = Vec::new();
    (&mut lock)
        .take(MAX_LINE as u64)
        .read_until(b'\n', &mut buf)
        .map_err(|e| format!("could not read from the app: {e}"))?;
    if buf.is_empty() {
        return Err("the app closed the connection".into());
    }
    String::from_utf8(buf).map_err(|_| "the app sent invalid text".into())
}

fn execute(job: &Job, allow_local: bool) -> Result<String, String> {
    match job {
        Job::Clone { url, dest } => clone(url, Path::new(dest), allow_local),
        Job::Fetch { repo } => fetch(Path::new(repo), allow_local),
        Job::Push {
            repo,
            remote,
            expect_url,
        } => push(Path::new(repo), remote, expect_url, allow_local),
    }
}

/// State shared by one operation's libgit2 callbacks.
struct Session {
    token_requests: Cell<u32>,
    last_progress: Cell<Option<Instant>>,
    received: Cell<(usize, usize)>,
    fetched: RefCell<Vec<String>>,
    pushed: RefCell<Vec<String>>,
    rejected: RefCell<Vec<String>>,
}

impl Session {
    fn new() -> Self {
        Self {
            token_requests: Cell::new(0),
            last_progress: Cell::new(None),
            received: Cell::new((0, 0)),
            fetched: RefCell::new(Vec::new()),
            pushed: RefCell::new(Vec::new()),
            rejected: RefCell::new(Vec::new()),
        }
    }

    fn progress(&self, text: String, force: bool) {
        let now = Instant::now();
        let due = self
            .last_progress
            .get()
            .is_none_or(|t| now.duration_since(t) >= PROGRESS_EVERY);
        if force || due {
            self.last_progress.set(Some(now));
            emit(&WorkerMsg::Progress { text });
        }
    }

    fn credentials(
        &self,
        url: &str,
        user: Option<&str>,
        allowed: git2::CredentialType,
    ) -> Result<git2::Cred, git2::Error> {
        let host = host_key(url).unwrap_or_else(|| "the server".into());
        if !allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
            return Err(git2::Error::from_str(&format!(
                "{host} asked for a kind of sign-in the built-in Git library does not support"
            )));
        }
        let n = self.token_requests.get();
        self.token_requests.set(n + 1);
        if n > 0 {
            return Err(git2::Error::from_str(&format!(
                "{host} rejected the access token stored for it; replace the token in the app's Git settings"
            )));
        }
        emit(&WorkerMsg::NeedToken { host: host.clone() });
        let reply = read_stdin_line()
            .ok()
            .and_then(|l| serde_json::from_str::<TokenReply>(l.trim()).ok());
        match reply.and_then(|r| r.token).filter(|t| !t.is_empty()) {
            Some(token) => {
                let user = user.filter(|u| !u.is_empty()).unwrap_or("x-access-token");
                git2::Cred::userpass_plaintext(user, &token)
            }
            None => Err(git2::Error::from_str(&format!(
                "{host} needs a sign-in. Add an access token for {host} in the app's Git settings, then try again"
            ))),
        }
    }

    fn callbacks(&self) -> git2::RemoteCallbacks<'_> {
        let mut cb = git2::RemoteCallbacks::new();
        cb.credentials(|url, user, allowed| self.credentials(url, user, allowed));
        cb.transfer_progress(|p| {
            self.received
                .set((p.received_objects(), p.received_bytes()));
            let total = p.total_objects().max(1);
            let done = p.received_objects() == p.total_objects();
            let text = if done && p.total_deltas() > 0 {
                format!(
                    "Resolving deltas: {}/{}",
                    p.indexed_deltas(),
                    p.total_deltas()
                )
            } else {
                format!(
                    "Receiving objects: {}% ({}/{}), {}",
                    p.received_objects() * 100 / total,
                    p.received_objects(),
                    p.total_objects(),
                    human_bytes(p.received_bytes())
                )
            };
            self.progress(text, false);
            true
        });
        cb.sideband_progress(|bytes| {
            // Remote text is untrusted: one escaped line per message.
            let text = String::from_utf8_lossy(bytes);
            if let Some(line) = text
                .split(['\r', '\n'])
                .rev()
                .map(str::trim)
                .find(|l| !l.is_empty())
            {
                self.progress(
                    policy::sanitize::one_line(&format!("remote: {line}"), 200),
                    false,
                );
            }
            true
        });
        cb.update_tips(|refname, old, new| {
            let line = if old.is_zero() {
                format!(" * [new]   {} -> {refname}", short(new))
            } else {
                format!("   {}..{}  {refname}", short(old), short(new))
            };
            self.fetched.borrow_mut().push(line);
            true
        });
        cb
    }
}

fn short(oid: git2::Oid) -> String {
    oid.to_string().chars().take(7).collect()
}

fn human_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", n as f64 / 1024.0)
    }
}

fn proxy() -> git2::ProxyOptions<'static> {
    let mut p = git2::ProxyOptions::new();
    p.auto();
    p
}

fn fetch_options(s: &Session) -> git2::FetchOptions<'_> {
    let mut fo = git2::FetchOptions::new();
    fo.remote_callbacks(s.callbacks());
    fo.proxy_options(proxy());
    fo.follow_redirects(git2::RemoteRedirect::None);
    fo
}

/// Refuses a repository whose own config names a proxy (`remote.<name>.proxy`,
/// `http.proxy`, `http.<url>.proxy`). libgit2's automatic proxy reads those before the
/// environment, and on Windows WinHTTP answers a proxy's NTLM/Negotiate challenge with
/// the user's Windows credentials. The user's own (global and system) settings and the
/// app's proxy variables still apply.
fn refuse_repo_proxy(repo: &git2::Repository) -> Result<(), String> {
    let Ok(cfg) = repo.config() else {
        return Ok(());
    };
    let Ok(mut entries) = cfg.entries(Some(r"^(http|remote)\..*proxy$")) else {
        return Ok(());
    };
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(gerr)?;
        if matches!(
            entry.level(),
            git2::ConfigLevel::Local | git2::ConfigLevel::Worktree
        ) {
            return Err(format!(
                "this repository's own config sets a proxy ({}). The built-in Git library does not use a proxy chosen by a repository; remove that setting, or set the proxy in your own Git settings file (.gitconfig) or in the system's proxy settings.",
                policy::sanitize::one_line(entry.name().unwrap_or("a proxy setting"), 200)
            ));
        }
    }
    Ok(())
}

fn check_url(url: &str, allow_local: bool) -> Result<(), String> {
    fallback_remote(url, allow_local)
        .map(|_| ())
        .map_err(|r| r.message().to_string())
}

fn gerr(e: git2::Error) -> String {
    let msg = e.message().to_string();
    if msg.contains("cannot redirect from") {
        format!(
            "{msg}. The built-in Git library does not follow redirects to another host; use the new address directly"
        )
    } else {
        msg
    }
}

fn received_line(s: &Session) -> String {
    let (objects, bytes) = s.received.get();
    if objects == 0 {
        String::new()
    } else {
        format!("\nReceived {objects} objects ({})", human_bytes(bytes))
    }
}

fn clone(url: &str, dest: &Path, allow_local: bool) -> Result<String, String> {
    check_url(url, allow_local)?;
    if dest.exists()
        && std::fs::read_dir(dest)
            .map(|mut d| d.next().is_some())
            .unwrap_or(true)
    {
        return Err(format!(
            "{} already exists and is not empty",
            dest.display()
        ));
    }
    let s = Session::new();
    let mut builder = git2::build::RepoBuilder::new();
    builder.fetch_options(fetch_options(&s));
    let repo = builder.clone(url, dest).map_err(gerr)?;
    let mut text = format!("Cloned {} into {}", display_url(url), dest.display());
    if let Ok(head) = repo.head()
        && let Ok(b) = head.shorthand()
    {
        text.push_str(&format!(" (branch {b})"));
    }
    text.push_str(&received_line(&s));
    if git_fallback::uses_filter_drivers(dest) {
        text.push('\n');
        text.push_str(git_fallback::FILTER_NOTE);
    }
    Ok(text)
}

fn fetch(root: &Path, allow_local: bool) -> Result<String, String> {
    let repo = git_fallback::open_at(root)?;
    refuse_repo_proxy(&repo)?;
    let branch = git_fallback::current_branch(&repo)?;
    let remote_name = git_fallback::upstream_remote(&repo, &branch)?;
    let mut remote = repo
        .find_remote(&remote_name)
        .map_err(|_| format!("the remote {remote_name} does not exist"))?;
    let url = remote
        .url()
        .map_err(|_| format!("the remote {remote_name} has an unreadable URL"))?
        .to_string();
    check_url(&url, allow_local)?;
    // git refuses to fetch into the checked-out branch; libgit2 does not. A refspec such
    // as `+refs/heads/*:refs/heads/*` (repository config) would move the branch under an
    // unchanged working tree, and the next "commit all" would undo the remote's work.
    if remote
        .refspecs()
        .any(|r| r.direction() == git2::Direction::Fetch && r.dst_matches(&branch))
    {
        return Err(format!(
            "refusing to fetch into the checked-out branch {}: the fetch refspec of the remote {remote_name} writes to it. Nothing was fetched; fix remote.{remote_name}.fetch in the repository's config.",
            branch.strip_prefix("refs/heads/").unwrap_or(&branch)
        ));
    }
    let s = Session::new();
    let mut fo = fetch_options(&s);
    remote
        .fetch::<&str>(&[], Some(&mut fo), None)
        .map_err(gerr)?;
    let mut text = format!("From {}", display_url(&url));
    let fetched = s.fetched.borrow();
    if fetched.is_empty() {
        text.push_str("\n(nothing new)");
    } else {
        for l in fetched.iter().take(50) {
            text.push('\n');
            text.push_str(l);
        }
    }
    text.push_str(&received_line(&s));
    Ok(text)
}

fn push(
    root: &Path,
    remote_name: &str,
    expect_url: &str,
    allow_local: bool,
) -> Result<String, String> {
    let repo = git_fallback::open_at(root)?;
    refuse_repo_proxy(&repo)?;
    let mut remote = repo
        .find_remote(remote_name)
        .map_err(|_| format!("the remote {remote_name} does not exist"))?;
    // Resolved once, from the same object that pushes: what runs is what was shown.
    let url = git_fallback::resolved_push_url(&remote)
        .ok_or_else(|| format!("the remote {remote_name} has no URL"))?;
    if url != expect_url {
        return Err(format!(
            "the push URL changed after approval (now {}); nothing was pushed",
            display_url(&url)
        ));
    }
    check_url(&url, allow_local)?;
    let branch = git_fallback::current_branch(&repo)?;
    let refspec = format!("{branch}:{branch}");
    let s = Session::new();
    let mut cb = s.callbacks();
    cb.push_negotiation(|updates| {
        for u in updates {
            let src = u.src_refname().unwrap_or("?");
            let dst = u.dst_refname().unwrap_or("?");
            let name = |r: &str| r.strip_prefix("refs/heads/").unwrap_or(r).to_string();
            let line = if u.src().is_zero() {
                format!(" * [new branch]      {} -> {}", name(src), name(dst))
            } else {
                format!(
                    "   {}..{}  {} -> {}",
                    short(u.src()),
                    short(u.dst()),
                    name(src),
                    name(dst)
                )
            };
            s.pushed.borrow_mut().push(line);
        }
        Ok(())
    });
    cb.push_update_reference(|refname, status| {
        if let Some(why) = status {
            s.rejected.borrow_mut().push(policy::sanitize::one_line(
                &format!("{refname}: {why}"),
                300,
            ));
        }
        Ok(())
    });
    cb.push_transfer_progress(|current, total, bytes| {
        s.progress(
            format!(
                "Writing objects: {}/{} ({})",
                current,
                total,
                human_bytes(bytes)
            ),
            false,
        );
    });
    let mut po = git2::PushOptions::new();
    po.remote_callbacks(cb);
    po.proxy_options(proxy());
    po.follow_redirects(git2::RemoteRedirect::None);
    remote
        .push(&[refspec.as_str()], Some(&mut po))
        .map_err(gerr)?;
    let rejected = s.rejected.borrow();
    if !rejected.is_empty() {
        return Err(format!(
            "the remote rejected the push:\n{}",
            rejected.join("\n")
        ));
    }
    let pushed = s.pushed.borrow();
    let body = if pushed.is_empty() {
        "Everything up-to-date".to_string()
    } else {
        pushed.join("\n")
    };
    Ok(format!("To {}\n{body}", display_url(&url)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_round_trip_and_stray_output_is_ignored() {
        let msg = WorkerMsg::NeedToken {
            host: "github.com".into(),
        };
        let line = encode_line(&msg);
        assert!(line.starts_with(LINE_PREFIX) && line.ends_with('\n'));
        assert_eq!(decode_line(&line), Some(msg.clone()));
        // The test harness prints a name without a newline first.
        assert_eq!(decode_line(&format!("test x::y ... {line}")), Some(msg));
        assert_eq!(decode_line("running 1 test\n"), None);
        assert_eq!(decode_line(&format!("{LINE_PREFIX}not json\n")), None);
        assert_eq!(
            decode_line(&format!(
                "{LINE_PREFIX}{{\"kind\":\"done\",\"text\":\"ok\"}}"
            )),
            Some(WorkerMsg::Done { text: "ok".into() })
        );
    }

    /// A server's error text can be up to a pkt-line long, and JSON escapes each control
    /// character as six bytes: the line must still fit what the parent reads.
    #[test]
    fn long_texts_still_fit_one_protocol_line() {
        for text in ["\u{1}".repeat(70_000), "é".repeat(300_000)] {
            let line = encode_line(&WorkerMsg::Failed { text });
            assert!(line.len() <= MAX_LINE, "{} bytes", line.len());
            assert!(matches!(
                decode_line(&line),
                Some(WorkerMsg::Failed { text }) if text.ends_with("[... cut ...]")
            ));
        }
        let short = WorkerMsg::Done { text: "ok".into() };
        assert_eq!(decode_line(&encode_line(&short)), Some(short));
    }

    #[test]
    fn jobs_serialise_with_their_op() {
        let job = Job::Push {
            repo: "/r".into(),
            remote: "origin".into(),
            expect_url: "https://h/r.git".into(),
        };
        let s = serde_json::to_string(&job).unwrap();
        assert!(s.contains(r#""op":"push""#), "{s}");
        assert_eq!(serde_json::from_str::<Job>(&s).unwrap(), job);
    }

    #[test]
    fn tokens_go_only_to_the_approved_host() {
        assert!(may_send_token("github.com", Some("github.com")));
        assert!(may_send_token("GitHub.com", Some("github.com")));
        assert!(!may_send_token("evil.example", Some("github.com")));
        assert!(!may_send_token("github.com:8443", Some("github.com")));
        assert!(!may_send_token("github.com", None));
        assert!(!may_send_token("", Some("")));
    }

    #[test]
    fn token_reply_debug_hides_the_token() {
        let r = TokenReply {
            token: Some("ghp_secret".into()),
        };
        assert!(!format!("{r:?}").contains("ghp_secret"));
    }

    #[test]
    fn the_production_command_is_the_app_with_the_flag() {
        let c = WorkerCommand::app(PathBuf::from("/Applications/H.app/Contents/MacOS/h"));
        assert_eq!(c.args, vec![OsString::from(WORKER_FLAG)]);
        assert!(c.env.is_empty());
    }
}
