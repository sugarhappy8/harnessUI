//! What a picture or video model costs, as its company states it, and what a run would cost.
//!
//! - **OpenRouter video** ([`parse_pricing_skus`]): the catalogue's `pricing_skus`, keys in
//!   dollars or in cents (`cents_…`), per second of video. Only the key patterns seen on
//!   2026-11-17 are read; any other key makes the price [`Price::Unknown`], never a guess.
//! - **OpenRouter pictures** ([`parse_image_endpoints`]): the saved `/endpoints` answer, a
//!   price per picture for each company that runs the model.
//! - **OpenAI pictures**: the bundled settings file's prices per picture, where OpenAI states
//!   them (built in [`super::openai`]; an edit has no estimate, as OpenAI states no price for
//!   added pictures), and what a finished run cost from the answer's token counts
//!   ([`openai_cost`]).
//! - **An estimate** ([`estimate`]) for the settings of a request ([`choice_for`]): `None`
//!   when the price is not known; a run on this computer costs nothing.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::spec::routing_allows;
use super::{MediaKind, MediaModel, MediaRequest, RefRole, Routing};

/// How a model is priced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Price {
    /// A price per picture made, from a table of rows.
    PerImage(Table),
    /// A price per second of video, from a table of rows.
    PerSecond(Table),
    /// Priced per token: no estimate before the run.
    PerToken,
    /// The price is not known (not fetched yet, or in a form Studio cannot read).
    Unknown,
    /// A run on this computer: nothing is charged.
    Free,
}

/// The rows of a picture or video price.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    /// The prices, each for the settings it names.
    pub rows: Vec<PriceRow>,
    /// What each added picture costs, in US dollars, when the company charges for them.
    pub per_input_image_usd: Option<f64>,
    /// Whether added pictures are charged at a price the company does not state (OpenAI's
    /// edits): a run that sends any has no estimate.
    #[serde(default)]
    pub input_images_unpriced: bool,
    /// The least a run costs, in US dollars, when the company states one.
    pub minimum_usd: Option<f64>,
}

/// One price, for the settings it names. A setting that is `None` applies to every value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceRow {
    /// Text to video, or a video from a start frame.
    pub mode: Option<Mode>,
    /// The resolution, as the company names it (`720p`, `2K`).
    pub resolution: Option<String>,
    /// The quality, as the company names it (`low`, `high`).
    pub quality: Option<String>,
    /// With sound (`true`) or without (`false`).
    pub audio: Option<bool>,
    /// The price in US dollars, per picture or per second.
    pub usd: f64,
}

/// How a video is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    /// From the prompt alone.
    TextToVideo,
    /// From a start frame and the prompt.
    ImageToVideo,
}

/// The settings of a run that its price depends on ([`choice_for`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PriceChoice {
    /// How many pictures (always 1 for a video).
    pub count: u32,
    /// A video's length in seconds.
    pub seconds: Option<f64>,
    /// The resolution, as the company names it (`720p`, `1K`), or a picture's exact size
    /// (`1024x1024`).
    pub resolution: Option<String>,
    /// The quality, as the company names it.
    pub quality: Option<String>,
    /// With sound or without, for a model that makes sound.
    pub audio: Option<bool>,
    /// Whether a start frame is sent.
    pub start_frame: bool,
    /// How many pictures are sent with the request.
    pub input_images: u32,
}

/// An OpenAI picture model's token prices, in US dollars per million tokens, as its entry in
/// the bundled settings file states them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenAiRates {
    /// Text input (the prompt); `None` where OpenAI states no price for it.
    pub text_input: Option<f64>,
    /// Picture input (added pictures).
    pub image_input: f64,
    /// Picture output.
    pub image_output: f64,
}

impl Price {
    /// The lowest and highest price of one picture or one second, for the picker's price
    /// hint: `(0, 0)` for a run on this computer; `None` when there is no number.
    pub fn range(&self) -> Option<(f64, f64)> {
        match self {
            Self::PerImage(table) | Self::PerSecond(table) => bounds(&table.rows),
            Self::Free => Some((0.0, 0.0)),
            Self::PerToken | Self::Unknown => None,
        }
    }
}

/// Parts of a `pricing_skus` key for features Studio does not offer in 0.1.0: such keys are
/// left out.
const IGNORED_SKUS: &[&str] = &["video_continuation", "with_video_input"];

/// One kind of per-second `pricing_skus` key: `<prefix>` alone (when `plain`) or
/// `<prefix>_<resolution>`, for the mode and sound it names.
struct SkuFamily {
    prefix: &'static str,
    plain: bool,
    mode: Option<Mode>,
    audio: Option<bool>,
}

/// The per-second key families, in the order of the lookup. When two keys name the same
/// settings (`duration_seconds_720p` and `cents_per_second_output_720p`), the earlier
/// family's price is kept, and a plain `duration_seconds` leaves out every later family:
/// the lookup stops at it before any cents key.
const SKU_FAMILIES: &[SkuFamily] = &[
    SkuFamily {
        prefix: "duration_seconds_with_audio",
        plain: true,
        mode: None,
        audio: Some(true),
    },
    SkuFamily {
        prefix: "duration_seconds_without_audio",
        plain: true,
        mode: None,
        audio: Some(false),
    },
    SkuFamily {
        prefix: "text_to_video_duration_seconds",
        plain: false,
        mode: Some(Mode::TextToVideo),
        audio: None,
    },
    SkuFamily {
        prefix: "image_to_video_duration_seconds",
        plain: false,
        mode: Some(Mode::ImageToVideo),
        audio: None,
    },
    SkuFamily {
        prefix: "duration_seconds",
        plain: true,
        mode: None,
        audio: None,
    },
    SkuFamily {
        prefix: "cents_per_video_output_second",
        plain: false,
        mode: None,
        audio: None,
    },
    SkuFamily {
        prefix: "cents_per_second_output",
        plain: true,
        mode: None,
        audio: None,
    },
];

