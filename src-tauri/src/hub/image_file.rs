//! Image files (0.0.7, package P2): the file a vision model reads pictures with, which
//! llama.cpp loads with `--mmproj` (a "projector"; what the user reads is "image file").
//!
//! - **Which one a load uses** ([`choose`]): the one recorded on the model's Installed row,
//!   else one next to the model file (`*mmproj*.gguf`, the best precision first by
//!   [`models::files::projector_rank`]), so an image file put there after the model was
//!   downloaded or imported is used. A load records what it found ([`prepare_for_load`]).
//! - **One that could not be read** ([`mark_unreadable`]): when llama-server fails on it
//!   and the supervisor loads the model again without it, the app remembers the file on the
//!   model's row by path, size and modification time ([`FileStamp`]). Later loads go
//!   straight to the model without it, and say so, until the file changes or another image
//!   file is downloaded ([`attach`]).
//! - **Downloading one** ([`pick`], [`download_image_file`]): for a model with a Hugging
//!   Face repository, the best image file of the repository's listing that is not one
//!   remembered as unreadable, saved in the model's folder through the resumable, checked
//!   downloads (`downloads.rs`), then recorded on the row.
//!
//! Only models on the Installed list are looked after: a model file loaded from anywhere
//! else loads without an image file, as before 0.0.7. File names and listings are
//! untrusted: a name from a listing is checked like every download path, and only its
//! last part is used, inside the model's own folder.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use models::RepoFile;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, State};

use super::StoredSummary;
use super::compare::{Hub as _, LiveHub};
use crate::downloads::{DownloadManager, GroupMeta};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// What identifies an image file on disk: its path, size and when it last changed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileStamp {
    pub path: String,
    pub size: u64,
    /// Milliseconds since 1970; absent where the system does not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_ms: Option<u64>,
}

impl FileStamp {
    /// The stamp of the file at `path` now; `None` when it is not a file there.
    pub fn of(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
        Some(Self {
            path: path.display().to_string(),
            size: meta.len(),
            modified_ms: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| u64::try_from(d.as_millis()).ok()),
        })
    }

    fn file_name(&self) -> Option<&str> {
        Path::new(&self.path).file_name().and_then(|n| n.to_str())
    }

    /// Whether `other` is the same file: the same name, size and modification time. A
    /// path spelled another way (a link, a mapped drive) or a folder moved with its files
    /// is still the same file; a file that was replaced or changed is not.
    pub fn same_file(&self, other: &Self) -> bool {
        self.size == other.size
            && self.modified_ms == other.modified_ms
            && self.file_name().is_some()
            && self.file_name() == other.file_name()
    }
}

/// The most image files remembered as unreadable for one model; the oldest goes first.
pub const MAX_UNREADABLE: usize = 8;

/// `list` with `stamp` remembered as unreadable: an older entry for the same path or the
/// same file is replaced, and at most [`MAX_UNREADABLE`] stay.
pub fn remember(list: &[FileStamp], stamp: FileStamp) -> Vec<FileStamp> {
    let mut out: Vec<FileStamp> = list
        .iter()
        .filter(|s| s.path != stamp.path && !s.same_file(&stamp))
        .cloned()
        .collect();
    out.push(stamp);
    let extra = out.len().saturating_sub(MAX_UNREADABLE);
    out.drain(..extra);
    out
}

/// Whether the file stamped `stamp` is remembered in `list` as unreadable.
pub fn is_unreadable(list: &[FileStamp], stamp: &FileStamp) -> bool {
    list.iter().any(|s| s.same_file(stamp))
}

/// Image files next to the model file (a name with `mmproj` that ends in `.gguf`, as
/// downloads and other tools name them), the best precision first, then by name.
pub fn beside(model: &Path) -> Vec<PathBuf> {
    let Some(dir) = model.parent() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(u8, String, PathBuf)> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|path| {
            let name = path.file_name()?.to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            (lower.contains("mmproj") && lower.ends_with(".gguf") && path.is_file())
                .then(|| (models::files::projector_rank(&name), lower, path))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, _, path)| path).collect()
}

/// The image file a load of a model uses ([`choose`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choice {
    /// The image file to load with the model, if any.
    pub path: Option<String>,
    /// `path` was found next to the model, not the one recorded: a load records it.
    pub found: bool,
    /// An image file was left out because it could not be read before, and no other
    /// usable one is there: the model loads without image input, and the app says why.
    pub left_out: bool,
}

/// Which image file a load uses: the `recorded` one while it is there and not remembered
/// as unreadable; else the first usable one `beside` the model (see [`beside`]). `stamp_of`
/// reads a file's stamp (`None`: not there).
pub fn choose(
    recorded: Option<&str>,
    beside: &[PathBuf],
    unreadable: &[FileStamp],
    stamp_of: impl Fn(&Path) -> Option<FileStamp>,
) -> Choice {
    let usable = |p: &Path| stamp_of(p).map(|s| !is_unreadable(unreadable, &s));
    let mut left_out = false;
    if let Some(recorded) = recorded {
        match usable(Path::new(recorded)) {
            Some(true) => {
                return Choice {
                    path: Some(recorded.to_owned()),
                    found: false,
                    left_out: false,
                };
            }
            Some(false) => left_out = true,
            None => {}
        }
    }
    for candidate in beside {
        match usable(candidate) {
            Some(true) => {
                return Choice {
                    path: Some(candidate.display().to_string()),
                    found: true,
                    left_out: false,
                };
            }
            Some(false) => left_out = true,
            None => {}
        }
    }
    Choice {
        path: None,
        found: false,
        left_out,
    }
}

