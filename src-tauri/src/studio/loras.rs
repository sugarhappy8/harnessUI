//! LoRAs (design 8, Task S10): Studio's LoRAs, each a row of `gen_loras` whose file is in one
//! folder, `<models>/@loras` ([`lora_folder`]), so the engine reads every LoRA by its name
//! relative to that folder (`--lora-model-dir`): the engine's LoRA name cannot hold `:`, so
//! never a full Windows path (correction 14).
//!
//! - **Its name** (`rel_name`, the name its engine tag carries): `<owner>/<repo>/<path without
//!   its extension>` for a LoRA downloaded from a Hugging Face search
//!   ([`rel_name_for_download`]), `local/<its file's stem>` for one added from this computer
//!   ([`rel_name_for_import`]), each part without what the engine's tag cannot carry
//!   ([`sanitize_lora_component`]). A name another LoRA has (in any case: one file on Windows
//!   and macOS), or one the engine would read another file for, is followed by " (2)", " (3)",
//!   …. Its file is `<models>/@loras/<rel_name>` with `.safetensors` for a `.safetensors` or
//!   `.sft` file and `.gguf` for a `.gguf` one ([`stored_extension`]): the engine tries only
//!   `.gguf`, `.safetensors`, `.pt` and `.ckpt` after a name.
//! - **What is shown** ([`LoraDto`]): its file's stem as written, spaces and any Unicode kept;
//!   the base version its header says; whether its file is gone.
//! - **Downloading one** ([`prepare_lora_download`]): its repo's listing at the commit read
//!   again; a file Studio cannot read, one a run on this computer holds, one with no published
//!   SHA-256, and one whose header
//!   (one bounded read, kept in the `DetectService`'s cache) is not a LoRA's
//!   (`studio_not_a_lora`) are refused before anything starts. Then one download group of its
//!   one file, pinned by repository, commit, path, size and SHA-256 and checked against it;
//!   once checked it is added (`downloads::register_lora`) as `hf:<owner>/<repo>:<path>`.
//! - **Adding one from this computer** ([`import_lora_file`]): the path is untrusted input,
//!   checked as a model file's is (`setup_plan::checked_disk_file_with`); its header is read
//!   where it is, and a file that is not a LoRA is refused before anything is copied. It is always
//!   copied (the engine's names are relative to one folder) into `<models>/@loras/local/`
//!   through a `.partial` file, hashed on the way with `studio-import-progress` events, as a
//!   model file's import is (`import::copy_to_partial`). Its id is `local:<SHA-256>`, so the
//!   same file added again is the same row, with one file. A copy no row owns after a failure
//!   is removed.
//! - **Deleting one** ([`delete_lora_files`]): refused while a run on this computer waits for
//!   it or uses it (`studio_lora_in_use`); else its row goes, then its file unless something
//!   else uses it or a download not finished yet writes it (`local_models::remove_unused_files`,
//!   inside the models folder only).
//!
//! Downloading and adding one need the note about local models accepted, as a model does.
//! What a LoRA's refusals say never calls it a model or a part (ruling S10-lora-words).
//!
//! Each command is a thin wrapper over a plain function taking what it reads
//! ([`LocalCmdDeps`]), as Get local models' are.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use models::download::FileSpec;
use models::hf::{Gated, HfError};
use models::sd::FileFacts;
use models::sd::detect::SdVersion;
use models::sd::inspect::{FileKind, LoraFormat, inspect_local};
use models::sd::remote::RemoteError;
use tauri::{AppHandle, Emitter, Manager, State};

use super::errors;
use super::import::{IMPORT_PROGRESS_EVENT, ProgressSink, Report, copy_to_partial, place_partial};
use super::local_cmd::{LocalCmdDeps, app_deps_for_writing};
use super::local_models::remove_unused_files;
use super::manager::StudioManager;
use super::setup_plan::{checked_disk_file_with, readable_file_name_with};
use crate::commands::only_main_window;
use crate::downloads::{DownloadManager, GroupMeta, LoraGroup, files_being_written};
use crate::dto::LoraDto;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The models folder's folder of LoRAs. Not a Hugging Face owner name, so never a model's
/// place.
pub const LORA_FOLDER: &str = "@loras";

/// The LoRA folder's folder of the LoRAs added from this computer: the first part of their
/// names.
const IMPORTED: &str = "local";

/// The name of a part with nothing of its own left.
const NO_NAME: &str = "lora";

/// The extensions the engine tries after a LoRA's name, in its order, when no file has the
/// name itself (`extract_and_remove_lora`, `examples/common/common.cpp:2357` at the pinned
/// commit).
const ENGINE_EXTENSIONS: [&str; 4] = [".gguf", ".safetensors", ".pt", ".ckpt"];

/// Studio's LoRA folder in the models folder `models_dir`: the engine's `--lora-model-dir`.
pub fn lora_folder(models_dir: &Path) -> PathBuf {
    models_dir.join(LORA_FOLDER)
}

/// One part of a LoRA's name: `s` without `:` `<` `>` `|` `\` and control characters (what
/// the engine's tag cannot carry, `runtime::sd::args::valid_lora_name`) and without `?` `*`
/// `"` (which Windows refuses in a file name; ruling S10-windows-names), with spaces and dots
/// trimmed at both ends (so never `.` or `..`, and no name Windows would trim); "lora" when
/// nothing is left. A Windows device name (`CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9`,
/// `LPT1`–`LPT9`, in any case, alone or before a dot) gets `_` after it. The same on every
/// system, so a LoRA has one name on every computer. Spaces inside and any other Unicode are
/// kept.
pub fn sanitize_lora_component(s: &str) -> String {
    let kept: String = s
        .chars()
        .filter(|c| !matches!(c, ':' | '<' | '>' | '|' | '\\' | '?' | '*' | '"') && !c.is_control())
        .collect();
    let trimmed = match kept.trim_matches([' ', '.']) {
        "" => NO_NAME,
        trimmed => trimmed,
    };
    match windows_device(trimmed) {
        Some(len) => format!("{}_{}", &trimmed[..len], &trimmed[len..]),
        None => trimmed.to_owned(),
    }
}

/// The length of the Windows device name `part` starts with, when it is one: the part before
/// its first dot, trailing spaces aside, is `CON`, `PRN`, `AUX`, `NUL`, `COM1`–`COM9` or
/// `LPT1`–`LPT9`, in any case. Windows opens the device for such a name, whatever follows the
/// dot.
fn windows_device(part: &str) -> Option<usize> {
    let base = part.split('.').next().unwrap_or(part).trim_end_matches(' ');
    let upper = base.to_ascii_uppercase();
    let device = matches!(
        upper.as_bytes(),
        [b'C', b'O', b'N']
            | [b'P', b'R', b'N']
            | [b'A', b'U', b'X']
            | [b'N', b'U', b'L']
            | [b'C', b'O', b'M', b'1'..=b'9']
            | [b'L', b'P', b'T', b'1'..=b'9']
    );
    device.then_some(base.len())
}

/// A file name without its last extension (`pixel art ü.safetensors` → `pixel art ü`).
fn stem(file_name: &str) -> &str {
    file_name
        .rsplit_once('.')
        .map_or(file_name, |(stem, _)| stem)
}

/// The name a LoRA from the file `file_name` is shown with: its stem as written, or the whole
/// name when that is empty.
fn shown_name(file_name: &str) -> String {
    match stem(file_name) {
        "" => file_name.to_owned(),
        stem => stem.to_owned(),
    }
}

