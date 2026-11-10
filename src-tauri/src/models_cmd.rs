//! Commands for the local runtime: backends, loading a model, model options,
//! settings, diagnostics.

use std::path::PathBuf;
use std::time::Duration;

use tauri::{AppHandle, Manager, State};

use crate::dto::{
    BackendDto, DiagnosticsReportDto, LocalModelSettings, ModelOptionDto, RuntimeStatusDto,
};
use crate::error::{AppError, AppResult};
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

#[tauri::command]
pub async fn load_local_model(
    state: State<'_, AppState>,
    path: String,
    mut settings: LocalModelSettings,
) -> AppResult<()> {
    let model = PathBuf::from(&path);
    if model
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("gguf")
    {
        return Err(AppError::bad_request("Pick a .gguf model file."));
    }
    settings::remember_local_model(&state, &path).await;
    settings.mmproj = crate::hub::projector_for(&state, &path).await;
    // Never ask for more context than the model was trained for (read from its header).
    let header_path = model.clone();
    let summary = tokio::task::spawn_blocking(move || {
        models::read_file(&header_path)
            .ok()
            .map(|h| models::ModelSummary::from_header(&h))
    })
    .await
    .ok()
    .flatten();
    let thinking = summary.as_ref().is_some_and(|s| s.template_thinking);
    let trained = summary.and_then(|s| s.context_length);
    if let Some(max) = trained.and_then(|t| u32::try_from(t).ok())
        && settings.ctx > max
    {
        tracing::info!(
            requested = settings.ctx,
            trained = max,
            "context capped at the model's trained length"
        );
        settings.ctx = max;
    }
    // Picking the model that is already loaded with the same context is a no-op.
    let current = state.runtime.status();
    if current.state == crate::dto::RuntimeStateDto::Ready
        && current.model_path.as_deref() == Some(path.as_str())
        && current.n_ctx == Some(settings.ctx)
        && settings
            .backend
            .as_deref()
            .is_none_or(|b| current.backend.as_deref() == Some(b))
    {
        return Ok(());
    }
    let runtime = state.runtime.clone();
    runtime.load(model, settings).await?;
    state.runtime.set_thinking(thinking);
    let checker = state.runtime.clone();
    tauri::async_runtime::spawn(async move { checker.speed_check().await });
    Ok(())
}

#[tauri::command]
pub async fn unload_model(state: State<'_, AppState>) -> AppResult<()> {
    state.runtime.unload().await;
    Ok(())
}

#[tauri::command]
pub async fn get_local_model_settings(app: AppHandle) -> AppResult<LocalModelSettings> {
    Ok(settings::local_model_settings(&app).await)
}

#[tauri::command]
pub async fn set_local_model_settings(
    state: State<'_, AppState>,
    mut settings: LocalModelSettings,
) -> AppResult<()> {
    settings.mmproj = None;
    if !(512..=1_048_576).contains(&settings.ctx) {
        return Err(AppError::bad_request(
            "The context size must be between 512 and 1,048,576 tokens.",
        ));
    }
    state
        .store
        .set_setting(settings::LOCAL_MODEL, &settings)
        .await?;
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

#[tauri::command]
pub async fn set_setting(
    state: State<'_, AppState>,
    key: String,
    value: serde_json::Value,
) -> AppResult<()> {
    // Only plain UI preferences go through this generic command; anything with a
    // policy meaning has its own command (ARCHITECTURE.md 13.5).
    const ALLOWED: &[&str] = &["theme", settings::DEFAULT_SYSTEM_PROMPT];
    if !ALLOWED.contains(&key.as_str()) {
        return Err(AppError::bad_request(
            "That setting cannot be changed this way.",
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
        let previous = state.runtime.status();
        for (b, p) in backends.iter().filter(|(_, p)| p.error.is_none()) {
            let kind = crate::runtime_manager::kind_name(b.kind).to_string();
            let settings = LocalModelSettings {
                ctx: 4096,
                backend: Some(kind.clone()),
                device: runtime::pick_device(&p.devices).map(|d| d.name.clone()),
                mmproj: None,
            };
            match state
                .runtime
                .clone()
                .load(PathBuf::from(&path), settings)
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
        if let Some(model) = previous.model_path {
            let s = settings::local_model_settings(&app).await;
            let _ = tokio::time::timeout(
                Duration::from_secs(600),
                state.runtime.clone().load(PathBuf::from(model), s),
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
