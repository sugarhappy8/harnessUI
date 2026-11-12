//! Hugging Face Hub client: search, repository detail, token check and file
//! resolution (ARCHITECTURE.md 11, docs/research/huggingface.md).
//!
//! Two HTTP clients, neither following redirects on its own:
//! - the API client talks to the Hub endpoint and carries the optional token;
//! - the download client fetches the redirect target with no credentials, and only
//!   when that target is HTTPS on `huggingface.co`, a `*.hf.co` host or the
//!   configured mirror.
//!
//! The token is never sent to a mirror. Signed URLs are never logged.

use std::time::Duration;

use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{StatusCode, Url};
use serde::{Deserialize, Serialize};

use crate::files::RepoFile;

pub const OFFICIAL_ENDPOINT: &str = "https://huggingface.co";
/// Model cards larger than this are cut.
const MAX_CARD_BYTES: usize = 256 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum HfError {
    #[error("this model is gated; add a Hugging Face token in Settings")]
    NeedsToken,
    #[error("this model is gated; open its page on Hugging Face and accept its terms")]
    NeedsTerms,
    #[error("the repository was not found, or it is private")]
    NotFound,
    #[error("that file is not in the repository")]
    FileNotFound,
    #[error("Hugging Face is rate-limiting requests; try again in {0} seconds")]
    RateLimited(u64),
    #[error("the download was redirected to a host the app does not trust: {0}")]
    BlockedHost(String),
    #[error("invalid repository or file name: {0}")]
    InvalidName(String),
    #[error("the Hugging Face token is not valid")]
    InvalidToken,
    #[error("unexpected answer from Hugging Face (HTTP {status}): {message}")]
    Http { status: u16, message: String },
    #[error("could not reach Hugging Face: {0}")]
    Network(String),
    #[error("could not read Hugging Face's answer: {0}")]
    Parse(String),
}

impl From<reqwest::Error> for HfError {
    fn from(e: reqwest::Error) -> Self {
        // reqwest errors can include the URL; strip it so signed URLs never reach logs.
        HfError::Network(e.without_url().to_string())
    }
}

/// Client settings.
#[derive(Debug, Clone)]
pub struct HfConfig {
    /// Hub endpoint: the official one, or an `https` mirror.
    pub endpoint: String,
    /// Sent only to the official endpoint.
    pub token: Option<String>,
    /// `<app>/<version>`, non-identifying.
    pub user_agent: String,
}

