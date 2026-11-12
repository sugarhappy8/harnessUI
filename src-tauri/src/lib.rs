//! The app crate: Tauri setup, windows, commands.
//!
//! Everything with logic worth testing lives in the workspace crates under `crates/`.
//! This crate wires them to Tauri. See ARCHITECTURE.md section 5.

mod approvals;
mod artifacts;
mod attachments;
mod browser_cmd;
mod chat;
mod children;
mod commands;
mod computer_cmd;
mod context_cmd;
mod diagnostics;
mod downloads;
mod dto;
mod error;
mod evals;
mod git_cmd;
mod gpu;
mod hotkey;
mod hub;
mod load_settings;
mod load_tracker;
mod logging;
mod mcp_cmd;
mod mentions;
mod models_cmd;
mod navigation;
mod outputs;
mod outside_agent;
mod overlay;
mod packs_cmd;
mod permissions;
mod pinned;
mod providers;
mod runtime_manager;
mod search_cmd;
mod settings;
mod share;
mod skills_cmd;
mod state;
mod subagent;
mod task;
mod tool_catalog;
mod tool_select;
mod toolset;
mod turns;
mod updates;

use std::sync::Arc;

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
    let context = tauri::generate_context!();
    let with_updater = updates::configured(context.config());
    let mut builder = tauri::Builder::default()
        // Must be registered first: a second launch focuses the existing window
        // instead of starting a second llama-server and a second database writer.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            match app.get_webview_window(MAIN_WINDOW) {
                Some(window) => {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                // A copy without its main window is on its way out; make sure it goes, so
                // the next start is a fresh one.
                None => app.exit(0),
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        // Artifact previews load from this scheme, into a sandboxed frame (artifacts.rs).
        .register_asynchronous_uri_scheme_protocol(artifacts::SCHEME, artifacts::serve)
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
            mentions::search_mentions,
            mentions::resolve_mentions,
            models_cmd::runtime_status,
            models_cmd::list_backends,
            models_cmd::load_local_model,
            models_cmd::unload_model,
            models_cmd::list_model_options,
            models_cmd::get_local_model_settings,
            models_cmd::set_local_model_settings,
            models_cmd::run_diagnostics,
            logging::open_logs_folder,
            providers::list_providers,
            providers::save_provider,
            providers::delete_provider,
            providers::test_provider,
            search_cmd::get_search_settings,
            search_cmd::set_search_settings,
            search_cmd::set_search_key,
            search_cmd::clear_search_key,
            search_cmd::test_search,
            search_cmd::list_sources,
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
            hotkey::stop_hotkey_status,
            hotkey::set_stop_hotkey,
            permissions::list_approved_folders,
            permissions::approve_folder,
            permissions::remove_approved_folder,
            permissions::set_chat_working_folder,
            permissions::list_audit,
            permissions::export_audit,
            turns::resolve_approval,
            approvals::get_approval_mode,
            approvals::set_approval_mode,
            approvals::get_allow_all_default,
            approvals::set_allow_all_default,
            mcp_cmd::list_mcp_servers,
            mcp_cmd::preview_mcp_config,
            mcp_cmd::add_mcp_servers,
            mcp_cmd::remove_mcp_server,
            mcp_cmd::set_mcp_server_enabled,
            mcp_cmd::restart_mcp_server,
            mcp_cmd::list_chat_tools,
            mcp_cmd::list_server_tools,
            mcp_cmd::set_chat_thinking,
            mcp_cmd::set_chat_tools,
            mcp_cmd::set_mcp_tool_pref,
            mcp_cmd::clear_chat_grants,
            mcp_cmd::mcp_server_log,
            tool_select::get_tool_selection,
            tool_select::set_tool_selection,
            tool_select::tool_catalog_problems,
            skills_cmd::list_skills,
            skills_cmd::import_skill,
            skills_cmd::set_skill_enabled,
            skills_cmd::delete_skill,
            skills_cmd::get_skill_source,
            skills_cmd::validate_skill,
            skills_cmd::save_skill,
            skills_cmd::set_shared_skills,
            skills_cmd::open_skills_folder,
            skills_cmd::loaded_skills,
            skills_cmd::unload_skill,
            updates::update_status,
            updates::set_update_auto_check,
            updates::check_for_update,
            updates::install_update,
            overlay::overlay_stop,
            overlay::minimize_main_window,
            toolset::get_terminal_pass_through,
            toolset::set_terminal_pass_through,
            browser_cmd::browser_status,
            browser_cmd::set_browser_settings,
            browser_cmd::choose_browser_path,
            browser_cmd::download_agent_browser,
            browser_cmd::cancel_agent_browser_download,
            browser_cmd::remove_agent_browser,
            computer_cmd::computer_status,
            computer_cmd::set_computer_profile,
            git_cmd::git_info,
            git_cmd::set_git_token,
            git_cmd::remove_git_token,
            git_cmd::set_git_identity,
            artifacts::list_artifacts,
            artifacts::get_artifact,
            artifacts::artifact_frame,
            artifacts::allow_artifact_scripts,
            artifacts::save_artifact_version,
            artifacts::rename_artifact,
            artifacts::delete_artifact,
            artifacts::export_artifact,
            artifacts::report_artifact_render,
            artifacts::artifact_isolation,
            artifacts::begin_artifact_check,
            artifacts::finish_artifact_check,
            artifacts::artifact_canary,
            outputs::get_tool_output,
            outputs::tool_output_for_call,
            pinned::list_pins,
            pinned::add_pin,
            pinned::edit_pin,
            pinned::remove_pin,
            pinned::move_pin,
            pinned::set_pin_scope,
            pinned::pin_message,
            context_cmd::inspect_context,
            task::get_task,
            task::edit_task,
            task::set_task_status,
            task::clear_task,
            evals::commands::get_model_profile,
            evals::commands::forget_model_profile,
            evals::commands::start_capability_check,
            evals::commands::cancel_eval,
            evals::commands::list_bench_tasks,
            evals::commands::start_benchmark,
            evals::commands::list_bench_runs,
            evals::commands::get_bench_run,
            models_cmd::model_load_info,
            models_cmd::set_model_load_settings,
            models_cmd::reset_model_load_settings,
            models_cmd::reload_local_model,
            models_cmd::cancel_pending_reload,
            models_cmd::model_busy,
            hub::catalog_list,
            hub::compare_facts,
            share::share_status,
            share::set_share_enabled,
            share::set_share_port,
            share::reveal_share_key,
            share::new_share_key,
            share::share_setup,
            outside_agent::coding_apps_status,
            outside_agent::find_coding_apps,
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
            let skills = Arc::new(tools::SkillsServer::new());
            let pass_through: Vec<String> =
                tauri::async_runtime::block_on(store.get_setting(toolset::TERMINAL_PASS_THROUGH))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
            let identity: git_cmd::GitIdentity =
                tauri::async_runtime::block_on(store.get_setting(git_cmd::IDENTITY))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
            let host = tauri::async_runtime::block_on(toolset::build_host(
                &handle,
                secrets.clone(),
                skills.clone(),
                pass_through,
                identity,
            ));
            let browser = tauri::async_runtime::block_on(browser_cmd::build(&handle, &store));
            host.add_builtin(browser.clone());
            app.manage(browser_cmd::BrowserHandle(browser));
            let computer = computer_cmd::build(&handle);
            if let Some(server) = &computer {
                host.add_builtin(server.clone());
            }
            app.manage(computer_cmd::ComputerHandle(computer));
            app.manage(state::AppState {
                store,
                secrets,
                runtime,
                generation: state::Generation::default(),
                host: host.clone(),
                skills,
                grants: Default::default(),
                model_busy: Default::default(),
            });
            app.manage(turns::PendingApprovals::default());
            // 0.0.5: sharing the loaded model with other apps; listens again only if the
            // user left it on.
            app.manage(share::Sharing::default());
            share::start_saved(&handle);
            // 0.0.5: the coding apps found when the user last turned that group on.
            {
                let handle = handle.clone();
                tauri::async_runtime::spawn(
                    async move { outside_agent::register_saved(&handle).await },
                );
            }
            // "Allow all in every chat", if the user switched it on (a saved setting).
            approvals::load_default(&handle);
            // Before the startup check below, which records what it finds here.
            app.manage(updates::LastFound::default());
            toolset::forward_events(handle.clone(), host);
            {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move {
                    toolset::reload_skills(&handle).await;
                    toolset::start_saved_servers(&handle).await;
                    updates::auto_check(handle.clone()).await;
                });
            }
            app.manage(packs_cmd::PackDownloads::default());
            app.manage(hotkey::Hotkey::default());
            {
                let handle = handle.clone();
                tauri::async_runtime::spawn(async move { hotkey::init(&handle).await });
            }
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

    if with_updater {
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }
    let app = match builder.build(context) {
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
        // Closing the main window quits the app. Without this the process lived on,
        // invisibly: the hidden border and Stop bar of computer use are windows too, and
        // the app only ends by itself when its last window is gone. Starting the app again
        // then reached that invisible copy (single instance) and nothing appeared.
        if let RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::Destroyed,
            ..
        } = &event
            && label == MAIN_WINDOW
        {
            handle.exit(0);
        }
        if let RunEvent::Exit = event {
            // Stop llama-server cleanly; the Job Object / reaper is the backstop.
            if let Some(state) = handle.try_state::<state::AppState>() {
                let runtime = state.runtime.clone();
                let host = state.host.clone();
                let browser = handle
                    .try_state::<browser_cmd::BrowserHandle>()
                    .map(|b| b.0.clone());
                if let Some(c) = handle.try_state::<computer_cmd::ComputerHandle>()
                    && let Some(server) = c.0.clone()
                {
                    server.shutdown();
                }
                tauri::async_runtime::block_on(async move {
                    if let Some(b) = browser {
                        b.shutdown().await;
                    }
                    host.shutdown().await;
                    runtime.shutdown().await;
                });
            }
            tracing::info!("stopped");
        }
    });
}

