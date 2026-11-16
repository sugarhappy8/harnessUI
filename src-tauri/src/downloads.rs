//! Model downloads (ARCHITECTURE.md 11): one group per model (its shards and the
//! projector), persisted in the `downloads` table so an interrupted download resumes
//! after a restart. Pause, resume and cancel act on a whole group.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};

use models::download::{FileSpec, download_file, resumable_bytes};
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
}

#[derive(Default)]
struct Live {
    /// Bytes on disk per file path, while running.
    done: HashMap<String, u64>,
    speed: Option<f64>,
    last_emit: Option<Instant>,
    last_persist: Option<Instant>,
    window: Option<(Instant, u64)>,
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

#[derive(Default)]
pub struct DownloadManager {
    running: StdMutex<HashMap<String, (CancellationToken, bool)>>,
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

/// Whether a row's file needs no more downloading: the group is done with it, or the file
/// is whole and only the group's last step (adding the model) is left, or failed.
fn file_done(status: DownloadStatus, whole: bool) -> bool {
    match status {
        DownloadStatus::Completed => true,
        DownloadStatus::Verifying | DownloadStatus::Failed => whole,
        _ => false,
    }
}

/// What Resume makes of a row: `None` leaves it (the group is done with it); a whole file
/// waits for the last step again (`Verifying`), so it is never downloaded again; any other
/// file is downloaded (`Queued`), going on from its partial file.
fn status_on_resume(status: DownloadStatus, whole: bool) -> Option<DownloadStatus> {
    if status == DownloadStatus::Completed {
        None
    } else if file_done(status, whole) {
        Some(DownloadStatus::Verifying)
    } else {
        Some(DownloadStatus::Queued)
    }
}

/// The status the UI shows for a group, from its rows and whether its task runs. A finished
/// file (`Verifying`) is progress only while the task runs: a group paused after its first
/// file is paused, and one that stopped for any other reason can be resumed.
fn group_status(rows: &[DownloadStatus], running: bool) -> DownloadStatus {
    use DownloadStatus as S;
    if rows.iter().all(|s| *s == S::Completed) {
        S::Completed
    } else if rows.contains(&S::Failed) {
        S::Failed
    } else if rows.contains(&S::Cancelled) {
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

/// What a new download makes of the downloads not finished yet that write the same files.
#[derive(Debug, PartialEq, Eq)]
enum Overlap {
    /// None of its files is being downloaded: it starts.
    None,
    /// A download of all its files is there already (a second click on Download, the same
    /// model picked again): that one is it.
    Same(String),
    /// Another download fetches some of its files (two models of one repository that share
    /// an image file): it waits until that one is done.
    Part,
}

/// Compares the files of a new download (`dests`) with the downloads not finished yet
/// (anything but done or cancelled: a paused or failed one can go on), which would write
/// the same partial files at the same time.
fn overlap(rows: &[store::Download], dests: &[String]) -> Overlap {
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
        return Overlap::None;
    };
    let one_group = busy.iter().all(|r| r.group_id == first.group_id);
    let all_there = dests
        .iter()
        .all(|d| busy.iter().any(|r| &r.dest_path == d));
    if one_group && all_there {
        Overlap::Same(first.group_id.clone())
    } else {
        Overlap::Part
    }
}

/// A new download some of whose files another download fetches ([`Overlap::Part`]).
fn already_downloading() -> AppError {
    AppError::new(
        "already_downloading",
        "One of this model's files is already being downloaded.",
        "Wait until that download is done (see Downloads), then try again.",
        String::new(),
    )
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
        "download_not_added",
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
        if let Some(status) = status_on_resume(row.status, whole) {
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

/// The finished files Cancel deletes besides the partial ones: those the group finished
/// (whole at their final name, never added to Installed), except a file an installed model
/// uses (`in_use`: its own file, or its image file, which two models can share).
fn finished_files_to_delete(
    rows: &[store::Download],
    in_use: impl Fn(&Path) -> bool,
) -> Vec<PathBuf> {
    rows.iter()
        .filter(|r| {
            matches!(
                r.status,
                DownloadStatus::Verifying | DownloadStatus::Failed
            )
        })
        .filter(|r| file_whole(Path::new(&r.dest_path), r.size_bytes))
        .map(|r| PathBuf::from(&r.dest_path))
        .filter(|p| !in_use(p))
        .collect()
}

impl DownloadManager {
    fn emit(&self, app: &AppHandle, dto: DownloadGroupDto) {
        let _ = app.emit_to(crate::MAIN_WINDOW, "download-progress", dto);
    }

    /// Whether the group's task runs now.
    fn is_running(&self, group: &str) -> bool {
        self.running
            .lock()
            .is_ok_and(|r| r.contains_key(group))
    }

    /// Builds the DTO for one group from its rows and live progress.
    async fn group_dto(
        &self,
        state: &AppState,
        group: &str,
    ) -> AppResult<Option<DownloadGroupDto>> {
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let meta: Option<GroupMeta> = state.store.get_setting(&meta_key(group)).await?;
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
            if let Some(dto) = self.group_dto(state, &g).await? {
                out.push(dto);
            }
        }
        Ok(out)
    }

    /// Registers a new group and starts it.
    ///
    /// 0.0.7 (P6): two downloads never write one file at once. When a download not finished
    /// yet fetches all of these files already, that one is returned (and goes on if it had
    /// stopped); when it fetches only some of them, this one is refused until it is done.
    pub async fn start(
        self: &Arc<Self>,
        app: &AppHandle,
        meta: GroupMeta,
        files: Vec<(FileSpec, PathBuf)>,
    ) -> AppResult<String> {
        let state = app.state::<AppState>();
        let dests: Vec<String> = files
            .iter()
            .map(|(_, dest)| dest.display().to_string())
            .collect();
        let starting = self.starting.lock().await;
        match overlap(&state.store.list_downloads(None).await?, &dests) {
            Overlap::None => {}
            Overlap::Same(group) => {
                drop(starting);
                if !self.is_running(&group) {
                    self.resume(app, &group).await?;
                }
                return Ok(group);
            }
            Overlap::Part => return Err(already_downloading()),
        }
        let group = uuid::Uuid::new_v4().to_string();
        state.store.set_setting(&meta_key(&group), &meta).await?;
        for (spec, dest) in &files {
            state
                .store
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
        self.spawn_group(app.clone(), group.clone());
        drop(starting);
        Ok(group)
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
            if resumes_at_start(&statuses) {
                tracing::info!(group = %g, "resuming an interrupted download");
                self.spawn_group(app.clone(), g);
            }
        }
    }

    fn spawn_group(self: &Arc<Self>, app: AppHandle, group: String) {
        let token = CancellationToken::new();
        {
            let Ok(mut running) = self.running.lock() else {
                return;
            };
            if running.contains_key(&group) {
                return;
            }
            running.insert(group.clone(), (token.clone(), false));
        }
        let me = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            let result = me.run_group(&app, &group, &token).await;
            let paused = me
                .running
                .lock()
                .ok()
                .and_then(|mut r| r.remove(&group))
                .map(|(_, p)| p)
                .unwrap_or(false);
            if let Ok(mut l) = me.live.lock() {
                l.remove(&group);
            }
            if let Err(e) = result {
                tracing::warn!(group = %group, "download group stopped: {e}");
            }
            if paused {
                tracing::info!(group = %group, "download paused");
            }
            let state = app.state::<AppState>();
            if let Ok(Some(dto)) = me.group_dto(&state, &group).await {
                me.emit(&app, dto);
            }
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
            if file_done(
                row.status,
                file_whole(Path::new(&row.dest_path), row.size_bytes),
            ) {
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
                move |n: u64| self.on_progress(&app, &group, &file, row_id, n)
            };
            match download_file(&client, &spec, &dest, token, &progress).await {
                Ok(_) => {
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
                        .and_then(|r| r.get(group).map(|(_, p)| *p))
                        .unwrap_or(false);
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
        Ok(())
    }

    fn on_progress(
        &self,
        app: &AppHandle,
        group: &str,
        file: &str,
        row_id: store::DownloadId,
        n: u64,
    ) {
        let now = Instant::now();
        let (emit, persist) = {
            let Ok(mut all) = self.live.lock() else {
                return;
            };
            let live = all.entry(group.to_string()).or_default();
            live.done.insert(file.to_string(), n);
            let total: u64 = live.done.values().sum();
            match live.window {
                Some((t0, b0)) if now.duration_since(t0) >= Duration::from_secs(2) => {
                    let secs = now.duration_since(t0).as_secs_f64();
                    live.speed = Some(total.saturating_sub(b0) as f64 / secs);
                    live.window = Some((now, total));
                }
                None => live.window = Some((now, total)),
                _ => {}
            }
            let emit = live
                .last_emit
                .is_none_or(|t| now.duration_since(t) >= Duration::from_millis(300));
            if emit {
                live.last_emit = Some(now);
            }
            let persist = live
                .last_persist
                .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(3));
            if persist {
                live.last_persist = Some(now);
            }
            (emit, persist)
        };
        if !(emit || persist) {
            return;
        }
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
                && let Ok(Some(dto)) = dl.group_dto(&state, &group).await
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
            projector,
            ModelSource::Download,
            saved_facts(meta),
        )
        .await?;
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
        tracing::info!(repo = %meta.repo, "image file downloaded");
        Ok(())
    }

    pub fn pause(&self, group: &str) {
        if let Ok(mut r) = self.running.lock()
            && let Some((token, paused)) = r.get_mut(group)
        {
            *paused = true;
            token.cancel();
        }
    }

    /// Goes on with a stopped group. 0.0.7 (P6): a file that finished stays as it is and is
    /// never downloaded again ([`status_on_resume`]).
    pub async fn resume(self: &Arc<Self>, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        requeue(&state.store, group).await?;
        self.spawn_group(app.clone(), group.to_string());
        Ok(())
    }

    /// Stops the group, deletes its partial files and forgets it. 0.0.7 (P6): the files it
    /// finished go too, unless an installed model uses them ([`finished_files_to_delete`]).
    pub async fn cancel(&self, app: &AppHandle, group: &str) -> AppResult<()> {
        let token = self
            .running
            .lock()
            .ok()
            .and_then(|r| r.get(group).map(|(t, _)| t.clone()));
        if let Some(t) = token {
            t.cancel();
            // Give the task a moment to close its file handles.
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        let state = app.state::<AppState>();
        let rows = state.store.list_downloads(Some(group.to_string())).await?;
        let mut dto = self.group_dto(&state, group).await?;
        for row in &rows {
            if row.status != DownloadStatus::Completed {
                models::download::discard_partial(Path::new(&row.dest_path)).await;
            }
        }
        match state.store.list_models().await {
            Ok(installed) => {
                let in_use = |p: &Path| crate::hub::used_by_installed(&installed, p);
                for path in finished_files_to_delete(&rows, in_use) {
                    if let Err(e) = tokio::fs::remove_file(&path).await {
                        tracing::warn!("a finished file of a cancelled download stayed: {e}");
                    }
                }
            }
            Err(e) => tracing::warn!("the finished files of a cancelled download stayed: {e}"),
        }
        state.store.delete_download_group(group.to_string()).await?;
        let _ = state.store.delete_setting(&meta_key(group)).await;
        if let Some(d) = dto.as_mut() {
            d.status = "cancelled".into();
            self.emit(app, d.clone());
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
        assert!(file_done(rows[0].status, file_whole(&model, 100)));
        assert!(!file_done(rows[1].status, file_whole(&image, 50)));

        // A finished file that is gone since is downloaded again.
        std::fs::remove_file(&model).unwrap();
        requeue(&store, "g").await.unwrap();
        assert_eq!(statuses(&store, "g").await, vec![S::Queued, S::Queued]);

        // Every row: what Resume makes of it.
        assert_eq!(status_on_resume(S::Completed, true), None);
        assert_eq!(status_on_resume(S::Verifying, true), Some(S::Verifying));
        assert_eq!(status_on_resume(S::Failed, true), Some(S::Verifying));
        for s in [S::Queued, S::Downloading, S::Paused, S::Verifying, S::Failed] {
            assert_eq!(status_on_resume(s, false), Some(S::Queued), "{s:?}");
        }
        // A file whole at its name that this group did not finish is checked by
        // `download_file`, which does not fetch it again when it matches its checksum.
        assert_eq!(status_on_resume(S::Paused, true), Some(S::Queued));
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
        assert_eq!(group_status(&[S::Verifying, S::Downloading], true), S::Downloading);
        assert_eq!(group_status(&[S::Verifying, S::Queued], true), S::Downloading);
        assert_eq!(group_status(&[S::Queued, S::Queued], true), S::Queued);
        // Stopped any other way (nothing runs it): it can be resumed.
        assert_eq!(group_status(&[S::Verifying, S::Verifying], false), S::Paused);
        assert_eq!(group_status(&[S::Verifying, S::Queued], false), S::Paused);
        // Done, failed and cancelled.
        assert_eq!(group_status(&[S::Completed, S::Completed], false), S::Completed);
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

    /// The bug: two downloads of one file (a double click on Download, Download again after
    /// Back, two models of one repository sharing one image file, "Download the image file")
    /// wrote the same partial file at once and spoiled each other.
    #[test]
    fn two_downloads_never_write_one_file() {
        let dir = Path::new("/models/example-quants/Sample-7B-GGUF");
        let q4 = dir.join("Sample-7B-Q4_K_M.gguf");
        let q8 = dir.join("Sample-7B-Q8_0.gguf");
        let image = dir.join("mmproj-Sample-F16.gguf");
        let text = |p: &PathBuf| p.display().to_string();
        let rows = vec![
            row("a", &q4, 1, S::Downloading),
            row("a", &image, 1, S::Queued),
        ];
        // The same model again: that download is it.
        assert_eq!(
            overlap(&rows, &[text(&q4), text(&image)]),
            Overlap::Same("a".into())
        );
        assert_eq!(overlap(&rows, &[text(&q4)]), Overlap::Same("a".into()));
        // Its image file alone (Models → Installed) follows that download too.
        assert_eq!(overlap(&rows, &[text(&image)]), Overlap::Same("a".into()));
        // Another model of the repository that shares the image file waits for it.
        assert_eq!(overlap(&rows, &[text(&q8), text(&image)]), Overlap::Part);
        assert_eq!(overlap(&rows, &[text(&q8)]), Overlap::None);

        // A paused or failed download can go on, so it counts; a done or cancelled one not.
        let stopped = vec![row("a", &q4, 1, S::Paused), row("b", &q8, 1, S::Failed)];
        assert_eq!(overlap(&stopped, &[text(&q8)]), Overlap::Same("b".into()));
        assert_eq!(overlap(&stopped, &[text(&q4), text(&q8)]), Overlap::Part);
        let over = vec![row("a", &q4, 1, S::Completed), row("b", &q4, 1, S::Cancelled)];
        assert_eq!(overlap(&over, &[text(&q4)]), Overlap::None);
        assert_eq!(overlap(&[], &[text(&q4)]), Overlap::None);

        let err = already_downloading();
        assert_eq!(err.code, "already_downloading");
        assert_eq!(
            err.message,
            "One of this model's files is already being downloaded."
        );
        assert!(err.next_step.contains("Downloads"));
    }

    /// The bug: Cancel deleted only the partial files, so a file the group had finished
    /// (often gigabytes) stayed on disk, not on the Installed list but in the folder's size.
    /// A file an installed model uses stays: its own file, or its image file, which two
    /// models of one repository can share.
    #[test]
    fn cancel_deletes_the_finished_files_no_installed_model_uses() {
        let dir = Scratch::new();
        let repo = dir.path().join("example-quants/Sample-7B-GGUF");
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
        let used = |models: Vec<store::ModelRecord>| {
            move |p: &Path| crate::hub::used_by_installed(&models, p)
        };
        // Nothing installed uses them: both finished files go.
        assert_eq!(
            finished_files_to_delete(&rows, used(Vec::new())),
            vec![q8.clone(), image.clone()]
        );
        // Another model of the repository is installed next to the image file (a load finds
        // it there), or has it recorded: the image file stays.
        let q4 = repo.join("Sample-7B-Q4_K_M.gguf");
        std::fs::write(&q4, vec![3u8; 10]).unwrap();
        assert_eq!(
            finished_files_to_delete(&rows, used(vec![installed(&q4, None)])),
            vec![q8.clone()]
        );
        let elsewhere = dir.path().join("Other-Q4_K_M.gguf");
        std::fs::write(&elsewhere, vec![3u8; 10]).unwrap();
        assert_eq!(
            finished_files_to_delete(&rows, used(vec![installed(&elsewhere, Some(&image))])),
            vec![q8.clone()]
        );
        // A model on the list downloaded again: its file stays.
        assert_eq!(
            finished_files_to_delete(&rows, used(vec![installed(&q8, None)])),
            Vec::<PathBuf>::new()
        );
        // Every shard of a split model on the list stays.
        let shard2 = repo.join("Sample-7B-Q2_K-00002-of-00002.gguf");
        std::fs::write(&shard2, vec![4u8; 10]).unwrap();
        let split = installed(&repo.join("Sample-7B-Q2_K-00001-of-00002.gguf"), None);
        assert!(
            finished_files_to_delete(&[row("g", &shard2, 10, S::Verifying)], used(vec![split]))
                .is_empty()
        );
        // Not whole (another size): not a file the group finished.
        assert!(finished_files_to_delete(&[row("g", &q8, 101, S::Verifying)], |_| false).is_empty());
        // Whole after the last step failed: deleted too. A paused row's file found at its name
        // was not finished by this group, so it stays.
        assert_eq!(
            finished_files_to_delete(
                &[row("g", &q8, 100, S::Failed), row("g", &image, 50, S::Paused)],
                |_| false
            ),
            vec![q8.clone()]
        );
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
        assert_eq!(statuses(&store, "g").await, vec![S::Verifying, S::Verifying]);
        let rows = store.list_downloads(Some("g".into())).await.unwrap();
        assert!(rows.iter().all(|r| r.error.is_none()));

        let image_step = last_step_error(true, &AppError::internal("locked"));
        assert_eq!(
            image_step.message,
            "The image file was downloaded, but it could not be added to its model. Something went wrong inside the app."
        );
    }
}