/// A video's price from OpenRouter's `pricing_skus`, per second of video.
///
/// Keys in cents start with `cents_` (or `minimum_cents_`); the others are in dollars. Read:
/// `duration_seconds[_with_audio|_without_audio][_<res>]`,
/// `text_to_video_duration_seconds_<res>`, `image_to_video_duration_seconds_<res>`,
/// `cents_per_video_output_second_<res>`, `cents_per_second_output[_<res>]`,
/// `cents_per_image_input` (per added picture) and `minimum_cents_per_generation` (the least
/// a run costs). Keys containing `video_continuation` or `with_video_input` are left out
/// (features not in 0.1.0); any `video_tokens…` key makes the price [`Price::PerToken`]; any
/// other key, or a value that is not a price, makes the whole price [`Price::Unknown`].
pub fn parse_pricing_skus(skus: &Map<String, Value>) -> Price {
    let mut rows: Vec<(usize, PriceRow)> = Vec::new();
    let mut per_input_image_usd = None;
    let mut minimum_usd = None;
    let mut per_token = false;
    for (key, value) in skus {
        if IGNORED_SKUS.iter().any(|part| key.contains(part)) {
            continue;
        }
        if key.starts_with("video_tokens") {
            per_token = true;
            continue;
        }
        let Some(amount) = amount(value) else {
            return Price::Unknown;
        };
        let in_cents = key.starts_with("cents_") || key.starts_with("minimum_cents_");
        let usd = if in_cents { amount / 100.0 } else { amount };
        match key.as_str() {
            "cents_per_image_input" => per_input_image_usd = Some(usd),
            "minimum_cents_per_generation" => minimum_usd = Some(usd),
            _ => match per_second_row(key, usd) {
                Some(ranked) => rows.push(ranked),
                None => return Price::Unknown,
            },
        }
    }
    if per_token {
        return Price::PerToken;
    }
    rows.sort_by_key(|(rank, _)| *rank);
    // The lookup stops at a row that fits every choice (plain `duration_seconds`), so the
    // rows of later families (the cents keys) are never reached; `second_price` tries every
    // resolution row before a plain one, so they are left out here.
    let last_rank = rows
        .iter()
        .find(|(_, row)| row.mode.is_none() && row.audio.is_none() && row.resolution.is_none())
        .map(|(rank, _)| *rank);
    let mut kept: Vec<PriceRow> = Vec::new();
    for (rank, row) in rows {
        if last_rank.is_some_and(|last| rank > last) {
            continue;
        }
        let same = |other: &PriceRow| {
            other.mode == row.mode
                && other.audio == row.audio
                && same_resolution(other.resolution.as_deref(), row.resolution.as_deref())
        };
        if !kept.iter().any(same) {
            kept.push(row);
        }
    }
    if kept.is_empty() {
        return Price::Unknown;
    }
    Price::PerSecond(Table {
        rows: kept,
        per_input_image_usd,
        input_images_unpriced: false,
        minimum_usd,
    })
}

/// A picture model's price from one saved `/endpoints` answer. With several companies, each
/// priced item is the dearest of theirs (below), unless `routing` pins one (`only`, or
/// `allow_fallbacks == false`): then that company's price.
///
/// Per company: `output_image` rows with unit `image` are prices per picture, keyed by their
/// `variant` (none: every resolution), matched without regard to case to the company's
/// resolution values; when a variant matches none of them, every choice gets the company's
/// dearest `output_image` row, so the estimate is never too low. `input_image` and
/// `input_reference` rows with unit `image` are the price of an added picture. A row with
/// unit `token` makes the company [`Price::PerToken`]; any other row makes it
/// [`Price::Unknown`].
///
/// Among the companies the routing allows, an unknown price wins, then a price per token.
/// Otherwise every resolution any of them names, and any other resolution, costs the dearest
/// of their prices for it, and an added picture the dearest of their prices for one: a run the
/// routing sends to any of these companies never costs more than its estimate. No company
/// allowed: unknown.
pub fn parse_image_endpoints(answer: &Value, routing: Option<&Routing>) -> Price {
    let prices = answer
        .get("endpoints")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|record| routing_allows(routing, provider_tag(record)))
        .map(company_price);
    let mut tables = Vec::new();
    let mut per_token = false;
    for price in prices {
        match price {
            Price::PerImage(table) => tables.push(table),
            Price::PerToken => per_token = true,
            _ => return Price::Unknown,
        }
    }
    if per_token {
        return Price::PerToken;
    }
    dearest_of(tables).map_or(Price::Unknown, Price::PerImage)
}

/// One table for several companies' picture prices: a row for each resolution any of them
/// names (and the plain row, for any other resolution) at the dearest price among the
/// companies that price it, and the dearest added picture. One company's table is kept as it
/// is; none gives `None`.
fn dearest_of(tables: Vec<Table>) -> Option<Table> {
    if tables.len() <= 1 {
        return tables.into_iter().next();
    }
    let mut resolutions: Vec<Option<String>> = Vec::new();
    for row in tables.iter().flat_map(|table| &table.rows) {
        if !resolutions
            .iter()
            .any(|seen| same_resolution(seen.as_deref(), row.resolution.as_deref()))
        {
            resolutions.push(row.resolution.clone());
        }
    }
    let rows = resolutions
        .into_iter()
        .filter_map(|resolution| {
            let choice = PriceChoice {
                count: 1,
                seconds: None,
                resolution: resolution.clone(),
                quality: None,
                audio: None,
                start_frame: false,
                input_images: 0,
            };
            let usd = tables
                .iter()
                .filter_map(|table| picture_price(table, &choice))
                .reduce(f64::max)?;
            Some(PriceRow {
                mode: None,
                resolution,
                quality: None,
                audio: None,
                usd,
            })
        })
        .collect();
    Some(Table {
        rows,
        per_input_image_usd: tables
            .iter()
            .filter_map(|table| table.per_input_image_usd)
            .reduce(f64::max),
        input_images_unpriced: tables.iter().any(|table| table.input_images_unpriced),
        minimum_usd: tables
            .iter()
            .filter_map(|table| table.minimum_usd)
            .reduce(f64::max),
    })
}

