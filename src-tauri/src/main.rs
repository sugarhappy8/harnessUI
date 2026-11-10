// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same binary is the macOS crash-safety reaper when started with the hidden
    // flag (ARCHITECTURE.md 4.1). It never opens a window.
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some(flag) if flag == app_lib::REAPER_FLAG => {
            let parent = args.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
            runtime::reaper_main(parent);
        }
        // PDF parsing runs in this binary as a short-lived worker (ARCHITECTURE.md 8.6).
        Some(flag) if flag == tools::pdf::WORKER_FLAG => {
            tools::pdf::worker_main(&args.next().unwrap_or_default());
        }
        // libgit2 network operations (clone, pull, push) run in this binary as a
        // short-lived worker so Stop can kill them (ARCHITECTURE.md 4, 9.3).
        Some(flag) if flag == tools::git::WORKER_FLAG => {
            tools::git::worker_main();
        }
        _ => {}
    }
    app_lib::run();
}
