//! The Studio manager's tests: starting runs under the spending rules, the queue, pictures,
//! cancel and the audit row (Task A13); video jobs, restarts, *Get it anyway* and shutdown
//! (Task A14); deleting results and changing the preferences (Task A16).
//!
//! The harness keeps an OpenRouter endpoint whose saved lists are the recorded fixtures (with
//! every recorded `/endpoints` answer, so an OpenRouter picture has a price), plus test models
//! priced at fixed amounts ([`TEST_PICTURE_PRICES`], [`TEST_VIDEO_PRICES`]), and an OpenAI
//! endpoint whose GPT Image 2.6 entry states no price per picture. Every request goes to a
//! [`FakeProvider`] that counts its starts, checks and downloads and answers as scripted; a
//! second one stands in for the company after a restart. Video jobs are checked every few
//! milliseconds ([`fast_poll`]).
//!
//! The local lane's tests (Task L15) add the installed models of the bundled catalog behind a
//! [`FakeLocal`] engine that records each start's options, a [`FakeLease`] that grants the
//! graphics card as scripted and records what it was asked, and [`FixedHardware`].

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent::ProviderError;
use agent::media::{
    Company, Cost, FailureKind, Fetched, JobDone, JobState, LocalIo, MediaError, MediaFailure,
    MediaKind, MediaLimits, MediaModel, MediaProvider, MediaRequest, ParamKind, Price, RawCatalog,
    RefRole, RequestPreview, Routing, SEED_MAX, Saved, Stage, Started,
};
use async_trait::async_trait;
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use store::AuditDecision;
use url::Url;

use search::FetchError;
use search::netguard::AddressClass;

use super::catalog::{CachedCatalog, CachedPrice, cache_key};
use super::deps::{
    CardLease, Clock, Endpoint, ForeignFetch, LocalHardware, LocalLane, ProviderSource, StudioSink,
};
use super::label::read_label;
use super::local::{LocalJobOptions, LocalMediaProvider, LocalProvider, LocalRunStatus};
use super::manager::{
    LOCAL_CHECK_EVERY, LocalVerdict, PICTURES_AT_ONCE, PollPlan, StudioDeps, StudioManager,
    VIDEOS_AT_ONCE, local_verdict, place_extra_or_refuse, poll_delay, spend_row,
};
use super::prefs::{self, GpuPolicy, RunTarget, StudioPrefs, StudioPrefsPatch};
use super::spend::{Charge, ConfirmReason, UNKNOWN_PRICE_RESERVE_USD, day_start_in};
use super::test_support::{Scratch, mp4, png};
use crate::attachments::PictureWorker;
use crate::dto::{RefInput, RunDto, StartRequest};
use crate::error::{AppError, AppResult};
use crate::gpu_lease::{Grant, LeaseWait, Refused, StudioNeed};
use models::catalog::Catalog;
use models::fit::Target;
use models::gen_catalog::{GenEntry, GenKind, gen_catalog};
use models::gen_eta::FitWord;
use models::gen_fit::GenSize;
use runtime::sd::args::processor_threads;
use runtime::sd::progress::{LocalProgress, LocalStage};
use tokio_util::sync::CancellationToken;

const IMAGES: &str =
    include_str!("../../../crates/agent/tests/fixtures/media/openrouter-images-2026-11-17.json");
const VIDEOS: &str =
    include_str!("../../../crates/agent/tests/fixtures/media/openrouter-videos-2026-11-17.json");
const ENDPOINTS: &str = include_str!(
    "../../../crates/agent/tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json"
);
const OPENAI_MODELS: &str =
    include_str!("../../../crates/agent/tests/fixtures/media/openai-models.json");

const FLUX: &str = "black-forest-labs/flux-3-image";
const NANO_BANANA: &str = "google/gemini-nano-banana-2.1";
const SEEDREAM: &str = "bytedance-seed/seedream-5-5";
const FLARE_ON_OPENROUTER: &str = "openai/gpt-image-2.6-flare";
const VEO: &str = "google/veo-3.1";
const FLARE: &str = "gpt-image-2.6-flare";

/// Test picture models, each priced at one picture for this many US dollars.
const TEST_PICTURE_PRICES: [f64; 4] = [0.96, 1.00, 1.50, 4.50];
/// Test video models, each priced at a 4-second video for this many US dollars.
const TEST_VIDEO_PRICES: [f64; 1] = [0.96];

fn test_picture(usd: f64) -> String {
    format!("test/picture-{usd:.2}")
}

fn test_video(usd: f64) -> String {
    format!("test/video-{usd:.2}")
}

fn object(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

// ---- the fakes --------------------------------------------------------------------------------

/// What [`FakeProvider::start`] answers.
enum Answer {
    /// The pictures come back with the answer.
    Done { pictures: usize, cost: Option<Cost> },
    /// The answer holds one file said to be a PNG that is not a picture at all.
    NotAPicture { cost: Option<Cost> },
    /// The request failed.
    Fail(MediaError),
    /// The company's code panics (a bug in the app).
    Panic,
}

/// What [`FakeProvider::download`] answers; by default it saves an MP4 file.
enum Download {
    /// The result is only at this link on the company's file host.
    Elsewhere(Url),
    /// The download failed.
    Fail(MediaError),
}

/// What a check answers.
type CheckAnswer = Result<JobState, MediaError>;

/// One check the fake answered.
#[derive(Debug, Clone)]
struct Check {
    /// The job it asked about.
    remote_id: String,
    at: Instant,
    /// The status and remote id of the run that held that job when it was asked; `None` when
    /// no run held it.
    holder: Option<(String, Option<String>)>,
}

/// A media company that counts its starts, checks and downloads and answers as scripted (by
/// default two PNG pictures costing $0.04 for a picture, a job `vid_1` for a video, `pending`
/// to a check, an MP4 file to a download) and can hold each start until released.
struct FakeProvider {
    starts: AtomicUsize,
    script: Mutex<VecDeque<Answer>>,
    held: AtomicBool,
    gate: tokio::sync::Semaphore,
    requests: Mutex<Vec<MediaRequest>>,
    /// The run's row as each start found it (status, started_at).
    seen: Mutex<Vec<(String, Option<i64>)>>,
    store: OnceLock<store::Store>,
    /// The answers to the next checks, then `check_default` for every one after them.
    checks: Mutex<VecDeque<CheckAnswer>>,
    check_default: Mutex<CheckAnswer>,
    checked: Mutex<Vec<Check>>,
    downloads: Mutex<VecDeque<Download>>,
    downloaded: AtomicUsize,
}

impl FakeProvider {
    fn new() -> Self {
        Self {
            starts: AtomicUsize::new(0),
            script: Mutex::new(VecDeque::new()),
            held: AtomicBool::new(false),
            gate: tokio::sync::Semaphore::new(0),
            requests: Mutex::new(Vec::new()),
            seen: Mutex::new(Vec::new()),
            store: OnceLock::new(),
            checks: Mutex::new(VecDeque::new()),
            check_default: Mutex::new(Ok(JobState::Pending)),
            checked: Mutex::new(Vec::new()),
            downloads: Mutex::new(VecDeque::new()),
            downloaded: AtomicUsize::new(0),
        }
    }

    /// A company whose every check says the job is done ([`done_job`]).
    fn done_on_first_check() -> Self {
        let fake = Self::new();
        fake.check_from_now(Ok(done_job()));
        fake
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    fn checks(&self) -> usize {
        self.checked.lock().unwrap().len()
    }

    fn downloads(&self) -> usize {
        self.downloaded.load(Ordering::SeqCst)
    }

    /// The answer to the next start not yet scripted.
    fn answer(&self, answer: Answer) {
        self.script.lock().unwrap().push_back(answer);
    }

    /// The answer to the next check not yet scripted.
    fn answer_check(&self, answer: CheckAnswer) {
        self.checks.lock().unwrap().push_back(answer);
    }

    /// The answer to every check once the scripted ones are used.
    fn check_from_now(&self, answer: CheckAnswer) {
        *self.check_default.lock().unwrap() = answer;
    }

    /// The answer to the next download not yet scripted.
    fn answer_download(&self, answer: Download) {
        self.downloads.lock().unwrap().push_back(answer);
    }

    /// From now on each start waits for [`Self::release_all`].
    fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    fn release_all(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.gate.add_permits(64);
    }

    /// Lets one held start go on.
    fn release_one(&self) {
        self.gate.add_permits(1);
    }

    fn last_request(&self) -> MediaRequest {
        self.requests
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a request was sent")
    }
}

#[async_trait]
impl MediaProvider for FakeProvider {
    fn company(&self) -> Company {
        Company::OpenRouter
    }

    fn host(&self) -> String {
        "openrouter.ai".into()
    }

    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
        unreachable!("starting a run never fetches the lists")
    }

    async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
        unreachable!("starting a run never fetches a price")
    }

    fn preview(&self, req: &MediaRequest) -> RequestPreview {
        RequestPreview {
            method: "POST".into(),
            url: "https://openrouter.ai/api/v1/images".into(),
            host: "openrouter.ai".into(),
            body: json!({
                "model": req.model,
                "params": req.params,
                "extra": req.extra,
                "options": req.provider_options,
            }),
        }
    }

    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(req.clone());
        if let Some(store) = self.store.get() {
            let run_id = scratch.file_name().unwrap().to_string_lossy().into_owned();
            let row = store.get_creation_run(&run_id).await.unwrap().unwrap();
            self.seen
                .lock()
                .unwrap()
                .push((row.status.as_str().to_owned(), row.started_at));
        }
        if self.held.load(Ordering::SeqCst) {
            self.gate.acquire().await.unwrap().forget();
        }
        let scripted = self.script.lock().unwrap().pop_front();
        let answer = match (scripted, req.kind) {
            (Some(answer), _) => answer,
            (None, MediaKind::Video) => {
                return Ok(Started::Job {
                    remote_id: "vid_1".into(),
                });
            }
            (None, MediaKind::Image) => Answer::Done {
                pictures: 2,
                cost: Some(Cost {
                    usd: 0.04,
                    reported: true,
                }),
            },
        };
        match answer {
            Answer::Done { pictures, cost } => {
                let mut outputs = Vec::new();
                for i in 0..pictures {
                    let bytes = png(4, 4);
                    let path = scratch.join(format!("picture-{i}"));
                    std::fs::write(&path, &bytes).unwrap();
                    outputs.push(Saved {
                        path,
                        mime: "image/png".into(),
                        size_bytes: bytes.len() as u64,
                        width: Some(4),
                        height: Some(4),
                        duration_ms: None,
                    });
                }
                Ok(Started::Done { outputs, cost })
            }
            Answer::NotAPicture { cost } => {
                let path = scratch.join("picture-0");
                std::fs::write(&path, b"<html>not a picture</html>").unwrap();
                Ok(Started::Done {
                    outputs: vec![Saved {
                        path,
                        mime: "image/png".into(),
                        size_bytes: 26,
                        width: None,
                        height: None,
                        duration_ms: None,
                    }],
                    cost,
                })
            }
            Answer::Fail(err) => Err(err),
            Answer::Panic => panic!("the fake company's code broke"),
        }
    }

    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError> {
        let at = Instant::now();
        let holder = match self.store.get() {
            Some(store) => store
                .live_creation_runs()
                .await
                .unwrap()
                .into_iter()
                .find(|run| run.remote_id.as_deref() == Some(remote_id))
                .map(|run| (run.status.as_str().to_owned(), run.remote_id)),
            None => None,
        };
        self.checked.lock().unwrap().push(Check {
            remote_id: remote_id.to_owned(),
            at,
            holder,
        });
        let scripted = self.checks.lock().unwrap().pop_front();
        scripted.unwrap_or_else(|| self.check_default.lock().unwrap().clone())
    }

    async fn download(
        &self,
        _remote_id: &str,
        _job: &JobDone,
        _index: usize,
        dest: &Path,
    ) -> Result<Fetched, MediaError> {
        self.downloaded.fetch_add(1, Ordering::SeqCst);
        match self.downloads.lock().unwrap().pop_front() {
            Some(Download::Elsewhere(url)) => Ok(Fetched::Elsewhere(url)),
            Some(Download::Fail(err)) => Err(err),
            None => {
                let bytes = mp4();
                std::fs::write(dest, &bytes).unwrap();
                Ok(Fetched::Saved(Saved {
                    path: dest.to_owned(),
                    mime: "video/mp4".into(),
                    size_bytes: bytes.len() as u64,
                    width: Some(1280),
                    height: Some(720),
                    duration_ms: Some(4_000),
                }))
            }
        }
    }
}

/// A finished video job with one result, which cost $0.36.
fn done_job() -> JobState {
    JobState::Done(JobDone {
        outputs: 1,
        signed: Vec::new(),
        cost: Some(Cost {
            usd: 0.36,
            reported: true,
        }),
    })
}

/// The endpoints the harness made, each run by the one [`FakeProvider`].
struct FakeSource {
    provider: Arc<FakeProvider>,
    endpoints: HashMap<i64, Endpoint>,
}

#[async_trait]
impl ProviderSource for FakeSource {
    async fn media(&self, id: store::ProviderId) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)> {
        let endpoint = self
            .endpoints
            .get(&id.0)
            .cloned()
            .ok_or_else(super::errors::no_endpoint)?;
        Ok((self.provider.clone(), endpoint))
    }

    async fn company(&self, id: store::ProviderId) -> Option<Company> {
        self.endpoints.get(&id.0).map(|endpoint| endpoint.company)
    }
}

/// Keeps every progress event, and can hold the run task that sends the next `sending`
/// event ([`Self::pause_at_sending`]).
#[derive(Default)]
struct FakeSink {
    events: Mutex<Vec<RunDto>>,
    pause: Mutex<Option<Arc<Pause>>>,
}

/// Holds a run's task between recording `sending` and sending the request.
#[derive(Default)]
struct Pause {
    /// The task is held.
    reached: AtomicBool,
    /// The task may go on.
    go: AtomicBool,
}

impl FakeSink {
    /// The next `sending` event holds its run's task until the pause's `go` is set.
    fn pause_at_sending(&self) -> Arc<Pause> {
        let pause = Arc::new(Pause::default());
        *self.pause.lock().unwrap() = Some(Arc::clone(&pause));
        pause
    }
}

