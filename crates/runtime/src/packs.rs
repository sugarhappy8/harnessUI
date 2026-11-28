//! Downloadable backends (ARCHITECTURE.md 7.1, decisions D1, D2 and D4): CUDA and ROCm
//! are one-click downloads in Settings, pinned by the compiled-in lock file and verified
//! file by file before use.
//!
//! A pack is downloaded into `<packs>/.downloads/` (resumable), every archive is checked
//! against its SHA-256 and size, unpacked into `<packs>/.tmp-<dir>/`, the VC++ runtime
//! DLLs are copied in, a `.complete` marker is written, and only then is the folder
//! renamed to `<packs>/<dir>`. Downloads come from GitHub release assets only.
//! [`install_files`] does this for llama.cpp's packs ([`install`]) and for the Studio
//! engine's Windows pack ([`crate::sd::engine::install`]).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

use crate::llama::backend::PACK_COMPLETE_MARKER;
use crate::llama::lock::{LlamaLock, PackFile, PackSpec};

/// Hosts a pack download may be served from: GitHub and its release-asset CDN.
const ALLOWED_HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

/// The ROCm pack's kernel files exceed Windows' path limit below this depth.
pub const ROCM_MAX_DIR_CHARS: usize = 118;

/// Why installing a pack failed.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    /// The pack is for another platform.
    #[error("this backend is not available for this platform")]
    WrongPlatform,
    /// The ROCm folder would exceed Windows' path limit.
    #[error(
        "the backends folder path is too long ({0} characters); ROCm needs at most {ROCM_MAX_DIR_CHARS}"
    )]
    PathTooLong(usize),
    /// The user cancelled.
    #[error("the download was cancelled")]
    Cancelled,
    /// The URL or a redirect is on a host outside the allowed list.
    #[error("the download was redirected to a host the app does not trust: {0}")]
    BlockedHost(String),
    /// An archive's SHA-256 differs from the lock file.
    #[error("{file} does not match its pinned checksum")]
    ChecksumMismatch {
        /// The archive.
        file: String,
    },
    /// An archive's size differs from the lock file.
    #[error("{file} has the wrong size ({got} bytes, expected {expected})")]
    SizeMismatch {
        /// The archive.
        file: String,
        /// Bytes received.
        got: u64,
        /// Bytes pinned.
        expected: u64,
    },
    /// The download failed.
    #[error("could not download {file}: {message}")]
    Network {
        /// The archive.
        file: String,
        /// What went wrong.
        message: String,
    },
    /// The archive is damaged or unsafe.
    #[error("the archive {file} could not be unpacked: {message}")]
    Archive {
        /// The archive.
        file: String,
        /// What went wrong.
        message: String,
    },
    /// The disk is full.
    #[error("the disk is full")]
    DiskFull,
    /// Another file-system error.
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for PackError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::StorageFull
            || e.raw_os_error() == Some(28)
            || e.raw_os_error() == Some(112)
        {
            PackError::DiskFull
        } else {
            PackError::Io(e.to_string())
        }
    }
}

/// Where a pack stands on this machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackState {
    /// Nothing on disk.
    NotInstalled,
    /// Some archives (or parts of them) are already downloaded.
    Partial {
        /// Bytes on disk.
        bytes: u64,
    },
    /// Unpacked, with its `.complete` marker.
    Installed,
}

/// Progress of an install: bytes downloaded so far out of the pack's total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackProgress {
    /// Bytes downloaded.
    pub done: u64,
    /// Bytes in all archives.
    pub total: u64,
    /// True once every archive is downloaded and the pack is being unpacked.
    pub unpacking: bool,
}

/// Where downloads in progress are kept: `<packs>/.downloads`.
fn downloads_dir(packs_dir: &Path) -> PathBuf {
    packs_dir.join(".downloads")
}

/// Bytes of `files` already downloaded into `<root>/.downloads` (whole or in part) and not
/// yet unpacked: what [`PackState::Partial`] reports.
pub(crate) fn partial_bytes(files: &[PackFile], root: &Path) -> u64 {
    let downloads = downloads_dir(root);
    files
        .iter()
        .filter_map(|f| std::fs::metadata(downloads.join(part_name(f))).ok())
        .map(|m| m.len())
        .sum()
}

