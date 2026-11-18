//! Studio's runs (0.1.0): starting them under the spending rules, their queues, cancel, the
//! pictures they make and their audit rows. Modelled on `DownloadManager`.
//!
//! - **Starting** ([`StudioManager::start`]), one at a time (the `starting` lock), so two
//!   presses at once cannot both pass the daily limit: the same press sent twice (its client
//!   token, the last [`RECENT_TOKENS`] kept, recorded only once its run is) gives the run it
//!   made; the model comes from the endpoint's saved lists and its kind must be the
//!   request's; Studio picks a seed when the model takes one and none is set; the settings are
//!   checked (`spec::validate`) and the extra settings placed by company
//!   (`openrouter::place_extra`; a model with no saved `/endpoints` answer, such as any OpenAI
//!   model, keeps them at the top level); the estimate is worked out (an OpenRouter picture
//!   from its saved `/endpoints` answer and the routing, so a pinned company's price is used); the
//!   daily limit is checked against today's runs, those still going included; and the
//!   confirmation rules. Only then is the run recorded (`queued`), with its audit row (the
//!   prompt's length, never the prompt), and queued. Nothing is sent before that.
//! - **Queues**: per endpoint, [`PICTURES_AT_ONCE`] picture runs and [`VIDEOS_AT_ONCE`] video
//!   jobs at once; more wait in order. A run's task frees its place however it ends, a panic
//!   included (`TaskEnd`), and starts the next one waiting.
//! - **A run's task** writes `sending` and its start time before the request goes out, sends
//!   it once (`MediaProvider::start`), and keeps the pictures that come back in the gallery
//!   (type-checked, at most `MediaLimits::picture_cap` each), with a thumbnail when a picture
//!   worker is set and the run's sidecar. A started video job is recorded as `waiting` with
//!   its remote id (Task A14 follows it). Every failure is recorded with whether it was
//!   charged: not sent, never; a timeout or a cut connection, and a Cancel while sending, not
//!   for an OpenRouter picture (OpenRouter bills only a picture it delivered:
//!   <https://openrouter.ai/docs/guides/overview/multimodal/image-generation>, "Billing and
//!   Cancellation") and unknown otherwise; a Cancel while queued, never.
//! - **Cancel** cancels the run's token: its request's future is dropped, which closes the
//!   connection (an answer already there when Cancel comes is kept: it was delivered, so it
//!   was charged). Nothing is ever sent again on its own.
//! - **The audit row** is completed once when the run ends, with its status and its cost.
//! - **Runs on this computer** are refused until the local lane exists (Task L15).

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use agent::media::openrouter::place_extra;
use agent::media::{
    Company, FailureKind, Invalid, MediaError, MediaKind, MediaLimits, MediaModel, MediaProvider,
    MediaRequest, ParamKind, RefImage, RequestPreview, Routing, SEED_MAX, Saved, Stage, Started,
    choice_for, estimate as estimate_usd, parse_image_endpoints, validate,
};
use rand::Rng;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::catalog;
use super::deps::{Clock, Endpoint, ForeignFetch, ProviderSource, StudioSink};
use super::errors;
use super::gallery::{Gallery, io_error};
use super::prefs::{RunTarget, StudioPrefs};
use super::spend::{self, Billing, LimitRefusal, RunState, Spend, SpendRow};
use crate::attachments::PictureWorker;
use crate::dto::{
    EstimateDto, LimitDto, OutputDto, RefDto, RunDto, RunErrorDto, StartRequest, UsageDto,
};
use crate::error::{AppError, AppResult};

/// Picture runs one endpoint makes at once; more wait.
pub const PICTURES_AT_ONCE: usize = 2;
/// Video jobs one endpoint runs at once; more wait.
pub const VIDEOS_AT_ONCE: usize = 2;

/// How many recent client tokens are kept, so a press sent twice makes one run.
const RECENT_TOKENS: usize = 64;

/// How long Cancel waits for a run's task to end and record it.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// What the manager works with.
pub struct StudioDeps {
    /// Runs, outputs, the saved lists, settings and the audit log.
    pub store: store::Store,
    /// The endpoints' media companies, with their keys.
    pub providers: Arc<dyn ProviderSource>,
    /// Where each run's new state goes (the webview).
    pub sink: Arc<dyn StudioSink>,
    /// Downloads from a company's file host, without the key (Task A14).
    pub fetch: Arc<dyn ForeignFetch>,
    /// The time, and where a local calendar day starts.
    pub clock: Arc<dyn Clock>,
    /// The gallery folder when Settings → Studio names none.
    pub default_gallery: PathBuf,
    /// Makes thumbnails; `None` makes none.
    pub thumbs: Option<PictureWorker>,
    /// The caps on results.
    pub limits: MediaLimits,
}

/// Studio's runs. Cheap to clone: every clone is the same manager.
#[derive(Clone)]
pub struct StudioManager {
    inner: Arc<Inner>,
}

struct Inner {
    deps: StudioDeps,
    /// Settings → Studio as they are now; each new run reads them.
    prefs: RwLock<StudioPrefs>,
    /// The runs being made and the runs waiting for a place.
    lanes: Mutex<Lanes>,
    /// Held while a start is checked and recorded.
    starting: tokio::sync::Mutex<()>,
    /// The last client tokens and the runs they made, oldest first.
    recent_tokens: Mutex<VecDeque<(String, String)>>,
}

/// A queue: one endpoint's runs of one kind.
type LaneKey = (store::ProviderId, MediaKind);

