//! Studio's read commands (0.1.0).
//!
//! Each command checks that the main window called it ([`only_main_window`]), then calls a
//! plain function its tests call directly, or the [`StudioManager`] method of the same
//! name. They read the store, the gallery folder and the manager's held preferences; only
//! [`studio_refresh_models`] and [`studio_model_price`] ask the company, and only when the
//! user pressed for it. A run's new state comes as the `studio-progress` event
//! ([`super::deps::PROGRESS_EVENT`]); [`studio_runs`] and [`studio_live`] read it again, as
//! events are not replayed.
//!
//! A result's bytes go to the webview whole ([`studio_output_bytes`]), at most
//! [`SHOW_CAP`]: above that a video would sit in memory four times over (here, in the copy
//! to the webview, in its buffer and in its blob), so it is opened from its folder instead.

use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use agent::media::{Company, MediaError, MediaKind, RequestPreview};
use tauri::State;

use super::catalog::{self, EndpointModels};
use super::errors;
use super::gallery::{GalleryError, io_error};
use super::manager::{StudioManager, creation_kind};
use super::prefs::StudioPrefs;
use crate::commands::only_main_window;
use crate::dto::{
    EstimateDto, MediaModelDto, RunDto, RunsCursorDto, RunsFilter, RunsPage, StartRequest,
    StudioEndpointDto, StudioModelsDto, UsageDto, media_model_dto,
};
use crate::error::{AppError, AppResult};
use crate::providers::may_hold_key;
use crate::state::AppState;

/// The largest result the viewer gets whole: 256 MiB.
pub const SHOW_CAP: u64 = 256 * 1024 * 1024;

/// The most runs one page of the gallery holds.
const RUNS_PAGE_MAX: u32 = 500;

// ---- commands ---------------------------------------------------------------------------------