/// What a run with these settings would cost, in US dollars: `None` when the price is not
/// known (priced per token, unknown, or no row for these settings). A run on this computer
/// costs nothing.
///
/// A picture costs its row's price times the count; a video, its row's price per second
/// times the length. Each added picture adds its price, and the company's minimum is the
/// least a run costs. A run that sends pictures charged at a price the company does not
/// state has no estimate.
pub fn estimate(price: &Price, choice: &PriceChoice) -> Option<f64> {
    let (table, each) = match price {
        Price::Free => return Some(0.0),
        Price::PerToken | Price::Unknown => return None,
        Price::PerImage(table) => (table, picture_price(table, choice)?),
        Price::PerSecond(table) => (table, second_price(table, choice)? * choice.seconds?),
    };
    if table.input_images_unpriced && choice.input_images > 0 {
        return None;
    }
    let pictures = table.per_input_image_usd.unwrap_or(0.0) * f64::from(choice.input_images);
    let total = each * f64::from(choice.count) + pictures;
    Some(
        table
            .minimum_usd
            .map_or(total, |minimum| total.max(minimum)),
    )
}

/// What a finished OpenAI picture run cost, in US dollars, from its answer's `usage` and the
/// model's token prices: text input tokens at the text price, picture input tokens at the
/// picture input price, and every output token at the picture output price (the dearest).
/// Input tokens the answer does not name as text or picture are counted at the dearer input
/// price, so the cost is never too low.
///
/// `None` when the usage has no whole `input_tokens` and `output_tokens`, or when text may
/// have been sent and the model has no text price.
pub fn openai_cost(usage: &Value, rates: &OpenAiRates) -> Option<f64> {
    let tokens = |value: &Value, key: &str| value.get(key).and_then(Value::as_u64);
    let input = tokens(usage, "input_tokens")?;
    let output = tokens(usage, "output_tokens")?;
    let details = usage.get("input_tokens_details");
    let named = |key: &str| details.and_then(|d| tokens(d, key)).unwrap_or(0);
    let image = named("image_tokens").min(input);
    let text = named("text_tokens").min(input - image);
    let unnamed = input - image - text;
    let text_cost = match (text + unnamed, rates.text_input) {
        (0, _) => 0.0,
        (_, None) => return None,
        (_, Some(price)) => text as f64 * price + unnamed as f64 * price.max(rates.image_input),
    };
    let total = text_cost + image as f64 * rates.image_input + output as f64 * rates.image_output;
    Some(total / 1e6)
}

/// The settings of `req` that its price depends on. A setting that is not sent counts as
/// its company default.
///
/// The count is `n` (1 when absent; always 1 for a video); the length is `duration`; the
/// resolution is `resolution`, else a picture's `size` as sent (`1024x1024`); a video's
/// `size`, sent in place of its resolution, gives its shorter side (`1280x720` → `720p`,
/// `3840x2160` → `4K`). What the request sends comes first: the company defaults of
/// `resolution` and `size` are read, in the same order, only when it sends neither. The
/// quality is `quality`, the sound `generate_audio`; a start frame is noted, and every added
/// picture counts.
pub fn choice_for(model: &MediaModel, req: &MediaRequest) -> PriceChoice {
    let setting = |key: &str| {
        req.params.get(key).or_else(|| {
            model
                .params
                .iter()
                .find(|p| p.key == key)?
                .company_default
                .as_ref()
        })
    };
    let text = |key: &str| setting(key).and_then(Value::as_str).map(str::to_owned);
    let sent = |key: &str| {
        req.params
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_owned)
    };
    let video = model.kind == MediaKind::Video;
    let count = if video {
        1
    } else {
        setting("n")
            .and_then(Value::as_f64)
            .filter(|n| *n >= 1.0)
            // A count is a small whole number; `as` saturates anything else.
            .map_or(1, |n| n.round() as u32)
    };
    let resolution_from = |read: &dyn Fn(&str) -> Option<String>| {
        if video {
            read("size")
                .as_deref()
                .and_then(shorter_side)
                .or_else(|| read("resolution"))
        } else {
            read("resolution").or_else(|| read("size"))
        }
    };
    let resolution = resolution_from(&sent).or_else(|| resolution_from(&text));
    PriceChoice {
        count,
        seconds: setting("duration")
            .and_then(Value::as_f64)
            .filter(|s| *s > 0.0),
        resolution,
        quality: text("quality"),
        audio: setting("generate_audio").and_then(Value::as_bool),
        start_frame: req.refs.iter().any(|r| r.role == RefRole::StartFrame),
        input_images: u32::try_from(req.refs.len()).unwrap_or(u32::MAX),
    }
}

// ---- reading prices --------------------------------------------------------------------------

/// A price in a catalogue: a number, or a number written as text. `None` for anything else,
/// or a price below zero.
fn amount(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(n) => n.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        _ => None,
    }?;
    (number.is_finite() && number >= 0.0).then_some(number)
}

/// The row of a per-second key, with its family's place in the lookup order; `None` for a
/// key of no family.
fn per_second_row(key: &str, usd: f64) -> Option<(usize, PriceRow)> {
    SKU_FAMILIES.iter().enumerate().find_map(|(rank, family)| {
        let rest = key.strip_prefix(family.prefix)?;
        let resolution = if rest.is_empty() && family.plain {
            None
        } else {
            Some(
                rest.strip_prefix('_')
                    .filter(|r| is_resolution(r))?
                    .to_owned(),
            )
        };
        Some((
            rank,
            PriceRow {
                mode: family.mode,
                resolution,
                quality: None,
                audio: family.audio,
                usd,
            },
        ))
    })
}

