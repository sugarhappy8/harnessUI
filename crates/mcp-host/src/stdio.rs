//! Starting stdio servers (ARCHITECTURE.md 4.1, 4.2, 9.2 "Spawn", "Missing runtime",
//! "Environment").
//!
//! - **Command:** resolved to an absolute path with [`runtime::spawn::resolve_program`],
//!   against the login-shell `PATH` the app captured (macOS) or the app's own. On macOS the
//!   usual tool folders (Homebrew, `~/.local/bin`, `~/.cargo/bin`, Volta) are searched
//!   after it, and Apple's `/usr/bin/python3`, `/usr/bin/pip3` and `/usr/bin/git` stubs are
//!   never run unless the developer tools are installed (a bare command name is looked up
//!   past them, so a Homebrew install later in `PATH` still works). A missing `npx`/`node`,
//!   `uvx`/`uv`/`python` or `docker` is reported as the runtime the user must install.
//! - **Windows shims:** a resolved `.cmd`/`.bat` runs through `cmd.exe` with the strict
//!   quoting of [`crate::winquote`].
//! - **Environment:** the runtime crate's minimal allowlist plus the app's extra names,
//!   taken from the captured environment when there is one, the lookup `PATH`, then the
//!   config's `env` after secret resolution. Never the app's whole environment.
//! - **Process:** built with [`runtime::spawn::command_wrap`] (Job Object with
//!   kill-on-close and no console window on Windows, its own process group on Unix),
//!   registered with the reaper, and owned by a supervisor task that kills the whole tree
//!   on request, when its handle is dropped, or when the task is dropped.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::Arc;
use std::time::Duration;

use process_wrap::tokio::ChildWrapper;
use runtime::spawn::{EnvSpec, SpawnSpec, StdioMode};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, watch};

use crate::error::{HostError, MissingRuntime};
use crate::host::HostOptions;
use crate::secrets;
use crate::stderr::StderrLog;
use crate::winquote;

/// Everything needed to start one stdio server, with secrets already resolved.
pub(crate) struct Prepared {
    /// What the spawn module starts.
    pub spec: SpawnSpec,
    /// Windows batch shims: the command line passed to `cmd.exe` as one raw argument.
    pub raw_args: Option<String>,
    /// The resolved program, for diagnostics.
    pub program: PathBuf,
    /// The command as configured (after unwrapping `cmd /c`), before resolution.
    pub command: String,
    /// The `PATH` the command was looked up in.
    pub search: OsString,
}

/// Resolves the command, arguments, environment and working folder of a stdio server.
pub(crate) fn prepare(
    command: &str,
    args: &[String],
    env: &BTreeMap<String, String>,
    cwd: Option<&str>,
    opts: &HostOptions,
) -> Result<Prepared, HostError> {
    let resolver = opts.secret_resolver.as_ref();
    // Windows: `"command": "cmd", "args": ["/c", "npx", ...]` (a common Claude Desktop
    // form) runs the inner command directly, so a shim goes through the strict quoting
    // below rather than through Rust's generic argument quoting into cmd.exe.
    let unwrapped = if cfg!(windows) {
        unwrap_cmd_c(command, args)
    } else {
        None
    };
    let (command, args): (&str, &[String]) = match &unwrapped {
        Some((inner, rest)) => (inner.as_str(), rest.as_slice()),
        None => (command, args),
    };
    let args: Vec<String> = args
        .iter()
        .map(|a| secrets::resolve(a, resolver))
        .collect::<Result<_, _>>()?;
    let mut env_values = BTreeMap::new();
    for (key, value) in env {
        env_values.insert(key.clone(), secrets::resolve(value, resolver)?);
    }
    let cwd = working_folder(cwd, opts)?;
    let search = lookup_path(opts);
    let program = resolve_command(command, &search, &cwd)?;
    let env = build_env(opts, &search, &env_values);
    let mut spec = SpawnSpec::new(program.clone(), cwd)
        .env(env)
        .stdin(StdioMode::Piped)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped);
    let mut raw_args = None;
    let program_text = program.to_string_lossy().into_owned();
    if cfg!(windows) && winquote::is_batch_file(&program_text) {
        let comspec = comspec(opts);
        let line = winquote::cmd_command_line(&program_text, &args)?;
        spec.program = comspec;
        raw_args = Some(line);
    } else {
        spec = spec.args(args);
    }
    Ok(Prepared {
        spec,
        raw_args,
        program,
        command: command.trim().to_owned(),
        search,
    })
}

