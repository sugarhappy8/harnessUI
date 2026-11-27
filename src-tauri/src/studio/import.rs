//! Adding a model file from this computer (design 7.8, Task S8): *Add a model file* on Get
//! models and in the model switcher.
//!
//! - **Reading it** ([`inspect_import`]): the path is untrusted input from the webview, made
//!   canonical and checked (`local_models::checked_disk_file`: a regular file of a format
//!   Studio reads; a pickle file is refused by its extension, "Studio can't read .ckpt files",
//!   never opened). Only its header is read: its label, the set-up plan of a file that works
//!   (its main file already here, from this computer), whether it is inside the models folder,
//!   and the mode the person chose last time. Nothing is hashed or sent.
//! - **Adding it** ([`import_model`]): a file inside the models folder is used where it is,
//!   without asking; any other is copied into `<models>/@imported/<SHA-256, 16
//!   characters>/<its name>` (through a `.partial` file, then renamed; a copy already there
//!   with the same SHA-256 is reused) or used where it is ([`ImportMode`]). It is never
//!   moved. The file is hashed with SHA-256 while it is read or copied, with progress
//!   ([`IMPORT_PROGRESS_EVENT`]) at most every 250 ms and once at the end. Everything that can
//!   refuse it (a LoRA, `studio_import_is_lora`; a set-up missing a part; a gated file with no
//!   token; no room on the disk) is checked before the file is read through.
//! - **Its row**: id `local:<SHA-256, 16 characters>`, source `import`, the main part the
//!   person's own file (no repository, revision or licence; `local`). Its set-up follows the
//!   plan as a download's does: when the plan fetches files, one download group starts with
//!   the main part already here (`present`) and the model is added once they are checked
//!   (`downloads::register_gen_model`); otherwise it is added at once
//!   (`downloads::register_present_gen_model`). Deleting it never deletes the person's file;
//!   only Studio's copy under `@imported` goes (`local_models::delete_files_of`).
//!
//! Each command is a thin wrapper over a plain function taking what it reads
//! ([`LocalCmdDeps`]), as Get models' are.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use models::sd::FileLabel;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager};

use super::errors;
use super::local_cmd::{
    LocalCmdDeps, app_deps_for_writing, plan_deps, recipe_choices, setup_plan_dto,
};
use super::local_models::{
    Added, DownloadDeps, is_studios_copy, plan_download, prepared_download, remove_unused_files,
};
use super::manager::StudioManager;
use super::setup_plan::{DiskFile, disk_setup, plan_disk_with, read_disk_file, ready_to_fetch};
use crate::downloads::{
    DownloadManager, files_being_written, inside_folder, register_present_gen_model,
};
use crate::dto::{RecipeChoiceDto, SetupChoice, SetupPlanDto};
use crate::error::AppResult;

/// The event a model file's import sends to the main window while it is read or copied.
pub const IMPORT_PROGRESS_EVENT: &str = "studio-import-progress";

/// The folder of the models folder where imported files are copied, one folder per file's
/// SHA-256 (16 characters). Not a Hugging Face owner name, so never a repo's place.
pub(crate) const IMPORTED_FOLDER: &str = "@imported";

/// At most one progress event this often while a file is read.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How much of a file is read at once.
const CHUNK: usize = 1 << 20;

/// How a model file from outside the models folder is added: copied into it, or used where it
/// is. It is never moved.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImportMode {
    Copy,
    InPlace,
}

/// What an import is doing with the file.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ImportStage {
    /// Reading it through for its SHA-256 (a file used where it is, or a copy already there).
    Hashing,
    /// Copying it into the models folder, hashing it on the way.
    Copying,
}

/// How far an import of the file at `path` (as the webview named it) is.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub path: String,
    pub stage: ImportStage,
    pub done_bytes: u64,
    pub total_bytes: u64,
}

/// A model file from this computer, read before it is added.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportInspectionDto {
    /// Its canonical path, which the import takes.
    pub path: String,
    /// Its file name.
    pub name: String,
    pub size_bytes: u64,
    /// What it is (`FILE_LABEL` words it): a LoRA is added as one instead.
    pub label: FileLabel,
    /// Its set-up when it works in Studio: its main file already here, the companions it
    /// fetches, their licences and the total.
    pub plan: Option<SetupPlanDto>,
    /// The mode to offer first: the last one chosen, else copying.
    pub last_mode: ImportMode,
    /// It is inside the models folder: used where it is, without asking.
    pub inside_models: bool,
    /// For weights Studio can't place (`CannotRun { UnknownWeights }`): the model types it may
    /// be set up as by hand (*Set it up yourself*, untested), every recipe Studio runs, by
    /// kind and then name. Empty for any other file.
    pub by_hand_recipes: Vec<RecipeChoiceDto>,
}

/// What an import did: the model added (`gen_model_id`), or the download group that adds it
/// once its companions are fetched.
#[derive(Debug, Serialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportResultDto {
    pub gen_model_id: Option<String>,
    pub download_group: Option<String>,
}

/// Where an import's progress goes: the main window in the app, a list in tests.
pub(crate) type ProgressSink = Arc<dyn Fn(ImportProgress) + Send + Sync>;

/// The model file at `path` (untrusted) read before it is added: its label from its header,
/// its set-up plan when it works in Studio, the mode to offer first and whether it is inside
/// the models folder. Header only: nothing is hashed, copied or sent.
pub async fn inspect_import(deps: &LocalCmdDeps, path: &Path) -> AppResult<ImportInspectionDto> {
    let file = read_disk_file(path).await?;
    let label = file.label();
    let by_hand_recipes = if by_hand(&label) {
        recipe_choices(None, true)
    } else {
        Vec::new()
    };
    let plan = match label {
        FileLabel::Works { .. } => {
            let catalog = deps.catalog.gen_catalog().await;
            let plan_deps = plan_deps(deps, &catalog);
            let choice = SetupChoice::default();
            let setup = disk_setup(&plan_deps, &file, &choice).await?;
            let at = (file.path.as_path(), None, as_found(&file));
            let planned = plan_disk_with(&plan_deps, &file, &setup, at, &choice)?;
            Some(setup_plan_dto(deps, &planned).await?)
        }
        _ => None,
    };
    Ok(ImportInspectionDto {
        path: file.path.display().to_string(),
        name: file.name.clone(),
        size_bytes: file.size,
        label,
        plan,
        last_mode: deps.last_import_mode.unwrap_or(ImportMode::Copy),
        inside_models: inside_models(&deps.models_dir, &file.path),
        by_hand_recipes,
    })
}

/// Whether a file with `label` can be set up by hand, with a model type the person chooses:
/// weights the engine does not recognise. Never a pickle file (refused before it is read), a
/// LoRA, an extra part or a model the engine is known to misrun.
fn by_hand(label: &FileLabel) -> bool {
    matches!(
        label,
        FileLabel::CannotRun {
            reason: models::sd::CannotRunReason::UnknownWeights,
            ..
        }
    )
}

