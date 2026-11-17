//! OpenRouter's model lists, picture prices, pictures and video jobs (0.1.0 Studio).
//!
//! - **Lists and prices**: `GET {api}/images/models` and `{api}/videos/models`, and a picture
//!   model's `/endpoints` answer (`GET {api}/images/models/{id}/endpoints`), all through the
//!   endpoint's own client, so the key goes only to its host.
//! - **Pictures**: `POST {api}/images`, sent at most once ([`OpenAiClient::post_once`]). The
//!   body follows OpenRouter's docs as checked on 2026-11-17: added pictures as
//!   `input_references` items `{"type": "image_url", "image_url": {"url": "data:…"}}`, the
//!   routing in `provider` (`only` or `order` lists of `provider_tag`, `allow_fallbacks`,
//!   `zdr`) and each company's extra settings under `provider.options[<provider_slug>]`
//!   ([`place_extra`]). The answer is streamed to a file, then its pictures are written one
//!   at a time.
//! - **Video**: `POST {api}/videos`, sent at most once, starts a job; its body follows the
//!   video docs as checked on 2026-11-17: the typed settings, and the start and end frames as
//!   `frame_images` items `{"type": "image_url", "image_url": {"url": "data:…"},
//!   "frame_type": "first_frame" | "last_frame"}`. It carries no `provider` and no extra
//!   settings: OpenRouter keys a video's extra settings by the company the job is routed to
//!   (nested in `parameters` for Veo), which its video list does not name. `GET
//!   {api}/videos/{id}` checks on the job, each check within its own time limit, and `GET
//!   {api}/videos/{id}/content?index=N` downloads a result with the key
//!   ([`OpenAiClient::download_to_file`]); a link to another host is handed back, never
//!   followed with the key.
//! - **Failures**: [`openrouter_failure`]. A moderation refusal and a 502 are not charged,
//!   nor is a video job that failed.

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::Path;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Map, Value, json};
use url::Url;

use super::{
    Company, Cost, FailureKind, Fetched, JobDone, JobState, LocalIo, MediaError, MediaFailure,
    MediaKind, MediaLimits, MediaModel, MediaProvider, MediaRequest, RawCatalog, RefImage, RefRole,
    RequestPreview, Routing, Saved, SignedUrl, Stage, Started, base_failure,
};
use crate::client::{OnceBody, OnceReply, OpenAiClient, join};
use crate::cloud::scrub_text;
use crate::error::{ProviderError, bounded};

/// The settings a video request sends besides `model`, `prompt` and the frames, in this
/// order. `size`, when set, replaces `resolution` and `aspect_ratio`.
const VIDEO_FIELDS: [&str; 6] = [
    "duration",
    "resolution",
    "aspect_ratio",
    "size",
    "generate_audio",
    "seed",
];

/// Pictures and video through an OpenRouter endpoint.
#[derive(Debug, Clone)]
pub struct OpenRouterMedia {
    client: OpenAiClient,
    limits: MediaLimits,
}

impl OpenRouterMedia {
    /// OpenRouter media through the endpoint's own client, with the default limits.
    pub fn new(client: OpenAiClient) -> Self {
        Self {
            client,
            limits: MediaLimits::default(),
        }
    }

    /// The same, with other limits.
    #[must_use]
    pub fn with_limits(self, limits: MediaLimits) -> Self {
        Self { limits, ..self }
    }

    fn key(&self) -> Option<&str> {
        self.client.config().api_key.as_deref()
    }

    fn url(&self, path: &str) -> Url {
        join(self.client.api_url(), path)
    }

    /// `{api}/videos/{id}` followed by `tail`, the id as one path segment (escaped).
    fn job_url(&self, remote_id: &str, tail: &[&str]) -> Url {
        let mut url = self.url("videos");
        if let Ok(mut path) = url.path_segments_mut() {
            path.push(remote_id).extend(tail);
        }
        url
    }

    /// A read through the endpoint's client: a model list or a price answer.
    async fn read(&self, url: Url) -> Result<Value, MediaError> {
        self.client
            .get_json(url)
            .await
            .map_err(|err| read_failed(err, self.key()))
    }

    /// Makes pictures: one request whose answer carries them.
    async fn start_pictures(
        &self,
        req: &MediaRequest,
        scratch: &Path,
    ) -> Result<Started, MediaError> {
        let mut items = Vec::new();
        for picture in references(req) {
            let url = data_url(picture).await?;
            items.push(json!({"type": "image_url", "image_url": {"url": url}}));
        }
        let reply = self
            .client
            .post_once(
                self.url("images"),
                OnceBody::Json(picture_body(req, items)),
                self.limits.picture_timeout,
                self.limits.response_cap,
                scratch,
            )
            .await?;
        match reply {
            OnceReply::Success { path, len, headers } => {
                let saved = if len == 0 {
                    Err(unreadable("the answer was empty".to_owned()))
                } else {
                    let (answer, folder) = (path.clone(), scratch.to_owned());
                    let cap = self.limits.picture_cap;
                    tokio::task::spawn_blocking(move || save_pictures(&answer, &folder, cap))
                        .await
                        .unwrap_or_else(|err| {
                            Err(MediaError::Local(LocalIo::Io(format!(
                                "the pictures were not written ({err})"
                            ))))
                        })
                };
                let _ = tokio::fs::remove_file(&path).await;
                let (outputs, cost) =
                    saved.map_err(|err| naming_the_answer(err, &headers, self.key()))?;
                Ok(Started::Done { outputs, cost })
            }
            OnceReply::Error {
                status,
                body,
                headers,
            } => Err(refused(status, &headers, &body, self.key())),
        }
    }

