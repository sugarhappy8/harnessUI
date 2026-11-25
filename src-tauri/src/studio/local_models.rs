//! Downloading a picture or video model to this computer (studio-local-design.md, "Get local
//! models"; 0.1.0 Task L11).
//!
//! - **Which model.** A source ([`LocalSource`]) names an entry of the signed catalog's
//!   recommended list, a file of a Hugging Face repository at a commit (its listing read again
//!   here and its set-up planned, the webview's word never taken for it), a file on this
//!   computer (planned from its header), or an installed model (its saved parts, with no
//!   request).
//! - **A set-up plan** ([`plan_source`], Task S6): a searched or local file's recipe and the
//!   file for each slot (`models::sd::setup::plan_setup`), with the person's choices, as the
//!   generic entry it makes. Each fetched file is under its companion licence or its own
//!   repo's label (ruling S4-licence-ref).
//! - **What it takes** ([`plan_download`]): every part, pinned by repository, revision, size
//!   and SHA-256, lands at `<models>/<owner>/<repo>/<path>`, so a part already there (a shared
//!   text encoder) is checked instead of fetched; where another installed model uses another
//!   revision of the file there, the part lands at `<models>/<owner>/<repo>/@<commit>/<path>`
//!   instead, never renamed over it. A diffusers folder's files land in one folder under the
//!   engine's names, `<models>/<owner>/<repo>[/@<commit>][/@fp16]/<name>`. A part the set-up
//!   plan takes from this computer (an installed model's file, or the person's own) is used
//!   where it is, never fetched. A Hub file with no published SHA-256 is never fetched, and a
//!   download that does not fit on the disk is refused before anything starts.
//! - **Before downloading**, Studio shows the model licences and asks first-time users to read the local model notice.
//! - **The download** ([`start_local_download`]): one group of the download manager whose parts
//!   are exactly the set-up plan's, each Hub part at its own revision; a set-up that misses a
//!   part, or a gated repository's file without a Hugging Face token, is refused before any
//!   group starts. Once every part is checked, and the model file's own header is a model its
//!   recipe runs, it joins Studio's installed models with its recipe
//!   (`downloads::register_gen_model`). The Windows engine pack comes with the first model;
//!   where no engine is found and none can come, the model still downloads, and Get local
//!   models tells it from the engine's state.
//! - **Deleting** ([`delete_local_model`]): refused while a run on this computer waits for the
//!   model or uses it; else the row goes, then each part file nothing else uses (a chat model,
//!   another picture or video model, a download not finished yet), then the folders that
//!   leaves empty. A file from this computer is the person's own and stays, the models folder
//!   included; only an import's copy under `<models>/@imported/` goes with its model.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use models::download::FileSpec;

use crate::hub::{UsedFile, chat_files_used, download_destination, gen_files_used};
use models::files::{RepoFile, quant_label};
use models::gen_catalog::{
    FamilyRecord, GateMode, GenCatalog, GenEntry, GenFamily, GenGate, GenLicence, GenPart, GenRole,
    GenericSpec, gen_catalog, generic_entry,
};
use models::gen_search::{repo_licence, searched_licence, unread_licence};
use models::hf::{Gated, HfClient, HfError, HubInfo, RepoDetail};
use models::sd::candidates::candidates;
use models::sd::companions::CompanionTable;
use models::sd::detect::SdVersion;
use models::sd::index::{PICKLE_EXTENSIONS, READ_EXTENSIONS};
use models::sd::inspect::inspect_local;
use models::sd::matching::{NoisePart, out_of_scope, recipes_for};
use models::sd::recipes::{SlotId, recipe};
use models::sd::setup::{
    InstalledFile, LicenceRef, MainFile, PartSource, PlannedFile, PlannedPart, SetupInput,
    SetupPlan, SlotChoice, plan_setup,
};
use models::sd::{FileFacts, FileLabel, NotRead, label_for};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::detect::{DetectService, RepoReading};
use super::engine_cmd::EngineNeed;
use super::errors;
use super::manager::StudioManager;
use crate::downloads::{DownloadManager, GenGroup, GenGroupPart, GroupMeta, files_being_written};
use crate::dto::{SetupChoice, SlotFile, SlotOverride};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The model a local download, fit or set-up plan is for, as the webview names it:
/// `{"catalog":{"entryId":…}}`, `{"search":{"repo":…,"commit":…,"file":…}}`,
/// `{"installed":{"genModelId":…}}` or `{"disk":{"path":…}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum LocalSource {
    /// An entry of the signed catalog's recommended list.
    Catalog { entry_id: String },
    /// A file of a Hugging Face repository at a commit, found by search.
    Search {
        repo: String,
        commit: String,
        file: String,
    },
    /// An installed model: its saved parts, with no request (its fit, and *Download again*,
    /// which checks every part and fetches what is missing or damaged).
    Installed { gen_model_id: String },
    /// A model file on this computer (untrusted: checked before it is read), set up as a
    /// download is; it is added, never downloaded.
    Disk { path: String },
}

/// What downloading an entry takes on this computer.
#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub entry: GenEntry,
    /// Every Hub part's pinned file and where it goes, in the parts' order, a part already
    /// in its place included: the download checks that one against its SHA-256 instead of
    /// fetching it. A part [`present`](Self::present) has none.
    pub files: Vec<(FileSpec, PathBuf)>,
    /// For each part, in the parts' order, the file on this computer it uses where it is,
    /// never fetched nor checked: a file from this computer (`local`), and for a set-up plan
    /// a file an installed model uses.
    pub present: Vec<Option<PathBuf>>,
    /// The places in [`files`](Self::files) whose file is already there with its pinned size.
    pub already_here: Vec<PathBuf>,
    /// Every part's size.
    pub total_bytes: u64,
    /// The parts not in place yet: what is fetched.
    pub to_fetch_bytes: u64,
    /// Every licence of every part, each once, in the parts' order, then the Studio engine's
    /// when it downloads with the model: shown in the confirmation.
    pub licences: Vec<GenLicence>,
    /// The Studio engine is not on this computer and this platform has it as a pack (Windows):
    /// it downloads with the model, its licence among [`licences`](Self::licences).
    pub needs_engine: bool,
    /// The free space on the models folder's disk, when the system says.
    pub free_bytes: Option<u64>,
}

/// Below this much free space left after a download, Get local models says the disk is
/// nearly full.
pub const LOW_DISK_BYTES: u64 = 10_000_000_000;

impl DownloadPlan {
    /// Less than [`LOW_DISK_BYTES`] would be left on the disk after the download.
    pub fn low_disk(&self) -> bool {
        self.free_bytes
            .is_some_and(|free| free.saturating_sub(self.to_fetch_bytes) < LOW_DISK_BYTES)
    }
}

/// The files the installed models use, which a download never renames over another
/// revision of ([`plan_download`]).
#[derive(Debug, Clone, Default)]
pub struct InstalledFiles {
    /// The installed chat models' files.
    pub chat: Vec<UsedFile>,
    /// The installed picture and video models.
    pub gen_models: Vec<store::GenModel>,
}

impl InstalledFiles {
    /// The installed models in `store` and their files.
    pub async fn read(store: &store::Store) -> AppResult<Self> {
        Ok(Self {
            chat: chat_files_used(&store.list_models().await?),
            gen_models: store.list_gen_models().await?,
        })
    }

    /// The files models other than picture or video model `id` (the one being downloaded,
    /// perhaps again) use.
    pub fn except(&self, id: &str) -> Vec<UsedFile> {
        let mut used = self.chat.clone();
        used.extend(gen_files_used(&self.gen_models, Some(id)));
        used
    }
}

/// What downloading `entry` into `models_dir` takes: one pinned file per part (`commit` the
/// part's revision) that is not [`present`](DownloadPlan::present) — a part from this computer
/// (`local`), or with the set-up plan `setup` that made `entry` (its parts are the plan's
/// files, in its order) a file the plan takes from this computer, is used where it is. Each
/// lands at `<models>/<owner>/<repo>/<path>` unless a file there that another installed model
/// uses (`used`: chat models, and picture and video models other than this one) is another
/// revision's (`hub::download_destination`: the part then has a place of its own, never
/// renamed over that file); a diffusers folder's parts land together under the engine's names
/// ([`folder_place`]). A place that already has its part's pinned size counts as here. With
/// every licence to show, and what the Studio engine needs (`engine`,
/// `engine_cmd::engine_need`): its pack comes too, with its licence; an engine that is neither
/// here nor a pack changes nothing here (Get local models tells it from the engine's state).
/// Refused with `studio_no_checksum` for a Hub file with no published SHA-256 (it could not
/// be checked), and with `studio_disk_too_small` when what it fetches is more than
/// `free_bytes` (when known).
pub fn plan_download(
    entry: GenEntry,
    setup: Option<&SetupPlan>,
    models_dir: &Path,
    engine: &EngineNeed,
    free_bytes: Option<u64>,
    used: &[UsedFile],
) -> AppResult<DownloadPlan> {
    let present = present_parts(&entry, setup)?;
    let folder = folder_place(&entry, &present, models_dir, used)?;
    let mut files = Vec::with_capacity(entry.parts.len());
    let mut already_here = Vec::new();
    let mut to_fetch_bytes = 0;
    for (part, here) in entry.parts.iter().zip(&present) {
        if here.is_some() {
            continue;
        }
        if part.sha256.is_empty() {
            return Err(errors::no_published_checksum());
        }
        let dest = match (&part.folder, &folder) {
            (Some(name), Some(root)) => in_folder(root, name)?,
            _ => download_destination(
                models_dir,
                &part.repo,
                &part.path,
                &part.revision,
                Some(&part.sha256),
                used,
            )
            .map_err(|e| AppError::internal(format!("a part's place: {e}")))?,
        };
        if std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == part.size) {
            already_here.push(dest.clone());
        } else {
            to_fetch_bytes += part.size;
        }
        let spec = FileSpec {
            repo: part.repo.clone(),
            commit: part.revision.clone(),
            path: part.path.clone(),
            size: part.size,
            sha256: Some(part.sha256.clone()),
        };
        files.push((spec, dest));
    }
    if free_bytes.is_some_and(|free| to_fetch_bytes > free) {
        return Err(errors::disk_too_small(to_fetch_bytes));
    }
    let mut licences: Vec<GenLicence> = Vec::new();
    // A file from this computer is the person's own: it names no licence.
    for part in entry.parts.iter().filter(|p| p.local.is_none()) {
        let licence = entry.licenses.get(&part.license).ok_or_else(|| {
            AppError::internal(format!("a part's licence {} is not listed", part.license))
        })?;
        if !licences.iter().any(|l| l.id == licence.id) {
            licences.push(licence.clone());
        }
    }
    let needs_engine = match engine {
        EngineNeed::Found | EngineNeed::Missing => false,
        EngineNeed::Pack(licence) => {
            if !licences.iter().any(|l| l.sha256 == licence.sha256) {
                licences.push(licence.clone());
            }
            true
        }
    };
    Ok(DownloadPlan {
        total_bytes: entry.total_bytes(),
        entry,
        files,
        present,
        already_here,
        to_fetch_bytes,
        licences,
        needs_engine,
        free_bytes,
    })
}

/// For each of `entry`'s parts, the file on this computer it uses where it is: a part from
/// this computer's own (`local`), and with the set-up plan that made the entry, each part the
/// plan takes from this computer (`PlannedFile::Local`: an installed model's file, or the
/// person's). The entry's parts are the plan's files that fill a slot, in its order
/// (`plan_with`): each pair of the same role, and a file from this computer at the part's own
/// path or with its SHA-256 (when the plan knows none, a file an installed model uses, whose
/// part has one). A plan that does not match is an internal error.
fn present_parts(entry: &GenEntry, setup: Option<&SetupPlan>) -> AppResult<Vec<Option<PathBuf>>> {
    let mut present: Vec<Option<PathBuf>> = entry.parts.iter().map(|p| p.local.clone()).collect();
    let Some(setup) = setup else {
        return Ok(present);
    };
    let files: Vec<(&PlannedPart, &PlannedFile)> = setup
        .parts
        .iter()
        .filter(|p| GenRole::from_slot(p.slot).is_some())
        .filter_map(|p| Some((p, p.file.as_ref()?)))
        .collect();
    let not_its = || AppError::internal("a set-up plan that is not its entry's");
    if files.len() != present.len() {
        return Err(not_its());
    }
    for ((here, part), (planned, file)) in present.iter_mut().zip(&entry.parts).zip(files) {
        if GenRole::from_slot(planned.slot) != Some(part.role) {
            return Err(not_its());
        }
        if let PlannedFile::Local { path, sha256, .. } = file {
            let same_path = part.local.as_ref() == Some(path);
            let same_sha = match sha256 {
                Some(sha) => sha.eq_ignore_ascii_case(&part.sha256),
                None => part.local.is_none() && !part.sha256.is_empty(),
            };
            if !(same_path || same_sha) {
                return Err(not_its());
            }
            *here = Some(path.clone());
        }
    }
    Ok(present)
}

/// Where the diffusers folder among `entry`'s parts to fetch goes, so the engine finds each
/// file under its own name in one folder: `<models>/<owner>/<repo>`, then `@<commit, 12
/// characters>` when another installed model (`used`) uses another file at any of its places
/// (the whole folder moves, never a part of it), then `@fp16` for a folder of fp16 files (its
/// files have the `.fp16` names in the repo), so the two precisions never share a file.
/// `None` without folder parts.
fn folder_place(
    entry: &GenEntry,
    present: &[Option<PathBuf>],
    models_dir: &Path,
    used: &[UsedFile],
) -> AppResult<Option<PathBuf>> {
    let parts: Vec<(&GenPart, &str)> = entry
        .parts
        .iter()
        .zip(present)
        .filter(|(_, here)| here.is_none())
        .filter_map(|(p, _)| Some((p, p.folder.as_deref()?)))
        .collect();
    let Some((first, _)) = parts.first() else {
        return Ok(None);
    };
    if parts
        .iter()
        .any(|(p, _)| p.repo != first.repo || p.revision != first.revision)
    {
        return Err(AppError::internal(
            "a folder's files from more than one revision",
        ));
    }
    let place = |e: models::hf::HfError| AppError::internal(format!("a folder's place: {e}"));
    let fp16 = parts.iter().any(|(p, name)| !p.path.ends_with(name));
    let inside = |name: &str| {
        if fp16 {
            format!("{FP16_FOLDER}/{name}")
        } else {
            name.to_owned()
        }
    };
    let mut moved = false;
    for (p, name) in &parts {
        let inside = inside(name);
        let usual = models::download::destination(models_dir, &p.repo, &inside).map_err(place)?;
        let dest = download_destination(
            models_dir,
            &p.repo,
            &inside,
            &p.revision,
            Some(&p.sha256),
            used,
        )
        .map_err(place)?;
        moved |= dest != usual;
    }
    let (owner, name) = models::hf::validate_repo_id(&first.repo).map_err(place)?;
    let mut root = models_dir.join(owner).join(name);
    if moved {
        let tag: String = first.revision.chars().take(12).collect();
        if tag.is_empty() || !tag.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(place(models::hf::HfError::InvalidName(
                first.revision.clone(),
            )));
        }
        root.push(format!("@{tag}"));
    }
    if fp16 {
        root.push(FP16_FOLDER);
    }
    Ok(Some(root))
}

/// The folder of a diffusers folder's fp16 files, inside its repo's place.
const FP16_FOLDER: &str = "@fp16";

/// The place of a diffusers folder's file `name` (its engine name) in the folder at `root`.
fn in_folder(root: &Path, name: &str) -> AppResult<PathBuf> {
    let mut dest = root.to_path_buf();
    for segment in models::hf::validate_repo_path(name)
        .map_err(|e| AppError::internal(format!("a folder file's name: {e}")))?
    {
        dest.push(segment);
    }
    Ok(dest)
}

/// The catalog's picture and video models as this build's Studio engine can run them.
pub(crate) async fn current_gen_catalog(app: &AppHandle) -> GenCatalog {
    gen_catalog(
        &crate::hub::current_catalog(app).await,
        runtime::sd::lock::embedded().sd_cpp.build,
    )
}

/// The entry a source names, and how its installed row will be recorded.
struct Resolved {
    entry: GenEntry,
    catalog_id: Option<String>,
    source: store::GenSource,
}

