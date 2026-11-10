//! Resumable, verified file downloads from the Hub (ARCHITECTURE.md 11).
//!
//! - The partial file is `<dest>.part`, with `<dest>.part.json` beside it recording the
//!   published SHA-256, size and commit it belongs to. A resume continues only if they
//!   still match; otherwise the partial is discarded.
//! - Every attempt re-resolves the signed URL through the Hub, because it expires
//!   after an hour.
//! - Resume uses `Range` and asks for uncompressed bytes. A `206` must report the
//!   expected total; a `200` restarts from zero; a `416` means "already complete, verify".
//! - The finished file is hashed and compared before it is renamed into place.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::hf::{HfClient, HfError, validate_repo_id, validate_repo_path};

/// Attempts per file before giving up on network errors.
const MAX_ATTEMPTS: u32 = 6;

#[derive(Debug, thiserror::Error)]
pub enum DownloadError {
    #[error(transparent)]
    Hub(#[from] HfError),
    #[error("the download was cancelled")]
    Cancelled,
    #[error("the downloaded file does not match its published checksum")]
    ChecksumMismatch,
    #[error("the file changed on Hugging Face since it was listed; open the model again")]
    ChangedUpstream,
    #[error("the server sent an unexpected range")]
    BadRange,
    #[error("the disk is full")]
    DiskFull,
    #[error("could not write the file: {0}")]
    Io(String),
}

impl From<std::io::Error> for DownloadError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::StorageFull
            || e.raw_os_error() == Some(28)
            || e.raw_os_error() == Some(112)
        {
            DownloadError::DiskFull
        } else {
            DownloadError::Io(e.to_string())
        }
    }
}

/// One file to fetch.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileSpec {
    pub repo: String,
    /// Pinned commit.
    pub commit: String,
    /// Path inside the repository.
    pub path: String,
    pub size: u64,
    /// Published SHA-256 (lowercase hex). Downloads without one are flagged in the UI.
    pub sha256: Option<String>,
}

/// What the partial file belongs to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct PartMeta {
    repo: String,
    path: String,
    commit: String,
    size: u64,
    sha256: Option<String>,
}

/// `<models>/<owner>/<repo>/<file path>`, after validating every component.
pub fn destination(models_dir: &Path, repo: &str, file: &str) -> Result<PathBuf, HfError> {
    let (owner, name) = validate_repo_id(repo)?;
    let mut p = models_dir.join(owner).join(name);
    for seg in validate_repo_path(file)? {
        p.push(seg);
    }
    Ok(p)
}

fn part_paths(dest: &Path) -> (PathBuf, PathBuf) {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    let part = PathBuf::from(part);
    let mut meta = part.as_os_str().to_owned();
    meta.push(".json");
    (part, PathBuf::from(meta))
}

/// Bytes already on disk for a resumable download of `spec` (0 if none or stale).
pub fn resumable_bytes(dest: &Path, spec: &FileSpec) -> u64 {
    let (part, meta) = part_paths(dest);
    let Ok(text) = std::fs::read_to_string(&meta) else {
        return 0;
    };
    let Ok(m) = serde_json::from_str::<PartMeta>(&text) else {
        return 0;
    };
    if m != meta_of(spec) {
        return 0;
    }
    std::fs::metadata(part)
        .map(|m| m.len())
        .unwrap_or(0)
        .min(spec.size)
}

