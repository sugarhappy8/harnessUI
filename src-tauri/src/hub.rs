//! Model manager commands (ARCHITECTURE.md 11): Hugging Face search and detail,
//! downloads, installed models, hardware detection, the fit check, the catalog and
//! first run.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use models::catalog::{Catalog, first_run};
use models::download::FileSpec;
use models::fit::{FitInput, Target};
use models::hf::{HfClient, HfConfig, HfError, SearchPage, SearchQuery};
use models::summary::ModelSummary;
use serde::Serialize;
use store::{ModelInput, ModelSource};
use tauri::{AppHandle, Manager, State};

use crate::downloads::{DownloadGroupDto, DownloadManager, GroupMeta};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

mod compare;
use compare::{Hub as _, LiveHub};

const HF_TOKEN_KEY: &str = "hf-token";
const SETTING_MODELS_DIR: &str = "modelsDir";
const SETTING_HF_ENDPOINT: &str = "hfEndpoint";
const SETTING_FIRST_RUN_DONE: &str = "firstRunDone";
const SETTING_CATALOG: &str = "catalog";
/// Bytes read from the start of a GGUF to get its parameters before download.
const HEAD_BYTES: u64 = 6 * 1024 * 1024;

fn hf_error(e: HfError) -> AppError {
    let (code, message, next) = match &e {
        HfError::NeedsToken => (
            "hf_needs_token",
            "This model is gated.",
            "Add a Hugging Face token in Settings → Models.",
        ),
        HfError::NeedsTerms => (
            "hf_needs_terms",
            "You have not accepted this model's terms.",
            "Open the model page on Hugging Face and accept its terms.",
        ),
        HfError::NotFound => (
            "hf_not_found",
            "That repository was not found, or it is private.",
            "Check the name.",
        ),
        HfError::FileNotFound => (
            "hf_file_not_found",
            "That file is not in the repository.",
            "Open the model again.",
        ),
        HfError::RateLimited(_) => (
            "hf_rate_limited",
            "Hugging Face is limiting requests from this network.",
            "Wait a minute, then try again.",
        ),
        HfError::InvalidToken => (
            "hf_invalid_token",
            "The Hugging Face token is not valid.",
            "Paste a new token.",
        ),
        HfError::Network(_) => (
            "network",
            "Hugging Face could not be reached.",
            "Check your connection. Everything else in the app works offline.",
        ),
        HfError::BlockedHost(_) => (
            "blocked_host",
            "The download was redirected to a host the app does not trust.",
            "Report this; Hugging Face may have changed its download servers.",
        ),
        _ => (
            "hf_error",
            "Hugging Face returned an unexpected answer.",
            "Try again later.",
        ),
    };
    AppError::new(code, message, next, e.to_string())
}

/// The Hub client with the user's token (official endpoint only) and mirror setting.
pub async fn hf_client(app: &AppHandle) -> AppResult<HfClient> {
    let state = app.state::<AppState>();
    let endpoint: Option<String> = state.store.get_setting(SETTING_HF_ENDPOINT).await?;
    let token = state.secrets.get(HF_TOKEN_KEY).ok().flatten();
    let info = app.package_info();
    HfClient::new(HfConfig {
        endpoint: endpoint.unwrap_or_else(|| models::hf::OFFICIAL_ENDPOINT.to_string()),
        token,
        user_agent: format!("{}/{}", info.name, info.version),
    })
    .map_err(hf_error)
}

/// The models folder: the setting, or `<home>/<app name>/models` (12).
pub async fn models_dir(app: &AppHandle) -> PathBuf {
    let state = app.state::<AppState>();
    if let Ok(Some(dir)) = state.store.get_setting::<String>(SETTING_MODELS_DIR).await {
        return PathBuf::from(dir);
    }
    let home = app.path().home_dir().unwrap_or_else(|_| PathBuf::from("."));
    home.join(app.package_info().name.clone()).join("models")
}

// ---- Hardware and fit -------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareDto {
    pub target: String,
    pub gpu_name: Option<String>,
    pub gpu_free_bytes: Option<u64>,
    pub ram_total_bytes: Option<u64>,
    pub ram_available_bytes: Option<u64>,
    pub usable_bytes: u64,
    pub backend: Option<String>,
    /// A Mac with an Intel processor: models run on the CPU there (see `detect`), which
    /// the first-run screen says in those words instead of "no supported GPU found".
    pub intel_mac: bool,
}

async fn detect(state: &AppState) -> (Target, HardwareDto) {
    let ram = models::hardware::system_memory();
    let ram_free = ram.map(|m| m.available_bytes).unwrap_or(0);
    let backends = state.runtime.backends(false).await;
    let mib = 1024 * 1024;
    for kind in [
        runtime::BackendKind::Metal,
        runtime::BackendKind::Vulkan,
        runtime::BackendKind::Cuda,
        runtime::BackendKind::Rocm,
    ] {
        // A Mac with an Intel processor runs models on the CPU unless Metal is picked by
        // hand, so its suggestion is sized for system memory, not for the GPU.
        // TODO(M9): size it for the GPU when the user has picked Metal on such a Mac.
        if kind == runtime::BackendKind::Metal && crate::runtime_manager::auto_prefers_cpu() {
            continue;
        }
        let Some((b, p)) = backends
            .iter()
            .find(|(b, p)| b.kind == kind && p.is_usable())
        else {
            continue;
        };
        let Some(d) = runtime::pick_device(&p.devices) else {
            continue;
        };
        let free = d.free_mib.saturating_mul(mib);
        let (target, label) = if kind == runtime::BackendKind::Metal {
            (
                Target::AppleUnified {
                    working_set_bytes: free,
                },
                "apple",
            )
        } else {
            (
                Target::Gpu {
                    name: d.description.clone(),
                    free_bytes: free,
                    ram_free_bytes: ram_free,
                },
                "gpu",
            )
        };
        let usable = match &target {
            Target::AppleUnified { working_set_bytes } => *working_set_bytes,
            Target::Gpu { free_bytes, .. } => *free_bytes,
            Target::Cpu { .. } => 0,
        };
        return (
            target,
            HardwareDto {
                target: label.into(),
                gpu_name: Some(d.description.clone()),
                gpu_free_bytes: Some(free),
                ram_total_bytes: ram.map(|m| m.total_bytes),
                ram_available_bytes: ram.map(|m| m.available_bytes),
                usable_bytes: usable,
                backend: Some(b.kind.as_str().to_string()),
                intel_mac: false,
            },
        );
    }
    cpu_hardware(
        ram.map(|m| m.total_bytes),
        ram.map(|m| m.available_bytes),
        crate::runtime_manager::auto_prefers_cpu(),
    )
}