/// The entry `source` names, with the catalog id and the origin its installed row records:
/// from the catalog (an entry it hides is `studio_not_supported_yet`); for a searched file
/// the generic entry its set-up plan makes, with nothing chosen ([`plan_source`]: the
/// repository's listing at that commit, read again here, so the webview's word is never
/// taken for it); for a file on this computer the same from its header; for an installed
/// model from its saved parts in `store` and its family record. Only a search source sends a
/// request.
async fn resolve_with(
    store: &store::Store,
    deps: &PlanDeps<'_>,
    source: &LocalSource,
) -> AppResult<Resolved> {
    let catalog = deps.catalog;
    match source {
        LocalSource::Catalog { entry_id } => {
            let entry = match catalog.entries.get(entry_id) {
                Some(entry) => entry.clone(),
                None => {
                    return Err(match catalog.hidden.iter().find(|h| h.id == *entry_id) {
                        Some(hidden) => errors::not_supported_yet(hidden.reason.clone()),
                        None => AppError::not_found("model"),
                    });
                }
            };
            Ok(Resolved {
                entry,
                catalog_id: Some(entry_id.clone()),
                source: store::GenSource::Catalog,
            })
        }
        LocalSource::Search { .. } | LocalSource::Disk { .. } => {
            let planned = plan_source(deps, source, &SetupChoice::default()).await?;
            Ok(Resolved {
                entry: planned.entry,
                catalog_id: None,
                source: match source {
                    LocalSource::Disk { .. } => store::GenSource::Import,
                    _ => store::GenSource::Search,
                },
            })
        }
        LocalSource::Installed { gen_model_id } => {
            let model = store
                .get_gen_model(gen_model_id)
                .await?
                .ok_or_else(|| AppError::not_found("model"))?;
            Ok(Resolved {
                entry: entry_from_installed(&model, catalog)?,
                catalog_id: model.catalog_id.clone(),
                source: model.source,
            })
        }
    }
}

/// The entry `source` names ([`resolve_with`]): a request only for a search source's
/// listing.
pub(crate) async fn entry_with(
    store: &store::Store,
    deps: &PlanDeps<'_>,
    source: &LocalSource,
) -> AppResult<GenEntry> {
    Ok(resolve_with(store, deps, source).await?.entry)
}

// ---- set-up plans (design section 7.4) --------------------------------------------------------

/// What planning a searched or local file's set-up reads beside the file: the Hub (a
/// searched file's repo, a chosen file of another repo), the repos read so far, the
/// catalog's companion table, and the files the installed models use.
pub(crate) struct PlanDeps<'a> {
    pub hf: Option<&'a HfClient>,
    pub detect: &'a DetectService,
    pub catalog: &'a GenCatalog,
    pub installed: &'a InstalledFiles,
}

/// A searched repo's listing at a commit, and what reading its files found
/// ([`DetectService::read_repo`]).
pub(crate) struct ReadRepo {
    pub detail: RepoDetail,
    pub reading: RepoReading,
}

impl ReadRepo {
    /// `repo` at `commit` (its latest when `None`), its files read through `detect` (each
    /// once while it stays in the cache).
    pub(crate) async fn read(
        hf: &HfClient,
        detect: &DetectService,
        repo: &str,
        commit: Option<&str>,
    ) -> AppResult<Self> {
        let detail = hf.repo(repo, commit).await.map_err(crate::hub::hf_error)?;
        Ok(Self::of(detect, hf, detail).await)
    }

    /// `detail`'s files read through `detect`.
    pub(crate) async fn of(detect: &DetectService, hf: &HfClient, detail: RepoDetail) -> Self {
        let reading = detect.read_repo(hf, &detail).await;
        Self { detail, reading }
    }
}

/// A set-up planned for one file, and the generic entry it makes.
pub(crate) struct Planned {
    pub plan: SetupPlan,
    /// Its recipe, and one part per planned file, in the plan's order.
    pub entry: GenEntry,
    /// For each of `plan.parts`, its part's index in `entry.parts`: `None` for a part the
    /// main file carries and for one nothing fills.
    pub part_of: Vec<Option<usize>>,
    /// The repos of the Hub files it fetches whose listing says they are gated (a companion's
    /// repo is not, the table's files being open): Hugging Face serves them only with a token.
    pub gated: BTreeSet<String>,
}

/// The set-up of the file `source` names, with the person's `choice`: a searched file of
/// its repo at its commit ([`plan_file`]), or a file on this computer ([`plan_disk`]). A
/// catalog or installed model has its parts already: `studio_not_supported_yet`.
pub(crate) async fn plan_source(
    deps: &PlanDeps<'_>,
    source: &LocalSource,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    match source {
        LocalSource::Search { repo, commit, file } => {
            let hf = deps
                .hf
                .ok_or_else(|| AppError::internal("a search source without a Hub client"))?;
            let read = ReadRepo::read(hf, deps.detect, repo, Some(commit)).await?;
            plan_file(deps, &read, file, choice).await
        }
        LocalSource::Disk { path } => plan_disk(deps, path, choice).await,
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } => {
            Err(errors::not_supported_yet(errors::PLAN_FOR_A_FILE_ONLY))
        }
    }
}

/// The set-up of `file` of the repo `read`, with `choice`. The file must be one Studio
/// runs (labelled `Works`): a diffusers folder's UNet plans the folder; any other file not
/// listed is `not_found`, and one Studio does not run `studio_not_supported_yet`.
pub(crate) async fn plan_file(
    deps: &PlanDeps<'_>,
    read: &ReadRepo,
    file: &str,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let detail = &read.detail;
    let listed = detail
        .files
        .iter()
        .find(|f| f.path == file)
        .ok_or_else(|| AppError::not_found("model file"))?;
    let (version, recipe_id, part, folder) = match read.reading.labels.get(file) {
        Some(FileLabel::Works {
            version,
            recipe,
            part,
            folder,
            ..
        }) => (*version, recipe.clone(), *part, *folder),
        // A gated repo's file that could not be read says what Hugging Face asks for.
        Some(FileLabel::NotRead {
            reason: NotRead::NeedsToken,
        }) => return Err(crate::hub::hf_error(HfError::NeedsToken)),
        Some(FileLabel::NotRead {
            reason: NotRead::NeedsTerms,
        }) => return Err(crate::hub::hf_error(HfError::NeedsTerms)),
        _ => return Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE)),
    };
    let facts = read
        .reading
        .facts
        .get(file)
        .ok_or_else(|| AppError::internal("a file that works without its facts"))?;
    let folder = if folder {
        let found = candidates(&detail.files)
            .folder
            .filter(|f| f.unet.file.path == file)
            .ok_or_else(|| AppError::internal("a folder model without its folder"))?;
        Some(found)
    } else {
        None
    };
    let main = match &folder {
        Some(folder) => MainFile::HubFolder {
            repo: &detail.id,
            revision: &detail.commit,
            folder,
        },
        None => MainFile::Hub {
            repo: &detail.id,
            revision: &detail.commit,
            file: listed,
        },
    };
    let mut others = OtherRepos::default();
    let overrides = slot_choices(deps, Some(detail), &choice.overrides, &mut others).await?;
    let quant = match &folder {
        Some(folder) => Some(if folder.fp16 { "fp16" } else { "fp32" }.to_owned()),
        None => quant_label(file),
    };
    let repo_name = detail.id.rsplit('/').next().unwrap_or(&detail.id);
    let named = Named {
        id: format!("hf:{}:{}", detail.id, file),
        name: match quant {
            Some(quant) => format!("{repo_name} ({quant})"),
            None => repo_name.to_owned(),
        },
        publisher: detail.id.split('/').next().unwrap_or_default().to_owned(),
        gate: GenGate {
            mode: GateMode::from(detail.gated),
            collects: Vec::new(),
        },
    };
    plan_with(
        deps,
        PlanMain {
            main,
            facts,
            version,
            recipe_id: &recipe_id,
            part,
            listing: &detail.files,
            listing_facts: &read.reading.facts,
            detail: Some(detail),
        },
        named,
        choice,
        &overrides,
        &others,
    )
}

/// The set-up of the file at `path` on this computer (untrusted: [`checked_disk_file`]),
/// read from its header, with `choice`. A file Studio does not run is
/// `studio_not_supported_yet`.
pub(crate) async fn plan_disk(
    deps: &PlanDeps<'_>,
    path: &str,
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let file = read_disk_file(path).await?;
    let setup = disk_setup(deps, &file, choice).await?;
    let id = format!("local:{}", file.path.display());
    plan_disk_with(deps, &file, &setup, (&file.path, None, id), choice)
}

/// A model file on this computer the webview named, checked ([`checked_disk_file`]) and its
/// header read.
pub(crate) struct DiskFile {
    /// Its canonical path.
    pub path: PathBuf,
    pub size: u64,
    /// Its file name.
    pub name: String,
    pub facts: FileFacts,
}

impl DiskFile {
    /// Its label, by its header and its file name.
    pub(crate) fn label(&self) -> FileLabel {
        label_for(&self.facts, &self.name)
    }
}

/// The file at `path` on this computer (untrusted): checked ([`checked_disk_file`]: never
/// opened when it is a pickle file), then its header read off the async runtime. A header
/// that does not read is `studio_not_supported_yet`.
pub(crate) async fn read_disk_file(path: impl AsRef<Path>) -> AppResult<DiskFile> {
    let (path, size) = checked_disk_file(path)?;
    let read_from = path.clone();
    let facts = tokio::task::spawn_blocking(move || inspect_local(&read_from))
        .await
        .map_err(|e| AppError::internal(format!("reading a file's header: {e}")))?
        .map_err(|e| errors::not_supported_yet(format!("its header does not read: {e}")))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(DiskFile {
        path,
        size,
        name,
        facts,
    })
}

/// What planning a file on this computer takes besides where it is: the recipe its label
/// gives, and the person's choices for its slots, read once ([`slot_choices`]: a chosen file
/// of another repo asks for that repo's listing).
pub(crate) struct DiskSetup {
    version: SdVersion,
    recipe_id: String,
    part: Option<NoisePart>,
    overrides: Vec<(SlotId, SlotChoice)>,
    others: OtherRepos,
}

/// [`DiskSetup`] for `file` with `choice`. A file Studio does not run (its label is not
/// *Works in Studio*) is `studio_not_supported_yet`.
pub(crate) async fn disk_setup(
    deps: &PlanDeps<'_>,
    file: &DiskFile,
    choice: &SetupChoice,
) -> AppResult<DiskSetup> {
    let FileLabel::Works {
        version,
        recipe,
        part,
        ..
    } = file.label()
    else {
        return Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE));
    };
    let mut others = OtherRepos::default();
    let overrides = slot_choices(deps, None, &choice.overrides, &mut others).await?;
    Ok(DiskSetup {
        version,
        recipe_id: recipe,
        part,
        overrides,
        others,
    })
}

/// The set-up of `file` with `setup` and `choice`, its main file at `at`: `(path, its
/// SHA-256 when known, the entry's id)`. The path is the file's own, or a copy of it (an
/// imported file copied into the models folder, which has its name and header).
pub(crate) fn plan_disk_with(
    deps: &PlanDeps<'_>,
    file: &DiskFile,
    setup: &DiskSetup,
    at: (&Path, Option<&str>, String),
    choice: &SetupChoice,
) -> AppResult<Planned> {
    let (path, sha256, id) = at;
    let named = Named {
        id,
        name: file.name.clone(),
        publisher: String::new(),
        gate: GenGate {
            mode: GateMode::None,
            collects: Vec::new(),
        },
    };
    let no_facts = BTreeMap::new();
    plan_with(
        deps,
        PlanMain {
            main: MainFile::Local {
                path,
                size: file.size,
                sha256,
            },
            facts: &file.facts,
            version: setup.version,
            recipe_id: &setup.recipe_id,
            part: setup.part,
            listing: &[],
            listing_facts: &no_facts,
            detail: None,
        },
        named,
        choice,
        &setup.overrides,
        &setup.others,
    )
}

/// The main file of a plan and what was read about it.
struct PlanMain<'a> {
    main: MainFile<'a>,
    facts: &'a FileFacts,
    version: SdVersion,
    /// The recipe its label gives.
    recipe_id: &'a str,
    part: Option<NoisePart>,
    listing: &'a [RepoFile],
    listing_facts: &'a BTreeMap<String, FileFacts>,
    /// Its repo's listing; `None` for a file on this computer.
    detail: Option<&'a RepoDetail>,
}

/// How the entry a plan makes is known.
struct Named {
    id: String,
    name: String,
    publisher: String,
    gate: GenGate,
}

/// The listings of other repos a choice names, by repo and commit: `None` when the read
/// failed.
type OtherRepos = BTreeMap<(String, String), Option<RepoDetail>>;

/// Plans `main` with `choice`'s recipe (an in-scope recipe; untested when its version's
/// recipes do not include it) and `overrides`, and makes its entry.
fn plan_with(
    deps: &PlanDeps<'_>,
    m: PlanMain<'_>,
    named: Named,
    choice: &SetupChoice,
    overrides: &[(SlotId, SlotChoice)],
    others: &OtherRepos,
) -> AppResult<Planned> {
    let recipe_id = choice.recipe.as_deref().unwrap_or(m.recipe_id);
    let recipe = recipe(recipe_id)
        .filter(|r| out_of_scope(&r.id).is_none())
        .ok_or_else(errors::recipe_not_offered)?;
    let untested = !recipes_for(m.version).contains(&recipe_id);
    let installed = installed_files(deps.installed);
    let plan = plan_setup(&SetupInput {
        main: m.main,
        facts: m.facts,
        recipe,
        version: Some(m.version),
        part: m.part,
        listing: m.listing,
        listing_facts: m.listing_facts,
        installed: &installed,
        companions: &deps.catalog.companions,
        overrides,
        untested,
    });
    let mut licenses = BTreeMap::new();
    let mut parts = Vec::new();
    let mut part_of = Vec::with_capacity(plan.parts.len());
    let mut gated = BTreeSet::new();
    for planned in &plan.parts {
        let (Some(file), Some(role)) = (&planned.file, GenRole::from_slot(planned.slot)) else {
            part_of.push(None);
            continue;
        };
        if let PlannedFile::Hub { repo, revision, .. } = file {
            let listing = m
                .detail
                .filter(|d| d.id == *repo && d.commit == *revision)
                .or_else(|| others.get(&(repo.clone(), revision.clone()))?.as_ref());
            if listing.is_some_and(|d| d.gated != Gated::No) {
                gated.insert(repo.clone());
            }
        }
        let part = match file {
            // The main file from this computer is the person's own, wherever it lies (an
            // installed model's file included): it names no licence and is never deleted.
            PlannedFile::Local { path, size, sha256 } if planned.source == PartSource::Main => {
                local_part(role, path, *size, sha256.as_deref())
            }
            _ => gen_part(deps, m.detail, others, &mut licenses, role, file),
        };
        part_of.push(Some(parts.len()));
        parts.push(part);
    }
    let entry = generic_entry(GenericSpec {
        id: named.id,
        name: named.name,
        publisher: named.publisher,
        recipe,
        version: Some(m.version),
        parts,
        licenses,
        gate: named.gate,
        untested,
    });
    Ok(Planned {
        plan,
        entry,
        part_of,
        gated,
    })
}

/// The part a planned `file` makes for `role`, its licence added to `licenses`: a Hub file
/// under its companion licence or its own repo's ([`file_licence`]); a file an installed
/// model uses, as that model has it (so it is checked in place, not fetched); else a file
/// of this computer, the person's own, which names no licence.
fn gen_part(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    others: &OtherRepos,
    licenses: &mut BTreeMap<String, GenLicence>,
    role: GenRole,
    file: &PlannedFile,
) -> GenPart {
    match file {
        PlannedFile::Hub {
            repo,
            revision,
            path,
            size,
            sha256,
            licence,
            save_as,
        } => {
            let licence = file_licence(
                &deps.catalog.companions,
                main,
                others,
                repo,
                revision,
                licence,
            );
            GenPart {
                role,
                repo: repo.clone(),
                revision: revision.clone(),
                path: path.clone(),
                size: *size,
                sha256: sha256.clone(),
                license: add_licence(licenses, licence),
                local: None,
                folder: save_as.clone(),
            }
        }
        PlannedFile::Local { path, size, sha256 } => {
            let spelled = path.display().to_string();
            let installed = deps.installed.gen_models.iter().find_map(|row| {
                let part = row.parts.as_array()?.iter().find(|p| {
                    p.get("file")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|f| crate::model_options::same_file(f, &spelled))
                })?;
                Some((row, serde_json::from_value::<GenPart>(part.clone()).ok()?))
            });
            match installed {
                Some((row, mut part)) if part.local.is_none() => {
                    let licence = installed_licence(deps, main, row, &part);
                    part.role = role;
                    part.license = add_licence(licenses, licence);
                    part
                }
                _ => local_part(role, path, *size, sha256.as_deref()),
            }
        }
    }
}

