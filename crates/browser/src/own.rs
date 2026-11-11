//! The agent's own browser (ARCHITECTURE.md 9.3): a pinned Chrome for Testing build the
//! user can download when no supported browser is installed (offered on macOS, where a
//! Mac with only Safari has nothing the tool can drive).
//!
//! The pin lives in `browser.lock.json`, compiled into the app. The archive is downloaded
//! from `storage.googleapis.com` only (resumable), checked against its pinned size and
//! SHA-256, unpacked into `<root>/.tmp-<version>/` and renamed to `<root>/<version>/`
//! once a `.complete` marker is written. Nothing is run before all of that has passed.
//!
//! The archive is a macOS app bundle: it contains relative symbolic links inside the
//! framework bundle and executable bits, and both must survive unpacking.
//! [`unpack_tree`] keeps them, and refuses every entry that could leave the unpack folder.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use runtime::PackFile;
use runtime::packs::PackError;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::discovery::Os;

/// The lock file as compiled into this build.
const EMBEDDED: &str = include_str!("../../../browser.lock.json");

/// The only host the browser archive may be downloaded from, redirects included.
pub const DOWNLOAD_HOSTS: &[&str] = &["storage.googleapis.com"];

/// Written last into an unpacked build; a folder without it is never used.
const COMPLETE_MARKER: &str = ".complete";

/// The most an archive may unpack to (the pinned builds are about 390 MB).
pub const MAX_UNPACKED_BYTES: u64 = 1_500_000_000;

/// The most entries an archive may hold (the pinned builds have 673).
pub const MAX_ENTRIES: usize = 20_000;

/// `browser.lock.json`. Unknown fields are ignored, so the file may grow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserLock {
    /// Schema version of the file.
    #[serde(default)]
    pub schema: u32,
    /// The pinned Chrome for Testing release.
    pub chrome_for_testing: BrowserPin,
    /// One archive per platform.
    #[serde(default)]
    pub builds: Vec<BrowserBuild>,
}

/// The pinned release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserPin {
    /// Full version, for example `157.0.8103.50`; also the install folder's name.
    pub version: String,
    /// Where the builds are published.
    #[serde(default)]
    pub source: String,
}

/// The archive for one platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BrowserBuild {
    /// Platform key as in `llama.lock.json` (`macos-aarch64`, `macos-x86_64`).
    pub platform: String,
    /// The browser executable, relative to the unpacked archive, `/`-separated.
    pub exe: String,
    /// The archive, pinned by size and SHA-256.
    pub file: PackFile,
}

impl BrowserLock {
    /// Parses a lock file.
    pub fn parse(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// The build for a platform key, if one is pinned.
    pub fn build_for(&self, platform: &str) -> Option<&BrowserBuild> {
        self.builds.iter().find(|b| b.platform == platform)
    }

    /// The build for the platform this app runs on, if one is pinned.
    pub fn build_for_current_platform(&self) -> Option<&BrowserBuild> {
        self.build_for(runtime::llama::lock::current_platform())
    }
}

/// The lock file compiled into this build, parsed once.
///
/// # Panics
///
/// Panics if the embedded file does not parse. The file is part of the source tree and a
/// unit test parses it, so a broken lock file cannot pass CI.
pub fn embedded() -> &'static BrowserLock {
    static LOCK: OnceLock<BrowserLock> = OnceLock::new();
    LOCK.get_or_init(|| match BrowserLock::parse(EMBEDDED) {
        Ok(lock) => lock,
        // The input is a compile-time constant checked by `embedded_lock_parses`.
        Err(error) => panic!("embedded browser.lock.json is invalid: {error}"),
    })
}

/// Whether the download is offered on `os`. Windows always has Edge and Linux is a
/// development platform, so only a Mac is ever without a browser the tool can drive.
pub fn offered_on(os: Os) -> bool {
    os == Os::Mac
}

/// The build this computer would download: only where the download is offered and a build
/// is pinned for this processor.
pub fn offered_build() -> Option<&'static BrowserBuild> {
    offered_on(Os::current())
        .then(|| embedded().build_for_current_platform())
        .flatten()
}

/// An unpacked, complete build on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledBrowser {
    /// Its version (the folder name).
    pub version: String,
    /// The browser executable.
    pub exe: PathBuf,
    /// The versioned folder.
    pub dir: PathBuf,
}

/// Where the download stands, from what is on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnState {
    /// Nothing on disk.
    NotInstalled,
    /// Part of the pinned archive is downloaded.
    Partial {
        /// Bytes on disk.
        bytes: u64,
    },
    /// Unpacked and complete.
    Installed(InstalledBrowser),
}

/// Progress of an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnProgress {
    /// Bytes downloaded.
    pub done: u64,
    /// Bytes in the archive.
    pub total: u64,
    /// True once the archive is verified and is being unpacked.
    pub unpacking: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct Marker {
    version: String,
    exe: String,
    #[serde(default)]
    sha256: String,
}

