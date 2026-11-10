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

#[derive(Default)]
pub struct DownloadManager {
    running: StdMutex<HashMap<String, (CancellationToken, bool)>>,
    live: StdMutex<HashMap<String, Live>>,
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

impl DownloadManager {
    fn emit(&self, app: &AppHandle, dto: DownloadGroupDto) {
        let _ = app.emit_to(crate::MAIN_WINDOW, "download-progress", dto);
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
        let status = if rows.iter().all(|r| r.status == DownloadStatus::Completed) {
            DownloadStatus::Completed
        } else if let Some(r) = rows.iter().find(|r| r.status == DownloadStatus::Failed) {
            r.status
        } else if rows.iter().any(|r| {
            r.status == DownloadStatus::Downloading || r.status == DownloadStatus::Verifying
        }) {
            DownloadStatus::Downloading
        } else if rows.iter().any(|r| r.status == DownloadStatus::Paused) {
            DownloadStatus::Paused
        } else if rows.iter().all(|r| r.status == DownloadStatus::Cancelled) {
            DownloadStatus::Cancelled
        } else {
            DownloadStatus::Queued
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
    pub async fn start(
        self: &Arc<Self>,
        app: &AppHandle,
        meta: GroupMeta,
        files: Vec<(FileSpec, PathBuf)>,
    ) -> AppResult<String> {
        let state = app.state::<AppState>();
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
        Ok(group)
    }

    /// Resumes every group that was running or queued when the app last stopped.
    pub async fn resume_interrupted(self: &Arc<Self>, app: &AppHandle) {
        let state = app.state::<AppState>();
        let Ok(rows) = state.store.list_downloads(None).await else {
            return;
        };
        let mut groups: Vec<String> = Vec::new();
        for r in rows {
            if matches!(
                r.status,
                DownloadStatus::Downloading | DownloadStatus::Queued | DownloadStatus::Verifying
            ) && !groups.contains(&r.group_id)
            {
                groups.push(r.group_id);
            }
        }
        for g in groups {
            tracing::info!(group = %g, "resuming an interrupted download");
            self.spawn_group(app.clone(), g);
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
            let finished_file = std::fs::metadata(&row.dest_path)
                .map(|m| m.len() == row.size_bytes)
                .unwrap_or(false);
            if row.status == DownloadStatus::Completed
                || (row.status == DownloadStatus::Verifying && finished_file)
            {
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
        self.register_model(app, group, &meta).await?;
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
        )
        .await?;
        tracing::info!(repo = %meta.repo, label = %meta.label, "download finished");
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

    pub async fn resume(self: &Arc<Self>, app: &AppHandle, group: &str) -> AppResult<()> {
        let state = app.state::<AppState>();
        for row in state.store.list_downloads(Some(group.to_string())).await? {
            if row.status != DownloadStatus::Completed {
                state
                    .store
                    .update_download(
                        row.id,
                        DownloadPatch {
                            status: Some(DownloadStatus::Queued),
                            error: Some(None),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
        }
        self.spawn_group(app.clone(), group.to_string());
        Ok(())
    }

    /// Stops the group, deletes its partial files and forgets it.
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
        state.store.delete_download_group(group.to_string()).await?;
        let _ = state.store.delete_setting(&meta_key(group)).await;
        if let Some(d) = dto.as_mut() {
            d.status = "cancelled".into();
            self.emit(app, d.clone());
        }
        Ok(())
    }
}
