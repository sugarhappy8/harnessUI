//! The video jobs of Studio's runs: following a started job (checked until it is done, fails
//! or is gone, or left still not ready), downloading its results, Stop watching, picking one
//! up again after a restart, and *Get it anyway*'s follow-up. The rules they keep are in the
//! manager's documentation.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use agent::media::{
    Company, FailureKind, Fetched, JobDone, JobState, MediaError, MediaKind, Saved,
};
use serde_json::{Value, json};

use super::{
    Ending, Inner, Job, LaneKey, PROGRESS_EVERY, Step, Task, Waited, media_kind, poll_delay,
    tool_of, usable_cost,
};
use crate::error::{AppError, AppResult};
use crate::studio::errors;
use crate::studio::gallery::io_error;

impl Inner {
    /// Picks up one cloud run the app left unfinished (see
    /// [`StudioManager::resume_interrupted`](super::StudioManager::resume_interrupted)).
    pub(super) async fn resume(self: &Arc<Self>, run: store::CreationRun) {
        use store::CreationStatus as S;
        let company = self.company_of(&run).await;
        let end = |status, error, charged| Ending {
            status,
            ..Ending::failed(error, charged)
        };
        match (run.status, run.remote_id.clone()) {
            (S::Queued, _) => {
                let ending = end(
                    S::Cancelled,
                    errors::closed_before_its_turn(company),
                    Some(false),
                );
                self.record_end(&run.id, company, run.audit_id, ending, None)
                    .await;
            }
            (S::Waiting | S::Running | S::Downloading, Some(remote_id)) => {
                match self.follow_job(&run, remote_id, run.started_at).await {
                    Ok(job) => {
                        if let Err(e) = self.watch(job).await {
                            tracing::warn!(run = %run.id, "a Studio video was not followed: {e:?}");
                        }
                    }
                    Err(error) => {
                        // A job that finished was charged; any other may still be.
                        let ending = Ending::failed(error, finished_job(&run).then_some(true));
                        self.record_end(&run.id, company, run.audit_id, ending, run.started_at)
                            .await;
                    }
                }
            }
            // Sent with no job to check: Studio cannot know whether it started.
            _ => {
                let ending = end(S::Interrupted, errors::interrupted(company), None);
                self.record_end(&run.id, company, run.audit_id, ending, run.started_at)
                    .await;
            }
        }
    }

    /// The company behind a recorded run: its endpoint's, else (the endpoint is gone) the one
    /// its host belongs to.
    async fn company_of(&self, run: &store::CreationRun) -> Company {
        if let Some(id) = run.provider_id
            && let Some(company) = self.deps.providers.company(id).await
        {
            return company;
        }
        if run.provider_host.ends_with("openai.com") {
            Company::OpenAi
        } else {
            Company::OpenRouter
        }
    }

    /// The job that follows recorded run `run`'s video job `remote_id`, with its endpoint and
    /// key (refused as `ProviderSource::media` refuses), and whether the job is known to have
    /// finished (read from `run` as it is before it is followed).
    pub(super) async fn follow_job(
        &self,
        run: &store::CreationRun,
        remote_id: String,
        started: Option<i64>,
    ) -> AppResult<Job> {
        let id = run.provider_id.ok_or_else(errors::no_endpoint)?;
        let (provider, endpoint) = self.deps.providers.media(id).await?;
        let kind = media_kind(run.kind);
        Ok(Job {
            run_id: run.id.clone(),
            created_at: run.created_at,
            kind,
            lane: LaneKey::Endpoint(id, kind),
            provider,
            endpoint,
            seed: run.params.pointer("/params/seed").and_then(Value::as_u64),
            audit_id: run.audit_id,
            step: Step::Follow {
                remote_id,
                started,
                finished: finished_job(run),
            },
        })
    }

    /// *Get it anyway* on an ended run whose audit row is complete: a follow-up row (the
    /// first row's tool and arguments, `parent_audit_id` set to the run's row) and the run
    /// back to `waiting` with that row, its error and its end cleared. The job to follow it.
    pub(super) async fn follow_up(
        &self,
        run: &store::CreationRun,
        remote_id: String,
    ) -> AppResult<Job> {
        let now = self.deps.clock.now_ms();
        let mut job = self.follow_job(run, remote_id, Some(now)).await?;
        let (tool, arguments) = match self.audit_row_of(run).await {
            Some(row) => (row.tool, row.arguments),
            None => (
                tool_of(job.kind).to_owned(),
                json!({
                    "endpoint": job.endpoint.name,
                    "host": job.endpoint.host,
                    "model": run.model,
                }),
            ),
        };
        let audit_id = self
            .deps
            .store
            .append_audit(store::AuditEntry {
                ts: Some(now),
                chat_id: None,
                server: "studio".to_owned(),
                tool,
                arguments,
                preview: format!(
                    "Got the stopped video with {} through {}",
                    run.model_label, job.endpoint.name
                ),
                risk: "normal".to_owned(),
                decision: store::AuditDecision::AllowedWithoutPrompt,
                decided_by: "user".to_owned(),
                note: None,
                parent_audit_id: run.audit_id,
                result: None,
            })
            .await?;
        job.audit_id = Some(audit_id);
        let back = store::CreationPatch {
            status: Some(store::CreationStatus::Waiting),
            error: Some(None),
            finished_at: Some(None),
            progress: Some(None),
            audit_id: Some(Some(audit_id)),
            ..Default::default()
        };
        if let Err(e) = self.deps.store.update_creation_run(&run.id, back).await {
            // Nothing was followed: the follow-up row says so rather than stay open.
            self.complete_audit(
                &run.id,
                Some(audit_id),
                store::CreationStatus::Failed,
                None,
                None,
            )
            .await;
            return Err(e.into());
        }
        Ok(job)
    }

