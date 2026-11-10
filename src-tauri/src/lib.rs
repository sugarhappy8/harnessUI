//! The app crate: Tauri setup, windows, commands.
//!
//! Everything with logic worth testing lives in the workspace crates under `crates/`.
//! This crate wires them to Tauri. See ARCHITECTURE.md section 5.

mod attachments;
mod chat;
mod children;
mod commands;
mod diagnostics;
mod downloads;
mod dto;
mod error;
mod hub;
mod logging;
mod models_cmd;
mod navigation;
mod packs_cmd;
mod providers;
mod runtime_manager;
mod settings;
mod state;

use std::sync::{Arc, Mutex};

use tauri::webview::NewWindowResponse;
use tauri::{Manager, RunEvent, WebviewWindowBuilder};

pub use children::REAPER_FLAG;

/// Window labels. The capability files bind permissions to these labels.
pub const MAIN_WINDOW: &str = "main";
pub const OVERLAY_WINDOW: &str = "overlay";

/// WebView2 switches. The first is wry's default (setting any arguments replaces it):
/// no Office UI, no PDF UI, no SmartScreen lookups. The others ask the runtime not to
/// run its crash reporter, so a renderer crash (whose dump holds chat text) is not
/// uploaded (ARCHITECTURE.md 13.6). wry 0.57 does not expose WebView2's
/// `IsCustomCrashReportingEnabled` option, which is the documented switch; see the
/// M0 notes in CHANGELOG.md.
#[cfg(windows)]
const WEBVIEW2_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --disable-breakpad --disable-crash-reporter";

/// File name of the app database inside the app data folder.
const DATABASE_FILE: &str = "app.db";

/// Starts the app. Called from `main.rs`.
pub fn run() {
    runtime::spawn::init_parent_error_mode();
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
        .invoke_handler(tauri::generate_handler![
            commands::app_info,
            chat::list_chats,
            chat::create_chat,
            chat::get_chat,
            chat::rename_chat,
            chat::delete_chat,
            chat::set_chat_model,
            chat::set_system_prompt,
            chat::switch_branch,
            chat::send_message,
            chat::regenerate,
            chat::stop_generation,
            attachments::get_attachment,
            models_cmd::runtime_status,
            models_cmd::list_backends,
            models_cmd::load_local_model,
            models_cmd::unload_model,
            models_cmd::list_model_options,
            models_cmd::get_local_model_settings,
            models_cmd::set_local_model_settings,
            models_cmd::run_diagnostics,
            providers::list_providers,
            providers::save_provider,
            providers::delete_provider,
            providers::test_provider,
            models_cmd::get_setting,
            models_cmd::set_setting,
            hub::hardware_info,
            hub::hub_search,
            hub::hub_search_next,
            hub::hub_repo,
            hub::hub_token_status,
            hub::hub_set_token,
            hub::hub_clear_token,
            hub::get_hf_endpoint,
            hub::set_hf_endpoint,
            hub::download_start,
            hub::download_catalog_entry,
            hub::download_pause,
            hub::download_resume,
            hub::download_cancel,
            hub::list_downloads,
            hub::list_installed_models,
            hub::import_model,
            hub::delete_model,
            hub::disk_usage,
            hub::set_models_dir,
            hub::first_run_info,
            hub::complete_first_run,
            hub::refresh_catalog,
            packs_cmd::list_packs,
            packs_cmd::download_pack,
            packs_cmd::cancel_pack_download,
            packs_cmd::remove_pack,
        ])
        .setup(|app| {
            let handle = app.handle().clone();
            logging::init(&handle);
            tracing::info!(version = %app.package_info().version, "starting");
            // The reaper client needs a Tokio context.
            let children =
                tauri::async_runtime::block_on(async { children::Children::init(&handle) });
            app.manage(children);

            // The local (non-roaming) folder: SQLite's WAL mode does not work on the
            // network shares roaming profiles are often redirected to (ARCHITECTURE.md 12).
            let data_dir = app.path().app_local_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join(DATABASE_FILE);
            let store =
                tauri::async_runtime::block_on(store::Store::open(&db_path)).map_err(|e| {
                    tracing::error!("could not open the database: {e}");
                    Box::new(e) as Box<dyn std::error::Error>
                })?;
            let secrets: Arc<dyn store::secrets::SecretStore> =
                Arc::from(store::secrets::platform_default(&app.config().identifier));
            let runtime = Arc::new(runtime_manager::RuntimeManager::new(
                handle.clone(),
                runtime_manager::layout(&handle),
            ));
            app.manage(state::AppState {
                store,
                secrets,
                runtime,
                generation: Mutex::new(None),
            });
            app.manage(packs_cmd::PackDownloads::default());
            let downloads = Arc::new(downloads::DownloadManager::default());
            app.manage(downloads.clone());
            {
                let handle = handle.clone();
                tauri::async_runtime::spawn(
                    async move { downloads.resume_interrupted(&handle).await },
                );
            }
            create_windows(app)?;
            Ok(())
        });

    let app = match builder.build(tauri::generate_context!()) {
        Ok(app) => app,
        Err(err) => {
            // There is no window to show an error in at this point; write it where a
            // support request can find it and exit with a failure code.
            tracing::error!("the app failed to start: {err}");
            eprintln!("the app failed to start: {err}");
            std::process::exit(1);
        }
    };
    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            // Stop llama-server cleanly; the Job Object / reaper is the backstop.
            if let Some(state) = handle.try_state::<state::AppState>() {
                let runtime = state.runtime.clone();
                tauri::async_runtime::block_on(runtime.shutdown());
            }
            tracing::info!("stopped");
        }
    });
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
        #[cfg(windows)]
        {
            builder = builder.additional_browser_args(WEBVIEW2_ARGS);
        }
        let window = builder.build()?;
        // Click-through has no config key; it is a runtime call. On Linux (development
        // only) GTK has no native window until the overlay is first shown, so the call is
        // made when M7 shows it.
        #[cfg(not(target_os = "linux"))]
        if config.label == OVERLAY_WINDOW {
            window.set_ignore_cursor_events(true)?;
        }
        #[cfg(target_os = "linux")]
        let _ = &window;
    }
    Ok(())
}
