//! OpenAI's GPT Image models (0.1.0 Studio): pictures, and edits with words.
//!
//! - **Settings and prices** come from a bundled, dated file (`specs/openai_images.json`),
//!   checked against OpenAI's pricing page, image guide, API reference and deprecations page
//!   on the date it names. An id gets the entry whose prefix is the longest one that is the
//!   whole id or is followed only by a snapshot's name: a dated snapshot
//!   (`gpt-image-2-2026-04-21`, or `-latest`) is its model, while a newer version
//!   (`gpt-image-2.5-flare`) or another model with the same start (`gpt-image-2-pro`) never
//!   gets an older model's settings or prices. An id with no entry gets the basic fields
//!   (size, quality, how many) and a note that the settings may not match it.
//! - **Lists**: `GET {api}/models` with the key; [`openai_image_models`] keeps the GPT Image
//!   models.
//! - **Pictures**: with no added pictures, `POST {api}/images/generations` with a JSON body;
//!   with added pictures, `POST {api}/images/edits` with a `multipart/form-data` body that
//!   sends them as `image[]` files. Either is sent at most once ([`OpenAiClient::post_once`]).
//!   The answer's pictures (`data[].b64_json`) are written one at a time. OpenAI reports no
//!   cost: it is worked out from the answer's `usage` and the entry's token prices
//!   ([`openai_cost`]).
//! - **Failures**: [`openai_failure`].

use std::collections::BTreeMap;
use std::io::BufReader;
use std::path::Path;
use std::sync::LazyLock;

use async_trait::async_trait;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::{Map, Value};
use url::Url;

use super::multipart::{FilePart, form};
use super::openrouter::{
    AnswerPicture, naming_the_answer, picture_label, unreadable, write_pictures,
};
use super::price::{OpenAiRates, PriceRow, Table, openai_cost};
use super::spec::{choice_label, entries, field, gif_fields, is_company_default, unmet};
use super::{
    Canonical, Choice, Company, Cost, FailureKind, Fetched, JobDone, JobState, Layer, LocalIo,
    MediaError, MediaFailure, MediaKind, MediaLimits, MediaModel, MediaProvider, MediaRequest,
    OPENAI_SIZE_RULES, ParamKind, ParamSpec, Price, RawCatalog, RefImage, RefRole, RequestPreview,
    Requires, Saved, Stage, Started, base_failure, family_of, unanswered,
};
use crate::client::{OnceBody, OnceReply, OpenAiClient, join};
use crate::cloud::{openai_media_kind, scrub_text};
use crate::error::{ProviderError, bounded};

/// The bundled settings file.
const BUNDLED: &str = include_str!("specs/openai_images.json");

/// The note on a model the settings file has no entry for.
const NEW_MODEL: &str = "Settings may not match this new model.";

/// The help of the Safety level field.
const MODERATION_HELP: &str =
    "Low makes OpenAI's filter less strict; it still refuses what its rules forbid.";

/// The sizes offered by name, with their names on screen. Any other `WxH` within
/// [`OPENAI_SIZE_RULES`] can be typed.
const SIZES: [(&str, &str); 4] = [
    ("auto", "Auto"),
    ("1024x1024", "Square (1024 × 1024)"),
    ("1536x1024", "Landscape (1536 × 1024)"),
    ("1024x1536", "Portrait (1024 × 1536)"),
];

/// The quality steps of a model with no entry.
const BASIC_QUALITIES: [&str; 4] = ["auto", "low", "medium", "high"];

/// The most pictures one request makes, for a model with no entry (OpenAI's API reference).
const BASIC_MAX_N: u32 = 10;

/// The form field of an added picture.
const REFERENCES: &str = "image[]";

/// The settings an edit's form can carry besides `model`, `prompt` and the pictures: those
/// OpenAI's `/images/edits` reference documents (checked on 2026-11-17). It also documents
/// streaming (`stream`, `partial_images`), the other picture parts (`mask`, `markup`), the
/// SVG author (`svg`) and end-user identifiers, none of which is a Studio setting.
const EDIT_SETTINGS: [&str; 11] = [
    "background",
    "complexity",
    "frame_count",
    "frame_duration_ms",
    "input_fidelity",
    "moderation",
    "n",
    "output_compression",
    "output_format",
    "quality",
    "size",
];

/// Added to the help of a setting the edit endpoint does not take.
const NOT_IN_EDITS: &str = "Not sent when you add pictures.";

/// The bundled settings file: OpenAI's picture models as checked on one date.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenAiSpec {
    /// The day the file was checked against OpenAI's pages, `YYYY-MM-DD`.
    pub checked: String,
    /// The pages it was checked against.
    pub sources: Vec<String>,
    /// Where a figure that may look wrong was read, and what the page shows: `<prefix>: …`
    /// for one model, `all models: …` for every one.
    #[serde(default)]
    pub notes: Vec<String>,
    /// One entry per model, the newest first.
    pub models: Vec<OpenAiEntry>,
}

/// One model's entry in the settings file.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct OpenAiEntry {
    /// The model id, and the start of its dated snapshots' ids.
    pub prefix: String,
    /// The name shown.
    pub label: String,
    /// A plain sentence shown with the model.
    pub note: Option<String>,
    /// The day OpenAI retires the model, `YYYY-MM-DD`.
    pub retires: Option<String>,
    /// The most pictures one request makes.
    pub max_n: u32,
    /// The most pictures one edit takes.
    pub max_refs: u32,
    /// The quality steps.
    pub qualities: Vec<String>,
    /// The file types besides SVG and GIF.
    pub formats: Vec<String>,
    /// Whether it writes SVG.
    pub svg: bool,
    /// Whether it makes animated GIFs.
    pub gif: bool,
    /// Its token prices.
    pub rates: OpenAiRates,
    /// The prices per picture made that OpenAI states, by `<quality>|<size>`, in US dollars.
    /// They leave out added pictures, which OpenAI charges for at a price it does not state.
    pub per_image: BTreeMap<String, f64>,
}

/// Pictures and edits through an OpenAI endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiImages {
    client: OpenAiClient,
    limits: MediaLimits,
}

impl OpenAiImages {
    /// OpenAI pictures through the endpoint's own client, with the default limits.
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

    /// The address a request goes to: the edit endpoint when it sends pictures.
    fn endpoint(&self, req: &MediaRequest) -> Url {
        self.url(if references(req).next().is_some() {
            "images/edits"
        } else {
            "images/generations"
        })
    }