    /// Run `run`'s audit row as it is now (its first row, or its last follow-up), when it can
    /// be read.
    async fn audit_row_of(&self, run: &store::CreationRun) -> Option<store::AuditRow> {
        let audit_id = run.audit_id?;
        match self.deps.store.get_audit(audit_id).await {
            Ok(row) => row,
            Err(e) => {
                tracing::warn!(run = %run.id, "a Studio run's audit row could not be read: {e}");
                None
            }
        }
    }

    /// Checks video job `remote_id` until it is done (then downloads it), fails or is gone,
    /// or until the watch limit (then it is left `waiting`, still not ready). A check that
    /// fails keeps it waiting. Its progress is written and sent at most every
    /// [`PROGRESS_EVERY`]. `started` is when its request was sent.
    pub(super) async fn follow(&self, t: &Task<'_>, remote_id: &str, started: Option<i64>) {
        let job = t.job;
        let plan = self.deps.poll;
        let watching_since = Instant::now();
        // Its `waiting` event was just sent.
        let mut last_event = Instant::now();
        let mut progress: Option<f64> = None;
        loop {
            let state = match self
                .until_stopped(t.token, job.provider.check(remote_id))
                .await
            {
                Waited::Got(state) => state,
                Waited::Stopped => return self.stop_watching(job, started).await,
                Waited::Shutdown => return,
            };
            match state {
                Ok(JobState::Done(done)) => {
                    return self.download(t, remote_id, &done, started).await;
                }
                Ok(JobState::Failed(failure)) => {
                    let ending = self.ending_of(job, MediaError::Answered(failure));
                    return self.end_run(job, ending, started).await;
                }
                Ok(JobState::Gone) => {
                    let lost = errors::job_lost(job.endpoint.company);
                    let ending = Ending::failed(lost, job.job_charge());
                    return self.end_run(job, ending, started).await;
                }
                Ok(JobState::Running {
                    progress: Some(now),
                }) => {
                    let now = f64::from(now);
                    if progress != Some(now) && last_event.elapsed() >= PROGRESS_EVERY {
                        progress = Some(now);
                        last_event = Instant::now();
                        self.write_progress(job, now).await;
                    }
                }
                Ok(JobState::Pending | JobState::Running { progress: None }) => {}
                // It keeps waiting.
                Err(e) => tracing::debug!(run = %job.run_id, "a Studio video check failed: {e}"),
            }
            let watched = watching_since.elapsed();
            if watched >= plan.watch_limit {
                return self.leave_stale(job.following(remote_id, started)).await;
            }
            match self
                .until_stopped(t.token, tokio::time::sleep(poll_delay(&plan, watched)))
                .await
            {
                Waited::Got(()) => {}
                Waited::Stopped => return self.stop_watching(job, started).await,
                Waited::Shutdown => return,
            }
        }
    }

    /// Stop watching a video job: it started, so it may still finish and be charged (it was,
    /// when it is known to have finished); its remote id stays for *Get it anyway*.
    async fn stop_watching(&self, job: &Job, started: Option<i64>) {
        self.end_run(job, Ending::stopped(job, job.job_charge()), started)
            .await;
    }

    /// The watch limit passed: the run stays `waiting`, its audit row open, no longer
    /// checked, until *Check again* or Stop watching. `job` follows its started job.
    async fn leave_stale(&self, job: Job) {
        let run_id = job.run_id.clone();
        self.stale().insert(run_id.clone(), job);
        self.emit(&run_id).await;
    }

    /// Records a job's progress and sends it.
    async fn write_progress(&self, job: &Job, progress: f64) {
        let patch = store::CreationPatch {
            progress: Some(Some(progress)),
            ..Default::default()
        };
        match self
            .deps
            .store
            .update_creation_run(&job.run_id, patch)
            .await
        {
            Ok(()) => self.emit(&job.run_id).await,
            Err(e) => {
                tracing::debug!(run = %job.run_id, "a Studio run's progress was not recorded: {e}")
            }
        }
    }

