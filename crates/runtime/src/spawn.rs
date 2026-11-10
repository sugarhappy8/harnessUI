//! The one place in the codebase that creates processes (ARCHITECTURE.md 4.1).
//!
//! Every child is built here on top of `process-wrap`, the same crate rmcp accepts for
//! stdio MCP servers, so llama-server, tool commands, workers and MCP servers share one
//! spawn path:
//!
//! - **Windows:** the child is created suspended (`CREATE_SUSPENDED` is added by
//!   process-wrap) with `CREATE_NO_WINDOW`, assigned to its own Job Object and resumed.
//!   Because [`KillOnDrop`] is also wrapped, process-wrap sets
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: when the app exits for any reason the OS closes
//!   the only job handle and kills the whole tree. The app itself is never put in a job.
//! - **Unix:** the child leads its own process group, so [`ManagedChild::start_kill`]
//!   signals the whole tree. On macOS the [`crate::reaper`] covers a crash of the app; if a
//!   reaper is installed with [`crate::reaper::ReaperClient::install_global`], every group
//!   started here is registered with it automatically.
//!
//! The child environment is always cleared first and rebuilt from an [`EnvSpec`]
//! allowlist, so variables such as `LLAMA_ARG_*`, `GGML_BACKEND_PATH` or `HF_TOKEN` in the
//! app's own environment never reach a child unless a caller names them.
//!
//! `scripts/check-spawn.sh` and the workspace clippy rule reject process creation anywhere
//! else in the codebase.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};

use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
#[cfg(windows)]
use process_wrap::tokio::{CreationFlags, JobObject};
#[cfg(unix)]
use process_wrap::tokio::{ProcessGroup, ProcessSession};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

/// Variables [`EnvSpec::minimal`] passes through from the app's environment on Windows.
pub const WINDOWS_ENV_ALLOWLIST: &[&str] = &[
    "APPDATA",
    "HOMEDRIVE",
    "HOMEPATH",
    "LOCALAPPDATA",
    "PATH",
    "PATHEXT",
    "PROCESSOR_ARCHITECTURE",
    "SYSTEMDRIVE",
    "SYSTEMROOT",
    "TEMP",
    "TMP",
    "USERNAME",
    "USERPROFILE",
    "PROGRAMFILES",
    "COMSPEC",
    "WINDIR",
];

/// Variables [`EnvSpec::minimal`] passes through from the app's environment on Unix.
pub const UNIX_ENV_ALLOWLIST: &[&str] = &[
    "HOME", "LOGNAME", "PATH", "SHELL", "TERM", "USER", "TMPDIR", "LANG",
];

/// The environment a child starts with.
///
/// The child environment is always cleared first. Then every variable named in
/// [`inherit`](Self::inherit) that exists in the app's environment is copied, and finally
/// every pair in [`set`](Self::set) is applied, replacing an inherited value of the same
/// name. Names compare case-insensitively on Windows, as the OS does.
///
/// `Debug` prints the names of the variables that are set but not their values, which
/// carry secrets (the llama-server API key, MCP server keys from the keychain).
#[derive(Clone, Default, PartialEq, Eq)]
pub struct EnvSpec {
    /// Names of variables copied from the app's environment when present.
    pub inherit: Vec<String>,
    /// Variables set explicitly, applied after the inherited ones.
    pub set: Vec<(String, String)>,
}

impl std::fmt::Debug for EnvSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let set: Vec<(&str, &str)> = self
            .set
            .iter()
            .map(|(name, _)| (name.as_str(), "<redacted>"))
            .collect();
        f.debug_struct("EnvSpec")
            .field("inherit", &self.inherit)
            .field("set", &set)
            .finish()
    }
}

impl EnvSpec {
    /// The platform allowlist: [`WINDOWS_ENV_ALLOWLIST`] or [`UNIX_ENV_ALLOWLIST`], nothing set.
    pub fn minimal() -> Self {
        let list = if cfg!(windows) {
            WINDOWS_ENV_ALLOWLIST
        } else {
            UNIX_ENV_ALLOWLIST
        };
        Self {
            inherit: list.iter().map(|name| (*name).to_owned()).collect(),
            set: Vec::new(),
        }
    }

