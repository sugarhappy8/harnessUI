//! Pictures and video (0.1.0 Studio): the types every media company shares and the
//! [`MediaProvider`] trait the companies sit behind.
//!
//! - [`spec`]: the controls each model offers ([`ParamSpec`]), read from the companies' own
//!   catalogues, and [`validate`], which checks a request against them;
//! - [`price`]: what a model costs ([`Price`]) and what a run would cost ([`estimate`]);
//! - [`openrouter`]: OpenRouter's lists, prices, pictures and video jobs ([`OpenRouterMedia`]);
//! - [`openai`]: OpenAI's picture models, read against a bundled, dated settings file, and its
//!   pictures and edits ([`OpenAiImages`]).
//!
//! The companies are OpenRouter (pictures and video), OpenAI (pictures) and this computer
//! ([`Company::Local`]). A request goes only to the chosen endpoint's host; a run on this
//! computer sends nothing anywhere.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::Url;

use crate::client::OpenAiClient;
use crate::cloud::{parse_retry_after, scrub_text};
use crate::error::{ProviderError, bounded};

mod multipart;
pub mod openai;
pub mod openrouter;
pub mod price;
pub mod spec;

pub use crate::cloud::media_company;
pub use openai::OpenAiImages;
pub use openrouter::OpenRouterMedia;
pub use price::{
    Mode, OpenAiRates, Price, PriceChoice, PriceRow, Table, choice_for, estimate, openai_cost,
    parse_image_endpoints, parse_pricing_skus,
};
pub use spec::{
    Canonical, Choice, EndpointCompany, Invalid, Layer, OPENAI_SIZE_RULES, ParamKind, ParamSpec,
    Requires, SEED_MAX, SizeProblem, SizeRules, check_size, endpoint_companies, family_of,
    openrouter_image_models, openrouter_video_models, parse_catalog, validate,
};

/// Pictures or video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    /// Still pictures (an animated GIF counts as a picture).
    Image,
    /// Video clips.
    Video,
}

/// Who makes the pictures and video.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Company {
    /// OpenRouter: pictures and video, run by the companies it passes the request on to.
    OpenRouter,
    /// OpenAI: pictures.
    OpenAi,
    /// This computer, through the Studio engine: nothing is sent anywhere.
    Local,
}

/// A picture or video model and the controls it offers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaModel {
    /// The id sent to the company, for example `google/veo-3.1`.
    pub id: String,
    /// The name shown, without the maker's prefix: "Veo 3.1".
    pub display_name: String,
    /// The maker and the model's name without its version ([`family_of`]).
    pub family: String,
    /// Pictures or video.
    pub kind: MediaKind,
    /// Whether the model takes added pictures (references, or a start or end frame).
    pub accepts_images: bool,
    /// The controls, simple ones first.
    pub params: Vec<ParamSpec>,
    /// What a run costs.
    pub price: Price,
    /// Plain sentences shown with the model, for example about watermarks.
    pub notes: Vec<String>,
    /// Why Studio cannot use the model, when it cannot.
    pub usable: Option<String>,
    /// When the company added the model, in seconds since 1970.
    pub created: Option<i64>,
    /// The companies that run the model, from the saved `/endpoints` answer (OpenRouter
    /// pictures); empty when none is saved.
    pub companies: Vec<EndpointCompany>,
}

/// What an added picture is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RefRole {
    /// A picture to follow or change.
    Reference,
    /// The first frame of a video.
    StartFrame,
    /// The last frame of a video.
    EndFrame,
}

/// A picture added to a request, as it is kept in the gallery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefImage {
    /// What the picture is for.
    pub role: RefRole,
    /// Its media type, for example `image/png`.
    pub mime: String,
    /// Where the file is.
    pub path: PathBuf,
    /// The SHA-256 of the file, in lowercase hex.
    pub sha256: String,
    /// The file's size.
    pub size_bytes: u64,
}

