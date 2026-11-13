//! Where reverse engineering lives on disk (item5-spec.md section 2 and R8):
//!
//! ```text
//! <app local data>/reverse/
//!   rea-6.9.0/  node-24.22.3/  kuna-1.990/  ghidra-12.1.4/  java-21.0.13+9/
//!   home/   tmp/   cache/kuna/   work/   .downloads/   .tmp-<dir>/
//! ```
//!
//! The root is inside the app's local data folder, which the path sandbox forbids in every
//! mode (13.4 step 1). In debug builds only, `HARNESS_REVERSE_ROOT` replaces it (the e2e
//! scenario points it at a fake pack), and the app then forbids that folder too
//! ([`extra_forbidden`]).
//!
//! One folder can lie outside the root, on Linux only ([`ReverseLayout::run_tmp`]): rea's
//! temporary folder while Ghidra is the decompiler, `<XDG_RUNTIME_DIR>/<app folder>-reverse`.
//! The app's folder on Linux is under `~/.local/share`, and rea moves Ghidra's session (its
//! project, logs, a copy of the opened file and the bridge socket) to `/tmp` when its
//! temporary folder has a folder whose name starts with a dot, because Ghidra refuses such
//! project paths (rea's `GhidraSessionRoot.js`). The app forbids that folder to the tools as
//! well ([`run_tmp_forbidden`]); rea's process makes it afresh at each start and removes it
//! at each stop and when the app starts.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// Debug builds only: replaces the reverse root.
pub const ROOT_ENV: &str = "HARNESS_REVERSE_ROOT";

/// The marker written last into a part's folder.
pub const COMPLETE_MARKER: &str = ".complete";

/// Added to the app folder's name for rea's temporary folder under `XDG_RUNTIME_DIR`.
pub const RUN_TMP_SUFFIX: &str = "-reverse";
/// The longest path rea's temporary folder under `XDG_RUNTIME_DIR` may have: Ghidra's
/// session folder in it (`rea-ghidra-` and 6 characters) holds the bridge socket
/// `bridge.sock`, and Java takes at most 106 bytes for a socket's path on Linux (rea moves a
/// longer one to `/tmp`).
pub const RUN_TMP_MAX_BYTES: usize = 106 - "/rea-ghidra-XXXXXX/bridge.sock".len();
/// The free space `XDG_RUNTIME_DIR` must have for rea's temporary folder: room for Ghidra's
/// project of a typical program (tens of MB) and the copy of the opened file. The system
/// keeps that folder in memory, limited by default to a tenth of it, so computers with about
/// 2.5 GB of memory or more have room; a smaller one is left alone.
pub const RUN_TMP_MIN_FREE: u64 = 256 << 20;

/// The reverse-engineering folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseLayout {
    root: PathBuf,
    /// Linux: rea's temporary folder while Ghidra is the decompiler ([`Self::run_tmp`]).
    run_tmp: Option<PathBuf>,
}

impl ReverseLayout {
    /// A layout rooted at `root` exactly (tests, and the debug override), with no folder
    /// outside it.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            run_tmp: None,
        }
    }

    /// The app's layout: `<local_data>/reverse`, or `HARNESS_REVERSE_ROOT` in a debug build;
    /// on Linux with rea's temporary folder for Ghidra under `XDG_RUNTIME_DIR` when that can
    /// be used ([`run_tmp_in`]).
    pub fn for_app(local_data: &Path) -> Self {
        let root = match root_override(std::env::var_os(ROOT_ENV), cfg!(debug_assertions)) {
            Some(root) => root,
            None => local_data.join("reverse"),
        };
        Self::new(root).with_run_tmp(app_run_tmp(local_data))
    }

    /// The same layout with this temporary folder for Ghidra outside the root (tests; the
    /// app's comes from [`Self::for_app`]).
    pub fn with_run_tmp(mut self, run_tmp: Option<PathBuf>) -> Self {
        self.run_tmp = run_tmp;
        self
    }

    /// The root folder.
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Linux only: rea's `TMPDIR` while Ghidra is the decompiler,
    /// `<XDG_RUNTIME_DIR>/<app folder>-reverse`, made afresh (mode 0700) at each start of rea
    /// and removed at each stop and when the app starts. `None` elsewhere, and when that
    /// folder cannot be used: then rea gets [`Self::tmp`] and, on Linux, moves Ghidra's
    /// session to `/tmp` (a private folder it removes at the stop).
    pub fn run_tmp(&self) -> Option<&Path> {
        self.run_tmp.as_deref()
    }
    /// Resumable downloads (`<file>.<first 16 hex of sha256>.part`).
    pub fn downloads(&self) -> PathBuf {
        self.root.join(".downloads")
    }
    /// A part's folder (`rea-6.9.0`).
    pub fn part_dir(&self, dir: &str) -> PathBuf {
        self.root.join(dir)
    }
    /// Where a part is unpacked before it is renamed into place.
    pub fn tmp_part_dir(&self, dir: &str) -> PathBuf {
        self.root.join(format!(".tmp-{dir}"))
    }
    /// `HOME`, `USERPROFILE`, `APPDATA`, `LOCALAPPDATA` and `XDG_*` for rea, Ghidra and Java.
    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    /// `TEMP`, `TMP` or `TMPDIR` (emptied at each start of rea).
    pub fn tmp(&self) -> PathBuf {
        self.root.join("tmp")
    }
    /// The cache folder.
    pub fn cache(&self) -> PathBuf {
        self.root.join("cache")
    }
    /// Kuna's exports (`REA_KUNA_CACHE_DIR`, capped at 1 GiB).
    pub fn kuna_cache(&self) -> PathBuf {
        self.cache().join("kuna")
    }
    /// rea's working folder (empty).
    pub fn work(&self) -> PathBuf {
        self.root.join("work")
    }
}