/// Whether a key's ending names a resolution: a number and `p` or `k` (`720p`, `4k`, `1.5k`).
fn is_resolution(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower
        .strip_suffix('p')
        .or_else(|| lower.strip_suffix('k'))
        .is_some_and(|number| {
            !number.is_empty()
                && number.bytes().all(|b| b.is_ascii_digit() || b == b'.')
                && number.parse::<f64>().is_ok()
        })
}

/// The `provider_tag` of an `/endpoints` record (its `provider_slug` when it has none), as a
/// request's routing names it.
fn provider_tag(record: &Value) -> &str {
    ["provider_tag", "provider_slug"]
        .iter()
        .find_map(|key| record.get(*key).and_then(Value::as_str))
        .unwrap_or_default()
}

/// The lowest and highest price of `rows`; `None` when there are none.
fn bounds(rows: &[PriceRow]) -> Option<(f64, f64)> {
    rows.iter().fold(None, |range, row| {
        Some(match range {
            None => (row.usd, row.usd),
            Some((low, high)) => (f64::min(low, row.usd), f64::max(high, row.usd)),
        })
    })
}

/// One company's price, from its `/endpoints` record.
fn company_price(record: &Value) -> Price {
    let mut rows = Vec::new();
    let mut per_input_image_usd: Option<f64> = None;
    let mut per_token = false;
    let pricing = record.get("pricing").and_then(Value::as_array);
    for item in pricing.into_iter().flatten() {
        let text = |key: &str| item.get(key).and_then(Value::as_str);
        match text("unit") {
            Some("token") => {
                per_token = true;
                continue;
            }
            Some("image") => {}
            _ => return Price::Unknown,
        }
        let Some(usd) = item.get("cost_usd").and_then(amount) else {
            return Price::Unknown;
        };
        match text("billable") {
            Some("output_image") => rows.push(PriceRow {
                mode: None,
                resolution: text("variant").map(str::to_owned),
                quality: None,
                audio: None,
                usd,
            }),
            Some("input_image" | "input_reference") => {
                per_input_image_usd = Some(per_input_image_usd.map_or(usd, |p| p.max(usd)));
            }
            _ => return Price::Unknown,
        }
    }
    if per_token {
        return Price::PerToken;
    }
    let offered: Vec<&str> = record
        .pointer("/supported_parameters/resolution/values")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let unmatched = rows.iter().any(|row| {
        row.resolution
            .as_deref()
            .is_some_and(|variant| !offered.iter().any(|o| o.eq_ignore_ascii_case(variant)))
    });
    if unmatched {
        let usd = rows.iter().map(|row| row.usd).fold(0.0, f64::max);
        rows = vec![PriceRow {
            mode: None,
            resolution: None,
            quality: None,
            audio: None,
            usd,
        }];
    }
    if rows.is_empty() {
        return Price::Unknown;
    }
    Price::PerImage(Table {
        rows,
        per_input_image_usd,
        input_images_unpriced: false,
        minimum_usd: None,
    })
}

/// A video's shorter side as a resolution name: `1280x720` → `720p`; 2160 lines are `4K`,
/// as every catalogue names 3840 × 2160.
fn shorter_side(size: &str) -> Option<String> {
    let (width, height) = size.split_once('x')?;
    let lines = width.parse::<u32>().ok()?.min(height.parse().ok()?);
    match lines {
        0 => None,
        2160 => Some("4K".to_owned()),
        n => Some(format!("{n}p")),
    }
}

// ---- estimating ------------------------------------------------------------------------------