impl Default for HfConfig {
    fn default() -> Self {
        Self {
            endpoint: OFFICIAL_ENDPOINT.into(),
            token: None,
            user_agent: "app/0".into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HfClient {
    api: reqwest::Client,
    download: reqwest::Client,
    endpoint: Url,
    token: Option<String>,
    official: bool,
    #[cfg(test)]
    allow_http_loopback: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    #[default]
    Downloads,
    TrendingScore,
    LastModified,
    Likes,
}

impl Sort {
    fn as_str(self) -> &'static str {
        match self {
            Sort::Downloads => "downloads",
            Sort::TrendingScore => "trendingScore",
            Sort::LastModified => "lastModified",
            Sort::Likes => "likes",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SearchQuery {
    pub text: String,
    #[serde(default)]
    pub sort: Sort,
    /// Show every GGUF repository, not only chat models.
    #[serde(default)]
    pub all_gguf: bool,
}

/// `false`, `"auto"` or `"manual"` in the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Gated {
    No,
    Auto,
    Manual,
}

fn gated_of(v: &serde_json::Value) -> Gated {
    match v.as_str() {
        Some("manual") => Gated::Manual,
        Some(_) => Gated::Auto,
        None if v.as_bool() == Some(true) => Gated::Auto,
        None => Gated::No,
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoSummary {
    pub id: String,
    pub downloads: u64,
    pub likes: u64,
    pub gated: Gated,
    pub license: Option<String>,
    pub last_modified: Option<String>,
    pub pipeline_tag: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub items: Vec<RepoSummary>,
    /// Opaque URL of the next page, or `None` at the end.
    pub next: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GgufInfo {
    pub architecture: Option<String>,
    pub context_length: Option<u64>,
    /// Parameter count (not bytes).
    pub parameters: Option<u64>,
    /// Untrusted text; only used for the tools/thinking heuristics.
    #[serde(skip_serializing)]
    pub chat_template: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoDetail {
    pub id: String,
    /// The commit the listing describes; downloads pin it.
    pub commit: String,
    pub gated: Gated,
    pub license: Option<String>,
    pub files: Vec<RepoFile>,
    pub gguf: Option<GgufInfo>,
    pub template_tools: bool,
    pub template_thinking: bool,
    /// 0.0.6: what the uploader lists about the repository, made safe to show.
    pub info: HubInfo,
}

// ---- What an uploader lists (0.0.6 item 4, D6 source 2) ------------------------------

/// The most characters of any one piece of uploader text that is kept (0.0.6 item 4).
pub const UPLOADER_TEXT_CAP: usize = 300;
/// The most base models and languages kept from one repository.
const MAX_BASE_MODELS: usize = 5;
const MAX_LANGUAGES: usize = 50;

/// Tags that say what a model does. Every other tag (formats, regions, model families,
/// anything an uploader made up) is dropped. The webview has a phrase for each of these
/// (`TAG_WORDS` in `src/lib/abilities.ts`).
pub const KNOWN_TAGS: [&str; 23] = [
    "conversational",
    "chat",
    "code",
    "coding",
    "math",
    "reasoning",
    "thinking",
    "tool-use",
    "function-calling",
    "tools",
    "agent",
    "vision",
    "multimodal",
    "long-context",
    "multilingual",
    "translation",
    "creative-writing",
    "roleplay",
    "medical",
    "legal",
    "finance",
    "uncensored",
    "abliterated",
];

/// What the uploader of a repository lists about it. Every string is uploader text made
/// safe to show with [`uploader_text`]; links are https only; tags are [`KNOWN_TAGS`]
/// only. Nothing here is ever rendered as markup or treated as an instruction.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HubInfo {
    /// The account that uploaded the repository.
    pub author: Option<String>,
    /// What the uploader says the model does (`text-generation`…), shape-checked.
    pub pipeline_tag: Option<String>,
    /// The base models the uploader names, as `owner/name` (at most five). An
    /// uploader's claim only: anyone can name any model here.
    pub base_models: Vec<String>,
    /// Language codes (`en`, `zh-Hans`) or `multilingual`.
    pub languages: Vec<String>,
    /// The licence's name or id.
    pub license: Option<String>,
    /// An https link to the licence text, when the uploader gives one.
    pub license_url: Option<String>,
    /// Known tags only, lower case, once each.
    pub tags: Vec<String>,
    /// Downloads in the last 30 days, as Hugging Face counts them.
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    /// When the repository last changed, as Hugging Face reports it (ISO 8601).
    pub last_modified: Option<String>,
}

/// Characters that change how text looks without being visible, and control characters:
/// the same set as the approval cards' sanitiser (`crates/policy/src/sanitize.rs`) and the
/// comparison screen's.
fn is_invisible_or_control(c: char) -> bool {
    let u = c as u32;
    c.is_control()
        || matches!(u,
            0x00AD            // soft hyphen
            | 0x061C          // Arabic letter mark
            | 0x115F | 0x1160 // Hangul fillers
            | 0x17B4 | 0x17B5
            | 0x180B..=0x180F // Mongolian variation selectors and vowel separator
            | 0x200B..=0x200F // zero-width space and joiners, LRM, RLM
            | 0x202A..=0x202E // bidi embeddings and overrides
            | 0x2060..=0x206F // word joiner, invisible operators, bidi isolates
            | 0x3164          // Hangul filler
            | 0xFE00..=0xFE0F // variation selectors
            | 0xFEFF          // zero-width no-break space
            | 0xFFA0
            | 0xFFF9..=0xFFFB // interlinear annotation
            | 0x1D173..=0x1D17A
            | 0xE0000..=0xE007F // tag characters
            | 0xE0100..=0xE01EF)
}

/// Uploader text as the app shows it: one line (runs of spaces, tabs and line breaks
/// become one space), every invisible or control character written as `[U+XXXX]`, and
/// at most `cap` characters with `…` at the end when it was cut. An escape is never cut
/// in half. Empty text gives an empty string.
///
/// Reading stops as soon as the text is known to be longer than `cap`, so a hostile
/// string of many megabytes costs no more than its first few hundred characters.
pub fn uploader_text(text: &str, cap: usize) -> String {
    let mut t = Capped {
        out: String::new(),
        used: 0,
        cut_at: None,
        cap,
    };
    let mut space = false;
    let mut buf = [0u8; 4];
    for c in text.trim().chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        let fits = (!space || t.push(" ", 1))
            && if is_invisible_or_control(c) {
                let escape = format!("[U+{:04X}]", c as u32);
                t.push(&escape, escape.len())
            } else {
                t.push(c.encode_utf8(&mut buf), 1)
            };
        space = false;
        if !fits {
            // Too long: cut after the last whole piece that leaves room for `…`.
            let mut out = t.out;
            out.truncate(t.cut_at.unwrap_or(0));
            out.truncate(out.trim_end().len());
            out.push('…');
            return out;
        }
    }
    t.out
}

/// Text being built by [`uploader_text`], one piece (a character, a space or an escape)
/// at a time.
struct Capped {
    out: String,
    /// Characters in `out`.
    used: usize,
    /// The end of the last whole piece that fits in `cap - 1` characters, leaving room
    /// for `…`; where `out` is cut when the text turns out longer than `cap`.
    cut_at: Option<usize>,
    cap: usize,
}

impl Capped {
    /// Adds a piece of `chars` characters; false once the text is longer than the cap.
    fn push(&mut self, piece: &str, chars: usize) -> bool {
        if self.cut_at.is_none() && self.used + chars > self.cap.saturating_sub(1) {
            self.cut_at = Some(self.out.len());
        }
        self.out.push_str(piece);
        self.used += chars;
        self.used <= self.cap
    }
}

/// A string, or the strings of a list (other values dropped).
fn strings(v: Option<&serde_json::Value>) -> Vec<&str> {
    match v {
        Some(serde_json::Value::String(s)) => vec![s.as_str()],
        Some(serde_json::Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).collect(),
        _ => Vec::new(),
    }
}

/// A language code (`en`, `fra`, `zh-Hans`, `pt-BR`) or `multilingual`.
fn is_language_code(s: &str) -> bool {
    if s.eq_ignore_ascii_case("multilingual") {
        return true;
    }
    let mut parts = s.split('-');
    let first = parts.next().unwrap_or("");
    (2..=3).contains(&first.len())
        && first.bytes().all(|b| b.is_ascii_alphabetic())
        && parts.all(|p| (2..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// A Hugging Face task name: lower-case letters, digits and hyphens.
fn is_task_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 50
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An absolute https link without a user name, as text; `None` for anything else.
fn https_link(text: &str) -> Option<String> {
    let url = Url::parse(text.trim()).ok()?;
    (url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty())
        .then(|| url.to_string())
        .filter(|u| u.len() <= MAX_LINK_CHARS)
}

/// The longest licence link kept (a link is percent-encoded ASCII once parsed).
const MAX_LINK_CHARS: usize = 500;

/// What the uploader lists, from a repository answer (`repo_id` already validated).
fn parse_hub_info(repo_id: &str, commit: &str, endpoint: &Url, v: &serde_json::Value) -> HubInfo {
    let card = v.get("cardData").filter(|c| c.is_object());
    let card_field = |k: &str| card.and_then(|c| c.get(k));
    let tags: Vec<String> = strings(v.get("tags"))
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect();

    // Base models: the card's `base_model`, else the `base_model:[relation:]owner/name`
    // tags. Only real repository ids are kept.
    let mut bases: Vec<String> = strings(card_field("base_model"))
        .into_iter()
        .map(str::trim)
        .map(str::to_string)
        .collect();
    if bases.is_empty() {
        for t in strings(v.get("tags")) {
            let Some(rest) = t.strip_prefix("base_model:") else {
                continue;
            };
            let rest = ["quantized:", "finetune:", "adapter:", "merge:"]
                .iter()
                .find_map(|r| rest.strip_prefix(r))
                .unwrap_or(rest);
            bases.push(rest.trim().to_string());
        }
    }
    let mut base_models: Vec<String> = Vec::new();
    for b in bases {
        if validate_repo_id(&b).is_ok() && !base_models.iter().any(|x| x.eq_ignore_ascii_case(&b)) {
            base_models.push(b);
        }
        if base_models.len() == MAX_BASE_MODELS {
            break;
        }
    }

    let mut languages: Vec<String> = Vec::new();
    for l in strings(card_field("language")) {
        let l = l.trim();
        if is_language_code(l) && !languages.iter().any(|x| x.eq_ignore_ascii_case(l)) {
            languages.push(l.to_string());
        }
        if languages.len() == MAX_LANGUAGES {
            break;
        }
    }

    // The licence: the `license:` tag, else the card's; "other" names itself in
    // `license_name`.
    let license_id = v.get("tags").and_then(license_from_tags).or_else(|| {
        strings(card_field("license"))
            .first()
            .map(|s| s.to_string())
    });
    let license = license_id
        .map(
            |id| match card_field("license_name").and_then(|n| n.as_str()) {
                Some(name) if id.eq_ignore_ascii_case("other") && !name.trim().is_empty() => {
                    name.to_string()
                }
                _ => id,
            },
        )
        .map(|l| uploader_text(&l, UPLOADER_TEXT_CAP))
        .filter(|l| !l.is_empty());
    let license_url = card_field("license_link")
        .and_then(|l| l.as_str())
        .and_then(|link| {
            https_link(link).or_else(|| {
                let path = link.trim().trim_start_matches("./");
                validate_repo_path(path).ok()?;
                let mut u = endpoint.clone();
                u.path_segments_mut()
                    .ok()?
                    .clear()
                    .extend(repo_id.split('/'))
                    .push("blob")
                    .push(commit)
                    .extend(path.split('/'));
                Some(u.to_string())
            })
        });

    let mut known: Vec<String> = Vec::new();
    for t in &tags {
        if KNOWN_TAGS.contains(&t.as_str()) && !known.contains(t) {
            known.push(t.clone());
        }
    }

    let pipeline_tag = v
        .get("pipeline_tag")
        .or_else(|| card_field("pipeline_tag"))
        .and_then(|p| p.as_str())
        .filter(|p| is_task_name(p))
        .map(str::to_string);
    let last_modified = v
        .get("lastModified")
        .and_then(|d| d.as_str())
        .filter(|d| {
            let b = d.as_bytes();
            b.len() >= 10
                && b[..4].iter().all(u8::is_ascii_digit)
                && b[4] == b'-'
                && b[5..7].iter().all(u8::is_ascii_digit)
                && b[7] == b'-'
                && b[8..10].iter().all(u8::is_ascii_digit)
        })
        .map(|d| uploader_text(d, 40));

    HubInfo {
        author: v
            .get("author")
            .and_then(|a| a.as_str())
            .map(|a| uploader_text(a, UPLOADER_TEXT_CAP))
            .filter(|a| !a.is_empty()),
        pipeline_tag,
        base_models,
        languages,
        license,
        license_url,
        tags: known,
        downloads: v.get("downloads").and_then(|d| d.as_u64()),
        likes: v.get("likes").and_then(|d| d.as_u64()),
        last_modified,
    }
}

/// Where a file's bytes are, with the Hub's own size and hash for it.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// The URL to fetch with [`HfClient::download_client`]. Never log it.
    pub url: Url,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    pub commit: Option<String>,
}

/// Checks a repository id: `owner/name`, ASCII letters, digits, `-`, `_`, `.`.
pub fn validate_repo_id(id: &str) -> Result<(&str, &str), HfError> {
    let bad = || HfError::InvalidName(id.to_string());
    let (owner, name) = id.split_once('/').ok_or_else(bad)?;
    let ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 96
            && s != "."
            && s != ".."
            && !s.starts_with('.')
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if ok(owner) && ok(name) {
        Ok((owner, name))
    } else {
        Err(bad())
    }
}

/// Checks a file path inside a repository and returns its segments. Rejects
/// anything that could escape the destination folder on Windows or macOS.
pub fn validate_repo_path(path: &str) -> Result<Vec<&str>, HfError> {
    let bad = || HfError::InvalidName(path.to_string());
    if path.is_empty() || path.len() > 512 || path.starts_with('/') {
        return Err(bad());
    }
    let segments: Vec<&str> = path.split('/').collect();
    for s in &segments {
        let reserved = {
            let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
            matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
                || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                    && stem.len() == 4
                    && stem.as_bytes()[3].is_ascii_digit())
        };
        if s.is_empty()
            || *s == "."
            || *s == ".."
            || s.ends_with('.')
            || s.ends_with(' ')
            || reserved
            || s.chars().any(|c| {
                c.is_control() || matches!(c, '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            })
        {
            return Err(bad());
        }
    }
    Ok(segments)
}

/// Whether a redirect target may be fetched (HTTPS on the Hub, its CDN, or the mirror).
fn allowed_download_host(url: &Url, mirror_host: Option<&str>) -> bool {
    if url.scheme() != "https" || url.port().is_some_and(|p| p != 443) {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "huggingface.co" || host.ends_with(".hf.co") || mirror_host.is_some_and(|m| host == m)
}

fn strip_quotes(s: &str) -> String {
    s.trim()
        .trim_start_matches("W/")
        .trim_matches('"')
        .to_ascii_lowercase()
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn header_str<'a>(h: &'a HeaderMap, name: &str) -> Option<&'a str> {
    h.get(name).and_then(|v| v.to_str().ok())
}

/// Seconds until the rate-limit window resets, from `ratelimit: "api";r=0;t=29`.
fn retry_after(h: &HeaderMap) -> u64 {
    header_str(h, "ratelimit")
        .and_then(|v| {
            v.split(';')
                .find_map(|p| p.trim().strip_prefix("t=")?.parse().ok())
        })
        .or_else(|| header_str(h, "retry-after").and_then(|v| v.parse().ok()))
        .unwrap_or(60)
}

/// Maps an error response to [`HfError`] (docs/research/huggingface.md).
fn error_for(status: StatusCode, headers: &HeaderMap, body: &str) -> HfError {
    let code = header_str(headers, "x-error-code").unwrap_or("");
    match (status.as_u16(), code) {
        (401, "GatedRepo") => HfError::NeedsToken,
        (403, "GatedRepo") => HfError::NeedsTerms,
        (401, _) => HfError::NotFound,
        (404, "EntryNotFound") => HfError::FileNotFound,
        (404, _) => HfError::NotFound,
        (429, _) => HfError::RateLimited(retry_after(headers)),
        (s, _) => HfError::Http {
            status: s,
            message: header_str(headers, "x-error-message")
                .map(str::to_string)
                .unwrap_or_else(|| body.chars().take(300).collect()),
        },
    }
}

fn license_from_tags(tags: &serde_json::Value) -> Option<String> {
    tags.as_array()?
        .iter()
        .filter_map(|t| t.as_str())
        .find_map(|t| t.strip_prefix("license:").map(str::to_string))
}

fn template_flags(template: Option<&str>) -> (bool, bool) {
    let t = template.unwrap_or("").to_ascii_lowercase();
    let tools = t.contains("tools");
    let thinking =
        t.contains("<think>") || t.contains("enable_thinking") || t.contains("reasoning_content");
    (tools, thinking)
}

impl HfClient {
    pub fn new(config: HfConfig) -> Result<Self, HfError> {
        let endpoint = Url::parse(config.endpoint.trim_end_matches('/'))
            .map_err(|_| HfError::InvalidName(config.endpoint.clone()))?;
        if endpoint.scheme() != "https" {
            return Err(HfError::InvalidName(
                "the Hugging Face endpoint must use https".into(),
            ));
        }
        let official = endpoint.host_str() == Some("huggingface.co");
        let build = || {
            reqwest::Client::builder()
                .user_agent(config.user_agent.clone())
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(20))
                .read_timeout(Duration::from_secs(60))
                .build()
                .map_err(HfError::from)
        };
        Ok(Self {
            api: build()?,
            download: build()?,
            endpoint,
            token: if official {
                config.token.filter(|t| !t.trim().is_empty())
            } else {
                None
            },
            official,
            #[cfg(test)]
            allow_http_loopback: false,
        })
    }

    /// A client for tests that talks plain HTTP to a local server.
    #[cfg(test)]
    pub(crate) fn for_test(endpoint: &str) -> Self {
        let api = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .unwrap();
        Self {
            download: api.clone(),
            api,
            endpoint: Url::parse(endpoint).unwrap(),
            token: None,
            official: false,
            allow_http_loopback: true,
        }
    }

    pub fn is_official(&self) -> bool {
        self.official
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn url(&self, path: &str) -> Url {
        let mut u = self.endpoint.clone();
        u.set_path(path);
        u
    }

    fn api_get(&self, url: Url) -> reqwest::RequestBuilder {
        let req = self.api.get(url);
        match &self.token {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    async fn json(&self, url: Url) -> Result<(serde_json::Value, HeaderMap), HfError> {
        let resp = self.api_get(url).send().await?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(error_for(status, &headers, &body));
        }
        let v = resp
            .json()
            .await
            .map_err(|e| HfError::Parse(e.without_url().to_string()))?;
        Ok((v, headers))
    }

    /// One page of GGUF repositories matching the query.
    pub async fn search(&self, q: &SearchQuery) -> Result<SearchPage, HfError> {
        let text = q.text.trim();
        let mut url = self.url("/api/models");
        {
            let mut p = url.query_pairs_mut();
            p.append_pair("filter", "gguf");
            if !q.all_gguf {
                p.append_pair("filter", "conversational");
            }
            if !text.is_empty() {
                p.append_pair("search", text);
            }
            p.append_pair("sort", q.sort.as_str());
            p.append_pair("limit", "50");
            for e in [
                "downloads",
                "likes",
                "gated",
                "tags",
                "lastModified",
                "pipeline_tag",
            ] {
                p.append_pair("expand[]", e);
            }
        }
        self.search_url(url).await
    }

    /// The page after `next` (from a previous [`SearchPage`]). The URL must point at
    /// this client's endpoint and its search API.
    pub async fn search_next(&self, next: &str) -> Result<SearchPage, HfError> {
        let url = Url::parse(next).map_err(|_| HfError::InvalidName(next.into()))?;
        if url.origin() != self.endpoint.origin() || url.path() != "/api/models" {
            return Err(HfError::InvalidName("unexpected next-page address".into()));
        }
        self.search_url(url).await
    }

    async fn search_url(&self, url: Url) -> Result<SearchPage, HfError> {
        let (v, headers) = self.json(url).await?;
        let arr = v
            .as_array()
            .ok_or_else(|| HfError::Parse("expected a list".into()))?;
        let items = arr
            .iter()
            .filter_map(|m| {
                let id = m.get("id")?.as_str()?.to_string();
                validate_repo_id(&id).ok()?;
                Some(RepoSummary {
                    downloads: m.get("downloads").and_then(|d| d.as_u64()).unwrap_or(0),
                    likes: m.get("likes").and_then(|d| d.as_u64()).unwrap_or(0),
                    gated: gated_of(m.get("gated").unwrap_or(&serde_json::Value::Bool(false))),
                    license: m.get("tags").and_then(license_from_tags),
                    last_modified: m
                        .get("lastModified")
                        .and_then(|d| d.as_str())
                        .map(str::to_string),
                    pipeline_tag: m
                        .get("pipeline_tag")
                        .and_then(|d| d.as_str())
                        .map(str::to_string),
                    id,
                })
            })
            .collect();
        let next = header_str(&headers, "link")
            .and_then(parse_next_link)
            .filter(|n| {
                Url::parse(n).is_ok_and(|u| {
                    u.origin() == self.endpoint.origin() && u.path() == "/api/models"
                })
            });
        Ok(SearchPage { items, next })
    }

    /// Every file with size and SHA-256, the commit, the gated state and the GGUF block.
    pub async fn repo(&self, repo_id: &str, revision: Option<&str>) -> Result<RepoDetail, HfError> {
        validate_repo_id(repo_id)?;
        let path = match revision {
            Some(rev) => {
                if !rev
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
                {
                    return Err(HfError::InvalidName(rev.into()));
                }
                format!("/api/models/{repo_id}/revision/{rev}")
            }
            None => format!("/api/models/{repo_id}"),
        };
        let mut url = self.url(&path);
        url.query_pairs_mut()
            .append_pair("blobs", "true")
            .append_pair("expand[]", "gguf");
        // expand[] limits the answer to the listed fields, so ask for the rest too
        // (0.0.6: and what the comparison shows from the uploader).
        for e in REPO_EXPAND {
            url.query_pairs_mut().append_pair("expand[]", e);
        }
        let (v, _) = self.json(url).await?;
        parse_repo_detail(repo_id, &v, &self.endpoint)
    }

    /// The repository's page on this client's endpoint (`<endpoint>/<owner>/<name>`), for
    /// "Show model card". `None` for a name that is not a repository id.
    pub fn page_url(&self, repo_id: &str) -> Option<String> {
        validate_repo_id(repo_id).ok()?;
        let mut u = self.endpoint.clone();
        u.path_segments_mut()
            .ok()?
            .clear()
            .extend(repo_id.split('/'));
        Some(u.to_string())
    }

    /// The endpoint as text (`https://huggingface.co`), to key what is remembered per
    /// endpoint.
    pub fn endpoint(&self) -> &str {
        self.endpoint.as_str()
    }

    /// The model card as text, cut at 256 KB. Untrusted: shown as sanitised text.
    pub async fn model_card(&self, repo_id: &str, commit: &str) -> Result<String, HfError> {
        validate_repo_id(repo_id)?;
        let url = self.url(&format!("/{repo_id}/raw/{commit}/README.md"));
        let resp = self.api_get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let headers = resp.headers().clone();
            return Err(error_for(status, &headers, ""));
        }
        let bytes = resp.bytes().await?;
        let cut = &bytes[..bytes.len().min(MAX_CARD_BYTES)];
        Ok(String::from_utf8_lossy(cut).into_owned())
    }

    /// Checks the token and returns the account name.
    pub async fn whoami(&self) -> Result<String, HfError> {
        if self.token.is_none() {
            return Err(HfError::InvalidToken);
        }
        match self.json(self.url("/api/whoami-v2")).await {
            Ok((v, _)) => Ok(v
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or("")
                .to_string()),
            Err(HfError::NotFound) | Err(HfError::NeedsToken) => Err(HfError::InvalidToken),
            Err(e) => Err(e),
        }
    }

    /// Resolves a file to its download URL without downloading it, following at
    /// most a few same-host redirects.
    pub async fn resolve(
        &self,
        repo_id: &str,
        commit: &str,
        file: &str,
    ) -> Result<Resolved, HfError> {
        validate_repo_id(repo_id)?;
        validate_repo_path(file)?;
        if !commit
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(HfError::InvalidName(commit.into()));
        }
        let mut url = self.url(&format!("/{repo_id}/resolve/{commit}/"));
        url.path_segments_mut()
            .map_err(|_| HfError::InvalidName(file.into()))?
            .pop_if_empty()
            .extend(file.split('/'));
        let mut size = None;
        let mut sha = None;
        let mut repo_commit = None;
        for _ in 0..5 {
            let req = self.api.head(url.clone());
            let req = match &self.token {
                Some(t) => req.bearer_auth(t),
                None => req,
            };
            let resp = req.send().await?;
            let status = resp.status();
            let headers = resp.headers().clone();
            if size.is_none() {
                size = header_str(&headers, "x-linked-size").and_then(|v| v.parse().ok());
            }
            if sha.is_none() {
                sha = header_str(&headers, "x-linked-etag")
                    .map(strip_quotes)
                    .filter(|s| is_sha256_hex(s));
            }
            if repo_commit.is_none() {
                repo_commit = header_str(&headers, "x-repo-commit").map(str::to_string);
            }
            if status.is_redirection() {
                let loc = header_str(&headers, LOCATION.as_str())
                    .ok_or_else(|| HfError::Parse("redirect without a location".into()))?;
                let next = url
                    .join(loc)
                    .map_err(|_| HfError::Parse("bad redirect location".into()))?;
                if next.origin() == self.endpoint.origin() {
                    url = next;
                    continue;
                }
                let mirror = (!self.official).then(|| self.endpoint.host_str()).flatten();
                if !self.download_host_ok(&next, mirror) {
                    return Err(HfError::BlockedHost(
                        next.host_str().unwrap_or("?").to_string(),
                    ));
                }
                return Ok(Resolved {
                    url: next,
                    size,
                    sha256: sha,
                    commit: repo_commit,
                });
            }
            if status.is_success() {
                // Served directly (small files, or a mirror without a CDN).
                if size.is_none() {
                    size = header_str(&headers, "content-length").and_then(|v| v.parse().ok());
                }
                return Ok(Resolved {
                    url,
                    size,
                    sha256: sha,
                    commit: repo_commit,
                });
            }
            let body = resp.text().await.unwrap_or_default();
            return Err(error_for(status, &headers, &body));
        }
        Err(HfError::Http {
            status: 310,
            message: "too many redirects".into(),
        })
    }

    /// The first `max_bytes` of a file (for reading a GGUF header before download).
    pub async fn fetch_head(
        &self,
        repo_id: &str,
        commit: &str,
        file: &str,
        max_bytes: u64,
    ) -> Result<Vec<u8>, HfError> {
        let resolved = self.resolve(repo_id, commit, file).await?;
        let resp = self
            .request_for(&resolved)
            .header("Range", format!("bytes=0-{}", max_bytes.saturating_sub(1)))
            .header("Accept-Encoding", "identity")
            .send()
            .await?;
        let status = resp.status();
        if !(status.is_success()) {
            let headers = resp.headers().clone();
            return Err(error_for(status, &headers, ""));
        }
        // A server that ignores Range sends the whole file; stop reading at the limit.
        let mut out = Vec::with_capacity(max_bytes.min(16 << 20) as usize);
        let mut stream = resp.bytes_stream();
        use futures_util::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            let room = (max_bytes as usize).saturating_sub(out.len());
            out.extend_from_slice(&chunk[..chunk.len().min(room)]);
            if out.len() as u64 >= max_bytes {
                break;
            }
        }
        Ok(out)
    }

    fn download_host_ok(&self, url: &Url, mirror: Option<&str>) -> bool {
        #[cfg(test)]
        if self.allow_http_loopback && url.host_str() == Some("127.0.0.1") {
            return true;
        }
        allowed_download_host(url, mirror)
    }

    /// The credential-free client for fetching [`Resolved::url`]; or, when the URL is
    /// on the Hub endpoint itself, the API client.
    pub(crate) fn request_for(&self, resolved: &Resolved) -> reqwest::RequestBuilder {
        if resolved.url.origin() == self.endpoint.origin() {
            self.api_get(resolved.url.clone())
        } else {
            self.download.get(resolved.url.clone())
        }
    }
}

/// The `rel="next"` target of a `Link` header.
fn parse_next_link(link: &str) -> Option<String> {
    link.split(',').find_map(|part| {
        let (target, params) = part.split_once(';')?;
        if !params
            .split(';')
            .any(|p| p.trim().replace(' ', "") == "rel=\"next\"")
        {
            return None;
        }
        Some(
            target
                .trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string(),
        )
    })
}

/// The fields [`HfClient::repo`] asks for besides `gguf` (expand[] limits the answer to
/// the listed fields).
pub const REPO_EXPAND: [&str; 10] = [
    "sha",
    "gated",
    "tags",
    "siblings",
    "cardData",
    "author",
    "downloads",
    "likes",
    "lastModified",
    "pipeline_tag",
];

/// Reads a repository answer (`GET /api/models/<repo>` with [`REPO_EXPAND`]) for
/// `repo_id`, already validated. Public so other crates' tests can read fixtures; the app
/// gets it through [`HfClient::repo`].
pub fn parse_repo_detail(
    repo_id: &str,
    v: &serde_json::Value,
    endpoint: &Url,
) -> Result<RepoDetail, HfError> {
    let commit = v
        .get("sha")
        .and_then(|s| s.as_str())
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| HfError::Parse("no commit in the repository listing".into()))?
        .to_string();
    let files = v
        .get("siblings")
        .and_then(|s| s.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|f| {
                    let path = f.get("rfilename")?.as_str()?.to_string();
                    validate_repo_path(&path).ok()?;
                    let lfs = f.get("lfs");
                    let size = lfs
                        .and_then(|l| l.get("size"))
                        .or_else(|| f.get("size"))
                        .and_then(|s| s.as_u64())
                        .unwrap_or(0);
                    let sha256 = lfs
                        .and_then(|l| l.get("sha256"))
                        .and_then(|s| s.as_str())
                        .map(str::to_ascii_lowercase)
                        .filter(|s| is_sha256_hex(s));
                    Some(RepoFile { path, size, sha256 })
                })
                .collect()
        })
        .unwrap_or_default();
    let gguf = v.get("gguf").filter(|g| g.is_object()).map(|g| GgufInfo {
        architecture: g
            .get("architecture")
            .and_then(|s| s.as_str())
            .map(str::to_string),
        context_length: g.get("context_length").and_then(|s| s.as_u64()),
        parameters: g.get("total").and_then(|s| s.as_u64()),
        chat_template: g
            .get("chat_template")
            .and_then(|s| s.as_str())
            .map(str::to_string),
    });
    let (template_tools, template_thinking) =
        template_flags(gguf.as_ref().and_then(|g| g.chat_template.as_deref()));
    let license = v.get("tags").and_then(license_from_tags).or_else(|| {
        v.get("cardData")
            .and_then(|c| c.get("license"))
            .and_then(|l| l.as_str())
            .map(str::to_string)
    });
    let info = parse_hub_info(repo_id, &commit, endpoint, v);
    Ok(RepoDetail {
        info,
        id: repo_id.to_string(),
        commit,
        gated: gated_of(v.get("gated").unwrap_or(&serde_json::Value::Bool(false))),
        license,
        files,
        gguf,
        template_tools,
        template_thinking,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_ids() {
        assert!(validate_repo_id("unsloth/Qwen3-8B-GGUF").is_ok());
        assert!(validate_repo_id("ggml-org/gpt-oss-20b-GGUF").is_ok());
        for bad in [
            "noslash",
            "a/b/c",
            "../x",
            "a/..",
            "a/.hidden",
            "a /b",
            "a/b?x",
            "",
            "/b",
            "a/",
        ] {
            assert!(validate_repo_id(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn repo_paths_cannot_escape() {
        assert_eq!(
            validate_repo_path("BF16/Model-BF16-00001-of-00002.gguf")
                .unwrap()
                .len(),
            2
        );
        assert!(validate_repo_path("model.gguf").is_ok());
        for bad in [
            "../x.gguf",
            "a/../../x.gguf",
            "/etc/passwd",
            "a//b.gguf",
            "a\\..\\b.gguf",
            "C:/x.gguf",
            "c:x.gguf",
            "con.gguf",
            "dir/NUL",
            "COM1.gguf",
            "lpt9",
            "trailing.",
            "space ",
            "a/./b",
            "",
            "x\u{0}.gguf",
            "what?.gguf",
        ] {
            assert!(validate_repo_path(bad).is_err(), "{bad:?}");
        }
        assert!(validate_repo_path("COMPUTE.gguf").is_ok());
    }

    #[test]
    fn download_hosts() {
        let ok = |s: &str, m: Option<&str>| allowed_download_host(&Url::parse(s).unwrap(), m);
        assert!(ok("https://us.aws.cdn.hf.co/xet-bridge-us/abc?sig=1", None));
        assert!(ok("https://cas-bridge.xethub.hf.co/x", None));
        assert!(ok("https://huggingface.co/x", None));
        assert!(!ok("http://us.aws.cdn.hf.co/x", None));
        assert!(!ok("https://evil.com/x", None));
        assert!(!ok("https://hf.co.evil.com/x", None));
        assert!(!ok("https://evilhf.co/x", None));
        assert!(!ok("https://us.aws.cdn.hf.co:8443/x", None));
        assert!(ok("https://mirror.example/x", Some("mirror.example")));
        assert!(!ok("https://mirror.example/x", None));
    }

    #[test]
    fn errors_map_from_status_and_code() {
        let mut h = HeaderMap::new();
        h.insert("x-error-code", "GatedRepo".parse().unwrap());
        assert!(matches!(
            error_for(StatusCode::UNAUTHORIZED, &h, ""),
            HfError::NeedsToken
        ));
        assert!(matches!(
            error_for(StatusCode::FORBIDDEN, &h, ""),
            HfError::NeedsTerms
        ));
        let empty = HeaderMap::new();
        assert!(matches!(
            error_for(StatusCode::UNAUTHORIZED, &empty, ""),
            HfError::NotFound
        ));
        let mut h = HeaderMap::new();
        h.insert("x-error-code", "EntryNotFound".parse().unwrap());
        assert!(matches!(
            error_for(StatusCode::NOT_FOUND, &h, ""),
            HfError::FileNotFound
        ));
        let mut h = HeaderMap::new();
        h.insert("ratelimit", "\"api\";r=0;t=29".parse().unwrap());
        assert!(matches!(
            error_for(StatusCode::TOO_MANY_REQUESTS, &h, ""),
            HfError::RateLimited(29)
        ));
    }

    #[test]
    fn next_link() {
        let link = r#"<https://huggingface.co/api/models?filter=gguf&cursor=abc>; rel="next""#;
        assert_eq!(
            parse_next_link(link).as_deref(),
            Some("https://huggingface.co/api/models?filter=gguf&cursor=abc")
        );
        assert_eq!(parse_next_link(r#"<https://x>; rel="prev""#), None);
    }

    #[test]
    fn token_only_for_official_endpoint() {
        let c = HfClient::new(HfConfig {
            token: Some("hf_x".into()),
            ..HfConfig::default()
        })
        .unwrap();
        assert!(c.has_token());
        let m = HfClient::new(HfConfig {
            endpoint: "https://mirror.example".into(),
            token: Some("hf_x".into()),
            ..HfConfig::default()
        })
        .unwrap();
        assert!(!m.has_token());
        assert!(
            HfClient::new(HfConfig {
                endpoint: "http://mirror.example".into(),
                ..HfConfig::default()
            })
            .is_err()
        );
    }

    #[test]
    fn repo_detail_parsing() {
        let v = serde_json::json!({
            "sha": "0123456789abcdef0123456789abcdef01234567",
            "gated": "manual",
            "tags": ["gguf", "license:apache-2.0"],
            "gguf": {"architecture": "qwen3", "context_length": 40960, "total": 600000000,
                      "chat_template": "{% if tools %}{% endif %}<think>"},
            "siblings": [
                {"rfilename": "README.md", "size": 100},
                {"rfilename": "Qwen3-0.6B-Q8_0.gguf", "size": 1, "lfs": {"sha256": "AB".repeat(32), "size": 639447744}},
                {"rfilename": "../escape.gguf", "size": 1}
            ]
        });
        let d = parse_repo_detail("Qwen/Qwen3-0.6B-GGUF", &v, &official()).unwrap();
        assert_eq!(d.gated, Gated::Manual);
        assert_eq!(d.license.as_deref(), Some("apache-2.0"));
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[1].size, 639447744);
        assert_eq!(d.files[1].sha256.as_deref(), Some("ab".repeat(32).as_str()));
        assert!(d.template_tools && d.template_thinking);
        assert_eq!(d.gguf.unwrap().context_length, Some(40960));
        assert!(
            parse_repo_detail("a/b", &serde_json::json!({"sha": "short"}), &official()).is_err()
        );
        // Without the new fields the uploader's list holds only the licence.
        let d = parse_repo_detail("Qwen/Qwen3-0.6B-GGUF", &v, &official()).unwrap();
        assert_eq!(
            d.info,
            HubInfo {
                license: Some("apache-2.0".into()),
                ..HubInfo::default()
            }
        );
    }

    // ---- 0.0.6 item 4: what the uploader lists -----------------------------------------

    fn official() -> Url {
        Url::parse(OFFICIAL_ENDPOINT).unwrap()
    }

    const FULL: &str = include_str!("../tests/fixtures/hf/repo-full.json");
    const HOSTILE: &str = include_str!("../tests/fixtures/hf/repo-hostile.json");

    fn detail(json: &str) -> RepoDetail {
        let v: serde_json::Value = serde_json::from_str(json).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        parse_repo_detail(&id, &v, &official()).unwrap()
    }

    #[test]
    fn uploader_fields_are_kept() {
        let d = detail(FULL);
        assert_eq!(d.files.len(), 6);
        assert_eq!(d.gguf.as_ref().unwrap().parameters, Some(7_241_732_096));
        let i = &d.info;
        assert_eq!(i.author.as_deref(), Some("example-quants"));
        assert_eq!(i.pipeline_tag.as_deref(), Some("text-generation"));
        assert_eq!(i.base_models, ["example-labs/Sample-7B"]);
        assert_eq!(i.languages, ["en", "fr", "zh-Hans"]);
        // "other" names itself; a relative link points at the file in the repository.
        assert_eq!(i.license.as_deref(), Some("example-model-license"));
        assert_eq!(
            i.license_url.as_deref(),
            Some(
                "https://huggingface.co/example-quants/Sample-7B-GGUF/blob/4444444444444444444444444444444444444444/LICENSE"
            )
        );
        // Known tags only, lower case, once each: no formats, regions or base-model tags.
        assert_eq!(i.tags, ["conversational", "code", "tool-use"]);
        assert_eq!(i.downloads, Some(123_456));
        assert_eq!(i.likes, Some(789));
        assert_eq!(i.last_modified.as_deref(), Some("2026-04-18T09:12:44.000Z"));
    }

    /// The base models come from the tags when the card names none.
    #[test]
    fn base_model_from_tags() {
        let mut v: serde_json::Value = serde_json::from_str(FULL).unwrap();
        v["cardData"].as_object_mut().unwrap().remove("base_model");
        v["tags"] = serde_json::json!([
            "base_model:finetune:example-labs/Sample-7B-Base",
            "base_model:example-labs/Sample-7B-Base",
            "base_model:quantized:not a repo"
        ]);
        let d = parse_repo_detail("example-quants/Sample-7B-GGUF", &v, &official()).unwrap();
        assert_eq!(d.info.base_models, ["example-labs/Sample-7B-Base"]);
        assert!(d.info.tags.is_empty());
    }

    /// Everything an uploader could use to mislead is made visible, capped or dropped.
    #[test]
    fn hostile_uploader_text_is_made_safe() {
        let d = detail(HOSTILE);
        let i = &d.info;
        // The right-to-left override is shown as its code, never applied.
        assert_eq!(i.author.as_deref(), Some("evil[U+202E]uploads"));
        // Markup in a task name or date is not a task name or a date.
        assert_eq!(i.pipeline_tag, None);
        assert_eq!(i.last_modified, None);
        // Only real repository ids; the zero-width space makes one invalid; five at most.
        assert_eq!(
            i.base_models,
            ["example-labs/Sample-7B", "a/b", "c/d", "e/f", "g/h"]
        );
        assert_eq!(i.languages, ["en", "multilingual"]);
        assert_eq!(i.tags, ["uncensored"]);
        // A licence name is one line of at most 300 characters; a javascript: link is not
        // a link.
        let l = i.license.as_deref().unwrap();
        assert!(
            (290..=UPLOADER_TEXT_CAP).contains(&l.chars().count()),
            "{l}"
        );
        assert!(
            l.starts_with("AAAA") && l.ends_with('…') && !l.contains('\n'),
            "{l}"
        );
        assert_eq!(i.license_url, None);
        assert_eq!(d.gated, Gated::Manual);
        // Nothing invisible survives anywhere.
        let all = serde_json::to_string(i).unwrap();
        for c in ['\u{202E}', '\u{200B}', '\n'] {
            assert!(!all.contains(c), "{c:?} in {all}");
        }
    }

    #[test]
    fn uploader_text_rules() {
        assert_eq!(uploader_text("  plain  ", 300), "plain");
        assert_eq!(uploader_text("a\n\tb\r\n  c", 300), "a b c");
        assert_eq!(uploader_text("abc\u{202E}def", 300), "abc[U+202E]def");
        assert_eq!(
            uploader_text("x\u{200B}\u{FEFF}y", 300),
            "x[U+200B][U+FEFF]y"
        );
        assert_eq!(uploader_text("\u{7}bell", 300), "[U+0007]bell");
        assert_eq!(
            uploader_text("<b>bold</b>", 300),
            "<b>bold</b>",
            "text, never markup"
        );
        assert_eq!(uploader_text("", 300), "");
        // Cut to the cap with an ellipsis, counted in characters.
        let long = "é".repeat(400);
        let cut = uploader_text(&long, 300);
        assert_eq!(cut.chars().count(), 300);
        assert!(cut.ends_with('…'));
        assert_eq!(uploader_text(&"a".repeat(300), 300), "a".repeat(300));
        // An escape is never cut in half.
        let cut = uploader_text(&format!("{}\u{202E}tail", "a".repeat(295)), 300);
        assert_eq!(cut, format!("{}…", "a".repeat(295)));
    }

    /// The rules of [`uploader_text`] as first written: every character expanded, then
    /// cut. The real one must give the same text without expanding past the cap.
    fn uploader_text_reference(text: &str, cap: usize) -> String {
        let mut pieces: Vec<String> = Vec::new();
        let mut space = false;
        for c in text.trim().chars() {
            if c.is_whitespace() {
                space = true;
                continue;
            }
            if space {
                pieces.push(" ".into());
                space = false;
            }
            if is_invisible_or_control(c) {
                pieces.push(format!("[U+{:04X}]", c as u32));
            } else {
                pieces.push(c.to_string());
            }
        }
        let len = |p: &String| p.chars().count();
        if pieces.iter().map(len).sum::<usize>() <= cap {
            return pieces.concat();
        }
        let mut out = String::new();
        let mut used = 0;
        for p in &pieces {
            let n = len(p);
            if used + n > cap.saturating_sub(1) {
                break;
            }
            out.push_str(p);
            used += n;
        }
        out.truncate(out.trim_end().len());
        out.push('…');
        out
    }

    /// Hostile uploader text of many megabytes (a licence name, a GGUF architecture)
    /// costs no more than its first few hundred characters, and the result is the same
    /// as the rules give.
    #[test]
    fn uploader_text_stops_at_the_cap() {
        let huge = "a".repeat(16 << 20);
        let invisible = "\u{200B}".repeat(4 << 20);
        for (text, cap) in [(&huge, 300), (&invisible, 300), (&huge, 64)] {
            let started = std::time::Instant::now();
            let cut = uploader_text(text, cap);
            let took = started.elapsed();
            assert!(
                took < std::time::Duration::from_millis(200),
                "took {took:?} for {} bytes",
                text.len()
            );
            assert!(cut.chars().count() <= cap && cut.ends_with('…'));
        }

        // The same text as the rules, on edge cases and on many made-up strings.
        let alphabet = [
            'a', ' ', '\n', '\t', 'é', '\u{202E}', '\u{200B}', '…', '\u{7}', '中',
        ];
        let mut cases: Vec<String> = vec![
            String::new(),
            "   ".into(),
            "a".into(),
            " a ".into(),
            "\u{202E}".into(),
            format!("{}\u{202E}", "a".repeat(287)),
            format!("{}\u{202E}", "a".repeat(288)),
            format!("{} b", "a".repeat(298)),
            format!("{} b", "a".repeat(299)),
            format!("{}  \n b", "a".repeat(297)),
        ];
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        for _ in 0..3000 {
            let mut s = String::new();
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let len = (seed >> 33) % 40;
            for _ in 0..len {
                seed = seed
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                s.push(alphabet[((seed >> 33) % alphabet.len() as u64) as usize]);
            }
            cases.push(s);
        }
        for text in &cases {
            for cap in [0, 1, 2, 3, 7, 8, 9, 10, 11, 12, 13, 20, 25, 300] {
                assert_eq!(
                    uploader_text(text, cap),
                    uploader_text_reference(text, cap),
                    "{text:?} at {cap}"
                );
            }
        }
    }

    #[test]
    fn licence_links() {
        assert_eq!(
            https_link(" https://example.com/licence ").as_deref(),
            Some("https://example.com/licence")
        );
        for bad in [
            "http://example.com/licence",
            "javascript:alert(1)",
            "https://user@example.com/x",
            "LICENSE",
            "",
        ] {
            assert_eq!(https_link(bad), None, "{bad}");
        }
        assert_eq!(
            https_link(&format!("https://example.com/{}", "a".repeat(600))),
            None
        );
        // Invisible characters in a link are percent-encoded, never kept as they are.
        let l = https_link("https://example.com/a\u{202E}b").unwrap();
        assert!(!l.contains('\u{202E}') && l.contains("%E2%80%AE"), "{l}");
    }

    #[test]
    fn page_urls() {
        let c = HfClient::new(HfConfig::default()).unwrap();
        assert_eq!(
            c.page_url("owner/name").as_deref(),
            Some("https://huggingface.co/owner/name")
        );
        assert_eq!(c.page_url("../etc"), None);
        assert_eq!(c.endpoint(), "https://huggingface.co/");
        let m = HfClient::new(HfConfig {
            endpoint: "https://mirror.example/hf".into(),
            ..HfConfig::default()
        })
        .unwrap();
        assert_eq!(
            m.page_url("owner/name").as_deref(),
            Some("https://mirror.example/owner/name")
        );
    }

    /// A one-request server: answers every request with `status`, `headers` and `body`,
    /// and sends the request line it saw.
    async fn answer_once(
        status: &'static str,
        headers: &'static str,
        body: String,
    ) -> (String, tokio::sync::oneshot::Receiver<String>) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let (r, mut w) = sock.into_split();
            let mut r = BufReader::new(r);
            let mut line = String::new();
            r.read_line(&mut line).await.unwrap();
            loop {
                let mut h = String::new();
                if r.read_line(&mut h).await.unwrap() == 0 || h == "\r\n" {
                    break;
                }
            }
            let _ = tx.send(line);
            let resp = format!(
                "HTTP/1.1 {status}\r\n{headers}content-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            w.write_all(resp.as_bytes()).await.unwrap();
            let _ = w.shutdown().await;
        });
        (format!("http://{addr}"), rx)
    }

    /// `repo` asks for the uploader's fields and reads them from the answer.
    #[tokio::test]
    async fn repo_asks_for_the_uploader_fields() {
        let (url, seen) = answer_once("200 OK", "", FULL.to_string()).await;
        let d = HfClient::for_test(&url)
            .repo("example-quants/Sample-7B-GGUF", None)
            .await
            .unwrap();
        assert_eq!(d.info.downloads, Some(123_456));
        let line = seen.await.unwrap();
        assert!(
            line.starts_with("GET /api/models/example-quants/Sample-7B-GGUF?"),
            "{line}"
        );
        for e in REPO_EXPAND.iter().chain(&["gguf"]) {
            assert!(
                line.contains(&format!("expand%5B%5D={e}&"))
                    || line.contains(&format!("expand%5B%5D={e} ")),
                "{e}: {line}"
            );
        }
    }

    /// The answers the comparison turns into "Hugging Face is busy" and "needs a token".
    #[tokio::test]
    async fn rate_limit_and_gated_answers() {
        let (url, _) = answer_once(
            "429 Too Many Requests",
            "ratelimit: \"api\";r=0;t=42\r\n",
            "{}".into(),
        )
        .await;
        let e = HfClient::for_test(&url)
            .repo("a/b", None)
            .await
            .unwrap_err();
        assert!(matches!(e, HfError::RateLimited(42)), "{e:?}");

        let (url, _) = answer_once(
            "401 Unauthorized",
            "x-error-code: GatedRepo\r\n",
            "{}".into(),
        )
        .await;
        let e = HfClient::for_test(&url)
            .fetch_head("a/b", "main", "m.gguf", 1024)
            .await
            .unwrap_err();
        assert!(matches!(e, HfError::NeedsToken), "{e:?}");

        // Nothing listening: offline.
        let e = HfClient::for_test("http://127.0.0.1:9")
            .repo("a/b", None)
            .await
            .unwrap_err();
        assert!(matches!(e, HfError::Network(_)), "{e:?}");
    }
}