/// The name of the LoRA downloaded from `repo`'s file `path`: `<owner>/<repo>/<path without
/// its extension>`, each part sanitised ([`sanitize_lora_component`]).
pub fn rel_name_for_download(repo: &str, path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').collect();
    if let Some(last) = parts.last_mut() {
        *last = stem(last);
    }
    repo.split('/')
        .chain(parts)
        .map(sanitize_lora_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// The name of a LoRA added from the file `file_name` on this computer: `local/<its stem,
/// sanitised>`, followed by " (2)", " (3)", … while `taken` has that name in any case (two
/// names that differ only in case are one file on Windows and macOS).
pub fn rel_name_for_import(file_name: &str, taken: &BTreeSet<String>) -> String {
    let base = format!("{IMPORTED}/{}", sanitize_lora_component(stem(file_name)));
    let taken: BTreeSet<String> = taken.iter().map(|t| t.to_lowercase()).collect();
    free_name(&base, |name| taken.contains(&name.to_lowercase()))
}

/// `base`, or the first of `base (2)`, `base (3)`, … that is not `taken`.
fn free_name(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let mut name = base.to_owned();
    let mut n = 1u32;
    while taken(&name) {
        n += 1;
        name = format!("{base} ({n})");
    }
    name
}

/// The extension a LoRA's file is saved with, from its file's name: `.gguf` for a `.gguf`
/// file, else `.safetensors` (a `.safetensors` or `.sft` file: the only other names Studio
/// reads, `setup_plan::readable_file_name`). The engine finds a `.sft` file under no name.
pub fn stored_extension(path: &str) -> &'static str {
    if path.to_lowercase().ends_with(".gguf") {
        ".gguf"
    } else {
        ".safetensors"
    }
}

/// The file of the LoRA named `rel_name` saved with extension `ext`: `<models>/@loras/
/// <rel_name><ext>`, one path component per part of the name.
fn lora_file(models_dir: &Path, rel_name: &str, ext: &str) -> PathBuf {
    in_lora_folder(&lora_folder(models_dir), rel_name, ext)
}

/// `<lora_dir>/<rel_name><ext>`, one path component per part of the name.
fn in_lora_folder(lora_dir: &Path, rel_name: &str, ext: &str) -> PathBuf {
    let mut path = lora_dir.to_path_buf();
    path.extend(rel_name.split('/'));
    let mut file = path.into_os_string();
    file.push(ext);
    PathBuf::from(file)
}

/// The file the engine reads for the LoRA named `rel_name` in the LoRA folder `lora_dir`, as
/// it looks for it (`extract_and_remove_lora`, `examples/common/common.cpp:2384-2399` at the
/// pinned commit): the name itself when anything is there, else the name with the first of
/// its extensions that is there; `None` when that is not a file (nothing at all, or a folder,
/// which the engine would try to read).
pub fn engine_lora_file(lora_dir: &Path, rel_name: &str) -> Option<PathBuf> {
    std::iter::once("")
        .chain(ENGINE_EXTENSIONS)
        .map(|ext| in_lora_folder(lora_dir, rel_name, ext))
        .find(|path| path.exists())
        .filter(|path| path.is_file())
}

/// Whether the engine would read something else for the LoRA named `rel_name` saved with
/// `ext`: anything at the name itself (it looks there first: a folder of other LoRAs), or a
/// file of the name with another extension it tries.
fn shadowed(models_dir: &Path, rel_name: &str, ext: &str) -> bool {
    lora_file(models_dir, rel_name, "").exists()
        || ENGINE_EXTENSIONS
            .iter()
            .filter(|e| **e != ext)
            .any(|e| lora_file(models_dir, rel_name, e).exists())
}

/// The names the entries of the imported LoRAs' folder `dir` take: each one's own name and its
/// name without its last extension, after `local/`. A name the engine would read one of them
/// for is not given to a new file.
fn names_in(dir: &Path) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        names.insert(format!("{IMPORTED}/{}", stem(&name)));
        names.insert(format!("{IMPORTED}/{name}"));
    }
    names
}

/// The store's word for a LoRA's format.
pub(crate) fn file_format(format: LoraFormat) -> store::LoraFileFormat {
    match format {
        LoraFormat::Kohya => store::LoraFileFormat::Kohya,
        LoraFormat::Peft => store::LoraFileFormat::Peft,
        LoraFormat::Lycoris => store::LoraFileFormat::Lycoris,
        LoraFormat::Diff => store::LoraFileFormat::Diff,
    }
}

/// A LoRA's format, from the store's word.
fn lora_format(format: store::LoraFileFormat) -> LoraFormat {
    match format {
        store::LoraFileFormat::Kohya => LoraFormat::Kohya,
        store::LoraFileFormat::Peft => LoraFormat::Peft,
        store::LoraFileFormat::Lycoris => LoraFormat::Lycoris,
        store::LoraFileFormat::Diff => LoraFormat::Diff,
    }
}

/// LoRA `row` as the webview gets it, the models folder being `models_dir`: missing when the
/// engine would find no file for it there ([`engine_lora_file`], the check a run makes),
/// whatever path its row was saved at (ruling S11-lora-list-path).
pub(crate) fn lora_dto(row: &store::GenLora, models_dir: &Path) -> LoraDto {
    LoraDto {
        id: row.id.clone(),
        name: row.name.clone(),
        base: row.base_hint.as_deref().and_then(SdVersion::from_word),
        format: lora_format(row.format),
        size_bytes: row.size_bytes,
        source: row.source.as_str().to_owned(),
        repo: row.repo.clone(),
        missing: engine_lora_file(&lora_folder(models_dir), &row.rel_name).is_none(),
    }
}

/// Every LoRA, newest first, each saying whether its file is gone, the models folder being
/// `models_dir`.
pub async fn list_loras(store: &store::Store, models_dir: &Path) -> AppResult<Vec<LoraDto>> {
    Ok(store
        .list_gen_loras()
        .await?
        .iter()
        .map(|row| lora_dto(row, models_dir))
        .collect())
}

/// The format and base version of the LoRA whose header says `facts`; any other file is
/// `studio_not_a_lora`, its details saying what it is.
fn lora_of(facts: &FileFacts) -> AppResult<(LoraFormat, Option<SdVersion>)> {
    let is = match &facts.kind {
        FileKind::Lora { format, base } => return Ok((*format, *base)),
        FileKind::Model { version, .. } => format!("a model for {}", version.word()),
        FileKind::Extra { slot } => format!("a {} part", slot.word()),
        FileKind::CannotRun { reason, .. } => format!("a file Studio cannot run ({reason:?})"),
    };
    Err(errors::not_a_lora(format!("the file is {is}")))
}

/// The format and base version of the LoRA file at `path`, from its header (read off the
/// async runtime); a file whose header does not read, or is not a LoRA's, is
/// `studio_not_a_lora`. A downloaded LoRA is checked with it before it is added.
pub(crate) async fn read_lora_file(path: &Path) -> AppResult<(LoraFormat, Option<SdVersion>)> {
    let read_from = path.to_path_buf();
    let facts = tokio::task::spawn_blocking(move || inspect_local(&read_from))
        .await
        .map_err(|e| AppError::internal(format!("reading a LoRA's header: {e}")))?
        .map_err(|e| errors::not_a_lora(format!("its header does not read: {e}")))?;
    lora_of(&facts)
}

// ---- downloading one ------------------------------------------------------------------------

/// A LoRA's download ready to start: its group's metadata and its one file, with where it
/// goes.
pub(crate) struct PreparedLora {
    pub meta: GroupMeta,
    pub files: Vec<(FileSpec, PathBuf)>,
}

/// A header read that failed: Hugging Face's own refusal (a gated file with no token is
/// `hf_needs_token`), or a header that does not read, which is no LoRA's.
fn header_error(e: RemoteError) -> AppError {
    match e {
        RemoteError::Hf(e) => errors::lora_hub_error(e),
        RemoteError::Index(e) => errors::not_a_lora(format!("its header does not read: {e}")),
    }
}

