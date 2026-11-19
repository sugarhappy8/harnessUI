//! Studio's runs (0.1.0): starting them under the spending rules, their queues, cancel, the
//! pictures they make, the video jobs they follow (across restarts too) and their audit rows.
//! Modelled on `DownloadManager`.
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
//!   jobs at once; more wait in order. A video followed again (after a restart, *Get it anyway*
//!   or *Check again*) takes a place like a new run. A run's task frees its place however it
//!   ends, a panic included (`TaskEnd`), and starts the next one waiting; when a run leaves a
//!   queue, each run behind it is sent with its new place. A task that broke off (a panic)
//!   first has its run recorded as a restart would find it: `interrupted`, charged unknown
//!   (never charged on this computer), its audit row completed.
//! - **A run's task** writes `sending` and its start time before the request goes out, sends
//!   it once (`MediaProvider::start`, never after a Cancel or a Shutdown already came: a run
//!   stopped before it is sent is not charged), and keeps the pictures that come back in the
//!   gallery (type-checked, at most `MediaLimits::picture_cap` each), with a thumbnail when a
//!   picture worker is set and the run's sidecar. Every failure is recorded with whether it
//!   was charged: not sent, never; a timeout or a cut connection, and a Cancel while sending, not
//!   for an OpenRouter picture (OpenRouter bills only a picture it delivered:
//!   <https://openrouter.ai/docs/guides/overview/multimodal/image-generation>, "Billing and
//!   Cancellation") and unknown otherwise (a video stopped while sending has no job id, so
//!   nothing to get later); a Cancel while queued, never. An added picture no longer in the
//!   gallery is refused before anything is recorded.
//! - **A video job** is recorded as `waiting` with its remote id before it is first checked,
//!   then checked ([`poll_delay`]) until it is done, fails or is gone. A check that fails keeps
//!   it waiting. After [`PollPlan::watch_limit`] it is left `waiting`, "still not ready"
//!   (`stale`), its audit row open, and no longer checked. A done job's cost is recorded first,
//!   then it is `downloading`: each result from the company, or from its file host through the
//!   guarded fetcher (without the key) when the company points there, into
//!   `.partial/<run>/<output id>`, then into the gallery (at most `MediaLimits::video_cap`).
//!   It was charged, so a failure from there keeps the remote id and the cost and is recorded
//!   as charged. Progress is written and sent at most every [`PROGRESS_EVERY`]; a status
//!   change is sent at once.
//! - **Cancel** cancels the run's token: its request's future is dropped, which closes the
//!   connection (an answer already there when Cancel comes is kept: it was delivered, so it
//!   was charged). A video being watched (Stop watching) ends `cancelled`, may still be
//!   charged, and keeps its remote id. Nothing is ever sent again on its own.
//! - **Get it anyway and Check again** ([`StudioManager::fetch_anyway`]) check a stopped,
//!   failed or still-not-ready video job again and download its result; they never send a
//!   create.
//! - **Restarts**: at start, [`StudioManager::resume_interrupted`] removes yesterday's spending
//!   stubs and the added pictures no kept run uses, and picks up what the app left: a run
//!   waiting for its turn was never sent; one being sent may or may not have started; a video
//!   job waiting or downloading is checked again. At exit, [`StudioManager::shutdown`] stops every task without recording anything,
//!   so the next start finds each run as it was.
//! - **The audit row** is completed once when the run ends, with its status and its cost
//!   (after a restart too, through the run's `audit_id`). *Get it anyway* on a run whose row
//!   is already complete appends a follow-up row and completes that one.
//! - **Deleting** results ([`StudioManager::delete_outputs`]) or the runs that ended without
//!   finishing ([`StudioManager::delete_unfinished`]) never lowers today's spending: a run made
//!   today stays as a spending stub. The files, thumbnails and sidecars go; the audit rows
//!   stay. Added pictures stay too (the prompt tray may still hold one): each one no kept
//!   run uses goes at the next start.
//! - **Preferences** ([`StudioManager::set_prefs`]) are checked, saved, then held: the next
//!   run reads them.
//! - **Runs on this computer** (the local lane, [`lane`]): one at a time for the whole
//!   computer, in a queue of their own beside the cloud queues; free, never asked about and
//!   never refused for the daily limit. A run is recorded once the prompt check, the prompt's
//!   length, the settings and the fit pass; its task waits for the graphics card (`queued`,
//!   with what it waits for), runs one engine job checked every [`LOCAL_CHECK_EVERY`], labels
//!   each result as AI-made and keeps it. Stop kills the engine; a run the app left is
//!   `interrupted`, never charged. Without a local lane (`StudioDeps::local`), runs on this
//!   computer are refused.

mod lane;
mod verdict;
mod video;

pub(crate) use verdict::{
    LocalVerdict, device_label, device_name, eta_dto, local_verdict, word_tag,
};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, RwLock};
use std::time::Duration;

use agent::media::openrouter::place_extra;
use agent::media::{
    Company, FailureKind, Invalid, MediaError, MediaKind, MediaLimits, MediaModel, MediaProvider,
    MediaRequest, ParamKind, RefImage, RequestPreview, Routing, SEED_MAX, Saved, Stage, Started,
    choice_for, estimate as estimate_usd, parse_image_endpoints, validate,
};
use models::gen_fit::{self, GenFitResult};
use rand::Rng;
use runtime::sd::args::processor_threads;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::catalog;
use super::deps::{Clock, Endpoint, ForeignFetch, LocalLane, ProviderSource, StudioSink};
use super::errors;
use super::gallery::{Gallery, GalleryError, io_error};
use super::local::{LocalJobOptions, MemoryChoice, runs_on_processor};
use super::prefs::{self, RunTarget, StudioPrefs, StudioPrefsPatch};
use super::spend::{self, Billing, LimitRefusal, RunState, Spend, SpendRow};
use crate::attachments::PictureWorker;
use crate::dto::{EstimateDto, OutputDto, RefDto, RunDto, RunErrorDto, StartRequest, UsageDto};
use crate::error::{AppError, AppResult};
use crate::gpu_lease::Grant;

/// Picture runs one endpoint makes at once; more wait.
pub const PICTURES_AT_ONCE: usize = 2;
/// Video jobs one endpoint runs at once; more wait.
pub const VIDEOS_AT_ONCE: usize = 2;

/// How many recent client tokens are kept, so a press sent twice makes one run.
const RECENT_TOKENS: usize = 64;

/// How long Cancel waits for a run's task to end and record it, and Shutdown for every task
/// to stop.
const CANCEL_WAIT: Duration = Duration::from_secs(30);

/// A run's progress is written and sent at most this often; a status change is sent at once.
pub const PROGRESS_EVERY: Duration = Duration::from_millis(250);

/// How often a run on this computer is checked: in memory, with no `PollPlan` and no watch
/// limit.
pub const LOCAL_CHECK_EVERY: Duration = Duration::from_millis(250);

/// When a video job is checked, and for how long Studio keeps checking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PollPlan {
    /// The wait between checks at first.
    pub first: Duration,
    /// The wait between checks once the job has been watched for `second_from`.
    pub second: Duration,
    /// The wait between checks once the job has been watched for `third_from`.
    pub third: Duration,
    /// When checks move from `first` to `second`.
    pub second_from: Duration,
    /// When checks move from `second` to `third`.
    pub third_from: Duration,
    /// How long a job is watched before it is left "still not ready" and no longer checked.
    pub watch_limit: Duration,
}

