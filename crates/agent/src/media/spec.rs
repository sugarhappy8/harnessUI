//! The controls each picture and video model offers ([`ParamSpec`]), read from the companies'
//! own catalogues, and the check of a request against them ([`validate`]).
//!
//! - **OpenRouter pictures** ([`openrouter_image_models`]): each `supported_parameters` entry
//!   becomes a typed field (Shape, How many, Quality, Resolution, reference pictures, and
//!   under Advanced the file type, compression, background, seed and size). A key Studio has
//!   no field for is offered under Extra fields and sent at the top level, as the catalogue
//!   lists it, so no setting the catalogue names is dropped. A saved `/endpoints` answer adds
//!   the companies that run the model and the extra settings each of them takes, which are
//!   sent under `provider.options[<provider_slug>]`.
//! - **OpenRouter video** ([`openrouter_video_models`]): from `supported_aspect_ratios`,
//!   `supported_durations`, `supported_resolutions`, `supported_frame_images`,
//!   `supported_sizes`, `generate_audio` and `seed`. The first try is cheap: the shortest
//!   length of at least 4 s and 720p (or the lowest resolution). Extra settings are named in
//!   a note but not offered: OpenRouter keys them by the company a job is routed to, which
//!   the video catalogue does not name.
//! - **OpenAI pictures** ([`super::openai::openai_image_models`]): the GPT Image models of
//!   `GET /v1/models`, each with the controls of its entry in the bundled, dated settings
//!   file.
//! - **This computer**: the models the local provider built, read as they are.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::price::{parse_image_endpoints, parse_pricing_skus};
use super::{Company, MediaKind, MediaModel, MediaRequest, Price, RawCatalog, RefRole, Routing};

/// The largest seed: 2³¹ − 1, the largest signed 32-bit integer.
pub const SEED_MAX: u64 = 2_147_483_647;

/// OpenAI's custom picture sizes: both sides multiples of 16, the long side at most three
/// times the short one, the long side at most 3840 px, and 655,360 to 8,294,400 pixels.
pub const OPENAI_SIZE_RULES: SizeRules = SizeRules {
    multiple_of: 16,
    min_ratio: 1.0 / 3.0,
    max_ratio: 3.0,
    max_side: 3840,
    min_pixels: 655_360,
    max_pixels: 8_294_400,
};

/// The longest prompt sent to a cloud company, in characters.
const PROMPT_MAX_CHARS: usize = 32_000;

/// Names an extra setting never takes: they belong to the request itself.
const RESERVED: &[&str] = &["model", "prompt", "provider"];

/// The key of the Extra fields entry.
const EXTRA_KEY: &str = "extra";

/// The order of picture fields: the simple ones in front, then Advanced, then Extra fields.
const PICTURE_ORDER: &[&str] = &[
    "aspect_ratio",
    "n",
    "quality",
    "resolution",
    "input_references",
    "output_format",
    "output_compression",
    "frame_count",
    "frame_duration_ms",
    "background",
    "seed",
    "size",
];

/// Why a video model cannot be used: it changes a video, and Studio cannot add one.
const NEEDS_A_VIDEO: &str = "Needs a video to start from, which Studio does not offer yet.";

/// The note on every OpenRouter video model.
const VIDEO_NOT_ZDR: &str = "OpenRouter video is not covered by zero data retention.";

/// The note on every Google model.
const SYNTHID: &str = "Google adds an invisible SynthID watermark.";

/// One control a model offers. The webview draws every control from these, so it has no
/// code per company.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParamSpec {
    /// The name sent to the company, for example `quality`.
    pub key: String,
    /// What the field means on any model, so a value can be carried to another model.
    pub canonical: Option<Canonical>,
    /// The field's name on screen.
    pub label: String,
    /// A plain sentence shown with the field.
    pub help: Option<String>,
    /// In front, or under Advanced.
    pub layer: Layer,
    /// The kind of value and its limits.
    pub kind: ParamKind,
    /// What the company uses when the field is not sent. A field that is not set is not sent.
    pub company_default: Option<Value>,
    /// The value Studio starts with, where it picks one (a video's length and resolution).
    pub initial: Option<Value>,
    /// Whether the price estimate can change with this field.
    pub affects_price: bool,
    /// Values another field must have for this one to apply. A field sent with its company
    /// default counts as not set, and needs nothing.
    pub requires: Option<Requires>,
}

/// What a field means on any model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Canonical {
    /// The aspect ratio.
    Shape,
    /// How many pictures.
    Count,
    /// The quality step.
    Quality,
    /// The resolution.
    Resolution,
    /// A video's length.
    Length,
    /// Whether a video has sound.
    Sound,
    /// The seed.
    Seed,
    /// Reference pictures.
    References,
    /// A video's first frame.
    StartFrame,
    /// A video's last frame.
    EndFrame,
    /// An exact size in pixels.
    Size,
    /// The file type.
    Format,
    /// A transparent or opaque background.
    Background,
}

/// Where a field is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layer {
    /// Always in front.
    Simple,
    /// Under Advanced.
    Advanced,
}

/// The kind of value a field takes, and its limits.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ParamKind {
    /// One value from a list.
    Enum {
        /// The values offered.
        values: Vec<Choice>,
    },
    /// A number from `min` to `max`.
    Range {
        /// The smallest value.
        min: f64,
        /// The largest value.
        max: f64,
        /// The values are `min` plus whole steps of this, when set.
        step: Option<f64>,
        /// The unit shown after the number, for example `ms`.
        unit: Option<String>,
    },
    /// On or off.
    Boolean,
    /// A seed: a whole number from 0 to `max`.
    Seed {
        /// The largest seed.
        max: u64,
    },
    /// Added pictures with one role, from `min` to `max` of them. They come from the
    /// request's pictures, never from a value.
    Images {
        /// The fewest pictures the model needs.
        min: u32,
        /// The most it takes.
        max: u32,
        /// What they are for.
        role: RefRole,
    },
    /// An exact size: one of `values`, or a `WxH` within `rules` ([`check_size`]).
    Size {
        /// The rules a custom size must meet.
        rules: SizeRules,
        /// The sizes offered by name.
        values: Vec<Choice>,
    },
    /// Free text of at most `max_chars` characters.
    Text {
        /// The longest text, in characters.
        max_chars: u32,
    },
    /// Settings beyond the typed fields, by name, entered as JSON.
    Extra {
        /// The names the model takes.
        allowed_keys: Vec<String>,
    },
}

/// One company that runs an OpenRouter picture model: a record of the model's `/endpoints`
/// answer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EndpointCompany {
    /// `provider_slug`: where its extra settings go (`provider.options[<slug>]`).
    pub slug: String,
    /// `provider_tag`: its name in a request's routing (`order`, `only`).
    pub tag: String,
    /// `provider_name`, shown on screen.
    pub name: String,
    /// `allowed_passthrough_parameters`: the extra settings it takes.
    pub passthrough: Vec<String>,
}

/// One value of a list field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    /// The value sent, a string or a number, as the company lists it.
    pub value: Value,
    /// The value's name on screen.
    pub label: String,
}

/// Values another field must have for a field to apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Requires {
    /// The other field's key.
    pub key: String,
    /// The values it must have. A field that is not set counts as its company default.
    pub values: Vec<Value>,
}

/// The rules a custom `WxH` size must meet.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeRules {
    /// Both sides are multiples of this.
    pub multiple_of: u32,
    /// The smallest width divided by height.
    pub min_ratio: f64,
    /// The largest width divided by height.
    pub max_ratio: f64,
    /// The longest side, in pixels.
    pub max_side: u32,
    /// The fewest pixels.
    pub min_pixels: u64,
    /// The most pixels.
    pub max_pixels: u64,
}

