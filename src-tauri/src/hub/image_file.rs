//! Image files (0.0.7, package P2): the file a vision model reads pictures with, which
//! llama.cpp loads with `--mmproj` (a "projector"; what the user reads is "image file").
//!
//! - **Which one a load uses** ([`choose`]): the one recorded on the model's Installed row,
//!   else one next to the model file that is its own ([`candidates`]: a plain
//!   `*mmproj*.gguf` file whose width matches the model's, or named after it or after no
//!   model; the best precision first by [`models::files::projector_rank`]), so an image
//!   file put there after the model was downloaded or imported is used, and another model's
//!   in a shared folder is not. A load records what it found ([`prepare_for_load`]).
//! - **One that could not be read** ([`mark_unreadable`]): when llama-server fails on it
//!   and the supervisor loads the model again without it, the app remembers the file on the
//!   model's row by path, size and modification time ([`FileStamp`]). Later loads go
//!   straight to the model without it, and say so, until the file changes, another image
//!   file is downloaded ([`attach`]) or the user tries it again ([`forget_unreadable`]).
//! - **Downloading one** ([`pick`], [`download_image_file`]): for a model with a Hugging
//!   Face repository, the best image file of the repository that is not one remembered as
//!   unreadable (or the same one again when its copy here may differ), saved in the
//!   model's folder through the resumable, checked downloads (`downloads.rs`), then
//!   recorded on the row. The repository's image files are saved on the row when the model
//!   is downloaded, so Installed offers one without asking Hugging Face ([`saved_offer`]);
//!   it asks only when the user looks for one ([`image_file_offer`], a model downloaded
//!   before 0.0.7) or downloads it.
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
    /// The SHA-256 its repository published, for a file the app downloaded (0.0.7 review
    /// M1): whether downloading it again could bring other bytes. Absent for any other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
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
            sha256: None,
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
/// downloads and other tools name them), the best precision first, then by name. Only
/// plain files: never a link, which could point anywhere (0.0.7 review m9).
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
            let plain = std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_file());
            (lower.contains("mmproj") && lower.ends_with(".gguf") && plain)
                .then(|| (models::files::projector_rank(&name), lower, path))
        })
        .collect();
    found.sort();
    found.into_iter().map(|(_, _, path)| path).collect()
}

/// Words of a file name that say nothing about which model it is for: its extension and
/// kind, and the words of precisions and quantizations (`Q4_K_M` reads as "q4", "k", "m").
const NAMELESS_WORDS: &[&str] = &[
    "gguf",
    "ggml",
    "mmproj",
    "model",
    "models",
    "vision",
    "projector",
    "proj",
    "clip",
    "encoder",
    "image",
    "ud",
    "i1",
    "imatrix",
    "k",
    "m",
    "s",
    "l",
    "xs",
    "xxs",
    "xl",
    "nl",
];

/// The words of a file name that name a model (0.0.7 review m4): its runs of letters and
/// digits, lowercased, without [`NAMELESS_WORDS`], single digits and precision labels
/// (`f16`, `bf16`, `q8`, `iq4` and the like).
pub(crate) fn name_words(name: &str) -> Vec<String> {
    let label = |w: &str| {
        let digits = w.trim_start_matches(|c: char| c.is_ascii_alphabetic());
        let letters = &w[..w.len() - digits.len()];
        !digits.is_empty()
            && digits.bytes().all(|b| b.is_ascii_digit())
            && matches!(letters, "f" | "bf" | "fp" | "q" | "iq" | "tq")
    };
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| !NAMELESS_WORDS.contains(w))
        .filter(|w| !(w.len() == 1 && w.as_bytes()[0].is_ascii_digit()))
        .filter(|w| !label(w))
        .map(str::to_owned)
        .collect()
}

/// How an image file's name relates to the model it may be for (0.0.7 review m4), from the
/// model's [`name_words`]: 0 named after it, 1 named after no model (or the model's own name
/// names nothing), 2 named after another model.
fn name_tier(model_words: &[String], file_name: &str) -> u8 {
    let words = name_words(file_name);
    if words.is_empty() || model_words.is_empty() {
        1
    } else if words.iter().all(|w| model_words.contains(w)) {
        0
    } else {
        2
    }
}

/// The kinds of image file whose header does not give the width llama.cpp checks (it
/// takes theirs from their weights, `clip_n_mmproj_embd`): the old LLaVA kinds name the
/// encoder's own width there, MiniCPM-V's `resampler` 0, Ultravox's that of its own layer.
const NO_WIDTH_KINDS: &[&str] = &["mlp", "mlp_norm", "ldp", "ldpv2", "resampler", "ultravox"];