/// What is reported when models run on the CPU: no usable GPU, or a Mac with an Intel
/// processor (`intel_mac`), where the CPU is used even though there is one.
fn cpu_hardware(
    ram_total: Option<u64>,
    ram_available: Option<u64>,
    intel_mac: bool,
) -> (Target, HardwareDto) {
    let ram_free = ram_available.unwrap_or(0);
    (
        Target::Cpu {
            ram_free_bytes: ram_free,
        },
        HardwareDto {
            target: "cpu".into(),
            gpu_name: None,
            gpu_free_bytes: None,
            ram_total_bytes: ram_total,
            ram_available_bytes: ram_available,
            usable_bytes: ram_free.saturating_sub(models::fit::RAM_RESERVE_BYTES),
            backend: Some("cpu".into()),
            intel_mac,
        },
    )
}

#[tauri::command]
pub async fn hardware_info(state: State<'_, AppState>) -> AppResult<HardwareDto> {
    Ok(detect(&state).await.1)
}

// ---- Search and detail ------------------------------------------------------

#[tauri::command]
pub async fn hub_search(app: AppHandle, query: SearchQuery) -> AppResult<SearchPage> {
    hf_client(&app)
        .await?
        .search(&query)
        .await
        .map_err(hf_error)
}

#[tauri::command]
pub async fn hub_search_next(app: AppHandle, next: String) -> AppResult<SearchPage> {
    hf_client(&app)
        .await?
        .search_next(&next)
        .await
        .map_err(hf_error)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariantView {
    pub label: String,
    pub files: Vec<models::RepoFile>,
    pub total_size: u64,
    pub bits: Option<f32>,
    pub fit: Option<models::FitResult>,
    pub installed: bool,
    pub has_checksum: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoView {
    pub id: String,
    pub commit: String,
    pub gated: models::hf::Gated,
    pub license: Option<String>,
    pub architecture: Option<String>,
    pub architecture_supported: Option<bool>,
    pub context_length: Option<u64>,
    pub parameters: Option<u64>,
    pub template_tools: bool,
    pub template_thinking: bool,
    pub projector: Option<models::RepoFile>,
    pub variants: Vec<VariantView>,
    pub incomplete: Vec<String>,
    pub card: Option<String>,
    pub fit_context: u64,
}

#[tauri::command]
pub async fn hub_repo(
    app: AppHandle,
    state: State<'_, AppState>,
    repo_id: String,
    ctx: Option<u64>,
) -> AppResult<RepoView> {
    let client = hf_client(&app).await?;
    let detail = client.repo(&repo_id, None).await.map_err(hf_error)?;
    // 0.0.6: the comparison reuses this listing and header instead of asking again.
    let live = LiveHub::new(Some(client.clone()));
    live.remember_listing(None, Arc::new(detail.clone()));
    let classified = models::classify(&detail.files);
    let fit_context = ctx
        .unwrap_or(u64::from(
            crate::load_settings::browse_fit_ctx(&state).await,
        ))
        .max(512);

    // Read the header of the smallest variant to get the model's real parameters (once
    // per commit in a session: headers at a commit never change).
    let summary = match classified.variants.iter().min_by_key(|v| v.total_size) {
        Some(v) => live
            .head(&detail.id, &detail.commit, &v.first_file().path)
            .await
            .ok()
            .map(|s| (*s).clone()),
        None => None,
    };
    let card = client.model_card(&detail.id, &detail.commit).await.ok();
    let (target, _) = detect(&state).await;
    let dir = models_dir(&app).await;
    let projector = classified.projectors.first().cloned();

    let mut variants = Vec::new();
    for v in &classified.variants {
        let fit = summary.as_ref().map(|s| {
            // Every field is set today; the update keeps this call site unchanged when
            // fields are added (0.0.5 WP1b).
            #[allow(clippy::needless_update)]
            let input = FitInput {
                weights_bytes: v.total_size,
                projector_bytes: projector.as_ref().map_or(0, |p| p.size),
                kv_bytes_per_token: s.kv_bytes_per_token(),
                vocab_size: s.vocab_size.unwrap_or(0),
                ctx: fit_context,
                max_ctx: s
                    .context_length
                    .or(detail.gguf.as_ref().and_then(|g| g.context_length)),
                ..Default::default()
            };
            models::check(&input, &target)
        });
        let installed = match models::download::destination(&dir, &detail.id, &v.first_file().path)
        {
            Ok(dest) => state
                .store
                .model_by_path(&dest.display().to_string())
                .await?
                .is_some(),
            Err(_) => false,
        };
        variants.push(VariantView {
            label: v.label.clone(),
            files: v.files.clone(),
            total_size: v.total_size,
            bits: v.bits,
            fit,
            installed,
            has_checksum: v.files.iter().all(|f| f.sha256.is_some()),
        });
    }
    let architecture = summary
        .as_ref()
        .map(|s| s.architecture.clone())
        .or_else(|| detail.gguf.as_ref().and_then(|g| g.architecture.clone()));
    Ok(RepoView {
        architecture_supported: architecture.as_deref().map(models::arch::is_supported),
        architecture,
        context_length: summary
            .as_ref()
            .and_then(|s| s.context_length)
            .or(detail.gguf.as_ref().and_then(|g| g.context_length)),
        parameters: detail.gguf.as_ref().and_then(|g| g.parameters),
        template_tools: summary
            .as_ref()
            .map_or(detail.template_tools, |s| s.template_tools),
        template_thinking: summary
            .as_ref()
            .map_or(detail.template_thinking, |s| s.template_thinking),
        id: detail.id,
        commit: detail.commit,
        gated: detail.gated,
        license: detail.license,
        projector,
        variants,
        incomplete: classified.incomplete,
        card,
        fit_context,
    })
}

// ---- Token ------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenStatus {
    pub saved: bool,
    pub account: Option<String>,
}

#[tauri::command]
pub async fn hub_token_status(state: State<'_, AppState>) -> AppResult<TokenStatus> {
    Ok(TokenStatus {
        saved: state.secrets.get(HF_TOKEN_KEY).ok().flatten().is_some(),
        account: None,
    })
}

/// Checks the token with Hugging Face, then stores it in the keychain.
#[tauri::command]
pub async fn hub_set_token(
    app: AppHandle,
    state: State<'_, AppState>,
    token: String,
) -> AppResult<String> {
    let token = token.trim().to_string();
    if token.is_empty()
        || token.len() > 512
        || token.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(AppError::bad_request(
            "That does not look like a Hugging Face token.",
        ));
    }
    let info = app.package_info();
    let client = HfClient::new(HfConfig {
        endpoint: models::hf::OFFICIAL_ENDPOINT.into(),
        token: Some(token.clone()),
        user_agent: format!("{}/{}", info.name, info.version),
    })
    .map_err(hf_error)?;
    let account = client.whoami().await.map_err(hf_error)?;
    state.secrets.set(HF_TOKEN_KEY, &token).map_err(|e| {
        AppError::new(
            "keychain",
            "The token could not be saved to the system keychain.",
            "Unlock the keychain and try again.",
            e.to_string(),
        )
    })?;
    Ok(account)
}

