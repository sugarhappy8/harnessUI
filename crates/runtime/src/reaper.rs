//! Crash safety for macOS (ARCHITECTURE.md 4.1).
//!
//! macOS has no kernel feature that kills a process's children when it dies, so the app
//! starts one small **reaper** at launch: its own binary run with a hidden flag, in its own
//! session, connected to the app by a pipe on its stdin. The app registers the process group
//! of every child it starts. When the pipe closes or the app's PID disappears, the reaper
//! sends `SIGTERM` to every registered group, waits up to two seconds, sends `SIGKILL` to
//! whatever is left and exits.
//!
//! The protocol on the pipe is one ASCII line per command:
//!
//! ```text
//! add <pgid>
//! remove <pgid>
//! ```
//!
//! The app's entry point checks for the hidden flag before doing anything else and calls
//! [`reaper_main`] with the PID that follows it:
//!
//! ```text
//! <app binary> <hidden flag> <app pid>
//! ```
//!
//! On Linux (a development platform) the same code runs, with the parent watched by
//! polling. On Windows every function here is a no-op, because Job Objects already kill
//! children when the app exits.

use std::collections::BTreeSet;
use std::io;
use std::path::Path;
use std::sync::OnceLock;

/// How often the reaper checks that the app is still alive when nothing else wakes it.
pub const PARENT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

/// How long registered groups get between `SIGTERM` and `SIGKILL`.
pub const TERMINATE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// One command on the reaper's stdin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReaperCommand {
    /// Start watching a process group.
    Add(u32),
    /// Stop watching a process group (it exited or was killed by the app).
    Remove(u32),
}

/// Parses one protocol line (`add <pgid>` or `remove <pgid>`), tolerating surrounding
/// whitespace and a trailing `\r`. Returns `None` for anything else, including ids that
/// could never be a child's group (0 and 1).
pub fn parse_command(line: &str) -> Option<ReaperCommand> {
    let mut parts = line.split_ascii_whitespace();
    let verb = parts.next()?;
    let pgid: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || pgid <= 1 || i32::try_from(pgid).is_err() {
        return None;
    }
    match verb {
        "add" => Some(ReaperCommand::Add(pgid)),
        "remove" => Some(ReaperCommand::Remove(pgid)),
        _ => None,
    }
}

/// The set of process groups the reaper will kill, minus groups it must never touch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GroupSet {
    groups: BTreeSet<u32>,
    protected: BTreeSet<u32>,
}

impl GroupSet {
    /// An empty set that refuses to register any id in `protected` (the reaper's own group
    /// and the app's group). Ids 0 and 1 are always refused.
    pub fn new(protected: impl IntoIterator<Item = u32>) -> Self {
        let mut protected: BTreeSet<u32> = protected.into_iter().collect();
        protected.extend([0, 1]);
        Self {
            groups: BTreeSet::new(),
            protected,
        }
    }

    /// Applies a command. Returns whether the set changed.
    pub fn apply(&mut self, command: ReaperCommand) -> bool {
        match command {
            ReaperCommand::Add(pgid) if self.protected.contains(&pgid) => false,
            ReaperCommand::Add(pgid) => self.groups.insert(pgid),
            ReaperCommand::Remove(pgid) => self.groups.remove(&pgid),
        }
    }

    /// The registered groups, in ascending order.
    pub fn groups(&self) -> impl Iterator<Item = u32> + '_ {
        self.groups.iter().copied()
    }

    /// Number of registered groups.
    pub fn len(&self) -> usize {
        self.groups.len()
    }

    /// Whether no group is registered.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }
}

