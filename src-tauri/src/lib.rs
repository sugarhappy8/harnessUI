//! The app crate: Tauri setup, windows, commands.
//!
//! Everything with logic worth testing lives in the workspace crates under `crates/`.
//! This crate wires them to Tauri. See ARCHITECTURE.md section 5.

mod commands;
mod navigation;

use tauri::webview::NewWindowResponse;
use tauri::{Manager, WebviewWindowBuilder};

/// Window labels. The capability files bind permissions to these labels.
pub const MAIN_WINDOW: &str = "main";
pub const OVERLAY_WINDOW: &str = "overlay";

/// Starts the app. Called from `main.rs`.
pub fn run() {
    let builder = tauri::Builder::default()
        // Must be registered first: a second launch focuses the existing window
        // instead of starting a second llama-server and a second database writer.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .invoke_handler(tauri::generate_handler![commands::app_info])
        .setup(|app| {
            create_windows(app)?;
            Ok(())
        });

    if let Err(err) = builder.run(tauri::generate_context!()) {
        // There is no window to show an error in at this point; write it where a
        // support request can find it and exit with a failure code.
        eprintln!("the app failed to start: {err}");
        std::process::exit(1);
    }
}

/// Builds the windows declared in `tauri.conf.json` (they have `"create": false`)
/// so navigation and pop-up handlers can be attached. Navigation away from the app's
/// own origin is blocked, and so is `window.open` (ARCHITECTURE.md 13.2).
fn create_windows(app: &mut tauri::App) -> tauri::Result<()> {
    let product_name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| "App".to_string());
    let window_configs = app.config().app.windows.clone();
    let dev = cfg!(debug_assertions);

    for config in &window_configs {
        let mut builder = WebviewWindowBuilder::from_config(app.handle(), config)?
            .on_navigation(move |url| navigation::is_allowed(url, dev))
            .on_new_window(|_url, _features| NewWindowResponse::Deny);
        if config.label == MAIN_WINDOW {
            builder = builder.title(&product_name);
        }
        let window = builder.build()?;
        if config.label == OVERLAY_WINDOW {
            // Click-through has no config key; it is a runtime call.
            window.set_ignore_cursor_events(true)?;
        }
    }
    Ok(())
}