/// The mirror endpoint, or `None` for huggingface.co.
#[tauri::command]
pub async fn get_hf_endpoint(state: State<'_, AppState>) -> AppResult<Option<String>> {
    Ok(state.store.get_setting(SETTING_HF_ENDPOINT).await?)
}

/// Sets or clears the mirror. It must be https; the token is never sent to it.
#[tauri::command]
pub async fn set_hf_endpoint(
    state: State<'_, AppState>,
    endpoint: Option<String>,
) -> AppResult<()> {
    match endpoint
        .map(|e| e.trim().trim_end_matches('/').to_string())
        .filter(|e| !e.is_empty())
    {
        None => {
            state.store.delete_setting(SETTING_HF_ENDPOINT).await?;
        }
        Some(e) => {
            let url = url::Url::parse(&e)
                .map_err(|_| AppError::bad_request("The mirror address is not a valid URL."))?;
            if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() {
                return Err(AppError::bad_request(
                    "The mirror must be an https:// address without a user name.",
                ));
            }
            state.store.set_setting(SETTING_HF_ENDPOINT, &e).await?;
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn hub_clear_token(state: State<'_, AppState>) -> AppResult<()> {
    let _ = state.secrets.delete(HF_TOKEN_KEY);
    Ok(())
}

// ---- Downloads ----------------------------------------------------------------

fn file_spec(repo: &str, commit: &str, f: &models::RepoFile) -> FileSpec {
    FileSpec {
        repo: repo.into(),
        commit: commit.into(),
        path: f.path.clone(),
        size: f.size,
        sha256: f.sha256.clone(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn start_group(
    app: &AppHandle,
    repo: &str,
    commit: &str,
    label: &str,
    model_files: &[models::RepoFile],
    projector: Option<&models::RepoFile>,
    catalog_id: Option<String>,
    facts: Option<(crate::dto::HubFacts, Option<u64>)>,
) -> AppResult<String> {
    let dir = models_dir(app).await;
    std::fs::create_dir_all(&dir).map_err(|e| {
        AppError::new(
            "models_dir",
            "The models folder could not be created.",
            "Choose another folder in Models → Installed.",
            e.to_string(),
        )
    })?;
    let mut files = Vec::new();
    for f in model_files.iter().chain(projector) {
        let dest = models::download::destination(&dir, repo, &f.path).map_err(hf_error)?;
        files.push((file_spec(repo, commit, f), dest));
    }
    let meta = GroupMeta {
        repo: repo.into(),
        commit: commit.into(),
        label: label.into(),
        model_files: model_files.iter().map(|f| f.path.clone()).collect(),
        projector: projector.map(|p| p.path.clone()),
        catalog_id,
        parameters: facts.as_ref().and_then(|(_, p)| *p),
        hub: facts.map(|(h, _)| h),
    };
    let manager = app.state::<Arc<DownloadManager>>().inner().clone();
    manager.start(app, meta, files).await
}

#[tauri::command]
pub async fn download_start(
    app: AppHandle,
    repo_id: String,
    commit: String,
    label: String,
    with_projector: bool,
) -> AppResult<String> {
    let client = hf_client(&app).await?;
    // Re-read the listing at the pinned commit so sizes and hashes come from the Hub.
    let detail = client
        .repo(&repo_id, Some(&commit))
        .await
        .map_err(hf_error)?;
    let classified = models::classify(&detail.files);
    let variant = classified
        .variants
        .iter()
        .find(|v| v.label == label)
        .ok_or_else(|| AppError::not_found("model file"))?;
    let projector = if with_projector {
        classified.projectors.first()
    } else {
        None
    };
    // 0.0.6: what the uploader lists, saved with the model for comparing it offline.
    let facts = (
        compare::hub_facts(
            &detail,
            client
                .page_url(&detail.id)
                .unwrap_or_else(|| format!("{}/{}", models::hf::OFFICIAL_ENDPOINT, detail.id)),
        ),
        detail.gguf.as_ref().and_then(|g| g.parameters),
    );
    start_group(
        &app,
        &detail.id,
        &detail.commit,
        &label,
        &variant.files,
        projector,
        None,
        Some(facts),
    )
    .await
}

#[tauri::command]
pub async fn download_catalog_entry(app: AppHandle, entry_id: String) -> AppResult<String> {
    let catalog = current_catalog(&app).await;
    let entry = catalog
        .models
        .get(&entry_id)
        .ok_or_else(|| AppError::not_found("catalog entry"))?;
    // 0.0.6: the uploader's facts, saved with the model for comparing it offline: from
    // this session's cache, else one listing request at the pinned commit. The download
    // goes ahead without them when that fails; the catalog id is saved either way.
    let live = LiveHub::new(hf_client(&app).await.ok());
    let facts = compare::catalog_download_facts(&live, entry).await;
    start_group(
        &app,
        &entry.repo,
        &entry.revision,
        &entry.quant,
        &entry.files,
        entry.mmproj.as_ref(),
        Some(entry_id),
        facts,
    )
    .await
}

#[tauri::command]
pub async fn download_pause(app: AppHandle, group_id: String) -> AppResult<()> {
    app.state::<Arc<DownloadManager>>().pause(&group_id);
    Ok(())
}

#[tauri::command]
pub async fn download_resume(app: AppHandle, group_id: String) -> AppResult<()> {
    let m = app.state::<Arc<DownloadManager>>().inner().clone();
    m.resume(&app, &group_id).await
}

#[tauri::command]
pub async fn download_cancel(app: AppHandle, group_id: String) -> AppResult<()> {
    let m = app.state::<Arc<DownloadManager>>().inner().clone();
    m.cancel(&app, &group_id).await
}

#[tauri::command]
pub async fn list_downloads(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<DownloadGroupDto>> {
    app.state::<Arc<DownloadManager>>().list(&state).await
}

// ---- Installed models ---------------------------------------------------------

/// The app's JSON stored in `models.gguf_summary`.
#[derive(Debug, Clone, Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StoredSummary {
    #[serde(default)]
    architecture: Option<String>,
    #[serde(default)]
    quantization: Option<String>,
    #[serde(default)]
    size_label: Option<String>,
    #[serde(default)]
    context_length: Option<u64>,
    #[serde(default)]
    template_tools: bool,
    #[serde(default)]
    template_thinking: bool,
    #[serde(default)]
    is_moe: bool,
    #[serde(default)]
    projector: Option<String>,
    // ---- 0.0.6 item 4: for comparing installed models offline. Absent in rows saved
    // before; each is left out of the JSON when unknown.
    /// For a mixture of experts: how many experts it has and uses for each token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expert_count: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expert_used_count: Option<u64>,
    /// The parameter count Hugging Face gave when it was downloaded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    parameters: Option<u64>,
    /// The catalog entry it was downloaded from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog_id: Option<String>,
    /// The file's path in its Hugging Face repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo_file: Option<String>,
    /// What the uploader listed when it was downloaded (already made safe to show).
    #[serde(
        default,
        deserialize_with = "lenient_hub",
        skip_serializing_if = "Option::is_none"
    )]
    hub: Option<crate::dto::HubFacts>,
}

/// Reads saved uploader facts leniently: facts this version cannot read (a field missing
/// or of another type, written by a later version or edited by hand) are dropped on their
/// own, instead of making the whole saved value unreadable (the model's summary in
/// `gguf_summary`, a download group's [`GroupMeta`]).
pub(crate) fn lenient_hub<'de, D>(d: D) -> Result<Option<crate::dto::HubFacts>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Option<serde_json::Value> as serde::Deserialize>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// What a download knows about a model besides its file (0.0.6 item 4), saved with it.
#[derive(Debug, Clone, Default)]
pub struct SavedFacts {
    pub catalog_id: Option<String>,
    pub repo_file: Option<String>,
    pub hub: Option<crate::dto::HubFacts>,
    pub parameters: Option<u64>,
}

impl SavedFacts {
    fn is_empty(&self) -> bool {
        self.catalog_id.is_none()
            && self.repo_file.is_none()
            && self.hub.is_none()
            && self.parameters.is_none()
    }
}

/// Adds a model file to the installed list, reading its header for the badges.
#[allow(clippy::too_many_arguments)]
pub async fn register_installed(
    state: &AppState,
    path: &Path,
    repo: Option<String>,
    size: u64,
    sha256: Option<String>,
    projector: Option<String>,
    source: ModelSource,
    saved: SavedFacts,
) -> AppResult<store::ModelId> {
    register_installed_in(
        &state.store,
        path,
        repo,
        size,
        sha256,
        projector,
        source,
        saved,
    )
    .await
}

/// [`register_installed`] on a store. Registering a file again without saved facts (an
/// import of a downloaded file) keeps the facts saved before, as long as the file still
/// has the same size.
#[allow(clippy::too_many_arguments)]
async fn register_installed_in(
    store: &store::Store,
    path: &Path,
    repo: Option<String>,
    size: u64,
    sha256: Option<String>,
    projector: Option<String>,
    source: ModelSource,
    saved: SavedFacts,
) -> AppResult<store::ModelId> {
    let header = models::read_file(path).map_err(|e| {
        AppError::new(
            "not_gguf",
            "This file is not a GGUF model the app can read.",
            "Pick a .gguf file.",
            e.to_string(),
        )
    })?;
    let s = ModelSummary::from_header(&header);
    if s.is_projector {
        return Err(AppError::bad_request(
            "This is an image projector (mmproj), not a model. Import the model file next to it instead.",
        ));
    }
    let projector = projector.or_else(|| find_projector(path));
    let path_text = path.display().to_string();
    let saved = if saved.is_empty() {
        match store.model_by_path(&path_text).await? {
            Some(old) if old.size_bytes == size => {
                let before: StoredSummary =
                    serde_json::from_value(old.gguf_summary).unwrap_or_default();
                SavedFacts {
                    catalog_id: before.catalog_id,
                    repo_file: before.repo_file,
                    hub: before.hub,
                    parameters: before.parameters,
                }
            }
            _ => saved,
        }
    } else {
        saved
    };
    let stored = StoredSummary {
        is_moe: s.is_moe(),
        architecture: Some(s.architecture),
        quantization: s.quantization,
        size_label: s.size_label,
        context_length: s.context_length,
        template_tools: s.template_tools,
        template_thinking: s.template_thinking,
        projector,
        expert_count: s.expert_count,
        expert_used_count: s.expert_used_count,
        parameters: saved.parameters,
        catalog_id: saved.catalog_id,
        repo_file: saved.repo_file,
        hub: saved.hub,
    };
    let id = store
        .upsert_model(ModelInput {
            path: path_text,
            repo,
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            size_bytes: size,
            sha256,
            gguf_summary: serde_json::to_value(&stored).unwrap_or_default(),
            source,
        })
        .await?;
    Ok(id)
}

/// An `mmproj*.gguf` beside the model, preferring F16.
fn find_projector(model: &Path) -> Option<String> {
    let dir = model.parent()?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            let name = p
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            name.contains("mmproj") && name.ends_with(".gguf")
        })
        .collect();
    found.sort_by_key(|p| {
        let n = p.to_string_lossy().to_ascii_lowercase();
        if n.contains("f16") && !n.contains("bf16") {
            0
        } else if n.contains("bf16") {
            1
        } else {
            2
        }
    });
    found.first().map(|p| p.display().to_string())
}

/// The projector recorded for an installed model, if any.
pub async fn projector_for(state: &AppState, model_path: &str) -> Option<String> {
    let rec = state.store.model_by_path(model_path).await.ok().flatten()?;
    let stored: StoredSummary = serde_json::from_value(rec.gguf_summary).ok()?;
    stored.projector.filter(|p| Path::new(p).is_file())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledModelDto {
    pub id: i64,
    pub path: String,
    pub repo: Option<String>,
    pub file_name: String,
    pub size_bytes: u64,
    pub source: String,
    pub projector_path: Option<String>,
    pub summary: Option<serde_json::Value>,
    pub architecture_supported: Option<bool>,
}

#[tauri::command]
pub async fn list_installed_models(
    state: State<'_, AppState>,
) -> AppResult<Vec<InstalledModelDto>> {
    let mut out = Vec::new();
    for m in state.store.list_models().await? {
        let stored: StoredSummary =
            serde_json::from_value(m.gguf_summary.clone()).unwrap_or_default();
        out.push(InstalledModelDto {
            id: m.id.0,
            path: m.path.clone(),
            repo: m.repo.clone(),
            file_name: m.file_name.clone(),
            size_bytes: m.size_bytes,
            source: match m.source {
                ModelSource::Download => "download".into(),
                ModelSource::Import => "import".into(),
            },
            projector_path: stored.projector.clone(),
            architecture_supported: stored
                .architecture
                .as_deref()
                .map(models::arch::is_supported),
            summary: stored
                .architecture
                .is_some()
                .then(|| serde_json::to_value(&stored).unwrap_or_default()),
        });
    }
    Ok(out)
}

#[tauri::command]
pub async fn import_model(
    state: State<'_, AppState>,
    path: String,
) -> AppResult<InstalledModelDto> {
    let p = PathBuf::from(&path);
    if p.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
        != Some("gguf")
    {
        return Err(AppError::bad_request("Pick a .gguf model file."));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).map_err(|e| {
        AppError::new(
            "model_missing",
            "The file could not be read.",
            "Pick it again.",
            e.to_string(),
        )
    })?;
    register_installed(
        &state,
        &p,
        None,
        size,
        None,
        None,
        ModelSource::Import,
        SavedFacts::default(),
    )
    .await?;
    crate::settings::remember_local_model(&state, &path).await;
    list_installed_models(state)
        .await?
        .into_iter()
        .find(|m| m.path == path)
        .ok_or_else(|| AppError::internal("the imported model was not saved"))
}

#[tauri::command]
pub async fn delete_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    delete_files: bool,
) -> AppResult<()> {
    let Some(rec) = state.store.get_model(store::ModelId(id)).await? else {
        return Err(AppError::not_found("model"));
    };
    if state.runtime.status().model_path.as_deref() == Some(rec.path.as_str()) {
        state.runtime.unload().await;
    }
    if delete_files && rec.source == ModelSource::Download {
        // Only files inside the models folder are deleted, and all shards of a split model.
        let dir = models_dir(&app).await;
        let path = PathBuf::from(&rec.path);
        let inside = dunce::canonicalize(&path)
            .ok()
            .zip(dunce::canonicalize(&dir).ok())
            .is_some_and(|(p, d)| p.starts_with(d));
        if !inside {
            return Err(AppError::bad_request(
                "Only files inside the models folder are deleted; this one was left in place.",
            ));
        }
        for f in shard_siblings(&path) {
            let _ = std::fs::remove_file(f);
        }
    }
    state.store.delete_model(rec.id).await?;
    state.runtime.forget_unloaded(&rec.path);
    Ok(())
}

/// All shards of a split model (`-00001-of-00003.gguf`), or just the file.
fn shard_siblings(path: &Path) -> Vec<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let re = regex::Regex::new(r"(?i)^(.+)-(\d{5})-of-(\d{5})\.gguf$").expect("valid regex");
    let Some(c) = re.captures(&name) else {
        return vec![path.to_path_buf()];
    };
    let total: u32 = c[3].parse().unwrap_or(1);
    let base = c[1].to_string();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (1..=total)
        .map(|n| dir.join(format!("{base}-{n:05}-of-{total:05}.gguf")))
        .collect()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskUsageDto {
    pub models_dir: String,
    pub total_bytes: u64,
}

fn dir_size(dir: &Path, depth: u32) -> u64 {
    if depth > 6 {
        return 0;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 0;
    };
    rd.filter_map(|e| e.ok())
        .map(|e| match e.file_type() {
            Ok(t) if t.is_dir() => dir_size(&e.path(), depth + 1),
            Ok(t) if t.is_file() => e.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[tauri::command]
pub async fn disk_usage(app: AppHandle) -> AppResult<DiskUsageDto> {
    let dir = models_dir(&app).await;
    let d2 = dir.clone();
    let total = tokio::task::spawn_blocking(move || dir_size(&d2, 0))
        .await
        .unwrap_or(0);
    Ok(DiskUsageDto {
        models_dir: dir.display().to_string(),
        total_bytes: total,
    })
}

#[tauri::command]
pub async fn set_models_dir(state: State<'_, AppState>, path: String) -> AppResult<()> {
    let p = PathBuf::from(&path);
    if !p.is_dir() {
        return Err(AppError::bad_request("Pick an existing folder."));
    }
    let probe = p.join(".write-test");
    std::fs::write(&probe, b"").map_err(|e| {
        AppError::new(
            "models_dir",
            "The app cannot write to that folder.",
            "Pick a folder you own.",
            e.to_string(),
        )
    })?;
    let _ = std::fs::remove_file(probe);
    state.store.set_setting(SETTING_MODELS_DIR, &path).await?;
    Ok(())
}

// ---- Catalog and first run ------------------------------------------------------

async fn current_catalog(app: &AppHandle) -> Catalog {
    let state = app.state::<AppState>();
    let bundled = Catalog::bundled();
    match state.store.get_setting::<String>(SETTING_CATALOG).await {
        Ok(Some(json)) => match Catalog::parse(&json) {
            Ok(c) if c.sequence > bundled.sequence => c,
            _ => bundled,
        },
        _ => bundled,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionDto {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub quant: String,
    pub capabilities: Vec<String>,
    pub license: String,
    pub total_bytes: u64,
    pub memory_needed: u64,
    pub recommended_ctx: u64,
    pub tier_gb: u64,
    pub fits: bool,
    pub tested: bool,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FirstRunDto {
    pub done: bool,
    pub hardware: HardwareDto,
    pub pick: Option<String>,
    pub suggestions: Vec<SuggestionDto>,
    pub catalog_sequence: u64,
    pub refresh_disabled_reason: Option<String>,
}

#[tauri::command]
pub async fn first_run_info(app: AppHandle, state: State<'_, AppState>) -> AppResult<FirstRunDto> {
    let done: bool = state
        .store
        .get_setting(SETTING_FIRST_RUN_DONE)
        .await?
        .unwrap_or(false);
    // Someone who already has chats or models has effectively finished first run.
    let has_models = !state.store.list_models().await?.is_empty();
    let catalog = current_catalog(&app).await;
    let (_, hardware) = detect(&state).await;
    let picked = first_run(&catalog, hardware.usable_bytes);
    let dir = models_dir(&app).await;
    let mut suggestions = Vec::new();
    for s in picked.suggestions {
        let Some(e) = catalog.models.get(&s.id) else {
            continue;
        };
        let installed = match models::download::destination(&dir, &e.repo, &e.files[0].path) {
            Ok(dest) => state
                .store
                .model_by_path(&dest.display().to_string())
                .await?
                .is_some(),
            Err(_) => false,
        };
        suggestions.push(SuggestionDto {
            name: e.name.clone(),
            repo: e.repo.clone(),
            quant: e.quant.clone(),
            capabilities: e.capabilities.clone(),
            license: e.license.spdx.clone(),
            total_bytes: e.total_bytes(),
            memory_needed: s.memory_needed,
            recommended_ctx: s.recommended_ctx,
            tier_gb: s.tier_gb,
            fits: s.fits,
            tested: s.tested,
            installed,
            id: s.id,
        });
    }
    Ok(FirstRunDto {
        done: done || has_models,
        hardware,
        pick: picked.pick,
        suggestions,
        catalog_sequence: catalog.sequence,
        refresh_disabled_reason: updater_key(&app).is_none().then(|| {
            "Refresh needs the updater signing key, which has not been set up yet.".to_string()
        }),
    })
}

#[tauri::command]
pub async fn complete_first_run(state: State<'_, AppState>) -> AppResult<()> {
    state
        .store
        .set_setting(SETTING_FIRST_RUN_DONE, &true)
        .await?;
    Ok(())
}

/// The updater's public key from the app config, if one is configured.
fn updater_key(app: &AppHandle) -> Option<String> {
    app.config()
        .plugins
        .0
        .get("updater")
        .and_then(|u| u.get("pubkey"))
        .and_then(|k| k.as_str())
        .filter(|k| !k.trim().is_empty())
        .map(str::to_string)
}

/// Downloads `models.json` and its signature from this repo's latest release and
/// accepts it only if it verifies against the updater key and is newer.
#[tauri::command]
pub async fn refresh_catalog(app: AppHandle, state: State<'_, AppState>) -> AppResult<u64> {
    let Some(key) = updater_key(&app) else {
        return Err(AppError::new(
            "refresh_disabled",
            "Catalog refresh is not available in this build.",
            "It needs the updater signing key, which has not been set up yet.",
            String::new(),
        ));
    };
    let base = "https://github.com/sugarhappy8/harnessUI/releases/latest/download";
    let client = reqwest::Client::builder()
        .user_agent(format!(
            "{}/{}",
            app.package_info().name,
            app.package_info().version
        ))
        .build()
        .map_err(|e| AppError::internal(e.to_string()))?;
    let fetch = |url: String| {
        let client = client.clone();
        async move {
            let resp = client.get(&url).send().await.map_err(|e| {
                AppError::new(
                    "network",
                    "The catalog could not be downloaded.",
                    "Check your connection.",
                    e.without_url().to_string(),
                )
            })?;
            if !resp.status().is_success() {
                return Err(AppError::new(
                    "catalog_missing",
                    "No newer catalog was found.",
                    "Try again later.",
                    resp.status().to_string(),
                ));
            }
            resp.text()
                .await
                .map_err(|e| AppError::internal(e.without_url().to_string()))
        }
    };
    let json = fetch(format!("{base}/models.json")).await?;
    let sig = fetch(format!("{base}/models.json.minisig")).await?;
    let current = current_catalog(&app).await.sequence;
    let catalog = match models::catalog::verify_refresh(&json, &sig, Some(&key), current) {
        Ok(c) => c,
        // The release's catalog is the one this build already has: up to date, not an error.
        Err(models::catalog::CatalogError::NotNewer) => return Ok(current),
        Err(e) => {
            return Err(AppError::new(
                "catalog_rejected",
                "The downloaded catalog was rejected.",
                "Nothing changed; the current catalog stays.",
                e.to_string(),
            ));
        }
    };
    state.store.set_setting(SETTING_CATALOG, &json).await?;
    Ok(catalog.sequence)
}

// ---- Comparison (0.0.6 item 4, WP4a; the rules are in `compare`) ---------------------

/// The size a comparison is checked at: the one asked for, or the defaults' size (as the
/// repository page uses it).
async fn compare_ctx(state: &AppState, ctx: Option<u32>) -> u32 {
    match ctx {
        Some(c) => compare::clamp_ctx(c),
        None => compare::clamp_ctx(crate::load_settings::browse_fit_ctx(state).await),
    }
}

/// What a comparison needs from this computer: the catalog, the installed models, the
/// hardware and the user's results.
async fn compare_known(app: &AppHandle, state: &AppState) -> AppResult<compare::Known> {
    let catalog = current_catalog(app).await;
    let installed = state
        .store
        .list_models()
        .await?
        .iter()
        .map(compare::Installed::from_record)
        .collect();
    let (target, hardware) = detect(state).await;
    let profiles = crate::evals::commands::list_profiles(&state.store).await;
    let app2 = app.clone();
    // Benchmark runs are files; each with the size its model file has now.
    let bench = tokio::task::spawn_blocking(move || {
        crate::evals::commands::saved_bench_runs(&app2)
            .into_iter()
            .map(|run| {
                let size = run
                    .model_ref
                    .strip_prefix("local:")
                    .and_then(|p| std::fs::metadata(p).ok())
                    .map(|m| m.len());
                (run, size)
            })
            .collect::<Vec<_>>()
    })
    .await
    .unwrap_or_default();
    Ok(compare::Known {
        catalog,
        installed,
        target,
        usable_bytes: hardware.usable_bytes,
        results: compare::local_results(&profiles, &bench),
    })
}

/// The catalog's entries for Suggested models, best for this computer first, each with
/// one fit verdict at `ctx` (`None`: the defaults' size) when it is known without the
/// network, its installed path and the user's results. Fetches nothing.
#[tauri::command]
pub async fn catalog_list(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    ctx: Option<u32>,
) -> AppResult<Vec<crate::dto::CatalogListEntry>> {
    crate::commands::only_main_window(window.label())?;
    let ctx = compare_ctx(&state, ctx).await;
    let known = compare_known(&app, &state).await?;
    let hub = LiveHub::new(hf_client(&app).await.ok());
    Ok(compare::catalog_list(&hub, &known, ctx))
}

/// Facts for each model to compare, in the order given, with one fit verdict per item at
/// `ctx` (`None`: the defaults' size). At most four items; never fetches more than the one
/// repository of each item. What cannot be found is named in each item's `problem`.
#[tauri::command]
pub async fn compare_facts(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    items: Vec<crate::dto::CompareItem>,
    ctx: Option<u32>,
) -> AppResult<Vec<crate::dto::CompareFacts>> {
    crate::commands::only_main_window(window.label())?;
    check_compare_items(&items)?;
    let ctx = compare_ctx(&state, ctx).await;
    let known = compare_known(&app, &state).await?;
    let hub = LiveHub::new(hf_client(&app).await.ok());
    let mut out = Vec::with_capacity(items.len());
    // One at a time: gentler on Hugging Face's limits than all at once.
    for item in items {
        out.push(compare::facts_for(&hub, &known, item, ctx).await);
    }
    Ok(out)
}

/// A comparison holds at most [`compare::COMPARE_LIMIT`] models.
fn check_compare_items(items: &[crate::dto::CompareItem]) -> AppResult<()> {
    if items.len() > compare::COMPARE_LIMIT {
        return Err(AppError::bad_request(
            "You can compare up to 4 models. Remove one first.",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// More than four models is refused before anything is read.
    #[test]
    fn a_comparison_holds_at_most_four_models() {
        use crate::dto::CompareItem;
        let item = |n: usize| CompareItem::Catalog {
            catalog_id: format!("id-{n}"),
        };
        let four: Vec<CompareItem> = (0..4).map(item).collect();
        assert!(check_compare_items(&four).is_ok());
        assert!(check_compare_items(&[]).is_ok());
        let five: Vec<CompareItem> = (0..5).map(item).collect();
        let err = check_compare_items(&five).unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert!(err.message.contains("up to 4"), "{}", err.message);
    }

    /// Both comparison commands check the calling window first.
    #[test]
    fn comparison_commands_check_the_window_first() {
        let source = include_str!("hub.rs");
        for name in ["catalog_list", "compare_facts"] {
            let start = source
                .find(&format!("pub async fn {name}("))
                .unwrap_or_else(|| panic!("{name}"));
            let body = &source[start..];
            let body = &body[body.find(") -> ").unwrap()..];
            let first = body[body.find('{').unwrap() + 1..].trim_start();
            assert!(
                first.starts_with("crate::commands::only_main_window(window.label())?;"),
                "{name}"
            );
        }
    }

    /// Saved facts survive in `gguf_summary`; a row saved before 0.0.6 still reads.
    #[test]
    fn stored_summary_old_and_new() {
        let old: StoredSummary = serde_json::from_value(serde_json::json!({
            "architecture": "sample", "quantization": "Q4_K_M", "contextLength": 32768,
            "templateTools": true, "isMoe": false, "projector": null
        }))
        .unwrap();
        assert_eq!(old.architecture.as_deref(), Some("sample"));
        assert!(old.hub.is_none() && old.catalog_id.is_none() && old.parameters.is_none());
        // Unknown facts are left out of what is saved.
        let json = serde_json::to_value(&old).unwrap();
        for key in ["hub", "catalogId", "repoFile", "parameters", "expertCount"] {
            assert!(json.get(key).is_none(), "{key}");
        }
    }

    /// Saved uploader facts that this version cannot read (a field missing, written by a
    /// later version or edited by hand) are dropped on their own: the rest of the saved
    /// summary (the Installed badges, the catalog id) stays.
    #[test]
    fn a_saved_hub_this_version_cannot_read_loses_only_itself() {
        let hub = serde_json::json!({
            "repo": "example-quants/Sample-7B-GGUF", "author": "example-quants",
            "pipelineTag": null, "baseModel": null, "languages": [], "license": null,
            "tags": [], "downloads": 1, "likes": 2, "lastModified": null, "gated": false,
            "cardUrl": "https://huggingface.co/example-quants/Sample-7B-GGUF"
        });
        let summary = |hub: serde_json::Value| {
            serde_json::json!({
                "architecture": "sample", "quantization": "Q4_K_M", "isMoe": true,
                "catalogId": "sample-7b-q4km", "repoFile": "Sample-7B-Q4_K_M.gguf",
                "hub": hub
            })
        };
        let whole: StoredSummary = serde_json::from_value(summary(hub.clone())).unwrap();
        assert_eq!(whole.hub.as_ref().map(|h| h.downloads), Some(Some(1)));

        let mut partial = hub.clone();
        partial.as_object_mut().unwrap().remove("cardUrl");
        let mut wrong_type = hub.clone();
        wrong_type["tags"] = serde_json::json!("code");
        for bad in [
            partial,
            wrong_type,
            serde_json::json!("text"),
            serde_json::Value::Null,
        ] {
            let s: StoredSummary = serde_json::from_value(summary(bad.clone()))
                .unwrap_or_else(|e| panic!("{bad}: {e}"));
            assert!(s.hub.is_none(), "{bad}");
            assert_eq!(s.architecture.as_deref(), Some("sample"));
            assert_eq!(s.quantization.as_deref(), Some("Q4_K_M"));
            assert!(s.is_moe);
            assert_eq!(s.catalog_id.as_deref(), Some("sample-7b-q4km"));
            assert_eq!(s.repo_file.as_deref(), Some("Sample-7B-Q4_K_M.gguf"));
        }
    }

    /// The first-run screen reads `intelMac` to say "Intel Mac: models run on the
    /// processor" instead of "No supported GPU found".
    #[test]
    fn cpu_hardware_says_when_it_is_an_intel_mac() {
        let gb = 1_000_000_000u64;
        let (target, dto) = cpu_hardware(Some(16 * gb), Some(12 * gb), true);
        assert!(matches!(target, Target::Cpu { ram_free_bytes } if ram_free_bytes == 12 * gb));
        let json = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["target"], "cpu");
        assert_eq!(json["intelMac"], true);
        assert_eq!(json["backend"], "cpu");
        assert_eq!(json["ramAvailableBytes"], 12 * gb);
        assert_eq!(
            dto.usable_bytes,
            12 * gb - models::fit::RAM_RESERVE_BYTES.min(12 * gb)
        );

        // Any other computer without a usable GPU, and one whose memory could not be read.
        let (_, other) = cpu_hardware(None, None, false);
        assert!(!other.intel_mac);
        assert_eq!(other.usable_bytes, 0);
        assert_eq!(serde_json::to_value(&other).unwrap()["intelMac"], false);
    }
}