impl StudioSink for FakeSink {
    fn progress(&self, run: &RunDto) {
        self.events.lock().unwrap().push(run.clone());
        if run.status != "sending" {
            return;
        }
        let pause = self.pause.lock().unwrap().take();
        let Some(pause) = pause else {
            return;
        };
        pause.reached.store(true, Ordering::SeqCst);
        // The run's task is on the app's runtime, not the test's: this thread can wait.
        let deadline = Instant::now() + Duration::from_secs(10);
        while !pause.go.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A company's file host: it notes each link it is asked for (with the cap and where the file
/// goes) and saves an MP4 file there, or fails as scripted.
#[derive(Default)]
struct FakeFetch {
    seen: Mutex<Vec<(Url, u64, PathBuf)>>,
    fail: Mutex<Option<FetchError>>,
}

#[async_trait]
impl ForeignFetch for FakeFetch {
    async fn fetch_to_file(&self, url: &Url, cap: u64, dest: &Path) -> Result<String, FetchError> {
        self.seen
            .lock()
            .unwrap()
            .push((url.clone(), cap, dest.to_owned()));
        if let Some(err) = self.fail.lock().unwrap().take() {
            return Err(err);
        }
        std::fs::write(dest, mp4()).unwrap();
        Ok("video/mp4".into())
    }
}

/// Always the same instant; days start at local midnight.
struct FixedClock(i64);

impl Clock for FixedClock {
    fn now_ms(&self) -> i64 {
        self.0
    }

    fn day_start_ms(&self, at_ms: i64) -> i64 {
        day_start_in(&TimeZone::system(), at_ms)
    }
}

// ---- the harness ------------------------------------------------------------------------------

struct H {
    manager: StudioManager,
    provider: Arc<FakeProvider>,
    /// The company as the manager started after a restart finds it: every check says done.
    provider2: Arc<FakeProvider>,
    sink: Arc<FakeSink>,
    fetch: Arc<FakeFetch>,
    store: store::Store,
    gallery_root: PathBuf,
    openrouter: i64,
    openai: i64,
    now: i64,
    endpoints: HashMap<i64, Endpoint>,
    poll: PollPlan,
    /// Every manager the test made, shut down when it ends.
    managers: Mutex<Vec<StudioManager>>,
    /// The Studio engine on this computer, the graphics card's lease and the hardware; the
    /// manager has them only when the harness was made with them ([`harness_with_local`]).
    local: Arc<FakeLocal>,
    lease: Arc<FakeLease>,
    hardware: Arc<FixedHardware>,
    lane: Option<LocalLane>,
    _scratch: Scratch,
}

impl Drop for H {
    fn drop(&mut self) {
        // Starts still held would wait for ever.
        self.provider.release_all();
        self.provider2.release_all();
        self.local.release_all();
        self.lease.release();
        // Video jobs are checked until their watch limit: every task stops before the
        // gallery folder goes. The tasks run on the app's runtime, so this thread can wait.
        let managers = std::mem::take(&mut *self.managers.lock().unwrap());
        let _ = std::thread::spawn(move || {
            tauri::async_runtime::block_on(async move {
                for manager in managers {
                    manager.shutdown().await;
                }
            });
        })
        .join();
    }
}

/// Video jobs checked every 5 ms, for at most a minute.
fn fast_poll() -> PollPlan {
    let every = Duration::from_millis(5);
    PollPlan {
        first: every,
        second: every,
        third: every,
        watch_limit: Duration::from_secs(60),
        ..PollPlan::default()
    }
}

/// Noon on 17 November 2026 on this computer's clock.
fn noon_2026_11_17() -> i64 {
    jiff::civil::date(2026, 11, 17)
        .at(12, 0, 0, 0)
        .to_zoned(TimeZone::system())
        .unwrap()
        .timestamp()
        .as_millisecond()
}

async fn add_endpoint(store: &store::Store, name: &str, kind: &str, base_url: &str) -> i64 {
    store
        .upsert_provider(store::ProviderInput {
            id: None,
            name: name.into(),
            kind: kind.into(),
            base_url: base_url.into(),
            secret_ref: None,
            enabled: true,
        })
        .await
        .unwrap()
        .0
}

/// OpenRouter's recorded lists and answers, plus the test models.
fn openrouter_catalog(now: i64) -> CachedCatalog {
    let mut images: Value = serde_json::from_str(IMAGES).unwrap();
    let mut videos: Value = serde_json::from_str(VIDEOS).unwrap();
    let recorded: Value = serde_json::from_str(ENDPOINTS).unwrap();
    let mut image_prices: std::collections::BTreeMap<String, CachedPrice> = recorded["answers"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(model, raw)| {
            (
                model.clone(),
                CachedPrice {
                    fetched_at: now,
                    raw: raw.clone(),
                },
            )
        })
        .collect();
    for usd in TEST_PICTURE_PRICES {
        let id = test_picture(usd);
        images["data"].as_array_mut().unwrap().push(json!({
            "id": id, "name": format!("Test: Picture {usd:.2}"), "supported_parameters": {}
        }));
        let raw = json!({"id": id, "endpoints": [{
            "provider_name": "Test Co", "provider_slug": "test-co", "provider_tag": "test-co",
            "supported_parameters": {}, "allowed_passthrough_parameters": [],
            "pricing": [{"billable": "output_image", "unit": "image", "cost_usd": usd}]
        }]});
        image_prices.insert(
            id,
            CachedPrice {
                fetched_at: now,
                raw,
            },
        );
    }
    for usd in TEST_VIDEO_PRICES {
        videos["data"].as_array_mut().unwrap().push(json!({
            "id": test_video(usd), "name": format!("Test: Video {usd:.2}"),
            "supported_durations": [4],
            "pricing_skus": {"duration_seconds": format!("{}", usd / 4.0)}
        }));
    }
    CachedCatalog {
        fetched_at: now,
        raw: RawCatalog {
            images,
            videos: Some(videos),
        },
        image_prices,
    }
}

async fn harness() -> H {
    harness_with(None).await
}

async fn harness_with(thumbs: Option<PictureWorker>) -> H {
    build_harness(thumbs, fast_poll(), false).await
}

async fn harness_polling(poll: PollPlan) -> H {
    build_harness(None, poll, false).await
}

/// The harness with the local lane: the installed models of the bundled catalog, an 8 GB
/// Vulkan card with no chat model loaded, and a lease that grants the card beside it.
async fn harness_with_local() -> H {
    build_harness(None, fast_poll(), true).await
}

async fn build_harness(thumbs: Option<PictureWorker>, poll: PollPlan, with_local: bool) -> H {
    let store = store::Store::open_in_memory().unwrap();
    let now = noon_2026_11_17();
    let openrouter = add_endpoint(
        &store,
        "My OpenRouter",
        "openrouter",
        "https://openrouter.ai/api/v1",
    )
    .await;
    let openai = add_endpoint(&store, "My OpenAI", "openai", "https://api.openai.com/v1").await;
    store
        .set_setting(
            &cache_key(store::ProviderId(openrouter)),
            &openrouter_catalog(now),
        )
        .await
        .unwrap();
    store
        .set_setting(
            &cache_key(store::ProviderId(openai)),
            &CachedCatalog {
                fetched_at: now,
                raw: RawCatalog {
                    images: serde_json::from_str(OPENAI_MODELS).unwrap(),
                    videos: None,
                },
                image_prices: Default::default(),
            },
        )
        .await
        .unwrap();

    let provider = Arc::new(FakeProvider::new());
    let _ = provider.store.set(store.clone());
    let provider2 = Arc::new(FakeProvider::done_on_first_check());
    let _ = provider2.store.set(store.clone());
    let endpoint = |id: i64, name: &str, host: &str, company| Endpoint {
        id: Some(store::ProviderId(id)),
        name: name.into(),
        host: host.into(),
        company,
    };
    let endpoints = HashMap::from([
        (
            openrouter,
            endpoint(
                openrouter,
                "My OpenRouter",
                "openrouter.ai",
                Company::OpenRouter,
            ),
        ),
        (
            openai,
            endpoint(openai, "My OpenAI", "api.openai.com", Company::OpenAi),
        ),
    ]);
    let scratch = Scratch::new();
    let gallery_root = scratch.path().join("gallery");
    let sink = Arc::new(FakeSink::default());
    let fetch = Arc::new(FakeFetch::default());
    let local = Arc::new(FakeLocal::new());
    let lease = Arc::new(FakeLease::new(&sink));
    let hardware = Arc::new(FixedHardware::gpu(8, 32));
    let lane = with_local.then(|| LocalLane {
        provider: local.clone(),
        lease: lease.clone(),
        hardware: hardware.clone(),
    });
    let manager = make_manager(
        &store,
        &provider,
        &endpoints,
        &sink,
        &fetch,
        now,
        &gallery_root,
        thumbs,
        poll,
        lane.clone(),
    );
    H {
        managers: Mutex::new(vec![manager.clone()]),
        manager,
        provider,
        provider2,
        sink,
        fetch,
        store,
        gallery_root,
        openrouter,
        openai,
        now,
        endpoints,
        poll,
        local,
        lease,
        hardware,
        lane,
        _scratch: scratch,
    }
}

#[allow(clippy::too_many_arguments)]
fn make_manager(
    store: &store::Store,
    provider: &Arc<FakeProvider>,
    endpoints: &HashMap<i64, Endpoint>,
    sink: &Arc<FakeSink>,
    fetch: &Arc<FakeFetch>,
    now: i64,
    gallery_root: &Path,
    thumbs: Option<PictureWorker>,
    poll: PollPlan,
    local: Option<LocalLane>,
) -> StudioManager {
    StudioManager::new(
        StudioDeps {
            store: store.clone(),
            providers: Arc::new(FakeSource {
                provider: provider.clone(),
                endpoints: endpoints.clone(),
            }),
            sink: sink.clone(),
            fetch: fetch.clone(),
            clock: Arc::new(FixedClock(now)),
            default_gallery: gallery_root.to_owned(),
            thumbs,
            limits: MediaLimits::default(),
            poll,
            local,
        },
        StudioPrefs::default(),
    )
}

impl H {
    /// Another manager on the same store, gallery and webview, whose requests go to
    /// `provider`: the app as it starts again after a restart.
    fn manager_with(&self, provider: &Arc<FakeProvider>) -> StudioManager {
        let manager = make_manager(
            &self.store,
            provider,
            &self.endpoints,
            &self.sink,
            &self.fetch,
            self.now,
            &self.gallery_root,
            None,
            self.poll,
            self.lane.clone(),
        );
        self.managers.lock().unwrap().push(manager.clone());
        manager
    }

    /// The app as it starts again after a restart that finds no endpoint: no job can be
    /// checked.
    fn manager_without_endpoints(&self) -> StudioManager {
        let manager = StudioManager::new(
            StudioDeps {
                store: self.store.clone(),
                providers: Arc::new(FakeSource {
                    provider: self.provider2.clone(),
                    endpoints: HashMap::new(),
                }),
                sink: self.sink.clone(),
                fetch: self.fetch.clone(),
                clock: Arc::new(FixedClock(self.now)),
                default_gallery: self.gallery_root.clone(),
                thumbs: None,
                limits: MediaLimits::default(),
                poll: fast_poll(),
                local: None,
            },
            StudioPrefs::default(),
        );
        self.managers.lock().unwrap().push(manager.clone());
        manager
    }

    fn req(&self, target: i64, kind: MediaKind, model: &str, prompt: &str) -> StartRequest {
        StartRequest {
            client_token: uuid::Uuid::new_v4().to_string(),
            target: RunTarget::Endpoint(target),
            kind,
            model: model.into(),
            prompt: prompt.into(),
            params: Map::new(),
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
        }
    }

    /// FLUX 3 at 1K on OpenRouter: $0.048, so it never asks.
    fn picture_req(&self, prompt: &str) -> StartRequest {
        StartRequest {
            params: object(json!({"resolution": "1K"})),
            ..self.req(self.openrouter, MediaKind::Image, FLUX, prompt)
        }
    }

    /// Veo 3.1, 4 s with sound: $1.60, and a video always asks.
    fn video_req(&self) -> StartRequest {
        StartRequest {
            params: object(json!({"duration": 4})),
            ..self.req(self.openrouter, MediaKind::Video, VEO, "a wave breaking")
        }
    }

    fn video_req_estimated(&self, usd: f64) -> StartRequest {
        StartRequest {
            params: object(json!({"duration": 4})),
            ..self.req(
                self.openrouter,
                MediaKind::Video,
                &test_video(usd),
                "a wave breaking",
            )
        }
    }

    fn req_estimated(&self, usd: f64, token: &str) -> StartRequest {
        StartRequest {
            client_token: token.into(),
            ..self.req(
                self.openrouter,
                MediaKind::Image,
                &test_picture(usd),
                "a lighthouse",
            )
        }
    }

    fn req_token(&self, token: &str) -> StartRequest {
        StartRequest {
            client_token: token.into(),
            ..self.picture_req("x")
        }
    }

    /// GPT Image 2.6 Flare on OpenAI: no price per picture, so it always asks.
    fn openai_req(&self) -> StartRequest {
        self.req(self.openai, MediaKind::Image, FLARE, "a red kite")
    }

    fn openai_req_token(&self, token: &str) -> StartRequest {
        StartRequest {
            client_token: token.into(),
            ..self.openai_req()
        }
    }

    async fn run_dto(&self, id: &str) -> RunDto {
        let run = self.store.get_creation_run(id).await.unwrap().unwrap();
        self.manager.run_dto(&run).await
    }

    /// The run once its status is `status` (at most 10 s). For a status a run ends with, also
    /// once its task is gone, so its last event, audit row, sidecar and scratch folder are
    /// done.
    async fn wait_status(&self, id: &str, status: &str) -> RunDto {
        self.wait_status_in(&self.manager, id, status).await
    }

    /// [`Self::wait_status`] as `manager`, the one following the run, sees it.
    async fn wait_status_in(&self, manager: &StudioManager, id: &str, status: &str) -> RunDto {
        let ends = matches!(
            status,
            "done" | "failed" | "blocked" | "cancelled" | "interrupted"
        );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let row = self.store.get_creation_run(id).await.unwrap().unwrap();
            let run = manager.run_dto(&row).await;
            if run.status == status && !(ends && run.watching) {
                return run;
            }
            assert!(
                Instant::now() < deadline,
                "run {id} is still {} ({:?}), not {status}",
                run.status,
                run.error
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The statuses the progress events of run `id` gave, each once in a row.
    fn statuses(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for event in self.sink.events.lock().unwrap().iter() {
            if event.id == id && out.last() != Some(&event.status) {
                out.push(event.status.clone());
            }
        }
        out
    }

    async fn store_runs(&self) -> Vec<store::CreationRun> {
        self.store
            .list_creation_runs(store::CreationQuery::default())
            .await
            .unwrap()
    }

    async fn audit_rows(&self) -> Vec<store::AuditRow> {
        self.store
            .list_audit(store::AuditQuery {
                server: Some("studio".into()),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    /// A picture run made earlier today that finished costing `usd`, with one file.
    async fn seed_done_run_costing(&self, usd: f64) -> store::CreationRun {
        self.seed_done_run(usd, json!([])).await
    }

    /// [`Self::seed_done_run_costing`], with the added pictures `refs` as recorded.
    async fn seed_done_run(&self, usd: f64, refs: Value) -> store::CreationRun {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = self.now - 60_000;
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(store::ProviderId(self.openrouter)),
                provider_name: "My OpenRouter".into(),
                provider_host: "openrouter.ai".into(),
                model: FLUX.into(),
                model_label: "FLUX 3 Image".into(),
                prompt: "an earlier picture".into(),
                params: json!({"params": {}, "extra": {}, "routing": null}),
                refs,
                estimate_usd: Some(usd),
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at,
            })
            .await
            .unwrap();
        let path = self
            .manager
            .gallery()
            .output_path(created_at, &id, 0, "png");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, png(4, 4)).unwrap();
        self.store
            .insert_creation_output(store::NewCreationOutput {
                id: uuid::Uuid::new_v4().to_string(),
                run_id: id.clone(),
                idx: 0,
                path: path.display().to_string(),
                thumb_path: None,
                mime: "image/png".into(),
                size_bytes: 10,
                width: Some(4),
                height: Some(4),
                duration_ms: None,
                seed: None,
                created_at,
            })
            .await
            .unwrap();
        self.store
            .update_creation_run(
                &id,
                store::CreationPatch {
                    status: Some(store::CreationStatus::Done),
                    cost_usd: Some(Some(usd)),
                    started_at: Some(Some(created_at)),
                    finished_at: Some(Some(created_at)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        self.store.get_creation_run(&id).await.unwrap().unwrap()
    }

    async fn store_run(&self, id: &str) -> store::CreationRun {
        self.store.get_creation_run(id).await.unwrap().unwrap()
    }

    /// The run once `done` holds for it (at most 10 s).
    async fn wait_for(&self, id: &str, what: &str, done: impl Fn(&RunDto) -> bool) -> RunDto {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let run = self.run_dto(id).await;
            if done(&run) {
                return run;
            }
            assert!(Instant::now() < deadline, "run {id} is not {what}: {run:?}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// The progress events of run `id`, in order.
    fn events_for(&self, id: &str) -> Vec<RunDto> {
        self.sink
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.id == id)
            .cloned()
            .collect()
    }

    /// Run `id`'s audit rows, oldest first: its first row, then each follow-up.
    async fn audit_rows_for(&self, id: &str) -> Vec<store::AuditRow> {
        let all = self.audit_rows().await;
        let mut next = self.store_run(id).await.audit_id;
        let mut chain = Vec::new();
        while let Some(audit_id) = next {
            let row = all
                .iter()
                .find(|row| row.id == audit_id)
                .expect("the run's audit row")
                .clone();
            next = row.parent_audit_id;
            chain.push(row);
        }
        chain.reverse();
        chain
    }

    /// Whether the scratch folder of run `id` is gone (or empty).
    fn scratch_is_gone(&self, id: &str) -> bool {
        let dir = self.gallery_root.join(".partial").join(id);
        std::fs::read_dir(&dir).map_or(true, |mut entries| entries.next().is_none())
    }

    /// A Veo video run as the app left it when it closed: `status`, with `remote_id`, sent a
    /// minute ago, with its audit row still open.
    async fn seed_video_run(
        &self,
        status: store::CreationStatus,
        remote_id: Option<&str>,
    ) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        let created_at = self.now - 60_000;
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Video,
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(store::ProviderId(self.openrouter)),
                provider_name: "My OpenRouter".into(),
                provider_host: "openrouter.ai".into(),
                model: VEO.into(),
                model_label: "Veo 3.1".into(),
                prompt: "a wave breaking".into(),
                params: json!({"params": {"duration": 4, "seed": 7}, "extra": {}, "routing": null}),
                refs: json!([]),
                estimate_usd: Some(1.6),
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at,
            })
            .await
            .unwrap();
        let audit_id = self
            .store
            .append_audit(store::AuditEntry {
                ts: Some(created_at),
                chat_id: None,
                server: "studio".into(),
                tool: "video".into(),
                arguments: json!({"model": VEO, "prompt_chars": 15}),
                preview: "Started a video with Veo 3.1 through My OpenRouter".into(),
                risk: "normal".into(),
                decision: AuditDecision::AllowedWithoutPrompt,
                decided_by: "user".into(),
                note: None,
                parent_audit_id: None,
                result: None,
            })
            .await
            .unwrap();
        self.store
            .update_creation_run(
                &id,
                store::CreationPatch {
                    status: Some(status),
                    remote_id: Some(remote_id.map(str::to_owned)),
                    audit_id: Some(Some(audit_id)),
                    started_at: Some(Some(created_at)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        id
    }

    /// A spending stub: a picture run made at `created_at` and deleted that same day.
    async fn seed_stub(&self, created_at: i64) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(store::ProviderId(self.openrouter)),
                provider_name: "My OpenRouter".into(),
                provider_host: "openrouter.ai".into(),
                model: FLUX.into(),
                model_label: "FLUX 3 Image".into(),
                prompt: "deleted".into(),
                params: json!({}),
                refs: json!([]),
                estimate_usd: Some(0.05),
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at,
            })
            .await
            .unwrap();
        self.store
            .delete_creation_runs(
                vec![id.clone()],
                day_start_in(&TimeZone::system(), created_at),
            )
            .await
            .unwrap();
        id
    }

    /// The ids of the spending stubs kept, sorted.
    async fn stub_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self
            .store
            .creation_runs_since(0)
            .await
            .unwrap()
            .into_iter()
            .filter(|run| run.deleted_at.is_some())
            .map(|run| run.id)
            .collect();
        ids.sort();
        ids
    }
}

/// Waits until `done` holds (at most 10 s).
async fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "still waiting: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn network(text: &str) -> ProviderError {
    ProviderError::Network(text.into())
}

// ---- starting a run ---------------------------------------------------------------------------

#[tokio::test]
async fn a_picture_run_goes_from_queued_to_done_with_its_files() {
    let h = harness().await;
    let run = h
        .manager
        .start(h.picture_req("lighthouse at dusk"), false)
        .await
        .unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 2);
    assert!(Path::new(&done.outputs[0].path).starts_with(h.gallery_root.join("2026-11")));
    assert!(Path::new(&done.outputs[0].path).is_file());
    assert_eq!(done.outputs[0].mime, "image/png");
    assert_eq!(done.cost_usd, Some(0.04));
    assert_eq!(done.estimate_usd, Some(0.048));
    assert!(done.started_at.is_some() && done.finished_at.is_some());
    assert_eq!(h.statuses(&run.id), ["queued", "sending", "done"]);
    // The scratch folder the answer was written to is gone.
    assert!(!h.gallery_root.join(".partial").join(&run.id).exists());
}

#[tokio::test]
async fn sending_is_recorded_before_the_request_goes_out() {
    let h = harness().await;
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let seen = h.provider.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].0, "sending");
    assert_eq!(seen[0].1, Some(h.now));
}

#[tokio::test]
async fn a_video_without_confirmation_is_refused() {
    let h = harness().await;
    let err = h.manager.start(h.video_req(), false).await.unwrap_err();
    assert_eq!(err.code, "studio_needs_confirmation");
    assert_eq!(h.provider.starts(), 0);
    assert!(h.store_runs().await.is_empty());
    assert!(h.audit_rows().await.is_empty());
}

#[tokio::test]
async fn a_video_model_sent_as_a_picture_is_refused() {
    let h = harness().await;
    for confirmed in [false, true] {
        let err = h
            .manager
            .start(
                StartRequest {
                    kind: MediaKind::Image,
                    ..h.video_req()
                },
                confirmed,
            )
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_request");
    }
    assert_eq!(h.provider.starts(), 0);
}

#[tokio::test]
async fn a_started_video_waits_with_its_remote_id() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let waiting = h.wait_status(&run.id, "waiting").await;
    assert_eq!(waiting.remote_id.as_deref(), Some("vid_1"));
    assert_eq!(waiting.estimate_usd, Some(1.6));
    // A started video's audit row stays open until the job ends (Task A14).
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.tool, "video");
    assert_eq!(row.result_status, None);
}

// ---- the spending rules -----------------------------------------------------------------------

