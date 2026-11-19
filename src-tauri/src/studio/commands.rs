//! Studio's commands (0.1.0).
//!
//! Each command checks that the main window called it ([`only_main_window`]), then calls a
//! plain function its tests call directly, or the [`StudioManager`] method of the same
//! name.
//!
//! - **The read commands** read the store, the gallery folder and the manager's held
//!   preferences; only [`studio_refresh_models`] and [`studio_model_price`] ask the company,
//!   and only when the user pressed for it. A run's new state comes as the `studio-progress`
//!   event ([`super::deps::PROGRESS_EVENT`]); [`studio_runs`] and [`studio_live`] read it
//!   again, as events are not replayed. A result's bytes go to the webview whole
//!   ([`studio_output_bytes`]), at most [`SHOW_CAP`]: above that a video would sit in memory
//!   four times over (here, in the copy to the webview, in its buffer and in its blob), so it
//!   is opened from its folder instead.
//! - **The changing commands** start, stop and follow runs (the manager), save a video's
//!   poster frame ([`POSTER_CAP`]), star, delete and export results, open the gallery folder,
//!   add pictures to a prompt, give a picture to a chat ([`for_chat`]) and change the
//!   preferences; for runs on this computer, answer "Unload the chat model?" and *Quit and
//!   stop*.
//!
//! What the gallery actions do (export, a video's poster frame, *Send to chat* and *Use in
//! prompt*) is in [`super::actions`]; their commands here only call it.
//!
//! [`POSTER_CAP`]: super::actions::POSTER_CAP
//! [`for_chat`]: super::actions::for_chat

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use agent::media::{Company, MediaError, MediaKind, RequestPreview};
use tauri::{Manager, State};
use tauri_plugin_opener::OpenerExt;