    /// No variables at all.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Adds an explicit variable; a later `set` of the same name wins.
    pub fn set(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.set.push((key.into(), value.into()));
        self
    }

    /// Adds a variable to copy from the app's environment when present.
    pub fn inherit(mut self, key: impl Into<String>) -> Self {
        self.inherit.push(key.into());
        self
    }

    /// The exact environment a child gets, resolved against the app's environment.
    pub fn resolve(&self) -> Vec<(OsString, OsString)> {
        self.resolve_from(std::env::vars_os())
    }

    /// The exact environment a child gets, resolved against `parent` instead of the app's
    /// own environment. Used by tests and by [`resolve`](Self::resolve).
    pub fn resolve_from<I>(&self, parent: I) -> Vec<(OsString, OsString)>
    where
        I: IntoIterator<Item = (OsString, OsString)>,
    {
        let parent: Vec<(OsString, OsString)> = parent.into_iter().collect();
        let mut out: Vec<(OsString, OsString)> = Vec::new();
        for wanted in &self.inherit {
            if out.iter().any(|(name, _)| same_name(name, wanted)) {
                continue;
            }
            if let Some((name, value)) = parent.iter().find(|(name, _)| same_name(name, wanted)) {
                out.push((name.clone(), value.clone()));
            }
        }
        for (key, value) in &self.set {
            out.retain(|(name, _)| !same_name(name, key));
            out.push((OsString::from(key), OsString::from(value)));
        }
        out
    }
}

fn same_name(actual: &OsStr, wanted: &str) -> bool {
    if cfg!(windows) {
        actual
            .to_str()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(wanted))
    } else {
        actual == OsStr::new(wanted)
    }
}

/// What a child's stdin, stdout or stderr is connected to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StdioMode {
    /// The null device.
    Null,
    /// A pipe the app reads from or writes to; take it with the `take_*` methods.
    Piped,
}

impl StdioMode {
    fn to_stdio(self) -> Stdio {
        match self {
            Self::Null => Stdio::null(),
            Self::Piped => Stdio::piped(),
        }
    }
}

/// Everything needed to start one child.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpawnSpec {
    /// The program to run. Pass an absolute path (see [`resolve_program`]); a bare name is
    /// looked up by the OS in the child's `PATH`.
    pub program: PathBuf,
    /// Arguments, passed as separate strings and never through a shell.
    pub args: Vec<OsString>,
    /// Working directory. Always an app-controlled or approved folder: llama.cpp loads
    /// backend libraries from the current directory too (ARCHITECTURE.md 7.3).
    pub cwd: PathBuf,
    /// The child's environment; it starts cleared.
    pub env: EnvSpec,
    /// Where stdin comes from.
    pub stdin: StdioMode,
    /// Where stdout goes.
    pub stdout: StdioMode,
    /// Where stderr goes.
    pub stderr: StdioMode,
}

impl SpawnSpec {
    /// A spec with no arguments, the [`EnvSpec::minimal`] environment, stdin from the null
    /// device and stdout and stderr piped.
    pub fn new(program: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            cwd: cwd.into(),
            env: EnvSpec::minimal(),
            stdin: StdioMode::Null,
            stdout: StdioMode::Piped,
            stderr: StdioMode::Piped,
        }
    }

    /// Appends one argument.
    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Appends several arguments.
    pub fn args<I, A>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Replaces the environment.
    pub fn env(mut self, env: EnvSpec) -> Self {
        self.env = env;
        self
    }

    /// Sets where stdin comes from.
    pub fn stdin(mut self, mode: StdioMode) -> Self {
        self.stdin = mode;
        self
    }

    /// Sets where stdout goes.
    pub fn stdout(mut self, mode: StdioMode) -> Self {
        self.stdout = mode;
        self
    }

    /// Sets where stderr goes.
    pub fn stderr(mut self, mode: StdioMode) -> Self {
        self.stderr = mode;
        self
    }
}

/// How the child is detached from the app's own process group.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Detach {
    /// Leader of a new process group (Unix). Every ordinary child.
    Group,
    /// Leader of a new session (Unix). Only the reaper, so a terminal's signals and the
    /// app's own group never reach it.
    #[cfg_attr(not(unix), allow(dead_code))]
    Session,
}