/// The pack's state from what is on disk.
pub fn state(pack: &PackSpec, packs_dir: &Path) -> PackState {
    if packs_dir
        .join(&pack.dir)
        .join(PACK_COMPLETE_MARKER)
        .is_file()
    {
        return PackState::Installed;
    }
    let bytes = partial_bytes(&pack.files, packs_dir);
    if bytes > 0 {
        PackState::Partial { bytes }
    } else {
        PackState::NotInstalled
    }
}

/// Partial files are named after their hash so a changed pin never resumes stale bytes.
pub(crate) fn part_name(f: &PackFile) -> String {
    format!("{}.{}.part", f.name, &f.sha256[..16.min(f.sha256.len())])
}

#[cfg(test)]
fn host_allowed(url: &reqwest::Url) -> bool {
    host_allowed_in(url, ALLOWED_HOSTS)
}

/// Whether `url` is https on exactly one of `hosts` (no subdomains, no other scheme).
fn host_allowed_in(url: &reqwest::Url, hosts: &[&str]) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(|h| hosts.contains(&h))
}

fn client() -> Result<reqwest::Client, PackError> {
    client_for(ALLOWED_HOSTS)
}

/// A client whose redirects stay on `hosts`.
fn client_for(hosts: &'static [&'static str]) -> Result<reqwest::Client, PackError> {
    reqwest::Client::builder()
        .user_agent(concat!("app-backend-download/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else if host_allowed_in(attempt.url(), hosts) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|e| PackError::Io(e.to_string()))
}

/// Downloads one pinned file to `dest` (resuming what is already there) and returns once
/// its size and SHA-256 match the pin. The URL and every redirect must be https on one of
/// `hosts`. For pinned downloads that are not llama.cpp backends (the agent's own
/// browser); backends go through [`install`].
pub async fn download_verified(
    f: &PackFile,
    dest: &Path,
    hosts: &'static [&'static str],
    cancel: &CancellationToken,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), PackError> {
    let client = client_for(hosts)?;
    fetch(&client, f, dest, hosts, cancel, on_bytes).await
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Downloads one archive with resume; returns once it is complete and verified.
/// The wait before retry number `attempt + 1` (2 to 32 seconds). Cancel ends it at once, so
/// a download that is retrying while offline stops as soon as the user says so.
async fn pause(attempt: u32, cancel: &CancellationToken) -> Result<(), PackError> {
    tokio::select! {
        () = cancel.cancelled() => Err(PackError::Cancelled),
        () = tokio::time::sleep(Duration::from_secs(2u64.pow(attempt + 1))) => Ok(()),
    }
}

async fn fetch(
    client: &reqwest::Client,
    f: &PackFile,
    dest: &Path,
    hosts: &[&str],
    cancel: &CancellationToken,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), PackError> {
    let url = reqwest::Url::parse(&f.url).map_err(|_| PackError::BlockedHost(f.url.clone()))?;
    if !host_allowed_in(&url, hosts) {
        return Err(PackError::BlockedHost(
            url.host_str().unwrap_or("?").to_string(),
        ));
    }
    let net = |e: reqwest::Error| PackError::Network {
        file: f.name.clone(),
        message: e.without_url().to_string(),
    };
    for attempt in 0..6u32 {
        let have = tokio::fs::metadata(dest)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        if have > f.size {
            tokio::fs::remove_file(dest).await?;
            continue;
        }
        if have == f.size {
            break;
        }
        let mut req = client
            .get(url.clone())
            .header("Accept-Encoding", "identity");
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = tokio::select! {
            _ = cancel.cancelled() => return Err(PackError::Cancelled),
            r = req.send() => r,
        };
        let resp = match resp {
            Ok(r) => r,
            Err(e) if attempt < 5 => {
                tracing::warn!(file = %f.name, "backend download interrupted: {}", e.without_url());
                pause(attempt, cancel).await?;
                continue;
            }
            Err(e) => return Err(net(e)),
        };
        if resp.status().is_redirection() {
            let host = resp
                .headers()
                .get("location")
                .and_then(|l| l.to_str().ok())
                .and_then(|l| reqwest::Url::parse(l).ok())
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "?".into());
            return Err(PackError::BlockedHost(host));
        }
        let append = match resp.status().as_u16() {
            206 => true,
            200 => false,
            416 => break,
            s => {
                return Err(PackError::Network {
                    file: f.name.clone(),
                    message: format!("HTTP {s}"),
                });
            }
        };
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(dest)
            .await?;
        let mut done = if append { have } else { 0 };
        on_bytes(done);
        let mut stream = resp.bytes_stream();
        let mut interrupted = false;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    tokio::io::AsyncWriteExt::flush(&mut file).await?;
                    return Err(PackError::Cancelled);
                }
                c = stream.next() => c,
            };
            match chunk {
                None => break,
                Some(Ok(bytes)) => {
                    if done + bytes.len() as u64 > f.size {
                        drop(file);
                        tokio::fs::remove_file(dest).await?;
                        return Err(PackError::SizeMismatch {
                            file: f.name.clone(),
                            got: done + bytes.len() as u64,
                            expected: f.size,
                        });
                    }
                    tokio::io::AsyncWriteExt::write_all(&mut file, &bytes).await?;
                    done += bytes.len() as u64;
                    on_bytes(done);
                }
                Some(Err(e)) => {
                    tracing::warn!(file = %f.name, "backend download interrupted: {}", e.without_url());
                    interrupted = true;
                    break;
                }
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut file).await?;
        file.sync_all().await?;
        if !interrupted && done == f.size {
            break;
        }
        if attempt == 5 {
            return Err(PackError::Network {
                file: f.name.clone(),
                message: "the connection kept dropping".into(),
            });
        }
        pause(attempt, cancel).await?;
    }
    // Verify size and hash.
    let got = tokio::fs::metadata(dest).await?.len();
    if got != f.size {
        return Err(PackError::SizeMismatch {
            file: f.name.clone(),
            got,
            expected: f.size,
        });
    }
    let path = dest.to_path_buf();
    let sha = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
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
        Ok(hex(&h.finalize()))
    })
    .await
    .map_err(|e| PackError::Io(e.to_string()))??;
    if !sha.eq_ignore_ascii_case(&f.sha256) {
        let _ = tokio::fs::remove_file(dest).await;
        return Err(PackError::ChecksumMismatch {
            file: f.name.clone(),
        });
    }
    Ok(())
}