/// `cmd /c <program> <args...>` (optionally with `/d`, `/s` or `/q` before `/c`) as
/// `(<program>, <args...>)`. `None` for anything else, including a program token with
/// spaces or quotes, which would need cmd.exe's own parsing.
pub(crate) fn unwrap_cmd_c(command: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    let file = command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(command)
        .to_ascii_lowercase();
    if file != "cmd" && file != "cmd.exe" {
        return None;
    }
    let mut rest = args.iter();
    loop {
        let switch = rest.next()?.to_ascii_lowercase();
        match switch.as_str() {
            "/d" | "/s" | "/q" => continue,
            "/c" => break,
            _ => return None,
        }
    }
    let inner = rest.next()?;
    if inner.is_empty() || inner.contains(char::is_whitespace) || inner.contains('"') {
        return None;
    }
    Some((inner.clone(), rest.cloned().collect()))
}

/// The configured working folder (absolute), or the app's default, the home folder, or
/// the temporary folder.
fn working_folder(cwd: Option<&str>, opts: &HostOptions) -> Result<PathBuf, HostError> {
    if let Some(cwd) = cwd {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(HostError::InvalidConfig(format!(
                "the working folder \"{}\" must be an absolute path",
                policy::sanitize::one_line(cwd, 120)
            )));
        }
        if !path.is_dir() {
            return Err(HostError::InvalidConfig(format!(
                "the working folder \"{}\" does not exist",
                policy::sanitize::one_line(cwd, 120)
            )));
        }
        return Ok(path);
    }
    if let Some(dir) = &opts.default_cwd {
        return Ok(dir.clone());
    }
    let home_var = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    let home = env_value(opts, home_var).map(PathBuf::from);
    Ok(home
        .filter(|h| h.is_absolute() && h.is_dir())
        .unwrap_or_else(std::env::temp_dir))
}

/// A variable from the captured environment, or else from the app's own.
fn env_value(opts: &HostOptions, name: &str) -> Option<String> {
    if let Some(captured) = &opts.captured_env
        && let Some(value) = lookup(captured, name)
    {
        return Some(value.clone());
    }
    std::env::var(name).ok()
}

fn lookup<'a>(map: &'a BTreeMap<String, String>, name: &str) -> Option<&'a String> {
    if cfg!(windows) {
        map.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    } else {
        map.get(name)
    }
}

/// The `PATH` used to find commands, also given to the child so `#!/usr/bin/env node`
/// shebangs inside shims resolve the same way.
pub(crate) fn lookup_path(opts: &HostOptions) -> OsString {
    let base: OsString = match &opts.captured_env {
        Some(captured) => lookup(captured, "PATH")
            .map(OsString::from)
            .unwrap_or_default(),
        None => std::env::var_os("PATH").unwrap_or_default(),
    };
    extend_with_tool_folders(base, env_value(opts, "HOME").as_deref())
}

/// macOS: appends the folders package managers install into, when missing. Apps started
/// from Finder get a minimal `PATH`, and a login shell may not set all of these.
fn extend_with_tool_folders(base: OsString, home: Option<&str>) -> OsString {
    if !cfg!(target_os = "macos") {
        return base;
    }
    let mut entries: Vec<PathBuf> = std::env::split_paths(&base).collect();
    let mut extra: Vec<PathBuf> = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    if let Some(home) = home {
        let home = Path::new(home);
        extra.push(home.join(".local/bin"));
        extra.push(home.join(".cargo/bin"));
        extra.push(home.join(".volta/bin"));
    }
    for dir in extra {
        if !entries.contains(&dir) {
            entries.push(dir);
        }
    }
    std::env::join_paths(entries).unwrap_or(base)
}

