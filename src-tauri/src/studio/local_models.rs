//! Downloading a picture or video model to this computer (studio-local-design.md, "Get local
//! models"; 0.1.0 Task L11).
//!
//! - **Which model.** A source ([`LocalSource`]) names an entry of the signed catalog's
//!   recommended list, a file of a Hugging Face repository at a commit (recognised again here
//!   from the repository's listing: the webview's word is never taken for it), or an installed
//!   model (its saved parts, with no request).
//! - **What it takes** ([`plan_download`]): every part, pinned by repository, revision, size
//!   and SHA-256, lands at `<models>/<owner>/<repo>/<path>`, so a part already there (a shared
//!   text encoder) is checked instead of fetched. A download that does not fit on the disk is
//!   refused before anything starts.
//! - **What it needs first** ([`check_acceptance`]): the note about local models, then every
//!   licence of every part, and the Studio engine's when it comes too, each accepted by its
//!   SHA-256 (recorded in `license_acceptances`).
//! - **The download** ([`start_local_download`]): one group of the download manager, each part
//!   at its own revision; once every part is checked, and the diffusion file's own tensor table
//!   is its family's, it joins Studio's installed models (`downloads::register_gen_model`). The
//!   Windows engine pack comes with the first model; where no engine is found and none can
//!   come, the model still downloads, and Get local models tells it from the engine's state.
//! - **Deleting** ([`delete_local_model`]): refused while a run on this computer waits for the
//!   model or uses it; else the row goes, then each part file nothing else uses (a chat model,
//!   another picture or video model, a download not finished yet), then the folders that
//!   leaves empty.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use models::download::{FileSpec, destination};
use models::gen_catalog::{
    FamilyRecord, GateMode, GenCatalog, GenEntry, GenFamily, GenGate, GenLicence, GenPart, GenRole,
    gen_catalog,
};
use models::gen_search::{Recognition, entry_for, recognise, searched_licence};
use models::hf::{Gated, HfClient, HubInfo, RepoDetail};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::engine_cmd::{ENGINE_LICENCE_ID, EngineNeed};
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
    /// when it downloads with the model: all are accepted first.
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