#[tokio::test]
async fn a_run_over_the_daily_limit_is_refused_whatever_the_webview_says() {
    let h = harness().await;
    h.seed_done_run_costing(9.50).await;
    let err = h
        .manager
        .start(h.video_req_estimated(0.96), true)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_daily_limit");
    assert_eq!(err.message, "Daily limit reached: $9.50 of $10 spent today");
    assert_eq!(h.provider.starts(), 0);
}

#[tokio::test]
async fn queued_and_running_estimates_count_against_the_limit() {
    let h = harness().await;
    h.provider.hold();
    for token in ["t1", "t2"] {
        h.manager
            .start(h.req_estimated(4.50, token), true)
            .await
            .unwrap();
    }
    wait_until("both pictures are sent", || h.provider.starts() == 2).await;
    let err = h
        .manager
        .start(h.req_estimated(1.50, "t3"), true)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_daily_limit");
    assert_eq!(err.message, "Daily limit reached: $9.00 of $10 spent today");
    assert_eq!(h.provider.starts(), 2);
}

#[tokio::test]
async fn two_starts_at_once_cannot_both_pass_the_limit() {
    let h = harness().await;
    h.seed_done_run_costing(8.50).await;
    // Held, so the run that passes still counts its estimate when the other is checked.
    h.provider.hold();
    let (a, b) = tokio::join!(
        h.manager.start(h.req_estimated(1.00, "t1"), true),
        h.manager.start(h.req_estimated(1.00, "t2"), true)
    );
    assert_eq!([a.is_ok(), b.is_ok()].iter().filter(|ok| **ok).count(), 1);
    let refused = a.err().or(b.err()).unwrap();
    assert_eq!(refused.code, "studio_daily_limit");
}

#[tokio::test]
async fn the_same_create_press_twice_makes_one_run() {
    let h = harness().await;
    let a = h
        .manager
        .start(h.req_token("press-1"), false)
        .await
        .unwrap();
    let b = h
        .manager
        .start(h.req_token("press-1"), false)
        .await
        .unwrap();
    assert_eq!(a.id, b.id);
    h.wait_status(&a.id, "done").await;
    assert_eq!(h.provider.starts(), 1);
    assert_eq!(h.store_runs().await.len(), 1);
}

#[tokio::test]
async fn a_refused_unconfirmed_press_then_the_confirmed_one_makes_one_run() {
    let h = harness().await;
    let req = || StartRequest {
        client_token: "press-2".into(),
        ..h.video_req()
    };
    assert_eq!(
        h.manager.start(req(), false).await.unwrap_err().code,
        "studio_needs_confirmation"
    );
    let a = h.manager.start(req(), true).await.unwrap();
    let b = h.manager.start(req(), true).await.unwrap();
    assert_eq!(a.id, b.id);
    assert_eq!(h.store_runs().await.len(), 1);
}

#[tokio::test]
async fn three_openai_runs_in_a_row_stay_within_the_limit() {
    let h = harness().await;
    for token in ["o1", "o2", "o3"] {
        h.provider.answer(Answer::Done {
            pictures: 1,
            cost: Some(Cost {
                usd: 0.07,
                reported: false,
            }),
        });
        let r = h
            .manager
            .start(h.openai_req_token(token), true)
            .await
            .unwrap();
        h.wait_status(&r.id, "done").await;
    }
    h.provider.hold();
    assert!(
        h.manager
            .start(h.openai_req_token("o4"), true)
            .await
            .is_ok()
    );
    wait_until("the fourth run is sent", || h.provider.starts() == 4).await;
    let u = h.manager.usage().await.unwrap();
    assert!(
        (u.spent_today_usd - 0.21).abs() < 1e-9,
        "{}",
        u.spent_today_usd
    );
    assert_eq!(u.pending_usd, UNKNOWN_PRICE_RESERVE_USD);
    assert_eq!(u.limit_usd, Some(10.0));
}

#[tokio::test]
async fn a_negative_cost_counts_as_unreported() {
    let h = harness().await;
    h.provider.answer(Answer::Done {
        pictures: 1,
        cost: Some(Cost {
            usd: -1.0,
            reported: true,
        }),
    });
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.cost_usd, None);
    // It counts at its estimate.
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.048).abs() < 1e-9, "{spent}");
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_summary.as_deref(), Some("cost not reported"));
}

// ---- the request ------------------------------------------------------------------------------

#[tokio::test]
async fn studio_picks_a_seed_when_the_model_takes_one() {
    let h = harness().await;
    let seeded = h
        .manager
        .start(
            StartRequest {
                params: object(json!({"resolution": "1K"})),
                ..h.req(h.openrouter, MediaKind::Image, SEEDREAM, "a fox")
            },
            true,
        )
        .await
        .unwrap();
    h.wait_status(&seeded.id, "done").await;
    let sent = h.provider.last_request();
    let row = h.store.get_creation_run(&seeded.id).await.unwrap().unwrap();
    let seed = row.params["params"]["seed"].as_u64().expect("a seed");
    assert!(seed <= SEED_MAX);
    assert_eq!(sent.params["seed"], json!(seed));
    assert_eq!(row.outputs[0].seed, Some(seed));

    let plain = h
        .manager
        .start(
            h.req(h.openrouter, MediaKind::Image, NANO_BANANA, "a fox"),
            true,
        )
        .await
        .unwrap();
    h.wait_status(&plain.id, "done").await;
    let row = h.store.get_creation_run(&plain.id).await.unwrap().unwrap();
    assert!(row.params["params"].get("seed").is_none());
    assert!(h.provider.last_request().params.get("seed").is_none());
}

#[tokio::test]
async fn a_seed_the_user_set_is_kept() {
    let h = harness().await;
    let run = h
        .manager
        .start(
            StartRequest {
                params: object(json!({"resolution": "1K", "seed": 42})),
                ..h.req(h.openrouter, MediaKind::Image, SEEDREAM, "a fox")
            },
            true,
        )
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    assert_eq!(h.provider.last_request().params["seed"], json!(42));
}

#[tokio::test]
async fn a_field_the_model_does_not_list_is_refused_before_anything_is_sent() {
    let h = harness().await;
    let err = h
        .manager
        .start(
            StartRequest {
                params: object(json!({"seed": 1})),
                ..h.req(h.openrouter, MediaKind::Image, NANO_BANANA, "a fox")
            },
            true,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_bad_settings");
    assert_eq!(err.message, "Seed is not offered by this model.");
    assert_eq!(err.next_step, "Change the settings and try again.");
    assert_eq!(h.provider.starts(), 0);
    assert!(h.store_runs().await.is_empty());
}

#[tokio::test]
async fn extra_settings_go_to_the_companies_that_take_them() {
    let h = harness().await;
    let run = h
        .manager
        .start(
            StartRequest {
                extra: object(json!({"moderation": "low"})),
                ..h.req(
                    h.openrouter,
                    MediaKind::Image,
                    FLARE_ON_OPENROUTER,
                    "a red kite",
                )
            },
            true,
        )
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    let sent = h.provider.last_request();
    assert!(sent.extra.is_empty());
    assert_eq!(
        sent.provider_options["openai"],
        object(json!({"moderation": "low"}))
    );
    // The run keeps the extra settings as the user set them.
    let row = h.store.get_creation_run(&run.id).await.unwrap().unwrap();
    assert_eq!(row.params["extra"], json!({"moderation": "low"}));
}

#[test]
fn an_extra_setting_only_companies_outside_the_routing_take_is_refused() {
    let catalog = openrouter_catalog(0);
    let answers = catalog
        .image_prices
        .iter()
        .map(|(id, p)| (id.clone(), p.raw.clone()))
        .collect();
    let flare = agent::media::parse_catalog(Company::OpenRouter, &catalog.raw, &answers)
        .into_iter()
        .find(|m| m.id == FLARE_ON_OPENROUTER)
        .unwrap();
    let request = |routing: Option<Routing>| MediaRequest {
        kind: MediaKind::Image,
        model: FLARE_ON_OPENROUTER.into(),
        prompt: "a red kite".into(),
        params: Map::new(),
        extra: object(json!({"moderation": "low"})),
        refs: Vec::new(),
        routing,
        provider_options: Default::default(),
    };
    // `spec::validate` refuses this first; the placing refuses it too, so it can never be
    // sent nowhere.
    let mut elsewhere = request(Some(Routing {
        order: vec!["azure".into()],
        only: true,
        allow_fallbacks: false,
        zdr: false,
    }));
    let problems = place_extra_or_refuse(&flare, &mut elsewhere).unwrap_err();
    let err = super::errors::invalid_settings(&problems, &flare);
    assert_eq!(err.code, "studio_bad_settings");
    assert_eq!(err.message, "Safety level is not offered by this model.");
    // With no routing every company that lists it gets it.
    let mut open = request(None);
    place_extra_or_refuse(&flare, &mut open).unwrap();
    assert!(open.extra.is_empty());
    assert_eq!(
        open.provider_options["openai"],
        object(json!({"moderation": "low"}))
    );
}

#[tokio::test]
async fn a_pinned_company_or_the_only_one_is_recorded_as_upstream() {
    let h = harness().await;
    // FLUX 3 has one company in its saved answer.
    let flux = h.manager.start(h.picture_req("x"), false).await.unwrap();
    assert_eq!(flux.upstream.as_deref(), Some("Black Forest Labs"));
    h.wait_status(&flux.id, "done").await;
    // Nano Banana has two: none is recorded unless the routing pins one. Its price is not
    // known, so each run reserves $5 until it ends: one ends before the next starts.
    let open = h
        .manager
        .start(
            h.req(h.openrouter, MediaKind::Image, NANO_BANANA, "a fox"),
            true,
        )
        .await
        .unwrap();
    assert_eq!(open.upstream, None);
    h.wait_status(&open.id, "done").await;
    let pinned = h
        .manager
        .start(
            StartRequest {
                routing: Some(Routing {
                    order: vec!["google-ai-studio".into()],
                    only: true,
                    allow_fallbacks: false,
                    zdr: false,
                }),
                ..h.req(h.openrouter, MediaKind::Image, NANO_BANANA, "a fox")
            },
            true,
        )
        .await
        .unwrap();
    assert_eq!(pinned.upstream.as_deref(), Some("Google AI Studio"));
    assert_eq!(
        pinned.routing.as_ref().map(|r| r.order.clone()),
        Some(vec!["google-ai-studio".to_owned()])
    );
}

#[tokio::test]
async fn a_model_the_saved_lists_do_not_have_is_refused() {
    let h = harness().await;
    let err = h
        .manager
        .start(
            h.req(h.openrouter, MediaKind::Image, "nobody/no-such-model", "x"),
            true,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_no_endpoint");
    let err = h
        .manager
        .start(h.req(9_999, MediaKind::Image, FLUX, "x"), true)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_no_endpoint");
    assert_eq!(h.provider.starts(), 0);
}

#[tokio::test]
async fn an_added_picture_no_longer_on_this_computer_is_refused() {
    let h = harness().await;
    // A draft that still names a picture which went with the last run that used it.
    let req = StartRequest {
        refs: vec![RefInput {
            sha256: "a".repeat(64),
            role: RefRole::Reference,
        }],
        ..h.picture_req("x")
    };
    let err = h.manager.start(req, false).await.unwrap_err();
    assert_eq!(err.code, "studio_ref_missing");
    assert_eq!(
        err.message,
        "One of the added pictures is no longer on this computer."
    );
    assert_eq!(err.next_step, "Add the picture again.");
    assert_eq!(h.provider.starts(), 0);
    assert!(h.store_runs().await.is_empty());
    assert!(h.audit_rows().await.is_empty());
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
}

// ---- the queue --------------------------------------------------------------------------------

#[tokio::test]
async fn two_pictures_run_at_once_and_the_third_waits() {
    let h = harness().await;
    assert_eq!(PICTURES_AT_ONCE, 2);
    h.provider.hold();
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(h.manager.start(h.picture_req("x"), false).await.unwrap().id);
    }
    wait_until("two pictures are sent", || h.provider.starts() == 2).await;
    let mut runs = Vec::new();
    for id in &ids {
        runs.push(h.run_dto(id).await);
    }
    let statuses: Vec<&str> = runs.iter().map(|r| r.status.as_str()).collect();
    assert_eq!(statuses, ["sending", "sending", "queued"]);
    assert_eq!(runs[2].queue_position, Some(1));
    assert_eq!(runs[0].queue_position, None);
    assert!(runs.iter().all(|r| r.watching));
    let live = h.manager.live().await.unwrap();
    assert_eq!(live.len(), 3);

    h.provider.release_all();
    for id in &ids {
        h.wait_status(id, "done").await;
    }
    assert_eq!(h.provider.starts(), 3);
    assert!(h.manager.live().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_cancelled_queued_run_is_never_sent() {
    let h = harness().await;
    h.provider.hold();
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(h.manager.start(h.picture_req("x"), false).await.unwrap().id);
    }
    wait_until("two pictures are sent", || h.provider.starts() == 2).await;
    let r = h.manager.cancel(&ids[2]).await.unwrap();
    assert_eq!(r.status, "cancelled");
    let error = r.error.unwrap();
    assert_eq!(error.code, "studio_stopped");
    assert_eq!(error.charged, Some(false));
    assert_eq!(error.company, "openrouter");

    h.provider.release_all();
    for id in &ids[..2] {
        h.wait_status(id, "done").await;
    }
    assert_eq!(h.provider.starts(), 2);
    let rows = h.audit_rows().await;
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter()
            .filter(|r| r.result_status.as_deref() == Some("cancelled"))
            .count(),
        1
    );
}

#[tokio::test]
async fn the_runs_behind_one_that_leaves_the_queue_are_told_their_new_places() {
    let h = harness().await;
    h.provider.hold();
    let mut ids = Vec::new();
    for _ in 0..5 {
        ids.push(h.manager.start(h.picture_req("x"), false).await.unwrap().id);
    }
    wait_until("two pictures are sent", || h.provider.starts() == 2).await;
    let place = |id: &str| {
        h.events_for(id)
            .last()
            .and_then(|event| event.queue_position)
    };
    assert_eq!(
        [place(&ids[2]), place(&ids[3]), place(&ids[4])],
        [Some(1), Some(2), Some(3)]
    );

    // The first one waiting is cancelled: the two behind it move up.
    h.manager.cancel(&ids[2]).await.unwrap();
    let last = |id: &str| h.events_for(id).last().cloned().unwrap();
    for (id, now) in [(&ids[3], 1), (&ids[4], 2)] {
        let event = last(id);
        assert_eq!(event.status, "queued");
        assert_eq!(event.queue_position, Some(now));
        assert!(event.watching);
    }

    // One being sent finishes: the first one waiting starts, and the last moves up.
    h.provider.release_one();
    wait_until("the next one waiting is sent", || h.provider.starts() == 3).await;
    wait_until("the last one is told its new place", || {
        place(&ids[4]) == Some(1)
    })
    .await;
    assert_eq!(last(&ids[4]).status, "queued");

    h.provider.release_all();
    for id in [&ids[0], &ids[1], &ids[3], &ids[4]] {
        h.wait_status(id, "done").await;
    }
    // A run that got its place is never told a place in the queue after it.
    for id in [&ids[3], &ids[4]] {
        let events = h.events_for(id);
        let sent = events.iter().position(|e| e.status == "sending").unwrap();
        assert!(events[sent..].iter().all(|e| e.queue_position.is_none()));
    }
}

// ---- cancel and failures ----------------------------------------------------------------------

#[tokio::test]
async fn a_cancel_that_comes_before_the_request_goes_out_sends_nothing() {
    let h = harness().await;
    let pause = h.sink.pause_at_sending();
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    wait_until("the run is recorded as sending", || {
        pause.reached.load(Ordering::SeqCst)
    })
    .await;
    // One poll of Cancel cancels the run's token while its task is still between recording
    // `sending` and sending the request; Cancel then waits for the task.
    let mut cancel = Box::pin(h.manager.cancel(&run.id));
    tokio::select! {
        biased;
        _ = &mut cancel => panic!("Cancel waits for the run's task"),
        () = std::future::ready(()) => {}
    }
    pause.go.store(true, Ordering::SeqCst);
    let r = cancel.await.unwrap();
    assert_eq!(r.status, "cancelled");
    assert_eq!(h.provider.starts(), 0);
    let error = r.error.unwrap();
    assert_eq!(error.code, "studio_stopped");
    assert_eq!(error.charged, Some(false));
    assert_eq!(error.message, "Stopped. Nothing was charged.");
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_video_stopped_while_sending_has_no_job_to_get_later() {
    let h = harness().await;
    h.provider.hold();
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    wait_until("the request is out", || h.provider.starts() == 1).await;
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled");
    assert_eq!(r.remote_id, None);
    let error = r.error.unwrap();
    assert_eq!(error.code, "studio_stopped");
    assert_eq!(error.charged, None);
    assert_eq!(
        error.message,
        "Stopped while sending; it may or may not have started."
    );
    assert!(
        !error.next_step.contains("Get it anyway"),
        "{}",
        error.next_step
    );
    // There is no job to check, so Get it anyway is refused and nothing is sent again.
    let refused = h.manager.fetch_anyway(&run.id).await.unwrap_err();
    assert_eq!(refused.code, "bad_request");
    h.provider.release_all();
    assert_eq!(h.provider.starts(), 1);
    assert_eq!(h.provider.checks(), 0);
}

#[tokio::test]
async fn a_run_whose_task_breaks_off_is_recorded_as_interrupted_and_frees_its_place() {
    let h = harness().await;
    assert_eq!(PICTURES_AT_ONCE, 2);
    let mut ids = Vec::new();
    for _ in 0..PICTURES_AT_ONCE {
        h.provider.answer(Answer::Panic);
        ids.push(h.manager.start(h.picture_req("x"), false).await.unwrap().id);
    }
    for id in &ids {
        let ended = h.wait_status(id, "interrupted").await;
        let error = ended.error.unwrap();
        assert_eq!(error.code, "studio_interrupted");
        assert_eq!(error.charged, None);
        assert!(ended.finished_at.is_some());
        let rows = h.audit_rows_for(id).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].result_status.as_deref(), Some("interrupted"));
        assert!(h.scratch_is_gone(id));
        assert_eq!(
            h.statuses(id).last().map(String::as_str),
            Some("interrupted")
        );
    }
    assert!(h.manager.live().await.unwrap().is_empty());
    // Both places are free again: the next run is sent and finishes.
    let next = h.manager.start(h.picture_req("y"), false).await.unwrap();
    h.wait_status(&next.id, "done").await;
    assert_eq!(h.provider.starts(), 3);
}