/// The id a file's plan has before its SHA-256 is known (`local_models::plan_disk`'s).
fn as_found(file: &DiskFile) -> String {
    format!("local:{}", file.path.display())
}

/// Whether the file at `path` (canonical) is inside the models folder `models_dir`.
fn inside_models(models_dir: &Path, path: &Path) -> bool {
    dunce::canonicalize(models_dir).is_ok_and(|root| inside_folder(path, &root))
}

/// An import ready to finish: the model added already, or the download that adds it, and the
/// mode the person chose.
pub(crate) struct PreparedImport {
    pub added: Added,
    /// The Studio engine is not on this computer and comes as a pack: it downloads with the
    /// model, as with a downloaded model.
    pub needs_engine: bool,
    /// The mode the person was offered and chose, to offer first next time; `None` for a file
    /// inside the models folder, used where it is without asking.
    pub chosen: Option<ImportMode>,
    /// Studio's copy under `<models>/@imported/` that this import made or reused; `None` for a
    /// file used where it is. Removed if the download that adds the model cannot start.
    pub copied: Option<PathBuf>,
}

/// *Add* on the model file at `path` (untrusted) with `mode` and the person's `choice` for
/// its set-up, the download it needs not started yet:
/// 1. the note about local models must have been accepted (`studio_notice_needed`), as for a
///    download; the file is checked and its header read; a LoRA is `studio_import_is_lora`,
///    another file Studio does not run `studio_not_supported_yet`;
/// 2. its set-up is planned as a download's (`studio_setup_incomplete`, `hf_needs_token`,
///    `studio_no_checksum`), and what it fetches, plus a copy of the file, must fit on the disk
///    (`studio_disk_too_small`): all before the file is read through;
/// 3. the file is hashed as it is read (used where it is: inside the models folder always) or
///    copied into the models folder ([`copy_in`]), with progress to `progress`;
/// 4. planned again where it now is, with its SHA-256, as `local:<SHA-256, 16 characters>`:
///    added at once when every part is here, else one download group of what it fetches. A
///    file imported again replaces its row; the files only the old row used stay while a run
///    on this computer holds the model (`in_use`: `downloads::register_present_gen_model`).
pub(crate) async fn prepare_import(
    deps: &LocalCmdDeps,
    path: &Path,
    mode: ImportMode,
    choice: Option<&SetupChoice>,
    progress: &ProgressSink,
    in_use: &(dyn Fn(&str) -> bool + Sync),
) -> AppResult<PreparedImport> {
    if !deps.notice_accepted {
        return Err(errors::notice_needed());
    }
    let file = read_disk_file(path).await?;
    let label = file.label();
    match label {
        FileLabel::Works { .. } => {}
        FileLabel::Lora { .. } => return Err(errors::import_is_lora()),
        // *Set it up yourself*: with the model type the person chose (`disk_setup`).
        _ if by_hand(&label) && choice.is_some_and(|c| c.recipe.is_some()) => {}
        _ => return Err(errors::not_supported_yet(errors::NOT_A_RUNNABLE_FILE)),
    }
    let inside = inside_models(&deps.models_dir, &file.path);
    let copy = mode == ImportMode::Copy && !inside;
    let catalog = deps.catalog.gen_catalog().await;
    let plan_deps = plan_deps(deps, &catalog);
    let choice = choice.cloned().unwrap_or_default();
    let setup = disk_setup(&plan_deps, &file, &choice).await?;

    // Checked before the file is read through.
    let first = plan_disk_with(
        &plan_deps,
        &file,
        &setup,
        (&file.path, None, as_found(&file)),
        &choice,
    )?;
    ready_to_fetch(&first, plan_deps.hf)?;
    let used = deps.installed_files.except(&first.entry.id);
    let fetched = plan_download(
        first.entry,
        Some(&first.plan),
        &deps.models_dir,
        &deps.engine_need,
        None,
        &used,
    )?
    .to_fetch_bytes;
    let needed = fetched + if copy { file.size } else { 0 };
    if deps.free_disk_bytes.is_some_and(|free| needed > free) {
        return Err(errors::disk_too_small(needed));
    }

    let mut report = Report::new(path.display().to_string(), progress.clone());
    let (src, size, models_dir) = (file.path.clone(), file.size, deps.models_dir.clone());
    let (at, sha256) = tokio::task::spawn_blocking(move || {
        if copy {
            copy_in(&src, size, &models_dir, &mut report)
        } else {
            let sha256 = read_through(&src, size, None, ImportStage::Hashing, &mut report)?;
            Ok((src, sha256))
        }
    })
    .await
    .map_err(|e| errors::import_failed(format!("reading the file stopped: {e}")))??;

    // From here on a failure leaves no copy behind (final review, area 2, M1).
    let copied = copy.then(|| at.clone());
    let finish = async {
        let id = format!("local:{}", &sha256[..16]);
        let planned = plan_disk_with(&plan_deps, &file, &setup, (&at, Some(&sha256), id), &choice)?;
        let download_deps = DownloadDeps {
            plan: plan_deps,
            store: &deps.store,
            models_dir: &deps.models_dir,
            // What it fetches was checked against the free space with the copy, above.
            free_bytes: None,
            engine: &deps.engine_need,
        };
        let prepared = prepared_download(
            &download_deps,
            planned.entry,
            Some(&planned.plan),
            None,
            store::GenSource::Import,
        )?;
        let needs_engine = prepared.needs_engine;
        let added = if prepared.files.is_empty() {
            Added::Now(
                register_present_gen_model(&deps.store, &deps.models_dir, &prepared.meta, in_use)
                    .await?,
            )
        } else {
            Added::Download(Box::new(prepared))
        };
        Ok((added, needs_engine))
    };
    let (added, needs_engine) =
        removing_copy_on_error(&deps.store, &deps.models_dir, copied.as_deref(), finish).await?;
    Ok(PreparedImport {
        added,
        needs_engine,
        chosen: (!inside).then_some(mode),
        copied,
    })
}

/// `work`'s result; when it fails, Studio's own `copy` made for this import is removed
/// ([`remove_unused_copy`]), so a failed import leaves no copy of many GB that no row owns
/// and nothing would ever sweep (final review, area 2, M1).
pub(crate) async fn removing_copy_on_error<T>(
    store: &store::Store,
    models_dir: &Path,
    copy: Option<&Path>,
    work: impl std::future::Future<Output = AppResult<T>>,
) -> AppResult<T> {
    let result = work.await;
    if result.is_err()
        && let Some(copy) = copy
    {
        remove_unused_copy(store, models_dir, copy).await;
    }
    result
}