#[derive(Clone, Copy, Debug)]
struct Options {
    detach: Detach,
    kill_on_drop: bool,
    register_with_reaper: bool,
}

impl Options {
    const CHILD: Self = Self {
        detach: Detach::Group,
        kill_on_drop: true,
        register_with_reaper: true,
    };
}

/// Builds the wrapped command for `spec` without starting it: environment cleared and
/// rebuilt, working directory set, stdio configured, and the platform wrappers applied
/// (Windows: `CREATE_NO_WINDOW`, Job Object, kill-on-drop; Unix: process-group leader,
/// kill-on-drop).
///
/// This is what the MCP host hands to rmcp's `TokioChildProcess`, so external servers get
/// the same containment as every other child.
pub fn command_wrap(spec: &SpawnSpec) -> CommandWrap {
    build(spec, Options::CHILD)
}

fn build(spec: &SpawnSpec, options: Options) -> CommandWrap {
    let env = spec.env.resolve();
    let mut wrap = CommandWrap::with_new(&spec.program, |command| {
        command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(env)
            .stdin(spec.stdin.to_stdio())
            .stdout(spec.stdout.to_stdio())
            .stderr(spec.stderr.to_stdio());
    });
    #[cfg(windows)]
    {
        use windows::Win32::System::Threading::CREATE_NO_WINDOW;
        let _ = options.detach;
        wrap.wrap(CreationFlags(CREATE_NO_WINDOW));
        wrap.wrap(JobObject);
    }
    #[cfg(unix)]
    match options.detach {
        Detach::Group => {
            wrap.wrap(ProcessGroup::leader());
        }
        Detach::Session => {
            wrap.wrap(ProcessSession);
        }
    }
    if options.kill_on_drop {
        // On Windows this is also what makes process-wrap set JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
        wrap.wrap(KillOnDrop);
    }
    wrap
}

/// Starts a child described by `spec`. Must be called inside a Tokio runtime.
///
/// The returned [`ManagedChild`] kills the child's whole tree when dropped.
pub fn spawn(spec: SpawnSpec) -> io::Result<ManagedChild> {
    spawn_with(&spec, Options::CHILD)
}

/// Starts the reaper: leader of its own session, not killed when the handle drops and not
/// registered with itself.
#[cfg(unix)]
pub(crate) fn spawn_reaper(spec: &SpawnSpec) -> io::Result<ManagedChild> {
    spawn_with(
        spec,
        Options {
            detach: Detach::Session,
            kill_on_drop: false,
            register_with_reaper: false,
        },
    )
}

fn spawn_with(spec: &SpawnSpec, options: Options) -> io::Result<ManagedChild> {
    init_parent_error_mode();
    let inner = build(spec, options).spawn()?;
    let pid = inner.id();
    let child = ManagedChild {
        inner,
        pid,
        kill_on_drop: options.kill_on_drop,
        finished: false,
        #[cfg(unix)]
        reaper_pgid: None,
    };
    #[cfg(unix)]
    let child = {
        let mut child = child;
        if options.register_with_reaper {
            child.register_with_reaper();
        }
        child
    };
    #[cfg(not(unix))]
    let _ = options.register_with_reaper;
    Ok(child)
}

/// Sets the Windows error mode of the app process once, so that a child whose DLL is
/// missing exits with `STATUS_DLL_NOT_FOUND` instead of raising a modal system dialog.
///
/// Children inherit the error mode. The flags are `SEM_FAILCRITICALERRORS |
/// SEM_NOOPENFILEERRORBOX`, added to whatever mode the process already had. [`spawn`]
/// calls this before the first child starts; the app may also call it early at startup.
/// It does nothing on other platforms.
pub fn init_parent_error_mode() {
    #[cfg(windows)]
    {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            use windows::Win32::System::Diagnostics::Debug::{
                GetErrorMode, SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetErrorMode,
                THREAD_ERROR_MODE,
            };
            let wanted = SEM_FAILCRITICALERRORS.0 | SEM_NOOPENFILEERRORBOX.0;
            // SAFETY: GetErrorMode only reads and SetErrorMode only changes this process's
            // error-mode flags. Reading first keeps every flag the process already had, with
            // no moment in which they are dropped.
            unsafe {
                let previous = GetErrorMode();
                SetErrorMode(THREAD_ERROR_MODE(previous | wanted));
            }
        });
    }
}

