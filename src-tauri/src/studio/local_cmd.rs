//! Get local models (studio-local-design.md, "Get local models"; 0.1.0 Task L16): Studio's
//! own view of the picture and video models this computer can run.
//!
//! - **The list** ([`local_models`]): what harnessUI detected, the Studio engine's state, the
//!   installed models, and the signed catalog's recommended list split into the entries that
//!   run here and those that do not ("Not for this computer", each with its reason), with
//!   "Recommended for this computer" marked once per kind at most. It sends nothing.
//! - **Hugging Face**, each only on a click: a search for picture or video repositories
//!   ([`local_search`]), and a repository's listing at its latest commit ([`local_repo`]),
//!   recognised here: each of its family's files with the parts it needs beside it, their
//!   licences and its fit, or why nothing is offered.
//! - **A fit** ([`local_fit`]) at a size and a length in seconds, with no request for a
//!   catalog or an installed model.
//! - **Download, delete and the note about local models**: `local_models`'
//!   [`start_local_download`] and [`delete_local_model`], and
//!   [`StudioManager::accept_local_notice`].
//!
//! Every fit is the local lane's verdict ([`local_verdict`]), so Create and Get local models
//! say the same; this module only words it. Each command is a thin wrapper over a plain
//! function taking what it reads ([`LocalCmdDeps`]), and checks that the main window called
//! it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use agent::media::MediaKind;
use models::fit::{RAM_RESERVE_BYTES, Target};
use models::gen_catalog::{
    Commercial, GateMode, GenEntry, GenGate, GenKind, GenLicence, GenRole, GenTiers,
};
use models::gen_eta::{self, FitWord};
use models::gen_fit::{GenFit, GenSize};
use models::gen_search::{Recognition, entry_for, recognise, searched_licence};
use models::hf::{Gated, GenSearchKind, HfClient, RepoDetail, RepoSummary};
use tauri::{AppHandle, Manager, State};