/// Removes `copy` when it is Studio's copy under `<models>/@imported/`
/// (`local_models::is_studios_copy`), through the path that removes any unused model file
/// (`local_models::remove_unused_files`): never while an installed model uses it or an
/// unfinished download writes or will use it. Anything else, the person's own file above
/// all, is left alone. A failure is logged: the import's own error is what the person sees.
async fn remove_unused_copy(store: &store::Store, models_dir: &Path, copy: &Path) {
    if !is_studios_copy(models_dir, copy) {
        return;
    }
    let writing = match files_being_written(store, None).await {
        Ok(writing) => writing,
        Err(e) => {
            tracing::warn!(code = %e.code, "a failed import's copy stays for now");
            return;
        }
    };
    let busy = |p: &Path| writing.contains(&p.display().to_string());
    if let Err(e) = remove_unused_files(store, models_dir, &[copy.to_path_buf()], &busy).await {
        tracing::warn!(code = %e.code, "a failed import's copy stays for now");
    }
}

/// *Add* in the app ([`prepare_import`], progress to the main window): the model added, or its
/// download started (the Studio engine's pack with it where it is needed); the mode the
/// person chose is kept for next time. A failed read of the installed models' files refuses
/// it before anything is read through, copied or started (`local_cmd::app_deps_for_writing`).
pub async fn import_model(
    app: &AppHandle,
    path: &Path,
    mode: ImportMode,
    choice: Option<SetupChoice>,
) -> AppResult<ImportResultDto> {
    let studio = app.state::<Arc<StudioManager>>();
    let hf = crate::hub::hf_client(app).await?;
    // Strict: the installed models' files guard what the import writes.
    let deps = app_deps_for_writing(app, &studio, Some(hf)).await?;
    let to = app.clone();
    let progress: ProgressSink = Arc::new(move |p| {
        let _ = to.emit_to(crate::MAIN_WINDOW, IMPORT_PROGRESS_EVENT, p);
    });
    let in_use = |id: &str| studio.local_uses_model(id);
    let prepared = prepare_import(&deps, path, mode, choice.as_ref(), &progress, &in_use).await?;
    let result = match prepared.added {
        Added::Now(id) => ImportResultDto {
            gen_model_id: Some(id),
            download_group: None,
        },
        Added::Download(download) => {
            let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
            // Refused while another download writes one of its files: the copy goes too.
            let start = downloads.start(app, download.meta, download.files);
            let copy = prepared.copied.as_deref();
            let group = removing_copy_on_error(&deps.store, &deps.models_dir, copy, start).await?;
            ImportResultDto {
                gen_model_id: None,
                download_group: Some(group),
            }
        }
    };
    if prepared.needs_engine
        && let Err(e) = super::engine_cmd::start_engine_download(app).await
    {
        tracing::warn!(code = %e.code, "the Studio engine did not download with the model");
    }
    if let Some(mode) = prepared.chosen
        && let Err(e) = studio.note_import_mode(mode).await
    {
        tracing::warn!(code = %e.code, "the import mode was not kept");
    }
    Ok(result)
}

/// Copies the file at `src` (`size` bytes) into `<models_dir>/@imported/<SHA-256, 16
/// characters>/<its name>`, hashing it on the way ([`copy_to_partial`], then
/// [`place_partial`]): a file already there with the same SHA-256 is reused, any other file
/// there is replaced. Returns where the copy is and the SHA-256.
fn copy_in(
    src: &Path,
    size: u64,
    models_dir: &Path,
    report: &mut Report,
) -> AppResult<(PathBuf, String)> {
    let name = src
        .file_name()
        .ok_or_else(|| errors::import_failed("a file without a name"))?;
    let folder = models_dir.join(IMPORTED_FOLDER);
    let (partial, sha256) = copy_to_partial(src, size, &folder, report)?;
    let dest = folder.join(&sha256[..16]).join(name);
    place_partial(&partial, &dest, size, &sha256, report)?;
    Ok((dest, sha256))
}

/// Copies the file at `src` (`size` bytes) into a new `<uuid>.partial` file in `folder`
/// (made when missing), hashing it on the way with progress to `report` (`Copying`). Returns
/// the partial file and the SHA-256; on any failure the partial file is removed. Task S10's
/// LoRA import copies through it too (`loras::import_lora_file`).
pub(crate) fn copy_to_partial(
    src: &Path,
    size: u64,
    folder: &Path,
    report: &mut Report,
) -> AppResult<(PathBuf, String)> {
    let failed = |e: std::io::Error| errors::import_failed(e.to_string());
    std::fs::create_dir_all(folder).map_err(failed)?;
    let partial = folder.join(format!("{}.partial", uuid::Uuid::new_v4()));
    let copied = std::fs::File::create(&partial)
        .map_err(failed)
        .and_then(|mut out| {
            let sha256 = read_through(src, size, Some(&mut out), ImportStage::Copying, report)?;
            out.sync_all().map_err(failed)?;
            Ok(sha256)
        });
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    copied.map(|sha256| (partial, sha256))
}

/// Puts the partial file [`copy_to_partial`] made (`size` bytes, SHA-256 `sha256`) at `dest`
/// by renaming it there (its folder made when missing). A file already at `dest` with the same
/// size and SHA-256 (read through to tell, `Hashing`) is kept and the partial file removed;
/// any other file there is replaced. On failure the partial file is removed.
pub(crate) fn place_partial(
    partial: &Path,
    dest: &Path,
    size: u64,
    sha256: &str,
    report: &mut Report,
) -> AppResult<()> {
    let failed = |e: std::io::Error| errors::import_failed(e.to_string());
    let there = std::fs::metadata(dest).is_ok_and(|m| m.is_file() && m.len() == size)
        && read_through(dest, size, None, ImportStage::Hashing, report).is_ok_and(|s| s == sha256);
    if there {
        let _ = std::fs::remove_file(partial);
        return Ok(());
    }
    let placed = dest
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::rename(partial, dest))
        .map_err(failed);
    if placed.is_err() {
        let _ = std::fs::remove_file(partial);
    }
    placed
}

/// Reads the file at `path` through (writing each chunk to `out` too, when copying) and gives
/// its SHA-256 in lowercase hex, telling `report` how far it is. A file that is not `size`
/// bytes long when read (something writes it) is `studio_import_changed`.
fn read_through(
    path: &Path,
    size: u64,
    mut out: Option<&mut std::fs::File>,
    stage: ImportStage,
    report: &mut Report,
) -> AppResult<String> {
    let failed = |e: std::io::Error| errors::import_failed(e.to_string());
    let mut file = std::fs::File::open(path).map_err(failed)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        let n = match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(failed(e)),
        };
        hash.update(&buf[..n]);
        if let Some(out) = out.as_deref_mut() {
            out.write_all(&buf[..n]).map_err(failed)?;
        }
        done += n as u64;
        if done > size {
            return Err(errors::import_file_changed());
        }
        report.tick(stage, done, size);
    }
    if done != size {
        return Err(errors::import_file_changed());
    }
    report.end(stage, size);
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The progress of one import: its file as the webview named it, where events go, and when
/// the last went.
pub(crate) struct Report {
    path: String,
    sink: ProgressSink,
    throttle: Throttle,
}