/// Finds `name` the way a shell would: in `path` (or the app's `PATH` when `None`), and on
/// Windows with the extensions in `PATHEXT`, so `npx` resolves to `npx.cmd`.
///
/// An absolute `name` is checked as is (with `PATHEXT` on Windows). A relative name with a
/// directory part is refused, and the current directory is never searched, not even
/// through an empty or relative `PATH` entry such as `.`: a program planted in a cloned
/// repository must not shadow a system one.
pub fn resolve_program(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let candidate = Path::new(name);
    if candidate.is_absolute() {
        return which::which_global(candidate).ok();
    }
    if candidate.components().count() > 1 {
        return None;
    }
    let search = absolute_search_path(&match path {
        Some(path) => path.to_os_string(),
        None => std::env::var_os("PATH")?,
    })?;
    which::which_in_global(name, Some(search)).ok()?.next()
}

/// `path` with every entry that is not absolute removed. `which` resolves a relative entry
/// (including the empty entry, which Unix shells read as `.`) against the current
/// directory. `None` when no absolute entry is left.
fn absolute_search_path(path: &OsStr) -> Option<OsString> {
    let absolute: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|entry| entry.is_absolute())
        .collect();
    if absolute.is_empty() {
        return None;
    }
    std::env::join_paths(absolute).ok()
}

/// A running child and its process tree.
///
/// Dropping it kills the whole tree (the Job Object on Windows, the process group on Unix)
/// unless the child has already exited and, on Unix, its group is empty.
#[derive(Debug)]
pub struct ManagedChild {
    inner: Box<dyn ChildWrapper>,
    pid: Option<u32>,
    kill_on_drop: bool,
    /// The leader has been reaped and nothing is left to kill. On Unix the group id may be
    /// reused after this, so no signal is sent to it again.
    finished: bool,
    /// Group registered with the global reaper, to unregister once it is gone.
    #[cfg(unix)]
    reaper_pgid: Option<u32>,
}

impl ManagedChild {
    /// The child's process id. On Unix this is also its process-group id.
    pub fn id(&self) -> Option<u32> {
        self.pid
    }