/// What an image file's header says (0.0.7): whether it is an image file at all, and the
/// width of what it makes when the header gives one llama.cpp would agree with
/// (`clip.vision.projection_dim`, or the audio one: not 0, not of [`NO_WIDTH_KINDS`]).
/// `None` when the header cannot be read (a damaged file, which a load then finds out
/// about).
fn image_header(image: &Path) -> Option<(bool, Option<u64>)> {
    let header = models::read_file(image).ok()?;
    let image_file =
        header.architecture() == Some("clip") || header.get_str("general.type") == Some("mmproj");
    let kind = header.get_str("clip.projector_type").unwrap_or("mlp");
    let width = header
        .get_u64("clip.vision.projection_dim")
        .or_else(|| header.get_u64("clip.audio.projection_dim"))
        .filter(|&w| w > 0 && !NO_WIDTH_KINDS.contains(&kind));
    Some((image_file, width))
}

/// The words of the name of the model file at `path`, without the part number of a split
/// model (`-00001-of-00003`).
fn model_words(path: &Path) -> Vec<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = name.strip_suffix(".gguf").unwrap_or(&name);
    let unsplit = match stem.rsplit_once("-of-") {
        Some((before, total)) if total.len() == 5 && total.bytes().all(|b| b.is_ascii_digit()) => {
            match before.rsplit_once('-') {
                Some((base, part))
                    if part.len() == 5 && part.bytes().all(|b| b.is_ascii_digit()) =>
                {
                    base
                }
                _ => stem,
            }
        }
        _ => stem,
    };
    name_words(unsplit)
}

/// Whether the model at `model` is the only model in its folder (P2 verification N5): every
/// model file there (a `.gguf` whose name has no "mmproj") is a quantization or a part of
/// the same model, by the words of its name, as in the folder a download makes for one.
fn alone_in_folder(model: &Path) -> bool {
    let (Some(dir), own) = (model.parent(), model_words(model)) else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.filter_map(Result::ok).all(|entry| {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        !name.ends_with(".gguf") || name.contains("mmproj") || model_words(&entry.path()) == own
    })
}