/// The reaper process's entry point. Never returns.
///
/// Reads [`ReaperCommand`] lines from stdin. When stdin reaches end-of-file or the process
/// `parent_pid` is gone (watched with kqueue `NOTE_EXIT` on macOS, and by polling every
/// [`PARENT_POLL_INTERVAL`] everywhere), it terminates every registered group as described
/// in the module documentation and exits with status 0. `SIGHUP`, `SIGINT`, `SIGTERM` and
/// `SIGPIPE` are ignored so that the reaper outlives the signals that end the app.
///
/// On Windows it exits immediately with status 0.
pub fn reaper_main(parent_pid: u32) -> ! {
    #[cfg(unix)]
    {
        unix::run(parent_pid);
        std::process::exit(0)
    }
    #[cfg(not(unix))]
    {
        let _ = parent_pid;
        std::process::exit(0)
    }
}

static GLOBAL: OnceLock<ReaperClient> = OnceLock::new();

/// The reaper installed with [`ReaperClient::install_global`], if any.
///
/// [`crate::spawn::spawn`] registers its children automatically. Code that starts a child
/// some other way from a [`crate::spawn::command_wrap`] (the MCP host hands it to rmcp)
/// registers the child's process group here itself and unregisters it once it is gone.
pub fn global() -> Option<&'static ReaperClient> {
    GLOBAL.get()
}

/// The app's handle on its reaper process.
#[derive(Debug)]
pub struct ReaperClient {
    #[cfg(unix)]
    inner: unix::Client,
}