/// Builds the windows declared in `tauri.conf.json` (they have `"create": false`)
/// so navigation and pop-up handlers can be attached. Navigation away from the app's
/// own origin is blocked, and so is `window.open` (ARCHITECTURE.md 13.2).
/// The page background (`--bg` in src/styles.css) for the system theme, as the colour of
/// the main window before its page has painted.
fn startup_background(dark: bool) -> (u8, u8, u8) {
    if dark {
        (0x0e, 0x0f, 0x13)
    } else {
        (0xfb, 0xfb, 0xfd)
    }
}

fn create_windows(app: &mut tauri::App) -> tauri::Result<()> {
    let product_name = app
        .config()
        .product_name
        .clone()
        .unwrap_or_else(|| "App".to_string());
    let window_configs = app.config().app.windows.clone();
    let dev = cfg!(debug_assertions);

    for config in &window_configs {
        // A frame inside the main window may load an artifact preview (artifacts.rs).
        let previews = config.label == MAIN_WINDOW;
        let mut builder = WebviewWindowBuilder::from_config(app.handle(), config)?
            .on_navigation(move |url| {
                navigation::is_allowed(url, dev)
                    || (previews && artifacts::is_frame_navigation(url))
            })
            .on_new_window(|_url, _features| NewWindowResponse::Deny);
        if config.label == MAIN_WINDOW {
            // Created hidden and shown below with the theme's background: a window is
            // white until its page has painted, which is a flash on a dark system.
            builder = builder.title(&product_name).visible(false);
        }
        #[cfg(windows)]
        {
            builder = builder.additional_browser_args(WEBVIEW2_ARGS);
        }
        let window = builder.build()?;
        if config.label == MAIN_WINDOW {
            let dark = matches!(window.theme(), Ok(tauri::Theme::Dark));
            let (r, g, b) = startup_background(dark);
            let _ = window.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
            // Shown whatever the calls above returned: a window that stays hidden is
            // worse than a flash.
            window.show()?;
        }
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

#[cfg(test)]
mod tests {
    use super::startup_background;

    /// The window's first colour must be the page's, or the start still flashes.
    #[test]
    fn the_startup_background_is_the_page_background() {
        let css = include_str!("../../src/styles.css");
        let hex = |(r, g, b): (u8, u8, u8)| format!("--bg: #{r:02x}{g:02x}{b:02x};");
        let light = hex(startup_background(false));
        let dark = hex(startup_background(true));
        // Light is declared once (:root); dark twice (system setting, and chosen).
        assert_eq!(css.matches(&light).count(), 1, "{light}");
        assert_eq!(css.matches(&dark).count(), 2, "{dark}");
    }
}
