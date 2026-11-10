//! The startup sweep of children that outlived a previous run (ARCHITECTURE.md 4.1).
//!
//! The app records every long-lived child (llama-server, MCP servers) in a small JSON file
//! in its local data folder. At the next launch it opens the file, kills any recorded
//! process that is still alive **and** still runs the same executable (and, where the OS
//! reports it, started at the same time), then starts a fresh list.
//!
//! A PID whose executable does not match is never touched: PIDs are reused, and the
//! process now holding one may belong to anything.
//!
//! Platform support: Linux reads `/proc`, macOS uses `proc_pidpath` / `proc_pidinfo`,
//! Windows uses `QueryFullProcessImageNameW` / `GetProcessTimes`. No extra crate is needed.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// One recorded child.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRecord {
    /// Process id.
    pub pid: u32,
    /// The executable the child was started from, as recorded.
    pub exe: PathBuf,
    /// OS-specific start time of the process, when it could be read at registration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_time: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    #[serde(default)]
    children: Vec<ChildRecord>,
}

/// The persistent list of child PIDs, backed by a JSON file.
///
/// All methods take `&self`, so one registry can be shared behind an `Arc`. Every change is
/// written to disk at once (write to a temporary file, then rename).
#[derive(Debug)]
pub struct ChildRegistry {
    path: PathBuf,
    children: Mutex<Vec<ChildRecord>>,
}

impl ChildRegistry {
    /// Opens the registry at `path`. A missing file is an empty registry; an unreadable or
    /// corrupt one is logged and treated as empty, so the app always starts.
    pub fn open(path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        let children = match fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<RegistryFile>(&bytes) {
                Ok(file) => file.children,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "child registry is corrupt; ignoring it");
                    Vec::new()
                }
            },
            Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not read child registry");
                Vec::new()
            }
        };
        Self {
            path,
            children: Mutex::new(children),
        }
    }

    /// The file this registry is stored in.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A copy of the current records.
    pub fn records(&self) -> Vec<ChildRecord> {
        self.lock().clone()
    }

    /// Records a running child. Its start time is read from the OS now, so a later sweep can
    /// tell it apart from an unrelated process that reuses the PID.
    pub fn register(&self, pid: u32, exe: &Path) -> io::Result<()> {
        let record = ChildRecord {
            pid,
            exe: exe.to_path_buf(),
            start_time: os::start_time(pid),
        };
        let mut children = self.lock();
        children.retain(|child| child.pid != pid);
        children.push(record);
        self.persist(&children)
    }

    /// Forgets a child (it exited or was stopped).
    pub fn unregister(&self, pid: u32) -> io::Result<()> {
        let mut children = self.lock();
        let before = children.len();
        children.retain(|child| child.pid != pid);
        if children.len() == before {
            return Ok(());
        }
        self.persist(&children)
    }

    /// Kills every recorded process that is still alive and still matches its record by
    /// executable path (and start time, where both are known), then clears the list.
    /// Returns the PIDs that were killed. This process's own PID is never killed.
    pub fn sweep_stale(&self) -> Vec<u32> {
        let mut children = self.lock();
        let own = std::process::id();
        let mut killed = Vec::new();
        for record in children.iter() {
            if record.pid == own || record.pid == 0 {
                continue;
            }
            if !matches_live_process(record) {
                continue;
            }
            match os::kill_tree(record.pid) {
                Ok(()) => killed.push(record.pid),
                Err(error) => {
                    tracing::warn!(pid = record.pid, %error, "could not kill stale child");
                }
            }
        }
        children.clear();
        if let Err(error) = self.persist(&children) {
            tracing::warn!(path = %self.path.display(), %error, "could not clear child registry");
        }
        killed
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<ChildRecord>> {
        // A panic while holding the lock leaves a valid Vec; keep using it.
        self.children
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn persist(&self, children: &[ChildRecord]) -> io::Result<()> {
        let file = RegistryFile {
            children: children.to_vec(),
        };
        let bytes = serde_json::to_vec_pretty(&file).map_err(io::Error::other)?;
        if let Some(dir) = self.path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            fs::create_dir_all(dir)?;
        }
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        {
            let mut out = fs::File::create(&tmp)?;
            out.write_all(&bytes)?;
            out.sync_all()?;
        }
        fs::rename(&tmp, &self.path)
    }
}

/// Whether process `pid` exists and has not exited. A zombie (exited but not yet reaped)
/// counts as gone on Linux and Windows; on macOS it still counts as alive until reaped.
pub fn process_alive(pid: u32) -> bool {
    pid != 0 && os::alive(pid)
}

/// Whether the process `record.pid` is alive and runs the recorded executable.
fn matches_live_process(record: &ChildRecord) -> bool {
    if !process_alive(record.pid) {
        return false;
    }
    let Some(actual) = os::exe_path(record.pid) else {
        return false;
    };
    if !same_executable(&record.exe, &actual) {
        return false;
    }
    match (record.start_time, os::start_time(record.pid)) {
        (Some(recorded), Some(now)) => recorded == now,
        // The start time could not be read on one side; the executable match decides.
        _ => true,
    }
}

