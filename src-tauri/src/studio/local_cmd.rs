//! Get local models (studio-local-design.md, "Get local models"; 0.1.0 Task L16): Studio's
//! own view of the picture and video models this computer can run.
//!
//! - **The list** ([`local_models`]): what harnessUI detected, the Studio engine's state, the
//!   installed models, and the signed catalog's recommended list split into the entries that
//!   run here and those that do not ("Not for this computer", each with its reason), with
//!   "Recommended for this computer" marked once per kind at most. It sends nothing.
//! - **Hugging Face**, each only on a click: a search for picture, video or LoRA repositories
//!   ([`local_search`]), each hit with what it offers (exact once its files were read, else
//!   from its name), and a repository's listing at its latest commit ([`local_repo`]): every
//!   file's label, one choice per quantisation of each model that works with its set-up plan,
//!   licences and fit, and the best for this computer marked.
//! - **A set-up plan** ([`setup_plan`]) of a searched file or a file on this computer, with
//!   another recipe or files the person chose.
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

use agent::media::{MediaKind, MediaModel, Price};
use models::fit::{RAM_RESERVE_BYTES, Target};
use models::gen_catalog::{Commercial, GateMode, GenEntry, GenGate, GenKind, GenLicence, GenTiers};
use models::gen_eta::{self, FitWord};
use models::gen_fit::{GenFit, GenSize};
use models::gen_search::{repo_licence, verdict_hint};
use models::hf::{HfClient, RepoSummary, uploader_text};
use models::sd::candidates::candidates;
use models::sd::matching::{kind_of, recipes_for};
use models::sd::{FileLabel, Verdict, verdict_of};
use tauri::{AppHandle, Manager, State};

