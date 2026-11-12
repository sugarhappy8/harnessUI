//! Server diagnostics (ARCHITECTURE.md 9.2 "Status and logs"): a stdio server's stderr is
//! drained continuously (a full pipe would block the server) into a bounded ring of recent
//! lines, shown next to any start-up error, and into a size-capped log file when the app
//! gives a log folder.
//!
//! The text is the server's own output: untrusted, shown to the user only, and never
//! written to the app's tracing log.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// Lines kept in memory per server.
pub const TAIL_LINES: usize = 50;

/// Longest line kept; longer lines are cut with a marker.
pub const MAX_LINE_BYTES: usize = 4096;

/// Size at which a server's log file is rotated (one previous file is kept).
pub const MAX_LOG_FILE_BYTES: u64 = 1024 * 1024;

/// Recent stderr lines of one server, and optionally its log file.
#[derive(Debug)]
pub(crate) struct StderrLog {
    lines: Mutex<VecDeque<String>>,
    file: Option<PathBuf>,
}

impl StderrLog {
    /// A log for server `name`, writing to `<dir>/mcp-server-<name>.log` when `dir` is set.
    pub(crate) fn new(name: &str, dir: Option<&Path>) -> Self {
        let file = dir.map(|dir| dir.join(format!("mcp-server-{}.log", file_stem(name))));
        Self {
            lines: Mutex::new(VecDeque::with_capacity(TAIL_LINES)),
            file,
        }
    }

    /// A log writing to exactly `file`, when set (the app's own managed server,
    /// `mcp-reverse.log`, which no user server's `mcp-server-<name>.log` can share).
    pub(crate) fn to_file(file: Option<PathBuf>) -> Self {
        Self {
            lines: Mutex::new(VecDeque::with_capacity(TAIL_LINES)),
            file,
        }
    }