/// The runs being made and those waiting, under one lock so a place is never given twice.
#[derive(Default)]
struct Lanes {
    /// Each run whose task is running, by run id.
    running: HashMap<String, Running>,
    /// The runs waiting for a place, in order, per queue.
    queues: HashMap<LaneKey, VecDeque<Job>>,
}

/// A run's task while it runs.
struct Running {
    /// Cancelled by Cancel.
    token: CancellationToken,
    /// Cancelled once the task has ended, so Cancel can wait for it.
    ended: CancellationToken,
    /// Its queue.
    lane: LaneKey,
}

/// A recorded run and what its task needs to send it.
struct Job {
    run_id: String,
    created_at: i64,
    kind: MediaKind,
    lane: LaneKey,
    provider: Arc<dyn MediaProvider>,
    endpoint: Endpoint,
    request: MediaRequest,
    /// The seed sent, kept with each output.
    seed: Option<u64>,
    audit_id: Option<store::AuditId>,
}

/// A task that may start now: the job and its tokens.
type Admitted = (Job, CancellationToken, CancellationToken);

impl Lanes {
    fn active(&self, lane: LaneKey) -> usize {
        self.running.values().filter(|r| r.lane == lane).count()
    }

    fn has_room(&self, lane: LaneKey) -> bool {
        self.active(lane) < places(lane.1)
    }

    /// Gives `job` a place: its task may start.
    fn admit(&mut self, job: Job) -> Admitted {
        let token = CancellationToken::new();
        let ended = CancellationToken::new();
        self.running.insert(
            job.run_id.clone(),
            Running {
                token: token.clone(),
                ended: ended.clone(),
                lane: job.lane,
            },
        );
        (job, token, ended)
    }

    /// The first run waiting in `lane`, admitted, when the queue has room.
    fn next_in(&mut self, lane: LaneKey) -> Option<Admitted> {
        if !self.has_room(lane) {
            return None;
        }
        let job = self.queues.get_mut(&lane)?.pop_front()?;
        Some(self.admit(job))
    }

    /// Takes run `run_id` out of its queue, when it is waiting.
    fn take_queued(&mut self, run_id: &str) -> Option<Job> {
        self.queues.values_mut().find_map(|queue| {
            let at = queue.iter().position(|job| job.run_id == run_id)?;
            queue.remove(at)
        })
    }

    /// Run `run_id`'s place in its queue, from 1, when it is waiting.
    fn position(&self, run_id: &str) -> Option<u32> {
        self.queues.values().find_map(|queue| {
            let at = queue.iter().position(|job| job.run_id == run_id)?;
            Some(u32::try_from(at + 1).unwrap_or(u32::MAX))
        })
    }
}

/// How many runs of `kind` one endpoint makes at once.
fn places(kind: MediaKind) -> usize {
    match kind {
        MediaKind::Image => PICTURES_AT_ONCE,
        MediaKind::Video => VIDEOS_AT_ONCE,
    }
}

/// Frees a run's place however its task ends, a panic included: the task leaves `running`,
/// the next run waiting in its queue starts, and `ended` fires, so Cancel never waits for a
/// task that is gone.
struct TaskEnd {
    inner: Arc<Inner>,
    run_id: String,
    lane: LaneKey,
    ended: CancellationToken,
}

impl Drop for TaskEnd {
    fn drop(&mut self) {
        let next = {
            let mut lanes = self.inner.lanes();
            lanes.running.remove(&self.run_id);
            lanes.next_in(self.lane)
        };
        self.ended.cancel();
        if let Some(admitted) = next {
            spawn(&self.inner, admitted);
        }
    }
}

/// Starts an admitted run's task.
fn spawn(inner: &Arc<Inner>, (job, token, ended): Admitted) {
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        let end = TaskEnd {
            inner: Arc::clone(&inner),
            run_id: job.run_id.clone(),
            lane: job.lane,
            ended,
        };
        inner.run_job(&job, &token).await;
        drop(end);
    });
}

/// How a run ended without its results, before it is recorded.
struct Ending {
    status: store::CreationStatus,
    error: AppError,
    charged: Option<bool>,
    request_id: Option<String>,
    stage: Option<Stage>,
    cost_usd: Option<f64>,
}

impl Ending {
    /// A failure, charged as `charged`.
    fn failed(error: AppError, charged: Option<bool>) -> Self {
        Self {
            status: store::CreationStatus::Failed,
            error,
            charged,
            request_id: None,
            stage: None,
            cost_usd: None,
        }
    }

    /// The user stopped `job`, charged as `charged`.
    fn stopped(job: &Job, charged: Option<bool>) -> Self {
        Self {
            status: store::CreationStatus::Cancelled,
            ..Self::failed(
                errors::stopped(job.endpoint.company, job.kind, charged),
                charged,
            )
        }
    }
}

/// What a request that was sent but got no answer, or was stopped while sending, was
/// charged: an OpenRouter picture nothing (OpenRouter bills only a picture it delivered),
/// anything else unknown.
fn unanswered_charge(job: &Job) -> Option<bool> {
    (job.endpoint.company == Company::OpenRouter && job.kind == MediaKind::Image).then_some(false)
}

/// What a run's request, checked and priced, would send and cost.
struct Prepared {
    model: MediaModel,
    /// The request as it would be sent: the seed added, extra settings placed by company.
    request: MediaRequest,
    /// The seed sent, when there is one.
    seed: Option<u64>,
    /// What the settings break; empty when they pass.
    problems: Vec<Invalid>,
    /// The estimate in US dollars; `None` when the price is unknown.
    usd: Option<f64>,
    billing: Billing,
}

