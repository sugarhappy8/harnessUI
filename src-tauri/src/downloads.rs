//! Model downloads (ARCHITECTURE.md 11): one group per model (its shards and the
//! projector), persisted in the `downloads` table so an interrupted download resumes
//! after a restart. Pause, resume and cancel act on a whole group.
//!
//! 0.1.0 Studio: a picture or video model is one group too ([`GenGroup`]): its parts come
//! from several repositories, each at its own revision ([`row_commit`]), and once every part
//! is checked it joins Studio's installed models instead of the chat list
//! ([`register_gen_model`]).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use models::download::{FileSpec, Progress, download_file_checked, resumable_bytes};
use models::gen_catalog::{GateMode, GenEntry, GenFamily, GenGate, GenPart, GenRole, GenericSpec};
use models::sd::detect::SdVersion;
use models::sd::inspect::{FileKind, inspect_local, inspect_local_folder};
use models::sd::matching::{Match, match_recipe, recipes_for};
use serde::{Deserialize, Serialize};
use store::{DownloadPatch, DownloadStatus, ModelSource, NewDownload};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::studio::errors as studio_errors;

/// 0.1.0 Studio: the picture or video model a group downloads (its saved metadata's
/// [`GroupMeta::generator`]), as its installed row will be.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenGroup {
    /// The installed model's id: the catalog id, `hf:<owner>/<repo>:<file>`, or (0.2.0) for a
    /// file imported from this computer `local:<its SHA-256, 16 characters>`.
    pub gen_model_id: String,
    /// The signed catalog's entry; `None` for a searched model.
    pub catalog_id: Option<String>,
    pub name: String,
    /// `image`, `image-edit` or `video`.
    pub kind: String,
    /// Its family's key in the known-good family table (`flux2-klein-4b`).
    pub family: String,
    /// `catalog`, `search` or (0.2.0) `import`.
    pub source: String,
    /// Every part, in the entry's order.
    pub parts: Vec<GenGroupPart>,
    /// 0.2.0: the engine recipe it runs with (a searched file's from its set-up plan, else
    /// its entry's). Absent in a group saved before, whose family gives it.
    #[serde(default)]
    pub recipe_id: Option<String>,
    /// 0.2.0: its set-up is untested: the recipe is not among those of the version its file
    /// was detected as (or no version was), so the downloaded file is checked only for being
    /// weights that read ([`check_main_file`]).
    #[serde(default)]
    pub untested: bool,
}

/// One part of a [`GenGroup`], pinned as its catalog entry or search result gave it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenGroupPart {
    /// Its role's key (`diffusion`, `llm`, `vae`…).
    pub role: String,
    pub repo: String,
    /// The 40-hex commit it is downloaded at.
    pub revision: String,
    /// Its path inside the repository.
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Its licence's id.
    pub license: String,
    /// 0.2.0: a diffusers folder's part: its engine name in the folder, under which it is
    /// saved (`unet/diffusion_pytorch_model.safetensors`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    /// 0.2.0: a file from this computer, used where it is (its repository and revision are
    /// empty); always [`present`](Self::present) there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
    /// 0.2.0: the file already on this computer the part uses (another installed model's, or
    /// a file from this computer: an imported file's main part): it has no row, nothing
    /// fetches or checks it, and the model's row records it as the part's file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub present: Option<String>,
}

/// What a group is for, kept beside its rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMeta {
    pub repo: String,
    pub commit: String,
    pub label: String,
    /// Model files in order (the first is what llama-server opens).
    pub model_files: Vec<String>,
    pub projector: Option<String>,
    pub catalog_id: Option<String>,
    /// 0.0.6: what the uploader listed when the download started, saved with the model
    /// for comparing it offline (absent in groups saved before; dropped on its own when
    /// this version cannot read it).
    #[serde(
        default,
        deserialize_with = "crate::hub::lenient_hub",
        skip_serializing_if = "Option::is_none"
    )]
    pub hub: Option<crate::dto::HubFacts>,
    /// 0.0.6: the parameter count Hugging Face gave for the repository.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<u64>,
    /// 0.0.7 (P2): this group is only the image file (`projector`) of the installed model
    /// at this path ("Download the image file" in Models → Installed; `model_files` is
    /// empty). When it is done it is recorded on that model's row instead of adding a
    /// model. Absent in every other group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_file_for: Option<String>,
    /// 0.0.7 (P2 review M2): the image files of the model's repository when the download
    /// started (Browse's listing, or the catalog's image file), saved with the model so
    /// Installed can offer one later without asking Hugging Face. Absent in groups saved
    /// before, and in a group that is only an image file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_files: Option<Vec<models::RepoFile>>,
    /// 0.1.0 Studio: the picture or video model this group downloads (`model_files` is
    /// empty, and `repo` and `commit` are its diffusion part's). When it is done it joins
    /// Studio's installed models ([`register_gen_model`]). Absent in every other group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<GenGroup>,
    /// Save a verified Hub file without registering an installed chat or Studio model.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub file_only: bool,
}

/// The commit `file` of `repo` is downloaded at. The `downloads` table has no commit column:
/// a picture or video model's part downloads at its own revision (its parts come from
/// several repositories), any other row at the group's.
pub(crate) fn row_commit(meta: &GroupMeta, repo: &str, file: &str) -> String {
    meta.generator
        .as_ref()
        .and_then(|g| g.parts.iter().find(|p| p.repo == repo && p.path == file))
        .map_or_else(|| meta.commit.clone(), |p| p.revision.clone())
}

/// The installed row a picture or video model's group makes once every part completed with
/// its pinned SHA-256 (its row checked, `Verifying`, or done), each part with its own file on
/// disk. A row is its part's when it is of the part's repository and path: two repositories
/// may hold files of the same name, and each part's file is the row's own destination. A part
/// already on this computer ([`GenGroupPart::present`]) has no row: its file is that one,
/// still there with its size. A part missing, unfinished or unchecked is
/// `studio_model_incomplete` (Resume finishes it).
pub(crate) fn gen_model_from(
    meta: &GroupMeta,
    rows: &[store::Download],
) -> AppResult<store::NewGenModel> {
    let Some(generator) = &meta.generator else {
        return Err(AppError::internal(
            "a download without a picture or video model",
        ));
    };
    let mut parts = Vec::with_capacity(generator.parts.len());
    for part in &generator.parts {
        let file = match &part.present {
            Some(present) => {
                if !file_whole(Path::new(present), part.size) {
                    return Err(studio_errors::model_incomplete());
                }
                present.clone()
            }
            None => row_of(rows, part)
                .filter(|r| {
                    matches!(
                        r.status,
                        DownloadStatus::Verifying | DownloadStatus::Completed
                    )
                })
                .filter(|r| {
                    !r.sha256.is_empty()
                        && r.sha256.eq_ignore_ascii_case(&part.sha256)
                        && r.size_bytes == part.size
                })
                .ok_or_else(studio_errors::model_incomplete)?
                .dest_path
                .clone(),
        };
        let mut saved = serde_json::to_value(part).map_err(AppError::internal)?;
        if let Some(saved) = saved.as_object_mut() {
            saved.remove("present");
        }
        saved["file"] = file.into();
        parts.push(saved);
    }
    let source = generator
        .source
        .parse::<store::GenSource>()
        .map_err(AppError::internal)?;
    // 0.2.0: the engine recipe it runs with. A group saved before has none: its family's. A
    // family this build does not know has none; `register_gen_model` refuses it.
    let recipe_id = generator.recipe_id.clone().or_else(|| {
        serde_json::from_value::<GenFamily>(generator.family.clone().into())
            .ok()
            .map(|f| f.recipe_id().to_owned())
    });
    Ok(store::NewGenModel {
        id: generator.gen_model_id.clone(),
        catalog_id: generator.catalog_id.clone(),
        name: generator.name.clone(),
        kind: generator.kind.clone(),
        family: generator.family.clone(),
        recipe_id,
        parts: serde_json::Value::Array(parts),
        size_bytes: generator.parts.iter().map(|p| p.size).sum(),
        source,
    })
}

/// The row of `part` among a group's `rows`: the one of its repository and path.
fn row_of<'a>(rows: &'a [store::Download], part: &GenGroupPart) -> Option<&'a store::Download> {
    rows.iter()
        .find(|r| r.repo == part.repo && r.file == part.path)
}

/// Whether the model file at `path` (a download's or an installed model's) is a model `entry`
/// runs, by the engine's own detection on its header (`sd::inspect`: most such files name no
/// architecture), so a file whose name lies is never added or run:
/// - the file is a model of a version that takes the entry's recipe
///   (`sd::matching::recipes_for`): an SDXL Vega or SSD-1B checkpoint takes
///   `sdxl-checkpoint`;
/// - for a 0.1.x family's entry, the recipe the file's own facts pick is the entry's, as
///   0.1.x's tensor signatures told sizes apart: a FLUX.2 klein 9B named like the 4B is
///   refused. The name the matching reads is the entry's recipe, never the file's: the
///   family decided the recipe (schema 4 maps it by family alone), so only the file's facts
///   can disagree, and a file whose repository and name never say `turbo` (found by its
///   `base_model` tag) is still Z-Image Turbo;
/// - an untested set-up takes any weights that read;
/// - a diffusers folder's entry (parts with a `folder` name) is checked by its folder.
///
/// Otherwise `studio_not_that_model`.
pub(crate) fn check_main_file(entry: &GenEntry, path: &Path) -> AppResult<()> {
    let main = entry
        .parts
        .iter()
        .find(|p| matches!(p.role, GenRole::Diffusion | GenRole::Model));
    let read = if entry.parts.iter().any(|p| p.folder.is_some()) {
        inspect_local_folder(&folder_root(path, main.and_then(|p| p.folder.as_deref()))?)
    } else {
        inspect_local(path)
    };
    let facts = read.map_err(|e| studio_errors::not_that_model(format!("its header: {e}")))?;
    let refused = |is: String| {
        studio_errors::not_that_model(format!("expected a model for {}, {is}", entry.recipe))
    };
    if entry.untested {
        return match facts.kind {
            FileKind::CannotRun {
                reason: models::sd::CannotRunReason::Pickle,
                ..
            } => Err(refused("the file is a pickle file".into())),
            _ => Ok(()),
        };
    }
    let (version, embeds) = match &facts.kind {
        FileKind::Model {
            version, embeds, ..
        } => (*version, embeds),
        FileKind::Lora { .. } => return Err(refused("the file is a LoRA".into())),
        FileKind::Extra { slot } => {
            return Err(refused(format!("the file is a {} part", slot.word())));
        }
        FileKind::CannotRun { reason, .. } => {
            return Err(refused(format!("Studio cannot run the file ({reason:?})")));
        }
    };
    if !recipes_for(version).contains(&entry.recipe.as_str()) {
        return Err(refused(format!("the file is a {} model", version.word())));
    }
    if entry.family.is_some() {
        match match_recipe(version, &facts.facts, embeds, &entry.recipe) {
            Match::Recipe { id, .. } if id == entry.recipe => {}
            Match::Recipe { id, .. } => {
                return Err(refused(format!("the file is one for {id}")));
            }
            Match::Refused(reason) => {
                return Err(refused(format!("Studio cannot run the file ({reason:?})")));
            }
        }
    }
    Ok(())
}

/// The root of the diffusers folder whose file `inside` (`unet/…`, the main part's engine
/// name in it) is at `path`; `path` itself when it is the folder. A file that is not at that
/// name (saved under its name in the repository, `…fp16.safetensors`) is in no folder the
/// engine reads: an internal error, never a guess at a parent folder.
fn folder_root(path: &Path, inside: Option<&str>) -> AppResult<PathBuf> {
    if path.is_dir() {
        return Ok(path.to_path_buf());
    }
    let inside = inside.map(Path::new).unwrap_or(Path::new(""));
    let depth = inside.components().count();
    match path.ancestors().nth(depth) {
        Some(root) if depth > 0 && path.ends_with(inside) => Ok(root.to_path_buf()),
        _ => Err(AppError::internal(format!(
            "a folder model's file is not at {} in its folder",
            inside.display()
        ))),
    }
}

/// The entry the finished download `new` is checked as ([`check_main_file`]): its parts, with
/// its recipe: a 0.1.x family's (as 0.1.2 checked it), else the recipe the group recorded,
/// with the version its family column names and whether its set-up is `untested`. A model
/// with no recipe (a family this build does not know, in a group saved before) is
/// `studio_not_supported_yet`.
fn downloaded_entry(new: &store::NewGenModel, untested: bool) -> AppResult<GenEntry> {
    let family = serde_json::from_value::<GenFamily>(new.family.clone().into()).ok();
    let recipe_id = match (family, new.recipe_id.as_deref()) {
        (Some(family), _) => family.recipe_id(),
        (None, Some(recipe_id)) => recipe_id,
        (None, None) => {
            return Err(studio_errors::not_supported_yet(
                studio_errors::NO_RECIPE_YET,
            ));
        }
    };
    let recipe = models::sd::recipes::recipe(recipe_id)
        .ok_or_else(|| studio_errors::not_supported_yet(format!("no engine recipe {recipe_id}")))?;
    let parts: Vec<GenPart> = serde_json::from_value(new.parts.clone())
        .map_err(|e| AppError::internal(format!("a downloaded model's parts: {e}")))?;
    let mut entry = models::gen_catalog::generic_entry(GenericSpec {
        id: new.id.clone(),
        name: new.name.clone(),
        publisher: String::new(),
        recipe,
        version: match family {
            Some(family) => Some(family.version()),
            None => SdVersion::from_word(&new.family),
        },
        parts,
        licenses: Default::default(),
        gate: GenGate {
            mode: GateMode::None,
            collects: Vec::new(),
        },
        untested: family.is_none() && untested,
    });
    entry.family = family;
    Ok(entry)
}

/// 0.1.0 Studio: adds the picture or video model `group` downloaded ([`GroupMeta::generator`])
/// to Studio's installed models: every part checked ([`gen_model_from`]), then the model
/// part's header ([`check_main_file`], read off the async runtime). A diffusion file
/// of another model is refused: nothing is added, and the diffusion file and the files the
/// group wrote itself ([`written_by`], as Cancel deletes only those) are removed when no
/// installed model uses them and no other download writes them, with their emptied folders.
/// A part the download found already in place stays.
pub(crate) async fn register_gen_model(
    store: &store::Store,
    models_dir: &Path,
    group: &str,
    meta: &GroupMeta,
) -> AppResult<()> {
    let rows = store.list_downloads(Some(group.to_string())).await?;
    let new = gen_model_from(meta, &rows)?;
    let main = main_part(meta)?;
    // The model file: the row's, or the file already here (a file from this computer).
    let (path, fetched) = match &main.present {
        Some(present) => (PathBuf::from(present), None),
        None => {
            let row = row_of(&rows, main).ok_or_else(|| {
                AppError::internal("a picture or video model without its model part")
            })?;
            (PathBuf::from(&row.dest_path), Some(row.dest_path.clone()))
        }
    };
    if let Err(refused) = check_added(meta, &new, path).await? {
        let writing = files_being_written(store, Some(group)).await?;
        let written = written_by(store, group).await;
        // Only the group's own rows: a file already here is never removed.
        let files: Vec<PathBuf> = rows
            .iter()
            .filter(|r| fetched.as_ref() == Some(&r.dest_path) || written.contains(&r.dest_path))
            .map(|r| PathBuf::from(&r.dest_path))
            .collect();
        let busy = |p: &Path| writing.contains(&p.display().to_string());
        crate::studio::local_models::remove_unused_files(store, models_dir, &files, &busy).await?;
        tracing::warn!(
            model = %new.id,
            "a downloaded model file is another model: {}",
            refused.details
        );
        return Err(refused);
    }
    let id = new.id.clone();
    store.insert_gen_model(new).await?;
    tracing::info!(model = %id, "picture or video model downloaded");
    Ok(())
}

/// 0.2.0 (Task S8): adds the picture or video model `meta` describes when every part of it is
/// already on this computer ([`GenGroupPart::present`]: an imported file whose set-up fetches
/// nothing), with no group or rows: each present file still whole ([`gen_model_from`]), then
/// the model file's header ([`check_main_file`]). A refusal adds nothing and removes nothing.
/// Returns the model's id.
pub(crate) async fn register_present_gen_model(
    store: &store::Store,
    meta: &GroupMeta,
) -> AppResult<String> {
    let new = gen_model_from(meta, &[])?;
    let path = main_part(meta)?
        .present
        .as_ref()
        .map(PathBuf::from)
        .ok_or_else(|| AppError::internal("a model added at once whose file is not here"))?;
    check_added(meta, &new, path).await??;
    let id = new.id.clone();
    store.insert_gen_model(new).await?;
    tracing::info!(model = %id, "picture or video model added");
    Ok(id)
}

/// The model part of the picture or video model `meta` downloads.
fn main_part(meta: &GroupMeta) -> AppResult<&GenGroupPart> {
    meta.generator
        .iter()
        .flat_map(|g| &g.parts)
        .find(|p| matches!(p.role.as_str(), "diffusion" | "model"))
        .ok_or_else(|| AppError::internal("a picture or video model without its model part"))
}

/// Whether the model file at `path` of `new` (the row `meta`'s group makes) is a model its
/// recipe runs ([`check_main_file`], read off the async runtime): the outer error is the
/// check's own failure, the inner one its refusal.
async fn check_added(
    meta: &GroupMeta,
    new: &store::NewGenModel,
    path: PathBuf,
) -> AppResult<AppResult<()>> {
    let untested = meta.generator.as_ref().is_some_and(|g| g.untested);
    let entry = downloaded_entry(new, untested)?;
    tokio::task::spawn_blocking(move || check_main_file(&entry, &path))
        .await
        .map_err(|e| AppError::internal(format!("the model file's check stopped: {e}")))
}

/// What a picture or video model's group's task does once every file is fetched and checked
/// (`DownloadManager::run_group`), for tests that put the files in place themselves: each row
/// checked (`Verifying`) and its file noted as one the group wrote, the model added
/// ([`register_gen_model`]), then every row done and the note dropped.
#[cfg(test)]
pub(crate) async fn finish_gen_group(
    store: &store::Store,
    models_dir: &Path,
    group: &str,
) -> AppResult<()> {
    let meta: GroupMeta = store
        .get_setting(&meta_key(group))
        .await?
        .ok_or_else(|| AppError::internal("download group without metadata"))?;
    let rows = store.list_downloads(Some(group.to_string())).await?;
    for row in &rows {
        note_written(store, group, &row.dest_path).await?;
        store
            .update_download(
                row.id,
                DownloadPatch {
                    status: Some(DownloadStatus::Verifying),
                    bytes_done: Some(row.size_bytes),
                    ..Default::default()
                },
            )
            .await?;
    }
    register_gen_model(store, models_dir, group, &meta).await?;
    for row in &rows {
        store
            .update_download(
                row.id,
                DownloadPatch {
                    status: Some(DownloadStatus::Completed),
                    ..Default::default()
                },
            )
            .await?;
    }
    let _ = store.delete_setting(&written_key(group)).await;
    Ok(())
}

/// The files a download not finished yet writes (a paused, failed or cancelled one too: it
/// can go on), other than group `except`'s, and the files such a picture or video model's
/// download found already here and will use ([`GenGroupPart::present`]): nothing deletes
/// them meanwhile. A group's metadata that cannot be read is an error, never taken for a
/// group that uses nothing: what this returns guards deleting.
pub(crate) async fn files_being_written(
    store: &store::Store,
    except: Option<&str>,
) -> AppResult<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    let mut groups = BTreeSet::new();
    for row in store.list_downloads(None).await? {
        if row.status != DownloadStatus::Completed && Some(row.group_id.as_str()) != except {
            files.insert(row.dest_path);
            groups.insert(row.group_id);
        }
    }
    for group in groups {
        let meta: Option<GroupMeta> = store.get_setting(&meta_key(&group)).await?;
        let present = meta
            .into_iter()
            .flat_map(|m| m.generator)
            .flat_map(|g| g.parts)
            .filter_map(|p| p.present);
        files.extend(present);
    }
    Ok(files)
}

fn meta_key(group: &str) -> String {
    format!("downloadGroup:{group}")
}

/// 0.0.7 (P6b round 2): the settings key saved while a group is being cancelled, from before
/// its task is stopped until its files are gone. Meanwhile it shows as "cancelling", nothing
/// follows it or starts beside it, it cannot be resumed, and if the app stops first the
/// cancel is finished at the next start.
fn cancelling_key(group: &str) -> String {
    format!("downloadCancelling:{group}")
}