/// Finds the program for `command`, or explains what is missing.
fn resolve_command(command: &str, search: &OsStr, cwd: &Path) -> Result<PathBuf, HostError> {
    let command = command.trim();
    let as_path = Path::new(command);
    // A relative path with a folder part ("build/server") is taken relative to the
    // configured working folder; the current directory of the app is never searched.
    if !as_path.is_absolute() && as_path.components().count() > 1 {
        let joined = cwd.join(as_path);
        if joined.is_file() {
            return Ok(joined);
        }
        return Err(HostError::CommandNotFound {
            command: command.to_owned(),
        });
    }
    let search = (!search.is_empty()).then_some(search);
    match runtime::spawn::resolve_program(command, search) {
        Some(path) => Ok(path),
        None => Err(match MissingRuntime::for_command(command) {
            Some(runtime) => HostError::MissingRuntime {
                runtime,
                command: command.to_owned(),
            },
            None => HostError::CommandNotFound {
                command: command.to_owned(),
            },
        }),
    }
}

/// The child's environment. See the module documentation.
pub(crate) fn build_env(
    opts: &HostOptions,
    search: &OsStr,
    config_env: &BTreeMap<String, String>,
) -> EnvSpec {
    let mut names: Vec<String> = EnvSpec::minimal().inherit;
    for extra in &opts.env_allowlist_extra {
        let duplicate = names.iter().any(|n| {
            if cfg!(windows) {
                n.eq_ignore_ascii_case(extra)
            } else {
                n == extra
            }
        });
        if !duplicate && !extra.is_empty() {
            names.push(extra.clone());
        }
    }
    let mut spec = EnvSpec::empty();
    for name in names {
        match opts.captured_env.as_ref().and_then(|c| lookup(c, &name)) {
            Some(value) => spec = spec.set(name, value.clone()),
            None => spec = spec.inherit(name),
        }
    }
    if let Some(path) = search.to_str().filter(|p| !p.is_empty()) {
        spec = spec.set("PATH", path);
    }
    for (key, value) in config_env {
        spec = spec.set(key.clone(), value.clone());
    }
    spec
}

