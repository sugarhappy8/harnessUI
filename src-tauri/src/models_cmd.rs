//! Commands for the local runtime: backends, loading a model, model options,
//! settings, diagnostics.

use std::path::PathBuf;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};
use tokio_util::sync::CancellationToken;

use crate::dto::{
    BackendDto, DiagnosticsReportDto, LocalModelSettings, ModelBusy, ModelOptionDto,
    RuntimeStatusDto,
};
use crate::error::{AppError, AppResult};
use crate::load_settings::{self as load, ModelLoadInfo, ModelLoadSettings, Plan};
use crate::settings;
use crate::state::{AppState, BusyKind};

#[tauri::command]
pub async fn runtime_status(state: State<'_, AppState>) -> AppResult<RuntimeStatusDto> {
    Ok(state.runtime.status())
}

#[tauri::command]
pub async fn list_backends(
    state: State<'_, AppState>,
    reprobe: bool,
) -> AppResult<Vec<BackendDto>> {
    Ok(state.runtime.backend_dtos(reprobe).await)
}

/// A local model file the user picked: a `.gguf` file.
fn gguf_path(path: &str) -> AppResult<PathBuf> {
    let model = PathBuf::from(path);
    if model
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("gguf")
    {
        return Err(AppError::bad_request("Pick a .gguf model file."));
    }
    Ok(model)
}

/// A context size a command was given: one of [`load::CTX_RANGE`] (`-c 0` would mean the
/// trained length, which can be far more than this computer has).
fn check_ctx(ctx: u32) -> AppResult<()> {
    if load::CTX_RANGE.contains(&ctx) {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "The context size must be between 512 and 1,048,576 tokens.",
        ))
    }
}

/// A model file that must be there (the settings panel's commands).
fn existing_model(path: &str) -> AppResult<PathBuf> {
    let model = gguf_path(path)?;
    if !model.is_file() {
        return Err(AppError::new(
            "model_missing",
            "The model file could not be found.",
            "Pick the file again; it may have been moved or deleted.",
            path.to_string(),
        ));
    }
    Ok(model)
}

/// Starts the load of `plan` and, once the model is ready, records whether it can think
/// and starts the speed check.
async fn run_load(state: &AppState, model: PathBuf, plan: &Plan) -> AppResult<()> {
    state
        .runtime
        .clone()
        .load(model, plan.resolved.clone(), plan.footprint())
        .await?;
    state.runtime.set_thinking(plan.thinking());
    // Settings saved while the model loaded could not mark it then (only a ready model is
    // compared): compare what this load used with the settings as saved now.
    if plan.saved {
        refresh_settings_changed(state).await;
    }
    let checker = state.runtime.clone();
    tauri::async_runtime::spawn(async move { checker.speed_check().await });
    Ok(())
}

/// After any settings change: whether the loaded model runs with older settings than the
/// saved ones ("Settings changed. Restart the model to use them.").
async fn refresh_settings_changed(state: &AppState) {
    let Some(path) = state.runtime.status().model_path else {
        return;
    };
    let Some(loaded) = state.runtime.loaded_choices(&path) else {
        return;
    };
    let now = load::plan(state, &path, None).await;
    state
        .runtime
        .set_settings_changed(!loaded.same_launch(&now.resolved));
}