    /// Makes pictures: one request whose answer carries them.
    async fn make(&self, req: &MediaRequest, scratch: &Path) -> Result<Started, MediaError> {
        let body = if references(req).next().is_none() {
            OnceBody::Json(json_body(req))
        } else {
            let mut files = Vec::new();
            for (i, picture) in references(req).enumerate() {
                files.push(file_part(i + 1, picture).await?);
            }
            let (boundary, bytes) = form(&form_fields(req), &files);
            OnceBody::Multipart {
                boundary,
                bytes: bytes.into(),
            }
        };
        let reply = self
            .client
            .post_once(
                self.endpoint(req),
                body,
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
                    let rates = match_spec(&req.model).map(|entry| &entry.rates);
                    tokio::task::spawn_blocking(move || save_answer(&answer, &folder, cap, rates))
                        .await
                        .unwrap_or_else(|err| {
                            Err(MediaError::Local(LocalIo::Io(format!(
                                "the pictures were not written ({err})"
                            ))))
                        })
                };
                let _ = tokio::fs::remove_file(&path).await;
                let (outputs, cost) =
                    saved.map_err(|err| naming_the_answer(err, &headers, self.key(), "OpenAI"))?;
                Ok(Started::Done { outputs, cost })
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
impl MediaProvider for OpenAiImages {
    fn company(&self) -> Company {
        Company::OpenAi
    }

    fn host(&self) -> String {
        self.client
            .api_url()
            .host_str()
            .unwrap_or_default()
            .to_owned()
    }

    /// `GET {api}/models`, with the key. The answer is kept as it came; [`super::parse_catalog`]
    /// keeps its picture models.
    async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
        let images = self
            .client
            .get_json(self.url("models"))
            .await
            .map_err(|err| read_failed(err, self.key()))?;
        Ok(RawCatalog {
            images,
            videos: None,
        })
    }

    /// Nothing to fetch: the prices are in the bundled settings file.
    async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
        Ok(Value::Null)
    }

    /// The request [`MediaProvider::start`] would send. An edit's form is shown as an object
    /// of its text fields, as the text they are sent as, with the added pictures under
    /// `image[]` by their hash and size.
    fn preview(&self, req: &MediaRequest) -> RequestPreview {
        let mut url = self.endpoint(req);
        // Credentials in the address are not shown either.
        let _ = url.set_username("");
        let _ = url.set_password(None);
        let body = if references(req).next().is_none() {
            json_body(req)
        } else {
            let mut body: Map<String, Value> = form_fields(req)
                .into_iter()
                .map(|(name, value)| (name.to_owned(), Value::String(value)))
                .collect();
            let pictures = references(req)
                .enumerate()
                .map(|(i, picture)| Value::String(picture_label(i + 1, picture)))
                .collect();
            body.insert(REFERENCES.to_owned(), Value::Array(pictures));
            Value::Object(body)
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
            MediaKind::Image => self.make(req, scratch).await,
            MediaKind::Video => Err(not_offered("OpenAI makes no video in Studio")),
        }
    }

    async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
        Err(not_offered(
            "OpenAI pictures come back with the answer; there is no job",
        ))
    }

    async fn download(
        &self,
        _remote_id: &str,
        _job: &JobDone,
        _index: usize,
        _dest: &Path,
    ) -> Result<Fetched, MediaError> {
        Err(not_offered(
            "OpenAI pictures come back with the answer; there is no job",
        ))
    }
}

/// What an OpenAI error answer says: [`base_failure`], with the request id from OpenAI's
/// `x-request-id` header, plus:
///
/// - `moderation_blocked` is [`FailureKind::Blocked`], at the stage `moderation_details`
///   names (`input`: the prompt or the added pictures; `output`: the picture made; none
///   otherwise). Whether it was charged is not known.
/// - A 403 whose message asks for the organization to be verified is
///   [`FailureKind::OrgVerification`].
/// - An account with no credit left (`insufficient_quota`, which OpenAI sends as a 429, or
///   `billing_hard_limit_reached`) is [`FailureKind::NoCredit`], not charged, with no wait:
///   waiting does not bring credit back.
///
/// Every 5xx leaves the charge unknown.
pub(crate) fn openai_failure(
    status: u16,
    headers: &HeaderMap,
    body: &[u8],
    key: Option<&str>,
) -> MediaFailure {
    let mut failure = base_failure(status, headers, body, key);
    if let Some(id) = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        failure.request_id = Some(scrub_text(&bounded(id), key));
    }
    if failure.company_code.as_deref() == Some("moderation_blocked") {
        failure.kind = FailureKind::Blocked;
        failure.charged = None;
        let value = serde_json::from_slice::<Value>(body).ok();
        failure.stage = match value
            .as_ref()
            .and_then(|v| v.pointer("/error/moderation_details/moderation_stage"))
            .and_then(Value::as_str)
        {
            Some("input") => Some(Stage::Prompt),
            Some("output") => Some(Stage::Output),
            _ => None,
        };
    } else if status == 403 && asks_for_verification(&failure.message) {
        failure.kind = FailureKind::OrgVerification;
    } else if says_no_credit(failure.company_code.as_deref(), body) {
        failure.kind = FailureKind::NoCredit;
        failure.charged = Some(false);
        failure.retry_after = None;
    }
    failure
}

/// Whether an error answer says the account has no credit left: its code is
/// `insufficient_quota` or `billing_hard_limit_reached`, or its type is `insufficient_quota`.
fn says_no_credit(code: Option<&str>, body: &[u8]) -> bool {
    const NO_CREDIT: [&str; 2] = ["insufficient_quota", "billing_hard_limit_reached"];
    code.is_some_and(|code| NO_CREDIT.contains(&code))
        || serde_json::from_slice::<Value>(body)
            .ok()
            .as_ref()
            .and_then(|v| v.pointer("/error/type"))
            .and_then(Value::as_str)
            == Some("insufficient_quota")
}

/// Whether an error message asks for the organization to be verified.
fn asks_for_verification(message: &str) -> bool {
    let lower = message.to_lowercase();
    (lower.contains("organization") || lower.contains("organisation")) && lower.contains("verif")
}

/// The error for a create OpenAI refused: its failure, except a server failure, whose charge
/// is unknown, which is reported as no answer, like a timeout: the request may have run.
fn refused(status: u16, headers: &HeaderMap, body: &[u8], key: Option<&str>) -> MediaError {
    let failure = openai_failure(status, headers, body, key);
    if failure.kind == FailureKind::Server {
        unanswered(status, failure)
    } else {
        MediaError::Answered(failure)
    }
}

/// A model list read that failed: an error status is OpenAI's answer, and anything else means
/// no answer came.
fn read_failed(err: ProviderError, key: Option<&str>) -> MediaError {
    match err {
        ProviderError::Unauthorized => {
            MediaError::Answered(openai_failure(401, &HeaderMap::new(), b"", key))
        }
        ProviderError::Http { status, message } => MediaError::Answered(openai_failure(
            status,
            &HeaderMap::new(),
            message.as_bytes(),
            key,
        )),
        other => MediaError::NotSent(other),
    }
}

/// A request Studio does not send to OpenAI.
fn not_offered(what: &str) -> MediaError {
    MediaError::NotSent(ProviderError::InvalidRequest(what.to_owned()))
}