    /// Starts a video job: one request whose answer carries the job's id.
    async fn start_video(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        let mut items = Vec::new();
        for (picture, frame_type) in frames(req) {
            items.push(frame_item(data_url(picture).await?, frame_type));
        }
        // The whole-request limit of a picture request: the frames go out with it.
        let reply = self
            .client
            .post_once(
                self.url("videos"),
                OnceBody::Json(video_body(req, items)),
                self.limits.picture_timeout,
                self.limits.response_cap,
                scratch,
            )
            .await?;
        match reply {
            OnceReply::Success { path, headers, .. } => {
                let answer = tokio::fs::read(&path).await;
                let _ = tokio::fs::remove_file(&path).await;
                let answer = answer.map_err(|err| MediaError::Local(LocalIo::from_io(&err)))?;
                let remote_id =
                    job_id(&answer).map_err(|err| naming_the_answer(err, &headers, self.key()))?;
                Ok(Started::Job { remote_id })
            }
            OnceReply::Error {
                status,
                body,
                headers,
            } => Err(refused(status, &headers, &body, self.key())),
        }
    }
}

#[async_trait]
impl MediaProvider for OpenRouterMedia {
    fn company(&self) -> Company {
        Company::OpenRouter
    }

    fn host(&self) -> String {
        self.client
            .api_url()
            .host_str()
            .unwrap_or_default()
            .to_owned()
    }

    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
        let (images, videos) = tokio::try_join!(
            self.read(self.url("images/models")),
            self.read(self.url("videos/models")),
        )?;
        Ok(RawCatalog {
            images,
            videos: Some(videos),
        })
    }

    async fn fetch_image_price(&self, model: &str) -> Result<Value, MediaError> {
        self.read(self.url(&format!("images/models/{model}/endpoints")))
            .await
    }

    fn preview(&self, req: &MediaRequest) -> RequestPreview {
        let mut url = self.url(match req.kind {
            MediaKind::Image => "images",
            MediaKind::Video => "videos",
        });
        // Credentials in the address are not shown either.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        let body = match req.kind {
            MediaKind::Image => picture_body(
                req,
                references(req)
                    .enumerate()
                    .map(|(i, picture)| Value::String(picture_label(i + 1, picture)))
                    .collect(),
            ),
            MediaKind::Video => video_body(
                req,
                frames(req)
                    .enumerate()
                    .map(|(i, (picture, frame_type))| {
                        frame_item(picture_label(i + 1, picture), frame_type)
                    })
                    .collect(),
            ),
        };
        RequestPreview {
            method: "POST".to_owned(),
            url: url.to_string(),
            host: self.host(),
            body,
        }
    }

    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        match req.kind {
            MediaKind::Image => self.start_pictures(req, scratch).await,
            MediaKind::Video => self.start_video(req, scratch).await,
        }
    }

    /// `GET {api}/videos/{id}`, within `check_timeout`. A 404 means OpenRouter no longer has
    /// the job ([`JobState::Gone`]); a check that got no answer is an error, not a state.
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError> {
        match self
            .client
            .get_json_within(self.job_url(remote_id, &[]), self.limits.check_timeout)
            .await
        {
            Ok(answer) => job_state(&answer, self.key()),
            Err(ProviderError::Http { status: 404, .. }) => Ok(JobState::Gone),
            Err(err) => Err(check_failed(err, self.key())),
        }
    }

    /// `GET {api}/videos/{id}/content?index=N` with the key, streamed to `dest` (at most
    /// `video_cap`, at most `download_idle` without data); the docs say a result stays there
    /// for as long as the output is available. When the job gave only signed links, result
    /// `index`'s link is handed back for the guarded fetcher instead, as is a redirect to
    /// another host. A 404 or 410 means the result has expired.
    async fn download(
        &self,
        remote_id: &str,
        job: &JobDone,
        index: usize,
        dest: &Path,
    ) -> Result<Fetched, MediaError> {
        if let Some(link) = job.signed.get(index) {
            return Ok(Fetched::Elsewhere(link.url.clone()));
        }
        let mut url = self.job_url(remote_id, &["content"]);
        url.query_pairs_mut()
            .append_pair("index", &index.to_string());
        self.client
            .download_to_file(url, dest, self.limits.video_cap, self.limits.download_idle)
            .await
            .map_err(|err| match err {
                MediaError::Answered(mut failure) if matches!(failure.status, Some(404 | 410)) => {
                    failure.kind = FailureKind::Expired;
                    MediaError::Answered(failure)
                }
                other => other,
            })
    }
}