/// Whether `group` is being cancelled ([`cancelling_key`]).
async fn is_cancelling(store: &store::Store, group: &str) -> bool {
    store
        .get_setting::<bool>(&cancelling_key(group))
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// What the UI shows for a group.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadGroupDto {
    pub id: String,
    pub repo: String,
    pub label: String,
    pub file_count: usize,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub status: String,
    pub error: Option<AppError>,
    pub bytes_per_second: Option<f64>,
    pub unverified: bool,
    /// 0.0.7 (P6b review m5): a file is being checked (its SHA-256 worked out), which takes
    /// minutes for a large one; the row says so instead of "downloading".
    pub checking: bool,
    /// 0.1.0 Studio: what the picture or video model this group downloads makes (`image`,
    /// `image-edit` or `video`), so Downloads labels it; `None` for a chat model's group.
    pub generator_kind: Option<String>,
    /// 0.1.0 Studio: the installed id of the picture or video model this group downloads
    /// ([`GenGroup::gen_model_id`]), so Get local models shows its progress on that model's
    /// row; `None` for a chat model's group.
    pub gen_model_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub saved_file: Option<SavedFileDto>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SavedFileDto {
    pub commit: String,
    pub path: String,
}

#[derive(Default)]
struct Live {
    /// Bytes on disk per file path, while running.
    done: HashMap<String, u64>,
    speed: Option<f64>,
    last_emit: Option<Instant>,
    last_persist: Option<Instant>,
    window: Option<(Instant, u64)>,
    /// A file is being checked now ([`Progress::Checking`]).
    checking: bool,
}

impl Live {
    /// Notes progress on `file` at `now`. Returns whether to tell the window, and whether to
    /// save the bytes on the file's row.
    fn note(&mut self, file: &str, p: Progress, now: Instant) -> (bool, bool) {
        let n = match p {
            Progress::Bytes(n) => {
                self.checking = false;
                n
            }
            // A file found in place counts as done as far as it is checked; the check of a
            // file that just finished downloading keeps its bytes, so the bar never goes back.
            Progress::Checking(n) => {
                self.checking = true;
                let done = self.done.get(file).copied().unwrap_or(0).max(n);
                self.done.insert(file.to_string(), done);
                let emit = self
                    .last_emit
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(300));
                if emit {
                    self.last_emit = Some(now);
                }
                return (emit, false);
            }
        };
        self.done.insert(file.to_string(), n);
        let total: u64 = self.done.values().sum();
        match self.window {
            Some((t0, b0)) if now.duration_since(t0) >= Duration::from_secs(2) => {
                let secs = now.duration_since(t0).as_secs_f64();
                self.speed = Some(total.saturating_sub(b0) as f64 / secs);
                self.window = Some((now, total));
            }
            None => self.window = Some((now, total)),
            _ => {}
        }
        let emit = self
            .last_emit
            .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(300));
        if emit {
            self.last_emit = Some(now);
        }
        let persist = self
            .last_persist
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(3));
        if persist {
            self.last_persist = Some(now);
        }
        (emit, persist)
    }
}

/// What a finished download saves with the model besides its file (0.0.6 item 4): the
/// catalog id, the file's path in its repository and what the uploader listed.
fn saved_facts(meta: &GroupMeta) -> crate::hub::SavedFacts {
    crate::hub::SavedFacts {
        catalog_id: meta.catalog_id.clone(),
        repo_file: meta.model_files.first().cloned(),
        hub: meta.hub.clone(),
        parameters: meta.parameters,
        image_files: meta.image_files.clone(),
        projector_sha256: None,
    }
}

/// A group's task while it runs.
struct Running {
    /// Cancelled by Pause and by Cancel.
    token: CancellationToken,
    /// Pause cancelled `token` (else Cancel did).
    paused: bool,
    /// 0.0.7 (P6b review m1): cancelled once the task has ended. Cancel waits for it before it
    /// deletes anything, so no file is written, renamed or added after Cancel looked.
    ended: CancellationToken,
    /// Which task this is ([`NEXT_TASK`]), so the end of one never takes off a newer task of
    /// the same group ([`TaskEnd`]).
    id: u64,
}

/// The id of the next group task.
static NEXT_TASK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// 0.0.7 (P6b round 2): ends a group's task however it ends, a panic included: its entry
/// leaves `running` (only its own: a newer task of the group keeps its entry), its live
/// progress goes, and `ended` fires, so Cancel never waits for a task that is gone.
struct TaskEnd {
    manager: Arc<DownloadManager>,
    group: String,
    id: u64,
    ended: CancellationToken,
    finished: bool,
}

impl TaskEnd {
    fn new(
        manager: Arc<DownloadManager>,
        group: String,
        id: u64,
        ended: CancellationToken,
    ) -> Self {
        Self {
            manager,
            group,
            id,
            ended,
            finished: false,
        }
    }

    /// Takes the task off `running` and drops its live progress, once. Returns whether Pause
    /// stopped it.
    fn finish(&mut self) -> bool {
        if std::mem::replace(&mut self.finished, true) {
            return false;
        }
        let mine = self.manager.running.lock().ok().and_then(|mut r| {
            if r.get(&self.group).is_some_and(|e| e.id == self.id) {
                r.remove(&self.group)
            } else {
                None
            }
        });
        let Some(mine) = mine else {
            return false;
        };
        if let Ok(mut l) = self.manager.live.lock() {
            l.remove(&self.group);
        }
        mine.paused
    }
}

impl Drop for TaskEnd {
    fn drop(&mut self) {
        self.finish();
        self.ended.cancel();
    }
}

/// How long Cancel waits for a group's task to end. Every step stops at once on cancel
/// (checking a file too) except adding the finished model, which reads only its header.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct DownloadManager {
    running: StdMutex<HashMap<String, Running>>,
    live: StdMutex<HashMap<String, Live>>,
    /// 0.0.7 (P6): held while a new download is checked against the others and recorded,
    /// so two starts at once (a double click) cannot both find nothing.
    starting: tokio::sync::Mutex<()>,
}

/// The unsigned resolve URL stored with each row (the signed CDN URL is never stored).
fn resolve_url(repo: &str, commit: &str, file: &str) -> String {
    format!("https://huggingface.co/{repo}/resolve/{commit}/{file}")
}

fn status_name(s: DownloadStatus) -> &'static str {
    match s {
        DownloadStatus::Queued => "queued",
        DownloadStatus::Downloading => "downloading",
        DownloadStatus::Paused => "paused",
        DownloadStatus::Verifying => "verifying",
        DownloadStatus::Completed => "done",
        DownloadStatus::Failed => "error",
        DownloadStatus::Cancelled => "cancelled",
    }
}

fn download_error_to_app(e: &models::download::DownloadError) -> AppError {
    use models::download::DownloadError as D;
    use models::hf::HfError as H;
    match e {
        D::Hub(H::NeedsToken) => AppError::new(
            "hf_needs_token",
            "This model is gated.",
            "Add a Hugging Face token in Settings → Models, then resume the download.",
            e.to_string(),
        ),
        D::Hub(H::NeedsTerms) => AppError::new(
            "hf_needs_terms",
            "You have not accepted this model's terms.",
            "Open the model page on Hugging Face, accept its terms, then resume the download.",
            e.to_string(),
        ),
        D::Hub(H::BlockedHost(h)) => AppError::new(
            "blocked_host",
            format!("The download was redirected to a host the app does not trust ({h})."),
            "Report this; Hugging Face may have changed its download servers.",
            e.to_string(),
        ),
        D::Hub(H::RateLimited(s)) => AppError::new(
            "hf_rate_limited",
            "Hugging Face is limiting requests from this network.",
            format!("Wait about {s} seconds, then resume."),
            e.to_string(),
        ),
        D::Hub(H::Network(_)) => AppError::new(
            "network",
            "The download was interrupted.",
            "Check your connection, then resume; the download continues where it stopped.",
            e.to_string(),
        ),
        D::ChecksumMismatch => AppError::new(
            "checksum_mismatch",
            "The downloaded file does not match its published checksum, so it was deleted.",
            "Try the download again. If it keeps failing, the file on Hugging Face may be damaged.",
            e.to_string(),
        ),
        D::ChangedUpstream => AppError::new(
            "changed_upstream",
            "The file changed on Hugging Face after it was listed.",
            "Open the model again and start a new download.",
            e.to_string(),
        ),
        D::DiskFull => AppError::new(
            "disk_full",
            "The disk is full.",
            "Free some space or choose another models folder, then resume.",
            e.to_string(),
        ),
        _ => AppError::new(
            "download_failed",
            "The download failed.",
            "Resume it, or cancel and try again.",
            e.to_string(),
        ),
    }
}

// ---- 0.0.7 (P6): what a group's rows mean -------------------------------------------

/// Whether the file at `dest` is whole: `size` bytes at its final name. A download gets its
/// final name only once it finished and was checked (`download_file` renames it then).
fn file_whole(dest: &Path, size: u64) -> bool {
    std::fs::metadata(dest).is_ok_and(|m| m.is_file() && m.len() == size)
}

/// The code of [`last_step_error`]: the group's files were all finished and checked when
/// its last step failed.
const LAST_STEP_FAILED: &str = "download_not_added";

/// Whether a failed row failed at the group's last step, after its file was finished and
/// checked ([`mark_failed`] with [`last_step_error`]), not while it was being downloaded.
fn failed_at_last_step(row: &store::Download) -> bool {
    row.status == DownloadStatus::Failed
        && row
            .error
            .as_deref()
            .and_then(|e| serde_json::from_str::<AppError>(e).ok())
            .is_some_and(|e| e.code == LAST_STEP_FAILED)
}

/// Whether a row's file needs no more downloading: the group is done with it, or the file
/// is whole and only the group's last step (adding the model) is left, or failed. 0.0.7
/// (P6b review M1): a row that failed while downloading never counts, whatever is at its
/// final name (an older or damaged file of the same size the fetch never replaced): it is
/// downloaded again, so checked, and not fetched again when it is the same file.
fn file_done(row: &store::Download, whole: bool) -> bool {
    match row.status {
        DownloadStatus::Completed => true,
        DownloadStatus::Verifying => whole,
        DownloadStatus::Failed => whole && failed_at_last_step(row),
        _ => false,
    }
}

/// What Resume makes of a row: `None` leaves it (the group is done with it); a whole file
/// waits for the last step again (`Verifying`), so it is never downloaded again; any other
/// file is downloaded (`Queued`), going on from its partial file.
fn status_on_resume(row: &store::Download, whole: bool) -> Option<DownloadStatus> {
    if row.status == DownloadStatus::Completed {
        None
    } else if file_done(row, whole) {
        Some(DownloadStatus::Verifying)
    } else {
        Some(DownloadStatus::Queued)
    }
}

/// The status the UI shows for a group, from its rows and whether its task runs. A finished
/// file (`Verifying`) is progress only while the task runs: a group paused after its first
/// file is paused, and one that stopped for any other reason can be resumed. A cancelled
/// row means "cancelled" only while Cancel stops the task (P6b review m2): left over by a
/// cancel that did not finish, the group shows as paused, so Cancel and Resume are there.
fn group_status(rows: &[DownloadStatus], running: bool) -> DownloadStatus {
    use DownloadStatus as S;
    if rows.iter().all(|s| *s == S::Completed) {
        S::Completed
    } else if rows.contains(&S::Failed) {
        S::Failed
    } else if running && rows.contains(&S::Cancelled) {
        S::Cancelled
    } else if rows.contains(&S::Paused) || !running {
        S::Paused
    } else if rows
        .iter()
        .any(|s| matches!(s, S::Downloading | S::Verifying))
    {
        S::Downloading
    } else {
        S::Queued
    }
}

/// Whether a group was going when the app last stopped, so it goes on by itself at the next
/// start: a file to download or the last step left, and nothing paused, failed or cancelled.
fn resumes_at_start(rows: &[DownloadStatus]) -> bool {
    use DownloadStatus as S;
    rows.iter()
        .any(|s| matches!(s, S::Downloading | S::Queued | S::Verifying))
        && !rows
            .iter()
            .any(|s| matches!(s, S::Paused | S::Failed | S::Cancelled))
}

/// What a new download does about the downloads not finished yet that write the same files
/// (anything not done: a paused or failed one can go on, and so can one whose cancel did not
/// finish), so that two downloads never write one file at once ([`plan_start`]).
#[derive(Debug, PartialEq, Eq)]
enum Start {
    /// No other download writes its files: it is recorded and starts.
    New,
    /// A download of the same kind writes all its files and runs (a second click on
    /// Download, the same model again after Back, the image file asked for again): that
    /// one is it.
    Join(String),
    /// That download stopped (paused, or the app closed meanwhile): it goes on.
    Resume(String),
    /// That download failed: it is forgotten, and this one starts with the newest listing,
    /// going on from the partial files that are still the same file.
    Replace(String),
    /// Another download writes some of its files (another model of the repository that
    /// shares the image file, or the image file for another model): refused until that one
    /// is done. `running`: that download's task runs (else it is paused or failed).
    Busy { running: bool },
    /// A download that writes some of its files is being cancelled: refused until it is gone.
    Cancelling,
}

/// Decides [`Start`] for a new download of `new` that writes `dests`, from every download
/// row, the saved metadata of a group (`meta_of`) and whether a group's task runs
/// (`running`) or is being cancelled (`cancelling`, [`cancelling_key`]). A download joins only
/// one of the same kind that writes all its files: an
/// image file for an installed model joins only a download of the image file for that same
/// model (never another model's download, which would then be restarted, shown and added in
/// its place), and a model download only a model download.
fn plan_start(
    new: &GroupMeta,
    dests: &[String],
    rows: &[store::Download],
    meta_of: impl Fn(&str) -> Option<GroupMeta>,
    running: impl Fn(&str) -> bool,
    cancelling: impl Fn(&str) -> bool,
) -> Start {
    // P6b round 2: a cancelled row counts too. It is left only by a cancel that did not
    // finish, and that group's cancel would delete what a second download wrote.
    let busy: Vec<&store::Download> = rows
        .iter()
        .filter(|r| r.status != DownloadStatus::Completed)
        .filter(|r| dests.contains(&r.dest_path))
        .collect();
    let Some(first) = busy.first() else {
        return Start::New;
    };
    if busy.iter().any(|r| cancelling(&r.group_id)) {
        return Start::Cancelling;
    }
    let group = first.group_id.as_str();
    let one_group = busy.iter().all(|r| r.group_id == group);
    let all_there = dests.iter().all(|d| busy.iter().any(|r| &r.dest_path == d));
    // 0.1.0 Studio: a chat download of FLUX.2 klein's text encoder never joins klein's.
    let same_kind = meta_of(group).is_some_and(|m| {
        m.file_only == new.file_only
            && m.image_file_for == new.image_file_for
            && generator_id(&m) == generator_id(new)
    });
    if !(one_group && all_there && same_kind) {
        return Start::Busy {
            running: busy.iter().any(|r| running(&r.group_id)),
        };
    }
    let failed = rows
        .iter()
        .any(|r| r.group_id == group && r.status == DownloadStatus::Failed);
    if running(group) {
        Start::Join(group.to_string())
    } else if failed {
        Start::Replace(group.to_string())
    } else {
        Start::Resume(group.to_string())
    }
}

/// The picture or video model a group downloads, if any.
fn generator_id(meta: &GroupMeta) -> Option<&str> {
    meta.generator.as_ref().map(|g| g.gen_model_id.as_str())
}

/// A new download refused because another download writes some of its files
/// ([`Start::Busy`]). An image file for an installed model is written next to that model, so
/// once the other download is done the model finds it there and uses it. `running`: that
/// download runs (else it is paused or failed).
fn busy_error(image_file: bool, running: bool) -> AppError {
    let (message, next_step) = match (image_file, running) {
        (true, true) => (
            "This image file is already being downloaded with another model. This model uses it too once that download is done.",
            "See Models → Downloads.",
        ),
        (true, false) => (
            "This image file is part of another model's download, which has stopped. This model uses it too once that download is done.",
            "Resume that download in Models → Downloads, or cancel it there and try again.",
        ),
        (false, true) => (
            "One of this model's files is already being downloaded.",
            "Let that download finish, or cancel it in Downloads, then try again.",
        ),
        (false, false) => (
            "One of this model's files is part of another download, which has stopped.",
            "Resume or cancel that download in Downloads, then try again.",
        ),
    };
    AppError::new("already_downloading", message, next_step, String::new())
}

/// A new download refused because a download of some of its files is being cancelled
/// ([`Start::Cancelling`]).
fn cancelling_error(image_file: bool) -> AppError {
    let message = if image_file {
        "This image file is part of a download that is being cancelled."
    } else {
        "One of this model's files is part of a download that is being cancelled."
    };
    AppError::new(
        "download_cancelling",
        message,
        "Wait a moment, then try again.",
        String::new(),
    )
}

/// What [`DownloadManager::record`] did with a new download.
#[derive(Debug)]
enum Recorded {
    /// Recorded it as a new group, to start.
    New(String),
    /// The group that writes the same files, which runs: nothing to do.
    Join(String),
    /// Readied the group that writes the same files to go on, to start.
    Resume(String),
    /// Recorded it as a new group, to start, in place of a failed group; `old` is how that
    /// one now shows (cancelled).
    Replaced {
        new: String,
        old: Option<Box<DownloadGroupDto>>,
    },
}

/// Records a new group: its metadata and a row per file. Returns its id.
async fn insert_group(
    store: &store::Store,
    meta: &GroupMeta,
    files: &[(FileSpec, PathBuf)],
) -> AppResult<String> {
    let group = uuid::Uuid::new_v4().to_string();
    store.set_setting(&meta_key(&group), meta).await?;
    for (spec, dest) in files {
        store
            .insert_download(NewDownload {
                group_id: group.clone(),
                repo: spec.repo.clone(),
                file: spec.path.clone(),
                url: resolve_url(&spec.repo, &spec.commit, &spec.path),
                dest_path: dest.display().to_string(),
                size_bytes: spec.size,
                sha256: spec.sha256.clone().unwrap_or_default(),
            })
            .await?;
    }
    Ok(group)
}

/// Forgets a failed group that a new download of the same files replaces: its rows and
/// metadata go. Its partial files of the files in `keep` stay, so the new download goes on
/// from those that are still the same file; its other partial files go. Returns the files
/// in `keep` it wrote itself ([`written_key`]), which the new download takes over.
async fn forget_group(
    store: &store::Store,
    group: &str,
    keep: &[String],
) -> AppResult<Vec<String>> {
    for row in store.list_downloads(Some(group.to_string())).await? {
        if !keep.contains(&row.dest_path) {
            models::download::discard_partial(Path::new(&row.dest_path)).await;
        }
    }
    let written: Vec<String> = written_by(store, group)
        .await
        .into_iter()
        .filter(|d| keep.contains(d))
        .collect();
    store.delete_download_group(group.to_string()).await?;
    let _ = store.delete_setting(&meta_key(group)).await;
    let _ = store.delete_setting(&written_key(group)).await;
    let _ = store.delete_setting(&cancelling_key(group)).await;
    Ok(written)
}

/// What the group `meta` says when its files are all there but its last step failed: adding
/// the model to Installed (0.1.0: a picture or video model to Studio), or recording the image
/// file on its model. Resume tries that step again without downloading anything.
fn last_step_error(meta: &GroupMeta, e: &AppError) -> AppError {
    let what = if meta.image_file_for.is_some() {
        "The image file was downloaded, but it could not be added to its model."
    } else if meta.generator.is_some() {
        "The model was downloaded, but it could not be added to Studio."
    } else {
        "The model was downloaded, but it could not be added to Installed."
    };
    AppError::new(
        LAST_STEP_FAILED,
        format!("{what} {}", e.message),
        "Resume to try again. If it fails again, cancel the download.",
        format!("{}: {}", e.code, e.details),
    )
}

/// Readies the rows of a stopped group to go on, each as [`status_on_resume`] says, with its
/// error cleared.
async fn requeue(store: &store::Store, group: &str) -> AppResult<()> {
    for row in store.list_downloads(Some(group.to_string())).await? {
        let whole = file_whole(Path::new(&row.dest_path), row.size_bytes);
        if let Some(status) = status_on_resume(&row, whole) {
            store
                .update_download(
                    row.id,
                    DownloadPatch {
                        status: Some(status),
                        error: Some(None),
                        ..Default::default()
                    },
                )
                .await?;
        }
    }
    Ok(())
}

/// Marks every row of `group` failed with `err`, for the UI ([`last_step_error`]).
async fn mark_failed(store: &store::Store, group: &str, err: &AppError) -> AppResult<()> {
    let text = serde_json::to_string(err).unwrap_or_default();
    for row in store.list_downloads(Some(group.to_string())).await? {
        store
            .update_download(
                row.id,
                DownloadPatch {
                    status: Some(DownloadStatus::Failed),
                    error: Some(Some(text.clone())),
                    ..Default::default()
                },
            )
            .await?;
    }
    Ok(())
}

/// 0.0.7 (P6b review C1): the settings key of the files a group wrote itself (renamed from
/// its own partial file into place), the only finished files Cancel may delete. Kept beside
/// the group's metadata, so nothing that builds a `GroupMeta` changes.
fn written_key(group: &str) -> String {
    format!("downloadWritten:{group}")
}

