//! Downloading, checking, unpacking, repairing, updating and removing the reverse rows
//! (item5-spec.md R1, R15 and WP9a `install.rs`), following the ROCm pack
//! (`runtime::packs`): pinned files, resumable downloads into `.downloads/`, size and
//! SHA-256 checked **before** unpacking, unpacked into `.tmp-<dir>`, a `.complete` marker
//! written last, then renamed into place.
//!
//! - A decompiler's download fetches rea first when rea is missing, in the same operation.
//! - Free disk space (download plus unpacked size plus 10 %) and, on Windows, the longest
//!   path (`.tmp-<dir>` plus the part's longest file, at most 250 characters) are checked
//!   before anything is fetched.
//! - An update unpacks the new version into its own folder; the old one is deleted only
//!   after the new one is complete. A download over a broken copy repairs it.
//! - A row's downloads are deleted only once all of its parts are in place, so a Cancel
//!   while a later part unpacks leaves the row paused ("Paused at …"), not broken.
//! - A release build offers only parts whose publishers' checks were recorded at pin time
//!   (`lock::release_checks`); a debug build also offers one pinned on first download only.
//! - Names and digests from the lock are used under `.downloads/` only when they are one
//!   plain file name and 64 hex digits.
//! - Fetching goes through [`Fetch`], so tests install from local archives ([`DirFetch`])
//!   and the app from the network ([`NetFetch`]: `runtime::packs::download_verified`, that
//!   part's hosts only).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use runtime::PackFile;
use runtime::packs::PackError;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::archive::{self, ArchiveError, Rules, safe_relative};
use crate::diskspace::{self, FreeSpace};
use crate::layout::{COMPLETE_MARKER, ReverseLayout};
use crate::lock::{self, Acceptance, Build, Part, ReverseLock, safe_pinned_file, version_key};
use crate::platform::{self, Os, Platform};
use crate::{
    BrokenWhy, Component, ComponentState, ComponentStatus, Decompiler, DownloadStage, LicenseInfo,
    PartInfo, ReverseError, UpdateInfo, UpdateWhy,
};

/// Longest path, in characters, allowed under the reverse folder on Windows.
pub const WINDOWS_MAX_PATH: usize = 250;

/// One progress report (the `reverse-progress` event without `finished` and `error`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    /// The row whose Download was clicked.
    pub component: Component,
    /// The part being fetched, checked or unpacked.
    pub part: String,
    /// Bytes so far in this stage.
    pub done: u64,
    /// Bytes in this stage.
    pub total: u64,
    /// The stage.
    pub stage: DownloadStage,
}

/// Brings one pinned file to a local path.
#[async_trait]
pub trait Fetch: Send + Sync {
    /// Brings `file` to `dest`, resuming the bytes already there, from `hosts` only.
    /// `on_bytes` gets the bytes at `dest` so far. The caller checks size and SHA-256
    /// afterwards.
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        hosts: &'static [&'static str],
        cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError>;
}

/// The network, through `runtime::packs::download_verified` (redirects stay on `hosts`).
pub struct NetFetch;

fn url_host(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_string))
        .unwrap_or_else(|| "?".into())
}

/// One quick request before a download, so a computer that is offline hears "Can't reach
/// <host>" in seconds instead of after the download's retries. Any answer at all counts as
/// reachable; nothing is followed.
async fn preflight(url: &reqwest::Url, cancel: &CancellationToken) -> Result<(), ReverseError> {
    let host = url.host_str().unwrap_or("?").to_string();
    let client = reqwest::Client::builder()
        .user_agent(concat!("app-reverse-download/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| ReverseError::Io {
            message: e.to_string(),
        })?;
    let sent = tokio::select! {
        () = cancel.cancelled() => return Err(ReverseError::Cancelled),
        r = client.head(url.clone()).send() => r,
    };
    match sent {
        Err(e) if e.is_connect() || e.is_timeout() => Err(ReverseError::CantReach { host }),
        _ => Ok(()),
    }
}

/// The status of a `runtime::packs` failure that was an HTTP answer ("HTTP 404").
fn http_status(message: &str) -> Option<u16> {
    message.strip_prefix("HTTP ")?.trim().parse().ok()
}

/// Maps the pack downloader's errors. A failure before any byte arrived is "can't reach",
/// unless the host answered with an error status: then it was reached and said no.
fn from_pack_error(e: PackError, part: &str, host: &str, got_bytes: bool) -> ReverseError {
    if let PackError::Network { message, .. } = &e
        && let Some(status) = http_status(message)
    {
        return ReverseError::ServerError {
            host: host.to_string(),
            status,
        };
    }
    match e {
        PackError::Cancelled => ReverseError::Cancelled,
        PackError::BlockedHost(host) => ReverseError::BlockedHost { host },
        PackError::ChecksumMismatch { .. } | PackError::SizeMismatch { .. } => {
            ReverseError::Mismatch {
                part: part.to_string(),
            }
        }
        PackError::Network { .. } if !got_bytes => ReverseError::CantReach {
            host: host.to_string(),
        },
        PackError::Network { .. } => ReverseError::Interrupted {
            part: part.to_string(),
        },
        PackError::DiskFull => ReverseError::DiskFull { need_bytes: 0 },
        PackError::Archive { message, .. } => ReverseError::Archive {
            part: part.to_string(),
            message,
        },
        other => ReverseError::Io {
            message: other.to_string(),
        },
    }
}

#[async_trait]
impl Fetch for NetFetch {
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        hosts: &'static [&'static str],
        cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError> {
        let host = url_host(&file.url);
        let url = reqwest::Url::parse(&file.url)
            .map_err(|_| ReverseError::BlockedHost { host: host.clone() })?;
        if url.scheme() != "https" || !hosts.contains(&host.as_str()) {
            return Err(ReverseError::BlockedHost { host });
        }
        let have = tokio::fs::metadata(dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if have < file.size {
            preflight(&url, cancel).await?;
        }
        let got = AtomicBool::new(have > 0);
        runtime::packs::download_verified(file, dest, hosts, cancel, &|n| {
            if n > 0 {
                got.store(true, Ordering::Relaxed);
            }
            on_bytes(n);
        })
        .await
        .map_err(|e| from_pack_error(e, &file.name, &host, got.load(Ordering::Relaxed)))
    }
}