/// Splits the user's extra settings for a picture model. A setting that a company running the
/// model lists (its `allowed_passthrough_parameters`) goes under `provider.options`, by
/// `provider_slug`, for every company that lists it (with `only` routing, for the pinned
/// companies alone); any other setting stays at the top level, as the catalogue names it.
/// Returns the top-level settings and the settings per company. [`super::validate`] has
/// already refused settings the model does not offer.
pub fn place_extra(
    model: &MediaModel,
    extra: Map<String, Value>,
    routing: Option<&Routing>,
) -> (Map<String, Value>, BTreeMap<String, Map<String, Value>>) {
    let pinned = |tag: &str| match routing {
        Some(r) if r.only && !r.order.is_empty() => r.order.iter().any(|t| t == tag),
        _ => true,
    };
    let mut top = Map::new();
    let mut options: BTreeMap<String, Map<String, Value>> = BTreeMap::new();
    for (key, value) in extra {
        let mut listing = model
            .companies
            .iter()
            .filter(|company| company.passthrough.contains(&key))
            .peekable();
        if listing.peek().is_none() {
            top.insert(key, value);
            continue;
        }
        for company in listing.filter(|company| pinned(&company.tag)) {
            options
                .entry(company.slug.clone())
                .or_default()
                .insert(key.clone(), value.clone());
        }
    }
    (top, options)
}

/// What an OpenRouter error answer says: [`base_failure`], plus a moderation refusal (a 400
/// or 403 whose message speaks of moderation or a flag, or whose metadata names one) as
/// [`FailureKind::Blocked`], not charged, and a 502 ("failed, not charged") as not charged.
pub(crate) fn openrouter_failure(
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
    key: Option<&str>,
) -> MediaFailure {
    let mut failure = base_failure(status, headers, body, key);
    let metadata = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| value.pointer("/error/metadata").cloned());
    let moderation = speaks_of_moderation(&failure.message)
        || metadata
            .as_ref()
            .is_some_and(|m| m.get("moderation").is_some() || m.get("flagged_input").is_some());
    if matches!(status, 400 | 403) && moderation {
        failure.kind = FailureKind::Blocked;
        failure.charged = Some(false);
        // OpenRouter's own check reads the input: the text it flagged was in the prompt.
        if metadata.is_some_and(|m| m.get("flagged_input").is_some()) {
            failure.stage = Some(Stage::Prompt);
        }
    } else if status == 502 {
        failure.charged = Some(false);
    }
    failure
}

/// Whether an error message speaks of moderation or a flag, as OpenRouter's refusals do.
fn speaks_of_moderation(message: &str) -> bool {
    let lower = message.to_lowercase();
    lower.contains("moderation") || lower.contains("flagged")
}

