//! Commands for the local runtime: backends, loading a model, model options,
//! settings, diagnostics.

use std::path::PathBuf;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};

use crate::dto::{
    BackendDto, DiagnosticsReportDto, LocalModelSettings, ModelBusy, ModelOptionDto,
    RuntimeStatusDto,
};
use crate::error::{AppError, AppResult};
use crate::load_settings::{self as load, ModelLoadInfo, ModelLoadSettings, Plan};
use crate::settings;
use crate::state::AppState;

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

/// Everything the settings panel shows about one model file.
#[tauri::command]
pub async fn model_load_info(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<ModelLoadInfo> {
    crate::commands::only_main_window(window.label())?;
    existing_model(&path)?;
    info_for(&state, &path).await
}

/// The panel's facts for `path` as saved now.
async fn info_for(state: &AppState, path: &str) -> AppResult<ModelLoadInfo> {
    let plan = load::plan(state, path, None).await;
    let facts = plan.facts.as_ref().ok_or_else(|| {
        AppError::new(
            "model_missing",
            "The model file could not be found.",
            "Pick the file again; it may have been moved or deleted.",
            path.to_string(),
        )
    })?;
    let loaded = state.runtime.loaded_choices(path);
    Ok(load::build_info(
        path,
        facts,
        plan.overrides.clone(),
        plan.defaults.clone(),
        plan.resolved.clone(),
        &plan.machine,
        loaded.as_ref(),
    ))
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
    load::validate(&overrides, facts.trained_ctx(), &plan.machine)
        .map_err(AppError::bad_request)?;
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
    if !(512..=1_048_576).contains(&settings.ctx) {
        return Err(AppError::bad_request(
            "The context size must be between 512 and 1,048,576 tokens.",
        ));
    }
    state
        .store
        .set_setting(settings::LOCAL_MODEL, &settings)
        .await?;
    refresh_settings_changed(&state).await;
    Ok(())
}

/// Model picker entries: recently used local files, then cached endpoint models.
/// Makes no network request.
#[tauri::command]
pub async fn list_model_options(state: State<'_, AppState>) -> AppResult<Vec<ModelOptionDto>> {
    let mut out = Vec::new();
    let recent: Vec<String> = state
        .store
        .get_setting(settings::RECENT_LOCAL_MODELS)
        .await?
        .unwrap_or_default();
    for path in recent {
        if !std::path::Path::new(&path).is_file() {
            continue;
        }
        let label = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.clone());
        out.push(ModelOptionDto {
            reference: format!("local:{path}"),
            label,
            source: "local".into(),
            context_length: None,
        });
    }
    for p in state.store.list_providers().await? {
        let cached: Vec<ModelOptionDto> = state
            .store
            .get_setting(&settings::provider_models_key(p.id))
            .await?
            .unwrap_or_default();
        out.extend(cached);
    }
    Ok(out)
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
    let mut env = runtime::EnvSpec::minimal();
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

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> AppResult<()> {
    // Only plain UI preferences go through this generic command; anything with a
    // policy meaning has its own command (ARCHITECTURE.md 13.5).
    const ALLOWED: &[&str] = &[
        "theme",
        settings::DEFAULT_SYSTEM_PROMPT,
        settings::REMOTE_CONTEXT_LENGTH,
        settings::STEPS_PER_TURN,
    ];
    if !ALLOWED.contains(&key.as_str()) {
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
#[tauri::command]
pub async fn run_diagnostics(
    app: AppHandle,
    state: State<'_, AppState>,
    model_path: Option<String>,
) -> AppResult<DiagnosticsReportDto> {
    use std::fmt::Write;
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
    if let Some(path) = model_path.filter(|p| std::path::Path::new(p).is_file()) {
        let _ = writeln!(report, "\ntimed generation with {path}");
        // The check replaces the loaded model: a reply or model check using it stops
        // first, and a change waiting for the model is cancelled.
        state
            .runtime
            .before_change(&state.model_busy, false)
            .await?;
        let previous = state.runtime.status();
        for (b, p) in backends.iter().filter(|(_, p)| p.error.is_none()) {
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
        state.runtime.unload().await;
        // The model loaded before comes back as a load resolves it: with its projector,
        // its own settings, whether it can think, and the speed check.
        if let Some(model) = previous.model_path {
            let plan = load::plan(&state, &model, None).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(600),
                run_load(&state, PathBuf::from(model), &plan),
            )
            .await;
        }
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
    }
}

#[cfg(test)]
mod tests {
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