/// The part a file of this computer makes, the person's own: no repository, revision or
/// licence, its file name as its path, used where it is (`local`).
fn local_part(role: GenRole, path: &Path, size: u64, sha256: Option<&str>) -> GenPart {
    GenPart {
        role,
        repo: String::new(),
        revision: String::new(),
        path: path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        size,
        sha256: sha256.unwrap_or_default().to_owned(),
        license: String::new(),
        local: Some(path.to_path_buf()),
        folder: None,
    }
}

/// The licence of a Hub file a plan fetches (ruling S4-licence-ref): a companion's, from the
/// table; else its own repo's label ([`repo_licence`]): the main repo's listing for a file
/// of the main repo, the chosen repo's listing at its commit for a file the person chose,
/// and "Check the licence" (red) when that listing could not be read. A file of another
/// repo never shows the main repo's licence.
fn file_licence(
    companions: &CompanionTable,
    main: Option<&RepoDetail>,
    others: &OtherRepos,
    repo: &str,
    revision: &str,
    licence: &LicenceRef,
) -> GenLicence {
    match licence {
        LicenceRef::Companion(id) => match companions.licence(id) {
            Some(l) => l.clone(),
            None => unread_licence(repo, revision),
        },
        LicenceRef::Repo => {
            let listing = main
                .filter(|d| d.id == repo && d.commit == revision)
                .or_else(|| {
                    others
                        .get(&(repo.to_owned(), revision.to_owned()))?
                        .as_ref()
                });
            match listing {
                Some(detail) => repo_licence(detail),
                None => unread_licence(repo, revision),
            }
        }
    }
}

/// The licence of a file an installed model uses: a companion's, by its SHA-256; else the
/// one that model records for it; else, for a file of the main repo, that repo's label;
/// else "Check the licence".
fn installed_licence(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    row: &store::GenModel,
    part: &GenPart,
) -> GenLicence {
    let companions = &deps.catalog.companions;
    if let Some(l) = companions
        .by_sha(&part.sha256)
        .and_then(|c| companions.licence(&c.license))
    {
        return l.clone();
    }
    if let Some(l) = entry_from_installed(row, deps.catalog)
        .ok()
        .and_then(|e| e.licenses.get(&part.license).cloned())
    {
        return l;
    }
    match main.filter(|d| d.id == part.repo && d.commit == part.revision) {
        Some(detail) => repo_licence(detail),
        None => unread_licence(&part.repo, &part.revision),
    }
}

/// Adds `licence` to `licenses` and gives the id its parts name: the id of the same licence
/// (by SHA-256) when it is there already; its own id when that is free; else its id with a
/// number, so two repos' different licences with one label never share an id.
fn add_licence(licenses: &mut BTreeMap<String, GenLicence>, mut licence: GenLicence) -> String {
    if let Some((id, _)) = licenses.iter().find(|(_, l)| l.sha256 == licence.sha256) {
        return id.clone();
    }
    let id = (1..)
        .map(|n| match n {
            1 => licence.id.clone(),
            n => format!("{}-{n}", licence.id),
        })
        .find(|id| !licenses.contains_key(id))
        .unwrap_or_default();
    licence.id.clone_from(&id);
    licenses.insert(id.clone(), licence);
    id
}

/// The files the installed picture and video models use that are on this computer with
/// their recorded size, each once: what a plan takes before fetching anything.
fn installed_files(installed: &InstalledFiles) -> Vec<InstalledFile> {
    let mut seen = std::collections::BTreeSet::new();
    let mut files = Vec::new();
    for row in &installed.gen_models {
        for part in row.parts.as_array().into_iter().flatten() {
            let (Some(file), Ok(part)) = (
                part.get("file").and_then(serde_json::Value::as_str),
                serde_json::from_value::<GenPart>(part.clone()),
            ) else {
                continue;
            };
            let path = PathBuf::from(file);
            let here = std::fs::metadata(&path).is_ok_and(|m| m.is_file() && m.len() == part.size);
            if here && !part.sha256.is_empty() && seen.insert(file.to_owned()) {
                files.push(InstalledFile {
                    slot: part.role.slot(),
                    path,
                    sha256: part.sha256,
                    size: part.size,
                });
            }
        }
    }
    files
}

/// What `overrides` give each slot: a Hub file (of the main repo `main`, else of its own
/// repo at its commit, whose listing is read once into `others`); a file an installed model
/// uses; a file on this computer ([`checked_disk_file`]); or none (`SlotChoice::Empty`, the
/// only choice the planner leaves out). A Hub file is always pinned by its SHA-256, never
/// handed over without one:
/// - its listing read, the file is the listing's: `not_found` when it is not listed,
///   `studio_no_checksum` when it has no published SHA-256;
/// - its listing not read, a companion row with that repo, revision and path pins it; with
///   none, the read's own error (a temporary failure is never taken for a choice).
async fn slot_choices(
    deps: &PlanDeps<'_>,
    main: Option<&RepoDetail>,
    overrides: &[SlotOverride],
    others: &mut OtherRepos,
) -> AppResult<Vec<(SlotId, SlotChoice)>> {
    let mut choices = Vec::with_capacity(overrides.len());
    // Why each listing in `others` that is `None` was not read.
    let mut failed: BTreeMap<(String, String), AppError> = BTreeMap::new();
    for o in overrides {
        let choice = match &o.file {
            SlotFile::Hub { repo, commit, path } => {
                let key = (repo.clone(), commit.clone());
                if main.is_none_or(|d| d.id != *repo || d.commit != *commit)
                    && !others.contains_key(&key)
                {
                    let read = match deps.hf {
                        Some(hf) => hf
                            .repo(repo, Some(commit))
                            .await
                            .map_err(crate::hub::hf_error),
                        None => Err(AppError::internal("a chosen Hub file without a Hub client")),
                    };
                    match read {
                        Ok(detail) => {
                            others.insert(key.clone(), Some(detail));
                        }
                        Err(e) => {
                            tracing::warn!(%repo, code = %e.code, "a chosen file's listing was not read");
                            failed.insert(key.clone(), e);
                            others.insert(key.clone(), None);
                        }
                    }
                }
                let listing = main
                    .filter(|d| d.id == *repo && d.commit == *commit)
                    .or_else(|| others.get(&key).and_then(Option::as_ref));
                let file = match listing {
                    Some(detail) => {
                        let file = detail
                            .files
                            .iter()
                            .find(|f| f.path == *path)
                            .ok_or_else(|| AppError::not_found("model file"))?;
                        if file.sha256.is_none() {
                            return Err(errors::no_published_checksum());
                        }
                        file.clone()
                    }
                    None => {
                        let pinned =
                            deps.catalog.companions.files.iter().find(|c| {
                                c.repo == *repo && c.revision == *commit && c.path == *path
                            });
                        match pinned {
                            Some(c) => RepoFile {
                                path: c.path.clone(),
                                size: c.size,
                                sha256: Some(c.sha256.clone()),
                            },
                            None => {
                                return Err(failed.get(&key).cloned().unwrap_or_else(|| {
                                    AppError::internal("a chosen file's listing was not read")
                                }));
                            }
                        }
                    }
                };
                SlotChoice::Hub {
                    repo: repo.clone(),
                    revision: commit.clone(),
                    file,
                }
            }
            SlotFile::Installed { gen_model_id, role } => {
                let part = deps
                    .installed
                    .gen_models
                    .iter()
                    .find(|row| row.id == *gen_model_id)
                    .and_then(|row| {
                        row.parts.as_array()?.iter().find_map(|p| {
                            let part: GenPart = serde_json::from_value(p.clone()).ok()?;
                            let file = p.get("file")?.as_str()?.to_owned();
                            (part.role == *role).then_some((part, file))
                        })
                    })
                    .ok_or_else(|| AppError::not_found("model file"))?;
                SlotChoice::Local {
                    path: PathBuf::from(part.1),
                    size: part.0.size,
                    sha256: Some(part.0.sha256).filter(|s| !s.is_empty()),
                }
            }
            SlotFile::Disk { path } => {
                let (path, size) = checked_disk_file(path)?;
                SlotChoice::Local {
                    path,
                    size,
                    sha256: None,
                }
            }
            SlotFile::None => SlotChoice::Empty,
        };
        choices.push((o.role.slot(), choice));
    }
    Ok(choices)
}

/// A file on this computer the webview names (untrusted): made canonical, a regular file,
/// of a format Studio reads; with its size. A pickle file is refused by its extension
/// ("Studio can't read .ckpt files"), never opened.
pub(crate) fn checked_disk_file(path: impl AsRef<Path>) -> AppResult<(PathBuf, u64)> {
    let real = dunce::canonicalize(path).map_err(|_| AppError::not_found("file"))?;
    let meta = std::fs::metadata(&real).map_err(|_| AppError::not_found("file"))?;
    if !meta.is_file() {
        return Err(AppError::not_found("file"));
    }
    let name = real
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if let Some(ext) = PICKLE_EXTENSIONS.iter().find(|ext| name.ends_with(*ext)) {
        return Err(errors::cannot_read_file(ext));
    }
    if !READ_EXTENSIONS.iter().any(|ext| name.ends_with(ext)) {
        return Err(errors::not_a_model_format());
    }
    Ok((real, meta.len()))
}

/// The entry an installed model is, without a request: its saved parts (the revisions and
/// SHA-256s it was downloaded at), with the rest from its catalog entry when the catalog still
/// lists it, else from its family record when its family is one of 0.1.x's, as a searched
/// file's entry is made (`gen_search::entry_for`), else from its recipe alone
/// ([`generic_entry`]). A searched file's licence is worked out again from its saved
/// repository, revision and licence id, so repairs show the same metadata; a model of its
/// recipe alone has each part's worked out so ([`rebuilt_licences`], ruling S7-licences). A
/// 0.1.x family this build has no record of, and a model with no recipe yet (*Set it up
/// yourself*), are `studio_not_supported_yet`.
pub fn entry_from_installed(model: &store::GenModel, catalog: &GenCatalog) -> AppResult<GenEntry> {
    let parts: Vec<GenPart> = serde_json::from_value(model.parts.clone())
        .map_err(|e| AppError::internal(format!("an installed model's parts: {e}")))?;
    let mut model_parts = parts
        .iter()
        .filter(|p| matches!(p.role, GenRole::Diffusion | GenRole::Model));
    let (Some(diffusion), None) = (model_parts.next(), model_parts.next()) else {
        return Err(AppError::internal(
            "an installed model without exactly one model part",
        ));
    };
    let listed = model
        .catalog_id
        .as_deref()
        .and_then(|id| catalog.entries.get(id));
    let family = serde_json::from_value::<GenFamily>(model.family.clone().into());
    let mut entry = match (listed, family) {
        (Some(entry), _) => entry.clone(),
        (None, Ok(family)) => {
            let record = catalog
                .families
                .get(&family)
                .ok_or_else(|| errors::not_supported_yet(errors::FAMILY_RECORD_MISSING))?;
            from_record(model, record, diffusion)
        }
        (None, Err(_)) => {
            let mut entry = from_recipe(model)?;
            entry.licenses = rebuilt_licences(catalog, &parts);
            entry
        }
    };
    entry.id.clone_from(&model.id);
    // A file from this computer is the person's own: it names no licence.
    if let Some(missing) = parts
        .iter()
        .filter(|p| p.local.is_none())
        .find(|p| !entry.licenses.contains_key(&p.license))
    {
        return Err(errors::not_supported_yet(format!(
            "a part's licence {} is not known",
            missing.license
        )));
    }
    entry.parts = parts;
    Ok(entry)
}

/// An installed model of no 0.1.x family: a generic entry of its recipe, its family column the
/// engine version its file was detected as. Untested when that is `unknown`, or a version
/// whose recipes do not include it (the person chose it). No recipe yet is
/// `studio_not_supported_yet`: such a model is listed with *Set it up yourself*.
fn from_recipe(model: &store::GenModel) -> AppResult<GenEntry> {
    let recipe_id = model
        .recipe_id
        .as_deref()
        .ok_or_else(|| errors::not_supported_yet(errors::NO_RECIPE_YET))?;
    let recipe = models::sd::recipes::recipe(recipe_id)
        .ok_or_else(|| errors::not_supported_yet(format!("no engine recipe {recipe_id}")))?;
    let version = SdVersion::from_word(&model.family);
    let untested = model.family == UNKNOWN_FAMILY
        || version.is_none_or(|v| !recipes_for(v).contains(&recipe_id));
    Ok(generic_entry(GenericSpec {
        id: model.id.clone(),
        name: model.name.clone(),
        publisher: String::new(),
        recipe,
        version,
        parts: Vec::new(),
        licenses: BTreeMap::new(),
        // Not saved: the stored Hugging Face token is used for every download anyway.
        gate: GenGate {
            mode: GateMode::None,
            collects: Vec::new(),
        },
        untested,
    }))
}

/// The licences of a model of its recipe alone, each part's (not a file from this computer,
/// which names none) worked out again from its saved repository, revision and licence id, as
/// its set-up plan gave it (`gen_part`, ruling S7-licences): a companion's, by the part's
/// SHA-256 and that id; a catalog model's part's (an installed model's file it reuses), the
/// same way; "Check the licence" for a repo whose listing was not read (`unknown`); else its
/// repository's label. An id a clash numbered (`other-2`, [`add_licence`]) reads as its label
/// when an earlier part has that label's id, as it did when the plan named them.
fn rebuilt_licences(catalog: &GenCatalog, parts: &[GenPart]) -> BTreeMap<String, GenLicence> {
    let mut licenses: BTreeMap<String, GenLicence> = BTreeMap::new();
    for part in parts.iter().filter(|p| p.local.is_none()) {
        if licenses.contains_key(&part.license) {
            continue;
        }
        let label = match part.license.rsplit_once('-') {
            Some((base, n))
                if n.parse::<u32>().is_ok_and(|n| n >= 2) && licenses.contains_key(base) =>
            {
                base
            }
            _ => part.license.as_str(),
        };
        let mut licence = catalog_licence(catalog, part, label).unwrap_or_else(|| {
            if label == UNREAD_LICENCE {
                unread_licence(&part.repo, &part.revision)
            } else {
                repo_licence(&RepoDetail {
                    quantized_from: None,
                    id: part.repo.clone(),
                    commit: part.revision.clone(),
                    gated: Gated::No,
                    license: Some(label.to_owned()),
                    files: Vec::new(),
                    gguf: None,
                    template_tools: false,
                    template_thinking: false,
                    info: HubInfo::default(),
                    not_for_all_audiences: false,
                })
            }
        });
        licence.id.clone_from(&part.license);
        licenses.insert(part.license.clone(), licence);
    }
    licenses
}

/// The id of "Check the licence" for a repo whose listing was not read
/// (`gen_search::unread_licence`).
const UNREAD_LICENCE: &str = "unknown";

/// The licence `label` of a part the catalog pins by its SHA-256: a companion row's, else a
/// recommended entry's or a family record's part's.
fn catalog_licence(catalog: &GenCatalog, part: &GenPart, label: &str) -> Option<GenLicence> {
    let same = |p: &GenPart| p.sha256.eq_ignore_ascii_case(&part.sha256) && p.license == label;
    let companions = &catalog.companions;
    companions
        .files
        .iter()
        .filter(|c| c.sha256.eq_ignore_ascii_case(&part.sha256) && c.license == label)
        .find_map(|c| companions.licence(&c.license))
        .cloned()
        .or_else(|| {
            catalog
                .entries
                .values()
                .find(|e| e.parts.iter().any(same))
                .and_then(|e| e.licenses.get(label))
                .cloned()
        })
        .or_else(|| {
            catalog
                .families
                .values()
                .find(|r| r.companions.iter().any(same))
                .and_then(|r| r.licenses.get(label))
                .cloned()
        })
}

/// The family column of a model whose file's version was not detected.
const UNKNOWN_FAMILY: &str = "unknown";

/// The family column an entry's installed row keeps: its 0.1.x family, else the engine
/// version its model is, else `unknown`.
pub(crate) fn family_word(entry: &GenEntry) -> String {
    match (entry.family, entry.version) {
        (Some(family), _) => word(&family),
        (None, Some(version)) => version.word().to_owned(),
        (None, None) => UNKNOWN_FAMILY.to_owned(),
    }
}