    /// Downloads a done job's results into the gallery, its cost recorded first. It was
    /// charged, so a failure from here keeps the remote id and the cost and is recorded as
    /// charged.
    async fn download(&self, t: &Task<'_>, remote_id: &str, done: &JobDone, started: Option<i64>) {
        let job = t.job;
        let cost_usd = usable_cost(done.cost.map(|c| c.usd));
        let downloading = store::CreationPatch {
            status: Some(store::CreationStatus::Downloading),
            cost_usd: cost_usd.map(Some),
            progress: Some(None),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, downloading)
            .await
        {
            tracing::warn!(run = %job.run_id, "a done Studio video was not recorded: {e}");
        }
        // Results kept before (ahead of a restart, or of a download that broke) stay as they
        // are.
        let kept: Vec<u32> = match self.deps.store.get_creation_run(&job.run_id).await {
            Ok(Some(run)) => {
                self.deps.sink.progress(&self.dto_of(&run));
                run.outputs.iter().map(|out| out.idx).collect()
            }
            Ok(None) => Vec::new(),
            Err(e) => {
                tracing::warn!(run = %job.run_id, "a Studio run could not be read: {e}");
                Vec::new()
            }
        };
        let charged = |error: AppError| Ending {
            cost_usd,
            ..Ending::failed(error, Some(true))
        };
        if let Err(e) = tokio::fs::create_dir_all(&t.scratch).await {
            let error = errors::from_gallery(&io_error(e));
            return self.end_run(job, charged(error), started).await;
        }
        let cap = self.cap_of(job.kind);
        for idx in 0..done.outputs {
            let number = u32::try_from(idx).unwrap_or(u32::MAX);
            if kept.contains(&number) {
                continue;
            }
            let output_id = uuid::Uuid::new_v4().to_string();
            let partial = t.scratch.join(&output_id);
            let fetched = self.fetch_result(job, remote_id, done, idx, &partial);
            let saved = match self.until_stopped(t.token, fetched).await {
                Waited::Got(Ok(saved)) => saved,
                Waited::Got(Err(error)) => return self.end_run(job, charged(error), started).await,
                Waited::Stopped => {
                    let ending = Ending {
                        cost_usd,
                        ..Ending::stopped(job, Some(true))
                    };
                    return self.end_run(job, ending, started).await;
                }
                // The run stays `downloading`: the next start checks and downloads again.
                Waited::Shutdown => return,
            };
            if let Err(error) = self
                .keep_one(job, &t.gallery, output_id, number, &saved, cap)
                .await
            {
                return self.end_run(job, charged(error), started).await;
            }
        }
        self.finish(job, &t.gallery, cost_usd, started).await;
    }

    /// Result `idx` of done job `remote_id`, downloaded to `partial`: from the company with
    /// the key, or from its file host through the guarded fetcher (without the key) when the
    /// company points there. A failure in Studio's words.
    async fn fetch_result(
        &self,
        job: &Job,
        remote_id: &str,
        done: &JobDone,
        idx: usize,
        partial: &Path,
    ) -> AppResult<Saved> {
        let company = job.endpoint.company;
        let cap = self.cap_of(job.kind);
        match job.provider.download(remote_id, done, idx, partial).await {
            Ok(Fetched::Saved(saved)) => Ok(saved),
            Ok(Fetched::Elsewhere(url)) => {
                let mime = self
                    .deps
                    .fetch
                    .fetch_to_file(&url, cap, partial)
                    .await
                    .map_err(|e| errors::from_fetch(company, &e))?;
                let size_bytes = tokio::fs::metadata(partial).await.map_or(0, |m| m.len());
                Ok(Saved {
                    path: partial.to_owned(),
                    mime,
                    size_bytes,
                    width: None,
                    height: None,
                    duration_ms: None,
                })
            }
            Err(MediaError::Local(e)) => Err(errors::from_local(&e, cap)),
            Err(MediaError::Answered(f)) if f.kind == FailureKind::Expired => {
                Err(errors::result_expired(company))
            }
            Err(e) => Err(AppError {
                details: e.to_string(),
                ..errors::download_failed(company)
            }),
        }
    }

    /// The cap on one result of `kind`.
    pub(super) fn cap_of(&self, kind: MediaKind) -> u64 {
        match kind {
            MediaKind::Image => self.deps.limits.picture_cap,
            MediaKind::Video => self.deps.limits.video_cap,
        }
    }
}

/// Whether recorded run `run`'s video job is known to have reported done, so it was charged:
/// its result was downloading, its cost is recorded (only a job that reported done has one),
/// or how it ended said it was charged (a failure after done, or a Stop while downloading).
fn finished_job(run: &store::CreationRun) -> bool {
    run.status == store::CreationStatus::Downloading
        || run.cost_usd.is_some()
        || run
            .error
            .as_ref()
            .and_then(|error| error.get("charged"))
            .and_then(Value::as_bool)
            == Some(true)
}