    /// Appends one line to the ring (not to the file).
    pub(crate) fn push(&self, line: String) {
        let mut lines = self.lines.lock().unwrap_or_else(|e| e.into_inner());
        if lines.len() == TAIL_LINES {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The most recent lines, oldest first.
    pub(crate) fn tail(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    /// The last `n` lines joined, for error messages.
    pub(crate) fn tail_text(&self, n: usize) -> String {
        let tail = self.tail();
        let start = tail.len().saturating_sub(n);
        tail[start..].join("\n")
    }

    /// Notes an event (start, restart, exit) in the ring and the file.
    pub(crate) async fn note(&self, text: &str) {
        let line = format!("--- {text} ---");
        self.push(line.clone());
        if let Some(path) = &self.file {
            append(path, &[line]).await;
        }
    }

    /// Reads `reader` to the end, line by line, into the ring and the file.
    pub(crate) async fn drain<R: AsyncRead + Unpin>(&self, mut reader: R) {
        let mut buf = vec![0u8; 8192];
        let mut pending: Vec<u8> = Vec::new();
        let mut truncated = false;
        loop {
            let n = match reader.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut finished: Vec<String> = Vec::new();
            for &byte in &buf[..n] {
                if byte == b'\n' {
                    finished.push(finish_line(&mut pending, truncated));
                    truncated = false;
                } else if pending.len() < MAX_LINE_BYTES {
                    pending.push(byte);
                } else {
                    truncated = true;
                }
            }
            self.record(finished).await;
        }
        if !pending.is_empty() {
            let last = finish_line(&mut pending, truncated);
            self.record(vec![last]).await;
        }
    }

    async fn record(&self, lines: Vec<String>) {
        if lines.is_empty() {
            return;
        }
        for line in &lines {
            self.push(line.clone());
        }
        if let Some(path) = &self.file {
            append(path, &lines).await;
        }
    }
}

fn finish_line(pending: &mut Vec<u8>, truncated: bool) -> String {
    let mut line = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    if line.ends_with('\r') {
        line.pop();
    }
    if truncated {
        line.push_str(" […]");
    }
    line
}

/// A file-name-safe version of a server name: lowercase ASCII letters, digits, `-` and
/// `_`. When that changed the name, a short hash of the real name is appended, so two
/// servers never share a log file: not "my server" and "my_server", and not "GitHub" and
/// "github" on the case-insensitive file systems of Windows and macOS.
fn file_stem(name: &str) -> String {
    let stem: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    if stem == name && !stem.is_empty() {
        return stem;
    }
    let digest = Sha256::digest(name.as_bytes());
    let hash: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    if stem.is_empty() {
        format!("server-{hash}")
    } else {
        format!("{stem}-{hash}")
    }
}

/// Appends lines to the log file, rotating it to `<file>.1` once it passes the cap.
/// Failures are ignored: the log is a diagnostic aid and must never stop a server.
async fn append(path: &Path, lines: &[String]) {
    if let Some(dir) = path.parent() {
        let _ = tokio::fs::create_dir_all(dir).await;
    }
    if let Ok(meta) = tokio::fs::metadata(path).await
        && meta.len() >= MAX_LOG_FILE_BYTES
    {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        let _ = tokio::fs::rename(path, PathBuf::from(rotated)).await;
    }
    let Ok(mut file) = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
    else {
        return;
    };
    let mut text = String::new();
    for line in lines {
        text.push_str(line);
        text.push('\n');
    }
    // tokio hands writes to a blocking thread; flush so they are done before the handle
    // goes and before the next append checks the size.
    if file.write_all(text.as_bytes()).await.is_ok() {
        let _ = file.flush().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn keeps_the_last_lines_and_cuts_long_ones() {
        let log = StderrLog::new("srv", None);
        let mut input = String::new();
        for i in 0..(TAIL_LINES + 10) {
            input.push_str(&format!("line {i}\r\n"));
        }
        input.push_str(&"x".repeat(MAX_LINE_BYTES + 100));
        input.push('\n');
        input.push_str("no newline at end");
        log.drain(input.as_bytes()).await;
        let tail = log.tail();
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail[0], "line 12");
        assert!(tail[TAIL_LINES - 2].ends_with(" […]"));
        assert!(tail[TAIL_LINES - 2].len() < MAX_LINE_BYTES + 10);
        assert_eq!(tail[TAIL_LINES - 1], "no newline at end");
        assert_eq!(log.tail_text(1), "no newline at end");
    }

    #[test]
    fn servers_never_share_a_log_file() {
        assert_eq!(file_stem("github"), "github");
        assert_eq!(file_stem("my-server_2"), "my-server_2");
        let names = [
            "GitHub",
            "github",
            "my server",
            "my_server",
            "my/server",
            "",
            "ü",
        ];
        let stems: std::collections::BTreeSet<String> =
            names.iter().map(|n| file_stem(n)).collect();
        assert_eq!(stems.len(), names.len(), "{stems:?}");
        for stem in &stems {
            assert!(
                stem.bytes().all(|b| b.is_ascii_lowercase()
                    || b.is_ascii_digit()
                    || b == b'-'
                    || b == b'_'),
                "{stem}"
            );
        }
        assert!(file_stem("GitHub").starts_with("github-"));
    }

    #[tokio::test]
    async fn invalid_utf8_is_replaced() {
        let log = StderrLog::new("srv", None);
        log.drain(&b"bad \xff byte\n"[..]).await;
        assert_eq!(log.tail(), vec!["bad \u{FFFD} byte".to_owned()]);
    }

    #[tokio::test]
    async fn writes_and_rotates_the_log_file() {
        let dir = tempfile::tempdir().unwrap();
        let log = StderrLog::new("my server/1", Some(dir.path()));
        log.note("starting").await;
        log.drain(&b"hello\nworld\n"[..]).await;
        let path = dir
            .path()
            .join(format!("mcp-server-{}.log", file_stem("my server/1")));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "--- starting ---\nhello\nworld\n");
        // Past the cap the file is moved aside and a new one started.
        std::fs::write(&path, vec![b'x'; MAX_LOG_FILE_BYTES as usize]).unwrap();
        log.drain(&b"after\n"[..]).await;
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
        let rotated = PathBuf::from(format!("{}.1", path.display()));
        assert_eq!(
            std::fs::metadata(rotated).unwrap().len(),
            MAX_LOG_FILE_BYTES
        );
    }
}