/// A folder of already downloaded archives, named as the lock names them (tests, and the
/// cache `scripts/pin_reverse.py --download-to` fills for WP9e). Resumes like the network.
pub struct DirFetch {
    dir: PathBuf,
}

impl DirFetch {
    /// Archives are read from `dir/<file name>`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

#[async_trait]
impl Fetch for DirFetch {
    async fn fetch(
        &self,
        file: &PackFile,
        dest: &Path,
        _hosts: &'static [&'static str],
        cancel: &CancellationToken,
        on_bytes: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<(), ReverseError> {
        use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
        let src = self.dir.join(&file.name);
        let mut input = tokio::fs::File::open(&src)
            .await
            .map_err(|_| ReverseError::CantReach {
                host: "the local download cache".into(),
            })?;
        let have = tokio::fs::metadata(dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        let have = if have > file.size { 0 } else { have };
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(have > 0)
            .truncate(have == 0)
            .open(dest)
            .await?;
        input.seek(std::io::SeekFrom::Start(have)).await?;
        let mut done = have;
        on_bytes(done);
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if cancel.is_cancelled() {
                out.flush().await?;
                return Err(ReverseError::Cancelled);
            }
            let n = input.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).await?;
            done += n as u64;
            on_bytes(done);
        }
        out.flush().await?;
        Ok(())
    }
}

/// A file the app or rea runs or reads, with its size on disk.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramFile {
    /// Relative path inside the part's folder.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
}

/// `.complete`: written last into a part's folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Marker {
    component: Component,
    part: String,
    version: String,
    archive_sha256: Vec<String>,
    programs: Vec<ProgramFile>,
    disk_bytes: u64,
}

/// One installed part.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstalledPart {
    /// `rea`, `node`, `kuna`, `ghidra` or `java`.
    pub name: String,
    /// Installed version.
    pub version: String,
    /// Its folder.
    pub dir: PathBuf,
    /// The files checked on every status (the first is the one run, where there is one).
    pub programs: Vec<ProgramFile>,
    /// SHA-256 of the archives it came from.
    pub archive_sha256: Vec<String>,
    /// Bytes on disk.
    pub disk_bytes: u64,
}

impl InstalledPart {
    /// The absolute path of the part's first program (`bin/node`, `kuna`, …).
    pub fn main_program(&self) -> Option<PathBuf> {
        self.programs
            .first()
            .map(|p| join_relative(&self.dir, &p.path))
    }
}

/// An installed, usable row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    /// The row.
    pub component: Component,
    /// Version of its main part.
    pub version: String,
    /// Its parts, in lock order.
    pub parts: Vec<InstalledPart>,
    /// Bytes on disk.
    pub disk_bytes: u64,
}

impl Installed {
    /// A part by name.
    pub fn part(&self, name: &str) -> Option<&InstalledPart> {
        self.parts.iter().find(|p| p.name == name)
    }
}

/// What is installed and usable now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installation {
    /// rea with Node.js.
    pub rea: Option<Installed>,
    /// Kuna.
    pub kuna: Option<Installed>,
    /// Ghidra with Java.
    pub ghidra: Option<Installed>,
    /// rea's entry script (`<rea>/scripts/rea.mjs`), when rea is installed.
    pub rea_entry: Option<PathBuf>,
    /// `JAVA_HOME`, when Ghidra is installed.
    pub java_home: Option<PathBuf>,
}

impl Installation {
    /// A row.
    pub fn get(&self, component: Component) -> Option<&Installed> {
        match component {
            Component::Rea => self.rea.as_ref(),
            Component::Kuna => self.kuna.as_ref(),
            Component::Ghidra => self.ghidra.as_ref(),
        }
    }
    /// The installed decompilers, Kuna first.
    pub fn decompilers(&self) -> Vec<Decompiler> {
        [Decompiler::Kuna, Decompiler::Ghidra]
            .into_iter()
            .filter(|d| self.get(d.component()).is_some())
            .collect()
    }
    /// The `node` program, when rea is installed.
    pub fn node(&self) -> Option<PathBuf> {
        self.rea.as_ref()?.part("node")?.main_program()
    }
    /// The `kuna` program, when Kuna is installed.
    pub fn kuna_program(&self) -> Option<PathBuf> {
        self.kuna.as_ref()?.part("kuna")?.main_program()
    }
    /// `GHIDRA_INSTALL_DIR`, when Ghidra is installed.
    pub fn ghidra_dir(&self) -> Option<PathBuf> {
        Some(self.ghidra.as_ref()?.part("ghidra")?.dir.clone())
    }
}

fn join_relative(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for part in rel.split('/') {
        p.push(part);
    }
    p
}

/// Opens a file for reading, to see whether the system lets it be used.
pub type Opener = Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;
/// The file system name of a folder's volume (Windows); injectable for tests.
pub type FsName = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

fn open_for_read(path: &Path) -> std::io::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.custom_flags(libc::O_NONBLOCK);
    }
    o.open(path).map(|_| ())
}

/// Whether a failure to open a present file means the system blocks it (antivirus, Smart
/// App Control, a policy) or that it is broken. Windows codes: access denied (5), virus
/// infected (225), virus deleted (226), disabled by policy (1260), integrity policy (4551).
pub fn classify_open_error(kind: std::io::ErrorKind, raw: Option<i32>, windows: bool) -> BrokenWhy {
    if kind == std::io::ErrorKind::PermissionDenied
        || (windows && matches!(raw, Some(5 | 225 | 226 | 1260 | 4551)))
    {
        BrokenWhy::Blocked
    } else {
        BrokenWhy::Missing
    }
}