/// The error for a create the company refused: its failure, except a server failure whose
/// charge is unknown (any 5xx but 502), which is reported as no answer, like a timeout: the
/// request may have run.
fn refused(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaError {
    let failure = openrouter_failure(status, headers, body, key);
    if failure.kind == FailureKind::Server && failure.charged.is_none() {
        MediaError::Unknown(ProviderError::Http {
            status,
            message: failure.message,
        })
    } else {
        MediaError::Answered(failure)
    }
}

/// A read (a model list or a price answer) that failed. Reads are never charged: an error
/// status is the company's answer, and anything else means no answer came.
fn read_failed(err: ProviderError, key: Option<&str>) -> MediaError {
    match err {
        ProviderError::Unauthorized => {
            MediaError::Answered(openrouter_failure(401, &HeaderMap::new(), b"", key))
        }
        ProviderError::Http { status, message } => MediaError::Answered(openrouter_failure(
            status,
            &HeaderMap::new(),
            message.as_bytes(),
            key,
        )),
        other => MediaError::NotSent(other),
    }
}

/// A check on a video job that failed. An error status is the company's answer, which says
/// nothing about what the job cost; anything else means no answer came.
fn check_failed(err: ProviderError, key: Option<&str>) -> MediaError {
    match read_failed(err, key) {
        MediaError::Answered(mut failure) => {
            failure.charged = None;
            MediaError::Answered(failure)
        }
        MediaError::NotSent(err) => MediaError::Unknown(err),
        other => other,
    }
}

/// An accepted answer whose pictures cannot be read: the run may have been charged.
fn unreadable(what: String) -> MediaError {
    MediaError::Unknown(ProviderError::Decode(what))
}

/// An unreadable answer's error, naming the answer's type when it is not JSON (a proxy's
/// page, for example). Other errors are returned unchanged.
fn naming_the_answer(err: MediaError, headers: &HeaderMap, key: Option<&str>) -> MediaError {
    let kind = headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .filter(|kind| !kind.trim_start().starts_with("application/json"));
    match (err, kind) {
        (MediaError::Unknown(ProviderError::Decode(what)), Some(kind)) => {
            let kind = scrub_text(&bounded(kind), key);
            unreadable(format!("{what} (OpenRouter answered with {kind})"))
        }
        (err, _) => err,
    }
}

// ---- the picture request -----------------------------------------------------------------------

/// An added picture as a `data:` URL, read from its file. A file that cannot be read stops
/// the request before it is sent.
async fn data_url(picture: &RefImage) -> Result<String, MediaError> {
    let bytes = tokio::fs::read(&picture.path).await.map_err(|err| {
        MediaError::NotSent(ProviderError::InvalidRequest(format!(
            "could not read an added picture: {err}"
        )))
    })?;
    Ok(format!(
        "data:{};base64,{}",
        picture.mime,
        STANDARD.encode(&bytes)
    ))
}

/// The added pictures a picture request sends: its references.
fn references(req: &MediaRequest) -> impl Iterator<Item = &RefImage> {
    req.refs
        .iter()
        .filter(|picture| picture.role == RefRole::Reference)
}

/// The picture request's body: `model` and `prompt`, every typed setting, the extra settings
/// that stay at the top level (none replaces another key), the added pictures as
/// `input_references` and the routing and company settings as `provider`.
fn picture_body(req: &MediaRequest, references: Vec<Value>) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(req.model.clone()));
    body.insert("prompt".to_owned(), Value::String(req.prompt.clone()));
    for (key, value) in req.params.iter().chain(&req.extra) {
        body.entry(key.clone()).or_insert_with(|| value.clone());
    }
    if !references.is_empty() {
        body.insert("input_references".to_owned(), Value::Array(references));
    }
    if let Some(provider) = provider(req) {
        body.insert("provider".to_owned(), provider);
    }
    Value::Object(body)
}

/// `provider`: the routing (`only` or `order`, `allow_fallbacks`, and `zdr` when asked) and
/// each company's extra settings under `options`; `None` when there are neither.
fn provider(req: &MediaRequest) -> Option<Value> {
    let mut provider = Map::new();
    if let Some(routing) = &req.routing {
        if !routing.order.is_empty() {
            let list = if routing.only { "only" } else { "order" };
            provider.insert(list.to_owned(), json!(routing.order));
        }
        provider.insert(
            "allow_fallbacks".to_owned(),
            Value::Bool(routing.allow_fallbacks),
        );
        if routing.zdr {
            provider.insert("zdr".to_owned(), Value::Bool(true));
        }
    }
    if !req.provider_options.is_empty() {
        provider.insert("options".to_owned(), json!(req.provider_options));
    }
    (!provider.is_empty()).then_some(Value::Object(provider))
}

/// An added picture as the preview shows it: `picture 1 (sha 3f2a9c41d0b7…, 1.2 MB)`.
fn picture_label(n: usize, picture: &RefImage) -> String {
    let sha: String = picture.sha256.chars().take(12).collect();
    format!(
        "picture {n} (sha {sha}…, {})",
        size_text(picture.size_bytes)
    )
}

/// A file size in decimal units, as the webview shows sizes: `1.2 MB`, `340 KB`, `28 B`.
fn size_text(bytes: u64) -> String {
    if bytes >= 1_000_000 {
        format!("{:.1} MB", bytes as f64 / 1e6)
    } else if bytes >= 1_000 {
        format!("{:.0} KB", bytes as f64 / 1e3)
    } else {
        format!("{bytes} B")
    }
}

// ---- the picture answer ------------------------------------------------------------------------

/// OpenRouter's picture answer.
#[derive(Deserialize)]
struct PictureAnswer {
    data: Vec<AnswerPicture>,
    usage: Option<AnswerUsage>,
}

/// One picture of the answer.
#[derive(Deserialize)]
struct AnswerPicture {
    b64_json: String,
    media_type: Option<String>,
}

/// The answer's usage figures.
#[derive(Deserialize)]
struct AnswerUsage {
    cost: Option<f64>,
}

/// Reads the answer in the file `answer` and writes each picture to `scratch/<uuid>-<n>`, one
/// at a time; what the run cost, when the answer says. The pictures already written are
/// removed when a later one fails.
fn save_pictures(
    answer: &Path,
    scratch: &Path,
    picture_cap: u64,
) -> Result<(Vec<Saved>, Option<Cost>), MediaError> {
    let file =
        std::fs::File::open(answer).map_err(|err| MediaError::Local(LocalIo::from_io(&err)))?;
    let parsed: PictureAnswer = serde_json::from_reader(BufReader::new(file))
        .map_err(|err| unreadable(format!("the answer is not a picture answer ({err})")))?;
    let stem = uuid::Uuid::new_v4();
    let mut saved: Vec<Saved> = Vec::new();
    for (n, picture) in parsed.data.into_iter().enumerate() {
        match save_picture(picture, &scratch.join(format!("{stem}-{n}")), picture_cap) {
            Ok(one) => saved.push(one),
            Err(err) => {
                for done in &saved {
                    let _ = std::fs::remove_file(&done.path);
                }
                return Err(err);
            }
        }
    }
    if saved.is_empty() {
        return Err(unreadable("the answer has no pictures".to_owned()));
    }
    Ok((saved, reported(parsed.usage.and_then(|usage| usage.cost))))
}