#[tokio::test]
async fn an_openai_picture_cancelled_while_sending_may_be_charged_and_counts() {
    let h = harness().await;
    h.provider.hold();
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    h.wait_status(&run.id, "sending").await;
    wait_until("the request is out", || h.provider.starts() == 1).await;
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled");
    assert_eq!(r.error.as_ref().unwrap().charged, None);
    assert_eq!(
        r.error.unwrap().message,
        "Stopped waiting. OpenAI may still charge for it."
    );
    assert_eq!(
        h.manager.usage().await.unwrap().spent_today_usd,
        UNKNOWN_PRICE_RESERVE_USD
    );
    assert_eq!(
        h.statuses(&run.id).last().map(String::as_str),
        Some("cancelled")
    );
}

#[tokio::test]
async fn an_openrouter_picture_cancelled_while_sending_is_not_charged() {
    let h = harness().await;
    h.provider.hold();
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "sending").await;
    wait_until("the request is out", || h.provider.starts() == 1).await;
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled");
    let error = r.error.unwrap();
    assert_eq!(error.charged, Some(false));
    assert_eq!(
        error.message,
        "Stopped. OpenRouter does not charge for a picture you did not receive."
    );
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_run_that_was_not_sent_counts_nothing() {
    let h = harness().await;
    h.provider.answer(Answer::Fail(MediaError::NotSent(network(
        "connection refused",
    ))));
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_not_sent");
    assert_eq!(error.charged, Some(false));
    assert_eq!(error.message, "Not sent. Nothing was charged.");
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("failed"));
    assert_eq!(row.result_summary.as_deref(), Some("cost not reported"));
}

#[tokio::test]
async fn a_delivered_file_that_is_not_a_picture_is_not_kept_but_counts() {
    let h = harness().await;
    h.provider.answer(Answer::NotAPicture {
        cost: Some(Cost {
            usd: 0.04,
            reported: true,
        }),
    });
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_not_media");
    // The company delivered it, so it charged for it.
    assert_eq!(error.charged, Some(true));
    assert_eq!(failed.cost_usd, Some(0.04));
    assert!(failed.outputs.is_empty());
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.04).abs() < 1e-9, "{spent}");
    // Nothing is left in the gallery or its scratch folder.
    assert!(!h.gallery_root.join("2026-11").exists());
    assert!(!h.gallery_root.join(".partial").join(&run.id).exists());
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("failed"));
    assert!(row.result_summary.unwrap().contains("$0.04"));
}

#[tokio::test]
async fn a_full_disk_while_the_answer_is_written_is_said_plainly() {
    let h = harness().await;
    h.provider
        .answer(Answer::Fail(MediaError::Local(LocalIo::DiskFull)));
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    let error = h.wait_status(&run.id, "failed").await.error.unwrap();
    assert_eq!(error.code, "studio_disk_full");
    assert_eq!(error.charged, None);
    assert_eq!(error.company, "openai");
}

#[tokio::test]
async fn a_company_refusal_says_what_the_company_said_and_counts_nothing() {
    let h = harness().await;
    h.provider
        .answer(Answer::Fail(MediaError::Answered(MediaFailure {
            kind: FailureKind::NoCredit,
            stage: None,
            charged: Some(false),
            company_code: None,
            request_id: Some("gen-1".into()),
            status: Some(402),
            message: "Insufficient credits".into(),
            retry_after: None,
        })));
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let error = h.wait_status(&run.id, "failed").await.error.unwrap();
    assert_eq!(error.code, "studio_no_credit");
    assert_eq!(error.message, "Your OpenRouter account has no credit left.");
    assert!(error.details.contains("Insufficient credits"));
    assert_eq!(error.charged, Some(false));
    assert_eq!(error.request_id.as_deref(), Some("gen-1"));
    assert_eq!(error.stage, None);
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
}

#[tokio::test]
async fn a_timeout_fails_the_run_and_it_is_never_sent_again() {
    let h = harness().await;
    h.provider
        .answer(Answer::Fail(MediaError::Unknown(network("timed out"))));
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_not_answered");
    assert_eq!(error.charged, None);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn an_openrouter_picture_that_timed_out_was_not_charged() {
    let h = harness().await;
    h.provider
        .answer(Answer::Fail(MediaError::Unknown(network("timed out"))));
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let error = h.wait_status(&run.id, "failed").await.error.unwrap();
    assert_eq!(error.code, "studio_not_answered");
    assert_eq!(error.charged, Some(false));
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
}

#[tokio::test]
async fn a_blocked_run_is_never_sent_again() {
    let h = harness().await;
    h.provider
        .answer(Answer::Fail(MediaError::Answered(MediaFailure {
            kind: FailureKind::Blocked,
            stage: Some(Stage::Prompt),
            charged: None,
            company_code: Some("moderation_blocked".into()),
            request_id: Some("req_8f2".into()),
            status: Some(400),
            message: "Your request was rejected by the safety system.".into(),
            retry_after: None,
        })));
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    let blocked = h.wait_status(&run.id, "blocked").await;
    let error = blocked.error.unwrap();
    assert_eq!(error.message, "OpenAI's safety filter refused the prompt.");
    assert_eq!(error.code, "studio_blocked");
    assert_eq!(error.request_id.as_deref(), Some("req_8f2"));
    assert_eq!(error.stage.as_deref(), Some("prompt"));
    assert_eq!(error.company, "openai");
    assert_eq!(error.charged, None);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.provider.starts(), 1);
}

// ---- the audit row ----------------------------------------------------------------------------

#[tokio::test]
async fn the_audit_row_has_the_prompt_length_not_the_prompt() {
    let h = harness().await;
    let run = h
        .manager
        .start(h.picture_req("lighthouse at dusk"), false)
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(
        (row.server.as_str(), row.tool.as_str()),
        ("studio", "picture")
    );
    assert_eq!(row.chat_id, None);
    assert_eq!(row.decision, AuditDecision::AllowedWithoutPrompt);
    assert_eq!(row.decided_by, "user");
    assert_eq!(row.risk, "normal");
    assert_eq!(row.arguments["prompt_chars"], 18);
    assert_eq!(row.arguments["endpoint"], "My OpenRouter");
    assert_eq!(row.arguments["host"], "openrouter.ai");
    assert_eq!(row.arguments["model"], FLUX);
    assert_eq!(row.arguments["changed"], json!({"resolution": "1K"}));
    assert_eq!(row.arguments["pictures_sent"], 0);
    assert_eq!(row.arguments["estimate_usd"], 0.048);
    assert_eq!(row.arguments["confirmed"], false);
    assert!(!row.arguments.to_string().contains("lighthouse"));
    assert!(!row.preview.contains("lighthouse"));
    assert_eq!(
        row.preview,
        "Made a picture with FLUX 3 Image through My OpenRouter"
    );
    assert_eq!(row.result_status.as_deref(), Some("done"));
    assert!(row.result_summary.unwrap().contains("$0.04"));
    let stored = h.store.get_creation_run(&run.id).await.unwrap().unwrap();
    assert_eq!(stored.audit_id, Some(row.id));
}

#[tokio::test]
async fn extra_field_values_are_not_audited() {
    let h = harness().await;
    let run = h
        .manager
        .start(
            StartRequest {
                extra: object(json!({"moderation": "low"})),
                ..h.req(
                    h.openrouter,
                    MediaKind::Image,
                    FLARE_ON_OPENROUTER,
                    "a red kite",
                )
            },
            true,
        )
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.arguments["extra_fields"], json!(["moderation"]));
    assert!(!row.arguments.to_string().contains("low"));
    assert_eq!(row.arguments["confirmed"], true);
    assert_eq!(row.arguments["estimate_usd"], Value::Null);
}

#[tokio::test]
async fn a_video_audit_row_says_it_started_a_video() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(
        row.preview,
        "Started a video with Veo 3.1 through My OpenRouter"
    );
}

// ---- refusals and where runs go ---------------------------------------------------------------

#[tokio::test]
async fn the_gallery_cap_is_said_plainly() {
    let h = harness().await;
    for i in 0..store::MAX_CREATION_RUNS {
        h.store
            .insert_creation_run(store::NewCreationRun {
                id: format!("bulk-{i}"),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Cloud,
                provider_id: None,
                provider_name: "p".into(),
                provider_host: "h".into(),
                model: "m".into(),
                model_label: "M".into(),
                prompt: String::new(),
                params: json!({}),
                refs: json!([]),
                estimate_usd: Some(0.0),
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at: 1,
            })
            .await
            .unwrap();
    }
    let err = h
        .manager
        .start(h.picture_req("x"), false)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_gallery_full");
    assert_eq!(h.provider.starts(), 0);
    assert!(h.audit_rows().await.is_empty());
}

#[tokio::test]
async fn a_run_on_this_computer_without_a_local_lane_is_refused() {
    let h = harness().await;
    let err = h
        .manager
        .start(
            StartRequest {
                target: RunTarget::ThisComputer,
                ..h.picture_req("x")
            },
            false,
        )
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_no_local_engine");
    assert!(h.store_runs().await.is_empty());
    let local = StartRequest {
        target: RunTarget::ThisComputer,
        ..h.picture_req("x")
    };
    assert_eq!(
        h.manager.estimate(&local).await.unwrap_err().code,
        "studio_no_local_engine"
    );
    assert_eq!(
        h.manager.preview(&local).await.unwrap_err().code,
        "studio_no_local_engine"
    );
}

#[tokio::test]
async fn cloud_runs_say_they_ran_in_the_cloud() {
    let h = harness().await;
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    assert_eq!(run.runs_on, "cloud");
    assert!(run.eta.is_none() && run.local.is_none());
    assert_eq!(run.kind, MediaKind::Image);
    assert_eq!(run.provider_id, Some(h.openrouter));
    assert_eq!(run.provider_name, "My OpenRouter");
    assert_eq!(run.provider_host, "openrouter.ai");
    assert_eq!(run.model_label, "FLUX 3 Image");
    assert_eq!(run.params, object(json!({"resolution": "1K"})));
}

// ---- estimate, preview, usage -----------------------------------------------------------------

#[tokio::test]
async fn an_estimate_sends_nothing_and_says_what_a_run_would_meet() {
    let h = harness().await;
    let e = h.manager.estimate(&h.picture_req("x")).await.unwrap();
    assert_eq!(e.usd, Some(0.048));
    assert!(e.reasons.is_empty());
    assert_eq!(e.confirm_at_usd, 1.0);
    assert!(e.invalid.is_empty());
    assert_eq!(e.limit.limit_usd, Some(10.0));
    assert!(e.limit.would_pass);
    assert!(e.eta.is_none() && e.local.is_none());

    let video = h.manager.estimate(&h.video_req()).await.unwrap();
    assert_eq!(
        video.reasons,
        [ConfirmReason::Video, ConfirmReason::AtLeast]
    );

    h.seed_done_run_costing(9.50).await;
    let over = h
        .manager
        .estimate(&h.video_req_estimated(0.96))
        .await
        .unwrap();
    assert!(!over.limit.would_pass);
    assert_eq!(over.limit.spent_today_usd, 9.5);

    let bad = h
        .manager
        .estimate(&StartRequest {
            params: object(json!({"seed": 1})),
            ..h.req(h.openrouter, MediaKind::Image, NANO_BANANA, "")
        })
        .await
        .unwrap();
    assert_eq!(
        bad.invalid,
        [
            "Write a prompt first.",
            "Seed is not offered by this model."
        ]
    );
    assert_eq!(h.provider.starts(), 0);
    assert!(
        h.store_runs()
            .await
            .iter()
            .all(|r| r.prompt == "an earlier picture")
    );
}

#[tokio::test]
async fn a_preview_shows_the_request_with_its_extra_settings_placed() {
    let h = harness().await;
    let preview = h
        .manager
        .preview(&StartRequest {
            extra: object(json!({"moderation": "low"})),
            ..h.req(
                h.openrouter,
                MediaKind::Image,
                FLARE_ON_OPENROUTER,
                "a red kite",
            )
        })
        .await
        .unwrap();
    assert_eq!(preview.host, "openrouter.ai");
    assert_eq!(
        preview.body["options"]["openai"],
        json!({"moderation": "low"})
    );
    assert_eq!(preview.body["extra"], json!({}));
    assert_eq!(h.provider.starts(), 0);
}

#[tokio::test]
async fn usage_says_where_the_gallery_is_and_what_it_holds() {
    let h = harness().await;
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let u = h.manager.usage().await.unwrap();
    assert_eq!(u.folder, h.gallery_root.display().to_string());
    assert_eq!(u.pictures, 2);
    assert_eq!(u.videos, 0);
    assert!(u.bytes > 0);
    assert!((u.spent_today_usd - 0.04).abs() < 1e-9);
    assert_eq!(u.pending_usd, 0.0);
}

#[tokio::test]
async fn a_finished_picture_gets_its_thumbnail_and_its_sidecar() {
    let h = harness_with(Some(crate::attachments::tests::test_worker("thumb"))).await;
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert!(done.outputs.iter().all(|o| o.has_thumb));
    let thumb = h
        .gallery_root
        .join(".thumbs")
        .join(format!("{}.jpg", done.outputs[0].id));
    assert!(
        std::fs::read(&thumb)
            .unwrap()
            .starts_with(&[0xFF, 0xD8, 0xFF])
    );
    let sidecar = h
        .gallery_root
        .join("2026-11")
        .join(format!("{}.json", run.id));
    let record: Value = serde_json::from_slice(&std::fs::read(sidecar).unwrap()).unwrap();
    assert_eq!(record["status"], "done");
    assert_eq!(record["outputs"].as_array().unwrap().len(), 2);
}

// ---- what each run counts ---------------------------------------------------------------------

#[tokio::test]
async fn spending_follows_the_status_and_the_charge() {
    let h = harness().await;
    let run = h.seed_done_run_costing(0.5).await;
    assert_eq!(spend_row(&run).charge, Charge::Final);
    let with = |status: store::CreationStatus, error: Option<Value>| store::CreationRun {
        status,
        error,
        ..run.clone()
    };
    assert_eq!(
        spend_row(&with(store::CreationStatus::Sending, None)).charge,
        Charge::Pending
    );
    assert_eq!(
        spend_row(&with(
            store::CreationStatus::Failed,
            Some(json!({"charged": false}))
        ))
        .charge,
        Charge::None
    );
    assert_eq!(
        spend_row(&with(
            store::CreationStatus::Cancelled,
            Some(json!({"charged": null}))
        ))
        .charge,
        Charge::Maybe
    );
    assert_eq!(
        spend_row(&with(
            store::CreationStatus::Blocked,
            Some(json!({"charged": true}))
        ))
        .charge,
        Charge::Final
    );
    // An error without the field counts as unknown.
    assert_eq!(
        spend_row(&with(store::CreationStatus::Interrupted, Some(json!({})))).charge,
        Charge::Maybe
    );
    let row = spend_row(&run);
    assert_eq!((row.estimate_usd, row.cost_usd), (Some(0.5), Some(0.5)));
    assert_eq!(row.created_at, run.created_at);
}

/// Every refusal the start makes is a four-part error.
#[tokio::test]
async fn refusals_have_four_parts() {
    let h = harness().await;
    let refusals: Vec<AppError> = vec![
        h.manager.start(h.video_req(), false).await.unwrap_err(),
        h.manager
            .start(
                StartRequest {
                    kind: MediaKind::Image,
                    ..h.video_req()
                },
                true,
            )
            .await
            .unwrap_err(),
    ];
    for e in refusals {
        assert!(!e.code.is_empty() && !e.message.is_empty() && !e.next_step.is_empty());
    }
}

// ---- video jobs (Task A14) --------------------------------------------------------------------

fn secs(n: u64) -> Duration {
    Duration::from_secs(n)
}

/// A failure the company answered, of `kind`.
fn answered(kind: FailureKind) -> MediaError {
    MediaError::Answered(MediaFailure {
        kind,
        stage: None,
        charged: None,
        company_code: None,
        request_id: None,
        status: Some(404),
        message: "Not found".into(),
        retry_after: None,
    })
}

#[test]
fn checks_come_every_5_then_10_then_20_seconds() {
    let p = PollPlan::default();
    assert_eq!(poll_delay(&p, secs(0)), secs(5));
    assert_eq!(poll_delay(&p, secs(59)), secs(5));
    assert_eq!(poll_delay(&p, secs(60)), secs(10));
    assert_eq!(poll_delay(&p, secs(299)), secs(10));
    assert_eq!(poll_delay(&p, secs(300)), secs(20));
    assert_eq!(poll_delay(&p, secs(3_600)), secs(20));
    assert_eq!(p.watch_limit, secs(30 * 60));
}

#[tokio::test]
async fn the_remote_id_is_saved_before_the_first_check() {
    let h = harness().await;
    h.provider.answer_check(Ok(done_job()));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let checked = h.provider.checked.lock().unwrap().clone();
    assert_eq!(checked[0].remote_id, "vid_1");
    assert_eq!(
        checked[0].holder,
        Some(("waiting".to_owned(), Some("vid_1".to_owned())))
    );
}

#[tokio::test]
async fn a_finished_video_is_downloaded_into_the_gallery() {
    let h = harness().await;
    h.provider.answer_check(Ok(JobState::Pending));
    h.provider.answer_check(Ok(done_job()));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    let output = &done.outputs[0];
    assert_eq!(output.mime, "video/mp4");
    assert!(Path::new(&output.path).starts_with(h.gallery_root.join("2026-11")));
    assert!(output.path.ends_with(".mp4"));
    assert!(Path::new(&output.path).is_file());
    assert_eq!(output.duration_ms, Some(4_000));
    assert_eq!(output.seed, done.params.get("seed").and_then(Value::as_u64));
    assert!(!output.has_thumb);
    assert_eq!(done.cost_usd, Some(0.36));
    assert_eq!(done.remote_id.as_deref(), Some("vid_1"));
    assert!(done.error.is_none() && done.finished_at.is_some());
    assert_eq!(
        h.statuses(&run.id),
        ["queued", "sending", "waiting", "downloading", "done"]
    );
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("done"));
    assert!(row.result_summary.unwrap().contains("$0.36"));
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.36).abs() < 1e-9, "{spent}");
    wait_until("the scratch folder goes", || h.scratch_is_gone(&run.id)).await;
}

#[tokio::test]
async fn a_network_error_while_checking_keeps_waiting() {
    let h = harness().await;
    h.provider
        .answer_check(Err(MediaError::Unknown(network("timed out"))));
    h.provider
        .answer_check(Err(MediaError::Unknown(network("connection reset"))));
    h.provider.answer_check(Ok(done_job()));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.provider.checks(), 3);
    assert_eq!(h.provider.starts(), 1);
    assert_eq!(
        h.statuses(&run.id),
        ["queued", "sending", "waiting", "downloading", "done"]
    );
}