// ---- the settings file -----------------------------------------------------------------------

/// The bundled settings file, read once. A file that cannot be read gives no entries (the
/// tests read it), so every model gets the basic fields and an unknown price.
pub(crate) fn bundled() -> &'static OpenAiSpec {
    static SPEC: LazyLock<OpenAiSpec> =
        LazyLock::new(|| serde_json::from_str(BUNDLED).unwrap_or_default());
    &SPEC
}

/// The entry for a model id: the longest prefix that is the whole id or is followed only by a
/// snapshot's name (`-YYYY-MM-DD` or `-latest`). Any other ending (`-pro`, `-hd`) is another
/// model, which never gets this one's settings or prices.
pub(crate) fn match_spec(id: &str) -> Option<&'static OpenAiEntry> {
    let id = id.to_ascii_lowercase();
    bundled()
        .models
        .iter()
        .filter(|entry| {
            id.strip_prefix(entry.prefix.as_str())
                .is_some_and(|rest| rest.is_empty() || is_snapshot(rest))
        })
        .max_by_key(|entry| entry.prefix.len())
}

/// Whether the end of an id names a snapshot of its model: `-latest`, or a date `-YYYY-MM-DD`.
fn is_snapshot(rest: &str) -> bool {
    if rest == "-latest" {
        return true;
    }
    let Some(date) = rest.strip_prefix('-') else {
        return false;
    };
    let parts: Vec<&str> = date.split('-').collect();
    matches!(parts.as_slice(), [year, month, day]
        if year.len() == 4 && month.len() == 2 && day.len() == 2
            && parts.iter().all(|part| part.bytes().all(|b| b.is_ascii_digit())))
}

/// The picture models of OpenAI's `/v1/models` answer, in its order, each with the controls
/// of its entry in the settings file ([`openai_media_kind`] decides what is a picture model).
pub fn openai_image_models(list: &Value) -> Vec<MediaModel> {
    entries(list)
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            (openai_media_kind(id) == Some(MediaKind::Image)).then(|| MediaModel {
                created: entry.get("created").and_then(Value::as_i64),
                ..model_for(id)
            })
        })
        .collect()
}

/// The model `id` with the controls, price and notes of its entry, or the basic fields of a
/// model with no entry.
pub(crate) fn model_for(id: &str) -> MediaModel {
    let Some(entry) = match_spec(id) else {
        return new_model(id);
    };
    let mut formats: Vec<&str> = entry.formats.iter().map(String::as_str).collect();
    if entry.gif {
        formats.push("gif");
    }
    if entry.svg {
        formats.push("svg");
    }
    // An SVG is one picture, made from words alone: How many and added pictures need another
    // file type. A field that is not sent counts as PNG, the company default.
    let not_svg = entry.svg.then(|| Requires {
        key: "output_format".to_owned(),
        values: formats
            .iter()
            .filter(|format| **format != "svg")
            .map(|format| Value::from(*format))
            .collect(),
    });

    let mut params = vec![
        size_field(),
        quality_field(&entry.qualities),
        ParamSpec {
            requires: not_svg.clone(),
            ..count_field(entry.max_n)
        },
    ];
    if entry.max_refs > 0 {
        params.push(ParamSpec {
            requires: not_svg,
            ..references_field(entry.max_refs)
        });
    }
    params.push(format_field(&formats));
    let compressed: Vec<Value> = formats
        .iter()
        .filter(|format| matches!(**format, "jpeg" | "webp"))
        .map(|format| Value::from(*format))
        .collect();
    if !compressed.is_empty() {
        params.push(compression_field(compressed));
    }
    if entry.gif {
        let [frames, duration] = gif_fields();
        params.push(ParamSpec {
            company_default: Some(Value::from(9)),
            ..frames
        });
        params.push(ParamSpec {
            company_default: Some(Value::from(200)),
            ..duration
        });
    }
    params.push(background_field());
    params.push(moderation_field());
    if entry.max_refs > 0 {
        note_what_edits_leave_out(&mut params);
    }

    let mut notes: Vec<String> = entry.note.iter().cloned().collect();
    if let Some(date) = &entry.retires {
        notes.push(format!("OpenAI retires this model on {}.", long_date(date)));
    }
    MediaModel {
        id: id.to_owned(),
        display_name: entry_name(id, entry),
        family: family_of(id),
        kind: MediaKind::Image,
        accepts_images: entry.max_refs > 0,
        params,
        price: entry_price(entry),
        notes,
        usable: None,
        created: None,
        companies: Vec::new(),
    }
}

/// A model the settings file has no entry for: size, quality and how many, an unknown price,
/// and a note that the settings may not match it.
fn new_model(id: &str) -> MediaModel {
    MediaModel {
        id: id.to_owned(),
        display_name: new_name(id),
        family: family_of(id),
        kind: MediaKind::Image,
        accepts_images: false,
        params: vec![
            size_field(),
            quality_field(&BASIC_QUALITIES),
            count_field(BASIC_MAX_N),
        ],
        price: Price::Unknown,
        notes: vec![NEW_MODEL.to_owned()],
        usable: None,
        created: None,
        companies: Vec::new(),
    }
}

/// The name shown for an id with an entry: its label, and a dated snapshot's date (or other
/// ending) after it: `GPT Image 2.6 Flare (2026-11-04)`.
fn entry_name(id: &str, entry: &OpenAiEntry) -> String {
    match id
        .get(entry.prefix.len()..)
        .map(|rest| rest.trim_start_matches('-'))
    {
        Some(rest) if !rest.is_empty() => format!("{} ({rest})", entry.label),
        _ => entry.label.clone(),
    }
}

/// The name shown for an id with no entry: `gpt-image-3-mini` → `GPT Image 3 Mini`.
fn new_name(id: &str) -> String {
    let lower = id.to_ascii_lowercase();
    let (lead, rest) = if let Some(rest) = lower.strip_prefix("chatgpt-image") {
        ("ChatGPT Image", rest)
    } else if let Some(rest) = lower.strip_prefix("gpt-image") {
        ("GPT Image", rest)
    } else {
        return id.to_owned();
    };
    let mut name = lead.to_owned();
    for word in rest.split('-').filter(|word| !word.is_empty()) {
        let mut chars = word.chars();
        name.push(' ');
        name.extend(chars.next().map(|c| c.to_ascii_uppercase()));
        name.push_str(chars.as_str());
    }
    name
}

/// A `YYYY-MM-DD` day in words: `2027-02-10` → `February 10, 2027`; any other text as it is.
fn long_date(date: &str) -> String {
    in_words(date).unwrap_or_else(|| date.to_owned())
}