impl Report {
    pub(crate) fn new(path: String, sink: ProgressSink) -> Self {
        Self {
            path,
            sink,
            throttle: Throttle::default(),
        }
    }

    fn send(&self, stage: ImportStage, done_bytes: u64, total_bytes: u64) {
        (self.sink)(ImportProgress {
            path: self.path.clone(),
            stage,
            done_bytes,
            total_bytes,
        });
    }

    /// `done` of `total` bytes, unless an event went less than 250 ms ago.
    fn tick(&mut self, stage: ImportStage, done: u64, total: u64) {
        if self.throttle.due(Instant::now()) {
            self.send(stage, done, total);
        }
    }

    /// The end of a pass: always sent, `done_bytes == total_bytes`.
    fn end(&mut self, stage: ImportStage, total: u64) {
        self.throttle.last = Some(Instant::now());
        self.send(stage, total, total);
    }
}

/// At most one event per [`PROGRESS_EVERY`].
#[derive(Debug, Default)]
struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    /// Whether an event may go at `now`; if so, it counts as sent.
    fn due(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_some_and(|last| now.saturating_duration_since(last) < PROGRESS_EVERY)
        {
            return false;
        }
        self.last = Some(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use models::gen_catalog::GenRole;
    use models::sd::setup::PartSource;
    use models::sd::testing::{file_of, fixture};
    use serde_json::json;
    use store::GenSource;

    use super::*;
    use crate::downloads::{GroupMeta, finish_gen_group};
    use crate::studio::local_cmd::tests::{
        RepoHub, bundled_lacking, gpu, hub, repo_deps, with_catalog,
    };
    use crate::studio::local_models::tests::{install_catalog_entry, small_weights};
    use crate::studio::local_models::{
        InstalledFiles, LocalSource, add_now_if_present, delete_files_of, prepare_download,
    };
    use crate::studio::test_support::Scratch;

    /// A file whose header is the detection fixture `name`'s tensor list, with no tensor data:
    /// detection reads only the header.
    fn fixture_bytes(name: &str) -> Vec<u8> {
        file_of(&fixture(name))
    }

    /// The fixture `name` as a GGUF file.
    fn gguf_bytes(name: &str) -> Vec<u8> {
        let mut index = fixture(name);
        index.format = models::sd::index::FileFormat::Gguf;
        file_of(&index)
    }

    async fn sha256_file(path: &Path) -> String {
        models::download::sha256_file(path).await.unwrap()
    }

    /// *Add a model file* with Get models' dependencies (the bundled catalog and its
    /// companion table, a models folder of its own, 500 GB free, the note accepted), a download
    /// manager that records groups without running them, and the progress events kept.
    struct T {
        deps: LocalCmdDeps,
        downloads: DownloadManager,
        events: Arc<Mutex<Vec<ImportProgress>>>,
        scratch: Scratch,
        _hub: RepoHub,
    }

    async fn harness() -> T {
        let scratch = Scratch::new();
        let served = hub(Vec::new()).await;
        let mut deps = repo_deps(&served, gpu(24, 64), &scratch);
        deps.notice_accepted = true;
        T {
            deps,
            downloads: DownloadManager::default(),
            events: Arc::new(Mutex::new(Vec::new())),
            scratch,
            _hub: served,
        }
    }

    /// The harness for a file whose companions come from the catalog's companion table (its
    /// files pinned to their Hub repos, never fetched here: [`T::complete`] puts them in place).
    async fn harness_with_hub() -> T {
        harness().await
    }

    impl T {
        fn store(&self) -> &store::Store {
            &self.deps.store
        }

        fn models_dir(&self) -> &Path {
            &self.deps.models_dir
        }

        fn write(path: &Path, bytes: Vec<u8>) -> PathBuf {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
            dunce::canonicalize(path).unwrap()
        }

        /// A file outside the models folder, at `rel` (made canonical, as the import gives it).
        fn outside_file(&self, rel: &str, bytes: Vec<u8>) -> PathBuf {
            Self::write(&self.scratch.path().join("outside").join(rel), bytes)
        }

        /// A file inside the models folder, at `rel`.
        fn models_file(&self, rel: &str, bytes: Vec<u8>) -> PathBuf {
            Self::write(&self.models_dir().join(rel), bytes)
        }

        fn sink(&self) -> ProgressSink {
            let events = self.events.clone();
            Arc::new(move |p| events.lock().unwrap().push(p))
        }

        /// The installed models' files as a command reads them.
        async fn refresh(&mut self) {
            self.deps.installed_files = InstalledFiles::read(self.store()).await.unwrap();
        }

        async fn inspect(&mut self, path: &Path) -> AppResult<ImportInspectionDto> {
            self.refresh().await;
            inspect_import(&self.deps, path).await
        }

        async fn prepare(
            &mut self,
            path: &Path,
            mode: ImportMode,
            choice: Option<SetupChoice>,
        ) -> AppResult<PreparedImport> {
            self.refresh().await;
            let sink = self.sink();
            prepare_import(&self.deps, path, mode, choice.as_ref(), &sink, &|_| false).await
        }

        /// *Add* on the file at `path`: what `import_model` does, a download group recorded but
        /// not run.
        async fn import(
            &mut self,
            path: &Path,
            mode: ImportMode,
            choice: Option<SetupChoice>,
        ) -> AppResult<ImportResultDto> {
            Ok(match self.prepare(path, mode, choice).await?.added {
                Added::Now(id) => ImportResultDto {
                    gen_model_id: Some(id),
                    download_group: None,
                },
                Added::Download(d) => ImportResultDto {
                    gen_model_id: None,
                    download_group: Some(
                        self.downloads
                            .record_group(self.store(), &d.meta, &d.files)
                            .await?,
                    ),
                },
            })
        }

        async fn rows(&self, group: &str) -> Vec<store::Download> {
            self.store()
                .list_downloads(Some(group.to_owned()))
                .await
                .unwrap()
        }

        async fn meta(&self, group: &str) -> GroupMeta {
            self.store()
                .get_setting(&format!("downloadGroup:{group}"))
                .await
                .unwrap()
                .unwrap()
        }

        /// Puts each file `group` fetches in place (a small safetensors file) and finishes it
        /// as its task does.
        async fn complete(&self, group: &str) {
            for row in self.rows(group).await {
                T::write(Path::new(&row.dest_path), small_weights());
            }
            finish_gen_group(self.store(), self.models_dir(), group)
                .await
                .unwrap();
        }

        /// Delete on installed model `id`, as `delete_local_model` does.
        async fn delete(&self, id: &str) -> AppResult<Vec<PathBuf>> {
            let writing = files_being_written(self.store(), None).await?;
            let busy = |p: &Path| writing.contains(&p.display().to_string());
            delete_files_of(self.store(), self.models_dir(), id, &busy).await
        }

        fn events(&self) -> Vec<ImportProgress> {
            self.events.lock().unwrap().clone()
        }

        /// The files left in `<models>/@imported`, each anywhere under it.
        fn imported_files(&self) -> Vec<PathBuf> {
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
            walk(&self.models_dir().join(IMPORTED_FOLDER), &mut out);
            out
        }
    }

    /// The saved part of `row` in `role`.
    fn part_in(row: &store::GenModel, role: &str) -> serde_json::Value {
        row.parts
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["role"] == role)
            .unwrap_or_else(|| panic!("no {role} part in {}", row.parts))
            .clone()
    }

    #[tokio::test]
    async fn a_copied_import_lands_in_the_imported_folder_and_registers_with_its_sha() {
        let mut t = harness().await;
        let src = t.outside_file("ü dir/my sd15.safetensors", fixture_bytes("sd15"));
        let r = t.import(&src, ImportMode::Copy, None).await.unwrap();
        assert_eq!(r.download_group, None);
        let row = t
            .store()
            .get_gen_model(r.gen_model_id.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap();
        let sha = sha256_file(&src).await;
        assert_eq!(row.id, format!("local:{}", &sha[..16]));
        assert_eq!(
            (row.recipe_id.as_deref(), row.source),
            (Some("sd1-checkpoint"), GenSource::Import)
        );
        // `<models>/@imported/<sha[..16]>/my sd15.safetensors`, built from its components so
        // its string is the stored one on Windows too.
        let copy = t
            .models_dir()
            .join(IMPORTED_FOLDER)
            .join(&sha[..16])
            .join("my sd15.safetensors");
        assert!(copy.exists());
        assert!(src.exists(), "copied, never moved");
        assert_eq!(
            t.imported_files(),
            std::slice::from_ref(&copy),
            "no partial file left"
        );
        // The main part is the person's own file, at its copy.
        let main = part_in(&row, "model");
        let size = std::fs::metadata(&src).unwrap().len();
        assert_eq!(
            main,
            json!({"role": "model", "repo": "", "revision": "", "path": "my sd15.safetensors",
                   "size": size, "sha256": sha, "license": "",
                   "local": copy.display().to_string(), "file": copy.display().to_string()})
        );
    }

    #[tokio::test]
    async fn an_in_place_import_keeps_the_file_where_it_is_and_never_deletes_it() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let id = t
            .import(&src, ImportMode::InPlace, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let row = t.store().get_gen_model(&id).await.unwrap().unwrap();
        assert_eq!(part_in(&row, "model")["file"], src.display().to_string());
        assert!(t.imported_files().is_empty(), "nothing copied");
        t.delete(&id).await.unwrap();
        assert!(src.exists());
    }

    #[tokio::test]
    async fn a_file_inside_the_models_folder_is_used_in_place_without_asking() {
        let mut t = harness().await;
        let f = t.models_file("@files/abc/m.safetensors", fixture_bytes("sd15"));
        assert!(t.inspect(&f).await.unwrap().inside_models);
        // Copy asked for, it is used where it is, and the choice is not the person's.
        let prepared = t.prepare(&f, ImportMode::Copy, None).await.unwrap();
        assert_eq!(prepared.chosen, None);
        let Added::Now(id) = prepared.added else {
            panic!("nothing to fetch");
        };
        let row = t.store().get_gen_model(&id).await.unwrap().unwrap();
        assert_eq!(part_in(&row, "model")["file"], f.display().to_string());
        assert!(t.imported_files().is_empty());
        // A file outside the folder is asked about, and the choice kept.
        let out = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        assert!(!t.inspect(&out).await.unwrap().inside_models);
        let prepared = t.prepare(&out, ImportMode::InPlace, None).await.unwrap();
        assert_eq!(prepared.chosen, Some(ImportMode::InPlace));
    }

    /// Ruling S7-local-delete: a file the person brought is theirs, though it lies inside the
    /// models folder.
    #[tokio::test]
    async fn deleting_an_import_used_in_place_inside_the_models_folder_keeps_its_file() {
        let mut t = harness().await;
        let f = t.models_file("my models/m.safetensors", fixture_bytes("sd15"));
        let id = t
            .import(&f, ImportMode::InPlace, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        assert!(t.delete(&id).await.unwrap().is_empty());
        assert!(f.exists());
        assert!(t.store().get_gen_model(&id).await.unwrap().is_none());
    }

    /// Ruling S7-local-delete: the copy an import made is Studio's, so it goes with the model
    /// once no other model uses it; the original stays.
    #[tokio::test]
    async fn deleting_a_copied_import_removes_studios_copy_and_keeps_the_original() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let id = t
            .import(&src, ImportMode::Copy, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let copy = t.imported_files().remove(0);
        // Another model uses the copy too (a slot the person filled with it).
        let shared = json!([{"role": "vae", "repo": "", "revision": "", "path": "m.safetensors",
                             "size": 1, "sha256": "", "license": "",
                             "local": copy.display().to_string(),
                             "file": copy.display().to_string()}]);
        t.store()
            .insert_gen_model(store::NewGenModel {
                id: "other".into(),
                catalog_id: None,
                name: "Other".into(),
                kind: "image".into(),
                family: "sd1".into(),
                recipe_id: Some("sd1-checkpoint".into()),
                parts: shared,
                size_bytes: 1,
                source: GenSource::Import,
            })
            .await
            .unwrap();
        assert!(t.delete(&id).await.unwrap().is_empty());
        assert!(copy.exists(), "another model uses it");
        assert_eq!(
            t.delete("other").await.unwrap(),
            std::slice::from_ref(&copy)
        );
        assert!(!copy.exists() && src.exists());
        assert!(
            !t.models_dir().join(IMPORTED_FOLDER).exists(),
            "its emptied folders go"
        );
        assert!(t.models_dir().exists());
    }

    /// A file another installed model uses (here Z-Image Turbo's own model file), picked to
    /// add: the person's own file used where it is, never taken for the other model's part,
    /// so deleting the import keeps it for that model.
    #[tokio::test]
    async fn a_file_another_model_uses_is_added_as_the_persons_own() {
        let mut t = harness().await;
        install_catalog_entry(t.store(), t.models_dir(), "z-image-turbo-q4").await;
        let theirs = t.models_file(
            "leejet/Z-Image-Turbo-GGUF/z_image_turbo-Q4_K.gguf",
            gguf_bytes("z-image-turbo"),
        );
        let id = t
            .import(&theirs, ImportMode::Copy, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let row = t.store().get_gen_model(&id).await.unwrap().unwrap();
        let main = part_in(&row, "diffusion");
        assert_eq!(
            (main["repo"].as_str(), main["local"].as_str()),
            (Some(""), Some(theirs.display().to_string().as_str()))
        );
        assert_eq!(main["sha256"], sha256_file(&theirs).await.as_str());
        t.delete(&id).await.unwrap();
        assert!(theirs.exists());
        assert!(
            t.store()
                .get_gen_model("z-image-turbo-q4")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn a_pickle_file_is_refused_with_the_extension_named() {
        let mut t = harness().await;
        let pickle = t.outside_file("m.ckpt", vec![0x80, 2]);
        let e = t.inspect(&pickle).await.unwrap_err();
        assert!(e.message.contains(".ckpt"), "{e:?}");
        assert_eq!(e.message, "Studio can't read .ckpt files");
        let e = t.import(&pickle, ImportMode::Copy, None).await.unwrap_err();
        assert!(e.message.contains(".ckpt"), "{e:?}");
        assert!(t.imported_files().is_empty());
    }

    /// A LoRA is not a model: the inspection says what it is (the webview adds it as a LoRA),
    /// and adding it as a model is refused with its own code.
    #[tokio::test]
    async fn a_lora_file_is_refused_so_the_webview_adds_it_as_a_lora() {
        let mut t = harness().await;
        let lora = t.outside_file(
            "pixel art.safetensors",
            fixture_bytes("sdxl-lora-pixel-art"),
        );
        let seen = t.inspect(&lora).await.unwrap();
        assert!(
            matches!(seen.label, FileLabel::Lora { .. }),
            "{:?}",
            seen.label
        );
        assert!(seen.plan.is_none());
        let e = t.import(&lora, ImportMode::Copy, None).await.unwrap_err();
        assert_eq!(e.code, "studio_import_is_lora");
        assert!(t.imported_files().is_empty());
        assert!(t.store().list_gen_models().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_import_that_needs_companions_starts_one_download_and_registers_on_completion() {
        let mut t = harness_with_hub().await;
        let src = t.outside_file("chroma.gguf", fixture_bytes("chroma"));
        let r = t.import(&src, ImportMode::InPlace, None).await.unwrap();
        assert!(r.gen_model_id.is_none());
        let group = r.download_group.unwrap();
        // The main file is here already: no row fetches it, and the group uses it where it is.
        let rows = t.rows(&group).await;
        assert!(!rows.is_empty());
        assert!(rows.iter().all(|row| !row.repo.is_empty()));
        let meta = t.meta(&group).await;
        let generator = meta.generator.unwrap();
        let main = generator
            .parts
            .iter()
            .find(|p| p.role == "diffusion")
            .unwrap();
        assert_eq!(
            main.present.as_deref(),
            Some(src.display().to_string().as_str())
        );
        assert_eq!(main.local.as_deref(), Some(src.as_path()));
        assert_eq!(generator.source, "import");
        t.complete(&group).await;
        let rows = t.store().list_gen_models().await.unwrap();
        assert_eq!(rows[0].recipe_id.as_deref(), Some("chroma"));
        assert_eq!(rows[0].source, GenSource::Import);
        let sha = sha256_file(&src).await;
        assert_eq!(rows[0].id, format!("local:{}", &sha[..16]));
        assert_eq!(
            part_in(&rows[0], "diffusion")["file"],
            src.display().to_string()
        );
    }

    #[tokio::test]
    async fn progress_is_sent_while_hashing_and_ends_complete() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        t.import(&src, ImportMode::Copy, None).await.unwrap();
        let last = t.events().last().cloned().unwrap();
        assert_eq!(last.done_bytes, last.total_bytes);
        assert_eq!(last.total_bytes, std::fs::metadata(&src).unwrap().len());
        assert_eq!(last.stage, ImportStage::Copying);
        assert_eq!(last.path, src.display().to_string());
        // Used where it is, the file is hashed as it is read.
        t.events.lock().unwrap().clear();
        let other = t.outside_file("n.safetensors", fixture_bytes("sdxl"));
        t.import(&other, ImportMode::InPlace, None).await.unwrap();
        let events = t.events();
        assert!(events.iter().all(|e| e.stage == ImportStage::Hashing));
        let last = events.last().unwrap();
        assert_eq!(last.done_bytes, last.total_bytes);
        assert_eq!(
            serde_json::to_value(last).unwrap(),
            json!({"path": other.display().to_string(), "stage": "hashing",
                   "doneBytes": last.done_bytes, "totalBytes": last.total_bytes})
        );
    }

    /// At most one event per 250 ms while a file is read; the last one always goes.
    #[test]
    fn progress_is_sent_at_most_every_250_ms() {
        let start = Instant::now();
        let mut throttle = Throttle::default();
        assert!(throttle.due(start));
        assert!(!throttle.due(start + Duration::from_millis(100)));
        assert!(!throttle.due(start + Duration::from_millis(249)));
        assert!(throttle.due(start + Duration::from_millis(250)));
        assert!(!throttle.due(start + Duration::from_millis(400)));
        assert!(throttle.due(start + Duration::from_millis(600)));
    }

    /// The same file copied again reuses the copy that has its SHA-256 (checked, not trusted);
    /// a damaged copy is replaced.
    #[tokio::test]
    async fn importing_the_same_file_again_reuses_its_copy() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let first = t.import(&src, ImportMode::Copy, None).await.unwrap();
        let copy = t.imported_files().remove(0);
        t.events.lock().unwrap().clear();
        let again = t.import(&src, ImportMode::Copy, None).await.unwrap();
        assert_eq!(again.gen_model_id, first.gen_model_id);
        assert_eq!(t.imported_files(), std::slice::from_ref(&copy));
        assert!(
            t.events().iter().any(|e| e.stage == ImportStage::Hashing),
            "the copy there is checked"
        );
        // Damaged at the same size: replaced by a good copy.
        let len = std::fs::metadata(&copy).unwrap().len();
        std::fs::write(&copy, vec![0u8; usize::try_from(len).unwrap()]).unwrap();
        t.import(&src, ImportMode::Copy, None).await.unwrap();
        assert_eq!(std::fs::read(&copy).unwrap(), std::fs::read(&src).unwrap());
        assert_eq!(t.imported_files(), [copy]);
        assert_eq!(t.store().list_gen_models().await.unwrap().len(), 1);
    }

    /// The webview's dialog: the file's label, its plan (its main file already here, from
    /// this computer, never fetched), and the mode to offer first.
    #[tokio::test]
    async fn the_inspection_shows_a_plan_whose_main_file_is_already_here() {
        let mut t = harness().await;
        let src = t.outside_file("my chroma.gguf", fixture_bytes("chroma"));
        let seen = t.inspect(&src).await.unwrap();
        let size = std::fs::metadata(&src).unwrap().len();
        assert_eq!(
            (seen.path.as_str(), seen.name.as_str(), seen.size_bytes),
            (src.display().to_string().as_str(), "my chroma.gguf", size)
        );
        assert!(matches!(&seen.label, FileLabel::Works { recipe, .. } if recipe == "chroma"));
        assert_eq!(seen.last_mode, ImportMode::Copy, "the default");
        assert!(!seen.inside_models);
        let plan = seen.plan.unwrap();
        assert_eq!(plan.recipe.id, "chroma");
        let main = plan.slots[0].part.as_ref().unwrap();
        assert_eq!(plan.slots[0].source, PartSource::Main);
        assert!(main.local && main.already_here && main.repo.is_empty());
        assert_eq!(main.path, src.display().to_string());
        assert_eq!(main.size_bytes, size);
        assert_eq!(plan.to_fetch_bytes, plan.total_bytes - size);
        assert!(plan.to_fetch_bytes > 0 && plan.can_download);
        // Nothing was hashed or sent.
        assert!(t.events().is_empty());
        // The last mode chosen is offered first.
        t.deps.last_import_mode = Some(ImportMode::InPlace);
        assert_eq!(
            t.inspect(&src).await.unwrap().last_mode,
            ImportMode::InPlace
        );
    }

    /// Chroma imported beside an installed Z-Image Turbo: Z-Image's VAE is used where it is,
    /// and the download fetches exactly what the plan shown says.
    #[tokio::test]
    async fn an_import_beside_an_installed_model_fetches_only_what_its_plan_says() {
        let mut t = harness_with_hub().await;
        install_catalog_entry(t.store(), t.models_dir(), "z-image-turbo-q4").await;
        let src = t.outside_file("chroma.gguf", fixture_bytes("chroma"));
        let plan = t.inspect(&src).await.unwrap().plan.unwrap();
        let vae = plan.slots.iter().find(|s| s.role == GenRole::Vae).unwrap();
        assert_eq!(vae.source, PartSource::Installed);
        let vae_part = vae.part.as_ref().unwrap();
        assert!(vae_part.already_here && !vae_part.local);
        let group = t
            .import(&src, ImportMode::InPlace, None)
            .await
            .unwrap()
            .download_group
            .unwrap();
        let rows = t.rows(&group).await;
        assert_eq!(
            rows.iter().map(|r| r.size_bytes).sum::<u64>(),
            plan.to_fetch_bytes
        );
        assert!(!rows.iter().any(|r| r.file.ends_with("ae.safetensors")));
    }

    #[tokio::test]
    async fn an_import_waits_for_the_note_about_local_models() {
        let mut t = harness().await;
        t.deps.notice_accepted = false;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let e = t.import(&src, ImportMode::Copy, None).await.unwrap_err();
        assert_eq!(e.code, "studio_notice_needed");
        assert!(t.imported_files().is_empty() && t.events().is_empty());
        // Reading it is no import: the dialog shows it before the note.
        assert!(t.inspect(&src).await.is_ok());
    }

    /// Everything that can refuse an import is checked before the file is read through or
    /// copied: a set-up missing a part, and a disk without room for the copy and what it
    /// fetches.
    #[tokio::test]
    async fn an_import_is_refused_before_anything_is_copied() {
        let mut t = harness().await;
        // A Hunyuan Video 1.5 file the table has no glyph encoder or decoder for.
        with_catalog(
            &mut t.deps,
            bundled_lacking(
                models::sd::detect::SdVersion::HunyuanVideo,
                &[
                    models::sd::recipes::SlotId::T5xxl,
                    models::sd::recipes::SlotId::Vae,
                ],
            ),
        );
        let bare = t.outside_file("h-Q4_0.gguf", gguf_bytes("hunyuan-video-15"));
        let e = t.import(&bare, ImportMode::Copy, None).await.unwrap_err();
        assert_eq!(e.code, "studio_setup_incomplete");
        assert!(e.details.contains("t5xxl"), "{}", e.details);
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let size = std::fs::metadata(&src).unwrap().len();
        t.deps.free_disk_bytes = Some(size - 1);
        let e = t.import(&src, ImportMode::Copy, None).await.unwrap_err();
        assert_eq!(e.code, "studio_disk_too_small");
        assert!(t.imported_files().is_empty() && t.events().is_empty());
        assert!(t.store().list_gen_models().await.unwrap().is_empty());
        // Used where it is, it takes no room.
        assert!(t.import(&src, ImportMode::InPlace, None).await.is_ok());
    }

    impl T {
        /// What *Repair* on installed model `id` prepares: its download, nothing started.
        async fn prepare_repair(&mut self, id: &str) -> AppResult<(GroupMeta, usize)> {
            self.refresh().await;
            let catalog = self.deps.catalog.gen_catalog().await;
            let deps = DownloadDeps {
                plan: plan_deps(&self.deps, &catalog),
                store: &self.deps.store,
                models_dir: &self.deps.models_dir,
                free_bytes: None,
                engine: &self.deps.engine_need,
            };
            let source = LocalSource::Installed {
                gen_model_id: id.into(),
            };
            let prepared = prepare_download(&deps, &source, None).await?;
            Ok((prepared.meta, prepared.files.len()))
        }

        /// *Repair* on installed model `id`, as `start_local_download` does, a download
        /// recorded but not run.
        async fn repair(&mut self, id: &str) -> AppResult<Added> {
            self.refresh().await;
            let catalog = self.deps.catalog.gen_catalog().await;
            let deps = DownloadDeps {
                plan: plan_deps(&self.deps, &catalog),
                store: &self.deps.store,
                models_dir: &self.deps.models_dir,
                free_bytes: None,
                engine: &self.deps.engine_need,
            };
            let source = LocalSource::Installed {
                gen_model_id: id.into(),
            };
            let prepared = prepare_download(&deps, &source, None).await?;
            add_now_if_present(&deps, prepared, &|_| false).await
        }
    }

    /// Final review, area 2, I1: *Repair* on a model added from a file, every part of it on
    /// this computer, has nothing to fetch. It is checked and added again at once, with no
    /// download group (which would never finish), and a file of the person's own that was
    /// moved says so, never "download it".
    #[tokio::test]
    async fn repair_on_an_in_place_import_checks_it_at_once() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let id = t
            .import(&src, ImportMode::InPlace, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let (_, fetched) = t.prepare_repair(&id).await.unwrap();
        assert_eq!(fetched, 0, "every part is the person's own");
        let Added::Now(again) = t.repair(&id).await.unwrap() else {
            panic!("nothing to fetch, so nothing to download");
        };
        assert_eq!(again, id);
        assert!(t.store().list_downloads(None).await.unwrap().is_empty());
        assert!(t.store().get_gen_model(&id).await.unwrap().is_some());

        // The person's file moved: said as such, and the model and the file stay.
        let moved = src.with_file_name("moved.safetensors");
        std::fs::rename(&src, &moved).unwrap();
        let Err(e) = t.repair(&id).await else {
            panic!("a moved file is refused");
        };
        assert_eq!(e.code, "studio_local_file_moved");
        assert!(!e.next_step.to_lowercase().contains("download"), "{e:?}");
        assert!(t.store().list_downloads(None).await.unwrap().is_empty());
        assert!(t.store().get_gen_model(&id).await.unwrap().is_some());
        assert!(moved.exists());
    }

    /// A file of the person's own that changed is named as moved or changed, whatever adds
    /// the model again (`register_present_gen_model`): an import's set-up, a set-up change, or
    /// *Repair*.
    #[tokio::test]
    async fn a_changed_file_of_the_persons_own_is_said_to_be_moved_or_changed() {
        let mut t = harness().await;
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let id = t
            .import(&src, ImportMode::InPlace, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let (meta, _) = t.prepare_repair(&id).await.unwrap();
        std::fs::write(&src, b"shorter now").unwrap();
        let e = register_present_gen_model(t.store(), t.models_dir(), &meta, &|_| false)
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_local_file_moved");
        assert_eq!(
            e.message,
            "A file of this model was moved or changed on this computer."
        );
    }

    /// Final review, area 2, M1: an import that fails after its copy was made removes
    /// Studio's copy when nothing uses it, and never a file that is not Studio's copy or that
    /// a model uses.
    #[tokio::test]
    async fn a_failure_after_the_copy_removes_studios_unused_copy() {
        let mut t = harness().await;
        let failed = || async { Err::<(), _>(errors::import_failed("a later step")) };
        let src = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        let copy = t.models_file(
            &format!("{IMPORTED_FOLDER}/0123456789abcdef/m.safetensors"),
            fixture_bytes("sd15"),
        );
        let e = removing_copy_on_error(t.store(), t.models_dir(), Some(&copy), failed())
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_import_failed");
        assert!(!copy.exists(), "the copy is removed");
        assert!(!copy.parent().unwrap().exists(), "and its empty folder");
        assert!(src.exists(), "never the person's file");

        // Success keeps it.
        let copy = t.models_file(
            &format!("{IMPORTED_FOLDER}/0123456789abcdef/m.safetensors"),
            fixture_bytes("sd15"),
        );
        removing_copy_on_error(t.store(), t.models_dir(), Some(&copy), async { Ok(()) })
            .await
            .unwrap();
        assert!(copy.exists());

        // A copy an installed model uses stays (the same file imported before).
        let id = t
            .import(&src, ImportMode::Copy, None)
            .await
            .unwrap()
            .gen_model_id
            .unwrap();
        let row = t.store().get_gen_model(&id).await.unwrap().unwrap();
        let used = PathBuf::from(part_in(&row, "model")["file"].as_str().unwrap());
        assert!(
            removing_copy_on_error(t.store(), t.models_dir(), Some(&used), failed())
                .await
                .is_err()
        );
        assert!(used.exists());

        // A file outside `@imported` is the person's own, even inside the models folder.
        let own = t.models_file("@files/abc/m.safetensors", fixture_bytes("sd15"));
        assert!(
            removing_copy_on_error(t.store(), t.models_dir(), Some(&own), failed())
                .await
                .is_err()
        );
        assert!(own.exists());

        // A file used where it is (no copy) is never touched.
        assert!(
            removing_copy_on_error(t.store(), t.models_dir(), None, failed())
                .await
                .is_err()
        );
    }

    /// Weights of no kind Studio knows: a safetensors file with one tensor no version has.
    fn mystery_bytes() -> Vec<u8> {
        models::sd::testing::safetensors(
            &json!({"mystery.block.0.weight": {"dtype": "F16", "shape": [1], "data_offsets": [0, 2]}}),
        )
    }

    /// Final review, area 3, I2: *Set it up yourself* from *Add a model file*. A file Studio
    /// can't place offers every model type; with one chosen it is planned and added as an
    /// untested set-up, as a searched file is; without one it is still refused.
    #[tokio::test]
    async fn an_unknown_file_from_this_computer_can_be_set_up_by_hand() {
        let mut t = harness().await;
        let src = t.outside_file("mystery.safetensors", mystery_bytes());
        let seen = t.inspect(&src).await.unwrap();
        assert!(matches!(
            seen.label,
            FileLabel::CannotRun {
                reason: models::sd::CannotRunReason::UnknownWeights,
                ..
            }
        ));
        assert!(seen.plan.is_none());
        assert_eq!(
            seen.by_hand_recipes.len(),
            models::sd::matching::in_scope_recipes().len()
        );
        // A file Studio runs has its plan, and no list for setting it up by hand.
        let known = t.outside_file("m.safetensors", fixture_bytes("sd15"));
        assert!(t.inspect(&known).await.unwrap().by_hand_recipes.is_empty());

        // With a model type: planned like a searched file set up by hand, untested.
        let choice = SetupChoice {
            recipe: Some("flux1-dev".into()),
            overrides: Vec::new(),
        };
        let source = crate::studio::local_models::LocalSource::Disk {
            path: src.display().to_string(),
        };
        t.refresh().await;
        let plan = crate::studio::setup_cmd::setup_plan(&t.deps, &source, &choice)
            .await
            .unwrap();
        assert!(plan.untested && plan.can_download, "{plan:?}");
        assert_eq!(plan.recipe.id, "flux1-dev");
        // Without one, it is refused as before, and nothing is copied.
        let e = t.import(&src, ImportMode::Copy, None).await.unwrap_err();
        assert_eq!(e.code, "studio_not_supported_yet");
        assert!(t.imported_files().is_empty());
        // Added with it: its encoders and decoder download, then it joins as untested.
        let r = t
            .import(&src, ImportMode::InPlace, Some(choice))
            .await
            .unwrap();
        let group = r.download_group.expect("its companions download");
        t.complete(&group).await;
        let rows = t.store().list_gen_models().await.unwrap();
        let row = rows.iter().find(|m| m.source == GenSource::Import).unwrap();
        assert_eq!(
            (row.recipe_id.as_deref(), row.family.as_str()),
            (Some("flux1-dev"), "unknown")
        );
        assert_eq!(part_in(row, "diffusion")["file"], src.display().to_string());
    }

    /// A file that is not the size it was when it was checked (still being written) is
    /// refused: its SHA-256 would not be the file's.
    #[test]
    fn a_file_that_changes_while_it_is_read_is_refused() {
        let scratch = Scratch::new();
        let file = scratch.path().join("m.safetensors");
        std::fs::write(&file, b"0123456789").unwrap();
        let mut report = Report::new("m.safetensors".into(), Arc::new(|_| {}));
        for size in [9, 11] {
            let e = read_through(&file, size, None, ImportStage::Hashing, &mut report).unwrap_err();
            assert_eq!(e.code, "studio_import_changed");
        }
        assert_eq!(
            read_through(&file, 10, None, ImportStage::Hashing, &mut report).unwrap(),
            "84d89877f0d4041efb6bf91a16f0248f2fd573e6af05c19f96bedb9f882f7882"
        );
    }

    /// The webview's words for the mode and the result.
    #[test]
    fn the_mode_and_the_result_read_as_the_webview_has_them() {
        assert_eq!(
            serde_json::to_value(ImportMode::InPlace).unwrap(),
            json!("inPlace")
        );
        assert_eq!(
            serde_json::from_value::<ImportMode>(json!("copy")).unwrap(),
            ImportMode::Copy
        );
        assert_eq!(
            serde_json::to_value(ImportResultDto {
                gen_model_id: None,
                download_group: Some("g".into()),
            })
            .unwrap(),
            json!({"genModelId": null, "downloadGroup": "g"})
        );
    }
}
