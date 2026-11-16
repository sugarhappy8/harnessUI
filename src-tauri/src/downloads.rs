//! Model downloads (ARCHITECTURE.md 11): one group per model (its shards and the
//! projector), persisted in the `downloads` table so an interrupted download resumes
//! after a restart. Pause, resume and cancel act on a whole group.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use models::download::{FileSpec, Progress, download_file_checked, resumable_bytes};
use serde::{Deserialize, Serialize};
use store::{DownloadPatch, DownloadStatus, ModelSource, NewDownload};
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

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
}

fn meta_key(group: &str) -> String {
    format!("downloadGroup:{group}")
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
/// (anything but done or cancelled: a paused or failed one can go on), so that two
/// downloads never write one file at once ([`plan_start`]).
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
}

/// Decides [`Start`] for a new download of `new` that writes `dests`, from every download
/// row, the saved metadata of a group (`meta_of`) and whether a group's task runs
/// (`running`). A download joins only one of the same kind that writes all its files: an
/// image file for an installed model joins only a download of the image file for that same
/// model (never another model's download, which would then be restarted, shown and added in
/// its place), and a model download only a model download.
fn plan_start(
    new: &GroupMeta,
    dests: &[String],
    rows: &[store::Download],
    meta_of: impl Fn(&str) -> Option<GroupMeta>,
    running: impl Fn(&str) -> bool,
) -> Start {
    let busy: Vec<&store::Download> = rows
        .iter()
        .filter(|r| {
            !matches!(
                r.status,
                DownloadStatus::Completed | DownloadStatus::Cancelled
            )
        })
        .filter(|r| dests.contains(&r.dest_path))
        .collect();
    let Some(first) = busy.first() else {
        return Start::New;
    };
    let group = first.group_id.as_str();
    let one_group = busy.iter().all(|r| r.group_id == group);
    let all_there = dests.iter().all(|d| busy.iter().any(|r| &r.dest_path == d));
    let same_kind = meta_of(group).is_some_and(|m| m.image_file_for == new.image_file_for);
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
    Ok(written)
}