/// An installed searched model's entry from its family `record`, as `gen_search::entry_for`
/// made it: the record's figures and licences, plus the licence its diffusion file was
/// downloaded under. `searched_licence` reads only the repository, its commit and its label,
/// which the saved diffusion part holds, so the licence (and its SHA-256) is the same.
fn from_record(model: &store::GenModel, record: &FamilyRecord, diffusion: &GenPart) -> GenEntry {
    let saved = RepoDetail {
        quantized_from: None,
        id: diffusion.repo.clone(),
        commit: diffusion.revision.clone(),
        gated: Gated::No,
        license: Some(diffusion.license.clone()),
        files: Vec::new(),
        gguf: None,
        template_tools: false,
        template_thinking: false,
        info: HubInfo::default(),
        not_for_all_audiences: false,
    };
    let licence = searched_licence(&saved, record).licence;
    let mut licenses = record.licenses.clone();
    licenses.insert(licence.id.clone(), licence);
    GenEntry {
        id: model.id.clone(),
        name: model.name.clone(),
        summary: String::new(),
        publisher: record.publisher.clone(),
        kind: record.kind,
        family: Some(record.family),
        recipe: record.family.recipe_id().to_owned(),
        version: Some(record.family.version()),
        untested: false,
        inputs: record.inputs.clone(),
        input_counts: Vec::new(),
        parts: Vec::new(),
        licenses,
        // Not saved: the stored Hugging Face token is used for every download anyway.
        gate: GenGate {
            mode: GateMode::None,
            collects: Vec::new(),
        },
        defaults: record.defaults.clone(),
        limits: record.limits.clone(),
        fallback: record.fallback.clone(),
        memory: record.memory.clone(),
        speed: record.speed.clone(),
        quirks: record.quirks.clone(),
        min_build: record.min_build,
        runs_on_apple: record.runs_on_apple,
        tested: None,
    }
}

/// A value's serde word (`GenRole::Llm` → `llm`, `GenFamily::Flux2Klein4b` →
/// `flux2-klein-4b`).
pub(crate) fn word<T: Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(word)) => word,
        _ => String::new(),
    }
}

/// What a download group keeps about the model it downloads: its recipe, and each part at
/// its own revision, with the file on this computer it uses where it is (`present`, one per
/// part: [`DownloadPlan::present`]).
fn gen_group(
    entry: &GenEntry,
    catalog_id: Option<String>,
    source: store::GenSource,
    present: &[Option<PathBuf>],
) -> GenGroup {
    GenGroup {
        gen_model_id: entry.id.clone(),
        catalog_id,
        name: entry.name.clone(),
        kind: word(&entry.kind),
        family: family_word(entry),
        source: source.as_str().to_string(),
        parts: entry
            .parts
            .iter()
            .zip(present)
            .map(|(p, here)| GenGroupPart {
                role: word(&p.role),
                repo: p.repo.clone(),
                revision: p.revision.clone(),
                path: p.path.clone(),
                size: p.size,
                sha256: p.sha256.clone(),
                license: p.license.clone(),
                folder: p.folder.clone(),
                local: p.local.clone(),
                present: here.as_ref().map(|h| h.display().to_string()),
            })
            .collect(),
        recipe_id: Some(entry.recipe.clone()),
        untested: entry.untested,
    }
}

fn plan_weight_file(
    detail: &RepoDetail,
    commit: &str,
    path: &str,
    models_dir: &Path,
) -> AppResult<(FileSpec, PathBuf)> {
    if detail.commit != commit
        || commit.len() != 40
        || !commit.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(AppError::bad_request(
            "The repository revision does not match this download.",
        ));
    }
    let file = detail
        .files
        .iter()
        .find(|f| f.path == path && f.size > 0 && f.sha256.is_some())
        .ok_or_else(|| {
            AppError::bad_request("Choose a repository file with a published size and SHA-256.")
        })?;
    // A namespace outside the valid Hub owner names keeps raw files separate from installed models.
    let root = models_dir.join("@files").join(commit);
    let dest =
        models::download::destination(&root, &detail.id, path).map_err(crate::hub::hf_error)?;
    let spec = crate::hub::file_spec(&detail.id, commit, file);
    Ok((spec, dest))
}

/// Re-read the pinned listing and save the selected weights through the normal resumable queue.
pub async fn download_weight_file(
    app: &AppHandle,
    repo: &str,
    commit: &str,
    file: &str,
) -> AppResult<String> {
    let hf = crate::hub::hf_client(app).await?;
    let detail = hf
        .repo(repo, Some(commit))
        .await
        .map_err(crate::hub::hf_error)?;
    let models_dir = crate::hub::models_dir(app).await;
    let (spec, dest) = plan_weight_file(&detail, commit, file, &models_dir)?;
    let present = std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == spec.size);
    let remaining = if present {
        0
    } else {
        spec.size
            .saturating_sub(models::download::resumable_bytes(&dest, &spec))
    };
    if reverse::diskspace::free_bytes(&models_dir).is_some_and(|free| remaining > free) {
        return Err(errors::disk_too_small(remaining));
    }
    let meta = GroupMeta {
        repo: detail.id,
        commit: detail.commit,
        label: spec.path.clone(),
        file_only: true,
        model_files: Vec::new(),
        projector: None,
        catalog_id: None,
        parameters: None,
        hub: None,
        image_file_for: None,
        image_files: None,
        generator: None,
    };
    app.state::<Arc<DownloadManager>>()
        .inner()
        .clone()
        .start(app, meta, vec![(spec, dest)])
        .await
}

/// What starting a local download reads: what planning reads, the store (an installed
/// model's row), the models folder and its free space (when the system says), and what the
/// Studio engine needs.
pub(crate) struct DownloadDeps<'a> {
    pub plan: PlanDeps<'a>,
    pub store: &'a store::Store,
    pub models_dir: &'a Path,
    pub free_bytes: Option<u64>,
    pub engine: &'a EngineNeed,
}

/// A local download ready to start: its group's metadata, the files it fetches or checks, and
/// whether the Studio engine comes with it.
pub(crate) struct PreparedDownload {
    pub meta: GroupMeta,
    pub files: Vec<(FileSpec, PathBuf)>,
    pub needs_engine: bool,
}

/// The download `source` names, nothing started yet:
/// - a searched file: its repo's listing at its commit read again, its files through the
///   readings (`DetectService`, kept from opening it), and its set-up planned with `choice`
///   ([`plan_source`]). A set-up that misses a part is `studio_setup_incomplete`, and one that
///   fetches a gated repo's file with no Hugging Face token the Hub's own `hf_needs_token`.
///   Its group's parts are exactly the plan's: the Hub files it fetches, and those it takes
///   from this computer, `present`;
/// - a catalog entry or an installed model (*Download again*), as 0.1.2 did: every Hub part
///   checked, and fetched when it is missing or damaged. A `choice` other than none is for a
///   searched file only: `studio_not_supported_yet`;
/// - a file on this computer is added, never downloaded: `studio_not_supported_yet`
///   (`import::prepare_import` adds it, with this group machinery for its companions).
///
/// Then what it takes here ([`prepared_download`]).
pub(crate) async fn prepare_download(
    deps: &DownloadDeps<'_>,
    source: &LocalSource,
    choice: Option<&SetupChoice>,
) -> AppResult<PreparedDownload> {
    let choice = choice.filter(|c| **c != SetupChoice::default());
    let (entry, setup, catalog_id, origin) = match source {
        LocalSource::Disk { .. } => {
            return Err(errors::not_supported_yet(errors::A_DISK_FILE_IS_ADDED));
        }
        LocalSource::Search { .. } => {
            let planned = plan_source(
                &deps.plan,
                source,
                choice.unwrap_or(&SetupChoice::default()),
            )
            .await?;
            ready_to_fetch(&planned, deps.plan.hf)?;
            (
                planned.entry,
                Some(planned.plan),
                None,
                store::GenSource::Search,
            )
        }
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } => {
            if choice.is_some() {
                return Err(errors::not_supported_yet(errors::PLAN_FOR_A_FILE_ONLY));
            }
            let resolved = resolve_with(deps.store, &deps.plan, source).await?;
            (resolved.entry, None, resolved.catalog_id, resolved.source)
        }
    };
    prepared_download(deps, entry, setup.as_ref(), catalog_id, origin)
}

/// A planned set-up that can be fetched: one that misses a part is `studio_setup_incomplete`,
/// and one that fetches a gated repository's file with no Hugging Face token (`hf`) the Hub's
/// own `hf_needs_token`.
pub(crate) fn ready_to_fetch(planned: &Planned, hf: Option<&HfClient>) -> AppResult<()> {
    if !planned.plan.can_run() {
        let missing: Vec<GenRole> = planned
            .plan
            .missing()
            .into_iter()
            .filter_map(GenRole::from_slot)
            .collect();
        return Err(errors::setup_incomplete(&missing));
    }
    if !planned.gated.is_empty() && !hf.is_some_and(HfClient::has_token) {
        return Err(crate::hub::hf_error(HfError::NeedsToken));
    }
    Ok(())
}

/// The download group of `entry` (made by the set-up plan `setup`, if any; recorded as from
/// `origin`, with its catalog id): what it takes here ([`plan_download`]:
/// `studio_no_checksum`, `studio_disk_too_small`), each part's file, a part already on this
/// computer (`present`) with none, and the group's metadata, nothing started. A group whose
/// parts are all present has no files: its model is added at once
/// (`downloads::register_present_gen_model`).
pub(crate) fn prepared_download(
    deps: &DownloadDeps<'_>,
    entry: GenEntry,
    setup: Option<&SetupPlan>,
    catalog_id: Option<String>,
    origin: store::GenSource,
) -> AppResult<PreparedDownload> {
    // The files the other installed models use, which no part is renamed over.
    let used = deps.plan.installed.except(&entry.id);
    let plan = plan_download(
        entry,
        setup,
        deps.models_dir,
        deps.engine,
        deps.free_bytes,
        &used,
    )?;
    let diffusion = plan.entry.diffusion();
    let meta = GroupMeta {
        repo: diffusion.repo.clone(),
        commit: diffusion.revision.clone(),
        label: plan.entry.name.clone(),
        model_files: Vec::new(),
        projector: None,
        catalog_id: catalog_id.clone(),
        hub: None,
        parameters: None,
        image_file_for: None,
        image_files: None,
        file_only: false,
        generator: Some(gen_group(&plan.entry, catalog_id, origin, &plan.present)),
    };
    Ok(PreparedDownload {
        meta,
        files: plan.files,
        needs_engine: plan.needs_engine,
    })
}

/// Starts a new download, or checks and repairs the pinned files of an installed model
/// ([`prepare_download`], with the person's `choice` for a searched file), once the note
/// about local models was accepted; returns its download group.
pub async fn start_local_download(
    app: &AppHandle,
    source: LocalSource,
    choice: Option<SetupChoice>,
) -> AppResult<String> {
    if app
        .state::<Arc<StudioManager>>()
        .prefs()
        .local_notice_accepted_at
        .is_none()
    {
        return Err(errors::notice_needed());
    }
    let state = app.state::<AppState>();
    let catalog = current_gen_catalog(app).await;
    let hf = match source {
        LocalSource::Search { .. } => Some(crate::hub::hf_client(app).await?),
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } | LocalSource::Disk { .. } => {
            None
        }
    };
    let installed = InstalledFiles::read(&state.store).await?;
    let detect = app.state::<Arc<DetectService>>();
    let models_dir = crate::hub::models_dir(app).await;
    let engine = super::engine_cmd::engine_need(app);
    let deps = DownloadDeps {
        plan: PlanDeps {
            hf: hf.as_ref(),
            detect: &detect,
            catalog: &catalog,
            installed: &installed,
        },
        store: &state.store,
        models_dir: &models_dir,
        free_bytes: reverse::diskspace::free_bytes(&models_dir),
        engine: &engine,
    };
    let prepared = prepare_download(&deps, &source, choice.as_ref()).await?;
    let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
    let group = downloads.start(app, prepared.meta, prepared.files).await?;
    // Refused only while an engine process runs from the folder it would replace: the
    // model downloads, and the engine's own button offers it again.
    if prepared.needs_engine
        && let Err(e) = super::engine_cmd::start_engine_download(app).await
    {
        tracing::warn!(code = %e.code, "the Studio engine did not download with the model");
    }
    Ok(group)
}

/// Deletes the installed model `id`, and each of its part files that nothing else uses and no
/// download not finished yet writes, unless a run on this computer waits for it or uses it
/// ([`delete_unless_in_use`]).
pub async fn delete_local_model(app: &AppHandle, id: &str) -> AppResult<()> {
    let state = app.state::<AppState>();
    let studio = app.state::<Arc<StudioManager>>();
    let models_dir = crate::hub::models_dir(app).await;
    let writing = files_being_written(&state.store, None).await?;
    let busy = |p: &Path| writing.contains(&p.display().to_string());
    delete_unless_in_use(&studio, &state.store, &models_dir, id, &busy).await?;
    Ok(())
}

/// [`delete_files_of`], refused with `studio_model_in_use` while `studio`'s local lane holds a
/// run of model `id`, queued or running (deleting a file the Studio engine has open fails
/// half-way on Windows); a refusal deletes nothing.
pub(crate) async fn delete_unless_in_use(
    studio: &StudioManager,
    store: &store::Store,
    models_dir: &Path,
    id: &str,
    busy: &(dyn Fn(&Path) -> bool + Sync),
) -> AppResult<Vec<PathBuf>> {
    if studio.local_uses_model(id) {
        return Err(errors::model_in_use());
    }
    delete_files_of(store, models_dir, id, busy).await
}

/// Deletes the row `id`, then each part file no installed model (chat, picture or video) uses
/// any more and no unfinished download writes (`busy`), then empty folders up to `models_dir`;
/// returns the files removed. A file from this computer (a part with `local`) is the
/// person's own and is never deleted, wherever it lies, the models folder included (ruling
/// S7-local-delete); the one exception is the copy an import made under
/// `<models>/@imported/` (`import::IMPORTED_FOLDER`), which Studio made. A model not
/// installed is `not_found`.
pub(crate) async fn delete_files_of(
    store: &store::Store,
    models_dir: &Path,
    id: &str,
    busy: &(dyn Fn(&Path) -> bool + Sync),
) -> AppResult<Vec<PathBuf>> {
    let model = store
        .delete_gen_model(id)
        .await?
        .ok_or_else(|| AppError::not_found("model"))?;
    let imported = dunce::canonicalize(models_dir.join(super::import::IMPORTED_FOLDER)).ok();
    let studios_copy = |file: &str| {
        let real = dunce::canonicalize(file).ok();
        real.zip(imported.as_ref())
            .is_some_and(|(real, root)| crate::downloads::inside_folder(&real, root))
    };
    let files: Vec<PathBuf> = model
        .parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let file = p.get("file")?.as_str()?;
            let local = p.get("local").is_some_and(|l| !l.is_null());
            (!local || studios_copy(file)).then(|| PathBuf::from(file))
        })
        .collect();
    remove_unused_files(store, models_dir, &files, busy).await
}

/// Removes each of `files` that no installed model uses (a chat model, or a part of a picture
/// or video model, under any spelling of its path: `hub::used_elsewhere`), that no unfinished
/// download writes (`busy`) and that is inside `models_dir`, then the folders that leaves
/// empty, up to `models_dir` itself (never removed). A file already gone, or that cannot be
/// removed, is left out of what it returns: the files removed.
pub(crate) async fn remove_unused_files(
    store: &store::Store,
    models_dir: &Path,
    files: &[PathBuf],
    busy: &(dyn Fn(&Path) -> bool + Sync),
) -> AppResult<Vec<PathBuf>> {
    let chat = store.list_models().await?;
    let gen_files = store.gen_model_files().await?;
    let Ok(root) = dunce::canonicalize(models_dir) else {
        return Ok(Vec::new());
    };
    let mut removed = Vec::new();
    for file in files {
        if busy(file) || crate::hub::used_elsewhere(&chat, &gen_files, file) {
            continue;
        }
        let Ok(real) = dunce::canonicalize(file) else {
            continue;
        };
        if !real.is_file() || !crate::downloads::inside_folder(&real, &root) {
            continue;
        }
        match tokio::fs::remove_file(file).await {
            Ok(()) => {
                remove_empty_folders(&real, &root);
                removed.push(file.clone());
            }
            Err(e) => tracing::warn!("a model file stayed: {e}"),
        }
    }
    Ok(removed)
}

