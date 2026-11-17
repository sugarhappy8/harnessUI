//! What a picture or video model costs, as its company states it.

use serde::{Deserialize, Serialize};

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
