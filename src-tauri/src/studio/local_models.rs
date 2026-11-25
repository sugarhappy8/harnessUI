//! Downloading a picture or video model to this computer (studio-local-design.md, "Get local
//! models"; 0.1.0 Task L11).
//!
//! - **Which model.** A source ([`LocalSource`]) names an entry of the signed catalog's
//!   recommended list, a file of a Hugging Face repository at a commit (recognised again here
//!   from the repository's listing: the webview's word is never taken for it), or an installed
//!   model (its saved parts, with no request).
//! - **What it takes** ([`plan_download`]): every part, pinned by repository, revision, size
//!   and SHA-256, lands at `<models>/<owner>/<repo>/<path>`, so a part already there (a shared
//!   text encoder) is checked instead of fetched; where another installed model uses another
//!   revision of the file there, the part lands at `<models>/<owner>/<repo>/@<commit>/<path>`
//!   instead, never renamed over it. A download that does not fit on the disk is refused
//!   before anything starts.
//! - **Before downloading**, Studio shows the model licences and asks first-time users to read the local model notice.
//! - **The download** ([`start_local_download`]): one group of the download manager, each part
//!   at its own revision; once every part is checked, and the model file's own header is a
//!   model its recipe runs, it joins Studio's installed models (`downloads::register_gen_model`). The
//!   Windows engine pack comes with the first model; where no engine is found and none can
//!   come, the model still downloads, and Get local models tells it from the engine's state.
//! - **Deleting** ([`delete_local_model`]): refused while a run on this computer waits for the
//!   model or uses it; else the row goes, then each part file nothing else uses (a chat model,
//!   another picture or video model, a download not finished yet), then the folders that
//!   leaves empty.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use models::download::FileSpec;

use crate::hub::{UsedFile, chat_files_used, download_destination, gen_files_used};
use models::gen_catalog::{
    FamilyRecord, GateMode, GenCatalog, GenEntry, GenFamily, GenGate, GenLicence, GenPart, GenRole,
    GenericSpec, gen_catalog, generic_entry,
};
use models::gen_search::{Recognition, entry_for, recognise, searched_licence};
use models::hf::{Gated, HfClient, HubInfo, RepoDetail};
use models::sd::detect::SdVersion;
use models::sd::matching::recipes_for;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::engine_cmd::EngineNeed;
use super::errors;
use super::manager::StudioManager;
use crate::downloads::{DownloadManager, GenGroup, GenGroupPart, GroupMeta, files_being_written};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The model a local download or fit is for, as the webview names it:
/// `{"catalog":{"entryId":…}}`, `{"search":{"repo":…,"commit":…,"file":…}}` or
/// `{"installed":{"genModelId":…}}`.
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
}