/// Simple `*` glob over file names, ignoring case (the locks' `keep_files`).
pub(crate) fn glob_match(pattern: &str, name: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let n = name.to_ascii_lowercase();
    match p.split_once('*') {
        None => p == n,
        Some((a, b)) => n.len() >= a.len() + b.len() && n.starts_with(a) && n.ends_with(b),
    }
}

/// Unpacks `archive` into `out`, flattening one shared top-level folder, refusing entries
/// that would escape `out`, and keeping only `keep` names unless `keep_all`.
pub fn unpack_zip(
    archive: &Path,
    out: &Path,
    keep: &[String],
    keep_all: bool,
) -> Result<usize, PackError> {
    let name = archive
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let bad = |m: String| PackError::Archive {
        file: name.clone(),
        message: m,
    };
    let file = std::fs::File::open(archive)?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| bad(e.to_string()))?;
    // A single top-level folder shared by every entry is dropped.
    let names: Vec<PathBuf> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().and_then(|e| e.enclosed_name()))
        .collect();
    let first: Option<std::ffi::OsString> = names
        .first()
        .and_then(|p| p.components().next())
        .map(|c| c.as_os_str().to_owned());
    let strip = first.filter(|f| {
        names.len() > 1
            && names.iter().all(|p| {
                p.components().next().map(|c| c.as_os_str()) == Some(f.as_os_str())
                    && p.components().count() > 1
            })
    });
    let mut written = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).map_err(|e| bad(e.to_string()))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(bad(format!("unsafe path in archive: {:?}", entry.name())));
        };
        let rel = match &strip {
            Some(prefix) => rel
                .strip_prefix(prefix)
                .map(Path::to_path_buf)
                .unwrap_or(rel),
            None => rel,
        };
        if rel.as_os_str().is_empty() || entry.is_dir() {
            continue;
        }
        if entry.is_symlink() {
            continue;
        }
        let file_name = rel
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !keep_all && !keep.iter().any(|k| glob_match(k, &file_name)) {
            continue;
        }
        // Without keep_all the runtime is flat: llama.cpp loads DLLs from beside the exe.
        let target = if keep_all {
            out.join(&rel)
        } else {
            out.join(&file_name)
        };
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut f = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut f)?;
        f.flush()?;
        written += 1;
    }
    Ok(written)
}