fn meta_of(spec: &FileSpec) -> PartMeta {
    PartMeta {
        repo: spec.repo.clone(),
        path: spec.path.clone(),
        commit: spec.commit.clone(),
        size: spec.size,
        sha256: spec.sha256.clone(),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SHA-256 of a file, in lowercase hex.
pub async fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = tokio::fs::File::open(path).await?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

/// Discards any partial download for `dest`.
pub async fn discard_partial(dest: &Path) {
    let (part, meta) = part_paths(dest);
    let _ = tokio::fs::remove_file(part).await;
    let _ = tokio::fs::remove_file(meta).await;
}

/// Downloads `spec` to `dest`, resuming a matching partial file. `progress` gets the
/// bytes on disk so far (call sites throttle it). Returns the verified SHA-256.
pub async fn download_file(
    client: &HfClient,
    spec: &FileSpec,
    dest: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<String, DownloadError> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let (part, meta_path) = part_paths(dest);
    let meta = meta_of(spec);
    if resumable_bytes(dest, spec) == 0 {
        discard_partial(dest).await;
    }
    tokio::fs::write(&meta_path, serde_json::to_vec(&meta).unwrap_or_default()).await?;

    let mut attempt = 0;
    loop {
        attempt += 1;
        match fetch_once(client, spec, &part, cancel, progress).await {
            Ok(()) => break,
            Err(DownloadError::Hub(HfError::Network(e))) if attempt < MAX_ATTEMPTS => {
                tracing::warn!(attempt, "download interrupted, retrying: {e}");
                let wait = Duration::from_secs(2u64.pow(attempt.min(5)));
                tokio::select! {
                    _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
                    _ = tokio::time::sleep(wait) => {}
                }
            }
            Err(e) => return Err(e),
        }
    }

    let actual = sha256_file(&part).await?;
    if spec
        .sha256
        .as_ref()
        .is_some_and(|expected| &actual != expected)
    {
        discard_partial(dest).await;
        return Err(DownloadError::ChecksumMismatch);
    }
    tokio::fs::rename(&part, dest).await?;
    let _ = tokio::fs::remove_file(&meta_path).await;
    Ok(actual)
}

async fn fetch_once(
    client: &HfClient,
    spec: &FileSpec,
    part: &Path,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64) + Send + Sync),
) -> Result<(), DownloadError> {
    let have = tokio::fs::metadata(part)
        .await
        .map(|m| m.len())
        .unwrap_or(0);
    if have > spec.size {
        tokio::fs::remove_file(part).await?;
        return Box::pin(fetch_once(client, spec, part, cancel, progress)).await;
    }
    if have == spec.size && spec.size > 0 {
        return Ok(());
    }
    let resolved = tokio::select! {
        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
        r = client.resolve(&spec.repo, &spec.commit, &spec.path) => r?,
    };
    if resolved.size.is_some_and(|s| s != spec.size)
        || (spec.sha256.is_some() && resolved.sha256.is_some() && resolved.sha256 != spec.sha256)
    {
        return Err(DownloadError::ChangedUpstream);
    }
    let mut req = client
        .request_for(&resolved)
        .header("Accept-Encoding", "identity");
    if have > 0 {
        req = req.header("Range", format!("bytes={have}-"));
    }
    let resp = tokio::select! {
        _ = cancel.cancelled() => return Err(DownloadError::Cancelled),
        r = req.send() => r.map_err(HfError::from)?,
    };
    let status = resp.status().as_u16();
    let mut start = have;
    match status {
        206 => {
            let range = resp
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range)
                .ok_or(DownloadError::BadRange)?;
            if range.0 != have || range.1 != spec.size {
                return Err(DownloadError::BadRange);
            }
        }
        200 => start = 0,
        416 if have > 0 => return Ok(()),
        _ => {
            let headers = resp.headers().clone();
            let code = headers
                .get("x-error-code")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            return Err(match (status, code) {
                (401, "GatedRepo") => HfError::NeedsToken,
                (403, "GatedRepo") => HfError::NeedsTerms,
                (403, _) | (410, _) => {
                    HfError::Network(format!("download link refused (HTTP {status})"))
                }
                _ => HfError::Http {
                    status,
                    message: "download failed".into(),
                },
            }
            .into());
        }
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(start == 0)
        .append(start > 0)
        .open(part)
        .await?;
    let mut done = start;
    progress(done);
    let mut stream = resp.bytes_stream();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => {
                file.flush().await?;
                return Err(DownloadError::Cancelled);
            }
            c = stream.next() => c,
        };
        let Some(chunk) = chunk else { break };
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                file.flush().await?;
                return Err(HfError::from(e).into());
            }
        };
        if done + chunk.len() as u64 > spec.size {
            drop(file);
            tokio::fs::remove_file(part).await?;
            return Err(DownloadError::BadRange);
        }
        file.write_all(&chunk).await?;
        done += chunk.len() as u64;
        progress(done);
    }
    file.flush().await?;
    file.sync_all().await?;
    if done != spec.size {
        return Err(
            HfError::Network("the connection closed before the file was complete".into()).into(),
        );
    }
    Ok(())
}