use super::actions::{export, output_for_chat, output_of, ref_dto, ref_from_output, save_thumb};
use super::catalog::{self, EndpointModels};
use super::errors;
use super::gallery::{GalleryError, io_error, ref_file_in};
use super::manager::{StudioManager, creation_kind, output_dto};
use super::prefs::{StudioPrefs, StudioPrefsPatch};
use crate::attachments::{ImageInput, PictureWorker};
use crate::commands::only_main_window;
use crate::dto::{
    EstimateDto, MediaModelDto, OutputDto, RefDto, RunDto, RunsCursorDto, RunsFilter, RunsPage,
    StartRequest, StudioEndpointDto, StudioModelsDto, UsageDto, media_model_dto,
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

/// Records a run of `req` and queues it once it passes every check; `confirmed` says the
/// user confirmed it.
#[tauri::command]
pub async fn studio_start(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    req: StartRequest,
    confirmed: bool,
) -> AppResult<RunDto> {
    only_main_window(window.label())?;
    studio.start(req, confirmed).await
}

/// Cancel, Stop watching: stops run `run_id`.
#[tauri::command]
pub async fn studio_cancel(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    run_id: String,
) -> AppResult<RunDto> {
    only_main_window(window.label())?;
    studio.cancel(&run_id).await
}

/// *Get it anyway* and *Check again*: checks a video job again and downloads its result;
/// never creates.
#[tauri::command]
pub async fn studio_fetch_anyway(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    run_id: String,
) -> AppResult<RunDto> {
    only_main_window(window.label())?;
    studio.fetch_anyway(&run_id).await
}

/// Saves the poster frame the webview took of a video as output `output_id`'s thumbnail.
#[tauri::command]
pub async fn studio_save_thumb(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
    jpeg_base64: String,
) -> AppResult<OutputDto> {
    only_main_window(window.label())?;
    save_thumb(&studio, &output_id, &jpeg_base64).await
}

/// Stars or unstars output `output_id`.
#[tauri::command]
pub async fn studio_star(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
    starred: bool,
) -> AppResult<OutputDto> {
    only_main_window(window.label())?;
    star(&studio, &output_id, starred).await
}

/// Deletes results; today's spending stays as it is.
#[tauri::command]
pub async fn studio_delete(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_ids: Vec<String>,
) -> AppResult<()> {
    only_main_window(window.label())?;
    studio.delete_outputs(output_ids).await
}

/// *Delete stopped and failed runs*: gives how many went.
#[tauri::command]
pub async fn studio_delete_unfinished(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<u32> {
    only_main_window(window.label())?;
    studio.delete_unfinished().await
}

/// Copies output `output_id` to `path`, the file the user chose in the save dialog, with its
/// settings beside it when `with_settings`; gives where it was written.
#[tauri::command]
pub async fn studio_export(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
    path: String,
    with_settings: bool,
) -> AppResult<String> {
    only_main_window(window.label())?;
    let forbidden: Vec<PathBuf> = crate::permissions::forbidden_for_tools(window.app_handle())
        .into_iter()
        .map(PathBuf::from)
        .collect();
    export(&studio, &output_id, &path, with_settings, &forbidden).await
}

/// *Show in folder*: shows output `output_id`'s file in the file manager.
#[tauri::command]
pub async fn studio_show_in_folder(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
) -> AppResult<()> {
    only_main_window(window.label())?;
    let path = PathBuf::from(output_of(&studio, &output_id).await?.path);
    tokio::fs::metadata(&path)
        .await
        .map_err(|e| errors::from_gallery(&io_error(e)))?;
    let folder = path.parent().unwrap_or(&path).to_owned();
    window
        .app_handle()
        .opener()
        .reveal_item_in_dir(&path)
        .map_err(|e| errors::not_opened(&folder, e.to_string()))
}

/// *Open folder*: opens the gallery folder in the file manager, made first if it is not
/// there yet.
#[tauri::command]
pub async fn studio_open_folder(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<()> {
    only_main_window(window.label())?;
    let root = studio.gallery().root().to_owned();
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| errors::from_gallery(&io_error(e)))?;
    window
        .app_handle()
        .opener()
        .open_path(root.display().to_string(), None::<&str>)
        .map_err(|e| errors::not_opened(&root, e.to_string()))
}

/// Keeps a picture the user added to the prompt in the gallery's `refs/`.
#[tauri::command]
pub async fn studio_add_ref(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    image: ImageInput,
) -> AppResult<RefDto> {
    only_main_window(window.label())?;
    let stored = studio.add_ref(&PictureWorker::app(), &image).await?;
    Ok(ref_dto(stored))
}

/// *Drop a picture* (the native drop, which carries files on Windows too): adds file `index`
/// of the files last dropped on the main window to the prompt, for `role`
/// ([`super::dropped::add_dropped`]). The webview names the file by its place in the drop,
/// never by its path.
#[tauri::command]
pub async fn studio_add_dropped(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    dropped: State<'_, super::dropped::DroppedFiles>,
    index: usize,
    role: Option<agent::media::RefRole>,
) -> AppResult<RefDto> {
    only_main_window(window.label())?;
    super::dropped::add_dropped(
        &studio,
        &PictureWorker::app(),
        &dropped,
        index,
        role,
        std::time::Instant::now(),
    )
    .await
}

/// *Use in prompt*: adds picture output `output_id` to the prompt.
#[tauri::command]
pub async fn studio_ref_from_output(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
) -> AppResult<RefDto> {
    only_main_window(window.label())?;
    ref_from_output(&studio, &PictureWorker::app(), &output_id).await
}

/// *Send to chat*: picture output `output_id` as the composer takes a picture.
#[tauri::command]
pub async fn studio_output_for_chat(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    output_id: String,
) -> AppResult<ImageInput> {
    only_main_window(window.label())?;
    output_for_chat(&studio, &output_id).await
}

/// Settings → Studio: changes the fields `patch` sends, and gives the preferences as held.
#[tauri::command]
pub async fn studio_set_prefs(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    patch: StudioPrefsPatch,
) -> AppResult<StudioPrefs> {
    only_main_window(window.label())?;
    studio.set_prefs(patch).await
}

/// "Unload the chat model?" under Settings → Studio's Ask: the answer for local run `run_id`.
#[tauri::command]
pub async fn studio_answer_unload(
    window: tauri::WebviewWindow,
    studio: State<'_, Arc<StudioManager>>,
    run_id: String,
    unload: bool,
) -> AppResult<RunDto> {
    only_main_window(window.label())?;
    studio.answer_unload(&run_id, unload).await
}

/// *Quit and stop*, asked while a local job works: stops the local runs, then quits. The exit
/// carries a code, so it is not asked again (`quit::ask_before_quit`).
#[tauri::command]
pub async fn studio_quit_now(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    studio: State<'_, Arc<StudioManager>>,
) -> AppResult<()> {
    only_main_window(window.label())?;
    studio.stop_local().await;
    app.exit(0);
    Ok(())
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
        text: Some(filter.text.trim().to_owned()).filter(|text| !text.is_empty()),
        model: filter.model,
        since: filter.since,
        until: filter.until,
        before: before.map(Into::into),
        limit,
    };
    let seq = studio.next_seq();
    let rows = studio.deps().store.list_creation_runs(query).await?;
    let full = rows.len() >= limit as usize;
    let next = rows.last().filter(|_| full).map(|last| RunsCursorDto {
        created_at: last.created_at,
        id: last.id.clone(),
    });
    let mut runs = Vec::with_capacity(rows.len());
    for run in &rows {
        runs.push(studio.run_dto(run, seq).await);
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

/// The bytes of the added picture named `sha256` in the gallery folder's `refs/`, or an
/// earlier gallery folder's (Settings → Studio moved the gallery since it was added). A name
/// that is not a SHA-256 is refused before anything is looked up.
pub async fn ref_bytes(studio: &StudioManager, sha256: &str) -> AppResult<Vec<u8>> {
    let galleries = studio.galleries().await;
    let sha256 = sha256.to_owned();
    tokio::task::spawn_blocking(move || {
        let (path, _) = ref_file_in(&galleries, &sha256).map_err(|e| match e {
            GalleryError::BadName => errors::bad_ref_name(),
            e => errors::from_gallery(&e),
        })?;
        read_shown(&path)
    })
    .await
    .map_err(AppError::internal)?
}

/// Stars (or unstars) output `output_id` and gives it.
pub async fn star(studio: &StudioManager, output_id: &str, starred: bool) -> AppResult<OutputDto> {
    let mut output = output_of(studio, output_id).await?;
    studio
        .deps()
        .store
        .set_creation_starred(&output.id, starred)
        .await?;
    output.starred = starred;
    Ok(output_dto(&output))
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

/// A failed fetch of a model list or a price, in Studio's words: a read's, never a run's.
fn fetch_error(company: Company, e: MediaError) -> AppError {
    match e {
        MediaError::Answered(failure) => errors::from_read_failure(company, &failure),
        MediaError::NotSent(e) | MediaError::Unknown(e) => AppError {
            details: e.to_string(),
            ..errors::not_fetched(company)
        },
        // Saving the lists failed.
        MediaError::Local(e) => errors::lists_not_saved(e.to_string()),
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
pub(crate) mod tests {
    //! The errors these commands give are checked beside their constructors, in `errors.rs`:
    //! `studio_errors_have_four_parts` and
    //! `the_safety_message_names_the_company_the_stage_and_the_charge`.

    use std::path::Path;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use agent::ProviderError;
    use agent::media::{
        Company, FailureKind, Fetched, JobDone, JobState, LocalIo, MediaError, MediaFailure,
        MediaKind, MediaLimits, MediaModel, MediaProvider, MediaRequest, ParamKind, Price,
        PriceRow, RawCatalog, RequestPreview, Started, Table,
    };
    use async_trait::async_trait;
    use jiff::tz::TimeZone;
    use models::catalog::Catalog;
    use models::download::destination;
    use models::fit::Target;
    use models::gen_catalog::{GenCatalog, gen_catalog};
    use runtime::sd::engine::{EngineLayout, program_file_name};
    use runtime::sd::lock as sd_lock;
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use url::Url;

    use super::*;
    use crate::artifacts::export_target;
    use crate::attachments::PictureWorker;
    use crate::dto::{MediaModelDto, RunDto, RunsCursorDto, RunsFilter, StudioEndpointDto};
    use crate::studio::actions::{POSTER_CAP, for_chat};
    use crate::studio::catalog::{CachedCatalog, CachedPrice, cache_key};
    use crate::studio::deps::{
        Clock, Endpoint, ForeignFetch, LocalHardware, ProviderSource, StudioSink,
    };
    use crate::studio::engine_cmd::{EngineNeed, status_from};
    use crate::studio::local::GenCatalogSource;
    use crate::studio::local_cmd::{LocalCmdDeps, local_models};
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
    pub(crate) fn now() -> i64 {
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

    pub(crate) struct H {
        pub(crate) manager: StudioManager,
        providers: Arc<CountingSource>,
        company: Arc<FakeCompany>,
        pub(crate) store: store::Store,
        openrouter: store::ProviderId,
        _scratch: Scratch,
    }

    /// An OpenRouter endpoint whose recorded lists are saved (no picture price yet).
    pub(crate) async fn harness() -> H {
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
                local: None,
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
        pub(crate) async fn seed_run(
            &self,
            id: &str,
            kind: MediaKind,
            prompt: &str,
            created_at: i64,
        ) {
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
        pub(crate) async fn seed_output(
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
    async fn a_list_or_price_the_company_does_not_give_is_worded_as_a_read() {
        let h = harness().await;
        let answered = |kind| {
            MediaError::Answered(MediaFailure {
                kind,
                stage: None,
                charged: None,
                company_code: None,
                request_id: None,
                status: Some(404),
                message: "No endpoints found".into(),
                retry_after: None,
            })
        };
        *h.company.fail.lock().unwrap() = Some(answered(FailureKind::Other));
        let err = refresh_models(&h.manager, h.openrouter.0, |_| true)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_not_fetched");
        assert_eq!(
            err.message,
            "OpenRouter could not give the model list or price."
        );
        assert!(err.details.contains("No endpoints found"), "{err:?}");

        *h.company.fail.lock().unwrap() = Some(answered(FailureKind::NotFound));
        let err = model_price(&h.manager, h.openrouter.0, FLUX)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_not_fetched");
        assert_eq!(
            err.message,
            "OpenRouter could not give the model list or price."
        );
        assert!(err.details.contains("HTTP status: 404"), "{err:?}");

        // The lists could not be kept on this computer.
        *h.company.fail.lock().unwrap() =
            Some(MediaError::Local(LocalIo::Io("database is locked".into())));
        let err = refresh_models(&h.manager, h.openrouter.0, |_| true)
            .await
            .unwrap_err();
        assert_eq!(err.code, "studio_not_saved");
        assert_eq!(
            err.message,
            "The model list or price could not be saved on this computer."
        );
        // The lists saved before stay.
        let listed = models(&h.manager, |_| true).await.unwrap();
        assert_eq!(listed.endpoints[0].models.len(), 101);
    }

    #[tokio::test]
    async fn search_text_is_matched_without_its_outer_spaces() {
        let h = harness().await;
        h.seed_run("a", MediaKind::Image, "a red kite", now() - 2)
            .await;
        h.seed_run("d", MediaKind::Image, "Red fox", now() - 1)
            .await;
        let padded = RunsFilter {
            text: " red ".into(),
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(padded).await, ["d", "a"]);
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

    // ---- the changing commands -------------------------------------------------------------

    /// Every Studio command.
    const STUDIO_COMMANDS: &[&str] = &[
        // The read commands.
        "studio_models",
        "studio_refresh_models",
        "studio_model_price",
        "studio_estimate",
        "studio_preview",
        "studio_runs",
        "studio_live",
        "studio_output_bytes",
        "studio_ref_bytes",
        "studio_usage",
        "studio_get_prefs",
        // The changing commands.
        "studio_start",
        "studio_cancel",
        "studio_fetch_anyway",
        "studio_save_thumb",
        "studio_star",
        "studio_delete",
        "studio_delete_unfinished",
        "studio_export",
        "studio_show_in_folder",
        "studio_open_folder",
        "studio_add_ref",
        "studio_add_dropped",
        "studio_ref_from_output",
        "studio_output_for_chat",
        "studio_set_prefs",
        "studio_answer_unload",
        "studio_quit_now",
        // The Studio engine (studio::engine_cmd).
        "studio_engine_status",
        "studio_engine_download",
        "studio_engine_cancel",
        "studio_engine_remove",
        // Get local models (studio::local_cmd).
        "studio_local_models",
        "studio_local_search",
        "studio_local_repo",
        "studio_local_fit",
        "studio_local_download",
        "studio_local_delete",
        "studio_accept_local_notice",
    ];

    /// The modules Studio's commands live in.
    const STUDIO_COMMAND_MODULES: &[&str] = &["commands", "engine_cmd", "local_cmd"];

    #[test]
    fn every_studio_command_is_listed_granted_and_registered() {
        let list = include_str!("../../commands.list");
        let main = include_str!("../../capabilities/main.json");
        let lib = include_str!("../lib.rs");
        for c in STUDIO_COMMANDS {
            assert!(list.lines().any(|l| l.trim() == *c), "{c}");
            assert!(
                main.contains(&format!("\"allow-{}\"", c.replace('_', "-"))),
                "{c}"
            );
            // A `#[tauri::command]` function is registered by its own module path (a
            // re-export does not work in generate_handler!).
            assert!(
                STUDIO_COMMAND_MODULES
                    .iter()
                    .any(|m| lib.contains(&format!("studio::{m}::{c},"))),
                "{c}"
            );
        }
    }

    fn base64(bytes: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// A JPEG's start, padded with zeros to `len` bytes.
    fn jpeg(len: usize) -> Vec<u8> {
        let mut bytes = b"\xFF\xD8\xFF\xE0 a poster frame".to_vec();
        bytes.resize(len, 0);
        bytes
    }

    #[tokio::test]
    async fn a_saved_poster_must_be_a_small_jpeg() {
        let h = harness().await;
        h.seed_run("v1", MediaKind::Video, "a river", now()).await;
        let video = h
            .seed_output("v1", "video/mp4", &mp4(), mp4().len() as u64)
            .await;
        let thumb = h.manager.gallery().thumb_path(&video);
        for refused in [
            base64(&png(4, 4)),
            base64(&jpeg(POSTER_CAP + 1)),
            "not base64!".to_owned(),
        ] {
            let err = save_thumb(&h.manager, &video, &refused).await.unwrap_err();
            assert_eq!(err.code, "bad_request");
            assert!(!thumb.exists());
        }

        let poster = jpeg(POSTER_CAP);
        let saved = save_thumb(&h.manager, &video, &base64(&poster))
            .await
            .unwrap();
        assert!(saved.has_thumb);
        assert_eq!(saved.id, video);
        assert_eq!(std::fs::read(&thumb).unwrap(), poster);
        assert_eq!(
            output_bytes(&h.manager, &video, true).await.unwrap(),
            poster
        );
        let err = save_thumb(&h.manager, "no-such-output", &base64(&poster))
            .await
            .unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    #[tokio::test]
    async fn a_poster_is_saved_for_a_video_only() {
        let h = harness().await;
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        let err = save_thumb(&h.manager, &out, &base64(&jpeg(1024)))
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert_eq!(err.message, "Only a video can have a poster frame saved.");
        assert!(!h.manager.gallery().thumb_path(&out).exists());
        let kept = h.store.get_creation_output(&out).await.unwrap().unwrap();
        assert_eq!(kept.thumb_path, None);
    }

    #[tokio::test]
    async fn a_star_is_kept_on_the_result() {
        let h = harness().await;
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        assert!(star(&h.manager, &out, true).await.unwrap().starred);
        let starred = RunsFilter {
            starred: true,
            ..RunsFilter::default()
        };
        assert_eq!(h.run_ids(starred.clone()).await, ["p1"]);
        assert!(!star(&h.manager, &out, false).await.unwrap().starred);
        assert!(h.run_ids(starred).await.is_empty());
        let err = star(&h.manager, "no-such-output", true).await.unwrap_err();
        assert_eq!(err.code, "not_found");
    }

    fn output_of_type(mime: &str, size_bytes: u64) -> store::CreationOutput {
        store::CreationOutput {
            id: "out".into(),
            run_id: "run".into(),
            idx: 0,
            path: "unused".into(),
            thumb_path: None,
            mime: mime.into(),
            size_bytes,
            width: None,
            height: None,
            duration_ms: None,
            seed: None,
            starred: false,
            created_at: now(),
        }
    }

    #[test]
    fn a_video_or_a_big_picture_cannot_go_to_a_chat() {
        const MIB: u64 = 1024 * 1024;
        for mime in ["video/mp4", "video/webm", "image/svg+xml"] {
            let err = for_chat(&output_of_type(mime, 1_000)).unwrap_err();
            assert_eq!(err.code, "bad_request", "{mime}");
        }
        let err = for_chat(&output_of_type("video/mp4", 1_000)).unwrap_err();
        assert_eq!(
            err.message,
            "A video cannot go into a chat: chat models do not take video."
        );
        let err = for_chat(&output_of_type("image/png", 21 * MIB)).unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert_eq!(
            err.message,
            "This picture is over 20 MB, so it cannot go into a chat."
        );
        assert!(for_chat(&output_of_type("image/png", 20 * MIB)).is_ok());
        for mime in ["image/png", "image/jpeg", "image/gif", "image/webp"] {
            assert!(for_chat(&output_of_type(mime, 2 * MIB)).is_ok(), "{mime}");
        }
    }

    #[tokio::test]
    async fn a_picture_goes_to_a_chat_or_back_to_a_prompt_as_it_is() {
        let h = harness().await;
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        let image = output_for_chat(&h.manager, &out).await.unwrap();
        assert_eq!(image.mime, "image/png");
        assert_eq!(image.file_name, "Studio picture.png");
        assert_eq!(image.data, base64(&picture));
        let v = serde_json::to_value(&image).unwrap();
        assert_eq!(v["fileName"], json!("Studio picture.png"));

        let added = ref_from_output(&h.manager, &PictureWorker::app(), &out)
            .await
            .unwrap();
        let sha256: String = Sha256::digest(&picture)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(added.sha256, sha256);
        assert_eq!(added.mime, "image/png");
        assert_eq!(added.size_bytes, picture.len() as u64);
        assert_eq!(ref_bytes(&h.manager, &sha256).await.unwrap(), picture);

        // A video goes to neither.
        h.seed_run("v1", MediaKind::Video, "a river", now()).await;
        let video = h
            .seed_output("v1", "video/mp4", &mp4(), mp4().len() as u64)
            .await;
        let err = output_for_chat(&h.manager, &video).await.unwrap_err();
        assert_eq!(err.code, "bad_request");
        let err = ref_from_output(&h.manager, &PictureWorker::app(), &video)
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_request");
        assert_eq!(
            err.message,
            "A video cannot be added to a prompt as a picture."
        );
    }

    #[test]
    fn export_keeps_the_original_extension_and_refuses_app_folders() {
        let scratch = Scratch::new();
        let docs = scratch.path().join("docs");
        let own = scratch.path().join("app-data");
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&own).unwrap();
        let forbidden = vec![own.clone()];
        let at = |p: &Path| p.display().to_string();
        let target = |p: &Path, ext: &str| export_target(&at(p), ext, &forbidden);

        let ok = target(&docs.join("wave.mp4"), "mp4").unwrap();
        assert_eq!(ok, dunce::canonicalize(&docs).unwrap().join("wave.mp4"));
        assert!(target(&docs.join("Wave.MP4"), "mp4").is_ok());
        assert!(target(&docs.join("kite.png"), "png").is_ok());
        for other in ["wave.webm", "wave", "wave.mp4.exe"] {
            assert_eq!(
                target(&docs.join(other), "mp4").unwrap_err(),
                "Give the file a name that ends in .mp4.",
                "{other}"
            );
        }
        assert_eq!(
            target(&own.join("wave.mp4"), "mp4").unwrap_err(),
            "That folder belongs to the app. Choose another one."
        );
    }

    #[tokio::test]
    async fn export_copies_the_file_and_its_settings_beside_it() {
        let h = harness().await;
        let scratch = Scratch::new();
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        let at = |name: &str| scratch.path().join(name).display().to_string();

        let saved = export(&h.manager, &out, &at("kite.png"), false, &[])
            .await
            .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), picture);
        assert!(!scratch.path().join("kite.json").exists());

        let saved = export(&h.manager, &out, &at("again.png"), true, &[])
            .await
            .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), picture);
        let record: Value =
            serde_json::from_slice(&std::fs::read(scratch.path().join("again.json")).unwrap())
                .unwrap();
        assert_eq!(record["prompt"], json!("a red kite"));
        assert_eq!(record["model"], json!(FLUX));
        assert_eq!(record["endpoint"], json!("My OpenRouter"));
        assert!(record.get("costUsd").is_some());
        let text = record.to_string();
        assert!(!text.contains(&h.manager.gallery().root().display().to_string()));

        // The original's extension only; never into the gallery folder.
        let err = export(&h.manager, &out, &at("kite.jpg"), false, &[])
            .await
            .unwrap_err();
        assert_eq!(err.code, "bad_request");
        let inside = h.manager.gallery().root().join("copy.png");
        let err = export(&h.manager, &out, &inside.display().to_string(), false, &[])
            .await
            .unwrap_err();
        assert_eq!(
            err.message,
            "That folder belongs to the app. Choose another one."
        );
    }

    #[tokio::test]
    async fn a_result_kept_in_an_earlier_gallery_folder_is_never_saved_over() {
        // Made first, so they go after the harness.
        let docs = Scratch::new();
        let moved_to = Scratch::new();
        let h = harness().await;
        h.seed_run("p1", MediaKind::Image, "a red kite", now())
            .await;
        let picture = png(4, 4);
        let out = h
            .seed_output("p1", "image/png", &picture, picture.len() as u64)
            .await;
        let earlier = h.manager.gallery().root().to_owned();
        let source = h.manager.gallery().output_path(now(), "p1", 0, "png");
        let month = source.parent().unwrap().to_owned();
        let sidecar = month.join("p1.json");
        std::fs::write(&sidecar, b"the run's own sidecar").unwrap();
        std::fs::create_dir_all(earlier.join(".thumbs")).unwrap();
        // Settings -> Studio moves the gallery; the result stays where it was made.
        h.manager
            .set_prefs(serde_json::from_value(json!({"galleryDir": moved_to.path()})).unwrap())
            .await
            .unwrap();
        assert_eq!(h.manager.gallery().root(), moved_to.path());
        let at = |p: &Path| p.display().to_string();

        for (target, with_settings) in [
            // The result's own file.
            (source.clone(), false),
            // A copy whose settings would replace the run's sidecar.
            (month.join("p1.png"), true),
            // The earlier gallery's other folders.
            (earlier.join(".thumbs").join("kite.png"), false),
            (earlier.join("kite.png"), false),
        ] {
            let err = export(&h.manager, &out, &at(&target), with_settings, &[])
                .await
                .unwrap_err();
            assert_eq!(
                err.message,
                "That folder belongs to the app. Choose another one.",
                "{}",
                target.display()
            );
        }
        assert!(!month.join("p1.png").exists());
        assert!(!earlier.join(".thumbs").join("kite.png").exists());

        // A link elsewhere that leads to the result itself.
        #[cfg(unix)]
        {
            let link = docs.path().join("link.png");
            std::os::unix::fs::symlink(&source, &link).unwrap();
            let err = export(&h.manager, &out, &at(&link), false, &[])
                .await
                .unwrap_err();
            assert_eq!(err.code, "bad_request");
            assert_eq!(err.message, "That file is the result itself.");
        }

        assert_eq!(std::fs::read(&source).unwrap(), picture);
        assert_eq!(std::fs::read(&sidecar).unwrap(), b"the run's own sidecar");
        // Anywhere else, the copy is made as before.
        let saved = export(
            &h.manager,
            &out,
            &at(&docs.path().join("kite.png")),
            true,
            &[],
        )
        .await
        .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), picture);
        assert!(docs.path().join("kite.json").exists());
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

    // ---- the webview's fixture (Task B1) ----------------------------------------------------
    //
    // The webview's tests build their data from what the backend sends, written here as typed
    // TypeScript literals: a field serde names differently from `src/components/studio/
    // types.ts` fails `npm run typecheck` (a `.json` import would widen its strings and let it
    // pass). Every value that could differ between computers is fixed: the clock, the
    // hardware, the engine (built in, as on a Mac, whatever the host), the paths.

    /// The fixture, from the repository's root.
    const WEBVIEW_FIXTURE: [&str; 5] = ["src", "components", "studio", "fixtures", "models.ts"];
    /// Set to write the fixture instead of comparing it.
    const WRITE_FIXTURE: &str = "STUDIO_WRITE_FIXTURE";
    /// Its first line.
    const FIXTURE_HEADER: &str =
        "// Generated by studio::commands::tests::the_webview_fixture_is_current. Do not edit.";
    /// The cloud models it holds, as the picker lists them: three pictures, then three videos.
    const FIXTURE_MODELS: [&str; 6] = [
        "openai/gpt-image-2.6-flare",
        "google/gemini-nano-banana-2.1",
        "krea/k3-edit",
        "google/veo-3.1",
        "x-ai/grok-imagine-video-1.5-lite",
        "bytedance/seedance-2.5-flash",
    ];
    /// The picture whose recorded `/endpoints` answer is saved with the lists.
    const FLARE: &str = "openai/gpt-image-2.6-flare";
    /// 17 November 2026 in UTC: the lists fetched at 09:00, flare's price at 10:00, the cloud
    /// run made at 11:00 and the local one at 11:30.
    const LISTS_AT: i64 = 1_794_906_000_000;
    const PRICE_AT: i64 = 1_794_909_600_000;
    const CLOUD_RUN_AT: i64 = 1_794_913_200_000;
    const LOCAL_RUN_AT: i64 = 1_794_915_000_000;
    /// When the two local models were installed, in seconds (as `created` counts): Wan 2.2
    /// on 15 November, klein the day after.
    const WAN_INSTALLED_S: i64 = 1_794_733_200;
    const KLEIN_INSTALLED_S: i64 = 1_794_819_600;
    const KLEIN_ID: &str = "flux2-klein-4b-q4";
    const WAN_ID: &str = "wan22-ti2v-5b-fast-q6";
    const GIB: u64 = 1 << 30;
    /// The fixture's graphics card as llama.cpp describes it: an 8 GB Vulkan card, shown as
    /// "Radeon RX 7600".
    const FIXTURE_CARD: &str = "Radeon RX 7600 (RADV NAVI33)";
    /// Where a run on it records it ran.
    const FIXTURE_DEVICE: &str = "Radeon RX 7600 (Vulkan)";

    /// The 8 GB Vulkan card, with nothing of the chat model on it.
    struct FixtureCard;

    #[async_trait]
    impl LocalHardware for FixtureCard {
        async fn target(&self) -> (Target, String, Option<String>) {
            let card = Target::Gpu {
                name: FIXTURE_CARD.into(),
                free_bytes: 7_900 * (1 << 20),
                ram_free_bytes: Some(24 * GIB),
            };
            (card, "vulkan".into(), Some(FIXTURE_CARD.into()))
        }

        async fn device_total_bytes(&self) -> Option<u64> {
            Some(8 * GIB)
        }

        fn chat_device_bytes(&self) -> Option<u64> {
            None
        }

        fn logical_cores(&self) -> usize {
            12
        }
    }

    /// The bundled catalog's picture and video models.
    struct BundledCatalog;

    #[async_trait]
    impl GenCatalogSource for BundledCatalog {
        async fn gen_catalog(&self) -> GenCatalog {
            gen_catalog(&Catalog::bundled(), sd_lock::embedded().sd_cpp.build)
        }
    }

    /// Records `run` as ended by `patch`, with one 1024×1024 PNG per entry of `outputs` (its id
    /// and size; each made with `seed` and named `<run>-<n>` in the gallery), and gives it as
    /// the webview gets it.
    async fn record_ended(
        h: &H,
        run: store::NewCreationRun,
        patch: store::CreationPatch,
        outputs: &[(&str, u64)],
        seed: u64,
    ) -> RunDto {
        let id = run.id.clone();
        let created_at = run.created_at;
        h.store.insert_creation_run(run).await.unwrap();
        h.store.update_creation_run(&id, patch).await.unwrap();
        for (idx, (output, size_bytes)) in (0..).zip(outputs) {
            h.store
                .insert_creation_output(store::NewCreationOutput {
                    id: (*output).into(),
                    run_id: id.clone(),
                    idx,
                    path: format!("/home/me/harnessUI/studio/2026-11/{id}-{idx}.png"),
                    thumb_path: Some(format!("/home/me/harnessUI/studio/.thumbs/{output}.jpg")),
                    mime: "image/png".into(),
                    size_bytes: *size_bytes,
                    width: Some(1024),
                    height: Some(1024),
                    duration_ms: None,
                    seed: Some(seed),
                    created_at,
                })
                .await
                .unwrap();
        }
        let seq = h.manager.next_seq();
        let row = h.store.get_creation_run(&id).await.unwrap().unwrap();
        h.manager.run_dto(&row, seq).await
    }

    /// Installs catalog entry `id` of the bundled catalog, its parts in `models`.
    async fn install_bundled(store: &store::Store, models: &Path, id: &str) {
        let catalog = BundledCatalog.gen_catalog().await;
        let entry = &catalog.entries[id];
        let parts: Vec<Value> = entry
            .parts
            .iter()
            .map(|part| {
                let mut saved = serde_json::to_value(part).unwrap();
                let file = destination(models, &part.repo, &part.path).unwrap();
                saved["file"] = json!(file.display().to_string());
                saved
            })
            .collect();
        let kind = serde_json::to_value(entry.kind).unwrap();
        let family = serde_json::to_value(entry.family).unwrap();
        store
            .insert_gen_model(store::NewGenModel {
                id: id.into(),
                catalog_id: Some(id.into()),
                name: entry.name.clone(),
                kind: kind.as_str().unwrap().into(),
                family: family.as_str().unwrap().into(),
                parts: json!(parts),
                size_bytes: entry.total_bytes(),
                source: store::GenSource::Catalog,
            })
            .await
            .unwrap();
    }

    /// `value` as a TypeScript literal of `ty`, exported as `name`.
    fn typed_literal(name: &str, ty: &str, value: &impl serde::Serialize) -> String {
        let json = serde_json::to_string_pretty(value).unwrap();
        format!("export const {name}: {ty} = {json};\n")
    }

    #[tokio::test]
    async fn the_webview_fixture_is_current() {
        let h = harness().await;
        let scratch = Scratch::new();

        // The six cloud models, from the recorded lists, with flare's recorded `/endpoints`
        // answer saved (the other pictures' prices are shown when picked).
        let answers: Value = serde_json::from_str(ENDPOINTS).unwrap();
        let lists = CachedCatalog {
            fetched_at: LISTS_AT,
            raw: recorded_lists(),
            image_prices: [(
                FLARE.to_owned(),
                CachedPrice {
                    fetched_at: PRICE_AT,
                    raw: answers["answers"][FLARE].clone(),
                },
            )]
            .into(),
        };
        h.store
            .set_setting(&cache_key(h.openrouter), &lists)
            .await
            .unwrap();
        let listed = models(&h.manager, |_| true).await.unwrap();
        let cloud: Vec<MediaModelDto> = FIXTURE_MODELS
            .iter()
            .map(|id| model_in(&listed.endpoints[0], id).clone())
            .collect();
        assert!(cloud[0].price_fetched_at == Some(PRICE_AT) && !cloud[0].companies.is_empty());
        assert_eq!(cloud[1].price_fetched_at, None);
        assert_eq!(cloud[3].price_fetched_at, Some(LISTS_AT));

        // A finished cloud run: four pictures of flare, run at OpenAI, $0.04, 13.8 s. Flare
        // takes no seed; the run keeps seed 81723 all the same, so the viewer's tests (Task B9)
        // have one to reuse or leave out.
        let run = record_ended(
            &h,
            store::NewCreationRun {
                id: "cloud-run".into(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Cloud,
                provider_id: Some(h.openrouter),
                provider_name: "My OpenRouter".into(),
                provider_host: "openrouter.ai".into(),
                model: FLARE.into(),
                model_label: cloud[0].display_name.clone(),
                prompt: "A lighthouse on a rocky coast at dusk, in watercolour".into(),
                params: json!({
                    "params": {"aspect_ratio": "1:1", "n": 4, "quality": "low", "seed": 81_723},
                    "extra": {},
                    "routing": null,
                }),
                refs: json!([]),
                estimate_usd: None,
                gen_model_id: None,
                engine_build: None,
                device: None,
                created_at: CLOUD_RUN_AT,
            },
            store::CreationPatch {
                status: Some(store::CreationStatus::Done),
                remote_id: Some(Some("gen-1794913200-k7Qm2xRb9TfLw4Hc".into())),
                cost_usd: Some(Some(0.04)),
                upstream: Some(Some("OpenAI".into())),
                started_at: Some(Some(CLOUD_RUN_AT + 200)),
                finished_at: Some(Some(CLOUD_RUN_AT + 200 + 13_800)),
                ..Default::default()
            },
            &[
                ("o1", 1_812_334),
                ("o2", 1_790_221),
                ("o3", 1_845_903),
                ("o4", 1_801_456),
            ],
            81_723,
        )
        .await;
        assert_eq!(run.status, "done");
        assert_eq!(run.upstream.as_deref(), Some("OpenAI"));
        assert_eq!(run.cost_usd, Some(0.04));
        assert_eq!(run.params["seed"], json!(81_723));
        assert_eq!(
            run.outputs
                .iter()
                .map(|o| o.id.as_str())
                .collect::<Vec<_>>(),
            ["o1", "o2", "o3", "o4"]
        );
        assert!(!run.watching);

        // This computer: Wan 2.2 installed, then klein, an earlier run of klein at its
        // defaults (one picture in 1 min 54 s, not in the fixture) and a finished run of
        // klein (two pictures in 3 min 48 s): klein's measured times here.
        let models_dir = scratch.path().join("models");
        install_bundled(&h.store, &models_dir, WAN_ID).await;
        install_bundled(&h.store, &models_dir, KLEIN_ID).await;
        record_ended(
            &h,
            store::NewCreationRun {
                id: "earlier-local-run".into(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Local,
                provider_id: None,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                model: KLEIN_ID.into(),
                model_label: "FLUX.2 klein 4B".into(),
                prompt: "A lighthouse at dusk".into(),
                params: json!({
                    "params": {"aspect_ratio": "1:1", "seed": 7},
                    "extra": {},
                    "routing": null,
                    "job": {"width": 1024, "height": 1024, "frames": 1, "steps": 4, "pictures": 1},
                }),
                refs: json!([]),
                estimate_usd: Some(0.0),
                gen_model_id: Some(KLEIN_ID.into()),
                engine_build: Some(1106),
                device: Some(FIXTURE_DEVICE.into()),
                created_at: LOCAL_RUN_AT - 3_600_000,
            },
            store::CreationPatch {
                status: Some(store::CreationStatus::Done),
                cost_usd: Some(Some(0.0)),
                elapsed_ms: Some(Some(114_000)),
                ..Default::default()
            },
            &[("l0", 1_398_204)],
            7,
        )
        .await;
        let local_run = record_ended(
            &h,
            store::NewCreationRun {
                id: "local-run".into(),
                kind: store::CreationKind::Image,
                runs_on: store::RunsOn::Local,
                provider_id: None,
                provider_name: "This computer".into(),
                provider_host: String::new(),
                model: KLEIN_ID.into(),
                model_label: "FLUX.2 klein 4B".into(),
                prompt: "A red fox asleep in fresh snow, morning light".into(),
                params: json!({
                    "params": {"aspect_ratio": "1:1", "n": 2, "seed": 81_723},
                    "extra": {},
                    "routing": null,
                    "job": {"width": 1024, "height": 1024, "frames": 1, "steps": 4, "pictures": 2},
                }),
                refs: json!([]),
                estimate_usd: Some(0.0),
                gen_model_id: Some(KLEIN_ID.into()),
                engine_build: Some(1106),
                device: Some(FIXTURE_DEVICE.into()),
                created_at: LOCAL_RUN_AT,
            },
            store::CreationPatch {
                status: Some(store::CreationStatus::Done),
                cost_usd: Some(Some(0.0)),
                started_at: Some(Some(LOCAL_RUN_AT + 400)),
                finished_at: Some(Some(LOCAL_RUN_AT + 400 + 228_900)),
                elapsed_ms: Some(Some(228_000)),
                ..Default::default()
            },
            &[("l1", 1_402_118), ("l2", 1_389_675)],
            81_723,
        )
        .await;
        let local_dto = local_run.local.as_ref().unwrap();
        assert_eq!(local_dto.elapsed_ms, Some(228_000));
        assert_eq!(local_dto.device.as_deref(), Some(FIXTURE_DEVICE));
        assert_eq!(local_dto.engine_build, Some(1106));
        assert!(local_dto.labelled);

        // The engine built in, as on a Mac, whatever the host (its state would else differ
        // between the CI's systems).
        let bundled = scratch.path().join("MacOS");
        std::fs::create_dir_all(&bundled).unwrap();
        std::fs::write(bundled.join(program_file_name()), b"sd-cli").unwrap();
        let layout = EngineLayout {
            bundled_dir: Some(bundled),
            engines_dir: scratch.path().join("engines"),
            ..EngineLayout::default()
        };
        let deps = LocalCmdDeps {
            store: h.store.clone(),
            catalog: Arc::new(BundledCatalog),
            hardware: Arc::new(FixtureCard),
            hf: None,
            models_dir,
            free_disk_bytes: Some(412 * GIB),
            ram_total_bytes: Some(32 * GIB),
            engine: status_from(sd_lock::embedded(), &layout, "macos-aarch64", None),
            engine_need: EngineNeed::Found,
            notice_accepted: true,
            installed_files: crate::studio::local_models::InstalledFiles::default(),
        };
        let mut local = local_models(&deps).await.unwrap();
        // The store dates an install by its own clock: fixed here.
        for (installed, at) in local
            .installed
            .iter_mut()
            .zip([KLEIN_INSTALLED_S, WAN_INSTALLED_S])
        {
            installed.model.created = Some(at);
        }
        assert_eq!(local.engine.state, "builtIn");
        assert_eq!(local.engine.download_bytes, 0);
        assert_eq!(
            local
                .installed
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            [KLEIN_ID, WAN_ID]
        );
        let klein = &local.installed[0];
        assert_eq!(klein.fit.word, "fits");
        assert!(klein.fit.eta.as_ref().is_some_and(|eta| eta.measured));
        assert_eq!(local.recommended[0].id, KLEIN_ID);

        let text = [
            format!(
                "{FIXTURE_HEADER}\nimport type {{ LocalModels, MediaModel, Run }} from \"../types\";\n"
            ),
            typed_literal("MODELS", "MediaModel[]", &cloud),
            typed_literal("RUN", "Run", &run),
            typed_literal("LOCAL", "LocalModels", &local),
            typed_literal("LOCAL_RUN", "Run", &local_run),
        ]
        .join("\n");
        let path = WEBVIEW_FIXTURE
            .iter()
            .fold(Path::new(env!("CARGO_MANIFEST_DIR")).join(".."), |p, s| {
                p.join(s)
            });
        if std::env::var_os(WRITE_FIXTURE).is_some() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, &text).unwrap();
            return;
        }
        let refresh = format!(
            "run `{WRITE_FIXTURE}=1 cargo test -p app --lib \
             studio::commands::tests::the_webview_fixture_is_current` to write it"
        );
        let saved = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("{} cannot be read ({e}): {refresh}", path.display()));
        // A Windows checkout may turn its line ends into CRLF.
        let saved = saved.replace("\r\n", "\n");
        if let Some((n, (want, have))) = text
            .lines()
            .zip(saved.lines())
            .enumerate()
            .find(|(_, (want, have))| want != have)
        {
            panic!(
                "{} is out of date at line {}: it has `{have}`, the backend sends `{want}`; {refresh}",
                path.display(),
                n + 1
            );
        }
        assert_eq!(
            text.lines().count(),
            saved.lines().count(),
            "{} is out of date: {refresh}",
            path.display()
        );
    }
}