    /// Takes the child's stdin pipe, if it was [`StdioMode::Piped`] and not taken yet.
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.inner.stdin().take()
    }

    /// Takes the child's stdout pipe, if it was [`StdioMode::Piped`] and not taken yet.
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.inner.stdout().take()
    }

    /// Takes the child's stderr pipe, if it was [`StdioMode::Piped`] and not taken yet.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.inner.stderr().take()
    }

    /// Waits for the child to exit and returns its status. On Windows this also waits until
    /// every other process in the job has exited.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let status = self.inner.wait().await?;
        self.after_exit();
        Ok(status)
    }

    /// Returns the exit status if the child has exited, without waiting.
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        let status = self.try_wait_leader()?;
        if status.is_some() {
            self.after_exit();
        }
        Ok(status)
    }

    /// Polls the leader without waiting.
    ///
    /// On Unix this goes to the native Tokio child rather than process-wrap's process-group
    /// wrapper: the wrapper reaps the leader with a raw `waitpid`, so Tokio and std never
    /// learn that it exited, and Tokio's kill-on-drop later sends `SIGKILL` to that PID,
    /// which by then may belong to an unrelated process. Reaped through std, the status is
    /// recorded and every later kill of the leader is a no-op. Nothing else needs reaping:
    /// the leader is the only process in its group that is this process's child.
    #[cfg(unix)]
    fn try_wait_leader(&mut self) -> io::Result<Option<ExitStatus>> {
        // SAFETY: polling the native child keeps the wrappers' invariants. The process-group
        // wrapper only caches the status it reaped itself, and its `wait` starts with the
        // native child's `wait`, which returns the status std recorded here.
        match unsafe { self.inner.as_mut().try_inner_child_mut() } {
            Some(native) => native.try_wait(),
            None => self.inner.try_wait(),
        }
    }

    /// Polls the leader (and, through the Job Object, the rest of the tree) without waiting.
    #[cfg(not(unix))]
    fn try_wait_leader(&mut self) -> io::Result<Option<ExitStatus>> {
        self.inner.try_wait()
    }

    /// Kills the whole tree without waiting: `TerminateJobObject` on Windows, `SIGKILL` to
    /// the process group on Unix. Killing a tree that is already gone succeeds.
    pub fn start_kill(&mut self) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        ignore_gone(self.inner.start_kill())
    }

    /// Kills the whole tree and waits for the child to exit.
    pub async fn kill(&mut self) -> io::Result<()> {
        self.start_kill()?;
        self.wait().await.map(|_| ())
    }

    /// Sends `signal` (for example `libc::SIGTERM`) to the child's whole process group.
    /// Signalling a group that is already gone succeeds.
    #[cfg(unix)]
    pub fn signal(&self, signal: i32) -> io::Result<()> {
        if self.finished {
            return Ok(());
        }
        ignore_gone(self.inner.signal(signal))
    }

    fn after_exit(&mut self) {
        #[cfg(unix)]
        {
            let gone = self.pid.is_none_or(|pgid| !group_alive(pgid));
            if gone {
                self.finished = true;
                self.unregister_from_reaper();
            }
        }
        #[cfg(not(unix))]
        {
            // The job handle is unique to this child, so terminating it later can never hit
            // an unrelated process; nothing to track here.
        }
    }

    #[cfg(unix)]
    fn register_with_reaper(&mut self) {
        let (Some(reaper), Some(pgid)) = (crate::reaper::global(), self.pid) else {
            return;
        };
        match reaper.register(pgid) {
            Ok(()) => self.reaper_pgid = Some(pgid),
            Err(error) => {
                tracing::warn!(pgid, %error, "could not register child group with the reaper");
            }
        }
    }

    fn unregister_from_reaper(&mut self) {
        #[cfg(unix)]
        if let (Some(pgid), Some(reaper)) = (self.reaper_pgid.take(), crate::reaper::global())
            && let Err(error) = reaper.unregister(pgid)
        {
            tracing::warn!(pgid, %error, "could not unregister child group from the reaper");
        }
    }
}

impl Drop for ManagedChild {
    fn drop(&mut self) {
        if self.kill_on_drop
            && !self.finished
            && let Err(error) = ignore_gone(self.inner.start_kill())
        {
            tracing::warn!(pid = ?self.pid, %error, "could not kill child tree on drop");
        }
        self.unregister_from_reaper();
    }
}

/// Maps "no such process" (the tree already exited) to success.
fn ignore_gone(result: io::Result<()>) -> io::Result<()> {
    match result {
        #[cfg(unix)]
        Err(error) if error.raw_os_error() == Some(libc::ESRCH) => Ok(()),
        other => other,
    }
}

