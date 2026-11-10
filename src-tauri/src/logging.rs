//! Local log files (ARCHITECTURE.md 15). Nothing is sent anywhere. Logs never
//! contain message text, API keys or file contents; commands log what happened,
//! not what was said.

use std::sync::OnceLock;

use tauri::{AppHandle, Manager};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Keeps the background log writer alive for the life of the process.
static GUARD: OnceLock<tracing_appender::non_blocking::WorkerGuard> = OnceLock::new();

/// Number of daily log files kept.
const KEEP_FILES: usize = 7;

pub fn init(app: &AppHandle) {
    let filter = if cfg!(debug_assertions) {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"))
    } else {
        EnvFilter::new("info")
    };
    // process-wrap records the whole Command, environment included (the llama-server
    // key, MCP secrets), in debug-level spans. Never let those through.
    let filter = match "process_wrap=info".parse() {
        Ok(directive) => filter.add_directive(directive),
        Err(_) => filter,
    };
    let file_layer = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let appender = tracing_appender::rolling::Builder::new()
            .rotation(tracing_appender::rolling::Rotation::DAILY)
            .filename_prefix("app")
            .filename_suffix("log")
            .max_log_files(KEEP_FILES)
            .build(&dir)
            .ok()?;
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let _ = GUARD.set(guard);
        Some(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(writer),
        )
    });
    let stderr_layer = cfg!(debug_assertions)
        .then(|| tracing_subscriber::fmt::layer().with_writer(std::io::stderr));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(stderr_layer)
        .try_init();
}