impl StudioManager {
    /// A manager holding `prefs` (Settings → Studio as saved).
    pub fn new(deps: StudioDeps, prefs: StudioPrefs) -> Self {
        Self {
            inner: Arc::new(Inner {
                deps,
                prefs: RwLock::new(prefs),
                lanes: Mutex::new(Lanes::default()),
                starting: tokio::sync::Mutex::new(()),
                recent_tokens: Mutex::new(VecDeque::new()),
            }),
        }
    }

    /// The gallery folder: Settings → Studio's, else the default one. Each run reads it when
    /// its task starts, so a new folder applies to the next run.
    pub fn gallery(&self) -> Gallery {
        self.inner.gallery()
    }

    /// What a run of `req` would cost and meet (confirmation, the daily limit, settings the
    /// model refuses), without recording or sending anything.
    pub async fn estimate(&self, req: &StartRequest) -> AppResult<EstimateDto> {
        let id = match req.target {
            RunTarget::ThisComputer => return self.estimate_local(req).await,
            RunTarget::Endpoint(id) => store::ProviderId(id),
        };
        let prepared = self.inner.prepare(req, id).await?;
        let prefs = self.inner.prefs();
        let spend = self.inner.spend_today().await?;
        Ok(EstimateDto {
            usd: prepared.usd,
            reasons: spend::confirm_reasons(
                prepared.model.kind,
                prepared.billing,
                &prefs.confirm_rules(),
            ),
            confirm_at_usd: prefs.confirm_at_usd,
            limit: LimitDto {
                limit_usd: prefs.daily_limit_usd,
                spent_today_usd: spend.settled_usd,
                pending_usd: spend.pending_usd,
                would_pass: spend::check_limit(prefs.daily_limit_usd, &spend, prepared.billing)
                    .is_ok(),
            },
            invalid: errors::invalid_sentences(&prepared.problems, &prepared.model),
            eta: None,
            local: None,
        })
    }

    /// The exact request a run of `req` would send, with the key removed. Sends nothing.
    pub async fn preview(&self, req: &StartRequest) -> AppResult<RequestPreview> {
        let id = match req.target {
            RunTarget::ThisComputer => return self.preview_local(req).await,
            RunTarget::Endpoint(id) => store::ProviderId(id),
        };
        let prepared = self.inner.prepare(req, id).await?;
        if !prepared.problems.is_empty() {
            return Err(errors::invalid_settings(
                &prepared.problems,
                &prepared.model,
            ));
        }
        let (provider, _) = self.inner.deps.providers.media(id).await?;
        Ok(provider.preview(&prepared.request))
    }

    /// Records a run of `req` and queues it, once it passes every check (see the module
    /// documentation); `confirmed` says the user confirmed it. The same press sent again
    /// gives the run it made.
    pub async fn start(&self, req: StartRequest, confirmed: bool) -> AppResult<RunDto> {
        let id = match req.target {
            RunTarget::ThisComputer => return self.start_local(req).await,
            RunTarget::Endpoint(id) => store::ProviderId(id),
        };
        let _starting = self.inner.starting.lock().await;
        if let Some(run) = self.inner.known_press(&req.client_token).await? {
            return Ok(self.inner.dto_of(&run));
        }
        let prepared = self.inner.prepare(&req, id).await?;
        if !prepared.problems.is_empty() {
            return Err(errors::invalid_settings(
                &prepared.problems,
                &prepared.model,
            ));
        }
        let prefs = self.inner.prefs();
        let spend = self.inner.spend_today().await?;
        if let Err(LimitRefusal::Over { spent, limit }) =
            spend::check_limit(prefs.daily_limit_usd, &spend, prepared.billing)
        {
            return Err(errors::daily_limit(spent, limit));
        }
        let reasons = spend::confirm_reasons(
            prepared.model.kind,
            prepared.billing,
            &prefs.confirm_rules(),
        );
        if !reasons.is_empty() && !confirmed {
            return Err(errors::needs_confirmation(&reasons));
        }
        let (provider, endpoint) = self.inner.deps.providers.media(id).await?;
        let job = self
            .inner
            .record(&req, id, prepared, provider, endpoint, confirmed)
            .await?;
        self.inner.remember_press(&req.client_token, &job.run_id);
        self.inner.enqueue(job).await
    }