/// The override, honoured only in debug builds and only for an absolute path.
fn root_override(value: Option<OsString>, debug: bool) -> Option<PathBuf> {
    if !debug {
        return None;
    }
    let path = PathBuf::from(value?);
    (path.is_absolute()).then_some(path)
}

/// Folders the app adds to the tools' forbidden folders: the debug override's root when it
/// is set (the normal root is already inside the forbidden local data folder). Empty in a
/// release build.
pub fn extra_forbidden() -> Vec<PathBuf> {
    root_override(std::env::var_os(ROOT_ENV), cfg!(debug_assertions))
        .into_iter()
        .collect()
}

/// Linux only: rea's temporary folder for Ghidra under `XDG_RUNTIME_DIR`, for the tools'
/// forbidden folders. Named whenever `XDG_RUNTIME_DIR` is an absolute path, whether or not
/// rea can use it ([`run_tmp_in`]), so it is forbidden in every case.
pub fn run_tmp_forbidden(local_data: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    run_tmp_candidate(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), local_data)
}

/// `<runtime>/<name of local_data>-reverse` when `runtime` is an absolute path.
fn run_tmp_candidate(runtime: Option<&OsStr>, local_data: &Path) -> Option<PathBuf> {
    let runtime = Path::new(runtime?);
    let name = local_data.file_name()?;
    runtime.is_absolute().then(|| run_tmp_path(runtime, name))
}

fn run_tmp_path(runtime: &Path, name: &OsStr) -> PathBuf {
    let mut folder = name.to_os_string();
    folder.push(RUN_TMP_SUFFIX);
    runtime.join(folder)
}

/// The app's rea temporary folder for Ghidra (Linux only).
fn app_run_tmp(local_data: &Path) -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let name = local_data.file_name()?.to_str()?;
    run_tmp_in(
        std::env::var_os("XDG_RUNTIME_DIR").as_deref(),
        name,
        &crate::diskspace::free_bytes,
    )
}

/// Whether a path has a folder whose name starts with a dot (rea and Ghidra refuse those).
fn dotted(path: &Path) -> bool {
    path.components()
        .any(|c| matches!(c, Component::Normal(n) if n.to_string_lossy().starts_with('.')))
}

/// `<runtime>/<name>-reverse`, when rea can use it as its temporary folder for Ghidra and
/// it stays private: `runtime` (the system's `XDG_RUNTIME_DIR`) is an absolute path in plain
/// text with no `.` or `..` part; it is a real folder (not a link) of this user's, closed
/// to everyone else (mode 0700), with at least [`RUN_TMP_MIN_FREE`] free (`free`, or unknown
/// and refused); `name` is a plain name not starting with a dot; and the folder, as written
/// and as the system resolves it, has no folder whose name starts with a dot and is at most
/// [`RUN_TMP_MAX_BYTES`] long. `None` otherwise, and always on Windows.
pub fn run_tmp_in(
    runtime: Option<&OsStr>,
    name: &str,
    free: &dyn Fn(&Path) -> Option<u64>,
) -> Option<PathBuf> {
    let text = runtime?.to_str()?;
    let plain = |s: &str| !s.is_empty() && !s.chars().any(char::is_control);
    if !plain(text)
        || !text.starts_with('/')
        || text.split('/').any(|part| part == "." || part == "..")
        || !plain(name)
        || name.starts_with('.')
        || name.contains(['/', '\\'])
    {
        return None;
    }
    let runtime = Path::new(text);
    if !private_folder(runtime) || free(runtime)? < RUN_TMP_MIN_FREE {
        return None;
    }
    let folder = run_tmp_path(runtime, OsStr::new(name));
    let resolved = run_tmp_path(&std::fs::canonicalize(runtime).ok()?, OsStr::new(name));
    for path in [&folder, &resolved] {
        if dotted(path) || path.to_str()?.len() > RUN_TMP_MAX_BYTES {
            return None;
        }
    }
    Some(folder)
}