/// What [`install_files`] downloads and unpacks: one pack's pinned archives, the names it
/// keeps, and where the Visual C++ runtime files it needs come from.
#[derive(Clone, Copy, Debug)]
pub struct InstallSpec<'a> {
    /// The pack's id, recorded in its `.complete` marker.
    pub id: &'a str,
    /// The build the pack comes from, recorded in its `.complete` marker.
    pub build: &'a str,
    /// The folder name under the root the pack is installed into.
    pub dir: &'a str,
    /// The archives to download, verify and unpack together.
    pub files: &'a [PackFile],
    /// File-name patterns kept when unpacking (`*` wildcards), unless
    /// [`keep_all`](Self::keep_all).
    pub keep: &'a [String],
    /// Keep every unpacked file, with its folders, instead of filtering by
    /// [`keep`](Self::keep).
    pub keep_all: bool,
    /// The Visual C++ runtime DLLs copied in from [`vc_runtime_dir`](Self::vc_runtime_dir).
    pub vc_runtime_files: &'a [String],
    /// The folder holding the Visual C++ runtime DLLs (llama.cpp's installed folder on
    /// Windows); `None` copies nothing.
    pub vc_runtime_dir: Option<&'a Path>,
}

/// Installs `pack` into `packs_dir`, copying the VC++ runtime DLLs from `vc_runtime_dir`
/// (the installed runtime folder on Windows). Resumes a partial download.
pub async fn install(
    lock: &LlamaLock,
    pack: &PackSpec,
    packs_dir: &Path,
    vc_runtime_dir: Option<&Path>,
    cancel: &CancellationToken,
    progress: &(dyn Fn(PackProgress) + Send + Sync),
) -> Result<PathBuf, PackError> {
    if pack.platform != crate::llama::lock::current_platform() {
        return Err(PackError::WrongPlatform);
    }
    let final_dir = packs_dir.join(&pack.dir);
    if pack.backend == crate::llama::backend::BackendKind::Rocm {
        let len = final_dir.to_string_lossy().chars().count();
        if len > ROCM_MAX_DIR_CHARS {
            return Err(PackError::PathTooLong(len));
        }
    }
    if final_dir.join(PACK_COMPLETE_MARKER).is_file() {
        return Ok(final_dir);
    }
    let spec = InstallSpec {
        id: &pack.id,
        build: &pack.build,
        dir: &pack.dir,
        files: &pack.files,
        keep: &lock.keep_files,
        keep_all: pack.keep_all,
        vc_runtime_files: &lock.vc_runtime_files,
        vc_runtime_dir,
    };
    install_files(&spec, packs_dir, cancel, progress).await
}

/// Downloads (resuming) into `<root>/.downloads`, verifies size and SHA-256, unpacks into
/// `<root>/.tmp-<dir>`, copies the VC++ files, writes `.complete`, renames to
/// `<root>/<dir>`. No platform check: the caller makes it.
///
/// A folder already at `<root>/<dir>` is replaced only once the new one is complete, so
/// whether to install at all is the caller's decision too.
pub async fn install_files(
    spec: &InstallSpec<'_>,
    root: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(PackProgress) + Send + Sync),
) -> Result<PathBuf, PackError> {
    let final_dir = root.join(spec.dir);
    let downloads = downloads_dir(root);
    tokio::fs::create_dir_all(&downloads).await?;
    let total: u64 = spec.files.iter().map(|f| f.size).sum();
    let client = client()?;
    let mut finished_before = 0u64;
    for f in spec.files {
        let dest = downloads.join(part_name(f));
        let base = finished_before;
        fetch(&client, f, &dest, ALLOWED_HOSTS, cancel, &|n| {
            progress(PackProgress {
                done: base + n,
                total,
                unpacking: false,
            })
        })
        .await?;
        finished_before += f.size;
    }
    progress(PackProgress {
        done: total,
        total,
        unpacking: true,
    });

    let tmp = root.join(format!(".tmp-{}", spec.dir));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await?;
    let keep = spec.keep.to_vec();
    let keep_all = spec.keep_all;
    let archives: Vec<PathBuf> = spec
        .files
        .iter()
        .map(|f| downloads.join(part_name(f)))
        .collect();
    let tmp2 = tmp.clone();
    let count = tokio::task::spawn_blocking(move || -> Result<usize, PackError> {
        let mut n = 0;
        for a in &archives {
            n += unpack_zip(a, &tmp2, &keep, keep_all)?;
        }
        Ok(n)
    })
    .await
    .map_err(|e| PackError::Io(e.to_string()))??;
    if count == 0 {
        return Err(PackError::Archive {
            file: spec.id.to_string(),
            message: "no files were kept".into(),
        });
    }
    if let Some(vc) = spec.vc_runtime_dir {
        for name in spec.vc_runtime_files {
            let src = vc.join(name);
            if src.is_file() {
                tokio::fs::copy(&src, tmp.join(name)).await?;
            } else {
                tracing::warn!(file = %name, "VC++ runtime file not found beside the installed runtime");
            }
        }
    }
    let marker = serde_json::json!({
        "pack": spec.id,
        "build": spec.build,
        "files": spec.files.iter().map(|f| &f.sha256).collect::<Vec<_>>(),
    });
    tokio::fs::write(tmp.join(PACK_COMPLETE_MARKER), marker.to_string()).await?;
    let _ = tokio::fs::remove_dir_all(&final_dir).await;
    tokio::fs::rename(&tmp, &final_dir).await?;
    for f in spec.files {
        let _ = tokio::fs::remove_file(downloads.join(part_name(f))).await;
    }
    tracing::info!(pack = %spec.id, "pack installed");
    Ok(final_dir)
}