/// Download on the LoRA file `file` of `repo` at `commit` (a Hugging Face search's), nothing
/// started yet, every refusal in a LoRA's words (ruling S10-lora-words): the note about local
/// models must have been accepted (`studio_notice_needed`, as for a model); the listing at
/// that commit read again (`deps.hf`); a file not listed, one of a format Studio cannot read
/// (a pickle file: "Studio can't read .ckpt files"), a gated repo's file with no Hugging Face
/// token (`hf_needs_token`, before any byte is read), one a run on this computer holds
/// (`in_use`: `studio_lora_in_use`, ruling S10-lora-in-use), one with no published SHA-256
/// (`studio_no_checksum`) and one whose header is not a LoRA's (`studio_not_a_lora`) refused;
/// then its name ([`rel_name_for_download`]; the same LoRA downloaded again keeps its own, and
/// another's name is never taken) and its file, which must fit on the disk
/// (`studio_disk_too_small`). A file already there with its pinned size is checked instead of
/// fetched.
pub(crate) async fn prepare_lora_download(
    deps: &LocalCmdDeps,
    in_use: &(dyn Fn(&str) -> bool + Sync),
    repo: &str,
    commit: &str,
    file: &str,
) -> AppResult<PreparedLora> {
    if !deps.notice_accepted {
        return Err(errors::notice_needed());
    }
    let hf = deps
        .hf
        .as_ref()
        .ok_or_else(|| AppError::internal("a LoRA download without Hugging Face"))?;
    let pinned = |c: &str| c.len() == 40 && c.bytes().all(|b| b.is_ascii_hexdigit());
    let wrong_revision =
        || AppError::bad_request("The repository revision does not match this download.");
    if !pinned(commit) {
        return Err(wrong_revision());
    }
    let detail = hf
        .repo(repo, Some(commit))
        .await
        .map_err(errors::lora_hub_error)?;
    if detail.commit != commit {
        return Err(wrong_revision());
    }
    let listed = detail
        .files
        .iter()
        .find(|f| f.path == file)
        .ok_or_else(|| errors::lora_hub_error(HfError::FileNotFound))?;
    readable_file_name_with(&listed.path, errors::not_a_lora_format)?;
    // A gated repo's file with no token is refused before any byte is read (as a model's).
    if detail.gated != Gated::No && !hf.has_token() {
        return Err(errors::lora_hub_error(HfError::NeedsToken));
    }
    let id = format!("hf:{}:{}", detail.id, listed.path);
    if in_use(&id) {
        return Err(errors::lora_in_use_for_download());
    }
    let Some(sha256) = listed.sha256.clone() else {
        return Err(errors::lora_no_checksum());
    };
    let facts = deps
        .detect
        .read_one(hf, &detail, listed)
        .await
        .map_err(header_error)?;
    lora_of(&facts)?;

    let ext = stored_extension(&listed.path);
    let loras = deps.store.list_gen_loras().await?;
    let rel_name = match loras.iter().find(|l| l.id == id) {
        Some(again) => again.rel_name.clone(),
        None => {
            let others: BTreeSet<String> =
                loras.iter().map(|l| l.rel_name.to_lowercase()).collect();
            free_name(&rel_name_for_download(&detail.id, &listed.path), |name| {
                others.contains(&name.to_lowercase()) || shadowed(&deps.models_dir, name, ext)
            })
        }
    };
    let dest = lora_file(&deps.models_dir, &rel_name, ext);
    let spec = FileSpec {
        repo: detail.id.clone(),
        commit: detail.commit.clone(),
        path: listed.path.clone(),
        size: listed.size,
        sha256: Some(sha256.clone()),
    };
    let here = std::fs::metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == listed.size);
    let to_fetch = if here {
        0
    } else {
        listed
            .size
            .saturating_sub(models::download::resumable_bytes(&dest, &spec))
    };
    if deps.free_disk_bytes.is_some_and(|free| to_fetch > free) {
        return Err(errors::lora_disk_too_small(to_fetch));
    }
    let name = shown_name(listed.path.rsplit('/').next().unwrap_or(&listed.path));
    let meta = GroupMeta {
        repo: detail.id.clone(),
        commit: detail.commit.clone(),
        label: name.clone(),
        model_files: Vec::new(),
        projector: None,
        catalog_id: None,
        hub: None,
        parameters: None,
        image_file_for: None,
        image_files: None,
        generator: None,
        file_only: false,
        lora: Some(LoraGroup {
            id,
            name,
            rel_name,
            repo: detail.id.clone(),
            revision: detail.commit.clone(),
            path: listed.path.clone(),
            size: listed.size,
            sha256,
        }),
    };
    Ok(PreparedLora {
        meta,
        files: vec![(spec, dest)],
    })
}

// ---- adding one from this computer ----------------------------------------------------------

/// One import at a time names its LoRA, places its file and records it, so two never take one
/// name or one file.
static PLACING: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// *Add a LoRA file* on the file at `path` (untrusted; as the webview named it, which the
/// progress events carry), every refusal in a LoRA's words (ruling S10-lora-words):
/// 1. the note about local models must have been accepted (`studio_notice_needed`, as for a
///    model); the file is checked (`setup_plan::checked_disk_file_with`: canonical, a regular
///    file; a pickle file is refused by its name, another format with "This is not a LoRA file
///    Studio can read.") and its header read where it is ([`read_lora_file`]): one that does
///    not read, or is not a LoRA's, is `studio_not_a_lora`, before anything is copied; a file
///    larger than the free space is `studio_disk_too_small`;
/// 2. it is copied into `<models>/@loras/local/` through a `.partial` file, hashed on the way
///    (`import::copy_to_partial`, progress to `progress`);
/// 3. one import at a time: its id is `local:<SHA-256>`. The same file added before is that
///    row: its own file (its name and extension, in the current models folder), still whole
///    with the same SHA-256, is kept and the copy removed (a file gone or damaged is put back
///    from the copy). Else it takes the first free name ([`rel_name_for_import`]: another
///    LoRA's, or one the engine would read another file in the folder for, is taken), its copy
///    is renamed into place and it is recorded. A copy no row owns after a failure is removed;
///    a file a row owns, under any spelling of its path, never is (ruling
///    S10-reimport-own-file).
pub(crate) async fn import_lora_file(
    deps: &LocalCmdDeps,
    path: &Path,
    progress: &ProgressSink,
) -> AppResult<LoraDto> {
    if !deps.notice_accepted {
        return Err(errors::notice_needed());
    }
    let (src, size) = checked_disk_file_with(path, errors::not_a_lora_format)?;
    let name = src
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let lora = read_lora_file(&src).await?;
    if deps.free_disk_bytes.is_some_and(|free| size > free) {
        return Err(errors::lora_disk_too_small(size));
    }
    let mut report = Report::new(path.display().to_string(), progress.clone());
    let into = lora_folder(&deps.models_dir).join(IMPORTED);
    let (partial, sha256, report) = tokio::task::spawn_blocking(move || {
        copy_to_partial(&src, size, &into, &mut report).map(|(p, sha)| (p, sha, report))
    })
    .await
    .map_err(|e| errors::import_failed(format!("copying the file stopped: {e}")))??;
    let copy = Copied {
        name: &name,
        size,
        partial: &partial,
        sha256,
    };
    let added = place_and_record(deps, copy, lora, report).await;
    if added.is_err() {
        // Renamed into place, or removed, already when the failure came after.
        let _ = tokio::fs::remove_file(&partial).await;
    }
    added
}

/// A LoRA file from this computer copied into the LoRA folder, not placed yet: its file name,
/// its size, the partial file and its SHA-256.
struct Copied<'a> {
    name: &'a str,
    size: u64,
    partial: &'a Path,
    sha256: String,
}

/// Step 3 of [`import_lora_file`]: `copy` (a LoRA of `lora`'s format and base) placed and
/// recorded, one import at a time.
async fn place_and_record(
    deps: &LocalCmdDeps,
    copy: Copied<'_>,
    lora: (LoraFormat, Option<SdVersion>),
    mut report: Report,
) -> AppResult<LoraDto> {
    let _one = PLACING.lock().await;
    let id = format!("local:{}", copy.sha256);
    let loras = deps.store.list_gen_loras().await?;
    let before = loras.iter().find(|l| l.id == id);
    // The same file added before keeps its own name and extension, whatever this copy is
    // called: never a second copy of it.
    let (rel_name, ext) = match before {
        Some(row) => (row.rel_name.clone(), stored_extension(&row.file)),
        None => {
            let mut taken: BTreeSet<String> = loras.iter().map(|l| l.rel_name.clone()).collect();
            taken.extend(names_in(&lora_folder(&deps.models_dir).join(IMPORTED)));
            (
                rel_name_for_import(copy.name, &taken),
                stored_extension(copy.name),
            )
        }
    };
    let dest = lora_file(&deps.models_dir, &rel_name, ext);
    let (from, to, size, sha) = (
        copy.partial.to_path_buf(),
        dest.clone(),
        copy.size,
        copy.sha256.clone(),
    );
    tokio::task::spawn_blocking(move || place_partial(&from, &to, size, &sha, &mut report))
        .await
        .map_err(|e| errors::import_failed(format!("placing the copy stopped: {e}")))??;
    let placed = dest.display().to_string();
    let owns = |row: &store::GenLora| crate::model_options::same_file(&row.file, &placed);
    if let Some(row) = before
        && owns(row)
    {
        return Ok(lora_dto(row, &deps.models_dir));
    }
    let new = store::NewGenLora {
        id,
        name: before.map_or_else(|| shown_name(copy.name), |row| row.name.clone()),
        file: placed.clone(),
        rel_name,
        repo: None,
        revision: None,
        path: None,
        size_bytes: copy.size,
        sha256: copy.sha256,
        base_hint: lora.1.map(|b| b.word().to_owned()),
        format: file_format(lora.0),
        source: store::LoraSource::Import,
    };
    match deps.store.insert_gen_lora(new).await {
        Ok(row) => Ok(lora_dto(&row, &deps.models_dir)),
        Err(e) => {
            // The file placed here goes only when no row owns it, under any spelling.
            if !loras.iter().any(owns) {
                let _ = tokio::fs::remove_file(&dest).await;
            }
            Err(e.into())
        }
    }
}