/// `bytes <start>-<end>/<total>` → (start, total).
fn parse_content_range(v: &str) -> Option<(u64, u64)> {
    let rest = v.trim().strip_prefix("bytes ")?;
    let (range, total) = rest.split_once('/')?;
    let (start, _end) = range.split_once('-')?;
    Some((start.trim().parse().ok()?, total.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::net::TcpListener;

    #[test]
    fn destination_is_inside_models_dir() {
        let root = Path::new("/models");
        let d = destination(
            root,
            "unsloth/Qwen3-8B-GGUF",
            "BF16/Qwen3-8B-BF16-00001-of-00002.gguf",
        )
        .unwrap();
        assert_eq!(
            d,
            Path::new("/models/unsloth/Qwen3-8B-GGUF/BF16/Qwen3-8B-BF16-00001-of-00002.gguf")
        );
        assert!(destination(root, "unsloth/x", "../../etc/passwd").is_err());
        assert!(destination(root, "../x", "a.gguf").is_err());
        assert!(destination(root, "a/b", "C:\\x.gguf").is_err());
    }

    #[test]
    fn content_range() {
        assert_eq!(parse_content_range("bytes 100-199/1000"), Some((100, 1000)));
        assert_eq!(parse_content_range("bytes */1000"), None);
        assert_eq!(parse_content_range("items 1-2/3"), None);
    }

    /// A tiny HTTP server: `/a/b/resolve/<commit>/<file>` redirects to `/cdn/file`,
    /// which serves `body` and honours `Range`. `cut_after` closes the first full
    /// response early to simulate a dropped connection.
    async fn server(
        body: Vec<u8>,
        sha: String,
        cut_after: Option<usize>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let body = Arc::new(body);
        let hits2 = hits.clone();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                let body = body.clone();
                let sha = sha.clone();
                let hits = hits2.clone();
                tokio::spawn(async move {
                    let (r, mut w) = sock.into_split();
                    let mut r = BufReader::new(r);
                    let mut line = String::new();
                    r.read_line(&mut line).await.unwrap();
                    let mut parts = line.split_whitespace();
                    let method = parts.next().unwrap_or("").to_string();
                    let path = parts.next().unwrap_or("").to_string();
                    let mut range = None;
                    loop {
                        let mut h = String::new();
                        if r.read_line(&mut h).await.unwrap() == 0 || h == "\r\n" {
                            break;
                        }
                        let lower = h.to_ascii_lowercase();
                        if let Some(v) = lower.strip_prefix("range: bytes=") {
                            range = v.trim().trim_end_matches('-').parse::<usize>().ok();
                        }
                    }
                    if path.contains("/resolve/") {
                        let resp = format!(
                            "HTTP/1.1 302 Found\r\nlocation: http://{addr}/cdn/file\r\nx-linked-size: {}\r\nx-linked-etag: \"{sha}\"\r\nx-repo-commit: c0ffee\r\ncontent-length: 0\r\n\r\n",
                            body.len()
                        );
                        w.write_all(resp.as_bytes()).await.unwrap();
                        return;
                    }
                    if method == "HEAD" {
                        let resp =
                            format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", body.len());
                        w.write_all(resp.as_bytes()).await.unwrap();
                        return;
                    }
                    let n = hits.fetch_add(1, Ordering::SeqCst);
                    let start = range.unwrap_or(0);
                    let slice = &body[start..];
                    let head = if range.is_some() {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\ncontent-range: bytes {start}-{}/{}\r\ncontent-length: {}\r\n\r\n",
                            body.len() - 1,
                            body.len(),
                            slice.len()
                        )
                    } else {
                        format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", slice.len())
                    };
                    w.write_all(head.as_bytes()).await.unwrap();
                    let send = match cut_after {
                        Some(c) if n == 0 => &slice[..c.min(slice.len())],
                        _ => slice,
                    };
                    let _ = w.write_all(send).await;
                    let _ = w.shutdown().await;
                });
            }
        });
        (format!("http://{addr}"), hits)
    }

    fn body(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i * 31 % 251) as u8).collect()
    }

    fn sha_of(b: &[u8]) -> String {
        hex(&Sha256::digest(b))
    }

    fn spec(b: &[u8], sha: Option<String>) -> FileSpec {
        FileSpec {
            repo: "a/b".into(),
            commit: "c0ffee".into(),
            path: "m-Q4_K_M.gguf".into(),
            size: b.len() as u64,
            sha256: sha,
        }
    }

    #[tokio::test]
    async fn downloads_and_verifies() {
        let b = body(300_000);
        let sha = sha_of(&b);
        let (url, _) = server(b.clone(), sha.clone(), None).await;
        let client = HfClient::for_test(&url);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a/b/m-Q4_K_M.gguf");
        let got = download_file(
            &client,
            &spec(&b, Some(sha.clone())),
            &dest,
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap();
        assert_eq!(got, sha);
        assert_eq!(std::fs::read(&dest).unwrap(), b);
        assert!(!part_paths(&dest).0.exists());
        assert!(!part_paths(&dest).1.exists());
    }

    #[tokio::test]
    async fn resumes_after_a_dropped_connection() {
        let b = body(500_000);
        let sha = sha_of(&b);
        let (url, hits) = server(b.clone(), sha.clone(), Some(123_456)).await;
        let client = HfClient::for_test(&url);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        let got = download_file(
            &client,
            &spec(&b, Some(sha.clone())),
            &dest,
            &CancellationToken::new(),
            &move |n| seen2.lock().unwrap().push(n),
        )
        .await
        .unwrap();
        assert_eq!(got, sha);
        assert_eq!(std::fs::read(&dest).unwrap(), b);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
        // The second request continued from the cut, not from zero.
        assert!(seen.lock().unwrap().contains(&123_456));
    }

    #[tokio::test]
    async fn checksum_mismatch_discards_the_file() {
        let b = body(10_000);
        let (url, _) = server(b.clone(), "00".repeat(32), None).await;
        let client = HfClient::for_test(&url);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        // The Hub advertises a hash that differs from what we listed → changed upstream.
        let err = download_file(
            &client,
            &spec(&b, Some(sha_of(&b))),
            &dest,
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::ChangedUpstream), "{err:?}");
        // The listing and the Hub agree, but the bytes don't → checksum mismatch.
        let (url, _) = server(b.clone(), "11".repeat(32), None).await;
        let client = HfClient::for_test(&url);
        let err = download_file(
            &client,
            &spec(&b, Some("11".repeat(32))),
            &dest,
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::ChecksumMismatch), "{err:?}");
        assert!(!dest.exists());
        assert!(!part_paths(&dest).0.exists());
    }

    #[tokio::test]
    async fn stale_partial_is_discarded() {
        let b = body(50_000);
        let sha = sha_of(&b);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let (part, meta) = part_paths(&dest);
        std::fs::write(&part, vec![0xAA; 20_000]).unwrap();
        let mut old = spec(&b, Some("ff".repeat(32)));
        old.commit = "old".into();
        std::fs::write(&meta, serde_json::to_vec(&meta_of(&old)).unwrap()).unwrap();
        assert_eq!(resumable_bytes(&dest, &spec(&b, Some(sha.clone()))), 0);
        let (url, _) = server(b.clone(), sha.clone(), None).await;
        let client = HfClient::for_test(&url);
        download_file(
            &client,
            &spec(&b, Some(sha)),
            &dest,
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b);
    }

    #[tokio::test]
    async fn cancel_keeps_the_partial_for_later() {
        let b = body(2_000_000);
        let sha = sha_of(&b);
        let (url, _) = server(b.clone(), sha.clone(), None).await;
        let client = HfClient::for_test(&url);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("m.gguf");
        let cancel = CancellationToken::new();
        let c2 = cancel.clone();
        let err = download_file(
            &client,
            &spec(&b, Some(sha.clone())),
            &dest,
            &cancel,
            &move |n| {
                if n > 0 {
                    c2.cancel();
                }
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, DownloadError::Cancelled));
        let have = resumable_bytes(&dest, &spec(&b, Some(sha.clone())));
        assert!(have > 0 && have < b.len() as u64, "{have}");
        download_file(
            &client,
            &spec(&b, Some(sha)),
            &dest,
            &CancellationToken::new(),
            &|_| {},
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b);
    }
}
