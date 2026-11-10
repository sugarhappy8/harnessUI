// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The same binary is the macOS crash-safety reaper when started with the hidden
    // flag (ARCHITECTURE.md 4.1). It never opens a window.
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some(app_lib::REAPER_FLAG) {
        let parent = args.next().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
        runtime::reaper_main(parent);
    }
    app_lib::run();
}