/// Compares two executable paths, canonicalising both when possible. Case-insensitive on
/// Windows and macOS, whose default file systems are.
fn same_executable(recorded: &Path, actual: &Path) -> bool {
    let canon = |p: &Path| fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let a = canon(recorded);
    let b = canon(actual);
    if cfg!(any(windows, target_os = "macos")) {
        let a = strip_verbatim(&a.to_string_lossy()).to_lowercase();
        let b = strip_verbatim(&b.to_string_lossy()).to_lowercase();
        a == b
    } else {
        a == b
    }
}

/// Removes the Windows `\\?\` prefix that `canonicalize` adds, so it compares equal to the
/// form the OS reports for a running process.
fn strip_verbatim(path: &str) -> &str {
    path.strip_prefix(r"\\?\").unwrap_or(path)
}

#[cfg(target_os = "linux")]
mod os {
    use std::io;
    use std::path::PathBuf;

    pub(super) fn exe_path(pid: u32) -> Option<PathBuf> {
        let link = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
        // A replaced binary reads as "<path> (deleted)"; it is still the same program.
        let text = link.to_string_lossy();
        Some(match text.strip_suffix(" (deleted)") {
            Some(stripped) => PathBuf::from(stripped),
            None => link,
        })
    }

    pub(super) fn alive(pid: u32) -> bool {
        let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
            return false;
        };
        let state = stat
            .rfind(')')
            .and_then(|end| stat[end + 1..].split_ascii_whitespace().next());
        !matches!(state, None | Some("Z" | "X" | "x"))
    }

    /// Field 22 of /proc/<pid>/stat: start time in clock ticks since boot.
    pub(super) fn start_time(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        parse_stat_start_time(&stat)
    }

    pub(super) fn parse_stat_start_time(stat: &str) -> Option<u64> {
        // The command name (field 2) is in parentheses and may contain spaces or ')'.
        let after = &stat[stat.rfind(')')? + 1..];
        after.split_ascii_whitespace().nth(19)?.parse().ok()
    }

    pub(super) fn kill_tree(pid: u32) -> io::Result<()> {
        super::unix_kill_tree(pid)
    }
}

#[cfg(target_os = "macos")]
mod os {
    use std::io;
    use std::path::PathBuf;

    pub(super) fn alive(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let rc = unsafe { libc::kill(pid, 0) };
        rc == 0 || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    pub(super) fn exe_path(pid: u32) -> Option<PathBuf> {
        let pid = libc::c_int::try_from(pid).ok()?;
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let size = u32::try_from(buf.len()).ok()?;
        // SAFETY: buf is valid for `size` bytes; proc_pidpath writes at most that many.
        let len = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr().cast(), size) };
        if len <= 0 {
            return None;
        }
        buf.truncate(usize::try_from(len).ok()?);
        use std::os::unix::ffi::OsStringExt;
        Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
    }

    /// Start time in microseconds since the epoch, from `proc_bsdinfo`.
    pub(super) fn start_time(pid: u32) -> Option<u64> {
        let pid = libc::c_int::try_from(pid).ok()?;
        // SAFETY: an all-zero proc_bsdinfo is a valid value for the output buffer.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: info is valid for `size` bytes, the size proc_pidinfo expects for PROC_PIDTBSDINFO.
        let written = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        if written != size {
            return None;
        }
        Some(
            info.pbi_start_tvsec
                .saturating_mul(1_000_000)
                .saturating_add(info.pbi_start_tvusec),
        )
    }

    pub(super) fn kill_tree(pid: u32) -> io::Result<()> {
        super::unix_kill_tree(pid)
    }
}

/// Kills the process, and its whole group when it leads one (every child the spawn module
/// starts on Unix does).
#[cfg(unix)]
fn unix_kill_tree(pid: u32) -> io::Result<()> {
    let pid = libc::pid_t::try_from(pid).map_err(io::Error::other)?;
    if pid <= 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "refusing to kill pid 0 or 1",
        ));
    }
    // SAFETY: getpgid has no preconditions and returns -1 on failure.
    let pgid = unsafe { libc::getpgid(pid) };
    // SAFETY: getpgrp has no preconditions.
    let own_group = unsafe { libc::getpgrp() };
    // SAFETY: plain signal delivery to a positive id that is not init and not our own group.
    let rc = if pgid == pid && pgid != own_group {
        unsafe { libc::killpg(pgid, libc::SIGKILL) }
    } else {
        unsafe { libc::kill(pid, libc::SIGKILL) }
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(windows)]
mod os {
    use std::ffi::OsString;
    use std::io;
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;