    /// Stops run `run_id`: a run waiting for its place is never sent; a run being sent stops
    /// waiting for its answer (its connection is closed). Waits for the run's task to record
    /// how it ended, then gives the run as it is.
    pub async fn cancel(&self, run_id: &str) -> AppResult<RunDto> {
        enum Found {
            Queued(Box<Job>),
            Running(CancellationToken, CancellationToken),
            Neither,
        }
        let found = {
            let mut lanes = self.inner.lanes();
            if let Some(job) = lanes.take_queued(run_id) {
                Found::Queued(Box::new(job))
            } else if let Some(running) = lanes.running.get(run_id) {
                Found::Running(running.token.clone(), running.ended.clone())
            } else {
                Found::Neither
            }
        };
        match found {
            Found::Queued(job) => {
                self.inner
                    .end_run(&job, Ending::stopped(&job, Some(false)), None)
                    .await;
            }
            Found::Running(token, ended) => {
                token.cancel();
                if tokio::time::timeout(CANCEL_WAIT, ended.cancelled())
                    .await
                    .is_err()
                {
                    tracing::warn!(run = %run_id, "a Studio run did not stop in time");
                }
            }
            Found::Neither => {}
        }
        let run = self
            .inner
            .deps
            .store
            .get_creation_run(run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        Ok(self.inner.dto_of(&run))
    }

    /// The runs still working, oldest first.
    pub async fn live(&self) -> AppResult<Vec<RunDto>> {
        let runs = self.inner.deps.store.live_creation_runs().await?;
        Ok(runs.iter().map(|run| self.inner.dto_of(run)).collect())
    }

    /// `run` as the webview shows it, with its place in its queue.
    pub async fn run_dto(&self, run: &store::CreationRun) -> RunDto {
        self.inner.dto_of(run)
    }

    /// Today's spending, the limit, and what the gallery holds.
    pub async fn usage(&self) -> AppResult<UsageDto> {
        let prefs = self.inner.prefs();
        let spend = self.inner.spend_today().await?;
        let kept = self.inner.deps.store.creation_usage().await?;
        let gallery = self.gallery();
        let folder = gallery.root().display().to_string();
        let refs = tokio::task::spawn_blocking(move || gallery.refs_bytes())
            .await
            .unwrap_or(0);
        Ok(UsageDto {
            spent_today_usd: spend.settled_usd,
            pending_usd: spend.pending_usd,
            limit_usd: prefs.daily_limit_usd,
            folder,
            bytes: kept.bytes.saturating_add(refs),
            pictures: kept.pictures,
            videos: kept.videos,
        })
    }

    /// A run on this computer. Task L15 gives it the local lane.
    async fn start_local(&self, _req: StartRequest) -> AppResult<RunDto> {
        Err(errors::no_local_engine())
    }

    /// The estimate of a run on this computer. Task L15 gives it the local lane.
    async fn estimate_local(&self, _req: &StartRequest) -> AppResult<EstimateDto> {
        Err(errors::no_local_engine())
    }

    /// The preview of a run on this computer. Task L15 gives it the local lane.
    async fn preview_local(&self, _req: &StartRequest) -> AppResult<RequestPreview> {
        Err(errors::no_local_engine())
    }
}

impl Inner {
    fn lanes(&self) -> MutexGuard<'_, Lanes> {
        self.lanes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn prefs(&self) -> StudioPrefs {
        self.prefs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn gallery(&self) -> Gallery {
        let dir = self
            .prefs
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .gallery_dir
            .clone();
        Gallery::new(dir.map_or_else(|| self.deps.default_gallery.clone(), PathBuf::from))
    }

    /// The run an earlier press with `token` made, when it is still kept.
    async fn known_press(&self, token: &str) -> AppResult<Option<store::CreationRun>> {
        let run_id = self
            .recent_tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|(t, _)| t == token)
            .map(|(_, run)| run.clone());
        match run_id {
            Some(run_id) => Ok(self.deps.store.get_creation_run(&run_id).await?),
            None => Ok(None),
        }
    }

    /// Notes that the press `token` made run `run_id` (after the run was recorded).
    fn remember_press(&self, token: &str, run_id: &str) {
        let mut tokens = self
            .recent_tokens
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        tokens.push_back((token.to_owned(), run_id.to_owned()));
        while tokens.len() > RECENT_TOKENS {
            tokens.pop_front();
        }
    }

    /// Today's spending, from the runs made since the local day started.
    async fn spend_today(&self) -> AppResult<Spend> {
        let start = self.deps.clock.day_start_ms(self.deps.clock.now_ms());
        let rows: Vec<SpendRow> = self
            .deps
            .store
            .creation_runs_since(start)
            .await?
            .iter()
            .map(spend_row)
            .collect();
        Ok(spend::spent_since(&rows, start))
    }

    /// `req` for endpoint `id` checked and priced: the model from the endpoint's saved lists
    /// (sends nothing), the seed added, the settings checked, the extra settings placed by
    /// company, and the estimate.
    async fn prepare(&self, req: &StartRequest, id: store::ProviderId) -> AppResult<Prepared> {
        let (endpoint, priced) = catalog::model(&self.deps.store, id, &req.model)
            .await
            .ok_or_else(errors::no_endpoint)?;
        let model = priced.model;
        if req.kind != model.kind {
            return Err(errors::wrong_kind(model.kind));
        }
        let mut params = req.params.clone();
        let seed_field = model.params.iter().find_map(|p| match p.kind {
            ParamKind::Seed { max } => Some((p.key.as_str(), max.min(SEED_MAX))),
            _ => None,
        });
        let seed = seed_field.and_then(|(key, max)| {
            let value = params
                .entry(key)
                .or_insert_with(|| Value::from(rand::rng().random_range(0..=max)));
            value.as_u64()
        });
        let mut request = MediaRequest {
            kind: model.kind,
            model: model.id.clone(),
            prompt: req.prompt.clone(),
            params,
            extra: req.extra.clone(),
            refs: self.refs_of(req)?,
            routing: req.routing.clone(),
            provider_options: BTreeMap::new(),
        };
        // Checked as the user set it, then placed: never sent without the placing. A model
        // without a saved `/endpoints` answer (any OpenAI model) keeps every extra setting at
        // the top level.
        let mut problems = validate(&model, &request).err().unwrap_or_default();
        if problems.is_empty()
            && let Err(dropped) = place_extra_or_refuse(&model, &mut request)
        {
            problems = dropped;
        }
        let price = match (&priced.endpoints, endpoint.company, model.kind) {
            (Some(answer), Company::OpenRouter, MediaKind::Image) => {
                parse_image_endpoints(answer, request.routing.as_ref())
            }
            _ => model.price.clone(),
        };
        let usd = estimate_usd(&price, &choice_for(&model, &request));
        Ok(Prepared {
            billing: spend::billing_of(&price, usd),
            usd,
            model,
            request,
            seed,
            problems,
        })
    }

