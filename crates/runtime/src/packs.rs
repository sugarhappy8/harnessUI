//! Downloadable backends (ARCHITECTURE.md 7.1, decisions D1, D2 and D4): CUDA and ROCm
//! are one-click downloads in Settings, pinned by the compiled-in lock file and verified
//! file by file before use.
//!
//! A pack is downloaded into `<packs>/.downloads/` (resumable), every archive is checked
//! against its SHA-256 and size, unpacked into `<packs>/.tmp-<dir>/`, the VC++ runtime
//! DLLs are copied in, a `.complete` marker is written, and only then is the folder
//! renamed to `<packs>/<dir>`. Downloads come from GitHub release assets only.

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
    /// A redirect left GitHub's hosts.
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

fn downloads_dir(packs_dir: &Path) -> PathBuf {
    packs_dir.join(".downloads")
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
    let bytes: u64 = pack
        .files
        .iter()
        .filter_map(|f| std::fs::metadata(downloads_dir(packs_dir).join(part_name(f))).ok())
        .map(|m| m.len())
        .sum();
    if bytes > 0 {
        PackState::Partial { bytes }
    } else {
        PackState::NotInstalled
    }
}

/// Partial files are named after their hash so a changed pin never resumes stale bytes.
fn part_name(f: &PackFile) -> String {
    format!("{}.{}.part", f.name, &f.sha256[..16.min(f.sha256.len())])
}

fn host_allowed(url: &reqwest::Url) -> bool {
    url.scheme() == "https" && url.host_str().is_some_and(|h| ALLOWED_HOSTS.contains(&h))
}

fn client() -> Result<reqwest::Client, PackError> {
    reqwest::Client::builder()
        .user_agent(concat!("app-backend-download/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(20))
        .read_timeout(Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() > 5 {
                attempt.error("too many redirects")
            } else if host_allowed(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .build()
        .map_err(|e| PackError::Io(e.to_string()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Downloads one archive with resume; returns once it is complete and verified.
async fn fetch(
    client: &reqwest::Client,
    f: &PackFile,
    dest: &Path,
    cancel: &CancellationToken,
    on_bytes: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), PackError> {
    let url = reqwest::Url::parse(&f.url).map_err(|_| PackError::BlockedHost(f.url.clone()))?;
    if !host_allowed(&url) {
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
                tokio::time::sleep(Duration::from_secs(2u64.pow(attempt + 1))).await;
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
        tokio::time::sleep(Duration::from_secs(2u64.pow(attempt + 1))).await;
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

/// Simple `*` glob over file names (the lock's `keep_files`).
fn glob_match(pattern: &str, name: &str) -> bool {
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
    let downloads = downloads_dir(packs_dir);
    tokio::fs::create_dir_all(&downloads).await?;
    let total = pack.download_size();
    let client = client()?;
    let mut finished_before = 0u64;
    for f in &pack.files {
        let dest = downloads.join(part_name(f));
        let base = finished_before;
        fetch(&client, f, &dest, cancel, &|n| {
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

    let tmp = packs_dir.join(format!(".tmp-{}", pack.dir));
    let _ = tokio::fs::remove_dir_all(&tmp).await;
    tokio::fs::create_dir_all(&tmp).await?;
    let keep = lock.keep_files.clone();
    let keep_all = pack.keep_all;
    let archives: Vec<PathBuf> = pack
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
            file: pack.id.clone(),
            message: "no files were kept".into(),
        });
    }
    if let Some(vc) = vc_runtime_dir {
        for name in &lock.vc_runtime_files {
            let src = vc.join(name);
            if src.is_file() {
                tokio::fs::copy(&src, tmp.join(name)).await?;
            } else {
                tracing::warn!(file = %name, "VC++ runtime file not found beside the installed runtime");
            }
        }
    }
    let marker = serde_json::json!({
        "pack": pack.id,
        "build": pack.build,
        "files": pack.files.iter().map(|f| &f.sha256).collect::<Vec<_>>(),
    });
    tokio::fs::write(tmp.join(PACK_COMPLETE_MARKER), marker.to_string()).await?;
    let _ = tokio::fs::remove_dir_all(&final_dir).await;
    tokio::fs::rename(&tmp, &final_dir).await?;
    for f in &pack.files {
        let _ = tokio::fs::remove_file(downloads.join(part_name(f))).await;
    }
    tracing::info!(pack = %pack.id, "backend installed");
    Ok(final_dir)
}

/// Removes an installed pack and any partial download.
pub async fn remove(pack: &PackSpec, packs_dir: &Path) -> Result<(), PackError> {
    let dir = packs_dir.join(&pack.dir);
    if dir.exists() {
        tokio::fs::remove_dir_all(&dir).await?;
    }
    let _ = tokio::fs::remove_dir_all(packs_dir.join(format!(".tmp-{}", pack.dir))).await;
    for f in &pack.files {
        let _ = tokio::fs::remove_file(downloads_dir(packs_dir).join(part_name(f))).await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