/// Whether `path` is a real folder (not a link) owned by this user and closed to everyone
/// else (mode 0700). Always false on Windows.
pub(crate) fn private_folder(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).is_ok_and(|m| {
            m.file_type().is_dir() && m.uid() == effective_uid() && m.mode() & 0o777 == 0o700
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

/// This process's effective user id.
#[cfg(unix)]
fn effective_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_are_under_the_root() {
        let l = ReverseLayout::new("/data/app/reverse");
        assert_eq!(l.downloads(), Path::new("/data/app/reverse/.downloads"));
        assert_eq!(
            l.part_dir("rea-6.9.0"),
            Path::new("/data/app/reverse/rea-6.9.0")
        );
        assert_eq!(
            l.tmp_part_dir("java-21.0.13+9"),
            Path::new("/data/app/reverse/.tmp-java-21.0.13+9")
        );
        assert_eq!(l.kuna_cache(), Path::new("/data/app/reverse/cache/kuna"));
        for p in [l.home(), l.tmp(), l.work(), l.cache()] {
            assert!(p.starts_with(l.root()));
        }
    }

    #[test]
    fn a_release_build_ignores_the_override() {
        let abs = std::env::temp_dir().join("fake-pack");
        assert_eq!(root_override(Some(abs.clone().into()), false), None);
        assert_eq!(root_override(Some(abs.clone().into()), true), Some(abs));
        assert_eq!(root_override(None, true), None);
        // A relative path would depend on the working folder: refused.
        assert_eq!(root_override(Some("relative/pack".into()), true), None);
    }

    #[test]
    fn the_app_root_is_inside_local_data_without_the_override() {
        if std::env::var_os(ROOT_ENV).is_some() {
            return;
        }
        let local = std::env::temp_dir().join("app.harness");
        assert_eq!(ReverseLayout::for_app(&local).root(), local.join("reverse"));
        assert!(extra_forbidden().is_empty());
    }

    #[test]
    fn only_the_app_layout_on_linux_has_a_folder_outside_the_root() {
        let l = ReverseLayout::new("/data/app/reverse");
        assert_eq!(l.run_tmp(), None);
        let l = l.with_run_tmp(Some("/run/user/1000/app-reverse".into()));
        assert_eq!(l.run_tmp(), Some(Path::new("/run/user/1000/app-reverse")));
        assert_eq!(l.root(), Path::new("/data/app/reverse"));
        let local = std::env::temp_dir().join("app.harness");
        if !cfg!(target_os = "linux") {
            assert_eq!(ReverseLayout::for_app(&local).run_tmp(), None);
            assert_eq!(run_tmp_forbidden(&local), None);
        }
        // Whatever the app uses is what it forbids.
        if let Some(used) = ReverseLayout::for_app(&local).run_tmp() {
            assert_eq!(run_tmp_forbidden(&local).as_deref(), Some(used));
        }
    }

    #[test]
    fn the_forbidden_candidate_needs_only_an_absolute_runtime_folder() {
        let local = Path::new("/home/u/.local/share/io.example.app");
        assert_eq!(
            run_tmp_candidate(Some(OsStr::new("/run/user/1000")), local),
            Some(PathBuf::from("/run/user/1000/io.example.app-reverse"))
        );
        assert_eq!(run_tmp_candidate(Some(OsStr::new("run/user")), local), None);
        assert_eq!(run_tmp_candidate(None, local), None);
    }

    /// A private folder (mode 0700) for the runtime folder tests.
    #[cfg(unix)]
    fn private_dir(path: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(path).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::canonicalize(path).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_runtime_folder_is_used_only_when_it_is_private_and_fits_rea() {
        use std::os::unix::fs::PermissionsExt;
        let plenty = |_: &Path| Some(RUN_TMP_MIN_FREE);
        let base = tempfile::Builder::new().prefix("rt-").tempdir().unwrap();
        let base = std::fs::canonicalize(base.path()).unwrap();
        if dotted(&base) {
            // A temporary folder under a dot folder cannot hold the good case.
            return;
        }
        let run = private_dir(&base.join("run"));
        let os = |p: &Path| p.as_os_str().to_os_string();
        let good = run_tmp_in(Some(&os(&run)), "io.example.app", &plenty);
        assert_eq!(good, Some(run.join("io.example.app-reverse")));
        // The forbidden folder is the same folder.
        assert_eq!(
            run_tmp_candidate(Some(&os(&run)), Path::new("/x/io.example.app")),
            good
        );

        // Not set, relative, or with a `.` or `..` part.
        assert_eq!(run_tmp_in(None, "app", &plenty), None);
        assert_eq!(
            run_tmp_in(Some(OsStr::new("run/user")), "app", &plenty),
            None
        );
        let dots = format!("{}/../run", run.display());
        assert_eq!(run_tmp_in(Some(OsStr::new(&dots)), "app", &plenty), None);
        let dot = format!("{}/./", run.display());
        assert_eq!(run_tmp_in(Some(OsStr::new(&dot)), "app", &plenty), None);
        // A name that is not plain.
        for name in ["", ".app", "a/b", "a\\b", "a\nb"] {
            assert_eq!(run_tmp_in(Some(&os(&run)), name, &plenty), None, "{name:?}");
        }
        // Missing, or a file.
        assert_eq!(
            run_tmp_in(Some(&os(&base.join("none"))), "app", &plenty),
            None
        );
        std::fs::write(base.join("file"), b"x").unwrap();
        assert_eq!(
            run_tmp_in(Some(&os(&base.join("file"))), "app", &plenty),
            None
        );
        // Open to others.
        let open = private_dir(&base.join("open"));
        std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(run_tmp_in(Some(&os(&open)), "app", &plenty), None);
        // A link to a private folder.
        let link = base.join("link");
        std::os::unix::fs::symlink(&run, &link).unwrap();
        assert_eq!(run_tmp_in(Some(&os(&link)), "app", &plenty), None);
        // A folder whose name starts with a dot on the way: rea would move Ghidra's session.
        let hidden = private_dir(&base.join(".hidden").join("run"));
        assert_eq!(run_tmp_in(Some(&os(&hidden)), "app", &plenty), None);
        // So long that the bridge socket would not fit.
        let mut long = base.clone();
        while long.as_os_str().len() + "/app-reverse".len() <= RUN_TMP_MAX_BYTES {
            long.push("long-folder");
        }
        let long = private_dir(&long);
        assert_eq!(run_tmp_in(Some(&os(&long)), "app", &plenty), None);
        // Too little free space, or not known.
        let little = |_: &Path| Some(RUN_TMP_MIN_FREE - 1);
        assert_eq!(run_tmp_in(Some(&os(&run)), "app", &little), None);
        assert_eq!(run_tmp_in(Some(&os(&run)), "app", &|_| None), None);
        assert!(private_folder(&run) && !private_folder(&open) && !private_folder(&link));
    }

    #[cfg(unix)]
    #[test]
    fn the_runtime_folder_of_a_computer_with_4_gb_of_memory_or_more_is_used() {
        let base = tempfile::Builder::new().prefix("rt-").tempdir().unwrap();
        let base = std::fs::canonicalize(base.path()).unwrap();
        if dotted(&base) {
            return;
        }
        let run = private_dir(&base.join("run"));
        let run = Some(run.as_os_str());
        // systemd gives the runtime folder a tenth of the memory by default
        // (`RuntimeDirectorySize=10%`); the session's own files already use a little of it.
        let gib = 1u64 << 30;
        let free_with = |memory: u64| move |_: &Path| Some(memory / 10 - (16 << 20));
        for memory in [4 * gib, 8 * gib, 16 * gib] {
            let found = run_tmp_in(run, "app", &free_with(memory));
            assert!(found.is_some(), "{} GiB of memory", memory / gib);
        }
        // A tiny one, as with 2 GB of memory, is left alone.
        assert_eq!(run_tmp_in(run, "app", &free_with(2 * gib)), None);
        assert_eq!(RUN_TMP_MIN_FREE, 256 << 20);
    }

    #[test]
    fn the_bridge_socket_fits_in_the_longest_runtime_folder() {
        // `<folder>/rea-ghidra-XXXXXX/bridge.sock` at most 106 bytes.
        assert_eq!(RUN_TMP_MAX_BYTES, 76);
        assert_eq!(
            RUN_TMP_MAX_BYTES + "/rea-ghidra-AbC123".len() + "/bridge.sock".len(),
            106
        );
    }
}