    /// The added pictures of `req`, from the gallery's `refs/`.
    fn refs_of(&self, req: &StartRequest) -> AppResult<Vec<RefImage>> {
        let gallery = self.gallery();
        req.refs
            .iter()
            .map(|input| {
                let (path, mime) = gallery
                    .ref_file(&input.sha256)
                    .map_err(|e| errors::from_gallery(&e))?;
                let size_bytes = std::fs::metadata(&path)
                    .map_err(|e| errors::from_gallery(&io_error(e)))?
                    .len();
                Ok(RefImage {
                    role: input.role,
                    mime,
                    path,
                    sha256: input.sha256.clone(),
                    size_bytes,
                })
            })
            .collect()
    }

    /// Records the run (`queued`) and its audit row, and gives its job.
    async fn record(
        &self,
        req: &StartRequest,
        id: store::ProviderId,
        prepared: Prepared,
        provider: Arc<dyn MediaProvider>,
        endpoint: Endpoint,
        confirmed: bool,
    ) -> AppResult<Job> {
        let Prepared {
            model,
            request,
            seed,
            usd,
            ..
        } = prepared;
        let run_id = uuid::Uuid::new_v4().to_string();
        let created_at = self.deps.clock.now_ms();
        let refs: Vec<Value> = request
            .refs
            .iter()
            .map(|r| {
                json!({
                    "sha256": r.sha256,
                    "role": r.role,
                    "mime": r.mime,
                    "sizeBytes": r.size_bytes,
                })
            })
            .collect();
        self.deps
            .store
            .insert_creation_run(store::NewCreationRun {
                id: run_id.clone(),
                kind: creation_kind(model.kind),
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(id),
                provider_name: endpoint.name.clone(),
                provider_host: endpoint.host.clone(),
                model: model.id.clone(),
                model_label: model.display_name.clone(),
                prompt: req.prompt.clone(),
                params: json!({
                    "params": request.params,
                    "extra": req.extra,
                    "routing": req.routing,
                }),
                refs: Value::Array(refs),
                estimate_usd: usd,
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at,
            })
            .await
            .map_err(|e| match e {
                store::StoreError::TooManyCreations => errors::gallery_full(),
                other => other.into(),
            })?;
        let mut job = Job {
            run_id,
            created_at,
            kind: model.kind,
            lane: (id, model.kind),
            provider,
            endpoint,
            request,
            seed,
            audit_id: None,
        };
        let entry = audit_entry(req, &model, &job, usd, confirmed, created_at);
        let audit_id = match self.deps.store.append_audit(entry).await {
            Ok(audit_id) => audit_id,
            Err(e) => return Err(self.abandon(&job, e.into()).await),
        };
        job.audit_id = Some(audit_id);
        let patch = store::CreationPatch {
            audit_id: Some(Some(audit_id)),
            upstream: Some(upstream_of(&model, job.request.routing.as_ref())),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, patch)
            .await
        {
            return Err(self.abandon(&job, e.into()).await);
        }
        Ok(job)
    }

    /// Ends a run that was recorded but cannot be queued (its records could not be written):
    /// it was not sent, so nothing was charged. Gives `err` back.
    async fn abandon(&self, job: &Job, err: AppError) -> AppError {
        self.end_run(
            job,
            Ending::failed(errors::not_sent(job.endpoint.company), Some(false)),
            None,
        )
        .await;
        err
    }

    /// Queues a recorded run: its task starts now when its queue has room, else it waits.
    /// Gives the run as recorded, after its `queued` event.
    async fn enqueue(self: &Arc<Self>, job: Job) -> AppResult<RunDto> {
        let run = match self.deps.store.get_creation_run(&job.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return Err(self.abandon(&job, AppError::not_found("run")).await),
            Err(e) => return Err(self.abandon(&job, e.into()).await),
        };
        let admitted = {
            let mut lanes = self.lanes();
            if lanes.has_room(job.lane)
                && lanes.queues.get(&job.lane).is_none_or(VecDeque::is_empty)
            {
                Some(lanes.admit(job))
            } else {
                lanes.queues.entry(job.lane).or_default().push_back(job);
                None
            }
        };
        let dto = self.dto_of(&run);
        self.deps.sink.progress(&dto);
        if let Some(admitted) = admitted {
            spawn(self, admitted);
        }
        Ok(dto)
    }

    /// A run's task: sends the request once and records how it went.
    async fn run_job(&self, job: &Job, token: &CancellationToken) {
        if token.is_cancelled() {
            self.end_run(job, Ending::stopped(job, Some(false)), None)
                .await;
            return;
        }
        let started = self.deps.clock.now_ms();
        let sending = store::CreationPatch {
            status: Some(store::CreationStatus::Sending),
            started_at: Some(Some(started)),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, sending)
            .await
        {
            // Never send a run whose sending is not recorded.
            tracing::warn!(run = %job.run_id, "a Studio run could not be marked as sending: {e}");
            self.end_run(
                job,
                Ending::failed(errors::not_sent(job.endpoint.company), Some(false)),
                None,
            )
            .await;
            return;
        }
        self.emit(&job.run_id).await;
        let gallery = self.gallery();
        let scratch = gallery.partial_dir().join(&job.run_id);
        if let Err(e) = tokio::fs::create_dir_all(&scratch).await {
            let error = errors::from_gallery(&io_error(e));
            self.end_run(job, Ending::failed(error, Some(false)), Some(started))
                .await;
            return;
        }
        // An answer that is already there wins over a Cancel pressed at the same moment: its
        // pictures were delivered, so they were charged and are kept.
        let answer = tokio::select! {
            biased;
            answer = job.provider.start(&job.request, &scratch) => Some(answer),
            () = token.cancelled() => None,
        };
        match answer {
            None => {
                let ending = Ending::stopped(job, unanswered_charge(job));
                self.end_run(job, ending, Some(started)).await;
            }
            Some(Ok(Started::Done { outputs, cost })) => {
                self.keep_pictures(job, &gallery, &outputs, cost.map(|c| c.usd), started)
                    .await;
            }
            Some(Ok(Started::Job { remote_id })) => {
                // Task A14 follows the job from here.
                let waiting = store::CreationPatch {
                    status: Some(store::CreationStatus::Waiting),
                    remote_id: Some(Some(remote_id)),
                    ..Default::default()
                };
                if let Err(e) = self
                    .deps
                    .store
                    .update_creation_run(&job.run_id, waiting)
                    .await
                {
                    tracing::warn!(
                        run = %job.run_id,
                        "a started Studio video was not recorded: {e}"
                    );
                }
                self.emit(&job.run_id).await;
            }
            Some(Err(err)) => {
                let ending = self.ending_of(job, err);
                self.end_run(job, ending, Some(started)).await;
            }
        }
        match tokio::fs::remove_dir_all(&scratch).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(run = %job.run_id, "a Studio scratch folder stayed: {e}"),
        }
    }