/// A part found on disk.
#[derive(Clone, Debug)]
struct Found {
    dir: PathBuf,
    marker: Marker,
}

/// What the disk says about a row, with the part a problem is in.
#[derive(Clone, Debug, PartialEq, Eq)]
enum DiskState {
    NotInstalled,
    Partial(u64),
    Broken(BrokenWhy, String),
    NeedsUpdate(UpdateWhy),
    Installed,
}

/// Downloads, status and removal for the reverse rows. One per app.
pub struct Installer {
    layout: ReverseLayout,
    platform: Platform,
    lock: Arc<ReverseLock>,
    fetch: Arc<dyn Fetch>,
    free_space: FreeSpace,
    opener: Opener,
    fs_name: FsName,
    release_rules: bool,
    active: Mutex<HashMap<Component, ComponentState>>,
}

/// Releases a row's download slot when the download ends, however it ends.
struct Claim<'a> {
    active: &'a Mutex<HashMap<Component, ComponentState>>,
    components: Vec<Component>,
}

impl Drop for Claim<'_> {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        for c in &self.components {
            active.remove(c);
        }
    }
}

/// The name of a part's resumable download: named after its hash, so a changed pin never
/// resumes stale bytes.
pub fn part_file_name(f: &PackFile) -> String {
    format!("{}.{}.part", f.name, &f.sha256[..16.min(f.sha256.len())])
}

/// `<name>-<version>` folders of a part, as written by any app version.
fn is_part_dir(name: &str, part: &str) -> Option<String> {
    let version = name.strip_prefix(part)?.strip_prefix('-')?;
    version_key(version).map(|_| version.to_string())
}

fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

fn remove_dir_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