/// A cost the company reported, when it is a sound amount.
fn reported(usd: Option<f64>) -> Option<Cost> {
    usd.filter(|usd| usd.is_finite() && *usd >= 0.0)
        .map(|usd| Cost {
            usd,
            reported: true,
        })
}

/// Decodes one picture and writes it to `path`. Its type is the answer's `media_type`, else
/// read from the bytes.
fn save_picture(picture: AnswerPicture, path: &Path, cap: u64) -> Result<Saved, MediaError> {
    let bytes = STANDARD
        .decode(picture.b64_json.trim())
        .map_err(|err| unreadable(format!("a picture in the answer is not base64 ({err})")))?;
    let size_bytes = bytes.len() as u64;
    if size_bytes > cap {
        return Err(MediaError::Local(LocalIo::TooLarge(cap)));
    }
    let mime = picture
        .media_type
        .map(|kind| kind.trim().to_ascii_lowercase())
        .filter(|kind| !kind.is_empty())
        .or_else(|| sniff(&bytes).map(str::to_owned))
        .unwrap_or_else(|| "application/octet-stream".to_owned());
    if let Err(err) = std::fs::write(path, &bytes) {
        let _ = std::fs::remove_file(path);
        return Err(MediaError::Local(LocalIo::from_io(&err)));
    }
    Ok(Saved {
        path: path.to_owned(),
        mime,
        size_bytes,
        width: None,
        height: None,
        duration_ms: None,
    })
}

/// A picture's media type from its first bytes: PNG, JPEG, GIF, WebP or SVG.
fn sniff(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        let head = String::from_utf8_lossy(&bytes[..bytes.len().min(1024)]).to_ascii_lowercase();
        let head = head.trim_start_matches('\u{feff}').trim_start();
        (head.starts_with("<svg") || (head.starts_with("<?xml") && head.contains("<svg")))
            .then_some("image/svg+xml")
    }
}

// ---- the video request -------------------------------------------------------------------------

/// The added pictures a video request sends, with their `frame_type`: its start frame
/// (`first_frame`) and end frame (`last_frame`).
fn frames(req: &MediaRequest) -> impl Iterator<Item = (&RefImage, &'static str)> {
    req.refs.iter().filter_map(|picture| match picture.role {
        RefRole::StartFrame => Some((picture, "first_frame")),
        RefRole::EndFrame => Some((picture, "last_frame")),
        RefRole::Reference => None,
    })
}

/// One `frame_images` item: `{"type": "image_url", "image_url": {"url": …}, "frame_type": …}`.
fn frame_item(url: String, frame_type: &str) -> Value {
    json!({"type": "image_url", "image_url": {"url": url}, "frame_type": frame_type})
}

/// The video request's body: `model` and `prompt`, the typed settings of [`VIDEO_FIELDS`]
/// that are set (`size` in place of `resolution` and `aspect_ratio`), and the frames as
/// `frame_images`. Never `provider` or any other key.
fn video_body(req: &MediaRequest, frames: Vec<Value>) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(req.model.clone()));
    body.insert("prompt".to_owned(), Value::String(req.prompt.clone()));
    let sized = req.params.contains_key("size");
    for key in VIDEO_FIELDS {
        if sized && matches!(key, "resolution" | "aspect_ratio") {
            continue;
        }
        if let Some(value) = req.params.get(key) {
            body.insert(key.to_owned(), value.clone());
        }
    }
    if !frames.is_empty() {
        body.insert("frame_images".to_owned(), Value::Array(frames));
    }
    Value::Object(body)
}

// ---- the video answers -------------------------------------------------------------------------

/// The job's id in the answer to a video request (`{"id": …, "polling_url": …, "status": …}`).
fn job_id(answer: &[u8]) -> Result<String, MediaError> {
    let value: Value = serde_json::from_slice(answer)
        .map_err(|err| unreadable(format!("the answer is not a video job ({err})")))?;
    value
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| unreadable("the answer has no job id".to_owned()))
}

