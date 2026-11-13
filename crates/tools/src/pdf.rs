//! PDF text extraction in a worker process (ARCHITECTURE.md 4 and 8.6). A crafted PDF
//! can hang the parser or overflow its stack (an abort, not a catchable panic), so
//! parsing runs in the app's own binary started with a hidden flag, under a wall-clock
//! timeout, with a memory cap the worker sets on itself before it reads the file.

use std::path::Path;
use std::time::Duration;

/// Hidden command-line flag that turns the app binary into the PDF worker.
pub const WORKER_FLAG: &str = "--internal-pdf-worker";
/// Largest PDF accepted.
pub const MAX_PDF_BYTES: u64 = 64 * 1024 * 1024;
/// Text returned at most.
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;
/// Memory the worker may use.
pub const MEMORY_CAP_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("the PDF is larger than {} MB", MAX_PDF_BYTES / (1024 * 1024))]
    TooLarge,
    #[error("reading the PDF took too long")]
    Timeout,
    #[error("the PDF could not be read (it may be damaged, encrypted or scanned images only)")]
    Unreadable,
    #[error("the PDF worker could not start: {0}")]
    Worker(String),
}

/// Caps the worker's own memory before it parses anything.
fn limit_memory() {
    limit_own_memory(MEMORY_CAP_BYTES);
}

/// Caps this process's own memory at `cap` bytes. [0.0.6] Also used by the app's picture
/// worker (`src-tauri/src/attachments.rs`), before it reads a WebP picture.
pub fn limit_own_memory(cap: u64) {
    #[cfg(unix)]
    {
        let lim = libc::rlimit {
            rlim_cur: cap as libc::rlim_t,
            rlim_max: cap as libc::rlim_t,
        };
        // SAFETY: setrlimit only reads the struct; failure is harmless (the timeout and
        // the process boundary still apply).
        // macOS ignores RLIMIT_AS for most allocations, so RLIMIT_DATA is set too.
        unsafe {
            libc::setrlimit(libc::RLIMIT_AS, &lim);
            libc::setrlimit(libc::RLIMIT_DATA, &lim);
        }
    }
    #[cfg(windows)]
    {
        use windows::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_PROCESS_MEMORY,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };
        use windows::Win32::System::Threading::GetCurrentProcess;
        // SAFETY: plain Win32 calls on our own process with a properly sized struct.
        // Nested jobs are allowed since Windows 8, so this works even though the app
        // already put the worker in its kill-on-close job.
        unsafe {
            if let Ok(job) = CreateJobObjectW(None, None) {
                let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_PROCESS_MEMORY;
                info.ProcessMemoryLimit = cap as usize;
                let _ = SetInformationJobObject(
                    job,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                let _ = AssignProcessToJobObject(job, GetCurrentProcess());
            }
        }
    }
}

/// The worker's entry point: extracts the text of the PDF at `path` and writes it to
/// stdout. Never returns. Exit code 0 on success, 2 on an unreadable file.
pub fn worker_main(path: &str) -> ! {
    use std::io::Write;
    limit_memory();
    let bytes = match std::fs::metadata(path) {
        Ok(m) if m.len() <= MAX_PDF_BYTES => std::fs::read(path).ok(),
        _ => None,
    };
    let Some(bytes) = bytes else {
        std::process::exit(2)
    };
    let text = match std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(&bytes)) {
        Ok(Ok(t)) => t,
        _ => std::process::exit(2),
    };
    let mut out = std::io::stdout().lock();
    let end = floor_char_boundary(&text, MAX_TEXT_BYTES);
    let _ = out.write_all(&text.as_bytes()[..end]);
    let _ = out.flush();
    std::process::exit(0)
}

fn floor_char_boundary(s: &str, max: usize) -> usize {
    if s.len() <= max {
        return s.len();
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Extracts a PDF's text by running `app_exe WORKER_FLAG <path>` with a timeout.
pub async fn extract_text(
    app_exe: &Path,
    pdf: &Path,
    timeout: Duration,
) -> Result<String, PdfError> {
    use tokio::io::AsyncReadExt;
    let size = std::fs::metadata(pdf)
        .map_err(|e| PdfError::Worker(e.to_string()))?
        .len();
    if size > MAX_PDF_BYTES {
        return Err(PdfError::TooLarge);
    }
    let cwd = pdf.parent().unwrap_or_else(|| Path::new("."));
    let spec = runtime::SpawnSpec::new(app_exe, cwd)
        .arg(WORKER_FLAG)
        .arg(pdf.as_os_str())
        .stdin(runtime::StdioMode::Null)
        .stdout(runtime::StdioMode::Piped)
        .stderr(runtime::StdioMode::Null);
    let mut child = runtime::spawn(spec).map_err(|e| PdfError::Worker(e.to_string()))?;
    let mut stdout = child
        .take_stdout()
        .ok_or_else(|| PdfError::Worker("no stdout".into()))?;
    let work = async {
        let mut buf = Vec::new();
        let _ = (&mut stdout)
            .take(MAX_TEXT_BYTES as u64 + 16)
            .read_to_end(&mut buf)
            .await;
        let status = child.wait().await;
        (buf, status)
    };
    match tokio::time::timeout(timeout, work).await {
        Ok((buf, Ok(status))) if status.success() => Ok(String::from_utf8_lossy(&buf).into_owned()),
        Ok(_) => Err(PdfError::Unreadable),
        Err(_) => {
            // Dropping the child kills its process tree (Job Object / process group).
            Err(PdfError::Timeout)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn char_boundary() {
        let s = "aé€";
        assert_eq!(floor_char_boundary(s, 2), 1);
        assert_eq!(floor_char_boundary(s, 100), s.len());
    }
}