// ---- deleting one ---------------------------------------------------------------------------

/// *Delete* on LoRA `id`: refused with `studio_lora_in_use` while a run on this computer waits
/// for it or uses it (`in_use`), deleting nothing; else its row goes, then its file unless an
/// installed model or another LoRA uses it, a download not finished yet writes it (`busy`), or
/// it is outside the models folder (`local_models::remove_unused_files`), then the folders that
/// leaves empty. Returns the files removed. A LoRA not there is `not_found`.
pub(crate) async fn delete_lora_files(
    store: &store::Store,
    models_dir: &Path,
    id: &str,
    in_use: &(dyn Fn(&str) -> bool + Sync),
    busy: &(dyn Fn(&Path) -> bool + Sync),
) -> AppResult<Vec<PathBuf>> {
    if in_use(id) {
        return Err(errors::lora_in_use());
    }
    let lora = store
        .delete_gen_lora(id)
        .await?
        .ok_or_else(|| AppError::not_found("LoRA"))?;
    remove_unused_files(store, models_dir, &[PathBuf::from(&lora.file)], busy).await
}

// ---- commands -------------------------------------------------------------------------------

/// The LoRAs, newest first. Sends nothing.
#[tauri::command]
pub async fn studio_loras(window: tauri::WebviewWindow, app: AppHandle) -> AppResult<Vec<LoraDto>> {
    only_main_window(window.label())?;
    let models_dir = crate::hub::models_dir(&app).await;
    list_loras(&app.state::<AppState>().store, &models_dir).await
}

/// Download on a Hugging Face search's LoRA file `file` of `repo` at `commit`: its download
/// group, which adds the LoRA once its file is checked ([`prepare_lora_download`]).
#[tauri::command]
pub async fn studio_lora_download(
    window: tauri::WebviewWindow,
    app: AppHandle,
    repo: String,
    commit: String,
    file: String,
) -> AppResult<String> {
    only_main_window(window.label())?;
    let studio = app.state::<Arc<StudioManager>>();
    let hf = crate::hub::hf_client(&app).await?;
    // Strict: what is installed guards what a download writes.
    let deps = app_deps_for_writing(&app, &studio, Some(hf)).await?;
    let in_use = |id: &str| studio.local_uses_lora(id);
    let prepared = prepare_lora_download(&deps, &in_use, &repo, &commit, &file).await?;
    let downloads = app.state::<Arc<DownloadManager>>().inner().clone();
    downloads.start(&app, prepared.meta, prepared.files).await
}

/// *Add a LoRA file* on the file at `path`, copied into Studio's LoRA folder with progress to
/// the main window ([`import_lora_file`]).
#[tauri::command]
pub async fn studio_lora_import(
    window: tauri::WebviewWindow,
    app: AppHandle,
    path: String,
) -> AppResult<LoraDto> {
    only_main_window(window.label())?;
    let studio = app.state::<Arc<StudioManager>>();
    let deps = app_deps_for_writing(&app, &studio, None).await?;
    let to = app.clone();
    let progress: ProgressSink = Arc::new(move |p| {
        let _ = to.emit_to(crate::MAIN_WINDOW, IMPORT_PROGRESS_EVENT, p);
    });
    import_lora_file(&deps, Path::new(&path), &progress).await
}