/// Whether a row's resolution is the choice's, without regard to case; `None` only matches
/// `None`.
fn same_resolution(row: Option<&str>, choice: Option<&str>) -> bool {
    match (row, choice) {
        (None, None) => true,
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

/// The price of one picture: the row that names the most of the choice's settings (and
/// nothing else), the dearest of equals.
fn picture_price(table: &Table, choice: &PriceChoice) -> Option<f64> {
    let mode = video_mode(choice);
    let fits = |setting: Option<&str>, chosen: Option<&str>| {
        setting.is_none_or(|s| chosen.is_some_and(|c| s.eq_ignore_ascii_case(c)))
    };
    table
        .rows
        .iter()
        .filter(|row| {
            fits(row.resolution.as_deref(), choice.resolution.as_deref())
                && fits(row.quality.as_deref(), choice.quality.as_deref())
                && row.mode.is_none_or(|m| m == mode)
                && row.audio.is_none_or(|a| choice.audio == Some(a))
        })
        .max_by(|a, b| {
            specificity(a)
                .cmp(&specificity(b))
                .then(a.usd.total_cmp(&b.usd))
        })
        .map(|row| row.usd)
}

/// How many settings a row names.
fn specificity(row: &PriceRow) -> usize {
    usize::from(row.mode.is_some())
        + usize::from(row.resolution.is_some())
        + usize::from(row.quality.is_some())
        + usize::from(row.audio.is_some())
}

/// The price of one second of video, tried in this order: with or without sound at the
/// resolution, then with or without sound; then, whatever the sound, the mode at the
/// resolution, the resolution, and the plain price. `None` when no row fits. (A table from
/// [`parse_pricing_skus`] has no cents row beside a plain `duration_seconds`, so this follows
/// the key order of [`SKU_FAMILIES`].)
fn second_price(table: &Table, choice: &PriceChoice) -> Option<f64> {
    let resolution = choice.resolution.as_deref().filter(|r| !r.is_empty());
    let mut tries: Vec<(Option<Mode>, Option<&str>, Option<bool>)> = Vec::new();
    if let Some(sound) = choice.audio {
        tries.push((None, resolution, Some(sound)));
        tries.push((None, None, Some(sound)));
    }
    if resolution.is_some() {
        tries.push((Some(video_mode(choice)), resolution, None));
        tries.push((None, resolution, None));
    }
    tries.push((None, None, None));
    tries.into_iter().find_map(|(mode, resolution, audio)| {
        table
            .rows
            .iter()
            .find(|row| {
                row.mode == mode
                    && row.audio == audio
                    && row.quality.is_none()
                    && same_resolution(row.resolution.as_deref(), resolution)
            })
            .map(|row| row.usd)
    })
}

/// Text to video, or image to video when a start frame is sent.
fn video_mode(choice: &PriceChoice) -> Mode {
    if choice.start_frame {
        Mode::ImageToVideo
    } else {
        Mode::TextToVideo
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use serde_json::{Map, Value, json};

    use super::*;
    use crate::media::{
        Company, MediaModel, MediaRequest, RawCatalog, RefImage, RefRole, Routing, parse_catalog,
    };

    const IMAGES: &str =
        include_str!("../../tests/fixtures/media/openrouter-images-2026-11-17.json");
    const VIDEOS: &str =
        include_str!("../../tests/fixtures/media/openrouter-videos-2026-11-17.json");
    const ENDPOINTS: &str =
        include_str!("../../tests/fixtures/media/openrouter-image-endpoints-2026-11-17.json");

    /// Every video in the recorded catalogue, with its `pricing_skus`.
    fn catalogue_videos() -> impl Iterator<Item = (String, Map<String, Value>)> {
        let list: Value = serde_json::from_str(VIDEOS).unwrap();
        list["data"]
            .as_array()
            .unwrap()
            .clone()
            .into_iter()
            .map(|entry| {
                let id = entry["id"].as_str().unwrap().to_owned();
                let skus = entry["pricing_skus"]
                    .as_object()
                    .cloned()
                    .unwrap_or_default();
                (id, skus)
            })
    }

    fn sku(id: &str) -> Price {
        let (_, skus) = catalogue_videos()
            .find(|(video, _)| video == id)
            .unwrap_or_else(|| panic!("no video {id}"));
        parse_pricing_skus(&skus)
    }

    fn secs(
        price: &Price,
        seconds: f64,
        resolution: &str,
        audio: Option<bool>,
        start_frame: bool,
        input_images: u32,
    ) -> Option<f64> {
        estimate(
            price,
            &PriceChoice {
                count: 1,
                seconds: Some(seconds),
                resolution: (!resolution.is_empty()).then(|| resolution.to_owned()),
                quality: None,
                audio,
                start_frame,
                input_images,
            },
        )
    }

    fn usd(a: Option<f64>, b: f64) {
        assert!((a.unwrap() - b).abs() < 1e-9, "{a:?} != {b}");
    }

    fn answers() -> Value {
        let saved: Value = serde_json::from_str(ENDPOINTS).unwrap();
        saved["answers"].clone()
    }

    fn endpoints_price(id: &str, routing: Option<&Routing>) -> Price {
        parse_image_endpoints(&answers()[id], routing)
    }

    fn pics(count: u32, resolution: &str, input_images: u32) -> PriceChoice {
        PriceChoice {
            count,
            seconds: None,
            resolution: Some(resolution.to_owned()),
            quality: None,
            audio: None,
            start_frame: false,
            input_images,
        }
    }

    /// A synthetic `/endpoints` answer in the recorded shape: two companies, one plain
    /// per-picture price each.
    fn two_companies(a: (&str, f64), b: (&str, f64)) -> Value {
        let record = |(tag, cost): (&str, f64)| {
            json!({
                "provider_name": tag.to_uppercase(),
                "provider_slug": tag,
                "provider_tag": tag,
                "supported_parameters": {"resolution": {"type": "enum", "values": ["1K", "2K"]}},
                "allowed_passthrough_parameters": [],
                "pricing": [{"billable": "output_image", "unit": "image", "cost_usd": cost}]
            })
        };
        json!({"id": "test/two-companies", "endpoints": [record(a), record(b)]})
    }

    fn only(tag: &str) -> Routing {
        Routing {
            order: vec![tag.to_owned()],
            only: true,
            allow_fallbacks: true,
            zdr: false,
        }
    }

    fn order(tag: &str, allow_fallbacks: bool) -> Routing {
        Routing {
            order: vec![tag.to_owned()],
            only: false,
            allow_fallbacks,
            zdr: false,
        }
    }

    fn row(resolution: Option<&str>, usd: f64) -> PriceRow {
        PriceRow {
            mode: None,
            resolution: resolution.map(str::to_owned),
            quality: None,
            audio: None,
            usd,
        }
    }

    fn openrouter_models(endpoints: &BTreeMap<String, Value>) -> Vec<MediaModel> {
        let raw = RawCatalog {
            images: serde_json::from_str(IMAGES).unwrap(),
            videos: Some(serde_json::from_str(VIDEOS).unwrap()),
        };
        parse_catalog(Company::OpenRouter, &raw, endpoints)
    }

    fn model(models: &[MediaModel], id: &str) -> MediaModel {
        models
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .unwrap_or_else(|| panic!("no model {id}"))
    }

    fn request(model: &MediaModel, params: Value) -> MediaRequest {
        MediaRequest {
            kind: model.kind,
            model: model.id.clone(),
            prompt: "a lighthouse at dusk".to_owned(),
            params: params.as_object().cloned().unwrap(),
            extra: Map::new(),
            refs: Vec::new(),
            routing: None,
            provider_options: BTreeMap::new(),
        }
    }

    fn with_refs(req: MediaRequest, roles: &[RefRole]) -> MediaRequest {
        let refs = roles
            .iter()
            .enumerate()
            .map(|(i, role)| RefImage {
                role: *role,
                mime: "image/png".to_owned(),
                path: PathBuf::from(format!("/gallery/refs/{i}.png")),
                sha256: format!("{i:064x}"),
                size_bytes: 1_200,
            })
            .collect();
        MediaRequest { refs, ..req }
    }

    #[test]
    fn veo_3_1_prices() {
        let p = sku("google/veo-3.1");
        usd(secs(&p, 8.0, "720p", Some(true), false, 0), 3.20);
        usd(secs(&p, 8.0, "4K", Some(true), false, 0), 4.80);
        usd(secs(&p, 8.0, "1080p", Some(false), false, 0), 1.60);
    }

    #[test]
    fn the_resolution_row_comes_before_the_plain_one() {
        let p = sku("google/veo-3.1-fast");
        usd(secs(&p, 8.0, "720p", Some(true), false, 0), 0.80);
        usd(secs(&p, 8.0, "1080p", Some(true), false, 0), 0.96);
    }

    /// The price of a synthetic `pricing_skus` object, for key mixes no recorded model has.
    fn skus(object: Value) -> Price {
        parse_pricing_skus(object.as_object().unwrap())
    }

    #[test]
    fn cents_keys_come_after_every_dollar_key() {
        // Plain `duration_seconds` comes before every cents key, even one for the resolution.
        let p = skus(json!({
            "duration_seconds": 0.10,
            "cents_per_video_output_second_720p": 50,
            "cents_per_second_output_1080p": 60,
            "cents_per_second_output": 70,
        }));
        usd(secs(&p, 5.0, "720p", Some(true), false, 0), 0.50);
        usd(secs(&p, 5.0, "1080p", None, false, 0), 0.50);
        usd(secs(&p, 5.0, "", None, false, 0), 0.50);
        assert_eq!(p.range(), Some((0.10, 0.10)));
        // Without it: a dollar row for the resolution, then a cents row for the resolution,
        // then the plain cents price.
        let p = skus(json!({
            "duration_seconds_1080p": 0.20,
            "cents_per_video_output_second_720p": 30,
            "cents_per_second_output_720p": 40,
            "cents_per_second_output_1080p": 90,
            "cents_per_second_output": 10,
        }));
        usd(secs(&p, 5.0, "720p", None, false, 0), 1.50);
        usd(secs(&p, 5.0, "1080p", None, false, 0), 1.00);
        usd(secs(&p, 5.0, "480p", None, false, 0), 0.50);
    }

    #[test]
    fn cents_are_cents() {
        usd(
            secs(
                &sku("x-ai/grok-imagine-video-1.5-lite"),
                4.0,
                "480p",
                Some(true),
                true,
                1,
            ),
            0.09,
        );
        usd(
            secs(&sku("tbc/mischief-1"), 5.0, "720p", None, false, 0),
            0.05,
        );
    }

    #[test]
    fn mode_keys_follow_the_start_frame() {
        let p = sku("alibaba/wan-2.6");
        usd(secs(&p, 5.0, "720p", Some(true), false, 0), 0.40);
        usd(secs(&p, 5.0, "720p", Some(true), true, 0), 0.50);
        assert_eq!(secs(&p, 5.0, "480p", Some(true), true, 0), None);
    }

    #[test]
    fn sound_keys() {
        let k = sku("kwaivgi/kling-v3.0-pro");
        usd(secs(&k, 5.0, "720p", Some(true), false, 0), 0.84);
        usd(secs(&k, 5.0, "720p", Some(false), false, 0), 0.56);
        assert_eq!(
            secs(
                &sku("google/gemini-omni-1.2-flash"),
                8.0,
                "720p",
                Some(false),
                false,
                0
            ),
            None
        );
    }

    #[test]
    fn a_minimum_per_run_applies() {
        usd(secs(&sku("runway/aleph-2"), 1.0, "", None, false, 0), 0.56);
        usd(secs(&sku("runway/aleph-2"), 3.0, "", None, false, 0), 0.84);
    }

    #[test]
    fn video_tokens_have_no_estimate() {
        assert_eq!(sku("bytedance/seedance-2.5"), Price::PerToken);
        assert_eq!(
            secs(
                &sku("bytedance/seedance-2.5"),
                5.0,
                "720p",
                Some(true),
                false,
                0
            ),
            None
        );
    }

    #[test]
    fn keys_it_does_not_know_give_unknown() {
        for id in [
            "heygen/heygen-video-1",
            "black-forest-labs/flux-video-upscale",
            "minimax/hailuo-3",
        ] {
            assert_eq!(sku(id), Price::Unknown, "{id}");
        }
    }

    #[test]
    fn features_studio_does_not_offer_are_ignored() {
        usd(
            secs(
                &sku("black-forest-labs/flux-3-video"),
                5.0,
                "720p",
                Some(true),
                false,
                0,
            ),
            0.85,
        );
    }

    #[test]
    fn every_catalogue_video_gets_a_price_or_says_why() {
        let prices: Vec<(String, Price)> = catalogue_videos()
            .map(|(id, skus)| (id, parse_pricing_skus(&skus)))
            .collect();
        assert_eq!(prices.len(), 33);
        let unknown: Vec<_> = prices
            .iter()
            .filter(|(_, p)| *p == Price::Unknown)
            .map(|(id, _)| id.as_str())
            .collect();
        assert_eq!(unknown.len(), 3, "{unknown:?}");
        assert_eq!(
            prices.iter().filter(|(_, p)| *p == Price::PerToken).count(),
            5
        );
        assert_eq!(
            prices
                .iter()
                .filter(|(_, p)| matches!(p, Price::PerSecond(_)))
                .count(),
            25
        );
    }

    #[test]
    fn picture_prices_from_the_recorded_endpoints() {
        assert_eq!(
            endpoints_price("openai/gpt-image-2.6-flare", None),
            Price::PerToken
        );
        // Both companies price it per token.
        assert_eq!(
            endpoints_price("google/gemini-nano-banana-2.1", None),
            Price::PerToken
        );
        let flux = endpoints_price("black-forest-labs/flux-3-image", None);
        usd(estimate(&flux, &pics(1, "1K", 0)), 0.048);
        usd(estimate(&flux, &pics(1, "1.5K", 0)), 0.07);
        usd(estimate(&flux, &pics(1, "4K", 0)), 0.607);
        // "high_resolution" matches no resolution value: the dearer row, whatever the choice.
        let seedream = endpoints_price("bytedance-seed/seedream-5-5", None);
        usd(estimate(&seedream, &pics(1, "1K", 0)), 0.035);
        // Plus 0.008 per added picture.
        usd(estimate(&seedream, &pics(1, "2K", 1)), 0.043);
    }

    #[test]
    fn a_pinned_company_is_used_only_when_routing_cannot_fall_back() {
        let answer = two_companies(("a", 0.04), ("b", 0.06));
        usd(
            estimate(&parse_image_endpoints(&answer, None), &pics(1, "1K", 0)),
            0.06,
        );
        usd(
            estimate(
                &parse_image_endpoints(&answer, Some(&only("a"))),
                &pics(1, "1K", 0),
            ),
            0.04,
        );
        usd(
            estimate(
                &parse_image_endpoints(&answer, Some(&order("a", true))),
                &pics(1, "1K", 0),
            ),
            0.06,
        );
        usd(
            estimate(
                &parse_image_endpoints(&answer, Some(&order("a", false))),
                &pics(1, "1K", 0),
            ),
            0.04,
        );
    }

    #[test]
    fn with_several_companies_each_choice_takes_the_dearest_price() {
        // Their tables cross: a is dearer at 2K, b at 1K, and only b charges for an added
        // picture. A run may go to either, so each priced item is the dearer of the two.
        let record = |tag: &str, pricing: Value| {
            json!({
                "provider_slug": tag,
                "provider_tag": tag,
                "supported_parameters": {"resolution": {"type": "enum", "values": ["1K", "2K", "4K"]}},
                "pricing": pricing
            })
        };
        let answer = json!({"endpoints": [
            record("a", json!([
                {"billable": "output_image", "unit": "image", "variant": "1K", "cost_usd": 0.03},
                {"billable": "output_image", "unit": "image", "variant": "2K", "cost_usd": 0.12}
            ])),
            record("b", json!([
                {"billable": "output_image", "unit": "image", "variant": "1k", "cost_usd": 0.05},
                {"billable": "output_image", "unit": "image", "variant": "2K", "cost_usd": 0.08},
                {"billable": "output_image", "unit": "image", "cost_usd": 0.20},
                {"billable": "input_image", "unit": "image", "cost_usd": 0.01}
            ]))
        ]});
        let both = parse_image_endpoints(&answer, None);
        usd(estimate(&both, &pics(1, "1K", 0)), 0.05);
        usd(estimate(&both, &pics(1, "2K", 0)), 0.12);
        usd(estimate(&both, &pics(2, "1K", 3)), 0.13);
        // Only b states a price for 4K.
        usd(estimate(&both, &pics(1, "4K", 0)), 0.20);
        // Pinned, each company keeps its own prices.
        let a = parse_image_endpoints(&answer, Some(&only("a")));
        usd(estimate(&a, &pics(1, "1K", 1)), 0.03);
        assert_eq!(estimate(&a, &pics(1, "4K", 0)), None);
        let b = parse_image_endpoints(&answer, Some(&only("b")));
        usd(estimate(&b, &pics(1, "2K", 0)), 0.08);
    }

    #[test]
    fn count_multiplies_and_pictures_in_add() {
        let p = Price::PerImage(Table {
            rows: vec![row(None, 0.04)],
            per_input_image_usd: Some(0.01),
            input_images_unpriced: false,
            minimum_usd: None,
        });
        usd(estimate(&p, &pics(4, "1K", 2)), 0.18);
    }

    #[test]
    fn a_local_run_costs_nothing_whatever_its_size() {
        assert_eq!(estimate(&Price::Free, &pics(4, "1K", 2)), Some(0.0));
        assert_eq!(Price::Free.range(), Some((0.0, 0.0)));
    }

    #[test]
    fn a_range_spans_the_rows_and_needs_a_number() {
        assert_eq!(sku("google/veo-3.1").range(), Some((0.20, 0.60)));
        assert_eq!(
            endpoints_price("black-forest-labs/flux-3-image", None).range(),
            Some((0.041, 0.607))
        );
        assert_eq!(Price::PerToken.range(), None);
        assert_eq!(Price::Unknown.range(), None);
    }

    #[test]
    fn a_company_priced_in_a_way_studio_cannot_read_is_unknown() {
        let answer = |pricing: Value| json!({"endpoints": [{"provider_slug": "a", "provider_tag": "a", "pricing": pricing}]});
        let megapixel = answer(json!([
            {"billable": "output_image", "unit": "megapixel", "cost_usd": 0.03}
        ]));
        assert_eq!(parse_image_endpoints(&megapixel, None), Price::Unknown);
        let no_output = answer(json!([
            {"billable": "input_image", "unit": "image", "cost_usd": 0.01}
        ]));
        assert_eq!(parse_image_endpoints(&no_output, None), Price::Unknown);
        // A number never wins over a company Studio cannot price.
        let mut mixed = two_companies(("a", 0.04), ("b", 0.06));
        mixed["endpoints"][1]["pricing"][0]["unit"] = json!("token");
        assert_eq!(parse_image_endpoints(&mixed, None), Price::PerToken);
        usd(
            estimate(
                &parse_image_endpoints(&mixed, Some(&only("a"))),
                &pics(1, "1K", 0),
            ),
            0.04,
        );
        // A pinned company the answer does not list has no price.
        assert_eq!(
            parse_image_endpoints(&mixed, Some(&only("c"))),
            Price::Unknown
        );
    }

    #[test]
    fn the_catalogue_carries_the_prices() {
        let answers: BTreeMap<String, Value> = answers()
            .as_object()
            .unwrap()
            .iter()
            .map(|(id, answer)| (id.clone(), answer.clone()))
            .collect();
        let with = openrouter_models(&answers);
        let flux = model(&with, "black-forest-labs/flux-3-image");
        usd(estimate(&flux.price, &pics(1, "2K", 0)), 0.1);
        assert_eq!(model(&with, "google/veo-3.1").price, sku("google/veo-3.1"));
        assert_eq!(
            model(&with, "bytedance/seedance-2.5").price,
            Price::PerToken
        );
        let without = openrouter_models(&BTreeMap::new());
        assert_eq!(
            model(&without, "black-forest-labs/flux-3-image").price,
            Price::Unknown
        );
        let local = MediaModel {
            price: Price::Free,
            ..flux
        };
        let raw = RawCatalog {
            images: serde_json::to_value(vec![local.clone()]).unwrap(),
            videos: None,
        };
        assert_eq!(
            parse_catalog(Company::Local, &raw, &BTreeMap::new())[0].price,
            Price::Free
        );
    }

    #[test]
    fn choice_for_reads_the_request() {
        let models = openrouter_models(&BTreeMap::new());

        let flare = model(&models, "openai/gpt-image-2.6-flare");
        let plain = choice_for(&flare, &request(&flare, json!({})));
        assert_eq!(plain.count, 1);
        assert_eq!(plain.seconds, None);
        assert_eq!(plain.resolution, None);
        assert_eq!(plain.audio, None);
        let set = choice_for(
            &flare,
            &with_refs(
                request(&flare, json!({"n": 3, "quality": "high"})),
                &[RefRole::Reference, RefRole::Reference],
            ),
        );
        assert_eq!(set.count, 3);
        assert_eq!(set.quality.as_deref(), Some("high"));
        assert_eq!(set.input_images, 2);
        assert!(!set.start_frame);

        let nano = model(&models, "google/gemini-nano-banana-2.1");
        let res = choice_for(&nano, &request(&nano, json!({"resolution": "2K"})));
        assert_eq!(res.resolution.as_deref(), Some("2K"));
        // A picture's size is matched as it is sent.
        let sized = request(&flare, json!({"size": "1536x1024"}));
        assert_eq!(
            choice_for(&flare, &sized).resolution.as_deref(),
            Some("1536x1024")
        );

        let veo = model(&models, "google/veo-3.1");
        let first = choice_for(
            &veo,
            &with_refs(
                request(&veo, json!({"duration": 8, "resolution": "1080p", "n": 3})),
                &[RefRole::StartFrame, RefRole::EndFrame],
            ),
        );
        assert_eq!(first.count, 1);
        assert_eq!(first.seconds, Some(8.0));
        assert_eq!(first.resolution.as_deref(), Some("1080p"));
        assert_eq!(first.audio, Some(true), "the company default");
        assert!(first.start_frame);
        assert_eq!(first.input_images, 2);
        let quiet = choice_for(&veo, &request(&veo, json!({"generate_audio": false})));
        assert_eq!(quiet.audio, Some(false));
        // A video's size is sent instead of its resolution: its shorter side decides.
        let tall = request(&veo, json!({"resolution": "720p", "size": "1080x1920"}));
        assert_eq!(choice_for(&veo, &tall).resolution.as_deref(), Some("1080p"));
        let uhd = request(&veo, json!({"duration": 8, "size": "3840x2160"}));
        let choice = choice_for(&veo, &uhd);
        assert_eq!(choice.resolution.as_deref(), Some("4K"));
        usd(estimate(&sku("google/veo-3.1"), &choice), 4.80);

        let mischief = model(&models, "tbc/mischief-1");
        assert_eq!(
            choice_for(&mischief, &request(&mischief, json!({}))).audio,
            None
        );
    }

    #[test]
    fn what_the_request_sends_comes_before_any_default() {
        let models = openrouter_models(&BTreeMap::new());
        // A picture model whose resolution and size both have a company default.
        let mut nano = model(&models, "google/gemini-nano-banana-2.1");
        let resolution = nano
            .params
            .iter_mut()
            .find(|p| p.key == "resolution")
            .unwrap();
        resolution.company_default = Some(json!("1K"));
        let mut size = resolution.clone();
        size.key = "size".into();
        size.company_default = Some(json!("auto"));
        nano.params.push(size);
        let sent_size = request(&nano, json!({"size": "1536x1024"}));
        assert_eq!(
            choice_for(&nano, &sent_size).resolution.as_deref(),
            Some("1536x1024")
        );
        let neither = request(&nano, json!({}));
        assert_eq!(
            choice_for(&nano, &neither).resolution.as_deref(),
            Some("1K")
        );

        // A video whose size has a default: a sent resolution comes first.
        let mut veo = model(&models, "google/veo-3.1");
        let mut size = veo
            .params
            .iter()
            .find(|p| p.key == "resolution")
            .unwrap()
            .clone();
        size.key = "size".into();
        size.company_default = Some(json!("3840x2160"));
        veo.params.retain(|p| p.key != "size");
        veo.params.push(size);
        let sent = request(&veo, json!({"resolution": "720p"}));
        assert_eq!(choice_for(&veo, &sent).resolution.as_deref(), Some("720p"));
        let neither = request(&veo, json!({}));
        assert_eq!(choice_for(&veo, &neither).resolution.as_deref(), Some("4K"));
    }

    #[test]
    fn openai_cost_counts_unnamed_input_at_the_dearer_rate() {
        let rates = OpenAiRates {
            text_input: Some(5.0),
            image_input: 8.0,
            image_output: 30.0,
        };
        // No details: the 1,000 input tokens may be pictures.
        let bare = json!({"input_tokens": 1000, "output_tokens": 100});
        usd(
            openai_cost(&bare, &rates),
            (1000.0 * 8.0 + 100.0 * 30.0) / 1e6,
        );
        // Only picture tokens sent: no text rate is needed.
        let pictures = json!({"input_tokens": 500, "input_tokens_details": {"text_tokens": 0, "image_tokens": 500}, "output_tokens": 10});
        let no_text = OpenAiRates {
            text_input: None,
            ..rates.clone()
        };
        usd(
            openai_cost(&pictures, &no_text),
            (500.0 * 8.0 + 10.0 * 30.0) / 1e6,
        );
        assert_eq!(openai_cost(&bare, &no_text), None, "the tokens may be text");
        assert_eq!(openai_cost(&json!({"input_tokens": 5}), &rates), None);
        assert_eq!(
            openai_cost(&json!({"input_tokens": -5, "output_tokens": 1}), &rates),
            None
        );
    }
}