/// The image files next to `model` that may be its own, in the order a load tries them
/// (0.0.7 review m4, P2 verification N1 and N5). A file whose header gives a width it can
/// be believed on ([`image_header`]) that matches the model's embedding length
/// (`embedding_length`, when known) comes first; one whose believed width differs is
/// another model's, and so is a file that is not an image file. Other kinds' headers don't
/// give the width llama.cpp checks, so for them only the names and the folder count. In
/// the model's own folder ([`alone_in_folder`]) every image file there may be its own: then
/// those named after it, those named after no model, the rest. In a folder several models
/// share, only those named after it, and one whose name names no model when its width
/// matches; never one named after another model, so a text-only model there takes none of
/// the others' files. The recorded file is not judged here ([`choice_of_row`]).
pub fn candidates(model: &Path, embedding_length: Option<u64>) -> Vec<PathBuf> {
    let model_words = model_words(model);
    let alone = alone_in_folder(model);
    let mut tiered: Vec<(u8, PathBuf)> = beside(model)
        .into_iter()
        .filter_map(|path| {
            let (image_file, width) = image_header(&path).unwrap_or((true, None));
            // Not an image file, or one whose header gives a width (believed: not 0, not a
            // kind of `NO_WIDTH_KINDS`) other than the model's: another model's, also when
            // the names are the same (`gemma-2-27b-it` and `gemma-3-27b-it`).
            let other_width = matches!((width, embedding_length), (Some(w), Some(n)) if w != n);
            if !image_file || other_width {
                return None;
            }
            let matches = width.is_some() && width == embedding_length;
            let name = name_tier(&model_words, &path.file_name()?.to_string_lossy());
            let tier = match (alone, name) {
                _ if matches && (alone || name < 2) => 0,
                (true, name) => 1 + name,
                (false, 0) => 1,
                (false, _) => return None,
            };
            Some((tier, path))
        })
        .collect();
    // Stable: the best precision first within each tier, as `beside` gave them.
    tiered.sort_by_key(|(tier, _)| *tier);
    tiered.into_iter().map(|(_, path)| path).collect()
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
    /// The repository's image files are all ones that could not be read here, but the
    /// copy here of this one may differ from what the repository holds under its name (it
    /// was not downloaded by the app, or the repository published other bytes since): it
    /// can be downloaded again (0.0.7 review M1).
    #[serde(rename_all = "camelCase")]
    Again { file: String, size_bytes: u64 },
    /// The repository's image files are all ones that could not be read here, downloaded
    /// from what it holds now.
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

/// A repository's image files (`projectors`, ranked as Browse ranks them) in the order
/// they go with the model file `model` (a path, or a path in the repository): those named
/// after it, those named after no model, then the others (0.0.7 review m4), the ranking
/// kept within each.
fn for_model(mut projectors: Vec<RepoFile>, model: &str) -> Vec<RepoFile> {
    let words = model_words(Path::new(last_part(&model.replace('\\', "/"))));
    projectors.sort_by_key(|f| name_tier(&words, last_part(&f.path)));
    projectors
}

/// The image file Browse downloads with the model file `model` (a path in its repository):
/// the first of the repository's image files `projectors` for it ([`for_model`]), so a
/// repository with several models' image files gives the chosen model its own.
pub(super) fn best_for(projectors: &[RepoFile], model: &str) -> Option<RepoFile> {
    for_model(projectors.to_vec(), model).into_iter().next()
}

/// The image file to download from a repository listing for the model at `model`: those
/// named after it first, then those named after no model, then the others (0.0.7 review
/// m4: a repository may hold the image files of several sizes of a model), each by the
/// ranking Browse uses ([`models::classify`], F16 first); skipping one remembered as
/// unreadable (the same name and size). When those are all there is, one whose copy here
/// may differ from the repository's (its SHA-256 unknown here or published anew) is offered
/// again. Returns what to offer and the file.
pub fn pick(
    files: &[RepoFile],
    unreadable: &[FileStamp],
    model: &str,
) -> (Offer, Option<RepoFile>) {
    let projectors = for_model(models::classify(files).projectors, model);
    if projectors.is_empty() {
        return (Offer::NoImageFile, None);
    }
    let remembered = |f: &RepoFile| {
        unreadable
            .iter()
            .find(|s| s.size == f.size && s.file_name() == Some(last_part(&f.path)))
    };
    if let Some(file) = projectors.iter().find(|f| remembered(f).is_none()) {
        let offer = Offer::Available {
            file: last_part(&file.path).to_owned(),
            size_bytes: file.size,
        };
        return (offer, Some(file.clone()));
    }
    let may_differ = |f: &&RepoFile| {
        remembered(f).is_some_and(|s| match (&s.sha256, &f.sha256) {
            (Some(here), Some(there)) => here != there,
            _ => true,
        })
    };
    match projectors.iter().find(may_differ) {
        Some(file) => (
            Offer::Again {
                file: last_part(&file.path).to_owned(),
                size_bytes: file.size,
            },
            Some(file.clone()),
        ),
        None => (Offer::NoOther, None),
    }
}

/// What Installed offers for the image file of the model at `model` from what is saved on
/// its row (0.0.7 review M2), without asking Hugging Face. `None` when the model has no
/// repository, or the app never saw its repository's image files (a model downloaded
/// before 0.0.7): Installed then offers to look ([`image_file_offer`]).
pub(super) fn saved_offer(
    repo: Option<&str>,
    model: &str,
    stored: &StoredSummary,
) -> Option<Offer> {
    repo?;
    let files = stored.repo_image_files.as_deref()?;
    Some(pick(files, &stored.unreadable_projectors, model).0)
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

/// The choice for the row of the model at `path` as it is saved ([`choose`] among
/// [`candidates`]). The recorded image file is never set aside for the width in its
/// header, which llama.cpp does not check (P2 verification N1).
pub(super) fn choice_of_row(path: &str, stored: &StoredSummary) -> Choice {
    choose(
        stored.projector.as_deref(),
        &candidates(Path::new(path), stored.embedding_length),
        &stored.unreadable_projectors,
        FileStamp::of,
    )
}

/// The embedding length in the header of the model at `path`, when it can be read.
pub(super) fn embedding_length_of(path: &Path) -> Option<u64> {
    models::read_file(path)
        .ok()?
        .arch_u64("embedding_length")
        .filter(|&n| n > 0)
}

/// The image file the next load of `model_path` uses ([`choose`]); a model that is not on
/// the Installed list has none. A row saved without the model's embedding length gets it
/// first, so the width check applies (0.0.7 review m4).
pub async fn choice_for(store: &store::Store, model_path: &str) -> Choice {
    let Some((rec, mut stored)) = row(store, model_path).await else {
        return Choice::default();
    };
    if stored.embedding_length.is_none()
        && let Some(n) = embedding_length_of(Path::new(&rec.path))
    {
        stored.embedding_length = Some(n);
        if let Err(e) = save(store, &rec, &stored).await {
            tracing::warn!("could not save the model's embedding length: {e}");
        }
    }
    choice_of_row(&rec.path, &stored)
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
    let choice = choice_of_row(&rec.path, &stored);
    if choice.found && planned.is_some() && choice.path.as_deref() == planned {
        stored.projector = choice.path.clone();
        // Found, not downloaded: what its repository published is not known for it.
        stored.projector_sha256 = None;
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
    let Some(mut stamp) = FileStamp::of(image) else {
        return;
    };
    let Some((rec, mut stored)) = row(store, &model_path.display().to_string()).await else {
        return;
    };
    // The image file the app downloaded for it: what its repository published goes along.
    if stored.projector.as_deref() == Some(stamp.path.as_str()) {
        stamp.sha256.clone_from(&stored.projector_sha256);
    }
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

/// "Try it again" in Models → Installed (0.0.7 review M1): forgets the image files
/// remembered as unreadable with the model at `model_path`, so its next load tries them
/// again (an image file can fail for a reason that was not its own). Returns whether
/// there was anything to forget; nothing happens for a model that is not on the list.
pub async fn forget_unreadable(store: &store::Store, model_path: &str) -> AppResult<bool> {
    let Some((rec, mut stored)) = row(store, model_path).await else {
        return Ok(false);
    };
    if stored.unreadable_projectors.is_empty() {
        return Ok(false);
    }
    stored.unreadable_projectors.clear();
    save(store, &rec, &stored).await?;
    Ok(true)
}

/// Saves the image files of `listing` (a listing of the model's repository) on the row of
/// the model at `model_path`, best first, and returns what Installed offers from them
/// (0.0.7 review M2); `None` for a model that is not on the list.
pub async fn save_repo_image_files(
    store: &store::Store,
    model_path: &str,
    listing: &[RepoFile],
) -> AppResult<Option<Offer>> {
    let Some((rec, mut stored)) = row(store, model_path).await else {
        return Ok(None);
    };
    let files = models::classify(listing).projectors;
    let offer = pick(&files, &stored.unreadable_projectors, &rec.path).0;
    stored.repo_image_files = Some(files);
    save(store, &rec, &stored).await?;
    Ok(Some(offer))
}

/// A downloaded image file for the model registered at `model_path`: recorded on its row
/// with the SHA-256 its repository published (`sha256`, when it did), and the files
/// remembered as unreadable are forgotten (another image file was downloaded). Nothing
/// happens when the model was removed meanwhile.
pub async fn attach(
    store: &store::Store,
    model_path: &str,
    image: &Path,
    sha256: Option<String>,
) -> AppResult<()> {
    let Some(rec) = store.model_by_path(model_path).await? else {
        return Ok(());
    };
    let mut stored: StoredSummary =
        serde_json::from_value(rec.gguf_summary.clone()).unwrap_or_default();
    stored.projector = Some(image.display().to_string());
    stored.projector_sha256 = sha256;
    stored.unreadable_projectors.clear();
    save(store, &rec, &stored).await
}

/// "Look for an image file" in Models → Installed, for a model whose repository's image
/// files the app has not seen (downloaded before 0.0.7), and right after its image file
/// could not be read (0.0.7 review M2: never by itself otherwise). Asks Hugging Face for
/// the repository's listing (reused for ten minutes like Browse's), saves its image files on
/// the model's row, and returns what Installed offers; a model without a repository has
/// none.
#[tauri::command]
pub async fn image_file_offer(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<Offer> {
    crate::commands::only_main_window(window.label())?;
    let (rec, _) = row(&state.store, &path)
        .await
        .ok_or_else(|| AppError::not_found("model"))?;
    let Some(repo) = rec.repo else {
        return Ok(Offer::NoImageFile);
    };
    let hub = LiveHub::new(Some(super::hf_client(&app).await?));
    let listing = hub.listing(&repo, None).await.map_err(super::hf_error)?;
    save_repo_image_files(&state.store, &rec.path, &listing.files)
        .await?
        .ok_or_else(|| AppError::not_found("model"))
}

/// "Try it again" in Models → Installed (0.0.7 review M1, [`forget_unreadable`]). Returns
/// whether there was an image file to try again.
#[tauri::command]
pub async fn forget_unreadable_image_file(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    path: String,
) -> AppResult<bool> {
    crate::commands::only_main_window(window.label())?;
    forget_unreadable(&state.store, &path).await
}

/// "Download the image file" in Models → Installed: the image file [`pick`] chooses from
/// the model's repository (listed again, so the size and checksum are the Hub's now) is
/// downloaded into the model's folder like any download (resumable, checked against its
/// published size and SHA-256), then recorded on the model's row. Returns the download's
/// id. 0.0.7 (P6): `DownloadManager::start` decides, in one step with recording it, what
/// happens when that file is downloading already: the image file asked for again for this
/// model follows its download (going on with it when it stopped, starting afresh in place of
/// one that failed); a download of it for another model (another quant of the repository)
/// is left alone and this one is refused, saying this model uses that file too once it is
/// done.
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
    // What the repository has now, for what Installed offers next (0.0.7 review M2).
    if let Err(e) = save_repo_image_files(&state.store, &rec.path, &detail.files).await {
        tracing::warn!("could not save the repository's image files: {e}");
    }
    let file = match pick(&detail.files, &stored.unreadable_projectors, &rec.path) {
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
        image_files: None,
        generator: None,
        file_only: false,
        lora: None,
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