use super::deps::{AppHardware, LocalHardware, with_device_free};
use super::engine_cmd::{self, EngineNeed};
use super::errors;
use super::local::{
    AppGenCatalog, GenCatalogSource, default_frames, frames_for, longest_side, media_kind,
    media_model,
};
use super::local_models::{
    DownloadPlan, LocalSource, delete_local_model, entry_from_installed, entry_with, plan_download,
    start_local_download, word,
};
use super::manager::{LocalVerdict, StudioManager, device_label, eta_dto, local_verdict, word_tag};
use super::prefs::StudioPrefs;
use crate::commands::only_main_window;
use crate::dto::{
    EngineStatusDto, GateDto, HardwareLineDto, InstalledLocalDto, LicenceDto, LocalEntryDto,
    LocalFileChoiceDto, LocalFitDto, LocalModelsDto, LocalRepoDto, LocalSearchHitDto,
    LocalSearchPage, PartDto, media_model_dto,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// A billion bytes: the GB the catalog's tiers count in.
const GB: u64 = 1_000_000_000;

/// What Get local models reads: each part a test can stand in for.
pub struct LocalCmdDeps {
    /// The installed models, this computer's measured runs and the accepted licences.
    pub store: store::Store,
    /// The signed catalog's picture and video models.
    pub catalog: Arc<dyn GenCatalogSource>,
    /// This computer, as the local lane judges it.
    pub hardware: Arc<dyn LocalHardware>,
    /// Hugging Face, with the stored token; `None` where nothing is asked of it.
    pub hf: Option<HfClient>,
    /// Where model files go (`<models>/<owner>/<repo>/<path>`).
    pub models_dir: PathBuf,
    /// The free space on the models folder's disk, when the system says.
    pub free_disk_bytes: Option<u64>,
    /// The computer's memory, when the system says.
    pub ram_total_bytes: Option<u64>,
    /// The Studio engine's state, as `studio_engine_status` gives it.
    pub engine: EngineStatusDto,
    /// What a download needs of the Studio engine.
    pub engine_need: EngineNeed,
    /// The note about local models was accepted (the manager's held preferences).
    pub notice_accepted: bool,
}

// ---- the list ---------------------------------------------------------------------------------

/// Get local models: this computer, the engine, the installed models (newest first, each at
/// its default size), and the recommended list at each entry's default size, split by its fit
/// word (`notHere` into `not_here`). Per kind, the first entry of the highest tier at or below
/// this computer's usable memory whose word is `fits` is "Recommended for this computer".
/// Sends nothing.
pub async fn local_models(deps: &LocalCmdDeps) -> AppResult<LocalModelsDto> {
    let catalog = deps.catalog.gen_catalog().await;
    let (target, backend, description) = deps.hardware.target().await;
    // The device as this computer has it: the loaded chat model's share counted as free, as
    // the fit reasons and the speed class count it.
    let device = with_device_free(target, |free| {
        free.saturating_add(deps.hardware.chat_device_bytes().unwrap_or(0))
    });

    let rows = deps.store.list_gen_models().await?;
    let mut installed = Vec::with_capacity(rows.len());
    for row in &rows {
        let entry = match entry_from_installed(row, &catalog) {
            Ok(entry) => entry,
            Err(e) => {
                tracing::debug!(model = %row.id, "an installed model is not listed: {}", e.details);
                continue;
            }
        };
        let verdict = judge(deps, &entry, default_size(&entry)).await;
        installed.push(InstalledLocalDto {
            id: row.id.clone(),
            name: entry.name.clone(),
            kind: media_kind(entry.kind),
            size_bytes: row.size_bytes,
            fit: fit_dto(&entry, &verdict),
            model: media_model_dto(&media_model(row, &entry), None),
        });
    }
    let installed_ids: BTreeSet<&str> = rows
        .iter()
        .filter_map(|r| r.catalog_id.as_deref())
        .collect();

    let mut entries = Vec::with_capacity(catalog.entries.len());
    let mut words = BTreeMap::new();
    for entry in catalog.entries.values() {
        let verdict = judge(deps, entry, default_size(entry)).await;
        match entry_dto(
            deps,
            entry,
            &verdict,
            installed_ids.contains(entry.id.as_str()),
        ) {
            Ok(dto) => {
                words.insert(entry.id.clone(), verdict.word);
                entries.push(dto);
            }
            Err(e) => {
                tracing::warn!(model = %entry.id, "a recommended model is not listed: {}", e.details)
            }
        }
    }
    let marked = recommended_for(&catalog.tiers, &words, usable_gb(&device));
    let (recommended, not_here): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .map(|mut e| {
            e.recommended = marked.contains(&e.id);
            e
        })
        .partition(|e| words.get(&e.id) != Some(&FitWord::NotHere));
    // "N more fit this computer": the recommended entries not installed whose word is "Fits"
    // or "Fits after the chat model is unloaded" (the two words Studio's default local model
    // may have); one that is only "Slow here" does not count.
    let more_fit = recommended
        .iter()
        .filter(|e| {
            !e.installed && matches!(words.get(&e.id), Some(FitWord::Fits | FitWord::AfterUnload))
        })
        .count();
    Ok(LocalModelsDto {
        hardware: hardware_line(deps, &device, &backend, description.as_deref()),
        engine: deps.engine.clone(),
        installed,
        recommended,
        not_here,
        notice_accepted: deps.notice_accepted,
        video_possible: !matches!(device, Target::Cpu { .. }),
        installed_bytes: space_taken(&rows),
        more_fit: u32::try_from(more_fit).unwrap_or(u32::MAX),
    })
}

/// The header's line: the device by name, its memory for models (the chat model's share
/// counted as free), the computer's memory and the models folder's free space.
fn hardware_line(
    deps: &LocalCmdDeps,
    device: &Target,
    backend: &str,
    description: Option<&str>,
) -> HardwareLineDto {
    let (graphics_bytes, shared_memory) = match *device {
        Target::Gpu { free_bytes, .. } => (Some(free_bytes), false),
        Target::AppleUnified {
            working_set_bytes, ..
        } => (Some(working_set_bytes), true),
        Target::Cpu { .. } => (None, false),
    };
    HardwareLineDto {
        device: device_label(description, backend),
        graphics_bytes,
        shared_memory,
        ram_bytes: deps.ram_total_bytes,
        free_disk_bytes: deps.free_disk_bytes,
    }
}

/// This computer's usable memory in whole GB (rounded, as a speed class's), as the catalog's
/// tiers count it: a card's free memory or Apple silicon's working set, the chat model's share
/// counted as free; on the processor the free memory less the reserve kept for other programs.
fn usable_gb(device: &Target) -> u64 {
    let bytes = match *device {
        Target::Gpu { free_bytes, .. } => free_bytes,
        Target::AppleUnified {
            working_set_bytes, ..
        } => working_set_bytes,
        Target::Cpu { ram_free_bytes } => ram_free_bytes.saturating_sub(RAM_RESERVE_BYTES),
    };
    bytes.saturating_add(GB / 2) / GB
}

/// "Recommended for this computer": for pictures and for video, the first id of the highest
/// tier at or below `usable_gb` whose fit word is `fits`; none when no id there fits.
fn recommended_for(
    tiers: &GenTiers,
    words: &BTreeMap<String, FitWord>,
    usable_gb: u64,
) -> BTreeSet<String> {
    [&tiers.image, &tiers.video]
        .into_iter()
        .filter_map(|by_memory| {
            let (_, ids) = by_memory.range(..=usable_gb).next_back()?;
            ids.iter()
                .find(|id| words.get(*id) == Some(&FitWord::Fits))
                .cloned()
        })
        .collect()
}

/// The disk the installed picture and video models take: each part file once, however many
/// models use it.
fn space_taken(rows: &[store::GenModel]) -> u64 {
    let mut seen = BTreeSet::new();
    rows.iter()
        .flat_map(|row| row.parts.as_array().into_iter().flatten())
        .filter_map(|part| Some((part.get("file")?.as_str()?, part.get("size")?.as_u64()?)))
        .filter(|(file, _)| seen.insert(*file))
        .map(|(_, size)| size)
        .sum()
}

/// A recommended entry as the list shows it, with what its download takes here.
fn entry_dto(
    deps: &LocalCmdDeps,
    entry: &GenEntry,
    verdict: &LocalVerdict,
    installed: bool,
) -> AppResult<LocalEntryDto> {
    let plan = plan(deps, entry.clone())?;
    Ok(LocalEntryDto {
        id: entry.id.clone(),
        name: entry.name.clone(),
        summary: entry.summary.clone(),
        publisher: entry.publisher.clone(),
        kind: media_kind(entry.kind),
        size_bytes: plan.total_bytes,
        licence: licence_dto(entry.effective_licence()),
        licences: plan.licences.iter().map(licence_dto).collect(),
        gate: gate_dto(&entry.gate),
        fit: fit_dto(entry, verdict),
        recommended: false,
        installed,
        parts: parts_dto(&plan),
        to_fetch_bytes: plan.to_fetch_bytes,
        needs_engine: plan.needs_engine,
        low_disk: plan.low_disk(),
    })
}

// ---- a fit ------------------------------------------------------------------------------------

/// The fit of the model `source` names at `width` × `height` and, for a clip, `seconds` (0:
/// its default length), with this computer as it is now. A request only for a search source's
/// listing. A size the model is never made at is refused (`size_not_offered`).
pub async fn local_fit(
    deps: &LocalCmdDeps,
    source: &LocalSource,
    width: u32,
    height: u32,
    seconds: u32,
) -> AppResult<LocalFitDto> {
    let catalog = deps.catalog.gen_catalog().await;
    let entry = entry_with(&deps.store, &catalog, deps.hf.as_ref(), source).await?;
    let longest = longest_side(&entry);
    if width == 0 || height == 0 || width.max(height) > longest {
        return Err(errors::size_not_offered());
    }
    let verdict = judge(deps, &entry, fit_size(&entry, width, height, seconds)).await;
    Ok(fit_dto(&entry, &verdict))
}

/// The size a run of `entry` at `width` × `height` makes: a picture is one frame; a clip of
/// `seconds` has the frames its family's rule gives (`local::frames_for`), and with 0 seconds
/// its default length's.
fn fit_size(entry: &GenEntry, width: u32, height: u32, seconds: u32) -> GenSize {
    let frames = match (entry.kind, seconds) {
        (GenKind::Video, 0) => default_frames(entry),
        (GenKind::Video, seconds) => frames_for(entry, seconds),
        (GenKind::Image | GenKind::ImageEdit, _) => 1,
    };
    GenSize {
        width,
        height,
        frames,
    }
}

/// What a run of `entry` with nothing set makes.
fn default_size(entry: &GenEntry) -> GenSize {
    fit_size(entry, entry.defaults.width, entry.defaults.height, 0)
}

/// `entry` at `size`, its default steps and one picture: the local lane's verdict.
async fn judge(deps: &LocalCmdDeps, entry: &GenEntry, size: GenSize) -> LocalVerdict {
    local_verdict(
        entry,
        size,
        entry.defaults.steps,
        1,
        deps.hardware.as_ref(),
        &deps.store,
    )
    .await
}

/// A verdict in the webview's words: the fit word; the colour of the fit the run would have
/// (`gen_eta::running_fit`); the reason and a smaller size that fits; the time, rounded as
/// people read it; and whether Studio offers a cloud model instead.
fn fit_dto(entry: &GenEntry, verdict: &LocalVerdict) -> LocalFitDto {
    let colour = match gen_eta::running_fit(&verdict.fit).0 {
        GenFit::Green => "green",
        GenFit::OnePartAtATime | GenFit::PartlyOnProcessor => "yellow",
        GenFit::Red => "red",
    };
    LocalFitDto {
        word: word_tag(verdict.word).to_owned(),
        verdict: colour.to_owned(),
        reason: verdict.reason.clone(),
        suggestion: verdict
            .fit
            .suggestion
            .map(|size| errors::fits_at(size, entry.kind == GenKind::Video)),
        eta: verdict.eta.as_ref().map(|eta| eta_dto(eta, None)),
        very_slow: gen_eta::very_slow(verdict.word, verdict.eta.as_ref()),
    }
}

// ---- Hugging Face -----------------------------------------------------------------------------

/// A Hugging Face search for picture (`image`) or video repositories matching `text`
/// (`HfClient::search_generators`: its three queries, merged into one page).
pub async fn local_search(
    hf: &HfClient,
    text: &str,
    kind: MediaKind,
) -> AppResult<LocalSearchPage> {
    let kind = match kind {
        MediaKind::Image => GenSearchKind::Pictures,
        MediaKind::Video => GenSearchKind::Video,
    };
    let found = hf
        .search_generators(text, kind)
        .await
        .map_err(crate::hub::hf_error)?;
    Ok(search_page(found))
}

/// The search's repositories as hits, in their order.
fn search_page(found: Vec<RepoSummary>) -> LocalSearchPage {
    LocalSearchPage {
        items: found
            .into_iter()
            .map(|r| LocalSearchHitDto {
                gated: word(&r.gated),
                repo: r.id,
                downloads: r.downloads,
                likes: r.likes,
                license_label: r.license,
            })
            .collect(),
    }
}

/// Repository `repo`'s listing at its latest commit, as Studio recognises it ([`repo_dto`]).
pub async fn local_repo(deps: &LocalCmdDeps, repo: &str) -> AppResult<LocalRepoDto> {
    let hf = deps
        .hf
        .as_ref()
        .ok_or_else(|| AppError::internal("a repository listing without a Hub client"))?;
    let detail = hf.repo(repo, None).await.map_err(crate::hub::hf_error)?;
    Ok(repo_dto(deps, &detail).await)
}

/// What a repository's listing offers (`gen_search::recognise`): for a family harnessUI runs,
/// each of its diffusion files with a published SHA-256 as the entry it makes
/// (`gen_search::entry_for`: its family's parts beside it, the stricter of the family's
/// licence and the repository's label), what its download takes here and its fit at the
/// family's default size; else why not, with no file.
async fn repo_dto(deps: &LocalCmdDeps, detail: &RepoDetail) -> LocalRepoDto {
    let catalog = deps.catalog.gen_catalog().await;
    let mut dto = LocalRepoDto {
        repo: detail.id.clone(),
        commit: detail.commit.clone(),
        status: String::new(),
        reason: None,
        family: None,
        files: Vec::new(),
        licence: None,
        licences: Vec::new(),
        label_mismatch: None,
        gate: gate_dto(&GenGate {
            mode: match detail.gated {
                Gated::No => GateMode::None,
                Gated::Auto => GateMode::Auto,
                Gated::Manual => GateMode::Manual,
            },
            collects: Vec::new(),
        }),
    };
    let (family, files) = match recognise(detail, &catalog.families) {
        Recognition::Supported { family, files } => (family, files),
        Recognition::NotOffered(why) => {
            dto.status = "notOffered".into();
            dto.reason = Some(errors::not_offered_reason(why).to_owned());
            return dto;
        }
        Recognition::NotSupportedYet(why) => {
            dto.status = "notSupportedYet".into();
            dto.reason = Some(why);
            return dto;
        }
    };
    let Some(record) = catalog.families.get(&family) else {
        dto.status = "notSupportedYet".into();
        dto.reason = Some(errors::FAMILY_RECORD_MISSING.to_owned());
        return dto;
    };
    dto.status = "supported".into();
    dto.family = Some(record.name.clone());
    let searched = searched_licence(detail, record);
    dto.label_mismatch = searched.label_mismatch;
    // The badge's licence: the strictest of a file's parts, the same for every file (each is
    // under the searched licence beside the family's companions).
    let mut badge: Option<GenLicence> = None;
    for file in &files {
        let entry = match entry_for(detail, file, record) {
            Ok(entry) => entry,
            Err(why) => {
                tracing::debug!(file = %file.path, "a searched file is not offered: {why}");
                continue;
            }
        };
        let plan = match plan(deps, entry) {
            Ok(plan) => plan,
            Err(e) => {
                tracing::warn!(file = %file.path, "a searched file is not offered: {}", e.details);
                continue;
            }
        };
        let verdict = judge(deps, &plan.entry, default_size(&plan.entry)).await;
        badge.get_or_insert_with(|| plan.entry.effective_licence().clone());
        let licences: Vec<LicenceDto> = plan.licences.iter().map(licence_dto).collect();
        for licence in &licences {
            if !dto.licences.iter().any(|l| l.sha256 == licence.sha256) {
                dto.licences.push(licence.clone());
            }
        }
        dto.files.push(LocalFileChoiceDto {
            file: file.path.clone(),
            quant: models::files::quant_label(&file.path).unwrap_or_default(),
            parts: parts_dto(&plan),
            licences,
            total_bytes: plan.total_bytes,
            to_fetch_bytes: plan.to_fetch_bytes,
            fit: fit_dto(&plan.entry, &verdict),
            needs_engine: plan.needs_engine,
            low_disk: plan.low_disk(),
        });
    }
    dto.licence = Some(licence_dto(&badge.unwrap_or(searched.licence)));
    dto
}

// ---- the words of a download ------------------------------------------------------------------

/// What downloading `entry` takes here (`local_models::plan_download`), never refused for the
/// disk: a list says that with `low_disk`, and the download itself refuses it.
fn plan(deps: &LocalCmdDeps, entry: GenEntry) -> AppResult<DownloadPlan> {
    let mut plan = plan_download(entry, &deps.models_dir, &deps.engine_need, None)?;
    plan.free_bytes = deps.free_disk_bytes;
    Ok(plan)
}

/// Each part of a plan, as the download dialog lists it.
fn parts_dto(plan: &DownloadPlan) -> Vec<PartDto> {
    plan.entry
        .parts
        .iter()
        .zip(&plan.files)
        .map(|(part, (_, dest))| PartDto {
            role: word(&part.role),
            label: part_label(part.role).to_owned(),
            repo: part.repo.clone(),
            path: part.path.clone(),
            size_bytes: part.size,
            licence_id: part.license.clone(),
            already_here: plan.already_here.contains(dest),
        })
        .collect()
}

/// What a part does, in the download dialog's words.
fn part_label(role: GenRole) -> &'static str {
    match role {
        GenRole::Diffusion | GenRole::HighNoiseDiffusion | GenRole::Model => {
            errors::PART_MAKES_THE_PICTURE
        }
        GenRole::Llm | GenRole::T5xxl | GenRole::ClipL | GenRole::ClipVision => {
            errors::PART_READS_THE_PROMPT
        }
        GenRole::Vae => errors::PART_PICTURE_DECODER,
    }
}