/// A folder of this pack from an older build.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OlderBuild {
    /// The folder.
    pub dir: PathBuf,
    /// It was installed whole (`.complete` is in it): the person had this backend before the
    /// app moved to a newer build. A folder without the marker is an install that never ended.
    pub complete: bool,
}

/// The folders of `pack` from older builds. A pack's folder carries the build
/// (`cuda-b12110`), so an app update that moves the pin leaves the old folder on disk, where
/// discovery never looks: only the folder the lock names is a backend. These are the folders
/// named `<id>-<build>` next to it; the staging folder (`.tmp-…`) and the download folder
/// (`.downloads`) start with a dot and are not among them.
pub fn older_builds(pack: &PackSpec, packs_dir: &Path) -> Vec<OlderBuild> {
    let prefix = format!("{}-", pack.id);
    let Ok(entries) = std::fs::read_dir(packs_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let ours = name.starts_with(&prefix)
                && name != pack.dir
                && crate::llama::backend::is_plain_dir_name(&name)
                && entry.file_type().is_ok_and(|t| t.is_dir());
            ours.then(|| {
                let dir = entry.path();
                let complete = dir.join(PACK_COMPLETE_MARKER).is_file();
                OlderBuild { dir, complete }
            })
        })
        .collect()
}

/// Bytes the older builds of `pack` take on disk.
pub fn older_bytes(pack: &PackSpec, packs_dir: &Path) -> u64 {
    older_builds(pack, packs_dir)
        .iter()
        .map(|o| folder_bytes(&o.dir, 8))
        .sum()
}

/// The size of the files under `dir`, `depth` folders deep (a symbolic link is not followed).
fn folder_bytes(dir: &Path, depth: u32) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => folder_bytes(&entry.path(), depth - 1),
            Ok(t) if t.is_file() => entry.metadata().map_or(0, |m| m.len()),
            _ => 0,
        })
        .sum()
}

/// Removes the older builds of `pack`, and nothing else: the build that works stays.
pub async fn remove_older(pack: &PackSpec, packs_dir: &Path) -> Result<(), PackError> {
    for older in older_builds(pack, packs_dir) {
        tokio::fs::remove_dir_all(&older.dir).await?;
    }
    Ok(())
}