/// Which companies may run an OpenRouter request (sent as `provider`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Routing {
    /// The companies to try, in order, as `provider_tag` values.
    pub order: Vec<String>,
    /// Send `order` as `only`: no company outside it.
    pub only: bool,
    /// Whether another company may run the request when those in `order` cannot.
    pub allow_fallbacks: bool,
    /// Only companies that keep no data.
    pub zdr: bool,
}

/// A request for pictures or a video, as the user set it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    /// Pictures or video.
    pub kind: MediaKind,
    /// The model's id.
    pub model: String,
    /// The prompt, sent as written.
    pub prompt: String,
    /// Values of the model's typed fields, by key. A field that is not set is not sent.
    pub params: Map<String, Value>,
    /// Extra settings sent at the top level of the request.
    pub extra: Map<String, Value>,
    /// The added pictures.
    pub refs: Vec<RefImage>,
    /// Which companies may run it (OpenRouter).
    pub routing: Option<Routing>,
    /// Extra settings for one company, by its `provider_slug` (OpenRouter pictures).
    pub provider_options: BTreeMap<String, Map<String, Value>>,
}

/// A company's model lists as fetched, kept as they came.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCatalog {
    /// The picture model list.
    pub images: Value,
    /// The video model list, for a company that makes video.
    pub videos: Option<Value>,
}

/// A result file on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    /// Where the file is.
    pub path: PathBuf,
    /// Its media type.
    pub mime: String,
    /// The file's size.
    pub size_bytes: u64,
    /// The width in pixels, when known.
    pub width: Option<u32>,
    /// The height in pixels, when known.
    pub height: Option<u32>,
    /// A video's length in milliseconds, when known.
    pub duration_ms: Option<u64>,
}

/// What a run cost.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    /// The amount in US dollars.
    pub usd: f64,
    /// Whether the company reported it (`false`: worked out from its usage figures).
    pub reported: bool,
}

/// What sending a request gave.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Started {
    /// The results came back with the answer (pictures).
    Done {
        /// The saved files.
        outputs: Vec<Saved>,
        /// What the run cost, when known.
        cost: Option<Cost>,
    },
    /// The company started a job to check on (video).
    Job {
        /// The company's id for the job.
        remote_id: String,
    },
}

/// A finished job.
#[derive(Debug, Clone, PartialEq)]
pub struct JobDone {
    /// How many results the job made.
    pub outputs: usize,
    /// Signed links to the results on the company's file host, when it gave them.
    pub signed: Vec<SignedUrl>,
    /// What the job cost, when known.
    pub cost: Option<Cost>,
}

/// A link to a result that carries its own permission, fetched without the key.
#[derive(Debug, Clone, PartialEq)]
pub struct SignedUrl {
    /// The link.
    pub url: Url,
    /// When it stops working, in seconds since 1970, when known.
    pub expires_at: Option<i64>,
}

/// Where a job is.
#[derive(Debug, Clone, PartialEq)]
pub enum JobState {
    /// Waiting to start.
    Pending,
    /// Being made.
    Running {
        /// How far along, from 0 to 1, when the company says.
        progress: Option<f32>,
    },
    /// Finished.
    Done(JobDone),
    /// Failed.
    Failed(MediaFailure),
    /// The company no longer knows the job.
    Gone,
}

/// Where a downloaded result went.
#[derive(Debug, Clone, PartialEq)]
pub enum Fetched {
    /// Saved on this computer.
    Saved(Saved),
    /// Only at this link on the company's file host (fetched without the key, elsewhere).
    Elsewhere(Url),
}

/// The exact request a run would send, with the key removed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPreview {
    /// The HTTP method (`run` for a run on this computer).
    pub method: String,
    /// The full URL.
    pub url: String,
    /// The host it goes to.
    pub host: String,
    /// The body, with added pictures shown by their hash and size.
    pub body: Value,
}

/// Where a company's safety filter stopped a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    /// It refused the prompt or the added pictures.
    Prompt,
    /// It stopped the result after it was made.
    Output,
}