/// The files `group` wrote itself ([`written_key`]); none when they cannot be read (then
/// Cancel keeps every finished file).
async fn written_by(store: &store::Store, group: &str) -> Vec<String> {
    store
        .get_setting(&written_key(group))
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

/// Remembers that `group` wrote the file at `dest` itself.
async fn note_written(store: &store::Store, group: &str, dest: &str) -> AppResult<()> {
    let mut written = written_by(store, group).await;
    if !written.iter().any(|d| d == dest) {
        written.push(dest.to_string());
        store.set_setting(&written_key(group), &written).await?;
    }
    Ok(())
}

/// The finished files Cancel deletes besides the partial ones (0.0.7, P6b review C1): only
/// files the group wrote itself (`written`), still whole at their final name, inside `root`
/// once links are followed (the models folder, or for an image file the model's own
/// folder), and used by no installed model (`in_use`: its own file, or its image file,
/// which two models can share; 0.1.0: or a part of a picture or video model). A file the
/// download found in place (another app's copy, a
/// model taken off the list, a file copied in by hand) or one a failed fetch never replaced
/// is not its own and stays. When unsure (no folder known, a path that cannot be followed),
/// the file stays.
fn files_to_delete_on_cancel(
    rows: &[store::Download],
    written: &[String],
    root: Option<&Path>,
    in_use: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    let Some(root) = root.and_then(|r| dunce::canonicalize(r).ok()) else {
        return Vec::new();
    };
    rows.iter()
        .filter(|r| r.status != DownloadStatus::Completed)
        .filter(|r| written.contains(&r.dest_path))
        .filter(|r| file_whole(Path::new(&r.dest_path), r.size_bytes))
        .map(|r| PathBuf::from(&r.dest_path))
        .filter(|p| dunce::canonicalize(p).is_ok_and(|real| inside_folder(&real, &root)))
        .filter(|p| !in_use(p))
        .collect()
}

/// 0.0.7 (P6b round 2): whether the real path `file` is inside the real folder `root`.
/// `dunce` keeps Windows' verbatim prefix (`\\?\C:\…`, `\\?\UNC\server\…`) for a path
/// over 260 characters, so one side can have it and the other not: it is left out of both.
pub(crate) fn inside_folder(file: &Path, root: &Path) -> bool {
    inside_by(
        &file.to_string_lossy(),
        &root.to_string_lossy(),
        cfg!(windows),
    )
}

/// [`inside_folder`] on paths as text; `windows`: `\` separates too, and the verbatim
/// prefix is known.
fn inside_by(file: &str, root: &str, windows: bool) -> bool {
    let plain = |p: &str| -> String {
        if windows {
            if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
                return format!(r"\\{rest}");
            }
            if let Some(rest) = p.strip_prefix(r"\\?\") {
                return rest.to_string();
            }
        }
        p.to_string()
    };
    let separator = |c: char| c == '/' || (windows && c == '\\');
    let (file, root) = (plain(file), plain(root));
    file.strip_prefix(root.trim_end_matches(separator))
        .is_some_and(|rest| rest.starts_with(separator))
}

/// Deletes a cancelled group's files and forgets it: its partial files, and the finished
/// files [`files_to_delete_on_cancel`] allows (the models folder, or for an image file the
/// model's own folder, is where they may be). P6b round 2: never a file, partial or whole,
/// that another download not finished yet writes; and for Delete's cancel of an image file
/// (`deleting_model`), the model being deleted counts as using nothing, so the image file the
/// download wrote goes.
async fn clean_up(
    store: &store::Store,
    group: &str,
    models_dir: &Path,
    deleting_model: Option<&str>,
) -> AppResult<()> {
    let rows = store.list_downloads(Some(group.to_string())).await?;
    let elsewhere = files_being_written(store, Some(group)).await?;
    let meta: Option<GroupMeta> = store.get_setting(&meta_key(group)).await.ok().flatten();
    let written = written_by(store, group).await;
    for row in &rows {
        if row.status != DownloadStatus::Completed && !elsewhere.contains(&row.dest_path) {
            models::download::discard_partial(Path::new(&row.dest_path)).await;
        }
    }
    let root = match meta.as_ref().and_then(|m| m.image_file_for.as_deref()) {
        Some(model) => Path::new(model).parent().map(Path::to_path_buf),
        None => Some(models_dir.to_path_buf()),
    };
    // 0.1.0 Studio: nor a part of an installed picture or video model (*Download again*
    // fetches a damaged shared part again).
    match (store.list_models().await, store.gen_model_files().await) {
        (Ok(mut installed), Ok(gen_files)) => {
            if let Some(model) = deleting_model {
                installed.retain(|m| !crate::model_options::same_file(&m.path, model));
            }
            let in_use = |p: &Path| {
                elsewhere.iter().any(|d| Path::new(d) == p)
                    || crate::hub::used_elsewhere(&installed, &gen_files, p)
            };
            for path in files_to_delete_on_cancel(&rows, &written, root.as_deref(), in_use) {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!("a finished file of a cancelled download stayed: {e}");
                }
            }
        }
        (Err(e), _) | (_, Err(e)) => {
            tracing::warn!("the finished files of a cancelled download stayed: {e}");
        }
    }
    store.delete_download_group(group.to_string()).await?;
    let _ = store.delete_setting(&meta_key(group)).await;
    let _ = store.delete_setting(&written_key(group)).await;
    let _ = store.delete_setting(&cancelling_key(group)).await;
    Ok(())
}

/// What the app does with a group it finds when it starts.
#[derive(Debug, PartialEq, Eq)]
enum AtStart {
    /// It was going when the app stopped: it goes on.
    Resume,
    /// It was being cancelled when the app stopped: the cancel is finished.
    FinishCancel,
    /// Paused, failed or done: it stays as it is.
    Leave,
}

/// [`AtStart`] for a group with rows in `statuses`. A cancelled row is left only by a cancel
/// that did not finish (the app was closed while it ran, or a store error): it is finished
/// (P6b review m2). P6b round 2: so is a group marked as being cancelled (`cancelling`,
/// [`cancelling_key`]) whatever its rows say: its last file had finished (nothing was marked
/// cancelled), or its task was stuck (still "downloading").
fn at_start(statuses: &[DownloadStatus], cancelling: bool) -> AtStart {
    if cancelling || statuses.contains(&DownloadStatus::Cancelled) {
        AtStart::FinishCancel
    } else if resumes_at_start(statuses) {
        AtStart::Resume
    } else {
        AtStart::Leave
    }
}

impl DownloadManager {
    fn emit(&self, app: &AppHandle, dto: DownloadGroupDto) {
        let _ = app.emit_to(crate::MAIN_WINDOW, "download-progress", dto);
    }

    /// Whether the group's task runs now.
    fn is_running(&self, group: &str) -> bool {
        self.running.lock().is_ok_and(|r| r.contains_key(group))
    }

    /// Builds the DTO for one group from its rows and live progress.
    async fn group_dto(
        &self,
        store: &store::Store,
        group: &str,
    ) -> AppResult<Option<DownloadGroupDto>> {
        let rows = store.list_downloads(Some(group.to_string())).await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let meta: Option<GroupMeta> = store.get_setting(&meta_key(group)).await?;
        let cancelling = is_cancelling(store, group).await;
        // 0.0.7 (P6): the status is worked out knowing whether the task runs.
        let running = self.is_running(group);
        let live = self.live.lock().ok();
        let live = live.as_ref().and_then(|l| l.get(group));
        let bytes_total = rows.iter().map(|r| r.size_bytes).sum();
        let bytes_done = rows
            .iter()
            .map(|r| {
                live.and_then(|l| l.done.get(&r.file).copied())
                    .unwrap_or(r.bytes_done)
            })
            .sum();
        let statuses: Vec<DownloadStatus> = rows.iter().map(|r| r.status).collect();
        let status = group_status(&statuses, running);
        // P6b round 2: while Cancel stops it and deletes its files (and after a cancel that
        // did not finish in time), it says so; done before Cancel could stop it, it is done.
        let status = if status != DownloadStatus::Completed && cancelling {
            "cancelling"
        } else {
            status_name(status)
        };
        let error = rows
            .iter()
            .find_map(|r| r.error.as_ref())
            .and_then(|e| serde_json::from_str::<AppError>(e).ok());
        Ok(Some(DownloadGroupDto {
            id: group.to_string(),
            repo: meta
                .as_ref()
                .map(|m| m.repo.clone())
                .unwrap_or_else(|| rows[0].repo.clone()),
            label: meta
                .as_ref()
                .map(|m| m.label.clone())
                .unwrap_or_else(|| rows[0].file.clone()),
            saved_file: meta.as_ref().filter(|m| m.file_only).map(|m| SavedFileDto {
                commit: m.commit.clone(),
                path: rows[0].dest_path.clone(),
            }),
            file_count: rows.len(),
            bytes_done,
            bytes_total,
            status: status.to_string(),
            error,
            bytes_per_second: live.and_then(|l| l.speed),
            unverified: rows.iter().any(|r| r.sha256.is_empty()),
            checking: live.is_some_and(|l| l.checking),
            generator_kind: meta
                .as_ref()
                .and_then(|m| m.generator.as_ref())
                .map(|g| g.kind.clone()),
            gen_model_id: meta
                .as_ref()
                .and_then(|m| m.generator.as_ref())
                .map(|g| g.gen_model_id.clone()),
        }))
    }

    pub async fn list(&self, state: &AppState) -> AppResult<Vec<DownloadGroupDto>> {
        let rows = state.store.list_downloads(None).await?;
        let mut groups: Vec<String> = Vec::new();
        for r in rows.iter().rev() {
            if !groups.contains(&r.group_id) {
                groups.push(r.group_id.clone());
            }
        }
        let mut out = Vec::new();
        for g in groups {
            if let Some(dto) = self.group_dto(&state.store, &g).await? {
                out.push(dto);
            }
        }
        Ok(out)
    }

    /// Registers a new group and starts it. Returns the id of the download to follow.
    ///
    /// 0.0.7 (P6): two downloads never write one file at once ([`Self::record`]): the same
    /// files asked for again follow the download that has them (going on with it when it
    /// stopped, or starting afresh in place of one that failed), and a download some of whose
    /// files another one writes is refused until that one is done.
    pub async fn start(
        self: &Arc<Self>,
        app: &AppHandle,
        meta: GroupMeta,
        files: Vec<(FileSpec, PathBuf)>,
    ) -> AppResult<String> {
        let state = app.state::<AppState>();
        let _starting = self.starting.lock().await;
        let group = match self.record_locked(&state.store, &meta, &files).await? {
            Recorded::Join(group) => return Ok(group),
            Recorded::New(group) | Recorded::Resume(group) => group,
            Recorded::Replaced { new, old } => {
                if let Some(old) = old {
                    self.emit(app, *old);
                }
                new
            }
        };
        self.spawn_group(app.clone(), group.clone());
        Ok(group)
    }

    /// [`Self::record_locked`] holding `starting`, as [`Self::start`] does.
    #[cfg(test)]
    async fn record(
        &self,
        store: &store::Store,
        meta: &GroupMeta,
        files: &[(FileSpec, PathBuf)],
    ) -> AppResult<Recorded> {
        let _starting = self.starting.lock().await;
        self.record_locked(store, meta, files).await
    }

    /// The group [`Self::record`] gives: a new one, or the one it joins, resumes or replaces.
    #[cfg(test)]
    pub(crate) async fn record_group(
        &self,
        store: &store::Store,
        meta: &GroupMeta,
        files: &[(FileSpec, PathBuf)],
    ) -> AppResult<String> {
        Ok(match self.record(store, meta, files).await? {
            Recorded::New(group) | Recorded::Join(group) | Recorded::Resume(group) => group,
            Recorded::Replaced { new, .. } => new,
        })
    }

    /// 0.0.7 (P6): checks a new download against the downloads not finished yet
    /// ([`plan_start`]) and records what it decided. The caller holds `starting`, so two
    /// starts at once (a double click, the image file asked for twice) cannot both find
    /// nothing and then write one file, and a resume or cancel cannot come in between.
    async fn record_locked(
        &self,
        store: &store::Store,
        meta: &GroupMeta,
        files: &[(FileSpec, PathBuf)],
    ) -> AppResult<Recorded> {
        let dests: Vec<String> = files
            .iter()
            .map(|(_, dest)| dest.display().to_string())
            .collect();
        let rows = store.list_downloads(None).await?;
        let mut metas: HashMap<String, Option<GroupMeta>> = HashMap::new();
        let mut cancelling: Vec<String> = Vec::new();
        for r in rows.iter().filter(|r| dests.contains(&r.dest_path)) {
            if !metas.contains_key(&r.group_id) {
                let saved = store
                    .get_setting(&meta_key(&r.group_id))
                    .await
                    .ok()
                    .flatten();
                metas.insert(r.group_id.clone(), saved);
                if is_cancelling(store, &r.group_id).await {
                    cancelling.push(r.group_id.clone());
                }
            }
        }
        let plan = plan_start(
            meta,
            &dests,
            &rows,
            |g| metas.get(g).cloned().flatten(),
            |g| self.is_running(g),
            |g| cancelling.iter().any(|c| c == g),
        );
        match plan {
            Start::New => Ok(Recorded::New(insert_group(store, meta, files).await?)),
            Start::Join(group) => Ok(Recorded::Join(group)),
            Start::Resume(group) => {
                requeue(store, &group).await?;
                Ok(Recorded::Resume(group))
            }
            Start::Replace(group) => {
                let old = self.group_dto(store, &group).await?.map(|mut d| {
                    d.status = status_name(DownloadStatus::Cancelled).into();
                    Box::new(d)
                });
                let written = forget_group(store, &group, &dests).await?;
                let new = insert_group(store, meta, files).await?;
                if !written.is_empty() {
                    store.set_setting(&written_key(&new), &written).await?;
                }
                Ok(Recorded::Replaced { new, old })
            }
            Start::Busy { running } => Err(busy_error(meta.image_file_for.is_some(), running)),
            Start::Cancelling => Err(cancelling_error(meta.image_file_for.is_some())),
        }
    }

    /// Resumes every group that was running or queued when the app last stopped. 0.0.7
    /// (P6): not one the user paused, or one that failed ([`resumes_at_start`]).
    pub async fn resume_interrupted(self: &Arc<Self>, app: &AppHandle) {
        let state = app.state::<AppState>();
        let Ok(rows) = state.store.list_downloads(None).await else {
            return;
        };
        let mut groups: Vec<(String, Vec<DownloadStatus>)> = Vec::new();
        for r in rows {
            match groups.iter_mut().find(|(g, _)| *g == r.group_id) {
                Some((_, statuses)) => statuses.push(r.status),
                None => groups.push((r.group_id, vec![r.status])),
            }
        }
        for (g, statuses) in groups {
            let cancelling = is_cancelling(&state.store, &g).await;
            match at_start(&statuses, cancelling) {
                AtStart::Resume => {
                    tracing::info!(group = %g, "resuming an interrupted download");
                    self.spawn_group(app.clone(), g);
                }
                AtStart::FinishCancel => {
                    tracing::info!(group = %g, "finishing a cancel the app stopped during");
                    if let Err(e) = self.cancel(app, &g).await {
                        tracing::warn!(group = %g, "could not finish the cancel: {e}");
                    }
                }
                AtStart::Leave => {}
            }
        }
    }