#[tokio::test]
async fn resume_after_restart_finishes_the_video_without_a_second_create() {
    let h = harness().await;
    let first = &h.manager;
    let run = first.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    first.shutdown().await; // its tasks are gone, not merely dropped
    assert_eq!(
        h.store_run(&run.id).await.status,
        store::CreationStatus::Waiting
    );
    let checks = h.provider.checks();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        h.provider.checks(),
        checks,
        "the first manager still checks"
    );

    let second = h.manager_with(&h.provider2);
    second.resume_interrupted().await;
    let done = h.wait_status_in(&second, &run.id, "done").await;
    assert_eq!(h.provider.starts() + h.provider2.starts(), 1);
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.provider.downloads() + h.provider2.downloads(), 1);
    assert_eq!(h.provider2.downloads(), 1);
    assert_eq!(h.provider2.checked.lock().unwrap()[0].remote_id, "vid_1");
}

#[tokio::test]
async fn a_run_left_sending_becomes_interrupted_and_is_never_sent_again() {
    let h = harness().await;
    let id = h.seed_video_run(store::CreationStatus::Sending, None).await;
    h.manager.resume_interrupted().await;
    let run = h.run_dto(&id).await;
    assert_eq!(run.status, "interrupted");
    let error = run.error.unwrap();
    assert_eq!(error.code, "studio_interrupted");
    assert!(error.message.contains("may or may not have started"));
    assert_eq!(error.charged, None);
    assert_eq!(error.company, "openrouter");
    assert!(run.finished_at.is_some());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.provider.starts(), 0);
    assert_eq!(h.provider.checks(), 0);
    let rows = h.audit_rows_for(&id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].result_status.as_deref(), Some("interrupted"));
    assert_eq!(
        h.statuses(&id).last().map(String::as_str),
        Some("interrupted")
    );
}

#[tokio::test]
async fn a_queued_run_is_not_sent_after_a_restart() {
    let h = harness().await;
    let id = h.seed_video_run(store::CreationStatus::Queued, None).await;
    h.manager.resume_interrupted().await;
    let run = h.run_dto(&id).await;
    assert_eq!(run.status, "cancelled");
    let error = run.error.unwrap();
    assert_eq!(
        error.message,
        "Not sent: the app closed before its turn. Nothing was charged."
    );
    assert_eq!(error.charged, Some(false));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.provider.starts(), 0);
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let rows = h.audit_rows_for(&id).await;
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_download_left_half_done_starts_over_from_the_result() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_7"))
        .await;
    // What the closed app had written of the file is still there.
    let left = h.gallery_root.join(".partial").join(&id);
    std::fs::create_dir_all(&left).unwrap();
    std::fs::write(left.join("half"), b"half a video").unwrap();
    h.provider.answer_check(Ok(done_job()));
    h.manager.resume_interrupted().await;
    let done = h.wait_status(&id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(done.outputs[0].seed, Some(7));
    assert_eq!(h.provider.starts(), 0);
    assert_eq!(h.provider.checks(), 1);
    assert_eq!(h.provider.checked.lock().unwrap()[0].remote_id, "vid_7");
    assert_eq!(h.provider.downloads(), 1);
    wait_until("the half-written file goes", || h.scratch_is_gone(&id)).await;
}

#[tokio::test]
async fn a_result_kept_before_a_restart_is_not_downloaded_again() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_8"))
        .await;
    let created_at = h.store_run(&id).await.created_at;
    let first = h.manager.gallery().output_path(created_at, &id, 0, "mp4");
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    std::fs::write(&first, mp4()).unwrap();
    h.store
        .insert_creation_output(store::NewCreationOutput {
            id: "kept-0".into(),
            run_id: id.clone(),
            idx: 0,
            path: first.display().to_string(),
            thumb_path: None,
            mime: "video/mp4".into(),
            size_bytes: mp4().len() as u64,
            width: None,
            height: None,
            duration_ms: None,
            seed: Some(7),
            created_at,
        })
        .await
        .unwrap();
    h.provider.answer_check(Ok(JobState::Done(JobDone {
        outputs: 2,
        signed: Vec::new(),
        cost: None,
    })));
    h.manager.resume_interrupted().await;
    let done = h.wait_status(&id, "done").await;
    assert_eq!(h.provider.downloads(), 1);
    let numbers: Vec<u32> = done.outputs.iter().map(|out| out.idx).collect();
    assert_eq!(numbers, [0, 1]);
    assert_eq!(done.outputs[0].id, "kept-0");
}

#[tokio::test]
async fn stop_watching_keeps_the_remote_id_and_get_it_anyway_downloads() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    let stopped = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    assert_eq!(stopped.remote_id.as_deref(), Some("vid_1"));
    assert!(!stopped.watching);
    let error = stopped.error.unwrap();
    assert_eq!(error.code, "studio_stopped");
    assert_eq!(error.charged, None);
    assert_eq!(
        error.message,
        "OpenRouter may still finish and charge for this."
    );

    h.provider.check_from_now(Ok(done_job()));
    let back = h.manager.fetch_anyway(&run.id).await.unwrap();
    assert_eq!(back.status, "waiting");
    assert!(back.watching && back.error.is_none() && back.finished_at.is_none());
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(done.remote_id.as_deref(), Some("vid_1"));
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn stop_watching_then_get_it_anyway_records_done_with_the_cost() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    h.manager.cancel(&run.id).await.unwrap();
    h.provider.check_from_now(Ok(done_job()));
    h.manager.fetch_anyway(&run.id).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.cost_usd, Some(0.36));
    let rows = h.audit_rows_for(&run.id).await; // oldest first
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
    assert_eq!(rows[1].parent_audit_id, Some(rows[0].id));
    assert_eq!(rows[1].result_status.as_deref(), Some("done"));
    assert!(rows[1].result_summary.as_deref().unwrap().contains("$0.36"));
    assert_eq!(rows[1].tool, "video");
    assert_eq!(rows[1].arguments, rows[0].arguments);
    assert_eq!(
        rows[1].preview,
        "Got the stopped video with Veo 3.1 through My OpenRouter"
    );
    assert_eq!(rows[1].decision, AuditDecision::AllowedWithoutPrompt);
}

#[tokio::test]
async fn a_second_get_it_anyway_copies_the_first_rows_arguments() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    h.manager.cancel(&run.id).await.unwrap();
    h.manager.fetch_anyway(&run.id).await.unwrap();
    h.wait_for(&run.id, "watched again", |r| r.watching).await;
    h.manager.cancel(&run.id).await.unwrap();
    h.provider.check_from_now(Ok(done_job()));
    h.manager.fetch_anyway(&run.id).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let rows = h.audit_rows_for(&run.id).await; // oldest first
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].parent_audit_id, Some(rows[1].id));
    assert_eq!(rows[2].tool, "video");
    assert_eq!(rows[2].arguments, rows[0].arguments);
    assert_eq!(rows[2].result_status.as_deref(), Some("done"));
}

#[tokio::test]
async fn after_30_minutes_it_says_still_not_ready() {
    let h = harness_polling(PollPlan {
        watch_limit: Duration::from_millis(50),
        ..fast_poll()
    })
    .await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    wait_until("a still-not-ready event", || {
        h.events_for(&run.id)
            .last()
            .is_some_and(|event| event.stale)
    })
    .await;
    let stale = h.run_dto(&run.id).await;
    assert!(stale.stale);
    assert_eq!(stale.status, "waiting");
    assert!(!stale.watching);
    assert_eq!(stale.remote_id.as_deref(), Some("vid_1"));
    // It is no longer checked, and its audit row stays open.
    let checks = h.provider.checks();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.provider.checks(), checks);
    assert_eq!(h.audit_rows_for(&run.id).await[0].result_status, None);

    // Check again.
    h.provider.check_from_now(Ok(done_job()));
    let again = h.manager.fetch_anyway(&run.id).await.unwrap();
    assert!(again.watching && !again.stale);
    h.wait_status(&run.id, "done").await;
    let rows = h.audit_rows_for(&run.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].result_status.as_deref(), Some("done"));
    assert!(rows[0].result_summary.as_deref().unwrap().contains("$0.36"));
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn a_video_still_not_ready_can_be_stopped() {
    let h = harness_polling(PollPlan {
        watch_limit: Duration::from_millis(50),
        ..fast_poll()
    })
    .await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_for(&run.id, "still not ready", |r| r.stale).await;
    let stopped = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    assert!(!stopped.stale && !stopped.watching);
    assert_eq!(stopped.error.unwrap().charged, None);
    assert_eq!(stopped.remote_id.as_deref(), Some("vid_1"));
    let rows = h.audit_rows_for(&run.id).await;
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
}

#[tokio::test]
async fn a_job_the_company_no_longer_has_says_so() {
    let h = harness().await;
    h.provider.answer_check(Ok(JobState::Gone));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_job_lost");
    assert_eq!(error.message, "OpenRouter no longer has this job.");
    assert_eq!(failed.remote_id.as_deref(), Some("vid_1"));
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.result_status.as_deref(), Some("failed"));
}

#[tokio::test]
async fn a_job_that_failed_at_the_company_says_why() {
    let h = harness().await;
    h.provider.answer_check(Ok(JobState::Failed(MediaFailure {
        kind: FailureKind::Blocked,
        stage: Some(Stage::Output),
        charged: Some(false),
        company_code: Some("content_policy".into()),
        request_id: Some("gen-9".into()),
        status: None,
        message: "The output was flagged".into(),
        retry_after: None,
    })));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let blocked = h.wait_status(&run.id, "blocked").await;
    let error = blocked.error.unwrap();
    assert_eq!(error.code, "studio_blocked");
    assert_eq!(error.charged, Some(false));
    assert_eq!(error.request_id.as_deref(), Some("gen-9"));
    assert_eq!(error.stage.as_deref(), Some("output"));
    assert_eq!(h.provider.downloads(), 0);
}

#[tokio::test]
async fn an_expired_result_says_so_and_keeps_the_remote_id() {
    let h = harness().await;
    h.provider.answer_check(Ok(done_job()));
    h.provider
        .answer_download(Download::Fail(answered(FailureKind::Expired)));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_result_expired");
    assert_eq!(error.charged, Some(true));
    assert_eq!(failed.remote_id.as_deref(), Some("vid_1"));
    assert_eq!(failed.cost_usd, Some(0.36));
    // Counted at the job's cost, not at its $1.60 estimate.
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.36).abs() < 1e-9, "{spent}");
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn a_full_disk_keeps_the_remote_id_for_get_it_anyway() {
    let h = harness().await;
    h.provider.check_from_now(Ok(done_job()));
    h.provider
        .answer_download(Download::Fail(MediaError::Local(LocalIo::DiskFull)));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_disk_full");
    assert_eq!(error.charged, Some(true));
    assert_eq!(failed.remote_id.as_deref(), Some("vid_1"));
    assert_eq!(failed.cost_usd, Some(0.36));
    wait_until(".partial is empty", || h.scratch_is_gone(&run.id)).await;

    h.manager.fetch_anyway(&run.id).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.provider.starts(), 1);
    assert_eq!(h.provider.downloads(), 2);
    let rows = h.audit_rows_for(&run.id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].result_status.as_deref(), Some("failed"));
    assert_eq!(rows[1].parent_audit_id, Some(rows[0].id));
    assert_eq!(rows[1].result_status.as_deref(), Some("done"));
}

#[tokio::test]
async fn a_video_over_its_cap_is_refused() {
    let h = harness().await;
    h.provider.answer_check(Ok(done_job()));
    h.provider
        .answer_download(Download::Fail(MediaError::Local(LocalIo::TooLarge(1024))));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_too_large");
    assert_eq!(
        error.message,
        "The result is larger than 1 GB, so it was not kept."
    );
    assert_eq!(error.charged, Some(true));
    assert!(failed.outputs.is_empty());
    wait_until("the partial file goes", || h.scratch_is_gone(&run.id)).await;
}

#[tokio::test]
async fn a_download_that_stops_can_be_fetched_again() {
    let h = harness().await;
    h.provider.check_from_now(Ok(done_job()));
    h.provider
        .answer_download(Download::Fail(MediaError::Unknown(network(
            "connection reset",
        ))));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_download_failed");
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.next_step, "Press Get it anyway to try again.");
    h.manager.fetch_anyway(&run.id).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn a_foreign_result_link_goes_through_the_guarded_fetcher() {
    let h = harness().await;
    let link = Url::parse("https://files.example.com/v/1.mp4?sig=abc").unwrap();
    h.provider.answer_check(Ok(done_job()));
    h.provider
        .answer_download(Download::Elsewhere(link.clone()));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    let seen = h.fetch.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1);
    let (url, cap, dest) = &seen[0];
    assert_eq!(url, &link);
    assert_eq!(*cap, MediaLimits::default().video_cap);
    assert!(dest.starts_with(h.gallery_root.join(".partial").join(&run.id)));
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(done.outputs[0].mime, "video/mp4");
    assert!(Path::new(&done.outputs[0].path).is_file());
}

#[tokio::test]
async fn a_refused_result_link_is_said_in_studios_words() {
    let h = harness().await;
    *h.fetch.fail.lock().unwrap() = Some(FetchError::Blocked {
        host: "files.example.com".into(),
        ip: "10.0.0.7".parse().unwrap(),
        class: AddressClass::Private,
    });
    h.provider.check_from_now(Ok(done_job()));
    h.provider.answer_download(Download::Elsewhere(
        Url::parse("https://files.example.com/v/1.mp4").unwrap(),
    ));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_download_failed");
    assert_eq!(error.charged, Some(true));
    for text in [&error.message, &error.next_step, &error.details] {
        assert!(
            !text.contains("10.0.0.7") && !text.contains("example.com") && !text.contains("page"),
            "{text}"
        );
    }
}

#[tokio::test]
async fn progress_is_sent_at_most_four_times_a_second() {
    let h = harness().await;
    for i in 0..100u8 {
        h.provider.answer_check(Ok(JobState::Running {
            progress: Some(f32::from(i) / 100.0),
        }));
    }
    h.provider.answer_check(Ok(done_job()));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let times: Vec<Instant> = h
        .provider
        .checked
        .lock()
        .unwrap()
        .iter()
        .map(|check| check.at)
        .collect();
    assert_eq!(times.len(), 101);
    let span = times[100].duration_since(times[0]);
    let progress_only = h
        .events_for(&run.id)
        .iter()
        .filter(|event| event.status == "waiting" && event.progress.is_some())
        .count();
    assert!(progress_only >= 1, "no progress was sent");
    // At most one every 250 ms: four in any second.
    let most = 1 + span.as_millis() / 250;
    assert!(
        progress_only as u128 <= most,
        "{progress_only} progress events in {span:?}"
    );
    // Every status change is still sent.
    assert_eq!(
        h.statuses(&run.id),
        ["queued", "sending", "waiting", "downloading", "done"]
    );
}

#[tokio::test]
async fn the_audit_row_is_completed_after_a_restart() {
    let h = harness().await;
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&run.id, "waiting").await;
    h.manager.shutdown().await;
    assert_eq!(h.audit_rows_for(&run.id).await[0].result_status, None);
    let second = h.manager_with(&h.provider2);
    second.resume_interrupted().await;
    h.wait_status_in(&second, &run.id, "done").await;
    let rows = h.audit_rows_for(&run.id).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].result_status.as_deref(), Some("done"));
    assert!(rows[0].result_summary.as_deref().unwrap().contains("$0.36"));
}

#[tokio::test]
async fn the_third_video_waits_for_a_slot() {
    assert_eq!(VIDEOS_AT_ONCE, 2);
    let h = harness_polling(PollPlan {
        watch_limit: secs(1),
        ..fast_poll()
    })
    .await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(h.manager.start(h.video_req(), true).await.unwrap().id);
    }
    for id in &ids[..2] {
        h.wait_status(id, "waiting").await;
    }
    let third = h.run_dto(&ids[2]).await;
    assert_eq!(third.status, "queued");
    assert_eq!(third.queue_position, Some(1));
    assert_eq!(h.provider.starts(), 2);
    // A video still not ready after its watch limit frees its place.
    wait_until("the third video is sent", || h.provider.starts() == 3).await;
    assert!(h.run_dto(&ids[0]).await.stale || h.run_dto(&ids[1]).await.stale);
    h.wait_status(&ids[2], "waiting").await;
}

#[tokio::test]
async fn yesterdays_spending_stubs_go_at_start() {
    let h = harness().await;
    let yesterday = h.seed_stub(h.now - 86_400_000).await;
    let today = h.seed_stub(h.now - 60_000).await;
    let mut both = vec![yesterday, today.clone()];
    both.sort();
    assert_eq!(h.stub_ids().await, both);
    h.manager.resume_interrupted().await;
    assert_eq!(h.stub_ids().await, [today]);
}

