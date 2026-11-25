//! Studio end to end, with nothing faked between the manager and the wire (Task D1).
//!
//! The cloud runs go through the real `OpenRouterMedia`, `StoreProviders` (the key in a
//! `MemorySecretStore`), `catalog`, `StudioManager` and gallery to [`MockOpenRouter`]: a local
//! HTTP server that answers as openrouter.ai does, from the recorded catalogue fixtures, and
//! counts every request it gets. A video job stays pending until the harness restarts the app
//! ([`WireHarness::restart`]), so only the manager started after the restart can finish it.
//!
//! The local runs go through the real `LocalMediaProvider` with the fake engine (this test
//! binary as `sd-cli`), the real `GpuLease` over a fake chat model and an installed FLUX.2
//! klein 4B whose parts are tiny files ([`WireHarness::with_local`]); the mock still runs, so
//! any request a local run made would be counted.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::media::{MediaKind, MediaLimits, RawCatalog};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

use super::actions::export;
use super::catalog::{CachedCatalog, CachedPrice, cache_key};
use super::commands;
use super::deps::{
    AppLease, Clock, GuardedFetch, LocalLane, StoreProviders, StudioSink, SystemClock,
};
use super::label;
use super::local::LocalMediaProvider;
use super::local::tests::{Fixed, FixedFolder, catalog, fake_engine, install};
use super::manager::{PollPlan, StudioDeps, StudioManager};
use super::manager_tests::FixedHardware;
use super::prefs::{self, RunTarget, StudioPrefs};
use super::test_support::{Scratch, mp4, png};
use crate::dto::{MediaModelDto, RunDto, RunsFilter, StartRequest};
use crate::gpu_lease::GpuLease;
use crate::gpu_lease::tests::FakeChat;
use crate::providers::secret_key;

const IMAGES: &str =
    include_str!("../../../crates/agent/tests/fixtures/media/openrouter-images-2026-11-17.json");
const VIDEOS: &str =
    include_str!("../../../crates/agent/tests/fixtures/media/openrouter-videos-2026-11-17.json");
const ENDPOINTS: &str = include_str!(
    "../../../crates/agent/tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json"
);

const FLUX: &str = "black-forest-labs/flux-3-image";
const GROK_LITE: &str = "x-ai/grok-imagine-video-1.5-lite";
const KLEIN: &str = "flux2-klein-4b-q4";
const KEY: &str = "sk-or-v1-wire-test-key-0123456789";
const GB: u64 = 1_000_000_000;

/// The video job the mock starts.
const JOB: &str = "vid_1";
/// Where the mock answers checks on that job.
const JOB_PATH: &str = "/api/v1/videos/vid_1";
/// Where the mock serves that job's result.
const CONTENT: &str = "/api/v1/videos/vid_1/content";

// ---- the mock ---------------------------------------------------------------------------------

/// openrouter.ai on this computer: the recorded model lists and FLUX 3's `/endpoints` answer,
/// one PNG for a picture (`usage.cost` 0.048), and video job [`JOB`], pending until
/// [`Self::complete_video`], then pending once more and completed (`usage.cost` 0.36), with a
/// small MP4 as its result. Every request is recorded, its path without the query.
#[derive(Clone)]
struct MockOpenRouter {
    url: String,
    state: Arc<MockState>,
}

#[derive(Default)]
struct MockState {
    /// Each request's method and path, in the order they came.
    requests: Mutex<Vec<(String, String)>>,
    /// The video job may complete.
    video_ready: AtomicBool,
    /// Checks on the video job since it may complete.
    checks_since_ready: AtomicUsize,
}

/// An answer: its status line, media type and body.
struct Reply {
    status: &'static str,
    kind: &'static str,
    body: Vec<u8>,
}

impl Reply {
    fn json(status: &'static str, body: &Value) -> Self {
        Self {
            status,
            kind: "application/json",
            body: body.to_string().into_bytes(),
        }
    }
}