/// Whether any process is left in process group `pgid`.
#[cfg(unix)]
pub(crate) fn group_alive(pgid: u32) -> bool {
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

    fn os(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    #[test]
    fn minimal_lists_the_platform_allowlist() {
        let spec = EnvSpec::minimal();
        assert!(spec.inherit.iter().any(|name| name == "PATH"));
        assert!(spec.set.is_empty());
        if cfg!(windows) {
            assert!(spec.inherit.iter().any(|name| name == "SYSTEMROOT"));
        } else {
            assert!(spec.inherit.iter().any(|name| name == "HOME"));
        }
    }

    #[test]
    fn resolve_keeps_only_allowlisted_and_set_variables() {
        let parent = os(&[
            ("PATH", "/usr/bin"),
            ("HOME", "/home/u"),
            ("HF_TOKEN", "secret"),
            ("LLAMA_ARG_TOOLS", "all"),
            ("GGML_BACKEND_PATH", "/tmp/evil.so"),
        ]);
        let env = EnvSpec::empty()
            .inherit("PATH")
            .inherit("HOME")
            .set("A", "1");
        let resolved = env.resolve_from(parent);
        assert_eq!(
            resolved,
            os(&[("PATH", "/usr/bin"), ("HOME", "/home/u"), ("A", "1")])
        );
    }

    #[test]
    fn set_overrides_inherited_and_earlier_set() {
        let parent = os(&[("PATH", "/usr/bin")]);
        let env = EnvSpec::empty()
            .inherit("PATH")
            .set("PATH", "/opt")
            .set("X", "1")
            .set("X", "2");
        let resolved = env.resolve_from(parent);
        assert_eq!(resolved, os(&[("PATH", "/opt"), ("X", "2")]));
    }

    #[test]
    fn missing_inherited_variables_are_skipped_and_duplicates_collapse() {
        let env = EnvSpec::empty()
            .inherit("NOPE")
            .inherit("PATH")
            .inherit("PATH");
        let resolved = env.resolve_from(os(&[("PATH", "p")]));
        assert_eq!(resolved, os(&[("PATH", "p")]));
    }

    #[test]
    fn debug_output_names_variables_but_hides_values() {
        let spec = SpawnSpec::new("/bin/prog", "/tmp")
            .env(EnvSpec::minimal().set("LLAMA_API_KEY", "very-secret-key"));
        let shown = format!("{spec:?}");
        assert!(!shown.contains("very-secret-key"), "{shown}");
        assert!(shown.contains("LLAMA_API_KEY"), "{shown}");
        assert!(shown.contains("PATH"), "{shown}");
    }

    #[test]
    fn empty_env_resolves_to_nothing() {
        assert!(
            EnvSpec::empty()
                .resolve_from(os(&[("PATH", "p")]))
                .is_empty()
        );
    }

    #[cfg(windows)]
    #[test]
    fn names_compare_case_insensitively_on_windows() {
        let env = EnvSpec::empty().inherit("PATH");
        assert_eq!(
            env.resolve_from(os(&[("Path", "C:\\x")])),
            os(&[("Path", "C:\\x")])
        );
    }

    #[cfg(unix)]
    #[test]
    fn names_compare_case_sensitively_on_unix() {
        let env = EnvSpec::empty().inherit("PATH");
        assert!(env.resolve_from(os(&[("Path", "/x")])).is_empty());
    }

    #[test]
    fn spawn_spec_builder_sets_fields() {
        let spec = SpawnSpec::new("/bin/prog", "/tmp")
            .arg("a")
            .args(["b", "c"])
            .env(EnvSpec::empty())
            .stdin(StdioMode::Piped)
            .stdout(StdioMode::Null)
            .stderr(StdioMode::Null);
        assert_eq!(spec.program, PathBuf::from("/bin/prog"));
        assert_eq!(
            spec.args,
            vec![
                OsString::from("a"),
                OsString::from("b"),
                OsString::from("c")
            ]
        );
        assert_eq!(spec.cwd, PathBuf::from("/tmp"));
        assert_eq!(spec.env, EnvSpec::empty());
        assert_eq!(spec.stdin, StdioMode::Piped);
        assert_eq!(spec.stdout, StdioMode::Null);
        assert_eq!(spec.stderr, StdioMode::Null);
    }

    #[test]
    fn spawn_spec_defaults() {
        let spec = SpawnSpec::new("p", "d");
        assert_eq!(spec.env, EnvSpec::minimal());
        assert_eq!(spec.stdin, StdioMode::Null);
        assert_eq!(spec.stdout, StdioMode::Piped);
        assert_eq!(spec.stderr, StdioMode::Piped);
    }

    #[test]
    fn resolve_program_rejects_relative_paths_and_empty_names() {
        assert_eq!(resolve_program("", None), None);
        assert_eq!(
            resolve_program("sub/prog", Some(OsStr::new("/usr/bin"))),
            None
        );
        assert_eq!(
            resolve_program("./prog", Some(OsStr::new("/usr/bin"))),
            None
        );
        assert_eq!(resolve_program("a\0b", None), None);
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_searches_the_given_path_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("my-tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let found = resolve_program("my-tool", Some(dir.path().as_os_str()));
        assert_eq!(found.as_deref(), Some(exe.as_path()));
        assert_eq!(
            resolve_program("my-tool", Some(OsStr::new("/nonexistent-dir"))),
            None
        );
        assert_eq!(
            resolve_program(exe.to_str().expect("utf8"), None).as_deref(),
            Some(exe.as_path())
        );
        assert!(resolve_program("sh", None).is_some());
    }

    #[cfg(unix)]
    #[test]
    fn resolve_program_ignores_relative_path_entries() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("planted-tool");
        std::fs::write(&exe, "#!/bin/sh\n").expect("write");
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        // The same directory, spelled relative to the current directory: `which` would
        // find the program through it.
        let cwd = std::env::current_dir().expect("cwd");
        let ups = cwd.components().count().saturating_sub(1);
        let relative: PathBuf = std::iter::repeat_n("..", ups)
            .collect::<PathBuf>()
            .join(dir.path().strip_prefix("/").expect("absolute tempdir"));
        assert!(relative.join("planted-tool").is_file(), "{relative:?}");
        let search = std::env::join_paths([relative.as_path(), Path::new("")]).expect("join");
        assert_eq!(resolve_program("planted-tool", Some(&search)), None);
        // With the absolute spelling added, it is found there and only there.
        let search = std::env::join_paths([relative.as_path(), dir.path()]).expect("join");
        assert_eq!(
            resolve_program("planted-tool", Some(&search)).as_deref(),
            Some(exe.as_path())
        );
    }

    #[test]
    fn absolute_search_path_keeps_only_absolute_entries() {
        let root = if cfg!(windows) { r"C:\bin" } else { "/bin" };
        let joined =
            std::env::join_paths([Path::new(""), Path::new("."), Path::new(root)]).expect("join");
        let kept = absolute_search_path(&joined).expect("one entry left");
        assert_eq!(
            std::env::split_paths(&kept).collect::<Vec<_>>(),
            vec![PathBuf::from(root)]
        );
        let relative_only = std::env::join_paths([".", "sub"]).expect("join");
        assert_eq!(absolute_search_path(&relative_only), None);
    }

    #[test]
    fn command_wrap_applies_the_platform_wrappers() {
        let wrap = command_wrap(&SpawnSpec::new("prog", "."));
        assert!(wrap.has_wrap::<KillOnDrop>());
        #[cfg(unix)]
        {
            assert!(wrap.has_wrap::<ProcessGroup>());
            assert!(!wrap.has_wrap::<ProcessSession>());
        }
        #[cfg(windows)]
        {
            assert!(wrap.has_wrap::<JobObject>());
            assert!(wrap.has_wrap::<CreationFlags>());
        }
        let command = wrap.command().as_std();
        assert_eq!(command.get_current_dir(), Some(Path::new(".")));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn try_wait_leaves_the_exit_status_with_std() {
        let sh = resolve_program("sh", None).expect("sh on PATH");
        let spec = SpawnSpec::new(sh, std::env::temp_dir())
            .args(["-c", "exit 3"])
            .stdout(StdioMode::Null)
            .stderr(StdioMode::Null);
        let mut child = spawn(spec).expect("spawn sh");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let status = loop {
            if let Some(status) = child.try_wait().expect("try_wait") {
                break status;
            }
            assert!(std::time::Instant::now() < deadline, "sh did not exit");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(status.code(), Some(3));
        // std recorded the status, so a later kill of the leader (Tokio's kill-on-drop)
        // can never signal a reused PID.
        // SAFETY: only reads the native child's recorded status.
        let native = unsafe { child.inner.as_mut().try_inner_child_mut() }.expect("native child");
        assert_eq!(
            native
                .try_wait()
                .expect("std knows the child")
                .map(|s| s.code()),
            Some(Some(3))
        );
        assert_eq!(child.wait().await.expect("wait").code(), Some(3));
        assert_eq!(
            child.try_wait().expect("again").map(|s| s.code()),
            Some(Some(3))
        );
        assert!(child.finished, "the empty group is never signalled again");
    }

    #[cfg(unix)]
    #[test]
    fn group_alive_rejects_reserved_ids() {
        assert!(!group_alive(0));
        assert!(!group_alive(1));
        assert!(!group_alive(u32::MAX));
    }
}
