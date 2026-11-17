//! OpenRouter's model lists, picture prices and pictures (0.1.0 Studio).
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
//! - **Failures**: [`openrouter_failure`]. A moderation refusal and a 502 are not charged.

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
    RequestPreview, Routing, Saved, Stage, Started, base_failure,
};
use crate::client::{OnceBody, OnceReply, OpenAiClient, join};
use crate::cloud::scrub_text;
use crate::error::{ProviderError, bounded};

/// Why a video request is refused here: video jobs are not offered through this provider yet.
const NO_VIDEO_YET: &str = "OpenRouter video jobs are not available yet.";

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

    /// A read through the endpoint's client: a model list or a price answer.
    async fn read(&self, url: Url) -> Result<Value, MediaError> {
        self.client
            .get_json(url)
            .await
            .map_err(|err| read_failed(err, self.key()))
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
            MediaKind::Video => Value::Null,
        };
        RequestPreview {
            method: "POST".to_owned(),
            url: url.to_string(),
            host: self.host(),
            body,
        }
    }

    async fn start(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        if req.kind == MediaKind::Video {
            return Err(not_offered());
        }
        let mut items = Vec::new();
        for picture in references(req) {
            let bytes = tokio::fs::read(&picture.path).await.map_err(|err| {
                MediaError::NotSent(ProviderError::InvalidRequest(format!(
                    "could not read an added picture: {err}"
                )))
            })?;
            let url = format!("data:{};base64,{}", picture.mime, STANDARD.encode(&bytes));
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

    async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
        Err(not_offered())
    }

    async fn download(
        &self,
        _remote_id: &str,
        _job: &JobDone,
        _index: usize,
        _dest: &Path,
    ) -> Result<Fetched, MediaError> {
        Err(not_offered())
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
    let lower = failure.message.to_lowercase();
    let moderation = lower.contains("moderation")
        || lower.contains("flagged")
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

/// The error for a request this provider does not make yet: refused before sending.
fn not_offered() -> MediaError {
    MediaError::NotSent(ProviderError::InvalidRequest(NO_VIDEO_YET.to_owned()))
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
    let cost = parsed
        .usage
        .and_then(|usage| usage.cost)
        .filter(|usd| usd.is_finite() && *usd >= 0.0)
        .map(|usd| Cost {
            usd,
            reported: true,
        });
    Ok((saved, cost))
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
    fn sizes_are_shown_in_decimal_units() {
        assert_eq!(size_text(28), "28 B");
        assert_eq!(size_text(340_000), "340 KB");
        assert_eq!(size_text(1_234_567), "1.2 MB");
    }
}