/// Removes each folder above `file` that is empty now, up to (not including) `root`.
fn remove_empty_folders(file: &Path, root: &Path) {
    let mut dir = file.parent();
    while let Some(folder) = dir {
        if !crate::downloads::inside_folder(folder, root) || std::fs::remove_dir(folder).is_err() {
            break;
        }
        dir = folder.parent();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use models::catalog::Catalog;
    use models::download::destination;
    use models::gen_catalog::{Badge, Commercial};
    use models::hf::{OFFICIAL_ENDPOINT, parse_repo_detail};
    use serde_json::{Value, json};

    use super::*;

    fn catalog() -> GenCatalog {
        gen_catalog(
            &Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        )
    }

    fn families() -> BTreeMap<GenFamily, FamilyRecord> {
        catalog().families
    }

    /// FLUX.2 klein 4B from the recommended list, its parts made a few kilobytes so that a
    /// test can write them.
    fn entry() -> GenEntry {
        let mut e = catalog().entries.remove("flux2-klein-4b-q4").unwrap();
        for (part, size) in e.parts.iter_mut().zip([4_000, 2_000, 300]) {
            part.size = size;
        }
        e
    }

    /// The bundled Apache-2.0 text's SHA-256, which the catalog pins.
    fn apache_sha() -> String {
        entry().licenses["apache-2.0"].sha256.clone()
    }

    const KOMMUNITY_SHA: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";

    /// [`entry`] with its diffusion model under a community licence (commercial use under a
    /// revenue limit), and its encoder and decoder under Apache-2.0.
    fn two_licences() -> GenEntry {
        let mut e = entry();
        e.parts[0].license = "kommunity".into();
        e.licenses.insert(
            "kommunity".into(),
            GenLicence {
                id: "kommunity".into(),
                name: "Example Community Licence".into(),
                url: "https://example.com/kommunity".into(),
                sha256: KOMMUNITY_SHA.into(),
                commercial_outputs: Commercial::UnderRevenue,
                revenue_limit_usd: Some(1_000_000),
                excluded_regions: Vec::new(),
                obligations: Vec::new(),
                plain_words: "Commercial use only under $1M revenue.".into(),
            },
        );
        e
    }

    /// A folder of its own for one test, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("local-models-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn write(file: &Path, len: usize) {
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, vec![0u8; len]).unwrap();
    }

    fn dest(models: &Path, part: &GenPart) -> PathBuf {
        destination(models, &part.repo, &part.path).unwrap()
    }

    /// A repository listing in crates/models/tests/fixtures/hf/.
    fn detail(name: &str) -> RepoDetail {
        let text = match name {
            "repo-unknown-diffusion.json" => {
                include_str!("../../../crates/models/tests/fixtures/hf/repo-unknown-diffusion.json")
            }
            "repo-z-image-turbo-gguf.json" => include_str!(
                "../../../crates/models/tests/fixtures/hf/repo-z-image-turbo-gguf.json"
            ),
            "repo-uncensored.json" => {
                include_str!("../../../crates/models/tests/fixtures/hf/repo-uncensored.json")
            }
            other => panic!("no listing fixture {other}"),
        };
        let v: Value = serde_json::from_str(text).unwrap();
        let endpoint = url::Url::parse(OFFICIAL_ENDPOINT).unwrap();
        parse_repo_detail(v["id"].as_str().unwrap(), &v, &endpoint).unwrap()
    }

    #[test]
    fn standalone_weights_are_bound_to_the_published_revision_and_path() {
        let mut listing = detail("repo-unknown-diffusion.json");
        let root = Path::new("models");
        let sha = listing.commit.clone();
        let path = "sample-diffusion-2b-Q4_0.gguf";
        let (spec, dest) = plan_weight_file(&listing, &sha, path, root).unwrap();
        assert_eq!(
            dest,
            root.join("@files").join(&sha).join(&listing.id).join(path)
        );
        assert_eq!(spec.sha256, Some("b".repeat(64)));
        for bad in ["README.md", "../hidden.gguf", "unlisted.gguf"] {
            assert!(plan_weight_file(&listing, &sha, bad, root).is_err());
        }
        assert!(plan_weight_file(&listing, &"c".repeat(40), path, root).is_err());
        let weight = listing.files.iter_mut().find(|f| f.path == path).unwrap();
        weight.sha256 = None;
        assert!(plan_weight_file(&listing, &sha, path, root).is_err());
    }

    /// The source names, as the webview sends them.
    #[test]
    fn a_source_reads_as_the_webview_writes_it() {
        let read = |v: Value| serde_json::from_value::<LocalSource>(v).unwrap();
        assert_eq!(
            read(json!({"catalog": {"entryId": "flux2-klein-4b-q4"}})),
            LocalSource::Catalog {
                entry_id: "flux2-klein-4b-q4".into()
            }
        );
        assert_eq!(
            read(json!({"search": {
                "repo": "leejet/Z-Image-Turbo-GGUF",
                "commit": "c".repeat(40),
                "file": "z.gguf"
            }})),
            LocalSource::Search {
                repo: "leejet/Z-Image-Turbo-GGUF".into(),
                commit: "c".repeat(40),
                file: "z.gguf".into()
            }
        );
        assert_eq!(
            read(json!({"installed": {"genModelId": "hf:someone/Z-Image-Turbo-GGUF:z.gguf"}})),
            LocalSource::Installed {
                gen_model_id: "hf:someone/Z-Image-Turbo-GGUF:z.gguf".into()
            }
        );
        assert_eq!(
            read(json!({"disk": {"path": "C:\\models\\my model.gguf"}})),
            LocalSource::Disk {
                path: "C:\\models\\my model.gguf".into()
            }
        );
    }

    /// A set-up choice reads as the webview writes it, a slot file of each kind field for
    /// field (`SlotFile`'s TypeScript type).
    #[test]
    fn a_setup_choice_reads_as_the_webview_writes_it() {
        let choice: SetupChoice = serde_json::from_value(json!({
            "recipe": "flux1-dev",
            "overrides": [
                {"role": "t5xxl", "file": {"hub": {"repo": "o/r", "commit": "c".repeat(40), "path": "t5.safetensors"}}},
                {"role": "vae", "file": {"installed": {"genModelId": "hf:o/r:f.gguf", "role": "vae"}}},
                {"role": "clip-l", "file": {"disk": {"path": "/m/clip.safetensors"}}},
                {"role": "llm-vision", "file": "none"},
            ],
        }))
        .unwrap();
        assert_eq!(choice.recipe.as_deref(), Some("flux1-dev"));
        assert_eq!(
            choice.overrides.iter().map(|o| &o.file).collect::<Vec<_>>(),
            [
                &SlotFile::Hub {
                    repo: "o/r".into(),
                    commit: "c".repeat(40),
                    path: "t5.safetensors".into()
                },
                &SlotFile::Installed {
                    gen_model_id: "hf:o/r:f.gguf".into(),
                    role: GenRole::Vae
                },
                &SlotFile::Disk {
                    path: "/m/clip.safetensors".into()
                },
                &SlotFile::None,
            ]
        );
        assert_eq!(choice.overrides[2].role, GenRole::ClipL);
        // Overrides may be left out.
        let bare: SetupChoice = serde_json::from_value(json!({"recipe": null})).unwrap();
        assert_eq!(bare, SetupChoice::default());
    }

    /// A part already in its place (FLUX.2 klein's text encoder, a common chat download) is
    /// not counted as something to fetch, but stays in the download: the download checks it
    /// against its SHA-256 instead of fetching it.
    #[test]
    fn a_part_already_on_disk_is_checked_not_fetched() {
        let dir = Scratch::new();
        let e = entry();
        let encoder = dest(&dir.0, &e.parts[1]);
        write(&encoder, 2_000);
        let plan = plan_download(e.clone(), None, &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert_eq!(plan.already_here, vec![encoder.clone()]);
        assert_eq!(plan.total_bytes, 6_300);
        assert_eq!(plan.to_fetch_bytes, 4_300);
        assert_eq!(plan.files.len(), 3);
        for ((spec, path), part) in plan.files.iter().zip(&e.parts) {
            assert_eq!(*path, dest(&dir.0, part));
            assert_eq!(
                spec,
                &FileSpec {
                    repo: part.repo.clone(),
                    commit: part.revision.clone(),
                    path: part.path.clone(),
                    size: part.size,
                    sha256: Some(part.sha256.clone()),
                }
            );
        }
        // A file of another size there is fetched.
        write(&encoder, 1_999);
        let plan = plan_download(e, None, &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert!(plan.already_here.is_empty());
        assert_eq!(plan.to_fetch_bytes, 6_300);
    }

    /// A set-up plan's files are its entry's parts, pair by pair: a file the plan takes from
    /// this computer is used where it is when it is the part's (its SHA-256 here); a plan of
    /// other roles, another file, or another count of files is an internal error, never a
    /// download of the wrong file.
    #[test]
    fn a_set_up_plan_that_is_not_its_entrys_is_refused() {
        let dir = Scratch::new();
        let e = entry();
        let hub = |p: &GenPart| PlannedFile::Hub {
            repo: p.repo.clone(),
            revision: p.revision.clone(),
            path: p.path.clone(),
            size: p.size,
            sha256: p.sha256.clone(),
            licence: LicenceRef::Repo,
            save_as: None,
        };
        let here = dir.0.join("elsewhere").join("encoder.gguf");
        let local = |sha256: &str| PlannedFile::Local {
            path: here.clone(),
            size: 2_000,
            sha256: Some(sha256.to_owned()),
        };
        let plan = |files: Vec<(GenRole, PlannedFile)>| SetupPlan {
            recipe_id: e.recipe.clone(),
            parts: files
                .into_iter()
                .map(|(role, file)| PlannedPart {
                    slot: role.slot(),
                    required: true,
                    source: PartSource::Repo,
                    file: Some(file),
                })
                .collect(),
            untested: false,
        };
        let [diffusion, encoder, vae] = [&e.parts[0], &e.parts[1], &e.parts[2]];
        let download = |setup: SetupPlan| {
            plan_download(
                e.clone(),
                Some(&setup),
                &dir.0,
                &EngineNeed::Found,
                None,
                &[],
            )
        };
        // The encoder another model has here, by its SHA-256: used where it is.
        let matching = plan(vec![
            (diffusion.role, hub(diffusion)),
            (encoder.role, local(&encoder.sha256.to_uppercase())),
            (vae.role, hub(vae)),
        ]);
        let ok = download(matching).unwrap();
        assert_eq!(ok.present, [None, Some(here.clone()), None]);
        assert_eq!(ok.files.len(), 2);
        let refused = [
            // Another file in the encoder's place.
            plan(vec![
                (diffusion.role, hub(diffusion)),
                (encoder.role, local(&"f".repeat(64))),
                (vae.role, hub(vae)),
            ]),
            // The right count, in another order of roles.
            plan(vec![
                (diffusion.role, hub(diffusion)),
                (vae.role, hub(vae)),
                (encoder.role, hub(encoder)),
            ]),
            // A file short.
            plan(vec![(diffusion.role, hub(diffusion)), (vae.role, hub(vae))]),
        ];
        for setup in refused {
            let e = download(setup).unwrap_err();
            assert_eq!(
                (e.code.as_str(), e.details.as_str()),
                ("internal", "a set-up plan that is not its entry's")
            );
        }
    }

    /// Finding local-shared-path-revision-overwrite: a part whose place holds a file an
    /// installed chat model uses under another SHA-256 (another revision of the same file)
    /// goes to a place of its own, `@<commit>`, never renamed over the chat model's file; the
    /// same file (the same SHA-256) is shared as before, and so is a file no model uses.
    #[test]
    fn a_part_never_replaces_a_chat_models_file_of_another_revision() {
        let dir = Scratch::new();
        let e = entry();
        let part = &e.parts[1];
        let shared = dest(&dir.0, part);
        write(&shared, 2_000);
        let chat = |sha256: &str| crate::hub::UsedFile {
            path: shared.display().to_string(),
            sha256: Some(sha256.to_owned()),
        };
        let place = |used: &[crate::hub::UsedFile]| {
            plan_download(e.clone(), None, &dir.0, &EngineNeed::Found, None, used)
                .unwrap()
                .files[1]
                .1
                .clone()
        };

        let other_revision = place(&[chat(&"f".repeat(64))]);
        assert_ne!(other_revision, shared);
        let (owner, name) = part.repo.split_once('/').unwrap();
        let tag: String = part.revision.chars().take(12).collect();
        let mut want = dir.0.join(owner).join(name).join(format!("@{tag}"));
        for segment in part.path.split('/') {
            want.push(segment);
        }
        assert_eq!(other_revision, want);
        // That place is fetched; the chat model's file is not counted as here.
        let plan = plan_download(
            e.clone(),
            None,
            &dir.0,
            &EngineNeed::Found,
            None,
            &[chat(&"f".repeat(64))],
        )
        .unwrap();
        assert!(plan.already_here.is_empty());
        // A chat model whose file records no SHA-256 cannot be told the same: not replaced.
        let unknown = crate::hub::UsedFile {
            path: shared.display().to_string(),
            sha256: None,
        };
        assert_eq!(place(&[unknown]), want);

        // The same file is shared, and a file no installed model uses is the part's place.
        assert_eq!(place(&[chat(&part.sha256.to_uppercase())]), shared);
        assert_eq!(place(&[]), shared);
        // Nothing there yet: the usual place.
        std::fs::remove_file(&shared).unwrap();
        assert_eq!(place(&[chat(&"f".repeat(64))]), shared);
    }

    #[test]
    fn the_download_plan_lists_each_parts_licence() {
        let dir = Scratch::new();
        let plan =
            plan_download(two_licences(), None, &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert_eq!(
            plan.licences
                .iter()
                .map(|l| (l.id.as_str(), l.sha256.clone()))
                .collect::<Vec<_>>(),
            [
                ("kommunity", KOMMUNITY_SHA.to_string()),
                ("apache-2.0", apache_sha())
            ],
            "each licence once, in the parts' order"
        );
    }

    /// A download that does not fit on the disk is refused before anything starts; one that
    /// leaves less than 10 GB free says so.
    #[test]
    fn a_disk_too_small_is_refused_before_anything_starts() {
        let dir = Scratch::new();
        let e =
            plan_download(entry(), None, &dir.0, &EngineNeed::Found, Some(1_000), &[]).unwrap_err();
        assert_eq!(e.code, "studio_disk_too_small");
        assert_eq!(
            e.next_step,
            "It needs 0.1 GB; free up space or choose another models folder."
        );
        let total = entry().total_bytes();
        assert!(
            plan_download(
                entry(),
                None,
                &dir.0,
                &EngineNeed::Found,
                Some(total + 5_000_000_000),
                &[]
            )
            .unwrap()
            .low_disk()
        );
        let roomy = plan_download(
            entry(),
            None,
            &dir.0,
            &EngineNeed::Found,
            Some(total + LOW_DISK_BYTES),
            &[],
        )
        .unwrap();
        assert!(!roomy.low_disk());
        assert_eq!(roomy.free_bytes, Some(total + LOW_DISK_BYTES));
        // The free space unknown: nothing is refused or said.
        assert!(
            !plan_download(entry(), None, &dir.0, &EngineNeed::Found, None, &[])
                .unwrap()
                .low_disk()
        );
        // Only what is fetched counts.
        write(&dest(&dir.0, &entry().parts[0]), 4_000);
        assert!(plan_download(entry(), None, &dir.0, &EngineNeed::Found, Some(2_300), &[]).is_ok());
    }

    /// The Studio engine's licence where it is a pack (Windows): `sd.lock.json`'s.
    fn engine_licence() -> GenLicence {
        let lock = runtime::sd::lock::embedded();
        let pack = lock
            .packs
            .iter()
            .find(|p| p.platform == "windows-x86_64")
            .unwrap();
        crate::studio::engine_cmd::engine_licence(pack)
    }

    /// Where the Studio engine is a pack (Windows), the first model's download brings it, and
    /// its licence is listed with the model's; where it is found, nothing comes; where it is
    /// neither found nor a pack, the plan promises no engine download.
    #[test]
    fn the_engine_comes_with_the_first_model_where_it_is_a_pack() {
        let dir = Scratch::new();
        let model_licences =
            plan_download(two_licences(), None, &dir.0, &EngineNeed::Found, None, &[])
                .unwrap()
                .licences;

        let engine = engine_licence();
        let plan = plan_download(
            two_licences(),
            None,
            &dir.0,
            &EngineNeed::Pack(engine.clone()),
            None,
            &[],
        )
        .unwrap();
        assert!(plan.needs_engine);
        let mut with_engine = model_licences.clone();
        with_engine.push(engine.clone());
        assert_eq!(plan.licences, with_engine, "the model's, then the engine's");
        let found =
            plan_download(two_licences(), None, &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert!(!found.needs_engine);
        assert!(!found.licences.contains(&engine));

        let missing = plan_download(
            two_licences(),
            None,
            &dir.0,
            &EngineNeed::Missing,
            None,
            &[],
        )
        .unwrap();
        assert!(!missing.needs_engine, "no engine download promised");
        assert_eq!(missing.licences, model_licences);
    }

    /// Ruling S4-licence-ref: a fetched file's licence is its companion row's, else its own
    /// repo's label: the main repo's listing for a file of the main repo, the chosen repo's
    /// listing at its commit for a file of another, and "Check the licence" when that listing
    /// could not be read. Never the main repo's for a file of another repo.
    #[test]
    fn a_files_licence_is_its_own_repos() {
        let table = catalog().companions;
        let labelled = |id: &str, commit: &str, label: &str| RepoDetail {
            id: id.into(),
            commit: commit.into(),
            license: Some(label.into()),
            ..detail("repo-z-image-turbo-gguf.json")
        };
        let main = labelled(
            "someone/sd15-merge",
            &"a".repeat(40),
            "creativeml-openrail-m",
        );
        let other = labelled("someone/vae", &"b".repeat(40), "mit");
        let mut others = OtherRepos::new();
        let of = |others: &OtherRepos, repo: &str, revision: &str, licence: &LicenceRef| {
            file_licence(&table, Some(&main), others, repo, revision, licence)
        };
        let from_main = of(&others, &main.id, &main.commit, &LicenceRef::Repo);
        assert_eq!(from_main.commercial_outputs, Commercial::WithUseRules);
        // Another repo whose listing could not be read: red, never the main repo's.
        let unread = of(&others, &other.id, &other.commit, &LicenceRef::Repo);
        assert_eq!(unread.badge(), Badge::Red);
        assert!(unread.url.contains("someone/vae"), "{}", unread.url);
        others.insert((other.id.clone(), other.commit.clone()), None);
        assert_eq!(
            of(&others, &other.id, &other.commit, &LicenceRef::Repo).badge(),
            Badge::Red
        );
        // Read at its commit: its own label.
        others.insert(
            (other.id.clone(), other.commit.clone()),
            Some(other.clone()),
        );
        let read = of(&others, &other.id, &other.commit, &LicenceRef::Repo);
        assert_eq!((read.id.as_str(), read.badge()), ("mit", Badge::Green));
        // The main repo at another commit is another listing.
        assert_eq!(
            of(&others, &main.id, &"c".repeat(40), &LicenceRef::Repo).badge(),
            Badge::Red
        );
        // A companion's licence comes from the table.
        let apache = of(
            &others,
            "comfyanonymous/flux_text_encoders",
            &"d".repeat(40),
            &LicenceRef::Companion("apache-2.0".into()),
        );
        assert_eq!(apache.sha256, apache_sha());
    }

    /// Two repos' different licences under one label keep their own ids; the same licence is
    /// listed once.
    #[test]
    fn a_licence_is_listed_once_and_ids_never_clash() {
        let mut licenses = BTreeMap::new();
        let mit = |sha: &str| GenLicence {
            id: "mit".into(),
            sha256: sha.into(),
            ..two_licences().licenses["apache-2.0"].clone()
        };
        assert_eq!(add_licence(&mut licenses, mit(&"1".repeat(64))), "mit");
        assert_eq!(add_licence(&mut licenses, mit(&"1".repeat(64))), "mit");
        assert_eq!(add_licence(&mut licenses, mit(&"2".repeat(64))), "mit-2");
        assert_eq!(licenses.len(), 2);
        assert_eq!(licenses["mit-2"].id, "mit-2");
    }

    /// An installed model whose parts are the files at `files`, each `(role, path)`.
    fn gen_model(id: &str, files: &[(&str, &Path)]) -> store::NewGenModel {
        store::NewGenModel {
            id: id.into(),
            catalog_id: None,
            name: format!("Model {id}"),
            kind: "image".into(),
            family: "z-image-turbo".into(),
            recipe_id: Some("z-image-turbo".into()),
            parts: Value::Array(
                files
                    .iter()
                    .map(|(role, file)| json!({"role": role, "file": file.display().to_string()}))
                    .collect(),
            ),
            size_bytes: 3,
            source: store::GenSource::Search,
        }
    }

    /// A file shared by two models goes only with the last of them, and a file a chat model
    /// uses stays; so do a file a download not finished yet writes and one outside the models
    /// folder. Emptied folders go, never the models folder.
    #[tokio::test]
    async fn deleting_a_model_keeps_a_shared_encoder() {
        let root = Scratch::new();
        let dir = root.0.join("models");
        let a = dir.join("acme").join("A-GGUF").join("a.gguf");
        let b = dir.join("acme").join("B-GGUF").join("b.gguf");
        let q = dir.join("unsloth").join("Qwen3-4B-GGUF").join("q.gguf");
        let c = dir.join("someone").join("C-GGUF").join("c.gguf");
        for f in [&a, &b, &q, &c] {
            write(f, 10);
        }
        let store = store::Store::open_in_memory().unwrap();
        store
            .insert_gen_model(gen_model(
                "a",
                &[("diffusion", &a), ("llm", &q), ("vae", &c)],
            ))
            .await
            .unwrap();
        store
            .insert_gen_model(gen_model("b", &[("diffusion", &b), ("llm", &q)]))
            .await
            .unwrap();
        store
            .upsert_model(store::ModelInput {
                path: c.display().to_string(),
                repo: Some("someone/C-GGUF".into()),
                file_name: "c.gguf".into(),
                size_bytes: 10,
                sha256: None,
                gguf_summary: json!({"architecture": "qwen3"}),
                source: store::ModelSource::Download,
            })
            .await
            .unwrap();

        let removed = delete_files_of(&store, &dir, "a", &|_| false)
            .await
            .unwrap();
        assert_eq!(removed, vec![a.clone()]);
        assert!(!a.exists() && q.exists() && c.exists() && b.exists());
        assert!(
            !dir.join("acme").join("A-GGUF").exists(),
            "its emptied folder"
        );
        assert!(store.get_gen_model("a").await.unwrap().is_none());

        let removed = delete_files_of(&store, &dir, "b", &|_| false)
            .await
            .unwrap();
        assert_eq!(removed, vec![b.clone(), q.clone()]);
        assert!(!dir.join("acme").exists() && !dir.join("unsloth").exists());
        assert!(c.exists(), "the chat model's file");
        assert!(dir.exists(), "never the models folder");
        assert_eq!(
            delete_files_of(&store, &dir, "b", &|_| false)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );

        // A file a download writes, and one outside the models folder, stay.
        let away = root.0.join("elsewhere").join("x.gguf");
        let d = dir.join("acme").join("D-GGUF").join("d.gguf");
        write(&away, 10);
        write(&d, 10);
        store
            .insert_gen_model(gen_model("d", &[("diffusion", &d), ("vae", &away)]))
            .await
            .unwrap();
        let busy = |p: &Path| p == d.as_path();
        assert!(
            delete_files_of(&store, &dir, "d", &busy)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(d.exists() && away.exists());
    }

    /// The saved parts of a model as `gen_model_from` records them from `entry`'s parts in
    /// `models`.
    fn installed_from(
        entry: &GenEntry,
        models: &Path,
        catalog_id: Option<&str>,
    ) -> store::GenModel {
        let parts = entry
            .parts
            .iter()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap();
                v["file"] = dest(models, p).display().to_string().into();
                v
            })
            .collect();
        store::GenModel {
            id: entry.id.clone(),
            catalog_id: catalog_id.map(str::to_string),
            name: entry.name.clone(),
            kind: "image".into(),
            family: "z-image-turbo".into(),
            recipe_id: Some("z-image-turbo".into()),
            parts: Value::Array(parts),
            size_bytes: entry.total_bytes(),
            source: if catalog_id.is_some() {
                store::GenSource::Catalog
            } else {
                store::GenSource::Search
            },
            added_at: 1,
        }
    }

    /// The entry 0.1.2 made for `file` of the searched repo `listed` of `record`'s family
    /// (its `gen_search::entry_for`): the diffusion part under the stricter of the family's
    /// licence and the repo's label, then the family's companions.
    fn searched_01x(
        catalog: &GenCatalog,
        listed: &RepoDetail,
        file: &RepoFile,
        record: &FamilyRecord,
    ) -> GenEntry {
        let licence = searched_licence(listed, record).licence;
        let diffusion = GenPart {
            role: GenRole::Diffusion,
            repo: listed.id.clone(),
            revision: listed.commit.clone(),
            path: file.path.clone(),
            size: file.size,
            sha256: file.sha256.clone().unwrap(),
            license: licence.id.clone(),
            local: None,
            folder: None,
        };
        let mut licenses = record.licenses.clone();
        licenses.insert(licence.id.clone(), licence);
        GenEntry {
            id: format!("hf:{}:{}", listed.id, file.path),
            name: format!("{} (Q4_K)", record.name),
            parts: std::iter::once(diffusion)
                .chain(record.companions.iter().cloned())
                .collect(),
            licenses,
            ..catalog.entries["z-image-turbo-q4"].clone()
        }
    }

    /// An installed model is rebuilt from its saved parts, at their saved revisions, and its
    /// family record, without a request; a searched file's licence is the one it was
    /// downloaded under, so *Download again* asks nothing that was accepted.
    #[test]
    fn an_installed_model_is_its_own_source() {
        let catalog = catalog();
        let models = Path::new("/m");
        let d = detail("repo-z-image-turbo-gguf.json");
        let record = &catalog.families[&GenFamily::ZImageTurbo];
        let file = d
            .files
            .iter()
            .find(|f| f.path == "z_image_turbo-Q4_K.gguf")
            .unwrap();
        let mut listed = d.clone();
        listed.id = "someone/Z-Image-Turbo-GGUF".into();
        listed.license = Some("cc-by-nc-4.0".into());
        let searched = searched_01x(&catalog, &listed, file, record);
        assert_eq!(
            searched.id,
            "hf:someone/Z-Image-Turbo-GGUF:z_image_turbo-Q4_K.gguf"
        );
        let row = installed_from(&searched, models, None);

        let e = entry_from_installed(&row, &catalog).unwrap();
        assert_eq!(e.id, row.id);
        assert_eq!(e.name, row.name);
        assert_eq!(
            e.parts, searched.parts,
            "the saved parts at their saved revisions"
        );
        assert_eq!(e.family, Some(GenFamily::ZImageTurbo));
        assert_eq!(e.defaults, record.defaults);
        assert_eq!(e.limits, record.limits);
        for part in &e.parts {
            assert_eq!(
                e.licenses[&part.license], searched.licenses[&part.license],
                "{}",
                part.license
            );
        }
        assert_eq!(e.effective_licence().badge(), Badge::Red);
        assert_eq!(
            plan_download(e.clone(), None, models, &EngineNeed::Found, None, &[])
                .unwrap()
                .licences,
            plan_download(
                searched.clone(),
                None,
                models,
                &EngineNeed::Found,
                None,
                &[]
            )
            .unwrap()
            .licences,
            "the same licences to show"
        );

        // A recommended model keeps its saved parts too, with the catalog's figures.
        let mut klein = catalog.entries["flux2-klein-4b-q4"].clone();
        klein.parts[0].revision = "0".repeat(40);
        let row = store::GenModel {
            family: "flux2-klein-4b".into(),
            recipe_id: Some("flux2-klein-4b".into()),
            ..installed_from(&klein, models, Some("flux2-klein-4b-q4"))
        };
        let e = entry_from_installed(&row, &catalog).unwrap();
        assert_eq!(e.parts, klein.parts);
        assert_eq!(e.name, catalog.entries["flux2-klein-4b-q4"].name);
        assert_eq!(e.speed, catalog.entries["flux2-klein-4b-q4"].speed);

        // A family this build has no record of cannot be rebuilt.
        let mut bare = catalog.clone();
        bare.families.clear();
        let row = installed_from(&searched, models, None);
        assert_eq!(
            entry_from_installed(&row, &bare).unwrap_err().code,
            "studio_not_supported_yet"
        );
    }

    /// An installed row with these saved parts (each `file` beside its fields).
    fn row_with(
        id: &str,
        family: &str,
        recipe_id: Option<&str>,
        catalog_id: Option<&str>,
        parts: Value,
    ) -> store::GenModel {
        store::GenModel {
            id: id.into(),
            catalog_id: catalog_id.map(str::to_owned),
            name: format!("Model {id}"),
            kind: "image".into(),
            family: family.into(),
            recipe_id: recipe_id.map(str::to_owned),
            parts,
            size_bytes: 1,
            source: store::GenSource::Search,
            added_at: 1,
        }
    }

    /// A 0.1.x search row: its 0.1.x family word, no catalog id and V4's recipe; its diffusion
    /// file from someone's repository, beside the family's companions.
    fn search_row(id: &str, family: &str, recipe_id: Option<&str>) -> store::GenModel {
        let (repo, path) = id
            .strip_prefix("hf:")
            .and_then(|rest| rest.split_once(':'))
            .unwrap();
        let record = &families()[&serde_json::from_value::<GenFamily>(json!(family)).unwrap()];
        let diffusion = json!({
            "role": "diffusion", "repo": repo, "revision": "c".repeat(40), "path": path,
            "file": format!("/models/{repo}/{path}"), "size": 4096, "sha256": "d".repeat(64),
            "license": record.upstream_license,
        });
        let companions = record.companions.iter().map(|p| {
            let mut v = serde_json::to_value(p).unwrap();
            v["file"] = format!("/models/{}/{}", p.repo, p.path).into();
            v
        });
        let parts = std::iter::once(diffusion).chain(companions).collect();
        row_with(id, family, recipe_id, None, Value::Array(parts))
    }

    #[test]
    fn an_01x_search_row_keeps_its_0_1_2_entry() {
        let row = search_row(
            "hf:o/FLUX.3-Image-Klein-GGUF:f3.gguf",
            "flux3-image-klein",
            Some("flux3-klein"),
        );
        let e = entry_from_installed(&row, &catalog()).unwrap();
        // The family record's defaults, as 0.1.2.
        assert_eq!(
            (e.family, e.untested, e.defaults.steps),
            (Some(GenFamily::Flux3ImageKlein), false, 16)
        );
        let record = &families()[&GenFamily::Flux3ImageKlein];
        assert_eq!(
            (e.recipe.as_str(), e.version, &e.limits, &e.fallback),
            (
                "flux3-klein",
                Some(SdVersion::Flux3),
                &record.limits,
                &record.fallback
            )
        );
        // A 0.1.x family's row keeps its record even with no recipe saved.
        let row = search_row(
            "hf:o/FLUX.3-Image-Klein-GGUF:f3.gguf",
            "flux3-image-klein",
            None,
        );
        assert_eq!(
            entry_from_installed(&row, &catalog()).unwrap().family,
            Some(GenFamily::Flux3ImageKlein)
        );
    }

    /// A model of no 0.1.x family is a generic entry of its recipe, its saved parts its own;
    /// it is untested when its version is not known or does not take that recipe. A file
    /// from this computer names no licence.
    #[test]
    fn a_model_of_no_01x_family_is_a_generic_entry_of_its_recipe() {
        let local = json!([{
            "role": "model", "repo": "", "revision": "", "path": "",
            "file": "/home/me/sdxl ü.safetensors", "local": "/home/me/sdxl ü.safetensors",
            "size": 6_938_078_334u64, "sha256": "ab".repeat(32), "license": "",
        }]);
        let row = |family: &str, recipe: Option<&str>| {
            row_with("local:1", family, recipe, None, local.clone())
        };
        let e = entry_from_installed(&row("sdxl", Some("sdxl-checkpoint")), &catalog()).unwrap();
        assert_eq!(
            (e.id.as_str(), e.name.as_str(), e.recipe.as_str()),
            ("local:1", "Model local:1", "sdxl-checkpoint")
        );
        assert_eq!(
            (e.family, e.version, e.untested),
            (None, Some(SdVersion::Sdxl), false)
        );
        assert_eq!((e.defaults.steps, e.defaults.cfg), (20, 5.0));
        assert_eq!(e.parts.len(), 1);
        assert_eq!(
            e.parts[0].local.as_deref(),
            Some(Path::new("/home/me/sdxl ü.safetensors"))
        );
        assert_eq!(family_word(&e), "sdxl");
        // SDXL Vega takes the SDXL recipe: still tested.
        let e = entry_from_installed(&row("sdxl-vega", Some("sdxl-checkpoint")), &catalog());
        assert!(!e.unwrap().untested);
        // Not known, or a recipe its version does not take: untested.
        for (family, recipe) in [
            ("unknown", "sdxl-checkpoint"),
            ("flux", "sdxl-checkpoint"),
            ("a-future-family", "sdxl-checkpoint"),
        ] {
            let e = entry_from_installed(&row(family, Some(recipe)), &catalog()).unwrap();
            assert!(e.untested, "{family}");
        }
        assert_eq!(
            family_word(
                &entry_from_installed(&row("unknown", Some("flux1-dev")), &catalog()).unwrap()
            ),
            "unknown"
        );
        // No recipe yet (Set it up yourself), or one the engine does not have: no entry.
        for recipe in [None, Some("no-such-recipe")] {
            assert_eq!(
                entry_from_installed(&row("sdxl", recipe), &catalog())
                    .unwrap_err()
                    .code,
                "studio_not_supported_yet",
                "{recipe:?}"
            );
        }
        // A Hub part's licence is worked out again from its repository's label (ruling
        // S7-licences), as its download recorded it.
        let hub = json!([{
            "role": "model", "repo": "o/r", "revision": "1".repeat(40), "path": "m.safetensors",
            "file": "/models/o/r/m.safetensors", "size": 1, "sha256": "ab".repeat(32),
            "license": "openrail",
        }]);
        let row = row_with(
            "hf:o/r:m.safetensors",
            "sdxl",
            Some("sdxl-checkpoint"),
            None,
            hub,
        );
        let e = entry_from_installed(&row, &catalog()).unwrap();
        let labelled = RepoDetail {
            license: Some("openrail".into()),
            ..unread_detail("o/r", &"1".repeat(40))
        };
        assert_eq!(e.licenses["openrail"], repo_licence(&labelled));
    }

    /// A listing of `repo` at `commit` with nothing in it and no label.
    fn unread_detail(repo: &str, commit: &str) -> RepoDetail {
        RepoDetail {
            quantized_from: None,
            id: repo.into(),
            commit: commit.into(),
            gated: Gated::No,
            license: None,
            files: Vec::new(),
            gguf: None,
            template_tools: false,
            template_thinking: false,
            info: HubInfo::default(),
            not_for_all_audiences: false,
        }
    }

    /// Ruling S7-licences: a generic model's licences are worked out again from each part's
    /// repository, revision and licence id, exactly as its set-up plan named them: a repo's
    /// label, two repos' different licences of one label (numbered), an unread repo, a
    /// companion's. A file from this computer names none.
    #[test]
    fn a_generic_models_licences_read_back_as_its_plan_named_them() {
        let catalog = catalog();
        let a = "1".repeat(40);
        let b = "2".repeat(40);
        let labelled = |repo: &str, commit: &str, label: &str| {
            repo_licence(&RepoDetail {
                license: Some(label.into()),
                ..unread_detail(repo, commit)
            })
        };
        let t5 = catalog
            .companions
            .files
            .iter()
            .find(|c| c.id == "t5xxl-fp8")
            .unwrap()
            .clone();
        let wanted = [
            labelled("o/main", &a, "other"),
            labelled("o/vae", &b, "other"),
            unread_licence("o/clip", &a),
            catalog.companions.licence(&t5.license).unwrap().clone(),
            labelled("o/llm", &b, "creativeml-openrail-m"),
        ];
        let mut licenses = BTreeMap::new();
        let ids: Vec<String> = wanted
            .iter()
            .map(|l| add_licence(&mut licenses, l.clone()))
            .collect();
        assert_eq!(
            ids,
            [
                "other",
                "other-2",
                "unknown",
                "apache-2.0",
                "creativeml-openrail-m"
            ]
        );
        let part = |role: &str, repo: &str, commit: &str, sha: &str, id: &str| {
            json!({"role": role, "repo": repo, "revision": commit, "path": "f.safetensors",
                   "file": format!("/models/{repo}/f.safetensors"), "size": 1,
                   "sha256": sha, "license": id})
        };
        let parts = json!([
            part("diffusion", "o/main", &a, &"a1".repeat(32), &ids[0]),
            part("vae", "o/vae", &b, &"a2".repeat(32), &ids[1]),
            part("clip-l", "o/clip", &a, &"a3".repeat(32), &ids[2]),
            part("t5xxl", &t5.repo, &t5.revision, &t5.sha256, &ids[3]),
            part("llm", "o/llm", &b, &"a4".repeat(32), &ids[4]),
            {"role": "llm-vision", "repo": "", "revision": "", "path": "", "size": 1,
             "sha256": "", "license": "", "local": "/me/v.gguf", "file": "/me/v.gguf"},
        ]);
        let row = row_with(
            "hf:o/main:f.safetensors",
            "flux",
            Some("flux1-dev"),
            None,
            parts,
        );
        let e = entry_from_installed(&row, &catalog).unwrap();
        assert_eq!(e.licenses, licenses);
    }

    /// A catalog row is its catalog entry, with the entry's recipe and version.
    #[test]
    fn a_catalog_row_is_its_catalog_entry() {
        let catalog = catalog();
        let klein = &catalog.entries["flux2-klein-4b-q4"];
        let parts = klein
            .parts
            .iter()
            .map(|p| {
                let mut v = serde_json::to_value(p).unwrap();
                v["file"] = format!("/models/{}", p.path).into();
                v
            })
            .collect();
        let row = row_with(
            "flux2-klein-4b-q4",
            "flux2-klein-4b",
            Some("flux2-klein-4b"),
            Some("flux2-klein-4b-q4"),
            Value::Array(parts),
        );
        let e = entry_from_installed(&row, &catalog).unwrap();
        assert_eq!((&e.recipe, e.version), (&klein.recipe, klein.version));
        assert_eq!(
            (e.defaults.steps, e.family),
            (4, Some(GenFamily::Flux2Klein4b))
        );
        assert_eq!(family_word(&e), "flux2-klein-4b");
    }

    /// What a download of an entry records: each part at its own revision, as the model's
    /// row and the group's rows later need.
    #[test]
    fn a_download_names_each_part_at_its_revision() {
        let e = entry();
        let group = gen_group(
            &e,
            Some("flux2-klein-4b-q4".into()),
            store::GenSource::Catalog,
            &vec![None; e.parts.len()],
        );
        assert_eq!(group.gen_model_id, "flux2-klein-4b-q4");
        assert_eq!(group.catalog_id.as_deref(), Some("flux2-klein-4b-q4"));
        assert_eq!(
            (
                group.kind.as_str(),
                group.family.as_str(),
                group.source.as_str()
            ),
            ("image", "flux2-klein-4b", "catalog")
        );
        assert_eq!(
            group
                .parts
                .iter()
                .map(|p| (p.role.as_str(), p.revision.as_str(), p.size))
                .collect::<Vec<_>>(),
            e.parts
                .iter()
                .zip(["diffusion", "llm", "vae"])
                .map(|(p, role)| (role, p.revision.as_str(), p.size))
                .collect::<Vec<_>>()
        );
    }

    // ---- one Download for a searched model and its set-up (Task S7) ------------------------

    use crate::downloads::{DownloadManager, finish_gen_group};
    use crate::dto::{SetupPlanDto, SlotOverride};
    use crate::studio::local_cmd::tests::{
        COMMIT, KONTEXT, RepoHub, TestRepo, bundled, flux_kontext_repo, gpu, hub, repo_deps,
        sd14_diffusers_repo, search_source, wan_a14b_repo,
    };
    use crate::studio::local_cmd::{LocalCmdDeps, setup_plan};
    use models::sd::testing::{file_of, fixture, safetensors};

    /// Get local models asking a Hub on this computer that serves `repos` (its client has no
    /// Hugging Face token: `HfClient::for_test`), with the bundled catalog and its companion
    /// table, a models folder of its own and a download manager. Nothing is fetched: a group is
    /// recorded as `DownloadManager::start` records it, and [`T::complete`] puts its files in
    /// place and finishes it as its task does.
    struct T {
        repos: Vec<TestRepo>,
        _hub: RepoHub,
        deps: LocalCmdDeps,
        downloads: DownloadManager,
        _scratch: crate::studio::test_support::Scratch,
    }

    async fn harness(repos: Vec<TestRepo>) -> T {
        let scratch = crate::studio::test_support::Scratch::new();
        let served = hub(repos.clone()).await;
        let deps = repo_deps(&served, gpu(24, 64), &scratch);
        T {
            repos,
            _hub: served,
            deps,
            downloads: DownloadManager::default(),
            _scratch: scratch,
        }
    }

    impl T {
        fn store(&self) -> &store::Store {
            &self.deps.store
        }

        fn models(&self) -> &Path {
            &self.deps.models_dir
        }

        /// Download on `source` with `choice`: what `start_local_download` does once the note
        /// was accepted, the group recorded but not run. Returns its id.
        async fn start(
            &self,
            source: LocalSource,
            choice: Option<SetupChoice>,
        ) -> AppResult<String> {
            let catalog = bundled();
            let installed = InstalledFiles::read(self.store()).await.unwrap();
            let deps = DownloadDeps {
                plan: PlanDeps {
                    hf: self.deps.hf.as_ref(),
                    detect: &self.deps.detect,
                    catalog: &catalog,
                    installed: &installed,
                },
                store: self.store(),
                models_dir: self.models(),
                free_bytes: self.deps.free_disk_bytes,
                engine: &self.deps.engine_need,
            };
            let prepared = prepare_download(&deps, &source, choice.as_ref()).await?;
            self.downloads
                .record_group(self.store(), &prepared.meta, &prepared.files)
                .await
        }

        /// The set-up plan Get local models shows for `source` with `choice`.
        async fn setup_plan(&mut self, source: LocalSource, choice: SetupChoice) -> SetupPlanDto {
            self.deps.installed_files = InstalledFiles::read(self.store()).await.unwrap();
            setup_plan(&self.deps, &source, &choice).await.unwrap()
        }

        async fn rows(&self, group: &str) -> Vec<store::Download> {
            self.store()
                .list_downloads(Some(group.to_owned()))
                .await
                .unwrap()
        }

        /// The repo paths of the files `group` fetches, in its order.
        async fn group_files(&self, group: &str) -> Vec<String> {
            self.rows(group).await.into_iter().map(|r| r.file).collect()
        }

        async fn group_bytes(&self, group: &str) -> u64 {
            self.rows(group).await.iter().map(|r| r.size_bytes).sum()
        }

        /// Every download row.
        async fn download_rows(&self) -> Vec<store::Download> {
            self.store().list_downloads(None).await.unwrap()
        }

        /// Every group with a row, each once.
        async fn groups(&self) -> BTreeSet<String> {
            self.download_rows()
                .await
                .into_iter()
                .map(|r| r.group_id)
                .collect()
        }

        /// Every file in the models folder.
        fn files_on_disk(&self) -> Vec<PathBuf> {
            fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
                for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        walk(&path, out);
                    } else {
                        out.push(path);
                    }
                }
            }
            let mut out = Vec::new();
            walk(self.models(), &mut out);
            out
        }

        /// Puts each file `group` fetches at its place (what its repo serves for it, else a
        /// small safetensors file), then finishes the group as its task does: each row
        /// checked, the model added (`downloads::register_gen_model`), every row done.
        async fn complete(&self, group: &str) {
            for row in self.rows(group).await {
                let bytes = self
                    .repos
                    .iter()
                    .find(|r| r.id() == row.repo)
                    .and_then(|r| r.served(&row.file))
                    .map_or_else(small_weights, <[u8]>::to_vec);
                let dest = Path::new(&row.dest_path);
                std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
                std::fs::write(dest, bytes).unwrap();
            }
            finish_gen_group(self.store(), self.models(), group)
                .await
                .unwrap();
        }

        /// Installs catalog entry `id` ([`install_catalog_entry`]).
        async fn install_catalog(&self, id: &str) {
            install_catalog_entry(self.store(), self.models(), id).await;
        }

        /// Delete on installed model `id`, as `delete_local_model` does.
        async fn delete(&self, id: &str) -> AppResult<Vec<PathBuf>> {
            let writing = files_being_written(self.store(), None).await?;
            let busy = |p: &Path| writing.contains(&p.display().to_string());
            delete_files_of(self.store(), self.models(), id, &busy).await
        }
    }

    /// Installs the bundled catalog's entry `id` in `models` as its download would: each part
    /// at its place, at its pinned size (sparse files), and its row in `store`.
    pub(crate) async fn install_catalog_entry(store: &store::Store, models: &Path, id: &str) {
        let entry = bundled().entries.remove(id).unwrap();
        let mut parts = Vec::new();
        for part in &entry.parts {
            let file = dest(models, part);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::File::create(&file)
                .unwrap()
                .set_len(part.size)
                .unwrap();
            let mut saved = serde_json::to_value(part).unwrap();
            saved["file"] = file.display().to_string().into();
            parts.push(saved);
        }
        store
            .insert_gen_model(store::NewGenModel {
                id: entry.id.clone(),
                catalog_id: Some(entry.id.clone()),
                name: entry.name.clone(),
                kind: word(&entry.kind),
                family: family_word(&entry),
                recipe_id: Some(entry.recipe.clone()),
                parts: Value::Array(parts),
                size_bytes: entry.total_bytes(),
                source: store::GenSource::Catalog,
            })
            .await
            .unwrap();
    }

    /// A small safetensors file, for a fetched file no check reads.
    pub(crate) fn small_weights() -> Vec<u8> {
        safetensors(&json!({"w": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]}}))
    }

    /// A Hunyuan Video 1.5 GGUF alone in its repo (its detection fixture's tensors as a
    /// GGUF): no text encoder or VAE beside it, and none in the companion table.
    fn bare_hunyuan_gguf_repo() -> TestRepo {
        let mut index = fixture("hunyuan-video-15");
        index.format = models::sd::index::FileFormat::Gguf;
        TestRepo::new("someone/HunyuanVideo-1.5-GGUF")
            .tagged("text-to-video")
            .serving("h-Q4_0.gguf", 5_000_000_000, file_of(&index))
    }

    /// FLUX.1 [dev], a full checkpoint with its own VAE file beside it.
    fn flux_dev_repo() -> TestRepo {
        TestRepo::new("black-forest-labs/FLUX.1-dev")
            .tagged("text-to-image")
            .file("flux1-dev.safetensors", 23_802_932_552.0, Some("flux1-dev"))
            .file("ae.safetensors", 335_304_388.0, None)
    }

    /// Chroma1-HD as a GGUF alone: its T5 and VAE come from elsewhere.
    fn chroma_repo() -> TestRepo {
        TestRepo::new("silveroxides/Chroma1-HD-GGUF")
            .tagged("text-to-image")
            .file("Chroma1-HD-Q4_0.gguf", 5_432_053_920.0, Some("chroma"))
    }

    fn kontext(file: &str) -> LocalSource {
        search_source(KONTEXT, file)
    }

    #[tokio::test]
    async fn a_kontext_file_downloads_its_listed_companions_and_registers_its_recipe() {
        let t = harness(vec![flux_kontext_repo(&["Q4_K_M"])]).await;
        let group = t
            .start(kontext("flux1-kontext-dev-Q4_K_M.gguf"), None)
            .await
            .unwrap();
        assert_eq!(
            t.group_files(&group).await,
            [
                "flux1-kontext-dev-Q4_K_M.gguf",
                "clip_l.safetensors",
                "t5xxl_fp8_e4m3fn.safetensors",
                "ae.safetensors"
            ]
        );
        t.complete(&group).await;
        let row = t
            .store()
            .get_gen_model("hf:QuantStack/FLUX.1-Kontext-dev-GGUF:flux1-kontext-dev-Q4_K_M.gguf")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (
                row.recipe_id.as_deref(),
                row.family.as_str(),
                row.kind.as_str()
            ),
            (Some("flux1-kontext"), "flux", "image-edit")
        );
        assert_eq!(row.source, store::GenSource::Search);
        // One part per planned slot, each with its role and its file where it landed.
        let parts = row.parts.as_array().unwrap();
        assert_eq!(
            parts
                .iter()
                .map(|p| p["role"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["diffusion", "clip-l", "t5xxl", "vae"]
        );
        for (part, fetched) in parts.iter().zip(t.rows(&group).await) {
            assert_eq!(part["file"], fetched.dest_path.as_str());
            assert!(part.get("present").is_none());
        }
        // It runs as a model of its recipe, every part's licence known (ruling S7-licences).
        let entry = entry_from_installed(&row, &bundled()).unwrap();
        assert_eq!(
            (entry.recipe.as_str(), entry.version, entry.untested),
            ("flux1-kontext", Some(SdVersion::Flux), false)
        );
        assert_eq!(
            entry.licenses[&entry.parts[1].license].name,
            "Apache License 2.0"
        );
        assert_eq!(
            entry.licenses[&entry.parts[0].license].name,
            "Unknown licence"
        );
    }

    #[tokio::test]
    async fn nothing_downloads_that_the_plan_does_not_list() {
        let mut t = harness(vec![flux_kontext_repo(&["Q4_K_M"])]).await;
        let plan = t
            .setup_plan(
                kontext("flux1-kontext-dev-Q4_K_M.gguf"),
                SetupChoice::default(),
            )
            .await;
        let group = t
            .start(kontext("flux1-kontext-dev-Q4_K_M.gguf"), None)
            .await
            .unwrap();
        assert_eq!(t.group_bytes(&group).await, plan.to_fetch_bytes);
        let listed: Vec<String> = plan
            .slots
            .iter()
            .filter_map(|s| Some(s.part.as_ref()?.path.clone()))
            .collect();
        assert_eq!(t.group_files(&group).await, listed);
    }

    #[tokio::test]
    async fn a_set_up_with_a_missing_required_file_is_refused() {
        let t = harness(vec![bare_hunyuan_gguf_repo()]).await;
        let e = t
            .start(
                search_source("someone/HunyuanVideo-1.5-GGUF", "h-Q4_0.gguf"),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_setup_incomplete");
        assert_eq!(
            (e.message.as_str(), e.next_step.as_str()),
            (
                "Choose a file for each part first.",
                "Open the model's files and pick the missing parts."
            )
        );
        assert!(e.details.contains("t5xxl"), "{}", e.details);
        assert!(t.groups().await.is_empty());
    }

    #[tokio::test]
    async fn a_gated_download_without_a_token_is_refused_before_anything_starts() {
        let t = harness(vec![flux_dev_repo().gated()]).await;
        let e = t
            .start(
                search_source("black-forest-labs/FLUX.1-dev", "flux1-dev.safetensors"),
                None,
            )
            .await
            .unwrap_err();
        // The existing code of `hub::hf_error(HfError::NeedsToken)`.
        assert_eq!(e.code, "hf_needs_token");
        assert!(t.groups().await.is_empty());
        assert!(t.download_rows().await.is_empty());
        assert!(t.files_on_disk().is_empty());
    }

    /// A file the person chose from a gated repo, without a token, is refused before anything
    /// starts too, though the main repo is open.
    #[tokio::test]
    async fn a_chosen_file_of_a_gated_repo_without_a_token_is_refused_before_anything_starts() {
        let other = "3".repeat(40);
        let t5 = TestRepo::new("someone/gated-t5")
            .at(&other)
            .file("t5xxl.safetensors", 4_893_934_904.0, None)
            .gated();
        let t = harness(vec![flux_kontext_repo(&["Q4_K_M"]), t5]).await;
        let choice = SetupChoice {
            recipe: None,
            overrides: vec![SlotOverride {
                role: GenRole::T5xxl,
                file: SlotFile::Hub {
                    repo: "someone/gated-t5".into(),
                    commit: other,
                    path: "t5xxl.safetensors".into(),
                },
            }],
        };
        let e = t
            .start(kontext("flux1-kontext-dev-Q4_K_M.gguf"), Some(choice))
            .await
            .unwrap_err();
        assert_eq!(e.code, "hf_needs_token");
        assert!(t.groups().await.is_empty() && t.files_on_disk().is_empty());
    }

    #[tokio::test]
    async fn a_wan_a14b_download_fetches_both_parts() {
        let t = harness(vec![wan_a14b_repo(&["Q4_K_M"])]).await;
        let g = t
            .start(
                search_source(
                    "QuantStack/Wan2.2-T2V-A14B-GGUF",
                    "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf",
                ),
                None,
            )
            .await
            .unwrap();
        let files = t.group_files(&g).await;
        assert!(files.contains(&"LowNoise/Wan2.2-T2V-A14B-LowNoise-Q4_K_M.gguf".to_string()));
        assert!(files.contains(&"HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf".to_string()));
        t.complete(&g).await;
        let row = t
            .store()
            .get_gen_model(
                "hf:QuantStack/Wan2.2-T2V-A14B-GGUF:HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.recipe_id.as_deref(), row.kind.as_str()),
            (Some("wan22-t2v"), "video")
        );
        assert_eq!(
            row.parts
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| matches!(
                    p["role"].as_str(),
                    Some("diffusion" | "high-noise-diffusion")
                ))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn a_diffusers_repo_downloads_its_folder_files_under_the_engines_names() {
        // Plain fp32 and fp16 files for the UNet and the text encoder, a plain VAE.
        let repo = sd14_diffusers_repo().without("vae/diffusion_pytorch_model.fp16.safetensors");
        let t = harness(vec![repo]).await;
        let g = t
            .start(
                search_source(
                    "CompVis/stable-diffusion-v1-4",
                    "unet/diffusion_pytorch_model.fp16.safetensors",
                ),
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            t.group_files(&g).await,
            [
                "unet/diffusion_pytorch_model.fp16.safetensors",
                "vae/diffusion_pytorch_model.safetensors",
                "text_encoder/model.fp16.safetensors"
            ]
        );
        t.complete(&g).await;
        let root = t.models().join("CompVis/stable-diffusion-v1-4/@fp16");
        for name in [
            "unet/diffusion_pytorch_model.safetensors",
            "vae/diffusion_pytorch_model.safetensors",
            "text_encoder/model.safetensors",
        ] {
            assert!(root.join(name).exists(), "{name}");
        }
        let row = t
            .store()
            .get_gen_model(
                "hf:CompVis/stable-diffusion-v1-4:unet/diffusion_pytorch_model.fp16.safetensors",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            (row.recipe_id.as_deref(), row.family.as_str()),
            (Some("sd1-checkpoint"), "sd1")
        );
        assert!(
            row.parts
                .as_array()
                .unwrap()
                .iter()
                .all(|p| p["folder"].is_string())
        );
        // Its OpenRAIL label reads back too.
        let entry = entry_from_installed(&row, &bundled()).unwrap();
        assert_eq!(
            word(&entry.licenses[&entry.parts[0].license].commercial_outputs),
            "withUseRules"
        );
    }

    #[tokio::test]
    async fn deleting_one_model_keeps_a_companion_a_generic_model_reuses() {
        let t = harness(vec![chroma_repo()]).await;
        // Owns <models>/Comfy-Org/Z-Image-Turbo/split_files/vae/ae.safetensors.
        t.install_catalog("z-image-turbo-q4").await;
        let g = t
            .start(
                search_source("silveroxides/Chroma1-HD-GGUF", "Chroma1-HD-Q4_0.gguf"),
                None,
            )
            .await
            .unwrap();
        // Reused by its SHA-256: present, not fetched.
        assert!(
            !t.group_files(&g)
                .await
                .iter()
                .any(|f| f.ends_with("ae.safetensors"))
        );
        t.complete(&g).await;
        let ae = t
            .models()
            .join("Comfy-Org/Z-Image-Turbo/split_files/vae/ae.safetensors");
        let chroma = "hf:silveroxides/Chroma1-HD-GGUF:Chroma1-HD-Q4_0.gguf";
        let row = t.store().get_gen_model(chroma).await.unwrap().unwrap();
        let vae = row
            .parts
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["role"] == "vae")
            .unwrap()
            .clone();
        assert_eq!(vae["file"], ae.display().to_string().as_str());
        t.delete("z-image-turbo-q4").await.unwrap();
        assert!(ae.exists());
        t.delete(chroma).await.unwrap();
        assert!(!ae.exists());
    }

    /// A file a download not finished yet will use where it is stays while it runs, though the
    /// model it came with is deleted meanwhile.
    #[tokio::test]
    async fn a_file_a_running_download_will_use_is_kept() {
        let t = harness(vec![chroma_repo()]).await;
        t.install_catalog("z-image-turbo-q4").await;
        let g = t
            .start(
                search_source("silveroxides/Chroma1-HD-GGUF", "Chroma1-HD-Q4_0.gguf"),
                None,
            )
            .await
            .unwrap();
        let ae = t
            .models()
            .join("Comfy-Org/Z-Image-Turbo/split_files/vae/ae.safetensors");
        t.delete("z-image-turbo-q4").await.unwrap();
        assert!(ae.exists());
        t.complete(&g).await;
        assert!(
            t.store()
                .get_gen_model("hf:silveroxides/Chroma1-HD-GGUF:Chroma1-HD-Q4_0.gguf")
                .await
                .unwrap()
                .is_some()
        );
    }

    /// Ruling S4-licence-ref: a file chosen from another repo downloads from that repo, and the
    /// model records that repo's licence (read at its commit), never the main repo's.
    #[tokio::test]
    async fn a_chosen_file_of_another_repo_downloads_under_that_repos_licence() {
        let other = "1".repeat(40);
        let vae = TestRepo::new("someone/my-flux-vae")
            .label("mit")
            .at(&other)
            .file("my-vae.safetensors", 335_304_388.0, None);
        let t = harness(vec![
            flux_kontext_repo(&["Q4_K_M"]).label("apache-2.0"),
            vae,
        ])
        .await;
        let choice = SetupChoice {
            recipe: None,
            overrides: vec![SlotOverride {
                role: GenRole::Vae,
                file: SlotFile::Hub {
                    repo: "someone/my-flux-vae".into(),
                    commit: other.clone(),
                    path: "my-vae.safetensors".into(),
                },
            }],
        };
        let g = t
            .start(kontext("flux1-kontext-dev-Q4_K_M.gguf"), Some(choice))
            .await
            .unwrap();
        let rows = t.rows(&g).await;
        let fetched = rows
            .iter()
            .find(|r| r.file == "my-vae.safetensors")
            .unwrap();
        assert_eq!(fetched.repo, "someone/my-flux-vae");
        t.complete(&g).await;
        let row = t
            .store()
            .get_gen_model("hf:QuantStack/FLUX.1-Kontext-dev-GGUF:flux1-kontext-dev-Q4_K_M.gguf")
            .await
            .unwrap()
            .unwrap();
        let entry = entry_from_installed(&row, &bundled()).unwrap();
        let vae = entry.parts.iter().find(|p| p.role == GenRole::Vae).unwrap();
        assert_eq!(
            (vae.repo.as_str(), vae.revision.as_str()),
            ("someone/my-flux-vae", other.as_str())
        );
        let theirs = &entry.licenses[&vae.license];
        assert_eq!(theirs.name, "MIT License");
        let mit = repo_licence(&RepoDetail {
            license: Some("mit".into()),
            ..unread_detail("someone/my-flux-vae", &other)
        });
        assert_eq!(theirs.sha256, mit.sha256);
        let main = &entry.licenses[&entry.diffusion().license];
        assert_eq!(main.name, "Apache License 2.0");
    }

    /// Global Constraint: a Hub file with no published SHA-256 is never fetched: the download
    /// is refused before any group or row exists, whatever the source. A searched file with
    /// none is not a choice at all (its label says so); an installed model's part with none
    /// (a row this build did not make) is refused at *Download again*.
    #[tokio::test]
    async fn a_file_with_no_published_checksum_is_never_downloaded() {
        let repo = TestRepo::new("someone/FLUX.1-Kontext-unpinned")
            .tagged("image-to-image")
            .unpinned("kontext-Q4_K_M.gguf", 6_931_817_760, "flux1-kontext");
        let t = harness(vec![repo]).await;
        let e = t
            .start(
                search_source("someone/FLUX.1-Kontext-unpinned", "kontext-Q4_K_M.gguf"),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_not_supported_yet");
        assert!(t.groups().await.is_empty() && t.files_on_disk().is_empty());

        let ae = bundled()
            .companions
            .files
            .into_iter()
            .find(|c| c.id == "ae-flux1")
            .unwrap();
        let part = |role: &str, repo: &str, path: &str, sha256: &str, license: &str| {
            json!({"role": role, "repo": repo, "revision": COMMIT, "path": path, "size": 10,
                   "sha256": sha256, "license": license,
                   "file": t.models().join(repo).join(path).display().to_string()})
        };
        t.store()
            .insert_gen_model(store::NewGenModel {
                id: "hf:o/r:m.gguf".into(),
                catalog_id: None,
                name: "m".into(),
                kind: "image".into(),
                family: "chroma".into(),
                recipe_id: Some("chroma".into()),
                parts: json!([
                    part("diffusion", "o/r", "m.gguf", &"ab".repeat(32), "unknown"),
                    part("vae", &ae.repo, &ae.path, "", &ae.license),
                ]),
                size_bytes: 20,
                source: store::GenSource::Search,
            })
            .await
            .unwrap();
        let again = LocalSource::Installed {
            gen_model_id: "hf:o/r:m.gguf".into(),
        };
        assert_eq!(
            t.start(again, None).await.unwrap_err().code,
            "studio_no_checksum"
        );
        assert!(t.groups().await.is_empty() && t.files_on_disk().is_empty());
    }

    /// A catalog entry and an installed model download as in 0.1.2, every Hub part checked,
    /// with the recipe their family runs; a set-up choice is for a searched file only.
    #[tokio::test]
    async fn a_catalog_model_downloads_every_part_with_its_familys_recipe() {
        let t = harness(Vec::new()).await;
        let source = LocalSource::Catalog {
            entry_id: "z-image-turbo-q4".into(),
        };
        let chosen = SetupChoice {
            recipe: Some("z-image".into()),
            overrides: Vec::new(),
        };
        assert_eq!(
            t.start(source.clone(), Some(chosen))
                .await
                .unwrap_err()
                .code,
            "studio_not_supported_yet"
        );
        let g = t.start(source, Some(SetupChoice::default())).await.unwrap();
        assert_eq!(t.rows(&g).await.len(), 3);
        let meta: GroupMeta = t
            .store()
            .get_setting(&format!("downloadGroup:{g}"))
            .await
            .unwrap()
            .unwrap();
        let generator = meta.generator.unwrap();
        assert_eq!(
            (generator.recipe_id.as_deref(), generator.untested),
            (Some("z-image-turbo"), false)
        );
        assert!(generator.parts.iter().all(|p| p.present.is_none()));
    }

    /// A diffusers folder's files land together under the engine's names: a plain folder at its
    /// repo's place, an fp16 one under `@fp16`, and the whole folder under `@<commit>` when
    /// another installed model uses another file at any one of its places.
    #[test]
    fn a_folder_lands_in_one_place_under_the_engines_names() {
        let dir = Scratch::new();
        let repo = "CompVis/stable-diffusion-v1-4";
        let folder_part = |role: GenRole, path: &str, name: &str| GenPart {
            role,
            repo: repo.into(),
            revision: COMMIT.into(),
            path: path.into(),
            size: 10,
            sha256: format!("{:0>64}", path.len()),
            license: "apache-2.0".into(),
            local: None,
            folder: Some(name.into()),
        };
        let unet = "unet/diffusion_pytorch_model.safetensors";
        let vae = "vae/diffusion_pytorch_model.safetensors";
        let folder = |unet_path: &str| {
            let mut e = entry();
            e.parts = vec![
                folder_part(GenRole::Model, unet_path, unet),
                folder_part(GenRole::Vae, vae, vae),
            ];
            e
        };
        let places = |e: GenEntry, used: &[UsedFile]| -> Vec<PathBuf> {
            plan_download(e, None, &dir.0, &EngineNeed::Found, None, used)
                .unwrap()
                .files
                .into_iter()
                .map(|(_, dest)| dest)
                .collect()
        };
        let base = dir.0.join("CompVis").join("stable-diffusion-v1-4");
        let at =
            |root: &Path, name: &str| name.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
        assert_eq!(places(folder(unet), &[]), [at(&base, unet), at(&base, vae)]);
        let fp16 = base.join("@fp16");
        assert_eq!(
            places(folder("unet/diffusion_pytorch_model.fp16.safetensors"), &[]),
            [at(&fp16, unet), at(&fp16, vae)]
        );
        // Another model uses another file at the VAE's place: the UNet moves with it.
        write(&at(&fp16, vae), 10);
        let other = UsedFile {
            path: at(&fp16, vae).display().to_string(),
            sha256: Some("f".repeat(64)),
        };
        let own = base.join(format!("@{}", &COMMIT[..12])).join("@fp16");
        assert_eq!(
            places(
                folder("unet/diffusion_pytorch_model.fp16.safetensors"),
                &[other]
            ),
            [at(&own, unet), at(&own, vae)]
        );
    }

    /// The webview's word for a download's choice: none, or a set-up choice.
    #[test]
    fn a_download_choice_reads_as_the_webview_sends_it() {
        let none: Option<SetupChoice> = serde_json::from_value(Value::Null).unwrap();
        assert_eq!(none, None);
        let some: Option<SetupChoice> =
            serde_json::from_value(json!({"recipe": "flux1-dev", "overrides": []})).unwrap();
        assert_eq!(some.unwrap().recipe.as_deref(), Some("flux1-dev"));
    }
}