use super::deps::{AppHardware, LocalHardware, with_device_free};
use super::detect::DetectService;
use super::engine_cmd::{self, EngineNeed};
use super::errors;
use super::local::{
    AppGenCatalog, GenCatalogSource, default_frames, frames_for, longest_side, media_kind,
    media_model,
};
use super::local_models::{
    DownloadPlan, InstalledFiles, LocalSource, PlanDeps, Planned, ReadRepo, delete_local_model,
    entry_from_installed, entry_with, plan_download, plan_file, plan_source, start_local_download,
    word,
};
use super::manager::{LocalVerdict, StudioManager, device_label, eta_dto, local_verdict, word_tag};
use super::prefs::StudioPrefs;
use crate::commands::only_main_window;
use crate::dto::{
    EngineStatusDto, GateDto, HardwareLineDto, InstalledLocalDto, LicenceDto, LocalEntryDto,
    LocalFileChoiceDto, LocalFitDto, LocalModelsDto, LocalRepoDto, LocalSearchHitDto,
    LocalSearchPage, PartDto, PlannedSlotDto, RecipeChoiceDto, RepoFileDto, SearchFilter,
    SetupChoice, SetupPlanDto, media_model_dto,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// A billion bytes: the GB the catalog's tiers count in.
const GB: u64 = 1_000_000_000;

/// What Get local models reads: each part a test can stand in for.
pub struct LocalCmdDeps {
    /// The installed models, this computer's measured runs.
    pub store: store::Store,
    /// The signed catalog's picture and video models.
    pub catalog: Arc<dyn GenCatalogSource>,
    /// This computer, as the local lane judges it.
    pub hardware: Arc<dyn LocalHardware>,
    /// Hugging Face, with the stored token; `None` where nothing is asked of it.
    pub hf: Option<HfClient>,
    /// What reading repos' files found, kept between openings.
    pub detect: Arc<DetectService>,
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
    /// The files the installed models use: a part's place is shown as its download would
    /// have it (`local_models::plan_download`).
    pub installed_files: InstalledFiles,
}

// ---- the list ---------------------------------------------------------------------------------

/// Get local models: this computer, the engine, the installed models (newest first, each at
/// its default size; one this build cannot read is still listed, not for this computer, so it
/// can be deleted), and the recommended list at each entry's default size, split by its fit
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
                tracing::warn!(
                    model = %row.id,
                    "an installed model cannot be run by this version: {}",
                    e.details
                );
                installed.push(unreadable(row));
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
    let total = deps.hardware.device_total_bytes().await;
    Ok(LocalModelsDto {
        hardware: hardware_line(deps, &device, total, &backend, description.as_deref()),
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

/// The header's line: the device by name, its memory (a card's total, `total`, as its probe
/// says, or its free memory with the chat model's share counted as free when the probe does
/// not say; Apple silicon's working set), the computer's memory and the models folder's free
/// space.
fn hardware_line(
    deps: &LocalCmdDeps,
    device: &Target,
    total: Option<u64>,
    backend: &str,
    description: Option<&str>,
) -> HardwareLineDto {
    let (graphics_bytes, shared_memory) = match *device {
        Target::Gpu { free_bytes, .. } => (Some(total.unwrap_or(free_bytes)), false),
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

/// An installed model this build cannot read (`entry_from_installed` refuses it: its family
/// record hidden by a newer signed catalog, a part's licence no longer known, a family this
/// build does not know): its stored id, name and kind, its parts' sizes, and not for this
/// computer, with why, so that *Delete* can still remove its files.
fn unreadable(row: &store::GenModel) -> InstalledLocalDto {
    let kind = serde_json::from_value::<GenKind>(row.kind.clone().into())
        .map_or(MediaKind::Image, media_kind);
    let reason = errors::NOT_RUN_BY_THIS_VERSION;
    let model = MediaModel {
        id: row.id.clone(),
        display_name: row.name.clone(),
        family: row.family.clone(),
        kind,
        accepts_images: false,
        params: Vec::new(),
        price: Price::Free,
        notes: Vec::new(),
        usable: Some(reason.to_owned()),
        created: Some(row.added_at / 1000),
        companies: Vec::new(),
    };
    InstalledLocalDto {
        id: row.id.clone(),
        name: row.name.clone(),
        kind,
        size_bytes: parts_bytes(row),
        fit: LocalFitDto {
            word: word_tag(FitWord::NotHere).to_owned(),
            verdict: "red".to_owned(),
            reason: Some(reason.to_owned()),
            suggestion: None,
            eta: None,
            very_slow: gen_eta::very_slow(FitWord::NotHere, None),
        },
        model: media_model_dto(&model, None),
    }
}

/// An installed model's parts' sizes added up; its recorded total when no part gives one.
fn parts_bytes(row: &store::GenModel) -> u64 {
    let sizes: Vec<u64> = row
        .parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part.get("size")?.as_u64())
        .collect();
    if sizes.is_empty() {
        row.size_bytes
    } else {
        sizes.iter().sum()
    }
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
/// its default length), with this computer as it is now; a searched file's or a file on this
/// computer's as its set-up plan makes it, with nothing chosen. A request only for a search
/// source's listing. A size the model is never made at is refused (`size_not_offered`).
pub async fn local_fit(
    deps: &LocalCmdDeps,
    source: &LocalSource,
    width: u32,
    height: u32,
    seconds: u32,
) -> AppResult<LocalFitDto> {
    let catalog = deps.catalog.gen_catalog().await;
    let entry = entry_with(&deps.store, &plan_deps(deps, &catalog), source).await?;
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

/// A Hugging Face search for picture, video or LoRA repositories matching `text`
/// (`HfClient::search_generators`: its three queries, merged into one page), each hit with
/// what it offers ([`search_page`]).
pub async fn local_search(
    hf: &HfClient,
    detect: &DetectService,
    text: &str,
    filter: SearchFilter,
) -> AppResult<LocalSearchPage> {
    let found = hf
        .search_generators(text, filter)
        .await
        .map_err(crate::hub::hf_error)?;
    Ok(search_page(found, detect))
}

/// The search's repositories as hits, in their order: each with its owner, when it last
/// changed, and its verdict: the last reading of its files' (`exact`,
/// [`DetectService::verdict`]), else its name's hint ([`verdict_hint`]).
fn search_page(found: Vec<RepoSummary>, detect: &DetectService) -> LocalSearchPage {
    LocalSearchPage {
        items: found
            .into_iter()
            .map(|r| {
                let exact = detect.verdict(&r.id);
                LocalSearchHitDto {
                    gated: word(&GateMode::from(r.gated)),
                    author: owner(&r.id).to_owned(),
                    verdict: exact
                        .clone()
                        .unwrap_or_else(|| verdict_hint(&r.id, r.pipeline_tag.as_deref())),
                    exact: exact.is_some(),
                    downloads: r.downloads,
                    likes: r.likes,
                    updated_at: r.last_modified,
                    license_label: r.license,
                    repo: r.id,
                }
            })
            .collect(),
    }
}

/// A repo id's owner (`owner/name`).
fn owner(repo: &str) -> &str {
    repo.split_once('/').map_or(repo, |(owner, _)| owner)
}

/// Repository `repo` at its latest commit, as Studio reads it ([`repo_dto`]): its listing,
/// then its files' headers (`DetectService::read_repo`) and its model card at once.
pub async fn local_repo(deps: &LocalCmdDeps, repo: &str) -> AppResult<LocalRepoDto> {
    let hf = deps
        .hf
        .as_ref()
        .ok_or_else(|| AppError::internal("a repository listing without a Hub client"))?;
    let detail = hf.repo(repo, None).await.map_err(crate::hub::hf_error)?;
    let (read, card) = tokio::join!(
        ReadRepo::of(&deps.detect, hf, detail.clone()),
        hf.model_card(&detail.id, &detail.commit)
    );
    Ok(repo_dto(deps, &read, card.ok().as_deref()).await)
}

/// The first paragraph of a model card after its front matter, made safe and cut at 400
/// characters (`hf::uploader_text`); headings, pictures and HTML lines are passed over.
/// `None` when it has none.
fn card_summary(card: &str) -> Option<String> {
    let mut lines = card.lines().peekable();
    if lines.peek().is_some_and(|l| l.trim() == "---") {
        lines.next();
        for line in lines.by_ref() {
            if line.trim() == "---" {
                break;
            }
        }
    }
    let paragraph: Vec<&str> = lines
        .map(str::trim)
        .skip_while(|l| l.is_empty() || !is_prose(l))
        .take_while(|l| !l.is_empty() && is_prose(l))
        .collect();
    let text = uploader_text(&paragraph.join(" "), 400);
    (!text.is_empty()).then_some(text)
}

/// A line of a paragraph: not a heading, a picture, an HTML tag or a rule.
fn is_prose(line: &str) -> bool {
    !(line.starts_with('#')
        || line.starts_with("![")
        || line.starts_with('<')
        || line.starts_with("---")
        || line.starts_with("```"))
}

/// What a repository offers, from what reading it found (`read`) and its model card: every
/// file's label; for each main file that works (an A14B pair's low-noise half in its
/// high-noise half's plan, a diffusers folder once) the set-up plan with nothing chosen,
/// largest first, "best for this computer" on the largest whose fit word is `fits` or
/// `afterUnload`; the licences; and the verdict: from its labels when a file works, it is a
/// LoRA or nothing went unread, else its name's hint. A file whose plan cannot be made is
/// left out of the choices, still labelled.
async fn repo_dto(deps: &LocalCmdDeps, read: &ReadRepo, card: Option<&str>) -> LocalRepoDto {
    let ReadRepo { detail, reading } = read;
    let catalog = deps.catalog.gen_catalog().await;
    let pd = plan_deps(deps, &catalog);
    let size_of = |path: &str| {
        detail
            .files
            .iter()
            .find(|f| f.path == path)
            .map_or(0, |f| f.size)
    };
    let view = |(path, label): (&String, &FileLabel)| RepoFileDto {
        path: path.clone(),
        size_bytes: size_of(path),
        quant: models::files::quant_label(path),
        label: label.clone(),
    };
    let file_views: Vec<RepoFileDto> = reading.labels.iter().map(view).collect();
    let loras = file_views
        .iter()
        .filter(|f| matches!(f.label, FileLabel::Lora { .. }))
        .cloned()
        .collect();
    let has_high = reading.labels.values().any(|l| {
        matches!(
            l,
            FileLabel::Works {
                part: Some(models::sd::matching::NoisePart::High),
                ..
            }
        )
    });
    let fp16_folder = candidates(&detail.files).folder.map(|f| f.fp16);

    let mut files = Vec::new();
    for (path, label) in &reading.labels {
        let FileLabel::Works { part, folder, .. } = label else {
            continue;
        };
        if has_high && *part == Some(models::sd::matching::NoisePart::Low) {
            continue;
        }
        let planned = match plan_file(&pd, read, path, &SetupChoice::default()).await {
            Ok(planned) => planned,
            Err(e) => {
                tracing::warn!(file = %path, "a searched file has no set-up: {}", e.details);
                continue;
            }
        };
        let plan = match setup_plan_dto(deps, &planned).await {
            Ok(plan) => plan,
            Err(e) => {
                tracing::warn!(file = %path, "a searched file has no set-up: {}", e.details);
                continue;
            }
        };
        let quant = match (folder, fp16_folder) {
            (true, Some(true)) => "fp16".to_owned(),
            (true, _) => "fp32".to_owned(),
            (false, _) => models::files::quant_label(path).unwrap_or_default(),
        };
        files.push(LocalFileChoiceDto {
            file: path.clone(),
            quant,
            label: label.clone(),
            plan,
            best: false,
        });
    }
    files.sort_by(|a, b| {
        b.plan
            .total_bytes
            .cmp(&a.plan.total_bytes)
            .then_with(|| a.file.cmp(&b.file))
    });
    if let Some(best) = files
        .iter_mut()
        .find(|f| matches!(f.plan.fit.word.as_str(), "fits" | "afterUnload"))
    {
        best.best = true;
    }
    let mut licences: Vec<LicenceDto> = Vec::new();
    for licence in files.iter().flat_map(|f| &f.plan.licences) {
        if !licences.iter().any(|l| l.sha256 == licence.sha256) {
            licences.push(licence.clone());
        }
    }
    let labels: Vec<FileLabel> = reading.labels.values().cloned().collect();
    let verdict = match verdict_of(&labels) {
        v @ (Verdict::Works { .. } | Verdict::Lora { .. }) => v,
        v if reading.unread == 0 => v,
        _ => verdict_hint(&detail.id, detail.info.pipeline_tag.as_deref()),
    };
    LocalRepoDto {
        repo: detail.id.clone(),
        commit: detail.commit.clone(),
        author: owner(&detail.id).to_owned(),
        summary: card.and_then(card_summary),
        verdict,
        files,
        loras,
        file_views,
        licence: Some(licence_dto(&repo_licence(detail))),
        licences,
        label_mismatch: None,
        gate: gate_dto(&GenGate {
            mode: GateMode::from(detail.gated),
            collects: Vec::new(),
        }),
        read_files: reading.read,
        unread_files: reading.unread,
    }
}

// ---- set-up plans -----------------------------------------------------------------------------

/// What planning reads in Get local models: its Hub client, readings, catalog and installed
/// files.
fn plan_deps<'a>(
    deps: &'a LocalCmdDeps,
    catalog: &'a models::gen_catalog::GenCatalog,
) -> PlanDeps<'a> {
    PlanDeps {
        hf: deps.hf.as_ref(),
        detect: &deps.detect,
        catalog,
        installed: &deps.installed_files,
    }
}

/// The set-up of the file `source` names (a searched file, at its repo's commit, or a file on
/// this computer) with the person's `choice`: another recipe, files for some slots
/// (`local_models::plan_source`).
pub async fn setup_plan(
    deps: &LocalCmdDeps,
    source: &LocalSource,
    choice: &SetupChoice,
) -> AppResult<SetupPlanDto> {
    let catalog = deps.catalog.gen_catalog().await;
    let planned = plan_source(&plan_deps(deps, &catalog), source, choice).await?;
    setup_plan_dto(deps, &planned).await
}

/// A recipe as the Model type menu offers it.
fn recipe_choice(id: &str) -> Option<RecipeChoiceDto> {
    let recipe = models::sd::recipes::recipe(id)?;
    Some(RecipeChoiceDto {
        id: recipe.id.clone(),
        name: recipe.name.clone(),
        kind: kind_of(id),
    })
}

/// A plan in the webview's words: each slot with its file, what its download takes here
/// (`local_models::plan_download`) and its fit at the recipe's default size.
async fn setup_plan_dto(deps: &LocalCmdDeps, planned: &Planned) -> AppResult<SetupPlanDto> {
    let download = plan(deps, planned.entry.clone())?;
    let verdict = judge(deps, &download.entry, default_size(&download.entry)).await;
    let parts = parts_dto(&download);
    let slots = planned
        .plan
        .parts
        .iter()
        .zip(&planned.part_of)
        .filter_map(|(p, at)| {
            Some(PlannedSlotDto {
                role: models::gen_catalog::GenRole::from_slot(p.slot)?,
                required: p.required,
                source: p.source,
                part: at.and_then(|i| parts.get(i).cloned()),
            })
        })
        .collect();
    let recipe = recipe_choice(&planned.plan.recipe_id)
        .ok_or_else(|| AppError::internal("a plan with a recipe the engine does not have"))?;
    let recipes = planned
        .entry
        .version
        .map(recipes_for)
        .unwrap_or_default()
        .into_iter()
        .filter_map(recipe_choice)
        .collect();
    Ok(SetupPlanDto {
        recipe,
        recipes,
        slots,
        licences: download.licences.iter().map(licence_dto).collect(),
        total_bytes: download.total_bytes,
        to_fetch_bytes: download.to_fetch_bytes,
        fit: fit_dto(&download.entry, &verdict),
        can_download: planned.plan.can_run(),
        low_disk: download.low_disk(),
        needs_engine: download.needs_engine,
        untested: planned.plan.untested,
    })
}

// ---- the words of a download ------------------------------------------------------------------

/// What downloading `entry` takes here (`local_models::plan_download`), never refused for the
/// disk: a list says that with `low_disk`, and the download itself refuses it.
fn plan(deps: &LocalCmdDeps, entry: GenEntry) -> AppResult<DownloadPlan> {
    let used = deps.installed_files.except(&entry.id);
    let mut plan = plan_download(entry, &deps.models_dir, &deps.engine_need, None, &used)?;
    plan.free_bytes = deps.free_disk_bytes;
    Ok(plan)
}

/// Each part of a plan, in the entry's order, as the download dialog lists it: a Hub part
/// already here when its place has its pinned size, a file from this computer always.
fn parts_dto(plan: &DownloadPlan) -> Vec<PartDto> {
    let mut places = plan.files.iter().map(|(_, dest)| dest);
    plan.entry
        .parts
        .iter()
        .map(|part| {
            let (path, already_here) = match &part.local {
                Some(local) => (local.display().to_string(), true),
                None => (
                    part.path.clone(),
                    places
                        .next()
                        .is_some_and(|dest| plan.already_here.contains(dest)),
                ),
            };
            PartDto {
                role: part.role,
                repo: part.repo.clone(),
                path,
                size_bytes: part.size,
                licence_id: part.license.clone(),
                already_here,
                local: part.local.is_some(),
            }
        })
        .collect()
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
        excluded_regions: licence.excluded_regions.clone(),
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
    let store = app.state::<AppState>().store.clone();
    let installed_files = InstalledFiles::read(&store)
        .await
        .inspect_err(
            |e| tracing::warn!(code = %e.code, "the installed models' files were not read"),
        )
        .unwrap_or_default();
    LocalCmdDeps {
        store,
        catalog: Arc::new(AppGenCatalog(app.clone())),
        hardware: Arc::new(AppHardware(app.clone())),
        hf,
        detect: app.state::<Arc<DetectService>>().inner().clone(),
        free_disk_bytes: reverse::diskspace::free_bytes(&models_dir),
        models_dir,
        ram_total_bytes: models::hardware::system_memory().map(|m| m.total_bytes),
        engine: engine_cmd::engine_status(app),
        engine_need: engine_cmd::engine_need(app),
        notice_accepted: studio.prefs().local_notice_accepted_at.is_some(),
        installed_files,
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

/// Search (or Enter) in Get local models: Hugging Face's picture, video or LoRA
/// repositories matching `text`, with the stored token, so gated repositories are found.
#[tauri::command]
pub async fn studio_local_search(
    window: tauri::WebviewWindow,
    app: AppHandle,
    detect: State<'_, Arc<DetectService>>,
    text: String,
    filter: SearchFilter,
) -> AppResult<LocalSearchPage> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    local_search(&hf, &detect, &text, filter).await
}

/// Opening a search result: repository `repo` at its latest commit, its files read here.
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
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } | LocalSource::Disk { .. } => {
            None
        }
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

/// The set-up of a searched file or a file on this computer, with the person's `choice`
/// (another recipe, files for some slots). Asks Hugging Face for a searched file's listing
/// (its headers come from the cache once its repo was opened) and for the listing of a file
/// the person chose from another repo.
#[tauri::command]
pub async fn studio_setup_plan(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    source: LocalSource,
    choice: SetupChoice,
) -> AppResult<SetupPlanDto> {
    only_main_window(window.label())?;
    let hf = crate::hub::hf_client(&app).await?;
    setup_plan(&app_deps(&app, &studio, Some(hf)).await, &source, &choice).await
}

/// Starts a checked download of the model and returns its download group.
#[tauri::command]
pub async fn studio_local_download(
    window: tauri::WebviewWindow,
    app: AppHandle,
    source: LocalSource,
) -> AppResult<String> {
    only_main_window(window.label())?;
    start_local_download(&app, source).await
}

/// Download an individual weight file even when Studio has no matching generation recipe.
#[tauri::command]
pub async fn studio_local_download_file(
    window: tauri::WebviewWindow,
    app: AppHandle,
    repo: String,
    commit: String,
    file: String,
) -> AppResult<String> {
    only_main_window(window.label())?;
    super::local_models::download_weight_file(&app, &repo, &commit, &file).await
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
    use crate::dto::{SlotFile, SlotOverride};
    use crate::error::AppResult;
    use crate::studio::deps::{
        Clock, Endpoint, ForeignFetch, LocalHardware, ProviderSource, StudioSink,
    };
    use crate::studio::engine_cmd::EngineNeed;
    use crate::studio::local::{GenCatalogSource, run_shape};
    use crate::studio::local_models::{LocalSource, delete_unless_in_use};
    use crate::studio::manager::{PollPlan, StudioDeps, StudioManager};
    use crate::studio::prefs::{self, StudioPrefs, StudioPrefsPatch};
    use crate::studio::spend::day_start_in;
    use crate::studio::test_support::Scratch;
    use models::sd::CannotRunReason;
    use models::sd::detect::SdVersion;
    use models::sd::matching::RecipeKind;
    use models::sd::setup::PartSource;
    use models::sd::testing::{Served, file_of, fixture, serve};

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
            family: Some(family),
            recipe: family.recipe_id().into(),
            version: Some(family.version()),
            untested: false,
            inputs: Vec::new(),
            input_counts: Vec::new(),
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
                    local: None,
                    folder: None,
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
                guidance: None,
                img_cfg: None,
                timestep_shift: None,
                sigmas: Vec::new(),
                high_noise: None,
                moe_boundary: None,
                strength: None,
            },
            limits: GenLimits {
                multiple_of: if video { 32 } else { 16 },
                max_side: if video { 1280 } else { 2048 },
                frame_rule: video.then_some(FrameRule::FourNPlusOne),
                max_frames: video.then_some(121),
                video: None,
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
            companions: bundled().companions,
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
        total: Option<u64>,
    }

    /// A Vulkan card with `free_gb` GB free, and `ram_gb` GB of free memory; its probe gives
    /// no total.
    fn gpu(free_gb: u64, ram_gb: u64) -> Fixed {
        Fixed {
            target: Target::Gpu {
                name: CARD.into(),
                free_bytes: free_gb * GB,
                ram_free_bytes: Some(ram_gb * GB),
            },
            backend: "vulkan",
            chat: None,
            total: None,
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
            total: None,
        }
    }

    impl Fixed {
        /// A chat model loaded, taking `gb` GB of the card.
        fn chat(mut self, gb: u64) -> Self {
            self.chat = Some(gb * GB);
            self
        }

        /// The card's total memory, as its probe says.
        fn total(mut self, gb: u64) -> Self {
            self.total = Some(gb * GB);
            self
        }
    }

    #[async_trait]
    impl LocalHardware for Fixed {
        async fn target(&self) -> (Target, String, Option<String>) {
            let description = (self.backend != "cpu").then(|| CARD.to_owned());
            (self.target.clone(), self.backend.to_owned(), description)
        }

        async fn device_total_bytes(&self) -> Option<u64> {
            self.total
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
            detect: Arc::new(DetectService::new()),
            models_dir: scratch.path().join("models"),
            free_disk_bytes: Some(500 * GB),
            ram_total_bytes: Some(32 * GB),
            engine: engine_built_in(),
            engine_need: EngineNeed::Found,
            notice_accepted: false,
            installed_files: InstalledFiles::default(),
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
    /// on an 8 GB card, and each entry not installed whose word is "Fits" or "Fits after the
    /// chat model is unloaded" counted as more (whatever its measured points make it).
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
        let fitting = local
            .recommended
            .iter()
            .filter(|e| !e.installed && matches!(e.fit.word.as_str(), "fits" | "afterUnload"))
            .count();
        assert_eq!(local.more_fit as usize, fitting);
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
    /// shows the card's 8 GB.
    #[tokio::test]
    async fn a_model_that_fits_after_the_chat_model_is_unloaded_says_so() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(3, 32).chat(5).total(8), &scratch))
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

    /// What harnessUI detected, for the header: an 8 GB card reads 8 GB however much of it
    /// is free ("Radeon RX 7600, 8 GB graphics memory").
    #[tokio::test]
    async fn the_header_says_what_this_computer_has() {
        let scratch = Scratch::new();
        let local = local_models(&deps(gpu(7, 32).total(8), &scratch))
            .await
            .unwrap();
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
        // A probe that gives no total: the free memory, the chat model's share counted back.
        let local = local_models(&deps(gpu(6, 32).chat(1), &scratch))
            .await
            .unwrap();
        assert_eq!(local.hardware.graphics_bytes, Some(7 * GB));
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

        // Where the engine comes with the first model, its licence is listed with it.
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
            local: None,
            folder: None,
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
                recipe_id: serde_json::from_value::<GenFamily>(json!(family))
                    .ok()
                    .map(|f| f.recipe_id().into()),
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

    /// An installed model this build cannot read (its catalog entry gone and its family
    /// record hidden by a newer catalog, a part's licence no longer known, a family this build
    /// does not know) is still listed with its stored name and its parts' sizes, not for this
    /// computer, with why, and Delete removes it.
    #[tokio::test]
    async fn an_installed_model_this_build_cannot_read_is_listed_so_it_can_be_deleted() {
        let scratch = Scratch::new();
        let mut newer = catalog();
        newer.entries.remove(KLEIN);
        newer.families.remove(&GenFamily::Flux2Klein4b);
        let deps = deps_with(newer, gpu(8, 32), &scratch, None);
        // Installed from the recommended list before the catalog changed.
        install_starter(&deps.store, &klein()).await;
        let mut relicensed = zimage();
        relicensed.parts[1].license = "an-old-licence".into();
        install_starter(&deps.store, &relicensed).await;
        let future_id = "hf:someone/Future-GGUF:future.gguf";
        let future = GenPart {
            role: GenRole::Diffusion,
            repo: "someone/Future-GGUF".into(),
            revision: "e".repeat(40),
            path: "future.gguf".into(),
            size: 1_234_567,
            sha256: "f".repeat(64),
            license: "apache-2.0".into(),
            local: None,
            folder: None,
        };
        install(
            &deps.store,
            &deps.models_dir,
            future_id,
            None,
            "A future model",
            "a-future-family",
            &[&future],
        )
        .await;

        let local = local_models(&deps).await.unwrap();
        assert_eq!(local.installed.len(), 3, "{:?}", local.installed);
        for (id, name, size) in [
            (KLEIN, KLEIN, klein().total_bytes()),
            (ZIMAGE, ZIMAGE, zimage().total_bytes()),
            (future_id, "A future model", 1_234_567),
        ] {
            let row = local
                .installed
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} is not listed"));
            assert_eq!(row.name, name);
            assert_eq!(row.kind, MediaKind::Image);
            assert_eq!(row.size_bytes, size, "{id}");
            assert_eq!(row.fit.word, "notHere", "{id}");
            assert_eq!(row.fit.verdict, "red", "{id}");
            assert_eq!(
                row.fit.reason.as_deref(),
                Some("This version of harnessUI cannot run this model.")
            );
            assert!(row.fit.very_slow);
            assert_eq!(row.fit.suggestion, None);
            assert_eq!(row.fit.eta, None);
            // Never offered as a model to run.
            assert_eq!(row.model.usable, row.fit.reason);
            assert!(row.model.params.is_empty());
            assert_eq!(row.model.price.unit, "free");
        }
        assert_eq!(
            local.installed_bytes,
            klein().total_bytes() + zimage().total_bytes() + 1_234_567
        );

        // Delete, as `studio_local_delete` does it: nothing runs it, so it goes.
        let manager = idle_manager(&deps.store, &scratch);
        for id in [KLEIN, future_id] {
            delete_unless_in_use(&manager, &deps.store, &deps.models_dir, id, &|_| false)
                .await
                .unwrap();
        }
        let local = local_models(&deps).await.unwrap();
        assert_eq!(
            local
                .installed
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            [ZIMAGE]
        );
        assert_eq!(local.installed_bytes, zimage().total_bytes());
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

    // ---- a searched repository (Task S6) ---------------------------------------------------

    /// The commit every test repo is listed at.
    const COMMIT: &str = "0123456789abcdef0123456789abcdef01234567";

    /// A repo as the test Hub lists it, and what the Hub serves for its files.
    #[derive(Clone)]
    struct TestRepo {
        id: String,
        commit: String,
        label: Option<String>,
        gated: bool,
        pipeline_tag: Option<String>,
        files: Vec<models::files::RepoFile>,
        served: BTreeMap<String, Served>,
        card: Option<String>,
    }

    impl TestRepo {
        fn new(id: &str) -> Self {
            TestRepo {
                id: id.into(),
                commit: COMMIT.into(),
                label: None,
                gated: false,
                pipeline_tag: None,
                files: Vec::new(),
                served: BTreeMap::new(),
                card: None,
            }
        }

        fn label(mut self, label: &str) -> Self {
            self.label = Some(label.into());
            self
        }

        fn at(mut self, commit: &str) -> Self {
            self.commit = commit.into();
            self
        }

        fn tagged(mut self, tag: &str) -> Self {
            self.pipeline_tag = Some(tag.into());
            self
        }

        fn card(mut self, text: &str) -> Self {
            self.card = Some(text.into());
            self
        }

        /// Lists `path` (`size` bytes, with a SHA-256 made from the repo and the path) and
        /// serves the header of the detection fixture `header`, if any.
        fn file(self, path: &str, size: f64, header: Option<&str>) -> Self {
            use std::hash::{DefaultHasher, Hash, Hasher};
            let mut h = DefaultHasher::new();
            (&self.id, path).hash(&mut h);
            let sha = format!("{:016x}", h.finish()).repeat(4);
            self.pinned(path, size as u64, &sha, header)
        }

        /// Lists `path` with this size and SHA-256, serving `header`'s fixture, if any.
        fn pinned(mut self, path: &str, size: u64, sha256: &str, header: Option<&str>) -> Self {
            self.files.push(models::files::RepoFile {
                path: path.into(),
                size,
                sha256: Some(sha256.into()),
            });
            if let Some(name) = header {
                self.served
                    .insert(path.into(), Served::Bytes(file_of(&fixture(name))));
            }
            self
        }

        /// Lists a file that is not stored with LFS (no SHA-256).
        fn plain(mut self, path: &str) -> Self {
            self.files.push(models::files::RepoFile {
                path: path.into(),
                size: 1_000,
                sha256: None,
            });
            self
        }

        /// The listing as Hugging Face's API gives it.
        fn listing(&self) -> Value {
            let siblings: Vec<Value> = self
                .files
                .iter()
                .map(|f| match &f.sha256 {
                    Some(sha) => json!({"rfilename": f.path, "size": f.size,
                                        "lfs": {"size": f.size, "sha256": sha}}),
                    None => json!({"rfilename": f.path, "size": f.size}),
                })
                .collect();
            let mut tags = vec![json!("diffusers")];
            if let Some(label) = &self.label {
                tags.push(json!(format!("license:{label}")));
            }
            json!({
                "id": self.id,
                "sha": self.commit,
                "gated": if self.gated { json!("auto") } else { json!(false) },
                "tags": tags,
                "pipeline_tag": self.pipeline_tag,
                "siblings": siblings,
            })
        }
    }

    /// A Hub on this computer for `repos`: each one's listing (at its latest commit, or at its
    /// own commit by revision), the headers of its files (`HEAD` and ranged `GET` on
    /// `resolve`), and its model card; anything else is 404. It keeps every request line.
    struct RepoHub {
        url: String,
        seen: Arc<Mutex<Vec<String>>>,
    }

    impl RepoHub {
        fn client(&self) -> HfClient {
            HfClient::for_test(&self.url)
        }

        fn requests(&self) -> Vec<String> {
            self.seen.lock().unwrap().clone()
        }

        /// The ranged `GET`s: one per header read.
        fn header_reads(&self) -> usize {
            self.requests()
                .iter()
                .filter(|l| l.starts_with("GET") && l.contains("/resolve/"))
                .count()
        }
    }

    fn not_found() -> Vec<u8> {
        b"HTTP/1.1 404 X\r\nx-error-code: EntryNotFound\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
            .to_vec()
    }

    fn ok(content_type: &str, body: &[u8]) -> Vec<u8> {
        let mut out = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: {content_type}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
            body.len()
        )
        .into_bytes();
        out.extend_from_slice(body);
        out
    }

    async fn hub(repos: Vec<TestRepo>) -> RepoHub {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let lines = seen.clone();
        let url = serve(move |line, headers| {
            lines.lock().unwrap().push(line.trim_end().to_owned());
            let target = line.split_whitespace().nth(1).unwrap_or_default();
            let path = target.split('?').next().unwrap_or_default();
            if let Some(rest) = path.strip_prefix("/api/models/") {
                let (id, revision) = match rest.split_once("/revision/") {
                    Some((id, rev)) => (id, Some(rev)),
                    None => (rest, None),
                };
                return match repos
                    .iter()
                    .find(|r| r.id == id && revision.is_none_or(|rev| rev == r.commit))
                {
                    Some(r) => ok("application/json", r.listing().to_string().as_bytes()),
                    None => b"HTTP/1.1 404 X\r\nx-error-code: RepoNotFound\r\nconnection: close\r\ncontent-length: 0\r\n\r\n".to_vec(),
                };
            }
            let Some(repo) = repos
                .iter()
                .find(|r| path.starts_with(&format!("/{}/", r.id)))
            else {
                return not_found();
            };
            let rest = &path[repo.id.len() + 2..];
            if rest == format!("raw/{}/README.md", repo.commit) {
                return match &repo.card {
                    Some(card) => ok("text/plain", card.as_bytes()),
                    None => not_found(),
                };
            }
            let Some(file) = rest.strip_prefix(&format!("resolve/{}/", repo.commit)) else {
                return not_found();
            };
            let bytes = match repo.served.get(file) {
                Some(Served::Bytes(bytes)) => bytes,
                Some(Served::Gated(status)) => {
                    return format!(
                        "HTTP/1.1 {status} X\r\nx-error-code: GatedRepo\r\nconnection: close\r\ncontent-length: 0\r\n\r\n"
                    )
                    .into_bytes();
                }
                None => return not_found(),
            };
            if line.starts_with("HEAD") {
                return format!(
                    "HTTP/1.1 200 OK\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
                    bytes.len()
                )
                .into_bytes();
            }
            let last = headers
                .iter()
                .find_map(|h| {
                    h.to_ascii_lowercase()
                        .strip_prefix("range: bytes=0-")
                        .map(str::to_string)
                })
                .and_then(|n| n.parse::<usize>().ok())
                .unwrap_or(usize::MAX - 1);
            let end = (last + 1).min(bytes.len());
            let mut out = format!(
                "HTTP/1.1 206 Partial Content\r\nconnection: close\r\ncontent-length: {end}\r\ncontent-range: bytes 0-{}/{}\r\n\r\n",
                end.saturating_sub(1),
                bytes.len()
            )
            .into_bytes();
            out.extend_from_slice(&bytes[..end]);
            out
        })
        .await;
        RepoHub { url, seen }
    }

    /// Get local models on `hardware` asking `hub`, with the bundled catalog (its companion
    /// table) and nothing installed.
    fn repo_deps(hub: &RepoHub, hardware: Fixed, scratch: &Scratch) -> LocalCmdDeps {
        deps_with(bundled(), hardware, scratch, Some(hub.client()))
    }

    fn search_source(repo: &str, file: &str) -> LocalSource {
        LocalSource::Search {
            repo: repo.into(),
            commit: COMMIT.into(),
            file: file.into(),
        }
    }

    /// The companion table's row `id`.
    fn companion(id: &str) -> models::sd::companions::Companion {
        bundled()
            .companions
            .files
            .into_iter()
            .find(|c| c.id == id)
            .unwrap()
    }

    const KONTEXT: &str = "QuantStack/FLUX.1-Kontext-dev-GGUF";

    /// QuantStack's FLUX.1 Kontext GGUFs at their real sizes, each serving the Q4_K_M file's
    /// header.
    fn flux_kontext_repo(quants: &[&str]) -> TestRepo {
        quants.iter().fold(
            TestRepo::new(KONTEXT)
                .plain("README.md")
                .tagged("image-to-image"),
            |repo, q| {
                let size = match *q {
                    "Q4_K_M" => 6_931_817_760.0,
                    "Q8_0" => 12_708_163_776.0,
                    "F16" => 23_802_816_640.0,
                    _ => 5e9,
                };
                repo.file(
                    &format!("flux1-kontext-dev-{q}.gguf"),
                    size,
                    Some("flux1-kontext"),
                )
            },
        )
    }

    fn lumina_repo() -> TestRepo {
        TestRepo::new("Comfy-Org/Lumina-Image-2")
            .file(
                "split_files/diffusion_models/lumina_2_model_bf16.safetensors",
                5_219_585_680.0,
                Some("lumina2"),
            )
            .file("split_files/vae/ae.safetensors", 335_304_388.0, None)
            .plain("README.md")
    }

    /// CompVis/stable-diffusion-v1-4 as a diffusers folder: plain and fp16 files, the UNet
    /// and text encoder serving the sd14 fixtures' headers, and its other folders.
    fn sd14_diffusers_repo() -> TestRepo {
        let unet = Some("sd14-diffusers-unet");
        let text_encoder = Some("sd14-diffusers-text-encoder");
        TestRepo::new("CompVis/stable-diffusion-v1-4")
            .label("creativeml-openrail-m")
            .plain("model_index.json")
            .file(
                "unet/diffusion_pytorch_model.safetensors",
                3_438_167_534.0,
                unet,
            )
            .file(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                1_719_125_304.0,
                unet,
            )
            .file(
                "text_encoder/model.safetensors",
                492_265_879.0,
                text_encoder,
            )
            .file(
                "text_encoder/model.fp16.safetensors",
                246_144_352.0,
                text_encoder,
            )
            .file(
                "vae/diffusion_pytorch_model.safetensors",
                334_643_268.0,
                None,
            )
            .file(
                "vae/diffusion_pytorch_model.fp16.safetensors",
                167_335_342.0,
                None,
            )
            .file("safety_checker/model.safetensors", 1_215_981_830.0, None)
            .plain("scheduler/scheduler_config.json")
            .file("sd-v1-4.ckpt", 4_265_380_512.0, None)
    }

    /// QuantStack's Wan 2.2 T2V A14B: each quantisation of the high-noise and the low-noise
    /// part, each serving its part's header.
    fn wan_a14b_repo(quants: &[&str]) -> TestRepo {
        quants.iter().enumerate().fold(
            TestRepo::new("QuantStack/Wan2.2-T2V-A14B-GGUF").tagged("text-to-video"),
            |repo, (i, q)| {
                let size = 5e9 + 1e9 * i as f64;
                repo.file(
                    &format!("HighNoise/Wan2.2-T2V-A14B-HighNoise-{q}.gguf"),
                    size,
                    Some("wan22-t2v-a14b-high"),
                )
                .file(
                    &format!("LowNoise/Wan2.2-T2V-A14B-LowNoise-{q}.gguf"),
                    size,
                    Some("wan22-t2v-a14b-low"),
                )
            },
        )
    }

    #[tokio::test]
    async fn a_flux_repo_shows_one_choice_per_quantisation_with_the_best_marked() {
        let scratch = Scratch::new();
        let hub = hub(vec![flux_kontext_repo(&["Q4_K_M", "Q8_0", "F16"])]).await;
        // A 16 GB card: Q8_0 runs one part at a time, F16 only partly on the processor.
        let deps = repo_deps(&hub, gpu(16, 32), &scratch);
        let r = local_repo(&deps, KONTEXT).await.unwrap();
        assert_eq!(
            r.files.iter().map(|f| f.quant.as_str()).collect::<Vec<_>>(),
            ["F16", "Q8_0", "Q4_K_M"]
        );
        assert_eq!(
            r.files
                .iter()
                .filter(|f| f.best)
                .map(|f| f.quant.as_str())
                .collect::<Vec<_>>(),
            ["Q8_0"]
        );
        assert_eq!(r.files[0].plan.fit.word, "slow");
        let p = &r.files[1].plan;
        assert_eq!(p.recipe.id, "flux1-kontext");
        assert_eq!(p.recipe.kind, RecipeKind::PictureEdit);
        assert_eq!(
            p.recipes.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
            recipes_for(SdVersion::Flux)
        );
        assert_eq!(
            p.slots.iter().map(|s| s.role).collect::<Vec<_>>(),
            [
                GenRole::Diffusion,
                GenRole::ClipL,
                GenRole::T5xxl,
                GenRole::Vae
            ]
        );
        assert_eq!(
            p.slots.iter().map(|s| s.source).collect::<Vec<_>>(),
            [
                PartSource::Main,
                PartSource::Companion,
                PartSource::Companion,
                PartSource::Companion
            ]
        );
        assert_eq!(p.to_fetch_bytes, p.total_bytes);
        assert_eq!(
            p.total_bytes,
            12_708_163_776
                + companion("clip-l").size
                + companion("t5xxl-fp8").size
                + companion("ae-flux1").size
        );
        assert!(p.can_download && !p.untested && !p.low_disk && !p.needs_engine);
        assert!(matches!(
            r.verdict,
            Verdict::Works {
                version: SdVersion::Flux,
                ..
            }
        ));
        // One header read labelled all three; the listing, the card and that read were asked.
        assert_eq!(hub.header_reads(), 1);
        assert_eq!((r.read_files, r.unread_files), (1, 0));
        assert_eq!(r.file_views.len(), 3);
        assert_eq!(r.author, "QuantStack");
        assert_eq!(r.summary, None, "no card served");
        // The companions are Apache 2.0; the repo's own licence states none: check it.
        assert_eq!(r.licence.as_ref().map(|l| l.badge.as_str()), Some("red"));
    }

    #[tokio::test]
    async fn a_file_studio_cant_run_is_listed_with_its_reason() {
        let scratch = Scratch::new();
        let hub = hub(vec![lumina_repo()]).await;
        let r = local_repo(
            &repo_deps(&hub, gpu(12, 32), &scratch),
            "Comfy-Org/Lumina-Image-2",
        )
        .await
        .unwrap();
        assert!(r.files.is_empty());
        assert!(r.file_views.iter().any(|f| f.label
            == FileLabel::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: Some(SdVersion::ZImage)
            }));
        assert!(
            r.file_views
                .iter()
                .any(|f| f.label == FileLabel::Extra { role: GenRole::Vae }
                    && f.path == "split_files/vae/ae.safetensors")
        );
        assert_eq!(
            r.verdict,
            Verdict::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: Some(SdVersion::ZImage)
            }
        );
        assert!(r.licences.is_empty() && r.loras.is_empty());
    }

    #[tokio::test]
    async fn the_setup_plan_command_takes_a_recipe_and_an_override() {
        let scratch = Scratch::new();
        let hub = hub(vec![flux_kontext_repo(&["Q4_K_M"])]).await;
        let deps = repo_deps(&hub, gpu(12, 32), &scratch);
        let t5 = companion("t5xxl-fp16");
        let p = setup_plan(
            &deps,
            &search_source(KONTEXT, "flux1-kontext-dev-Q4_K_M.gguf"),
            &SetupChoice {
                recipe: Some("flux1-dev".into()),
                overrides: vec![SlotOverride {
                    role: GenRole::T5xxl,
                    file: SlotFile::Hub {
                        repo: "comfyanonymous/flux_text_encoders".into(),
                        commit: t5.revision.clone(),
                        path: "t5xxl_fp16.safetensors".into(),
                    },
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(p.recipe.id, "flux1-dev");
        assert_eq!(p.recipe.kind, RecipeKind::Picture);
        assert!(!p.untested); // flux1-dev is among the version's recipes
        assert_eq!(p.slots[2].role, GenRole::T5xxl);
        assert_eq!(p.slots[2].source, PartSource::Chosen);
        // The chosen file is the companion table's own row: pinned by it, under its licence.
        let part = p.slots[2].part.as_ref().unwrap();
        assert_eq!(
            (part.repo.as_str(), part.size_bytes),
            ("comfyanonymous/flux_text_encoders", t5.size)
        );
        let licence = p.licences.iter().find(|l| l.id == part.licence_id).unwrap();
        assert_eq!(licence.name, "Apache License 2.0");
        assert!(p.can_download);
        // A recipe of another version is untested; one the engine does not have is refused.
        let other = SetupChoice {
            recipe: Some("sdxl-checkpoint".into()),
            overrides: Vec::new(),
        };
        let source = search_source(KONTEXT, "flux1-kontext-dev-Q4_K_M.gguf");
        assert!(setup_plan(&deps, &source, &other).await.unwrap().untested);
        let unknown = SetupChoice {
            recipe: Some("no-such-recipe".into()),
            overrides: Vec::new(),
        };
        assert_eq!(
            setup_plan(&deps, &source, &unknown).await.unwrap_err().code,
            "bad_request"
        );
        let left_out = SetupChoice {
            recipe: Some("sd1-controlnet".into()),
            overrides: Vec::new(),
        };
        assert_eq!(
            setup_plan(&deps, &source, &left_out)
                .await
                .unwrap_err()
                .code,
            "bad_request"
        );
    }

    /// Ruling S4-licence-ref: a file the person chose from another repo is under that repo's
    /// label, read at its commit, never the main repo's.
    #[tokio::test]
    async fn a_chosen_file_of_another_repo_has_that_repos_licence() {
        let scratch = Scratch::new();
        let other_commit = "1".repeat(40);
        let vae_repo = TestRepo::new("someone/my-flux-vae")
            .label("mit")
            .at(&other_commit)
            .file("my_ae.safetensors", 335_000_000.0, None);
        let hub = hub(vec![
            flux_kontext_repo(&["Q4_K_M"]).label("creativeml-openrail-m"),
            vae_repo,
        ])
        .await;
        let deps = repo_deps(&hub, gpu(12, 32), &scratch);
        let p = setup_plan(
            &deps,
            &search_source(KONTEXT, "flux1-kontext-dev-Q4_K_M.gguf"),
            &SetupChoice {
                recipe: None,
                overrides: vec![SlotOverride {
                    role: GenRole::Vae,
                    file: SlotFile::Hub {
                        repo: "someone/my-flux-vae".into(),
                        commit: other_commit.clone(),
                        path: "my_ae.safetensors".into(),
                    },
                }],
            },
        )
        .await
        .unwrap();
        let licence_of = |role: GenRole| {
            let part = p
                .slots
                .iter()
                .find(|s| s.role == role)
                .and_then(|s| s.part.as_ref())
                .unwrap();
            p.licences
                .iter()
                .find(|l| l.id == part.licence_id)
                .unwrap()
                .clone()
        };
        let vae = licence_of(GenRole::Vae);
        assert_eq!(
            (vae.name.as_str(), vae.badge.as_str()),
            ("MIT License", "green")
        );
        assert!(vae.url.contains("someone/my-flux-vae"), "{}", vae.url);
        let main = licence_of(GenRole::Diffusion);
        assert_eq!(
            (main.commercial.as_str(), main.badge.as_str()),
            ("withUseRules", "amber")
        );
        // The other repo's listing was asked at the chosen commit.
        assert!(hub.requests().iter().any(|l| l.contains(&format!(
            "/api/models/someone/my-flux-vae/revision/{other_commit}"
        ))));
        // A file that repo does not list is not found.
        let missing = SetupChoice {
            recipe: None,
            overrides: vec![SlotOverride {
                role: GenRole::Vae,
                file: SlotFile::Hub {
                    repo: "someone/my-flux-vae".into(),
                    commit: other_commit,
                    path: "nothing.safetensors".into(),
                },
            }],
        };
        let source = search_source(KONTEXT, "flux1-kontext-dev-Q4_K_M.gguf");
        assert_eq!(
            setup_plan(&deps, &source, &missing).await.unwrap_err().code,
            "not_found"
        );
    }

    #[tokio::test]
    async fn a_diffusers_repo_is_one_choice_whose_plan_lists_the_folder_files() {
        let scratch = Scratch::new();
        let hub = hub(vec![sd14_diffusers_repo()]).await;
        let r = local_repo(
            &repo_deps(&hub, gpu(8, 32), &scratch),
            "CompVis/stable-diffusion-v1-4",
        )
        .await
        .unwrap();
        assert_eq!(r.files.len(), 1);
        assert_eq!(
            (
                r.files[0].quant.as_str(),
                r.files[0].plan.recipe.id.as_str()
            ),
            ("fp16", "sd1-checkpoint")
        );
        assert_eq!(
            r.files[0].file,
            "unet/diffusion_pytorch_model.fp16.safetensors"
        );
        assert_eq!(
            r.files[0]
                .plan
                .slots
                .iter()
                .map(|s| (s.role, s.source))
                .collect::<Vec<_>>(),
            [
                (GenRole::Model, PartSource::Main),
                (GenRole::Vae, PartSource::Folder),
                (GenRole::ClipL, PartSource::Folder)
            ]
        );
        assert!(r.files[0].best);
        // Its licence is the repo's OpenRAIL label: amber, with use rules.
        let licence = r.licence.as_ref().unwrap();
        assert_eq!(
            (licence.badge.as_str(), licence.commercial.as_str()),
            ("amber", "withUseRules")
        );
        assert_eq!(r.licences, std::slice::from_ref(licence));
        assert!(r.file_views.iter().any(|f| f.path == "sd-v1-4.ckpt"
            && f.label
                == FileLabel::CannotRun {
                    reason: CannotRunReason::Pickle,
                    version: None
                }));
    }

    /// An A14B pair is one choice per quantisation: the low-noise half is in its high-noise
    /// half's plan, never a choice of its own.
    #[tokio::test]
    async fn an_a14b_pair_is_one_choice_per_quantisation() {
        let scratch = Scratch::new();
        let hub = hub(vec![wan_a14b_repo(&["Q4_K_M", "Q8_0"])]).await;
        let r = local_repo(
            &repo_deps(&hub, gpu(24, 64), &scratch),
            "QuantStack/Wan2.2-T2V-A14B-GGUF",
        )
        .await
        .unwrap();
        assert_eq!(
            r.files.iter().map(|f| f.file.as_str()).collect::<Vec<_>>(),
            [
                "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q8_0.gguf",
                "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf"
            ]
        );
        let slots = &r.files[1].plan.slots;
        let sibling = slots.iter().find(|s| s.role == GenRole::Diffusion).unwrap();
        assert_eq!(sibling.source, PartSource::Sibling);
        assert_eq!(
            sibling.part.as_ref().unwrap().path,
            "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf"
        );
        assert_eq!(r.file_views.len(), 4);
    }

    /// A part already on this computer (another model's shared file) is checked, not fetched.
    #[tokio::test]
    async fn a_shared_part_already_installed_is_not_fetched_again() {
        let scratch = Scratch::new();
        let hub = hub(vec![flux_kontext_repo(&["Q4_K_M"])]).await;
        let mut deps = repo_deps(&hub, gpu(12, 32), &scratch);
        let ae = companion("ae-flux1");
        let file = destination(&deps.models_dir, &ae.repo, &ae.path).unwrap();
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, vec![0u8; usize::try_from(ae.size).unwrap()]).unwrap();
        let part = GenPart {
            role: GenRole::Vae,
            repo: ae.repo.clone(),
            revision: ae.revision.clone(),
            path: ae.path.clone(),
            size: ae.size,
            sha256: ae.sha256.clone(),
            license: ae.license.clone(),
            local: None,
            folder: None,
        };
        install(
            &deps.store,
            &deps.models_dir,
            "someone-elses-model",
            None,
            "Another model",
            "z-image",
            &[&part],
        )
        .await;
        deps.installed_files = InstalledFiles::read(&deps.store).await.unwrap();
        let r = local_repo(&deps, KONTEXT).await.unwrap();
        let p = &r.files[0].plan;
        let vae = p.slots.iter().find(|s| s.role == GenRole::Vae).unwrap();
        assert_eq!(vae.source, PartSource::Installed);
        let vae_part = vae.part.as_ref().unwrap();
        assert!(vae_part.already_here && !vae_part.local);
        assert_eq!(vae_part.repo, ae.repo);
        assert_eq!(p.to_fetch_bytes, p.total_bytes - ae.size);
        assert_eq!(
            p.licences
                .iter()
                .find(|l| l.id == vae_part.licence_id)
                .map(|l| l.name.as_str()),
            Some("Apache License 2.0")
        );
    }

    /// A gated repo without a token: its files say they need one, no header is read, and
    /// the verdict stays its name's.
    #[tokio::test]
    async fn a_gated_repo_without_a_token_offers_no_choice_yet() {
        let scratch = Scratch::new();
        let mut repo = flux_kontext_repo(&["Q4_K_M"]);
        repo.gated = true;
        let hub = hub(vec![repo]).await;
        let r = local_repo(&repo_deps(&hub, gpu(12, 32), &scratch), KONTEXT)
            .await
            .unwrap();
        assert_eq!(hub.header_reads(), 0);
        assert!(r.files.is_empty());
        assert_eq!(
            r.file_views[0].label,
            FileLabel::NotRead {
                reason: models::sd::NotRead::NeedsToken
            }
        );
        assert_eq!((r.read_files, r.unread_files), (0, 1));
        assert_eq!(r.gate.mode, "auto");
        // The name's hint, from its pipeline tag.
        assert_eq!(
            r.verdict,
            Verdict::Works {
                version: SdVersion::Flux,
                kind: RecipeKind::PictureEdit
            }
        );
    }

    #[tokio::test]
    async fn a_repo_shows_its_cards_first_paragraph() {
        let scratch = Scratch::new();
        let card = "---\nlicense: other\ntags:\n- flux\n---\n\n# FLUX.1 Kontext GGUF\n\n![grid](grid.png)\n\nQuantised **FLUX.1 Kontext [dev]**\nfor\tstable-diffusion.cpp.\u{202E}\n\nMore below.\n";
        let hub = hub(vec![flux_kontext_repo(&["Q4_K_M"]).card(card)]).await;
        let r = local_repo(&repo_deps(&hub, gpu(12, 32), &scratch), KONTEXT)
            .await
            .unwrap();
        assert_eq!(
            r.summary.as_deref(),
            Some("Quantised **FLUX.1 Kontext [dev]** for stable-diffusion.cpp.[U+202E]")
        );
        assert_eq!(card_summary("---\na: b\n---\n# Only a heading\n"), None);
        assert_eq!(card_summary("Plain text."), Some("Plain text.".to_owned()));
        assert_eq!(
            card_summary(&"word ".repeat(200)).unwrap().chars().count(),
            400
        );
    }

    /// A file on this computer is planned from its header, its parts from the companion
    /// table; one that is not a model file Studio reads is refused by its extension.
    #[tokio::test]
    async fn a_file_on_this_computer_is_planned_from_its_header() {
        let scratch = Scratch::new();
        let hub = hub(vec![]).await;
        let deps = repo_deps(&hub, gpu(12, 32), &scratch);
        let dir = scratch.path().join("ü dir");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("my kontext.gguf");
        std::fs::write(&path, file_of(&fixture("flux1-kontext"))).unwrap();
        let source = LocalSource::Disk {
            path: path.display().to_string(),
        };
        let p = setup_plan(&deps, &source, &SetupChoice::default())
            .await
            .unwrap();
        assert_eq!(p.recipe.id, "flux1-kontext");
        let main = p.slots[0].part.as_ref().unwrap();
        assert!(main.local && main.already_here && main.repo.is_empty());
        assert_eq!(main.licence_id, "");
        assert_eq!(p.to_fetch_bytes, p.total_bytes - main.size_bytes);
        assert!(hub.requests().is_empty(), "{:?}", hub.requests());
        // Its fit comes from the same plan.
        let fit = local_fit(&deps, &source, 1024, 1024, 0).await.unwrap();
        assert_eq!(fit.word, p.fit.word);
        for (name, code, message) in [
            ("m.ckpt", "bad_request", "Studio can't read .ckpt files"),
            (
                "notes.txt",
                "bad_request",
                "This is not a model file Studio can read.",
            ),
        ] {
            let other = dir.join(name);
            std::fs::write(&other, b"x").unwrap();
            let e = setup_plan(
                &deps,
                &LocalSource::Disk {
                    path: other.display().to_string(),
                },
                &SetupChoice::default(),
            )
            .await
            .unwrap_err();
            assert_eq!((e.code.as_str(), e.message.as_str()), (code, message));
        }
        let gone = LocalSource::Disk {
            path: dir.join("gone.gguf").display().to_string(),
        };
        assert_eq!(
            setup_plan(&deps, &gone, &SetupChoice::default())
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    /// A plan is for a searched file or a file on this computer; a file Studio does not run,
    /// or one the repo does not list, has none.
    #[tokio::test]
    async fn a_plan_is_refused_for_a_file_studio_does_not_run() {
        let scratch = Scratch::new();
        let hub = hub(vec![lumina_repo()]).await;
        let deps = repo_deps(&hub, gpu(12, 32), &scratch);
        let repo = "Comfy-Org/Lumina-Image-2";
        for (file, code) in [
            (
                "split_files/diffusion_models/lumina_2_model_bf16.safetensors",
                "studio_not_supported_yet",
            ),
            ("split_files/vae/ae.safetensors", "studio_not_supported_yet"),
            ("not-listed.safetensors", "not_found"),
        ] {
            let e = setup_plan(&deps, &search_source(repo, file), &SetupChoice::default())
                .await
                .unwrap_err();
            assert_eq!(e.code, code, "{file}");
        }
        let catalog = LocalSource::Catalog {
            entry_id: KLEIN.into(),
        };
        assert_eq!(
            setup_plan(&deps, &catalog, &SetupChoice::default())
                .await
                .unwrap_err()
                .code,
            "studio_not_supported_yet"
        );
    }

    // ---- the search -----------------------------------------------------------------------

    /// A search asks Hugging Face its three queries (the two picture pipeline tags, then none)
    /// and gives one page, most downloads first (the untagged hit, asked last, comes first):
    /// each hit says who made it, how popular the repository is, when it changed, how it is
    /// gated in a gate's words, its own licence label and what its name says it offers; an
    /// untagged hit stays only when its name is a picture model's.
    #[tokio::test]
    async fn a_search_hit_says_what_hugging_face_lists() {
        let hub = Hub::new(|line| {
            let found = if line.contains("pipeline_tag=text-to-image") {
                json!([{"id": "leejet/Z-Image-Turbo-GGUF", "downloads": 9, "likes": 2,
                        "gated": "manual", "tags": ["gguf", "license:apache-2.0"],
                        "pipeline_tag": "text-to-image", "lastModified": "2026-10-01T00:00:00.000Z"}])
            } else if line.contains("pipeline_tag=") {
                json!([])
            } else {
                json!([
                    {"id": "someone/Z-Image-Turbo-Q8-GGUF", "downloads": 40, "likes": 1,
                     "gated": false, "tags": ["gguf"]},
                    {"id": "someone/Qwen3-4B-GGUF", "downloads": 99, "likes": 7,
                     "gated": false, "tags": ["gguf", "license:apache-2.0"]}
                ])
            };
            ("200 OK", found.to_string())
        })
        .await;
        let page = local_search(
            &hub.client(),
            &DetectService::new(),
            "z-image",
            SearchFilter::Pictures,
        )
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
        let z_image = Verdict::Works {
            version: SdVersion::ZImage,
            kind: RecipeKind::Picture,
        };
        assert_eq!(
            page.items,
            [
                LocalSearchHitDto {
                    repo: "someone/Z-Image-Turbo-Q8-GGUF".into(),
                    author: "someone".into(),
                    downloads: 40,
                    likes: 1,
                    updated_at: None,
                    gated: "none".into(),
                    license_label: None,
                    verdict: z_image.clone(),
                    exact: false,
                },
                LocalSearchHitDto {
                    repo: "leejet/Z-Image-Turbo-GGUF".into(),
                    author: "leejet".into(),
                    downloads: 9,
                    likes: 2,
                    updated_at: Some("2026-10-01T00:00:00.000Z".into()),
                    gated: "manual".into(),
                    license_label: Some("apache-2.0".into()),
                    verdict: z_image,
                    exact: false,
                },
            ]
        );
    }

    fn summary(id: &str, tag: Option<&str>, updated: Option<&str>) -> RepoSummary {
        RepoSummary {
            id: id.into(),
            downloads: 1,
            likes: 0,
            gated: models::hf::Gated::No,
            license: None,
            last_modified: updated.map(str::to_owned),
            pipeline_tag: tag.map(str::to_owned),
            not_for_all_audiences: false,
        }
    }

    #[test]
    fn hits_carry_author_update_and_a_verdict() {
        let page = search_page(
            vec![summary(
                "QuantStack/Wan2.2-T2V-A14B-GGUF",
                Some("text-to-video"),
                Some("2026-10-01T00:00:00.000Z"),
            )],
            &DetectService::new(),
        );
        assert_eq!(
            (page.items[0].author.as_str(), page.items[0].exact),
            ("QuantStack", false)
        );
        assert_eq!(
            page.items[0].updated_at.as_deref(),
            Some("2026-10-01T00:00:00.000Z")
        );
        assert!(matches!(
            page.items[0].verdict,
            Verdict::Works {
                kind: RecipeKind::Video,
                ..
            }
        ));
    }

    /// Once a repo was opened, its hit's verdict is what reading its files found, over its
    /// name's hint.
    #[tokio::test]
    async fn an_opened_repos_hit_carries_its_exact_verdict() {
        let scratch = Scratch::new();
        let hub = hub(vec![lumina_repo()]).await;
        let deps = repo_deps(&hub, gpu(12, 32), &scratch);
        // Before: the name says nothing it knows.
        let before = search_page(
            vec![summary(
                "Comfy-Org/Lumina-Image-2",
                Some("text-to-image"),
                None,
            )],
            &deps.detect,
        );
        assert_eq!(
            (before.items[0].verdict.clone(), before.items[0].exact),
            (
                Verdict::CannotRun {
                    reason: CannotRunReason::Lumina2,
                    version: None
                },
                false
            )
        );
        local_repo(&deps, "Comfy-Org/Lumina-Image-2").await.unwrap();
        let after = search_page(
            vec![summary(
                "Comfy-Org/Lumina-Image-2",
                Some("text-to-image"),
                None,
            )],
            &deps.detect,
        );
        assert_eq!(
            (after.items[0].verdict.clone(), after.items[0].exact),
            (
                Verdict::CannotRun {
                    reason: CannotRunReason::Lumina2,
                    version: Some(SdVersion::ZImage)
                },
                true
            )
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

    /// A Studio manager on `store` with no local lane: nothing runs on this computer.
    fn idle_manager(store: &store::Store, scratch: &Scratch) -> StudioManager {
        StudioManager::new(
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
        )
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
