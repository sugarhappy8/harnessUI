//! The Studio manager's tests (Task A13): starting runs under the spending rules, the queue,
//! pictures, cancel and the audit row.
//!
//! The harness keeps an OpenRouter endpoint whose saved lists are the recorded fixtures (with
//! every recorded `/endpoints` answer, so an OpenRouter picture has a price), plus test models
//! priced at fixed amounts ([`TEST_PICTURE_PRICES`], [`TEST_VIDEO_PRICES`]), and an OpenAI
//! endpoint whose GPT Image 2.6 entry states no price per picture. Every request goes to a
//! [`FakeProvider`] that counts its starts and answers as scripted.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agent::ProviderError;
use agent::media::{
    Company, Cost, FailureKind, Fetched, JobDone, JobState, LocalIo, MediaError, MediaFailure,
    MediaKind, MediaLimits, MediaProvider, MediaRequest, RawCatalog, RequestPreview, Routing,
    SEED_MAX, Saved, Stage, Started,
};
use async_trait::async_trait;
use jiff::tz::TimeZone;
use serde_json::{Map, Value, json};
use store::AuditDecision;
use url::Url;

use super::catalog::{CachedCatalog, CachedPrice, cache_key};
use super::deps::{Clock, Endpoint, ForeignFetch, ProviderSource, StudioSink};
use super::gallery::GalleryError;
use super::manager::{
    PICTURES_AT_ONCE, StudioDeps, StudioManager, place_extra_or_refuse, spend_row,
};
use super::prefs::{RunTarget, StudioPrefs};
use super::spend::{Charge, ConfirmReason, UNKNOWN_PRICE_RESERVE_USD, day_start_in};
use super::test_support::{Scratch, png};
use crate::attachments::PictureWorker;
use crate::dto::{RunDto, StartRequest};
use crate::error::{AppError, AppResult};

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
const TEST_PICTURE_PRICES: [f64; 3] = [1.00, 1.50, 4.50];
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
}

/// A media company that counts its starts, answers as scripted (by default two PNG pictures
/// costing $0.04 for a picture, a job `vid_1` for a video) and can hold each start until
/// released.
struct FakeProvider {
    starts: AtomicUsize,
    script: Mutex<VecDeque<Answer>>,
    held: AtomicBool,
    gate: tokio::sync::Semaphore,
    requests: Mutex<Vec<MediaRequest>>,
    /// The run's row as each start found it (status, started_at).
    seen: Mutex<Vec<(String, Option<i64>)>>,
    store: OnceLock<store::Store>,
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
        }
    }

    fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }

    /// The answer to the next start not yet scripted.
    fn answer(&self, answer: Answer) {
        self.script.lock().unwrap().push_back(answer);
    }

    /// From now on each start waits for [`Self::release_all`].
    fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    fn release_all(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.gate.add_permits(64);
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
        }
    }

    async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
        unreachable!("video jobs are checked from Task A14")
    }

    async fn download(
        &self,
        _remote_id: &str,
        _job: &JobDone,
        _index: usize,
        _dest: &Path,
    ) -> Result<Fetched, MediaError> {
        unreachable!("video jobs are downloaded from Task A14")
    }
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
}

/// Keeps every progress event.
#[derive(Default)]
struct FakeSink {
    events: Mutex<Vec<RunDto>>,
}

impl StudioSink for FakeSink {
    fn progress(&self, run: &RunDto) {
        self.events.lock().unwrap().push(run.clone());
    }
}

/// No result in these tests comes from a company's file host.
struct FakeFetch;

#[async_trait]
impl ForeignFetch for FakeFetch {
    async fn fetch_to_file(
        &self,
        _url: &Url,
        _cap: u64,
        _dest: &Path,
    ) -> Result<String, GalleryError> {
        unreachable!("no result link in these tests")
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
    sink: Arc<FakeSink>,
    store: store::Store,
    gallery_root: PathBuf,
    openrouter: i64,
    openai: i64,
    now: i64,
    _scratch: Scratch,
}

impl Drop for H {
    fn drop(&mut self) {
        // Starts still held would wait for ever.
        self.provider.release_all();
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
    let endpoint = |id: i64, name: &str, host: &str, company| Endpoint {
        id: Some(store::ProviderId(id)),
        name: name.into(),
        host: host.into(),
        company,
    };
    let source = FakeSource {
        provider: provider.clone(),
        endpoints: HashMap::from([
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
        ]),
    };
    let scratch = Scratch::new();
    let gallery_root = scratch.path().join("gallery");
    let sink = Arc::new(FakeSink::default());
    let manager = StudioManager::new(
        StudioDeps {
            store: store.clone(),
            providers: Arc::new(source),
            sink: sink.clone(),
            fetch: Arc::new(FakeFetch),
            clock: Arc::new(FixedClock(now)),
            default_gallery: gallery_root.clone(),
            thumbs,
            limits: MediaLimits::default(),
        },
        StudioPrefs::default(),
    );
    H {
        manager,
        provider,
        sink,
        store,
        gallery_root,
        openrouter,
        openai,
        now,
        _scratch: scratch,
    }
}

impl H {
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

    /// The run once its status is `status` (at most 10 s).
    async fn wait_status(&self, id: &str, status: &str) -> RunDto {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let run = self.run_dto(id).await;
            if run.status == status {
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
                refs: json!([]),
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

// ---- cancel and failures ----------------------------------------------------------------------

#[tokio::test]
async fn an_openai_picture_cancelled_while_sending_may_be_charged_and_counts() {
    let h = harness().await;
    h.provider.hold();
    let run = h.manager.start(h.openai_req(), true).await.unwrap();
    h.wait_status(&run.id, "sending").await;
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