/// Windows: the `cmd.exe` that runs batch shims (`ComSpec`, else under `SystemRoot`).
fn comspec(opts: &HostOptions) -> PathBuf {
    if let Some(path) = env_value(opts, "ComSpec").map(PathBuf::from)
        && path.is_absolute()
    {
        return path;
    }
    let root = env_value(opts, "SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    Path::new(&root).join("System32").join("cmd.exe")
}

/// macOS: Apple's stubs that open the developer-tools installer instead of running.
pub(crate) fn is_developer_tools_stub(program: &Path) -> bool {
    matches!(
        program.to_str(),
        Some("/usr/bin/python3" | "/usr/bin/pip3" | "/usr/bin/git")
    )
}

/// macOS: Apple's stubs are never run when the developer tools are missing. For a bare
/// command name (`python3`) the rest of the search path is tried first, so a Homebrew or
/// python.org install later in `PATH` is used instead of failing; a stub configured by its
/// full path is refused. Elsewhere, no-op.
pub(crate) async fn check_developer_tools(prepared: &mut Prepared) -> Result<(), HostError> {
    if !cfg!(target_os = "macos")
        || !is_developer_tools_stub(&prepared.program)
        || developer_tools_installed().await
    {
        return Ok(());
    }
    if let Some(alternative) = alternative_to_stub(&prepared.command, &prepared.search) {
        prepared.spec.program = alternative.clone();
        prepared.program = alternative;
        return Ok(());
    }
    Err(HostError::DeveloperToolsMissing {
        command: prepared.program.display().to_string(),
    })
}

/// The first program named `command` in `search` that is not one of Apple's stubs
/// (`/usr/bin` is skipped). `None` for a command given with a folder.
fn alternative_to_stub(command: &str, search: &OsStr) -> Option<PathBuf> {
    if Path::new(command).components().count() != 1 {
        return None;
    }
    let rest: Vec<PathBuf> = std::env::split_paths(search)
        .filter(|dir| dir != Path::new("/usr/bin"))
        .collect();
    let rest = std::env::join_paths(rest).ok()?;
    runtime::spawn::resolve_program(command, Some(&rest))
        .filter(|program| !is_developer_tools_stub(program))
}

/// Whether Apple's command line tools (or Xcode) are installed: the tools folder exists,
/// or `xcode-select -p` succeeds and names a folder that holds git. Checked once.
async fn developer_tools_installed() -> bool {
    static INSTALLED: tokio::sync::OnceCell<bool> = tokio::sync::OnceCell::const_new();
    *INSTALLED
        .get_or_init(|| async {
            if Path::new("/Library/Developer/CommandLineTools/usr/bin/git").is_file() {
                return true;
            }
            let spec = SpawnSpec::new("/usr/bin/xcode-select", "/")
                .arg("-p")
                .stdout(StdioMode::Piped)
                .stderr(StdioMode::Null);
            let Ok(mut child) = runtime::spawn::spawn(spec) else {
                return false;
            };
            let mut output = String::new();
            if let Some(mut stdout) = child.take_stdout() {
                use tokio::io::AsyncReadExt;
                let mut limited = (&mut stdout).take(4096);
                let _ = tokio::time::timeout(
                    Duration::from_secs(5),
                    limited.read_to_string(&mut output),
                )
                .await;
            }
            let status = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
            let ok = matches!(status, Ok(Ok(s)) if s.success());
            ok && Path::new(output.trim()).join("usr/bin/git").is_file()
        })
        .await
}

// ---------------------------------------------------------------------------------------
// The child process and its supervisor
// ---------------------------------------------------------------------------------------

/// How a server process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExitInfo {
    /// `exited with code 3`, `was stopped by signal 9`, …
    pub description: String,
}

impl ExitInfo {
    fn from_status(status: io::Result<ExitStatus>) -> Self {
        let description = match status {
            Ok(status) => match status.code() {
                Some(code) => format!("exited with code {code}"),
                None => signal_description(&status),
            },
            Err(e) => format!("ended, but its exit status could not be read ({e})"),
        };
        Self { description }
    }
}

#[cfg(unix)]
fn signal_description(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match status.signal() {
        Some(signal) => format!("was stopped by signal {signal}"),
        None => "ended".into(),
    }
}

#[cfg(not(unix))]
fn signal_description(_status: &ExitStatus) -> String {
    "ended".into()
}

/// A running server process. Dropping it kills the whole tree.
pub(crate) struct ChildProcess {
    pid: Option<u32>,
    kill: mpsc::UnboundedSender<()>,
    exit: watch::Receiver<Option<ExitInfo>>,
    stderr_done: watch::Receiver<bool>,
}

impl ChildProcess {
    /// The process id (on Unix also the process-group id).
    pub(crate) fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Kills the whole tree without waiting.
    pub(crate) fn kill(&self) {
        let _ = self.kill.send(());
    }

    /// Waits up to `timeout` for the tree to end.
    pub(crate) async fn wait_exit(&self, timeout: Duration) -> Option<ExitInfo> {
        let mut rx = self.exit.clone();
        match tokio::time::timeout(timeout, rx.wait_for(Option::is_some)).await {
            Ok(Ok(info)) => info.clone(),
            _ => self.exit.borrow().clone(),
        }
    }

    /// Waits up to `timeout` until everything the server wrote to stderr has been read.
    pub(crate) async fn wait_stderr(&self, timeout: Duration) {
        let mut rx = self.stderr_done.clone();
        let _ = tokio::time::timeout(timeout, rx.wait_for(|done| *done)).await;
    }

    /// A receiver that gets the exit information once the process has ended.
    pub(crate) fn exit_watch(&self) -> watch::Receiver<Option<ExitInfo>> {
        self.exit.clone()
    }
}