impl Default for PollPlan {
    /// Every 5 s for the first minute, then every 10 s, then every 20 s from 5 minutes; 30
    /// minutes in all.
    fn default() -> Self {
        Self {
            first: Duration::from_secs(5),
            second: Duration::from_secs(10),
            third: Duration::from_secs(20),
            second_from: Duration::from_secs(60),
            third_from: Duration::from_secs(5 * 60),
            watch_limit: Duration::from_secs(30 * 60),
        }
    }
}

/// The wait before the next check of a job watched for `watched_for`.
pub fn poll_delay(plan: &PollPlan, watched_for: Duration) -> Duration {
    if watched_for >= plan.third_from {
        plan.third
    } else if watched_for >= plan.second_from {
        plan.second
    } else {
        plan.first
    }
}

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
    /// When video jobs are checked.
    pub poll: PollPlan,
    /// Runs on this computer; `None` refuses them (`studio_no_local_engine`).
    pub local: Option<LocalLane>,
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
    /// Held while a start is checked and recorded, and while a delete decides what goes.
    starting: tokio::sync::Mutex<()>,
    /// Held (shared) while an added picture is kept, and (alone) while the start-up sweep
    /// removes those no kept run uses, so a picture just added is never swept.
    refs_sweep: tokio::sync::RwLock<()>,
    /// Held while a change to the preferences is checked, saved and held, so two changes
    /// never undo each other.
    prefs_writes: tokio::sync::Mutex<()>,
    /// The last client tokens and the runs they made, oldest first.
    recent_tokens: Mutex<VecDeque<(String, String)>>,
    /// Cancelled only by [`StudioManager::shutdown`]: every task stops where it is and records
    /// nothing, and nothing starts after it.
    root: CancellationToken,
    /// The videos still not ready after the watch limit, no longer checked, by run id, each
    /// with the job to follow it again (*Check again*) or to record a Stop watching.
    stale: Mutex<HashMap<String, Job>>,
    /// What the card of each local run Studio follows shows beyond its row, by run id.
    local_live: Mutex<HashMap<String, lane::LocalLive>>,
    /// The chat model a local run's grant unloaded, kept while the local queue has work.
    local_unloaded: Mutex<Option<String>>,
    /// Set by *Quit and stop*: the app is closing, so no chat model is loaded again.
    quitting: AtomicBool,
    /// The last number given to a run as the webview gets it ([`RunDto::seq`]): one count
    /// for every run, taken just before the row that the run is built from is read.
    seq: AtomicU64,
}

/// A queue: one endpoint's runs of one kind, or the one queue of runs on this computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum LaneKey {
    Endpoint(store::ProviderId, MediaKind),
    Local,
}

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
    /// The installed model a run on this computer makes it with (`None` for a cloud run):
    /// *Delete* refuses that model while it runs.
    local_model: Option<String>,
}

/// A recorded run and what its task needs to send it or follow its job. Never cloned: a copy
/// of a job that sends would send again ([`Job::following`] makes one that only follows).
struct Job {
    run_id: String,
    created_at: i64,
    kind: MediaKind,
    lane: LaneKey,
    provider: Arc<dyn MediaProvider>,
    endpoint: Endpoint,
    /// The seed sent, kept with each output.
    seed: Option<u64>,
    audit_id: Option<store::AuditId>,
    step: Step,
}

/// What a run's task does.
enum Step {
    /// Send the run's request, once.
    Send(MediaRequest),
    /// Follow the started video job `remote_id`: check it, then download its result. `started`
    /// is when its request was sent, or when *Get it anyway* started a follow-up. `finished`
    /// is whether the job is known to have reported done, so it was charged (see
    /// `video::finished_job`).
    Follow {
        remote_id: String,
        started: Option<i64>,
        finished: bool,
    },
    /// Make the run on this computer: the graphics card, then one engine job.
    Local(Box<lane::LocalStep>),
}

impl Job {
    /// The installed model a run on this computer makes it with; `None` for a cloud run.
    fn local_model(&self) -> Option<&str> {
        match &self.step {
            Step::Local(step) => Some(step.model()),
            Step::Send(_) | Step::Follow { .. } => None,
        }
    }

    /// When its request was sent (or its follow-up started), for its audit row's duration.
    fn started(&self) -> Option<i64> {
        match &self.step {
            Step::Send(_) | Step::Local(_) => None,
            Step::Follow { started, .. } => *started,
        }
    }

    /// What it is charged when it is stopped before its task does anything: nothing for a
    /// request never sent or a run on this computer; for a video job that started, as
    /// [`Self::job_charge`] says.
    fn charge_if_stopped_waiting(&self) -> Option<bool> {
        match self.step {
            Step::Send(_) | Step::Local(_) => Some(false),
            Step::Follow { .. } => self.job_charge(),
        }
    }

    /// What its started video job is charged when it ends before it reports done (again):
    /// charged when it is known to have finished, else unknown.
    fn job_charge(&self) -> Option<bool> {
        matches!(self.step, Step::Follow { finished: true, .. }).then_some(true)
    }

    /// The same run, to follow its started job `remote_id` (never to send it again), keeping
    /// whether that job is known to have finished.
    fn following(&self, remote_id: &str, started: Option<i64>) -> Self {
        Self {
            run_id: self.run_id.clone(),
            created_at: self.created_at,
            kind: self.kind,
            lane: self.lane,
            provider: Arc::clone(&self.provider),
            endpoint: self.endpoint.clone(),
            seed: self.seed,
            audit_id: self.audit_id,
            step: Step::Follow {
                remote_id: remote_id.to_owned(),
                started,
                finished: self.job_charge() == Some(true),
            },
        }
    }
}

/// A task that may start now: the job and its tokens.
type Admitted = (Job, CancellationToken, CancellationToken);

/// Whether Studio follows a run now.
enum Followed {
    /// Its task runs; `ended` fires when it is gone.
    Running { ended: CancellationToken },
    /// It waits for a place in its queue.
    Queued,
    /// Neither.
    No,
}

/// What waiting for one step of a run's task gave.
enum Waited<T> {
    /// The step's own result.
    Got(T),
    /// The user stopped the run.
    Stopped,
    /// The app is closing: the task records nothing.
    Shutdown,
}

impl Lanes {
    fn active(&self, lane: LaneKey) -> usize {
        self.running.values().filter(|r| r.lane == lane).count()
    }