/// Loads a local model. `settings` absent (0.0.5) means the model's own saved settings
/// (the resolver: its overrides, the defaults for all models, the projector, the trained
/// length, Automatic); the 0.0.4 shape `{ctx, backend, device}` still works and is used as
/// given. Picking the loaded model again with the same choices is a no-op; picking the
/// model being loaded with the same choices joins that load. `when_idle` (D3): `true`
/// waits until nothing uses the model (cancellable, ending with `load_replaced`); absent
/// or `false` loads now and stops the reply or model check that uses the model first.
#[tauri::command]
pub async fn load_local_model(
    state: State<'_, AppState>,
    path: String,
    settings: Option<LocalModelSettings>,
    when_idle: Option<bool>,
) -> AppResult<()> {
    let model = gguf_path(&path)?;
    if let Some(s) = &settings {
        check_ctx(s.ctx)?;
    }
    settings::remember_local_model(&state, &path).await;
    let plan = load::plan(&state, &path, settings.clone()).await;
    // A newer load cancels a change that waits, even when nothing needs to change.
    if state
        .runtime
        .loaded_choices(&path)
        .is_some_and(|loaded| loaded.same_launch(&plan.resolved))
    {
        state.runtime.cancel_waiting();
        return Ok(());
    }
    if let Some(result) = state.runtime.join_load(&model, &plan.resolved).await {
        state.runtime.cancel_waiting();
        return result;
    }
    let waited = state
        .runtime
        .before_change(&state.model_busy, when_idle.unwrap_or(false))
        .await?;
    // After a wait the settings may have changed meanwhile: use them as they are now.
    let plan = if waited {
        load::plan(&state, &path, settings).await
    } else {
        plan
    };
    run_load(&state, model, &plan).await
}

/// Unloads the model ("Unload to free memory"), and remembers it in `unloaded_path` for
/// "Unloaded to free memory · Load". `when_idle` (D3) as for `load_local_model`.
#[tauri::command]
pub async fn unload_model(state: State<'_, AppState>, when_idle: Option<bool>) -> AppResult<()> {
    state
        .runtime
        .before_change(&state.model_busy, when_idle.unwrap_or(false))
        .await?;
    state.runtime.unload_by_user().await;
    Ok(())
}

// ---- 0.0.5 per-model load settings and reload (WP1b) --------------------------------

/// Everything the settings panel shows about one model file. With `overrides` (the
/// panel's unsaved choices, checked like `set_model_load_settings` checks them), the facts
/// are worked out as if they were saved, so the fit marks follow the draft; nothing is
/// saved and nothing restarts. Without them, the model's saved settings.
#[tauri::command]
pub async fn model_load_info(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    overrides: Option<ModelLoadSettings>,
) -> AppResult<ModelLoadInfo> {
    crate::commands::only_main_window(window.label())?;
    let model = existing_model(&path)?;
    match overrides {
        None => {
            // The panel opened (item 6, I6-14): a measurement that failed or took too
            // long for this file may be tried once more.
            state.offload.forget_failures(&model);
            info_for(&state, &path).await
        }
        Some(draft) => {
            let plan = load::plan_draft(&state, &path, draft).await;
            let facts = plan_facts(&plan, &path)?;
            load::validate_for(&plan.overrides, facts, &plan.machine)
                .map_err(AppError::bad_request)?;
            Ok(info_from(&state, &path, &plan, facts))
        }
    }
}

/// "Where the model goes" with llama.cpp's own figures for the unsaved choices `overrides`
/// (0.0.6 item 6, 4.2 step 6; asked while Fine control is open): the model file must be
/// there, the choices are checked as `set_model_load_settings` checks them, nothing is
/// saved and nothing loads. One measurement runs at a time (a newer request cancels the
/// older one; the same choices join the run going), results are cached, and a failure
/// gives the instant estimate with the reason. The answer's `key` is the draft's, as in
/// `model_load_info`'s picture.
#[tauri::command]
pub async fn offload_measure(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    overrides: ModelLoadSettings,
) -> AppResult<crate::offload::OffloadPreview> {
    crate::commands::only_main_window(window.label())?;
    existing_model(&path)?;
    let plan = load::plan_draft(&state, &path, overrides).await;
    let facts = plan_facts(&plan, &path)?;
    load::validate_for(&plan.overrides, facts, &plan.machine).map_err(AppError::bad_request)?;
    let found = state.runtime.backends(false).await;
    Ok(crate::offload::measured(&state.offload, &path, &plan, facts, &found).await)
}

/// The model file's facts in `plan`, or "could not be found".
fn plan_facts<'a>(plan: &'a Plan, path: &str) -> AppResult<&'a load::ModelFacts> {
    plan.facts.as_ref().ok_or_else(|| {
        AppError::new(
            "model_missing",
            "The model file could not be found.",
            "Pick the file again; it may have been moved or deleted.",
            path.to_string(),
        )
    })
}

