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

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Debug builds only: replaces the reverse root.
pub const ROOT_ENV: &str = "HARNESS_REVERSE_ROOT";

/// The marker written last into a part's folder.
pub const COMPLETE_MARKER: &str = ".complete";

/// The reverse-engineering folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseLayout {
    root: PathBuf,
}

impl ReverseLayout {
    /// A layout rooted at `root` exactly (tests, and the debug override).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The app's layout: `<local_data>/reverse`, or `HARNESS_REVERSE_ROOT` in a debug build.
    pub fn for_app(local_data: &Path) -> Self {
        match root_override(std::env::var_os(ROOT_ENV), cfg!(debug_assertions)) {
            Some(root) => Self::new(root),
            None => Self::new(local_data.join("reverse")),
        }
    }

    /// The root folder.
    pub fn root(&self) -> &Path {
        &self.root
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
}