/// A `YYYY-MM-DD` day in words; `None` for any other text.
fn in_words(date: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let mut parts = date.splitn(3, '-');
    let year = parts.next().filter(|y| y.len() == 4)?.parse::<u16>().ok()?;
    let month = MONTHS.get(parts.next()?.parse::<usize>().ok()?.checked_sub(1)?)?;
    let day = parts
        .next()?
        .parse::<u8>()
        .ok()
        .filter(|d| (1..=31).contains(d))?;
    Some(format!("{month} {day}, {year}"))
}

/// The price per picture OpenAI states for an entry, one row per `<quality>|<size>`; unknown
/// when it states none. An edit has no estimate: OpenAI also charges for each added picture's
/// tokens, and states their number for none of these models (its vision guide's count covers
/// only the shut-down `gpt-image-1`), so a price from the picture made alone could be too low.
fn entry_price(entry: &OpenAiEntry) -> Price {
    let rows: Vec<PriceRow> = entry
        .per_image
        .iter()
        .filter(|(_, usd)| usd.is_finite() && **usd >= 0.0)
        .filter_map(|(key, usd)| {
            let (quality, size) = key.split_once('|')?;
            Some(PriceRow {
                mode: None,
                resolution: Some(size.to_owned()),
                quality: Some(quality.to_owned()),
                audio: None,
                usd: *usd,
            })
        })
        .collect();
    if rows.is_empty() {
        Price::Unknown
    } else {
        Price::PerImage(Table {
            rows,
            per_input_image_usd: None,
            input_images_unpriced: true,
            minimum_usd: None,
        })
    }
}

// ---- the controls ----------------------------------------------------------------------------

/// The choices of a list of words, with their names on screen.
fn word_choices(words: &[impl AsRef<str>]) -> Vec<Choice> {
    words
        .iter()
        .map(|word| {
            let value = Value::from(word.as_ref());
            Choice {
                label: choice_label(&value),
                value,
            }
        })
        .collect()
}

/// Size: a size offered by name, or one typed within OpenAI's rules.
fn size_field() -> ParamSpec {
    let values = SIZES
        .iter()
        .map(|(value, label)| Choice {
            value: Value::from(*value),
            label: (*label).to_owned(),
        })
        .collect();
    ParamSpec {
        company_default: Some(Value::from("auto")),
        affects_price: true,
        ..field(
            "size",
            Some(Canonical::Size),
            "Size",
            Layer::Simple,
            ParamKind::Size {
                rules: OPENAI_SIZE_RULES,
                values,
            },
        )
    }
}

/// Quality: the model's steps.
fn quality_field(qualities: &[impl AsRef<str>]) -> ParamSpec {
    ParamSpec {
        company_default: Some(Value::from("auto")),
        affects_price: true,
        ..field(
            "quality",
            Some(Canonical::Quality),
            "Quality",
            Layer::Simple,
            ParamKind::Enum {
                values: word_choices(qualities),
            },
        )
    }
}

/// How many: 1 to `max_n` pictures.
fn count_field(max_n: u32) -> ParamSpec {
    ParamSpec {
        company_default: Some(Value::from(1)),
        affects_price: true,
        ..field(
            "n",
            Some(Canonical::Count),
            "How many",
            Layer::Simple,
            ParamKind::Range {
                min: 1.0,
                max: f64::from(max_n),
                step: Some(1.0),
                unit: None,
            },
        )
    }
}

/// Reference pictures, sent as `image[]` files to the edit endpoint.
fn references_field(max: u32) -> ParamSpec {
    ParamSpec {
        affects_price: true,
        ..field(
            REFERENCES,
            Some(Canonical::References),
            "Reference pictures",
            Layer::Simple,
            ParamKind::Images {
                min: 0,
                max,
                role: RefRole::Reference,
            },
        )
    }
}

/// File type: PNG unless set.
fn format_field(formats: &[&str]) -> ParamSpec {
    ParamSpec {
        company_default: Some(Value::from("png")),
        ..field(
            "output_format",
            Some(Canonical::Format),
            "File type",
            Layer::Advanced,
            ParamKind::Enum {
                values: word_choices(formats),
            },
        )
    }
}

/// Compression, for the file types in `formats` (JPEG and WebP).
fn compression_field(formats: Vec<Value>) -> ParamSpec {
    ParamSpec {
        company_default: Some(Value::from(100)),
        requires: Some(Requires {
            key: "output_format".to_owned(),
            values: formats,
        }),
        ..field(
            "output_compression",
            None,
            "Compression",
            Layer::Advanced,
            ParamKind::Range {
                min: 0.0,
                max: 100.0,
                step: Some(1.0),
                unit: Some("%".to_owned()),
            },
        )
    }
}

/// Background: automatic, transparent or opaque.
fn background_field() -> ParamSpec {
    ParamSpec {
        company_default: Some(Value::from("auto")),
        ..field(
            "background",
            Some(Canonical::Background),
            "Background",
            Layer::Advanced,
            ParamKind::Enum {
                values: word_choices(&["auto", "transparent", "opaque"]),
            },
        )
    }
}

/// Safety level: OpenAI's `moderation`, automatic or low.
fn moderation_field() -> ParamSpec {
    ParamSpec {
        help: Some(MODERATION_HELP.to_owned()),
        company_default: Some(Value::from("auto")),
        ..field(
            "moderation",
            None,
            "Safety level",
            Layer::Advanced,
            ParamKind::Enum {
                values: word_choices(&["auto", "low"]),
            },
        )
    }
}

/// Adds [`NOT_IN_EDITS`] to the help of each setting the edit endpoint does not take
/// ([`EDIT_SETTINGS`]); an edit's form, and its preview, leave those out.
fn note_what_edits_leave_out(params: &mut [ParamSpec]) {
    let left_out = params
        .iter_mut()
        .filter(|spec| spec.key != REFERENCES && !EDIT_SETTINGS.contains(&spec.key.as_str()));
    for spec in left_out {
        spec.help = Some(match spec.help.take() {
            Some(help) => format!("{help} {NOT_IN_EDITS}"),
            None => NOT_IN_EDITS.to_owned(),
        });
    }
}

// ---- the request -----------------------------------------------------------------------------

/// The added pictures a request sends: its references.
fn references(req: &MediaRequest) -> impl Iterator<Item = &RefImage> {
    req.refs
        .iter()
        .filter(|picture| picture.role == RefRole::Reference)
}

/// The typed settings `req` sends, in its order; `model` and `prompt` are never settings. A
/// setting at its company default whose requirement is not met is left out: sending the
/// default is the same as leaving it out, and OpenAI could refuse it (How many 1 with an
/// SVG, compression 100 with a PNG).
fn settings(req: &MediaRequest) -> Vec<(&str, &Value)> {
    let model = model_for(&req.model);
    let idle = |key: &str, value: &Value| {
        model.params.iter().any(|spec| {
            spec.key == key
                && is_company_default(spec, value)
                && unmet(&model, &req.params, spec).is_some()
        })
    };
    req.params
        .iter()
        .map(|(key, value)| (key.as_str(), value))
        .filter(|(key, value)| !matches!(*key, "model" | "prompt") && !idle(key, value))
        .collect()
}