    use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS,
        PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        QueryFullProcessImageNameW, TerminateProcess,
    };
    use windows::core::PWSTR;

    struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: the handle came from OpenProcess and is closed exactly once.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<Handle> {
        // SAFETY: OpenProcess has no memory preconditions; failure is returned as an error.
        unsafe { OpenProcess(access, false, pid) }.ok().map(Handle)
    }

    /// `GetExitCodeProcess` reports this while the process runs.
    const STILL_ACTIVE: u32 = 259;

    pub(super) fn alive(pid: u32) -> bool {
        let Some(handle) = open(pid, PROCESS_QUERY_LIMITED_INFORMATION) else {
            return false;
        };
        let mut code = 0u32;
        // SAFETY: the handle has query access and `code` is a valid out-pointer.
        unsafe { GetExitCodeProcess(handle.0, &mut code) }.is_ok() && code == STILL_ACTIVE
    }

    pub(super) fn exe_path(pid: u32) -> Option<PathBuf> {
        let handle = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut buf = vec![0u16; 32_768];
        let mut len = u32::try_from(buf.len()).ok()?;
        // SAFETY: buf is valid for `len` UTF-16 units and `len` is updated to the length written.
        unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_WIN32,
                PWSTR(buf.as_mut_ptr()),
                &mut len,
            )
        }
        .ok()?;
        buf.truncate(usize::try_from(len).ok()?);
        Some(PathBuf::from(OsString::from_wide(&buf)))
    }

    /// Creation time as a FILETIME value (100 ns intervals since 1601).
    pub(super) fn start_time(pid: u32) -> Option<u64> {
        let handle = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: all four out-pointers are valid FILETIME values.
        unsafe { GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user) }
            .ok()?;
        Some((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime))
    }

    pub(super) fn kill_tree(pid: u32) -> io::Result<()> {
        // On Windows a stale child normally cannot exist: its Job Object died with the app.
        // Should one survive anyway, only that process is terminated.
        let handle = open(pid, PROCESS_TERMINATE).ok_or_else(io::Error::last_os_error)?;
        // SAFETY: the handle has PROCESS_TERMINATE access.
        unsafe { TerminateProcess(handle.0, 1) }.map_err(io::Error::other)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod os {
    use std::io;
    use std::path::PathBuf;

    pub(super) fn alive(_pid: u32) -> bool {
        false
    }

    pub(super) fn exe_path(_pid: u32) -> Option<PathBuf> {
        None
    }

    pub(super) fn start_time(_pid: u32) -> Option<u64> {
        None
    }

    pub(super) fn kill_tree(_pid: u32) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "stale-child sweep is not supported on this OS",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_is_empty_and_corrupt_file_is_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("children.json");
        assert!(ChildRegistry::open(&path).records().is_empty());
        fs::write(&path, b"{not json").expect("write");
        let registry = ChildRegistry::open(&path);
        assert!(registry.records().is_empty());
        assert!(registry.sweep_stale().is_empty());
    }

    #[test]
    fn register_and_unregister_persist() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join("children.json");
        let registry = ChildRegistry::open(&path);
        registry
            .register(4_194_400, Path::new("/bin/a"))
            .expect("register");
        registry
            .register(4_194_401, Path::new("/bin/b"))
            .expect("register");
        registry
            .register(4_194_400, Path::new("/bin/c"))
            .expect("re-register replaces");
        let reopened = ChildRegistry::open(&path);
        let records = reopened.records();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].pid, 4_194_400);
        assert_eq!(records[1].exe, PathBuf::from("/bin/c"));
        reopened.unregister(4_194_401).expect("unregister");
        reopened
            .unregister(4_194_401)
            .expect("unregister twice is fine");
        assert_eq!(ChildRegistry::open(&path).records().len(), 1);
    }

    #[test]
    fn sweep_skips_dead_pids_and_own_pid_and_clears_the_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("children.json");
        let registry = ChildRegistry::open(&path);
        let own_exe = std::env::current_exe().expect("current exe");
        registry
            .register(std::process::id(), &own_exe)
            .expect("register self");
        // Far above any default pid_max, so nothing lives there.
        registry
            .register(4_194_402, &own_exe)
            .expect("register dead");
        let reopened = ChildRegistry::open(&path);
        assert!(reopened.sweep_stale().is_empty());
        assert!(ChildRegistry::open(&path).records().is_empty());
    }

    #[test]
    fn same_executable_compares_canonical_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("prog");
        fs::write(&file, b"x").expect("write");
        let dotted = dir.path().join(".").join("prog");
        assert!(same_executable(&file, &dotted));
        assert!(!same_executable(&file, &dir.path().join("other")));
    }

    #[test]
    fn strips_verbatim_prefix() {
        assert_eq!(strip_verbatim(r"\\?\C:\x\y.exe"), r"C:\x\y.exe");
        assert_eq!(strip_verbatim(r"C:\x"), r"C:\x");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_proc_stat_with_awkward_command_names() {
        let stat = "1234 (weird) name)) S 1 1234 1234 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10";
        assert_eq!(os::parse_stat_start_time(stat), Some(987_654));
        assert_eq!(os::parse_stat_start_time("garbage"), None);
        assert_eq!(os::parse_stat_start_time("1 (x) S 1"), None);
    }

    #[test]
    fn own_process_is_alive_and_absurd_pids_are_not() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
        assert!(!process_alive(4_194_403));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn reads_own_exe_and_start_time() {
        let pid = std::process::id();
        let exe = os::exe_path(pid).expect("own exe");
        assert!(same_executable(
            &exe,
            &std::env::current_exe().expect("current exe")
        ));
        assert!(os::start_time(pid).is_some());
    }
}