/// What downloading an entry takes on this computer.
#[derive(Debug, Clone)]
pub struct DownloadPlan {
    pub entry: GenEntry,
    /// Every part's pinned file and where it goes, a part already here included: the download
    /// checks that one against its SHA-256 instead of fetching it.
    pub files: Vec<(FileSpec, PathBuf)>,
    /// The parts whose file is already in place with its pinned size.
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
/// part's revision), each at `<models>/<owner>/<repo>/<path>` unless a file there that
/// another installed model uses (`used`: chat models, and picture and video models other than
/// this one) is another revision's (`hub::download_destination`: the part then has a place
/// of its own, never renamed over that file), a part whose destination already has its
/// pinned size counted as here,
/// every licence to show, and what the Studio engine needs (`engine`,
/// `engine_cmd::engine_need`): its pack comes too, with its licence; an engine that
/// is neither here nor a pack changes nothing here (Get local models tells it from the
/// engine's state). Refused with `studio_disk_too_small` when what it fetches is more than
/// `free_bytes` (when known).
pub fn plan_download(
    entry: GenEntry,
    models_dir: &Path,
    engine: &EngineNeed,
    free_bytes: Option<u64>,
    used: &[UsedFile],
) -> AppResult<DownloadPlan> {
    let mut files = Vec::with_capacity(entry.parts.len());
    let mut already_here = Vec::new();
    let mut to_fetch_bytes = 0;
    for part in &entry.parts {
        let dest = download_destination(
            models_dir,
            &part.repo,
            &part.path,
            &part.revision,
            Some(&part.sha256),
            used,
        )
        .map_err(|e| AppError::internal(format!("a part's place: {e}")))?;
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
    for part in &entry.parts {
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
        already_here,
        to_fetch_bytes,
        licences,
        needs_engine,
        free_bytes,
    })
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

/// [`resolve_with`] with this app's store and catalog, and its Hub client for a search source
/// only.
async fn resolve(app: &AppHandle, source: &LocalSource) -> AppResult<Resolved> {
    let catalog = current_gen_catalog(app).await;
    let hf = match source {
        LocalSource::Search { .. } => Some(crate::hub::hf_client(app).await?),
        LocalSource::Catalog { .. } | LocalSource::Installed { .. } => None,
    };
    let state = app.state::<AppState>();
    resolve_with(&state.store, &catalog, hf.as_ref(), source).await
}

/// The entry `source` names, with the catalog id and the origin its installed row records:
/// from `catalog` (an entry it hides is `studio_not_supported_yet`); for a search source from
/// the repository's listing at that commit, asked of `hf` and recognised again here (the
/// webview's word is never taken for it); for an installed model from its saved parts in
/// `store` and its family record. Only a search source sends a request.
async fn resolve_with(
    store: &store::Store,
    catalog: &GenCatalog,
    hf: Option<&HfClient>,
    source: &LocalSource,
) -> AppResult<Resolved> {
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
        LocalSource::Search { repo, commit, file } => {
            let client =
                hf.ok_or_else(|| AppError::internal("a search source without a Hub client"))?;
            let detail = client
                .repo(repo, Some(commit))
                .await
                .map_err(crate::hub::hf_error)?;
            Ok(Resolved {
                entry: entry_from_listing(&detail, file, &catalog.families)?,
                catalog_id: None,
                source: store::GenSource::Search,
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
/// listing, asked of `hf`.
pub(crate) async fn entry_with(
    store: &store::Store,
    catalog: &GenCatalog,
    hf: Option<&HfClient>,
    source: &LocalSource,
) -> AppResult<GenEntry> {
    Ok(resolve_with(store, catalog, hf, source).await?.entry)
}

/// The entry `file` of the listing `detail` makes, recognised from the listing
/// (`gen_search::recognise`): a family harnessUI does not run is `studio_not_supported_yet`, and a file that is not one of the
/// repository's diffusion files with a published SHA-256 `not_found`.
pub fn entry_from_listing(
    detail: &RepoDetail,
    file: &str,
    families: &BTreeMap<GenFamily, FamilyRecord>,
) -> AppResult<GenEntry> {
    match recognise(detail, families) {
        Recognition::Supported { family, files } => {
            let found = files
                .iter()
                .find(|f| f.path == file)
                .ok_or_else(|| AppError::not_found("model file"))?;
            let record = families
                .get(&family)
                .ok_or_else(|| errors::not_supported_yet(errors::FAMILY_RECORD_MISSING))?;
            entry_for(detail, found, record).map_err(errors::not_supported_yet)
        }
        Recognition::NotSupportedYet(why) => Err(errors::not_supported_yet(why)),
    }
}

/// The entry an installed model is, without a request: its saved parts (the revisions and
/// SHA-256s it was downloaded at), with the rest from its catalog entry when the catalog still
/// lists it, else from its family record when its family is one of 0.1.x's, as a searched
/// file's entry is made (`gen_search::entry_for`), else from its recipe alone
/// ([`generic_entry`]). A searched file's licence is worked out again from its saved
/// repository, revision and licence id, so repairs show the same metadata. A 0.1.x family
/// this build has no record of, and a model with no recipe yet (*Set it up yourself*), are
/// `studio_not_supported_yet`.
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
        (None, Err(_)) => from_recipe(model)?,
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

/// What a download group keeps about the model it downloads: each part at its own revision.
fn gen_group(entry: &GenEntry, catalog_id: Option<String>, source: store::GenSource) -> GenGroup {
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
            .map(|p| GenGroupPart {
                role: word(&p.role),
                repo: p.repo.clone(),
                revision: p.revision.clone(),
                path: p.path.clone(),
                size: p.size,
                sha256: p.sha256.clone(),
                license: p.license.clone(),
            })
            .collect(),
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

/// Starts a new download or checks and repairs the pinned files of an installed model.
pub async fn start_local_download(app: &AppHandle, source: LocalSource) -> AppResult<String> {
    let state = app.state::<AppState>();
    if app
        .state::<Arc<StudioManager>>()
        .prefs()
        .local_notice_accepted_at
        .is_none()
    {
        return Err(errors::notice_needed());
    }
    let Resolved {
        entry,
        catalog_id,
        source: origin,
    } = resolve(app, &source).await?;
    let models_dir = crate::hub::models_dir(app).await;
    let free = reverse::diskspace::free_bytes(&models_dir);
    let engine = super::engine_cmd::engine_need(app);
    // The files the other installed models use, which no part is renamed over.
    let used = InstalledFiles::read(&state.store).await?.except(&entry.id);
    let plan = plan_download(entry, &models_dir, &engine, free, &used)?;
    let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
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
        generator: Some(gen_group(&plan.entry, catalog_id, origin)),
    };
    let group = downloads.start(app, meta, plan.files).await?;
    // Refused only while an engine process runs from the folder it would replace: the
    // model downloads, and the engine's own button offers it again.
    if plan.needs_engine
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
/// returns the files removed. A model not installed is `not_found`.
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
    let files: Vec<PathBuf> = model
        .parts
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p.get("file")?.as_str())
        .map(PathBuf::from)
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
mod tests {
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
        let plan = plan_download(e.clone(), &dir.0, &EngineNeed::Found, None, &[]).unwrap();
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
        let plan = plan_download(e, &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert!(plan.already_here.is_empty());
        assert_eq!(plan.to_fetch_bytes, 6_300);
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
            plan_download(e.clone(), &dir.0, &EngineNeed::Found, None, used)
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
        let plan = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None, &[]).unwrap();
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
        let e = plan_download(entry(), &dir.0, &EngineNeed::Found, Some(1_000), &[]).unwrap_err();
        assert_eq!(e.code, "studio_disk_too_small");
        assert_eq!(
            e.next_step,
            "It needs 0.1 GB; free up space or choose another models folder."
        );
        let total = entry().total_bytes();
        assert!(
            plan_download(
                entry(),
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
            !plan_download(entry(), &dir.0, &EngineNeed::Found, None, &[])
                .unwrap()
                .low_disk()
        );
        // Only what is fetched counts.
        write(&dest(&dir.0, &entry().parts[0]), 4_000);
        assert!(plan_download(entry(), &dir.0, &EngineNeed::Found, Some(2_300), &[]).is_ok());
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
        let model_licences = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None, &[])
            .unwrap()
            .licences;

        let engine = engine_licence();
        let plan = plan_download(
            two_licences(),
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
        let found = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None, &[]).unwrap();
        assert!(!found.needs_engine);
        assert!(!found.licences.contains(&engine));

        let missing =
            plan_download(two_licences(), &dir.0, &EngineNeed::Missing, None, &[]).unwrap();
        assert!(!missing.needs_engine, "no engine download promised");
        assert_eq!(missing.licences, model_licences);
    }

    /// The backend recognises a searched file again: the webview's word is never taken for
    /// it.
    #[test]
    fn an_unsupported_search_source_is_refused_by_the_backend() {
        let d = detail("repo-unknown-diffusion.json");
        assert_eq!(
            entry_from_listing(&d, &d.files[0].path, &families())
                .unwrap_err()
                .code,
            "studio_not_supported_yet"
        );
        let d = detail("repo-z-image-turbo-gguf.json");
        assert_eq!(
            entry_from_listing(&d, "not-in-the-listing.gguf", &families())
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            entry_from_listing(&d, "README.md", &families())
                .unwrap_err()
                .code,
            "not_found",
            "a file of the listing that is not a model file"
        );
        let e = entry_from_listing(&d, "z_image_turbo-Q4_K.gguf", &families()).unwrap();
        assert_eq!(e.id, "hf:leejet/Z-Image-Turbo-GGUF:z_image_turbo-Q4_K.gguf");
        assert_eq!(e.family, Some(GenFamily::ZImageTurbo));
        assert_eq!(e.diffusion().revision, d.commit);
        let d = detail("repo-uncensored.json");
        let file = "z_image_turbo_uncensored-Q4_K.gguf";
        let entry = entry_from_listing(&d, file, &families()).unwrap();
        assert_eq!(entry.diffusion().path, file);
        assert_eq!(entry.diffusion().revision, d.commit);
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
        let searched = models::gen_search::entry_for(&listed, file, record).unwrap();
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
            plan_download(e.clone(), models, &EngineNeed::Found, None, &[])
                .unwrap()
                .licences,
            plan_download(searched.clone(), models, &EngineNeed::Found, None, &[])
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
        // A Hub part still needs a licence the entry knows.
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
        assert_eq!(
            entry_from_installed(&row, &catalog()).unwrap_err().code,
            "studio_not_supported_yet"
        );
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
}