/// The JSON body of `/images/generations`: `model`, `prompt` and the typed settings
/// ([`settings`]). OpenAI's models take no extra settings.
fn json_body(req: &MediaRequest) -> Value {
    let mut body = Map::new();
    body.insert("model".to_owned(), Value::String(req.model.clone()));
    body.insert("prompt".to_owned(), Value::String(req.prompt.clone()));
    for (key, value) in settings(req) {
        body.insert(key.to_owned(), value.clone());
    }
    Value::Object(body)
}

/// The text fields of an `/images/edits` form: `model`, `prompt`, then the typed settings
/// ([`settings`]) the edit endpoint takes ([`EDIT_SETTINGS`]) by name, each as text.
fn form_fields(req: &MediaRequest) -> Vec<(&str, String)> {
    let mut fields = vec![("model", req.model.clone()), ("prompt", req.prompt.clone())];
    let mut settings: Vec<(&str, &Value)> = settings(req)
        .into_iter()
        .filter(|(key, _)| EDIT_SETTINGS.contains(key))
        .collect();
    settings.sort_by_key(|(key, _)| *key);
    for (key, value) in settings {
        match value {
            Value::Null => {}
            Value::String(text) => fields.push((key, text.clone())),
            other => fields.push((key, other.to_string())),
        }
    }
    fields
}

/// Added picture `n` as an `image[]` file, read from its file. A file that cannot be read
/// stops the request before it is sent.
async fn file_part(n: usize, picture: &RefImage) -> Result<FilePart, MediaError> {
    let bytes = tokio::fs::read(&picture.path).await.map_err(|err| {
        MediaError::NotSent(ProviderError::InvalidRequest(format!(
            "could not read an added picture: {err}"
        )))
    })?;
    let extension = match picture.mime.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        _ => "bin",
    };
    Ok(FilePart {
        name: REFERENCES.to_owned(),
        filename: format!("picture-{n}.{extension}"),
        mime: picture.mime.clone(),
        bytes,
    })
}

// ---- the answer ------------------------------------------------------------------------------

/// OpenAI's picture answer.
#[derive(Deserialize)]
struct ImagesAnswer {
    data: Vec<AnswerPicture>,
    output_format: Option<String>,
    usage: Option<Value>,
    svg_usage: Option<Value>,
}

/// Reads the answer in the file `answer` and writes each picture to `scratch` (their type is
/// the answer's `output_format`, else read from the bytes); what the run cost, worked out
/// from the answer's usage and `rates` ([`answer_cost`]).
fn save_answer(
    answer: &Path,
    scratch: &Path,
    picture_cap: u64,
    rates: Option<&OpenAiRates>,
) -> Result<(Vec<Saved>, Option<Cost>), MediaError> {
    let file =
        std::fs::File::open(answer).map_err(|err| MediaError::Local(LocalIo::from_io(&err)))?;
    let parsed: ImagesAnswer = serde_json::from_reader(BufReader::new(file))
        .map_err(|err| unreadable(format!("the answer is not a picture answer ({err})")))?;
    let mime = parsed.output_format.as_deref().and_then(format_mime);
    let pictures = parsed
        .data
        .into_iter()
        .map(|picture| AnswerPicture {
            media_type: picture.media_type.or_else(|| mime.map(str::to_owned)),
            ..picture
        })
        .collect();
    let saved = write_pictures(pictures, scratch, picture_cap)?;
    let cost = answer_cost(parsed.usage.as_ref(), parsed.svg_usage.as_ref(), rates);
    Ok((saved, cost))
}

/// The media type of an `output_format`.
fn format_mime(format: &str) -> Option<&'static str> {
    match format.trim().to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpeg" | "jpg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