#[tokio::test]
async fn get_it_anyway_never_runs_a_run_on_this_computer_or_one_without_a_job() {
    let h = harness().await;
    // A picture has no job to check.
    let picture = h.seed_done_run_costing(0.05).await;
    h.store
        .update_creation_run(
            &picture.id,
            store::CreationPatch {
                status: Some(store::CreationStatus::Failed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let err = h.manager.fetch_anyway(&picture.id).await.unwrap_err();
    assert_eq!(err.code, "bad_request");
    assert_eq!(err.next_step, "Make it again with Again.");

    // A local run is never checked with a company.
    h.store
        .insert_creation_run(store::NewCreationRun {
            id: "local-run".into(),
            kind: store::CreationKind::Video,
            runs_on: store::RunsOn::Local,
            provider_id: None,
            provider_name: "This computer".into(),
            provider_host: String::new(),
            model: "wan".into(),
            model_label: "Wan".into(),
            prompt: "x".into(),
            params: json!({}),
            refs: json!([]),
            estimate_usd: Some(0.0),
            gen_model_id: None,
            engine_build: None,
            device: None,
            created_at: h.now,
        })
        .await
        .unwrap();
    h.store
        .update_creation_run(
            "local-run",
            store::CreationPatch {
                status: Some(store::CreationStatus::Failed),
                remote_id: Some(Some("local-1".into())),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let err = h.manager.fetch_anyway("local-run").await.unwrap_err();
    assert_eq!(err.code, "bad_request");
    assert_eq!(err.next_step, "Make it again with Again.");
    assert_eq!(h.provider.checks(), 0);
    assert_eq!(
        h.manager
            .fetch_anyway("no-such-run")
            .await
            .unwrap_err()
            .code,
        "not_found"
    );
}

#[tokio::test]
async fn a_restart_with_the_key_gone_keeps_the_job_for_later() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Waiting, Some("vid_3"))
        .await;
    // The endpoint is no longer offered: the job cannot be checked now.
    h.manager_without_endpoints().resume_interrupted().await;
    let run = h.run_dto(&id).await;
    assert_eq!(run.status, "failed");
    assert_eq!(run.remote_id.as_deref(), Some("vid_3"));
    let error = run.error.unwrap();
    assert_eq!(error.code, "studio_no_endpoint");
    assert_eq!(error.charged, None);
    assert_eq!(h.provider2.checks(), 0);
}

// ---- a job known to have finished stays charged -----------------------------------------------

/// What Stop says of a video whose job finished.
const STOPPED_CHARGED: &str = "Stopped. OpenRouter finished this video, so it was charged.";

#[tokio::test]
async fn a_restart_mid_download_with_the_key_gone_says_it_was_charged() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_4"))
        .await;
    h.manager_without_endpoints().resume_interrupted().await;
    let run = h.run_dto(&id).await;
    assert_eq!(run.status, "failed");
    assert_eq!(run.remote_id.as_deref(), Some("vid_4"));
    let error = run.error.unwrap();
    assert_eq!(error.code, "studio_no_endpoint");
    assert_eq!(error.charged, Some(true));
    assert_eq!(h.provider2.checks(), 0);
}

#[tokio::test]
async fn a_restart_mid_download_then_stop_says_it_was_charged() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_7"))
        .await;
    // Its checks after the restart do not say done yet.
    h.manager.resume_interrupted().await;
    wait_until("the job is checked again", || h.provider.checks() >= 1).await;
    let stopped = h.manager.cancel(&id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    assert_eq!(stopped.remote_id.as_deref(), Some("vid_7"));
    let error = stopped.error.unwrap();
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.message, STOPPED_CHARGED);
    assert_eq!(h.provider.starts(), 0);
    assert_eq!(h.provider.downloads(), 0);
}

#[tokio::test]
async fn a_resumed_download_stopped_in_its_queue_says_it_was_charged() {
    let h = harness().await;
    // Two videos fill the endpoint's places.
    for _ in 0..VIDEOS_AT_ONCE {
        let run = h.manager.start(h.video_req(), true).await.unwrap();
        h.wait_status(&run.id, "waiting").await;
    }
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_5"))
        .await;
    h.manager.resume_interrupted().await;
    assert_eq!(h.run_dto(&id).await.queue_position, Some(1));
    let stopped = h.manager.cancel(&id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    let error = stopped.error.unwrap();
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.message, STOPPED_CHARGED);
    let checked = h.provider.checked.lock().unwrap().clone();
    assert!(checked.iter().all(|check| check.remote_id != "vid_5"));
}

#[tokio::test]
async fn a_resumed_download_still_not_ready_then_stopped_says_it_was_charged() {
    let h = harness_polling(PollPlan {
        watch_limit: Duration::from_millis(50),
        ..fast_poll()
    })
    .await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_8"))
        .await;
    h.manager.resume_interrupted().await;
    h.wait_for(&id, "still not ready", |r| r.stale).await;
    let stopped = h.manager.cancel(&id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    let error = stopped.error.unwrap();
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.message, STOPPED_CHARGED);
}

#[tokio::test]
async fn a_finished_job_the_company_no_longer_has_was_charged() {
    let h = harness().await;
    let id = h
        .seed_video_run(store::CreationStatus::Downloading, Some("vid_6"))
        .await;
    h.provider.answer_check(Ok(JobState::Gone));
    h.manager.resume_interrupted().await;
    let failed = h.wait_status(&id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(error.code, "studio_job_lost");
    assert_eq!(error.charged, Some(true));
}

/// A video whose job finished ($0.36) but whose download hit a full disk, then *Get it
/// anyway*: its checks from then on do not say done yet.
async fn get_it_anyway_after_a_full_disk(h: &H) -> String {
    h.provider.answer_check(Ok(done_job()));
    h.provider
        .answer_download(Download::Fail(MediaError::Local(LocalIo::DiskFull)));
    let run = h.manager.start(h.video_req(), true).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    assert_eq!(failed.error.unwrap().charged, Some(true));
    let back = h.manager.fetch_anyway(&run.id).await.unwrap();
    assert_eq!(back.status, "waiting");
    run.id
}

#[tokio::test]
async fn stopping_get_it_anyway_after_a_full_disk_keeps_it_charged() {
    let h = harness().await;
    let id = get_it_anyway_after_a_full_disk(&h).await;
    wait_until("the job is checked again", || h.provider.checks() >= 2).await;
    let stopped = h.manager.cancel(&id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    assert_eq!(stopped.cost_usd, Some(0.36));
    let error = stopped.error.unwrap();
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.message, STOPPED_CHARGED);
    let rows = h.audit_rows_for(&id).await;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].result_status.as_deref(), Some("cancelled"));
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn a_restart_during_get_it_anyway_keeps_it_charged() {
    let h = harness().await;
    let id = get_it_anyway_after_a_full_disk(&h).await;
    h.manager.shutdown().await;
    // Back to `waiting` with no error: only its recorded cost says the job finished.
    let left = h.store_run(&id).await;
    assert_eq!(left.status, store::CreationStatus::Waiting);
    assert!(left.error.is_none());
    assert_eq!(left.cost_usd, Some(0.36));
    let checks = h.provider.checks();
    let second = h.manager_with(&h.provider);
    second.resume_interrupted().await;
    wait_until("the job is checked again", || h.provider.checks() > checks).await;
    let stopped = second.cancel(&id).await.unwrap();
    assert_eq!(stopped.status, "cancelled");
    let error = stopped.error.unwrap();
    assert_eq!(error.charged, Some(true));
    assert_eq!(error.message, STOPPED_CHARGED);
    assert_eq!(h.provider.starts(), 1);
}

#[tokio::test]
async fn shutdown_writes_nothing_and_leaves_waiting_runs_queued() {
    let h = harness().await;
    let video = h.manager.start(h.video_req(), true).await.unwrap();
    h.wait_status(&video.id, "waiting").await;
    h.provider.hold();
    let sending = h.manager.start(h.picture_req("x"), false).await.unwrap();
    h.wait_status(&sending.id, "sending").await;
    wait_until("the picture is out", || h.provider.starts() == 2).await;
    h.manager.shutdown().await;
    h.provider.release_all();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.store_run(&sending.id).await.status,
        store::CreationStatus::Sending
    );
    assert_eq!(
        h.store_run(&video.id).await.status,
        store::CreationStatus::Waiting
    );
    // A run started after the app began to close waits for the next start.
    let late = h.manager.start(h.picture_req("y"), false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        h.store_run(&late.id).await.status,
        store::CreationStatus::Queued
    );
    assert_eq!(h.provider.starts(), 2);
}

// ---- deleting results and changing the preferences --------------------------------------------

fn output_ids(run: &store::CreationRun) -> Vec<String> {
    run.outputs.iter().map(|out| out.id.clone()).collect()
}

fn patch(value: Value) -> StudioPrefsPatch {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn deleting_todays_results_does_not_lower_spent_today() {
    let h = harness().await;
    let run = h.seed_done_run_costing(9.50).await;
    h.manager.delete_outputs(output_ids(&run)).await.unwrap();
    assert!(!Path::new(&run.outputs[0].path).exists());
    // Gone from the gallery, kept as a spending stub.
    assert!(h.store.get_creation_run(&run.id).await.unwrap().is_none());
    assert_eq!(h.stub_ids().await, std::slice::from_ref(&run.id));
    let err = h
        .manager
        .start(h.req_estimated(0.96, "after-delete"), true)
        .await
        .unwrap_err();
    assert_eq!(err.message, "Daily limit reached: $9.50 of $10 spent today");
    assert_eq!(h.provider.starts(), 0);
}

#[tokio::test]
async fn deleting_a_finished_run_removes_its_files_and_keeps_its_audit_row() {
    let h = harness_with(Some(crate::attachments::tests::test_worker("thumb"))).await;
    let run = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    let sidecar = h
        .gallery_root
        .join("2026-11")
        .join(format!("{}.json", run.id));
    let thumb = |id: &str| h.gallery_root.join(".thumbs").join(format!("{id}.jpg"));
    assert!(sidecar.exists() && thumb(&done.outputs[0].id).exists());

    // One of its two pictures: the run keeps the other.
    let first = &done.outputs[0];
    h.manager
        .delete_outputs(vec![first.id.clone()])
        .await
        .unwrap();
    assert!(!Path::new(&first.path).exists());
    assert!(!thumb(&first.id).exists());
    assert_eq!(h.run_dto(&run.id).await.outputs.len(), 1);
    assert!(sidecar.exists());

    // The last one: the run goes, with its sidecar; its audit row stays.
    let second = &done.outputs[1];
    h.manager
        .delete_outputs(vec![second.id.clone(), "no-such-output".into()])
        .await
        .unwrap();
    assert!(!Path::new(&second.path).exists());
    assert!(!thumb(&second.id).exists());
    assert!(!sidecar.exists());
    assert!(h.store.get_creation_run(&run.id).await.unwrap().is_none());
    let rows = h.audit_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].result_status.as_deref(), Some("done"));
}

#[tokio::test]
async fn delete_unfinished_removes_stopped_and_failed_runs_only() {
    use store::CreationStatus as S;
    let h = harness().await;
    let done = h.seed_done_run_costing(0.5).await;
    let waiting = h.seed_video_run(S::Waiting, Some("job-waiting")).await;
    let mut ended = Vec::new();
    for status in [S::Failed, S::Blocked, S::Cancelled, S::Interrupted] {
        ended.push(h.seed_video_run(status, Some("job-ended")).await);
    }
    assert_eq!(h.manager.delete_unfinished().await.unwrap(), 4);
    let mut kept: Vec<String> = h.store_runs().await.into_iter().map(|r| r.id).collect();
    kept.sort();
    let mut want = vec![done.id.clone(), waiting];
    want.sort();
    assert_eq!(kept, want);
    // Made today, so each stays as a spending stub.
    ended.sort();
    assert_eq!(h.stub_ids().await, ended);
    // The audit rows stay.
    assert_eq!(h.audit_rows().await.len(), 5);
    assert_eq!(h.manager.delete_unfinished().await.unwrap(), 0);
}

#[tokio::test]
async fn a_reference_no_run_uses_any_more_is_removed_and_usage_counts_refs() {
    let h = harness().await;
    let refs = h.gallery_root.join("refs");
    std::fs::create_dir_all(&refs).unwrap();
    let picture = png(2, 2);
    let (a, b) = ("a".repeat(64), "b".repeat(64));
    let file = |sha: &str| refs.join(format!("{sha}.png"));
    std::fs::write(file(&a), &picture).unwrap();
    std::fs::write(file(&b), &picture).unwrap();
    let uses = |hashes: &[&str]| -> Value {
        hashes
            .iter()
            .map(|sha| json!({"sha256": sha, "mime": "image/png", "sizeBytes": picture.len()}))
            .collect()
    };
    let uses_a = h.seed_done_run(0.5, uses(&[&a])).await;
    let uses_both = h.seed_done_run(0.5, uses(&[&a, &b])).await;

    h.manager
        .delete_outputs(output_ids(&uses_both))
        .await
        .unwrap();
    assert!(!file(&b).exists());
    assert!(file(&a).exists());
    // The output left (recorded as 10 bytes) and the added picture still kept.
    let usage = h.manager.usage().await.unwrap();
    assert_eq!(usage.bytes, 10 + picture.len() as u64);

    // Its last user goes, and so does it.
    h.manager.delete_outputs(output_ids(&uses_a)).await.unwrap();
    assert!(!file(&a).exists());
    assert_eq!(h.manager.usage().await.unwrap().bytes, 0);
}

#[tokio::test]
async fn a_new_gallery_folder_applies_to_the_next_run() {
    // Made first, so it goes after the harness has stopped every run.
    let other = Scratch::new();
    let h = harness().await;
    let prefs = h
        .manager
        .set_prefs(patch(json!({"galleryDir": other.path()})))
        .await
        .unwrap();
    let dir = other.path().display().to_string();
    assert_eq!(prefs.gallery_dir.as_deref(), Some(dir.as_str()));
    let r = h.manager.start(h.picture_req("x"), false).await.unwrap();
    let done = h.wait_status(&r.id, "done").await;
    assert!(Path::new(&done.outputs[0].path).starts_with(other.path()));
    assert_eq!(h.manager.usage().await.unwrap().folder, dir);
    // `null` puts the gallery back in its default place.
    h.manager
        .set_prefs(patch(json!({"galleryDir": null})))
        .await
        .unwrap();
    assert_eq!(h.manager.gallery().root(), h.gallery_root.as_path());
}

#[tokio::test]
async fn set_prefs_changes_only_what_it_is_sent_and_checks_it() {
    let h = harness().await;
    let video = h.video_req_estimated(0.96);
    assert_eq!(
        h.manager.estimate(&video).await.unwrap().reasons,
        [ConfirmReason::Video]
    );
    let changed = h
        .manager
        .set_prefs(patch(json!({"confirmEveryVideo": false})))
        .await
        .unwrap();
    assert!(!changed.confirm_every_video);
    assert_eq!(changed.daily_limit_usd, Some(10.0));
    // Held for the next run, and saved.
    assert_eq!(h.manager.prefs(), changed);
    assert_eq!(prefs::load(&h.store).await, changed);
    assert!(h.manager.estimate(&video).await.unwrap().reasons.is_empty());

    let err = h
        .manager
        .set_prefs(patch(json!({"dailyLimitUsd": 0.5})))
        .await
        .unwrap_err();
    assert_eq!(err.code, "bad_request");
    assert_eq!(err.message, "Choose an amount from $1 to $1,000.");
    assert_eq!(h.manager.prefs(), changed);
    assert_eq!(prefs::load(&h.store).await, changed);

    // `null` turns the limit off.
    let off = h
        .manager
        .set_prefs(patch(json!({"dailyLimitUsd": null})))
        .await
        .unwrap();
    assert_eq!(off.daily_limit_usd, None);
    assert!(!off.confirm_every_video);
}

// ---- the local lane (Task L15) ----------------------------------------------------------------

const KLEIN: &str = "flux2-klein-4b-q4";
const ZIMAGE: &str = "z-image-turbo-q4";
const WAN: &str = "wan22-ti2v-5b-fast-q6";
const GB: u64 = 1_000_000_000;
const CHAT: &str = "/m/chat.gguf";
/// llama.cpp's description of the test's graphics card.
const RX_7600: &str = "AMD Radeon RX 7600 (RADV NAVI33)";
/// The device a run on that card records, as shown.
const RX_7600_SHOWN: &str = "AMD Radeon RX 7600 (Vulkan)";

/// How a [`FakeLocal`] run goes once it started.
#[derive(Debug, Clone, Copy)]
enum Course {
    /// Done at its first check.
    Done,
    /// Ran out of memory and runs again with memory saving for its first two checks, then
    /// done.
    RetryThenDone,
    /// 100 steps over about a second, then done.
    Steps,
    /// Fails with this kind.
    Fail(FailureKind),
}

/// One run [`FakeLocal`] started.
struct FakeRun {
    course: Course,
    /// Its place among the starts, from 0.
    nth: usize,
    started: Instant,
    checks: u32,
    pictures: usize,
}

/// The Studio engine on this computer, in memory: the installed models are the bundled
/// catalog's starters; each start's request, folder and options are recorded; a run goes as
/// scripted ([`Course`], by default done at its first check, its results small PNG pictures)
/// and can be held running until released.
struct FakeLocal {
    entries: Vec<(store::GenModel, GenEntry)>,
    starts: Mutex<Vec<(MediaRequest, PathBuf, LocalJobOptions)>>,
    courses: Mutex<VecDeque<Course>>,
    held: AtomicBool,
    /// How many runs, in start order, were let go while held.
    let_go: AtomicUsize,
    runs: Mutex<HashMap<String, FakeRun>>,
    releases: AtomicUsize,
    cancels: AtomicUsize,
}

/// An installed model's row for a catalog `entry`.
fn installed_row(entry: &GenEntry) -> store::GenModel {
    let kind = match entry.kind {
        GenKind::Image => "image",
        GenKind::ImageEdit => "image-edit",
        GenKind::Video => "video",
    };
    store::GenModel {
        id: entry.id.clone(),
        catalog_id: Some(entry.id.clone()),
        name: entry.name.clone(),
        kind: kind.into(),
        family: serde_json::to_value(entry.family)
            .unwrap()
            .as_str()
            .unwrap()
            .to_owned(),
        parts: json!([]),
        size_bytes: entry.total_bytes(),
        source: store::GenSource::Catalog,
        added_at: 1,
    }
}

impl FakeLocal {
    fn new() -> Self {
        let catalog = gen_catalog(
            &Catalog::bundled(),
            runtime::sd::lock::embedded().sd_cpp.build,
        );
        let entries = [KLEIN, ZIMAGE, WAN]
            .iter()
            .map(|id| {
                let entry = catalog.entries.get(*id).expect("a bundled starter").clone();
                (installed_row(&entry), entry)
            })
            .collect();
        Self {
            entries,
            starts: Mutex::new(Vec::new()),
            courses: Mutex::new(VecDeque::new()),
            held: AtomicBool::new(false),
            let_go: AtomicUsize::new(0),
            runs: Mutex::new(HashMap::new()),
            releases: AtomicUsize::new(0),
            cancels: AtomicUsize::new(0),
        }
    }

    fn starts(&self) -> usize {
        self.starts.lock().unwrap().len()
    }

    fn releases(&self) -> usize {
        self.releases.load(Ordering::SeqCst)
    }

    fn cancels(&self) -> usize {
        self.cancels.load(Ordering::SeqCst)
    }

    /// The options of the last start.
    fn last_options(&self) -> LocalJobOptions {
        self.starts
            .lock()
            .unwrap()
            .last()
            .expect("a local run started")
            .2
            .clone()
    }

    /// How the next run not yet scripted goes.
    fn course(&self, course: Course) {
        self.courses.lock().unwrap().push_back(course);
    }

    /// From now on each run stays running until it is let go.
    fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    /// Lets the oldest held run go on.
    fn release_one(&self) {
        self.let_go.fetch_add(1, Ordering::SeqCst);
    }

    fn release_all(&self) {
        self.held.store(false, Ordering::SeqCst);
    }

    fn is_held(&self, nth: usize) -> bool {
        self.held.load(Ordering::SeqCst) && nth >= self.let_go.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl MediaProvider for FakeLocal {
    fn company(&self) -> Company {
        Company::Local
    }

    fn host(&self) -> String {
        String::new()
    }

    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
        let models: Vec<MediaModel> = self
            .entries
            .iter()
            .map(|(row, entry)| {
                let params = LocalMediaProvider::params_for(entry);
                MediaModel {
                    id: row.id.clone(),
                    display_name: entry.name.clone(),
                    family: row.family.clone(),
                    kind: if entry.kind == GenKind::Video {
                        MediaKind::Video
                    } else {
                        MediaKind::Image
                    },
                    accepts_images: params
                        .iter()
                        .any(|p| matches!(p.kind, ParamKind::Images { max, .. } if max > 0)),
                    params,
                    price: Price::Free,
                    notes: Vec::new(),
                    usable: None,
                    created: None,
                    companies: Vec::new(),
                }
            })
            .collect();
        Ok(RawCatalog {
            images: serde_json::to_value(models).unwrap(),
            videos: None,
        })
    }

    async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
        Ok(Value::Null)
    }