/// What a model's Hugging Face repository offers ([`pick`]), for Models → Installed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Offer {
    /// An image file to download: its name in the repository and its size.
    #[serde(rename_all = "camelCase")]
    Available { file: String, size_bytes: u64 },
    /// The repository's image files are all ones that could not be read here.
    NoOther,
    /// The repository has no image file (or the model has no repository). JSON
    /// `{"kind":"none"}`.
    #[serde(rename = "none")]
    NoImageFile,
}

/// The last part of a repository path.
fn last_part(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// The image file to download from a repository listing: the best by the ranking Browse
/// uses ([`models::classify`], F16 first), skipping one remembered as unreadable (the same
/// name and size). Returns what to offer and the file.
pub fn pick(files: &[RepoFile], unreadable: &[FileStamp]) -> (Offer, Option<RepoFile>) {
    let projectors = models::classify(files).projectors;
    if projectors.is_empty() {
        return (Offer::NoImageFile, None);
    }
    let known_bad = |f: &RepoFile| {
        unreadable
            .iter()
            .any(|s| s.size == f.size && s.file_name() == Some(last_part(&f.path)))
    };
    match projectors.into_iter().find(|f| !known_bad(f)) {
        Some(file) => (
            Offer::Available {
                file: last_part(&file.path).to_owned(),
                size_bytes: file.size,
            },
            Some(file),
        ),
        None => (Offer::NoOther, None),
    }
}

/// Where a downloaded image file goes: the model's own folder, under the last part of its
/// repository path, which must pass the download path check (no folder escapes, no
/// reserved or hidden-character names). `None` when it does not.
pub fn destination(model: &Path, repo_path: &str) -> Option<PathBuf> {
    let parts = models::hf::validate_repo_path(repo_path).ok()?;
    let name = parts.last()?;
    Some(model.parent()?.join(name))
}

/// The Installed row of `model_path` (any spelling of it) and its saved summary; `None`
/// when the model is not on the list or its summary cannot be read (nothing is then
/// written over it).
async fn row(
    store: &store::Store,
    model_path: &str,
) -> Option<(store::ModelRecord, StoredSummary)> {
    let rec = crate::model_options::installed_row(store, model_path)
        .await
        .ok()
        .flatten()?;
    let stored = serde_json::from_value(rec.gguf_summary.clone()).ok()?;
    Some((rec, stored))
}

/// Saves `stored` as the summary of `rec`, everything else as it is.
async fn save(
    store: &store::Store,
    rec: &store::ModelRecord,
    stored: &StoredSummary,
) -> AppResult<()> {
    store
        .upsert_model(store::ModelInput {
            path: rec.path.clone(),
            repo: rec.repo.clone(),
            file_name: rec.file_name.clone(),
            size_bytes: rec.size_bytes,
            sha256: rec.sha256.clone(),
            gguf_summary: serde_json::to_value(stored).unwrap_or_default(),
            source: rec.source,
        })
        .await?;
    Ok(())
}

/// The choice for a row as it is saved.
fn choice_of(rec: &store::ModelRecord, stored: &StoredSummary) -> Choice {
    choose(
        stored.projector.as_deref(),
        &beside(Path::new(&rec.path)),
        &stored.unreadable_projectors,
        FileStamp::of,
    )
}

/// The image file the next load of `model_path` uses ([`choose`]); a model that is not on
/// the Installed list has none.
pub async fn choice_for(store: &store::Store, model_path: &str) -> Choice {
    match row(store, model_path).await {
        Some((rec, stored)) => choice_of(&rec, &stored),
        None => Choice::default(),
    }
}

/// Right before a load of `model_path` that uses the image file `planned` (what the plan
/// chose): records an image file found next to the model on its Installed row, so the row
/// shows it from then on. Returns whether an image file was left out because it could not
/// be read before, for the header's note.
pub async fn prepare_for_load(
    store: &store::Store,
    model_path: &str,
    planned: Option<&str>,
) -> bool {
    let Some((rec, mut stored)) = row(store, model_path).await else {
        return false;
    };
    let choice = choice_of(&rec, &stored);
    if choice.found && planned.is_some() && choice.path.as_deref() == planned {
        stored.projector = choice.path.clone();
        if let Err(e) = save(store, &rec, &stored).await {
            tracing::warn!("could not record the image file found next to the model: {e}");
        }
    }
    planned.is_none() && choice.left_out
}

/// Remembers that `image` could not be read with the model at `model_path` (the supervisor
/// loaded the model again without it), on the model's Installed row. Nothing happens for a
/// model that is not on the list, or a file that is gone.
pub async fn mark_unreadable(store: &store::Store, model_path: &Path, image: &Path) {
    let Some(stamp) = FileStamp::of(image) else {
        return;
    };
    let Some((rec, mut stored)) = row(store, &model_path.display().to_string()).await else {
        return;
    };
    if is_unreadable(&stored.unreadable_projectors, &stamp) {
        return;
    }
    stored.unreadable_projectors = remember(&stored.unreadable_projectors, stamp);
    match save(store, &rec, &stored).await {
        Ok(()) => tracing::warn!(
            model = %rec.file_name,
            "the image file could not be read; later loads leave it out until it changes"
        ),
        Err(e) => tracing::warn!("could not remember the image file that could not be read: {e}"),
    }
}

/// A downloaded image file for the model registered at `model_path`: recorded on its row,
/// and the files remembered as unreadable are forgotten (another image file was
/// downloaded). Nothing happens when the model was removed meanwhile.
pub async fn attach(store: &store::Store, model_path: &str, image: &Path) -> AppResult<()> {
    let Some(rec) = store.model_by_path(model_path).await? else {
        return Ok(());
    };
    let mut stored: StoredSummary =
        serde_json::from_value(rec.gguf_summary.clone()).unwrap_or_default();
    stored.projector = Some(image.display().to_string());
    stored.unreadable_projectors.clear();
    save(store, &rec, &stored).await
}

/// What the model's Hugging Face repository offers as its image file (Models →
/// Installed, for a model without a usable one). Asks Hugging Face for the repository's
/// listing, reused for ten minutes like Browse's; a model without a repository has none.
#[tauri::command]
pub async fn image_file_offer(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Offer> {
    crate::commands::only_main_window(window.label())?;
    let (rec, stored) = row(&state.store, &path)
        .await
        .ok_or_else(|| AppError::not_found("model"))?;
    let Some(repo) = rec.repo else {
        return Ok(Offer::NoImageFile);
    };
    let hub = LiveHub::new(Some(super::hf_client(&app).await?));
    let listing = hub.listing(&repo, None).await.map_err(super::hf_error)?;
    Ok(pick(&listing.files, &stored.unreadable_projectors).0)
}

/// "Download the image file" in Models → Installed: the image file [`pick`] chooses from
/// the model's repository (listed again, so the size and checksum are the Hub's now) is
/// downloaded into the model's folder like any download (resumable, checked against its
/// published size and SHA-256), then recorded on the model's row. Returns the download's
/// id; the same file already downloading returns that download's id.
#[tauri::command]
pub async fn download_image_file(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<String> {
    crate::commands::only_main_window(window.label())?;
    let (rec, stored) = row(&state.store, &path)
        .await
        .ok_or_else(|| AppError::not_found("model"))?;
    let repo = rec.repo.clone().ok_or_else(|| {
        AppError::bad_request(
            "This model has no Hugging Face repository to get an image file from.",
        )
    })?;
    let client = super::hf_client(&app).await?;
    let detail = client.repo(&repo, None).await.map_err(super::hf_error)?;
    LiveHub::new(Some(client)).remember_listing(None, Arc::new(detail.clone()));
    let file = match pick(&detail.files, &stored.unreadable_projectors) {
        (_, Some(file)) => file,
        (Offer::NoOther, None) => {
            return Err(AppError::bad_request(
                "Its repository has no other image file.",
            ));
        }
        _ => return Err(AppError::bad_request("Its repository has no image file.")),
    };
    let dest = destination(Path::new(&rec.path), &file.path).ok_or_else(|| {
        AppError::bad_request(
            "The image file in its repository has a name the app does not accept.",
        )
    })?;
    let manager = app.state::<Arc<DownloadManager>>().inner().clone();
    // The same file downloading already (the button pressed again after the dialog was
    // closed and opened): follow that download, going on with it if it stopped.
    let dest_text = dest.display().to_string();
    if let Some(earlier) = state
        .store
        .list_downloads(None)
        .await?
        .into_iter()
        .find(|r| {
            r.dest_path == dest_text
                && !matches!(
                    r.status,
                    store::DownloadStatus::Completed | store::DownloadStatus::Cancelled
                )
        })
    {
        if matches!(
            earlier.status,
            store::DownloadStatus::Failed | store::DownloadStatus::Paused
        ) {
            manager.resume(&app, &earlier.group_id).await?;
        }
        return Ok(earlier.group_id);
    }
    let meta = GroupMeta {
        repo: detail.id.clone(),
        commit: detail.commit.clone(),
        label: "Image file".into(),
        model_files: Vec::new(),
        projector: Some(file.path.clone()),
        catalog_id: None,
        hub: None,
        parameters: None,
        image_file_for: Some(rec.path.clone()),
    };
    manager
        .start(
            &app,
            meta,
            vec![(super::file_spec(&detail.id, &detail.commit, &file), dest)],
        )
        .await
}

#[cfg(test)]
mod tests;