/// Starts the prepared server. Returns the process handle and the pipes rmcp talks over;
/// stderr is drained into `log`.
pub(crate) fn spawn(
    prepared: &Prepared,
    log: Arc<StderrLog>,
) -> io::Result<(ChildProcess, ChildStdout, ChildStdin)> {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut wrap = runtime::spawn::command_wrap(&prepared.spec);
    #[cfg(windows)]
    if let Some(raw) = &prepared.raw_args {
        wrap.command_mut().raw_arg(raw);
    }
    #[cfg(not(windows))]
    let _ = &prepared.raw_args;
    runtime::spawn::init_parent_error_mode();
    let child = wrap.spawn()?;
    let pid = child.id();
    let mut guard = Guard {
        child,
        pid,
        exited: false,
        reaper_pgid: None,
    };
    guard.register_with_reaper();
    let stdin = guard
        .child
        .stdin()
        .take()
        .ok_or_else(|| io::Error::other("the server's stdin was not piped"))?;
    let stdout = guard
        .child
        .stdout()
        .take()
        .ok_or_else(|| io::Error::other("the server's stdout was not piped"))?;
    let (stderr_tx, stderr_rx) = watch::channel(false);
    match guard.child.stderr().take() {
        Some(stderr) => {
            tokio::spawn(async move {
                log.drain(stderr).await;
                let _ = stderr_tx.send(true);
            });
        }
        None => {
            let _ = stderr_tx.send(true);
        }
    }
    let (kill_tx, kill_rx) = mpsc::unbounded_channel();
    let (exit_tx, exit_rx) = watch::channel(None);
    tokio::spawn(supervise(guard, kill_rx, exit_tx));
    Ok((
        ChildProcess {
            pid,
            kill: kill_tx,
            exit: exit_rx,
            stderr_done: stderr_rx,
        },
        stdout,
        stdin,
    ))
}

/// Owns the child until its tree has ended. Kills it on a request, or when every
/// [`ChildProcess`] sender is gone.
async fn supervise(
    mut guard: Guard,
    mut kill: mpsc::UnboundedReceiver<()>,
    exit: watch::Sender<Option<ExitInfo>>,
) {
    let status = tokio::select! {
        status = guard.child.wait() => status,
        _ = kill.recv() => {
            if let Err(error) = guard.child.start_kill() {
                tracing::warn!(pid = ?guard.pid, %error, "could not kill an MCP server's process tree");
            }
            guard.child.wait().await
        }
    };
    guard.exited = true;
    guard.after_exit();
    let _ = exit.send(Some(ExitInfo::from_status(status)));
}

/// The child itself. Kills the tree when dropped before the child was seen to exit.
struct Guard {
    child: Box<dyn ChildWrapper>,
    pid: Option<u32>,
    exited: bool,
    reaper_pgid: Option<u32>,
}

impl Guard {
    fn register_with_reaper(&mut self) {
        #[cfg(unix)]
        if let (Some(reaper), Some(pgid)) = (runtime::reaper::global(), self.pid) {
            match reaper.register(pgid) {
                Ok(()) => self.reaper_pgid = Some(pgid),
                Err(error) => {
                    tracing::warn!(pgid, %error, "could not register an MCP server with the reaper");
                }
            }
        }
    }

    /// After the leader has exited: on Unix, kills anything left in its group (a server
    /// started by a shim that has already exited) and unregisters the group.
    fn after_exit(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pid
            && group_alive(pgid)
        {
            let _ = self.child.start_kill();
        }
        self.unregister();
    }