fn downloads_dir(root: &Path) -> PathBuf {
    root.join(".downloads")
}

/// Partial files are named after their hash so a changed pin never resumes stale bytes.
fn part_path(root: &Path, f: &PackFile) -> PathBuf {
    downloads_dir(root).join(format!(
        "{}.{}.part",
        f.name,
        &f.sha256[..16.min(f.sha256.len())]
    ))
}

/// A version such as `157.0.8103.50` as numbers, for ordering; `None` when it is not
/// dotted decimal (such a folder is not one of ours).
fn version_key(version: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = version.split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty() && p.len() <= 6)
}

/// The components of a `/`-separated relative path that stays inside its base: no empty,
/// `.` or `..` component, no backslash, NUL or (on Windows) colon, not absolute.
fn safe_relative(name: &str) -> Option<Vec<&str>> {
    if name.is_empty() || name.contains(['\\', '\0']) {
        return None;
    }
    if cfg!(windows) && name.contains(':') {
        return None;
    }
    let parts: Vec<&str> = name.split('/').collect();
    parts
        .iter()
        .all(|p| !p.is_empty() && *p != "." && *p != "..")
        .then_some(parts)
}

fn read_marker(dir: &Path) -> Option<InstalledBrowser> {
    let text = std::fs::read_to_string(dir.join(COMPLETE_MARKER)).ok()?;
    let marker: Marker = serde_json::from_str(&text).ok()?;
    version_key(&marker.version)?;
    if dir.file_name().and_then(|n| n.to_str()) != Some(marker.version.as_str()) {
        return None;
    }
    let mut exe = dir.to_path_buf();
    for part in safe_relative(&marker.exe)? {
        exe.push(part);
    }
    exe.is_file().then(|| InstalledBrowser {
        version: marker.version,
        exe,
        dir: dir.to_path_buf(),
    })
}

/// The complete build under `root`, the newest when an app update left two.
pub fn installed(root: &Path) -> Option<InstalledBrowser> {
    let mut found: Vec<(Vec<u64>, InstalledBrowser)> = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let key = version_key(&name)?;
            Some((key, read_marker(&e.path())?))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().next().map(|(_, b)| b)
}

/// The state under `root` for `build` (the pinned archive for this computer, when there
/// is one; without it only an installed build can be reported).
pub fn state(root: &Path, build: Option<&BrowserBuild>) -> OwnState {
    if let Some(found) = installed(root) {
        return OwnState::Installed(found);
    }
    let bytes = build
        .and_then(|b| std::fs::metadata(part_path(root, &b.file)).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    if bytes > 0 {
        OwnState::Partial { bytes }
    } else {
        OwnState::NotInstalled
    }
}

fn archive_error(file: &str, message: impl Into<String>) -> PackError {
    PackError::Archive {
        file: file.to_string(),
        message: message.into(),
    }
}

/// Where a link at `link` (components relative to the unpack root) lands when it points
/// at `target`, worked out on the names alone: `None` when the target is absolute, empty,
/// malformed, or climbs out of the root (or is the root itself).
fn resolve_link(link: &[&str], target: &str) -> Option<Vec<String>> {
    if target.is_empty() || target.starts_with('/') || target.contains(['\\', '\0']) {
        return None;
    }
    if cfg!(windows) && target.contains(':') {
        return None;
    }
    let mut at: Vec<String> = link[..link.len().saturating_sub(1)]
        .iter()
        .map(|s| s.to_string())
        .collect();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                at.pop()?;
            }
            name => at.push(name.to_string()),
        }
    }
    (!at.is_empty()).then_some(at)
}

const S_IFMT: u32 = 0o170_000;
const S_IFREG: u32 = 0o100_000;
const S_IFDIR: u32 = 0o040_000;
const S_IFLNK: u32 = 0o120_000;

