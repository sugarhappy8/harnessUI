//! llama-server's last stderr lines reach the start error on a loaded machine (task F3).
//!
//! This test target has `harness = false`, so this file's `main` runs for it. Run under the
//! file name `llama-server` (`llama-server.exe` on Windows) it *is* a llama-server that
//! fails to load: it writes [`NUMBERED_LINES`] numbered lines to stderr in bursts, then
//! [`LAST_LINE`], and exits with code 1. Started any other way (by `cargo test`) it copies
//! itself under that name into a temporary folder and starts the copy [`STARTS`] times
//! through [`runtime::start`], the app's start path, while a thread keeps a processor busy.
//! Every start must fail with [`StartError::Exited`] whose stderr tail ends with
//! [`LAST_LINE`]: the exit is reported only after both pipes were read to the end.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use runtime::{
    Backend, BackendKind, BackendSource, LaunchConfig, StartError, StartSpec, free_loopback_port,
    random_api_key, start,
};
use tokio::sync::mpsc::unbounded_channel;

/// The fake's file name, without the extension.
const FAKE_STEM: &str = "llama-server";
/// The line the fake writes last, after the numbered ones.
const LAST_LINE: &str = "fake llama-server: the last line";
/// Numbered lines before [`LAST_LINE`].
const NUMBERED_LINES: usize = 400;
/// Lines written in one go before the fake pauses.
const BURST: usize = 50;
/// The fake's pause between bursts.
const BURST_PAUSE: Duration = Duration::from_millis(5);
/// How many times the fake is started.
const STARTS: usize = 50;

fn main() {
    let me = std::env::current_exe().expect("this program's path");
    if me.file_stem().and_then(|stem| stem.to_str()) == Some(FAKE_STEM) {
        fake_llama_server();
    }
    if std::env::args().any(|arg| arg == "--list") {
        println!("llama_tail: test");
        return;
    }
    every_start_reports_the_last_stderr_line(&me);
    println!("llama_tail: every_start_reports_the_last_stderr_line ... ok");
}

/// What the copy does: numbered lines in bursts, the last line, exit code 1.
fn fake_llama_server() -> ! {
    use std::io::Write as _;

    let mut stderr = std::io::stderr().lock();
    for n in 1..=NUMBERED_LINES {
        let _ = writeln!(stderr, "fake llama-server: line {n} of {NUMBERED_LINES}");
        if n % BURST == 0 {
            let _ = stderr.flush();
            std::thread::sleep(BURST_PAUSE);
        }
    }
    let _ = writeln!(stderr, "{LAST_LINE}");
    let _ = stderr.flush();
    std::process::exit(1)
}

fn every_start_reports_the_last_stderr_line(me: &Path) {
    let dir = tempfile::tempdir().expect("tempdir");
    let server = dir
        .path()
        .join(FAKE_STEM)
        .with_extension(std::env::consts::EXE_EXTENSION);
    std::fs::copy(me, &server).expect("copy this program as llama-server");
    let backend = Backend {
        kind: BackendKind::Cpu,
        dir: dir.path().to_path_buf(),
        server,
        fit_params: None,
        build: "test".into(),
        source: BackendSource::DevOverride,
    };

    let busy = Arc::new(AtomicBool::new(true));
    let load = {
        let busy = busy.clone();
        std::thread::spawn(move || {
            let mut x = 0u64;
            while busy.load(Ordering::Relaxed) {
                x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1));
            }
        })
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        for attempt in 1..=STARTS {
            let (tx, _rx) = unbounded_channel();
            let spec = StartSpec {
                backend: backend.clone(),
                launch: LaunchConfig::new(
                    dir.path().join("model.gguf"),
                    2048,
                    free_loopback_port().expect("free port"),
                    random_api_key(),
                ),
                load_timeout: Duration::from_secs(60),
            };
            match start(spec, tx).await {
                Err(StartError::Exited { stderr_tail, .. }) => assert_eq!(
                    stderr_tail.last().map(String::as_str),
                    Some(LAST_LINE),
                    "start {attempt} of {STARTS}: the tail has {} lines",
                    stderr_tail.len()
                ),
                Err(other) => panic!("start {attempt} of {STARTS}: {other:?}"),
                Ok(server) => panic!("start {attempt} of {STARTS}: the fake loaded: {server:?}"),
            }
        }
    });

    busy.store(false, Ordering::Relaxed);
    load.join().expect("the busy thread");
}