fn licence_dto(licence: &GenLicence) -> LicenceDto {
    LicenceDto {
        id: licence.id.clone(),
        name: licence.name.clone(),
        url: licence.url.clone(),
        sha256: licence.sha256.clone(),
        badge: word(&licence.badge()),
        commercial: word(&licence.commercial_outputs),
        revenue_limit_usd: licence
            .revenue_limit_usd
            .filter(|_| licence.commercial_outputs == Commercial::UnderRevenue),
        plain_words: licence.plain_words.clone(),
        obligations: licence.obligations.clone(),
    }
}

fn gate_dto(gate: &GenGate) -> GateDto {
    GateDto {
        mode: word(&gate.mode),
        collects: gate.collects.clone(),
    }
}

// ---- commands ---------------------------------------------------------------------------------

/// What the commands read in the app: its store, the signed catalog, this computer, the
/// models folder and its disk, the engine and the note as the manager holds it; `hf` only for
/// a command that asks Hugging Face.
async fn app_deps(app: &AppHandle, studio: &StudioManager, hf: Option<HfClient>) -> LocalCmdDeps {
    let models_dir = crate::hub::models_dir(app).await;
    LocalCmdDeps {
        store: app.state::<AppState>().store.clone(),
        catalog: Arc::new(AppGenCatalog(app.clone())),
        hardware: Arc::new(AppHardware(app.clone())),
        hf,
        free_disk_bytes: reverse::diskspace::free_bytes(&models_dir),
        models_dir,
        ram_total_bytes: models::hardware::system_memory().map(|m| m.total_bytes),
        engine: engine_cmd::engine_status(app),
        engine_need: engine_cmd::engine_need(app),
        notice_accepted: studio.prefs().local_notice_accepted_at.is_some(),
    }
}

/// Get local models. Sends nothing.
#[tauri::command]
pub async fn studio_local_models(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<LocalModelsDto> {
    only_main_window(window.label())?;
    local_models(&app_deps(&app, &studio, None).await).await
}

/// Search (or Enter) in Get local models: Hugging Face's picture or video repositories
/// matching `text`, with the stored token, so gated repositories are found.
#[tauri::command]
pub async fn studio_local_search(
    window: tauri::WebviewWindow,
    app: AppHandle,
    text: String,
    kind: MediaKind,
) -> AppResult<LocalSearchPage> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    local_search(&hf, &text, kind).await
}

/// Opening a search result: repository `repo` at its latest commit, recognised here.
#[tauri::command]
pub async fn studio_local_repo(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    repo: String,
) -> AppResult<LocalRepoDto> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    local_repo(&app_deps(&app, &studio, Some(hf)).await, &repo).await
}

/// The fit of the model `source` names at `width` × `height` and, for a clip, `seconds` (0
/// for a picture). Sends nothing for a catalog or an installed model.
#[tauri::command]
pub async fn studio_local_fit(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    source: LocalSource,
    width: u32,
    height: u32,
    seconds: u32,
) -> AppResult<LocalFitDto> {
    only_main_window(window.label())?;
    let hf = match source {
        LocalSource::Search { .. } => Some(crate::hub::hf_client(&app).await?),
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } => None,
    };
    local_fit(
        &app_deps(&app, &studio, hf).await,
        &source,
        width,
        height,
        seconds,
    )
    .await
}

/// Downloads the model `source` names, once the note about local models and every licence
/// of the download are accepted (`accepted_license_sha256s` adds those accepted now); returns
/// the download group's id.
#[tauri::command]
pub async fn studio_local_download(
    window: tauri::WebviewWindow,
    app: AppHandle,
    source: LocalSource,
    accepted_license_sha256s: Vec<String>,
) -> AppResult<String> {
    only_main_window(window.label())?;
    start_local_download(&app, source, accepted_license_sha256s).await
}