fn remove_file_if_present(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// Bytes of the files under `dir` (links not followed; at most 32 levels and 200,000
/// entries are counted).
fn folder_bytes(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: usize, left: &mut usize) -> u64 {
        if depth > 32 {
            return 0;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        let mut total = 0;
        for e in entries.filter_map(Result::ok) {
            if *left == 0 {
                break;
            }
            *left -= 1;
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                total += walk(&e.path(), depth + 1, left);
            } else if t.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        total
    }
    let mut left = 200_000;
    walk(dir, 0, &mut left)
}

impl Installer {
    /// The app's installer: the embedded lock, the network, the real disk.
    pub fn new(layout: ReverseLayout, platform: Platform) -> Self {
        Self {
            layout,
            platform,
            lock: Arc::new(lock::embedded().clone()),
            fetch: Arc::new(NetFetch),
            free_space: diskspace::system(),
            opener: Arc::new(open_for_read),
            fs_name: Arc::new(diskspace::filesystem_name),
            release_rules: platform::RELEASE_RULES,
            active: Mutex::new(HashMap::new()),
        }
    }
    /// Uses another lock (tests).
    pub fn with_lock(mut self, lock: Arc<ReverseLock>) -> Self {
        self.lock = lock;
        self
    }
    /// Fetches through `fetch` (tests, WP9e's cache).
    pub fn with_fetch(mut self, fetch: Arc<dyn Fetch>) -> Self {
        self.fetch = fetch;
        self
    }
    /// Reads free disk space through `free_space` (tests).
    pub fn with_free_space(mut self, free_space: FreeSpace) -> Self {
        self.free_space = free_space;
        self
    }
    /// Opens programs through `opener` when checking an install (tests).
    pub fn with_opener(mut self, opener: Opener) -> Self {
        self.opener = opener;
        self
    }
    /// Reads the volume's file system through `fs_name` (tests).
    pub fn with_fs_name(mut self, fs_name: FsName) -> Self {
        self.fs_name = fs_name;
        self
    }
    /// Follows the release rules (only parts with their publishers' checks are offered) or
    /// not; by default a release build does and a debug build does not (tests).
    pub fn with_release_rules(mut self, on: bool) -> Self {
        self.release_rules = on;
        self
    }

    /// The lock in use.
    pub fn lock(&self) -> &ReverseLock {
        &self.lock
    }
    /// The folders.
    pub fn layout(&self) -> &ReverseLayout {
        &self.layout
    }
    /// The platform.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// The parts of a row offered here, in lock order.
    fn parts(&self, component: Component) -> Vec<(&Part, &Build)> {
        self.lock
            .parts_of(component)
            .filter_map(|p| p.build_for(self.platform).map(|b| (p, b)))
            .collect()
    }

    /// Where a pinned file's resumable download lives, or `None` when its name or digest
    /// would leave `.downloads/` (never joined, never fetched).
    fn part_path(&self, f: &PackFile) -> Option<PathBuf> {
        safe_pinned_file(f).then(|| self.layout.downloads().join(part_file_name(f)))
    }

    /// Bytes of a part's paused download.
    fn part_bytes(&self, f: &PackFile) -> u64 {
        self.part_path(f)
            .and_then(|p| std::fs::metadata(p).ok())
            .map_or(0, |m| m.len())
    }

    /// Deletes the paused downloads of a row once every one of its parts is in place, so a
    /// Cancel while a later part unpacks keeps the row resumable ("Paused at …").
    fn forget_downloads_if_complete(&self, component: Component) {
        let parts = self.parts(component);
        if parts.iter().all(|(p, _)| self.pinned_ok(p)) {
            for (_, b) in parts {
                if let Some(path) = self.part_path(&b.file) {
                    let _ = remove_file_if_present(&path);
                }
            }
        }
    }

    fn read_marker(&self, dir: &Path, part: &Part, version: &str) -> Option<Marker> {
        let text = std::fs::read_to_string(dir.join(COMPLETE_MARKER)).ok()?;
        let marker: Marker = serde_json::from_str(&text).ok()?;
        (marker.part == part.name
            && marker.version == version
            && marker.component == part.component)
            .then_some(marker)
    }

    /// The copy of a part that would be used: the pinned version if complete, else the
    /// newest complete one.
    fn find(&self, part: &Part) -> Option<Found> {
        let pinned = self.layout.part_dir(&part.dir);
        if let Some(marker) = self.read_marker(&pinned, part, &part.version) {
            return Some(Found {
                dir: pinned,
                marker,
            });
        }
        let mut found: Vec<(Vec<u64>, Found)> = std::fs::read_dir(self.layout.root())
            .ok()?
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let version = is_part_dir(&name, &part.name)?;
                let marker = self.read_marker(&e.path(), part, &version)?;
                Some((
                    version_key(&version)?,
                    Found {
                        dir: e.path(),
                        marker,
                    },
                ))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0));
        found.into_iter().next().map(|(_, f)| f)
    }

    /// Checks every recorded program of a found part: present, a file, its recorded size,
    /// and openable.
    fn check_programs(&self, found: &Found) -> Result<(), BrokenWhy> {
        for p in &found.marker.programs {
            if safe_relative(&p.path).is_none() {
                return Err(BrokenWhy::Missing);
            }
            let path = join_relative(&found.dir, &p.path);
            let ok =
                std::fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && m.len() == p.size);
            if !ok {
                return Err(BrokenWhy::Missing);
            }
            if let Err(e) = (self.opener)(&path) {
                return Err(classify_open_error(
                    e.kind(),
                    e.raw_os_error(),
                    cfg!(windows),
                ));
            }
        }
        Ok(())
    }

    fn partial_bytes(&self, component: Component) -> u64 {
        self.parts(component)
            .iter()
            .map(|(_, b)| self.part_bytes(&b.file))
            .sum()
    }

    fn disk_state(&self, component: Component) -> (DiskState, Vec<Option<Found>>) {
        let parts = self.parts(component);
        if parts.is_empty() {
            return (DiskState::NotInstalled, Vec::new());
        }
        let found: Vec<Option<Found>> = parts.iter().map(|(p, _)| self.find(p)).collect();
        if found.iter().all(Option::is_none) {
            let bytes = self.partial_bytes(component);
            let state = if bytes > 0 {
                DiskState::Partial(bytes)
            } else {
                DiskState::NotInstalled
            };
            return (state, found);
        }
        // Some parts in place and the others still downloaded but not unpacked: a Cancel
        // while a later part unpacked. The row is paused, not broken.
        let missing: Vec<&Build> = parts
            .iter()
            .zip(&found)
            .filter(|(_, f)| f.is_none())
            .map(|((_, b), _)| *b)
            .collect();
        if !missing.is_empty() && missing.iter().all(|b| self.part_bytes(&b.file) > 0) {
            return (DiskState::Partial(self.partial_bytes(component)), found);
        }
        for ((part, _), f) in parts.iter().zip(&found) {
            let Some(f) = f else {
                return (
                    DiskState::Broken(BrokenWhy::Missing, part.name.clone()),
                    found,
                );
            };
            match part.accepts(&f.marker.version) {
                Acceptance::Yes => {}
                Acceptance::BelowMinimum => {
                    let why = UpdateWhy::SecurityFix {
                        part: part.name.clone(),
                    };
                    return (DiskState::NeedsUpdate(why), found);
                }
                Acceptance::NotAccepted => {
                    let why = UpdateWhy::NotAccepted {
                        need: part.version.clone(),
                    };
                    return (DiskState::NeedsUpdate(why), found);
                }
            }
        }
        for ((part, _), f) in parts.iter().zip(&found) {
            if let Some(f) = f
                && let Err(why) = self.check_programs(f)
            {
                return (DiskState::Broken(why, part.name.clone()), found);
            }
        }
        (DiskState::Installed, found)
    }

    /// What the disk says about a row, without a download in progress.
    pub fn verify_installed(&self, component: Component) -> ComponentState {
        match self.disk_state(component).0 {
            DiskState::NotInstalled => ComponentState::NotInstalled,
            DiskState::Partial(bytes) => ComponentState::Partial { bytes },
            DiskState::Broken(why, _) => ComponentState::Broken { why },
            DiskState::NeedsUpdate(why) => ComponentState::NeedsUpdate { why },
            DiskState::Installed => ComponentState::Installed,
        }
    }

    /// Where a row stands now, a download in progress included.
    pub fn state(&self, component: Component) -> ComponentState {
        let active = self
            .active
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&component)
            .cloned();
        active.unwrap_or_else(|| self.verify_installed(component))
    }

    /// A row's status for the tab.
    pub fn status(&self, component: Component) -> ComponentStatus {
        let (available, reason) =
            platform::availability_with(&self.lock, component, self.platform, self.release_rules);
        let parts = self.parts(component);
        let (_, found) = self.disk_state(component);
        let main_pinned = self
            .lock
            .parts_of(component)
            .next()
            .map(|p| p.version.clone())
            .unwrap_or_default();
        let installed_version = found
            .first()
            .and_then(|f| f.as_ref())
            .map(|f| f.marker.version.clone());
        let all_found = !found.is_empty() && found.iter().all(Option::is_some);
        let disk_bytes = if all_found {
            found.iter().flatten().map(|f| f.marker.disk_bytes).sum()
        } else {
            parts.iter().map(|(_, b)| b.unpacked_bytes).sum()
        };
        // Parts whose installed copy differs from the pin: (part, installed version, build).
        let stale: Vec<(&Part, &str, &Build)> = parts
            .iter()
            .zip(&found)
            .filter_map(|((p, b), f)| {
                let f = f.as_ref()?;
                (f.marker.version != p.version).then_some((*p, f.marker.version.as_str(), *b))
            })
            .collect();
        // The row's main part when it changes, else the first part that does.
        let update = stale
            .first()
            .filter(|_| all_found)
            .map(|(p, from, _)| UpdateInfo {
                part: p.name.clone(),
                from: (*from).to_string(),
                to: p.version.clone(),
                download_bytes: stale.iter().map(|(_, _, b)| b.file.size).sum(),
            });
        ComponentStatus {
            component,
            available,
            reason,
            state: self.state(component),
            installed_version,
            pinned_version: main_pinned,
            parts: parts
                .iter()
                .zip(found.iter().chain(std::iter::repeat(&None)))
                .map(|((p, _), f)| PartInfo {
                    name: p.name.clone(),
                    version: f
                        .as_ref()
                        .map_or_else(|| p.version.clone(), |f| f.marker.version.clone()),
                })
                .collect(),
            download_bytes: parts.iter().map(|(_, b)| b.file.size).sum(),
            disk_bytes,
            update,
            licenses: self
                .lock
                .parts_of(component)
                .map(|p| LicenseInfo {
                    part: p.name.clone(),
                    name: p.license_name.clone(),
                    url: p.license_url.clone(),
                })
                .collect(),
        }
    }

    /// Every row's status, in the tab's order.
    pub fn statuses(&self) -> Vec<ComponentStatus> {
        Component::ALL.iter().map(|c| self.status(*c)).collect()
    }

    /// The row if it is installed and usable, else why not.
    pub fn usable(&self, component: Component) -> Result<Installed, ReverseError> {
        let (state, found) = self.disk_state(component);
        match state {
            DiskState::NotInstalled | DiskState::Partial(_) => {
                Err(ReverseError::NotInstalled { component })
            }
            DiskState::Broken(BrokenWhy::Missing, part) => Err(ReverseError::Missing { part }),
            DiskState::Broken(BrokenWhy::Blocked, part) => Err(ReverseError::Blocked { part }),
            DiskState::NeedsUpdate(UpdateWhy::SecurityFix { part }) => {
                Err(ReverseError::NeedsUpdate { part })
            }
            DiskState::NeedsUpdate(UpdateWhy::NotAccepted { .. }) => {
                Err(ReverseError::NeedsUpdate { part: "rea".into() })
            }
            DiskState::Installed => {
                let parts: Vec<InstalledPart> = found
                    .into_iter()
                    .flatten()
                    .map(|f| InstalledPart {
                        name: f.marker.part,
                        version: f.marker.version,
                        dir: f.dir,
                        programs: f.marker.programs,
                        archive_sha256: f.marker.archive_sha256,
                        disk_bytes: f.marker.disk_bytes,
                    })
                    .collect();
                Ok(Installed {
                    component,
                    version: parts.first().map(|p| p.version.clone()).unwrap_or_default(),
                    disk_bytes: parts.iter().map(|p| p.disk_bytes).sum(),
                    parts,
                })
            }
        }
    }

    /// What is installed and usable now, with the paths rea's environment needs.
    pub fn installation(&self) -> Installation {
        let rea = self.usable(Component::Rea).ok();
        let ghidra = self.usable(Component::Ghidra).ok();
        let rea_entry = rea.as_ref().and_then(|r| {
            let entry = self.lock.part("rea")?.entry.clone()?;
            safe_relative(&entry)?;
            Some(join_relative(&r.part("rea")?.dir, &entry))
        });
        let java_home = ghidra.as_ref().and_then(|g| {
            let java = g.part("java")?;
            let rel = self
                .lock
                .part("java")?
                .build_for(self.platform)?
                .java_home
                .clone()
                .unwrap_or_default();
            if rel.is_empty() {
                Some(java.dir.clone())
            } else {
                safe_relative(&rel)?;
                Some(join_relative(&java.dir, &rel))
            }
        });
        Installation {
            rea,
            kuna: self.usable(Component::Kuna).ok(),
            ghidra,
            rea_entry,
            java_home,
        }
    }

    /// Whether a part's pinned version is complete and its programs check out.
    fn pinned_ok(&self, part: &Part) -> bool {
        let dir = self.layout.part_dir(&part.dir);
        self.read_marker(&dir, part, &part.version)
            .is_some_and(|marker| self.check_programs(&Found { dir, marker }).is_ok())
    }

    fn claim(&self, components: &[Component]) -> Result<Claim<'_>, ReverseError> {
        let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
        if components.iter().any(|c| active.contains_key(c)) {
            return Err(ReverseError::Busy);
        }
        for c in components {
            active.insert(
                *c,
                ComponentState::Downloading {
                    done: 0,
                    total: 0,
                    stage: DownloadStage::Downloading,
                },
            );
        }
        Ok(Claim {
            active: &self.active,
            components: components.to_vec(),
        })
    }

    fn report(
        &self,
        claimed: &[Component],
        progress: &(dyn Fn(Progress) + Send + Sync),
        p: Progress,
    ) {
        {
            let mut active = self.active.lock().unwrap_or_else(|e| e.into_inner());
            for c in claimed {
                active.insert(
                    *c,
                    ComponentState::Downloading {
                        done: p.done,
                        total: p.total,
                        stage: p.stage,
                    },
                );
            }
        }
        progress(p);
    }

    /// The checks made before anything is fetched: the path length on Windows, the
    /// volume's file system for Ghidra on Windows, and free disk space.
    fn preflight_checks(&self, plan: &[(&Part, &Build)]) -> Result<(), ReverseError> {
        if self.platform.os() == Os::Windows {
            for (part, build) in plan {
                let tmp = self.layout.tmp_part_dir(&part.dir);
                let chars = tmp.to_string_lossy().chars().count() + 1 + build.max_path_chars;
                if chars > WINDOWS_MAX_PATH {
                    return Err(ReverseError::PathTooLong {
                        chars,
                        max: WINDOWS_MAX_PATH,
                    });
                }
            }
            if plan.iter().any(|(p, _)| p.component == Component::Ghidra)
                && !diskspace::local_ntfs_or_refs((self.fs_name)(self.layout.root()).as_deref())
            {
                return Err(ReverseError::NotNtfs);
            }
        }
        let mut need: u64 = 0;
        for (_, build) in plan {
            let have = self.part_bytes(&build.file).min(build.file.size);
            need = need
                .saturating_add(build.file.size - have)
                .saturating_add(build.unpacked_bytes);
        }
        let need = need.saturating_add(need / 10);
        if let Some(free) = (self.free_space)(self.layout.root())
            && free < need
        {
            return Err(ReverseError::DiskFull { need_bytes: need });
        }
        Ok(())
    }

    /// Downloads, checks and unpacks a row (and rea first, when rea is missing), or repairs
    /// it. A paused download resumes; cancelling keeps what was downloaded.
    pub async fn download(
        &self,
        component: Component,
        cancel: &CancellationToken,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<Installed, ReverseError> {
        let (available, reason) =
            platform::availability_with(&self.lock, component, self.platform, self.release_rules);
        if !available {
            return Err(ReverseError::NotAvailable {
                reason: reason.unwrap_or_default(),
            });
        }
        let mut rows = Vec::new();
        if component != Component::Rea
            && matches!(
                self.disk_state(Component::Rea).0,
                DiskState::NotInstalled | DiskState::Partial(_)
            )
        {
            rows.push(Component::Rea);
        }
        rows.push(component);
        let _claim = self.claim(&rows)?;
        let mut plan: Vec<(&Part, &Build)> = rows
            .iter()
            .flat_map(|c| self.parts(*c))
            .filter(|(p, _)| !self.pinned_ok(p))
            .collect();
        plan.dedup_by(|a, b| a.0.name == b.0.name);
        if plan.is_empty() {
            return self.usable(component);
        }
        let mut hosts = Vec::new();
        for (part, build) in &plan {
            if !safe_pinned_file(&build.file) {
                return Err(ReverseError::Archive {
                    part: part.name.clone(),
                    message: format!("the pinned file name {:?} is unsafe", build.file.name),
                });
            }
            hosts.push(part.host_set().ok_or_else(|| ReverseError::BlockedHost {
                host: part.hosts.join(", "),
            })?);
        }
        self.preflight_checks(&plan)?;
        let downloads = self.layout.downloads();
        tokio::fs::create_dir_all(&downloads).await?;

        // 1. Fetch every file (resumable).
        let total: u64 = plan.iter().map(|(_, b)| b.file.size).sum();
        let mut base = 0u64;
        for ((part, build), hosts) in plan.iter().zip(&hosts) {
            let dest = downloads.join(part_file_name(&build.file));
            let name = part.name.clone();
            let at = base;
            self.report(
                &rows,
                progress,
                Progress {
                    component,
                    part: name.clone(),
                    done: at,
                    total,
                    stage: DownloadStage::Downloading,
                },
            );
            self.fetch
                .fetch(&build.file, &dest, hosts, cancel, &|n| {
                    self.report(
                        &rows,
                        progress,
                        Progress {
                            component,
                            part: name.clone(),
                            done: at + n.min(build.file.size),
                            total,
                            stage: DownloadStage::Downloading,
                        },
                    )
                })
                .await?;
            base += build.file.size;
        }

        // 2. Check every file's size and SHA-256 before anything is unpacked.
        let mut checked = 0u64;
        for (part, build) in &plan {
            self.report(
                &rows,
                progress,
                Progress {
                    component,
                    part: part.name.clone(),
                    done: checked,
                    total,
                    stage: DownloadStage::Checking,
                },
            );
            let dest = downloads.join(part_file_name(&build.file));
            let size = tokio::fs::metadata(&dest)
                .await
                .map(|m| m.len())
                .unwrap_or(0);
            let path = dest.clone();
            let sha = if size == build.file.size {
                tokio::task::spawn_blocking(move || sha256_file(&path))
                    .await
                    .map_err(|e| ReverseError::Io {
                        message: e.to_string(),
                    })??
            } else {
                String::new()
            };
            if size != build.file.size || !sha.eq_ignore_ascii_case(&build.file.sha256) {
                let _ = tokio::fs::remove_file(&dest).await;
                return Err(ReverseError::Mismatch {
                    part: part.name.clone(),
                });
            }
            checked += build.file.size;
        }

        // 3. Unpack each part into `.tmp-<dir>`, marker last, then rename into place. A
        // row's downloads are deleted once all of its parts are in place.
        for (part, build) in &plan {
            self.unpack_part(&rows, component, part, build, cancel, progress)
                .await?;
            self.forget_downloads_if_complete(part.component);
        }

        // 4. Older versions of these parts are no longer used (best effort: on Windows a
        // running rea keeps its files open until it stops).
        for (part, _) in &plan {
            self.remove_other_versions(part);
        }
        tracing::info!(?component, "reverse engineering part installed");
        self.usable(component)
    }

    /// The same as [`download`](Self::download): the pinned version goes into its own
    /// folder and the old one is deleted once the new one is complete.
    pub async fn update(
        &self,
        component: Component,
        cancel: &CancellationToken,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<Installed, ReverseError> {
        self.download(component, cancel, progress).await
    }

    async fn unpack_part(
        &self,
        rows: &[Component],
        component: Component,
        part: &Part,
        build: &Build,
        cancel: &CancellationToken,
        progress: &(dyn Fn(Progress) + Send + Sync),
    ) -> Result<(), ReverseError> {
        let src = self.layout.downloads().join(part_file_name(&build.file));
        let tmp = self.layout.tmp_part_dir(&part.dir);
        let final_dir = self.layout.part_dir(&part.dir);
        let (tx, mut rx) = tokio::sync::watch::channel(0u64);
        let job = {
            let (build, part, tmp, src, final_dir, cancel) = (
                build.clone(),
                part.clone(),
                tmp.clone(),
                src.clone(),
                final_dir.clone(),
                cancel.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let done = unpack_blocking(&part, &build, &src, &tmp, &final_dir, &cancel, &|n| {
                    tx.send_replace(n);
                });
                if done.is_err() {
                    let _ = remove_dir_if_present(&tmp);
                }
                done
            })
        };
        tokio::pin!(job);
        let total = build.unpacked_bytes;
        let report = |done: u64| {
            self.report(
                rows,
                progress,
                Progress {
                    component,
                    part: part.name.clone(),
                    done: done.min(total),
                    total,
                    stage: DownloadStage::Unpacking,
                },
            )
        };
        report(0);
        let result = loop {
            tokio::select! {
                r = &mut job => break r,
                changed = rx.changed() => {
                    if changed.is_ok() {
                        report(*rx.borrow_and_update());
                    }
                }
            }
        };
        result.map_err(|e| ReverseError::Io {
            message: e.to_string(),
        })??;
        Ok(())
    }

    fn remove_other_versions(&self, part: &Part) {
        let Ok(entries) = std::fs::read_dir(self.layout.root()) else {
            return;
        };
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name().to_string_lossy().into_owned();
            let bare = name.strip_prefix(".tmp-").unwrap_or(&name);
            if let Some(version) = is_part_dir(bare, &part.name)
                && (version != part.version || name.starts_with(".tmp-"))
                && e.file_type().is_ok_and(|t| t.is_dir())
                && let Err(err) = std::fs::remove_dir_all(e.path())
            {
                tracing::warn!(folder = %name, "an older reverse-engineering folder could not be removed yet: {err}");
            }
        }
    }

    /// Removes a row: every version of its parts, its paused download and, for Kuna, its
    /// saved analysis. Removing rea removes everything (Kuna and Ghidra need it). The
    /// caller stops rea first. Only the reverse folder's own names are touched.
    pub async fn remove(&self, component: Component) -> Result<(), ReverseError> {
        // Never under a download that is writing into these folders.
        let rows: Vec<Component> = match component {
            Component::Rea => Component::ALL.to_vec(),
            other => vec![other],
        };
        let _claim = self.claim(&rows)?;
        let names: Vec<String> = match component {
            Component::Rea => self.lock.parts.iter().map(|p| p.name.clone()).collect(),
            other => self.lock.parts_of(other).map(|p| p.name.clone()).collect(),
        };
        let root = self.layout.root().to_path_buf();
        let layout = self.layout.clone();
        let files: Vec<String> = self
            .lock
            .parts
            .iter()
            .filter(|p| names.contains(&p.name))
            .flat_map(|p| p.builds.iter())
            .filter(|b| safe_pinned_file(&b.file))
            .map(|b| part_file_name(&b.file))
            .collect();
        tokio::task::spawn_blocking(move || -> Result<(), ReverseError> {
            let Ok(entries) = std::fs::read_dir(&root) else {
                return Ok(());
            };
            for e in entries.filter_map(Result::ok) {
                let name = e.file_name().to_string_lossy().into_owned();
                let bare = name.strip_prefix(".tmp-").unwrap_or(&name);
                let ours = names.iter().any(|n| is_part_dir(bare, n).is_some());
                if ours && e.file_type().is_ok_and(|t| t.is_dir()) {
                    std::fs::remove_dir_all(e.path())?;
                }
            }
            for f in &files {
                remove_file_if_present(&layout.downloads().join(f))?;
            }
            match component {
                Component::Rea => {
                    for dir in [
                        layout.downloads(),
                        layout.home(),
                        layout.tmp(),
                        layout.cache(),
                        layout.work(),
                    ] {
                        remove_dir_if_present(&dir)?;
                    }
                }
                Component::Kuna => remove_dir_if_present(&layout.kuna_cache())?,
                Component::Ghidra => {}
            }
            Ok(())
        })
        .await
        .map_err(|e| ReverseError::Io {
            message: e.to_string(),
        })?
    }

    /// Deletes a row's paused download (Remove on "Paused at …").
    /// When some of the row's parts were already unpacked (a Cancel while a later part
    /// unpacked), those are removed too, so the row is "Not installed" again.
    pub async fn discard_partial(&self, component: Component) -> Result<(), ReverseError> {
        let _claim = self.claim(&[component])?;
        let (state, found) = self.disk_state(component);
        for (part, build) in self.parts(component) {
            if let Some(path) = self.part_path(&build.file) {
                remove_file_if_present(&path)?;
            }
            remove_dir_if_present(&self.layout.tmp_part_dir(&part.dir))?;
        }
        if matches!(state, DiskState::Partial(_)) {
            for f in found.into_iter().flatten() {
                remove_dir_if_present(&f.dir)?;
            }
        }
        Ok(())
    }

    /// Bytes of Kuna's saved analysis.
    pub fn cache_bytes(&self) -> u64 {
        folder_bytes(&self.layout.kuna_cache())
    }

    /// Deletes Kuna's saved analysis (the caller stops rea first).
    pub async fn clear_cache(&self) -> Result<(), ReverseError> {
        Ok(remove_dir_if_present(&self.layout.kuna_cache())?)
    }
}