/// *Delete* on LoRA `id`, unless a run on this computer waits for it or uses it
/// ([`delete_lora_files`]).
#[tauri::command]
pub async fn studio_lora_delete(
    window: tauri::WebviewWindow,
    app: AppHandle,
    studio: State<'_, Arc<StudioManager>>,
    id: String,
) -> AppResult<()> {
    only_main_window(window.label())?;
    let deps = app_deps_for_writing(&app, &studio, None).await?;
    let writing = files_being_written(&deps.store, None).await?;
    let busy = |p: &Path| writing.contains(&p.display().to_string());
    let in_use = |id: &str| studio.local_uses_lora(id);
    delete_lora_files(&deps.store, &deps.models_dir, &id, &in_use, &busy).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use models::sd::detect::SdVersion;
    use models::sd::inspect::LoraFormat;
    use models::sd::testing::{file_of, fixture};

    use super::*;
    use crate::downloads::{
        DownloadManager, GroupMeta, files_being_written, finish_gen_group, finish_gen_group_while,
    };
    use crate::error::AppResult;
    use crate::studio::import::{ImportProgress, ProgressSink};
    use crate::studio::local_cmd::LocalCmdDeps;
    use crate::studio::local_cmd::tests::{
        COMMIT, RepoHub, TestRepo, gpu, hub, repo_deps, sdxl_repo,
    };
    use crate::studio::test_support::Scratch;

    /// The SDXL base repo's commit in the test Hub.
    const SDXL_REV: &str = COMMIT;
    const XLABS: &str = "XLabs-AI/flux-lora-collection";
    const REALISM: &str = "realism_lora_comfy_converted.safetensors";

    /// A file whose header is the detection fixture `name`'s tensor list, with no tensor data:
    /// detection reads only the header.
    fn fixture_bytes(name: &str) -> Vec<u8> {
        file_of(&fixture(name))
    }

    /// XLabs' FLUX.1 LoRA collection, serving the realism LoRA's header as its whole file.
    fn xlabs_repo() -> TestRepo {
        let bytes = fixture_bytes("flux-lora-realism");
        TestRepo::new(XLABS)
            .serving(REALISM, bytes.len() as u64, bytes.clone())
            .serving("realism_lora.sft", bytes.len() as u64, bytes)
            .serving(
                "realism_lora.ckpt",
                1_000,
                fixture_bytes("flux-lora-realism"),
            )
            .unpinned("unpinned_lora.safetensors", 1_000, "flux-lora-realism")
    }

    /// What the LoRA commands read in the app: Get local models' dependencies (a models folder
    /// of its own, 500 GB free), a download manager that records groups without running them,
    /// the runs on this computer (the LoRAs one holds) and the progress events kept.
    struct TestApp {
        deps: LocalCmdDeps,
        downloads: DownloadManager,
        held: Mutex<BTreeSet<String>>,
        events: Arc<Mutex<Vec<ImportProgress>>>,
    }

    impl TestApp {
        fn sink(&self) -> ProgressSink {
            let events = self.events.clone();
            Arc::new(move |p| events.lock().unwrap().push(p))
        }

        fn store(&self) -> &store::Store {
            &self.deps.store
        }
    }

    struct T {
        app: TestApp,
        models_dir: PathBuf,
        scratch: Scratch,
        _hub: RepoHub,
    }

    async fn harness() -> T {
        harness_with(Vec::new()).await
    }

    async fn harness_with(repos: Vec<TestRepo>) -> T {
        let scratch = Scratch::new();
        let served = hub(repos).await;
        let mut deps = repo_deps(&served, gpu(24, 64), &scratch);
        deps.notice_accepted = true;
        T {
            models_dir: deps.models_dir.clone(),
            app: TestApp {
                deps,
                downloads: DownloadManager::default(),
                held: Mutex::new(BTreeSet::new()),
                events: Arc::new(Mutex::new(Vec::new())),
            },
            scratch,
            _hub: served,
        }
    }

    impl T {
        fn write(path: &Path, bytes: Vec<u8>) -> PathBuf {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
            dunce::canonicalize(path).unwrap()
        }

        /// A file outside the models folder, at `rel` (made canonical, as the webview's picker
        /// gives it).
        fn outside_file(&self, rel: &str, bytes: Vec<u8>) -> PathBuf {
            Self::write(&self.scratch.path().join("outside").join(rel), bytes)
        }

        /// A run on this computer holds LoRA `id`, queued or running.
        fn hold_run_with_lora(&self, id: &str) {
            self.app.held.lock().unwrap().insert(id.to_owned());
        }

        /// The runs on this computer ended.
        fn end_run(&self) {
            self.app.held.lock().unwrap().clear();
        }

        /// Every file under `<models>/@loras`, anywhere under it.
        fn lora_files(&self) -> Vec<PathBuf> {
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
            walk(&lora_folder(&self.models_dir), &mut out);
            out.sort();
            out
        }

        async fn meta(&self, group: &str) -> GroupMeta {
            self.app
                .store()
                .get_setting(&format!("downloadGroup:{group}"))
                .await
                .unwrap()
                .unwrap()
        }

        /// Puts the file `group` fetches in place (what the test Hub serves for it) and
        /// finishes the group as its task does.
        async fn complete(&self, group: &str, bytes: &[u8]) {
            for row in self
                .app
                .store()
                .list_downloads(Some(group.to_owned()))
                .await
                .unwrap()
            {
                T::write(Path::new(&row.dest_path), bytes.to_vec());
            }
            finish_gen_group(self.app.store(), &self.models_dir, group)
                .await
                .unwrap();
        }
    }

    /// *Add a LoRA file* on the file at `src`, as `studio_lora_import` does.
    async fn import_lora(app: &TestApp, src: &Path) -> AppResult<LoraDto> {
        import_lora_file(&app.deps, src, &app.sink()).await
    }

    /// Download on `repo`'s LoRA `file` at `commit`, as `studio_lora_download` does: its group
    /// recorded, not run.
    async fn download_lora(
        app: &TestApp,
        repo: &str,
        commit: &str,
        file: &str,
    ) -> AppResult<String> {
        let held = app.held.lock().unwrap().clone();
        let in_use = |id: &str| held.contains(id);
        let prepared = prepare_lora_download(&app.deps, &in_use, repo, commit, file).await?;
        app.downloads
            .record_group(app.store(), &prepared.meta, &prepared.files)
            .await
    }

    /// Delete on LoRA `id`, as `studio_lora_delete` does.
    async fn delete_lora(app: &TestApp, id: &str) -> AppResult<Vec<PathBuf>> {
        let writing = files_being_written(app.store(), None).await?;
        let busy = |p: &Path| writing.contains(&p.display().to_string());
        let held = app.held.lock().unwrap().clone();
        let in_use = |id: &str| held.contains(id);
        delete_lora_files(app.store(), &app.deps.models_dir, id, &in_use, &busy).await
    }

    // ---- the brief's tests ------------------------------------------------------------------

    /// Ruling S11-lora-list-path: the LoRA list says a LoRA is missing as a run would find it
    /// (`job_for`'s check, `engine_lora_file`): by its name in the current models folder's
    /// `@loras`, in the engine's extension order, never by the path its row was saved at.
    #[tokio::test]
    async fn the_list_finds_a_lora_where_the_engine_reads_it() {
        let t = harness().await;
        let saved = |rel_name: &str, file: &Path| store::NewGenLora {
            id: format!("local:{}", rel_name.len()),
            name: rel_name.rsplit('/').next().unwrap().to_owned(),
            file: file.display().to_string(),
            rel_name: rel_name.to_owned(),
            repo: None,
            revision: None,
            path: None,
            size_bytes: 3,
            sha256: "c".repeat(64),
            base_hint: None,
            format: store::LoraFileFormat::Kohya,
            source: store::LoraSource::Import,
        };
        // Saved while the models folder was elsewhere: the old file is still there.
        let old = T::write(
            &t.scratch
                .path()
                .join("old models/@loras/local/ink.safetensors"),
            b"abc".to_vec(),
        );
        t.app
            .store()
            .insert_gen_lora(saved("local/ink", &old))
            .await
            .unwrap();
        // In the current folder, under its name with the engine's `.gguf`.
        let here = T::write(
            &t.models_dir.join("@loras/local/pixel art ü.gguf"),
            b"abc".to_vec(),
        );
        t.app
            .store()
            .insert_gen_lora(saved("local/pixel art ü", &here))
            .await
            .unwrap();
        let listed = list_loras(t.app.store(), &t.models_dir).await.unwrap();
        let missing = |name: &str| listed.iter().find(|l| l.name == name).unwrap().missing;
        assert!(old.is_file());
        assert!(missing("ink"), "its file is not in the current @loras");
        assert!(!missing("pixel art ü"));
    }

    #[test]
    fn lora_names_keep_spaces_and_unicode_and_drop_colons_and_angles() {
        let none = BTreeSet::new();
        assert_eq!(
            rel_name_for_import("pixel art ü.safetensors", &none),
            "local/pixel art ü"
        );
        assert_eq!(
            rel_name_for_import("日本語 スタイル.safetensors", &none),
            "local/日本語 スタイル"
        );
        assert_eq!(
            rel_name_for_import("a:b<c>d|e.safetensors", &none),
            "local/abcde"
        );
        assert_eq!(rel_name_for_import(" . .safetensors", &none), "local/lora");
        assert_eq!(
            rel_name_for_download(
                "XLabs-AI/flux-lora-collection",
                "realism_lora_comfy_converted.safetensors"
            ),
            "XLabs-AI/flux-lora-collection/realism_lora_comfy_converted"
        );
        for n in [
            rel_name_for_import("a:b<c>d|e.safetensors", &none),
            rel_name_for_download("o/r", "dir/x y.sft"),
        ] {
            assert!(runtime::sd::args::valid_lora_name(&n), "{n}");
        }
    }

    #[test]
    fn two_imports_with_the_same_name_get_distinct_names() {
        assert_eq!(
            rel_name_for_import(
                "style.safetensors",
                &BTreeSet::from(["local/style".to_string()])
            ),
            "local/style (2)"
        );
    }

    #[tokio::test]
    async fn an_imported_lora_is_copied_into_the_lora_folder_and_registered_with_its_base() {
        let t = harness().await;
        let src = t.outside_file(
            "Ü dir/pixel art xl.safetensors",
            fixture_bytes("sdxl-lora-pixel-art"),
        );
        let l = import_lora(&t.app, &src).await.unwrap();
        assert_eq!(
            (l.name.as_str(), l.base, l.format),
            ("pixel art xl", Some(SdVersion::Sdxl), LoraFormat::Kohya)
        );
        assert!(
            t.models_dir
                .join("@loras/local/pixel art xl.safetensors")
                .exists()
                && src.exists()
        );
    }

    #[tokio::test]
    async fn a_sft_lora_is_saved_as_safetensors_so_the_engine_finds_it() {
        let t = harness().await;
        import_lora(
            &t.app,
            &t.outside_file("s.sft", fixture_bytes("flux-lora-realism")),
        )
        .await
        .unwrap();
        assert!(t.models_dir.join("@loras/local/s.safetensors").exists());
    }

    #[tokio::test]
    async fn a_lora_download_is_refused_for_a_file_that_is_not_a_lora() {
        let t = harness_with(vec![sdxl_repo()]).await;
        assert_eq!(
            download_lora(
                &t.app,
                "stabilityai/stable-diffusion-xl-base-1.0",
                SDXL_REV,
                "sd_xl_base_1.0.safetensors"
            )
            .await
            .unwrap_err()
            .code,
            "studio_not_a_lora"
        );
    }

    #[tokio::test]
    async fn deleting_a_lora_in_use_is_refused_and_otherwise_removes_its_file() {
        let t = harness().await;
        let l = import_lora(
            &t.app,
            &t.outside_file("s.safetensors", fixture_bytes("flux-lora-realism")),
        )
        .await
        .unwrap();
        t.hold_run_with_lora(&l.id);
        assert_eq!(
            delete_lora(&t.app, &l.id).await.unwrap_err().code,
            "studio_lora_in_use"
        );
        t.end_run();
        delete_lora(&t.app, &l.id).await.unwrap();
        assert!(!t.models_dir.join("@loras/local/s.safetensors").exists());
    }

    // ---- the rulings carried from S8 and S1 --------------------------------------------------

    /// A file whose header is not a LoRA is refused from its header at the source, before
    /// anything is copied: nothing lands in `@loras` and no progress is sent.
    #[tokio::test]
    async fn a_file_that_is_not_a_lora_is_refused_before_anything_is_copied() {
        let t = harness().await;
        let model = t.outside_file("my sd15.safetensors", fixture_bytes("sd15"));
        let e = import_lora(&t.app, &model).await.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            ("studio_not_a_lora", "This file isn't a LoRA.")
        );
        assert!(t.lora_files().is_empty(), "{:?}", t.lora_files());
        assert!(t.app.events.lock().unwrap().is_empty());
        assert!(t.app.store().list_gen_loras().await.unwrap().is_empty());
        // A pickle file is refused by its name, never opened.
        let pickle = t.outside_file("old.ckpt", b"not read".to_vec());
        assert_eq!(
            import_lora(&t.app, &pickle).await.unwrap_err().message,
            "Studio can't read .ckpt files"
        );
        assert!(t.lora_files().is_empty());
    }

    /// The same file imported again (from another place, under another name) is the same
    /// row, with one file on disk: the second copy is removed once its SHA-256 matches.
    #[tokio::test]
    async fn importing_the_same_file_again_gives_the_same_row_and_one_file() {
        let t = harness().await;
        let bytes = fixture_bytes("sdxl-lora-pixel-art");
        let first = import_lora(
            &t.app,
            &t.outside_file("a/pixel.safetensors", bytes.clone()),
        )
        .await
        .unwrap();
        let sha =
            models::download::sha256_file(&t.models_dir.join("@loras/local/pixel.safetensors"))
                .await
                .unwrap();
        assert_eq!(first.id, format!("local:{sha}"));
        let again = import_lora(
            &t.app,
            &t.outside_file("b/another name.safetensors", bytes.clone()),
        )
        .await
        .unwrap();
        assert_eq!(again, first);
        assert_eq!(
            t.lora_files(),
            [t.models_dir.join("@loras/local/pixel.safetensors")]
        );
        assert_eq!(t.app.store().list_gen_loras().await.unwrap().len(), 1);
        // The copy is checked by reading it through, then kept.
        let events = t.app.events.lock().unwrap().clone();
        assert!(
            events
                .iter()
                .any(|e| e.stage == crate::studio::import::ImportStage::Hashing)
        );
        assert!(events.iter().all(|e| e.total_bytes == bytes.len() as u64));

        // Its file gone, the same file imported again puts it back under its own name.
        std::fs::remove_file(t.models_dir.join("@loras/local/pixel.safetensors")).unwrap();
        assert!(list_loras(t.app.store(), &t.models_dir).await.unwrap()[0].missing);
        let back = import_lora(&t.app, &t.outside_file("c/x.safetensors", bytes))
            .await
            .unwrap();
        assert_eq!((back.id.as_str(), back.missing), (first.id.as_str(), false));
        assert_eq!(
            t.lora_files(),
            [t.models_dir.join("@loras/local/pixel.safetensors")]
        );
    }

    /// Two different files with the same name, imported at once, each get a name of their own
    /// and a file of their own: the name is chosen and the file placed one import at a time.
    #[tokio::test]
    async fn two_files_with_one_name_imported_at_once_get_names_of_their_own() {
        let t = harness().await;
        let a = t.outside_file("a/style.safetensors", fixture_bytes("sdxl-lora-pixel-art"));
        let b = t.outside_file("b/style.safetensors", fixture_bytes("flux-lora-realism"));
        let (one, two) = tokio::join!(import_lora(&t.app, &a), import_lora(&t.app, &b));
        let (one, two) = (one.unwrap(), two.unwrap());
        assert_ne!(one.id, two.id);
        assert_eq!((one.name.as_str(), two.name.as_str()), ("style", "style"));
        let rel: BTreeSet<String> = t
            .app
            .store()
            .list_gen_loras()
            .await
            .unwrap()
            .into_iter()
            .map(|l| l.rel_name)
            .collect();
        assert_eq!(
            rel,
            BTreeSet::from(["local/style".to_owned(), "local/style (2)".to_owned()])
        );
        assert_eq!(
            t.lora_files(),
            [
                t.models_dir.join("@loras/local/style (2).safetensors"),
                t.models_dir.join("@loras/local/style.safetensors"),
            ]
        );
    }

    /// A name is taken by a LoRA's row, and by anything in the folder the engine would find
    /// for it first: the name itself (a folder), or a file of it with any extension.
    #[test]
    fn a_name_is_taken_by_a_row_or_by_what_the_engine_would_find_for_it() {
        let taken = BTreeSet::from(["local/Style".to_owned()]);
        assert_eq!(
            rel_name_for_import("style.safetensors", &taken),
            "local/style (2)",
            "a name in another case is the same file on Windows and macOS"
        );
        assert_eq!(
            rel_name_for_import(
                "STYLE.safetensors",
                &BTreeSet::from(["local/style".to_owned()])
            ),
            "local/STYLE (2)",
            "whichever side has the capitals"
        );
        let taken = BTreeSet::from(["local/x".to_owned(), "local/x (2)".to_owned()]);
        assert_eq!(rel_name_for_import("x.gguf", &taken), "local/x (3)");
        assert_eq!(stored_extension("a/b.SFT"), ".safetensors");
        assert_eq!(stored_extension("b.safetensors"), ".safetensors");
        assert_eq!(stored_extension("b.Q4.gguf"), ".gguf");
        assert_eq!(sanitize_lora_component(" ..a\\b\u{7}c.. "), "abc");
        assert_eq!(sanitize_lora_component(".."), "lora");
    }

    /// A file left in the LoRA folder that no row owns keeps its name: the engine would read
    /// it for that name (it tries `.gguf` before `.safetensors`), so a new LoRA, downloaded or
    /// imported, takes the next name instead.
    #[tokio::test]
    async fn a_name_the_engine_would_read_another_file_for_is_not_given() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let left = |rel: &str| {
            T::write(
                &lora_folder(&t.models_dir).join(rel),
                b"left behind".to_vec(),
            );
        };
        left("local/style.gguf");
        left("XLabs-AI/flux-lora-collection/realism_lora.gguf");
        let l = import_lora(
            &t.app,
            &t.outside_file("style.safetensors", fixture_bytes("sdxl-lora-pixel-art")),
        )
        .await
        .unwrap();
        let row = t.app.store().get_gen_lora(&l.id).await.unwrap().unwrap();
        assert_eq!(row.rel_name, "local/style (2)");
        let group = download_lora(&t.app, XLABS, COMMIT, "realism_lora.sft")
            .await
            .unwrap();
        assert_eq!(
            t.meta(&group).await.lora.unwrap().rel_name,
            "XLabs-AI/flux-lora-collection/realism_lora (2)"
        );
    }

    // ---- downloads ----------------------------------------------------------------------------

    /// A LoRA from a Hugging Face search downloads like a model: pinned by its repository,
    /// commit, path, size and SHA-256, to `<models>/@loras/<owner>/<repo>/<path without its
    /// extension>.safetensors`, and it is added once its file is checked, with its header's
    /// base and format. Downloaded again, it keeps its name and file.
    #[tokio::test]
    async fn a_lora_download_lands_in_the_lora_folder_and_is_added_once_checked() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let group = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        let meta = t.meta(&group).await;
        let lora = meta.lora.clone().unwrap();
        let id = format!("hf:{XLABS}:{REALISM}");
        assert_eq!(
            (lora.id.as_str(), lora.name.as_str(), lora.rel_name.as_str()),
            (
                id.as_str(),
                "realism_lora_comfy_converted",
                "XLabs-AI/flux-lora-collection/realism_lora_comfy_converted"
            )
        );
        assert!(meta.generator.is_none() && !meta.file_only);
        let rows = t
            .app
            .store()
            .list_downloads(Some(group.clone()))
            .await
            .unwrap();
        let dest = t
            .models_dir
            .join("@loras/XLabs-AI/flux-lora-collection/realism_lora_comfy_converted.safetensors");
        assert_eq!(rows.len(), 1);
        assert_eq!(Path::new(&rows[0].dest_path), dest);
        assert_eq!(rows[0].sha256, lora.sha256);
        assert!(
            list_loras(t.app.store(), &t.models_dir)
                .await
                .unwrap()
                .is_empty(),
            "not yet"
        );

        let bytes = fixture_bytes("flux-lora-realism");
        t.complete(&group, &bytes).await;
        let listed = list_loras(t.app.store(), &t.models_dir).await.unwrap();
        assert_eq!(
            listed,
            [LoraDto {
                id: id.clone(),
                name: "realism_lora_comfy_converted".into(),
                base: Some(SdVersion::Flux),
                format: LoraFormat::Kohya,
                size_bytes: bytes.len() as u64,
                source: "search".into(),
                repo: Some(XLABS.into()),
                missing: false,
            }]
        );
        let row = t.app.store().get_gen_lora(&id).await.unwrap().unwrap();
        assert_eq!(
            (
                row.revision.as_deref(),
                row.path.as_deref(),
                Path::new(&row.file)
            ),
            (Some(COMMIT), Some(REALISM), dest.as_path())
        );

        // Downloaded again: the same name and file, checked in place.
        let again = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        assert_eq!(t.meta(&again).await.lora.unwrap().rel_name, lora.rel_name);
        t.complete(&again, &bytes).await;
        assert_eq!(
            list_loras(t.app.store(), &t.models_dir)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(t.lora_files(), [dest]);
    }

    /// A `.sft` file is saved as `.safetensors` (the engine tries `.gguf`, `.safetensors`,
    /// `.pt` and `.ckpt` only); a name another LoRA has is followed by " (2)".
    #[tokio::test]
    async fn a_downloaded_lora_whose_name_another_has_gets_its_own() {
        let bytes = fixture_bytes("flux-lora-realism");
        let repo = xlabs_repo().serving(
            "realism_lora.safetensors",
            bytes.len() as u64,
            bytes.clone(),
        );
        let t = harness_with(vec![repo]).await;
        let first = download_lora(&t.app, XLABS, COMMIT, "realism_lora.safetensors")
            .await
            .unwrap();
        t.complete(&first, &bytes).await;
        let second = download_lora(&t.app, XLABS, COMMIT, "realism_lora.sft")
            .await
            .unwrap();
        let meta = t.meta(&second).await;
        assert_eq!(
            meta.lora.unwrap().rel_name,
            "XLabs-AI/flux-lora-collection/realism_lora (2)"
        );
        t.complete(&second, &bytes).await;
        assert_eq!(
            t.lora_files(),
            [
                t.models_dir
                    .join("@loras/XLabs-AI/flux-lora-collection/realism_lora (2).safetensors"),
                t.models_dir
                    .join("@loras/XLabs-AI/flux-lora-collection/realism_lora.safetensors"),
            ]
        );
        assert_eq!(
            list_loras(t.app.store(), &t.models_dir)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    /// Before anything starts: a pickle file is refused by its name (Studio can't read its
    /// header), and a file with no published SHA-256 is never downloaded.
    #[tokio::test]
    async fn a_lora_download_is_refused_for_a_pickle_or_an_unpinned_file() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let pickle = download_lora(&t.app, XLABS, COMMIT, "realism_lora.ckpt")
            .await
            .unwrap_err();
        assert_eq!(pickle.message, "Studio can't read .ckpt files");
        let unpinned = download_lora(&t.app, XLABS, COMMIT, "unpinned_lora.safetensors")
            .await
            .unwrap_err();
        assert_eq!(unpinned.code, "studio_no_checksum");
        let other = download_lora(&t.app, XLABS, &"1".repeat(40), REALISM)
            .await
            .unwrap_err();
        assert_eq!(other.code, "hf_not_found");
        assert!(t.app.store().list_downloads(None).await.unwrap().is_empty());
    }

    /// A downloaded file whose own header is not a LoRA is not added, and the file the
    /// download wrote goes.
    #[tokio::test]
    async fn a_downloaded_file_that_is_not_a_lora_is_not_added_and_goes() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let group = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        let len = fixture_bytes("flux-lora-realism").len();
        let rows = t
            .app
            .store()
            .list_downloads(Some(group.clone()))
            .await
            .unwrap();
        T::write(Path::new(&rows[0].dest_path), vec![0u8; len]);
        let e = finish_gen_group(t.app.store(), &t.models_dir, &group)
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_not_a_lora");
        assert!(t.lora_files().is_empty());
        assert!(
            list_loras(t.app.store(), &t.models_dir)
                .await
                .unwrap()
                .is_empty()
        );
    }

    // ---- the list -----------------------------------------------------------------------------

    /// Get local models lists the LoRAs, newest first, each saying whether its file is gone.
    #[tokio::test]
    async fn get_local_models_lists_the_loras_and_says_which_file_is_gone() {
        let t = harness().await;
        let pixel = import_lora(
            &t.app,
            &t.outside_file(
                "pixel art ü.safetensors",
                fixture_bytes("sdxl-lora-pixel-art"),
            ),
        )
        .await
        .unwrap();
        let realism = import_lora(
            &t.app,
            &t.outside_file("realism.safetensors", fixture_bytes("flux-lora-realism")),
        )
        .await
        .unwrap();
        assert_eq!(
            (pixel.source.as_str(), pixel.repo.as_deref(), pixel.missing),
            ("import", None, false)
        );
        std::fs::remove_file(t.models_dir.join("@loras/local/realism.safetensors")).unwrap();
        let listed = crate::studio::local_cmd::local_models(&t.app.deps)
            .await
            .unwrap()
            .loras;
        assert_eq!(
            listed
                .iter()
                .map(|l| (l.id.as_str(), l.missing))
                .collect::<Vec<_>>(),
            [(realism.id.as_str(), true), (pixel.id.as_str(), false)]
        );
        let wire = serde_json::to_value(&listed[1]).unwrap();
        assert_eq!(
            wire,
            serde_json::json!({
                "id": pixel.id, "name": "pixel art ü", "base": "sdxl", "format": "kohya",
                "sizeBytes": fixture_bytes("sdxl-lora-pixel-art").len(), "source": "import",
                "repo": null, "missing": false,
            })
        );
    }

    /// A LoRA's file is kept while a download not finished yet writes it; the row goes.
    #[tokio::test]
    async fn deleting_a_lora_keeps_a_file_a_download_writes() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let bytes = fixture_bytes("flux-lora-realism");
        let group = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        t.complete(&group, &bytes).await;
        let id = format!("hf:{XLABS}:{REALISM}");
        // Downloaded again, not finished yet.
        let _again = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        assert_eq!(
            delete_lora(&t.app, &id).await.unwrap(),
            Vec::<PathBuf>::new()
        );
        assert!(t.app.store().get_gen_lora(&id).await.unwrap().is_none());
        assert_eq!(t.lora_files().len(), 1);
        assert_eq!(
            delete_lora(&t.app, &id).await.unwrap_err().code,
            "not_found"
        );
    }

    // ---- fix round 1 ----------------------------------------------------------------------------

    /// Ruling S10-lora-notice: a LoRA is downloaded or added only once the note about local
    /// models was accepted, as a model is; refused, nothing is read through, copied or recorded.
    #[tokio::test]
    async fn downloading_or_adding_a_lora_waits_for_the_note_about_local_models() {
        let mut t = harness_with(vec![xlabs_repo()]).await;
        t.app.deps.notice_accepted = false;
        let src = t.outside_file("s.safetensors", fixture_bytes("flux-lora-realism"));
        let e = import_lora(&t.app, &src).await.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str()),
            (
                "studio_notice_needed",
                "Read the note about local models first."
            )
        );
        assert!(t.lora_files().is_empty() && t.app.events.lock().unwrap().is_empty());
        let e = download_lora(&t.app, XLABS, COMMIT, REALISM)
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_notice_needed");
        assert_eq!(t._hub.header_reads(), 0, "no header read before the note");
        assert!(t.app.store().list_downloads(None).await.unwrap().is_empty());
        assert!(t.app.store().list_gen_loras().await.unwrap().is_empty());

        t.app.deps.notice_accepted = true;
        import_lora(&t.app, &src).await.unwrap();
        download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
    }

    /// Ruling S10-lora-words: what a LoRA's download and import refuse with calls it a LoRA,
    /// never a model or a part: an unreadable format, no checksum, a file not listed, a gated
    /// repo with no token (refused before any byte is read), no room on the disk, and another
    /// download writing its file.
    #[tokio::test]
    async fn a_loras_refusals_call_it_a_lora() {
        let bytes = fixture_bytes("flux-lora-realism");
        let gated = TestRepo::new("someone/gated-loras")
            .serving("g.safetensors", bytes.len() as u64, bytes.clone())
            .gated();
        let repo = xlabs_repo()
            .unpinned("notes.txt", 10, "flux-lora-realism")
            .serving(
                "realism_lora.safetensors",
                bytes.len() as u64,
                bytes.clone(),
            );
        let mut t = harness_with(vec![repo, gated]).await;
        let mut seen = Vec::new();
        let mut refused = |e: AppError, code: &str, message: &str| {
            assert_eq!(
                (e.code.as_str(), e.message.as_str()),
                (code, message),
                "{e:?}"
            );
            seen.push(e);
        };

        let notes = download_lora(&t.app, XLABS, COMMIT, "notes.txt").await;
        refused(
            notes.unwrap_err(),
            "bad_request",
            "This is not a LoRA file Studio can read.",
        );
        let unpinned = download_lora(&t.app, XLABS, COMMIT, "unpinned_lora.safetensors").await;
        let e = unpinned.unwrap_err();
        assert_eq!(e.next_step, "Choose another LoRA file.");
        refused(
            e,
            "studio_no_checksum",
            "Hugging Face publishes no checksum for this file, so Studio can't check it.",
        );
        let missing = download_lora(&t.app, XLABS, COMMIT, "nope.safetensors").await;
        let e = missing.unwrap_err();
        assert_eq!(e.next_step, "Open the repository again.");
        refused(
            e,
            "hf_file_not_found",
            "That file is not in the repository.",
        );
        let reads = t._hub.header_reads();
        let gated = download_lora(&t.app, "someone/gated-loras", COMMIT, "g.safetensors").await;
        refused(gated.unwrap_err(), "hf_needs_token", "This LoRA is gated.");
        assert_eq!(
            t._hub.header_reads(),
            reads,
            "no byte of a gated file is read"
        );

        // Another download writes its file: `x.safetensors` and `x.sft` meet in one name.
        download_lora(&t.app, XLABS, COMMIT, "realism_lora.safetensors")
            .await
            .unwrap();
        let busy = download_lora(&t.app, XLABS, COMMIT, "realism_lora.sft").await;
        refused(
            busy.unwrap_err(),
            "already_downloading",
            "This LoRA's file is part of another download, which has stopped.",
        );

        let notes = t.outside_file("notes.txt", b"not weights".to_vec());
        let e = import_lora(&t.app, &notes).await.unwrap_err();
        refused(e, "bad_request", "This is not a LoRA file Studio can read.");

        t.app.deps.free_disk_bytes = Some(1);
        let full = download_lora(&t.app, XLABS, COMMIT, REALISM).await;
        refused(
            full.unwrap_err(),
            "studio_disk_too_small",
            "There is not enough free space for this LoRA.",
        );
        let src = t.outside_file("s.safetensors", bytes);
        let full = import_lora(&t.app, &src).await;
        refused(
            full.unwrap_err(),
            "studio_disk_too_small",
            "There is not enough free space for this LoRA.",
        );
        assert!(t.lora_files().is_empty());

        for e in &seen {
            for text in [&e.message, &e.next_step] {
                let places = text
                    .to_lowercase()
                    .replace("models folder", "")
                    .replace("settings → models", "");
                assert!(
                    !places.contains("model") && !places.contains("this part"),
                    "{e:?} says {text:?}"
                );
            }
        }
    }

    /// The brief's check on import too: a file whose header does not read (cut short) is "not
    /// a LoRA", as the download path says, refused before anything is copied.
    #[tokio::test]
    async fn a_file_whose_header_does_not_read_is_not_a_lora() {
        let t = harness().await;
        let mut bytes = fixture_bytes("sdxl-lora-pixel-art");
        bytes.truncate(bytes.len() / 2);
        let cut = t.outside_file("cut short.safetensors", bytes);
        let e = import_lora(&t.app, &cut).await.unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_not_a_lora",
                "This file isn't a LoRA.",
                "Choose a LoRA file."
            )
        );
        assert!(t.lora_files().is_empty());
        assert!(t.app.events.lock().unwrap().is_empty());
        assert!(t.app.store().list_gen_loras().await.unwrap().is_empty());
    }

    /// Ruling S10-reimport-own-file: the same bytes added again under another extension, or
    /// with the row's file spelled another way, are that row and its own file: never a second
    /// copy, and the row's file is never rewritten or removed.
    #[tokio::test]
    async fn the_same_file_under_another_name_or_spelling_keeps_its_own_file() {
        let t = harness().await;
        let bytes = fixture_bytes("sdxl-lora-pixel-art");
        let first = import_lora(&t.app, &t.outside_file("pixel.safetensors", bytes.clone()))
            .await
            .unwrap();
        let own = t.models_dir.join("@loras/local/pixel.safetensors");
        // The same bytes named `.gguf` (Studio reads the header, not the name).
        let again = import_lora(&t.app, &t.outside_file("renamed.gguf", bytes.clone()))
            .await
            .unwrap();
        assert_eq!(again, first);
        assert_eq!(t.lora_files(), std::slice::from_ref(&own));

        // The row's file spelled another way: still its own file.
        let row = t
            .app
            .store()
            .get_gen_lora(&first.id)
            .await
            .unwrap()
            .unwrap();
        let spelled = t
            .models_dir
            .join("@loras")
            .join("local")
            .join("..")
            .join("local")
            .join("pixel.safetensors")
            .display()
            .to_string();
        t.app
            .store()
            .insert_gen_lora(store::NewGenLora {
                id: row.id.clone(),
                name: row.name.clone(),
                file: spelled.clone(),
                rel_name: row.rel_name.clone(),
                repo: None,
                revision: None,
                path: None,
                size_bytes: row.size_bytes,
                sha256: row.sha256.clone(),
                base_hint: row.base_hint.clone(),
                format: row.format,
                source: row.source,
            })
            .await
            .unwrap();
        let third = import_lora(&t.app, &t.outside_file("third.safetensors", bytes))
            .await
            .unwrap();
        assert_eq!(third, first);
        let row = t
            .app
            .store()
            .get_gen_lora(&first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.file, spelled, "the row is not rewritten");
        assert_eq!(t.lora_files(), [own]);
    }

    /// Ruling S10-lora-in-use: a LoRA a run on this computer holds is not downloaded again:
    /// refused when it is asked for, and, when a run took it up meanwhile, when it would be
    /// added (its row and file left as they are; it is added once the run ends).
    #[tokio::test]
    async fn a_lora_a_run_holds_is_not_downloaded_again() {
        let t = harness_with(vec![xlabs_repo()]).await;
        let bytes = fixture_bytes("flux-lora-realism");
        let group = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        t.complete(&group, &bytes).await;
        let id = format!("hf:{XLABS}:{REALISM}");
        let added = t.app.store().get_gen_lora(&id).await.unwrap().unwrap();

        t.hold_run_with_lora(&id);
        let e = download_lora(&t.app, XLABS, COMMIT, REALISM)
            .await
            .unwrap_err();
        assert_eq!(
            (e.code.as_str(), e.message.as_str(), e.next_step.as_str()),
            (
                "studio_lora_in_use",
                "A run on this computer is using this LoRA.",
                "Stop the run, then download it again."
            )
        );
        let rows = t.app.store().list_downloads(None).await.unwrap();
        assert_eq!(rows.len(), 1, "no new download");

        t.end_run();
        let again = download_lora(&t.app, XLABS, COMMIT, REALISM).await.unwrap();
        t.hold_run_with_lora(&id);
        let held = |l: &str| l == id;
        let e = finish_gen_group_while(t.app.store(), &t.models_dir, &again, &held)
            .await
            .unwrap_err();
        assert_eq!(e.code, "studio_lora_in_use");
        assert_eq!(
            t.app.store().get_gen_lora(&id).await.unwrap().unwrap(),
            added,
            "its row is left as it was"
        );
        assert!(Path::new(&added.file).is_file());
        t.end_run();
        finish_gen_group(t.app.store(), &t.models_dir, &again)
            .await
            .unwrap();
        assert_eq!(
            list_loras(t.app.store(), &t.models_dir)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    /// Ruling S10-windows-names: a name never has a character Windows refuses in a file name
    /// (`? * "`) or a Windows device name (`CON`, `NUL`, `COM1`, … alone or before a dot): the
    /// device name gets `_` after it. The same on every system; every name still reads back in
    /// the engine's tag and passes the store's rule.
    #[test]
    fn windows_refused_characters_and_device_names_never_make_a_name() {
        let none = BTreeSet::new();
        for (part, want) in [
            ("a?b*c\"d", "abcd"),
            ("CON", "CON_"),
            ("con", "con_"),
            ("Nul.v2", "Nul_.v2"),
            ("COM1", "COM1_"),
            ("lpt9", "lpt9_"),
            ("AUX .x", "AUX_ .x"),
            ("prn", "prn_"),
            ("COM10", "COM10"),
            ("COM0", "COM0"),
            ("CONSOLE", "CONSOLE"),
            ("console.x", "console.x"),
            ("LPT", "LPT"),
            ("\"*?", "lora"),
        ] {
            assert_eq!(sanitize_lora_component(part), want, "{part:?}");
        }
        let names = [
            rel_name_for_download("aux/CON", "dir?/NUL.safetensors"),
            rel_name_for_import("prn.safetensors", &none),
            rel_name_for_import("\"*?.safetensors", &none),
            rel_name_for_import("what? *now*.safetensors", &none),
        ];
        assert_eq!(
            names,
            [
                "aux_/CON_/dir/NUL_",
                "local/prn_",
                "local/lora",
                "local/what now"
            ]
        );
        for n in &names {
            assert!(runtime::sd::args::valid_lora_name(n), "{n}");
            assert!(!n.contains([':', '<', '>', '|']), "the store's rule: {n}");
        }
    }
}