/// Deletes the installed model `gen_model_id`, and each of its files nothing else uses.
#[tauri::command]
pub async fn studio_local_delete(
    window: tauri::WebviewWindow,
    app: AppHandle,
    gen_model_id: String,
) -> AppResult<()> {
    only_main_window(window.label())?;
    delete_local_model(&app, &gen_model_id).await
}

/// *I understand* on the note about local models: records when, and gives the preferences.
#[tauri::command]
pub async fn studio_accept_local_notice(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<StudioPrefs> {
    only_main_window(window.label())?;
    studio.accept_local_notice().await
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use agent::media::{Company, MediaKind, MediaLimits, MediaProvider, MediaRequest};
    use async_trait::async_trait;
    use jiff::tz::TimeZone;
    use models::catalog::Catalog;
    use models::download::destination;
    use models::fit::Target;
    use models::gen_catalog::{
        Commercial, FamilyRecord, FitFallback, FrameRule, GateMode, GenCatalog, GenDefaults,
        GenEntry, GenFamily, GenGate, GenKind, GenLicence, GenLimits, GenPart, GenRole, GenTiers,
        gen_catalog,
    };
    use models::gen_fit::GenSize;
    use models::hf::HfClient;
    use serde_json::{Map, Value, json};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use url::Url;

    use super::*;
    use crate::dto::{EngineStatusDto, LocalEntryDto, RunDto};
    use crate::error::AppResult;
    use crate::studio::deps::{
        Clock, Endpoint, ForeignFetch, LocalHardware, ProviderSource, StudioSink,
    };
    use crate::studio::engine_cmd::EngineNeed;
    use crate::studio::local::{GenCatalogSource, run_shape};
    use crate::studio::local_models::LocalSource;
    use crate::studio::manager::{PollPlan, StudioDeps, StudioManager};
    use crate::studio::prefs::{self, StudioPrefs, StudioPrefsPatch};
    use crate::studio::spend::day_start_in;
    use crate::studio::test_support::Scratch;

    const GB: u64 = 1_000_000_000;
    const KLEIN: &str = "flux2-klein-4b-q4";
    const ZIMAGE: &str = "z-image-turbo-q4";
    const WAN22: &str = "wan22-ti2v-5b-fast-q6";
    /// The test card as llama.cpp describes it (Mesa's driver code for the chip at the end).
    const CARD: &str = "AMD Radeon RX 7600 (RADV NAVI33)";

    // ---- the catalog: entries with stated part sizes and fallbacks ------------------------
    //
    // As in `gen_fit`'s tests: the pinned files' sizes, and the families' fallbacks written
    // as literals, so each fit expectation follows from the formulas and a re-tuned or longer
    // bundled catalog (Task L17) never moves it.

    fn bundled() -> GenCatalog {
        gen_catalog(
            &Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        )
    }

    /// The bundled Apache-2.0 licence, with the SHA-256 of its canonical text.
    fn apache() -> GenLicence {
        bundled().entries[KLEIN].licenses["apache-2.0"].clone()
    }

    fn picture_fallback() -> FitFallback {
        FitFallback {
            act_base_bytes: 500_000_000,
            act_bytes_per_token: 100_000,
            vae_bytes_per_pixel_frame: 300,
            vae_tile_cap_bytes: 500_000_000,
            token_div: 16,
            frame_div: None,
            vae_chunk_frames: None,
        }
    }

    fn wan22_fallback() -> FitFallback {
        FitFallback {
            act_base_bytes: 1_000_000_000,
            act_bytes_per_token: 250_000,
            vae_bytes_per_pixel_frame: 400,
            vae_tile_cap_bytes: 2_500_000_000,
            token_div: 32,
            frame_div: Some(4),
            vae_chunk_frames: Some(4),
        }
    }

    /// An entry of `kind` with these parts (each in a repository of its own, under
    /// Apache-2.0), made at `default` by default, with no measured point.
    fn entry(id: &str, kind: GenKind, family: GenFamily, parts: [(GenRole, u64); 3]) -> GenEntry {
        let video = kind == GenKind::Video;
        let (width, height, frames) = if video {
            (832, 480, 49)
        } else {
            (1024, 1024, 1)
        };
        GenEntry {
            id: id.into(),
            name: id.into(),
            summary: format!("{id}, for the test"),
            publisher: "Test Labs".into(),
            kind,
            family,
            inputs: Vec::new(),
            parts: parts
                .into_iter()
                .map(|(role, size)| GenPart {
                    role,
                    repo: format!("test/{id}"),
                    revision: "0".repeat(40),
                    path: format!("{role:?}.gguf"),
                    size,
                    sha256: "ab".repeat(32),
                    license: "apache-2.0".into(),
                })
                .collect(),
            licenses: BTreeMap::from([("apache-2.0".to_owned(), apache())]),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
            defaults: GenDefaults {
                steps: 4,
                cfg: 1.0,
                flow_shift: None,
                sampler: None,
                scheduler: None,
                width,
                height,
                frames: video.then_some(frames),
                fps: video.then_some(24),
            },
            limits: GenLimits {
                multiple_of: if video { 32 } else { 16 },
                max_side: if video { 1280 } else { 2048 },
                frame_rule: video.then_some(FrameRule::FourNPlusOne),
                max_frames: video.then_some(121),
            },
            fallback: if video {
                wan22_fallback()
            } else {
                picture_fallback()
            },
            memory: Vec::new(),
            speed: Vec::new(),
            quirks: Vec::new(),
            min_build: 1106,
            runs_on_apple: true,
            tested: None,
        }
    }

    /// FLUX.2 klein 4B: D 2.46, T 2.50, V 0.34.
    fn klein() -> GenEntry {
        entry(
            KLEIN,
            GenKind::Image,
            GenFamily::Flux2Klein4b,
            [
                (GenRole::Diffusion, 2_460_378_560),
                (GenRole::Llm, 2_497_281_312),
                (GenRole::Vae, 336_211_292),
            ],
        )
    }

    /// Z-Image Turbo: D 3.86, T 2.50, V 0.34.
    fn zimage() -> GenEntry {
        entry(
            ZIMAGE,
            GenKind::Image,
            GenFamily::ZImageTurbo,
            [
                (GenRole::Diffusion, 3_864_250_304),
                (GenRole::Llm, 2_497_281_120),
                (GenRole::Vae, 335_304_388),
            ],
        )
    }

    /// Wan 2.2 TI2V-5B fast: D 4.21, T 4.15, V 1.41.
    fn wan22() -> GenEntry {
        entry(
            WAN22,
            GenKind::Video,
            GenFamily::Wan22Ti2v5bFast,
            [
                (GenRole::Diffusion, 4_210_247_200),
                (GenRole::T5xxl, 4_145_878_880),
                (GenRole::Vae, 1_409_400_960),
            ],
        )
    }

    /// The bundled family records (what a searched repository is recognised by), with no
    /// measured point, the fallbacks above, and companions of a few kilobytes so that a test
    /// can put one in place.
    fn families() -> BTreeMap<GenFamily, FamilyRecord> {
        let mut families = bundled().families;
        for record in families.values_mut() {
            record.memory.clear();
            record.speed.clear();
            record.fallback = if record.kind == GenKind::Video {
                wan22_fallback()
            } else {
                picture_fallback()
            };
            for (part, size) in record.companions.iter_mut().zip([2_000, 300, 100]) {
                part.size = size;
            }
        }
        families
    }

    /// The three starters, recommended as the bundled catalog's tiers recommend them.
    fn catalog_of(entries: Vec<GenEntry>) -> GenCatalog {
        GenCatalog {
            entries: entries.into_iter().map(|e| (e.id.clone(), e)).collect(),
            families: families(),
            tiers: GenTiers {
                image: BTreeMap::from([
                    (0, vec![KLEIN.to_owned()]),
                    (8, vec![ZIMAGE.to_owned(), KLEIN.to_owned()]),
                ]),
                video: BTreeMap::from([(8, vec![WAN22.to_owned()])]),
            },
            hidden: Vec::new(),
        }
    }

    fn catalog() -> GenCatalog {
        catalog_of(vec![klein(), zimage(), wan22()])
    }

    struct TestCatalog(GenCatalog);

    #[async_trait]
    impl GenCatalogSource for TestCatalog {
        async fn gen_catalog(&self) -> GenCatalog {
            self.0.clone()
        }
    }

    // ---- this computer --------------------------------------------------------------------

    struct Fixed {
        target: Target,
        backend: &'static str,
        chat: Option<u64>,
    }

    /// A Vulkan card with `free_gb` GB free, and `ram_gb` GB of free memory.
    fn gpu(free_gb: u64, ram_gb: u64) -> Fixed {
        Fixed {
            target: Target::Gpu {
                name: CARD.into(),
                free_bytes: free_gb * GB,
                ram_free_bytes: Some(ram_gb * GB),
            },
            backend: "vulkan",
            chat: None,
        }
    }

    /// A computer that runs models on the processor, with `ram_gb` GB of free memory.
    fn cpu(ram_gb: u64) -> Fixed {
        Fixed {
            target: Target::Cpu {
                ram_free_bytes: ram_gb * GB,
            },
            backend: "cpu",
            chat: None,
        }
    }

    impl Fixed {
        /// A chat model loaded, taking `gb` GB of the card.
        fn chat(mut self, gb: u64) -> Self {
            self.chat = Some(gb * GB);
            self
        }
    }

    #[async_trait]
    impl LocalHardware for Fixed {
        async fn target(&self) -> (Target, String, Option<String>) {
            let description = (self.backend != "cpu").then(|| CARD.to_owned());
            (self.target.clone(), self.backend.to_owned(), description)
        }

        fn chat_device_bytes(&self) -> Option<u64> {
            self.chat
        }

        fn logical_cores(&self) -> usize {
            8
        }
    }

    // ---- Hugging Face, served here --------------------------------------------------------

    /// A Hub on this computer, reached over plain HTTP by [`Hub::client`]
    /// (`HfClient::for_test`): it answers each request with what `answer` gives for its
    /// request line and keeps the lines, so a test sees what was asked, or that nothing was.
    struct Hub {
        endpoint: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl Hub {
        async fn new(
            answer: impl Fn(&str) -> (&'static str, String) + Send + Sync + 'static,
        ) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            let seen = Arc::new(Mutex::new(Vec::new()));
            let lines = seen.clone();
            let answer = Arc::new(answer);
            tokio::spawn(async move {
                while let Ok((socket, _)) = listener.accept().await {
                    let (lines, answer) = (lines.clone(), answer.clone());
                    tokio::spawn(async move {
                        let (read, mut write) = socket.into_split();
                        let mut read = BufReader::new(read);
                        let mut line = String::new();
                        if read.read_line(&mut line).await.unwrap_or(0) == 0 {
                            return;
                        }
                        // The rest of the head (a GET has no body).
                        loop {
                            let mut header = String::new();
                            match read.read_line(&mut header).await {
                                Ok(n) if n > 0 && !header.trim_end().is_empty() => {}
                                _ => break,
                            }
                        }
                        let line = line.trim_end().to_owned();
                        let (status, body) = answer(&line);
                        lines.lock().unwrap().push(line);
                        let reply = format!(
                            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = write.write_all(reply.as_bytes()).await;
                        let _ = write.shutdown().await;
                    });
                }
            });
            Self { endpoint, seen }
        }

        /// A Hub that has nothing: for the tests that must ask it nothing.
        async fn empty() -> Self {
            Self::new(|_| ("404 Not Found", "{}".into())).await
        }

        /// A Hub that answers repository `fixture`'s listing (a file in
        /// crates/models/tests/fixtures/hf/) and nothing else.
        async fn listing(fixture: &str) -> Self {
            let text = match fixture {
                "repo-unknown-diffusion.json" => include_str!(
                    "../../../crates/models/tests/fixtures/hf/repo-unknown-diffusion.json"
                ),
                "repo-z-image-turbo-gguf.json" => include_str!(
                    "../../../crates/models/tests/fixtures/hf/repo-z-image-turbo-gguf.json"
                ),
                "repo-uncensored.json" => {
                    include_str!("../../../crates/models/tests/fixtures/hf/repo-uncensored.json")
                }
                other => panic!("no listing fixture {other}"),
            };
            let repo = serde_json::from_str::<Value>(text).unwrap()["id"]
                .as_str()
                .unwrap()
                .to_owned();
            let asked = format!("GET /api/models/{repo}?");
            Self::new(move |line| {
                if line.starts_with(&asked) {
                    ("200 OK", text.to_owned())
                } else {
                    ("404 Not Found", "{}".into())
                }
            })
            .await
        }

        /// A Hub client whose every request comes here.
        fn client(&self) -> HfClient {
            HfClient::for_test(&self.endpoint)
        }

        /// The request lines, in the order they came.
        fn requests(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }
    }

    // ---- the dependencies -----------------------------------------------------------------

    fn engine_built_in() -> EngineStatusDto {
        EngineStatusDto {
            state: "builtIn".into(),
            build: "master-1106-532d2c1".into(),
            download_bytes: 0,
            license_name: "MIT".into(),
            license_url: "https://example.com/licence".into(),
            bytes_done: None,
        }
    }

    /// Get local models' dependencies on `hardware`, with `catalog`, an empty store, a models
    /// folder of its own (in `scratch`), 500 GB free, 32 GB of memory and the engine built in.
    fn deps_with(
        catalog: GenCatalog,
        hardware: Fixed,
        scratch: &Scratch,
        hf: Option<HfClient>,
    ) -> LocalCmdDeps {
        LocalCmdDeps {
            store: store::Store::open_in_memory().unwrap(),
            catalog: Arc::new(TestCatalog(catalog)),
            hardware: Arc::new(hardware),
            hf,
            models_dir: scratch.path().join("models"),
            free_disk_bytes: Some(500 * GB),
            ram_total_bytes: Some(32 * GB),
            engine: engine_built_in(),
            engine_need: EngineNeed::Found,
            notice_accepted: false,
        }
    }

    fn deps(hardware: Fixed, scratch: &Scratch) -> LocalCmdDeps {
        deps_with(catalog(), hardware, scratch, None)
    }

    fn ids(entries: &[LocalEntryDto]) -> Vec<&str> {
        entries.iter().map(|e| e.id.as_str()).collect()
    }

    fn find<'a>(entries: &'a [LocalEntryDto], id: &str) -> &'a LocalEntryDto {
        entries
            .iter()
            .find(|e| e.id == id)
            .unwrap_or_else(|| panic!("{id} is not in {:?}", ids(entries)))
    }

    // ---- Get local models -----------------------------------------------------------------

    /// Opening Get local models sends nothing: it reads the catalog, the store and this
    /// computer.
    #[tokio::test]
    async fn opening_get_local_models_makes_no_request() {
        let scratch = Scratch::new();
        let hub = Hub::empty().await;
        let deps = deps_with(catalog(), gpu(8, 32), &scratch, Some(hub.client()));
        install_searched(&deps.store, &deps.models_dir).await;
        let local = local_models(&deps).await.unwrap();
        assert_eq!(local.installed.len(), 1);
        assert!(hub.requests().is_empty(), "{:?}", hub.requests());
    }

    /// On an 8 GB card the three starters all fit: one picture model and one video model are
    /// recommended, and all three are more that fit.
    #[tokio::test]
    async fn the_recommended_list_is_the_starters_with_one_marked_per_kind() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(8, 32), &scratch)).await.unwrap();
        let mut listed = ids(&local.recommended);
        listed.sort_unstable();
        assert_eq!(listed, [KLEIN, WAN22, ZIMAGE]);
        assert!(local.not_here.is_empty());
        let marked: Vec<(&str, MediaKind)> = local
            .recommended
            .iter()
            .filter(|e| e.recommended)
            .map(|e| (e.id.as_str(), e.kind))
            .collect();
        // The 8 GB tier's first picture model that fits, and its video model.
        assert_eq!(marked.len(), 2, "{marked:?}");
        assert!(marked.contains(&(ZIMAGE, MediaKind::Image)), "{marked:?}");
        assert!(marked.contains(&(WAN22, MediaKind::Video)), "{marked:?}");
        assert_eq!(local.more_fit, 3);
        assert!(local.video_possible);
        for e in &local.recommended {
            assert_eq!(e.fit.word, "fits", "{}", e.id);
            assert!(!e.installed && !e.needs_engine && !e.low_disk, "{}", e.id);
            assert_eq!(e.to_fetch_bytes, e.size_bytes, "{}", e.id);
            assert_eq!(e.parts.len(), 3, "{}", e.id);
        }
    }

    /// The bundled catalog, whatever it lists: one picture and one video model recommended
    /// on an 8 GB card, and every entry that fits counted as more.
    #[tokio::test]
    async fn the_bundled_starters_are_recommended_one_per_kind() {
        let scratch = Scratch::new();
        let catalog = bundled();
        let mut expected: Vec<String> = catalog.entries.keys().cloned().collect();
        let local = local_models(&deps_with(catalog, gpu(8, 32), &scratch, None))
            .await
            .unwrap();
        let mut listed: Vec<String> = local.recommended.iter().map(|e| e.id.clone()).collect();
        listed.sort_unstable();
        expected.sort_unstable();
        assert_eq!(listed, expected);
        for kind in [MediaKind::Image, MediaKind::Video] {
            let marked = local
                .recommended
                .iter()
                .filter(|e| e.recommended && e.kind == kind)
                .count();
            assert_eq!(marked, 1, "{kind:?}");
        }
        assert_eq!(local.more_fit as usize, expected.len());
    }

    /// On a 2 GB card with 4 GB of memory, Z-Image Turbo is not for this computer, with its
    /// reason; klein runs partly on the processor, slowly.
    #[tokio::test]
    async fn models_that_do_not_fit_are_listed_with_a_plain_reason() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(2, 4), &scratch)).await.unwrap();
        let z = find(&local.not_here, ZIMAGE);
        assert_eq!(z.fit.word, "notHere");
        assert_eq!(z.fit.verdict, "red");
        assert_eq!(
            z.fit.reason.as_deref(),
            Some("needs 6 GB of graphics memory")
        );
        assert!(z.fit.very_slow);
        assert!(!z.recommended);
        let klein = find(&local.recommended, KLEIN);
        assert_eq!(klein.fit.word, "slow");
        assert_eq!(klein.fit.verdict, "yellow");
        assert_eq!(klein.fit.reason, None);
        // Nothing fits well enough to be recommended here, and a model that is only "Slow
        // here" is not one more that fits.
        assert!(local.recommended.iter().all(|e| !e.recommended));
        assert_eq!(local.more_fit, 0);
    }

    /// With a 5 GB chat model on an 8 GB card (3 GB free), Z-Image Turbo fits once the chat
    /// model is unloaded: it says so, it is not "not for this computer", and the header
    /// counts the chat model's memory as the card's.
    #[tokio::test]
    async fn a_model_that_fits_after_the_chat_model_is_unloaded_says_so() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(3, 32).chat(5), &scratch))
            .await
            .unwrap();
        let z = find(&local.recommended, ZIMAGE);
        assert_eq!(z.fit.word, "afterUnload");
        assert_eq!(z.fit.verdict, "yellow");
        assert!(!local.not_here.iter().any(|e| e.id == ZIMAGE));
        assert_eq!(local.hardware.graphics_bytes, Some(8 * GB));
        // Each starter fits once the chat model is unloaded, so each is one more that fits.
        assert!(
            local
                .recommended
                .iter()
                .all(|e| e.fit.word == "afterUnload")
        );
        assert_eq!(local.more_fit, 3);
    }

    /// What harnessUI detected, for the header.
    #[tokio::test]
    async fn the_header_says_what_this_computer_has() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(8, 32), &scratch)).await.unwrap();
        assert_eq!(
            local.hardware,
            HardwareLineDto {
                device: "AMD Radeon RX 7600".into(),
                graphics_bytes: Some(8 * GB),
                shared_memory: false,
                ram_bytes: Some(32 * GB),
                free_disk_bytes: Some(500 * GB),
            }
        );
        assert_eq!(local.engine, engine_built_in());
        assert!(!local.notice_accepted);
        let local = local_models(&deps(cpu(16), &scratch)).await.unwrap();
        assert_eq!(local.hardware.device, "Processor");
        assert_eq!(local.hardware.graphics_bytes, None);
    }

    /// A licence for commercial use under a revenue limit.
    fn kommunity() -> GenLicence {
        GenLicence {
            id: "kommunity".into(),
            name: "Example Community Licence".into(),
            url: "https://example.com/kommunity".into(),
            sha256: "cd".repeat(32),
            commercial_outputs: Commercial::UnderRevenue,
            revenue_limit_usd: Some(5_000_000),
            excluded_regions: Vec::new(),
            obligations: vec!["Review pictures before you share them.".into()],
            plain_words: "Commercial use only under $5M revenue.".into(),
        }
    }

    /// The strictest licence decides the badge, and every licence of every part is listed
    /// for acceptance, each with its SHA-256; the Studio engine's too when it comes with the
    /// model.
    #[tokio::test]
    async fn every_licence_of_a_model_is_listed_for_acceptance() {
        let scratch = Scratch::new();
        let mut two = klein();
        two.parts[1].license = "kommunity".into();
        two.licenses.insert("kommunity".into(), kommunity());
        let mut deps = deps_with(
            catalog_of(vec![two, zimage(), wan22()]),
            gpu(8, 32),
            &scratch,
            None,
        );
        let local = local_models(&deps).await.unwrap();
        let e = find(&local.recommended, KLEIN);
        assert_eq!(e.licence.id, "kommunity");
        assert_eq!(e.licence.badge, "amber");
        assert_eq!(e.licence.commercial, "underRevenue");
        assert_eq!(e.licence.revenue_limit_usd, Some(5_000_000));
        assert_eq!(
            e.licences
                .iter()
                .map(|l| (l.id.as_str(), l.sha256.clone()))
                .collect::<Vec<_>>(),
            [
                ("apache-2.0", apache().sha256),
                ("kommunity", "cd".repeat(32))
            ]
        );
        assert_eq!(
            e.parts
                .iter()
                .map(|p| p.licence_id.as_str())
                .collect::<Vec<_>>(),
            ["apache-2.0", "kommunity", "apache-2.0"]
        );
        let z = find(&local.recommended, ZIMAGE);
        assert_eq!(z.licence.badge, "green");
        assert_eq!(z.licence.commercial, "yes");

        // Where the engine comes with the first model, its licence is accepted with it.
        let engine = GenLicence {
            id: crate::studio::engine_cmd::ENGINE_LICENCE_ID.into(),
            sha256: "ef".repeat(32),
            ..apache()
        };
        deps.engine_need = EngineNeed::Pack(engine);
        let local = local_models(&deps).await.unwrap();
        let e = find(&local.recommended, KLEIN);
        assert!(e.needs_engine);
        assert_eq!(
            e.licences.iter().map(|l| l.id.as_str()).collect::<Vec<_>>(),
            ["apache-2.0", "kommunity", "studio-engine"]
        );
    }

    /// Installs a searched Z-Image Turbo file (from someone's repository) with the test
    /// family's companions; returns its id.
    async fn install_searched(store: &store::Store, models: &Path) -> String {
        let id = "hf:someone/Z-Image-Turbo-GGUF:z.gguf";
        let record = &families()[&GenFamily::ZImageTurbo];
        let diffusion = GenPart {
            role: GenRole::Diffusion,
            repo: "someone/Z-Image-Turbo-GGUF".into(),
            revision: "c".repeat(40),
            path: "z.gguf".into(),
            size: 3_864_250_304,
            sha256: "d".repeat(64),
            license: "apache-2.0".into(),
        };
        let parts: Vec<&GenPart> = std::iter::once(&diffusion)
            .chain(&record.companions)
            .collect();
        install(
            store,
            models,
            id,
            None,
            "Z-Image Turbo (z)",
            "z-image-turbo",
            &parts,
        )
        .await;
        id.to_owned()
    }

    /// Installs catalog entry `entry`.
    async fn install_starter(store: &store::Store, entry: &GenEntry) {
        let parts: Vec<&GenPart> = entry.parts.iter().collect();
        let family = serde_json::to_value(entry.family).unwrap();
        install(
            store,
            Path::new("models"),
            &entry.id,
            Some(&entry.id),
            &entry.name,
            family.as_str().unwrap(),
            &parts,
        )
        .await;
    }

    async fn install(
        store: &store::Store,
        models: &Path,
        id: &str,
        catalog_id: Option<&str>,
        name: &str,
        family: &str,
        parts: &[&GenPart],
    ) {
        let saved: Vec<Value> = parts
            .iter()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap();
                let file: PathBuf = destination(models, &p.repo, &p.path).unwrap();
                v["file"] = json!(file.display().to_string());
                v
            })
            .collect();
        store
            .insert_gen_model(store::NewGenModel {
                id: id.into(),
                catalog_id: catalog_id.map(str::to_owned),
                name: name.into(),
                kind: "image".into(),
                family: family.into(),
                parts: json!(saved),
                size_bytes: parts.iter().map(|p| p.size).sum(),
                source: if catalog_id.is_some() {
                    store::GenSource::Catalog
                } else {
                    store::GenSource::Search
                },
            })
            .await
            .unwrap();
    }

    /// An installed starter is listed among the installed models and as installed in the
    /// recommended list, and is no longer counted as more that fit.
    #[tokio::test]
    async fn an_installed_starter_is_marked_and_not_counted_as_more() {
        let scratch = Scratch::new();
        let deps = deps(gpu(8, 32), &scratch);
        install_starter(&deps.store, &klein()).await;
        let local = local_models(&deps).await.unwrap();
        assert!(find(&local.recommended, KLEIN).installed);
        assert!(!find(&local.recommended, ZIMAGE).installed);
        assert_eq!(local.more_fit, 2);
        assert_eq!(local.installed.len(), 1);
        let installed = &local.installed[0];
        assert_eq!(installed.id, KLEIN);
        assert_eq!(installed.kind, MediaKind::Image);
        assert_eq!(installed.fit.word, "fits");
        assert_eq!(installed.model.price.unit, "free");
        assert_eq!(installed.size_bytes, klein().total_bytes());
        assert_eq!(local.installed_bytes, klein().total_bytes());
    }

    /// Two installed models sharing a file take its space once.
    #[tokio::test]
    async fn a_shared_file_is_counted_once_in_the_space_taken() {
        let scratch = Scratch::new();
        let deps = deps(gpu(8, 32), &scratch);
        let mut other = klein();
        other.id = "klein-again".into();
        other.parts[0].path = "Other.gguf".into();
        install_starter(&deps.store, &klein()).await;
        install_starter(&deps.store, &other).await;
        let local = local_models(&deps).await.unwrap();
        assert_eq!(
            local.installed_bytes,
            klein().total_bytes() + other.parts[0].size
        );
    }

    /// An installed searched model's fit comes from its saved parts and its family: no
    /// request.
    #[tokio::test]
    async fn an_installed_model_gets_its_fit_without_a_request() {
        let scratch = Scratch::new();
        let hub = Hub::empty().await;
        let deps = deps_with(catalog(), gpu(8, 32), &scratch, Some(hub.client()));
        let id = install_searched(&deps.store, &deps.models_dir).await;
        let source: LocalSource =
            serde_json::from_value(json!({"installed": {"genModelId": id}})).unwrap();
        let fit = local_fit(&deps, &source, 832, 480, 0).await.unwrap();
        // 3.86 GB of model and a few kilobytes of companions (the test family's): whole on
        // the card at 832×480.
        assert_eq!(fit.word, "fits");
        assert_eq!(fit.verdict, "green");
        assert_eq!(fit.reason, None);
        assert_eq!(fit.eta, None, "nothing measured, no first guess");
        assert!(hub.requests().is_empty(), "{:?}", hub.requests());
        // A catalog entry's fit needs no request either.
        let source = LocalSource::Catalog {
            entry_id: KLEIN.into(),
        };
        assert_eq!(
            local_fit(&deps, &source, 1024, 1024, 0)
                .await
                .unwrap()
                .verdict,
            "green"
        );
        assert!(hub.requests().is_empty(), "{:?}", hub.requests());
    }

    /// A clip's fit is asked in seconds and judged at the frames the run would make.
    #[tokio::test]
    async fn a_clips_fit_follows_its_length() {
        let scratch = Scratch::new();
        let deps = deps(gpu(8, 32), &scratch);
        let source = LocalSource::Catalog {
            entry_id: WAN22.into(),
        };
        // 2 s is the 49 frames the entry makes by default: one part at a time on 8 GB.
        let two = local_fit(&deps, &source, 832, 480, 2).await.unwrap();
        assert_eq!(two, local_fit(&deps, &source, 832, 480, 0).await.unwrap());
        assert_eq!(two.word, "fits");
        // 5 s at 720p (121 frames) does not run on the card: a smaller preset is suggested.
        let five = local_fit(&deps, &source, 1280, 704, 5).await.unwrap();
        assert_ne!(five.word, "fits");
        assert!(
            five.suggestion
                .as_deref()
                .is_some_and(|s| s.starts_with("Fits at ")),
            "{five:?}"
        );
        // A size of nothing is refused.
        assert_eq!(
            local_fit(&deps, &source, 0, 480, 2).await.unwrap_err().code,
            "bad_request"
        );
    }

    /// A model's default size is what a run with nothing set makes.
    #[test]
    fn the_default_size_is_what_a_run_with_nothing_set_makes() {
        for entry in bundled().entries.values().chain([&klein(), &wan22()]) {
            let request = MediaRequest {
                kind: if entry.kind == GenKind::Video {
                    MediaKind::Video
                } else {
                    MediaKind::Image
                },
                model: entry.id.clone(),
                prompt: "a fox".into(),
                params: Map::new(),
                extra: Map::new(),
                refs: Vec::new(),
                routing: None,
                provider_options: BTreeMap::new(),
            };
            let shape = run_shape(entry, &request).unwrap();
            assert_eq!(
                fit_size(entry, shape.size.width, shape.size.height, 0),
                shape.size
            );
        }
        assert_eq!(
            fit_size(&wan22(), 832, 480, 2),
            GenSize {
                width: 832,
                height: 480,
                frames: 49
            }
        );
        assert_eq!(fit_size(&klein(), 512, 512, 7).frames, 1);
    }

    /// On a computer that runs models on the processor, video is not offered.
    #[tokio::test]
    async fn video_is_hidden_on_a_processor_only_computer() {
        let scratch = Scratch::new();
        let local = local_models(&deps(cpu(16), &scratch)).await.unwrap();
        assert!(!local.video_possible);
        let wan = find(&local.not_here, WAN22);
        assert_eq!(
            wan.fit.reason.as_deref(),
            Some("video needs a graphics card or an Apple-silicon Mac")
        );
        // Pictures run there, slowly: listed, but none counted as more that fit.
        assert_eq!(find(&local.recommended, KLEIN).fit.word, "slow");
        assert_eq!(local.more_fit, 0);
    }

    // ---- a searched repository ------------------------------------------------------------

    /// A repository of a family harnessUI does not run is "Not supported yet", with no file
    /// to download.
    #[tokio::test]
    async fn a_searched_repo_of_an_unknown_family_offers_no_download() {
        let scratch = Scratch::new();
        let hub = Hub::listing("repo-unknown-diffusion.json").await;
        let deps = deps_with(catalog(), gpu(8, 32), &scratch, Some(hub.client()));
        let repo = local_repo(&deps, "example-labs/Sample-Diffusion-2B-GGUF")
            .await
            .unwrap();
        // One request: the listing at the latest commit (no revision in the address).
        let asked = hub.requests();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(
            asked[0].starts_with("GET /api/models/example-labs/Sample-Diffusion-2B-GGUF?"),
            "{asked:?}"
        );
        assert_eq!(repo.repo, "example-labs/Sample-Diffusion-2B-GGUF");
        assert_eq!(repo.commit, "2".repeat(40));
        assert_eq!(repo.status, "notSupportedYet");
        assert!(repo.reason.as_deref().is_some_and(|r| !r.is_empty()));
        assert!(repo.files.is_empty());
        assert!(repo.licences.is_empty());
        assert_eq!((repo.family, repo.licence), (None, None));
    }

    /// A repository Studio never offers says why in the clause `studio_not_offered` uses.
    #[tokio::test]
    async fn a_searched_repo_never_offered_says_why() {
        let scratch = Scratch::new();
        let hub = Hub::listing("repo-uncensored.json").await;
        let deps = deps_with(catalog(), gpu(8, 32), &scratch, Some(hub.client()));
        let repo = local_repo(&deps, "example-quants/Z-Image-Turbo-Uncensored-GGUF")
            .await
            .unwrap();
        assert_eq!(hub.requests().len(), 1);
        assert_eq!(repo.status, "notOffered");
        assert_eq!(
            repo.reason.as_deref(),
            Some("it was made without safety limits")
        );
        assert!(repo.files.is_empty());
    }

    /// Each diffusion file of a recognised repository is offered with its family's parts; a
    /// part already in place (a shared text encoder) is checked, not fetched.
    #[tokio::test]
    async fn a_searched_repo_lists_each_file_with_the_shared_parts_already_here() {
        let scratch = Scratch::new();
        let hub = Hub::listing("repo-z-image-turbo-gguf.json").await;
        let deps = deps_with(catalog(), gpu(8, 32), &scratch, Some(hub.client()));
        let encoder = &families()[&GenFamily::ZImageTurbo].companions[0];
        let file = destination(&deps.models_dir, &encoder.repo, &encoder.path).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, vec![0u8; usize::try_from(encoder.size).unwrap()]).unwrap();

        let repo = local_repo(&deps, "leejet/Z-Image-Turbo-GGUF")
            .await
            .unwrap();
        let asked = hub.requests();
        assert_eq!(asked.len(), 1, "{asked:?}");
        assert!(
            asked[0].starts_with("GET /api/models/leejet/Z-Image-Turbo-GGUF?"),
            "{asked:?}"
        );
        assert_eq!(repo.status, "supported");
        assert_eq!(repo.reason, None);
        assert_eq!(repo.family.as_deref(), Some("Z-Image Turbo"));
        assert_eq!(repo.commit, "c61c0e422dc8b541b7548cf33a4ef8302b0f8085");
        assert_eq!(repo.gate.mode, "none");
        assert_eq!(repo.label_mismatch, None);
        assert!(!repo.files.is_empty());
        let first = &repo.files[0];
        assert!(first.file.ends_with(".gguf"), "{}", first.file);
        assert!(!first.quant.is_empty());
        assert_eq!(first.parts.len(), 3);
        assert_eq!(first.parts[0].label, "makes the picture");
        assert_eq!(first.parts[1].label, "reads your prompt");
        assert_eq!(first.parts[2].label, "picture decoder");
        assert!(!first.parts[0].already_here);
        assert!(first.parts[1].already_here);
        assert!(first.to_fetch_bytes < first.total_bytes);
        assert_eq!(first.to_fetch_bytes, first.total_bytes - encoder.size);
        assert_eq!(first.fit.word, "fits");
        let licence = repo.licence.as_ref().unwrap();
        assert_eq!(licence.badge, "green");
        assert_eq!(licence.sha256, apache().sha256);
        assert_eq!(
            repo.licences
                .iter()
                .map(|l| l.id.as_str())
                .collect::<Vec<_>>(),
            ["apache-2.0"]
        );
        assert_eq!(first.licences, repo.licences);
    }

    // ---- the search -----------------------------------------------------------------------

    /// A search asks Hugging Face its three queries (the two picture pipeline tags, then none)
    /// and gives one page, most downloads first: each hit says how popular the repository
    /// is, how it is gated and its own licence label; an untagged hit stays only when its
    /// name is a picture family's.
    #[tokio::test]
    async fn a_search_hit_says_what_hugging_face_lists() {
        let hub = Hub::new(|line| {
            let found = if line.contains("pipeline_tag=text-to-image") {
                json!([{"id": "leejet/Z-Image-Turbo-GGUF", "downloads": 9, "likes": 2,
                        "gated": "manual", "tags": ["gguf", "license:apache-2.0"]}])
            } else if line.contains("pipeline_tag=") {
                json!([])
            } else {
                json!([
                    {"id": "someone/Z-Image-Turbo-Q8-GGUF", "downloads": 4, "likes": 1,
                     "gated": false, "tags": ["gguf"]},
                    {"id": "someone/Qwen3-4B-GGUF", "downloads": 99, "likes": 7,
                     "gated": false, "tags": ["gguf", "license:apache-2.0"]}
                ])
            };
            ("200 OK", found.to_string())
        })
        .await;
        let page = local_search(&hub.client(), "z-image", MediaKind::Image)
            .await
            .unwrap();
        let asked = hub.requests();
        assert_eq!(asked.len(), 3, "{asked:?}");
        assert!(
            asked
                .iter()
                .all(|l| l.starts_with("GET /api/models?") && l.contains("search=z-image")),
            "{asked:?}"
        );
        for tag in ["text-to-image", "image-to-image"] {
            assert!(
                asked
                    .iter()
                    .any(|l| l.contains(&format!("pipeline_tag={tag}&"))),
                "{tag}: {asked:?}"
            );
        }
        assert_eq!(
            page.items,
            [
                LocalSearchHitDto {
                    repo: "leejet/Z-Image-Turbo-GGUF".into(),
                    downloads: 9,
                    likes: 2,
                    gated: "manual".into(),
                    license_label: Some("apache-2.0".into()),
                },
                LocalSearchHitDto {
                    repo: "someone/Z-Image-Turbo-Q8-GGUF".into(),
                    downloads: 4,
                    likes: 1,
                    gated: "no".into(),
                    license_label: None,
                },
            ]
        );
    }

    // ---- the notice -----------------------------------------------------------------------

    struct NoProviders;

    #[async_trait]
    impl ProviderSource for NoProviders {
        async fn media(
            &self,
            _id: store::ProviderId,
        ) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)> {
            Err(crate::studio::errors::no_endpoint())
        }

        async fn company(&self, _id: store::ProviderId) -> Option<Company> {
            None
        }
    }

    struct NoSink;

    impl StudioSink for NoSink {
        fn progress(&self, _run: &RunDto) {}
    }

    struct NoFetch;

    #[async_trait]
    impl ForeignFetch for NoFetch {
        async fn fetch_to_file(
            &self,
            _url: &Url,
            _cap: u64,
            _dest: &Path,
        ) -> Result<String, search::FetchError> {
            unreachable!("accepting the notice downloads nothing")
        }
    }

    struct FixedClock(i64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }

        fn day_start_ms(&self, at_ms: i64) -> i64 {
            day_start_in(&TimeZone::system(), at_ms)
        }
    }

    /// The note about local models is recorded with the time it was accepted, saved, and held
    /// by the manager, so the next download sees it and a later Settings change keeps it.
    #[tokio::test]
    async fn accepting_the_notice_records_when() {
        let scratch = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        let manager = StudioManager::new(
            StudioDeps {
                store: store.clone(),
                providers: Arc::new(NoProviders),
                sink: Arc::new(NoSink),
                fetch: Arc::new(NoFetch),
                clock: Arc::new(FixedClock(1000)),
                default_gallery: scratch.path().join("gallery"),
                thumbs: None,
                limits: MediaLimits::default(),
                poll: PollPlan::default(),
                local: None,
            },
            StudioPrefs::default(),
        );
        let prefs = manager.accept_local_notice().await.unwrap();
        assert_eq!(prefs.local_notice_accepted_at, Some(1000));
        assert_eq!(manager.prefs().local_notice_accepted_at, Some(1000));
        assert_eq!(
            prefs::load(&store).await.local_notice_accepted_at,
            Some(1000)
        );
        let changed = manager
            .set_prefs(StudioPrefsPatch {
                advanced_open: Some(true),
                ..StudioPrefsPatch::default()
            })
            .await
            .unwrap();
        assert_eq!(changed.local_notice_accepted_at, Some(1000));
        assert_eq!(
            prefs::load(&store).await.local_notice_accepted_at,
            Some(1000)
        );
    }
}