/// Unpacks one checked archive into `.tmp-<dir>`, checks its programs, writes the marker
/// and renames the folder into place. Blocking.
fn unpack_blocking(
    part: &Part,
    build: &Build,
    src: &Path,
    tmp: &Path,
    final_dir: &Path,
    cancel: &CancellationToken,
    progress: &dyn Fn(u64),
) -> Result<(), ReverseError> {
    let bad = |message: String| ReverseError::Archive {
        part: part.name.clone(),
        message,
    };
    remove_dir_if_present(tmp)?;
    let report = archive::unpack(src, tmp, &Rules::for_build(build), cancel, progress).map_err(
        |e| match e {
            ArchiveError::Refused(m) => bad(m),
            ArchiveError::Cancelled => ReverseError::Cancelled,
            ArchiveError::Io(e) => ReverseError::from(e),
        },
    )?;
    let mut programs = Vec::new();
    for p in &build.programs {
        if safe_relative(&p.path).is_none() {
            return Err(bad(format!(
                "the pinned program path {:?} is unsafe",
                p.path
            )));
        }
        let path = join_relative(tmp, &p.path);
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| bad(format!("{} is missing from the archive", p.path)))?;
        if !meta.is_file() || meta.len() != p.size {
            return Err(bad(format!("{} is not the pinned file", p.path)));
        }
        #[cfg(unix)]
        if p.exec {
            use std::os::unix::fs::PermissionsExt;
            let mode = meta.permissions().mode();
            if mode & 0o100 == 0 {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode | 0o755))?;
            }
        }
        programs.push(ProgramFile {
            path: p.path.clone(),
            size: p.size,
        });
    }
    let marker = Marker {
        component: part.component,
        part: part.name.clone(),
        version: part.version.clone(),
        archive_sha256: vec![build.file.sha256.clone()],
        programs,
        disk_bytes: report.bytes,
    };
    let text = serde_json::to_string(&marker).map_err(|e| ReverseError::Io {
        message: e.to_string(),
    })?;
    std::fs::write(tmp.join(COMPLETE_MARKER), text)?;
    remove_dir_if_present(final_dir)?;
    rename_with_retries(tmp, final_dir)?;
    Ok(())
}