    /// How a failed request is recorded, with whether it was charged.
    fn ending_of(&self, job: &Job, err: MediaError) -> Ending {
        let company = job.endpoint.company;
        match err {
            MediaError::NotSent(e) => Ending::failed(
                AppError {
                    details: e.to_string(),
                    ..errors::not_sent(company)
                },
                Some(false),
            ),
            MediaError::Answered(f) => Ending {
                status: if f.kind == FailureKind::Blocked {
                    store::CreationStatus::Blocked
                } else {
                    store::CreationStatus::Failed
                },
                error: errors::from_failure(company, &f),
                charged: f.charged,
                request_id: f.request_id,
                stage: f.stage,
                cost_usd: None,
            },
            MediaError::Unknown(e) => {
                let charged = unanswered_charge(job);
                Ending::failed(
                    AppError {
                        details: e.to_string(),
                        ..errors::not_answered(company, charged)
                    },
                    charged,
                )
            }
            MediaError::Local(e) => {
                Ending::failed(errors::from_local(&e, self.deps.limits.picture_cap), None)
            }
        }
    }

    /// Keeps the pictures that came back: each through the gallery's type check and cap, its
    /// thumbnail, its output row; then the run is `done` with its cost and its sidecar. The
    /// pictures were delivered, so a failure here leaves the run charged.
    async fn keep_pictures(
        &self,
        job: &Job,
        gallery: &Gallery,
        outputs: &[Saved],
        cost_usd: Option<f64>,
        started: i64,
    ) {
        // A cost that is not a number, or is below zero, counts as not reported.
        let cost_usd = cost_usd.filter(|usd| usd.is_finite() && *usd >= 0.0);
        let charged = |error: AppError| Ending {
            cost_usd,
            ..Ending::failed(error, Some(true))
        };
        for (idx, saved) in outputs.iter().enumerate() {
            let idx = u32::try_from(idx).unwrap_or(u32::MAX);
            if let Err(error) = self.keep_one(job, gallery, idx, saved).await {
                self.end_run(job, charged(error), Some(started)).await;
                return;
            }
        }
        let now = self.deps.clock.now_ms();
        let done = store::CreationPatch {
            status: Some(store::CreationStatus::Done),
            cost_usd: Some(cost_usd),
            finished_at: Some(Some(now)),
            progress: Some(None),
            ..Default::default()
        };
        if let Err(e) = self.deps.store.update_creation_run(&job.run_id, done).await {
            self.end_run(job, charged(e.into()), Some(started)).await;
            return;
        }
        match self.deps.store.get_creation_run(&job.run_id).await {
            Ok(Some(run)) => {
                let gallery = gallery.clone();
                let written = tokio::task::spawn_blocking(move || gallery.write_sidecar(&run))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                if let Err(e) = written {
                    tracing::warn!(
                        run = %job.run_id,
                        "a Studio run's sidecar was not written: {e}"
                    );
                }
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(run = %job.run_id, "a Studio run could not be read: {e}"),
        }
        self.complete_audit(job, store::CreationStatus::Done, cost_usd, Some(started))
            .await;
        self.emit(&job.run_id).await;
    }

    /// Keeps one picture as output `idx`: moved into the gallery under the run's month once
    /// its bytes pass the type check and the cap, with its thumbnail and its row.
    async fn keep_one(
        &self,
        job: &Job,
        gallery: &Gallery,
        idx: u32,
        saved: &Saved,
    ) -> AppResult<()> {
        let target = gallery.output_stem(job.created_at, &job.run_id, idx);
        let (from, claimed, cap) = (
            saved.path.clone(),
            saved.mime.clone(),
            self.deps.limits.picture_cap,
        );
        let kept = {
            let gallery = gallery.clone();
            tokio::task::spawn_blocking(move || {
                gallery.finalize(&from, Some(claimed.as_str()), &target, cap)
            })
            .await
            .map_err(AppError::internal)?
        };
        let (path, mime) = kept.map_err(|e| errors::from_gallery(&e))?;
        let size_bytes = tokio::fs::metadata(&path)
            .await
            .map_or(saved.size_bytes, |meta| meta.len());
        let output_id = uuid::Uuid::new_v4().to_string();
        let thumb = self.thumbnail(gallery, &output_id, &path, &mime).await;
        self.deps
            .store
            .insert_creation_output(store::NewCreationOutput {
                id: output_id,
                run_id: job.run_id.clone(),
                idx,
                path: path.display().to_string(),
                thumb_path: thumb.map(|t| t.display().to_string()),
                mime,
                size_bytes,
                width: saved.width,
                height: saved.height,
                duration_ms: saved.duration_ms,
                seed: job.seed,
                created_at: self.deps.clock.now_ms(),
            })
            .await?;
        Ok(())
    }

    /// Output `output_id`'s thumbnail, made by the picture worker for a PNG, JPEG or WebP
    /// picture; `None` without a worker, for another type, or when it failed (logged).
    async fn thumbnail(
        &self,
        gallery: &Gallery,
        output_id: &str,
        path: &Path,
        mime: &str,
    ) -> Option<PathBuf> {
        let worker = self.deps.thumbs.as_ref()?;
        if !matches!(mime, "image/png" | "image/jpeg" | "image/webp") {
            return None;
        }
        let bytes = tokio::fs::read(path)
            .await
            .map_err(|e| {
                tracing::warn!("a Studio picture could not be read for its thumbnail: {e}")
            })
            .ok()?;
        let jpeg = worker
            .thumbnail(&bytes)
            .await
            .map_err(|e| tracing::warn!("a Studio thumbnail was not made: {e:?}"))
            .ok()?;
        let (gallery, output_id) = (gallery.clone(), output_id.to_owned());
        match tokio::task::spawn_blocking(move || gallery.write_thumb(&output_id, &jpeg)).await {
            Ok(Ok(path)) => Some(path),
            Ok(Err(e)) => {
                tracing::warn!("a Studio thumbnail was not written: {e}");
                None
            }
            Err(e) => {
                tracing::warn!("a Studio thumbnail was not written: {e}");
                None
            }
        }
    }

    /// Records how `job` ended without its results: its status, its error (always with
    /// `charged`), its end time and its cost; completes its audit row; sends its new state.
    /// `started` is when sending started, if it did.
    async fn end_run(&self, job: &Job, ending: Ending, started: Option<i64>) {
        let error = RunErrorDto {
            code: ending.error.code,
            message: ending.error.message,
            next_step: ending.error.next_step,
            details: ending.error.details,
            charged: ending.charged,
            request_id: ending.request_id,
            company: company_tag(job.endpoint.company).to_owned(),
            stage: ending.stage.map(|stage| stage_tag(stage).to_owned()),
        };
        let patch = store::CreationPatch {
            status: Some(ending.status),
            error: Some(serde_json::to_value(&error).ok()),
            cost_usd: ending.cost_usd.map(Some),
            finished_at: Some(Some(self.deps.clock.now_ms())),
            progress: Some(None),
            ..Default::default()
        };
        if let Err(e) = self
            .deps
            .store
            .update_creation_run(&job.run_id, patch)
            .await
        {
            tracing::warn!(run = %job.run_id, "how a Studio run ended was not recorded: {e}");
        }
        self.complete_audit(job, ending.status, ending.cost_usd, started)
            .await;
        self.emit(&job.run_id).await;
    }

    /// Completes `job`'s audit row with `status` and its cost.
    async fn complete_audit(
        &self,
        job: &Job,
        status: store::CreationStatus,
        cost_usd: Option<f64>,
        started: Option<i64>,
    ) {
        let Some(audit_id) = job.audit_id else {
            return;
        };
        let summary = match cost_usd {
            Some(usd) => format!("cost {}", errors::usd(usd)),
            None => "cost not reported".to_owned(),
        };
        let took = started.and_then(|at| u64::try_from(self.deps.clock.now_ms() - at).ok());
        if let Err(e) = self
            .deps
            .store
            .complete_audit(audit_id, status.as_str().to_owned(), Some(summary), took)
            .await
        {
            tracing::warn!(run = %job.run_id, "a Studio run's audit row was not completed: {e}");
        }
    }

    /// Sends run `run_id`'s state to the sink.
    async fn emit(&self, run_id: &str) {
        match self.deps.store.get_creation_run(run_id).await {
            Ok(Some(run)) => self.deps.sink.progress(&self.dto_of(&run)),
            Ok(None) => {}
            Err(e) => tracing::warn!(run = %run_id, "a Studio run could not be read: {e}"),
        }
    }

    /// `run` as the webview shows it, with whether Studio follows it and its queue place.
    fn dto_of(&self, run: &store::CreationRun) -> RunDto {
        let (watching, queue_position) = {
            let lanes = self.lanes();
            let position = lanes.position(&run.id);
            (
                position.is_some() || lanes.running.contains_key(&run.id),
                position,
            )
        };
        run_dto_of(run, watching, queue_position)
    }
}

/// Moves a request's extra settings to where they go (`openrouter::place_extra`): the top
/// level, or each company's `provider.options` (a model with no saved `/endpoints` answer
/// keeps them all at the top level). A setting only companies outside an `only` routing take
/// would be sent nowhere, so it is refused, by name.
pub(crate) fn place_extra_or_refuse(
    model: &MediaModel,
    request: &mut MediaRequest,
) -> Result<(), Vec<Invalid>> {
    let extra = std::mem::take(&mut request.extra);
    let placed = place_extra(model, extra, request.routing.as_ref());
    if !placed.dropped.is_empty() {
        return Err(placed
            .dropped
            .into_iter()
            .map(Invalid::NotOffered)
            .collect());
    }
    request.extra = placed.top;
    request.provider_options = placed.options;
    Ok(())
}

/// What a run counts toward today's spending: still going, finished, or ended with whether
/// it was charged (the error's `charged`; unknown when it is missing).
pub fn spend_row(run: &store::CreationRun) -> SpendRow {
    use store::CreationStatus as S;
    let state = match run.status {
        S::Queued | S::Sending | S::Waiting | S::Running | S::Downloading => RunState::Live,
        S::Done => RunState::Done,
        S::Failed | S::Blocked | S::Cancelled | S::Interrupted => RunState::Ended {
            charged: run
                .error
                .as_ref()
                .and_then(|e| e.get("charged"))
                .and_then(Value::as_bool),
        },
    };
    SpendRow {
        created_at: run.created_at,
        estimate_usd: run.estimate_usd,
        cost_usd: run.cost_usd,
        charge: spend::charge_of(state),
    }
}

/// The company OpenRouter runs a request at, by name, when it is known before sending: the
/// one the routing pins (`only`, or no fallbacks, with one company in `order`), else the only
/// company in the model's saved `/endpoints` answer.
fn upstream_of(model: &MediaModel, routing: Option<&Routing>) -> Option<String> {
    let pinned = routing
        .filter(|r| r.only || !r.allow_fallbacks)
        .and_then(|r| {
            let first = r.order.first()?;
            r.order.iter().all(|tag| tag == first).then_some(first)
        });
    match pinned {
        Some(tag) => model
            .companies
            .iter()
            .find(|c| c.tag == *tag)
            .map(|c| c.name.clone()),
        None => match model.companies.as_slice() {
            [only] => Some(only.name.clone()),
            _ => None,
        },
    }
}

/// The run's audit row: the endpoint, the model, the settings set (extra settings by name
/// only), how many pictures go with it, the estimate, whether the user confirmed, and the
/// prompt's length, never the prompt.
fn audit_entry(
    req: &StartRequest,
    model: &MediaModel,
    job: &Job,
    usd: Option<f64>,
    confirmed: bool,
    at: i64,
) -> store::AuditEntry {
    let extra_fields: Vec<&String> = req.extra.keys().collect();
    let (tool, preview) = match model.kind {
        MediaKind::Image => (
            "picture",
            format!(
                "Made a picture with {} through {}",
                model.display_name, job.endpoint.name
            ),
        ),
        MediaKind::Video => (
            "video",
            format!(
                "Started a video with {} through {}",
                model.display_name, job.endpoint.name
            ),
        ),
    };
    store::AuditEntry {
        ts: Some(at),
        chat_id: None,
        server: "studio".to_owned(),
        tool: tool.to_owned(),
        arguments: json!({
            "endpoint": job.endpoint.name,
            "host": job.endpoint.host,
            "model": model.id,
            "changed": job.request.params,
            "extra_fields": extra_fields,
            "pictures_sent": job.request.refs.len(),
            "estimate_usd": usd,
            "confirmed": confirmed,
            "prompt_chars": req.prompt.chars().count(),
        }),
        preview,
        risk: "normal".to_owned(),
        decision: store::AuditDecision::AllowedWithoutPrompt,
        decided_by: "user".to_owned(),
        note: None,
        parent_audit_id: None,
        result: None,
    }
}

/// A run as the webview shows it.
fn run_dto_of(run: &store::CreationRun, watching: bool, queue_position: Option<u32>) -> RunDto {
    let settings = |key: &str| -> Map<String, Value> {
        run.params
            .get(key)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    RunDto {
        id: run.id.clone(),
        kind: media_kind(run.kind),
        runs_on: run.runs_on.as_str().to_owned(),
        provider_id: run.provider_id.map(|id| id.0),
        provider_name: run.provider_name.clone(),
        provider_host: run.provider_host.clone(),
        upstream: run.upstream.clone(),
        model: run.model.clone(),
        model_label: run.model_label.clone(),
        prompt: run.prompt.clone(),
        params: settings("params"),
        extra: settings("extra"),
        routing: run
            .params
            .get("routing")
            .and_then(|r| serde_json::from_value(r.clone()).ok()),
        refs: serde_json::from_value::<Vec<RefDto>>(run.refs.clone()).unwrap_or_default(),
        status: run.status.as_str().to_owned(),
        watching,
        stale: false,
        remote_id: run.remote_id.clone(),
        progress: run.progress,
        estimate_usd: run.estimate_usd,
        cost_usd: run.cost_usd,
        error: run
            .error
            .as_ref()
            .and_then(|e| serde_json::from_value(e.clone()).ok()),
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        outputs: run.outputs.iter().map(output_dto).collect(),
        queue_position,
        eta: None,
        local: None,
    }
}

fn output_dto(out: &store::CreationOutput) -> OutputDto {
    OutputDto {
        id: out.id.clone(),
        run_id: out.run_id.clone(),
        idx: out.idx,
        mime: out.mime.clone(),
        size_bytes: out.size_bytes,
        width: out.width,
        height: out.height,
        duration_ms: out.duration_ms,
        seed: out.seed,
        starred: out.starred,
        has_thumb: out.thumb_path.is_some(),
        path: out.path.clone(),
    }
}

fn media_kind(kind: store::CreationKind) -> MediaKind {
    match kind {
        store::CreationKind::Image => MediaKind::Image,
        store::CreationKind::Video => MediaKind::Video,
    }
}

fn creation_kind(kind: MediaKind) -> store::CreationKind {
    match kind {
        MediaKind::Image => store::CreationKind::Image,
        MediaKind::Video => store::CreationKind::Video,
    }
}

/// A company as a run's error names it.
fn company_tag(company: Company) -> &'static str {
    match company {
        Company::OpenRouter => "openrouter",
        Company::OpenAi => "openai",
        Company::Local => "local",
    }
}

/// A safety filter's stage as a run's error names it.
fn stage_tag(stage: Stage) -> &'static str {
    match stage {
        Stage::Prompt => "prompt",
        Stage::Output => "output",
    }
}