/// What a run cost, worked out (never reported) from the answer's `usage` and the model's
/// `rates`. `None` for a model with no entry, an answer without usage, and an SVG answer that
/// carries `svg_usage`: another model wrote the SVG, at a price the settings do not state.
fn answer_cost(
    usage: Option<&Value>,
    svg_usage: Option<&Value>,
    rates: Option<&OpenAiRates>,
) -> Option<Cost> {
    if svg_usage.is_some_and(|svg| !svg.is_null()) {
        return None;
    }
    let usd = openai_cost(usage?, rates?)?;
    Some(Cost {
        usd,
        reported: false,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use reqwest::header::{HeaderMap, HeaderValue};
    use serde_json::{Value, json};

    use super::*;
    use crate::client::{ClientConfig, ProviderKind};
    use crate::media::{
        Invalid, OPENAI_SIZE_RULES, ParamKind, Price, RefImage, RefRole, SizeProblem, check_size,
        choice_for, estimate, validate,
    };

    fn keys(m: &MediaModel) -> Vec<&str> {
        m.params.iter().map(|p| p.key.as_str()).collect()
    }

    fn param<'a>(m: &'a MediaModel, key: &str) -> &'a ParamSpec {
        m.params
            .iter()
            .find(|p| p.key == key)
            .unwrap_or_else(|| panic!("{} has no field {key}", m.id))
    }

    fn values(m: &MediaModel, key: &str) -> Vec<Value> {
        match &param(m, key).kind {
            ParamKind::Enum { values } | ParamKind::Size { values, .. } => {
                values.iter().map(|c| c.value.clone()).collect()
            }
            other => panic!("{key} is not a list: {other:?}"),
        }
    }

    fn request(model: &str, params: Value) -> MediaRequest {
        let Value::Object(params) = params else {
            panic!("params are an object")
        };
        MediaRequest {
            kind: MediaKind::Image,
            model: model.to_owned(),
            prompt: "a lighthouse at dusk".to_owned(),
            params,
            extra: serde_json::Map::new(),
            refs: Vec::new(),
            routing: None,
            provider_options: BTreeMap::new(),
        }
    }

    fn with_ref(req: MediaRequest) -> MediaRequest {
        with_refs(req, 1)
    }

    fn with_refs(req: MediaRequest, count: usize) -> MediaRequest {
        MediaRequest {
            refs: (0..count)
                .map(|i| RefImage {
                    role: RefRole::Reference,
                    mime: "image/png".to_owned(),
                    path: PathBuf::from(format!("/gallery/refs/{i}.png")),
                    sha256: i.to_string().repeat(64),
                    size_bytes: 1_200,
                })
                .collect(),
            ..req
        }
    }

    fn names<'a>(fields: &'a [(&'a str, String)]) -> Vec<&'a str> {
        fields.iter().map(|(name, _)| *name).collect()
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_static(value));
        }
        map
    }

    #[test]
    fn the_bundled_spec_leads_with_2_6() {
        let s = bundled();
        assert_eq!(s.checked, "2026-11-17");
        assert_eq!(
            [s.models[0].prefix.as_str(), s.models[1].prefix.as_str()],
            ["gpt-image-2.6-sunburst", "gpt-image-2.6-flare"]
        );
        let prefixes: Vec<&str> = s.models.iter().map(|m| m.prefix.as_str()).collect();
        assert_eq!(
            prefixes[2..],
            [
                "gpt-image-2",
                "gpt-image-1.5",
                "gpt-image-1-mini",
                "chatgpt-image-latest"
            ]
        );
        assert!(!s.sources.is_empty());
    }

    #[test]
    fn every_bundled_entry_is_sound() {
        for entry in &bundled().models {
            let id = &entry.prefix;
            assert!(entry.max_n >= 1 && entry.max_refs >= 1, "{id}");
            assert!(entry.qualities.iter().any(|q| q == "auto"), "{id}");
            assert!(entry.formats.iter().any(|f| f == "png"), "{id}");
            assert!(entry.rates.image_input > 0.0 && entry.rates.image_output > 0.0);
            if let Some(date) = &entry.retires {
                assert!(long_date(date).ends_with(&date[..4]), "{id}: {date}");
            }
            for key in entry.per_image.keys() {
                let (quality, size) = key.split_once('|').expect("<quality>|<size>");
                assert!(entry.qualities.iter().any(|q| q == quality), "{id}: {key}");
                assert!(check_size(size, &OPENAI_SIZE_RULES).is_ok(), "{id}: {key}");
            }
        }
    }

    #[test]
    fn the_longest_prefix_wins_and_a_new_model_gets_basic_fields() {
        assert_eq!(
            match_spec("gpt-image-2.6-flare-2026-11-04").unwrap().prefix,
            "gpt-image-2.6-flare"
        );
        let m = model_for("gpt-image-3");
        assert_eq!(keys(&m), ["size", "quality", "n"]);
        assert!(
            m.notes
                .contains(&"Settings may not match this new model.".to_string())
        );
        assert_eq!(m.price, Price::Unknown);
        assert!(!m.accepts_images);
        assert_eq!(m.display_name, "GPT Image 3");
    }

    #[test]
    fn a_prefix_ends_where_the_id_starts_a_new_word() {
        // Dated snapshots belong to their model.
        assert_eq!(
            match_spec("gpt-image-2-2026-04-21").unwrap().prefix,
            "gpt-image-2"
        );
        assert_eq!(
            match_spec("gpt-image-1.5-2025-12-16").unwrap().prefix,
            "gpt-image-1.5"
        );
        // A newer version is not the older one: no settings or prices of gpt-image-2.
        assert!(match_spec("gpt-image-2.5-flare").is_none());
        assert!(match_spec("gpt-image-1").is_none());
        assert_eq!(
            match_spec("gpt-image-2-latest").unwrap().prefix,
            "gpt-image-2"
        );
        // Another model with the same start is not a snapshot: no older prices or settings.
        assert!(match_spec("gpt-image-2-pro").is_none());
        assert!(match_spec("gpt-image-1.5-hd").is_none());
        assert!(match_spec("gpt-image-2-2026-04").is_none());
        assert!(match_spec("gpt-image-2-20260421").is_none());
        let pro = model_for("gpt-image-2-pro");
        assert_eq!(pro.price, Price::Unknown);
        assert!(pro.notes.contains(&NEW_MODEL.to_string()));
        let snapshot = model_for("gpt-image-2.6-flare-2026-11-04");
        assert_eq!(snapshot.display_name, "GPT Image 2.6 Flare (2026-11-04)");
    }

    #[test]
    fn custom_sizes_are_checked() {
        assert!(check_size("1536x1024", &OPENAI_SIZE_RULES).is_ok());
        assert!(check_size("3840x2160", &OPENAI_SIZE_RULES).is_ok());
        assert_eq!(
            check_size("1000x1000", &OPENAI_SIZE_RULES),
            Err(SizeProblem::MultipleOf)
        );
        assert_eq!(
            check_size("3840x1024", &OPENAI_SIZE_RULES),
            Err(SizeProblem::Ratio)
        );
        assert_eq!(
            check_size("4096x2048", &OPENAI_SIZE_RULES),
            Err(SizeProblem::LongestSide)
        );
        assert_eq!(
            check_size("512x512", &OPENAI_SIZE_RULES),
            Err(SizeProblem::TooFewPixels)
        );
    }

    #[test]
    fn a_2_6_model_has_every_control() {
        let m = model_for("gpt-image-2.6-flare");
        assert_eq!(
            keys(&m),
            [
                "size",
                "quality",
                "n",
                "image[]",
                "output_format",
                "output_compression",
                "frame_count",
                "frame_duration_ms",
                "background",
                "moderation"
            ]
        );
        assert_eq!(m.display_name, "GPT Image 2.6 Flare");
        assert_eq!(m.kind, MediaKind::Image);
        assert!(m.accepts_images);
        assert_eq!(
            values(&m, "size"),
            ["auto", "1024x1024", "1536x1024", "1024x1536"]
        );
        assert!(matches!(
            param(&m, "size").kind,
            ParamKind::Size { rules, .. } if rules == OPENAI_SIZE_RULES
        ));
        assert_eq!(
            values(&m, "quality"),
            ["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(
            param(&m, "n").kind,
            ParamKind::Range {
                min: 1.0,
                max: 10.0,
                step: Some(1.0),
                unit: None
            }
        );
        assert_eq!(
            param(&m, "image[]").kind,
            ParamKind::Images {
                min: 0,
                max: 16,
                role: RefRole::Reference
            }
        );
        assert_eq!(
            values(&m, "output_format"),
            ["png", "jpeg", "webp", "gif", "svg"]
        );
        assert_eq!(
            param(&m, "output_format").company_default,
            Some(json!("png"))
        );
        assert_eq!(
            param(&m, "output_compression").requires,
            Some(Requires {
                key: "output_format".into(),
                values: vec![json!("jpeg"), json!("webp")]
            })
        );
        assert_eq!(values(&m, "moderation"), ["auto", "low"]);
        assert_eq!(
            param(&m, "moderation").help.as_deref(),
            Some("Low makes OpenAI's filter less strict; it still refuses what its rules forbid.")
        );
        assert_eq!(param(&m, "background").layer, Layer::Advanced);
        assert_eq!(
            m.price,
            Price::Unknown,
            "OpenAI states no price per picture"
        );
        assert!(m.notes.iter().all(|n| !n.contains("retires")));
    }

    #[test]
    fn an_older_model_retires_and_has_its_stated_prices() {
        let m = model_for("gpt-image-2");
        assert_eq!(
            keys(&m),
            [
                "size",
                "quality",
                "n",
                "image[]",
                "output_format",
                "output_compression",
                "background",
                "moderation"
            ]
        );
        assert_eq!(values(&m, "quality"), ["auto", "low", "medium", "high"]);
        assert_eq!(values(&m, "output_format"), ["png", "jpeg", "webp"]);
        assert!(param(&m, "n").requires.is_none(), "no SVG, no rule");
        assert!(
            m.notes
                .contains(&"OpenAI retires this model on February 10, 2027.".to_string())
        );
        let price =
            |params: Value| estimate(&m.price, &choice_for(&m, &request("gpt-image-2", params)));
        let usd = price(json!({"size": "1024x1024", "quality": "low", "n": 2})).unwrap();
        assert!((usd - 0.012).abs() < 1e-9, "{usd}");
        let usd = price(json!({"size": "1536X1024", "quality": "high"})).unwrap();
        assert!((usd - 0.165).abs() < 1e-9, "{usd}");
        // Auto, a size OpenAI states no price for, or no size at all: unknown.
        assert_eq!(price(json!({"size": "auto", "quality": "low"})), None);
        assert_eq!(price(json!({"size": "2048x2048", "quality": "low"})), None);
        assert_eq!(price(json!({"quality": "low"})), None);
        assert_eq!(price(json!({"size": "1024x1024"})), None, "auto quality");
    }

    #[test]
    fn svg_is_one_picture_and_never_an_edit() {
        let m = model_for("gpt-image-2.6-flare");
        let svg = request("gpt-image-2.6-flare", json!({"output_format": "svg"}));
        assert!(validate(&m, &svg).is_ok());
        assert_eq!(
            validate(
                &m,
                &request(
                    "gpt-image-2.6-flare",
                    json!({"output_format": "svg", "n": 2})
                )
            ),
            Err(vec![Invalid::Requires {
                key: "n".into(),
                needs: "output_format".into()
            }])
        );
        assert_eq!(
            validate(&m, &with_ref(svg)),
            Err(vec![Invalid::Requires {
                key: "image[]".into(),
                needs: "output_format".into()
            }])
        );
        // PNG by default: several pictures and an edit are fine.
        let edit = with_ref(request("gpt-image-2.6-flare", json!({"n": 2})));
        assert!(validate(&m, &edit).is_ok());
        let gif = request(
            "gpt-image-2.6-flare",
            json!({"output_format": "gif", "frame_count": 9}),
        );
        assert!(validate(&m, &gif).is_ok());
        assert_eq!(
            validate(
                &m,
                &request("gpt-image-2.6-flare", json!({"frame_count": 12}))
            ),
            Err(vec![Invalid::Requires {
                key: "frame_count".into(),
                needs: "output_format".into()
            }])
        );
    }

    #[test]
    fn sending_a_default_is_the_same_as_leaving_it_out() {
        let id = "gpt-image-2.6-flare";
        let m = model_for(id);
        // How many = 1 with an SVG: valid, and `n` is not sent.
        let svg = request(id, json!({"output_format": "svg", "n": 1}));
        assert_eq!(validate(&m, &svg), Ok(()));
        let body = json_body(&svg);
        assert_eq!(body.get("n"), None, "{body}");
        assert_eq!(body["output_format"], "svg");
        assert_eq!(
            validate(&m, &request(id, json!({"output_format": "svg", "n": 2}))),
            Err(vec![Invalid::Requires {
                key: "n".into(),
                needs: "output_format".into()
            }])
        );
        // Compression 100 with a PNG: valid, and not sent.
        let png = request(
            id,
            json!({"output_format": "png", "output_compression": 100}),
        );
        assert_eq!(validate(&m, &png), Ok(()));
        let body = json_body(&png);
        assert_eq!(body.get("output_compression"), None, "{body}");
        assert_eq!(
            validate(
                &m,
                &request(
                    id,
                    json!({"output_format": "png", "output_compression": 80})
                )
            ),
            Err(vec![Invalid::Requires {
                key: "output_compression".into(),
                needs: "output_format".into()
            }])
        );
        // Where the requirement is met, the default is sent as it was set.
        let jpeg = request(
            id,
            json!({"output_format": "jpeg", "output_compression": 100, "n": 1}),
        );
        let body = json_body(&jpeg);
        assert_eq!(
            (&body["n"], &body["output_compression"]),
            (&json!(1), &json!(100))
        );
        // An edit's form leaves it out the same way.
        let edit = with_ref(request(
            id,
            json!({"output_format": "png", "output_compression": 100, "n": 1}),
        ));
        assert_eq!(validate(&m, &edit), Ok(()));
        assert_eq!(
            names(&form_fields(&edit)),
            ["model", "prompt", "n", "output_format"]
        );
    }

    #[test]
    fn an_edit_is_never_estimated_from_the_picture_made_alone() {
        // OpenAI charges for added pictures at a token count it states for none of these
        // models, so an edit has no estimate and Studio asks first.
        for id in ["gpt-image-2", "gpt-image-1.5", "gpt-image-1-mini"] {
            let m = model_for(id);
            let plain = request(id, json!({"size": "1024x1024", "quality": "high"}));
            let price = |req: &MediaRequest| estimate(&m.price, &choice_for(&m, req));
            assert!(price(&plain).is_some_and(|usd| usd > 0.0), "{id}");
            assert_eq!(price(&with_refs(plain.clone(), 4)), None, "{id}");
            assert_eq!(price(&with_refs(plain, 1)), None, "{id}");
        }
    }

    #[test]
    fn a_non_square_price_below_the_square_one_names_its_source() {
        let spec = bundled();
        let guide = "https://developers.openai.com/api/docs/guides/image-generation";
        assert!(spec.sources.iter().any(|s| s == guide));
        let below = |entry: &OpenAiEntry, quality: &str| {
            let usd = |size: &str| entry.per_image.get(&format!("{quality}|{size}")).copied();
            usd("1024x1024").is_some_and(|square| {
                ["1024x1536", "1536x1024"]
                    .iter()
                    .any(|size| usd(size).is_some_and(|other| other < square))
            })
        };
        for entry in &spec.models {
            for quality in entry.qualities.iter().filter(|q| below(entry, q)) {
                let noted = spec.notes.iter().any(|note| {
                    note.starts_with(&format!("{}:", entry.prefix)) && note.contains(guide)
                });
                assert!(
                    noted,
                    "{} {quality}: no note says where it was read",
                    entry.prefix
                );
            }
        }
        // gpt-image-2 is such a model; its figures are kept as OpenAI shows them.
        let two = match_spec("gpt-image-2").unwrap();
        assert!(below(two, "high"));
        for (key, usd) in [
            ("low|1024x1024", 0.006),
            ("low|1536x1024", 0.005),
            ("medium|1024x1536", 0.041),
            ("high|1024x1024", 0.211),
            ("high|1536x1024", 0.165),
        ] {
            assert!((two.per_image[key] - usd).abs() < 1e-12, "{key}");
        }
    }

    #[test]
    fn an_edit_sends_only_what_the_edit_endpoint_documents() {
        // OpenAI's edits reference documents every typed setting, Safety level included, so
        // none says it is not sent.
        for entry in &bundled().models {
            for spec in &model_for(&entry.prefix).params {
                assert!(
                    spec.key == REFERENCES || EDIT_SETTINGS.contains(&spec.key.as_str()),
                    "{}: {}",
                    entry.prefix,
                    spec.key
                );
                let help = spec.help.as_deref().unwrap_or_default();
                assert!(!help.contains(NOT_IN_EDITS), "{}", spec.key);
            }
        }
        let id = "gpt-image-2.6-flare";
        let edit = with_ref(request(
            id,
            json!({"moderation": "low", "output_format": "gif", "frame_count": 12}),
        ));
        assert_eq!(validate(&model_for(id), &edit), Ok(()));
        assert_eq!(
            form_fields(&edit),
            [
                ("model", id.to_owned()),
                ("prompt", "a lighthouse at dusk".to_owned()),
                ("frame_count", "12".to_owned()),
                ("moderation", "low".to_owned()),
                ("output_format", "gif".to_owned()),
            ]
        );

        // A setting the reference does not document (`response_format`, which would change
        // the answer) is left out of the form and of the request preview built from it...
        let mut stray = edit;
        stray.params.insert("response_format".into(), json!("url"));
        assert!(!names(&form_fields(&stray)).contains(&"response_format"));
        let client = OpenAiClient::new(ClientConfig::new(
            "https://api.openai.com/v1",
            ProviderKind::OpenAi,
        ))
        .unwrap();
        let preview = OpenAiImages::new(client).preview(&stray);
        assert!(preview.url.ends_with("/images/edits"), "{}", preview.url);
        assert_eq!(preview.body["moderation"], "low");
        assert_eq!(
            preview.body.get("response_format"),
            None,
            "{}",
            preview.body
        );

        // ...and a field of that name would say so in its help.
        let mut params = vec![
            ParamSpec {
                help: Some("A link instead of the picture.".to_owned()),
                ..field(
                    "response_format",
                    None,
                    "Answer",
                    Layer::Advanced,
                    ParamKind::Enum {
                        values: word_choices(&["url", "b64_json"]),
                    },
                )
            },
            moderation_field(),
        ];
        note_what_edits_leave_out(&mut params);
        assert_eq!(
            params[0].help.as_deref(),
            Some("A link instead of the picture. Not sent when you add pictures.")
        );
        assert_eq!(params[1].help.as_deref(), Some(MODERATION_HELP));
    }

    #[test]
    fn cost_from_usage() {
        let flare = match_spec("gpt-image-2.6-flare").unwrap().rates.clone();
        assert_eq!(
            flare,
            OpenAiRates {
                text_input: Some(5.0),
                image_input: 8.0,
                image_output: 30.0
            }
        );
        let u = json!({"input_tokens": 1050, "input_tokens_details": {"text_tokens": 50, "image_tokens": 1000}, "output_tokens": 2000});
        assert!((openai_cost(&u, &flare).unwrap() - 0.06825).abs() < 1e-9); // 50×5 + 1000×8 + 2000×30, per 1e6
        let no_text_rate = OpenAiRates {
            text_input: None,
            ..flare
        };
        assert_eq!(openai_cost(&u, &no_text_rate), None);
        assert_eq!(openai_cost(&json!({}), &flare), None);
    }

    #[test]
    fn an_svg_answer_leaves_the_cost_unknown() {
        let rates = &match_spec("gpt-image-2.6-flare").unwrap().rates;
        let usage = json!({"input_tokens": 20, "input_tokens_details": {"text_tokens": 20, "image_tokens": 0}, "output_tokens": 100});
        let cost = answer_cost(Some(&usage), None, Some(rates)).unwrap();
        assert!((cost.usd - 0.0031).abs() < 1e-9, "{cost:?}");
        assert!(!cost.reported);
        // The SVG is written by another model, whose price the settings do not state.
        let svg = json!({"input_tokens": 400, "output_tokens": 900, "model": "svg-author"});
        assert_eq!(answer_cost(Some(&usage), Some(&svg), Some(rates)), None);
        assert_eq!(answer_cost(Some(&usage), None, None), None, "a new model");
        assert_eq!(answer_cost(None, None, Some(rates)), None);
    }

    #[test]
    fn openai_failures_are_named() {
        let none = HeaderMap::new();
        let output = br#"{"error": {"code": "moderation_blocked", "message": "Blocked", "moderation_details": {"moderation_stage": "output", "categories": ["violence"]}}}"#;
        let f = openai_failure(400, &none, output, None);
        assert_eq!(
            (f.kind, f.stage, f.charged),
            (FailureKind::Blocked, Some(Stage::Output), None)
        );
        let unknown = br#"{"error": {"code": "moderation_blocked", "message": "Blocked", "moderation_details": {"moderation_stage": "unknown"}}}"#;
        assert_eq!(openai_failure(400, &none, unknown, None).stage, None);
        let bare = br#"{"error": {"code": "moderation_blocked", "message": "Blocked"}}"#;
        assert_eq!(openai_failure(400, &none, bare, None).stage, None);

        // Every 5xx leaves the charge unknown, a 502 too.
        for status in [500, 502, 503] {
            let f = openai_failure(status, &none, b"", None);
            assert_eq!((f.kind, f.charged), (FailureKind::Server, None), "{status}");
        }
        let kind = |status| openai_failure(status, &none, b"", None).kind;
        assert_eq!(kind(401), FailureKind::Unauthorized);
        assert_eq!(kind(402), FailureKind::NoCredit);
        assert_eq!(kind(429), FailureKind::RateLimited);
        // An account with no credit left, whatever the status says.
        for (status, body) in [
            (
                429,
                &br#"{"error": {"code": "insufficient_quota", "type": "insufficient_quota", "message": "You exceeded your current quota"}}"#[..],
            ),
            (
                400,
                &br#"{"error": {"code": "billing_hard_limit_reached", "message": "Billing hard limit has been reached"}}"#[..],
            ),
            (
                429,
                &br#"{"error": {"code": null, "type": "insufficient_quota", "message": "You exceeded your current quota"}}"#[..],
            ),
        ] {
            let f = openai_failure(status, &headers(&[("retry-after", "20")]), body, None);
            assert_eq!(
                (f.kind, f.charged, f.retry_after),
                (FailureKind::NoCredit, Some(false), None),
                "{status}"
            );
        }
        // A 403 about something else is not about verification.
        let other = br#"{"error": {"message": "Project does not have access to this model"}}"#;
        assert_eq!(
            openai_failure(403, &none, other, None).kind,
            FailureKind::Other
        );
        // The request id is OpenAI's header.
        let f = openai_failure(
            400,
            &headers(&[("x-request-id", "req_8f2")]),
            br#"{"id": "not-a-request", "error": {"message": "Bad size"}}"#,
            None,
        );
        assert_eq!(f.request_id.as_deref(), Some("req_8f2"));
    }
}