/// Why a custom size does not meet its rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SizeProblem {
    /// It is not two whole numbers written `WxH`.
    NotWxH,
    /// A side is not a multiple of the rules' `multiple_of`.
    MultipleOf,
    /// One side is too long for the other.
    Ratio,
    /// The longest side is too long.
    LongestSide,
    /// Too few pixels.
    TooFewPixels,
    /// Too many pixels.
    TooManyPixels,
}

/// What is wrong with a request for a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Invalid {
    /// The model offers no setting of this name.
    NotOffered(String),
    /// The setting's value is not one the model takes.
    BadValue(String),
    /// Setting `key` applies only when the field `needs` has one of the values it lists.
    Requires {
        /// The setting that does not apply.
        key: String,
        /// The field it depends on.
        needs: String,
    },
    /// More pictures with this role than the model takes.
    TooManyPictures {
        /// What the pictures are for.
        role: RefRole,
        /// The most the model takes.
        max: u32,
    },
    /// Fewer pictures with this role than the model needs.
    TooFewPictures {
        /// What the pictures are for.
        role: RefRole,
        /// The fewest the model needs.
        min: u32,
    },
    /// An extra setting that has its own field, or belongs to the request itself.
    Reserved(String),
    /// The prompt is empty.
    EmptyPrompt,
    /// The prompt is longer than 32,000 characters.
    PromptTooLong,
    /// The model cannot be used; the reason.
    NotUsable(String),
}

/// The models in a company's saved lists, with their prices. `image_prices` holds the saved
/// OpenRouter `/endpoints` answers by model id: they give the companies that run a picture
/// model, the extra settings each takes and the picture's price ([`parse_image_endpoints`]).
/// A video's price is its `pricing_skus` ([`parse_pricing_skus`]); a picture with no saved
/// answer, or a video without one, has [`Price::Unknown`].
///
/// OpenRouter gives its pictures, then its videos. OpenAI gives the picture models of its
/// `/v1/models` list in the list's order, each read against the bundled settings file
/// ([`super::openai::openai_image_models`]); `image_prices` plays no part, since that file
/// carries OpenAI's prices. For [`Company::Local`], `raw.images` is the JSON array of models
/// the local provider built, read as it is (with the [`Price::Free`] it gave them); none when
/// it cannot be read.
pub fn parse_catalog(
    company: Company,
    raw: &RawCatalog,
    image_prices: &BTreeMap<String, Value>,
) -> Vec<MediaModel> {
    match company {
        Company::OpenRouter => {
            let mut models = openrouter_image_models(&raw.images, image_prices);
            if let Some(videos) = &raw.videos {
                models.extend(openrouter_video_models(videos));
            }
            models
        }
        Company::OpenAi => super::openai::openai_image_models(&raw.images),
        Company::Local => Vec::<MediaModel>::deserialize(&raw.images).unwrap_or_default(),
    }
}

/// OpenRouter's picture models (`GET /api/v1/images/models`) with their controls.
/// `image_prices` holds the saved `/endpoints` answers by model id.
pub fn openrouter_image_models(
    list: &Value,
    image_prices: &BTreeMap<String, Value>,
) -> Vec<MediaModel> {
    entries(list)
        .filter_map(|entry| {
            let id = entry.get("id")?.as_str()?;
            Some(picture_model(id, entry, image_prices.get(id)))
        })
        .collect()
}