/// What a check's answer says: `pending`, `in_progress` (with `progress`), `completed`, or
/// `failed`, `cancelled` and `expired`. Any other status is an answer Studio cannot read.
fn job_state(answer: &Value, key: Option<&str>) -> Result<JobState, MediaError> {
    let status = answer
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(match status {
        "pending" => JobState::Pending,
        "in_progress" => JobState::Running {
            progress: answer.get("progress").and_then(progress_of),
        },
        "completed" => JobState::Done(job_done(answer)),
        "failed" | "cancelled" | "expired" => JobState::Failed(job_failure(status, answer, key)),
        other => {
            return Err(unreadable(format!(
                "the job's status is not one Studio knows ({})",
                scrub_text(&bounded(other), key)
            )));
        }
    })
}

/// How far along a job is, from 0 to 1: a number from 0 to 1 as it is, one up to 100 as a
/// percentage; `None` for anything else.
fn progress_of(value: &Value) -> Option<f32> {
    let n = value.as_f64().filter(|n| n.is_finite() && *n >= 0.0)?;
    let fraction = if n <= 1.0 {
        n
    } else if n <= 100.0 {
        n / 100.0
    } else {
        return None;
    };
    Some(fraction as f32)
}

/// A completed job: as many results as it lists links for (at least one) and its reported
/// cost. Its signed `urls` are kept only when it gives no `unsigned_urls`: those are on
/// OpenRouter's own host and fetched with the key, so the signed links are needed only when
/// they are all there is. They are kept in order up to the first that cannot be read, so
/// link `n` is always result `n`'s.
fn job_done(answer: &Value) -> JobDone {
    let listed = |key: &str| answer.get(key).and_then(Value::as_array);
    let unsigned = listed("unsigned_urls").map_or(0, Vec::len);
    let signed = listed("urls")
        .filter(|_| unsigned == 0)
        .into_iter()
        .flatten()
        .map_while(signed_url)
        .collect();
    JobDone {
        outputs: unsigned.max(listed("urls").map_or(0, Vec::len)).max(1),
        signed,
        cost: reported(answer.pointer("/usage/cost").and_then(Value::as_f64)),
    }
}

/// One signed link: `{"url": …, "expires_at": …}`, or the bare link; http or https only.
fn signed_url(item: &Value) -> Option<SignedUrl> {
    let (link, expires_at) = match item {
        Value::String(link) => (link.as_str(), None),
        Value::Object(map) => (
            map.get("url")?.as_str()?,
            map.get("expires_at").and_then(Value::as_i64),
        ),
        _ => return None,
    };
    let url = Url::parse(link.trim())
        .ok()
        .filter(|url| matches!(url.scheme(), "http" | "https"))?;
    Some(SignedUrl { url, expires_at })
}

/// A job that ended without a result. The message is the answer's `error` (a text, or an
/// object with `message` and `code`), else a plain sentence; one that speaks of moderation
/// is [`FailureKind::Blocked`], and an expired job is [`FailureKind::Expired`]. OpenRouter
/// does not bill a job that failed or was cancelled; an expired one had finished, so what it
/// cost is unknown; a cost the answer reports decides over both. The request id is the
/// answer's `generation_id`, else the job's id.
fn job_failure(status: &str, answer: &Value, key: Option<&str>) -> MediaFailure {
    let clean = |text: &str| scrub_text(&bounded(text), key);
    let error = answer.get("error").filter(|error| !error.is_null());
    let text = match error {
        Some(Value::String(text)) => Some(text.as_str()),
        Some(Value::Object(map)) => ["message", "detail"]
            .iter()
            .find_map(|name| map.get(*name).and_then(Value::as_str)),
        _ => None,
    }
    .map(str::trim)
    .filter(|text| !text.is_empty());
    let message = text.map_or_else(
        || {
            match status {
                "cancelled" => "The job was cancelled.",
                "expired" => "The result has expired.",
                _ => "The job failed.",
            }
            .to_owned()
        },
        clean,
    );
    let kind = if status == "expired" {
        FailureKind::Expired
    } else if speaks_of_moderation(&message) {
        FailureKind::Blocked
    } else {
        FailureKind::Other
    };
    let charged = match reported(answer.pointer("/usage/cost").and_then(Value::as_f64)) {
        Some(cost) => Some(cost.usd > 0.0),
        None => (status != "expired").then_some(false),
    };
    MediaFailure {
        kind,
        stage: None,
        charged,
        company_code: error
            .and_then(|error| error.get("code"))
            .and_then(Value::as_str)
            .map(clean),
        request_id: ["generation_id", "id"]
            .iter()
            .find_map(|name| answer.get(*name).and_then(Value::as_str))
            .map(clean),
        status: None,
        message,
        retry_after: None,
    }
}

#[cfg(test)]
mod tests {
    use reqwest::header::HeaderValue;

    use super::*;
    use crate::media::{EndpointCompany, Price};

    fn company(slug: &str, passthrough: &[&str]) -> EndpointCompany {
        EndpointCompany {
            slug: slug.to_owned(),
            tag: slug.to_owned(),
            name: slug.to_owned(),
            passthrough: passthrough.iter().map(|p| (*p).to_owned()).collect(),
        }
    }