/// What kind of failure a run had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FailureKind {
    /// The company's safety filter refused it.
    Blocked,
    /// The key was refused.
    Unauthorized,
    /// The account has no credit left.
    NoCredit,
    /// The company needs the organization verified first (OpenAI).
    OrgVerification,
    /// Too many requests.
    RateLimited,
    /// The company refused the request as it was.
    BadRequest,
    /// The model or job was not found.
    NotFound,
    /// The result is no longer there.
    Expired,
    /// The company's server failed.
    Server,
    /// Any other failure.
    Other,
    /// This computer ran out of memory (local runs only).
    OutOfMemory,
    /// The Studio engine is not installed (local runs only).
    EngineMissing,
    /// The Studio engine failed (local runs only).
    EngineFailed,
    /// A model file is damaged or not what it says (local runs only).
    ModelFileBad,
    /// The graphics card stopped answering (local runs only).
    DeviceLost,
}

/// A run that failed after the company answered, or on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaFailure {
    /// What kind of failure.
    pub kind: FailureKind,
    /// Where a safety filter stopped it, when the company says.
    pub stage: Option<Stage>,
    /// Whether it was charged, when known. Always `Some(false)` for a local run.
    pub charged: Option<bool>,
    /// The company's own error code, for example `moderation_blocked`.
    pub company_code: Option<String>,
    /// The company's request id, for an appeal.
    pub request_id: Option<String>,
    /// The HTTP status, when there was one.
    pub status: Option<u16>,
    /// The company's message, without the key.
    pub message: String,
    /// How long the company asked to wait before trying again.
    pub retry_after: Option<Duration>,
}

/// A failure writing results on this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[serde(rename_all = "camelCase")]
pub enum LocalIo {
    /// The disk is full.
    #[error("the disk is full")]
    DiskFull,
    /// The result is larger than the limit, in bytes.
    #[error("the result is larger than {0} bytes")]
    TooLarge(u64),
    /// Any other error writing the file.
    #[error("could not write the result: {0}")]
    Io(String),
}

impl LocalIo {
    /// A failed write: a full disk by name, anything else with its text.
    pub(crate) fn from_io(err: &std::io::Error) -> Self {
        if err.kind() == std::io::ErrorKind::StorageFull {
            Self::DiskFull
        } else {
            Self::Io(err.to_string())
        }
    }
}

/// Why a media request did not give a result.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum MediaError {
    /// The request was not sent (refused before sending, or the connection never opened):
    /// nothing was charged.
    #[error("not sent: {0}")]
    NotSent(ProviderError),
    /// The company answered with a failure.
    #[error("{}", .0.message)]
    Answered(MediaFailure),
    /// The request was sent but no answer came (a timeout or a dropped connection): it may
    /// have been charged.
    #[error("no answer: {0}")]
    Unknown(ProviderError),
    /// Writing the result on this computer failed.
    #[error("{0}")]
    Local(LocalIo),
}

/// Limits on a media request's answers and waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaLimits {
    /// The largest picture answer, streamed to a file (base64 pictures inside JSON).
    pub response_cap: u64,
    /// The largest picture file.
    pub picture_cap: u64,
    /// The largest video file.
    pub video_cap: u64,
    /// The longest a picture request may take, start to end.
    pub picture_timeout: Duration,
    /// The longest one check on a video job may take.
    pub check_timeout: Duration,
    /// The longest a result download may go without data.
    pub download_idle: Duration,
}

impl Default for MediaLimits {
    fn default() -> Self {
        const MIB: u64 = 1024 * 1024;
        Self {
            response_cap: 192 * MIB,
            picture_cap: 50 * MIB,
            video_cap: 1024 * MIB,
            picture_timeout: Duration::from_secs(300),
            check_timeout: Duration::from_secs(30),
            download_idle: Duration::from_secs(60),
        }
    }
}

/// A company that makes pictures or video.
#[async_trait]
pub trait MediaProvider: Send + Sync {
    /// Which company this is.
    fn company(&self) -> Company;

    /// The host requests go to (empty for this computer).
    fn host(&self) -> String;

    /// Fetches the company's model lists.
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError>;

    /// Fetches the price answer for one picture model (OpenRouter's `/endpoints`);
    /// `Value::Null` for a company whose list carries its prices.
    async fn fetch_image_price(&self, model: &str) -> Result<Value, MediaError>;