impl ReaperClient {
    /// Starts the reaper: `app_exe hidden_flag <this process's pid>`, through the spawn
    /// module, as the leader of its own session, with stdin piped and stdout and stderr
    /// discarded. Must be called inside a Tokio runtime.
    ///
    /// On Windows nothing is started and the returned client does nothing.
    pub fn start(app_exe: &Path, hidden_flag: &str) -> io::Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                inner: unix::Client::start(app_exe, hidden_flag)?,
            })
        }
        #[cfg(not(unix))]
        {
            let _ = (app_exe, hidden_flag);
            Ok(Self {})
        }
    }

    /// Asks the reaper to watch process group `pgid`.
    pub fn register(&self, pgid: u32) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.send(&format!("add {pgid}\n"))
        }
        #[cfg(not(unix))]
        {
            let _ = pgid;
            Ok(())
        }
    }

    /// Asks the reaper to stop watching process group `pgid`.
    pub fn unregister(&self, pgid: u32) -> io::Result<()> {
        #[cfg(unix)]
        {
            self.inner.send(&format!("remove {pgid}\n"))
        }
        #[cfg(not(unix))]
        {
            let _ = pgid;
            Ok(())
        }
    }

    /// The reaper's process id (`None` on Windows).
    pub fn pid(&self) -> Option<u32> {
        #[cfg(unix)]
        {
            self.inner.pid()
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    /// Makes this the process-wide reaper. From then on every child started through
    /// [`crate::spawn::spawn`] is registered on start and unregistered once its group is
    /// gone. Fails, returning the client, if a reaper is already installed.
    pub fn install_global(self) -> Result<(), Self> {
        GLOBAL.set(self)
    }
}

#[cfg(unix)]
mod unix {
    use std::io::{self, BufRead, Write};
    use std::path::Path;
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::{GroupSet, PARENT_POLL_INTERVAL, ReaperCommand, TERMINATE_GRACE, parse_command};
    use crate::spawn::{self, EnvSpec, ManagedChild, SpawnSpec, StdioMode, group_alive};

    /// Longest protocol line the reader accepts; longer input is discarded.
    const MAX_LINE: usize = 64;

    enum Event {
        Command(ReaperCommand),
        StdinClosed,
        /// Only sent by the kqueue watcher on macOS; elsewhere the poll notices.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        ParentExited,
    }

    pub(super) fn run(parent_pid: u32) {
        ignore_terminal_signals();
        let parent = libc::pid_t::try_from(parent_pid).unwrap_or(0);
        // SAFETY: getpgrp and getpgid have no preconditions; getpgid reports failure as -1.
        let own_group = unsafe { libc::getpgrp() };
        let parent_group = if parent > 0 {
            unsafe { libc::getpgid(parent) }
        } else {
            -1
        };
        let protected = [own_group, parent_group]
            .into_iter()
            .filter_map(|id| u32::try_from(id).ok());
        let mut groups = GroupSet::new(protected);

        let (tx, rx) = mpsc::channel();
        spawn_stdin_reader(tx.clone());
        watch_parent(parent, tx);

        loop {
            match rx.recv_timeout(PARENT_POLL_INTERVAL) {
                Ok(Event::Command(command)) => {
                    groups.apply(command);
                }
                Ok(Event::StdinClosed) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Event::ParentExited) => {
                    drain_pending(&rx, &mut groups);
                    break;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if !parent_alive(parent) {
                        drain_pending(&rx, &mut groups);
                        break;
                    }
                }
            }
        }
        terminate_groups(&groups, TERMINATE_GRACE);
    }

    /// After the parent died, its last writes may still sit in the pipe; apply them until the
    /// reader reports end-of-file, for at most 200 ms.
    fn drain_pending(rx: &mpsc::Receiver<Event>, groups: &mut GroupSet) {
        let deadline = Instant::now() + Duration::from_millis(200);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            match rx.recv_timeout(left) {
                Ok(Event::Command(command)) => {
                    groups.apply(command);
                }
                Ok(Event::ParentExited) => {}
                Ok(Event::StdinClosed) | Err(_) => break,
            }
        }
    }

    fn ignore_terminal_signals() {
        for signal in [libc::SIGHUP, libc::SIGINT, libc::SIGTERM, libc::SIGPIPE] {
            // SAFETY: installing SIG_IGN has no handler code to run and is async-signal-safe.
            unsafe {
                libc::signal(signal, libc::SIG_IGN);
            }
        }
    }

    fn spawn_stdin_reader(tx: mpsc::Sender<Event>) {
        let spawned = std::thread::Builder::new()
            .name("reaper-stdin".into())
            .spawn(move || {
                let stdin = io::stdin();
                let mut reader = stdin.lock();
                let mut line = Vec::with_capacity(MAX_LINE);
                loop {
                    line.clear();
                    match read_bounded_line(&mut reader, &mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {
                            let text = String::from_utf8_lossy(&line);
                            if let Some(command) = parse_command(&text)
                                && tx.send(Event::Command(command)).is_err()
                            {
                                return;
                            }
                        }
                    }
                }
                let _ = tx.send(Event::StdinClosed);
            });
        if spawned.is_err() {
            // Without a reader the pipe cannot be watched; the poll on the parent still runs.
            tracing::warn!("reaper could not start its stdin reader thread");
        }
    }

    /// Reads one line into `line`, keeping at most [`MAX_LINE`] bytes of it. Returns the
    /// number of bytes consumed (0 at end-of-file).
    fn read_bounded_line(reader: &mut impl BufRead, line: &mut Vec<u8>) -> io::Result<usize> {
        let mut consumed = 0;
        loop {
            let buffer = reader.fill_buf()?;
            if buffer.is_empty() {
                return Ok(consumed);
            }
            let (chunk, done) = match buffer.iter().position(|&b| b == b'\n') {
                Some(end) => (&buffer[..=end], true),
                None => (buffer, false),
            };
            let room = MAX_LINE.saturating_sub(line.len());
            line.extend_from_slice(&chunk[..chunk.len().min(room)]);
            let used = chunk.len();
            reader.consume(used);
            consumed += used;
            if done {
                return Ok(consumed);
            }
        }
    }

    fn parent_alive(parent: libc::pid_t) -> bool {
        if parent <= 1 {
            return false;
        }
        // SAFETY: getppid has no preconditions. Once the app dies the reaper is re-parented,
        // which makes this check immune to the app's PID being reused.
        if unsafe { libc::getppid() } != parent {
            return false;
        }
        // SAFETY: signal 0 only checks that the process exists.
        let rc = unsafe { libc::kill(parent, 0) };
        rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    #[cfg(target_os = "macos")]
    fn watch_parent(parent: libc::pid_t, tx: mpsc::Sender<Event>) {
        let spawned = std::thread::Builder::new()
            .name("reaper-kqueue".into())
            .spawn(move || {
                if wait_for_exit_kqueue(parent) {
                    let _ = tx.send(Event::ParentExited);
                }
            });
        if spawned.is_err() {
            tracing::warn!("reaper could not start its kqueue thread; polling only");
        }
    }

    /// Blocks until `parent` exits. Returns false if kqueue could not be used, in which case
    /// the 500 ms poll in [`run`] still notices the exit.
    #[cfg(target_os = "macos")]
    fn wait_for_exit_kqueue(parent: libc::pid_t) -> bool {
        if parent <= 1 {
            return true;
        }
        // SAFETY: kqueue has no preconditions and returns -1 on failure.
        let kq = unsafe { libc::kqueue() };
        if kq < 0 {
            return false;
        }
        let ident = usize::try_from(parent).unwrap_or(0);
        let change = libc::kevent {
            ident,
            filter: libc::EVFILT_PROC,
            flags: libc::EV_ADD | libc::EV_ONESHOT,
            fflags: libc::NOTE_EXIT,
            data: 0,
            udata: std::ptr::null_mut(),
        };
        // SAFETY: an all-zero kevent is a valid value for the output buffer.
        let mut event: libc::kevent = unsafe { std::mem::zeroed() };
        let exited = loop {
            // SAFETY: one valid change record in, room for one event out, no timeout; kq is a
            // descriptor this function owns.
            let rc = unsafe { libc::kevent(kq, &change, 1, &mut event, 1, std::ptr::null()) };
            if rc > 0 {
                // EV_ERROR with ESRCH means the parent was already gone at registration.
                let error = (event.flags & libc::EV_ERROR) != 0;
                break !error || event.data == libc::ESRCH as isize;
            }
            if rc < 0 {
                match io::Error::last_os_error().raw_os_error() {
                    Some(libc::EINTR) => continue,
                    // The parent was already gone when the watch was registered.
                    Some(libc::ESRCH) => break true,
                    _ => {}
                }
            }
            break false;
        };
        // SAFETY: kq is open and owned here.
        unsafe {
            libc::close(kq);
        }
        exited
    }

    #[cfg(not(target_os = "macos"))]
    fn watch_parent(_parent: libc::pid_t, _tx: mpsc::Sender<Event>) {
        // Linux is a development platform: the poll in `run` is enough.
    }

    fn terminate_groups(groups: &GroupSet, grace: Duration) {
        if groups.is_empty() {
            return;
        }
        for pgid in groups.groups() {
            signal_group(pgid, libc::SIGTERM);
        }
        let deadline = Instant::now() + grace;
        while Instant::now() < deadline && groups.groups().any(group_alive) {
            std::thread::sleep(Duration::from_millis(50));
        }
        for pgid in groups.groups().filter(|&pgid| group_alive(pgid)) {
            signal_group(pgid, libc::SIGKILL);
        }
    }

    fn signal_group(pgid: u32, signal: libc::c_int) {
        let Ok(pgid) = libc::pid_t::try_from(pgid) else {
            return;
        };
        if pgid <= 1 {
            return;
        }
        // SAFETY: killpg with a positive group id that is not init's; failure is ignored
        // because the group may already be gone.
        unsafe {
            libc::killpg(pgid, signal);
        }
    }

    /// The app side: the reaper process and the write end of its stdin.
    #[derive(Debug)]
    pub(super) struct Client {
        // Held so the process is reaped when the app drops the client; never killed.
        child: Mutex<ManagedChild>,
        pid: Option<u32>,
        stdin: Mutex<std::fs::File>,
    }

    impl Client {
        pub(super) fn start(app_exe: &Path, hidden_flag: &str) -> io::Result<Self> {
            let cwd = app_exe
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or(Path::new("/"));
            let spec = SpawnSpec::new(app_exe, cwd)
                .arg(hidden_flag)
                .arg(std::process::id().to_string())
                .env(EnvSpec::minimal())
                .stdin(StdioMode::Piped)
                .stdout(StdioMode::Null)
                .stderr(StdioMode::Null);
            let mut child = spawn::spawn_reaper(&spec)?;
            let stdin = child
                .take_stdin()
                .ok_or_else(|| io::Error::other("reaper started without a stdin pipe"))?;
            // A blocking descriptor: writes are tiny (< PIPE_BUF, so atomic) and the reaper
            // reads continuously.
            let stdin = std::fs::File::from(stdin.into_owned_fd()?);
            let pid = child.id();
            Ok(Self {
                child: Mutex::new(child),
                pid,
                stdin: Mutex::new(stdin),
            })
        }

        pub(super) fn send(&self, line: &str) -> io::Result<()> {
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| io::Error::other("reaper pipe lock poisoned"))?;
            stdin.write_all(line.as_bytes())?;
            stdin.flush()
        }

        pub(super) fn pid(&self) -> Option<u32> {
            self.pid
        }
    }

    impl Drop for Client {
        fn drop(&mut self) {
            // Closing stdin (when `stdin` drops) tells the reaper to clean up and exit; the
            // child handle is not killed on drop, and Tokio reaps it in the background.
            if let Ok(mut child) = self.child.lock() {
                let _ = child.try_wait();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn bounded_reader_splits_lines_and_truncates_long_ones() {
            let long = "x".repeat(200);
            let input = format!("add 5\n{long}\nremove 5");
            let mut reader = io::BufReader::with_capacity(16, input.as_bytes());
            let mut line = Vec::new();
            assert_eq!(read_bounded_line(&mut reader, &mut line).expect("read"), 6);
            assert_eq!(line, b"add 5\n");
            line.clear();
            assert_eq!(
                read_bounded_line(&mut reader, &mut line).expect("read"),
                201
            );
            assert_eq!(line.len(), MAX_LINE);
            line.clear();
            assert_eq!(read_bounded_line(&mut reader, &mut line).expect("read"), 8);
            assert_eq!(line, b"remove 5");
            line.clear();
            assert_eq!(read_bounded_line(&mut reader, &mut line).expect("read"), 0);
        }

        #[test]
        fn terminate_groups_ignores_an_empty_set() {
            terminate_groups(&GroupSet::new([]), Duration::from_millis(10));
        }

        #[test]
        fn parent_alive_is_false_for_reserved_ids() {
            assert!(!parent_alive(0));
            assert!(!parent_alive(1));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_add_and_remove() {
        assert_eq!(parse_command("add 1234"), Some(ReaperCommand::Add(1234)));
        assert_eq!(
            parse_command("remove 1234\n"),
            Some(ReaperCommand::Remove(1234))
        );
        assert_eq!(
            parse_command("  add   77\r\n"),
            Some(ReaperCommand::Add(77))
        );
    }

    #[test]
    fn rejects_malformed_lines() {
        for line in [
            "",
            "add",
            "add x",
            "add -5",
            "add 0",
            "add 1",
            "kill 5",
            "add 5 6",
            "ADD 5",
            "add 4294967295",
            "add 5.0",
        ] {
            assert_eq!(parse_command(line), None, "{line:?}");
        }
    }

    #[test]
    fn group_set_bookkeeping() {
        let mut set = GroupSet::new([42]);
        assert!(set.is_empty());
        assert!(set.apply(ReaperCommand::Add(100)));
        assert!(
            !set.apply(ReaperCommand::Add(100)),
            "duplicate add changes nothing"
        );
        assert!(set.apply(ReaperCommand::Add(7)));
        assert_eq!(set.groups().collect::<Vec<_>>(), vec![7, 100]);
        assert!(set.apply(ReaperCommand::Remove(100)));
        assert!(
            !set.apply(ReaperCommand::Remove(100)),
            "removing an unknown group changes nothing"
        );
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn group_set_refuses_protected_ids() {
        let mut set = GroupSet::new([42]);
        assert!(!set.apply(ReaperCommand::Add(42)), "own or app group");
        assert!(!set.apply(ReaperCommand::Add(0)));
        assert!(!set.apply(ReaperCommand::Add(1)));
        assert!(set.is_empty());
    }
}