/// What a group says when its files are all there but its last step failed: adding the
/// model to Installed, or recording the image file on its model. Resume tries that step
/// again without downloading anything.
fn last_step_error(image_file: bool, e: &AppError) -> AppError {
    let what = if image_file {
        "The image file was downloaded, but it could not be added to its model."
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
/// which two models can share). A file the download found in place (another app's copy, a
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
        .filter(|p| dunce::canonicalize(p).is_ok_and(|real| real.starts_with(&root)))
        .filter(|p| !in_use(p))
        .collect()
}

/// Deletes a cancelled group's files and forgets it: its partial files, and the finished
/// files [`files_to_delete_on_cancel`] allows (the models folder, or for an image file the
/// model's own folder, is where they may be).
async fn clean_up(store: &store::Store, group: &str, models_dir: &Path) -> AppResult<()> {
    let rows = store.list_downloads(Some(group.to_string())).await?;
    let meta: Option<GroupMeta> = store.get_setting(&meta_key(group)).await.ok().flatten();
    let written = written_by(store, group).await;
    for row in &rows {
        if row.status != DownloadStatus::Completed {
            models::download::discard_partial(Path::new(&row.dest_path)).await;
        }
    }
    let root = match meta.as_ref().and_then(|m| m.image_file_for.as_deref()) {
        Some(model) => Path::new(model).parent().map(Path::to_path_buf),
        None => Some(models_dir.to_path_buf()),
    };
    match store.list_models().await {
        Ok(installed) => {
            let in_use = |p: &Path| crate::hub::used_by_installed(&installed, p);
            for path in files_to_delete_on_cancel(&rows, &written, root.as_deref(), in_use) {
                if let Err(e) = tokio::fs::remove_file(&path).await {
                    tracing::warn!("a finished file of a cancelled download stayed: {e}");
                }
            }
        }
        Err(e) => tracing::warn!("the finished files of a cancelled download stayed: {e}"),
    }
    store.delete_download_group(group.to_string()).await?;
    let _ = store.delete_setting(&meta_key(group)).await;
    let _ = store.delete_setting(&written_key(group)).await;
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
/// (P6b review m2).
fn at_start(statuses: &[DownloadStatus]) -> AtStart {
    if statuses.contains(&DownloadStatus::Cancelled) {
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
            file_count: rows.len(),
            bytes_done,
            bytes_total,
            status: status_name(status).to_string(),
            error,
            bytes_per_second: live.and_then(|l| l.speed),
            unverified: rows.iter().any(|r| r.sha256.is_empty()),
            checking: live.is_some_and(|l| l.checking),
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
        for r in rows.iter().filter(|r| dests.contains(&r.dest_path)) {
            if !metas.contains_key(&r.group_id) {
                let saved = store
                    .get_setting(&meta_key(&r.group_id))
                    .await
                    .ok()
                    .flatten();
                metas.insert(r.group_id.clone(), saved);
            }
        }
        let plan = plan_start(
            meta,
            &dests,
            &rows,
            |g| metas.get(g).cloned().flatten(),
            |g| self.is_running(g),
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
            match at_start(&statuses) {
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
                },
            );
        }
        let me = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let result = me.run_group(&app, &group, &token).await;
            let paused = me
                .running
                .lock()
                .ok()
                .and_then(|mut r| r.remove(&group))
                .is_some_and(|r| r.paused);
            if let Ok(mut l) = me.live.lock() {
                l.remove(&group);
            }
            if let Err(e) = result {
                tracing::warn!(group = %group, "download group stopped: {e}");
            }
            if paused {
                tracing::info!(group = %group, "download paused");
            }
            // A cancelled group is shown by Cancel, once its files are gone.
            if !token.is_cancelled() || paused {
                let state = app.state::<AppState>();
                if let Ok(Some(dto)) = me.group_dto(&state.store, &group).await {
                    me.emit(&app, dto);
                }
            }
            ended.cancel();
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
                commit: meta.commit.clone(),
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
            Some(model) => self.attach_image_file(app, group, &meta, model).await,
            None => self.register_model(app, group, &meta).await,
        };
        if let Err(e) = added {
            // 0.0.7 (P6): the group says why it stopped instead of staying "downloading"
            // (and starting again at every start of the app). Its files stay: Resume tries
            // this step again without downloading them.
            let err = last_step_error(meta.image_file_for.is_some(), &e);
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

    /// Adds the finished model to the installed list.
    async fn register_model(
        &self,
        app: &AppHandle,
        group: &str,
        meta: &GroupMeta,
    ) -> AppResult<()> {
        let state = app.state::<AppState>();
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
        crate::hub::register_installed(
            &state,
            Path::new(&first),
            Some(meta.repo.clone()),
            size,
            sha,
            projector.clone(),
            ModelSource::Download,
            saved_facts(meta),
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
        let Some(image) = meta
            .projector
            .as_deref()
            .and_then(|file| rows.iter().find(|r| r.file == file))
            .map(|r| PathBuf::from(&r.dest_path))
        else {
            return Err(AppError::internal("finished download has no image file"));
        };
        crate::hub::image_file::attach(&state.store, model, &image).await?;
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
    /// never downloaded again ([`status_on_resume`]).
    pub async fn resume(self: &Arc<Self>, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        self.ready_to_resume(&state.store, group).await?;
        self.spawn_group(app.clone(), group.to_string());
        Ok(())
    }

    /// Readies a stopped group to go on ([`requeue`]), holding `starting` (P6b review m3), so
    /// a start that checks and records a download of the same files is not in between.
    async fn ready_to_resume(&self, store: &store::Store, group: &str) -> AppResult<()> {
        let _starting = self.starting.lock().await;
        requeue(store, group).await
    }

    /// Stops the group, deletes its partial files and forgets it. 0.0.7 (P6): the files it
    /// wrote itself go too, unless an installed model uses them
    /// ([`files_to_delete_on_cancel`]). P6b review: it waits for the group's task to end
    /// first (m1), so nothing is written, renamed or added after it looked, and it holds
    /// `starting` (m3), so no start or resume comes in between. A group that finished before
    /// it could stop is left as it is.
    pub async fn cancel(&self, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        let _starting = self.starting.lock().await;
        let task = self.running.lock().ok().and_then(|r| {
            r.get(group)
                .map(|r| (r.token.clone(), r.ended.clone()))
        });
        if let Some((token, ended)) = task {
            token.cancel();
            if tokio::time::timeout(CANCEL_WAIT, ended.cancelled())
                .await
                .is_err()
            {
                return Err(AppError::new(
                    "download_stopping",
                    "The download is still stopping.",
                    "Wait a moment, then cancel it again.",
                    String::new(),
                ));
            }
        }
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        if !rows.is_empty() && rows.iter().all(|r| r.status == DownloadStatus::Completed) {
            if let Ok(Some(dto)) = self.group_dto(&state.store, group).await {
                self.emit(app, dto);
            }
            return Ok(());
        }
        let mut dto = self.group_dto(&state.store, group).await?;
        clean_up(&state.store, group, &crate::hub::models_dir(app).await).await?;
        if let Some(d) = dto.as_mut() {
            d.status = "cancelled".into();
            self.emit(app, d.clone());
        }
        Ok(())
    }

    /// 0.0.7 (P6b review m1): cancels the downloads of an image file for the installed model
    /// at `model` (Delete of that model), so none finishes after the model is gone and leaves
    /// its file behind.
    pub async fn cancel_image_files_for(&self, app: &AppHandle, model: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        let mut groups: Vec<String> = Vec::new();
        for r in state.store.list_downloads(None).await? {
            if r.status != DownloadStatus::Completed && !groups.contains(&r.group_id) {
                groups.push(r.group_id);
            }
        }
        for g in groups {
            let meta: Option<GroupMeta> = state.store.get_setting(&meta_key(&g)).await.ok().flatten();
            if meta
                .and_then(|m| m.image_file_for)
                .is_some_and(|m| crate::model_options::same_file(&m, model))
            {
                self.cancel(app, &g).await?;
            }
        }
        Ok(())
    }
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
        assert_eq!(status_on_resume(&at(S::Verifying), true), Some(S::Verifying));
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
        let err = last_step_error(false, &AppError::internal("locked"));
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

        let checked = last_step_error(false, &AppError::internal("locked"));
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
        assert_eq!(at_start(&[S::Verifying, S::Cancelled]), AtStart::FinishCancel);
        assert_eq!(at_start(&[S::Cancelled, S::Queued]), AtStart::FinishCancel);
        assert_eq!(at_start(&[S::Verifying, S::Downloading]), AtStart::Resume);
        assert_eq!(at_start(&[S::Verifying, S::Paused]), AtStart::Leave);
        assert_eq!(at_start(&[S::Completed]), AtStart::Leave);
        assert_eq!(group_status(&[S::Verifying, S::Cancelled], false), S::Paused);
        // While Cancel stops its task, it shows as cancelled.
        assert_eq!(group_status(&[S::Verifying, S::Cancelled], true), S::Cancelled);
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

        // Done and cancelled downloads write nothing.
        let over = vec![
            row("a", &q4, 1, S::Completed),
            row("q8", &q4, 1, S::Cancelled),
        ];
        assert_eq!(plan(&model, &[&q4], &over, &metas, &[]), Start::New);
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
        clean_up(&store, "g", &models).await.unwrap();
        assert!(!wrote.exists(), "the file it wrote goes");
        assert!(found.exists(), "the same file found in place stays");
        assert!(older.exists(), "the older file a failed fetch never replaced stays");
        assert!(!part.exists(), "its partial file goes");
        assert!(store.list_downloads(Some("g".into())).await.unwrap().is_empty());
        assert!(written_by(&store, "g").await.is_empty());
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
        clean_up(&store, "g", &models).await.unwrap();
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
        mark_failed(&store, "g", &last_step_error(false, &cause))
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

        let image_step = last_step_error(true, &AppError::internal("locked"));
        assert_eq!(
            image_step.message,
            "The image file was downloaded, but it could not be added to its model. Something went wrong inside the app."
        );
    }
}