    fn has_room(&self, lane: LaneKey) -> bool {
        self.active(lane) < places(lane)
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
                local_model: job.local_model().map(str::to_owned),
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

/// How many runs `lane` makes at once: of one kind at one endpoint, or on this computer.
fn places(lane: LaneKey) -> usize {
    match lane {
        LaneKey::Endpoint(_, MediaKind::Image) => PICTURES_AT_ONCE,
        LaneKey::Endpoint(_, MediaKind::Video) => VIDEOS_AT_ONCE,
        LaneKey::Local => lane::LOCAL_AT_ONCE,
    }
}

/// Frees a run's place however its task ends, a panic included: the task leaves `running`,
/// the next run waiting in its queue starts (and those behind it are told their new places),
/// and `ended` fires, so Cancel never waits for a task that is gone.
///
/// A task that broke off (a panic, so `finished` was never set) first has its run recorded as
/// the next start would find it ([`Inner::record_broken`]); the place is freed once that is
/// written. Nothing is recorded once the app is closing.
struct TaskEnd {
    inner: Arc<Inner>,
    run_id: String,
    lane: LaneKey,
    ended: CancellationToken,
    /// The run's company and audit row, to record it when its task broke off.
    company: Company,
    audit_id: Option<store::AuditId>,
    /// Set once the run's task returned: it recorded how the run went, or left it for the
    /// next start.
    finished: bool,
}

impl TaskEnd {
    /// Frees the place: the task leaves `running`, the next run waiting starts, `ended` fires.
    fn release(&self) {
        let next = {
            let mut lanes = self.inner.lanes();
            lanes.running.remove(&self.run_id);
            // Once the app is closing, nothing more starts.
            if self.inner.root.is_cancelled() {
                None
            } else {
                lanes.next_in(self.lane)
            }
        };
        self.ended.cancel();
        if let Some(admitted) = next {
            spawn(&self.inner, admitted);
            // The runs behind the one that started moved up.
            let (inner, lane) = (Arc::clone(&self.inner), self.lane);
            tauri::async_runtime::spawn(async move { inner.tell_queue(lane).await });
        }
    }
}

impl Drop for TaskEnd {
    fn drop(&mut self) {
        if self.finished || self.inner.root.is_cancelled() {
            self.release();
            return;
        }
        // The task broke off before it recorded how the run went. The record is written on
        // the runtime, never here (a drop must not block), then this guard's copy frees the
        // place, even if writing the record breaks off too.
        let end = TaskEnd {
            inner: Arc::clone(&self.inner),
            run_id: std::mem::take(&mut self.run_id),
            lane: self.lane,
            ended: self.ended.clone(),
            company: self.company,
            audit_id: self.audit_id,
            finished: true,
        };
        tauri::async_runtime::spawn(async move {
            end.inner
                .record_broken(&end.run_id, end.company, end.audit_id)
                .await;
            drop(end);
        });
    }
}

/// Starts an admitted run's task.
fn spawn(inner: &Arc<Inner>, (job, token, ended): Admitted) {
    let inner = Arc::clone(inner);
    tauri::async_runtime::spawn(async move {
        let mut end = TaskEnd {
            inner: Arc::clone(&inner),
            run_id: job.run_id.clone(),
            lane: job.lane,
            ended,
            company: job.endpoint.company,
            audit_id: job.audit_id,
            finished: false,
        };
        inner.run_job(&job, &token).await;
        end.finished = true;
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

    /// The user stopped `job` while its request was out, before any answer: a picture is
    /// charged as [`unanswered_charge`] says; a video has no job id yet, so there is nothing
    /// to get later, and it may or may not have started (charged unknown).
    fn stopped_sending(job: &Job) -> Self {
        match job.kind {
            MediaKind::Image => Self::stopped(job, unanswered_charge(job)),
            MediaKind::Video => Self {
                status: store::CreationStatus::Cancelled,
                ..Self::failed(errors::stopped_while_sending(job.endpoint.company), None)
            },
        }
    }
}

/// What a request that was sent but got no answer, or was stopped while sending, was
/// charged: an OpenRouter picture nothing (OpenRouter bills only a picture it delivered),
/// anything else unknown.
fn unanswered_charge(job: &Job) -> Option<bool> {
    (job.endpoint.company == Company::OpenRouter && job.kind == MediaKind::Image).then_some(false)
}

/// A cost a company reported, when it is one: a cost that is not a number, or is below zero,
/// counts as not reported.
fn usable_cost(usd: Option<f64>) -> Option<f64> {
    usd.filter(|usd| usd.is_finite() && *usd >= 0.0)
}

/// What one run's task works with.
struct Task<'a> {
    job: &'a Job,
    /// Cancelled by Cancel.
    token: &'a CancellationToken,
    gallery: Gallery,
    /// `<gallery>/.partial/<run>`: answers and downloads in progress, removed when the task
    /// ends.
    scratch: PathBuf,
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
                refs_sweep: tokio::sync::RwLock::new(()),
                prefs_writes: tokio::sync::Mutex::new(()),
                recent_tokens: Mutex::new(VecDeque::new()),
                root: CancellationToken::new(),
                stale: Mutex::new(HashMap::new()),
                local_live: Mutex::new(HashMap::new()),
                local_unloaded: Mutex::new(None),
                quitting: AtomicBool::new(false),
                seq: AtomicU64::new(0),
            }),
        }
    }

    /// The gallery folder: Settings → Studio's, else the default one. Each run reads it when
    /// its task starts, so a new folder applies to the next run.
    pub fn gallery(&self) -> Gallery {
        self.inner.gallery()
    }

    /// Settings → Studio as the manager holds them now (what new runs read).
    pub fn prefs(&self) -> StudioPrefs {
        self.inner.prefs()
    }

    /// What the manager works with: the store, the endpoints, the clock. The read commands
    /// use the same ones as the runs.
    pub(crate) fn deps(&self) -> &StudioDeps {
        &self.inner.deps
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
            limit: spend::limit_dto(prefs.daily_limit_usd, &spend, prepared.billing),
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
        let seq = self.inner.next_seq();
        if let Some(run) = self.inner.known_press(&req.client_token).await? {
            return Ok(self.inner.dto_of(&run, seq));
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
        spend::check_limit(prefs.daily_limit_usd, &spend, prepared.billing)
            .map_err(LimitRefusal::error)?;
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
    /// waiting for its answer (its connection is closed); a video being watched, or still not
    /// ready, is no longer checked and keeps its remote id (Stop watching); a run on this
    /// computer stops waiting for the graphics card, or its engine is killed. Waits for the
    /// run's task to record how it ended, then gives the run as it is.
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
                let ending = Ending::stopped(&job, job.charge_if_stopped_waiting());
                self.inner.end_run(&job, ending, job.started()).await;
                self.inner.forget_live(&job.run_id);
                // The runs behind it moved up.
                self.inner.tell_queue(job.lane).await;
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
        // A video still not ready has no task (or one just finishing): Stop watching ends it
        // here. It started, so it may still be charged (it was, when it is known to have
        // finished).
        if let Some(job) = self.inner.take_stale(run_id) {
            self.inner
                .end_run(&job, Ending::stopped(&job, job.job_charge()), job.started())
                .await;
        }
        let seq = self.inner.next_seq();
        let run = self
            .inner
            .deps
            .store
            .get_creation_run(run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        Ok(self.inner.dto_of(&run, seq))
    }

    /// "Get it anyway" on a stopped video, and "Check again" on one still not ready: checks
    /// and downloads; never creates.
    ///
    /// A video that was stopped, or failed after its job started (a full disk, a download
    /// that broke, a job the company no longer had), goes back to `waiting` with a follow-up
    /// audit row (the first row is complete and never changed); one still not ready is only
    /// watched again, and its own row is completed when it ends. A run on this computer, a
    /// picture, or a run without a job id is refused; a finished run, or one Studio follows
    /// now, is given as it is.
    pub async fn fetch_anyway(&self, run_id: &str) -> AppResult<RunDto> {
        // One at a time, and never beside a start, so two presses follow a job once.
        let _one_at_a_time = self.inner.starting.lock().await;
        let seq = self.inner.next_seq();
        let run = self
            .inner
            .deps
            .store
            .get_creation_run(run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        let remote_id = match (&run.runs_on, &run.remote_id) {
            (store::RunsOn::Cloud, Some(remote_id)) => remote_id.clone(),
            _ => return Err(errors::nothing_to_fetch()),
        };
        use store::CreationStatus as S;
        let follow_up = match run.status {
            S::Done => return Ok(self.inner.dto_of(&run, seq)),
            S::Waiting | S::Running | S::Downloading => false,
            S::Failed | S::Cancelled => true,
            S::Queued | S::Sending | S::Blocked | S::Interrupted => {
                return Err(errors::nothing_to_fetch());
            }
        };
        match self.inner.followed(run_id) {
            Followed::Queued => return Ok(self.inner.dto_of(&run, seq)),
            Followed::Running { ended } => {
                if !follow_up && !self.inner.is_stale(run_id) {
                    return Ok(self.inner.dto_of(&run, seq));
                }
                // Its last task is just finishing (it went stale or ended): it goes first, so
                // two tasks never share the run's scratch folder.
                if tokio::time::timeout(CANCEL_WAIT, ended.cancelled())
                    .await
                    .is_err()
                {
                    tracing::warn!(run = %run_id, "a Studio run's last task did not end in time");
                    return Ok(self.inner.dto_of(&run, seq));
                }
            }
            Followed::No => {}
        }
        let job = if follow_up {
            self.inner.follow_up(&run, remote_id).await?
        } else if let Some(job) = self.inner.take_stale(run_id) {
            job
        } else {
            self.inner
                .follow_job(&run, remote_id, run.started_at)
                .await?
        };
        self.inner.watch(job).await
    }

    /// At start: removes the spending stubs of runs made before today and each added picture
    /// no kept run uses (no prompt tray holds one yet: drafts are not kept), then picks up
    /// every run the app left unfinished. Runs on this computer first: a local job cannot outlive
    /// the app, so each one queued or running is `interrupted`, not charged, and its
    /// `.partial/<run>` goes. Then the cloud runs: a run waiting for its turn was never sent
    /// (`cancelled`, not charged); one being sent may or may not have started
    /// (`interrupted`, charged unknown); a video job waiting or downloading is checked again,
    /// its watch limit counted from now, and downloaded, never sent again. A job that cannot
    /// be checked now (its endpoint or key is gone) ends `failed` with why (charged when it
    /// had finished, else unknown), keeping its remote id for *Get it anyway*.
    pub async fn resume_interrupted(&self) {
        let inner = &self.inner;
        // Never beside a start: a run just recorded but not yet queued is not the app's leftover.
        let _no_start_now = inner.starting.lock().await;
        let today = inner.deps.clock.day_start_ms(inner.deps.clock.now_ms());
        if let Err(e) = inner.deps.store.purge_creation_stubs(today).await {
            tracing::warn!("yesterday's Studio spending stubs were not removed: {e}");
        }
        inner.sweep_refs().await;
        let runs = match inner.deps.store.live_creation_runs().await {
            Ok(runs) => runs,
            Err(e) => {
                tracing::warn!("Studio's unfinished runs could not be read: {e}");
                return;
            }
        };
        let (local, cloud): (Vec<_>, Vec<_>) = runs
            .into_iter()
            .filter(|run| !inner.is_followed(&run.id))
            .partition(|run| run.runs_on == store::RunsOn::Local);
        for run in local {
            inner.interrupt_local(&run).await;
        }
        for run in cloud {
            inner.resume(run).await;
        }
    }

    /// Stops every run task and waits for them, writing no status: what was `waiting` stays
    /// `waiting` for the next start. Called when the app exits, and by the restart tests in
    /// place of `drop`.
    ///
    /// Runs waiting for a place are left as they are recorded (`queued`, or a video job's
    /// `waiting`), and nothing starts after this.
    pub async fn shutdown(&self) {
        let ended: Vec<CancellationToken> = {
            let mut lanes = self.inner.lanes();
            self.inner.root.cancel();
            lanes.queues.clear();
            lanes.running.values().map(|r| r.ended.clone()).collect()
        };
        let deadline = tokio::time::Instant::now() + CANCEL_WAIT;
        for ended in ended {
            if tokio::time::timeout_at(deadline, ended.cancelled())
                .await
                .is_err()
            {
                tracing::warn!("a Studio run did not stop before the app closed");
                return;
            }
        }
    }

    /// The runs still working, oldest first.
    pub async fn live(&self) -> AppResult<Vec<RunDto>> {
        let seq = self.inner.next_seq();
        let runs = self.inner.deps.store.live_creation_runs().await?;
        Ok(runs.iter().map(|run| self.inner.dto_of(run, seq)).collect())
    }

    /// The next number for a run as the webview gets it ([`RunDto::seq`]). Taken just before
    /// the rows are read, so a row read later never carries a lower number.
    pub fn next_seq(&self) -> u64 {
        self.inner.next_seq()
    }

    /// `run` as the webview shows it, with its place in its queue, numbered `seq` (taken
    /// with [`Self::next_seq`] before `run` was read).
    pub async fn run_dto(&self, run: &store::CreationRun, seq: u64) -> RunDto {
        self.inner.dto_of(run, seq)
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

    /// Deletes outputs `ids` (ids that do not exist are skipped); a finished run left with
    /// none goes too, as a spending stub when it was made today. Their files, thumbnails and
    /// sidecars are removed. Added pictures stay (the prompt tray may still hold one, and
    /// *Reuse* copies a run's into it): those no kept run uses go at the next start
    /// ([`Self::resume_interrupted`]). The audit rows stay.
    pub async fn delete_outputs(&self, ids: Vec<String>) -> AppResult<()> {
        // Never beside a start or *Get it anyway*, which reads the run this may delete.
        let _no_start_now = self.inner.starting.lock().await;
        let today = self.inner.today_starts_at();
        let deleted = self
            .inner
            .deps
            .store
            .delete_creation_outputs(ids, today)
            .await?;
        self.inner.remove_deleted(deleted).await;
        Ok(())
    }

    /// *Delete stopped and failed runs*: deletes every run that ended without finishing
    /// (failed, blocked, cancelled or interrupted), as [`Self::delete_outputs`] deletes a
    /// run, and gives how many went.
    pub async fn delete_unfinished(&self) -> AppResult<u32> {
        // Never beside *Get it anyway*, which takes such a run back to `waiting`.
        let _no_start_now = self.inner.starting.lock().await;
        let store = &self.inner.deps.store;
        let ids = store.unfinished_creation_run_ids().await?;
        let deleted = store
            .delete_creation_runs(ids, self.inner.today_starts_at())
            .await?;
        let count = u32::try_from(deleted.runs.len()).unwrap_or(u32::MAX);
        self.inner.remove_deleted(deleted).await;
        Ok(count)
    }

    /// Checks a picture the user added and keeps it in the gallery's `refs/`
    /// ([`Gallery::add_ref`]), never beside the start-up sweep of unused ones.
    pub async fn add_ref(
        &self,
        worker: &PictureWorker,
        img: &crate::attachments::ImageInput,
    ) -> AppResult<super::gallery::RefStored> {
        let _not_swept_now = self.inner.refs_sweep.read().await;
        self.gallery().add_ref(worker, img).await
    }

    /// Applies `patch` to the preferences held (only the fields it sends change), saves the
    /// result once it keeps the rules (refused as `bad_request`, see [`prefs::save`]), then
    /// holds it, so the next run reads it.
    pub async fn set_prefs(&self, patch: StudioPrefsPatch) -> AppResult<StudioPrefs> {
        let _one_at_a_time = self.inner.prefs_writes.lock().await;
        let next = self
            .inner
            .prefs()
            .apply(patch)
            .map_err(AppError::bad_request)?;
        self.hold_prefs(next).await
    }

    /// Records that the note about local models was accepted, now (the clock's time): saved,
    /// then held like any other preference, so the next local download sees it and a later
    /// change in Settings keeps it. The only writer of `local_notice_accepted_at`.
    pub async fn accept_local_notice(&self) -> AppResult<StudioPrefs> {
        let _one_at_a_time = self.inner.prefs_writes.lock().await;
        let mut next = self.inner.prefs();
        next.local_notice_accepted_at = Some(self.inner.deps.clock.now_ms());
        self.hold_prefs(next).await
    }

    /// Saves `next` and holds it. The caller holds `prefs_writes`.
    async fn hold_prefs(&self, next: StudioPrefs) -> AppResult<StudioPrefs> {
        prefs::save(&self.inner.deps.store, &next).await?;
        *self
            .inner
            .prefs
            .write()
            .unwrap_or_else(PoisonError::into_inner) = next.clone();
        Ok(next)
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

    /// When today started on this computer's clock, in Unix ms.
    fn today_starts_at(&self) -> i64 {
        self.deps.clock.day_start_ms(self.deps.clock.now_ms())
    }

    /// Removes what a delete left behind: the files, thumbnails and sidecars of `deleted`.
    /// Added pictures stay for [`Self::sweep_refs`]. A failure is only logged: the records
    /// are already gone.
    async fn remove_deleted(&self, deleted: store::Deleted) {
        let gallery = self.gallery();
        let removed = tokio::task::spawn_blocking(move || gallery.remove_files(&deleted)).await;
        if let Err(e) = removed {
            tracing::warn!("the files of deleted Studio results were not all removed: {e}");
        }
    }

    /// At start (under the `starting` lock): removes each added picture in `refs/` that no
    /// kept run uses. No prompt tray holds one then (drafts are not kept), and a picture
    /// added meanwhile waits for this. A failure is only logged.
    async fn sweep_refs(&self) {
        let _alone = self.refs_sweep.write().await;
        let in_use = match self.deps.store.creation_ref_hashes().await {
            Ok(in_use) => in_use,
            Err(e) => {
                tracing::warn!("Studio's added pictures in use could not be read: {e}");
                return;
            }
        };
        let gallery = self.gallery();
        let swept = tokio::task::spawn_blocking(move || gallery.remove_unused_refs(&in_use)).await;
        if let Err(e) = swept {
            tracing::warn!("Studio's unused added pictures were not all removed: {e}");
        }
    }

    /// Today's spending, from the runs made since the local day started.
    async fn spend_today(&self) -> AppResult<Spend> {
        let start = self.today_starts_at();
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
        let seed = pick_seed(&model, &mut params);
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

    /// The added pictures of `req`, from the gallery's `refs/`. One no longer there (it went
    /// with the last run that used it, or was removed by hand) is refused by name.
    fn refs_of(&self, req: &StartRequest) -> AppResult<Vec<RefImage>> {
        let gallery = self.gallery();
        req.refs
            .iter()
            .map(|input| {
                let (path, mime) = gallery.ref_file(&input.sha256).map_err(|e| match e {
                    // `ref_file` found no file of any picture type under that name.
                    GalleryError::Io(_) => errors::added_picture_missing(),
                    e => errors::from_gallery(&e),
                })?;
                let size_bytes = std::fs::metadata(&path)
                    .map_err(|e| match e.kind() {
                        std::io::ErrorKind::NotFound => errors::added_picture_missing(),
                        _ => errors::from_gallery(&io_error(e)),
                    })?
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
        let run = store::NewCreationRun {
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
            refs: refs_json(&request.refs),
            estimate_usd: usd,
            gen_model_id: None,
            engine_build: None,
            device: None,
            created_at,
        };
        let entry = audit_entry(req, &model, &request, &endpoint, usd, confirmed, created_at);
        let upstream = upstream_of(&model, request.routing.as_ref());
        let job = Job {
            run_id,
            created_at,
            kind: model.kind,
            lane: LaneKey::Endpoint(id, model.kind),
            provider,
            endpoint,
            seed,
            audit_id: None,
            step: Step::Send(request),
        };
        let patch = store::CreationPatch {
            upstream: Some(upstream),
            ..Default::default()
        };
        self.insert_and_audit(run, entry, job, patch).await
    }

    /// Records `job`'s run (`run`, `queued`), then its audit row (`entry`), linked to the run
    /// with `patch`'s other fields; gives the job with its audit row. Past the most runs the
    /// gallery keeps, refused with `studio_gallery_full`; a run recorded whose audit row
    /// cannot be written or linked is ended as not sent ([`Self::abandon`]). Cloud and local
    /// runs are recorded through here alike.
    async fn insert_and_audit(
        &self,
        run: store::NewCreationRun,
        entry: store::AuditEntry,
        mut job: Job,
        patch: store::CreationPatch,
    ) -> AppResult<Job> {
        self.deps
            .store
            .insert_creation_run(run)
            .await
            .map_err(|e| match e {
                store::StoreError::TooManyCreations => errors::gallery_full(),
                other => other.into(),
            })?;
        let audit_id = match self.deps.store.append_audit(entry).await {
            Ok(audit_id) => audit_id,
            Err(e) => return Err(self.abandon(&job, e.into()).await),
        };
        job.audit_id = Some(audit_id);
        let patch = store::CreationPatch {
            audit_id: Some(Some(audit_id)),
            ..patch
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
        let seq = self.next_seq();
        let run = match self.deps.store.get_creation_run(&job.run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return Err(self.abandon(&job, AppError::not_found("run")).await),
            Err(e) => return Err(self.abandon(&job, e.into()).await),
        };
        Ok(self.place_and_tell(job, &run, seq))
    }

    /// Follows `job`'s video again: its task starts now when its queue has room, else it
    /// waits. Gives the run as it is, after its event.
    async fn watch(self: &Arc<Self>, job: Job) -> AppResult<RunDto> {
        let seq = self.next_seq();
        let run = self
            .deps
            .store
            .get_creation_run(&job.run_id)
            .await?
            .ok_or_else(|| AppError::not_found("run"))?;
        Ok(self.place_and_tell(job, &run, seq))
    }

    /// Gives `job` a place (or a turn in its queue), sends `run` as it is now (numbered
    /// `seq`, taken before it was read), then starts the task when it has a place. The task
    /// may send newer states before the command's answer arrives: each carries a higher
    /// number, so the webview keeps the newest. Once the app is closing, nothing starts.
    fn place_and_tell(self: &Arc<Self>, job: Job, run: &store::CreationRun, seq: u64) -> RunDto {
        let admitted = {
            let mut lanes = self.lanes();
            if !self.root.is_cancelled()
                && lanes.has_room(job.lane)
                && lanes.queues.get(&job.lane).is_none_or(VecDeque::is_empty)
            {
                Some(lanes.admit(job))
            } else {
                lanes.queues.entry(job.lane).or_default().push_back(job);
                None
            }
        };
        let dto = self.dto_of(run, seq);
        self.deps.sink.progress(&dto);
        if let Some(admitted) = admitted {
            spawn(self, admitted);
        }
        dto
    }

    /// Whether Studio follows run `run_id` now: its task runs, or it waits for a place.
    fn followed(&self, run_id: &str) -> Followed {
        let lanes = self.lanes();
        if let Some(running) = lanes.running.get(run_id) {
            Followed::Running {
                ended: running.ended.clone(),
            }
        } else if lanes.position(run_id).is_some() {
            Followed::Queued
        } else {
            Followed::No
        }
    }

    /// Whether this manager already has run `run_id`: followed now, or still not ready.
    fn is_followed(&self, run_id: &str) -> bool {
        !matches!(self.followed(run_id), Followed::No) || self.is_stale(run_id)
    }

    fn stale(&self) -> MutexGuard<'_, HashMap<String, Job>> {
        self.stale.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn is_stale(&self, run_id: &str) -> bool {
        self.stale().contains_key(run_id)
    }

    /// The job of run `run_id` when it is still not ready, no longer kept as such.
    fn take_stale(&self, run_id: &str) -> Option<Job> {
        self.stale().remove(run_id)
    }

    /// Waits for `step`, unless the user stops the run or the app closes first. An answer
    /// that is already there wins: what was delivered is kept.
    async fn until_stopped<T>(
        &self,
        token: &CancellationToken,
        step: impl Future<Output = T>,
    ) -> Waited<T> {
        tokio::select! {
            biased;
            out = step => Waited::Got(out),
            () = token.cancelled() => Waited::Stopped,
            () = self.root.cancelled() => Waited::Shutdown,
        }
    }

    /// A run's task: sends the request once, or follows the video job, and records how it
    /// went; the run's scratch folder goes when it ends. Once the app is closing it does
    /// nothing.
    async fn run_job(&self, job: &Job, token: &CancellationToken) {
        if self.root.is_cancelled() {
            return;
        }
        if token.is_cancelled() {
            let ending = Ending::stopped(job, job.charge_if_stopped_waiting());
            self.end_run(job, ending, job.started()).await;
            return;
        }
        let gallery = self.gallery();
        let scratch = gallery.partial_dir().join(&job.run_id);
        let task = Task {
            job,
            token,
            gallery,
            scratch,
        };
        match &job.step {
            Step::Send(request) => self.send(&task, request).await,
            Step::Follow {
                remote_id, started, ..
            } => self.follow(&task, remote_id, *started).await,
            Step::Local(step) => self.run_local(&task, step).await,
        }
        remove_scratch(&task.gallery, &job.run_id).await;
    }

    /// Sends the run's request once: pictures are kept; a video job is recorded as `waiting`
    /// with its remote id, then followed.
    async fn send(&self, t: &Task<'_>, request: &MediaRequest) {
        let job = t.job;
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
        if let Err(e) = tokio::fs::create_dir_all(&t.scratch).await {
            let error = errors::from_gallery(&io_error(e));
            self.end_run(job, Ending::failed(error, Some(false)), Some(started))
                .await;
            return;
        }
        // A Cancel or a Shutdown may have come during the awaits above. The request's first
        // poll may already hand it to an open connection, so it is never polled after either.
        if self.root.is_cancelled() {
            // The run stays `sending`, as a Shutdown leaves it.
            return;
        }
        if t.token.is_cancelled() {
            // Nothing was sent.
            self.end_run(job, Ending::stopped(job, Some(false)), Some(started))
                .await;
            return;
        }
        match self
            .until_stopped(t.token, job.provider.start(request, &t.scratch))
            .await
        {
            // The run stays `sending`: the next start says it may or may not have started.
            Waited::Shutdown => {}
            Waited::Stopped => {
                self.end_run(job, Ending::stopped_sending(job), Some(started))
                    .await;
            }
            Waited::Got(Ok(Started::Done { outputs, cost })) => {
                self.keep_pictures(job, &t.gallery, &outputs, cost.map(|c| c.usd), started)
                    .await;
            }
            Waited::Got(Ok(Started::Job { remote_id })) => {
                // Written before the first check, so a restart finds the job.
                let waiting = store::CreationPatch {
                    status: Some(store::CreationStatus::Waiting),
                    remote_id: Some(Some(remote_id.clone())),
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
                self.follow(t, &remote_id, Some(started)).await;
            }
            Waited::Got(Err(err)) => {
                let ending = self.ending_of(job, err);
                self.end_run(job, ending, Some(started)).await;
            }
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
        let cost_usd = usable_cost(cost_usd);
        let charged = |error: AppError| Ending {
            cost_usd,
            ..Ending::failed(error, Some(true))
        };
        let cap = self.deps.limits.picture_cap;
        for (idx, saved) in outputs.iter().enumerate() {
            let idx = u32::try_from(idx).unwrap_or(u32::MAX);
            let output_id = uuid::Uuid::new_v4().to_string();
            if let Err(error) = self
                .keep_one(job, gallery, output_id, idx, saved, cap)
                .await
            {
                self.end_run(job, charged(error), Some(started)).await;
                return;
            }
        }
        self.finish(job, gallery, cost_usd, Some(started)).await;
    }

    /// Keeps one result as output `output_id` (number `idx`): moved into the gallery under
    /// the run's month once its bytes pass the type check and `cap`, with its thumbnail (a
    /// picture's) and its row. A cloud result the company gave another of the seven types
    /// is kept as its bytes are ([`Gallery::finalize_paid`]): it was charged.
    async fn keep_one(
        &self,
        job: &Job,
        gallery: &Gallery,
        output_id: String,
        idx: u32,
        saved: &Saved,
        cap: u64,
    ) -> AppResult<()> {
        let target = gallery.output_stem(job.created_at, &job.run_id, idx);
        let (from, claimed) = (saved.path.clone(), saved.mime.clone());
        // A cloud result is already paid for: a wrong type on it never throws it away.
        let paid = job.endpoint.company != Company::Local;
        let kept = {
            let gallery = gallery.clone();
            tokio::task::spawn_blocking(move || {
                if paid {
                    gallery.finalize_paid(&from, Some(claimed.as_str()), &target, cap)
                } else {
                    gallery.finalize(&from, Some(claimed.as_str()), &target, cap)
                }
            })
            .await
            .map_err(AppError::internal)?
        };
        let (path, mime) = kept.map_err(|e| errors::from_gallery(&e))?;
        let size_bytes = tokio::fs::metadata(&path)
            .await
            .map_or(saved.size_bytes, |meta| meta.len());
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

    /// Records `job` done with its files: `done` with its cost (`None` keeps the one already
    /// recorded), its sidecar, its audit row with the cost recorded, its event. A failure to
    /// record it leaves the run failed, charged.
    async fn finish(
        &self,
        job: &Job,
        gallery: &Gallery,
        cost_usd: Option<f64>,
        started: Option<i64>,
    ) {
        let done = store::CreationPatch {
            status: Some(store::CreationStatus::Done),
            cost_usd: cost_usd.map(Some),
            finished_at: Some(Some(self.deps.clock.now_ms())),
            progress: Some(None),
            ..Default::default()
        };
        if let Err(e) = self.deps.store.update_creation_run(&job.run_id, done).await {
            let ending = Ending {
                cost_usd,
                ..Ending::failed(e.into(), Some(true))
            };
            self.end_run(job, ending, started).await;
            return;
        }
        let seq = self.next_seq();
        let run = self.deps.store.get_creation_run(&job.run_id).await;
        let recorded_cost = match &run {
            Ok(Some(run)) => {
                let (gallery, record) = (gallery.clone(), run.clone());
                let written = tokio::task::spawn_blocking(move || gallery.write_sidecar(&record))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r.map_err(|e| e.to_string()));
                if let Err(e) = written {
                    tracing::warn!(
                        run = %job.run_id,
                        "a Studio run's sidecar was not written: {e}"
                    );
                }
                run.cost_usd.or(cost_usd)
            }
            Ok(None) => cost_usd,
            Err(e) => {
                tracing::warn!(run = %job.run_id, "a Studio run could not be read: {e}");
                cost_usd
            }
        };
        self.complete_audit(
            &job.run_id,
            job.audit_id,
            store::CreationStatus::Done,
            recorded_cost,
            started,
        )
        .await;
        if let Ok(Some(run)) = run {
            self.deps.sink.progress(&self.dto_of(&run, seq));
        }
    }

    /// Records how `job` ended without its results (see [`Self::record_end`]).
    async fn end_run(&self, job: &Job, ending: Ending, started: Option<i64>) {
        self.record_end(
            &job.run_id,
            job.endpoint.company,
            job.audit_id,
            ending,
            started,
        )
        .await;
    }

    /// Records how run `run_id` (made through `company`) ended without its results: its
    /// status, its error (always with `charged`), its end time and its cost (`None` keeps
    /// the one recorded); completes its audit row `audit_id` with the cost recorded; sends
    /// its new state. `started` is when sending started, if it did.
    async fn record_end(
        &self,
        run_id: &str,
        company: Company,
        audit_id: Option<store::AuditId>,
        ending: Ending,
        started: Option<i64>,
    ) {
        self.stale().remove(run_id);
        let error = RunErrorDto {
            code: ending.error.code,
            message: ending.error.message,
            next_step: ending.error.next_step,
            details: ending.error.details,
            charged: ending.charged,
            request_id: ending.request_id,
            company: company_tag(company).to_owned(),
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
        if let Err(e) = self.deps.store.update_creation_run(run_id, patch).await {
            tracing::warn!(run = %run_id, "how a Studio run ended was not recorded: {e}");
        }
        let seq = self.next_seq();
        let run = self.deps.store.get_creation_run(run_id).await;
        let recorded_cost = match &run {
            Ok(Some(run)) => run.cost_usd.or(ending.cost_usd),
            _ => ending.cost_usd,
        };
        self.complete_audit(run_id, audit_id, ending.status, recorded_cost, started)
            .await;
        match run {
            Ok(Some(run)) => self.deps.sink.progress(&self.dto_of(&run, seq)),
            Ok(None) => {}
            Err(e) => tracing::warn!(run = %run_id, "a Studio run could not be read: {e}"),
        }
    }

    /// Completes run `run_id`'s audit row `audit_id` with `status` and its cost.
    async fn complete_audit(
        &self,
        run_id: &str,
        audit_id: Option<store::AuditId>,
        status: store::CreationStatus,
        cost_usd: Option<f64>,
        started: Option<i64>,
    ) {
        let Some(audit_id) = audit_id else {
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
            tracing::warn!(run = %run_id, "a Studio run's audit row was not completed: {e}");
        }
    }

    /// Records run `run_id` (made through `company`, with audit row `audit_id`) as the next
    /// start would find it, after its task broke off (a panic) before recording how it went:
    /// `interrupted`, charged unknown (not charged for a run on this computer), its audit row
    /// completed, its scratch folder removed. A run on this computer has its engine job
    /// stopped (the hold on the card went with the task, so the card reads as free); a cloud
    /// job that started (it has a job id) may still finish, so it is `failed`, charged as
    /// [`video::finished_job`] says, and *Get it anyway* can follow it again. A run already
    /// recorded as ended keeps its record.
    async fn record_broken(
        &self,
        run_id: &str,
        company: Company,
        audit_id: Option<store::AuditId>,
    ) {
        tracing::error!(run = %run_id, "a Studio run's task broke off");
        let run = match self.deps.store.get_creation_run(run_id).await {
            Ok(Some(run)) => run,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(run = %run_id, "a Studio run could not be read: {e}");
                return;
            }
        };
        if !still_going(run.status) {
            return;
        }
        let interrupted = |charged| Ending {
            status: store::CreationStatus::Interrupted,
            ..Ending::failed(errors::broke_off(company), charged)
        };
        let ending = match (company, run.remote_id.as_deref()) {
            // Never charged; its engine job, when one began, is stopped.
            (Company::Local, remote_id) => {
                if let (Some(remote_id), Some(local)) = (remote_id, &self.deps.local) {
                    lane::stop_engine(local.provider.as_ref(), remote_id).await;
                }
                interrupted(Some(false))
            }
            (_, Some(_)) => {
                let charged = video::finished_job(&run).then_some(true);
                Ending::failed(errors::broke_off_following(company, charged), charged)
            }
            // It may have started.
            (_, None) => interrupted(None),
        };
        self.record_end(run_id, company, audit_id, ending, run.started_at)
            .await;
        remove_scratch(&self.gallery(), run_id).await;
    }

    /// Tells each run still waiting in `lane` its place, after one left the queue (it got a
    /// place, or was cancelled) and those behind it moved up.
    async fn tell_queue(&self, lane: LaneKey) {
        let waiting: Vec<String> = self
            .lanes()
            .queues
            .get(&lane)
            .map(|queue| queue.iter().map(|job| job.run_id.clone()).collect())
            .unwrap_or_default();
        for run_id in waiting {
            let seq = self.next_seq();
            let run = match self.deps.store.get_creation_run(&run_id).await {
                Ok(Some(run)) => run,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(run = %run_id, "a Studio run could not be read: {e}");
                    continue;
                }
            };
            // Sent under the queues' lock, and only while the run still waits: once it has a
            // place, its own task sends its state, and never before this. A run waiting is
            // never still not ready (that one has no place to wait for).
            let lanes = self.lanes();
            if let Some(position) = lanes.position(&run_id) {
                let mut dto = run_dto_of(&run, true, Some(position), false, seq);
                self.fill_local(&run, &mut dto);
                self.deps.sink.progress(&dto);
            }
        }
    }

    /// Sends run `run_id`'s state to the sink.
    async fn emit(&self, run_id: &str) {
        let seq = self.next_seq();
        match self.deps.store.get_creation_run(run_id).await {
            Ok(Some(run)) => self.deps.sink.progress(&self.dto_of(&run, seq)),
            Ok(None) => {}
            Err(e) => tracing::warn!(run = %run_id, "a Studio run could not be read: {e}"),
        }
    }

    /// The next number for a run as the webview gets it ([`RunDto::seq`]): taken just before
    /// the row is read. Every change is written first, then read and sent, so the state sent
    /// for the last change always carries the highest number.
    fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// `run` as the webview shows it, numbered `seq` (taken with [`Self::next_seq`] before
    /// `run` was read): whether Studio follows it, its queue place, and whether it is still
    /// not ready (no longer followed).
    fn dto_of(&self, run: &store::CreationRun, seq: u64) -> RunDto {
        let (followed, queue_position) = {
            let lanes = self.lanes();
            let position = lanes.position(&run.id);
            (
                position.is_some() || lanes.running.contains_key(&run.id),
                position,
            )
        };
        let stale = self.is_stale(&run.id);
        let mut dto = run_dto_of(run, followed && !stale, queue_position, stale, seq);
        self.fill_local(run, &mut dto);
        dto
    }
}

/// What one run's engine job gets: the processor only from the grant (`UseProcessor`), the
/// `run_on` field (a picture's: a clip runs on the graphics card only) or a run whose fit is
/// on the processor (a processor-only computer, or a picture the graphics device cannot
/// take), with `processor_threads`; memory saving from the `memory_saving` field,
/// `Automatic` → `gen_fit::needs_memory_saving` of the fit it will run with (`running_fit`,
/// the after-unload one when the grant unloaded the chat model; never on the processor, where
/// nothing takes turns); the device description from `LocalHardware::target`, for a run on
/// the device.
pub fn job_options(
    grant: &Grant,
    fit: &GenFitResult,
    req: &MediaRequest,
    device: Option<&str>,
    logical_cores: usize,
) -> LocalJobOptions {
    let params = &req.params;
    let processor_only =
        matches!(grant, Grant::UseProcessor { .. }) || runs_on_processor(req) || fit.processor_only;
    let running = match grant {
        Grant::AfterUnload { .. } => fit.after_unload.unwrap_or(fit.fit),
        Grant::Alongside | Grant::UseProcessor { .. } => fit.fit,
    };
    let memory_saving = match MemoryChoice::of(params) {
        MemoryChoice::On => true,
        MemoryChoice::Off => false,
        MemoryChoice::Automatic => !processor_only && gen_fit::needs_memory_saving(running),
    };
    LocalJobOptions {
        processor_only,
        memory_saving,
        threads: processor_only
            .then(|| processor_threads(logical_cores, cfg!(target_arch = "x86_64"))),
        device_description: device.filter(|_| !processor_only).map(str::to_owned),
    }
}

/// The seed a run of `model` sends: the one `params` set, else one Studio picks and adds to
/// them; `None` when the model takes none.
fn pick_seed(model: &MediaModel, params: &mut Map<String, Value>) -> Option<u64> {
    let (key, max) = model.params.iter().find_map(|p| match p.kind {
        ParamKind::Seed { max } => Some((p.key.as_str(), max.min(SEED_MAX))),
        _ => None,
    })?;
    params
        .entry(key)
        .or_insert_with(|| Value::from(rand::rng().random_range(0..=max)))
        .as_u64()
}

/// The audit row's tool for a run of `kind`.
fn tool_of(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "picture",
        MediaKind::Video => "video",
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

/// Whether a run with `status` is still going (not yet ended).
fn still_going(status: store::CreationStatus) -> bool {
    use store::CreationStatus as S;
    match status {
        S::Queued | S::Sending | S::Waiting | S::Running | S::Downloading => true,
        S::Done | S::Failed | S::Blocked | S::Cancelled | S::Interrupted => false,
    }
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
/// only, free text by its length only: [`audit_settings`]), how many pictures go with it,
/// the estimate, whether the user confirmed, and the prompt's length, never the prompt.
fn audit_entry(
    req: &StartRequest,
    model: &MediaModel,
    request: &MediaRequest,
    endpoint: &Endpoint,
    usd: Option<f64>,
    confirmed: bool,
    at: i64,
) -> store::AuditEntry {
    let extra_fields: Vec<&String> = req.extra.keys().collect();
    let preview = match model.kind {
        MediaKind::Image => format!(
            "Made a picture with {} through {}",
            model.display_name, endpoint.name
        ),
        MediaKind::Video => format!(
            "Started a video with {} through {}",
            model.display_name, endpoint.name
        ),
    };
    let (changed, text_lengths) = audit_settings(model, &request.params);
    let mut arguments = json!({
        "endpoint": endpoint.name,
        "host": endpoint.host,
        "model": model.id,
        "changed": changed,
        "extra_fields": extra_fields,
        "pictures_sent": request.refs.len(),
        "estimate_usd": usd,
        "confirmed": confirmed,
        "prompt_chars": req.prompt.chars().count(),
    });
    if let Some(arguments) = arguments.as_object_mut() {
        arguments.extend(text_lengths);
    }
    store::AuditEntry {
        ts: Some(at),
        chat_id: None,
        server: "studio".to_owned(),
        tool: tool_of(model.kind).to_owned(),
        arguments,
        preview,
        risk: "normal".to_owned(),
        decision: store::AuditDecision::AllowedWithoutPrompt,
        decided_by: "user".to_owned(),
        note: None,
        parent_audit_id: None,
        result: None,
    }
}

/// What an audit row keeps of a run's settings `params`: the settings themselves (for
/// `changed`), except free text (a [`ParamKind::Text`] field of `model`, such as a negative
/// prompt), which is written like the prompt is, as its length only (`<key>_chars`, beside
/// `prompt_chars`), never its text.
pub(crate) fn audit_settings(
    model: &MediaModel,
    params: &Map<String, Value>,
) -> (Map<String, Value>, Map<String, Value>) {
    let is_text = |key: &str| {
        model
            .params
            .iter()
            .any(|p| p.key == key && matches!(p.kind, ParamKind::Text { .. }))
    };
    let mut changed = Map::new();
    let mut lengths = Map::new();
    for (key, value) in params {
        if is_text(key) {
            let chars = value
                .as_str()
                .map_or_else(|| value.to_string().chars().count(), |t| t.chars().count());
            lengths.insert(format!("{key}_chars"), json!(chars));
        } else {
            changed.insert(key.clone(), value.clone());
        }
    }
    (changed, lengths)
}

/// A run as the webview shows it, numbered `seq`.
fn run_dto_of(
    run: &store::CreationRun,
    watching: bool,
    queue_position: Option<u32>,
    stale: bool,
    seq: u64,
) -> RunDto {
    let settings = |key: &str| -> Map<String, Value> {
        run.params
            .get(key)
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    RunDto {
        id: run.id.clone(),
        seq,
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
        stale,
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

/// An output as the webview shows it.
pub(crate) fn output_dto(out: &store::CreationOutput) -> OutputDto {
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

/// The added pictures `refs` as a run records them, cloud or local: each by its SHA-256, role,
/// type and size, never its path (read back by `Store::creation_ref_hashes`).
fn refs_json(refs: &[RefImage]) -> Value {
    Value::Array(
        refs.iter()
            .map(|r| {
                json!({
                    "sha256": r.sha256,
                    "role": r.role,
                    "mime": r.mime,
                    "sizeBytes": r.size_bytes,
                })
            })
            .collect(),
    )
}

/// Removes run `run_id`'s scratch folder in `gallery` (`.partial/<run>`), with what it holds,
/// if it is there.
async fn remove_scratch(gallery: &Gallery, run_id: &str) {
    let scratch = gallery.partial_dir().join(run_id);
    match tokio::fs::remove_dir_all(&scratch).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(run = %run_id, "a Studio scratch folder stayed: {e}"),
    }
}

fn media_kind(kind: store::CreationKind) -> MediaKind {
    match kind {
        store::CreationKind::Image => MediaKind::Image,
        store::CreationKind::Video => MediaKind::Video,
    }
}

/// A media kind as the store names it.
pub(crate) fn creation_kind(kind: MediaKind) -> store::CreationKind {
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