    fn model(companies: Vec<EndpointCompany>) -> MediaModel {
        MediaModel {
            id: "maker/picture-1".to_owned(),
            display_name: "Picture 1".to_owned(),
            family: "maker/picture".to_owned(),
            kind: MediaKind::Image,
            accepts_images: false,
            params: Vec::new(),
            price: Price::Unknown,
            notes: Vec::new(),
            usable: None,
            created: None,
            companies,
        }
    }

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn company_settings_go_to_each_company_that_lists_them() {
        let m = model(vec![
            company("openai", &["moderation", "complexity"]),
            company("azure", &["moderation"]),
        ]);
        let extra = object(json!({"moderation": "low", "complexity": "high", "steps": 30}));
        let (top, options) = place_extra(&m, extra.clone(), None);
        assert_eq!(top, object(json!({"steps": 30})));
        assert_eq!(
            options["openai"],
            object(json!({"moderation": "low", "complexity": "high"}))
        );
        assert_eq!(options["azure"], object(json!({"moderation": "low"})));

        // Pinned with `only`: the pinned company alone.
        let only = Routing {
            order: vec!["azure".to_owned()],
            only: true,
            allow_fallbacks: false,
            zdr: false,
        };
        let (top, options) = place_extra(&m, extra.clone(), Some(&only));
        assert_eq!(top, object(json!({"steps": 30})));
        assert_eq!(options.len(), 1);
        assert_eq!(options["azure"], object(json!({"moderation": "low"})));

        // An order without `only` may still reach every company.
        let order = Routing {
            only: false,
            allow_fallbacks: true,
            ..only
        };
        assert_eq!(place_extra(&m, extra, Some(&order)).1.len(), 2);
    }

    #[test]
    fn the_body_carries_routing_and_never_lets_a_setting_replace_the_request() {
        let req = MediaRequest {
            kind: MediaKind::Image,
            model: "maker/picture-1".to_owned(),
            prompt: "a fox".to_owned(),
            params: object(json!({"aspect_ratio": "16:9"})),
            extra: object(json!({"model": "other", "steps": 30})),
            refs: Vec::new(),
            routing: Some(Routing {
                order: vec!["a".to_owned(), "b".to_owned()],
                only: false,
                allow_fallbacks: true,
                zdr: true,
            }),
            provider_options: BTreeMap::new(),
        };
        let body = picture_body(&req, Vec::new());
        assert_eq!(
            body,
            json!({
                "model": "maker/picture-1",
                "prompt": "a fox",
                "aspect_ratio": "16:9",
                "steps": 30,
                "provider": {"order": ["a", "b"], "allow_fallbacks": true, "zdr": true}
            })
        );
    }

    #[test]
    fn statuses_become_failures() {
        let none = HeaderMap::new();
        let kind = |status| openrouter_failure(status, &none, b"", None).kind;
        assert_eq!(kind(401), FailureKind::Unauthorized);
        assert_eq!(kind(402), FailureKind::NoCredit);
        assert_eq!(kind(404), FailureKind::NotFound);
        assert_eq!(kind(429), FailureKind::RateLimited);
        assert_eq!(kind(400), FailureKind::BadRequest);
        assert_eq!(kind(418), FailureKind::Other);

        let refused = openrouter_failure(402, &none, b"", None);
        assert_eq!(
            refused.charged,
            Some(false),
            "a refused request is not charged"
        );
        assert_eq!(refused.message, "HTTP 402");

        let busy = openrouter_failure(503, &headers(&[("retry-after", "20")]), b"", None);
        assert_eq!(busy.kind, FailureKind::Server);
        assert_eq!(busy.charged, None, "only a 502 says it was not charged");
        assert_eq!(busy.retry_after, Some(std::time::Duration::from_secs(20)));
    }

    #[test]
    fn the_request_id_comes_from_the_body_or_the_header() {
        let from_body = openrouter_failure(
            400,
            &headers(&[("x-request-id", "req_header")]),
            br#"{"id": "gen-123", "error": {"code": "bad_size", "message": "Bad size"}}"#,
            None,
        );
        assert_eq!(from_body.request_id.as_deref(), Some("gen-123"));
        assert_eq!(from_body.company_code.as_deref(), Some("bad_size"));
        assert_eq!(from_body.message, "Bad size");
        let from_header = openrouter_failure(
            400,
            &headers(&[("x-request-id", "req_header")]),
            b"not json",
            None,
        );
        assert_eq!(from_header.request_id.as_deref(), Some("req_header"));
        assert_eq!(from_header.message, "not json");
    }

    #[test]
    fn moderation_is_a_block_also_from_a_400() {
        let f = openrouter_failure(
            400,
            &HeaderMap::new(),
            br#"{"error": {"code": 400, "message": "Request refused", "metadata": {"reasons": ["hate"], "flagged_input": "..."}}}"#,
            None,
        );
        assert_eq!(f.kind, FailureKind::Blocked);
        assert_eq!(f.charged, Some(false));
        assert_eq!(f.stage, Some(Stage::Prompt));
        // The same words on a 500 are a server failure.
        let f = openrouter_failure(
            500,
            &HeaderMap::new(),
            br#"{"error": {"message": "moderation service down"}}"#,
            None,
        );
        assert_eq!(f.kind, FailureKind::Server);
    }