/// The panel's facts for `path` as saved now.
async fn info_for(state: &AppState, path: &str) -> AppResult<ModelLoadInfo> {
    let plan = load::plan(state, path, None).await;
    let facts = plan_facts(&plan, path)?;
    Ok(info_from(state, path, &plan, facts))
}

fn info_from(state: &AppState, path: &str, plan: &Plan, facts: &load::ModelFacts) -> ModelLoadInfo {
    let loaded = state.runtime.loaded_choices(path);
    load::build_info(
        path,
        facts,
        plan.overrides.clone(),
        plan.defaults.clone(),
        plan.resolved.clone(),
        &plan.machine,
        loaded.as_ref(),
    )
}

/// Saves one model's overrides (validated against the fixed lists, the trained length and
/// this computer's graphics cards) and returns the model's info with them. All "Same as
/// default" deletes the saved overrides. Nothing restarts: a loaded model shows
/// "Settings changed" until Apply and restart.
#[tauri::command]
pub async fn set_model_load_settings(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
    overrides: ModelLoadSettings,
) -> AppResult<ModelLoadInfo> {
    crate::commands::only_main_window(window.label())?;
    existing_model(&path)?;
    let plan = load::plan(&state, &path, None).await;
    let Some(facts) = plan.facts.as_ref() else {
        return Err(AppError::not_found("model file"));
    };
    // The device list is the backend's, which these overrides do not change.
    load::validate_for(&overrides, facts, &plan.machine).map_err(AppError::bad_request)?;
    load::save_overrides(&state.store, &facts.file_name, facts.size_bytes, &overrides).await?;
    refresh_settings_changed(&state).await;
    info_for(&state, &path).await
}