impl MockOpenRouter {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(MockState::default());
        let (served, base) = (Arc::clone(&state), url.clone());
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(serve(socket, Arc::clone(&served), base.clone()));
            }
        });
        Self { url, state }
    }

    /// `http://127.0.0.1:<port>`.
    fn url(&self) -> String {
        self.url.clone()
    }

    /// How many `method` requests came for `path`.
    fn count(&self, method: &str, path: &str) -> usize {
        self.requests()
            .iter()
            .filter(|(m, p)| m == method && p == path)
            .count()
    }

    /// Every request so far: its method and path.
    fn requests(&self) -> Vec<(String, String)> {
        self.state.requests.lock().unwrap().clone()
    }

    /// From now on video job [`JOB`] is pending once more, then completed.
    fn complete_video(&self) {
        self.state.video_ready.store(true, Ordering::SeqCst);
    }

    /// The answer to `method` `path` (`query` after the `?`).
    fn answer(state: &MockState, base: &str, method: &str, path: &str, query: &str) -> Reply {
        let price = format!("/api/v1/images/models/{FLUX}/endpoints");
        match method {
            "GET" if path == "/api/v1/images/models" => text_reply(IMAGES),
            "GET" if path == "/api/v1/videos/models" => text_reply(VIDEOS),
            "GET" if path == price => {
                let recorded: Value = serde_json::from_str(ENDPOINTS).unwrap();
                Reply::json("200 OK", &recorded["answers"][FLUX])
            }
            "POST" if path == "/api/v1/images" => Reply::json(
                "200 OK",
                &json!({
                    "data": [{"b64_json": STANDARD.encode(png(8, 8)), "media_type": "image/png"}],
                    "usage": {"cost": 0.048},
                }),
            ),
            "POST" if path == "/api/v1/videos" => Reply::json(
                "202 Accepted",
                &json!({
                    "id": JOB,
                    "polling_url": format!("{base}{JOB_PATH}"),
                    "status": "pending",
                }),
            ),
            "GET" if path == JOB_PATH => {
                let completed = state.video_ready.load(Ordering::SeqCst)
                    && state.checks_since_ready.fetch_add(1, Ordering::SeqCst) > 0;
                if completed {
                    Reply::json(
                        "200 OK",
                        &json!({
                            "id": JOB,
                            "status": "completed",
                            "unsigned_urls": [format!("{base}{CONTENT}?index=0")],
                            "usage": {"cost": 0.36},
                        }),
                    )
                } else {
                    Reply::json("200 OK", &json!({"id": JOB, "status": "pending"}))
                }
            }
            "GET" if path == CONTENT && query == "index=0" => Reply {
                status: "200 OK",
                kind: "video/mp4",
                body: mp4(),
            },
            _ => Reply::json(
                "404 Not Found",
                &json!({"error": {"message": "Not found", "code": 404}}),
            ),
        }
    }
}

/// A recorded fixture as a JSON answer.
fn text_reply(text: &str) -> Reply {
    Reply {
        status: "200 OK",
        kind: "application/json",
        body: text.as_bytes().to_vec(),
    }
}

/// Reads one request (its head and its `content-length` body), records it, answers it and
/// closes the connection.
async fn serve(socket: tokio::net::TcpStream, state: Arc<MockState>, base: String) {
    let (read, mut write) = socket.into_split();
    let mut read = BufReader::new(read);
    let mut line = String::new();
    if read.read_line(&mut line).await.unwrap_or(0) == 0 {
        return;
    }
    let mut length = 0;
    loop {
        let mut header = String::new();
        match read.read_line(&mut header).await {
            Ok(n) if n > 0 && !header.trim_end().is_empty() => {
                if let Some((name, value)) = header.split_once(':')
                    && name.trim().eq_ignore_ascii_case("content-length")
                {
                    length = value.trim().parse().unwrap_or(0);
                }
            }
            _ => break,
        }
    }
    let mut body = vec![0; length];
    if read.read_exact(&mut body).await.is_err() {
        return;
    }
    let mut words = line.split_whitespace();
    let method = words.next().unwrap_or_default().to_owned();
    let target = words.next().unwrap_or_default();
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    state
        .requests
        .lock()
        .unwrap()
        .push((method.clone(), path.to_owned()));
    let reply = MockOpenRouter::answer(&state, &base, &method, path, query);
    let head = format!(
        "HTTP/1.1 {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        reply.status,
        reply.kind,
        reply.body.len()
    );
    let _ = write.write_all(head.as_bytes()).await;
    let _ = write.write_all(&reply.body).await;
    let _ = write.shutdown().await;
}