    #[test]
    fn picture_types_are_read_from_their_bytes() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(sniff(b"GIF89a...."), Some("image/gif"));
        assert_eq!(sniff(b"RIFF\x10\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(
            sniff(b"\xef\xbb\xbf <svg xmlns=\"\">"),
            Some("image/svg+xml")
        );
        assert_eq!(
            sniff(b"<?xml version=\"1.0\"?>\n<svg>"),
            Some("image/svg+xml")
        );
        assert_eq!(sniff(b"<html>"), None);
    }

    #[test]
    fn progress_is_read_as_a_fraction() {
        assert_eq!(progress_of(&json!(0.4)), Some(0.4));
        assert_eq!(progress_of(&json!(1)), Some(1.0));
        assert_eq!(progress_of(&json!(75)), Some(0.75), "a percentage");
        assert_eq!(progress_of(&json!(0)), Some(0.0));
        assert_eq!(progress_of(&json!(150)), None);
        assert_eq!(progress_of(&json!(-0.5)), None);
        assert_eq!(progress_of(&json!("40%")), None);
    }

    #[test]
    fn a_job_answer_becomes_a_state() {
        let state = |answer: Value| job_state(&answer, None);
        assert_eq!(
            state(json!({"status": "in_progress"})).unwrap(),
            JobState::Running { progress: None }
        );
        // With both kinds of link, the content endpoint is used: no signed links are kept.
        let Ok(JobState::Done(job)) = state(json!({
            "status": "completed",
            "unsigned_urls": ["https://openrouter.ai/api/v1/videos/v/content?index=0"],
            "urls": ["https://cdn.example/a.mp4", "https://cdn.example/b.mp4"]
        })) else {
            panic!("done")
        };
        assert_eq!((job.outputs, job.signed.len(), job.cost), (2, 0, None));
        // Signed links stay in order, up to the first that cannot be read.
        let Ok(JobState::Done(job)) = state(json!({
            "status": "completed",
            "urls": ["https://cdn.example/a.mp4", "file:///etc/passwd", "https://cdn.example/c.mp4"]
        })) else {
            panic!("done")
        };
        assert_eq!(job.outputs, 3);
        assert_eq!(job.signed.len(), 1);
        assert_eq!(job.signed[0].url.as_str(), "https://cdn.example/a.mp4");
        assert_eq!(job.signed[0].expires_at, None);
        // A completed job without links still has its one result at the content endpoint.
        let Ok(JobState::Done(job)) = state(json!({"status": "completed"})) else {
            panic!("done")
        };
        assert_eq!(job.outputs, 1);

        assert!(matches!(
            state(json!({"status": "queued"})),
            Err(MediaError::Unknown(ProviderError::Decode(_)))
        ));
        assert!(state(json!({"id": "v"})).is_err(), "no status");
    }

    #[test]
    fn a_job_that_ended_without_a_result_says_why() {
        let failed = |answer: Value| match job_state(&answer, Some("sk-or-v1-secret")) {
            Ok(JobState::Failed(f)) => f,
            other => panic!("expected a failed state, got {other:?}"),
        };
        let f = failed(json!({"id": "vid_9", "status": "cancelled"}));
        assert_eq!(f.kind, FailureKind::Other);
        assert_eq!(f.charged, Some(false));
        assert_eq!(f.message, "The job was cancelled.");
        assert_eq!(f.request_id.as_deref(), Some("vid_9"));
        assert_eq!(f.status, None);

        let f = failed(json!({
            "status": "failed",
            "error": {"code": "content_policy", "message": "The output was flagged by the provider"}
        }));
        assert_eq!(f.kind, FailureKind::Blocked);
        assert_eq!(f.company_code.as_deref(), Some("content_policy"));
        assert_eq!(f.charged, Some(false));

        // A cost the answer reports decides whether it was charged.
        let f = failed(json!({"status": "failed", "usage": {"cost": 0.12}}));
        assert_eq!(f.charged, Some(true));
        let f = failed(json!({"status": "expired", "usage": {"cost": 0}}));
        assert_eq!((f.kind, f.charged), (FailureKind::Expired, Some(false)));

        let f = failed(json!({"status": "failed", "error": "bad key sk-or-v1-secret"}));
        assert!(!f.message.contains("sk-or-v1-secret"), "{}", f.message);
    }

    #[test]
    fn sizes_are_shown_in_decimal_units() {
        assert_eq!(size_text(28), "28 B");
        assert_eq!(size_text(340_000), "340 KB");
        assert_eq!(size_text(1_234_567), "1.2 MB");
    }
}