/// "Reset to defaults": deletes the model's overrides and returns its info.
#[tauri::command]
pub async fn reset_model_load_settings(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ModelLoadInfo> {
    crate::commands::only_main_window(window.label())?;
    let model = existing_model(&path)?;
    if let Some((name, size)) = load::file_identity(&model) {
        load::save_overrides(&state.store, &name, size, &ModelLoadSettings::default()).await?;
    }
    refresh_settings_changed(&state).await;
    info_for(&state, &path).await
}

/// Restarts the loaded model with its current resolved settings (Reload, Apply and
/// restart); also the model whose load failed. `force`: restart even when the loaded copy
/// already uses the current settings (Reload; it also retries the chosen backend after a
/// step-down); without it, nothing happens when nothing changed. `when_idle`: wait until
/// nothing uses the model (the busy counter is zero), with `reload_waiting` set on the
/// status, cancellable with `cancel_pending_reload`; without it, restart now: the reply or
/// model check that uses the model is stopped first (D3 "Stop and restart").
///
/// The same `when_idle` rules apply to `load_local_model` and `unload_model`. At most one
/// change waits at a time: a newer load, unload or reload (waiting or not) cancels the one
/// that waits. The call returns when the change is over; a wait that was cancelled ends
/// with the error code `load_replaced` (the UI shows nothing for it) and changes nothing.
#[tauri::command]
pub async fn reload_local_model(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    force: bool,
    when_idle: bool,
) -> AppResult<()> {
    crate::commands::only_main_window(window.label())?;
    let Some(path) = state.runtime.status().model_path else {
        return Err(AppError::new(
            "model_not_loaded",
            "No model is loaded.",
            "Pick a model from the list at the top of a chat.",
            String::new(),
        ));
    };
    let model = PathBuf::from(&path);
    let plan = load::plan(&state, &path, None).await;
    if !force
        && state
            .runtime
            .loaded_choices(&path)
            .is_some_and(|loaded| loaded.same_launch(&plan.resolved))
    {
        state.runtime.cancel_waiting();
        return Ok(());
    }
    let waited = state
        .runtime
        .before_change(&state.model_busy, when_idle)
        .await?;
    let plan = if waited {
        load::plan(&state, &path, None).await
    } else {
        plan
    };
    run_load(&state, model, &plan).await
}

/// Cancels the change that waits for the model to be free ("Restarts when the reply ends
/// · Cancel"): a reload, a load of another model or an unload called with `when_idle`. The
/// waiting call then ends with `load_replaced`. Nothing waiting is not an error.
#[tauri::command]
pub async fn cancel_pending_reload(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<()> {
    crate::commands::only_main_window(window.label())?;
    state.runtime.cancel_waiting();
    Ok(())
}

/// What is using the model now, for the question of D3.
#[tauri::command]
pub async fn model_busy(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<ModelBusy> {
    crate::commands::only_main_window(window.label())?;
    Ok(state.model_busy.snapshot())
}

/// The defaults for all models, with Automatic for a fresh install and the 0.0.4 default
/// (D2), and the one-time upgrade note on the first start after the update.
#[tauri::command]
pub async fn get_local_model_settings(state: State<'_, AppState>) -> AppResult<LocalModelSettings> {
    Ok(load::settings_page_defaults(&state.store).await)
}

#[tauri::command]
pub async fn set_local_model_settings(
    state: State<'_, AppState>,
    mut settings: LocalModelSettings,
) -> AppResult<()> {
    settings.mmproj = None;
    // A response-only field (the one-time upgrade note); never stored.
    settings.upgrade_note = None;
    check_ctx(settings.ctx)?;
    state
        .store
        .set_setting(settings::LOCAL_MODEL, &settings)
        .await?;
    refresh_settings_changed(&state).await;
    Ok(())
}

/// Model picker entries (0.0.6 item 8; see `model_options`): the local files used
/// recently, then cached endpoint models, then every other installed model by name. The
/// picker shows the local ones as "On this computer" and the endpoints' as "Endpoints".
/// Each file is listed once, with a folder hint when two files share a name, and the model
/// loaded now under the spelling it was loaded with. `chat_model` is the model reference of
/// the chat that asks (none for the Welcome card or a new chat): its own model file is
/// listed the way it names it (a link, a mapped drive), and no other chat sees that
/// spelling. Makes no network request.
#[tauri::command]
pub async fn list_model_options(
    state: State<'_, AppState>,
    chat_model: Option<String>,
) -> AppResult<Vec<crate::model_options::ModelOptionEntry>> {
    let status = state.runtime.status();
    let loaded = matches!(
        status.state,
        crate::dto::RuntimeStateDto::Ready | crate::dto::RuntimeStateDto::Starting
    )
    .then_some(status.model_path)
    .flatten();
    let chat = chat_model.as_deref().and_then(|r| r.strip_prefix("local:"));
    let local = crate::model_options::local_options(&state.store, loaded.as_deref(), chat).await?;
    let mut endpoints = Vec::new();
    for p in state.store.list_providers().await? {
        let cached: Vec<ModelOptionDto> = state
            .store
            .get_setting(&settings::provider_models_key(p.id))
            .await?
            .unwrap_or_default();
        endpoints.extend(
            cached
                .into_iter()
                .map(crate::model_options::ModelOptionEntry::plain),
        );
    }
    Ok(crate::model_options::arrange(local, endpoints))
}

#[tauri::command]
pub async fn get_setting(
    state: State<'_, AppState>,
    key: String,
) -> AppResult<Option<serde_json::Value>> {
    Ok(state.store.get_setting(&key).await?)
}

/// The app's environment for a diagnostic probe: every variable except the ones that
/// steer llama.cpp or look like secrets, which never reach a child (4.1).
fn full_environment(names: impl Iterator<Item = String>) -> runtime::EnvSpec {
    // A crash never starts a debugger (item 6, I6-8); the app's own GGML_* are left out.
    let mut env = runtime::EnvSpec::minimal().set(runtime::llama::launch::NO_BACKTRACE_ENV, "1");
    for name in names {
        let upper = name.to_ascii_uppercase();
        let steers = ["LLAMA_", "GGML_", "HF_", "HUGGING"]
            .iter()
            .any(|p| upper.starts_with(p));
        let secret = ["TOKEN", "KEY", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL"]
            .iter()
            .any(|p| upper.contains(p));
        if !steers && !secret {
            env = env.inherit(name);
        }
    }
    env
}

/// The keys the generic `set_setting` may write: plain UI preferences only. Anything with
/// a policy meaning has its own command (ARCHITECTURE.md 13.5); model settings (the
/// defaults for all models and each model's own) are never on it (D12).
const ALLOWED: &[&str] = &[
    "theme",
    settings::DEFAULT_SYSTEM_PROMPT,
    settings::REMOTE_CONTEXT_LENGTH,
    settings::STEPS_PER_TURN,
    // [0.0.6] Closing the note about renamed MCP servers (only ever cleared this way).
    crate::mcp_cmd::RENAMED_SERVERS,
];

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> AppResult<()> {
    if !ALLOWED.contains(&key.as_str()) {
        return Err(AppError::bad_request(
            "That setting cannot be changed this way.",
        ));
    }
    if key == crate::mcp_cmd::RENAMED_SERVERS && value.as_array().is_none_or(|a| !a.is_empty()) {
        return Err(AppError::bad_request(
            "That setting cannot be changed this way.",
        ));
    }
    if key == settings::STEPS_PER_TURN && settings::parse_steps_per_turn(&value).is_none() {
        return Err(AppError::bad_request(
            "The step limit must be a whole number from 5 to 500.",
        ));
    }
    if key == settings::REMOTE_CONTEXT_LENGTH && settings::parse_remote_context(&value).is_none() {
        return Err(AppError::bad_request(
            "The context size must be a whole number of tokens from 1,024 to 1,048,576.",
        ));
    }
    state.store.set_setting(&key, &value).await?;
    Ok(())
}

/// The backend check: every backend's devices, and with a model, a timed short
/// generation on each usable backend. The report is saved to the logs folder.
/// `when_idle` is the answer to the D3 question, as for a reload: the timed generation
/// replaces the loaded model, so with it the check waits until nothing uses the model
/// (a cancel of this wait ends with `load_replaced` and no report); without it,
/// what uses the model is stopped. With a model, that wait comes first, before the
/// backends are probed, and the check counts as a model check from then on. With
/// `when_idle` it waits again right before its first test load, for what began to use the
/// model while it probed (a reply, another app's request, another model check); a cancel
/// of that wait still gives the report of the devices, with nothing replaced.
#[tauri::command]
pub async fn run_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    model_path: Option<String>,
    when_idle: bool,
) -> AppResult<DiagnosticsReportDto> {
    use std::fmt::Write;
    // The timed generation replaces the loaded model (D3): as the user answered, the check
    // waits until nothing uses the model, or stops what does, before anything else (the
    // backends are probed first, up to 45 s each), so the waiting line and its Cancel are
    // true from the first moment. A change waiting for the model is replaced, as by a
    // reload. From then until what was there is back, the check is a model check: a change
    // of what is loaded asks first, then waits for the check or stops it.
    let timed = match model_path.filter(|p| std::path::Path::new(p).is_file()) {
        Some(path) => {
            state
                .runtime
                .before_change(&state.model_busy, when_idle)
                .await?;
            let previous = state.runtime.status();
            let stop = CancellationToken::new();
            let check = state
                .runtime
                .hold(&state.model_busy, BusyKind::ModelCheck, stop.clone());
            Some((path, previous, stop, check))
        }
        None => None,
    };
    let mut report = String::new();
    let info = app.package_info();
    let _ = writeln!(report, "{} {} diagnostics", info.name, info.version);
    let _ = writeln!(
        report,
        "os: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let lock = runtime::llama::lock::embedded();
    let _ = writeln!(
        report,
        "llama.cpp pin: {} ({})",
        lock.llama_cpp.tag, lock.llama_cpp.commit
    );
    let adapters = crate::gpu::adapters();
    if !adapters.is_empty() {
        let _ = writeln!(report, "graphics adapters (from the system):");
        for a in &adapters {
            let _ = writeln!(
                report,
                "  {} (driver {})",
                a.name,
                a.driver_version.as_deref().unwrap_or("unknown")
            );
        }
    }
    let _ = writeln!(report);
    let backends = state.runtime.backends(true).await;
    for (b, p) in &backends {
        let _ = writeln!(
            report,
            "backend {} [{}] {}",
            crate::runtime_manager::kind_name(b.kind),
            b.build,
            b.dir.display()
        );
        match &p.error {
            Some(e) => {
                let _ = writeln!(report, "  unusable: {e}");
                // What llama-server printed: "no device found" alone does not say why.
                if p.output.is_empty() {
                    let _ = writeln!(report, "  (llama-server printed nothing)");
                }
                for line in &p.output {
                    let _ = writeln!(report, "  | {line}");
                }
                // The app starts llama-server with a short list of environment variables.
                // If a graphics driver needs one that is not on it, the same check with
                // the whole environment finds the card: say so, it names the fix.
                if matches!(e, runtime::ProbeError::NoMatchingDevice { .. }) {
                    let wide = runtime::probe_with_env(
                        b,
                        std::time::Duration::from_secs(45),
                        full_environment(
                            std::env::vars_os()
                                .map(|(name, _)| name.to_string_lossy().into_owned()),
                        ),
                    )
                    .await;
                    if wide.devices.is_empty() {
                        let _ = writeln!(
                            report,
                            "  the same check with the system's full environment: also no device"
                        );
                    } else {
                        let _ = writeln!(
                            report,
                            "  the same check with the system's full environment FINDS {} device(s) (an environment variable the app does not pass on is needed):",
                            wide.devices.len()
                        );
                        for d in &wide.devices {
                            let _ = writeln!(
                                report,
                                "    {}: {} ({} MiB total)",
                                d.name, d.description, d.total_mib
                            );
                        }
                    }
                }
            }
            None => {
                for d in &p.devices {
                    let _ = writeln!(
                        report,
                        "  {}: {} ({} MiB total, {} MiB free)",
                        d.name, d.description, d.total_mib, d.free_mib
                    );
                }
            }
        }
    }
    if let Some((path, previous, stop, check)) = timed {
        let _ = writeln!(report, "\ntimed generation with {path}");
        // Asked to wait, the check waits again right before its first test load, for a
        // reply, another app's request or another model check that began while the
        // backends were probed: never stopped unasked, with the waiting line and its Cancel
        // as for the first wait. Cancel there, or a newer change, ends the check with the
        // report of the devices and nothing replaced.
        let cancelled = when_idle
            && state
                .runtime
                .wait_for_others(&state.model_busy, &stop)
                .await
                .is_err();
        if cancelled {
            let _ = writeln!(
                report,
                "  not run: cancelled while it waited for the model to be free (nothing was changed)"
            );
        }
        for (b, p) in backends.iter().filter(|(_, p)| p.error.is_none()) {
            if cancelled || stop.is_cancelled() {
                break;
            }
            let kind = crate::runtime_manager::kind_name(b.kind).to_string();
            let settings = diagnostic_load(
                &kind,
                runtime::pick_device(&p.devices).map(|d| d.name.clone()),
            );
            match state
                .runtime
                .clone()
                .load(PathBuf::from(&path), settings, load::Footprint::default())
                .await
            {
                Ok(()) if stop.is_cancelled() => break,
                Ok(()) => match crate::diagnostics::time_generation(&state).await {
                    Ok(line) => {
                        let _ = writeln!(report, "  {kind}: {line}");
                    }
                    Err(e) => {
                        let _ = writeln!(report, "  {kind}: generation failed: {}", e.details);
                    }
                },
                Err(e) => {
                    let _ = writeln!(
                        report,
                        "  {kind}: load failed: {} {}",
                        e.message,
                        e.details.lines().next().unwrap_or("")
                    );
                }
            }
        }
        let stopped = stop.is_cancelled();
        if stopped {
            let _ = writeln!(
                report,
                "  stopped: what is loaded was changed during the check"
            );
        }
        // Cancelled before its first test load, the check replaced nothing: it leaves it.
        match after_check(&previous, stopped || cancelled) {
            AfterCheck::LeaveIt => {}
            // The model loaded before comes back as a load resolves it: with its projector,
            // its own settings, whether it can think, and the speed check.
            AfterCheck::Load(model) => {
                state.runtime.stop_server().await;
                let plan = load::plan(&state, &model, None).await;
                let _ = tokio::time::timeout(
                    Duration::from_secs(600),
                    run_load(&state, PathBuf::from(model), &plan),
                )
                .await;
            }
            AfterCheck::Unloaded(path) => {
                state.runtime.stop_server().await;
                state.runtime.set_unloaded_path(path);
            }
        }
        // A change that waited for the check goes ahead now.
        drop(check);
    }
    let saved_to = app.path().app_log_dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let file = dir.join(format!(
            "diagnostics-{}.txt",
            chrono::Utc::now().format("%Y%m%d-%H%M%S")
        ));
        std::fs::write(&file, &report).ok()?;
        Some(file.display().to_string())
    });
    Ok(DiagnosticsReportDto {
        text: report,
        saved_to,
    })
}

/// What the backend check puts back when it ends.
#[derive(Debug, PartialEq, Eq)]
enum AfterCheck {
    /// The model loaded before (or whose load had failed), loaded again as a load
    /// resolves it.
    Load(String),
    /// Nothing was loaded: nothing is, and "Unloaded to free memory · Load" is as it was.
    Unloaded(Option<String>),
    /// A change of what is loaded stopped the check (Stop and restart): it decides what is
    /// loaded, so the check leaves it alone. Also when the check's wait before its first
    /// test load was cancelled: it replaced nothing.
    LeaveIt,
}

/// What the backend check puts back, from the status before it and whether a change
/// stopped it.
fn after_check(previous: &RuntimeStatusDto, stopped: bool) -> AfterCheck {
    if stopped {
        return AfterCheck::LeaveIt;
    }
    match &previous.model_path {
        Some(model) => AfterCheck::Load(model.clone()),
        None => AfterCheck::Unloaded(previous.unloaded_path.clone()),
    }
}

/// One timed load of the backend check: 4,096 tokens on `backend`, the whole model on the
/// graphics card, no projector.
fn diagnostic_load(backend: &str, device: Option<String>) -> load::ResolvedLoad {
    load::ResolvedLoad {
        ctx: 4096,
        ctx_choice: load::ContextSize::Fixed(4096),
        ctx_source: load::ContextSource::ModelSetting,
        gpu: load::GpuUse::All,
        save_memory: false,
        keep_in_memory: false,
        thinking_limit: load::ThinkingLimit::NoLimit,
        backend: Some(backend.to_string()),
        device,
        mmproj: None,
        fine: runtime::FineLaunch::default(),
    }
}

#[cfg(test)]
mod tests {
    /// D12: no tool, in any approval mode, reaches model settings through the generic
    /// setting command: neither the defaults for all models nor any model's own key is on
    /// its list (only their own validated commands write them).
    #[test]
    fn the_generic_setting_command_never_reaches_model_settings() {
        use crate::load_settings::{KEY_PREFIX, settings_key};
        assert_eq!(
            super::ALLOWED,
            [
                "theme",
                crate::settings::DEFAULT_SYSTEM_PROMPT,
                crate::settings::REMOTE_CONTEXT_LENGTH,
                crate::settings::STEPS_PER_TURN,
                crate::mcp_cmd::RENAMED_SERVERS,
            ],
            "the list changed: model settings must stay off it"
        );
        for key in [
            crate::settings::LOCAL_MODEL.to_string(),
            settings_key("model.gguf", 4_920_000_000),
        ] {
            assert!(!super::ALLOWED.contains(&key.as_str()), "{key}");
        }
        assert!(
            super::ALLOWED
                .iter()
                .all(|k| !k.starts_with(KEY_PREFIX) && *k != crate::settings::LOCAL_MODEL)
        );
    }

    /// What the backend check puts back when it ends: the model that was loaded (also one
    /// whose load had failed), or with nothing loaded "Unloaded to free memory · Load" as it
    /// was; nothing when a change of what is loaded stopped the check (Stop and restart),
    /// since that change decides what is loaded now.
    #[test]
    fn the_backend_check_puts_back_what_was_there() {
        use super::{AfterCheck, after_check};
        use crate::dto::{RuntimeStateDto, RuntimeStatusDto};
        let loaded = RuntimeStatusDto {
            state: RuntimeStateDto::Ready,
            model_path: Some("/m/a.gguf".into()),
            ..Default::default()
        };
        assert_eq!(
            after_check(&loaded, false),
            AfterCheck::Load("/m/a.gguf".into())
        );
        let failed = RuntimeStatusDto {
            state: RuntimeStateDto::Failed,
            ..loaded.clone()
        };
        assert_eq!(
            after_check(&failed, false),
            AfterCheck::Load("/m/a.gguf".into())
        );
        let unloaded = RuntimeStatusDto {
            unloaded_path: Some("/m/a.gguf".into()),
            ..Default::default()
        };
        assert_eq!(
            after_check(&unloaded, false),
            AfterCheck::Unloaded(Some("/m/a.gguf".into()))
        );
        assert_eq!(
            after_check(&RuntimeStatusDto::default(), false),
            AfterCheck::Unloaded(None)
        );
        for before in [loaded, failed, unloaded, RuntimeStatusDto::default()] {
            assert_eq!(after_check(&before, true), AfterCheck::LeaveIt);
        }
    }

    /// The backend check loads with fixed settings on the backend it checks.
    #[test]
    fn the_backend_check_loads_with_fixed_settings() {
        let r = super::diagnostic_load("vulkan", Some("Vulkan1".into()));
        assert_eq!(r.ctx, 4096);
        assert_eq!(r.ctx_choice, crate::load_settings::ContextSize::Fixed(4096));
        assert_eq!(r.backend.as_deref(), Some("vulkan"));
        assert_eq!(r.device.as_deref(), Some("Vulkan1"));
        assert!(r.mmproj.is_none() && !r.save_memory && !r.keep_in_memory);
    }

    /// Item 6 (I6-8): the backend check's probe with the whole environment never lets a
    /// crash start a debugger, whatever the app's own environment says.
    #[test]
    fn diagnostics_environment_turns_off_debugger_backtraces() {
        let env = super::full_environment(
            ["GGML_NO_BACKTRACE", "GGML_BACKTRACE_LLDB", "PATH"]
                .iter()
                .map(|s| s.to_string()),
        );
        let passed: Vec<(String, String)> = env
            .resolve_from(
                [
                    ("GGML_NO_BACKTRACE", ""),
                    ("GGML_BACKTRACE_LLDB", "1"),
                    ("PATH", "/usr/bin"),
                ]
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into())),
            )
            .into_iter()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.to_string_lossy().into_owned(),
                )
            })
            .collect();
        assert!(
            passed.contains(&("GGML_NO_BACKTRACE".into(), "1".into())),
            "{passed:?}"
        );
        assert!(!passed.iter().any(|(k, _)| k == "GGML_BACKTRACE_LLDB"));
        // The backend check's timed loads carry no Fine control.
        assert_eq!(
            super::diagnostic_load("cpu", None).fine,
            runtime::FineLaunch::default()
        );
    }

    #[test]
    fn the_diagnostic_environment_leaves_out_steering_and_secret_variables() {
        let names = [
            "ProgramData",
            "NUMBER_OF_PROCESSORS",
            "LLAMA_ARG_MODEL",
            "GGML_BACKEND_PATH",
            "HF_TOKEN",
            "OPENAI_API_KEY",
            "MY_PASSWORD",
            "VK_ICD_FILENAMES",
        ];
        let env = super::full_environment(names.iter().map(|s| s.to_string()));
        let passed: Vec<String> = env
            .resolve_from(names.iter().map(|n| ((*n).into(), "v".into())))
            .into_iter()
            .map(|(k, _)| k.to_string_lossy().into_owned())
            .collect();
        for wanted in ["ProgramData", "NUMBER_OF_PROCESSORS", "VK_ICD_FILENAMES"] {
            assert!(
                passed.iter().any(|p| p == wanted),
                "{wanted} missing: {passed:?}"
            );
        }
        for refused in [
            "LLAMA_ARG_MODEL",
            "GGML_BACKEND_PATH",
            "HF_TOKEN",
            "OPENAI_API_KEY",
            "MY_PASSWORD",
        ] {
            assert!(!passed.iter().any(|p| p == refused), "{refused} passed on");
        }
    }
}