/// Removes an installed pack, its older builds ([`older_builds`]) and any partial download.
pub async fn remove(pack: &PackSpec, packs_dir: &Path) -> Result<(), PackError> {
    let dir = packs_dir.join(&pack.dir);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    remove_older(pack, packs_dir).await?;
    let _ = tokio::fs::remove_dir_all(packs_dir.join(format!(".tmp-{}", pack.dir))).await;
    for f in &pack.files {
        let _ = tokio::fs::remove_file(downloads_dir(packs_dir).join(part_name(f))).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn cancel_ends_the_wait_before_a_retry() {
        let cancel = CancellationToken::new();
        // Not cancelled: the wait runs its course (2 s for the first retry).
        let started = tokio::time::Instant::now();
        pause(0, &cancel).await.expect("waited");
        assert_eq!(started.elapsed(), Duration::from_secs(2));
        // Cancelled during the longest wait: it ends at once.
        let waiting = tokio::spawn({
            let cancel = cancel.clone();
            async move { pause(4, &cancel).await }
        });
        tokio::time::sleep(Duration::from_secs(1)).await;
        cancel.cancel();
        assert!(matches!(waiting.await.unwrap(), Err(PackError::Cancelled)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn hosts() {
        let ok = |u: &str| host_allowed(&reqwest::Url::parse(u).unwrap());
        assert!(ok(
            "https://github.com/ggml-org/llama.cpp/releases/download/b1/x.zip"
        ));
        assert!(ok("https://release-assets.githubusercontent.com/x"));
        assert!(!ok("http://github.com/x"));
        assert!(!ok("https://github.com.evil.com/x"));
        assert!(!ok("https://example.com/x"));
    }

    /// A caller's own host list allows exactly those hosts: not GitHub's, no subdomains,
    /// no look-alikes, https only.
    #[test]
    fn a_callers_host_list_allows_exactly_its_hosts() {
        let only = &["storage.googleapis.com"];
        let ok = |u: &str| host_allowed_in(&reqwest::Url::parse(u).unwrap(), only);
        assert!(ok("https://storage.googleapis.com/bucket/file.zip"));
        assert!(!ok("http://storage.googleapis.com/bucket/file.zip"));
        assert!(!ok("https://evil.storage.googleapis.com/x"));
        assert!(!ok("https://storage.googleapis.com.evil.example/x"));
        assert!(!ok("https://googleapis.com/x"));
        assert!(!ok("https://github.com/x"));
        assert!(!ok("https://storage.googleapis.com@evil.example/x"));
    }

    #[tokio::test]
    async fn a_pinned_file_on_another_host_is_refused_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        let f = PackFile {
            name: "x.zip".into(),
            url: "https://github.com/x/y/releases/download/v1/x.zip".into(),
            size: 1,
            sha256: "0".repeat(64),
        };
        let err = download_verified(
            &f,
            &dir.path().join("x.part"),
            &["storage.googleapis.com"],
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(
            matches!(&err, PackError::BlockedHost(h) if h == "github.com"),
            "{err}"
        );
        assert!(!dir.path().join("x.part").exists());
    }

    #[test]
    fn globs() {
        assert!(glob_match("ggml-cpu-*.dll", "ggml-cpu-haswell.dll"));
        assert!(glob_match("cudart64_*.dll", "cudart64_13.dll"));
        assert!(glob_match("llama-server.exe", "LLAMA-SERVER.EXE"));
        assert!(!glob_match("llama-server.exe", "llama-rpc-server.exe"));
        assert!(!glob_match("ggml-cpu-*.dll", "ggml-cuda.dll"));
    }

    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let f = std::fs::File::create(path).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, data) in entries {
            w.start_file(*name, opts).unwrap();
            w.write_all(data).unwrap();
        }
        w.finish().unwrap();
    }

    #[test]
    fn unpack_filters_and_flattens() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("a.zip");
        make_zip(
            &z,
            &[
                ("build/llama-server.exe", b"exe"),
                ("build/ggml-cuda.dll", b"dll"),
                ("build/llama-rpc-server.exe", b"no"),
                ("build/sub/cudart64_13.dll", b"rt"),
            ],
        );
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        let keep = vec![
            "llama-server.exe".to_string(),
            "ggml-cuda.dll".to_string(),
            "cudart64_*.dll".to_string(),
        ];
        assert_eq!(unpack_zip(&z, &out, &keep, false).unwrap(), 3);
        assert!(out.join("llama-server.exe").is_file());
        assert!(out.join("cudart64_13.dll").is_file());
        assert!(!out.join("llama-rpc-server.exe").exists());
    }

    #[test]
    fn unpack_keep_all_keeps_structure() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("r.zip");
        make_zip(
            &z,
            &[
                ("llama-server.exe", b"exe"),
                ("rocblas/library/k.dat", b"k"),
            ],
        );
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert_eq!(unpack_zip(&z, &out, &[], true).unwrap(), 2);
        assert!(out.join("rocblas/library/k.dat").is_file());
    }

    #[test]
    fn unpack_refuses_escaping_entries() {
        let dir = tempfile::tempdir().unwrap();
        let z = dir.path().join("evil.zip");
        make_zip(&z, &[("../escape.dll", b"x")]);
        let out = dir.path().join("out");
        std::fs::create_dir_all(&out).unwrap();
        assert!(unpack_zip(&z, &out, &["*".to_string()], true).is_err());
        assert!(!dir.path().join("escape.dll").exists());
    }

    /// The Windows Studio engine's archive, stand-in: one small file for each name of the
    /// pinned zip, already downloaded, and llama's installed folder holding the Visual C++
    /// runtime files.
    struct SdPackFixture {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        vc: PathBuf,
        files: Vec<PackFile>,
    }

    /// The fixture with the archive pinned at its true SHA-256, or at `pinned_sha256`.
    fn sd_pack_fixture(pinned_sha256: Option<&str>) -> SdPackFixture {
        let tmp = tempfile::tempdir().unwrap();
        let listing = include_str!("../tests/fixtures/sd_win_pack_listing.txt");
        let names: Vec<&str> = listing.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(names.len(), 23, "the pinned zip holds 23 files");
        let zip = tmp.path().join("pack.zip");
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (*n, n.as_bytes())).collect();
        make_zip(&zip, &entries);
        let bytes = std::fs::read(&zip).unwrap();
        let pinned = &crate::sd::lock::embedded().packs[0].files[0];
        let file = PackFile {
            name: pinned.name.clone(),
            url: pinned.url.clone(),
            size: bytes.len() as u64,
            sha256: pinned_sha256.map_or_else(|| hex(&Sha256::digest(&bytes)), str::to_string),
        };
        let root = tmp.path().join("engines");
        std::fs::create_dir_all(downloads_dir(&root)).unwrap();
        std::fs::write(downloads_dir(&root).join(part_name(&file)), &bytes).unwrap();
        let vc = tmp.path().join("llama").join("main");
        std::fs::create_dir_all(&vc).unwrap();
        for name in &crate::sd::lock::embedded().vc_runtime_files {
            std::fs::write(vc.join(name), name.as_bytes()).unwrap();
        }
        SdPackFixture {
            _tmp: tmp,
            root,
            vc,
            files: vec![file],
        }
    }

    fn sd_spec(f: &SdPackFixture) -> InstallSpec<'_> {
        let lock = crate::sd::lock::embedded();
        InstallSpec {
            id: "studio-vulkan",
            build: "master-1106-532d2c1",
            dir: "sd-1106",
            files: &f.files,
            keep: &lock.keep_files,
            keep_all: false,
            vc_runtime_files: &lock.vc_runtime_files,
            vc_runtime_dir: Some(&f.vc),
        }
    }

    #[tokio::test]
    async fn install_files_keeps_what_the_pinned_zip_needs() {
        let f = sd_pack_fixture(None);
        let root = f.root.as_path();
        let dir = install_files(&sd_spec(&f), root, &CancellationToken::new(), &|_| {})
            .await
            .unwrap();
        assert_eq!(dir, root.join("sd-1106"));
        for kept in [
            "sd-cli.exe",
            "stable-diffusion.dll",
            "ggml-vulkan.dll",
            "ggml-cpu-haswell.dll",
            "libwebp.dll",
            "libwebpmux.dll",
            "libsharpyuv.dll",
            "webm.dll",
            "ggml.txt",
            "stable-diffusion.cpp.txt",
            "msvcp140_codecvt_ids.dll",
            PACK_COMPLETE_MARKER,
        ] {
            assert!(dir.join(kept).is_file(), "{kept}");
        }
        for vc in &crate::sd::lock::embedded().vc_runtime_files {
            assert!(dir.join(vc).is_file(), "{vc}");
        }
        for left in ["sd-server.exe", "start-server.bat", "start-server.sh"] {
            assert!(!dir.join(left).exists(), "{left}");
        }
        assert!(!root.join(".tmp-sd-1106").exists());
        assert!(!downloads_dir(root).join(part_name(&f.files[0])).exists());
    }

    #[tokio::test]
    async fn a_changed_archive_installs_nothing() {
        let f = sd_pack_fixture(Some(&"0".repeat(64)));
        let root = f.root.as_path();
        let err = install_files(&sd_spec(&f), root, &CancellationToken::new(), &|_| {})
            .await
            .unwrap_err();
        assert!(matches!(err, PackError::ChecksumMismatch { .. }), "{err}");
        assert!(!root.join("sd-1106").exists());
    }

    #[test]
    fn state_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let lock = crate::llama::lock::embedded();
        let pack = &lock.packs[0];
        assert_eq!(state(pack, dir.path()), PackState::NotInstalled);
        std::fs::create_dir_all(downloads_dir(dir.path())).unwrap();
        std::fs::write(
            downloads_dir(dir.path()).join(part_name(&pack.files[0])),
            b"12345",
        )
        .unwrap();
        assert_eq!(state(pack, dir.path()), PackState::Partial { bytes: 5 });
        std::fs::create_dir_all(dir.path().join(&pack.dir)).unwrap();
        std::fs::write(dir.path().join(&pack.dir).join(PACK_COMPLETE_MARKER), b"{}").unwrap();
        assert_eq!(state(pack, dir.path()), PackState::Installed);
    }

    /// A pack's folder carries the build (`cuda-b12110`), so an app update that moves the
    /// pin leaves the old folder behind, unused: the lock names a new folder.
    fn leftover(root: &Path, name: &str, complete: bool, bytes: usize) {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("kernel.bin"), vec![0u8; bytes]).unwrap();
        if complete {
            std::fs::write(dir.join(PACK_COMPLETE_MARKER), b"{}").unwrap();
        }
    }

    #[test]
    fn a_folder_of_an_older_build_is_found_by_the_packs_id() {
        let dir = tempfile::tempdir().unwrap();
        let lock = crate::llama::lock::embedded();
        let cuda = lock.packs.iter().find(|p| p.id == "cuda").unwrap();
        assert!(older_builds(cuda, dir.path()).is_empty());

        leftover(dir.path(), "cuda-b12110", true, 700);
        leftover(dir.path(), "cuda-b11000", false, 5);
        // Not this pack's: another pack, the current build, the staging and download folders,
        // and a plain file that happens to start the same way.
        leftover(dir.path(), "rocm-b1372", true, 1000);
        leftover(dir.path(), &cuda.dir, true, 900);
        leftover(dir.path(), &format!(".tmp-{}", cuda.dir), false, 3);
        leftover(dir.path(), ".downloads", false, 3);
        leftover(dir.path(), "cudafoo", true, 3);
        std::fs::write(dir.path().join("cuda-notes.txt"), b"x").unwrap();

        let mut found: Vec<(String, bool)> = older_builds(cuda, dir.path())
            .into_iter()
            .map(|o| {
                (
                    o.dir.file_name().unwrap().to_string_lossy().into_owned(),
                    o.complete,
                )
            })
            .collect();
        found.sort();
        assert_eq!(
            found,
            vec![
                ("cuda-b11000".to_string(), false),
                ("cuda-b12110".to_string(), true)
            ]
        );
        // 700 + 5 of kernel files and the two bytes of the finished one's marker.
        assert_eq!(older_bytes(cuda, dir.path()), 707);
    }

    /// Removing a backend takes its older builds with it, so Settings can reclaim them, and
    /// "only the older" keeps the build that works.
    #[tokio::test]
    async fn removing_a_pack_takes_the_older_builds_too_or_only_them() {
        let dir = tempfile::tempdir().unwrap();
        let lock = crate::llama::lock::embedded();
        let cuda = lock.packs.iter().find(|p| p.id == "cuda").unwrap();
        leftover(dir.path(), "cuda-b12110", true, 10);
        leftover(dir.path(), &cuda.dir, true, 10);
        leftover(dir.path(), "rocm-b1372", true, 10);

        remove_older(cuda, dir.path()).await.unwrap();
        assert!(!dir.path().join("cuda-b12110").exists());
        assert!(
            dir.path().join(&cuda.dir).exists(),
            "the build that works stays"
        );
        assert!(
            dir.path().join("rocm-b1372").exists(),
            "another pack's folder stays"
        );

        leftover(dir.path(), "cuda-b12110", true, 10);
        remove(cuda, dir.path()).await.unwrap();
        assert!(!dir.path().join("cuda-b12110").exists());
        assert!(!dir.path().join(&cuda.dir).exists());
        assert!(dir.path().join("rocm-b1372").exists());
    }

    #[test]
    fn partial_bytes_adds_up_every_files_part() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, sha256: &str| PackFile {
            name: name.into(),
            url: format!("https://github.com/o/r/releases/download/t/{name}"),
            size: 100,
            sha256: sha256.into(),
        };
        let files = [
            file("a.zip", &"a".repeat(64)),
            file("b.zip", &"b".repeat(64)),
        ];
        assert_eq!(partial_bytes(&files, dir.path()), 0);
        std::fs::create_dir_all(dir.path().join(".downloads")).unwrap();
        let part = |f: &PackFile| dir.path().join(".downloads").join(part_name(f));
        std::fs::write(part(&files[0]), b"12345").unwrap();
        assert_eq!(partial_bytes(&files, dir.path()), 5);
        std::fs::write(part(&files[1]), b"123").unwrap();
        assert_eq!(partial_bytes(&files, dir.path()), 8);
        // only the files asked about count
        assert_eq!(partial_bytes(&files[1..], dir.path()), 3);
    }
}