// ---- the harness ------------------------------------------------------------------------------

/// Keeps every progress event.
#[derive(Default)]
struct RecordingSink(Mutex<Vec<RunDto>>);

impl StudioSink for RecordingSink {
    fn progress(&self, run: &RunDto) {
        self.0.lock().unwrap().push(run.clone());
    }
}

impl RecordingSink {
    /// The statuses the events of run `id` gave, each once in a row.
    fn statuses(&self, id: &str) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for event in self.0.lock().unwrap().iter() {
            if event.id == id && out.last() != Some(&event.status) {
                out.push(event.status.clone());
            }
        }
        out
    }
}

/// Every manager a test made, shut down when the test ends: their tasks stop before the
/// gallery folder goes.
#[derive(Default)]
struct Managers(Mutex<Vec<StudioManager>>);

impl Managers {
    fn keep(&self, manager: &StudioManager) {
        self.0.lock().unwrap().push(manager.clone());
    }
}

impl Drop for Managers {
    fn drop(&mut self) {
        let managers = std::mem::take(&mut *self.0.lock().unwrap());
        // The tasks run on the app's runtime, so this thread can wait for them.
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

/// The app's Studio as it is wired at start-up, with an OpenRouter endpoint at the mock.
struct WireHarness {
    manager: StudioManager,
    mock: MockOpenRouter,
    store: store::Store,
    secrets: Arc<dyn store::secrets::SecretStore>,
    sink: Arc<RecordingSink>,
    /// The OpenRouter endpoint.
    provider_id: i64,
    /// The prompt every run sends.
    prompt: String,
    /// Runs on this computer ([`Self::with_local`]).
    local: Option<LocalLane>,
    /// The chat model on the graphics card ([`Self::with_local`]).
    chat: Option<Arc<FakeChat>>,
    managers: Managers,
    scratch: Scratch,
}

impl WireHarness {
    /// An OpenRouter endpoint at `mock.url() + "/api/v1"` with its key, and no runs on this
    /// computer.
    async fn new(mock: &MockOpenRouter) -> Self {
        Self::build(mock, None, None, Scratch::new()).await
    }

    /// [`Self::new`], with runs on this computer: FLUX.2 klein 4B installed (its parts tiny
    /// files, its diffusion part the 4B's tensor table with no metadata keys), the fake engine
    /// in `mode`, a 6 GB Vulkan card with 1 GB free beside a 5 GB chat model, and the app's
    /// lease over that chat model.
    async fn with_local(mock: &MockOpenRouter, mode: &str) -> Self {
        Self::with_local_on(mock, mode, 1, 32, 5).await
    }

    /// [`Self::with_local`] on a Vulkan card with `free_gb` GB free beside a chat model of
    /// `chat_gb` GB, and `ram_gb` GB of free memory.
    async fn with_local_on(
        mock: &MockOpenRouter,
        mode: &str,
        free_gb: u64,
        ram_gb: u64,
        chat_gb: u64,
    ) -> Self {
        let scratch = Scratch::new();
        let store = store::Store::open_in_memory().unwrap();
        store
            .insert_gen_model(install(
                &scratch.path().join("models"),
                KLEIN,
                "klein4b-tensors.txt",
            ))
            .await
            .unwrap();
        let record = scratch.path().join("record");
        std::fs::create_dir_all(&record).unwrap();
        let provider = LocalMediaProvider::new(
            store.clone(),
            Arc::new(Fixed(catalog())),
            Arc::new(|| None),
            Arc::new(FixedFolder(scratch.path().join("models"))),
        );
        provider.set_engine(Some(fake_engine(mode, &record)));
        let chat = FakeChat::loaded(chat_gb * GB);
        let hardware = FixedHardware::gpu(free_gb, ram_gb);
        hardware.set_chat(Some(chat_gb));
        let lane = LocalLane {
            provider: Arc::new(provider),
            lease: Arc::new(AppLease(GpuLease::new(chat.clone()))),
            hardware: Arc::new(hardware),
        };
        let mut h = Self::build(mock, Some(store), Some(lane), scratch).await;
        h.chat = Some(chat);
        h
    }

    async fn build(
        mock: &MockOpenRouter,
        store: Option<store::Store>,
        local: Option<LocalLane>,
        scratch: Scratch,
    ) -> Self {
        let store = store.unwrap_or_else(|| store::Store::open_in_memory().unwrap());
        let id = store
            .upsert_provider(store::ProviderInput {
                id: None,
                name: "My OpenRouter".into(),
                kind: "openrouter".into(),
                base_url: format!("{}/api/v1", mock.url()),
                secret_ref: None,
                enabled: true,
            })
            .await
            .unwrap();
        let secrets: Arc<dyn store::secrets::SecretStore> =
            Arc::new(store::MemorySecretStore::new());
        secrets.set(&secret_key(id), KEY).unwrap();
        let sink = Arc::new(RecordingSink::default());
        let managers = Managers::default();
        let gallery = scratch.path().join("gallery");
        let manager =
            make_manager(&store, &secrets, &sink, &gallery, local.clone(), &managers).await;
        Self {
            manager,
            mock: mock.clone(),
            store,
            secrets,
            sink,
            provider_id: id.0,
            prompt: "a red kite over a grey sea, seen from the cliffs".into(),
            local,
            chat: None,
            managers,
            scratch,
        }
    }

    /// The app closing and starting again: this manager stops every task (recording
    /// nothing), the video job may then complete, and a new manager starts on the same
    /// store, keychain and gallery.
    async fn restart(self) -> Self {
        self.manager.shutdown().await;
        self.mock.complete_video();
        let gallery = self.manager.gallery().root().to_owned();
        let manager = make_manager(
            &self.store,
            &self.secrets,
            &self.sink,
            &gallery,
            self.local.clone(),
            &self.managers,
        )
        .await;
        Self { manager, ..self }
    }

    /// *Get the list*: the endpoint's picture and video lists, fetched and saved.
    async fn refresh_lists(&self) {
        commands::refresh_models(&self.manager, self.provider_id, |_| true)
            .await
            .unwrap();
    }

    /// Picking OpenRouter picture model `model`: its price, fetched and saved.
    async fn refresh_price(&self, model: &str) {
        commands::model_price(&self.manager, self.provider_id, model)
            .await
            .unwrap();
    }

    /// The lists as *Get the list* saves them, written straight to the store.
    async fn save_lists(&self) {
        let now = SystemClock.now_ms();
        let recorded: Value = serde_json::from_str(ENDPOINTS).unwrap();
        let saved = CachedCatalog {
            fetched_at: now,
            raw: RawCatalog {
                images: serde_json::from_str(IMAGES).unwrap(),
                videos: Some(serde_json::from_str(VIDEOS).unwrap()),
            },
            image_prices: [(
                FLUX.to_owned(),
                CachedPrice {
                    fetched_at: now,
                    raw: recorded["answers"][FLUX].clone(),
                },
            )]
            .into(),
        };
        self.store
            .set_setting(&cache_key(store::ProviderId(self.provider_id)), &saved)
            .await
            .unwrap();
    }

    /// The models of the endpoint's saved lists, as the picker gets them.
    async fn models(&self) -> Vec<MediaModelDto> {
        commands::models(&self.manager, |_| true)
            .await
            .unwrap()
            .endpoints
            .into_iter()
            .find(|e| e.provider_id == self.provider_id)
            .expect("the OpenRouter endpoint")
            .models
    }

    fn request(
        &self,
        target: RunTarget,
        kind: MediaKind,
        model: &str,
        params: Value,
    ) -> StartRequest {
        let Value::Object(params) = params else {
            panic!("the settings are an object");
        };
        StartRequest {
            client_token: uuid::Uuid::new_v4().to_string(),
            target,
            kind,
            model: model.into(),
            prompt: self.prompt.clone(),
            params,
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
        }
    }

    /// A picture of `model` through the OpenRouter endpoint.
    fn picture(&self, model: &str, params: Value) -> StartRequest {
        let target = RunTarget::Endpoint(self.provider_id);
        self.request(target, MediaKind::Image, model, params)
    }

    /// A video of `model` through the OpenRouter endpoint.
    fn video(&self, model: &str, params: Value) -> StartRequest {
        let target = RunTarget::Endpoint(self.provider_id);
        self.request(target, MediaKind::Video, model, params)
    }

    /// A picture of installed model `model` on this computer.
    fn local(&self, model: &str, params: Value) -> StartRequest {
        self.request(RunTarget::ThisComputer, MediaKind::Image, model, params)
    }

    async fn run(&self, id: &str) -> RunDto {
        let seq = self.manager.next_seq();
        let row = self.store.get_creation_run(id).await.unwrap().unwrap();
        self.manager.run_dto(&row, seq).await
    }

    /// Run `id` once its status is `status`; for a status a run ends with, also once its
    /// task is gone (its last event, audit row, sidecar and scratch folder done).
    async fn wait(&self, id: &str, status: &str) -> RunDto {
        self.wait_for(id, status, |run| run.status == status).await
    }

    /// Run `id` once `done` holds for it, and its task is gone when it ended. Fails at once
    /// when it ended otherwise, and after 60 s (the fake engine is this test binary started
    /// again, slow in a debug build); `wanted` names what it waits for.
    async fn wait_for(&self, id: &str, wanted: &str, done: impl Fn(&RunDto) -> bool) -> RunDto {
        let deadline = Instant::now() + Duration::from_secs(60);
        loop {
            let run = self.run(id).await;
            let task_gone = ended(&run.status) && !run.watching;
            if done(&run) && (task_gone || !ended(&run.status)) {
                return run;
            }
            assert!(
                !task_gone,
                "run {id} ended {} ({:?}), not {wanted}",
                run.status, run.error
            );
            assert!(
                Instant::now() < deadline,
                "run {id} is still {} ({:?}), not {wanted}",
                run.status,
                run.error
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Whether `path` is a file in the gallery folder.
    fn gallery_file_exists(&self, path: &str) -> bool {
        let path = Path::new(path);
        path.starts_with(self.manager.gallery().root()) && path.is_file()
    }

    /// Whether run `run`'s sidecar is in the gallery folder.
    fn sidecar_exists(&self, run: &RunDto) -> bool {
        self.manager
            .gallery()
            .sidecar_path(run.created_at, &run.id)
            .is_file()
    }

    /// Whether the gallery's `.partial` folder is empty (or gone).
    fn partial_is_empty(&self) -> bool {
        std::fs::read_dir(self.manager.gallery().partial_dir())
            .map_or(true, |mut entries| entries.next().is_none())
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

    /// A picture run made today through the endpoint that finished costing `usd`.
    async fn seed_spent_today(&self, usd: f64) {
        let id = uuid::Uuid::new_v4().to_string();
        let now = SystemClock.now_ms();
        self.store
            .insert_creation_run(store::NewCreationRun {
                id: id.clone(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(store::ProviderId(self.provider_id)),
                provider_name: "My OpenRouter".into(),
                provider_host: "127.0.0.1".into(),
                model: FLUX.into(),
                model_label: "FLUX 3 Image".into(),
                prompt: "an earlier picture".into(),
                params: json!({"params": {}, "extra": {}, "routing": null}),
                refs: json!([]),
                estimate_usd: Some(usd),
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at: now,
            })
            .await
            .unwrap();
        self.store
            .update_creation_run(
                &id,
                store::CreationPatch {
                    status: Some(store::CreationStatus::Done),
                    cost_usd: Some(Some(usd)),
                    started_at: Some(Some(now)),
                    finished_at: Some(Some(now)),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }

    /// How many times the chat model was unloaded.
    fn chat_unloads(&self) -> usize {
        self.chat.as_ref().expect("a local harness").unloads()
    }
}

/// A manager as the app makes one at start-up (`lib.rs`), on `store`, the keychain
/// `secrets`, the gallery folder `gallery` and the webview `sink`, with runs on this computer
/// when `local` is set; `managers` shuts it down when the test ends.
async fn make_manager(
    store: &store::Store,
    secrets: &Arc<dyn store::secrets::SecretStore>,
    sink: &Arc<RecordingSink>,
    gallery: &Path,
    local: Option<LocalLane>,
    managers: &Managers,
) -> StudioManager {
    let manager = StudioManager::new(
        StudioDeps {
            store: store.clone(),
            providers: Arc::new(StoreProviders {
                store: store.clone(),
                secrets: Arc::clone(secrets),
            }),
            sink: sink.clone(),
            fetch: Arc::new(GuardedFetch(search::Fetcher::new("studio-wire-test"))),
            clock: Arc::new(SystemClock),
            default_gallery: gallery.to_owned(),
            thumbs: None,
            limits: MediaLimits::default(),
            poll: PollPlan {
                first: Duration::from_millis(20),
                second: Duration::from_millis(20),
                third: Duration::from_millis(20),
                ..PollPlan::default()
            },
            local,
        },
        prefs::load(store).await,
    );
    managers.keep(&manager);
    manager
}

/// Returns once `done` holds (at most 10 s).
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "still waiting: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Whether a run with `status` has ended.
fn ended(status: &str) -> bool {
    matches!(
        status,
        "done" | "failed" | "blocked" | "cancelled" | "interrupted"
    )
}

// ---- the tests --------------------------------------------------------------------------------

#[tokio::test]
async fn a_picture_and_a_video_through_a_mock_openrouter() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;
    h.refresh_lists().await;
    assert_eq!(h.models().await.len(), 101);
    // The pick that fetches a picture price.
    h.refresh_price(FLUX).await;
    // $0.048: no confirmation.
    let pic = h
        .manager
        .start(h.picture(FLUX, json!({"resolution": "1K"})), false)
        .await
        .unwrap();
    let pic = h.wait(&pic.id, "done").await;
    assert_eq!(pic.outputs.len(), 1);
    assert!(h.gallery_file_exists(&pic.outputs[0].path) && h.sidecar_exists(&pic));
    assert_eq!(pic.cost_usd, Some(0.048));
    assert_eq!(h.sink.statuses(&pic.id), ["queued", "sending", "done"]);

    let vid = h
        .manager
        .start(
            h.video(GROK_LITE, json!({"duration": 4, "resolution": "480p"})),
            true,
        )
        .await
        .unwrap();
    h.wait(&vid.id, "waiting").await;
    // The first manager follows the job and finds it pending: only the manager after the
    // restart can fetch its result.
    until("the first manager checks the job", || {
        mock.count("GET", JOB_PATH) > 0
    })
    .await;
    assert_eq!(mock.count("GET", CONTENT), 0);
    let h = h.restart().await;
    h.manager.resume_interrupted().await;
    let vid = h.wait(&vid.id, "done").await;
    assert_eq!(vid.outputs.len(), 1);
    assert_eq!(vid.outputs[0].mime, "video/mp4");
    assert!(h.gallery_file_exists(&vid.outputs[0].path));
    assert_eq!(mock.count("POST", "/api/v1/images"), 1);
    assert_eq!(mock.count("POST", "/api/v1/videos"), 1);
    assert_eq!(mock.count("GET", CONTENT), 1);
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.408).abs() < 1e-9, "{spent}");
    let rows = h.audit_rows().await;
    assert_eq!(rows.len(), 2);
    assert!(
        rows.iter()
            .all(|r| !r.arguments.to_string().contains(&h.prompt))
    );
}

#[tokio::test]
async fn opening_studio_makes_no_request() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;
    h.save_lists().await;
    // What opening Studio reads: the saved lists, the runs, today's spending and the
    // preferences.
    assert_eq!(h.models().await.len(), 101);
    let page = commands::runs(&h.manager, RunsFilter::default(), None, 50)
        .await
        .unwrap();
    assert!(page.runs.is_empty());
    assert!(h.manager.live().await.unwrap().is_empty());
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    assert_eq!(prefs::load(&h.store).await, StudioPrefs::default());
    assert!(mock.requests().is_empty(), "{:?}", mock.requests());
}

#[tokio::test]
async fn a_video_past_the_daily_limit_never_reaches_the_mock() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;
    h.refresh_lists().await;
    h.seed_spent_today(9.95).await;
    // 4 s at 480p with no start frame.
    let req = h.video(GROK_LITE, json!({"duration": 4, "resolution": "480p"}));
    let estimate = h.manager.estimate(&req).await.unwrap();
    let usd = estimate.usd.expect("a price");
    assert!((usd - 0.08).abs() < 1e-9, "{usd}");
    assert!(!estimate.limit.would_pass);
    let err = h.manager.start(req, true).await.unwrap_err();
    assert_eq!(err.code, "studio_daily_limit");
    assert_eq!(err.message, "Daily limit reached: $9.95 of $10 spent today");
    assert_eq!(mock.count("POST", "/api/v1/videos"), 0);
    assert!(h.manager.live().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_local_picture_through_the_fake_engine_sends_nothing_and_costs_nothing() {
    // Running, so any request would be counted.
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::with_local(&mock, "ok").await;
    let run = h
        .manager
        .start(
            h.local(KLEIN, json!({"aspect_ratio": "1:1", "n": 2})),
            false,
        )
        .await
        .unwrap();
    let run = h.wait(&run.id, "done").await;
    assert_eq!(
        (run.runs_on.as_str(), run.cost_usd, run.outputs.len()),
        ("local", Some(0.0), 2)
    );
    for o in &run.outputs {
        assert!(h.gallery_file_exists(&o.path));
        assert!(
            label::read_label(Path::new(&o.path), "image/png")
                .unwrap()
                .contains(label::IPTC_TRAINED)
        );
    }
    assert!(mock.requests().is_empty(), "{:?}", mock.requests());
    assert_eq!(h.manager.usage().await.unwrap().spent_today_usd, 0.0);
    let row = h.audit_rows().await.pop().unwrap();
    assert_eq!(row.arguments["runsOn"], "local");
    assert!(!row.arguments.to_string().contains(&h.prompt));
}

#[tokio::test]
async fn a_local_run_and_a_cloud_run_go_side_by_side_and_stop_works() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::with_local(&mock, "hang").await;
    let local = h
        .manager
        .start(h.local(KLEIN, json!({"aspect_ratio": "1:1"})), false)
        .await
        .unwrap();
    let drawing = h
        .wait_for(&local.id, "running", |run| {
            run.local.as_ref().and_then(|l| l.stage.as_deref()) == Some("drawing")
        })
        .await;
    assert_eq!(drawing.status, "running");
    // It did not fit beside the chat model, so the chat model made room.
    let shown = drawing.local.unwrap();
    assert!(!shown.alongside);
    assert!(shown.unloaded_chat.is_some());

    // A cloud picture finishes meanwhile.
    h.refresh_lists().await;
    h.refresh_price(FLUX).await;
    let pic = h
        .manager
        .start(h.picture(FLUX, json!({"resolution": "1K"})), false)
        .await
        .unwrap();
    let pic = h.wait(&pic.id, "done").await;
    assert_eq!(pic.outputs.len(), 1);
    assert_eq!(mock.count("POST", "/api/v1/images"), 1);
    assert_eq!(h.run(&local.id).await.status, "running");

    let at = Instant::now();
    let stopped = h.manager.cancel(&local.id).await.unwrap();
    assert!(at.elapsed() < Duration::from_secs(1), "{:?}", at.elapsed());
    assert_eq!(stopped.status, "cancelled");
    let error = stopped.error.unwrap();
    assert!(
        error.message.contains("Nothing was charged"),
        "{}",
        error.message
    );
    assert_eq!(error.charged, Some(false));
    assert!(h.partial_is_empty());
    assert_eq!(h.chat_unloads(), 1);
}

/// Design 9.2: a model that may not fit is a warning, never a refusal. A 2048 × 2048 FLUX.2
/// klein picture fits nowhere here (not beside the 1 GB chat model on a card with nothing
/// else free, not after its unload, not on the processor with 1 GB of memory): the app's
/// lease unloads the chat model, and the engine runs it.
#[tokio::test]
async fn a_local_run_that_fits_nowhere_unloads_the_chat_model_and_runs() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::with_local_on(&mock, "ok", 0, 1, 1).await;
    let big = json!({"size": "2048x2048"});
    let estimate = h
        .manager
        .estimate(&h.local(KLEIN, big.clone()))
        .await
        .unwrap();
    let local = estimate.local.unwrap();
    assert_eq!(local.fit, "notHere");
    assert!(local.reason.is_some());
    let run = h.manager.start(h.local(KLEIN, big), false).await.unwrap();
    let done = h.wait(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert!(done.error.is_none());
    assert_eq!(h.chat_unloads(), 1);
}

/// Ruling S11-ask-honoured: under "Ask before unloading", a run that fits nowhere asks like
/// any other unload, through the app's lease. While the question waits the run still holds
/// its model (*Delete* and a set-up change are refused). Keep: the chat model stays and the
/// run ends with "the chat model stays loaded", never charged. Unload: it runs.
#[tokio::test]
async fn under_ask_a_local_run_that_fits_nowhere_asks_first() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::with_local_on(&mock, "ok", 0, 1, 1).await;
    let ask: prefs::StudioPrefsPatch = serde_json::from_value(json!({"gpuPolicy": "ask"})).unwrap();
    h.manager.set_prefs(ask).await.unwrap();
    let big = json!({"size": "2048x2048"});
    let asking =
        |run: &RunDto| run.local.as_ref().and_then(|l| l.waiting_for.as_deref()) == Some("answer");

    let run = h
        .manager
        .start(h.local(KLEIN, big.clone()), false)
        .await
        .unwrap();
    let waiting = h.wait_for(&run.id, "asking", asking).await;
    assert_eq!(waiting.status, "queued");
    assert!(h.manager.local_uses_model(KLEIN));
    assert_eq!(h.chat_unloads(), 0);
    h.manager.answer_unload(&run.id, false).await.unwrap();
    let kept = h.wait(&run.id, "failed").await;
    let error = kept.error.unwrap();
    assert_eq!(
        (error.code.as_str(), error.charged),
        ("studio_chat_model_kept", Some(false))
    );
    assert!(!h.manager.local_uses_model(KLEIN));
    assert_eq!(h.chat_unloads(), 0);

    let run = h.manager.start(h.local(KLEIN, big), false).await.unwrap();
    h.wait_for(&run.id, "asking", asking).await;
    assert!(h.manager.local_uses_model(KLEIN));
    h.manager.answer_unload(&run.id, true).await.unwrap();
    let done = h.wait(&run.id, "done").await;
    assert_eq!(done.outputs.len(), 1);
    assert_eq!(h.chat_unloads(), 1);
    assert!(!h.manager.local_uses_model(KLEIN));
}

#[tokio::test]
async fn export_then_delete_leaves_the_audit_rows_and_the_spending() {
    let mock = MockOpenRouter::start().await;
    let h = WireHarness::new(&mock).await;
    h.refresh_lists().await;
    h.refresh_price(FLUX).await;
    let pic = h
        .manager
        .start(h.picture(FLUX, json!({"resolution": "1K"})), false)
        .await
        .unwrap();
    let pic = h.wait(&pic.id, "done").await;
    let output = &pic.outputs[0];
    let kept = std::fs::read(&output.path).unwrap();

    let folder = h.scratch.path().join("exported");
    std::fs::create_dir_all(&folder).unwrap();
    let target = folder.join("kite.png");
    let saved = export(&h.manager, &output.id, target.to_str().unwrap(), false, &[])
        .await
        .unwrap();
    assert_eq!(std::fs::read(&saved).unwrap(), kept);

    h.manager
        .delete_outputs(vec![output.id.clone()])
        .await
        .unwrap();
    assert!(!Path::new(&output.path).exists());
    assert!(!h.sidecar_exists(&pic));
    let page = commands::runs(&h.manager, RunsFilter::default(), None, 50)
        .await
        .unwrap();
    assert!(page.runs.is_empty());
    // The copy is the user's: deleting the result leaves it.
    assert!(Path::new(&saved).is_file());
    let rows = h.audit_rows().await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "picture");
    let spent = h.manager.usage().await.unwrap().spent_today_usd;
    assert!((spent - 0.048).abs() < 1e-9, "{spent}");
}