/// Every endpoint that makes pictures or video, with the models of its saved lists. Sends
/// nothing.
#[tauri::command]
pub async fn studio_models(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<StudioModelsDto> {
    only_main_window(window.label())?;
    models(&studio, |id| may_hold_key(state.secrets.as_ref(), id)).await
}

/// *Get the list* and *Update model lists*: fetches endpoint `provider_id`'s picture and
/// video lists, saves them, and gives every endpoint's models.
#[tauri::command]
pub async fn studio_refresh_models(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    studio: State<'_, Arc<StudioManager>>,
    provider_id: i64,
) -> AppResult<StudioModelsDto> {
    only_main_window(window.label())?;
    refresh_models(&studio, provider_id, |id| {
        may_hold_key(state.secrets.as_ref(), id)
    })
    .await
}

/// Picking an OpenRouter picture model, or *Get the price*: fetches its price and gives the
/// model with it.
#[tauri::command]
pub async fn studio_model_price(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    provider_id: i64,
    model: String,
) -> AppResult<MediaModelDto> {
    only_main_window(window.label())?;
    model_price(&studio, provider_id, &model).await
}

/// What a run of `req` would cost and meet. Sends nothing.
#[tauri::command]
pub async fn studio_estimate(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    req: StartRequest,
) -> AppResult<EstimateDto> {
    only_main_window(window.label())?;
    studio.estimate(&req).await
}

/// "See the exact request": what a run of `req` would send, and to which host, with the key
/// removed. Sends nothing.
#[tauri::command]
pub async fn studio_preview(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    req: StartRequest,
) -> AppResult<RequestPreview> {
    only_main_window(window.label())?;
    studio.preview(&req).await
}

/// One page of the gallery's runs, newest first.
#[tauri::command]
pub async fn studio_runs(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    filter: RunsFilter,
    before: Option<RunsCursorDto>,
    limit: u32,
) -> AppResult<RunsPage> {
    only_main_window(window.label())?;
    runs(&studio, filter, before, limit).await
}

/// The runs still working, oldest first.
#[tauri::command]
pub async fn studio_live(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<Vec<RunDto>> {
    only_main_window(window.label())?;
    studio.live().await
}

/// The bytes of output `output_id`, or of its thumbnail, for a `blob:` URL; a result over
/// [`SHOW_CAP`] is refused (`studio_too_big_to_show`).
#[tauri::command]
pub async fn studio_output_bytes(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
    thumb: bool,
) -> AppResult<tauri::ipc::Response> {
    only_main_window(window.label())?;
    let bytes = output_bytes(&studio, &output_id, thumb).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// The bytes of the added picture named `sha256`.
#[tauri::command]
pub async fn studio_ref_bytes(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    sha256: String,
) -> AppResult<tauri::ipc::Response> {
    only_main_window(window.label())?;
    let bytes = ref_bytes(&studio, &sha256).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

/// Today's spending, the limit, and what the gallery holds.
#[tauri::command]
pub async fn studio_usage(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<UsageDto> {
    only_main_window(window.label())?;
    studio.usage().await
}

/// Settings → Studio, as the manager holds them.
#[tauri::command]
pub async fn studio_get_prefs(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<StudioPrefs> {
    only_main_window(window.label())?;
    Ok(studio.prefs())
}

// ---- what the commands do ---------------------------------------------------------------------

/// Every endpoint that makes pictures or video, in the order of Settings → Endpoints, with
/// the models of its saved lists; `has_key` says whether an endpoint may have a key. Reads
/// the store only: no client is built and nothing is sent.
pub async fn models(
    studio: &StudioManager,
    has_key: impl Fn(store::ProviderId) -> bool,
) -> AppResult<StudioModelsDto> {
    let store = &studio.deps().store;
    let providers = store.list_providers().await?;
    let endpoints = catalog::models_for(store, &providers, has_key)
        .await
        .iter()
        .map(endpoint_dto)
        .collect();
    Ok(StudioModelsDto { endpoints })
}

/// Fetches endpoint `provider_id`'s picture and video lists through the endpoint (with its
/// key) and saves them, then gives every endpoint's models as [`models`] does. A failure
/// keeps the lists saved before.
pub async fn refresh_models(
    studio: &StudioManager,
    provider_id: i64,
    has_key: impl Fn(store::ProviderId) -> bool,
) -> AppResult<StudioModelsDto> {
    let deps = studio.deps();
    let id = store::ProviderId(provider_id);
    let (media, endpoint) = deps.providers.media(id).await?;
    catalog::refresh(&deps.store, id, &*media, deps.clock.now_ms())
        .await
        .map_err(|e| fetch_error(endpoint.company, e))?;
    models(studio, has_key).await
}

/// Model `model` of endpoint `provider_id`'s saved lists, with its price. An OpenRouter
/// picture's price is fetched (its `/endpoints` answer, with the companies that run it and
/// the extra settings they take) and saved with the lists; any other model's price comes
/// with its list, so nothing is sent for it. A model the saved lists do not have is refused
/// before anything is sent.
pub async fn model_price(
    studio: &StudioManager,
    provider_id: i64,
    model: &str,
) -> AppResult<MediaModelDto> {
    let deps = studio.deps();
    let id = store::ProviderId(provider_id);
    let (listed, saved) = catalog::model(&deps.store, id, model)
        .await
        .ok_or_else(errors::no_endpoint)?;
    if listed.company != Company::OpenRouter || saved.model.kind != MediaKind::Image {
        return Ok(media_model_dto(&saved.model, saved.price_fetched_at));
    }
    let (media, endpoint) = deps.providers.media(id).await?;
    catalog::refresh_price(&deps.store, id, &*media, model, deps.clock.now_ms())
        .await
        .map_err(|e| fetch_error(endpoint.company, e))?;
    let (_, priced) = catalog::model(&deps.store, id, model)
        .await
        .ok_or_else(errors::no_endpoint)?;
    Ok(media_model_dto(&priced.model, priced.price_fetched_at))
}

/// The runs `filter` selects, newest first, at most `limit` of them (1 to
/// [`RUNS_PAGE_MAX`]), starting after `before` (the previous page's `next`).
pub async fn runs(
    studio: &StudioManager,
    filter: RunsFilter,
    before: Option<RunsCursorDto>,
    limit: u32,
) -> AppResult<RunsPage> {
    let limit = limit.clamp(1, RUNS_PAGE_MAX);
    let query = store::CreationQuery {
        kind: filter.kind.map(creation_kind),
        starred: filter.starred,
        text: Some(filter.text).filter(|text| !text.trim().is_empty()),
        model: filter.model,
        since: filter.since,
        until: filter.until,
        before: before.map(Into::into),
        limit,
    };
    let rows = studio.deps().store.list_creation_runs(query).await?;
    let full = rows.len() >= limit as usize;
    let next = rows.last().filter(|_| full).map(|last| RunsCursorDto {
        created_at: last.created_at,
        id: last.id.clone(),
    });
    let mut runs = Vec::with_capacity(rows.len());
    for run in &rows {
        runs.push(studio.run_dto(run).await);
    }
    Ok(RunsPage { runs, next })
}

/// The bytes of output `output_id` (`thumb`: of its thumbnail), from the path recorded for
/// it. An output recorded, or found, larger than [`SHOW_CAP`] is refused with
/// `studio_too_big_to_show`; its thumbnail is not.
pub async fn output_bytes(
    studio: &StudioManager,
    output_id: &str,
    thumb: bool,
) -> AppResult<Vec<u8>> {
    let output = studio
        .deps()
        .store
        .get_creation_output(output_id)
        .await?
        .ok_or_else(|| AppError::not_found("result"))?;
    let path = if thumb {
        output
            .thumb_path
            .ok_or_else(|| AppError::not_found("thumbnail"))?
    } else {
        if output.size_bytes > SHOW_CAP {
            return Err(errors::too_big_to_show());
        }
        output.path
    };
    tokio::task::spawn_blocking(move || read_shown(Path::new(&path)))
        .await
        .map_err(AppError::internal)?
}

/// The bytes of the added picture named `sha256` in the gallery folder's `refs/`. A name
/// that is not a SHA-256 is refused before anything is looked up.
pub async fn ref_bytes(studio: &StudioManager, sha256: &str) -> AppResult<Vec<u8>> {
    let gallery = studio.gallery();
    let sha256 = sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let (path, _) = gallery.ref_file(&sha256).map_err(|e| match e {
            GalleryError::BadName => errors::bad_ref_name(),
            e => errors::from_gallery(&e),
        })?;
        read_shown(&path)
    })
    .await
    .map_err(AppError::internal)?
}

/// `endpoint` as the picker shows it.
fn endpoint_dto(endpoint: &EndpointModels) -> StudioEndpointDto {
    StudioEndpointDto {
        provider_id: endpoint.provider.id.0,
        name: endpoint.provider.name.clone(),
        company: endpoint.company,
        host: endpoint.host.clone(),
        cloud: endpoint.cloud,
        has_key: endpoint.has_key,
        fetched_at: endpoint.fetched_at,
        models: endpoint
            .models
            .iter()
            .map(|m| media_model_dto(&m.model, m.price_fetched_at))
            .collect(),
    }
}

/// A failed fetch of a model list or a price, in Studio's words.
fn fetch_error(company: Company, e: MediaError) -> AppError {
    match e {
        MediaError::Answered(failure) => errors::from_failure(company, &failure),
        MediaError::NotSent(e) | MediaError::Unknown(e) => AppError {
            details: e.to_string(),
            ..errors::not_fetched(company)
        },
        // Saving the lists failed.
        MediaError::Local(e) => AppError::internal(e),
    }
}

/// A gallery file's bytes for the viewer: refused with `studio_too_big_to_show` when the
/// file holds more than [`SHOW_CAP`] bytes, whatever was recorded for it.
fn read_shown(path: &Path) -> AppResult<Vec<u8>> {
    let unreachable = |e: std::io::Error| errors::from_gallery(&io_error(e));
    let file = std::fs::File::open(path).map_err(unreachable)?;
    if file.metadata().map_err(unreachable)?.len() > SHOW_CAP {
        return Err(errors::too_big_to_show());
    }
    let mut bytes = Vec::new();
    // A file that grew since is read no further than one byte past the cap.
    file.take(SHOW_CAP + 1)
        .read_to_end(&mut bytes)
        .map_err(unreachable)?;
    if bytes.len() as u64 > SHOW_CAP {
        return Err(errors::too_big_to_show());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    //! The errors these commands give are checked beside their constructors, in `errors.rs`:
    //! `studio_errors_have_four_parts` and
    //! `the_safety_message_names_the_company_the_stage_and_the_charge`.

    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agent::ProviderError;
    use agent::media::{
        Company, Fetched, JobDone, JobState, MediaError, MediaKind, MediaLimits, MediaModel,
        MediaProvider, MediaRequest, ParamKind, Price, PriceRow, RawCatalog, RequestPreview,
        Started, Table,
    };
    use async_trait::async_trait;
    use jiff::tz::TimeZone;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use url::Url;

    use super::*;
    use crate::dto::{MediaModelDto, RunDto, RunsCursorDto, RunsFilter, StudioEndpointDto};
    use crate::studio::catalog::{CachedCatalog, cache_key};
    use crate::studio::deps::{Clock, Endpoint, ForeignFetch, ProviderSource, StudioSink};
    use crate::studio::manager::{PollPlan, StudioDeps, StudioManager};
    use crate::studio::prefs::StudioPrefs;
    use crate::studio::spend::day_start_in;
    use crate::studio::test_support::{Scratch, mp4, png};

    const IMAGES: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-images-2026-11-17.json"
    );
    const VIDEOS: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-videos-2026-11-17.json"
    );
    const ENDPOINTS: &str = include_str!(
        "../../../crates/agent/tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json"
    );
    const OPENAI_MODELS: &str =
        include_str!("../../../crates/agent/tests/fixtures/media/openai-models.json");
    const FLUX: &str = "black-forest-labs/flux-3-image";
    const VEO: &str = "google/veo-3.1";
    const DAY: i64 = 86_400_000;
    const HOUR: i64 = 3_600_000;

    /// Noon on 17 November 2026 on this computer's clock.
    fn now() -> i64 {
        jiff::civil::date(2026, 11, 17)
            .at(12, 0, 0, 0)
            .to_zoned(TimeZone::system())
            .unwrap()
            .timestamp()
            .as_millisecond()
    }

    /// OpenRouter's two recorded lists, as `fetch_catalog` gives them.
    fn recorded_lists() -> RawCatalog {
        RawCatalog {
            images: serde_json::from_str(IMAGES).unwrap(),
            videos: Some(serde_json::from_str(VIDEOS).unwrap()),
        }
    }

    /// OpenRouter as recorded: it answers from the fixtures (or fails as told) and counts
    /// what it is asked.
    struct FakeCompany {
        requests: AtomicUsize,
        fail: Mutex<Option<MediaError>>,
    }

    impl FakeCompany {
        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }

        fn asked(&self) -> Result<(), MediaError> {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.fail.lock().unwrap().take().map_or(Ok(()), Err)
        }
    }

    #[async_trait]
    impl MediaProvider for FakeCompany {
        fn company(&self) -> Company {
            Company::OpenRouter
        }

        fn host(&self) -> String {
            "openrouter.ai".into()
        }

        async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
            self.asked()?;
            Ok(recorded_lists())
        }

        async fn fetch_image_price(&self, model: &str) -> Result<Value, MediaError> {
            self.asked()?;
            let saved: Value = serde_json::from_str(ENDPOINTS).unwrap();
            Ok(saved["answers"][model].clone())
        }

        fn preview(&self, _req: &MediaRequest) -> RequestPreview {
            unreachable!("the read commands never preview through the company here")
        }

        async fn start(&self, _req: &MediaRequest, _scratch: &Path) -> Result<Started, MediaError> {
            unreachable!("the read commands never start a run")
        }

        async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
            unreachable!("the read commands never check a job")
        }

        async fn download(
            &self,
            _remote_id: &str,
            _job: &JobDone,
            _index: usize,
            _dest: &Path,
        ) -> Result<Fetched, MediaError> {
            unreachable!("the read commands never download a result")
        }
    }

    /// The one OpenRouter endpoint, run by [`FakeCompany`]; counts `media()` calls.
    struct CountingSource {
        company: Arc<FakeCompany>,
        endpoint: Endpoint,
        calls: AtomicUsize,
    }

    impl CountingSource {
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ProviderSource for CountingSource {
        async fn media(
            &self,
            id: store::ProviderId,
        ) -> AppResult<(Arc<dyn MediaProvider>, Endpoint)> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if Some(id) != self.endpoint.id {
                return Err(errors::no_endpoint());
            }
            Ok((self.company.clone(), self.endpoint.clone()))
        }

        async fn company(&self, id: store::ProviderId) -> Option<Company> {
            (Some(id) == self.endpoint.id).then_some(Company::OpenRouter)
        }
    }

    struct NoSink;

    impl StudioSink for NoSink {
        fn progress(&self, _run: &RunDto) {}
    }

    struct NoFetch;

    #[async_trait]
    impl ForeignFetch for NoFetch {
        async fn fetch_to_file(
            &self,
            _url: &Url,
            _cap: u64,
            _dest: &Path,
        ) -> Result<String, search::FetchError> {
            unreachable!("the read commands never download a result")
        }
    }

    struct FixedClock(i64);

    impl Clock for FixedClock {
        fn now_ms(&self) -> i64 {
            self.0
        }

        fn day_start_ms(&self, at_ms: i64) -> i64 {
            day_start_in(&TimeZone::system(), at_ms)
        }
    }

    struct H {
        manager: StudioManager,
        providers: Arc<CountingSource>,
        company: Arc<FakeCompany>,
        store: store::Store,
        openrouter: store::ProviderId,
        _scratch: Scratch,
    }

    /// An OpenRouter endpoint whose recorded lists are saved (no picture price yet).
    async fn harness() -> H {
        let h = harness_without_lists().await;
        let lists = CachedCatalog {
            fetched_at: now(),
            raw: recorded_lists(),
            image_prices: Default::default(),
        };
        h.store
            .set_setting(&cache_key(h.openrouter), &lists)
            .await
            .unwrap();
        h
    }

    /// An OpenRouter endpoint with no saved lists.
    async fn harness_without_lists() -> H {
        let store = store::Store::open_in_memory().unwrap();
        let openrouter = store
            .upsert_provider(store::ProviderInput {
                id: None,
                name: "My OpenRouter".into(),
                kind: "openrouter".into(),
                base_url: "https://openrouter.ai/api/v1".into(),
                secret_ref: None,
                enabled: true,
            })
            .await
            .unwrap();
        let company = Arc::new(FakeCompany {
            requests: AtomicUsize::new(0),
            fail: Mutex::new(None),
        });
        let providers = Arc::new(CountingSource {
            company: company.clone(),
            endpoint: Endpoint {
                id: Some(openrouter),
                name: "My OpenRouter".into(),
                host: "openrouter.ai".into(),
                company: Company::OpenRouter,
            },
            calls: AtomicUsize::new(0),
        });
        let scratch = Scratch::new();
        let manager = StudioManager::new(
            StudioDeps {
                store: store.clone(),
                providers: providers.clone(),
                sink: Arc::new(NoSink),
                fetch: Arc::new(NoFetch),
                clock: Arc::new(FixedClock(now())),
                default_gallery: scratch.path().join("gallery"),
                thumbs: None,
                limits: MediaLimits::default(),
                poll: PollPlan::default(),
            },
            StudioPrefs::default(),
        );
        H {
            manager,
            providers,
            company,
            store,
            openrouter,
            _scratch: scratch,
        }
    }

    impl H {
        /// Records a cloud run of `kind` with `prompt`, made at `created_at`.
        async fn seed_run(&self, id: &str, kind: MediaKind, prompt: &str, created_at: i64) {
            let (kind, model) = match kind {
                MediaKind::Image => (store::CreationKind::Image, FLUX),
                MediaKind::Video => (store::CreationKind::Video, VEO),
            };
            self.store
                .insert_creation_run(store::NewCreationRun {
                    id: id.into(),
                    kind,
                    runs_on: store::RunsOn::Cloud,
                    provider_id: Some(self.openrouter),
                    provider_name: "My OpenRouter".into(),
                    provider_host: "openrouter.ai".into(),
                    model: model.into(),
                    model_label: model.into(),
                    prompt: prompt.into(),
                    params: json!({}),
                    refs: json!([]),
                    estimate_usd: None,
                    gen_model_id: None,
                    engine_build: None,
                    device: None,
                    created_at,
                })
                .await
                .unwrap();
        }

        /// Keeps `bytes` as run `run_id`'s first output in the gallery, recorded as
        /// `recorded_size` bytes, and gives its id.
        async fn seed_output(
            &self,
            run_id: &str,
            mime: &str,
            bytes: &[u8],
            recorded_size: u64,
        ) -> String {
            let id = uuid::Uuid::new_v4().to_string();
            let ext = if mime == "video/mp4" { "mp4" } else { "png" };
            let path = self.manager.gallery().output_path(now(), run_id, 0, ext);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes).unwrap();
            self.store
                .insert_creation_output(store::NewCreationOutput {
                    id: id.clone(),
                    run_id: run_id.into(),
                    idx: 0,
                    path: path.display().to_string(),
                    thumb_path: None,
                    mime: mime.into(),
                    size_bytes: recorded_size,
                    width: None,
                    height: None,
                    duration_ms: None,
                    seed: None,
                    created_at: now(),
                })
                .await
                .unwrap();
            id
        }

        /// Writes output `id`'s thumbnail and records it.
        async fn seed_thumb(&self, id: &str, jpeg: &[u8]) {
            let path = self.manager.gallery().write_thumb(id, jpeg).unwrap();
            self.store
                .set_creation_thumb(id, &path.display().to_string())
                .await
                .unwrap();
        }

        /// The ids of the runs `filter` selects, newest first.
        async fn run_ids(&self, filter: RunsFilter) -> Vec<String> {
            runs(&self.manager, filter, None, 60)
                .await
                .unwrap()
                .runs
                .into_iter()
                .map(|run| run.id)
                .collect()
        }
    }

    fn model_in<'a>(endpoint: &'a StudioEndpointDto, id: &str) -> &'a MediaModelDto {
        endpoint
            .models
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("no model {id}"))
    }

    #[tokio::test]
    async fn studio_models_reads_only_the_saved_lists() {
        let h = harness().await;
        let listed = models(&h.manager, |_| true).await.unwrap();
        assert_eq!(listed.endpoints.len(), 1);
        let endpoint = &listed.endpoints[0];
        // 68 pictures and 33 videos.
        assert_eq!(endpoint.models.len(), 101);
        assert_eq!(endpoint.provider_id, h.openrouter.0);
        assert_eq!(endpoint.name, "My OpenRouter");
        assert_eq!(endpoint.company, Company::OpenRouter);
        assert_eq!(endpoint.host, "openrouter.ai");
        assert!(endpoint.cloud);
        assert!(endpoint.has_key);
        assert_eq!(endpoint.fetched_at, Some(now()));
        // A picture's price is shown when it is picked; a video's comes with the list.
        let flux = model_in(endpoint, FLUX);
        assert_eq!(flux.kind, MediaKind::Image);
        assert_eq!(flux.price_fetched_at, None);
        assert!(!flux.price_known);
        assert_eq!(flux.price.unit, "unknown");
        let veo = model_in(endpoint, VEO);
        assert_eq!(veo.kind, MediaKind::Video);
        assert_eq!(veo.price_fetched_at, Some(now()));
        assert!(veo.price_known);
        assert_eq!(veo.price.unit, "second");
        // Nothing was asked of the company, and no client was built.
        assert_eq!(h.providers.calls(), 0);
        assert_eq!(h.company.requests(), 0);
        // Whether an endpoint has a key is the caller's answer.
        let without_key = models(&h.manager, |_| false).await.unwrap();
        assert!(!without_key.endpoints[0].has_key);
        assert_eq!(h.providers.calls(), 0);
    }

    #[tokio::test]
    async fn the_model_lists_are_camel_case() {
        let h = harness().await;
        let listed = models(&h.manager, |_| true).await.unwrap();
        let v = serde_json::to_value(&listed).unwrap();
        let endpoint = &v["endpoints"][0];
        assert_eq!(endpoint["providerId"], json!(h.openrouter.0));
        assert_eq!(endpoint["company"], json!("openrouter"));
        assert_eq!(endpoint["hasKey"], json!(true));
        assert_eq!(endpoint["fetchedAt"], json!(now()));
        let flux = endpoint["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == json!(FLUX))
            .unwrap();
        assert_eq!(flux["kind"], json!("image"));
        assert_eq!(flux["priceKnown"], json!(false));
        assert_eq!(flux["priceFetchedAt"], json!(null));
        assert_eq!(
            flux["price"],
            json!({"unit": "unknown", "low": null, "high": null})
        );
        for key in [
            "displayName",
            "family",
            "acceptsImages",
            "params",
            "companies",
            "notes",
            "usable",
            "created",
        ] {
            assert!(flux.get(key).is_some(), "{key}");
        }
    }

    #[test]
    fn a_price_hint_has_its_unit_and_range() {
        let model = |price| MediaModel {
            id: "maker/model".into(),
            display_name: "Model".into(),
            family: "maker/model".into(),
            kind: MediaKind::Image,
            accepts_images: false,
            params: Vec::new(),
            price,
            notes: Vec::new(),
            usable: None,
            created: None,
            companies: Vec::new(),
        };
        let row = |usd| PriceRow {
            mode: None,
            resolution: None,
            quality: None,
            audio: None,
            usd,
        };
        let table = Table {
            rows: vec![row(0.08), row(0.04), row(0.12)],
            per_input_image_usd: None,
            input_images_unpriced: false,
            minimum_usd: None,
        };
        let hint = |price, fetched| {
            let dto = crate::dto::media_model_dto(&model(price), fetched);
            (
                dto.price.unit,
                dto.price.low,
                dto.price.high,
                dto.price_known,
                dto.price_fetched_at,
            )
        };
        assert_eq!(
            hint(Price::PerImage(table.clone()), Some(5)),
            ("image".into(), Some(0.04), Some(0.12), true, Some(5))
        );
        assert_eq!(
            hint(Price::PerSecond(table), Some(5)),
            ("second".into(), Some(0.04), Some(0.12), true, Some(5))
        );
        assert_eq!(
            hint(Price::PerToken, Some(5)),
            ("token".into(), None, None, false, Some(5))
        );
        // Fetched, but in a form Studio cannot read: "price unknown".
        assert_eq!(
            hint(Price::Unknown, Some(5)),
            ("unknown".into(), None, None, false, Some(5))
        );
        // Not fetched yet: "price shown when picked".
        assert_eq!(
            hint(Price::Unknown, None),
            ("unknown".into(), None, None, false, None)
        );
        // A model on this computer (Task L16).
        assert_eq!(
            hint(Price::Free, None),
            ("free".into(), Some(0.0), Some(0.0), true, None)
        );
    }

    #[tokio::test]
    async fn the_lists_and_a_price_are_fetched_only_when_asked() {
        let h = harness_without_lists().await;
        let listed = models(&h.manager, |_| true).await.unwrap();
        assert_eq!(listed.endpoints[0].fetched_at, None);
        assert!(listed.endpoints[0].models.is_empty());

        let listed = refresh_models(&h.manager, h.openrouter.0, |_| true)
            .await
            .unwrap();
        assert_eq!(h.company.requests(), 1);
        assert_eq!(listed.endpoints[0].fetched_at, Some(now()));
        assert_eq!(listed.endpoints[0].models.len(), 101);

        // Picking an OpenRouter picture asks for its price once, and gives the model with
        // it, the companies that run it and the extra settings they take.
        let flux = model_price(&h.manager, h.openrouter.0, FLUX).await.unwrap();
        assert_eq!(h.company.requests(), 2);
        assert!(flux.price_known);
        assert_eq!(flux.price.unit, "image");
        assert_eq!(flux.price.low, Some(0.041));
        assert_eq!(flux.price.high, Some(0.607));
        assert_eq!(flux.price_fetched_at, Some(now()));
        assert_eq!(flux.companies.len(), 1);
        assert_eq!(flux.companies[0].name, "Black Forest Labs");
        let extra = flux
            .params
            .iter()
            .find_map(|p| match &p.kind {
                ParamKind::Extra { allowed_keys } => Some(allowed_keys.clone()),
                _ => None,
            })
            .unwrap();
        assert!(extra.contains(&"safety_tolerance".to_owned()), "{extra:?}");
        // The price is saved with the lists.
        let listed = models(&h.manager, |_| true).await.unwrap();
        assert!(model_in(&listed.endpoints[0], FLUX).price_known);

        // A video's price comes with the list: nothing is asked.
        let veo = model_price(&h.manager, h.openrouter.0, VEO).await.unwrap();
        assert_eq!(veo.price_fetched_at, Some(now()));
        assert_eq!(h.company.requests(), 2);
        // A model the saved lists do not have is refused before anything is asked.
        let err = model_price(&h.manager, h.openrouter.0, "nobody/nothing")
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_no_endpoint");
        let err = model_price(&h.manager, 9_999, FLUX).await.unwrap_err();
        assert_eq!(err.code, "studio_no_endpoint");
        assert_eq!(h.company.requests(), 2);

        // An OpenAI picture's price comes with its list: no client is built for it.
        let openai = h
            .store
            .upsert_provider(store::ProviderInput {
                id: None,
                name: "My OpenAI".into(),
                kind: "openai".into(),
                base_url: "https://api.openai.com/v1".into(),
                secret_ref: None,
                enabled: true,
            })
            .await
            .unwrap();
        let lists = CachedCatalog {
            fetched_at: now() - HOUR,
            raw: RawCatalog {
                images: serde_json::from_str(OPENAI_MODELS).unwrap(),
                videos: None,
            },
            image_prices: Default::default(),
        };
        h.store
            .set_setting(&cache_key(openai), &lists)
            .await
            .unwrap();
        let calls = h.providers.calls();
        let flare = model_price(&h.manager, openai.0, "gpt-image-2.6-flare")
            .await
            .unwrap();
        assert_eq!(flare.price_fetched_at, Some(now() - HOUR));
        assert_eq!(h.providers.calls(), calls);
    }

    #[tokio::test]
    async fn a_failed_update_says_so_and_keeps_the_saved_lists() {
        let h = harness().await;
        *h.company.fail.lock().unwrap() = Some(MediaError::Unknown(ProviderError::Network(
            "timed out".into(),
        )));
        let err = refresh_models(&h.manager, h.openrouter.0, |_| true)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_not_fetched");
        assert_eq!(
            err.message,
            "OpenRouter could not be reached, or did not answer in time."
        );
        assert!(err.details.contains("timed out"), "{err:?}");
        let listed = models(&h.manager, |_| true).await.unwrap();
        assert_eq!(listed.endpoints[0].models.len(), 101);
        assert_eq!(listed.endpoints[0].fetched_at, Some(now()));
        // An endpoint that is gone.
        let err = refresh_models(&h.manager, 9_999, |_| true)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_no_endpoint");
    }

    #[tokio::test]
    async fn output_bytes_gives_the_thumbnail_when_asked_and_refuses_a_huge_video() {
        let h = harness().await;
        let picture = png(4, 4);
        let thumb = b"\xFF\xD8\xFFa small thumbnail".to_vec();
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        h.seed_thumb(&out, &thumb).await;
        assert_eq!(
            output_bytes(&h.manager, &out, false).await.unwrap(),
            picture
        );
        assert_eq!(output_bytes(&h.manager, &out, true).await.unwrap(), thumb);

        // A video recorded over the cap is not read; its thumbnail still is.
        h.seed_run("v1", MediaKind::Video, "a river", now()).await;
        let big = h.seed_output("v1", "video/mp4", &mp4(), SHOW_CAP + 1).await;
        let err = output_bytes(&h.manager, &big, false).await.unwrap_err();
        assert_eq!(err.code, "studio_too_big_to_show");
        assert_eq!(err.message, "This video is too big to play here.");
        assert_eq!(
            err.next_step,
            "Use Show in folder to open it with your video player."
        );
        h.seed_thumb(&big, &thumb).await;
        assert_eq!(output_bytes(&h.manager, &big, true).await.unwrap(), thumb);

        // A video at the cap or under it plays.
        let small = h.seed_output("v1", "video/mp4", &mp4(), SHOW_CAP).await;
        assert_eq!(
            output_bytes(&h.manager, &small, false).await.unwrap(),
            mp4()
        );

        // No thumbnail made, no such output, a file that is gone.
        assert_eq!(
            output_bytes(&h.manager, &small, true)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        assert_eq!(
            output_bytes(&h.manager, "no-such-output", false)
                .await
                .unwrap_err()
                .code,
            "not_found"
        );
        let gone = h.store.get_creation_output(&out).await.unwrap().unwrap();
        std::fs::remove_file(&gone.path).unwrap();
        assert_eq!(
            output_bytes(&h.manager, &out, false)
                .await
                .unwrap_err()
                .code,
            "studio_files"
        );
    }

    #[tokio::test]
    async fn ref_bytes_refuses_a_name_that_is_not_a_hash() {
        let h = harness().await;
        let upper = "A".repeat(64);
        let escape = format!("{}/..", "a".repeat(61));
        for bad in ["../x", "", upper.as_str(), escape.as_str()] {
            let err = ref_bytes(&h.manager, bad).await.unwrap_err();
            assert_eq!(err.code, "bad_request", "{bad:?}");
        }
        let picture = png(2, 2);
        let sha256: String = Sha256::digest(&picture)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let refs = h.manager.gallery().root().join("refs");
        std::fs::create_dir_all(&refs).unwrap();
        std::fs::write(refs.join(format!("{sha256}.png")), &picture).unwrap();
        assert_eq!(ref_bytes(&h.manager, &sha256).await.unwrap(), picture);
        // A hash with no file kept under it.
        let err = ref_bytes(&h.manager, &"0".repeat(64)).await.unwrap_err();
        assert_eq!(err.code, "studio_files");
    }

    #[tokio::test]
    async fn runs_filter_by_date_text_and_kind() {
        let h = harness().await;
        let day = day_start_in(&TimeZone::system(), now());
        h.seed_run("a", MediaKind::Image, "a red kite", day - DAY + HOUR)
            .await;
        h.seed_run("b", MediaKind::Video, "a red balloon", day + HOUR)
            .await;
        h.seed_run("c", MediaKind::Image, "blue sky", day + 2 * HOUR)
            .await;
        h.seed_run("d", MediaKind::Image, "Red fox", day + DAY)
            .await;

        let all = h.run_ids(RunsFilter::default()).await;
        assert_eq!(all, ["d", "c", "b", "a"]);
        // `since` is the first moment of the range, `until` the first moment after it.
        let today = RunsFilter {
            since: Some(day),
            until: Some(day + DAY),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(today).await, ["c", "b"]);
        let red = RunsFilter {
            text: "red".into(),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(red.clone()).await, ["d", "b", "a"]);
        let videos = RunsFilter {
            kind: Some(MediaKind::Video),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(videos).await, ["b"]);
        let red_pictures = RunsFilter {
            kind: Some(MediaKind::Image),
            ..red
        };
        assert_eq!(h.run_ids(red_pictures).await, ["d", "a"]);
        let by_model = RunsFilter {
            model: Some(VEO.into()),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(by_model).await, ["b"]);
        // Blank text does not filter.
        let blank = RunsFilter {
            text: "  ".into(),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(blank).await.len(), 4);

        // The webview's JSON, with fields left out.
        let filter: RunsFilter =
            serde_json::from_value(json!({"kind": "image", "text": "red", "until": day + DAY}))
                .unwrap();
        assert_eq!(h.run_ids(filter).await, ["a"]);
        let filter: RunsFilter = serde_json::from_value(json!({})).unwrap();
        assert_eq!(filter, RunsFilter::default());
    }

    #[tokio::test]
    async fn runs_come_in_pages() {
        let h = harness().await;
        // Two runs in the same millisecond are neither skipped nor repeated.
        h.seed_run("a", MediaKind::Image, "one", now() - 2).await;
        h.seed_run("b", MediaKind::Image, "two", now() - 1).await;
        h.seed_run("c", MediaKind::Image, "three", now() - 1).await;

        let first = runs(&h.manager, RunsFilter::default(), None, 2)
            .await
            .unwrap();
        let ids: Vec<&str> = first.runs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["c", "b"]);
        let next = first.next.clone().unwrap();
        assert_eq!(
            next,
            RunsCursorDto {
                created_at: now() - 1,
                id: "b".into()
            }
        );
        assert_eq!(
            serde_json::to_value(&first).unwrap()["next"],
            json!({"createdAt": now() - 1, "id": "b"})
        );

        let second = runs(&h.manager, RunsFilter::default(), Some(next), 2)
            .await
            .unwrap();
        let ids: Vec<&str> = second.runs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["a"]);
        // Not a full page: there is no next one.
        assert_eq!(second.next, None);
        // The cursor reads back from the webview's JSON.
        let cursor: RunsCursorDto =
            serde_json::from_value(json!({"createdAt": now() - 1, "id": "b"})).unwrap();
        assert_eq!(cursor.id, "b");
    }
}