    fn unregister(&mut self) {
        if let (Some(pgid), Some(reaper)) = (self.reaper_pgid.take(), runtime::reaper::global())
            && let Err(error) = reaper.unregister(pgid)
        {
            tracing::warn!(pgid, %error, "could not unregister an MCP server from the reaper");
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if !self.exited {
            let _ = self.child.start_kill();
        }
        self.unregister();
    }
}

/// Whether any process is left in process group `pgid`.
#[cfg(unix)]
fn group_alive(pgid: u32) -> bool {
    let Ok(pgid) = libc::pid_t::try_from(pgid) else {
        return false;
    };
    if pgid <= 1 {
        return false;
    }
    // SAFETY: signal 0 performs only the existence and permission check; nothing is sent.
    let rc = unsafe { libc::killpg(pgid, 0) };
    rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts_with(captured: &[(&str, &str)]) -> HostOptions {
        HostOptions {
            captured_env: Some(
                captured
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                    .collect(),
            ),
            secret_resolver: Arc::new(|name: &str| (name == "tok").then(|| "s3cret".to_owned())),
            ..HostOptions::default()
        }
    }

    #[test]
    fn classifies_missing_commands() {
        let opts = opts_with(&[("PATH", "/nonexistent-dir")]);
        let empty = BTreeMap::new();
        // On macOS the search also covers Homebrew and user tool folders (by design), so
        // a runtime installed there is found: only check the ones that are missing.
        let search = lookup_path(&opts);
        let installed =
            |c: &str| runtime::spawn::resolve_program(c, Some(search.as_os_str())).is_some();
        for (command, runtime) in [
            ("npx", MissingRuntime::NodeJs),
            ("uvx", MissingRuntime::UvPython),
            ("docker", MissingRuntime::Docker),
        ] {
            if installed(command) {
                continue;
            }
            assert!(
                matches!(
                    prepare(command, &[], &empty, None, &opts),
                    Err(HostError::MissingRuntime { runtime: r, .. }) if r == runtime
                ),
                "{command}"
            );
        }
        assert!(matches!(
            prepare("some-server-that-does-not-exist", &[], &empty, None, &opts),
            Err(HostError::CommandNotFound { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn resolves_against_the_captured_path_and_builds_a_scrubbed_env() {
        let sh_dir = runtime::spawn::resolve_program("sh", None)
            .and_then(|p| p.parent().map(Path::to_path_buf))
            .expect("sh on PATH");
        let path = sh_dir.to_string_lossy().into_owned();
        let mut opts = opts_with(&[
            ("PATH", &path),
            ("HOME", "/captured-home"),
            ("AWS_SECRET_ACCESS_KEY", "leak"),
            ("EXTRA_OK", "yes"),
        ]);
        opts.env_allowlist_extra = vec!["EXTRA_OK".into()];
        let env = BTreeMap::from([
            ("API_KEY".to_owned(), "${secret:tok}".to_owned()),
            ("PLAIN".to_owned(), "v".to_owned()),
        ]);
        let dir = tempfile::tempdir().unwrap();
        let prepared = prepare(
            "sh",
            &["-c".into(), "echo ${secret:tok}".into()],
            &env,
            Some(dir.path().to_str().unwrap()),
            &opts,
        )
        .unwrap();
        assert!(prepared.spec.program.is_absolute());
        assert_eq!(prepared.spec.program, sh_dir.join("sh"));
        assert_eq!(
            prepared.spec.args,
            vec![OsString::from("-c"), OsString::from("echo s3cret")]
        );
        assert_eq!(prepared.spec.cwd, dir.path());
        assert!(prepared.raw_args.is_none());
        let resolved: BTreeMap<String, String> = prepared
            .spec
            .env
            .resolve_from(Vec::<(OsString, OsString)>::new())
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert_eq!(
            resolved.get("HOME").map(String::as_str),
            Some("/captured-home")
        );
        // macOS appends the package-manager folders to the captured PATH.
        let expected_path = extend_with_tool_folders(OsString::from(&path), Some("/captured-home"));
        assert_eq!(
            resolved.get("PATH").map(OsString::from),
            Some(expected_path)
        );
        assert_eq!(resolved.get("API_KEY").map(String::as_str), Some("s3cret"));
        assert_eq!(resolved.get("PLAIN").map(String::as_str), Some("v"));
        assert_eq!(resolved.get("EXTRA_OK").map(String::as_str), Some("yes"));
        assert!(!resolved.contains_key("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn rejects_bad_working_folders_and_missing_secrets() {
        let opts = opts_with(&[("PATH", "/nonexistent-dir")]);
        let empty = BTreeMap::new();
        assert!(matches!(
            prepare("x", &[], &empty, Some("relative/dir"), &opts),
            Err(HostError::InvalidConfig(_))
        ));
        assert!(matches!(
            prepare("x", &["${secret:missing}".into()], &empty, None, &opts),
            Err(HostError::Secret(secrets::SecretError::Missing(name))) if name == "missing"
        ));
        let env = BTreeMap::from([("K".to_owned(), "${secret:missing}".to_owned())]);
        assert!(matches!(
            prepare("x", &[], &env, None, &opts),
            Err(HostError::Secret(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn relative_commands_resolve_against_the_working_folder_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let exe = dir.path().join("bin/server");
        std::fs::write(&exe, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let opts = opts_with(&[("PATH", "/nonexistent-dir")]);
        let empty = BTreeMap::new();
        let prepared = prepare("bin/server", &[], &empty, dir.path().to_str(), &opts).unwrap();
        assert_eq!(prepared.program, exe);
        assert!(matches!(
            prepare("bin/other", &[], &empty, dir.path().to_str(), &opts),
            Err(HostError::CommandNotFound { .. })
        ));
    }

    #[test]
    fn unwraps_cmd_c_wrappers() {
        let v = |list: &[&str]| list.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            unwrap_cmd_c("cmd", &v(&["/c", "npx", "-y", "@scope/pkg"])),
            Some(("npx".to_owned(), v(&["-y", "@scope/pkg"])))
        );
        assert_eq!(
            unwrap_cmd_c(
                r"C:\Windows\System32\CMD.EXE",
                &v(&["/D", "/C", "uvx", "x"])
            ),
            Some(("uvx".to_owned(), v(&["x"])))
        );
        assert_eq!(unwrap_cmd_c("cmd", &v(&["/c", "npx -y pkg"])), None);
        assert_eq!(unwrap_cmd_c("cmd", &v(&["/k", "npx"])), None);
        assert_eq!(unwrap_cmd_c("cmd", &v(&["/c"])), None);
        assert_eq!(unwrap_cmd_c("node", &v(&["/c", "x"])), None);
    }

    #[test]
    fn recognises_developer_tools_stubs() {
        assert!(is_developer_tools_stub(Path::new("/usr/bin/python3")));
        assert!(is_developer_tools_stub(Path::new("/usr/bin/git")));
        assert!(!is_developer_tools_stub(Path::new(
            "/opt/homebrew/bin/python3"
        )));
    }

    fn prepared_for(program: &str, command: &str, search: &str) -> Prepared {
        Prepared {
            spec: SpawnSpec::new(program, "/"),
            raw_args: None,
            program: PathBuf::from(program),
            command: command.to_owned(),
            search: OsString::from(search),
        }
    }

    #[tokio::test]
    async fn developer_tools_check_is_a_no_op_off_macos() {
        if !cfg!(target_os = "macos") {
            let mut prepared = prepared_for("/usr/bin/git", "git", "/usr/bin");
            check_developer_tools(&mut prepared).await.unwrap();
            assert_eq!(prepared.program, Path::new("/usr/bin/git"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_stub_is_passed_over_for_a_real_install_later_in_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let python = dir.path().join("python3");
        std::fs::write(&python, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&python, std::fs::Permissions::from_mode(0o755)).unwrap();
        let search = format!("/usr/bin:{}", dir.path().display());
        assert_eq!(
            alternative_to_stub("python3", OsStr::new(&search)),
            Some(python)
        );
        // Nothing else in PATH, or a stub named by its full path: no alternative.
        assert_eq!(alternative_to_stub("python3", OsStr::new("/usr/bin")), None);
        assert_eq!(
            alternative_to_stub("/usr/bin/python3", OsStr::new(&search)),
            None
        );
    }
}