    /// The exact request [`MediaProvider::start`] would send, with the key removed.
    fn preview(&self, req: &MediaRequest) -> RequestPreview;

    /// Sent at most once. Pictures come back done; a video comes back as a job.
    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError>;

    /// Checks on a started job.
    async fn check(&self, remote_id: &str) -> Result<JobState, MediaError>;

    /// Downloads result `index` of a finished job to `dest`.
    async fn download(
        &self,
        remote_id: &str,
        job: &JobDone,
        index: usize,
        dest: &Path,
    ) -> Result<Fetched, MediaError>;

    /// Stops a started run. Cloud keeps this no-op (closing the connection is all it can do);
    /// the local provider kills the engine.
    async fn cancel(&self, _remote_id: &str) -> Result<(), MediaError> {
        Ok(())
    }
}

/// The media provider for an endpoint's client: OpenRouter's for an OpenRouter endpoint,
/// OpenAI's for an OpenAI endpoint; none for any other kind.
pub fn media_provider(client: OpenAiClient) -> Option<Arc<dyn MediaProvider>> {
    match media_company(client.config().kind)? {
        Company::OpenRouter => Some(Arc::new(OpenRouterMedia::new(client))),
        Company::OpenAi => Some(Arc::new(OpenAiImages::new(client))),
        Company::Local => None,
    }
}

/// What every cloud company's error answer says: 401 [`FailureKind::Unauthorized`], 402
/// [`FailureKind::NoCredit`], 404 [`FailureKind::NotFound`], 429
/// [`FailureKind::RateLimited`], 400, 413 and 422 [`FailureKind::BadRequest`], 5xx
/// [`FailureKind::Server`], anything else [`FailureKind::Other`]. A 4xx refused the request,
/// so it was not charged; for a 5xx the charge is unknown. The request id comes from the
/// body's `id` or the `x-request-id` header, the wait from `retry-after`, and the message
/// from the body; every text is cut to a bounded length and never carries the key. Each
/// company adds its own codes on top.
pub(crate) fn base_failure(
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
    key: Option<&str>,
) -> MediaFailure {
    let value = serde_json::from_slice::<Value>(body).ok();
    let kind = match status {
        401 => FailureKind::Unauthorized,
        402 => FailureKind::NoCredit,
        404 => FailureKind::NotFound,
        429 => FailureKind::RateLimited,
        400 | 413 | 422 => FailureKind::BadRequest,
        500..=599 => FailureKind::Server,
        _ => FailureKind::Other,
    };
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
    };
    let clean = |text: &str| scrub_text(&bounded(text), key);
    let request_id = value
        .as_ref()
        .and_then(|v| v.get("id"))
        .and_then(Value::as_str)
        .or_else(|| header("x-request-id"))
        .map(clean);
    let company_code = value
        .as_ref()
        .and_then(|v| v.pointer("/error/code"))
        .and_then(Value::as_str)
        .map(clean);
    MediaFailure {
        kind,
        stage: None,
        charged: (400..500).contains(&status).then_some(false),
        company_code,
        request_id,
        status: Some(status),
        message: clean(&failure_text(status, value.as_ref(), body)),
        retry_after: header("retry-after").and_then(parse_retry_after),
    }
}

/// The message in an error answer: the error object's `message` (or `detail`), a bare error
/// string, else the body as text, else the status.
fn failure_text(status: u16, value: Option<&Value>, body: &[u8]) -> String {
    if let Some(value) = value {
        let inner = value.get("error").filter(|e| !e.is_null()).unwrap_or(value);
        let message = match inner {
            Value::String(text) => Some(text.as_str()),
            Value::Object(map) => ["message", "detail"]
                .iter()
                .find_map(|key| map.get(*key).and_then(Value::as_str)),
            _ => None,
        };
        if let Some(message) = message.map(str::trim).filter(|m| !m.is_empty()) {
            return message.to_owned();
        }
    }
    let text = String::from_utf8_lossy(body);
    if text.trim().is_empty() {
        format!("HTTP {status}")
    } else {
        text.into_owned()
    }
}