    fn preview(&self, req: &MediaRequest) -> RequestPreview {
        self.preview_local(req, &LocalJobOptions::default())
    }

    async fn start(&self, _req: &MediaRequest, _scratch: &Path) -> Result<Started, MediaError> {
        unreachable!("the local lane starts a run with start_local")
    }

    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError> {
        let mut runs = self.runs.lock().unwrap();
        let Some(run) = runs.get_mut(remote_id) else {
            return Ok(JobState::Gone);
        };
        run.checks += 1;
        if self.is_held(run.nth) {
            return Ok(JobState::Running {
                progress: Some(0.2),
            });
        }
        let done = JobState::Done(JobDone {
            outputs: run.pictures,
            signed: Vec::new(),
            cost: Some(Cost {
                usd: 0.0,
                reported: true,
            }),
        });
        Ok(match run.course {
            Course::Done => done,
            Course::RetryThenDone if run.checks <= 2 => JobState::Running {
                progress: Some(0.1),
            },
            Course::RetryThenDone => done,
            Course::Steps if run.started.elapsed() < Duration::from_secs(1) => JobState::Running {
                progress: Some(0.5),
            },
            Course::Steps => done,
            Course::Fail(kind) => JobState::Failed(MediaFailure {
                kind,
                stage: None,
                charged: Some(false),
                company_code: None,
                request_id: None,
                status: None,
                message: "The Studio engine ran out of memory.".into(),
                retry_after: None,
            }),
        })
    }

    async fn download(
        &self,
        _remote_id: &str,
        _job: &JobDone,
        _index: usize,
        dest: &Path,
    ) -> Result<Fetched, MediaError> {
        let bytes = png(4, 4);
        std::fs::write(dest, &bytes).unwrap();
        Ok(Fetched::Saved(Saved {
            path: dest.to_owned(),
            mime: "image/png".into(),
            size_bytes: bytes.len() as u64,
            width: Some(4),
            height: Some(4),
            duration_ms: None,
        }))
    }

    async fn cancel(&self, remote_id: &str) -> Result<(), MediaError> {
        self.cancels.fetch_add(1, Ordering::SeqCst);
        self.runs.lock().unwrap().remove(remote_id);
        Ok(())
    }
}

#[async_trait]
impl LocalProvider for FakeLocal {
    async fn start_local(
        &self,
        req: &MediaRequest,
        scratch: &Path,
        opts: &LocalJobOptions,
    ) -> Result<Started, MediaError> {
        let nth = {
            let mut starts = self.starts.lock().unwrap();
            starts.push((req.clone(), scratch.to_owned(), opts.clone()));
            starts.len() - 1
        };
        let pictures = match req.kind {
            MediaKind::Image => req.params.get("n").and_then(Value::as_u64).unwrap_or(1) as usize,
            MediaKind::Video => 1,
        };
        let course = self
            .courses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Course::Done);
        let remote_id = format!("local:{}", scratch.file_name().unwrap().to_string_lossy());
        self.runs.lock().unwrap().insert(
            remote_id.clone(),
            FakeRun {
                course,
                nth,
                started: Instant::now(),
                checks: 0,
                pictures,
            },
        );
        Ok(Started::Job { remote_id })
    }

    fn preview_local(&self, req: &MediaRequest, opts: &LocalJobOptions) -> RequestPreview {
        RequestPreview {
            method: "run".into(),
            url: "sd-cli".into(),
            host: String::new(),
            body: json!({
                "prompt": req.prompt,
                "processorOnly": opts.processor_only,
                "memorySaving": opts.memory_saving,
                "device": opts.device_description,
            }),
        }
    }

    async fn release(&self, remote_id: &str) {
        self.releases.fetch_add(1, Ordering::SeqCst);
        self.runs.lock().unwrap().remove(remote_id);
    }

    fn status(&self, remote_id: &str) -> Option<LocalRunStatus> {
        let runs = self.runs.lock().unwrap();
        let run = runs.get(remote_id)?;
        let elapsed = run.started.elapsed();
        let progress = matches!(run.course, Course::Steps).then(|| LocalProgress {
            stage: LocalStage::Drawing,
            pass: 1,
            passes: 1,
            step: u32::try_from(elapsed.as_millis() / 10)
                .unwrap_or(100)
                .min(100),
            steps: 100,
            picture: 1,
            pictures: 1,
            elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            sec_per_step: Some(0.01),
        });
        Some(LocalRunStatus {
            progress,
            stalled: false,
            retrying: matches!(run.course, Course::RetryThenDone) && run.checks <= 2,
            outputs: 0,
            failure: None,
        })
    }

    async fn entry_of(&self, model_id: &str) -> Option<(store::GenModel, GenEntry)> {
        self.entries
            .iter()
            .find(|(row, _)| row.id == model_id)
            .cloned()
    }

    fn engine_build(&self) -> Option<u32> {
        Some(1106)
    }
}

/// What [`FakeLease`] does when it is asked next.
#[derive(Debug, Clone)]
enum LeaseStep {
    Grant(Grant),
    Refuse(Refused),
    /// Says the job waits for this, then grants once released (or ends when cancelled).
    WaitThen(LeaseWait, Grant),
    /// Asks the person (`NeedsAnswer`) until an answer comes, then grants on a yes.
    AskThen(Grant),
}

/// The graphics card's lease: grants as scripted (by default beside the chat model), and
/// records each need and policy it was asked with, each hold dropped (with whether a run was
/// done by then) and each reload.
struct FakeLease {
    script: Mutex<VecDeque<LeaseStep>>,
    needs: Mutex<Vec<StudioNeed>>,
    asked: Mutex<Vec<(GpuPolicy, Option<bool>)>>,
    gate: tokio::sync::Semaphore,
    drops: Arc<Mutex<Vec<bool>>>,
    reloads: Mutex<Vec<String>>,
    sink: Arc<FakeSink>,
}

/// A grant's hold: records its drop.
struct FakeHold {
    drops: Arc<Mutex<Vec<bool>>>,
    sink: Arc<FakeSink>,
}

impl Drop for FakeHold {
    fn drop(&mut self) {
        let done = self
            .sink
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event.status == "done");
        self.drops.lock().unwrap().push(done);
    }
}

impl FakeLease {
    fn new(sink: &Arc<FakeSink>) -> Self {
        Self {
            script: Mutex::new(VecDeque::new()),
            needs: Mutex::new(Vec::new()),
            asked: Mutex::new(Vec::new()),
            gate: tokio::sync::Semaphore::new(0),
            drops: Arc::new(Mutex::new(Vec::new())),
            reloads: Mutex::new(Vec::new()),
            sink: Arc::clone(sink),
        }
    }

    /// What the next ask not yet scripted gets.
    fn then(&self, step: LeaseStep) {
        self.script.lock().unwrap().push_back(step);
    }

    /// Lets every wait go on.
    fn release(&self) {
        self.gate.add_permits(64);
    }

    fn needs(&self) -> Vec<StudioNeed> {
        self.needs.lock().unwrap().clone()
    }

    fn asked(&self) -> Vec<(GpuPolicy, Option<bool>)> {
        self.asked.lock().unwrap().clone()
    }

    fn drops(&self) -> Vec<bool> {
        self.drops.lock().unwrap().clone()
    }

    fn reloads(&self) -> Vec<String> {
        self.reloads.lock().unwrap().clone()
    }
}

#[async_trait]
impl CardLease for FakeLease {
    async fn acquire(
        &self,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<(Grant, Box<dyn std::any::Any + Send + Sync>), Refused> {
        self.needs.lock().unwrap().push(*need);
        self.asked.lock().unwrap().push((policy, answered));
        let step = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(LeaseStep::Grant(Grant::Alongside));
        let grant = match step {
            LeaseStep::Grant(grant) => grant,
            LeaseStep::Refuse(refused) => return Err(refused),
            LeaseStep::WaitThen(wait, grant) => {
                on_wait(wait);
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => return Err(Refused::Cancelled),
                    permit = self.gate.acquire() => permit.unwrap().forget(),
                }
                grant
            }
            LeaseStep::AskThen(grant) => match answered {
                None => {
                    self.script
                        .lock()
                        .unwrap()
                        .push_front(LeaseStep::AskThen(grant));
                    on_wait(LeaseWait::NeedsAnswer);
                    return Err(Refused::NeedsAnswer);
                }
                Some(true) => grant,
                Some(false) => return Err(Refused::DoesNotFit),
            },
        };
        let hold = FakeHold {
            drops: Arc::clone(&self.drops),
            sink: Arc::clone(&self.sink),
        };
        Ok((grant, Box::new(hold)))
    }

    async fn reload_chat(&self, path: &str) {
        self.reloads.lock().unwrap().push(path.to_owned());
    }
}

/// This computer as probed: a graphics card (or the processor only), the chat model's bytes
/// on it, and 8 logical cores.
struct FixedHardware {
    target: Mutex<(Target, String, Option<String>)>,
    chat: Mutex<Option<u64>>,
}

impl FixedHardware {
    /// A Vulkan card with `free_gb` GB free, and `ram_gb` GB of free memory.
    fn gpu(free_gb: u64, ram_gb: u64) -> Self {
        let hardware = Self {
            target: Mutex::new((Target::Cpu { ram_free_bytes: 0 }, String::new(), None)),
            chat: Mutex::new(None),
        };
        hardware.set_gpu(free_gb, ram_gb);
        hardware
    }

    fn set_gpu(&self, free_gb: u64, ram_gb: u64) {
        *self.target.lock().unwrap() = (
            Target::Gpu {
                name: RX_7600.into(),
                free_bytes: free_gb * GB,
                ram_free_bytes: Some(ram_gb * GB),
            },
            "vulkan".into(),
            Some(RX_7600.into()),
        );
    }

    /// A computer that runs models on the processor, with `ram_gb` GB of free memory.
    fn set_cpu(&self, ram_gb: u64) {
        *self.target.lock().unwrap() = (
            Target::Cpu {
                ram_free_bytes: ram_gb * GB,
            },
            "cpu".into(),
            None,
        );
    }

    /// A chat model loaded taking `gb` GB on the card (`None`: none on it).
    fn set_chat(&self, gb: Option<u64>) {
        *self.chat.lock().unwrap() = gb.map(|gb| gb * GB);
    }
}

#[async_trait]
impl LocalHardware for FixedHardware {
    async fn target(&self) -> (Target, String, Option<String>) {
        self.target.lock().unwrap().clone()
    }

    fn chat_device_bytes(&self) -> Option<u64> {
        *self.chat.lock().unwrap()
    }

    fn logical_cores(&self) -> usize {
        8
    }
}

impl H {
    /// A run of installed model `model` on this computer.
    fn local_req_for(&self, model: &str, prompt: &str) -> StartRequest {
        StartRequest {
            client_token: uuid::Uuid::new_v4().to_string(),
            target: RunTarget::ThisComputer,
            kind: if model == WAN {
                MediaKind::Video
            } else {
                MediaKind::Image
            },
            model: model.into(),
            prompt: prompt.into(),
            params: Map::new(),
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
        }
    }

    /// A FLUX.2 klein 4B picture on this computer.
    fn local_req(&self, prompt: &str) -> StartRequest {
        self.local_req_for(KLEIN, prompt)
    }

    /// A Wan 2.2 clip on this computer.
    fn local_video_req(&self) -> StartRequest {
        self.local_req_for(WAN, "a wave breaking")
    }

    async fn set_prefs(&self, value: Value) {
        self.manager.set_prefs(patch(value)).await.unwrap();
    }

    /// The local part of each `running` event of run `id`.
    fn running_cards(&self, id: &str) -> Vec<crate::dto::LocalRunDto> {
        self.events_for(id)
            .into_iter()
            .filter(|event| event.status == "running")
            .filter_map(|event| event.local)
            .collect()
    }

