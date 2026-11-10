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
        // expand[] limits the answer to the listed fields, so ask for the rest too.
        for e in ["sha", "gated", "tags", "siblings", "cardData"] {
            url.query_pairs_mut().append_pair("expand[]", e);
        }
        let (v, _) = self.json(url).await?;
        parse_repo_detail(repo_id, &v)
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

fn parse_repo_detail(repo_id: &str, v: &serde_json::Value) -> Result<RepoDetail, HfError> {
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
    Ok(RepoDetail {
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
        let d = parse_repo_detail("Qwen/Qwen3-0.6B-GGUF", &v).unwrap();
        assert_eq!(d.gated, Gated::Manual);
        assert_eq!(d.license.as_deref(), Some("apache-2.0"));
        assert_eq!(d.files.len(), 2);
        assert_eq!(d.files[1].size, 639447744);
        assert_eq!(d.files[1].sha256.as_deref(), Some("ab".repeat(32).as_str()));
        assert!(d.template_tools && d.template_thinking);
        assert_eq!(d.gguf.unwrap().context_length, Some(40960));
        assert!(parse_repo_detail("a/b", &serde_json::json!({"sha": "short"})).is_err());
    }
}