    fn spawn_group(self: &Arc<Self>, app: AppHandle, group: String) {
        let token = CancellationToken::new();
        let ended = CancellationToken::new();
        let id = NEXT_TASK.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        {
            let Ok(mut running) = self.running.lock() else {
                return;
            };
            if running.contains_key(&group) {
                return;
            }
            running.insert(
                group.clone(),
                Running {
                    token: token.clone(),
                    paused: false,
                    ended: ended.clone(),
                    id,
                },
            );
        }
        let me = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            // P6b round 2: however the task ends, a panic included, it leaves `running` and
            // `ended` fires (when `end` is dropped, after the group's look was sent).
            let mut end = TaskEnd::new(Arc::clone(&me), group.clone(), id, ended);
            let result = me.run_group(&app, &group, &token).await;
            let paused = end.finish();
            if let Err(e) = result {
                tracing::warn!(group = %group, "download group stopped: {e}");
            }
            if paused {
                tracing::info!(group = %group, "download paused");
            }
            // P6b round 2: always shown: paused, failed or done, or "cancelling" when Cancel
            // did not finish in time (Cancel shows "cancelled" once the files are gone).
            let state = app.state::<AppState>();
            if let Ok(Some(dto)) = me.group_dto(&state.store, &group).await {
                me.emit(&app, dto);
            }
            drop(end);
        });
    }

    async fn run_group(
        &self,
        app: &AppHandle,
        group: &str,
        token: &CancellationToken,
    ) -> AppResult<()> {
        let state = app.state::<AppState>();
        let meta: GroupMeta = state
            .store
            .get_setting(&meta_key(group))
            .await?
            .ok_or_else(|| AppError::internal("download group without metadata"))?;
        let client = crate::hub::hf_client(app).await?;
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        for row in rows {
            if file_done(&row, file_whole(Path::new(&row.dest_path), row.size_bytes)) {
                continue;
            }
            let spec = FileSpec {
                repo: row.repo.clone(),
                commit: row_commit(&meta, &row.repo, &row.file),
                path: row.file.clone(),
                size: row.size_bytes,
                sha256: (!row.sha256.is_empty()).then(|| row.sha256.clone()),
            };
            let dest = PathBuf::from(&row.dest_path);
            state
                .store
                .update_download(
                    row.id,
                    DownloadPatch {
                        status: Some(DownloadStatus::Downloading),
                        error: Some(None),
                        bytes_done: Some(resumable_bytes(&dest, &spec)),
                        ..Default::default()
                    },
                )
                .await?;
            let progress = {
                let app = app.clone();
                let group = group.to_string();
                let file = row.file.clone();
                let row_id = row.id;
                move |p: Progress| self.on_progress(&app, &group, &file, row_id, p)
            };
            match download_file_checked(&client, &spec, &dest, token, &progress).await {
                Ok(downloaded) => {
                    // 0.0.7 (P6b review C1): a file this group wrote itself, the only kind
                    // Cancel may delete; remembered before the row says it is finished.
                    if downloaded.fetched {
                        note_written(&state.store, group, &row.dest_path).await?;
                    }
                    // Verified; the group becomes "done" once the model is registered.
                    state
                        .store
                        .update_download(
                            row.id,
                            DownloadPatch {
                                status: Some(DownloadStatus::Verifying),
                                bytes_done: Some(row.size_bytes),
                                ..Default::default()
                            },
                        )
                        .await?;
                }
                Err(models::download::DownloadError::Cancelled) => {
                    let paused = self
                        .running
                        .lock()
                        .ok()
                        .is_some_and(|r| r.get(group).is_some_and(|r| r.paused));
                    let status = if paused {
                        DownloadStatus::Paused
                    } else {
                        DownloadStatus::Cancelled
                    };
                    let done = resumable_bytes(&dest, &spec);
                    state
                        .store
                        .update_download(
                            row.id,
                            DownloadPatch {
                                status: Some(status),
                                bytes_done: Some(done),
                                ..Default::default()
                            },
                        )
                        .await?;
                    return Ok(());
                }
                Err(e) => {
                    let err = download_error_to_app(&e);
                    tracing::warn!(file = %row.file, code = %err.code, "download failed");
                    state
                        .store
                        .update_download(
                            row.id,
                            DownloadPatch {
                                status: Some(DownloadStatus::Failed),
                                error: Some(Some(serde_json::to_string(&err).unwrap_or_default())),
                                ..Default::default()
                            },
                        )
                        .await?;
                    return Err(err);
                }
            }
        }
        // 0.0.7 (P6b review m1): paused or cancelled meanwhile (a check that just ended): the
        // model is not added. Paused, it is added on Resume; cancelled, Cancel deletes it.
        if token.is_cancelled() {
            return Ok(());
        }
        let added = match meta.image_file_for.as_deref() {
            None if meta.file_only => Ok(()),
            Some(model) => self.attach_image_file(app, group, &meta, model).await,
            None => self.register_model(app, group, &meta).await,
        };
        if let Err(e) = added {
            // 0.0.7 (P6): the group says why it stopped instead of staying "downloading"
            // (and starting again at every start of the app). Its files stay: Resume tries
            // this step again without downloading them. 0.1.0 Studio: a picture or video
            // model whose diffusion file is another model says so in its own words; its files
            // are gone, so Resume would download them again.
            let err = if e.code == studio_errors::NOT_THAT_MODEL {
                e
            } else {
                last_step_error(&meta, &e)
            };
            mark_failed(&state.store, group, &err).await?;
            return Err(err);
        }
        for row in state.store.list_downloads(Some(group.to_string())).await? {
            state
                .store
                .update_download(
                    row.id,
                    DownloadPatch {
                        status: Some(DownloadStatus::Completed),
                        ..Default::default()
                    },
                )
                .await?;
        }
        // Its files belong to the model now: no Cancel will look for them.
        let _ = state.store.delete_setting(&written_key(group)).await;
        Ok(())
    }

    fn on_progress(
        &self,
        app: &AppHandle,
        group: &str,
        file: &str,
        row_id: store::DownloadId,
        p: Progress,
    ) {
        let now = Instant::now();
        let (emit, persist) = {
            let Ok(mut all) = self.live.lock() else {
                return;
            };
            all.entry(group.to_string()).or_default().note(file, p, now)
        };
        if !(emit || persist) {
            return;
        }
        let n = match p {
            Progress::Bytes(n) | Progress::Checking(n) => n,
        };
        let app = app.clone();
        let group = group.to_string();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<AppState>();
            if persist {
                let _ = state
                    .store
                    .update_download(
                        row_id,
                        DownloadPatch {
                            bytes_done: Some(n),
                            ..Default::default()
                        },
                    )
                    .await;
            }
            if emit
                && let Some(dl) = app.try_state::<Arc<DownloadManager>>()
                && let Ok(Some(dto)) = dl.group_dto(&state.store, &group).await
            {
                dl.emit(&app, dto);
            }
        });
    }

    /// Adds the finished model to the installed list; a picture or video model joins Studio's
    /// instead ([`register_gen_model`]).
    async fn register_model(
        &self,
        app: &AppHandle,
        group: &str,
        meta: &GroupMeta,
    ) -> AppResult<()> {
        let state = app.state::<AppState>();
        if meta.generator.is_some() {
            let models_dir = crate::hub::models_dir(app).await;
            return register_gen_model(&state.store, &models_dir, group, meta).await;
        }
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        let path_of = |file: &str| {
            rows.iter()
                .find(|r| r.file == file)
                .map(|r| r.dest_path.clone())
        };
        let Some(first) = meta.model_files.first().and_then(|f| path_of(f)) else {
            return Err(AppError::internal("finished download has no model file"));
        };
        let projector = meta.projector.as_deref().and_then(path_of);
        let size: u64 = meta
            .model_files
            .iter()
            .filter_map(|f| rows.iter().find(|r| &r.file == f))
            .map(|r| r.size_bytes)
            .sum();
        let sha = rows
            .iter()
            .find(|r| r.dest_path == first)
            .map(|r| r.sha256.clone())
            .filter(|s| !s.is_empty());
        // 0.0.7 (P2 review M1): what was published for the image file downloaded with it.
        let projector_sha256 = meta
            .projector
            .as_deref()
            .and_then(|file| rows.iter().find(|r| r.file == file))
            .map(|r| r.sha256.clone())
            .filter(|s| !s.is_empty());
        crate::hub::register_installed(
            &state,
            Path::new(&first),
            Some(meta.repo.clone()),
            size,
            sha,
            projector.clone(),
            ModelSource::Download,
            crate::hub::SavedFacts {
                projector_sha256,
                ..saved_facts(meta)
            },
        )
        .await?;
        // 0.0.7 (P6b review m4): an image file this download wrote itself is one Delete may
        // remove with the model; one it found in place is not.
        if let Some(image) = projector
            && written_by(&state.store, group).await.contains(&image)
        {
            crate::hub::note_downloaded_image(&state.store, &first, Path::new(&image)).await?;
        }
        tracing::info!(repo = %meta.repo, label = %meta.label, "download finished");
        Ok(())
    }

    /// 0.0.7 (P2): records a finished image file on the installed model it was downloaded
    /// for (`GroupMeta::image_file_for`).
    async fn attach_image_file(
        &self,
        app: &AppHandle,
        group: &str,
        meta: &GroupMeta,
        model: &str,
    ) -> AppResult<()> {
        let state = app.state::<AppState>();
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        let Some(row) = meta
            .projector
            .as_deref()
            .and_then(|file| rows.iter().find(|r| r.file == file))
        else {
            return Err(AppError::internal("finished download has no image file"));
        };
        let image = PathBuf::from(&row.dest_path);
        // Checked against what the repository published (0.0.7 P2 review M1).
        let sha256 = (!row.sha256.is_empty()).then(|| row.sha256.clone());
        crate::hub::image_file::attach(&state.store, model, &image, sha256).await?;
        // 0.0.7 (P6b review m4): written by this download, so Delete may remove it.
        let dest = image.display().to_string();
        if written_by(&state.store, group).await.contains(&dest) {
            crate::hub::note_downloaded_image(&state.store, model, &image).await?;
        }
        tracing::info!(repo = %meta.repo, "image file downloaded");
        Ok(())
    }

    pub fn pause(&self, group: &str) {
        if let Ok(mut r) = self.running.lock()
            && let Some(running) = r.get_mut(group)
        {
            running.paused = true;
            running.token.cancel();
        }
    }

    /// Goes on with a stopped group. 0.0.7 (P6): a file that finished stays as it is and is
    /// never downloaded again ([`status_on_resume`]). It holds `starting` (P6b review m3) until
    /// the task is in `running` (P6b round 2, as [`Self::start`] does), so a start or a Cancel
    /// that comes next sees it running.
    pub async fn resume(self: &Arc<Self>, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        let _starting = self.starting.lock().await;
        self.ready_to_resume_locked(&state.store, group).await?;
        self.spawn_group(app.clone(), group.to_string());
        Ok(())
    }

    /// [`Self::ready_to_resume_locked`] holding `starting`.
    #[cfg(test)]
    async fn ready_to_resume(&self, store: &store::Store, group: &str) -> AppResult<()> {
        let _starting = self.starting.lock().await;
        self.ready_to_resume_locked(store, group).await
    }

    /// Readies a stopped group to go on ([`requeue`]); the caller holds `starting`. P6b round
    /// 2: refused while the group is being cancelled, and while another download not finished
    /// yet writes one of its files (both would write one partial file). A group that runs is
    /// left as it is.
    async fn ready_to_resume_locked(&self, store: &store::Store, group: &str) -> AppResult<()> {
        if self.is_running(group) {
            return Ok(());
        }
        if is_cancelling(store, group).await {
            return Err(AppError::new(
                "download_cancelling",
                "This download is being cancelled.",
                "Wait a moment, then download it again.",
                String::new(),
            ));
        }
        let rows = store.list_downloads(None).await?;
        let mine: Vec<&str> = rows
            .iter()
            .filter(|r| r.group_id == group)
            .map(|r| r.dest_path.as_str())
            .collect();
        let others: Vec<&store::Download> = rows
            .iter()
            .filter(|r| r.group_id != group && r.status != DownloadStatus::Completed)
            .filter(|r| mine.contains(&r.dest_path.as_str()))
            .collect();
        if !others.is_empty() {
            let image_file = store
                .get_setting::<GroupMeta>(&meta_key(group))
                .await
                .ok()
                .flatten()
                .is_some_and(|m| m.image_file_for.is_some());
            let running = others.iter().any(|r| self.is_running(&r.group_id));
            return Err(busy_error(image_file, running));
        }
        requeue(store, group).await
    }

    /// Stops the group, deletes its partial files and forgets it. 0.0.7 (P6): the files it
    /// wrote itself go too, unless an installed model uses them
    /// ([`files_to_delete_on_cancel`]). P6b review: it waits for the group's task to end
    /// first (m1), so nothing is written, renamed or added after it looked. P6b round 2: it
    /// first saves the group's marker ([`cancelling_key`]) and tells the task to stop, holding
    /// `starting`; it waits for the task without it, so other downloads go on meanwhile (the
    /// marker keeps a start or resume of these files out); and it deletes holding it again. A
    /// group that finished before it could stop is left as it is.
    pub async fn cancel(&self, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        let models_dir = crate::hub::models_dir(app).await;
        self.cancel_core(&state.store, &models_dir, group, None, CANCEL_WAIT, |dto| {
            self.emit(app, dto)
        })
        .await
    }

    /// [`Self::cancel`], with the store, the models folder, how long to wait for the task and
    /// where the group's new look goes (`emit`) given. `deleting_model`: the installed model
    /// whose Delete cancels this download of its image file.
    async fn cancel_core(
        &self,
        store: &store::Store,
        models_dir: &Path,
        group: &str,
        deleting_model: Option<&str>,
        wait: Duration,
        emit: impl Fn(DownloadGroupDto),
    ) -> AppResult<()> {
        let ended = {
            let _starting = self.starting.lock().await;
            let rows = store.list_downloads(Some(group.to_string())).await?;
            if rows.is_empty() {
                return Ok(());
            }
            if rows.iter().all(|r| r.status == DownloadStatus::Completed) {
                return self.leave_done(store, group, &emit).await;
            }
            store.set_setting(&cancelling_key(group), &true).await?;
            let task = self
                .running
                .lock()
                .ok()
                .and_then(|r| r.get(group).map(|r| (r.token.clone(), r.ended.clone())));
            if let Ok(Some(dto)) = self.group_dto(store, group).await {
                emit(dto);
            }
            task.map(|(token, ended)| {
                token.cancel();
                ended
            })
        };
        if let Some(ended) = ended
            && tokio::time::timeout(wait, ended.cancelled()).await.is_err()
        {
            return Err(stopping_error());
        }
        let _starting = self.starting.lock().await;
        let rows = store.list_downloads(Some(group.to_string())).await?;
        if rows.is_empty() {
            // Another Cancel finished it meanwhile.
            return Ok(());
        }
        if self.is_running(group) {
            return Err(stopping_error());
        }
        if rows.iter().all(|r| r.status == DownloadStatus::Completed) {
            return self.leave_done(store, group, &emit).await;
        }
        let mut dto = self.group_dto(store, group).await?;
        clean_up(store, group, models_dir, deleting_model).await?;
        if let Some(d) = dto.as_mut() {
            d.status = "cancelled".into();
            emit(d.clone());
        }
        Ok(())
    }

    /// A group Cancel finds done (it finished before it could stop) stays done.
    async fn leave_done(
        &self,
        store: &store::Store,
        group: &str,
        emit: &impl Fn(DownloadGroupDto),
    ) -> AppResult<()> {
        let _ = store.delete_setting(&cancelling_key(group)).await;
        if let Ok(Some(dto)) = self.group_dto(store, group).await {
            emit(dto);
        }
        Ok(())
    }

    /// 0.0.7 (P6b review m1): cancels the downloads of an image file for the installed model
    /// at `model` (Delete of that model), so none finishes after the model is gone and leaves
    /// its file behind.
    pub async fn cancel_image_files_for(&self, app: &AppHandle, model: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        let models_dir = crate::hub::models_dir(app).await;
        for g in image_file_downloads_for(&state.store, model).await? {
            self.cancel_core(
                &state.store,
                &models_dir,
                &g,
                Some(model),
                CANCEL_WAIT,
                |dto| self.emit(app, dto),
            )
            .await?;
        }
        Ok(())
    }
}

/// What Cancel says when the group's task did not end in time.
fn stopping_error() -> AppError {
    AppError::new(
        "download_stopping",
        "The download is still stopping.",
        "Wait a moment, then cancel it again.",
        String::new(),
    )
}