/// What downloading `entry` into `models_dir` takes: one pinned file per part (`commit` the
/// part's revision), a part whose destination already has its pinned size counted as here,
/// every licence to accept, and what the Studio engine needs (`engine`,
/// `engine_cmd::engine_need`): its pack comes too, with its licence to accept; an engine that
/// is neither here nor a pack changes nothing here (Get local models tells it from the
/// engine's state). Refused with `studio_disk_too_small` when what it fetches is more than
/// `free_bytes` (when known).
pub fn plan_download(
    entry: GenEntry,
    models_dir: &Path,
    engine: &EngineNeed,
    free_bytes: Option<u64>,
) -> AppResult<DownloadPlan> {
    let mut files = Vec::with_capacity(entry.parts.len());
    let mut already_here = Vec::new();
    let mut to_fetch_bytes = 0;
    for part in &entry.parts {
        let dest = destination(models_dir, &part.repo, &part.path)
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

/// Refuses a download until the note about local models was accepted (`notice_accepted`),
/// then until every licence of the plan is among `accepted` (SHA-256s).
pub fn check_acceptance(
    plan: &DownloadPlan,
    accepted: &BTreeSet<String>,
    notice_accepted: bool,
) -> AppResult<()> {
    if !notice_accepted {
        return Err(errors::notice_needed());
    }
    if !plan.licences.iter().all(|l| accepted.contains(&l.sha256)) {
        return Err(errors::license_not_accepted());
    }
    Ok(())
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
/// (`gen_search::recognise`): a family harnessUI does not run is `studio_not_supported_yet`, a
/// repository it never offers `studio_not_offered`, and a file that is not one of the
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
        Recognition::NotOffered(why) => Err(errors::not_offered(why)),
        Recognition::NotSupportedYet(why) => Err(errors::not_supported_yet(why)),
    }
}

/// The entry an installed model is, without a request: its saved parts (the revisions and
/// SHA-256s it was downloaded at), with the rest from its catalog entry when the catalog still
/// lists it, else from its family record as a searched file's entry is made
/// (`gen_search::entry_for`). A searched file's licence is worked out again from its saved
/// repository, revision and licence id, so it has the SHA-256 it was accepted under. A family
/// this build has no record of is `studio_not_supported_yet`.
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
    let mut entry = match listed {
        Some(entry) => entry.clone(),
        None => {
            let family: GenFamily = serde_json::from_value(model.family.clone().into())
                .map_err(|e| errors::not_supported_yet(e.to_string()))?;
            let record = catalog
                .families
                .get(&family)
                .ok_or_else(|| errors::not_supported_yet(errors::FAMILY_RECORD_MISSING))?;
            from_record(model, record, diffusion)
        }
    };
    entry.id.clone_from(&model.id);
    if let Some(missing) = parts
        .iter()
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

/// An installed searched model's entry from its family `record`, as `gen_search::entry_for`
/// made it: the record's figures and licences, plus the licence its diffusion file was
/// downloaded under. `searched_licence` reads only the repository, its commit and its label,
/// which the saved diffusion part holds, so the licence (and its SHA-256) is the same.
fn from_record(model: &store::GenModel, record: &FamilyRecord, diffusion: &GenPart) -> GenEntry {
    let saved = RepoDetail {
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
        family: record.family,
        inputs: record.inputs.clone(),
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
        family: word(&entry.family),
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

/// Starts downloading the model `source` names, once the note about local models and every
/// licence of every part (and of the Studio engine, when it comes too) are accepted (those
/// accepted before, or sent now as `accepted_license_sha256s`; a sent hash that is no licence
/// of the download is ignored), and when it fits on the disk. Each newly accepted licence is
/// recorded. Where the Studio engine is a pack and not here (Windows), its download starts
/// too. Returns the download group's id.
///
/// An installed source with no new hashes is *Download again*: every part is checked, and
/// what is missing or does not match its SHA-256 is fetched again.
pub async fn start_local_download(
    app: &AppHandle,
    source: LocalSource,
    accepted_license_sha256s: Vec<String>,
) -> AppResult<String> {
    let state = app.state::<AppState>();
    let Resolved {
        entry,
        catalog_id,
        source: origin,
    } = resolve(app, &source).await?;
    let models_dir = crate::hub::models_dir(app).await;
    let free = reverse::diskspace::free_bytes(&models_dir);
    let engine = super::engine_cmd::engine_need(app);
    let plan = plan_download(entry, &models_dir, &engine, free)?;
    let studio = app.state::<Arc<StudioManager>>();
    let notice_accepted = studio.prefs().local_notice_accepted_at.is_some();
    let accepted = Accepted {
        sent: &accepted_license_sha256s,
        notice: notice_accepted,
        catalog_id: catalog_id.clone(),
        now: studio.deps().clock.now_ms(),
    };
    let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
    accept_then_start(&state.store, plan, accepted, |plan| async move {
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
    })
    .await
}

/// What a local download was given to go ahead: the licence SHA-256s sent with it, whether the
/// note about local models was accepted, the catalog entry it is for and the time now (Unix
/// ms) to record acceptances at.
struct Accepted<'a> {
    sent: &'a [String],
    notice: bool,
    catalog_id: Option<String>,
    now: i64,
}

/// The steps of [`start_local_download`] after its plan: refused ([`check_acceptance`]) unless
/// the note and every licence of the plan, the Studio engine's included when it comes, are
/// accepted, before (in `store`) or now (`accepted.sent`, any letter case); then each licence of
/// the plan sent now is recorded (the engine's for no catalog entry), and `start` starts the
/// download. A refusal records and starts nothing.
async fn accept_then_start<F, Fut>(
    store: &store::Store,
    plan: DownloadPlan,
    accepted: Accepted<'_>,
    start: F,
) -> AppResult<String>
where
    F: FnOnce(DownloadPlan) -> Fut,
    Fut: Future<Output = AppResult<String>>,
{
    let sent: BTreeSet<String> = accepted
        .sent
        .iter()
        .map(|h| h.to_ascii_lowercase())
        .collect();
    let mut known = store.accepted_license_hashes().await?;
    known.extend(sent.iter().cloned());
    check_acceptance(&plan, &known, accepted.notice)?;
    for licence in plan.licences.iter().filter(|l| sent.contains(&l.sha256)) {
        let catalog_id = if licence.id == ENGINE_LICENCE_ID {
            None
        } else {
            accepted.catalog_id.clone()
        };
        store
            .record_license_acceptance(store::LicenseAcceptance {
                sha256: licence.sha256.clone(),
                license_id: licence.id.clone(),
                catalog_id,
                accepted_at: accepted.now,
            })
            .await?;
    }
    start(plan).await
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

    fn set(hashes: &[&str]) -> BTreeSet<String> {
        hashes.iter().map(|h| (*h).to_string()).collect()
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
        let plan = plan_download(e.clone(), &dir.0, &EngineNeed::Found, None).unwrap();
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
        let plan = plan_download(e, &dir.0, &EngineNeed::Found, None).unwrap();
        assert!(plan.already_here.is_empty());
        assert_eq!(plan.to_fetch_bytes, 6_300);
    }

    /// Nothing downloads before the note about local models is accepted, and every licence of
    /// every part.
    #[test]
    fn downloads_are_refused_until_the_notice_and_every_licence_are_accepted() {
        let dir = Scratch::new();
        let plan = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None).unwrap();
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
        let apache = apache_sha();
        assert_eq!(
            check_acceptance(&plan, &set(&[]), false).unwrap_err().code,
            "studio_notice_needed"
        );
        assert_eq!(
            check_acceptance(&plan, &set(&[&apache, KOMMUNITY_SHA]), false)
                .unwrap_err()
                .code,
            "studio_notice_needed"
        );
        assert_eq!(
            check_acceptance(&plan, &set(&[&apache]), true)
                .unwrap_err()
                .code,
            "studio_license_not_accepted",
            "one licence of two"
        );
        assert!(check_acceptance(&plan, &set(&[&apache, KOMMUNITY_SHA]), true).is_ok());
    }

    /// A download that does not fit on the disk is refused before anything starts; one that
    /// leaves less than 10 GB free says so.
    #[test]
    fn a_disk_too_small_is_refused_before_anything_starts() {
        let dir = Scratch::new();
        let e = plan_download(entry(), &dir.0, &EngineNeed::Found, Some(1_000)).unwrap_err();
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
                Some(total + 5_000_000_000)
            )
            .unwrap()
            .low_disk()
        );
        let roomy = plan_download(
            entry(),
            &dir.0,
            &EngineNeed::Found,
            Some(total + LOW_DISK_BYTES),
        )
        .unwrap();
        assert!(!roomy.low_disk());
        assert_eq!(roomy.free_bytes, Some(total + LOW_DISK_BYTES));
        // The free space unknown: nothing is refused or said.
        assert!(
            !plan_download(entry(), &dir.0, &EngineNeed::Found, None)
                .unwrap()
                .low_disk()
        );
        // Only what is fetched counts.
        write(&dest(&dir.0, &entry().parts[0]), 4_000);
        assert!(plan_download(entry(), &dir.0, &EngineNeed::Found, Some(2_300)).is_ok());
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
    /// its licence is accepted with the model's; where it is found, nothing comes; where it is
    /// neither found nor a pack, the plan promises no engine download.
    #[test]
    fn the_engine_comes_with_the_first_model_where_it_is_a_pack() {
        let dir = Scratch::new();
        let model_licences = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None)
            .unwrap()
            .licences;

        let engine = engine_licence();
        let plan = plan_download(
            two_licences(),
            &dir.0,
            &EngineNeed::Pack(engine.clone()),
            None,
        )
        .unwrap();
        assert!(plan.needs_engine);
        let mut with_engine = model_licences.clone();
        with_engine.push(engine.clone());
        assert_eq!(plan.licences, with_engine, "the model's, then the engine's");
        let model_hashes: Vec<&str> = model_licences.iter().map(|l| l.sha256.as_str()).collect();
        assert_eq!(
            check_acceptance(&plan, &set(&model_hashes), true)
                .unwrap_err()
                .code,
            "studio_license_not_accepted",
            "the engine's licence not accepted"
        );
        let mut every = model_hashes.clone();
        every.push(&engine.sha256);
        assert!(check_acceptance(&plan, &set(&every), true).is_ok());

        let found = plan_download(two_licences(), &dir.0, &EngineNeed::Found, None).unwrap();
        assert!(!found.needs_engine);
        assert!(!found.licences.contains(&engine));

        let missing = plan_download(two_licences(), &dir.0, &EngineNeed::Missing, None).unwrap();
        assert!(!missing.needs_engine, "no engine download promised");
        assert_eq!(missing.licences, model_licences);
    }

    /// What a download is given: the licences `sent`, after the note was accepted.
    fn given(sent: &[String]) -> Accepted<'_> {
        Accepted {
            sent,
            notice: true,
            catalog_id: Some("flux2-klein-4b-q4".into()),
            now: 7,
        }
    }

    /// A download that brings the engine is refused without the engine's licence: nothing is
    /// recorded and nothing starts. With it, each licence sent is recorded and the download
    /// starts once.
    #[tokio::test]
    async fn a_start_without_the_engine_licence_downloads_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        let engine = engine_licence();
        let plan = plan_download(entry(), &dir.0, &EngineNeed::Pack(engine.clone()), None).unwrap();
        let apache = apache_sha();
        let started = AtomicUsize::new(0);
        let start = |_: DownloadPlan| async {
            started.fetch_add(1, Ordering::SeqCst);
            Ok("g".to_string())
        };
        let model_only = [apache.to_uppercase()];
        let err = accept_then_start(&store, plan.clone(), given(&model_only), start)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_license_not_accepted");
        assert_eq!(started.load(Ordering::SeqCst), 0, "nothing downloads");
        assert!(
            store.accepted_license_hashes().await.unwrap().is_empty(),
            "nothing is recorded"
        );

        let both = [apache.clone(), engine.sha256.clone()];
        let group = accept_then_start(&store, plan.clone(), given(&both), start)
            .await
            .unwrap();
        assert_eq!(group, "g");
        assert_eq!(started.load(Ordering::SeqCst), 1);
        assert_eq!(
            store.accepted_license_hashes().await.unwrap(),
            set(&[&apache, &engine.sha256])
        );

        // Accepted before: nothing needs sending again.
        accept_then_start(&store, plan, given(&[]), start)
            .await
            .unwrap();
        assert_eq!(started.load(Ordering::SeqCst), 2);
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
        assert_eq!(e.family, GenFamily::ZImageTurbo);
        assert_eq!(e.diffusion().revision, d.commit);
        let d = detail("repo-uncensored.json");
        assert_eq!(
            entry_from_listing(&d, &d.files[0].path, &families())
                .unwrap_err()
                .code,
            "studio_not_offered"
        );
    }

    /// An installed model whose parts are the files at `files`, each `(role, path)`.
    fn gen_model(id: &str, files: &[(&str, &Path)]) -> store::NewGenModel {
        store::NewGenModel {
            id: id.into(),
            catalog_id: None,
            name: format!("Model {id}"),
            kind: "image".into(),
            family: "z-image-turbo".into(),
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
        assert_eq!(e.family, GenFamily::ZImageTurbo);
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
            plan_download(e.clone(), models, &EngineNeed::Found, None)
                .unwrap()
                .licences,
            plan_download(searched.clone(), models, &EngineNeed::Found, None)
                .unwrap()
                .licences,
            "the same licences to accept"
        );

        // A recommended model keeps its saved parts too, with the catalog's figures.
        let mut klein = catalog.entries["flux2-klein-4b-q4"].clone();
        klein.parts[0].revision = "0".repeat(40);
        let row = store::GenModel {
            family: "flux2-klein-4b".into(),
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