    /// A local run of `model` that finished earlier today on `device` in `elapsed_ms`, at the
    /// model's default size and steps, one picture (or clip).
    async fn seed_measured_run(&self, model: &str, device: &str, elapsed_ms: u64) {
        let entry = self.local.entry_of(model).await.unwrap().1;
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Local,
                provider_id: None,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                model: model.into(),
                model_label: entry.name.clone(),
                prompt: "an earlier picture".into(),
                params: json!({
                    "params": {}, "extra": {}, "routing": null,
                    "job": {
                        "width": entry.defaults.width,
                        "height": entry.defaults.height,
                        "frames": entry.defaults.frames.unwrap_or(1),
                        "steps": entry.defaults.steps,
                        "pictures": 1,
                    },
                }),
                refs: json!([]),
                estimate_usd: Some(0.0),
                gen_model_id: Some(model.into()),
                engine_build: Some(1106),
                device: Some(device.into()),
                created_at: self.now - 60_000,
            })
            .await
            .unwrap();
        self.store
            .update_creation_run(
                &id,
                store::CreationPatch {
                    status: Some(store::CreationStatus::Done),
                    cost_usd: Some(Some(0.0)),
                    elapsed_ms: Some(Some(elapsed_ms)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// A local run as the app left it when it closed: `status`, with its scratch folder.
    async fn seed_local_run(&self, status: store::CreationStatus) -> String {
        let id = uuid::Uuid::new_v4().to_string();
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Local,
                provider_id: None,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                model: KLEIN.into(),
                model_label: "FLUX.2 klein 4B".into(),
                prompt: "a lighthouse".into(),
                params: json!({"params": {}, "extra": {}, "routing": null}),
                refs: json!([]),
                estimate_usd: Some(0.0),
                gen_model_id: Some(KLEIN.into()),
                engine_build: Some(1106),
                device: None,
                created_at: self.now - 60_000,
            })
            .await
            .unwrap();
        self.store
            .update_creation_run(
                &id,
                store::CreationPatch {
                    status: Some(status),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let scratch = self.gallery_root.join(".partial").join(&id);
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("prompt.txt"), "a lighthouse").unwrap();
        id
    }
}

/// Waits until `done` holds (at most 10 s).
async fn eventually(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_local_picture_runs_without_confirmation_and_costs_nothing() {
    let h = harness_with_local().await;
    let run = h
        .manager
        .start(h.local_req("lighthouse"), false)
        .await
        .unwrap();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(h.statuses(&run.id), ["queued", "running", "done"]);
    assert_eq!(
        (
            done.runs_on.as_str(),
            done.cost_usd,
            done.provider_name.as_str()
        ),
        ("local", Some(0.0), "This computer")
    );
    assert_eq!(done.estimate_usd, Some(0.0));
    assert_eq!(done.provider_id, None);
    assert_eq!(done.provider_host, "");
    let local = done.local.as_ref().unwrap();
    assert!(local.labelled);
    assert_eq!(local.device.as_deref(), Some(RX_7600_SHOWN));
    assert_eq!(local.engine_build, Some(1106));
    assert!(local.elapsed_ms.is_some());
    assert_eq!(done.outputs.len(), 1);
    let label = read_label(Path::new(&done.outputs[0].path), "image/png").unwrap();
    assert!(label.contains("FLUX.2 klein 4B"), "{label}");
    assert!(!label.contains("lighthouse"), "{label}");
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    // `.partial/<run>` and its prompt file go with the engine's run.
    assert_eq!(h.local.releases(), 1);
    assert!(h.scratch_is_gone(&run.id));
    // The run recorded what it was made at, for this computer's measured times.
    let row = h.store_run(&run.id).await;
    assert_eq!(row.gen_model_id.as_deref(), Some(KLEIN));
    assert_eq!(row.params["job"]["width"], 1024);
    assert_eq!(row.params["job"]["pictures"], 1);
    assert!(row.elapsed_ms.is_some());
    // The seed Studio picked is sent and kept.
    let sent = h.local.starts.lock().unwrap()[0].0.clone();
    assert!(sent.params.get("seed").and_then(Value::as_u64).is_some());
    assert_eq!(done.outputs[0].seed, sent.params["seed"].as_u64());
}

#[tokio::test]
async fn a_local_video_never_asks_and_never_meets_the_daily_limit() {
    let h = harness_with_local().await;
    h.seed_done_run_costing(9.99).await;
    let run = h.manager.start(h.local_video_req(), false).await.unwrap();
    assert_eq!(run.kind, MediaKind::Video);
    let estimate = h.manager.estimate(&h.local_video_req()).await.unwrap();
    assert!(estimate.reasons.is_empty());
    assert!(estimate.limit.would_pass);
    assert_eq!(estimate.usd, Some(0.0));
}

#[tokio::test]
async fn one_local_job_at_a_time_while_cloud_runs_go_on_beside_it() {
    let h = harness_with_local().await;
    h.local.hold();
    let first = h.manager.start(h.local_req("one"), false).await.unwrap();
    let second = h.manager.start(h.local_req("two"), false).await.unwrap();
    h.wait_status(&first.id, "running").await;
    let waiting = h.run_dto(&second.id).await;
    assert_eq!(waiting.status, "queued");
    assert_eq!(waiting.queue_position, Some(1));
    assert_eq!(h.local.starts(), 1);
    assert!(h.manager.local_running());

    h.provider.hold();
    let cloud = h.manager.start(h.picture_req("x"), false).await.unwrap();
    h.wait_status(&cloud.id, "sending").await;
    assert_eq!(h.run_dto(&second.id).await.status, "queued");
    h.provider.release_all();
    h.wait_status(&cloud.id, "done").await;

    h.local.release_all();
    h.wait_status(&first.id, "done").await;
    h.wait_status(&second.id, "done").await;
    assert_eq!(h.local.starts(), 2);
    assert!(!h.manager.local_running());
}

#[tokio::test]
async fn waiting_for_the_graphics_card_is_queued_with_its_reason() {
    let h = harness_with_local().await;
    h.lease.then(LeaseStep::WaitThen(
        LeaseWait::ReplyRunning,
        Grant::Alongside,
    ));
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    let waiting = h
        .wait_for(&run.id, "waiting for the reply", |r| {
            r.local.as_ref().and_then(|l| l.waiting_for.as_deref()) == Some("reply")
        })
        .await;
    assert_eq!(waiting.status, "queued");
    assert_eq!(waiting.queue_position, None);
    assert!(waiting.watching);
    assert_eq!(h.local.starts(), 0);
    h.lease.release();
    let done = h.wait_status(&run.id, "done").await;
    assert_eq!(done.local.unwrap().waiting_for, None);
}

#[tokio::test]
async fn the_unloaded_chat_model_is_named_on_the_card() {
    let h = harness_with_local().await;
    h.hardware.set_gpu(3, 32);
    h.hardware.set_chat(Some(5));
    h.lease.then(LeaseStep::Grant(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    h.local.hold();
    let run = h
        .manager
        .start(h.local_req_for(ZIMAGE, "x"), false)
        .await
        .unwrap();
    let running = h.wait_status(&run.id, "running").await;
    let local = running.local.unwrap();
    assert_eq!(local.unloaded_chat.as_deref(), Some(CHAT));
    assert!(!local.alongside);
    assert!(!local.on_processor);
    let need = h.lease.needs()[0];
    assert!(!need.alongside && need.after_unload, "{need:?}");
    assert!(!need.shared_memory);
    h.local.release_all();
    h.wait_status(&run.id, "done").await;
}

#[tokio::test]
async fn a_job_that_fits_after_an_unload_can_be_created() {
    let h = harness_with_local().await;
    // Partly on the processor beside the 5 GB chat model, one part at a time after it goes.
    h.hardware.set_gpu(3, 32);
    h.hardware.set_chat(Some(5));
    let e = h
        .manager
        .estimate(&h.local_req_for(ZIMAGE, "x"))
        .await
        .unwrap();
    assert_eq!(
        e.local.as_ref().map(|l| (l.fit.as_str(), l.unload_chat)),
        Some(("afterUnload", true))
    );
    assert!(e.invalid.is_empty());
    assert_eq!(e.local.as_ref().unwrap().reason, None);
    assert!(
        h.manager
            .start(h.local_req_for(ZIMAGE, "x"), false)
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn the_grant_and_the_fit_reach_the_engine_job() {
    let h = harness_with_local().await;

    // The processor, granted by the lease: its threads, and no device.
    h.set_prefs(json!({"gpuPolicy": "processor"})).await;
    h.lease
        .then(LeaseStep::Grant(Grant::UseProcessor { unloaded: None }));
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    let done = h.wait_status(&run.id, "done").await;
    let opts = h.local.last_options();
    assert!(opts.processor_only);
    assert_eq!(
        opts.threads,
        Some(processor_threads(8, cfg!(target_arch = "x86_64")))
    );
    assert_eq!(opts.device_description, None);
    assert_eq!(h.lease.asked()[0].0, GpuPolicy::Processor);
    assert!(h.running_cards(&run.id).iter().any(|l| l.on_processor));
    assert_eq!(done.local.unwrap().device.as_deref(), Some("Processor"));

    // One part at a time beside the chat model: memory saving (Automatic), the card's device.
    h.set_prefs(json!({"gpuPolicy": "unload"})).await;
    h.hardware.set_gpu(6, 32);
    h.hardware.set_chat(Some(5));
    let run = h
        .manager
        .start(h.local_req_for(ZIMAGE, "x"), false)
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    let opts = h.local.last_options();
    assert!(opts.memory_saving);
    assert!(!opts.processor_only);
    assert_eq!(opts.threads, None);
    assert_eq!(opts.device_description.as_deref(), Some(RX_7600));
    let cards = h.running_cards(&run.id);
    assert!(
        cards.iter().any(|l| l.alongside && !l.on_processor),
        "{cards:?}"
    );

    // Memory saving off in the request wins, whatever the fit.
    let mut req = h.local_req_for(ZIMAGE, "x");
    req.params.insert("memory_saving".into(), json!("off"));
    let run = h.manager.start(req, false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    assert!(!h.local.last_options().memory_saving);

    // After an unload, the fit it runs with is the one after the unload (Green here).
    h.lease.then(LeaseStep::Grant(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    h.hardware.set_gpu(3, 32);
    let run = h
        .manager
        .start(h.local_req_for(KLEIN, "x"), false)
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    assert!(!h.local.last_options().memory_saving);
}

#[tokio::test]
async fn a_picture_too_big_for_the_card_runs_on_the_processor() {
    let h = harness_with_local().await;
    h.hardware.set_gpu(1, 32);
    let e = h.manager.estimate(&h.local_req("x")).await.unwrap();
    assert_eq!(e.local.as_ref().unwrap().fit, "slow");
    h.lease
        .then(LeaseStep::Grant(Grant::UseProcessor { unloaded: None }));
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let need = h.lease.needs()[0];
    assert!(
        !need.alongside && !need.after_unload && need.processor_ok,
        "{need:?}"
    );
    assert!(h.local.last_options().processor_only);
}

#[tokio::test]
async fn the_card_stays_held_through_the_out_of_memory_retry() {
    let h = harness_with_local().await;
    h.hardware.set_gpu(3, 32);
    h.hardware.set_chat(Some(5));
    h.lease.then(LeaseStep::Grant(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    h.local.course(Course::RetryThenDone);
    let run = h
        .manager
        .start(h.local_req_for(ZIMAGE, "x"), false)
        .await
        .unwrap();
    h.wait_status(&run.id, "done").await;
    assert!(h.running_cards(&run.id).iter().any(|l| l.retrying));
    assert_eq!(h.local.starts(), 1);
    // Dropped once, after the run was done.
    assert_eq!(h.lease.drops(), [true]);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(h.lease.reloads().is_empty());
}

#[tokio::test]
async fn the_chat_model_reloads_once_when_the_queue_empties() {
    let h = harness_with_local().await;
    h.set_prefs(json!({"reloadChatAfter": true})).await;
    h.lease.then(LeaseStep::Grant(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    h.lease.then(LeaseStep::Grant(Grant::Alongside));
    h.local.hold();
    let first = h.manager.start(h.local_req("one"), false).await.unwrap();
    let second = h.manager.start(h.local_req("two"), false).await.unwrap();
    h.wait_status(&first.id, "running").await;
    h.local.release_one();
    h.wait_status(&first.id, "done").await;
    h.wait_status(&second.id, "running").await;
    assert!(h.lease.reloads().is_empty(), "the queue still had work");
    h.local.release_all();
    h.wait_status(&second.id, "done").await;
    eventually("the chat model reloads", || !h.lease.reloads().is_empty()).await;
    assert_eq!(h.lease.reloads(), [CHAT]);

    // Off: the chat model stays unloaded.
    h.set_prefs(json!({"reloadChatAfter": false})).await;
    h.lease.then(LeaseStep::Grant(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    let third = h.manager.start(h.local_req("three"), false).await.unwrap();
    h.wait_status(&third.id, "done").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.lease.reloads(), [CHAT]);
}

#[tokio::test]
async fn a_job_that_does_not_fit_even_after_an_unload_is_refused() {
    let h = harness_with_local().await;
    h.hardware.set_cpu(16);
    let err = h
        .manager
        .start(h.local_video_req(), false)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_does_not_fit");
    assert_eq!(err.message, "This model does not fit this computer.");
    assert_eq!(err.next_step, "Choose a smaller size or a cloud model.");
    assert!(h.store_runs().await.is_empty());
    assert!(h.audit_rows().await.is_empty());
    assert_eq!(h.local.starts(), 0);

    // The lease's own refusal ends the run the same way, never charged.
    h.hardware.set_gpu(8, 32);
    h.lease.then(LeaseStep::Refuse(Refused::DoesNotFit));
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.charged),
        ("studio_does_not_fit", Some(false))
    );
    assert_eq!(h.local.starts(), 0);
}

#[tokio::test]
async fn ask_waits_for_the_answer() {
    let h = harness_with_local().await;
    h.set_prefs(json!({"gpuPolicy": "ask"})).await;
    h.lease.then(LeaseStep::AskThen(Grant::AfterUnload {
        unloaded: Some(CHAT.into()),
    }));
    h.local.hold();
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    let asking = h
        .wait_for(&run.id, "asking", |r| {
            r.local.as_ref().and_then(|l| l.waiting_for.as_deref()) == Some("answer")
        })
        .await;
    assert_eq!(asking.status, "queued");
    assert_eq!(h.local.starts(), 0);
    let answered = h.manager.answer_unload(&run.id, true).await.unwrap();
    assert_eq!(answered.id, run.id);
    let running = h.wait_status(&run.id, "running").await;
    assert_eq!(running.local.unwrap().unloaded_chat.as_deref(), Some(CHAT));
    assert_eq!(
        h.lease.asked(),
        [(GpuPolicy::Ask, None), (GpuPolicy::Ask, Some(true))]
    );
    h.local.release_all();
    h.wait_status(&run.id, "done").await;
}

#[tokio::test]
async fn stop_on_a_local_run_kills_it_and_says_nothing_was_charged() {
    let h = harness_with_local().await;
    h.local.hold();
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "running").await;
    let r = h.manager.cancel(&run.id).await.unwrap();
    assert_eq!(r.status, "cancelled");
    assert_eq!(h.local.cancels(), 1);
    let error = r.error.unwrap();
    assert_eq!(
        error.message,
        "Stopped. Your computer is no longer working on it. Nothing was charged."
    );
    assert_eq!(error.charged, Some(false));
    assert!(h.scratch_is_gone(&run.id));
    assert!(!h.manager.local_running());
    assert_eq!(h.lease.drops().len(), 1);

    // A run still waiting for its turn is never started.
    let first = h.manager.start(h.local_req("one"), false).await.unwrap();
    let second = h.manager.start(h.local_req("two"), false).await.unwrap();
    h.wait_status(&first.id, "running").await;
    let r = h.manager.cancel(&second.id).await.unwrap();
    assert_eq!(r.status, "cancelled");
    assert_eq!(r.error.unwrap().charged, Some(false));
    h.local.release_all();
    h.wait_status(&first.id, "done").await;
    assert_eq!(h.local.starts(), 2);
}

#[tokio::test]
async fn a_local_run_left_running_becomes_interrupted_after_a_restart() {
    let h = harness_with_local().await;
    let running = h.seed_local_run(store::CreationStatus::Running).await;
    let queued = h.seed_local_run(store::CreationStatus::Queued).await;
    h.manager.resume_interrupted().await;
    for id in [&running, &queued] {
        let run = h.run_dto(id).await;
        assert_eq!(run.status, "interrupted");
        let error = run.error.unwrap();
        assert_eq!(error.charged, Some(false));
        assert_eq!(error.message, "The app closed while this was being made.");
        assert!(h.scratch_is_gone(id));
    }
    assert_eq!(h.local.starts(), 0);
}

#[tokio::test]
async fn studio_wont_make_what_the_check_refuses() {
    let h = harness_with_local().await;
    let err = h
        .manager
        .start(h.local_req("erotic photo of minors"), false)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_wont_make");
    assert!(h.store_runs().await.is_empty());
    assert_eq!(h.local.starts(), 0);
}

#[tokio::test]
async fn a_local_prompt_over_4000_characters_is_refused_before_anything_is_stored() {
    let h = harness_with_local().await;
    let long = "a".repeat(4_001);
    let err = h
        .manager
        .start(h.local_req(&long), false)
        .await
        .unwrap_err();
    assert_eq!(err.code, "studio_prompt_too_long_here");
    assert_eq!(err.next_step, "Shorten it, or choose a cloud model.");
    assert!(h.store_runs().await.is_empty());
    assert!(
        h.manager
            .estimate(&h.local_req(&long))
            .await
            .unwrap()
            .invalid
            .contains(&"Keep the prompt under 4,000 characters for local models.".to_string())
    );
    assert!(
        h.manager
            .estimate(&h.local_req(&"a".repeat(4_000)))
            .await
            .unwrap()
            .invalid
            .is_empty()
    );
}

#[tokio::test]
async fn the_estimate_for_a_local_run_is_a_time_not_a_price() {
    let h = harness_with_local().await;
    h.seed_measured_run(KLEIN, RX_7600_SHOWN, 30_000).await;
    let e = h.manager.estimate(&h.local_req("x")).await.unwrap();
    assert_eq!(e.usd, Some(0.0));
    assert!(e.reasons.is_empty());
    let eta = e.eta.clone().expect("a time measured on this computer");
    assert!(eta.measured);
    assert!(eta.low_ms <= 30_000 && eta.high_ms >= 30_000, "{eta:?}");
    let local = e.local.unwrap();
    assert_eq!(
        (
            local.fit.as_str(),
            local.unload_chat,
            local.very_slow,
            local.over_15_min
        ),
        ("fits", false, false, false)
    );
    assert_eq!(local.reason, None);

    // 20 minutes measured: very slow, over 15 minutes.
    h.seed_measured_run(ZIMAGE, RX_7600_SHOWN, 20 * 60_000)
        .await;
    let local = h
        .manager
        .estimate(&h.local_req_for(ZIMAGE, "x"))
        .await
        .unwrap()
        .local
        .unwrap();
    assert!(local.very_slow && local.over_15_min, "{local:?}");

    // Not here: very slow (Studio offers a cloud model), but no time, and why.
    h.hardware.set_cpu(16);
    let e = h.manager.estimate(&h.local_video_req()).await.unwrap();
    let local = e.local.unwrap();
    assert_eq!(local.fit, "notHere");
    assert!(local.very_slow && !local.over_15_min);
    assert_eq!(
        local.reason.as_deref(),
        Some("video needs a graphics card or an Apple-silicon Mac")
    );
    assert!(e.eta.is_none());
}

#[tokio::test]
async fn local_progress_is_sent_at_most_four_times_a_second() {
    let h = harness_with_local().await;
    h.local.course(Course::Steps);
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let running = h.running_cards(&run.id);
    // The first `running` event is the status change; the rest are progress only.
    assert!(
        running.len() - 1 <= 4,
        "{} progress events",
        running.len() - 1
    );
    assert!(LOCAL_CHECK_EVERY >= Duration::from_millis(250));
    assert!(
        running
            .iter()
            .any(|l| l.stage.as_deref() == Some("drawing") && l.steps == Some(100)),
        "{running:?}"
    );
}

#[tokio::test]
async fn the_audit_row_says_this_computer_and_never_the_prompt() {
    let h = harness_with_local().await;
    let prompt = "a lighthouse at dusk over a stormy sea";
    let run = h.manager.start(h.local_req(prompt), false).await.unwrap();
    h.wait_status(&run.id, "done").await;
    let rows = h.audit_rows().await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.tool, "picture");
    assert_eq!(
        row.preview,
        "Made a picture on this computer with FLUX.2 klein 4B"
    );
    assert_eq!(row.arguments["runsOn"], "local");
    assert_eq!(row.arguments["engine_build"], 1106);
    assert_eq!(row.arguments["catalog_id"], KLEIN);
    assert!(
        row.arguments["revision"]
            .as_str()
            .is_some_and(|r| r.len() == 40)
    );
    assert_eq!(row.arguments["prompt_chars"], prompt.chars().count());
    assert!(!row.arguments.to_string().contains("lighthouse"));
    assert_eq!(row.result_status.as_deref(), Some("done"));
    assert_eq!(row.result_summary.as_deref(), Some("cost $0.00"));
}

#[tokio::test]
async fn quitting_is_asked_only_while_a_local_job_works() {
    let h = harness_with_local().await;
    assert!(!h.manager.local_running());
    h.local.hold();
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "running").await;
    assert!(h.manager.local_running());
    assert!(super::quit::ask_before_quit(
        h.manager.local_running(),
        None
    ));
    h.local.release_all();
    h.wait_status(&run.id, "done").await;
    assert!(!h.manager.local_running());
    assert!(!super::quit::ask_before_quit(
        h.manager.local_running(),
        None
    ));
}

#[tokio::test]
async fn an_update_waits_for_a_local_job() {
    let h = harness_with_local().await;
    h.local.hold();
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    h.wait_status(&run.id, "running").await;
    let err = crate::updates::refuse_while_studio_works(Some(&h.manager)).unwrap_err();
    assert_eq!(err.code, "studio_busy_for_update");
    assert_eq!(
        err.next_step,
        "Stop it in Studio, or install the update when it finishes."
    );
    h.local.release_all();
    h.wait_status(&run.id, "done").await;
    assert!(crate::updates::refuse_while_studio_works(Some(&h.manager)).is_ok());
    assert!(crate::updates::refuse_while_studio_works(None).is_ok());
}

#[tokio::test]
async fn every_local_fit_is_the_lanes_verdict() {
    let h = harness_with_local().await;
    h.hardware.set_gpu(3, 32);
    h.hardware.set_chat(Some(5));
    let entry = h.local.entry_of(ZIMAGE).await.unwrap().1;
    let size = GenSize {
        width: 1024,
        height: 1024,
        frames: 1,
    };
    let steps = entry.defaults.steps;
    let verdict: LocalVerdict =
        local_verdict(&entry, size, steps, 1, h.hardware.as_ref(), &h.store).await;
    assert_eq!(verdict.word, FitWord::AfterUnload);
    assert_eq!(verdict.card, RX_7600_SHOWN);
    assert_eq!(verdict.description.as_deref(), Some(RX_7600));
    assert!(verdict.reason.is_none() && verdict.eta.is_none());
    // A time measured on this card is this computer's.
    h.seed_measured_run(ZIMAGE, RX_7600_SHOWN, 40_000).await;
    let verdict = local_verdict(&entry, size, steps, 1, h.hardware.as_ref(), &h.store).await;
    assert!(verdict.eta.is_some_and(|eta| eta.measured));
    // The estimate says the same.
    let e = h
        .manager
        .estimate(&h.local_req_for(ZIMAGE, "x"))
        .await
        .unwrap();
    assert_eq!(e.local.unwrap().fit, "afterUnload");
    assert!(e.eta.is_some_and(|eta| eta.measured));
}

#[tokio::test]
async fn a_local_run_that_fails_says_why_and_is_never_charged() {
    let h = harness_with_local().await;
    h.local.course(Course::Fail(FailureKind::OutOfMemory));
    let run = h.manager.start(h.local_req("x"), false).await.unwrap();
    let failed = h.wait_status(&run.id, "failed").await;
    let error = failed.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.charged, error.company.as_str()),
        ("studio_out_of_memory", Some(false), "local")
    );
    assert_eq!(error.next_step, "Try 480p or a shorter clip.");
    assert_eq!(h.local.releases(), 1);
    assert_eq!(h.lease.drops().len(), 1);
    assert!(failed.outputs.is_empty());
    assert!(!failed.local.unwrap().labelled);
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
}

#[tokio::test]
async fn quit_and_stop_stops_every_local_run_and_starts_none() {
    let h = harness_with_local().await;
    h.local.hold();
    let first = h.manager.start(h.local_req("one"), false).await.unwrap();
    let second = h.manager.start(h.local_req("two"), false).await.unwrap();
    h.wait_status(&first.id, "running").await;
    h.manager.stop_local().await;
    for id in [&first.id, &second.id] {
        let run = h.run_dto(id).await;
        assert_eq!(run.status, "cancelled");
        assert_eq!(run.error.unwrap().charged, Some(false));
    }
    assert!(!h.manager.local_running());
    assert_eq!(h.local.starts(), 1);
    assert_eq!(h.local.cancels(), 1);
}