/// The groups still downloading an image file for the installed model at `model` (P6b
/// review m1: Delete cancels them first).
async fn image_file_downloads_for(store: &store::Store, model: &str) -> AppResult<Vec<String>> {
    let mut unfinished: Vec<String> = Vec::new();
    for r in store.list_downloads(None).await? {
        if r.status != DownloadStatus::Completed && !unfinished.contains(&r.group_id) {
            unfinished.push(r.group_id);
        }
    }
    let mut out = Vec::new();
    for group in unfinished {
        let meta: Option<GroupMeta> = store.get_setting(&meta_key(&group)).await.ok().flatten();
        if meta
            .and_then(|m| m.image_file_for)
            .is_some_and(|m| crate::model_options::same_file(&m, model))
        {
            out.push(group);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A group saved before 0.0.6 still reads, and a finished download saves its catalog
    /// id, its file's path in the repository and the uploader's facts with the model.
    #[test]
    fn group_meta_old_and_new() {
        let old: GroupMeta = serde_json::from_value(serde_json::json!({
            "repo": "example-quants/Sample-7B-GGUF",
            "commit": "4444444444444444444444444444444444444444",
            "label": "Q4_K_M",
            "modelFiles": ["Q4/Sample-7B-Q4_K_M-00001-of-00002.gguf", "Q4/Sample-7B-Q4_K_M-00002-of-00002.gguf"],
            "projector": null,
            "catalogId": "sample-7b-q4km"
        }))
        .unwrap();
        assert!(old.hub.is_none() && old.parameters.is_none());
        let saved = saved_facts(&old);
        assert_eq!(saved.catalog_id.as_deref(), Some("sample-7b-q4km"));
        assert_eq!(
            saved.repo_file.as_deref(),
            Some("Q4/Sample-7B-Q4_K_M-00001-of-00002.gguf")
        );
        assert!(saved.hub.is_none());
        // Written again, the unknown facts stay out of the JSON.
        let json = serde_json::to_value(&old).unwrap();
        assert!(json.get("hub").is_none() && json.get("parameters").is_none());

        let hub = crate::dto::HubFacts {
            repo: "example-quants/Sample-7B-GGUF".into(),
            author: Some("example-quants".into()),
            pipeline_tag: Some("text-generation".into()),
            base_model: Some("example-labs/Sample-7B".into()),
            languages: vec!["en".into()],
            license: None,
            tags: vec!["code".into()],
            downloads: Some(1),
            likes: Some(2),
            last_modified: None,
            gated: false,
            card_url: "https://huggingface.co/example-quants/Sample-7B-GGUF".into(),
        };
        let new = GroupMeta {
            hub: Some(hub.clone()),
            parameters: Some(7_000_000_000),
            ..old
        };
        let back: GroupMeta = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
        let saved = saved_facts(&back);
        assert_eq!(saved.hub, Some(hub));
        assert_eq!(saved.parameters, Some(7_000_000_000));

        // Saved uploader facts this version cannot read (a field missing) are dropped on
        // their own: the group still reads, so it can still resume and be registered.
        let mut json = serde_json::to_value(&new).unwrap();
        json["hub"].as_object_mut().unwrap().remove("cardUrl");
        let partial: GroupMeta = serde_json::from_value(json).unwrap();
        assert!(partial.hub.is_none());
        assert_eq!(partial.catalog_id.as_deref(), Some("sample-7b-q4km"));
        assert_eq!(partial.parameters, Some(7_000_000_000));
        assert_eq!(partial.model_files.len(), 2);
    }

    /// 0.0.7 (P2): a group that is only an installed model's image file names that model; a
    /// group saved before (or any model download) has none, and the field stays out of its
    /// JSON.
    #[test]
    fn an_image_file_group_names_its_model() {
        let old: GroupMeta = serde_json::from_value(serde_json::json!({
            "repo": "example-quants/Sample-7B-GGUF",
            "commit": "4444444444444444444444444444444444444444",
            "label": "Q4_K_M",
            "modelFiles": ["Sample-7B-Q4_K_M.gguf"],
            "projector": "mmproj-Sample-F16.gguf",
            "catalogId": null
        }))
        .unwrap();
        assert_eq!(old.image_file_for, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("imageFileFor")
                .is_none()
        );
        let image = GroupMeta {
            label: "Image file".into(),
            model_files: Vec::new(),
            image_file_for: Some(
                "/models/example-quants/Sample-7B-GGUF/Sample-7B-Q4_K_M.gguf".into(),
            ),
            ..old
        };
        let json = serde_json::to_value(&image).unwrap();
        assert_eq!(
            json["imageFileFor"],
            "/models/example-quants/Sample-7B-GGUF/Sample-7B-Q4_K_M.gguf"
        );
        let back: GroupMeta = serde_json::from_value(json).unwrap();
        assert_eq!(back.image_file_for, image.image_file_for);
        assert_eq!(back.projector.as_deref(), Some("mmproj-Sample-F16.gguf"));
        assert!(back.model_files.is_empty());
    }

    /// 0.0.7 (P2 review M2): the repository's image files go with a model download to its
    /// row; a group saved before has none and leaves them out of its JSON.
    #[test]
    fn a_download_carries_its_repositorys_image_files() {
        let old: GroupMeta = serde_json::from_value(serde_json::json!({
            "repo": "example-quants/Sample-7B-GGUF",
            "commit": "4444444444444444444444444444444444444444",
            "label": "Q4_K_M",
            "modelFiles": ["Sample-7B-Q4_K_M.gguf"],
            "projector": null,
            "catalogId": null
        }))
        .unwrap();
        assert_eq!(old.image_files, None);
        assert_eq!(saved_facts(&old).image_files, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("imageFiles")
                .is_none()
        );
        let files = vec![models::RepoFile {
            path: "mmproj-Sample-F16.gguf".into(),
            size: 851_443_712,
            sha256: Some("ab".repeat(32)),
        }];
        let new = GroupMeta {
            image_files: Some(files.clone()),
            ..old
        };
        let back: GroupMeta = serde_json::from_value(serde_json::to_value(&new).unwrap()).unwrap();
        assert_eq!(back.image_files, Some(files.clone()));
        assert_eq!(saved_facts(&back).image_files, Some(files));
        assert_eq!(saved_facts(&back).projector_sha256, None);
    }

    // ---- 0.0.7 (P6) bug fixes ------------------------------------------------------------

    use DownloadStatus as S;

    /// A folder of its own for one test, removed when it is dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("downloads-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A row of group `group` for the file at `dest`.
    fn row(group: &str, dest: &Path, size: u64, status: DownloadStatus) -> store::Download {
        store::Download {
            id: store::DownloadId(1),
            group_id: group.into(),
            repo: "example-quants/Sample-7B-GGUF".into(),
            file: dest
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            url: String::new(),
            dest_path: dest.display().to_string(),
            size_bytes: size,
            sha256: String::new(),
            bytes_done: 0,
            status,
            etag: None,
            error: None,
            created_at: 0,
            updated_at: 0,
        }
    }

    /// A model on the Installed list at `path`, with the image file `projector` recorded.
    fn installed(path: &Path, projector: Option<&Path>) -> store::ModelRecord {
        store::ModelRecord {
            id: store::ModelId(1),
            path: path.display().to_string(),
            repo: Some("example-quants/Sample-7B-GGUF".into()),
            file_name: String::new(),
            size_bytes: 1,
            sha256: None,
            gguf_summary: serde_json::json!({
                "architecture": "sample",
                "projector": projector.map(|p| p.display().to_string()),
            }),
            added_at: 0,
            source: ModelSource::Download,
        }
    }

    /// Records the file at `dest` (`size` bytes) in group `group` of `store`, with `status`.
    async fn recorded(
        store: &store::Store,
        group: &str,
        dest: &Path,
        size: u64,
        status: DownloadStatus,
    ) {
        let id = store
            .insert_download(NewDownload {
                group_id: group.into(),
                repo: "example-quants/Sample-7B-GGUF".into(),
                file: dest.file_name().unwrap().to_string_lossy().into_owned(),
                url: String::new(),
                dest_path: dest.display().to_string(),
                size_bytes: size,
                sha256: String::new(),
            })
            .await
            .unwrap();
        let error = (status == S::Failed).then(|| "{}".to_string());
        store
            .update_download(
                id,
                DownloadPatch {
                    status: Some(status),
                    error: Some(error),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    async fn statuses(store: &store::Store, group: &str) -> Vec<DownloadStatus> {
        store
            .list_downloads(Some(group.into()))
            .await
            .unwrap()
            .iter()
            .map(|r| r.status)
            .collect()
    }

    /// A file is whole when it has its full size at its final name.
    #[test]
    fn a_whole_file_has_its_full_size() {
        let dir = Scratch::new();
        let file = dir.path().join("Sample-7B-Q8_0.gguf");
        assert!(!file_whole(&file, 100), "not there");
        std::fs::write(&file, vec![0u8; 100]).unwrap();
        assert!(file_whole(&file, 100));
        assert!(!file_whole(&file, 101), "another size");
        assert!(!file_whole(dir.path(), 0), "a folder");
    }

    /// The bug: a vision model whose model file finished and whose image file failed. Resume
    /// set every row that was not done back to Queued, so the model file (12.7 GB in the
    /// owner's case) was downloaded again from zero, over the good file. Now a finished file
    /// waits for the last step and only the image file is downloaded again.
    #[tokio::test]
    async fn resume_never_downloads_a_finished_file_again() {
        let dir = Scratch::new();
        let model = dir.path().join("Sample-7B-Q8_0.gguf");
        let image = dir.path().join("mmproj-Sample-F16.gguf");
        std::fs::write(&model, vec![0u8; 100]).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &model, 100, S::Verifying).await;
        recorded(&store, "g", &image, 50, S::Failed).await;
        requeue(&store, "g").await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Verifying, S::Queued]);
        let rows = store.list_downloads(Some("g".into())).await.unwrap();
        assert!(rows.iter().all(|r| r.error.is_none()), "errors cleared");
        // The group's task then skips the finished file and downloads the other one.
        assert!(file_done(&rows[0], file_whole(&model, 100)));
        assert!(!file_done(&rows[1], file_whole(&image, 50)));

        // A finished file that is gone since is downloaded again.
        std::fs::remove_file(&model).unwrap();
        requeue(&store, "g").await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Queued, S::Queued]);

        // Every row: what Resume makes of it.
        let at = |s: DownloadStatus| row("g", Path::new("/models/m.gguf"), 1, s);
        assert_eq!(status_on_resume(&at(S::Completed), true), None);
        assert_eq!(
            status_on_resume(&at(S::Verifying), true),
            Some(S::Verifying)
        );
        assert_eq!(
            status_on_resume(&last_step_failed(at(S::Failed)), true),
            Some(S::Verifying)
        );
        for s in [
            S::Queued,
            S::Downloading,
            S::Paused,
            S::Verifying,
            S::Failed,
        ] {
            assert_eq!(status_on_resume(&at(s), false), Some(S::Queued), "{s:?}");
        }
        // A file whole at its name that this group did not finish is checked by
        // `download_file`, which does not fetch it again when it matches its checksum.
        assert_eq!(status_on_resume(&at(S::Paused), true), Some(S::Queued));
    }

    /// `row` as `mark_failed` leaves it after the group's last step failed.
    fn last_step_failed(mut row: store::Download) -> store::Download {
        let err = last_step_error(&group_meta(None), &AppError::internal("locked"));
        row.status = S::Failed;
        row.error = Some(serde_json::to_string(&err).unwrap());
        row
    }

    /// P6b review M1: a row that failed while downloading, with a file of the right size at
    /// its final name (an older or damaged copy the failed fetch never replaced), is
    /// downloaded, and so checked, again on Resume; before, it was added to Installed
    /// unchecked. Only a row whose last step failed after its file was checked counts as
    /// finished.
    #[tokio::test]
    async fn resume_checks_a_file_a_failed_download_left_at_its_name() {
        let dir = Scratch::new();
        let older = dir.path().join("Sample-7B-Q8_0.gguf");
        std::fs::write(&older, vec![7u8; 100]).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &older, 100, S::Failed).await;
        requeue(&store, "g").await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Queued]);
        let rows = store.list_downloads(Some("g".into())).await.unwrap();
        assert!(!file_done(&rows[0], true));

        let checked = last_step_error(&group_meta(None), &AppError::internal("locked"));
        mark_failed(&store, "g", &checked).await.unwrap();
        requeue(&store, "g").await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Verifying]);
    }

    /// The bug: paused after its first file finished, a group showed "downloading" with a
    /// Pause that did nothing and no Resume, and it started again by itself at the next start
    /// of the app. A finished file now counts as progress only while the group's task runs.
    #[test]
    fn a_group_paused_after_its_first_file_is_paused() {
        assert_eq!(group_status(&[S::Verifying, S::Paused], false), S::Paused);
        assert_eq!(status_name(S::Paused), "paused");
        // Also while its task is still ending.
        assert_eq!(group_status(&[S::Verifying, S::Paused], true), S::Paused);
        assert!(!resumes_at_start(&[S::Verifying, S::Paused]));
        assert!(!resumes_at_start(&[S::Verifying, S::Paused, S::Queued]));

        // While the task runs, a finished file is progress.
        assert_eq!(
            group_status(&[S::Verifying, S::Downloading], true),
            S::Downloading
        );
        assert_eq!(
            group_status(&[S::Verifying, S::Queued], true),
            S::Downloading
        );
        assert_eq!(group_status(&[S::Queued, S::Queued], true), S::Queued);
        // Stopped any other way (nothing runs it): it can be resumed.
        assert_eq!(
            group_status(&[S::Verifying, S::Verifying], false),
            S::Paused
        );
        assert_eq!(group_status(&[S::Verifying, S::Queued], false), S::Paused);
        // Done, failed and cancelled.
        assert_eq!(
            group_status(&[S::Completed, S::Completed], false),
            S::Completed
        );
        assert_eq!(group_status(&[S::Verifying, S::Failed], false), S::Failed);
        assert_eq!(
            group_status(&[S::Verifying, S::Cancelled, S::Queued], true),
            S::Cancelled
        );

        // At the next start an interrupted group goes on; a failed or finished one does not.
        assert!(resumes_at_start(&[S::Verifying, S::Downloading, S::Queued]));
        assert!(resumes_at_start(&[S::Verifying, S::Verifying]));
        assert!(!resumes_at_start(&[S::Verifying, S::Failed]));
        assert!(!resumes_at_start(&[S::Completed, S::Completed]));
    }

    /// P6b review m2: a cancel the app stopped during (closed, or a store error half way)
    /// left rows like Verifying and Cancelled: the group showed "cancelled" with no buttons
    /// and blocked new downloads of its files for good. Now the cancel is finished at the next
    /// start, and until then the group shows as paused, with Cancel and Resume.
    #[test]
    fn a_cancel_left_half_done_is_finished_and_can_be_cancelled_again() {
        assert_eq!(
            at_start(&[S::Verifying, S::Cancelled], false),
            AtStart::FinishCancel
        );
        assert_eq!(
            at_start(&[S::Cancelled, S::Queued], false),
            AtStart::FinishCancel
        );
        assert_eq!(
            at_start(&[S::Verifying, S::Downloading], false),
            AtStart::Resume
        );
        assert_eq!(at_start(&[S::Verifying, S::Paused], false), AtStart::Leave);
        assert_eq!(at_start(&[S::Completed], false), AtStart::Leave);
        assert_eq!(
            group_status(&[S::Verifying, S::Cancelled], false),
            S::Paused
        );
        // While Cancel stops its task, it shows as cancelled.
        assert_eq!(
            group_status(&[S::Verifying, S::Cancelled], true),
            S::Cancelled
        );
    }

    /// P6b review m3: Resume waits for a start that is checking and recording a download, so
    /// a Download of the same failed group cannot forget it while the resumed one writes its
    /// partial file.
    #[tokio::test]
    async fn resume_waits_for_a_start_in_progress() {
        let dir = Scratch::new();
        let model = dir.path().join("Sample-7B-Q8_0.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &model, 100, S::Paused).await;
        let manager = DownloadManager::default();
        let starting = manager.starting.lock().await;
        let resume = manager.ready_to_resume(&store, "g");
        tokio::pin!(resume);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), &mut resume)
                .await
                .is_err(),
            "it waits"
        );
        assert_eq!(statuses(&store, "g").await, vec![S::Paused]);
        drop(starting);
        resume.await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Queued]);
    }

    /// P6b review m5: checking a file (minutes for a large one) shows as "checking the file"
    /// with its progress, instead of "downloading" at 0 bytes. The check of a file that just
    /// finished downloading keeps its bytes, so the bar does not go back.
    #[tokio::test]
    async fn checking_a_file_shows_as_checking() {
        let mut live = Live::default();
        let t = Instant::now();
        live.note("m.gguf", Progress::Checking(5), t);
        assert!(live.checking);
        assert_eq!(live.done["m.gguf"], 5);
        live.note("m.gguf", Progress::Checking(9), t);
        assert_eq!(live.done["m.gguf"], 9);
        live.note("mmproj.gguf", Progress::Bytes(3), t);
        assert!(!live.checking, "downloading again");
        live.note("mmproj.gguf", Progress::Bytes(100), t);
        live.note("mmproj.gguf", Progress::Checking(10), t);
        assert!(live.checking);
        assert_eq!(live.done["mmproj.gguf"], 100);

        let dir = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &dir.path().join("m.gguf"), 100, S::Downloading).await;
        let manager = DownloadManager::default();
        manager
            .live
            .lock()
            .unwrap()
            .entry("g".into())
            .or_default()
            .note("m.gguf", Progress::Checking(5), t);
        let dto = manager.group_dto(&store, "g").await.unwrap().unwrap();
        assert!(dto.checking);
        assert_eq!(serde_json::to_value(&dto).unwrap()["checking"], true);
    }

    /// A group's saved metadata: a model download, or (`image_file_for`) the image file of
    /// the installed model at that path.
    fn group_meta(image_file_for: Option<&Path>) -> GroupMeta {
        GroupMeta {
            repo: "example-quants/Sample-7B-GGUF".into(),
            commit: "4".repeat(40),
            label: "Q4_K_M".into(),
            model_files: Vec::new(),
            projector: None,
            catalog_id: None,
            hub: None,
            parameters: None,
            image_file_for: image_file_for.map(|p| p.display().to_string()),
            image_files: None,
            generator: None,
            file_only: false,
        }
    }

    /// One file of a new download, written to `dest`.
    fn file(dest: &Path, size: u64) -> (FileSpec, PathBuf) {
        let spec = FileSpec {
            repo: "example-quants/Sample-7B-GGUF".into(),
            commit: "4".repeat(40),
            path: dest.file_name().unwrap().to_string_lossy().into_owned(),
            size,
            sha256: Some("ab".repeat(32)),
        };
        (spec, dest.to_path_buf())
    }

    /// [`plan_start`] for a new download of `new` writing `dests`, with the groups' saved
    /// metadata `metas` and the groups whose task runs.
    fn plan<P: AsRef<Path>>(
        new: &GroupMeta,
        dests: &[P],
        rows: &[store::Download],
        metas: &[(&str, GroupMeta)],
        running: &[&str],
    ) -> Start {
        let dests: Vec<String> = dests
            .iter()
            .map(|p| p.as_ref().display().to_string())
            .collect();
        plan_start(
            new,
            &dests,
            rows,
            |g| {
                metas
                    .iter()
                    .find(|(id, _)| *id == g)
                    .map(|(_, m)| m.clone())
            },
            |g| running.contains(&g),
            |_| false,
        )
    }

    /// [`plan`] with the groups being cancelled.
    fn plan_while_cancelling<P: AsRef<Path>>(
        new: &GroupMeta,
        dests: &[P],
        rows: &[store::Download],
        metas: &[(&str, GroupMeta)],
        cancelling: &[&str],
    ) -> Start {
        let dests: Vec<String> = dests
            .iter()
            .map(|p| p.as_ref().display().to_string())
            .collect();
        plan_start(
            new,
            &dests,
            rows,
            |g| {
                metas
                    .iter()
                    .find(|(id, _)| *id == g)
                    .map(|(_, m)| m.clone())
            },
            |_| false,
            |g| cancelling.contains(&g),
        )
    }

    async fn set_all(store: &store::Store, group: &str, status: DownloadStatus) {
        for r in store.list_downloads(Some(group.into())).await.unwrap() {
            store
                .update_download(
                    r.id,
                    DownloadPatch {
                        status: Some(status),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
    }

    /// The bug: two downloads of one file (a double click on Download, Download again after
    /// Back, two models of one repository sharing one image file, "Download the image file")
    /// wrote the same partial file at once and spoiled each other. And (P2 review, m5)
    /// "Download the image file" followed any download of that file: asked for Q4, it could
    /// restart a paused download of Q8 (gigabytes), show its progress and errors, and add Q8
    /// to Installed instead of recording the image file; a failed one was resumed for ever
    /// instead of starting afresh.
    #[test]
    fn two_downloads_never_write_one_file() {
        let dir = Path::new("/models/example-quants/Sample-7B-GGUF");
        let q4 = dir.join("Sample-7B-Q4_K_M.gguf");
        let q8 = dir.join("Sample-7B-Q8_0.gguf");
        let image = dir.join("mmproj-Sample-F16.gguf");
        let model = group_meta(None);
        let image_for_q4 = group_meta(Some(q4.as_path()));
        let image_for_q8 = group_meta(Some(q8.as_path()));
        let metas = [
            ("a", model.clone()),
            ("q8", model.clone()),
            ("img4", image_for_q4.clone()),
        ];
        let browse_q4 = |s: DownloadStatus| vec![row("a", &q4, 1, s), row("a", &image, 1, s)];

        // The same model again: that download is it, going on when it stopped, and started
        // afresh in its place when it failed.
        let running = browse_q4(S::Downloading);
        let both: [&Path; 2] = [&q4, &image];
        assert_eq!(
            plan(&model, &both, &running, &metas, &["a"]),
            Start::Join("a".into())
        );
        assert_eq!(
            plan(&model, &[&q4], &running, &metas, &["a"]),
            Start::Join("a".into())
        );
        let paused = browse_q4(S::Paused);
        assert_eq!(
            plan(&model, &both, &paused, &metas, &[]),
            Start::Resume("a".into())
        );
        let failed = vec![
            row("a", &q4, 1, S::Verifying),
            row("a", &image, 1, S::Failed),
        ];
        assert_eq!(
            plan(&model, &both, &failed, &metas, &[]),
            Start::Replace("a".into())
        );
        // Another model of the repository that shares the image file waits for it.
        assert_eq!(
            plan(&model, &[&q8, &image], &running, &metas, &["a"]),
            Start::Busy { running: true }
        );
        assert_eq!(plan(&model, &[&q8], &running, &metas, &["a"]), Start::New);

        // The image file for Q4 while a download of Q8 writes it: refused, and that download is
        // left as it is, running or paused.
        let browse_q8 = |s: DownloadStatus| vec![row("q8", &q8, 1, s), row("q8", &image, 1, s)];
        for (rows, running) in [
            (browse_q8(S::Paused), &[][..]),
            (browse_q8(S::Downloading), &["q8"][..]),
            (browse_q8(S::Failed), &[][..]),
        ] {
            assert_eq!(
                plan(&image_for_q4, &[&image], &rows, &metas, running),
                Start::Busy {
                    running: !running.is_empty()
                }
            );
        }
        // The image file for Q4 asked for again follows its own download.
        let img4 = |s: DownloadStatus| vec![row("img4", &image, 1, s)];
        let single: [&Path; 1] = [&image];
        assert_eq!(
            plan(
                &image_for_q4,
                &single,
                &img4(S::Downloading),
                &metas,
                &["img4"]
            ),
            Start::Join("img4".into())
        );
        assert_eq!(
            plan(&image_for_q4, &single, &img4(S::Paused), &metas, &[]),
            Start::Resume("img4".into())
        );
        assert_eq!(
            plan(&image_for_q4, &single, &img4(S::Failed), &metas, &[]),
            Start::Replace("img4".into())
        );
        // ... but not for another model, or a model download that writes that file.
        assert_eq!(
            plan(&image_for_q8, &single, &img4(S::Paused), &metas, &[]),
            Start::Busy { running: false }
        );
        assert_eq!(
            plan(&model, &[&q8, &image], &img4(S::Paused), &metas, &[]),
            Start::Busy { running: false }
        );

        // A done download writes nothing. P6b round 2: one left cancelled (its cancel did not
        // finish) still writes its files, so the same model goes on with it.
        let over = vec![row("a", &q4, 1, S::Completed)];
        assert_eq!(plan(&model, &[&q4], &over, &metas, &[]), Start::New);
        let left = vec![row("q8", &q4, 1, S::Cancelled)];
        assert_eq!(
            plan(&model, &[&q4], &left, &metas, &[]),
            Start::Resume("q8".into())
        );
        assert_eq!(plan(&model, &[&q4], &[], &metas, &[]), Start::New);
        // A download whose saved metadata cannot be read is left alone.
        let unknown = vec![row("lost", &q4, 1, S::Paused)];
        assert_eq!(
            plan(&model, &[&q4], &unknown, &metas, &[]),
            Start::Busy { running: false }
        );

        let err = busy_error(false, true);
        assert_eq!(err.code, "already_downloading");
        assert_eq!(
            err.message,
            "One of this model's files is already being downloaded."
        );
        assert!(err.next_step.contains("Downloads"));
        let err = busy_error(true, true);
        assert_eq!(
            err.message,
            "This image file is already being downloaded with another model. This model uses it too once that download is done."
        );
        // P6b review nit: a download that is paused or failed is not "being downloaded".
        let err = busy_error(false, false);
        assert_eq!(
            err.message,
            "One of this model's files is part of another download, which has stopped."
        );
        assert_eq!(
            err.next_step,
            "Resume or cancel that download in Downloads, then try again."
        );
        let err = busy_error(true, false);
        assert_eq!(
            err.message,
            "This image file is part of another model's download, which has stopped. This model uses it too once that download is done."
        );
        assert!(!err.message.contains("being downloaded"));
    }

    /// Checking and recording are one step: two starts at once (a double click on Download)
    /// record one download, and the second follows it.
    #[tokio::test]
    async fn two_starts_at_once_record_one_download() {
        let dir = Path::new("/models/example-quants/Sample-7B-GGUF");
        let files = vec![
            file(&dir.join("Sample-7B-Q4_K_M.gguf"), 100),
            file(&dir.join("mmproj-Sample-F16.gguf"), 50),
        ];
        let store = store::Store::open_in_memory().unwrap();
        let manager = DownloadManager::default();
        let meta = group_meta(None);
        let (a, b) = tokio::join!(
            manager.record(&store, &meta, &files),
            manager.record(&store, &meta, &files)
        );
        let groups: std::collections::BTreeSet<String> = store
            .list_downloads(None)
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.group_id)
            .collect();
        assert_eq!(groups.len(), 1, "one download");
        let id = groups.into_iter().next().unwrap();
        match (a.unwrap(), b.unwrap()) {
            (Recorded::New(first), Recorded::Resume(second))
            | (Recorded::Resume(second), Recorded::New(first)) => {
                assert_eq!((first, second), (id.clone(), id));
            }
            other => panic!("{other:?}"),
        }
    }

    /// P6b review m1: Delete of a model cancels the image file still downloading for it (and
    /// waits for that download to stop), so the new image file is not left behind; only that
    /// model's, not another model's or a model download, and not one that finished.
    #[tokio::test]
    async fn delete_cancels_only_the_image_file_downloading_for_that_model() {
        let dir = Scratch::new();
        let q4 = dir.path().join("Sample-7B-Q4_K_M.gguf");
        let q8 = dir.path().join("Sample-7B-Q8_0.gguf");
        let store = store::Store::open_in_memory().unwrap();
        let manager = DownloadManager::default();
        let new = |r: Recorded| match r {
            Recorded::New(g) => g,
            other => panic!("a new download, not {other:?}"),
        };
        let image_for_q4 = new(manager
            .record(
                &store,
                &group_meta(Some(q4.as_path())),
                &[file(&dir.path().join("mmproj-A-F16.gguf"), 50)],
            )
            .await
            .unwrap());
        new(manager
            .record(
                &store,
                &group_meta(Some(q8.as_path())),
                &[file(&dir.path().join("mmproj-B-F16.gguf"), 50)],
            )
            .await
            .unwrap());
        new(manager
            .record(
                &store,
                &group_meta(None),
                &[file(&dir.path().join("Sample-7B-BF16.gguf"), 100)],
            )
            .await
            .unwrap());
        let q4 = q4.display().to_string();
        assert_eq!(
            image_file_downloads_for(&store, &q4).await.unwrap(),
            vec![image_for_q4.clone()]
        );
        set_all(&store, &image_for_q4, S::Completed).await;
        assert!(
            image_file_downloads_for(&store, &q4)
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// P2 review m5, end to end on the saved downloads: the image file for Q4 while a paused
    /// download of Q8 writes it is refused and that download is left paused; once it is gone
    /// the image file starts; asked for again while paused it goes on; failed, a new download
    /// takes its place and keeps the partial file to go on from; and the image file for Q8
    /// meanwhile is refused.
    #[tokio::test]
    async fn the_image_file_joins_only_its_own_models_download() {
        let dir = Scratch::new();
        let q4 = dir.path().join("Sample-7B-Q4_K_M.gguf");
        let q8 = dir.path().join("Sample-7B-Q8_0.gguf");
        let image = dir.path().join("mmproj-Sample-F16.gguf");
        let store = store::Store::open_in_memory().unwrap();
        let manager = DownloadManager::default();
        let Recorded::New(browse_q8) = manager
            .record(
                &store,
                &group_meta(None),
                &[file(&q8, 100), file(&image, 50)],
            )
            .await
            .unwrap()
        else {
            panic!("a new download")
        };
        set_all(&store, &browse_q8, S::Paused).await;

        let for_q4 = group_meta(Some(q4.as_path()));
        let err = manager
            .record(&store, &for_q4, &[file(&image, 50)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "already_downloading");
        assert_eq!(
            statuses(&store, &browse_q8).await,
            vec![S::Paused, S::Paused]
        );

        store.delete_download_group(browse_q8).await.unwrap();
        let Recorded::New(img) = manager
            .record(&store, &for_q4, &[file(&image, 50)])
            .await
            .unwrap()
        else {
            panic!("a new download")
        };
        set_all(&store, &img, S::Paused).await;
        let again = manager
            .record(&store, &for_q4, &[file(&image, 50)])
            .await
            .unwrap();
        assert!(
            matches!(again, Recorded::Resume(ref g) if *g == img),
            "{again:?}"
        );
        assert_eq!(statuses(&store, &img).await, vec![S::Queued]);

        set_all(&store, &img, S::Failed).await;
        let part = PathBuf::from(format!("{}.part", image.display()));
        std::fs::write(&part, b"partial").unwrap();
        let Recorded::Replaced { new, old } = manager
            .record(&store, &for_q4, &[file(&image, 50)])
            .await
            .unwrap()
        else {
            panic!("a download in place of the failed one")
        };
        assert_ne!(new, img);
        assert_eq!(old.map(|d| d.status).as_deref(), Some("cancelled"));
        assert!(store.list_downloads(Some(img)).await.unwrap().is_empty());
        assert_eq!(statuses(&store, &new).await, vec![S::Queued]);
        assert!(part.exists(), "its partial file stays to go on from");

        let err = manager
            .record(&store, &group_meta(Some(q8.as_path())), &[file(&image, 50)])
            .await
            .unwrap_err();
        assert_eq!(err.code, "already_downloading");
        assert_eq!(statuses(&store, &new).await, vec![S::Queued]);
    }

    /// The bug of P6: Cancel deleted only the partial files, so a file the group had finished
    /// (often gigabytes) stayed on disk. P6b review C1: Cancel then deleted every whole file
    /// at a row's final name, also one the download only found there (another app's copy, a
    /// model taken off the list, a file copied in by hand) or one a failed fetch never
    /// replaced. Now it deletes only the files the group wrote itself, inside the models folder
    /// once links are followed, that no installed model uses; when unsure, the file stays.
    #[test]
    fn cancel_deletes_only_files_the_download_wrote() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let q8 = repo.join("Sample-7B-Q8_0.gguf");
        let image = repo.join("mmproj-Sample-F16.gguf");
        std::fs::write(&q8, vec![1u8; 100]).unwrap();
        std::fs::write(&image, vec![2u8; 50]).unwrap();
        let rows = vec![
            row("g", &q8, 100, S::Verifying),
            row("g", &image, 50, S::Verifying),
            row("g", &repo.join("Sample-7B-BF16.gguf"), 10, S::Downloading),
        ];
        let text = |p: &PathBuf| p.display().to_string();
        let both = vec![text(&q8), text(&image)];
        let root = Some(models.as_path());
        let used = |models: Vec<store::ModelRecord>| {
            move |p: &Path| crate::hub::used_by_installed(&models, p)
        };
        // Both written by the group, nothing installed uses them: both go.
        assert_eq!(
            files_to_delete_on_cancel(&rows, &both, root, used(Vec::new())),
            vec![q8.clone(), image.clone()]
        );
        // The image file was found in place (the same file already there): it stays.
        assert_eq!(
            files_to_delete_on_cancel(&rows, &[text(&q8)], root, used(Vec::new())),
            vec![q8.clone()]
        );
        assert!(files_to_delete_on_cancel(&rows, &[], root, used(Vec::new())).is_empty());
        // Another model of the repository is installed next to the image file (a load finds
        // it there), or has it recorded: the image file stays.
        let q4 = repo.join("Sample-7B-Q4_K_M.gguf");
        std::fs::write(&q4, vec![3u8; 10]).unwrap();
        assert_eq!(
            files_to_delete_on_cancel(&rows, &both, root, used(vec![installed(&q4, None)])),
            vec![q8.clone()]
        );
        // A model on the list downloaded again: its file stays.
        assert!(
            files_to_delete_on_cancel(&rows, &both, root, used(vec![installed(&q8, None)]))
                .is_empty()
        );
        // Every shard of a split model on the list stays.
        let shard2 = repo.join("Sample-7B-Q2_K-00002-of-00002.gguf");
        std::fs::write(&shard2, vec![4u8; 10]).unwrap();
        let split = installed(&repo.join("Sample-7B-Q2_K-00001-of-00002.gguf"), None);
        assert!(
            files_to_delete_on_cancel(
                &[row("g", &shard2, 10, S::Verifying)],
                &[text(&shard2)],
                root,
                used(vec![split])
            )
            .is_empty()
        );
        // Not whole (another size): not the file the group wrote.
        assert!(
            files_to_delete_on_cancel(&[row("g", &q8, 101, S::Verifying)], &both, root, |_| false)
                .is_empty()
        );
        // Written, then the last step failed: deleted. A failed row's older file that its
        // fetch never replaced stays, whatever its size.
        assert_eq!(
            files_to_delete_on_cancel(
                &[
                    last_step_failed(row("g", &q8, 100, S::Failed)),
                    row("g", &image, 50, S::Failed)
                ],
                &[text(&q8)],
                root,
                |_| false
            ),
            vec![q8.clone()]
        );
        // Outside the folder it may be in, or no folder known: nothing goes.
        let other = dir.path().join("elsewhere");
        std::fs::create_dir_all(&other).unwrap();
        assert!(
            files_to_delete_on_cancel(&rows, &both, Some(other.as_path()), |_| false).is_empty()
        );
        assert!(files_to_delete_on_cancel(&rows, &both, None, |_| false).is_empty());
    }

    /// P6b review C1, on the saved downloads: Cancel discards the partial files, deletes only
    /// the files the download wrote, and forgets the group; the same file found in place and a
    /// failed row's older file survive.
    #[tokio::test]
    async fn a_cancelled_download_keeps_what_it_did_not_write() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let wrote = repo.join("Sample-7B-Q8_0-00001-of-00002.gguf");
        let found = repo.join("mmproj-Sample-F16.gguf");
        let older = repo.join("Sample-7B-Q8_0-00002-of-00002.gguf");
        std::fs::write(&wrote, vec![1u8; 100]).unwrap();
        std::fs::write(&found, vec![2u8; 50]).unwrap();
        std::fs::write(&older, vec![3u8; 30]).unwrap();
        let part = PathBuf::from(format!("{}.part", older.display()));
        std::fs::write(&part, b"partial").unwrap();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &wrote, 100, S::Verifying).await;
        recorded(&store, "g", &found, 50, S::Verifying).await;
        recorded(&store, "g", &older, 30, S::Failed).await;
        note_written(&store, "g", &wrote.display().to_string())
            .await
            .unwrap();
        clean_up(&store, "g", &models, None).await.unwrap();
        assert!(!wrote.exists(), "the file it wrote goes");
        assert!(found.exists(), "the same file found in place stays");
        assert!(
            older.exists(),
            "the older file a failed fetch never replaced stays"
        );
        assert!(!part.exists(), "its partial file goes");
        assert!(
            store
                .list_downloads(Some("g".into()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(written_by(&store, "g").await.is_empty());
    }

    /// 0.1.0 Studio: *Download again* of a picture model fetches a damaged shared part again
    /// (FLUX.2 klein's text encoder, which Z-Image could share); cancelled, it never deletes a
    /// file an installed picture or video model uses.
    #[tokio::test]
    async fn a_cancelled_download_keeps_a_file_a_picture_model_uses() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let repo = models.join("unsloth").join("Qwen3-4B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let encoder = repo.join("Qwen3-4B-Q4_K_M.gguf");
        let diffusion = repo.join("flux-2-klein-4b-Q4_0.gguf");
        std::fs::write(&encoder, vec![1u8; 100]).unwrap();
        std::fs::write(&diffusion, vec![2u8; 50]).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        store
            .insert_gen_model(store::NewGenModel {
                id: "z-image-turbo-q4".into(),
                catalog_id: Some("z-image-turbo-q4".into()),
                name: "Z-Image Turbo".into(),
                kind: "image".into(),
                family: "z-image-turbo".into(),
                recipe_id: Some("z-image-turbo".into()),
                parts: serde_json::json!([{"role": "llm", "file": encoder.display().to_string()}]),
                size_bytes: 100,
                source: store::GenSource::Catalog,
            })
            .await
            .unwrap();
        recorded(&store, "g", &encoder, 100, S::Verifying).await;
        recorded(&store, "g", &diffusion, 50, S::Verifying).await;
        for file in [&encoder, &diffusion] {
            note_written(&store, "g", &file.display().to_string())
                .await
                .unwrap();
        }
        clean_up(&store, "g", &models, None).await.unwrap();
        assert!(encoder.exists(), "Z-Image uses it");
        assert!(!diffusion.exists(), "nothing else uses it");
    }

    /// P6b review C1: a file the download wrote through a folder linked to somewhere outside
    /// the models folder (another drive) stays on Cancel.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_cancelled_download_leaves_a_linked_folder_alone() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let outside = dir.path().join("other-drive/example-quants");
        std::fs::create_dir_all(&models).unwrap();
        std::fs::create_dir_all(outside.join("Sample-7B-GGUF")).unwrap();
        std::os::unix::fs::symlink(&outside, models.join("example-quants")).unwrap();
        let linked = models.join("example-quants/Sample-7B-GGUF/Sample-7B-Q8_0.gguf");
        std::fs::write(&linked, vec![1u8; 100]).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &linked, 100, S::Verifying).await;
        note_written(&store, "g", &linked.display().to_string())
            .await
            .unwrap();
        clean_up(&store, "g", &models, None).await.unwrap();
        assert!(linked.exists());
    }

    /// The bug: when adding the finished model to Installed (or recording an image file on
    /// its model) failed, the error went only to the log: the rows stayed "verifying", the
    /// group showed 100% with a Pause that did nothing, and it started again at every start
    /// of the app. Now its rows fail with the error in plain words, and Resume tries that
    /// step again without downloading anything.
    #[tokio::test]
    async fn a_failed_last_step_shows_its_error_and_resumes_without_downloading() {
        let dir = Scratch::new();
        let model = dir.path().join("Sample-7B-Q8_0.gguf");
        let image = dir.path().join("mmproj-Sample-F16.gguf");
        std::fs::write(&model, vec![0u8; 100]).unwrap();
        std::fs::write(&image, vec![0u8; 50]).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &model, 100, S::Verifying).await;
        recorded(&store, "g", &image, 50, S::Verifying).await;
        let cause = AppError::new(
            "not_gguf",
            "This file is not a GGUF model the app can read.",
            "Pick a .gguf file.",
            "bad magic",
        );
        mark_failed(&store, "g", &last_step_error(&group_meta(None), &cause))
            .await
            .unwrap();
        let now = statuses(&store, "g").await;
        assert_eq!(now, vec![S::Failed, S::Failed]);
        assert_eq!(status_name(group_status(&now, false)), "error");
        assert!(!resumes_at_start(&now), "it does not start again by itself");
        let rows = store.list_downloads(Some("g".into())).await.unwrap();
        let shown: AppError = serde_json::from_str(rows[0].error.as_deref().unwrap()).unwrap();
        assert_eq!(shown.code, "download_not_added");
        assert_eq!(
            shown.message,
            "The model was downloaded, but it could not be added to Installed. This file is not a GGUF model the app can read."
        );
        assert_eq!(
            shown.next_step,
            "Resume to try again. If it fails again, cancel the download."
        );
        assert_eq!(shown.details, "not_gguf: bad magic");

        requeue(&store, "g").await.unwrap();
        assert_eq!(
            statuses(&store, "g").await,
            vec![S::Verifying, S::Verifying]
        );
        let rows = store.list_downloads(Some("g".into())).await.unwrap();
        assert!(rows.iter().all(|r| r.error.is_none()));

        let image_step = last_step_error(
            &group_meta(Some(Path::new("/models/m.gguf"))),
            &AppError::internal("locked"),
        );
        assert_eq!(
            image_step.message,
            "The image file was downloaded, but it could not be added to its model. Something went wrong inside the app."
        );
        // 0.1.0 Studio: a picture or video model is added to Studio, not to Installed.
        let studio_step = last_step_error(
            &GroupMeta {
                generator: Some(klein()),
                ..group_meta(None)
            },
            &AppError::internal("locked"),
        );
        assert_eq!(
            studio_step.message,
            "The model was downloaded, but it could not be added to Studio. Something went wrong inside the app."
        );
    }

    /// P6b round 2 (verification H, H2, H3): a group left with a cancelled row (its cancel
    /// did not finish) still writes its files. Before, it counted as writing nothing: the same
    /// model, another model sharing the image file, or "Download the image file" started a
    /// second download of the same partial file, which the old group's cancel then deleted.
    /// Now the same model goes on with it and anything else is refused; and while a cancel
    /// runs (its marker saved), nothing follows that download or starts beside it.
    #[test]
    fn a_download_left_cancelled_still_writes_its_files() {
        let dir = Path::new("/models/example-quants/Sample-7B-GGUF");
        let q4 = dir.join("Sample-7B-Q4_K_M.gguf");
        let q8 = dir.join("Sample-7B-Q8_0.gguf");
        let image = dir.join("mmproj-Sample-F16.gguf");
        let model = group_meta(None);
        let metas = [("g1", model.clone())];
        let solo = vec![row("g1", &q4, 1, S::Cancelled)];
        assert_eq!(
            plan(&model, &[&q4], &solo, &metas, &[]),
            Start::Resume("g1".into())
        );
        let left = vec![
            row("g1", &q4, 1, S::Verifying),
            row("g1", &image, 1, S::Cancelled),
        ];
        assert_eq!(
            plan(&model, &[&q8, &image], &left, &metas, &[]),
            Start::Busy { running: false }
        );
        assert_eq!(
            plan(
                &group_meta(Some(q4.as_path())),
                &[&image],
                &left,
                &metas,
                &[]
            ),
            Start::Busy { running: false }
        );
        let both: [&Path; 2] = [&q4, &image];
        for new in [&model, &group_meta(Some(q8.as_path()))] {
            assert_eq!(
                plan_while_cancelling(new, &both, &left, &metas, &["g1"]),
                Start::Cancelling
            );
        }
        let err = cancelling_error(false);
        assert_eq!(err.code, "download_cancelling");
        assert_eq!(
            err.message,
            "One of this model's files is part of a download that is being cancelled."
        );
        assert_eq!(err.next_step, "Wait a moment, then try again.");
        assert_eq!(
            cancelling_error(true).message,
            "This image file is part of a download that is being cancelled."
        );
    }

    /// P6b round 2 (verification H): a cancel deletes only its own partial files, never one
    /// another download not finished yet writes (a download of the same file that started
    /// while this one's cancel had not finished), and it forgets its marker.
    #[tokio::test]
    async fn a_cancel_never_deletes_another_downloads_partial_file() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let repo = models.join("example-quants/Sample-7B-GGUF");
        std::fs::create_dir_all(&repo).unwrap();
        let shared = repo.join("Solo-Q4_K_M.gguf");
        let own = repo.join("Own-Q4_K_M.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g1", &shared, 4000, S::Cancelled).await;
        recorded(&store, "g1", &own, 10, S::Cancelled).await;
        recorded(&store, "g2", &shared, 4000, S::Downloading).await;
        store
            .set_setting(&cancelling_key("g1"), &true)
            .await
            .unwrap();
        let part = |p: &Path| PathBuf::from(format!("{}.part", p.display()));
        std::fs::write(part(&shared), vec![1u8; 3000]).unwrap();
        std::fs::write(part(&own), vec![1u8; 5]).unwrap();
        clean_up(&store, "g1", &models, None).await.unwrap();
        assert!(
            part(&shared).exists(),
            "the partial file another download writes stays"
        );
        assert!(!part(&own).exists(), "its own partial file goes");
        assert!(
            store
                .list_downloads(Some("g1".into()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!is_cancelling(&store, "g1").await);
        assert_eq!(statuses(&store, "g2").await, vec![S::Downloading]);
    }

    /// P6b round 2 (verification H4): Resume of a download while another download writes one
    /// of its files is refused (both would append to one partial file), and a download being
    /// cancelled cannot be resumed.
    #[tokio::test]
    async fn resume_refuses_while_another_download_writes_its_file() {
        let dir = Scratch::new();
        let solo = dir.path().join("Solo-Q4_K_M.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g1", &solo, 4000, S::Cancelled).await;
        recorded(&store, "g2", &solo, 4000, S::Downloading).await;
        let manager = DownloadManager::default();
        let err = manager.ready_to_resume(&store, "g1").await.unwrap_err();
        assert_eq!(err.code, "already_downloading");
        assert_eq!(statuses(&store, "g1").await, vec![S::Cancelled]);

        let other = dir.path().join("Other-Q4_K_M.gguf");
        recorded(&store, "g3", &other, 10, S::Paused).await;
        store
            .set_setting(&cancelling_key("g3"), &true)
            .await
            .unwrap();
        let err = manager.ready_to_resume(&store, "g3").await.unwrap_err();
        assert_eq!(err.code, "download_cancelling");
        assert_eq!(statuses(&store, "g3").await, vec![S::Paused]);
    }

    /// P6b round 2 (verification K): Delete of a model cancels its image file's download, and
    /// that cancel deletes the image file the download wrote although the model being deleted
    /// would find it next to itself. A plain Cancel keeps it: the model uses it at its next
    /// load.
    #[tokio::test]
    async fn delete_removes_the_image_file_its_cancelled_download_wrote() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let folder = models.join("k/K-GGUF");
        std::fs::create_dir_all(&folder).unwrap();
        let model = folder.join("K-Q4.gguf");
        let image = folder.join("mmproj-K-F16.gguf");
        std::fs::write(&model, vec![1u8; 100]).unwrap();
        std::fs::write(&image, vec![2u8; 50]).unwrap();
        let model_text = model.display().to_string();
        let store = store::Store::open_in_memory().unwrap();
        store
            .upsert_model(store::ModelInput {
                path: model_text.clone(),
                repo: Some("k/K-GGUF".into()),
                file_name: "K-Q4.gguf".into(),
                size_bytes: 100,
                sha256: None,
                gguf_summary: serde_json::json!({}),
                source: ModelSource::Download,
            })
            .await
            .unwrap();
        let image_download = |group: &'static str| {
            let store = &store;
            let image = &image;
            let meta = group_meta(Some(model.as_path()));
            async move {
                store.set_setting(&meta_key(group), &meta).await.unwrap();
                recorded(store, group, image, 50, S::Verifying).await;
                note_written(store, group, &image.display().to_string())
                    .await
                    .unwrap();
            }
        };
        image_download("plain").await;
        clean_up(&store, "plain", &models, None).await.unwrap();
        assert!(
            image.exists(),
            "a plain Cancel keeps it for the model next to it"
        );
        image_download("delete").await;
        clean_up(&store, "delete", &models, Some(&model_text))
            .await
            .unwrap();
        assert!(
            !image.exists(),
            "Delete's cancel removes the image file it wrote"
        );
        assert!(model.exists());
    }

    /// The fake task of a group for the tests of Cancel: its stop token and its `ended`.
    fn fake_task(manager: &DownloadManager, group: &str) -> (CancellationToken, CancellationToken) {
        let (token, ended) = (CancellationToken::new(), CancellationToken::new());
        manager.running.lock().unwrap().insert(
            group.into(),
            Running {
                token: token.clone(),
                paused: false,
                ended: ended.clone(),
                id: 0,
            },
        );
        (token, ended)
    }

    /// P6b round 2 (verification items 4 and 6): Cancel saves its marker and stops the task,
    /// says "cancelling" at once, and waits for the task without holding up other downloads
    /// (before, every Download, Resume, Cancel and Delete waited up to 30 s behind it). Once
    /// the task has ended the download goes, with its marker.
    #[tokio::test]
    async fn cancel_marks_the_download_and_lets_others_go_on_while_it_waits() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let model = models.join("Sample-7B-Q8_0.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &model, 100, S::Downloading).await;
        let manager = DownloadManager::default();
        let (token, ended) = fake_task(&manager, "g");
        let shown = Arc::new(StdMutex::new(Vec::<String>::new()));
        let seen = shown.clone();
        let cancel = manager.cancel_core(
            &store,
            &models,
            "g",
            None,
            Duration::from_secs(10),
            move |d: DownloadGroupDto| seen.lock().unwrap().push(d.status),
        );
        tokio::pin!(cancel);
        assert!(
            tokio::time::timeout(Duration::from_millis(200), &mut cancel)
                .await
                .is_err(),
            "it waits for the task"
        );
        assert!(token.is_cancelled());
        assert!(is_cancelling(&store, "g").await);
        assert!(
            manager.starting.try_lock().is_ok(),
            "a start or resume goes on meanwhile"
        );
        assert_eq!(
            shown.lock().unwrap().clone(),
            vec!["cancelling".to_string()]
        );
        manager.running.lock().unwrap().remove("g");
        ended.cancel();
        cancel.await.unwrap();
        assert!(
            store
                .list_downloads(Some("g".into()))
                .await
                .unwrap()
                .is_empty()
        );
        assert!(!is_cancelling(&store, "g").await);
        assert_eq!(
            shown.lock().unwrap().last().map(String::as_str),
            Some("cancelled")
        );
    }

    /// P6b round 2 (verification items 3 and 4): a cancel whose task did not end in time
    /// leaves the download "cancelling" (not "downloading" or "paused"), with its rows, and the
    /// next start finishes it whatever its rows say: the last file had finished (nothing was
    /// marked cancelled), or the task was stuck (still "downloading"). A download that was
    /// done before the cancel could stop it is done.
    #[tokio::test]
    async fn a_cancel_cut_short_says_cancelling_and_is_finished_at_the_next_start() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        std::fs::create_dir_all(&models).unwrap();
        let model = models.join("Sample-7B-Q8_0.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "g", &model, 100, S::Downloading).await;
        let manager = DownloadManager::default();
        let _task = fake_task(&manager, "g");
        let err = manager
            .cancel_core(
                &store,
                &models,
                "g",
                None,
                Duration::from_millis(50),
                |_| {},
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "download_stopping");
        assert!(is_cancelling(&store, "g").await);
        manager.running.lock().unwrap().remove("g");
        let dto = manager.group_dto(&store, "g").await.unwrap().unwrap();
        assert_eq!(dto.status, "cancelling");
        assert_eq!(statuses(&store, "g").await, vec![S::Downloading]);
        assert_eq!(at_start(&[S::Verifying], true), AtStart::FinishCancel);
        assert_eq!(at_start(&[S::Downloading], true), AtStart::FinishCancel);
        assert_eq!(
            at_start(&[S::Verifying, S::Downloading], false),
            AtStart::Resume
        );
        set_all(&store, "g", S::Completed).await;
        let dto = manager.group_dto(&store, "g").await.unwrap().unwrap();
        assert_eq!(dto.status, "done");
    }

    /// P6b round 2 (verification item 7): a group's task that panics still ends: its entry
    /// goes and `ended` fires, so Cancel does not answer "still stopping" until the app
    /// restarts. The end of a task never takes off a newer task of the same group.
    #[test]
    fn a_task_that_panics_still_ends() {
        let manager = Arc::new(DownloadManager::default());
        let ended = CancellationToken::new();
        manager.running.lock().unwrap().insert(
            "g".into(),
            Running {
                token: CancellationToken::new(),
                paused: false,
                ended: ended.clone(),
                id: 7,
            },
        );
        manager
            .live
            .lock()
            .unwrap()
            .insert("g".into(), Live::default());
        let end = TaskEnd::new(manager.clone(), "g".into(), 7, ended.clone());
        let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _end = end;
            panic!("a bug in a download task");
        }));
        assert!(panicked.is_err());
        assert!(!manager.is_running("g"));
        assert!(ended.is_cancelled());
        assert!(!manager.live.lock().unwrap().contains_key("g"));

        let (newer, _) = fake_task(&manager, "g");
        drop(TaskEnd::new(
            manager.clone(),
            "g".into(),
            7,
            CancellationToken::new(),
        ));
        assert!(manager.is_running("g"), "the newer task keeps its entry");
        assert!(!newer.is_cancelled());
    }

    /// P6b round 2 (verification item 9, Windows): `dunce` keeps the verbatim prefix for paths
    /// over 260 characters, so a long written file and a short models folder (or the other way
    /// round) still compare; elsewhere only `/` separates, so a backslash in a name is no way
    /// out of the folder.
    #[test]
    fn a_long_windows_path_is_inside_its_folder() {
        for (file, root) in [
            (r"\\?\C:\Users\me\models\a\b.gguf", r"C:\Users\me\models"),
            (r"C:\Users\me\models\a\b.gguf", r"\\?\C:\Users\me\models"),
            (r"\\?\C:\Users\me\models\b.gguf", r"\\?\C:\Users\me\models\"),
            (
                r"\\?\UNC\server\share\models\b.gguf",
                r"\\server\share\models",
            ),
            (
                r"\\server\share\models\b.gguf",
                r"\\?\UNC\server\share\models",
            ),
            (r"C:\b.gguf", r"C:\"),
        ] {
            assert!(inside_by(file, root, true), "{file} in {root}");
        }
        for (file, root) in [
            (r"\\?\C:\Users\me\models-old\b.gguf", r"C:\Users\me\models"),
            (r"C:\Users\me\other\b.gguf", r"\\?\C:\Users\me\models"),
            (
                r"\\?\UNC\server\share\other\b.gguf",
                r"\\server\share\models",
            ),
        ] {
            assert!(!inside_by(file, root, true), "{file} not in {root}");
        }
        assert!(inside_by(
            "/home/me/models/a/b.gguf",
            "/home/me/models",
            false
        ));
        assert!(inside_by(
            "/home/me/models/b.gguf",
            "/home/me/models/",
            false
        ));
        assert!(inside_by("/b.gguf", "/", false));
        assert!(!inside_by(
            "/home/me/models\\evil/b.gguf",
            "/home/me/models",
            false
        ));
        assert!(!inside_by(
            "/home/me/models-old/b.gguf",
            "/home/me/models",
            false
        ));
    }

    // ---- 0.1.0 Studio: picture and video models ---------------------------------------------

    /// A model download's metadata.
    fn meta() -> GroupMeta {
        group_meta(None)
    }

    /// A picture model's group with a part per `(repo, path, revision)`: the first is the
    /// diffusion model, then the text encoder, then the decoder.
    fn group_with(parts: &[(&str, &str, String)]) -> GenGroup {
        let roles = ["diffusion", "llm", "vae"];
        GenGroup {
            gen_model_id: "flux2-klein-4b-q4".into(),
            catalog_id: Some("flux2-klein-4b-q4".into()),
            name: "FLUX.2 klein 4B".into(),
            kind: "image".into(),
            family: "flux2-klein-4b".into(),
            source: "catalog".into(),
            parts: parts
                .iter()
                .zip(roles.iter().cycle())
                .zip(1u64..)
                .map(|(((repo, path, revision), role), n)| GenGroupPart {
                    role: (*role).into(),
                    repo: (*repo).into(),
                    revision: revision.clone(),
                    path: (*path).into(),
                    size: 1_000 * n,
                    sha256: format!("{n:x}").repeat(64),
                    license: "apache-2.0".into(),
                    folder: None,
                    local: None,
                    present: None,
                })
                .collect(),
            recipe_id: None,
            untested: false,
        }
    }

    /// FLUX.2 klein's three parts, from three repositories at three revisions. The encoder and
    /// the decoder have the same file name in their repositories, as two repositories may.
    fn klein() -> GenGroup {
        group_with(&[
            (
                "leejet/FLUX.2-klein-4B-GGUF",
                "flux-2-klein-4b-Q4_0.gguf",
                "a".repeat(40),
            ),
            ("unsloth/Qwen3-4B-GGUF", "model.gguf", "b".repeat(40)),
            ("Comfy-Org/Flux-2-Klein-4B", "model.gguf", "d".repeat(40)),
        ])
    }

    /// The group's row for `part`, written to its place in `models` with `status`.
    fn part_row(models: &Path, part: &GenGroupPart, status: DownloadStatus) -> store::Download {
        let dest = models::download::destination(models, &part.repo, &part.path).unwrap();
        store::Download {
            repo: part.repo.clone(),
            file: part.path.clone(),
            sha256: part.sha256.clone(),
            ..row("g", &dest, part.size, status)
        }
    }

    /// The downloads table has no commit column: each part of a picture or video model is
    /// fetched at its own revision, and every other row at its group's.
    #[test]
    fn generator_parts_download_at_their_own_revisions() {
        let meta = GroupMeta {
            commit: "c".repeat(40),
            generator: Some(group_with(&[(
                "unsloth/Qwen3-4B-GGUF",
                "q.gguf",
                "a".repeat(40),
            )])),
            ..meta()
        };
        assert_eq!(
            row_commit(&meta, "unsloth/Qwen3-4B-GGUF", "q.gguf"),
            "a".repeat(40)
        );
        assert_eq!(
            row_commit(
                &GroupMeta {
                    generator: None,
                    ..meta.clone()
                },
                "unsloth/Qwen3-4B-GGUF",
                "q.gguf"
            ),
            "c".repeat(40)
        );
        // A file of another repository with the same name is not that part.
        assert_eq!(
            row_commit(&meta, "someone/Qwen3-4B-GGUF", "q.gguf"),
            "c".repeat(40)
        );
    }

    /// A picture or video model joins Studio only when every part finished with its pinned
    /// SHA-256; each part is recorded with its own file on disk, found by its repository and
    /// path (two repositories may hold files of the same name).
    #[test]
    fn a_generator_group_registers_only_when_every_part_is_verified() {
        let models = Path::new("/m");
        let group = klein();
        let meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let rows = |statuses: [DownloadStatus; 3]| -> Vec<store::Download> {
            group
                .parts
                .iter()
                .zip(statuses)
                .map(|(p, s)| part_row(models, p, s))
                .collect()
        };
        let incomplete = |rows: &[store::Download]| {
            let e = gen_model_from(&meta, rows).unwrap_err();
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                (
                    "studio_model_incomplete",
                    "A part of this model is not downloaded yet."
                )
            );
        };
        incomplete(&rows([S::Verifying, S::Verifying, S::Downloading]));
        incomplete(&rows([S::Verifying, S::Paused, S::Verifying]));
        incomplete(&rows([S::Verifying, S::Verifying, S::Verifying])[..2]);
        let mut unchecked = rows([S::Verifying, S::Verifying, S::Verifying]);
        unchecked[1].sha256 = String::new();
        incomplete(&unchecked);
        let mut other = rows([S::Verifying, S::Verifying, S::Verifying]);
        other[2].sha256 = "f".repeat(64);
        incomplete(&other);

        let new = gen_model_from(&meta, &rows([S::Verifying, S::Completed, S::Verifying])).unwrap();
        let dest = |p: &GenGroupPart| {
            models::download::destination(models, &p.repo, &p.path)
                .unwrap()
                .display()
                .to_string()
        };
        assert_eq!(
            (
                new.id.as_str(),
                new.catalog_id.as_deref(),
                new.name.as_str(),
                new.kind.as_str(),
                new.family.as_str(),
                new.recipe_id.as_deref(),
                new.size_bytes,
                new.source,
            ),
            (
                "flux2-klein-4b-q4",
                Some("flux2-klein-4b-q4"),
                "FLUX.2 klein 4B",
                "image",
                "flux2-klein-4b",
                Some("flux2-klein-4b"),
                6_000,
                store::GenSource::Catalog,
            )
        );
        let parts = new.parts.as_array().unwrap();
        assert_eq!(parts.len(), 3);
        for (saved, part) in parts.iter().zip(&group.parts) {
            assert_eq!(saved["file"], dest(part), "{part:?}");
            assert_eq!(saved["revision"], part.revision.as_str());
            assert_eq!(saved["sha256"], part.sha256.as_str());
            assert_eq!(saved["role"], part.role.as_str());
            assert_eq!(saved["license"], "apache-2.0");
        }
        assert_ne!(
            parts[1]["file"], parts[2]["file"],
            "same name, two repositories"
        );

        // A searched model has no catalog entry.
        let searched = GroupMeta {
            generator: Some(GenGroup {
                catalog_id: None,
                source: "search".into(),
                ..group.clone()
            }),
            ..meta.clone()
        };
        let new = gen_model_from(&searched, &rows([S::Verifying; 3])).unwrap();
        assert_eq!(
            (new.catalog_id, new.recipe_id.as_deref(), new.source),
            (None, Some("flux2-klein-4b"), store::GenSource::Search)
        );
        // A family this build does not know has no recipe (registering it is refused).
        let unknown = GroupMeta {
            generator: Some(GenGroup {
                family: "a-later-family".into(),
                ..group.clone()
            }),
            ..meta.clone()
        };
        assert_eq!(
            gen_model_from(&unknown, &rows([S::Verifying; 3]))
                .unwrap()
                .recipe_id,
            None
        );
        assert_eq!(
            gen_model_from(&group_meta(None), &[]).unwrap_err().code,
            "internal"
        );
    }

    /// A group saved before 0.1.0 has no picture or video model, and a group without one
    /// leaves the field out of its JSON.
    #[test]
    fn groups_saved_before_this_version_still_read() {
        let old: GroupMeta = serde_json::from_value(serde_json::json!({
            "repo": "example-quants/Sample-7B-GGUF",
            "commit": "4444444444444444444444444444444444444444",
            "label": "Q4_K_M",
            "modelFiles": ["Sample-7B-Q4_K_M.gguf"],
            "projector": null,
            "catalogId": null
        }))
        .unwrap();
        assert_eq!(old.generator, None);
        assert!(
            serde_json::to_value(&old)
                .unwrap()
                .get("generator")
                .is_none()
        );
        let new = GroupMeta {
            generator: Some(klein()),
            ..old
        };
        let json = serde_json::to_value(&new).unwrap();
        assert_eq!(json["generator"]["genModelId"], "flux2-klein-4b-q4");
        assert_eq!(json["generator"]["parts"][1]["revision"], "b".repeat(40));
        let back: GroupMeta = serde_json::from_value(json).unwrap();
        assert_eq!(back.generator, Some(klein()));
    }

    /// The bug this prevents: a chat download of Qwen3-4B (FLUX.2 klein's text encoder) joined
    /// a running klein download that writes the same file, and so never reached Installed. A
    /// download joins only one of the same model.
    #[test]
    fn a_chat_download_never_joins_a_picture_model_download() {
        let models = Path::new("/m");
        let group = klein();
        let klein_meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let rows: Vec<store::Download> = group
            .parts
            .iter()
            .map(|p| store::Download {
                group_id: "klein".into(),
                ..part_row(models, p, S::Downloading)
            })
            .collect();
        let dests: Vec<String> = rows.iter().map(|r| r.dest_path.clone()).collect();
        let encoder = [dests[1].clone()];
        let metas = [("klein", klein_meta.clone())];
        assert_eq!(
            plan(&meta(), &encoder, &rows, &metas, &["klein"]),
            Start::Busy { running: true }
        );
        assert_eq!(
            plan(&klein_meta, &dests, &rows, &metas, &["klein"]),
            Start::Join("klein".into())
        );
    }

    /// Downloads labels a picture or video model's group by what it makes, and the group says
    /// which installed model it downloads (Get local models shows its progress on that row).
    #[tokio::test]
    async fn a_picture_or_video_models_download_says_what_it_makes() {
        let dir = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        let manager = DownloadManager::default();
        recorded(
            &store,
            "chat",
            &dir.path().join("m.gguf"),
            100,
            S::Downloading,
        )
        .await;
        recorded(
            &store,
            "clip",
            &dir.path().join("v.gguf"),
            100,
            S::Downloading,
        )
        .await;
        store.set_setting(&meta_key("chat"), &meta()).await.unwrap();
        let video = GroupMeta {
            generator: Some(GenGroup {
                kind: "video".into(),
                ..klein()
            }),
            ..meta()
        };
        store.set_setting(&meta_key("clip"), &video).await.unwrap();
        let chat = manager.group_dto(&store, "chat").await.unwrap().unwrap();
        let clip = manager.group_dto(&store, "clip").await.unwrap().unwrap();
        assert_eq!(chat.generator_kind, None);
        assert_eq!(clip.generator_kind.as_deref(), Some("video"));
        assert_eq!(
            serde_json::to_value(&clip).unwrap()["generatorKind"],
            "video"
        );
        assert_eq!(
            serde_json::to_value(&chat).unwrap()["generatorKind"],
            serde_json::Value::Null
        );
        assert_eq!(chat.gen_model_id, None);
        assert_eq!(clip.gen_model_id.as_deref(), Some("flux2-klein-4b-q4"));
        assert_eq!(
            serde_json::to_value(&clip).unwrap()["genModelId"],
            "flux2-klein-4b-q4"
        );
        assert_eq!(
            serde_json::to_value(&chat).unwrap()["genModelId"],
            serde_json::Value::Null
        );
    }

    #[tokio::test]
    async fn a_standalone_file_keeps_its_destination_across_a_restart() {
        let dir = Scratch::new();
        let path = dir.path().join("weights.gguf");
        let store = store::Store::open_in_memory().unwrap();
        recorded(&store, "raw", &path, 100, S::Completed).await;
        let meta = GroupMeta {
            file_only: true,
            ..meta()
        };
        store.set_setting(&meta_key("raw"), &meta).await.unwrap();
        let group = DownloadManager::default()
            .group_dto(&store, "raw")
            .await
            .unwrap()
            .unwrap();
        let saved = group.saved_file.unwrap();
        assert_eq!(saved.commit, meta.commit);
        assert_eq!(saved.path, path.display().to_string());
        assert_eq!(group.status, "done");
        assert!(group.generator_kind.is_none() && group.gen_model_id.is_none());
    }

    /// No recommended model's download is refused as another model: each recommended entry
    /// whose diffusion file had its tensor table recorded (the fixtures' README in
    /// crates/models/tests/fixtures/gguf/ names each table's repository, revision and file) is
    /// detected as a model its recipe runs, by the check `register_gen_model` makes. An entry
    /// whose file was recorded at another revision fails: its table is recorded again.
    #[test]
    fn every_recommended_diffusion_file_is_its_own_family() {
        use models::gguf::testing::{diffusion_gguf, shapes};
        let readme = include_str!("../../crates/models/tests/fixtures/gguf/README.md");
        // `| `<fixture>` | <repo> @ <revision>, `<file>` … |`
        let recorded: Vec<(&str, &str, &str, &str)> = readme
            .lines()
            .filter_map(|line| {
                let (fixture, rest) = line.strip_prefix("| `")?.split_once("` | ")?;
                let (repo, rest) = rest.split_once(" @ ")?;
                let (revision, rest) = rest.split_once(", `")?;
                let (file, _) = rest.split_once('`')?;
                Some((fixture, repo, revision, file))
            })
            .collect();
        assert!(
            recorded.len() >= 5,
            "the README's table reads: {recorded:?}"
        );
        let catalog = models::gen_catalog::gen_catalog(
            &models::catalog::Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        );
        let dir = Scratch::new();
        let mut checked = Vec::new();
        for (id, entry) in &catalog.entries {
            let part = entry.diffusion();
            let Some(&(fixture, _, revision, _)) = recorded
                .iter()
                .find(|(_, repo, _, file)| *repo == part.repo && *file == part.path)
            else {
                continue;
            };
            assert_eq!(
                revision, part.revision,
                "{id}: its table was recorded at another revision; record it again"
            );
            assert!(!shapes(fixture).is_empty(), "{id}");
            let file = dir.path().join(format!("{id}.gguf"));
            std::fs::write(&file, diffusion_gguf(fixture)).unwrap();
            check_main_file(entry, &file).unwrap_or_else(|e| panic!("{id}: {e:?}"));
            checked.push(id.as_str());
        }
        assert!(
            !checked.is_empty(),
            "no recommended entry's diffusion file has a recorded table"
        );
    }

    /// The bundled catalog's entry `id`.
    fn catalog_entry(id: &str) -> GenEntry {
        models::gen_catalog::gen_catalog(
            &models::catalog::Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        )
        .entries
        .remove(id)
        .unwrap()
    }

    /// The tiny model file of 0.1.x's recorded table `<name>-tensors.txt`, written in `dir`.
    fn write_gguf_txt_fixture(dir: &Scratch, name: &str) -> PathBuf {
        let path = dir.path().join(format!("{name}.gguf"));
        std::fs::write(
            &path,
            models::gguf::testing::diffusion_gguf(&format!("{name}-tensors.txt")),
        )
        .unwrap();
        path
    }

    /// A `.safetensors` file at `path` whose header holds `tensors` (no data).
    fn write_safetensors(path: &Path, tensors: &[(&str, Vec<u64>)]) {
        let header: serde_json::Map<String, serde_json::Value> = tensors
            .iter()
            .map(|(name, shape)| {
                let entry =
                    serde_json::json!({"dtype": "F16", "shape": shape, "data_offsets": [0, 0]});
                ((*name).to_string(), entry)
            })
            .collect();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            path,
            models::sd::testing::safetensors(&serde_json::Value::Object(header)),
        )
        .unwrap();
    }

    /// An SDXL checkpoint without the middle block's attention and the output block 3.1's
    /// second transformer block: the engine detects Segmind Vega (row 48).
    fn sdxl_vega_names() -> Vec<(&'static str, Vec<u64>)> {
        vec![
            (
                "model.diffusion_model.input_blocks.0.0.weight",
                vec![320, 4, 3, 3],
            ),
            (
                "conditioner.embedders.1.model.token_embedding.weight",
                vec![49408, 1280],
            ),
            (
                "conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight",
                vec![49408, 768],
            ),
        ]
    }

    /// A generic entry of `recipe_id` with one `model` part.
    fn generic(recipe_id: &str, version: Option<models::sd::detect::SdVersion>) -> GenEntry {
        models::gen_catalog::generic_entry(GenericSpec {
            id: "local:x".into(),
            name: "x".into(),
            publisher: String::new(),
            recipe: models::sd::recipes::recipe(recipe_id).unwrap(),
            version,
            parts: Vec::new(),
            licenses: Default::default(),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
            untested: false,
        })
    }

    /// 0.1.x's family signatures are the engine's detection now: a file of the entry's family
    /// passes and one of another family does not, and a generic entry takes any version its
    /// recipe serves.
    #[test]
    fn the_01x_family_signature_check_now_uses_detection() {
        use models::sd::detect::SdVersion;
        let dir = Scratch::new();
        let klein = catalog_entry("flux2-klein-4b-q4");
        assert!(check_main_file(&klein, &write_gguf_txt_fixture(&dir, "klein4b")).is_ok());
        assert_eq!(
            check_main_file(&klein, &write_gguf_txt_fixture(&dir, "zimage"))
                .unwrap_err()
                .code,
            studio_errors::NOT_THAT_MODEL
        );
        // The 9B, of the same version, is told from the 4B by its width.
        assert_eq!(
            check_main_file(&klein, &write_gguf_txt_fixture(&dir, "klein9b"))
                .unwrap_err()
                .code,
            studio_errors::NOT_THAT_MODEL
        );
        let vega = dir.path().join("vega.safetensors");
        write_safetensors(&vega, &sdxl_vega_names());
        assert!(matches!(
            models::sd::inspect::inspect_local(&vega).unwrap().kind,
            FileKind::Model {
                version: SdVersion::SdxlVega,
                ..
            }
        ));
        let sdxl = generic("sdxl-checkpoint", Some(SdVersion::Sdxl));
        // sdxl-vega takes the sdxl-checkpoint recipe.
        assert!(check_main_file(&sdxl, &vega).is_ok());
        // A generic entry is not held to a name: any file of a version its recipe serves.
        let klein9b = generic("flux2-klein-4b", Some(SdVersion::Flux2Klein));
        assert!(check_main_file(&klein9b, &write_gguf_txt_fixture(&dir, "klein9b")).is_ok());
        for (name, why) in [
            ("klein4b", "another version"),
            ("qwen3-4b", "not a picture model"),
        ] {
            let e = check_main_file(&sdxl, &write_gguf_txt_fixture(&dir, name)).unwrap_err();
            assert_eq!(e.code, studio_errors::NOT_THAT_MODEL, "{why}");
        }
    }

    /// The recorded table of a model of each 0.1.x family (0.1.x's signature fixtures).
    fn recorded_table(family: GenFamily) -> &'static str {
        match family {
            GenFamily::Flux2Klein4b => "klein4b",
            GenFamily::Flux3ImageKlein => "flux3-klein",
            GenFamily::ZImageTurbo => "zimage",
            GenFamily::Wan22Ti2v5b | GenFamily::Wan22Ti2v5bFast => "wan22-ti2v-5b",
            GenFamily::Wan21T2v1_3b => "wan21-t2v-1.3b",
        }
    }

    /// A searched file of `family` whose repository and name say nothing of it (found by its
    /// `base_model` tag): its row as 0.1.2 installed it (schema 4's recipe by family) and the
    /// finished download that makes it, its diffusion part beside the family's companions.
    fn neutral_search(family: GenFamily) -> (store::GenModel, store::NewGenModel) {
        let catalog = models::gen_catalog::gen_catalog(
            &models::catalog::Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        );
        let record = &catalog.families[&family];
        let diffusion = serde_json::json!({
            "role": "diffusion", "repo": "someone/my-quants", "revision": "c".repeat(40),
            "path": "weights.gguf", "file": "/models/someone/my-quants/weights.gguf",
            "size": 4096, "sha256": "d".repeat(64), "license": record.upstream_license,
        });
        let companions = record.companions.iter().map(|p| {
            let mut v = serde_json::to_value(p).unwrap();
            v["file"] = format!("/models/{}/{}", p.repo, p.path).into();
            v
        });
        let parts =
            serde_json::Value::Array(std::iter::once(diffusion).chain(companions).collect());
        let family_word = serde_json::to_value(family)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned();
        let new = store::NewGenModel {
            id: "hf:someone/my-quants:weights.gguf".into(),
            catalog_id: None,
            name: record.name.clone(),
            kind: "image".into(),
            family: family_word,
            recipe_id: Some(family.recipe_id().into()),
            parts,
            size_bytes: 4096,
            source: store::GenSource::Search,
        };
        let row = store::GenModel {
            id: new.id.clone(),
            catalog_id: None,
            name: new.name.clone(),
            kind: new.kind.clone(),
            family: new.family.clone(),
            recipe_id: new.recipe_id.clone(),
            parts: new.parts.clone(),
            size_bytes: new.size_bytes,
            source: new.source,
            added_at: 1,
        };
        (row, new)
    }

    /// Every 0.1.x family's model passes the check under a name that says nothing of it, both
    /// as a finished download (`register_gen_model`'s entry) and as an installed row (each
    /// run's): the name the matching reads is the family's recipe, so only the file's own facts
    /// can disagree. A 9B klein is still refused for the 4B.
    #[test]
    fn every_01x_family_passes_with_its_recorded_table_under_a_neutral_name() {
        let dir = Scratch::new();
        let catalog = models::gen_catalog::gen_catalog(
            &models::catalog::Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        );
        for family in GenFamily::ALL {
            let file = write_gguf_txt_fixture(&dir, recorded_table(family));
            let (row, new) = neutral_search(family);
            let installed =
                crate::studio::local_models::entry_from_installed(&row, &catalog).unwrap();
            let downloaded = downloaded_entry(&new, false).unwrap();
            for entry in [&installed, &downloaded] {
                assert_eq!(
                    (entry.family, entry.recipe.as_str()),
                    (Some(family), family.recipe_id()),
                    "{family:?}"
                );
                assert_eq!(entry.diffusion().path, "weights.gguf");
                check_main_file(entry, &file).unwrap_or_else(|e| panic!("{family:?}: {e:?}"));
            }
        }
        let (row, new) = neutral_search(GenFamily::Flux2Klein4b);
        let klein9b = write_gguf_txt_fixture(&dir, "klein9b");
        for entry in [
            crate::studio::local_models::entry_from_installed(&row, &catalog).unwrap(),
            downloaded_entry(&new, false).unwrap(),
        ] {
            assert_eq!(
                check_main_file(&entry, &klein9b).unwrap_err().code,
                studio_errors::NOT_THAT_MODEL
            );
        }
    }

    /// A Z-Image Turbo file from a repository that never says `turbo` (0.1.x recognised it by
    /// its `base_model` tag) is still Z-Image Turbo: its download is added, and its 0.1.2 row
    /// runs. Z-Image's default recipe is the base model's, and only the name tells them apart.
    #[tokio::test]
    async fn a_turbo_file_found_by_its_base_model_tag_is_z_image_turbo() {
        use models::gguf::testing::diffusion_gguf;
        let dir = Scratch::new();
        let zimage = write_gguf_txt_fixture(&dir, "zimage");
        let (row, _) = neutral_search(GenFamily::ZImageTurbo);
        let catalog = models::gen_catalog::gen_catalog(
            &models::catalog::Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        );
        let entry = crate::studio::local_models::entry_from_installed(&row, &catalog).unwrap();
        check_main_file(&entry, &zimage).unwrap();

        // Downloaded and registered: added, its files kept.
        let models = dir.path().join("models");
        let group = GenGroup {
            gen_model_id: "hf:someone/my-quants:weights.gguf".into(),
            catalog_id: None,
            name: "Z-Image Turbo".into(),
            kind: "image".into(),
            family: "z-image-turbo".into(),
            source: "search".into(),
            parts: vec![GenGroupPart {
                role: "diffusion".into(),
                repo: "someone/my-quants".into(),
                revision: "c".repeat(40),
                path: "weights.gguf".into(),
                size: 4096,
                sha256: "d".repeat(64),
                license: "apache-2.0".into(),
                folder: None,
                local: None,
                present: None,
            }],
            recipe_id: None,
            untested: false,
        };
        let meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let part = &group.parts[0];
        let dest = models::download::destination(&models, &part.repo, &part.path).unwrap();
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, diffusion_gguf("zimage-tensors.txt")).unwrap();
        let store = store::Store::open_in_memory().unwrap();
        let id = store
            .insert_download(NewDownload {
                group_id: "g".into(),
                repo: part.repo.clone(),
                file: part.path.clone(),
                url: String::new(),
                dest_path: dest.display().to_string(),
                size_bytes: part.size,
                sha256: part.sha256.clone(),
            })
            .await
            .unwrap();
        store
            .update_download(
                id,
                DownloadPatch {
                    status: Some(S::Verifying),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        register_gen_model(&store, &models, "g", &meta)
            .await
            .unwrap();
        let added = store.list_gen_models().await.unwrap();
        assert_eq!(
            (added.len(), added[0].recipe_id.as_deref()),
            (1, Some("z-image-turbo"))
        );
        assert!(dest.exists());
    }

    /// An untested set-up takes any weights that read, never a pickle file or one that does
    /// not read.
    #[test]
    fn an_untested_set_up_takes_any_readable_weights() {
        let dir = Scratch::new();
        let mut sdxl = generic("sdxl-checkpoint", None);
        let zimage = write_gguf_txt_fixture(&dir, "zimage");
        assert!(check_main_file(&sdxl, &zimage).is_err());
        sdxl.untested = true;
        assert!(check_main_file(&sdxl, &zimage).is_ok());
        assert!(check_main_file(&sdxl, &write_gguf_txt_fixture(&dir, "qwen3-4b")).is_ok());
        let pickle = dir.path().join("model.ckpt");
        std::fs::write(&pickle, b"PK").unwrap();
        assert!(check_main_file(&sdxl, &pickle).is_err());
        let junk = dir.path().join("junk.safetensors");
        std::fs::write(&junk, b"not a model").unwrap();
        assert!(check_main_file(&sdxl, &junk).is_err());
    }

    /// A diffusers folder's entry is checked by its folder, from the main part's file in it or
    /// the folder itself.
    #[test]
    fn a_folder_entry_is_checked_by_its_folder() {
        use models::sd::testing::fixture;
        let dir = Scratch::new();
        let root = dir.path().join("sd 1.4 ü");
        for (name, inside) in [
            (
                "sd14-diffusers-unet",
                "unet/diffusion_pytorch_model.safetensors",
            ),
            (
                "sd14-diffusers-text-encoder",
                "text_encoder/model.safetensors",
            ),
        ] {
            let index = fixture(name);
            let tensors: Vec<(&str, Vec<u64>)> = index
                .tensors
                .iter()
                .map(|t| (t.name.as_str(), t.shape.clone()))
                .collect();
            write_safetensors(&root.join(inside), &tensors);
        }
        let mut sd1 = generic("sd1-checkpoint", Some(models::sd::detect::SdVersion::Sd1));
        let mut unet = models::gen_catalog::GenPart {
            role: GenRole::Model,
            repo: "CompVis/stable-diffusion-v1-4".into(),
            revision: "1".repeat(40),
            path: "unet/diffusion_pytorch_model.safetensors".into(),
            size: 1,
            sha256: "ab".repeat(32),
            license: String::new(),
            local: None,
            folder: Some("unet/diffusion_pytorch_model.safetensors".into()),
        };
        sd1.parts = vec![unet.clone()];
        let unet_file = root
            .join("unet")
            .join("diffusion_pytorch_model.safetensors");
        check_main_file(&sd1, &unet_file).unwrap();
        check_main_file(&sd1, &root).unwrap();
        // The UNet alone, read as a file, is not the folder's model.
        unet.folder = None;
        sd1.parts = vec![unet];
        assert!(check_main_file(&sd1, &unet_file).is_err());
        // Ledger S3 minor: a file not at its engine name in the folder (saved under its name
        // in the repository) is in no folder the engine reads: an error, never a guess at a
        // parent folder.
        let mut half = sd1.clone();
        half.parts[0].folder = Some("unet/diffusion_pytorch_model.safetensors".into());
        let fp16_name = root
            .join("unet")
            .join("diffusion_pytorch_model.fp16.safetensors");
        std::fs::copy(&unet_file, &fp16_name).unwrap();
        assert_eq!(
            check_main_file(&half, &fp16_name).unwrap_err().code,
            "internal"
        );
        let outside = dir.path().join("diffusion_pytorch_model.safetensors");
        std::fs::copy(&unet_file, &outside).unwrap();
        assert_eq!(
            check_main_file(&half, &outside).unwrap_err().code,
            "internal"
        );
        assert_eq!(
            folder_root(&unet_file, Some("unet/diffusion_pytorch_model.safetensors")).unwrap(),
            root
        );
        assert!(
            folder_root(&unet_file, None).is_err(),
            "a file with no folder name"
        );
        let sdxl = generic("sdxl-checkpoint", None);
        let mut sdxl_folder = sdxl.clone();
        sdxl_folder.parts = sd1.parts.clone();
        sdxl_folder.parts[0].folder = Some("unet/diffusion_pytorch_model.safetensors".into());
        assert!(
            check_main_file(&sdxl_folder, &unet_file).is_err(),
            "SD 1 is not SDXL"
        );
    }

    /// 0.2.0: a part already on this computer has no row: the model's row records the file it
    /// found, as long as that file is still there with its size; a group saved before 0.2.0
    /// (no recipe, no such parts) still reads, and a generic group's recipe is its own.
    #[test]
    fn a_part_already_here_is_recorded_without_a_row() {
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let mut group = klein();
        let present = dir.path().join("another model's vae.safetensors");
        std::fs::write(&present, vec![0u8; 3_000]).unwrap();
        group.parts[2].present = Some(present.display().to_string());
        group.recipe_id = Some("flux2-klein-4b".into());
        group.family = "flux2-klein".into();
        group.untested = true;
        let meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let rows: Vec<store::Download> = group.parts[..2]
            .iter()
            .map(|p| part_row(&models, p, S::Verifying))
            .collect();
        let new = gen_model_from(&meta, &rows).unwrap();
        let parts = new.parts.as_array().unwrap();
        assert_eq!(parts[2]["file"], present.display().to_string().as_str());
        assert!(parts.iter().all(|p| p.get("present").is_none()));
        assert_eq!(
            (new.recipe_id.as_deref(), new.family.as_str()),
            (Some("flux2-klein-4b"), "flux2-klein")
        );
        let entry = downloaded_entry(&new, true).unwrap();
        assert_eq!(
            (entry.family, entry.recipe.as_str(), entry.untested),
            (None, "flux2-klein-4b", true)
        );
        assert_eq!(
            entry.version,
            Some(models::sd::detect::SdVersion::Flux2Klein)
        );
        // Gone, or of another size, meanwhile: the model is not complete.
        std::fs::write(&present, vec![0u8; 2_999]).unwrap();
        assert_eq!(
            gen_model_from(&meta, &rows).unwrap_err().code,
            "studio_model_incomplete"
        );
        std::fs::remove_file(&present).unwrap();
        assert_eq!(
            gen_model_from(&meta, &rows).unwrap_err().code,
            "studio_model_incomplete"
        );

        // A group 0.1.2 saved: no recipe, no folder, file or present field.
        let old: GenGroup = serde_json::from_value(serde_json::json!({
            "genModelId": "flux2-klein-4b-q4", "catalogId": "flux2-klein-4b-q4",
            "name": "FLUX.2 klein 4B", "kind": "image", "family": "flux2-klein-4b",
            "source": "catalog",
            "parts": [{"role": "diffusion", "repo": "o/r", "revision": "a".repeat(40),
                       "path": "f.gguf", "size": 1, "sha256": "b".repeat(64),
                       "license": "apache-2.0"}],
        }))
        .unwrap();
        assert_eq!((old.recipe_id, old.untested), (None, false));
        assert_eq!(
            (
                &old.parts[0].folder,
                &old.parts[0].local,
                &old.parts[0].present
            ),
            (&None, &None, &None)
        );
        let written = serde_json::to_value(&old.parts[0]).unwrap();
        for absent in ["folder", "local", "present"] {
            assert!(written.get(absent).is_none(), "{absent}");
        }
    }

    /// A file a picture or video model's download not finished yet found here and will use
    /// is kept from deletion as the files it writes are; once it is done, neither is.
    #[tokio::test]
    async fn a_file_an_unfinished_download_will_use_counts_as_being_written() {
        let store = store::Store::open_in_memory().unwrap();
        let models = Path::new("/m");
        let mut group = klein();
        group.parts[2].present = Some("/m/elsewhere/vae.safetensors".into());
        let meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let files: Vec<(FileSpec, PathBuf)> = group.parts[..2]
            .iter()
            .map(|p| {
                let dest = models::download::destination(models, &p.repo, &p.path).unwrap();
                let spec = FileSpec {
                    repo: p.repo.clone(),
                    commit: p.revision.clone(),
                    path: p.path.clone(),
                    size: p.size,
                    sha256: Some(p.sha256.clone()),
                };
                (spec, dest)
            })
            .collect();
        let g = insert_group(&store, &meta, &files).await.unwrap();
        let writing = files_being_written(&store, None).await.unwrap();
        assert!(writing.contains("/m/elsewhere/vae.safetensors"));
        assert_eq!(writing.len(), 3);
        assert!(
            files_being_written(&store, Some(&g))
                .await
                .unwrap()
                .is_empty()
        );
        set_all(&store, &g, S::Completed).await;
        assert!(files_being_written(&store, None).await.unwrap().is_empty());
    }

    /// What `files_being_written` gives guards deleting, so an unfinished group whose
    /// metadata does not read is an error, never a group that uses nothing; a group with no
    /// metadata at all (a chat download from before) uses only its rows' files.
    #[tokio::test]
    async fn a_group_whose_metadata_does_not_read_keeps_deleting_from_guessing() {
        let store = store::Store::open_in_memory().unwrap();
        let (spec, dest) = file(Path::new("/m/a/b.gguf"), 10);
        let g = insert_group(&store, &meta(), &[(spec, dest)])
            .await
            .unwrap();
        store
            .set_setting(&meta_key(&g), &serde_json::json!({"not": "a group"}))
            .await
            .unwrap();
        assert!(files_being_written(&store, None).await.is_err());
        // Not asked about that group: its metadata is not read.
        assert!(
            files_being_written(&store, Some(&g))
                .await
                .unwrap()
                .is_empty()
        );
        store.delete_setting(&meta_key(&g)).await.unwrap();
        assert_eq!(
            files_being_written(&store, None).await.unwrap(),
            BTreeSet::from(["/m/a/b.gguf".to_owned()])
        );
    }

    /// A finished download of a family this build does not know adds nothing.
    #[test]
    fn a_download_of_an_unknown_family_has_no_entry() {
        let new = store::NewGenModel {
            id: "hf:o/r:x.gguf".into(),
            catalog_id: None,
            name: "x".into(),
            kind: "image".into(),
            family: "sd3.5-medium".into(),
            recipe_id: None,
            parts: serde_json::json!([]),
            size_bytes: 0,
            source: store::GenSource::Search,
        };
        assert_eq!(
            downloaded_entry(&new, false).unwrap_err().code,
            "studio_not_supported_yet"
        );
    }

    /// A downloaded diffusion file is checked by its own header before it is added: a FLUX.2
    /// klein 9B named like the 4B is refused, nothing is added, and the diffusion file goes
    /// with the files the download wrote (no other model uses them), while a part it found
    /// already in place stays.
    #[tokio::test]
    async fn a_searched_file_that_is_another_model_is_not_added() {
        use models::gguf::testing::diffusion_gguf;
        let dir = Scratch::new();
        let models = dir.path().join("models");
        let group = group_with(&[
            (
                "someone/FLUX.2-klein-4B-GGUF",
                "flux-2-klein-4b-Q8_0.gguf",
                "a".repeat(40),
            ),
            (
                "unsloth/Qwen3-4B-GGUF",
                "Qwen3-4B-Q4_K_M.gguf",
                "b".repeat(40),
            ),
            (
                "Comfy-Org/Flux-2-Klein-4B",
                "split_files/vae/flux2-vae.safetensors",
                "d".repeat(40),
            ),
        ]);
        let group = GenGroup {
            gen_model_id: "hf:someone/FLUX.2-klein-4B-GGUF:flux-2-klein-4b-Q8_0.gguf".into(),
            catalog_id: None,
            source: "search".into(),
            ..group
        };
        let meta = GroupMeta {
            generator: Some(group.clone()),
            ..meta()
        };
        let dests: Vec<PathBuf> = group
            .parts
            .iter()
            .map(|p| models::download::destination(&models, &p.repo, &p.path).unwrap())
            .collect();
        let write = |dest: &Path, bytes: &[u8]| {
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(dest, bytes).unwrap();
        };

        let diffusion = &dests[0];
        let entry = downloaded_entry(
            &store::NewGenModel {
                id: group.gen_model_id.clone(),
                catalog_id: None,
                name: group.name.clone(),
                kind: group.kind.clone(),
                family: group.family.clone(),
                recipe_id: Some("flux2-klein-4b".into()),
                parts: serde_json::to_value(&group.parts).unwrap(),
                size_bytes: 0,
                source: store::GenSource::Search,
            },
            false,
        )
        .unwrap();
        assert_eq!(
            (entry.family, entry.recipe.as_str(), entry.untested),
            (Some(GenFamily::Flux2Klein4b), "flux2-klein-4b", false)
        );
        assert_eq!(entry.diffusion().path, "flux-2-klein-4b-Q8_0.gguf");
        write(diffusion, &diffusion_gguf("klein9b-tensors.txt"));
        assert_eq!(
            check_main_file(&entry, diffusion).unwrap_err().code,
            "studio_not_that_model"
        );
        write(diffusion, b"not a model");
        assert_eq!(
            check_main_file(&entry, diffusion).unwrap_err().code,
            "studio_not_that_model",
            "a header that does not read"
        );
        write(diffusion, &diffusion_gguf("klein4b-tensors.txt"));
        check_main_file(&entry, diffusion).unwrap();
        write(&dests[2], b"safetensors bytes");
        assert_eq!(
            check_main_file(&entry, &dests[2]).unwrap_err().code,
            "studio_not_that_model",
            "a .safetensors model file is read too"
        );

        // Registering the download with the 9B file adds nothing and removes its files.
        let store = store::Store::open_in_memory().unwrap();
        for (part, dest) in group.parts.iter().zip(&dests) {
            let id = store
                .insert_download(NewDownload {
                    group_id: "g".into(),
                    repo: part.repo.clone(),
                    file: part.path.clone(),
                    url: String::new(),
                    dest_path: dest.display().to_string(),
                    size_bytes: part.size,
                    sha256: part.sha256.clone(),
                })
                .await
                .unwrap();
            store
                .update_download(
                    id,
                    DownloadPatch {
                        status: Some(S::Verifying),
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        write(diffusion, &diffusion_gguf("klein9b-tensors.txt"));
        write(&dests[1], b"encoder");
        // The download wrote the encoder; the decoder was already in place (written above),
        // so the download only checked it.
        note_written(&store, "g", &dests[1].display().to_string())
            .await
            .unwrap();
        let e = register_gen_model(&store, &models, "g", &meta)
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_not_that_model");
        assert!(store.list_gen_models().await.unwrap().is_empty());
        assert!(!diffusion.exists(), "the file that is another model goes");
        assert!(!dests[1].exists(), "the file the download wrote goes");
        assert!(dests[2].exists(), "the part found in place stays");
        assert!(
            !models.join("someone").exists() && !models.join("unsloth").exists(),
            "and their empty folders"
        );
        assert!(models.exists(), "never the models folder");

        // The 4B file is added, with every part.
        for (dest, bytes) in dests.iter().zip([
            diffusion_gguf("klein4b-tensors.txt"),
            b"encoder".to_vec(),
            b"decoder".to_vec(),
        ]) {
            write(dest, &bytes);
        }
        register_gen_model(&store, &models, "g", &meta)
            .await
            .unwrap();
        let added = store.list_gen_models().await.unwrap();
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].id, group.gen_model_id);
        assert_eq!(added[0].parts.as_array().unwrap().len(), 3);
        assert!(dests.iter().all(|d| d.exists()));
    }
}