/// The companies in one saved `/endpoints` answer, one per record, in the answer's order.
/// A record without a `provider_slug` is left out: its settings could not be addressed.
pub fn endpoint_companies(answer: &Value) -> Vec<EndpointCompany> {
    answer
        .get("endpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|record| {
            let slug = record.get("provider_slug")?.as_str()?;
            let text = |key: &str| record.get(key).and_then(Value::as_str).unwrap_or(slug);
            Some(EndpointCompany {
                slug: slug.to_owned(),
                tag: text("provider_tag").to_owned(),
                name: text("provider_name").to_owned(),
                passthrough: strings(record.get("allowed_passthrough_parameters"))
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

/// OpenRouter's video models (`GET /api/v1/videos/models`) with their controls.
pub fn openrouter_video_models(list: &Value) -> Vec<MediaModel> {
    entries(list)
        .filter_map(|entry| Some(video_model(entry.get("id")?.as_str()?, entry)))
        .collect()
}

/// A model's family: its maker and its name without the version, so versions of one model
/// group together: `google/veo-3.1-fast` → `google/veo`, `kwaivgi/kling-v3.0-pro` →
/// `kwaivgi/kling`. The name is cut before the first `-` followed by a digit, or by `v` and
/// a digit.
pub fn family_of(id: &str) -> String {
    let (maker, name) = match id.split_once('/') {
        Some((maker, name)) => (Some(maker), name),
        None => (None, id),
    };
    let bytes = name.as_bytes();
    let version_at = |i: usize| match bytes.get(i + 1) {
        Some(b'v') => bytes.get(i + 2).is_some_and(u8::is_ascii_digit),
        Some(next) => next.is_ascii_digit(),
        None => false,
    };
    let cut = name
        .match_indices('-')
        .map(|(i, _)| i)
        .find(|&i| version_at(i))
        .unwrap_or(name.len());
    match maker {
        Some(maker) => format!("{maker}/{}", &name[..cut]),
        None => name[..cut].to_owned(),
    }
}

/// Checks a request against the model's controls: every setting is one the model offers,
/// with a value it takes; extra settings are ones it lists (a company setting also needs a
/// company the routing allows that lists it), never a typed field's name, `model`, `prompt`
/// or `provider`; the pictures of each role are within the model's limits; the prompt is
/// 1 to 32,000 characters. A model that cannot be used is refused. A setting sent with its
/// company default is the same as one left out, so what it requires is not checked.
///
/// # Errors
/// Every problem found, in the order: prompt, settings, extra settings, pictures.
pub fn validate(model: &MediaModel, req: &MediaRequest) -> Result<(), Vec<Invalid>> {
    if let Some(reason) = &model.usable {
        return Err(vec![Invalid::NotUsable(reason.clone())]);
    }
    let mut problems = Vec::new();
    if req.prompt.trim().is_empty() {
        problems.push(Invalid::EmptyPrompt);
    } else if req.prompt.chars().count() > PROMPT_MAX_CHARS {
        problems.push(Invalid::PromptTooLong);
    }

    for (key, value) in &req.params {
        let Some(spec) = model
            .params
            .iter()
            .find(|p| p.key == *key && takes_a_value(&p.kind))
        else {
            problems.push(Invalid::NotOffered(key.clone()));
            continue;
        };
        if !fits(&spec.kind, value) {
            problems.push(Invalid::BadValue(key.clone()));
        } else if !is_company_default(spec, value)
            && let Some(problem) = unmet(model, &req.params, spec)
        {
            problems.push(problem);
        }
    }

    let allowed = model
        .params
        .iter()
        .find_map(|p| match &p.kind {
            ParamKind::Extra { allowed_keys } => Some(allowed_keys.as_slice()),
            _ => None,
        })
        .unwrap_or_default();
    for key in req.extra.keys() {
        let typed = model
            .params
            .iter()
            .any(|p| p.key == *key && !matches!(p.kind, ParamKind::Extra { .. }));
        if typed || RESERVED.contains(&key.as_str()) {
            problems.push(Invalid::Reserved(key.clone()));
        } else if !allowed.contains(key) || !reachable(model, key, req.routing.as_ref()) {
            problems.push(Invalid::NotOffered(key.clone()));
        }
    }

    let mut roles = Vec::new();
    for spec in &model.params {
        let ParamKind::Images { min, max, role } = spec.kind else {
            continue;
        };
        roles.push(role);
        let count = req.refs.iter().filter(|r| r.role == role).count();
        if count > max as usize {
            problems.push(Invalid::TooManyPictures { role, max });
        } else if count < min as usize {
            problems.push(Invalid::TooFewPictures { role, min });
        }
        if count > 0
            && let Some(problem) = unmet(model, &req.params, spec)
        {
            problems.push(problem);
        }
    }
    for role in [RefRole::Reference, RefRole::StartFrame, RefRole::EndFrame] {
        if !roles.contains(&role) && req.refs.iter().any(|r| r.role == role) {
            problems.push(Invalid::TooManyPictures { role, max: 0 });
        }
    }

    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

/// Checks a custom `WxH` size against `rules`.
///
/// # Errors
/// The first rule it breaks, in the order of [`SizeProblem`].
pub fn check_size(size: &str, rules: &SizeRules) -> Result<(), SizeProblem> {
    let side = |text: &str| {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
            .filter(|&n| n > 0)
    };
    let (width, height) = size
        .split_once('x')
        .and_then(|(w, h)| Some((side(w)?, side(h)?)))
        .ok_or(SizeProblem::NotWxH)?;
    if rules.multiple_of > 0 && (width % rules.multiple_of != 0 || height % rules.multiple_of != 0)
    {
        return Err(SizeProblem::MultipleOf);
    }
    let ratio = f64::from(width) / f64::from(height);
    if ratio < rules.min_ratio - 1e-9 || ratio > rules.max_ratio + 1e-9 {
        return Err(SizeProblem::Ratio);
    }
    if width.max(height) > rules.max_side {
        return Err(SizeProblem::LongestSide);
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels < rules.min_pixels {
        return Err(SizeProblem::TooFewPixels);
    }
    if pixels > rules.max_pixels {
        return Err(SizeProblem::TooManyPixels);
    }
    Ok(())
}

/// Whether an OpenRouter request with this routing may reach the company with this
/// `provider_tag`: with `only`, or with fallbacks off, only the companies in `order` run it.
pub(crate) fn routing_allows(routing: Option<&Routing>, tag: &str) -> bool {
    match routing {
        Some(r) if (r.only || !r.allow_fallbacks) && !r.order.is_empty() => {
            r.order.iter().any(|t| t == tag)
        }
        _ => true,
    }
}

// ---- reading the catalogues ------------------------------------------------------------------

/// The entries of a model list: its `data` array, or the list itself when it is an array.
pub(super) fn entries(list: &Value) -> impl Iterator<Item = &Value> {
    list.get("data")
        .unwrap_or(list)
        .as_array()
        .into_iter()
        .flatten()
}

/// The strings of a JSON array; nothing for anything else.
fn strings(value: Option<&Value>) -> impl Iterator<Item = &str> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// The name shown: the catalogue's `name` without its `"<Maker>: "` prefix, else the id.
fn display_name(entry: &Value, id: &str) -> String {
    match entry.get("name").and_then(Value::as_str).map(str::trim) {
        Some(name) if !name.is_empty() => name
            .split_once(": ")
            .map_or(name, |(_, rest)| rest)
            .to_owned(),
        _ => id.to_owned(),
    }
}

/// A field with no help, default, starting value, price effect or requirement.
pub(super) fn field(
    key: &str,
    canonical: Option<Canonical>,
    label: &str,
    layer: Layer,
    kind: ParamKind,
) -> ParamSpec {
    ParamSpec {
        key: key.to_owned(),
        canonical,
        label: label.to_owned(),
        help: None,
        layer,
        kind,
        company_default: None,
        initial: None,
        affects_price: false,
        requires: None,
    }
}

/// The seed field, with the honest note about what a seed is.
fn seed_field(kind: MediaKind) -> ParamSpec {
    let result = match kind {
        MediaKind::Image => "picture",
        MediaKind::Video => "video",
    };
    ParamSpec {
        help: Some(format!(
            "A seed is not a saved copy; the same seed on another model or version gives a \
             different {result}."
        )),
        ..field(
            "seed",
            Some(Canonical::Seed),
            "Seed",
            Layer::Advanced,
            ParamKind::Seed { max: SEED_MAX },
        )
    }
}

/// A value's name on screen.
pub(super) fn choice_label(value: &Value) -> String {
    let Some(text) = value.as_str() else {
        return value.to_string();
    };
    match text {
        "auto" => "Auto",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "max" => "Max",
        "transparent" => "Transparent",
        "opaque" => "Opaque",
        "png" => "PNG",
        "jpeg" => "JPEG",
        "webp" => "WebP",
        "gif" => "GIF",
        "svg" => "SVG",
        other => other,
    }
    .to_owned()
}

/// The choices of a list of strings and numbers; `None` when there are none.
fn choices_of(values: Option<&Value>) -> Option<Vec<Choice>> {
    let choices: Vec<Choice> = values?
        .as_array()?
        .iter()
        .filter(|v| v.is_string() || v.is_number())
        .map(|v| Choice {
            value: v.clone(),
            label: choice_label(v),
        })
        .collect();
    (!choices.is_empty()).then_some(choices)
}

/// The choices of an `{"type": "enum", "values": […]}` catalogue entry.
fn enum_choices(spec: &Value) -> Option<Vec<Choice>> {
    (spec.get("type")?.as_str()? == "enum")
        .then(|| choices_of(spec.get("values")))
        .flatten()
}

/// The bounds of a `{"type": "range", "min": …, "max": …}` catalogue entry.
fn range_bounds(spec: &Value) -> Option<(f64, f64)> {
    (spec.get("type")?.as_str()? == "range")
        .then(|| Some((spec.get("min")?.as_f64()?, spec.get("max")?.as_f64()?)))
        .flatten()
}

/// Whether a picture catalogue entry leaves nothing to set: `n` of at most one picture, or
/// `input_references` of none.
fn nothing_to_set(key: &str, spec: &Value) -> bool {
    match (key, range_bounds(spec)) {
        ("n", Some((_, max))) => max <= 1.0,
        ("input_references", Some((_, max))) => max < 1.0,
        _ => false,
    }
}

/// The typed field for a picture catalogue entry; `None` for a key Studio has no field for
/// (or one in a shape it does not expect), which is offered under Extra fields instead.
fn picture_field(id: &str, key: &str, spec: &Value) -> Option<ParamSpec> {
    let list = |canonical, label, layer| {
        enum_choices(spec)
            .map(|values| field(key, canonical, label, layer, ParamKind::Enum { values }))
    };
    match key {
        "aspect_ratio" => list(Some(Canonical::Shape), "Shape", Layer::Simple),
        "quality" => list(Some(Canonical::Quality), "Quality", Layer::Simple).map(|f| ParamSpec {
            affects_price: true,
            ..f
        }),
        "resolution" => {
            list(Some(Canonical::Resolution), "Resolution", Layer::Simple).map(|f| ParamSpec {
                affects_price: true,
                ..f
            })
        }
        "output_format" => list(Some(Canonical::Format), "File type", Layer::Advanced),
        "background" => list(Some(Canonical::Background), "Background", Layer::Advanced),
        "n" => range_bounds(spec).map(|(min, max)| ParamSpec {
            affects_price: true,
            ..field(
                key,
                Some(Canonical::Count),
                "How many",
                Layer::Simple,
                ParamKind::Range {
                    min,
                    max,
                    step: Some(1.0),
                    unit: None,
                },
            )
        }),
        "input_references" => {
            range_bounds(spec)
                .filter(|&(min, _)| min >= 0.0)
                .map(|(min, max)| ParamSpec {
                    affects_price: true,
                    ..field(
                        key,
                        Some(Canonical::References),
                        "Reference pictures",
                        Layer::Simple,
                        ParamKind::Images {
                            min: whole(min),
                            max: whole(max),
                            role: RefRole::Reference,
                        },
                    )
                })
        }
        "output_compression" => range_bounds(spec).map(|(min, max)| ParamSpec {
            requires: Some(Requires {
                key: "output_format".to_owned(),
                values: vec![Value::from("jpeg"), Value::from("webp")],
            }),
            ..field(
                key,
                None,
                "Compression",
                Layer::Advanced,
                ParamKind::Range {
                    min,
                    max,
                    step: Some(1.0),
                    unit: Some("%".to_owned()),
                },
            )
        }),
        "seed" if is_boolean(spec) => Some(seed_field(MediaKind::Image)),
        "size" if is_boolean(spec) && id.starts_with("openai/") => Some(ParamSpec {
            affects_price: true,
            ..field(
                key,
                Some(Canonical::Size),
                "Exact size",
                Layer::Advanced,
                ParamKind::Size {
                    rules: OPENAI_SIZE_RULES,
                    values: Vec::new(),
                },
            )
        }),
        _ => None,
    }
}

/// Whether a catalogue entry is `{"type": "boolean"}`: the model takes the setting.
fn is_boolean(spec: &Value) -> bool {
    spec.get("type").and_then(Value::as_str) == Some("boolean")
}

/// A count from a catalogue range, as a whole number.
fn whole(n: f64) -> u32 {
    // Catalogue counts are small whole numbers; `as` saturates anything else.
    n.round() as u32
}

/// GPT Image 2.6's animated GIF fields, which need `output_format` = `gif`.
pub(super) fn gif_fields() -> [ParamSpec; 2] {
    let gif = || {
        Some(Requires {
            key: "output_format".to_owned(),
            values: vec![Value::from("gif")],
        })
    };
    let range = |min, max, unit: Option<&str>| ParamKind::Range {
        min,
        max,
        step: Some(1.0),
        unit: unit.map(str::to_owned),
    };
    [
        ParamSpec {
            affects_price: true,
            requires: gif(),
            ..field(
                "frame_count",
                None,
                "Frames",
                Layer::Advanced,
                range(2.0, 16.0, None),
            )
        },
        ParamSpec {
            requires: gif(),
            ..field(
                "frame_duration_ms",
                None,
                "Time per frame",
                Layer::Advanced,
                range(20.0, 2000.0, Some("ms")),
            )
        },
    ]
}

/// One OpenRouter picture catalogue entry, with its controls; `answer` is the model's saved
/// `/endpoints` answer, when there is one.
fn picture_model(id: &str, entry: &Value, answer: Option<&Value>) -> MediaModel {
    let empty = Map::new();
    let supported = entry
        .get("supported_parameters")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let mut params = Vec::new();
    let mut extra_keys = Vec::new();
    for (key, spec) in supported {
        if nothing_to_set(key, spec) {
            continue;
        }
        match picture_field(id, key, spec) {
            Some(param) => params.push(param),
            None => extra_keys.push(key.clone()),
        }
    }
    let animated = supported
        .get("output_format")
        .and_then(enum_choices)
        .is_some_and(|formats| formats.iter().any(|c| c.value == "gif"));
    if id.starts_with("openai/gpt-image-2.6-") && animated {
        params.extend(gif_fields());
    }
    let rank = |key: &str| {
        PICTURE_ORDER
            .iter()
            .position(|k| *k == key)
            .unwrap_or(PICTURE_ORDER.len())
    };
    params.sort_by_key(|p| rank(&p.key));

    let companies = answer.map(endpoint_companies).unwrap_or_default();
    for name in companies.iter().flat_map(|c| &c.passthrough) {
        if !extra_keys.contains(name) {
            extra_keys.push(name.clone());
        }
    }
    if !extra_keys.is_empty() {
        params.push(field(
            EXTRA_KEY,
            None,
            "Extra fields (JSON)",
            Layer::Advanced,
            ParamKind::Extra {
                allowed_keys: extra_keys,
            },
        ));
    }

    let mut notes = Vec::new();
    if id.starts_with("google/") {
        notes.push(SYNTHID.to_owned());
    }
    MediaModel {
        id: id.to_owned(),
        display_name: display_name(entry, id),
        family: family_of(id),
        kind: MediaKind::Image,
        accepts_images: takes_pictures(&params),
        params,
        price: answer.map_or(Price::Unknown, |a| parse_image_endpoints(a, None)),
        notes,
        usable: None,
        created: entry.get("created").and_then(Value::as_i64),
        companies,
    }
}

/// A resolution's height, enough to order them: `720p` → 720; a `K` counts 1024 lines.
fn resolution_lines(label: &str) -> Option<f64> {
    let lower = label.trim().to_ascii_lowercase();
    match lower.strip_suffix('k') {
        Some(thousands) => thousands.parse::<f64>().ok().map(|k| k * 1024.0),
        None => lower.strip_suffix('p').unwrap_or(&lower).parse().ok(),
    }
}

/// One OpenRouter video catalogue entry, with its controls and notes.
fn video_model(id: &str, entry: &Value) -> MediaModel {
    let mut params = Vec::new();

    if let Some(values) = choices_of(entry.get("supported_aspect_ratios")) {
        params.push(field(
            "aspect_ratio",
            Some(Canonical::Shape),
            "Shape",
            Layer::Simple,
            ParamKind::Enum { values },
        ));
    }

    let mut lengths: Vec<(f64, &Value)> = entry
        .get("supported_durations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| Some((v.as_f64()?, v)))
        .filter(|(seconds, _)| *seconds > 0.0)
        .collect();
    lengths.sort_by(|a, b| a.0.total_cmp(&b.0));
    lengths.dedup_by(|a, b| a.0 == b.0);
    let first_try = lengths
        .iter()
        .find(|(seconds, _)| *seconds >= 4.0)
        .or(lengths.last());
    if let Some((_, initial)) = first_try {
        let values = lengths
            .iter()
            .map(|(_, v)| Choice {
                value: (*v).clone(),
                label: format!("{v} s"),
            })
            .collect();
        params.push(ParamSpec {
            initial: Some((*initial).clone()),
            affects_price: true,
            ..field(
                "duration",
                Some(Canonical::Length),
                "Length",
                Layer::Simple,
                ParamKind::Enum { values },
            )
        });
    }

    if let Some(values) = choices_of(entry.get("supported_resolutions")) {
        let initial = values
            .iter()
            .find(|c| c.value == "720p")
            .or_else(|| {
                values
                    .iter()
                    .filter_map(|c| Some((resolution_lines(c.value.as_str()?)?, c)))
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, c)| c)
            })
            .unwrap_or(&values[0])
            .value
            .clone();
        params.push(ParamSpec {
            initial: Some(initial),
            affects_price: true,
            ..field(
                "resolution",
                Some(Canonical::Resolution),
                "Resolution",
                Layer::Simple,
                ParamKind::Enum { values },
            )
        });
    }

    if entry.get("generate_audio") == Some(&Value::Bool(true)) {
        params.push(ParamSpec {
            company_default: Some(Value::Bool(true)),
            affects_price: true,
            ..field(
                "generate_audio",
                Some(Canonical::Sound),
                "Sound",
                Layer::Simple,
                ParamKind::Boolean,
            )
        });
    }

    let frames: Vec<&str> = strings(entry.get("supported_frame_images")).collect();
    let frame = |role| ParamKind::Images {
        min: 0,
        max: 1,
        role,
    };
    if frames.contains(&"first_frame") {
        params.push(ParamSpec {
            affects_price: true,
            ..field(
                "first_frame",
                Some(Canonical::StartFrame),
                "Start frame",
                Layer::Simple,
                frame(RefRole::StartFrame),
            )
        });
    }
    if frames.contains(&"last_frame") {
        params.push(field(
            "last_frame",
            Some(Canonical::EndFrame),
            "End frame",
            Layer::Advanced,
            frame(RefRole::EndFrame),
        ));
    }

    if entry.get("seed") == Some(&Value::Bool(true)) {
        params.push(seed_field(MediaKind::Video));
    }

    if let Some(mut values) = choices_of(entry.get("supported_sizes")) {
        for choice in &mut values {
            choice.label = choice.label.replace('x', " × ");
        }
        params.push(ParamSpec {
            help: Some("When set, Shape and Resolution are not sent.".to_owned()),
            affects_price: true,
            ..field(
                "size",
                Some(Canonical::Size),
                "Exact size",
                Layer::Advanced,
                ParamKind::Enum { values },
            )
        });
    }

    let mut notes = vec![VIDEO_NOT_ZDR.to_owned()];
    if id.starts_with("google/") {
        notes.push(SYNTHID.to_owned());
    }
    let passthrough: Vec<&str> = strings(entry.get("allowed_passthrough_parameters")).collect();
    if !passthrough.is_empty() {
        notes.push(format!(
            "OpenRouter lists extra settings for this model ({}), but Studio cannot send them \
             for video yet.",
            passthrough.join(", ")
        ));
    }
    let has_lengths = entry
        .get("supported_durations")
        .and_then(Value::as_array)
        .is_some_and(|d| !d.is_empty());

    MediaModel {
        id: id.to_owned(),
        display_name: display_name(entry, id),
        family: family_of(id),
        kind: MediaKind::Video,
        accepts_images: takes_pictures(&params),
        params,
        price: entry
            .get("pricing_skus")
            .and_then(Value::as_object)
            .map_or(Price::Unknown, parse_pricing_skus),
        notes,
        usable: (!has_lengths).then(|| NEEDS_A_VIDEO.to_owned()),
        created: entry.get("created").and_then(Value::as_i64),
        companies: Vec::new(),
    }
}

/// Whether any field takes added pictures.
fn takes_pictures(params: &[ParamSpec]) -> bool {
    params
        .iter()
        .any(|p| matches!(p.kind, ParamKind::Images { max, .. } if max > 0))
}

// ---- checking a request ----------------------------------------------------------------------

/// Whether a field is set through a value in `params` (pictures and extra settings are not).
fn takes_a_value(kind: &ParamKind) -> bool {
    !matches!(kind, ParamKind::Images { .. } | ParamKind::Extra { .. })
}

/// Two values are the same: numbers by their value (`4` and `4.0`), anything else exactly.
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        _ => a == b,
    }
}

/// Whether `value` is one the field takes.
fn fits(kind: &ParamKind, value: &Value) -> bool {
    match kind {
        ParamKind::Enum { values } => values.iter().any(|c| same_value(&c.value, value)),
        ParamKind::Range { min, max, step, .. } => value.as_f64().is_some_and(|x| {
            (*min..=*max).contains(&x) && step.is_none_or(|step| on_step(x - min, step))
        }),
        ParamKind::Boolean => value.is_boolean(),
        ParamKind::Seed { max } => value.as_u64().is_some_and(|seed| seed <= *max),
        ParamKind::Size { rules, values } => value.as_str().is_some_and(|size| {
            values.iter().any(|c| c.value == size) || check_size(size, rules).is_ok()
        }),
        ParamKind::Text { max_chars } => value
            .as_str()
            .is_some_and(|text| text.chars().count() <= *max_chars as usize),
        ParamKind::Images { .. } | ParamKind::Extra { .. } => false,
    }
}

/// Whether `offset` is a whole number of `step`s.
fn on_step(offset: f64, step: f64) -> bool {
    if step <= 0.0 {
        return true;
    }
    let steps = offset / step;
    (steps - steps.round()).abs() <= 1e-9 * steps.abs().max(1.0)
}

/// Whether `value` is `spec`'s company default: sending it is the same as leaving it out.
pub(super) fn is_company_default(spec: &ParamSpec, value: &Value) -> bool {
    spec.company_default
        .as_ref()
        .is_some_and(|default| same_value(default, value))
}

/// The problem when `spec`'s requirement is not met. A field that is not set counts as its
/// company default.
pub(super) fn unmet(
    model: &MediaModel,
    params: &Map<String, Value>,
    spec: &ParamSpec,
) -> Option<Invalid> {
    let requires = spec.requires.as_ref()?;
    let current = params.get(&requires.key).or_else(|| {
        model
            .params
            .iter()
            .find(|p| p.key == requires.key)?
            .company_default
            .as_ref()
    });
    let met = current.is_some_and(|v| requires.values.iter().any(|w| same_value(v, w)));
    (!met).then(|| Invalid::Requires {
        key: spec.key.clone(),
        needs: requires.key.clone(),
    })
}

/// Whether an extra setting can reach a company that takes it: a name no company lists is
/// sent at the top level; one that some company lists needs one the routing allows.
fn reachable(model: &MediaModel, key: &str, routing: Option<&Routing>) -> bool {
    let mut listing = model
        .companies
        .iter()
        .filter(|c| c.passthrough.iter().any(|name| name == key))
        .peekable();
    listing.peek().is_none() || listing.any(|c| routing_allows(routing, &c.tag))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    use async_trait::async_trait;
    use serde_json::{Map, Value, json};

    use super::*;
    use crate::client::ProviderKind;
    use crate::media::{
        FailureKind, Fetched, JobDone, JobState, MediaError, MediaLimits, MediaProvider, RefImage,
        RequestPreview, Started, media_company,
    };

    const IMAGES: &str =
        include_str!("../../tests/fixtures/media/openrouter-images-2026-11-17.json");
    const VIDEOS: &str =
        include_str!("../../tests/fixtures/media/openrouter-videos-2026-11-17.json");
    const ENDPOINTS: &str =
        include_str!("../../tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json");

    fn endpoint_answers() -> BTreeMap<String, Value> {
        let saved: Value = serde_json::from_str(ENDPOINTS).unwrap();
        saved["answers"]
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, answer)| (id.clone(), answer.clone()))
            .collect()
    }

    fn find(models: Vec<MediaModel>, id: &str) -> MediaModel {
        models
            .into_iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("no model {id}"))
    }

    fn image(id: &str) -> MediaModel {
        let list = serde_json::from_str(IMAGES).unwrap();
        find(openrouter_image_models(&list, &BTreeMap::new()), id)
    }

    fn image_with_endpoints(id: &str) -> MediaModel {
        let list = serde_json::from_str(IMAGES).unwrap();
        find(openrouter_image_models(&list, &endpoint_answers()), id)
    }

    fn video(id: &str) -> MediaModel {
        find(
            openrouter_video_models(&serde_json::from_str(VIDEOS).unwrap()),
            id,
        )
    }

    fn catalogue_entries(text: &str) -> Vec<(String, Value)> {
        let list: Value = serde_json::from_str(text).unwrap();
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| (entry["id"].as_str().unwrap().to_owned(), entry.clone()))
            .collect()
    }

    fn maybe<'a>(m: &'a MediaModel, key: &str) -> Option<&'a ParamSpec> {
        m.params.iter().find(|p| p.key == key)
    }

    fn param<'a>(m: &'a MediaModel, key: &str) -> &'a ParamSpec {
        maybe(m, key).unwrap_or_else(|| panic!("{} has no field {key}", m.id))
    }

    fn values(m: &MediaModel, key: &str) -> Vec<Value> {
        match &param(m, key).kind {
            ParamKind::Enum { values } => values.iter().map(|c| c.value.clone()).collect(),
            other => panic!("{key} is not a list: {other:?}"),
        }
    }

    fn object(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            other => panic!("not an object: {other}"),
        }
    }

    fn request(model: &str, params: Value) -> MediaRequest {
        MediaRequest {
            kind: MediaKind::Image,
            model: model.to_owned(),
            prompt: "a lighthouse at dusk".to_owned(),
            params: object(params),
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
            provider_options: BTreeMap::new(),
        }
    }

    fn request_p(params: Value) -> MediaRequest {
        request("test/model", params)
    }

    fn request_x(extra: Value) -> MediaRequest {
        MediaRequest {
            extra: object(extra),
            ..request("test/model", json!({}))
        }
    }

    fn with_refs(req: MediaRequest, role: RefRole, count: usize) -> MediaRequest {
        let refs = (0..count)
            .map(|i| RefImage {
                role,
                mime: "image/png".to_owned(),
                path: PathBuf::from(format!("/gallery/refs/{i}.png")),
                sha256: format!("{i:064x}"),
                size_bytes: 1_200,
            })
            .collect();
        MediaRequest { refs, ..req }
    }

    fn field(key: &str, kind: ParamKind) -> ParamSpec {
        ParamSpec {
            key: key.to_owned(),
            canonical: None,
            label: key.to_owned(),
            help: None,
            layer: Layer::Advanced,
            kind,
            company_default: None,
            initial: None,
            affects_price: false,
            requires: None,
        }
    }

    fn hand_made(params: Vec<ParamSpec>) -> MediaModel {
        MediaModel {
            id: "test/hand-made".to_owned(),
            display_name: "Hand made".to_owned(),
            family: "test/hand-made".to_owned(),
            kind: MediaKind::Image,
            accepts_images: false,
            params,
            price: Price::Unknown,
            notes: Vec::new(),
            usable: None,
            created: None,
            companies: Vec::new(),
        }
    }

    fn choices(values: &[&str]) -> Vec<Choice> {
        values
            .iter()
            .map(|v| Choice {
                value: json!(v),
                label: (*v).to_owned(),
            })
            .collect()
    }

    #[test]
    fn the_picture_catalogue_gives_68_models() {
        let m = openrouter_image_models(&serde_json::from_str(IMAGES).unwrap(), &BTreeMap::new());
        assert_eq!(m.len(), 68);
        assert!(
            m.iter()
                .all(|m| m.kind == MediaKind::Image && m.usable.is_none())
        );
    }

    #[test]
    fn gpt_image_2_6_flare_has_its_controls() {
        let m = image("openai/gpt-image-2.6-flare");
        assert_eq!(m.display_name, "GPT Image 2.6 Flare");
        assert_eq!(
            values(&m, "quality"),
            ["auto", "low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(param(&m, "quality").layer, Layer::Simple);
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
            param(&m, "input_references").kind,
            ParamKind::Images {
                min: 0,
                max: 16,
                role: RefRole::Reference
            }
        );
        assert_eq!(param(&m, "background").layer, Layer::Advanced);
        assert_eq!(
            param(&m, "output_compression").requires,
            Some(Requires {
                key: "output_format".into(),
                values: vec![json!("jpeg"), json!("webp")]
            })
        );
        assert!(maybe(&m, "frame_count").is_some() && maybe(&m, "seed").is_none());
    }

    #[test]
    fn nano_banana_has_no_count_and_no_seed() {
        let m = image("google/gemini-nano-banana-2.1");
        assert!(maybe(&m, "n").is_none() && maybe(&m, "seed").is_none());
        assert_eq!(values(&m, "resolution"), ["1K", "2K", "4K"]);
        assert!(param(&m, "resolution").affects_price);
    }

    #[test]
    fn k3_edit_needs_a_picture() {
        assert_eq!(
            param(&image("krea/k3-edit"), "input_references").kind,
            ParamKind::Images {
                min: 1,
                max: 8,
                role: RefRole::Reference
            }
        );
    }

    #[test]
    fn no_catalogue_parameter_is_dropped() {
        for (id, entry) in catalogue_entries(IMAGES) {
            let m = image(&id);
            for key in entry["supported_parameters"].as_object().unwrap().keys() {
                let typed = maybe(&m, key).is_some()
                    || (key == "n" && entry["supported_parameters"]["n"]["max"] == 1)
                    || (key == "input_references"
                        && entry["supported_parameters"][key]["max"] == 0);
                let hinted = matches!(maybe(&m, "extra").map(|p| &p.kind),
                    Some(ParamKind::Extra { allowed_keys }) if allowed_keys.contains(key));
                assert!(typed || hinted, "{id}: {key}");
            }
        }
    }

    #[test]
    fn the_param_kinds_serialise_as_the_webview_reads_them() {
        assert_eq!(
            serde_json::to_value(ParamKind::Extra {
                allowed_keys: vec!["moderation".into()]
            })
            .unwrap(),
            json!({"type": "extra", "allowedKeys": ["moderation"]})
        );
        assert_eq!(
            serde_json::to_value(ParamKind::Images {
                min: 0,
                max: 16,
                role: RefRole::Reference
            })
            .unwrap(),
            json!({"type": "images", "min": 0, "max": 16, "role": "reference"})
        );
        assert_eq!(
            serde_json::to_value(ParamKind::Text { max_chars: 4_000 }).unwrap(),
            json!({"type": "text", "maxChars": 4000})
        );
        let size = ParamKind::Size {
            rules: OPENAI_SIZE_RULES,
            values: choices(&["auto"]),
        };
        assert_eq!(
            serde_json::to_value(&size).unwrap(),
            json!({"type": "size", "rules": {"multipleOf": 16, "minRatio": 1.0 / 3.0, "maxRatio": 3.0,
                "maxSide": 3840, "minPixels": 655_360, "maxPixels": 8_294_400},
                "values": [{"value": "auto", "label": "auto"}]})
        );
    }

    #[test]
    fn picture_extra_settings_come_from_the_saved_endpoints_answer() {
        let flare = image_with_endpoints("openai/gpt-image-2.6-flare");
        assert!(
            matches!(&param(&flare, "extra").kind, ParamKind::Extra { allowed_keys } if allowed_keys == &["moderation", "complexity"])
        );
        assert!(maybe(&image("openai/gpt-image-2.6-flare"), "extra").is_none()); // no saved answer yet: nothing to offer
    }

    #[test]
    fn each_endpoints_record_is_a_company() {
        let answers = endpoint_answers();
        assert_eq!(
            endpoint_companies(&answers["google/gemini-nano-banana-2.1"]),
            [
                EndpointCompany {
                    slug: "google-vertex/global".into(),
                    tag: "google-vertex/global".into(),
                    name: "Google Vertex".into(),
                    passthrough: vec![],
                },
                EndpointCompany {
                    slug: "google-ai-studio".into(),
                    tag: "google-ai-studio".into(),
                    name: "Google AI Studio".into(),
                    passthrough: vec!["cachedContent".into()],
                },
            ]
        );
        let banana = image_with_endpoints("google/gemini-nano-banana-2.1");
        assert_eq!(banana.companies.len(), 2);
        assert!(image("google/gemini-nano-banana-2.1").companies.is_empty());
    }

    #[test]
    fn the_video_catalogue_gives_33_models_and_4_need_a_video() {
        let m = openrouter_video_models(&serde_json::from_str(VIDEOS).unwrap());
        assert_eq!(m.len(), 33);
        let mut unusable: Vec<_> = m
            .iter()
            .filter(|m| m.usable.is_some())
            .map(|m| m.id.as_str())
            .collect();
        unusable.sort();
        assert_eq!(
            unusable,
            [
                "black-forest-labs/flux-video-edit",
                "black-forest-labs/flux-video-upscale",
                "heygen/avatar-iv",
                "runway/aleph-2"
            ]
        );
    }

    #[test]
    fn video_defaults_keep_the_first_try_cheap() {
        let veo = video("google/veo-3.1");
        assert_eq!(values(&veo, "duration"), [json!(4), json!(6), json!(8)]);
        assert_eq!(param(&veo, "duration").initial, Some(json!(4)));
        assert_eq!(param(&veo, "resolution").initial, Some(json!("720p")));
        assert_eq!(
            param(&veo, "generate_audio").company_default,
            Some(json!(true))
        );
        assert_eq!(param(&veo, "first_frame").layer, Layer::Simple);
        assert_eq!(param(&veo, "last_frame").layer, Layer::Advanced);
        assert_eq!(
            values(&video("bytedance/seedance-2.5-flash"), "duration")[0],
            json!(4)
        ); // -1 dropped
        assert_eq!(
            values(&video("google/veo-3.1-lite"), "duration"),
            [json!(4), json!(6), json!(8)]
        ); // sorted
        assert_eq!(
            param(&video("minimax/hailuo-3-max"), "resolution").initial,
            Some(json!("480p"))
        ); // no 720p
        assert_eq!(
            param(&video("x-ai/grok-imagine-video-1.5-lite"), "duration").initial,
            Some(json!(4))
        );
        assert!(
            veo.notes
                .iter()
                .any(|n| n == "OpenRouter video is not covered by zero data retention.")
        );
        assert!(
            veo.notes
                .iter()
                .any(|n| n == "Google adds an invisible SynthID watermark.")
        );
    }

    #[test]
    fn video_lengths_read_in_seconds_and_a_size_list_is_advanced() {
        let veo = video("google/veo-3.1");
        let ParamKind::Enum { values } = &param(&veo, "duration").kind else {
            panic!("duration is a list")
        };
        assert_eq!(values[0].label, "4 s");
        assert_eq!(param(&veo, "duration").canonical, Some(Canonical::Length));
        let size = param(&veo, "size");
        assert_eq!(
            (size.canonical, size.layer),
            (Some(Canonical::Size), Layer::Advanced)
        );
        assert!(matches!(&size.kind, ParamKind::Enum { values } if values.len() == 6));
        assert_eq!(param(&veo, "seed").kind, ParamKind::Seed { max: SEED_MAX });
        // A 2 s clip is offered but the first try starts at 4 s; with nothing that long, the longest.
        assert_eq!(
            param(&video("alibaba/wan-2.7"), "duration").initial,
            Some(json!(4))
        );
        assert_eq!(
            param(&video("tbc/mischief-1"), "duration").initial,
            Some(json!(5))
        );
        assert!(maybe(&video("minimax/hailuo-3"), "generate_audio").is_some());
        assert!(maybe(&video("minimax/hailuo-3-max"), "generate_audio").is_none());
        assert!(maybe(&video("tbc/mischief-1"), "last_frame").is_none());
    }

    #[test]
    fn video_extra_settings_are_named_but_not_sent() {
        let veo = video("google/veo-3.1");
        assert!(maybe(&veo, "extra").is_none());
        assert!(
            veo.notes.iter().any(|n| n.contains("negativePrompt")
                && n.contains("Studio cannot send them for video yet"))
        );
    }

    #[test]
    fn this_computer_is_a_company_of_its_own() {
        assert_eq!(
            serde_json::to_value(Company::Local).unwrap(),
            json!("local")
        );
        assert_eq!(
            serde_json::to_value(FailureKind::OutOfMemory).unwrap(),
            json!("outOfMemory")
        );
    }

    #[test]
    fn only_openrouter_and_openai_make_media_in_the_cloud() {
        assert_eq!(
            media_company(ProviderKind::OpenRouter),
            Some(Company::OpenRouter)
        );
        assert_eq!(media_company(ProviderKind::OpenAi), Some(Company::OpenAi));
        for kind in [
            ProviderKind::Anthropic,
            ProviderKind::Abliteration,
            ProviderKind::Generic,
            ProviderKind::LlamaServer,
        ] {
            assert_eq!(media_company(kind), None);
        }
    }

    #[test]
    fn local_models_pass_through_parse_catalog() {
        let m = image("openai/gpt-image-2.6-flare");
        let raw = RawCatalog {
            images: serde_json::to_value(vec![m.clone()]).unwrap(),
            videos: None,
        };
        assert_eq!(
            parse_catalog(Company::Local, &raw, &BTreeMap::new()),
            vec![m]
        );
        assert!(
            parse_catalog(
                Company::Local,
                &RawCatalog {
                    images: json!({"x": 1}),
                    videos: None
                },
                &BTreeMap::new()
            )
            .is_empty()
        );
    }

    #[test]
    fn openrouter_catalogues_give_pictures_then_videos() {
        let raw = RawCatalog {
            images: serde_json::from_str(IMAGES).unwrap(),
            videos: Some(serde_json::from_str(VIDEOS).unwrap()),
        };
        let all = parse_catalog(Company::OpenRouter, &raw, &endpoint_answers());
        assert_eq!(all.len(), 68 + 33);
        assert!(all[..68].iter().all(|m| m.kind == MediaKind::Image));
        assert!(all[68..].iter().all(|m| m.kind == MediaKind::Video));
    }

    #[tokio::test]
    async fn cancel_does_nothing_unless_a_provider_says_otherwise() {
        struct Bare;
        #[async_trait]
        impl MediaProvider for Bare {
            fn company(&self) -> Company {
                unreachable!()
            }
            fn host(&self) -> String {
                unreachable!()
            }
            async fn fetch_catalog(&self) -> Result<RawCatalog, MediaError> {
                unreachable!()
            }
            async fn fetch_image_price(&self, _model: &str) -> Result<Value, MediaError> {
                unreachable!()
            }
            fn preview(&self, _req: &MediaRequest) -> RequestPreview {
                unreachable!()
            }
            async fn start(
                &self,
                _req: &MediaRequest,
                _scratch: &Path,
            ) -> Result<Started, MediaError> {
                unreachable!()
            }
            async fn check(&self, _remote_id: &str) -> Result<JobState, MediaError> {
                unreachable!()
            }
            async fn download(
                &self,
                _remote_id: &str,
                _job: &JobDone,
                _index: usize,
                _dest: &Path,
            ) -> Result<Fetched, MediaError> {
                unreachable!()
            }
        }
        assert!(Bare.cancel("vid_1").await.is_ok());
    }

    #[test]
    fn media_limits_default_to_the_caps() {
        let limits = MediaLimits::default();
        assert_eq!(limits.response_cap, 192 * 1024 * 1024);
        assert_eq!(limits.picture_cap, 50 * 1024 * 1024);
        assert_eq!(limits.video_cap, 1024 * 1024 * 1024);
        assert_eq!(limits.picture_timeout, std::time::Duration::from_secs(300));
        assert_eq!(limits.check_timeout, std::time::Duration::from_secs(30));
        assert_eq!(limits.download_idle, std::time::Duration::from_secs(60));
    }

    #[test]
    fn families_group_versions() {
        assert_eq!(family_of("google/veo-3.1-fast"), "google/veo");
        assert_eq!(family_of("openai/gpt-image-2.6-flare"), "openai/gpt-image");
        assert_eq!(family_of("kwaivgi/kling-v3.0-pro"), "kwaivgi/kling");
        assert_eq!(family_of("gpt-image-2.6-flare"), "gpt-image");
        assert_eq!(family_of("meta/muse-image"), "meta/muse-image");
    }

    #[test]
    fn validate_refuses_a_field_the_model_does_not_list() {
        let req = request("google/gemini-nano-banana-2.1", json!({"seed": 7}));
        assert_eq!(
            validate(&image("google/gemini-nano-banana-2.1"), &req),
            Err(vec![Invalid::NotOffered("seed".into())])
        );
    }

    #[test]
    fn validate_checks_values_requires_extra_and_pictures() {
        let flare = image("openai/gpt-image-2.6-flare");
        assert_eq!(
            validate(&flare, &request_p(json!({"quality": "ultra"}))),
            Err(vec![Invalid::BadValue("quality".into())])
        );
        assert!(matches!(
            validate(&flare, &request_p(json!({"output_compression": 80}))).unwrap_err()[0],
            Invalid::Requires { .. }
        ));
        assert!(
            validate(
                &flare,
                &request_p(json!({"output_format": "webp", "output_compression": 80}))
            )
            .is_ok()
        );
        assert_eq!(
            validate(&image("krea/k3-edit"), &request_p(json!({}))).unwrap_err(),
            vec![Invalid::TooFewPictures {
                role: RefRole::Reference,
                min: 1
            }]
        );
        let flare = image_with_endpoints("openai/gpt-image-2.6-flare");
        assert!(validate(&flare, &request_x(json!({"moderation": "low"}))).is_ok());
        assert_eq!(
            validate(&flare, &request_x(json!({"foo": 1}))),
            Err(vec![Invalid::NotOffered("foo".into())])
        );
        assert_eq!(
            validate(&flare, &request_x(json!({"quality": "low"}))),
            Err(vec![Invalid::Reserved("quality".into())])
        );
        assert_eq!(
            validate(
                &video("google/veo-3.1"),
                &request_x(json!({"negativePrompt": "rain"}))
            ),
            Err(vec![Invalid::NotOffered("negativePrompt".into())])
        );
    }

    #[test]
    fn validate_checks_ranges_seeds_and_picture_fields() {
        let flare = image("openai/gpt-image-2.6-flare");
        for bad in [json!(11), json!(0), json!(1.5), json!("2")] {
            assert_eq!(
                validate(&flare, &request_p(json!({ "n": bad }))),
                Err(vec![Invalid::BadValue("n".into())]),
                "n = {bad}"
            );
        }
        assert!(validate(&flare, &request_p(json!({"n": 4}))).is_ok());
        assert_eq!(
            validate(&flare, &request_p(json!({"input_references": 2}))),
            Err(vec![Invalid::NotOffered("input_references".into())])
        );
        let gif = json!({"output_format": "gif", "frame_count": 8, "frame_duration_ms": 120});
        assert!(validate(&flare, &request_p(gif)).is_ok());
        assert_eq!(
            validate(&flare, &request_p(json!({"frame_count": 8}))),
            Err(vec![Invalid::Requires {
                key: "frame_count".into(),
                needs: "output_format".into()
            }])
        );
        let qwen = image("qwen/qwen-image-3");
        assert!(validate(&qwen, &request_p(json!({"seed": 2_147_483_647u64}))).is_ok());
        for bad in [json!(2_147_483_648u64), json!(-1), json!(7.5)] {
            assert_eq!(
                validate(&qwen, &request_p(json!({ "seed": bad }))),
                Err(vec![Invalid::BadValue("seed".into())]),
                "seed = {bad}"
            );
        }
        assert!(
            validate(
                &video("google/veo-3.1"),
                &request_p(json!({"duration": 6, "generate_audio": false}))
            )
            .is_ok()
        );
        assert_eq!(
            validate(&video("google/veo-3.1"), &request_p(json!({"duration": 5}))),
            Err(vec![Invalid::BadValue("duration".into())])
        );
    }

    #[test]
    fn validate_checks_the_prompt_the_picture_count_and_usability() {
        let flare = image("openai/gpt-image-2.6-flare");
        let blank = MediaRequest {
            prompt: " \n ".into(),
            ..request_p(json!({}))
        };
        assert_eq!(validate(&flare, &blank), Err(vec![Invalid::EmptyPrompt]));
        let long = MediaRequest {
            prompt: "a".repeat(32_001),
            ..request_p(json!({}))
        };
        assert_eq!(validate(&flare, &long), Err(vec![Invalid::PromptTooLong]));
        let wide = MediaRequest {
            prompt: "é".repeat(32_000),
            ..request_p(json!({}))
        };
        assert!(validate(&flare, &wide).is_ok()); // characters, not bytes
        assert_eq!(
            validate(
                &image("krea/k3-edit"),
                &with_refs(request_p(json!({})), RefRole::Reference, 9)
            ),
            Err(vec![Invalid::TooManyPictures {
                role: RefRole::Reference,
                max: 8
            }])
        );
        assert_eq!(
            validate(
                &flare,
                &with_refs(request_p(json!({})), RefRole::StartFrame, 1)
            ),
            Err(vec![Invalid::TooManyPictures {
                role: RefRole::StartFrame,
                max: 0
            }])
        );
        let veo = video("google/veo-3.1");
        assert!(
            validate(
                &veo,
                &with_refs(request_p(json!({})), RefRole::StartFrame, 1)
            )
            .is_ok()
        );
        assert_eq!(
            validate(&video("runway/aleph-2"), &request_p(json!({}))),
            Err(vec![Invalid::NotUsable(
                "Needs a video to start from, which Studio does not offer yet.".into()
            )])
        );
        assert_eq!(
            validate(&flare, &request_x(json!({"model": "x", "prompt": "y"}))),
            Err(vec![
                Invalid::Reserved("model".into()),
                Invalid::Reserved("prompt".into())
            ])
        );
    }

    #[test]
    fn a_company_setting_needs_a_company_the_routing_allows() {
        let banana = image_with_endpoints("google/gemini-nano-banana-2.1");
        let req = request_x(json!({"cachedContent": "c1"}));
        assert!(validate(&banana, &req).is_ok());
        let routed = |tag: &str, only: bool, allow_fallbacks: bool| MediaRequest {
            routing: Some(Routing {
                order: vec![tag.to_owned()],
                only,
                allow_fallbacks,
                zdr: false,
            }),
            ..req.clone()
        };
        let refused = Err(vec![Invalid::NotOffered("cachedContent".into())]);
        assert!(validate(&banana, &routed("google-ai-studio", true, true)).is_ok());
        assert_eq!(
            validate(&banana, &routed("google-vertex/global", true, true)),
            refused
        );
        assert_eq!(
            validate(&banana, &routed("google-vertex/global", false, false)),
            refused
        );
        // With fallbacks allowed, the run may still reach AI Studio.
        assert!(validate(&banana, &routed("google-vertex/global", false, true)).is_ok());
    }

    #[test]
    fn validate_takes_a_listed_size_or_one_within_the_rules() {
        let m = hand_made(vec![field(
            "size",
            ParamKind::Size {
                rules: OPENAI_SIZE_RULES,
                values: choices(&["auto"]),
            },
        )]);
        for ok in ["auto", "1024x1024", "1536x1024", "3840x2160"] {
            assert!(
                validate(&m, &request_p(json!({ "size": ok }))).is_ok(),
                "{ok}"
            );
        }
        for bad in [json!("1000x1000"), json!("big"), json!(1024)] {
            assert_eq!(
                validate(&m, &request_p(json!({ "size": bad }))),
                Err(vec![Invalid::BadValue("size".into())]),
                "size = {bad}"
            );
        }
    }

    #[test]
    fn an_absent_field_counts_as_its_company_default() {
        let mut format = field(
            "output_format",
            ParamKind::Enum {
                values: choices(&["png", "svg"]),
            },
        );
        format.company_default = Some(json!("png"));
        let not_svg = Some(Requires {
            key: "output_format".into(),
            values: vec![json!("png")],
        });
        let mut n = field(
            "n",
            ParamKind::Range {
                min: 1.0,
                max: 4.0,
                step: Some(1.0),
                unit: None,
            },
        );
        n.requires = not_svg.clone();
        let mut refs = field(
            "image[]",
            ParamKind::Images {
                min: 0,
                max: 4,
                role: RefRole::Reference,
            },
        );
        refs.requires = not_svg;
        let m = hand_made(vec![format, n, refs]);
        assert!(validate(&m, &request_p(json!({"n": 2}))).is_ok());
        assert!(validate(&m, &with_refs(request_p(json!({})), RefRole::Reference, 1)).is_ok());
        assert_eq!(
            validate(&m, &request_p(json!({"n": 2, "output_format": "svg"}))),
            Err(vec![Invalid::Requires {
                key: "n".into(),
                needs: "output_format".into()
            }])
        );
        assert_eq!(
            validate(
                &m,
                &with_refs(
                    request_p(json!({"output_format": "svg"})),
                    RefRole::Reference,
                    1
                )
            ),
            Err(vec![Invalid::Requires {
                key: "image[]".into(),
                needs: "output_format".into()
            }])
        );
    }

    #[test]
    fn a_text_field_is_checked_by_its_length() {
        let m = hand_made(vec![field(
            "negative_prompt",
            ParamKind::Text { max_chars: 5 },
        )]);
        assert!(validate(&m, &request_p(json!({"negative_prompt": "rainé"}))).is_ok());
        for bad in [json!("rain, fog"), json!(5)] {
            assert_eq!(
                validate(&m, &request_p(json!({ "negative_prompt": bad }))),
                Err(vec![Invalid::BadValue("negative_prompt".into())]),
                "{bad}"
            );
        }
    }
}