/// Renames a freshly unpacked folder into place. On Windows, antivirus software often holds
/// new files open for a moment after they are written, which makes the rename fail with
/// "access denied"; it is tried again for up to about three seconds.
fn rename_with_retries(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied && attempt < 10 => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(300));
            }
            other => return other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_folders_are_recognised_by_name_and_version() {
        assert_eq!(is_part_dir("rea-6.9.0", "rea"), Some("6.9.0".into()));
        assert_eq!(
            is_part_dir("java-21.0.13+9", "java"),
            Some("21.0.13+9".into())
        );
        assert_eq!(is_part_dir("rea-agents", "rea"), None);
        assert_eq!(is_part_dir("reaX-1.0", "rea"), None);
        assert_eq!(is_part_dir("home", "rea"), None);
        assert_eq!(is_part_dir("node-", "node"), None);
    }

    #[test]
    fn open_errors_are_classified() {
        use std::io::ErrorKind;
        assert_eq!(
            classify_open_error(ErrorKind::PermissionDenied, None, false),
            BrokenWhy::Blocked
        );
        for code in [5, 225, 226, 1260, 4551] {
            assert_eq!(
                classify_open_error(ErrorKind::Other, Some(code), true),
                BrokenWhy::Blocked,
                "{code}"
            );
            assert_eq!(
                classify_open_error(ErrorKind::Other, Some(code), false),
                BrokenWhy::Missing,
                "{code} is a Windows code"
            );
        }
        assert_eq!(
            classify_open_error(ErrorKind::NotFound, Some(2), true),
            BrokenWhy::Missing
        );
    }

    #[test]
    fn pack_errors_say_what_happened() {
        let net = || PackError::Network {
            file: "f".into(),
            message: "dns".into(),
        };
        assert_eq!(
            from_pack_error(net(), "node", "nodejs.org", false),
            ReverseError::CantReach {
                host: "nodejs.org".into()
            }
        );
        assert_eq!(
            from_pack_error(net(), "node", "nodejs.org", true),
            ReverseError::Interrupted {
                part: "node".into()
            }
        );
        assert_eq!(
            from_pack_error(PackError::BlockedHost("evil".into()), "rea", "h", true),
            ReverseError::BlockedHost {
                host: "evil".into()
            }
        );
        assert_eq!(
            from_pack_error(
                PackError::ChecksumMismatch { file: "x".into() },
                "kuna",
                "h",
                true
            ),
            ReverseError::Mismatch {
                part: "kuna".into()
            }
        );
        assert_eq!(
            from_pack_error(PackError::Cancelled, "rea", "h", true),
            ReverseError::Cancelled
        );
    }

    /// A host that answered with an error status was reached: "can't reach" would send the
    /// owner to check a connection that works.
    #[test]
    fn an_http_error_status_is_not_cant_reach() {
        for (status, got) in [(404, false), (403, false), (503, false), (410, true)] {
            let e = PackError::Network {
                file: "f".into(),
                message: format!("HTTP {status}"),
            };
            assert_eq!(
                from_pack_error(e, "ghidra", "github.com", got),
                ReverseError::ServerError {
                    host: "github.com".into(),
                    status
                },
                "{status}"
            );
        }
        let dropped = PackError::Network {
            file: "f".into(),
            message: "the connection kept dropping".into(),
        };
        assert_eq!(
            from_pack_error(dropped, "ghidra", "github.com", true),
            ReverseError::Interrupted {
                part: "ghidra".into()
            }
        );
        assert_eq!(http_status("HTTP 404"), Some(404));
        assert_eq!(http_status("HTTPS 404"), None);
        assert_eq!(http_status("dns error"), None);
    }

    #[tokio::test]
    async fn net_fetch_refuses_a_file_on_another_host_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let f = PackFile {
            name: "x.tgz".into(),
            url: "https://evil.example/x.tgz".into(),
            size: 1,
            sha256: "0".repeat(64),
        };
        let e = NetFetch
            .fetch(
                &f,
                &dir.path().join("x.part"),
                lock::NPM_HOSTS,
                &CancellationToken::new(),
                &|_| {},
            )
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::BlockedHost {
                host: "evil.example".into()
            }
        );
        assert!(!dir.path().join("x.part").exists());
    }

    /// The preflight turns a host that refuses connections into "can't reach" at once.
    #[tokio::test]
    async fn an_unreachable_host_is_cant_reach() {
        // A loopback port with nothing listening refuses the connection immediately.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let url = reqwest::Url::parse(&format!("https://127.0.0.1:{port}/x")).unwrap();
        let e = preflight(&url, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::CantReach {
                host: "127.0.0.1".into()
            }
        );
    }
}