#[cfg(unix)]
fn make_link(target: &str, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(not(unix))]
fn make_link(_target: &str, _at: &Path) -> std::io::Result<()> {
    // The download is never offered here (see `offered_on`); creating links on Windows
    // needs a privilege most accounts lack.
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "links in archives are not supported on this system",
    ))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    // Permission bits only (no set-user-id, set-group-id or sticky), and the owner can
    // always read and replace the file, so the build can be removed again.
    std::fs::set_permissions(
        path,
        std::fs::Permissions::from_mode((mode & 0o777) | 0o600),
    )
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Unpacks `archive` into the new, empty folder `out`, keeping the folder structure, Unix
/// permission bits and relative symbolic links. Returns the number of files and links
/// written.
///
/// Refused, with nothing of the entry written: absolute paths, `..`, `.` and empty path
/// components, backslashes, entries that are not files, folders or links, a second entry
/// for the same path, links whose target is absolute or resolves outside `out`, links
/// that point at nothing, more than `max_entries` entries, and more than `max_bytes`
/// unpacked bytes (counted while writing, whatever the archive claims).
///
/// Files and folders are written first and links last, so nothing is ever written
/// through a link. Each link is checked twice: on its text before it is created, and on
/// the real file system afterwards (which also catches a link that reaches outside
/// through another link).
///
/// When this returns an error, `out` holds a partial tree and must be deleted.
pub fn unpack_tree(
    archive: &Path,
    out: &Path,
    max_bytes: u64,
    max_entries: usize,
    cancel: &CancellationToken,
) -> Result<usize, PackError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let bad = |m: String| archive_error(&name, m);
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(e.to_string()))?;
    if zip.len() > max_entries {
        return Err(bad(format!(
            "the archive has {} entries; at most {max_entries} are allowed",
            zip.len()
        )));
    }
    std::fs::create_dir_all(out)?;
    let mut left = max_bytes;
    let mut written = 0usize;
    // (path components, target text)
    let mut links: Vec<(Vec<String>, String)> = Vec::new();
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(PackError::Cancelled);
        }
        let mut entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
        let raw = entry.name().map_err(|e| bad(e.to_string()))?.into_owned();
        let is_dir = entry.is_dir();
        let Some(parts) = safe_relative(raw.strip_suffix('/').unwrap_or(&raw)) else {
            return Err(bad(format!("unsafe path in archive: {raw:?}")));
        };
        let mode = entry.unix_mode();
        if let Some(kind) = mode.map(|m| m & S_IFMT)
            && ![0, S_IFREG, S_IFDIR, S_IFLNK].contains(&kind)
        {
            return Err(bad(format!("{raw:?} is not a file, folder or link")));
        }
        let mut target = out.to_path_buf();
        for p in &parts {
            target.push(p);
        }
        if is_dir {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if mode.is_some_and(|m| m & S_IFMT == S_IFLNK) {
            let mut text = String::new();
            (&mut entry)
                .take(4097)
                .read_to_string(&mut text)
                .map_err(|e| bad(format!("the link {raw:?} could not be read: {e}")))?;
            if text.len() > 4096 || resolve_link(&parts, &text).is_none() {
                return Err(bad(format!(
                    "the link {raw:?} points outside the archive ({:?})",
                    text.chars().take(200).collect::<String>()
                )));
            }
            links.push((parts.iter().map(|s| s.to_string()).collect(), text));
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // `create_new`: a second entry for the same path is refused, not merged.
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::AlreadyExists {
                    bad(format!("{raw:?} is in the archive twice"))
                } else {
                    PackError::from(e)
                }
            })?;
        let copied = std::io::copy(&mut (&mut entry).take(left.saturating_add(1)), &mut f)?;
        if copied > left {
            return Err(bad(format!(
                "the archive unpacks to more than {max_bytes} bytes"
            )));
        }
        left -= copied;
        drop(f);
        if let Some(mode) = mode {
            set_mode(&target, mode)?;
        }
        written += 1;
    }
    // Links last: until here the tree holds only real files and folders.
    for (parts, text) in &links {
        let mut at = out.to_path_buf();
        for p in parts {
            at.push(p);
        }
        if let Some(parent) = at.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if at.symlink_metadata().is_ok() {
            return Err(bad(format!(
                "{:?} is in the archive twice",
                parts.join("/")
            )));
        }
        make_link(text, &at).map_err(|e| {
            bad(format!(
                "the link {:?} could not be created: {e}",
                parts.join("/")
            ))
        })?;
        written += 1;
    }
    // The second check, on the real file system: every link must lead to something
    // inside `out`, however many links it passes through.
    let root = std::fs::canonicalize(out)?;
    for (parts, text) in &links {
        let mut at = out.to_path_buf();
        for p in parts {
            at.push(p);
        }
        let inside = std::fs::canonicalize(&at).is_ok_and(|real| real.starts_with(&root));
        if !inside {
            return Err(bad(format!(
                "the link {:?} points outside the archive or at nothing ({text:?})",
                parts.join("/")
            )));
        }
    }
    Ok(written)
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o100 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Unpacks the verified `archive` for `build` under `root` and returns the installed
/// build. Blocking; run it off the async runtime.
fn unpack_and_finish(
    version: &str,
    build: &BrowserBuild,
    archive: &Path,
    root: &Path,
    cancel: &CancellationToken,
) -> Result<InstalledBrowser, PackError> {
    let bad = |m: &str| archive_error(&build.file.name, m);
    if version_key(version).is_none() {
        return Err(bad("the pinned version is not a version number"));
    }
    let Some(exe_parts) = safe_relative(&build.exe) else {
        return Err(bad("the pinned executable path is unsafe"));
    };
    let tmp = root.join(format!(".tmp-{version}"));
    let _ = std::fs::remove_dir_all(&tmp);
    let done = (|| -> Result<(), PackError> {
        unpack_tree(archive, &tmp, MAX_UNPACKED_BYTES, MAX_ENTRIES, cancel)?;
        let mut exe = tmp.clone();
        for p in &exe_parts {
            exe.push(p);
        }
        if !is_executable(&exe) {
            return Err(bad(
                "the browser's program file is missing from the archive",
            ));
        }
        let marker = Marker {
            version: version.to_string(),
            exe: build.exe.clone(),
            sha256: build.file.sha256.clone(),
        };
        let text = serde_json::to_string(&marker).map_err(|e| PackError::Io(e.to_string()))?;
        std::fs::write(tmp.join(COMPLETE_MARKER), text)?;
        Ok(())
    })();
    if let Err(e) = done {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let final_dir = root.join(version);
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&tmp, &final_dir)?;
    let _ = std::fs::remove_file(archive);
    // Builds an earlier app version installed are not used any more.
    if let Ok(entries) = std::fs::read_dir(root) {
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            if name != version && version_key(&name).is_some() {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    read_marker(&final_dir).ok_or_else(|| bad("the unpacked browser could not be read back"))
}

/// Downloads, verifies and unpacks `build` under `root`. A partial download is resumed;
/// cancelling keeps it for the next attempt. Returns the installed build, at once when
/// this version is already there.
pub async fn install(
    lock: &BrowserLock,
    build: &BrowserBuild,
    root: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(OwnProgress) + Send + Sync),
) -> Result<InstalledBrowser, PackError> {
    let version = lock.chrome_for_testing.version.clone();
    if let Some(found) = read_marker(&root.join(&version)) {
        return Ok(found);
    }
    tokio::fs::create_dir_all(downloads_dir(root)).await?;
    let part = part_path(root, &build.file);
    let total = build.file.size;
    runtime::packs::download_verified(&build.file, &part, DOWNLOAD_HOSTS, cancel, &|done| {
        progress(OwnProgress {
            done,
            total,
            unpacking: false,
        })
    })
    .await?;
    progress(OwnProgress {
        done: total,
        total,
        unpacking: true,
    });
    let (build, root, cancel) = (build.clone(), root.to_path_buf(), cancel.clone());
    let found = tokio::task::spawn_blocking(move || {
        unpack_and_finish(&version, &build, &part, &root, &cancel)
    })
    .await
    .map_err(|e| PackError::Io(e.to_string()))??;
    tracing::info!(version = %found.version, "the agent's own browser is installed");
    Ok(found)
}

/// Deletes every build, partial download and temporary folder under `root`. The caller
/// closes a running browser first.
pub async fn remove(root: &Path) -> Result<(), PackError> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || -> Result<(), PackError> {
        let Ok(entries) = std::fs::read_dir(&root) else {
            return Ok(());
        };
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let ours =
                version_key(&name).is_some() || name.starts_with(".tmp-") || name == ".downloads";
            if ours && e.file_type().is_ok_and(|t| t.is_dir()) {
                std::fs::remove_dir_all(e.path())?;
            }
        }
        Ok(())
    })
    .await
    .map_err(|e| PackError::Io(e.to_string()))?
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn is_hex64(s: &str) -> bool {
        s.len() == 64
            && s.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }

    #[test]
    fn embedded_lock_parses() {
        let lock = BrowserLock::parse(EMBEDDED).expect("embedded lock parses");
        assert_eq!(&lock, embedded());
        assert!(version_key(&lock.chrome_for_testing.version).is_some());
        assert!(lock.build_for("macos-aarch64").is_some());
        assert!(lock.build_for("macos-x86_64").is_some());
        assert!(lock.build_for("windows-x86_64").is_none());
    }

    #[test]
    fn every_build_is_pinned_https_on_the_one_allowed_host() {
        let lock = embedded();
        assert!(!lock.builds.is_empty());
        assert_eq!(DOWNLOAD_HOSTS, &["storage.googleapis.com"]);
        for b in &lock.builds {
            let f = &b.file;
            let url = url::Url::parse(&f.url).expect("url");
            assert_eq!(url.scheme(), "https", "{}", f.url);
            assert_eq!(url.host_str(), Some("storage.googleapis.com"), "{}", f.url);
            assert!(url.username().is_empty() && url.password().is_none());
            assert!(url.port().is_none() && url.query().is_none());
            assert!(
                url.path()
                    .contains(&format!("/{}/", lock.chrome_for_testing.version)),
                "{}: the URL is not for the pinned version",
                f.url
            );
            assert!(f.url.ends_with(&f.name), "{}", f.url);
            assert!(is_hex64(&f.sha256), "{}: sha256 {:?}", f.name, f.sha256);
            assert!(f.size > 50_000_000, "{}: size {}", f.name, f.size);
            assert!(f.size < MAX_UNPACKED_BYTES);
            assert!(b.platform.starts_with("macos-"), "{}", b.platform);
            assert!(safe_relative(&b.exe).is_some(), "{}", b.exe);
            assert!(b.exe.contains(".app/Contents/MacOS/"), "{}", b.exe);
        }
        let mut platforms: Vec<&str> = lock.builds.iter().map(|b| b.platform.as_str()).collect();
        platforms.sort_unstable();
        platforms.dedup();
        assert_eq!(platforms.len(), lock.builds.len());
    }

    #[test]
    fn the_download_is_offered_on_macos_only() {
        assert!(offered_on(Os::Mac));
        assert!(!offered_on(Os::Windows));
        assert!(!offered_on(Os::Linux));
        assert_eq!(offered_build().is_some(), cfg!(target_os = "macos"));
    }

    #[test]
    fn relative_paths() {
        assert_eq!(safe_relative("a/b c/d"), Some(vec!["a", "b c", "d"]));
        for bad in [
            "", "/a", "a//b", "a/../b", "../a", "a/..", "./a", "a/.", "a\\b", "a/\0b", "a/",
        ] {
            assert_eq!(safe_relative(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn link_targets_are_resolved_on_their_names() {
        let at = |link: &str, target: &str| {
            resolve_link(&link.split('/').collect::<Vec<_>>(), target).map(|p| p.join("/"))
        };
        assert_eq!(
            at("F/Versions/Current", "157.0"),
            Some("F/Versions/157.0".into())
        );
        assert_eq!(
            at("F/Resources", "Versions/Current/Resources"),
            Some("F/Versions/Current/Resources".into())
        );
        assert_eq!(at("a/b/link", "../c"), Some("a/c".into()));
        assert_eq!(at("a/link", "./x//y"), Some("a/x/y".into()));
        assert_eq!(at("a/link", "../../etc"), None, "climbs out");
        assert_eq!(at("link", ".."), None);
        assert_eq!(at("a/link", ".."), None, "the root itself");
        assert_eq!(at("a/link", "/etc/passwd"), None, "absolute");
        assert_eq!(at("a/link", ""), None);
        assert_eq!(at("a/link", "b\\..\\..\\c"), None);
    }

    #[test]
    fn versions_order_by_number() {
        assert!(version_key("157.0.8103.50") > version_key("99.0.1.2"));
        assert!(version_key("157.0.8103.50") < version_key("157.0.8103.100"));
        assert_eq!(version_key(".tmp-157.0"), None);
        assert_eq!(version_key("chrome"), None);
        assert_eq!(version_key(""), None);
    }

    enum E<'a> {
        File(&'a str, &'a [u8], u32),
        Dir(&'a str),
        Link(&'a str, &'a str),
    }

    fn make_zip(path: &Path, entries: &[E]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = || {
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated)
        };
        for e in entries {
            match e {
                E::File(name, data, mode) => {
                    w.start_file(*name, opts().unix_permissions(*mode)).unwrap();
                    w.write_all(data).unwrap();
                }
                E::Dir(name) => w.add_directory(*name, opts()).unwrap(),
                E::Link(name, target) => w.add_symlink(*name, *target, opts()).unwrap(),
            }
        }
        w.finish().unwrap();
    }

    fn unpack(entries: &[E]) -> (tempfile::TempDir, Result<usize, PackError>) {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("a.zip");
        make_zip(&z, entries);
        let out = dir.path().join("out");
        let r = unpack_tree(&z, &out, 1 << 20, 100, &CancellationToken::new());
        (dir, r)
    }

    fn refused(entries: &[E], why: &str) -> tempfile::TempDir {
        let (dir, r) = unpack(entries);
        match r {
            Err(PackError::Archive { message, .. }) => {
                assert!(
                    message.contains(why),
                    "{message:?} does not mention {why:?}"
                )
            }
            other => panic!("expected a refusal mentioning {why:?}, got {other:?}"),
        }
        dir
    }

    /// The shape of the real archive: a framework with `Versions/Current` and links into
    /// it, and a program file that must stay executable.
    #[cfg(unix)]
    #[test]
    fn a_bundle_with_relative_links_and_modes_unpacks_intact() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, r) = unpack(&[
            E::Dir("top/App.app/Contents/MacOS/"),
            E::File("top/App.app/Contents/MacOS/App", b"#!bin", 0o755),
            E::File("top/App.app/Contents/Info.plist", b"<plist/>", 0o644),
            E::File(
                "top/App.app/Contents/F.framework/Versions/1.2/F",
                b"lib",
                0o755,
            ),
            E::File(
                "top/App.app/Contents/F.framework/Versions/1.2/Resources/r.pak",
                b"r",
                0o444,
            ),
            E::Link("top/App.app/Contents/F.framework/Versions/Current", "1.2"),
            E::Link("top/App.app/Contents/F.framework/F", "Versions/Current/F"),
            E::Link(
                "top/App.app/Contents/F.framework/Resources",
                "Versions/Current/Resources",
            ),
            E::Link("top/App.app/Contents/up", "../Contents/Info.plist"),
        ]);
        assert_eq!(r.unwrap(), 8, "four files and four links");
        let out = dir.path().join("out");
        let c = out.join("top/App.app/Contents");
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&c.join("MacOS/App")), 0o755);
        assert_eq!(mode(&c.join("Info.plist")), 0o644);
        assert_eq!(
            mode(&c.join("F.framework/Versions/1.2/Resources/r.pak")),
            0o644,
            "the owner can always replace a file"
        );
        let current = c.join("F.framework/Versions/Current");
        assert!(current.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&current).unwrap(), Path::new("1.2"));
        assert_eq!(
            std::fs::read_link(c.join("F.framework/F")).unwrap(),
            Path::new("Versions/Current/F"),
            "the link text is kept as it is"
        );
        // Through two links.
        assert_eq!(std::fs::read(c.join("F.framework/F")).unwrap(), b"lib");
        assert_eq!(
            std::fs::read(c.join("F.framework/Resources/r.pak")).unwrap(),
            b"r"
        );
        assert_eq!(std::fs::read(c.join("up")).unwrap(), b"<plist/>");
    }

    #[cfg(unix)]
    #[test]
    fn special_permission_bits_are_dropped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("a.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
        // Set-user-id, as a Unix zip tool records it.
        let opts = zip::write::SimpleFileOptions::default().external_attributes(0o104_755 << 16);
        w.start_file("a/tool", opts).unwrap();
        w.write_all(b"x").unwrap();
        w.finish().unwrap();
        let out = dir.path().join("out");
        unpack_tree(&z, &out, 1 << 20, 100, &CancellationToken::new()).unwrap();
        let mode = std::fs::metadata(dir.path().join("out/a/tool"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o7777, 0o755);
    }

    #[test]
    fn a_link_that_climbs_out_is_refused() {
        let dir = refused(
            &[
                E::Dir("a/b/"),
                E::File("a/ok", b"x", 0o644),
                E::Link("a/b/escape", "../../../outside"),
            ],
            "points outside",
        );
        assert!(
            dir.path()
                .join("out/a/b/escape")
                .symlink_metadata()
                .is_err(),
            "the link was never created"
        );
    }

    #[test]
    fn an_absolute_link_is_refused() {
        let dir = refused(&[E::Link("a/passwd", "/etc/passwd")], "points outside");
        assert!(dir.path().join("out/a/passwd").symlink_metadata().is_err());
    }

    /// Each link looks harmless on its own text; together they leave the folder.
    #[cfg(unix)]
    #[test]
    fn a_link_that_leaves_through_another_link_is_refused() {
        refused(
            &[
                E::File("a/b/c/file", b"x", 0o644),
                // `d` is at depth 3 by name and resolves to the root's first level.
                E::Link("a/b/c/d", "../.."),
                // By name: a/b/c/d/../../.. = a. Really: (a)/../.. = above the root.
                E::Link("leak", "a/b/c/d/../../.."),
            ],
            "points outside",
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_link_to_nothing_is_refused() {
        refused(&[E::Link("a/dangling", "missing")], "at nothing");
    }

    #[test]
    fn parent_and_absolute_entry_paths_are_refused() {
        for name in ["../escape", "a/../../escape", "/abs/file", "a/./b", "a\\b"] {
            let dir = refused(&[E::File(name, b"x", 0o644)], "unsafe path");
            assert!(!dir.path().join("escape").exists(), "{name}");
            assert!(!Path::new("/abs/file").exists(), "{name}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn nothing_is_written_through_a_link() {
        // The link comes first in the archive and the file names a path under it.
        let dir = refused(
            &[
                E::Dir("real/"),
                E::Link("via", "real"),
                E::File("via/planted", b"x", 0o644),
            ],
            "twice",
        );
        assert!(!dir.path().join("out/real/planted").exists());
    }

    #[test]
    fn the_size_and_entry_caps_hold() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("big.zip");
        let big = vec![0u8; 4096];
        make_zip(&z, &[E::File("a", &big, 0o644), E::File("b", &big, 0o644)]);
        let cancel = CancellationToken::new();
        let err = unpack_tree(&z, &dir.path().join("o1"), 5000, 100, &cancel).unwrap_err();
        assert!(err.to_string().contains("more than 5000 bytes"), "{err}");
        let err = unpack_tree(&z, &dir.path().join("o2"), 1 << 20, 1, &cancel).unwrap_err();
        assert!(err.to_string().contains("at most 1"), "{err}");
        assert_eq!(
            unpack_tree(&z, &dir.path().join("o3"), 8192, 2, &cancel).unwrap(),
            2,
            "exactly at both caps"
        );
        cancel.cancel();
        assert!(matches!(
            unpack_tree(&z, &dir.path().join("o4"), 1 << 20, 100, &cancel),
            Err(PackError::Cancelled)
        ));
    }

    fn test_build(sha: &str, size: u64) -> (BrowserLock, BrowserBuild) {
        let build = BrowserBuild {
            platform: "test".into(),
            exe: "pkg/App.app/Contents/MacOS/App".into(),
            file: PackFile {
                name: "pkg.zip".into(),
                url: "https://storage.googleapis.com/x/1.2.3.4/pkg.zip".into(),
                size,
                sha256: sha.into(),
            },
        };
        let lock = BrowserLock {
            schema: 1,
            chrome_for_testing: BrowserPin {
                version: "1.2.3.4".into(),
                source: String::new(),
            },
            builds: vec![build.clone()],
        };
        (lock, build)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_verified_archive_becomes_a_versioned_folder_and_can_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent-browser");
        let (lock, build) = test_build(&"a".repeat(64), 10);
        assert_eq!(state(&root, Some(&build)), OwnState::NotInstalled);
        assert_eq!(installed(&root), None);

        std::fs::create_dir_all(downloads_dir(&root)).unwrap();
        let part = part_path(&root, &build.file);
        make_zip(
            &part,
            &[
                E::File("pkg/App.app/Contents/MacOS/App", b"bin", 0o755),
                E::File("pkg/App.app/Contents/F/Versions/9/F", b"lib", 0o755),
                E::Link("pkg/App.app/Contents/F/Versions/Current", "9"),
            ],
        );
        assert!(matches!(
            state(&root, Some(&build)),
            OwnState::Partial { bytes } if bytes > 0
        ));
        // An older build from an earlier app version, and a stale temporary folder.
        std::fs::create_dir_all(root.join("1.0.0.0")).unwrap();
        std::fs::create_dir_all(root.join(".tmp-1.2.3.4/junk")).unwrap();

        let found = unpack_and_finish(
            &lock.chrome_for_testing.version,
            &build,
            &part,
            &root,
            &CancellationToken::new(),
        )
        .unwrap();
        assert_eq!(found.version, "1.2.3.4");
        assert_eq!(found.dir, root.join("1.2.3.4"));
        assert_eq!(
            found.exe,
            root.join("1.2.3.4/pkg/App.app/Contents/MacOS/App")
        );
        assert!(is_executable(&found.exe));
        assert_eq!(installed(&root), Some(found.clone()));
        assert_eq!(state(&root, Some(&build)), OwnState::Installed(found));
        assert!(!part.exists(), "the archive is deleted after unpacking");
        assert!(!root.join(".tmp-1.2.3.4").exists());
        assert!(!root.join("1.0.0.0").exists(), "older builds are removed");

        // A folder without the marker, or whose marker names another version, is not used.
        std::fs::create_dir_all(root.join("9.9.9.9")).unwrap();
        std::fs::write(
            root.join("9.9.9.9").join(COMPLETE_MARKER),
            r#"{"version":"1.2.3.4","exe":"../1.2.3.4/pkg/App.app/Contents/MacOS/App"}"#,
        )
        .unwrap();
        assert_eq!(installed(&root).unwrap().version, "1.2.3.4");

        remove(&root).await.unwrap();
        assert_eq!(installed(&root), None);
        assert_eq!(state(&root, Some(&build)), OwnState::NotInstalled);
        assert!(!root.join("1.2.3.4").exists());
        assert!(!downloads_dir(&root).exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_bad_archive_leaves_nothing_installed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent-browser");
        let (lock, build) = test_build(&"a".repeat(64), 10);
        std::fs::create_dir_all(downloads_dir(&root)).unwrap();
        let part = part_path(&root, &build.file);
        let version = lock.chrome_for_testing.version.as_str();
        let cancel = CancellationToken::new();

        // The program file is there but not executable.
        make_zip(
            &part,
            &[E::File("pkg/App.app/Contents/MacOS/App", b"bin", 0o644)],
        );
        let err = unpack_and_finish(version, &build, &part, &root, &cancel).unwrap_err();
        assert!(err.to_string().contains("program file is missing"), "{err}");
        assert_eq!(installed(&root), None);
        assert!(!root.join(".tmp-1.2.3.4").exists());
        assert!(!root.join("1.2.3.4").exists());

        // An escaping link: the whole build is refused.
        make_zip(
            &part,
            &[
                E::File("pkg/App.app/Contents/MacOS/App", b"bin", 0o755),
                E::Link("pkg/out", "../../../.."),
            ],
        );
        let err = unpack_and_finish(version, &build, &part, &root, &cancel).unwrap_err();
        assert!(err.to_string().contains("points outside"), "{err}");
        assert_eq!(installed(&root), None);
        assert!(!root.join(".tmp-1.2.3.4").exists());
    }

    /// The real thing, on request only: downloads the pinned build for Apple-silicon Macs
    /// (about 200 MB) from the pinned URL, checks its size and SHA-256, unpacks it (about
    /// 400 MB) and looks at the result. It runs on Linux and macOS; nothing is started.
    /// Needs the network and about 600 MB of free disk in the temporary folder; everything
    /// is deleted afterwards. Run it with
    /// `cargo test -p browser --lib -- --ignored real_download --nocapture`.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "downloads about 200 MB from storage.googleapis.com"]
    async fn real_download_of_the_pinned_mac_build_verifies_and_unpacks() {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("agent-browser");
        let lock = embedded();
        let build = lock.build_for("macos-aarch64").expect("pinned build");
        let most = AtomicU64::new(0);
        let unpacking = AtomicBool::new(false);
        let found = install(lock, build, &root, &CancellationToken::new(), &|p| {
            most.fetch_max(p.done, Ordering::Relaxed);
            if p.unpacking {
                unpacking.store(true, Ordering::Relaxed);
            }
            assert_eq!(p.total, build.file.size);
        })
        .await
        .expect("download, verify and unpack");
        assert_eq!(most.load(Ordering::Relaxed), build.file.size);
        assert!(unpacking.load(Ordering::Relaxed));
        assert_eq!(found.version, lock.chrome_for_testing.version);
        assert_eq!(found.dir, root.join(&lock.chrome_for_testing.version));
        assert!(
            found
                .exe
                .ends_with("Contents/MacOS/Google Chrome for Testing")
        );
        assert!(is_executable(&found.exe), "the program file is executable");
        assert_eq!(installed(&root), Some(found.clone()));
        assert!(!downloads_dir(&root).read_dir().unwrap().any(|_| true));

        // Walk the tree without following links.
        let real_root = std::fs::canonicalize(&found.dir).unwrap();
        let (mut files, mut links, mut folders, mut executables, mut bytes) = (0, 0, 0, 0, 0u64);
        let mut stack = vec![found.dir.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let e = e.unwrap();
                let meta = e.path().symlink_metadata().unwrap();
                if meta.file_type().is_symlink() {
                    links += 1;
                    let target = std::fs::read_link(e.path()).unwrap();
                    assert!(target.is_relative(), "{target:?}");
                    let real = std::fs::canonicalize(e.path()).expect("the link leads somewhere");
                    assert!(real.starts_with(&real_root), "{real:?}");
                } else if meta.is_dir() {
                    folders += 1;
                    stack.push(e.path());
                } else {
                    files += 1;
                    bytes += meta.len();
                    let mode = meta.permissions().mode();
                    assert_eq!(mode & 0o7000, 0, "{:?}", e.path());
                    assert_eq!(mode & 0o600, 0o600, "{:?}", e.path());
                    if mode & 0o111 != 0 {
                        executables += 1;
                    }
                }
            }
        }
        eprintln!(
            "unpacked {files} files ({bytes} bytes), {links} links, {folders} folders, {executables} executable files"
        );
        assert_eq!(links, 5, "the framework bundle's links");
        let framework = found
            .exe
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("Frameworks/Google Chrome for Testing Framework.framework");
        let current = framework.join("Versions/Current");
        assert!(current.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(
            std::fs::read_link(&current).unwrap(),
            Path::new(&lock.chrome_for_testing.version)
        );
        // The framework's library is reached through two links.
        assert!(
            framework
                .join("Google Chrome for Testing Framework")
                .metadata()
                .unwrap()
                .len()
                > 100_000_000
        );
        assert!(executables > 5, "helpers and libraries keep their bits");
        assert!(bytes > 300_000_000 && bytes < MAX_UNPACKED_BYTES);
        // The archive has 673 entries; some folders have no entry of their own, and the
        // marker is ours.
        assert!(files - 1 + links <= 673 && files > 300, "{files} files");
        assert!(found.dir.join(COMPLETE_MARKER).is_file());

        remove(&root).await.unwrap();
        assert_eq!(installed(&root), None);
        assert!(!found.dir.exists());
    }

    /// `install` refuses a pin on any other host before it asks the network for anything.
    #[tokio::test]
    async fn a_build_on_another_host_is_never_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let (lock, mut build) = test_build(&"a".repeat(64), 10);
        build.file.url = "https://example.com/chrome.zip".into();
        let err = install(
            &lock,
            &build,
            dir.path(),
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, PackError::BlockedHost(h) if h == "example.com"),
            "{err}"
        );
    }
}
