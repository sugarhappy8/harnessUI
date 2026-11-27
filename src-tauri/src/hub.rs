//! Model manager commands (ARCHITECTURE.md 11): Hugging Face search and detail,
//! downloads, installed models, hardware detection, the fit check, the catalog and
//! first run.

use std::collections::BTreeSet;
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
// 0.0.7 (P2): which image file a load uses, ones that could not be read, downloading one.
pub mod image_file;

const HF_TOKEN_KEY: &str = "hf-token";
const SETTING_MODELS_DIR: &str = "modelsDir";
const SETTING_HF_ENDPOINT: &str = "hfEndpoint";
const SETTING_FIRST_RUN_DONE: &str = "firstRunDone";
const SETTING_CATALOG: &str = "catalog";
/// Bytes read from the start of a GGUF to get its parameters before download.
const HEAD_BYTES: u64 = 6 * 1024 * 1024;

pub(crate) fn hf_error(e: HfError) -> AppError {
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

pub(crate) async fn detect(state: &AppState) -> (Target, HardwareDto) {
    let ram = models::hardware::system_memory();
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
                    // 0.0.7 review: the saved copies of a long chat take this memory.
                    ram_free_bytes: ram.map(|m| m.available_bytes),
                },
                "apple",
            )
        } else {
            (
                Target::Gpu {
                    name: d.description.clone(),
                    free_bytes: free,
                    // 0.0.7 review m5: unknown stays unknown, not 0.
                    ram_free_bytes: ram.map(|m| m.available_bytes),
                },
                "gpu",
            )
        };
        let usable = match &target {
            Target::AppleUnified {
                working_set_bytes, ..
            } => *working_set_bytes,
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
    /// Set with `architecture_supported == Some(false)` when no newer version would change it
    /// (a picture, video or speech model): the page says what it is, not that a newer version
    /// may open it.
    pub cant_open_why: Option<models::arch::Why>,
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
    // An installed file is judged as its Compare column judges it (compare-fit-minors).
    let on_computer: Vec<compare::Installed> = state
        .store
        .list_models()
        .await?
        .iter()
        .map(compare::Installed::from_record)
        .collect();
    let fit_ctx = u32::try_from(fit_context).unwrap_or(u32::MAX);

    let mut variants = Vec::new();
    for v in &classified.variants {
        let estimate = summary.as_ref().map(|s| {
            // Every field is set today; the update keeps this call site unchanged when
            // fields are added (0.0.6 WP1b).
            #[allow(clippy::needless_update)]
            let input = FitInput {
                weights_bytes: v.total_size,
                projector_bytes: projector.as_ref().map_or(0, |p| p.size),
                // f16, as a load with the default settings stores it (0.0.7 P1: short-
                // memory layers counted at their window).
                kv: s.kv_size(models::summary::KvCacheType::F16),
                vocab_size: s.vocab_size.unwrap_or(0),
                ctx: fit_context,
                max_ctx: s
                    .context_length
                    .or(detail.gguf.as_ref().and_then(|g| g.context_length)),
                ..Default::default()
            };
            models::check(&input, &target)
        });
        let fit = compare::variant_fit(
            &on_computer,
            &StatePlans(&state),
            &detail.id,
            v,
            fit_ctx,
            estimate,
        )
        .await;
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
    // The page claims "this version can't open it" only on evidence that holds together: the
    // smallest file's header and Hugging Face's listing (`arch::page_architecture`). A header
    // with no kind is not read, and two that disagree claim nothing.
    let judged = models::arch::page_architecture(
        summary.as_ref().map(|s| s.architecture.as_str()),
        detail.gguf.as_ref().and_then(|g| g.architecture.as_deref()),
        detail.info.pipeline_tag.as_deref(),
    );
    Ok(RepoView {
        architecture_supported: judged.supported,
        cant_open_why: judged.why,
        architecture: judged.name,
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

pub(crate) fn file_spec(repo: &str, commit: &str, f: &models::RepoFile) -> FileSpec {
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
    image_files: Option<Vec<models::RepoFile>>,
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
    // 0.1.0: a file there that an installed picture or video model uses at another revision
    // is never replaced: this file then goes to a place of its own.
    let state = app.state::<AppState>();
    let used = gen_files_used(
        &state.store.list_gen_models().await?,
        &state.store.list_gen_loras().await?,
        None,
    );
    let mut files = Vec::new();
    for f in model_files.iter().chain(projector) {
        let dest = download_destination(&dir, repo, &f.path, commit, f.sha256.as_deref(), &used)
            .map_err(hf_error)?;
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
        image_file_for: None,
        image_files,
        generator: None,
        file_only: false,
        lora: None,
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
    // The repository's image file that goes with the chosen model (0.0.7: a repository
    // may hold several models' image files), as Installed offers it.
    let projector = if with_projector {
        variant
            .files
            .first()
            .and_then(|f| image_file::best_for(&classified.projectors, &f.path))
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
    // 0.0.7 (P2 review M2): the repository's image files, saved with the model so Installed
    // can offer one later without asking Hugging Face.
    let image_files = Some(classified.projectors.clone());
    start_group(
        &app,
        &detail.id,
        &detail.commit,
        &label,
        &variant.files,
        projector.as_ref(),
        None,
        Some(facts),
        image_files,
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
    // 0.0.7 (P2 review M2): the catalog names the image file that goes with the model.
    let image_files = Some(entry.mmproj.iter().cloned().collect());
    start_group(
        &app,
        &entry.repo,
        &entry.revision,
        &entry.quant,
        &entry.files,
        entry.mmproj.as_ref(),
        Some(entry_id),
        facts,
        image_files,
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
    /// 0.0.7 (P2): image files that could not be read with this model (path, size and
    /// modification time), left out of its loads until they change or another one is
    /// downloaded (`image_file`). Absent in rows saved before, and when there are none; a
    /// list this version cannot read is dropped on its own, like `hub`.
    #[serde(
        default,
        deserialize_with = "lenient_stamps",
        skip_serializing_if = "Vec::is_empty"
    )]
    unreadable_projectors: Vec<image_file::FileStamp>,
    /// 0.0.7 (P6b review m4): the image file the app downloaded for this model itself (with
    /// it, or by "Download the image file"), the only one Delete may remove. Absent for one
    /// found next to the model, in rows saved before, and for imports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    downloaded_projector: Option<String>,
    /// 0.0.7 (P2 review m4): the model's embedding length from its header, which an image
    /// file next to it must match to be its own (`image_file::candidates`). Absent in rows
    /// saved before; filled in when a load asks which image file to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    embedding_length: Option<u64>,
    /// 0.0.7 (P2 review M2): the image files of its Hugging Face repository as the app last
    /// saw them (when it was downloaded, or when Installed looked), best first: what
    /// "Download the image file" offers without asking Hugging Face. Absent when the app
    /// never saw them (downloaded before 0.0.7, an import); empty when there were none.
    #[serde(
        default,
        deserialize_with = "lenient_files",
        skip_serializing_if = "Option::is_none"
    )]
    repo_image_files: Option<Vec<models::RepoFile>>,
    /// 0.0.7 (P2 review M1): the SHA-256 its repository published for the image file in
    /// `projector`, when the app downloaded that one, so Installed can tell whether
    /// downloading it again could help when it cannot be read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projector_sha256: Option<String>,
}

/// Reads the saved image files of a repository leniently (see [`lenient_hub`]).
fn lenient_files<'de, D>(d: D) -> Result<Option<Vec<models::RepoFile>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Option<serde_json::Value> as serde::Deserialize>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok()))
}

/// Reads the image files remembered as unreadable leniently (see [`lenient_hub`]).
fn lenient_stamps<'de, D>(d: D) -> Result<Vec<image_file::FileStamp>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = <Option<serde_json::Value> as serde::Deserialize>::deserialize(d)?;
    Ok(v.and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default())
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
    /// 0.0.7 (P2 review M2): the repository's image files when it was downloaded.
    pub image_files: Option<Vec<models::RepoFile>>,
    /// 0.0.7 (P2 review M1): the SHA-256 published for the image file downloaded with it.
    pub projector_sha256: Option<String>,
}

impl SavedFacts {
    fn is_empty(&self) -> bool {
        self.catalog_id.is_none()
            && self.repo_file.is_none()
            && self.hub.is_none()
            && self.parameters.is_none()
            && self.image_files.is_none()
            && self.projector_sha256.is_none()
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

/// A file the import cannot read as a GGUF model, and why (`detail`).
fn not_gguf(detail: impl std::fmt::Display) -> AppError {
    AppError::new(
        "not_gguf",
        "This file is not a GGUF model the app can read.",
        "Pick a .gguf file.",
        detail.to_string(),
    )
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
    let header = models::read_file(path).map_err(not_gguf)?;
    let s = ModelSummary::from_header(&header);
    if s.is_projector {
        return Err(AppError::bad_request(
            "This is the image file of a model, not a model. Import the model file next to it instead.",
        ));
    }
    let path_text = path.display().to_string();
    // Registered again without saved facts (an import of a downloaded file) and the same
    // size: what was saved with it stays, and so do the image files that could not be read
    // with it (0.0.7). A download (which brings its own facts and image file) starts anew.
    let before: Option<StoredSummary> = if saved.is_empty() {
        match store.model_by_path(&path_text).await? {
            Some(old) if old.size_bytes == size => {
                Some(serde_json::from_value(old.gguf_summary).unwrap_or_default())
            }
            _ => None,
        }
    } else {
        None
    };
    let unreadable = before
        .as_ref()
        .map(|b| b.unreadable_projectors.clone())
        .unwrap_or_default();
    let before_projector = before.as_ref().and_then(|b| b.projector.clone());
    let saved = match before {
        Some(before) => SavedFacts {
            catalog_id: before.catalog_id,
            repo_file: before.repo_file,
            hub: before.hub,
            parameters: before.parameters,
            image_files: before.repo_image_files,
            projector_sha256: before.projector_sha256,
        },
        None => saved,
    };
    let embedding_length = (s.embedding_length > 0).then_some(s.embedding_length);
    // An image file next to the model that is its own, the best precision first (0.0.7: one
    // that could not be read with it is skipped).
    let found = projector.is_none();
    let projector = projector.or_else(|| {
        image_file::choose(
            None,
            &image_file::candidates(path, embedding_length),
            &unreadable,
            image_file::FileStamp::of,
        )
        .path
    });
    // What the repository published goes with the image file it was for only.
    let projector_sha256 = if found && projector != before_projector {
        None
    } else {
        saved.projector_sha256
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
        unreadable_projectors: unreadable,
        // Set by the download that brought the image file (`note_downloaded_image`).
        downloaded_projector: None,
        embedding_length,
        repo_image_files: saved.image_files,
        projector_sha256,
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

/// The image file (projector) a load of an installed model uses, under any spelling of
/// its path (a link, a mapped drive: 0.0.6 item 8, `model_options::installed_row`). 0.0.7:
/// the one recorded on its row while it is there and could be read, else one next to the
/// model file, so a file put there later is used ([`image_file::choose`]). A model that is
/// not on the Installed list has none.
pub async fn projector_for(state: &AppState, model_path: &str) -> Option<String> {
    image_file::choice_for(&state.store, model_path).await.path
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
    /// The image file the next load uses (0.0.7: as a load chooses it, `image_file`): the
    /// one recorded while it is there and could be read, else one next to the model file.
    pub projector_path: Option<String>,
    pub summary: Option<serde_json::Value>,
    pub architecture_supported: Option<bool>,
    /// The path the model loaded or loading now was loaded with, when it is this row's
    /// file under any spelling (a link, a mapped drive), so Delete asks before unloading it
    /// (0.0.6 item 8). Sent only then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loaded_as: Option<String>,
    /// 0.0.7 (P2): its image file could not be read, and no other usable one is there, so
    /// it loads without image input (Installed says so and offers another one).
    pub image_file_unreadable: bool,
    /// 0.0.7 (P2 review M2): what its repository offers as its image file, from what was
    /// saved when it was downloaded; `None` when not known (no repository, or downloaded
    /// before 0.0.7: Installed offers to look). No request is made for it.
    pub image_file_offer: Option<image_file::Offer>,
    /// 0.0.7 (P6b review): for a download, what Delete with files removes from disk: the
    /// bytes of its files and of the image file the app downloaded for it, inside the models
    /// folder ([`removal_plan_with`]), and how many of its files stay because they are
    /// outside it. Zero for an imported model (Delete only takes it off the list).
    pub delete_bytes: u64,
    pub kept_outside: usize,
    /// 0.0.7 (P6b round 2): the one file Delete leaves outside the models folder is the image
    /// file, and how many files it leaves because another installed model uses them.
    pub only_image_outside: bool,
    pub kept_in_use: usize,
}

#[tauri::command]
pub async fn list_installed_models(
    app: AppHandle,
    state: State<'_, AppState>,
) -> AppResult<Vec<InstalledModelDto>> {
    let status = state.runtime.status();
    let loaded = matches!(
        status.state,
        crate::dto::RuntimeStateDto::Ready | crate::dto::RuntimeStateDto::Starting
    )
    .then_some(status.model_path)
    .flatten();
    let all = state.store.list_models().await?;
    let models_dir = models_dir(&app).await;
    let in_use = InUse::of(&all);
    let gen_files = state.store.gen_model_files().await?;
    let mut out = Vec::new();
    for m in all {
        let stored: StoredSummary =
            serde_json::from_value(m.gguf_summary.clone()).unwrap_or_default();
        let preview = match m.source {
            ModelSource::Download => delete_preview(&m, &models_dir, |f| {
                in_use.by_other_than(m.id, f) || used_elsewhere(&[], &gen_files, f)
            }),
            ModelSource::Import => DeletePreview::default(),
        };
        let image = image_file::choice_of_row(&m.path, &stored);
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
            projector_path: image.path,
            architecture_supported: stored
                .architecture
                .as_deref()
                .map(models::arch::is_supported),
            summary: stored
                .architecture
                .is_some()
                .then(|| serde_json::to_value(&stored).unwrap_or_default()),
            loaded_as: loaded
                .clone()
                .filter(|l| crate::model_options::same_file(l, &m.path)),
            image_file_unreadable: image.left_out,
            image_file_offer: image_file::saved_offer(m.repo.as_deref(), &m.path, &stored),
            delete_bytes: preview.bytes,
            kept_outside: preview.outside,
            only_image_outside: preview.only_image_outside,
            kept_in_use: preview.in_use,
        });
    }
    Ok(out)
}

/// 0.1.0 Studio: why a file cannot join the chat list as a model: it is a picture or video
/// model, by its architecture (when it names one) or its own tensor table (most such files
/// name none), or a speech or sound model, by its architecture (the runner opens some of
/// those, and the chat then fails on its first message).
pub(crate) fn diffusion_import_refusal(
    arch: Option<&str>,
    shapes: &[models::TensorShape],
) -> Option<AppError> {
    if arch.is_some_and(models::arch::is_speech) {
        return Some(crate::studio::errors::speech_model());
    }
    (arch.is_some_and(models::arch::is_diffusion)
        || models::gen_search::looks_like_diffusion(shapes))
    .then(crate::studio::errors::picture_model)
}

/// [`diffusion_import_refusal`] for the file at `path`. A file that names no architecture
/// and whose tensor table does not read is refused as not a model the app can read: nothing
/// says it is a chat model, and a picture model's table can hold what the reader refuses
/// (a tensor of five dimensions).
fn refuse_picture_model(path: &Path) -> AppResult<()> {
    let arch = models::read_file(path)
        .ok()
        .and_then(|h| h.architecture().map(str::to_string));
    let shapes = match models::read_tensor_shapes(path) {
        Ok(shapes) => shapes,
        Err(e) if arch.is_none() => return Err(not_gguf(e)),
        Err(_) => Vec::new(),
    };
    match diffusion_import_refusal(arch.as_deref(), &shapes) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[tauri::command]
pub async fn import_model(
    app: AppHandle,
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
    refuse_picture_model(&p)?;
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
    list_installed_models(app, state)
        .await?
        .into_iter()
        .find(|m| m.path == path)
        .ok_or_else(|| AppError::internal("the imported model was not saved"))
}

/// Takes a model off the Installed list; for a download with `delete_files`, its files go
/// too ([`removal_plan_with`]). Returns a note for the user when files were left in place.
///
/// 0.0.7 (P6): what goes from disk is worked out before anything is unloaded, and a model
/// whose file is gone, or outside the models folder (a folder used before "Change folder"),
/// still leaves the list: before, it stayed there for good.
#[tauri::command]
pub async fn delete_model(
    app: AppHandle,
    state: State<'_, AppState>,
    id: i64,
    delete_files: bool,
) -> AppResult<Option<String>> {
    let Some(rec) = state.store.get_model(store::ModelId(id)).await? else {
        return Err(AppError::not_found("model"));
    };
    // P6b review m1: an image file still downloading for it is cancelled first, and that
    // download has stopped before anything goes, so its file is not left behind.
    let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
    let cancel = async {
        downloads
            .cancel_image_files_for(&app, &rec.path)
            .await
            .map_err(|e| {
                if e.code == "download_stopping" {
                    AppError::new(
                        "download_stopping",
                        "This model's image file download is still stopping.",
                        "Wait a moment, then delete it again.",
                        String::new(),
                    )
                } else {
                    e
                }
            })
    };
    let runtime = &state.runtime;
    let note = delete_installed(
        &state.store,
        rec.id,
        delete_files,
        &models_dir(&app).await,
        cancel,
        // The same file loaded under another spelling (a link, a mapped drive) is unloaded
        // too, so its files are never deleted while llama-server has them open (0.0.6 item 8).
        |now: &store::ModelRecord| {
            runtime
                .status()
                .model_path
                .is_some_and(|p| crate::model_options::same_file(&p, &now.path))
                .then(|| runtime.unload())
        },
    )
    .await?;
    state.runtime.forget_unloaded(&rec.path);
    Ok(note)
}

/// What [`delete_model`] does with the model `id` (0.0.7, P6b round 2): cancels an image file
/// still downloading for it and waits for that download to stop (`cancel_downloads`), then
/// takes the row as it is now and removes it ([`remove_installed`]), unloading the model
/// first when `unload` gives a way to.
async fn delete_installed<F: std::future::Future<Output = ()>>(
    store: &store::Store,
    id: store::ModelId,
    delete_files: bool,
    models_dir: &Path,
    cancel_downloads: impl std::future::Future<Output = AppResult<()>>,
    unload: impl FnOnce(&store::ModelRecord) -> Option<F>,
) -> AppResult<Option<String>> {
    cancel_downloads.await?;
    // Read after the wait: the image file's download may have recorded its file on the row
    // meanwhile (P6b round 2), and then that file goes too.
    let Some(rec) = store.get_model(id).await? else {
        return Ok(None);
    };
    remove_installed(store, &rec, delete_files, models_dir, unload(&rec)).await
}

/// What [`delete_model`] does with the row `rec` (0.0.7, P6): works out what goes from disk
/// ([`removal_plan_with`], for a download with `delete_files`: never a file another chat model
/// or an installed picture or video model uses, [`used_elsewhere`]), then unloads the model
/// (`unload`, when it is the one loaded), deletes those files and forgets the row. It never
/// refuses.
async fn remove_installed(
    store: &store::Store,
    rec: &store::ModelRecord,
    delete_files: bool,
    models_dir: &Path,
    unload: Option<impl std::future::Future<Output = ()>>,
) -> AppResult<Option<String>> {
    let removal = if delete_files && rec.source == ModelSource::Download {
        let others: Vec<store::ModelRecord> = store
            .list_models()
            .await?
            .into_iter()
            .filter(|m| m.id != rec.id)
            .collect();
        let gen_files = store.gen_model_files().await?;
        removal_plan_with(rec, models_dir, |f| used_elsewhere(&others, &gen_files, f))
    } else {
        Removal::default()
    };
    if let Some(unload) = unload {
        unload.await;
    }
    for f in &removal.delete {
        let _ = std::fs::remove_file(f);
    }
    store.delete_model(rec.id).await?;
    crate::model_options::forget(store, &rec.path).await; // [0.0.6 item 8, WP8]
    Ok(kept_note(&removal))
}

/// What Delete removes from disk for a downloaded model (0.0.7, P6).
#[derive(Debug, Default, PartialEq, Eq)]
struct Removal {
    /// Files to delete.
    delete: Vec<PathBuf>,
    /// Files left in place because they are outside the models folder.
    outside: Vec<PathBuf>,
    /// The image file the plan looked at, if any.
    image: Option<PathBuf>,
    /// 0.0.7 (P6b round 2): files left in place because another installed model uses them.
    in_use: Vec<PathBuf>,
}

/// What Delete with files would do for a downloaded model, for its question (0.0.7, P6b).
#[derive(Debug, Default, PartialEq, Eq)]
struct DeletePreview {
    /// The bytes it removes from disk.
    bytes: u64,
    /// How many files it leaves because they are outside the models folder.
    outside: usize,
    /// The one file it leaves outside the models folder is the image file.
    only_image_outside: bool,
    /// How many files it leaves because another installed model uses them.
    in_use: usize,
}

/// What Delete with files would remove for the downloaded model `rec` ([`removal_plan_with`]),
/// for its question: the bytes, and how many files would stay because they are outside the
/// models folder. `in_use` tells a file another installed model uses ([`InUse`]).
fn delete_preview(
    rec: &store::ModelRecord,
    models_dir: &Path,
    in_use: impl Fn(&Path) -> bool,
) -> DeletePreview {
    let plan = removal_plan_with(rec, models_dir, in_use);
    let bytes = plan
        .delete
        .iter()
        .filter_map(|f| std::fs::metadata(f).ok())
        .map(|m| m.len())
        .sum();
    DeletePreview {
        bytes,
        outside: plan.outside.len(),
        only_image_outside: only_image_outside(&plan),
        in_use: plan.in_use.len(),
    }
}

/// Whether the one file Delete leaves outside the models folder is the image file.
fn only_image_outside(removal: &Removal) -> bool {
    removal.outside.len() == 1 && removal.outside.first() == removal.image.as_ref()
}

/// 0.0.7 (P6b review): the files the installed models use, as [`used_by_installed`] finds
/// them, looked up once for the whole list, so the list can say what Delete would remove
/// for each model without reading every other model's folder again.
struct InUse(std::collections::HashMap<String, Vec<store::ModelId>>);

impl InUse {
    fn of(models: &[store::ModelRecord]) -> Self {
        let mut map: std::collections::HashMap<String, Vec<store::ModelId>> =
            std::collections::HashMap::new();
        for m in models {
            for f in files_used(m) {
                if let Some(key) = file_key(&f) {
                    map.entry(key).or_default().push(m.id);
                }
            }
        }
        Self(map)
    }

    /// Whether a model other than `id` uses `file`.
    fn by_other_than(&self, id: store::ModelId, file: &Path) -> bool {
        file_key(file)
            .and_then(|key| self.0.get(&key))
            .is_some_and(|ids| ids.iter().any(|i| *i != id))
    }
}

/// How this system tells a file that is there: its real path, spelled as
/// [`crate::model_options::same_file`] compares them (case ignored on Windows and macOS).
fn file_key(file: &Path) -> Option<String> {
    crate::model_options::locate(&file.display().to_string())
        .map(|real| crate::model_options::PathRule::this_system().key(&real))
}

/// 0.0.7 (P6b review m4): records that the app downloaded `image` for the installed model at
/// `model_path` itself, so Delete may remove it with the model. Nothing happens for a model
/// that is not on the list.
pub(crate) async fn note_downloaded_image(
    store: &store::Store,
    model_path: &str,
    image: &Path,
) -> AppResult<()> {
    let Some(rec) = store.model_by_path(model_path).await? else {
        return Ok(());
    };
    let mut stored: StoredSummary =
        serde_json::from_value(rec.gguf_summary.clone()).unwrap_or_default();
    stored.downloaded_projector = Some(image.display().to_string());
    store
        .upsert_model(ModelInput {
            path: rec.path,
            repo: rec.repo,
            file_name: rec.file_name,
            size_bytes: rec.size_bytes,
            sha256: rec.sha256,
            gguf_summary: serde_json::to_value(&stored).unwrap_or_default(),
            source: rec.source,
        })
        .await?;
    Ok(())
}

/// [`removal_plan_with`] keeping the files the chat models in `others` use (two models of
/// one repository can share one image file).
#[cfg(test)]
fn removal_plan(
    rec: &store::ModelRecord,
    models_dir: &Path,
    others: &[store::ModelRecord],
) -> Removal {
    removal_plan_with(rec, models_dir, |f| used_by_installed(others, f))
}

/// The files Delete removes for the downloaded model `rec`: every shard of a split model
/// and the image file the app downloaded for it (P6b review m4: never one only found next
/// to it, `StoredSummary::downloaded_projector`). Only files inside the models folder are
/// deleted (the rest is left in place and said so), and none that something else uses
/// (`in_use`: another installed chat model, as two models of one repository can share one
/// image file, or a picture or video model, [`used_elsewhere`]). A file already gone is left
/// out, so the model just leaves the list.
fn removal_plan_with(
    rec: &store::ModelRecord,
    models_dir: &Path,
    in_use: impl Fn(&Path) -> bool,
) -> Removal {
    let stored: StoredSummary =
        serde_json::from_value(rec.gguf_summary.clone()).unwrap_or_default();
    let dir = dunce::canonicalize(models_dir).ok();
    let mut files = shard_siblings(Path::new(&rec.path));
    let mut out = Removal {
        image: stored.downloaded_projector.map(PathBuf::from),
        ..Removal::default()
    };
    files.extend(out.image.clone());
    for file in files {
        let Ok(real) = dunce::canonicalize(&file) else {
            continue;
        };
        if !real.is_file() || out.delete.contains(&file) {
            continue;
        }
        if in_use(&file) {
            if !out.in_use.contains(&file) {
                out.in_use.push(file);
            }
        } else if dir
            .as_ref()
            .is_some_and(|d| crate::downloads::inside_folder(&real, d))
        {
            out.delete.push(file);
        } else if !out.outside.contains(&file) {
            out.outside.push(file);
        }
    }
    out
}

/// What Delete says when it left files in place because they are outside the models folder:
/// the model's file or files, or only its image file.
fn kept_note(removal: &Removal) -> Option<String> {
    let note = match removal.outside.len() {
        0 => return None,
        1 if only_image_outside(removal) => {
            "Removed from the list. Its image file is outside the models folder, so it was left where it is."
        }
        1 => {
            "Removed from the list. Its file is outside the models folder, so it was left where it is."
        }
        _ => {
            "Removed from the list. Its files are outside the models folder, so they were left where they are."
        }
    };
    Some(note.into())
}

/// What Delete with files may not remove from a chat model (0.1.0 Studio): a file another
/// installed chat model uses ([`used_by_installed`], `others`), or any part file of an
/// installed picture or video model (`gen_files`, as `Store::gen_model_files` gives them;
/// FLUX.2 klein's text encoder is the same file as a common chat download). Paths are
/// compared as [`crate::model_options::same_file`] does.
pub(crate) fn used_elsewhere(
    others: &[store::ModelRecord],
    gen_files: &BTreeSet<String>,
    file: &Path,
) -> bool {
    let spelled = file.display().to_string();
    used_by_installed(others, file)
        || gen_files
            .iter()
            .any(|g| crate::model_options::same_file(g, &spelled))
}

/// 0.1.0: a file an installed model uses, with the SHA-256 recorded for it (`None` when none
/// is recorded).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsedFile {
    pub path: String,
    pub sha256: Option<String>,
}

/// 0.1.0: the files the installed chat models use ([`files_used`]): each model's file with
/// the SHA-256 recorded on its row, and its other shards and image files with none.
pub(crate) fn chat_files_used(models: &[store::ModelRecord]) -> Vec<UsedFile> {
    models
        .iter()
        .flat_map(|m| {
            files_used(m).into_iter().map(move |file| {
                let path = file.display().to_string();
                let sha256 = crate::model_options::same_file(&path, &m.path)
                    .then(|| m.sha256.clone())
                    .flatten();
                UsedFile { path, sha256 }
            })
        })
        .collect()
}

/// 0.1.0: the part files of the installed picture and video models, other than model
/// `except` (the one being downloaded again), each with the SHA-256 its part records; and
/// (0.2.0, Task S10) every LoRA's file with its SHA-256, so no download is renamed over one.
pub(crate) fn gen_files_used(
    models: &[store::GenModel],
    loras: &[store::GenLora],
    except: Option<&str>,
) -> Vec<UsedFile> {
    let parts = models
        .iter()
        .filter(|m| Some(m.id.as_str()) != except)
        .flat_map(|m| m.parts.as_array().cloned().unwrap_or_default())
        .filter_map(|part| {
            Some(UsedFile {
                path: part.get("file")?.as_str()?.to_owned(),
                sha256: part
                    .get("sha256")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            })
        });
    let lora_files = loras.iter().map(|l| UsedFile {
        path: l.file.clone(),
        sha256: Some(l.sha256.clone()),
    });
    parts.chain(lora_files).collect()
}

/// 0.1.0: where a download of `repo`'s file `path` at revision `commit`, pinned to SHA-256
/// `pinned`, goes: `<models>/<owner>/<repo>/<path>` ([`models::download::destination`]),
/// unless a file is already there that an installed model in `used` uses with another
/// SHA-256 (or with none recorded, or the download pins none, so they cannot be told the
/// same): then `<models>/<owner>/<repo>/@<commit, 12 characters>/<path>`, a file of its own. A
/// download renames its file over its destination once it is checked, so it never replaces a
/// file another installed model uses; deletion and [`used_elsewhere`] treat that place as
/// another file.
pub(crate) fn download_destination(
    models_dir: &Path,
    repo: &str,
    path: &str,
    commit: &str,
    pinned: Option<&str>,
    used: &[UsedFile],
) -> Result<PathBuf, HfError> {
    let dest = models::download::destination(models_dir, repo, path)?;
    let spelled = dest.display().to_string();
    let another = used
        .iter()
        .filter(|u| crate::model_options::same_file(&u.path, &spelled))
        .any(|u| match (pinned, u.sha256.as_deref()) {
            (Some(pinned), Some(recorded)) => !pinned.eq_ignore_ascii_case(recorded),
            _ => true,
        });
    if !another || !dest.is_file() {
        return Ok(dest);
    }
    let tag: String = commit.chars().take(12).collect();
    if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(HfError::InvalidName(commit.to_owned()));
    }
    let (owner, name) = models::hf::validate_repo_id(repo)?;
    let mut own = models_dir.join(owner).join(name).join(format!("@{tag}"));
    for segment in models::hf::validate_repo_path(path)? {
        own.push(segment);
    }
    Ok(own)
}

/// 0.0.7 (P6): whether `file` is a file of a model on the Installed list (any shard of it),
/// or an image file one of them uses: the one recorded on its row, or one next to it (a load
/// finds those, [`image_file::beside`]). Downloads can share such files: two models of one
/// repository can share one image file.
pub(crate) fn used_by_installed(models: &[store::ModelRecord], file: &Path) -> bool {
    let file = file.display().to_string();
    models.iter().any(|m| {
        files_used(m)
            .iter()
            .any(|p| crate::model_options::same_file(&p.display().to_string(), &file))
    })
}

/// The files the installed model `m` uses ([`used_by_installed`]): every shard, the image
/// file recorded on its row and the ones next to it.
fn files_used(m: &store::ModelRecord) -> Vec<PathBuf> {
    let model = Path::new(&m.path);
    let stored: StoredSummary = serde_json::from_value(m.gguf_summary.clone()).unwrap_or_default();
    let mut files = shard_siblings(model);
    files.extend(stored.projector.map(PathBuf::from));
    files.extend(image_file::beside(model));
    files
}

/// All shards of a split model (`-00001-of-00003.gguf`), or just the file. 0.0.7 (P6b
/// review): spelled as the file is (`-OF-`, `.GGUF`), so on a system whose names keep case
/// every part is found; comparing paths ([`crate::model_options::same_file`]) ignores case
/// only where the system does.
fn shard_siblings(path: &Path) -> Vec<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let re = regex::Regex::new(r"(?i)^(.+)-(\d{5})(-of-)(\d{5})(\.gguf)$").expect("valid regex");
    let Some(c) = re.captures(&name) else {
        return vec![path.to_path_buf()];
    };
    let total: u32 = c[4].parse().unwrap_or(1);
    let (base, of, ext) = (&c[1], &c[3], &c[5]);
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    (1..=total)
        .map(|n| dir.join(format!("{base}-{n:05}{of}{total:05}{ext}")))
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
    check_writable_folder(Path::new(&path))?;
    state.store.set_setting(SETTING_MODELS_DIR, &path).await?;
    Ok(())
}

/// Checks a folder the user picked to keep files in (the models folder, Studio's gallery):
/// it exists and the app can write a file in it (an empty `.write-test`, removed again).
pub(crate) fn check_writable_folder(path: &Path) -> AppResult<()> {
    if !path.is_dir() {
        return Err(AppError::bad_request("Pick an existing folder."));
    }
    let probe = path.join(".write-test");
    std::fs::write(&probe, b"").map_err(|e| {
        AppError::new(
            "models_dir",
            "The app cannot write to that folder.",
            "Pick a folder you own.",
            e.to_string(),
        )
    })?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}

// ---- Catalog and first run ------------------------------------------------------

pub(crate) async fn current_catalog(app: &AppHandle) -> Catalog {
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

/// How a load of an installed model would go now, with its own settings (the plan Installed
/// and Settings for this model judge), made when a column, a suggestion or a repository
/// page shows the file, never for every installed model (compare-fit-minors).
struct StatePlans<'a>(&'a AppState);

impl compare::Plans for StatePlans<'_> {
    async fn plan(&self, path: &str) -> Option<crate::load_settings::Plan> {
        Some(crate::load_settings::plan(self.0, path, None).await)
    }
}

/// What a comparison needs from this computer: the catalog, the installed models (a plan
/// for each is made only when it is shown), the hardware and the user's results.
async fn compare_known<'a>(
    app: &AppHandle,
    state: &'a AppState,
) -> AppResult<compare::Known<StatePlans<'a>>> {
    let catalog = current_catalog(app).await;
    let installed: Vec<compare::Installed> = state
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
        plans: StatePlans(state),
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
    Ok(compare::catalog_list(&hub, &known, ctx).await)
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

    fn table(fixture: &str) -> Vec<models::TensorShape> {
        models::gguf::testing::shapes(fixture)
    }

    /// 0.1.0 Studio: a picture or video model never joins the chat list, whether its file
    /// names an architecture or (leejet's files) has no metadata at all.
    #[test]
    fn importing_a_picture_model_says_add_it_in_studio() {
        let e = diffusion_import_refusal(Some("flux"), &[]).unwrap();
        assert_eq!(e.code, "picture_model");
        assert_eq!(e.message, "This is a picture or video model.");
        assert_eq!(e.next_step, "Add it in Studio → Get models.");
        assert!(diffusion_import_refusal(None, &table("klein4b-tensors.txt")).is_some());
        assert!(diffusion_import_refusal(Some("qwen3"), &table("qwen3-4b-tensors.txt")).is_none());
        assert!(diffusion_import_refusal(None, &[]).is_none());
    }

    /// A speech or sound model is not a chat model either, and the runner opens some of them
    /// (Kokoro, Moshi): the chat would load it and fail on the first message.
    #[test]
    fn importing_a_speech_model_says_it_can_not_chat() {
        for arch in ["kokoro", "whisper", "moshi"] {
            let e = diffusion_import_refusal(Some(arch), &[]).unwrap();
            assert_eq!(e.code, "speech_model", "{arch}");
            assert_eq!(e.message, "This is a speech or sound model.");
            assert_eq!(
                e.next_step,
                "It can't chat, and nothing in this app uses it."
            );
        }
    }

    /// The import reads the file's own architecture and tensor table.
    #[test]
    fn the_import_check_reads_the_file() {
        let dir = scratch();
        let picture = dir.0.join("flux-2-klein-4b-Q4_0.gguf");
        std::fs::write(
            &picture,
            models::gguf::testing::diffusion_gguf("klein4b-tensors.txt"),
        )
        .unwrap();
        assert_eq!(
            refuse_picture_model(&picture).unwrap_err().code,
            "picture_model"
        );
        let chat = dir.0.join("chat.gguf");
        std::fs::write(
            &chat,
            models::gguf::testing::Writer::new()
                .str("general.architecture", "qwen3")
                .tensor("token_embd.weight", 64)
                .tensor("blk.0.attn_q.weight", 64)
                .build(),
        )
        .unwrap();
        assert!(refuse_picture_model(&chat).is_ok());
        let named = dir.0.join("named.gguf");
        std::fs::write(
            &named,
            models::gguf::testing::Writer::new()
                .str("general.architecture", "wan")
                .tensor("blk.0.attn_q.weight", 64)
                .build(),
        )
        .unwrap();
        assert!(refuse_picture_model(&named).is_err());
        // Not a GGUF: refused in the import's own words.
        let text = dir.0.join("notes.gguf");
        std::fs::write(&text, "not a model").unwrap();
        assert_eq!(refuse_picture_model(&text).unwrap_err().code, "not_gguf");
    }

    /// A file with no architecture whose tensor table the reader refuses (a picture model's
    /// five-dimensional patch embedding) never joins the chat list; with an architecture,
    /// the architecture decides.
    #[test]
    fn a_file_with_no_architecture_and_an_unreadable_table_is_refused() {
        let dir = scratch();
        let table = || {
            models::gguf::testing::Writer::new()
                .shaped_tensor("blk.0.attn_q.weight", &[64, 64])
                .shaped_tensor("patch_embedding.weight", &[2, 2, 1, 48, 1536])
        };
        let bare = dir.0.join("model.gguf");
        std::fs::write(&bare, table().build()).unwrap();
        assert!(models::read_file(&bare).unwrap().architecture().is_none());
        assert!(models::read_tensor_shapes(&bare).is_err());
        let e = refuse_picture_model(&bare).unwrap_err();
        assert_eq!(e.code, "not_gguf");
        assert_eq!(e.message, "This file is not a GGUF model the app can read.");
        let chat = dir.0.join("chat.gguf");
        std::fs::write(&chat, table().str("general.architecture", "qwen3").build()).unwrap();
        assert!(refuse_picture_model(&chat).is_ok());
        let wan = dir.0.join("wan.gguf");
        std::fs::write(&wan, table().str("general.architecture", "wan").build()).unwrap();
        assert_eq!(
            refuse_picture_model(&wan).unwrap_err().code,
            "picture_model"
        );
    }

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

    /// A model on the Installed list at `path`, downloaded, with `projector` recorded as the
    /// image file the app downloaded for it.
    fn downloaded(id: i64, path: &Path, projector: Option<&Path>) -> store::ModelRecord {
        let image = projector.map(|p| p.display().to_string());
        with_summary(
            id,
            path,
            StoredSummary {
                architecture: Some("sample".into()),
                projector: image.clone(),
                downloaded_projector: image,
                ..Default::default()
            },
        )
    }

    /// A downloaded model at `path` whose recorded image file was found next to it, not
    /// downloaded by the app.
    fn found_image(id: i64, path: &Path, image: &Path) -> store::ModelRecord {
        with_summary(
            id,
            path,
            StoredSummary {
                architecture: Some("sample".into()),
                projector: Some(image.display().to_string()),
                ..Default::default()
            },
        )
    }

    fn with_summary(id: i64, path: &Path, stored: StoredSummary) -> store::ModelRecord {
        store::ModelRecord {
            id: store::ModelId(id),
            path: path.display().to_string(),
            repo: Some("example-quants/Sample-7B-GGUF".into()),
            file_name: String::new(),
            size_bytes: 1,
            sha256: None,
            gguf_summary: serde_json::to_value(stored).unwrap(),
            added_at: 0,
            source: ModelSource::Download,
        }
    }

    /// A folder of its own for one test, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch() -> Scratch {
        let dir = std::env::temp_dir().join(format!("delete-model-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }

    /// 0.0.7 (P6): Delete removes a downloaded model's files inside the models folder: every
    /// shard and, new, the image file downloaded with it, unless another installed model
    /// uses that image file (recorded, or next to it: two models of one repository share
    /// one). The bug: a model whose file was gone, or in a folder used before "Change folder",
    /// could never leave the list ("Only files inside the models folder are deleted"). Now
    /// files already gone are left out, so the model just leaves the list, and files outside
    /// the models folder are left where they are, and the note says so.
    #[test]
    fn what_delete_removes_from_disk() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let shard1 = repo.join("Sample-7B-Q8_0-00001-of-00002.gguf");
        let shard2 = repo.join("Sample-7B-Q8_0-00002-of-00002.gguf");
        let image = repo.join("mmproj-Sample-F16.gguf");
        for f in [&shard1, &shard2, &image] {
            std::fs::write(f, b"gguf").unwrap();
        }
        let q8 = downloaded(1, &shard1, Some(image.as_path()));
        let plan = removal_plan(&q8, &models, &[]);
        assert_eq!(
            plan.delete,
            vec![shard1.clone(), shard2.clone(), image.clone()]
        );
        assert!(plan.outside.is_empty());
        assert_eq!(kept_note(&plan), None);

        // Another model of the repository uses the image file: recorded, or found next to it.
        let q4 = repo.join("Sample-7B-Q4_K_M.gguf");
        std::fs::write(&q4, b"gguf").unwrap();
        for other in [
            downloaded(2, &q4, Some(image.as_path())),
            downloaded(2, &q4, None),
        ] {
            assert_eq!(
                removal_plan(&q8, &models, &[other]).delete,
                vec![shard1.clone(), shard2.clone()]
            );
        }

        // Its files are gone already: nothing to delete and nothing to say; it leaves the list.
        let gone_image = repo.join("mmproj-Gone-F16.gguf");
        let gone = downloaded(
            3,
            &repo.join("Gone-Q4_K_M.gguf"),
            Some(gone_image.as_path()),
        );
        let plan = removal_plan(&gone, &models, &[]);
        assert_eq!((plan.delete.len(), plan.outside.len()), (0, 0));
        assert_eq!(kept_note(&plan), None);

        // In a folder used before "Change folder": left in place, and said so.
        let old = root.0.join("old-models/example-quants/Old-GGUF");
        std::fs::create_dir_all(&old).unwrap();
        let old_model = old.join("Old-Q4_K_M.gguf");
        let old_image = old.join("mmproj-Old-F16.gguf");
        std::fs::write(&old_model, b"gguf").unwrap();
        std::fs::write(&old_image, b"gguf").unwrap();
        let plan = removal_plan(
            &downloaded(4, &old_model, Some(old_image.as_path())),
            &models,
            &[],
        );
        assert!(plan.delete.is_empty());
        assert_eq!(plan.outside, vec![old_model.clone(), old_image.clone()]);
        assert_eq!(
            kept_note(&plan).as_deref(),
            Some(
                "Removed from the list. Its files are outside the models folder, so they were left where they are."
            )
        );
        let plan = removal_plan(&downloaded(4, &old_model, None), &models, &[]);
        assert_eq!(
            kept_note(&plan).as_deref(),
            Some(
                "Removed from the list. Its file is outside the models folder, so it was left where it is."
            )
        );
        // A models folder that is not there any more holds nothing to delete.
        let plan = removal_plan(&q8, &root.0.join("no-such-folder"), &[]);
        assert!(plan.delete.is_empty());
        assert_eq!(plan.outside.len(), 3);
    }

    /// P6b review m4: Delete removes only an image file the app downloaded for the model. One
    /// found next to it (put there by hand, or by another app) and recorded at registration or
    /// at a load stays.
    #[test]
    fn delete_keeps_an_image_file_the_app_did_not_download() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let model = repo.join("Sample-7B-Q4_K_M.gguf");
        let image = repo.join("mmproj-Sample-F16.gguf");
        std::fs::write(&model, b"gguf").unwrap();
        std::fs::write(&image, b"gguf").unwrap();
        let plan = removal_plan(&found_image(1, &model, &image), &models, &[]);
        assert_eq!(plan.delete, vec![model.clone()]);
        assert!(plan.outside.is_empty());
        let plan = removal_plan(&downloaded(1, &model, Some(image.as_path())), &models, &[]);
        assert_eq!(plan.delete, vec![model.clone(), image.clone()]);
        // Only the downloaded image file outside the models folder: the note says so.
        let elsewhere = root.0.join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        let far_image = elsewhere.join("mmproj-Sample-F16.gguf");
        std::fs::write(&far_image, b"gguf").unwrap();
        let plan = removal_plan(
            &downloaded(1, &model, Some(far_image.as_path())),
            &models,
            &[],
        );
        assert_eq!(plan.delete, vec![model.clone()]);
        assert_eq!(
            kept_note(&plan).as_deref(),
            Some(
                "Removed from the list. Its image file is outside the models folder, so it was left where it is."
            )
        );
    }

    /// P6b review nit: a split model whose names are spelled `-OF-` and `.GGUF` keeps that
    /// spelling, so Delete finds every part and a cancelled download sees them as in use.
    #[test]
    fn a_split_model_keeps_the_spelling_of_its_names() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("example-quants/Big-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let first = repo.join("Big-Q4_K_M-00001-OF-00002.GGUF");
        let second = repo.join("Big-Q4_K_M-00002-OF-00002.GGUF");
        std::fs::write(&first, b"gguf").unwrap();
        std::fs::write(&second, b"gguf").unwrap();
        assert_eq!(shard_siblings(&first), vec![first.clone(), second.clone()]);
        let big = downloaded(1, &first, None);
        assert_eq!(
            removal_plan(&big, &models, &[]).delete,
            vec![first.clone(), second.clone()]
        );
        assert!(used_by_installed(&[big], &second));
    }

    /// P6b review nit: Delete's question counts what really goes: every part and the image
    /// file the app downloaded, inside the models folder, unless another installed model uses
    /// it, and how many files stay because they are outside it. P6b round 2: and whether the
    /// one file left outside is the image file, and how many stay because another model on
    /// the list uses them (so the question never says "no longer on disk" for those).
    #[test]
    fn the_delete_question_counts_what_goes() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let model = repo.join("Sample-7B-Q4_K_M.gguf");
        let image = repo.join("mmproj-Sample-F16.gguf");
        std::fs::write(&model, vec![0u8; 1000]).unwrap();
        std::fs::write(&image, vec![0u8; 300]).unwrap();
        let rec = downloaded(1, &model, Some(image.as_path()));
        let alone = InUse::of(std::slice::from_ref(&rec));
        let preview = |rec: &store::ModelRecord, in_use: &InUse| {
            delete_preview(rec, &models, |f| in_use.by_other_than(rec.id, f))
        };
        let goes = |bytes: u64| DeletePreview {
            bytes,
            ..DeletePreview::default()
        };
        let stays = |outside: usize, only_image_outside: bool, in_use: usize| DeletePreview {
            bytes: 0,
            outside,
            only_image_outside,
            in_use,
        };
        assert_eq!(preview(&rec, &alone), goes(1300));

        // Another model of the repository uses the image file (found next to it): it stays,
        // as Delete leaves it (`used_by_installed`).
        let q8 = repo.join("Sample-7B-Q8_0.gguf");
        std::fs::write(&q8, vec![0u8; 2000]).unwrap();
        let other = downloaded(2, &q8, None);
        let both = InUse::of(&[rec.clone(), other.clone()]);
        assert!(used_by_installed(std::slice::from_ref(&other), &image));
        assert_eq!(
            preview(&rec, &both),
            DeletePreview {
                in_use: 1,
                ..goes(1000)
            }
        );
        assert_eq!(
            removal_plan(&rec, &models, std::slice::from_ref(&other)).delete,
            vec![model.clone()]
        );

        let old = root.0.join("old-models");
        std::fs::create_dir_all(&old).unwrap();
        let away = old.join("Old-Q4_K_M.gguf");
        let away_image = old.join("mmproj-Old-F16.gguf");
        std::fs::write(&away, vec![0u8; 500]).unwrap();
        std::fs::write(&away_image, vec![0u8; 50]).unwrap();
        let none = InUse::of(&[]);
        assert_eq!(
            preview(&downloaded(3, &away, None), &none),
            stays(1, false, 0)
        );
        assert_eq!(
            preview(&downloaded(3, &away, Some(away_image.as_path())), &none),
            stays(2, false, 0)
        );
        let gone = repo.join("Gone-Q4_K_M.gguf");
        assert_eq!(
            preview(&downloaded(4, &gone, None), &none),
            stays(0, false, 0)
        );
        // Its file is gone and its image file is outside: the question says the image file,
        // as the note after Delete does.
        assert_eq!(
            preview(&downloaded(4, &gone, Some(away_image.as_path())), &none),
            stays(1, true, 0)
        );
        // Another entry of the list is a part of the same split model: its files stay.
        let first = repo.join("Big-Q2_K-00001-of-00002.gguf");
        let second = repo.join("Big-Q2_K-00002-of-00002.gguf");
        std::fs::write(&first, vec![0u8; 10]).unwrap();
        std::fs::write(&second, vec![0u8; 10]).unwrap();
        let split = downloaded(5, &first, None);
        let part = downloaded(6, &second, None);
        let listed = InUse::of(&[split.clone(), part]);
        assert_eq!(preview(&split, &listed), stays(0, false, 2));
    }

    /// P6b round 2 (verification K2): Delete waits for the image file's download to stop, and
    /// that download can record its image file on the model's row meanwhile; Delete then plans
    /// with the row as it is now, so the image file goes with the model. Before, it planned
    /// with the row it read before the wait, and the image file stayed behind.
    #[tokio::test]
    async fn delete_takes_the_row_as_it_is_after_waiting_for_the_image_download() {
        let root = scratch();
        let models = root.0.join("models");
        let folder = models.join("k/K-GGUF");
        std::fs::create_dir_all(&folder).unwrap();
        let model = folder.join("K-Q4.gguf");
        let image = folder.join("mmproj-K-F16.gguf");
        std::fs::write(&model, b"gguf").unwrap();
        std::fs::write(&image, b"gguf").unwrap();
        let store = store::Store::open_in_memory().unwrap();
        let rec = on_the_list(&store, &downloaded(0, &model, None)).await;
        let finished_meanwhile = async { note_downloaded_image(&store, &rec.path, &image).await };
        let note = delete_installed(&store, rec.id, true, &models, finished_meanwhile, |_| {
            None::<std::future::Ready<()>>
        })
        .await
        .unwrap();
        assert_eq!(note, None);
        assert!(!model.exists());
        assert!(
            !image.exists(),
            "the image file recorded meanwhile goes too"
        );
        assert!(store.get_model(rec.id).await.unwrap().is_none());
    }

    /// Finding local-shared-path-revision-overwrite: a chat download at another commit of a
    /// file an installed picture model uses under another SHA-256 (FLUX.2 klein's text encoder)
    /// goes to a place of its own, never renamed over the picture model's part; the same file
    /// is shared. The chat models' files carry the SHA-256 their rows record.
    #[test]
    fn a_download_never_replaces_another_installed_models_file_of_another_revision() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = "unsloth/Qwen3-4B-GGUF";
        let file = "Qwen3-4B-Q4_K_M.gguf";
        let shared = models.join("unsloth").join("Qwen3-4B-GGUF").join(file);
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::fs::write(&shared, b"gguf").unwrap();
        let (old, new) = ("a".repeat(64), "b".repeat(64));
        let commit = "0123456789abcdef0123456789abcdef01234567";
        let klein = store::GenModel {
            id: "flux2-klein-4b-q4".into(),
            catalog_id: Some("flux2-klein-4b-q4".into()),
            name: "FLUX.2 klein 4B".into(),
            kind: "image".into(),
            family: "flux2-klein-4b".into(),
            recipe_id: Some("flux2-klein-4b".into()),
            parts: serde_json::json!([
                {"role": "llm", "file": shared.display().to_string(), "sha256": old}
            ]),
            size_bytes: 4,
            source: store::GenSource::Catalog,
            added_at: 1,
        };
        let used = gen_files_used(std::slice::from_ref(&klein), &[], None);
        assert_eq!(
            used,
            [UsedFile {
                path: shared.display().to_string(),
                sha256: Some(old.clone())
            }]
        );
        let own = download_destination(&models, repo, file, commit, Some(&new), &used).unwrap();
        assert_eq!(
            own,
            models
                .join("unsloth")
                .join("Qwen3-4B-GGUF")
                .join("@0123456789ab")
                .join(file)
        );
        assert!(!used_elsewhere(
            &[],
            &BTreeSet::from([shared.display().to_string()]),
            &own
        ));
        // The same file, the model itself being downloaded again, or nothing using it: its
        // usual place.
        assert_eq!(
            download_destination(&models, repo, file, commit, Some(&old), &used).unwrap(),
            shared
        );
        assert!(gen_files_used(std::slice::from_ref(&klein), &[], Some(&klein.id)).is_empty());
        // 0.2.0 (Task S10): a LoRA's file counts too, with its SHA-256, whichever model is
        // downloaded again.
        let lora = store::GenLora {
            id: "local:cd".into(),
            name: "pixel art ü".into(),
            file: models
                .join("@loras")
                .join("local")
                .join("pixel art ü.safetensors")
                .display()
                .to_string(),
            rel_name: "local/pixel art ü".into(),
            repo: None,
            revision: None,
            path: None,
            size_bytes: 4,
            sha256: "c".repeat(64),
            base_hint: Some("sdxl".into()),
            format: store::LoraFileFormat::Kohya,
            source: store::LoraSource::Import,
            added_at: 1,
        };
        assert_eq!(
            gen_files_used(
                std::slice::from_ref(&klein),
                std::slice::from_ref(&lora),
                Some(&klein.id)
            ),
            [UsedFile {
                path: lora.file.clone(),
                sha256: Some("c".repeat(64))
            }]
        );
        assert_eq!(
            download_destination(&models, repo, file, commit, Some(&new), &[]).unwrap(),
            shared
        );
        // A commit that cannot name a folder is refused rather than written anywhere.
        assert!(download_destination(&models, repo, file, "../x", Some(&new), &used).is_err());

        // A chat model's file with the SHA-256 its row records; its image file with none.
        let image = shared.with_file_name("mmproj-F16.gguf");
        let mut chat = downloaded(1, &shared, Some(&image));
        chat.sha256 = Some(new.clone());
        let files = chat_files_used(std::slice::from_ref(&chat));
        assert!(files.contains(&UsedFile {
            path: shared.display().to_string(),
            sha256: Some(new.clone())
        }));
        assert!(files.contains(&UsedFile {
            path: image.display().to_string(),
            sha256: None
        }));
    }

    /// 0.1.0 Studio: FLUX.2 klein's text encoder is the same file as a common chat download
    /// (Qwen3-4B). Delete with files of that chat model keeps it while a picture or video model
    /// uses it, and the question's count leaves it out; once none does, it goes.
    #[tokio::test]
    async fn deleting_a_chat_model_keeps_a_file_a_picture_model_uses() {
        let gen_files =
            BTreeSet::from(["/m/unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf".to_string()]);
        assert!(used_elsewhere(
            &[],
            &gen_files,
            Path::new("/m/unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q4_K_M.gguf")
        ));
        assert!(!used_elsewhere(
            &[],
            &gen_files,
            Path::new("/m/unsloth/Qwen3-4B-GGUF/Qwen3-4B-Q8_0.gguf")
        ));

        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("unsloth").join("Qwen3-4B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let encoder = repo.join("Qwen3-4B-Q4_K_M.gguf");
        std::fs::write(&encoder, vec![0u8; 700]).unwrap();
        let chat = downloaded(1, &encoder, None);
        let gen_files = BTreeSet::from([encoder.display().to_string()]);
        let plan = removal_plan_with(&chat, &models, |f| used_elsewhere(&[], &gen_files, f));
        assert!(plan.delete.is_empty());
        assert_eq!(plan.in_use, vec![encoder.clone()]);
        assert_eq!(
            delete_preview(&chat, &models, |f| used_elsewhere(&[], &gen_files, f)),
            DeletePreview {
                in_use: 1,
                ..DeletePreview::default()
            }
        );
        // Another chat model using it still counts.
        let other = downloaded(2, &encoder, None);
        assert!(used_elsewhere(
            std::slice::from_ref(&other),
            &BTreeSet::new(),
            &encoder
        ));

        // Through Delete: FLUX.2 klein is installed, so the file stays.
        let store = store::Store::open_in_memory().unwrap();
        let klein = store::NewGenModel {
            id: "flux2-klein-4b-q4".into(),
            catalog_id: Some("flux2-klein-4b-q4".into()),
            name: "FLUX.2 klein 4B".into(),
            kind: "image".into(),
            family: "flux2-klein-4b".into(),
            recipe_id: Some("flux2-klein-4b".into()),
            parts: serde_json::json!([{"role": "llm", "file": encoder.display().to_string()}]),
            size_bytes: 700,
            source: store::GenSource::Catalog,
        };
        store.insert_gen_model(klein).await.unwrap();
        let none = || None::<std::future::Ready<()>>;
        let rec = on_the_list(&store, &chat).await;
        remove_installed(&store, &rec, true, &models, none())
            .await
            .unwrap();
        assert!(encoder.exists(), "klein still uses it");
        assert!(store.get_model(rec.id).await.unwrap().is_none());

        // Once no picture or video model uses it, it goes with the chat model.
        store.delete_gen_model("flux2-klein-4b-q4").await.unwrap();
        let rec = on_the_list(&store, &chat).await;
        remove_installed(&store, &rec, true, &models, none())
            .await
            .unwrap();
        assert!(!encoder.exists());
    }

    async fn on_the_list(store: &store::Store, rec: &store::ModelRecord) -> store::ModelRecord {
        store
            .upsert_model(ModelInput {
                path: rec.path.clone(),
                repo: rec.repo.clone(),
                file_name: rec.file_name.clone(),
                size_bytes: rec.size_bytes,
                sha256: rec.sha256.clone(),
                gguf_summary: rec.gguf_summary.clone(),
                source: rec.source,
            })
            .await
            .unwrap();
        store.model_by_path(&rec.path).await.unwrap().unwrap()
    }

    /// 0.0.7 (P6): a model always leaves the list. Its files go only after the model was
    /// unloaded; a model whose file is gone, or in a folder used before "Change folder", is
    /// taken off the list (before, Delete refused, after it had unloaded the model).
    #[tokio::test]
    async fn a_model_always_leaves_the_list_and_is_unloaded_before_its_files_go() {
        let root = scratch();
        let models = root.0.join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        let none = || None::<std::future::Ready<()>>;

        // Its file is gone.
        let gone = on_the_list(&store, &downloaded(0, &repo.join("Gone.gguf"), None)).await;
        assert_eq!(
            remove_installed(&store, &gone, true, &models, none())
                .await
                .unwrap(),
            None
        );
        assert!(store.get_model(gone.id).await.unwrap().is_none());

        // In an old folder: kept there, and said so.
        let old = root.0.join("old-models");
        std::fs::create_dir_all(&old).unwrap();
        let away_file = old.join("Old-Q4_K_M.gguf");
        std::fs::write(&away_file, b"gguf").unwrap();
        let away = on_the_list(&store, &downloaded(0, &away_file, None)).await;
        let note = remove_installed(&store, &away, true, &models, none())
            .await
            .unwrap();
        assert!(note.unwrap().contains("outside the models folder"));
        assert!(away_file.exists());
        assert!(store.get_model(away.id).await.unwrap().is_none());

        // Loaded: unloaded while its files are still there, then they go.
        let model = repo.join("Sample-7B-Q4_K_M.gguf");
        std::fs::write(&model, b"gguf").unwrap();
        let loaded = on_the_list(&store, &downloaded(0, &model, None)).await;
        let there_at_unload = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let seen = there_at_unload.clone();
        let file = model.clone();
        let unload = async move {
            seen.store(file.exists(), std::sync::atomic::Ordering::SeqCst);
        };
        remove_installed(&store, &loaded, true, &models, Some(unload))
            .await
            .unwrap();
        assert!(there_at_unload.load(std::sync::atomic::Ordering::SeqCst));
        assert!(!model.exists());
        assert!(store.get_model(loaded.id).await.unwrap().is_none());
    }
}
