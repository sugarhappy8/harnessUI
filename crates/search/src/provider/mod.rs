//! Search services. Each one turns a question into a list of titles, addresses and
//! snippets; none of them reads the pages.
//!
//! - [`ProviderKind::DuckDuckGo`]: no key, the HTML endpoint. The default.
//! - [`ProviderKind::Brave`] and [`ProviderKind::Tavily`]: an API key.
//! - [`ProviderKind::Searxng`]: the user's own instance, by its address.
//!
//! The request goes to the service's fixed address, or to the address the user set for
//! SearXNG. That one may be on this computer or the local network: the user chose it, like
//! a model endpoint. Result pages are a different matter and are read only through the
//! guarded fetcher ([`crate::fetch`]).
//!
//! What a service sends back is untrusted. Each parser returns plain text only (markup in
//! titles and snippets is dropped), and [`clean`] keeps only `http` and `https` addresses.

mod brave;
mod duckduckgo;
mod searxng;
mod tavily;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use url::Url;

pub use brave::parse as parse_brave;
pub use duckduckgo::parse as parse_duckduckgo;
pub use searxng::{parse as parse_searxng, search_url as searxng_search_url};
pub use tavily::parse as parse_tavily;

/// One result of a search service.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Results asked of a service at most.
pub const MAX_RESULTS: usize = 10;
/// A question sent to a service at most, in characters.
pub const MAX_QUERY_CHARS: usize = 400;
/// A title kept at most, in characters.
pub const MAX_TITLE_CHARS: usize = 300;
/// A snippet kept at most, in characters.
pub const MAX_SNIPPET_CHARS: usize = 600;
/// An answer read from a service at most, in bytes.
pub const MAX_ANSWER_BYTES: usize = 2 * 1024 * 1024;
/// How long a service may take to answer.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// The search services.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    DuckDuckGo,
    Brave,
    Tavily,
    Searxng,
}

impl ProviderKind {
    /// The name shown to the user and the model.
    pub fn label(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "DuckDuckGo",
            Self::Brave => "Brave Search",
            Self::Tavily => "Tavily",
            Self::Searxng => "SearXNG",
        }
    }

    /// The name in stored settings.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DuckDuckGo => "duckduckgo",
            Self::Brave => "brave",
            Self::Tavily => "tavily",
            Self::Searxng => "searxng",
        }
    }

    /// Whether the service needs an API key.
    pub fn needs_key(self) -> bool {
        matches!(self, Self::Brave | Self::Tavily)
    }
}

/// Why a search service gave no results.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    #[error("no API key is saved")]
    NoKey,
    #[error("no address is set")]
    NoAddress,
    #[error("the API key was not accepted")]
    BadKey,
    #[error("too many searches")]
    RateLimited,
    #[error("the service refused the search")]
    Refused,
    #[error("the instance does not give results as JSON")]
    JsonOff,
    #[error("no answer in time")]
    Timeout,
    #[error("could not be reached: {0}")]
    Network(String),
    #[error("answered with status {0}")]
    Http(u16),
    #[error("the answer could not be read: {0}")]
    Unreadable(String),
}

impl ProviderError {
    /// What happened and what to do about it, in words for the user and the model.
    pub fn explain(&self, kind: ProviderKind) -> String {
        let name = kind.label();
        let other = "choose another search service in Settings → Search";
        match self {
            Self::NoKey => format!(
                "{name} needs an API key and none is saved. The user can add one in Settings → Search, or choose DuckDuckGo there, which needs no key."
            ),
            Self::NoAddress => "No SearXNG address is set. The user can enter the address of their instance in Settings → Search, or choose DuckDuckGo there.".to_string(),
            Self::BadKey => format!(
                "{name} did not accept the API key. The user can check the key in Settings → Search."
            ),
            Self::RateLimited => format!(
                "{name} is limiting searches right now. Wait a minute and try again, or {other}."
            ),
            Self::Refused => format!(
                "{name} refused the search: it turns automated searches away for a while. Try again later, or {other}."
            ),
            Self::JsonOff => format!(
                "This {name} instance does not give results as JSON. Switch on the json format in its settings (search, formats), or {other}."
            ),
            Self::Timeout => format!(
                "{name} did not answer within {} seconds. Check the internet connection and try again.",
                DEFAULT_TIMEOUT.as_secs()
            ),
            Self::Network(_) if kind == ProviderKind::Searxng => format!(
                "{name} could not be reached. Check that the instance is running and that its address in Settings → Search is right."
            ),
            Self::Network(_) => format!(
                "{name} could not be reached. Check the internet connection and try again."
            ),
            Self::Http(status) => format!(
                "{name} answered with an error ({status}). Try again later, or {other}."
            ),
            Self::Unreadable(_) => format!(
                "{name} sent an answer this app could not read. Try again later, or {other}."
            ),
        }
    }
}

/// What a provider is built from.
#[derive(Clone)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    /// The key for Brave or Tavily.
    pub api_key: Option<String>,
    /// The address of the user's SearXNG instance.
    pub searxng_url: Option<Url>,
    /// `<app name>/<version>`, and nothing about the user.
    pub user_agent: String,
    pub timeout: Duration,
}

impl std::fmt::Debug for ProviderConfig {
    /// Never prints the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("kind", &self.kind)
            .field("api_key", &self.api_key.as_ref().map(|_| "(set)"))
            .field("searxng_url", &self.searxng_url.as_ref().map(Url::as_str))
            .field("user_agent", &self.user_agent)
            .finish_non_exhaustive()
    }
}

impl ProviderConfig {
    /// A configuration with no key and no address.
    pub fn new(kind: ProviderKind, user_agent: impl Into<String>) -> Self {
        Self {
            kind,
            api_key: None,
            searxng_url: None,
            user_agent: user_agent.into(),
            timeout: DEFAULT_TIMEOUT,
        }
    }
}

/// A search service.
#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    /// Up to `max_results` results for `query`, in the service's order. The results are
    /// already cleaned ([`clean`]).
    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ProviderError>;
}

/// Builds the provider for a configuration. Fails when what it needs (a key, an address)
/// is missing, before any request is made.
pub fn build(config: ProviderConfig) -> Result<Arc<dyn Provider>, ProviderError> {
    let key = || {
        config
            .api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string)
            .ok_or(ProviderError::NoKey)
    };
    Ok(match config.kind {
        ProviderKind::DuckDuckGo => Arc::new(duckduckgo::DuckDuckGo {
            http: Http::new(&config.user_agent, config.timeout, false)?,
        }),
        ProviderKind::Brave => Arc::new(brave::Brave {
            key: key()?,
            http: Http::new(&config.user_agent, config.timeout, false)?,
        }),
        ProviderKind::Tavily => Arc::new(tavily::Tavily {
            key: key()?,
            http: Http::new(&config.user_agent, config.timeout, false)?,
        }),
        ProviderKind::Searxng => {
            let base = config
                .searxng_url
                .clone()
                .filter(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
                .ok_or(ProviderError::NoAddress)?;
            Arc::new(searxng::Searxng {
                http: Http::new(&config.user_agent, config.timeout, is_loopback_url(&base))?,
                base,
            })
        }
    })
}

/// Whether an address is on this computer (`localhost` or a loopback address).
pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Domain(domain)) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain.to_ascii_lowercase().ends_with(".localhost")
        }
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        None => false,
    }
}

/// What a non-success status of a service means. `None` for a success.
pub fn status_error(kind: ProviderKind, status: u16, body: &str) -> Option<ProviderError> {
    use ProviderError as E;
    use ProviderKind::{Brave, DuckDuckGo, Searxng, Tavily};
    Some(match (kind, status) {
        // DuckDuckGo answers a search it takes for automated with 202 and a challenge page.
        (DuckDuckGo, 202) => E::Refused,
        (_, 200..=299) => return None,
        (_, 429) => E::RateLimited,
        // Tavily: the plan's or the key's usage limit is used up.
        (Tavily, 432 | 433) => E::RateLimited,
        (Brave | Tavily, 401 | 403) => E::BadKey,
        // Brave reports a wrong key as a validation error with a JSON body.
        (Brave, 422) => match brave::parse(body) {
            Err(error) => error,
            Ok(_) => E::Unreadable("status 422".into()),
        },
        // A SearXNG instance that does not allow the JSON format answers 403.
        (Searxng, 403) => E::JsonOff,
        (DuckDuckGo | Searxng, 401 | 403) => E::Refused,
        (_, other) => E::Http(other),
    })
}

/// Makes results fit to use: only `http` and `https` addresses without a user name or
/// password, each address once (without its fragment), titles and snippets on one line,
/// without invisible characters and cut to their limits, at most `max` results. A result
/// without a title is shown under its address. Titles and snippets are plain text here:
/// a parser whose service sends markup takes it out itself.
pub fn clean(results: Vec<SearchResult>, max: usize) -> Vec<SearchResult> {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::new();
    for result in results {
        if kept.len() >= max {
            break;
        }
        let Ok(mut url) = Url::parse(result.url.trim()) else {
            continue;
        };
        if !matches!(url.scheme(), "http" | "https")
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            continue;
        }
        url.set_fragment(None);
        if !seen.insert(url.to_string()) {
            continue;
        }
        let mut title = cut(
            &crate::extract::plain_to_line(&result.title),
            MAX_TITLE_CHARS,
        );
        if title.is_empty() {
            let path = url.path().trim_end_matches('/');
            title = cut(
                &format!("{}{path}", crate::official::site_of(&url)),
                MAX_TITLE_CHARS,
            );
        }
        kept.push(SearchResult {
            title,
            url: url.to_string(),
            snippet: cut(
                &crate::extract::plain_to_line(&result.snippet),
                MAX_SNIPPET_CHARS,
            ),
        });
    }
    kept
}

/// `text` cut to `max` characters, with an ellipsis when something was cut.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The question as it is sent: one line, cut to [`MAX_QUERY_CHARS`].
pub fn clean_query(query: &str) -> String {
    crate::extract::plain_to_line(query)
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect::<String>()
        .trim()
        .to_string()
}

/// The HTTP side shared by the providers.
pub(crate) struct Http {
    client: reqwest::Client,
    timeout: Duration,
}

/// A service's answer: status and body text.
pub(crate) struct Reply {
    pub status: u16,
    pub body: String,
}

impl Http {
    /// A client for one service. `local` (a SearXNG instance on this computer): no proxy
    /// and no redirects, like the app's other loopback requests. Otherwise the system
    /// proxy is used, and a redirect is followed only on the same host and never from
    /// https to http.
    pub(crate) fn new(
        user_agent: &str,
        timeout: Duration,
        local: bool,
    ) -> Result<Self, ProviderError> {
        let mut builder = reqwest::Client::builder()
            .user_agent(user_agent)
            .connect_timeout(timeout)
            .timeout(timeout)
            .referer(false);
        builder = if local {
            builder
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
        } else {
            builder.redirect(reqwest::redirect::Policy::custom(|attempt| {
                let stays = attempt.previous().last().is_some_and(|from| {
                    from.host_str() == attempt.url().host_str()
                        && !(from.scheme() == "https" && attempt.url().scheme() != "https")
                });
                if stays && attempt.previous().len() <= 3 {
                    attempt.follow()
                } else {
                    attempt.stop()
                }
            }))
        };
        let client = builder
            .build()
            .map_err(|e| ProviderError::Network(e.without_url().to_string()))?;
        Ok(Self { client, timeout })
    }

    /// Sends one request and reads at most [`MAX_ANSWER_BYTES`] of the answer.
    pub(crate) async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<Reply, ProviderError> {
        let failed = |error: reqwest::Error| {
            if error.is_timeout() {
                ProviderError::Timeout
            } else {
                ProviderError::Network(error.without_url().to_string())
            }
        };
        let exchange = async {
            let mut response = request.send().await.map_err(failed)?;
            let status = response.status().as_u16();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(failed)? {
                let room = MAX_ANSWER_BYTES - body.len();
                if chunk.len() >= room {
                    body.extend_from_slice(&chunk[..room]);
                    break;
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Reply {
                status,
                body: String::from_utf8_lossy(&body).into_owned(),
            })
        };
        match tokio::time::timeout(self.timeout, exchange).await {
            Ok(reply) => reply,
            Err(_) => Err(ProviderError::Timeout),
        }
    }

    pub(crate) fn client(&self) -> &reqwest::Client {
        &self.client
    }
}

/// One search through a service's HTTP API: sends the request, turns a failure status
/// into its meaning, parses the answer and cleans the results.
pub(crate) async fn run(
    http: &Http,
    kind: ProviderKind,
    request: reqwest::RequestBuilder,
    parse: fn(&str) -> Result<Vec<SearchResult>, ProviderError>,
    max_results: usize,
) -> Result<Vec<SearchResult>, ProviderError> {
    let reply = http.send(request).await?;
    if let Some(error) = status_error(kind, reply.status, &reply.body) {
        return Err(error);
    }
    Ok(clean(
        parse(&reply.body)?,
        max_results.clamp(1, MAX_RESULTS),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(title: &str, url: &str, snippet: &str) -> SearchResult {
        SearchResult {
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
        }
    }

    #[test]
    fn kinds_have_stable_names() {
        for (kind, name) in [
            (ProviderKind::DuckDuckGo, "duckduckgo"),
            (ProviderKind::Brave, "brave"),
            (ProviderKind::Tavily, "tavily"),
            (ProviderKind::Searxng, "searxng"),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(serde_json::to_value(kind).unwrap(), serde_json::json!(name));
            assert_eq!(
                serde_json::from_value::<ProviderKind>(serde_json::json!(name)).unwrap(),
                kind
            );
        }
        assert_eq!(ProviderKind::default(), ProviderKind::DuckDuckGo);
        assert!(ProviderKind::Brave.needs_key() && ProviderKind::Tavily.needs_key());
        assert!(!ProviderKind::DuckDuckGo.needs_key() && !ProviderKind::Searxng.needs_key());
    }

    #[test]
    fn results_are_cleaned() {
        let long_title = "t".repeat(MAX_TITLE_CHARS + 50);
        let long_snippet = "s".repeat(MAX_SNIPPET_CHARS + 50);
        let cleaned = clean(
            vec![
                result(
                    " A  bold\n title ",
                    "https://a.example/x#part",
                    "One &\t two <tags stay text>",
                ),
                result(
                    "Same page, other fragment",
                    "https://a.example/x#other",
                    "dropped",
                ),
                result("ftp", "ftp://a.example/file", ""),
                result("script", "javascript:alert(1)", ""),
                result("file", "file:///etc/passwd", ""),
                result("not an address", "not a url", ""),
                result("with a password", "https://user:pw@b.example/", ""),
                result("", "https://c.example/some/page?x=1", "no title"),
                result(&long_title, "http://d.example/", &long_snippet),
                result(
                    "bidi \u{202E}title",
                    "https://e.example/",
                    "zero\u{200B}width",
                ),
            ],
            10,
        );
        assert_eq!(
            cleaned,
            vec![
                result(
                    "A bold title",
                    "https://a.example/x",
                    "One & two <tags stay text>"
                ),
                result(
                    "c.example/some/page",
                    "https://c.example/some/page?x=1",
                    "no title"
                ),
                result(
                    &format!("{}…", "t".repeat(MAX_TITLE_CHARS - 1)),
                    "http://d.example/",
                    &format!("{}…", "s".repeat(MAX_SNIPPET_CHARS - 1))
                ),
                result("bidi title", "https://e.example/", "zerowidth"),
            ]
        );
        // At most `max`, in the service's order.
        let many: Vec<SearchResult> = (0..30)
            .map(|i| result(&format!("r{i}"), &format!("https://x.example/{i}"), ""))
            .collect();
        let kept = clean(many, 3);
        assert_eq!(
            kept.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(),
            ["r0", "r1", "r2"]
        );
        assert!(clean(Vec::new(), 5).is_empty());
    }

    #[test]
    fn a_question_is_one_line_and_not_too_long() {
        assert_eq!(
            clean_query("  tokio\n spawn_blocking\t example "),
            "tokio spawn_blocking example"
        );
        assert_eq!(clean_query("a\u{202E}b\u{0000}c"), "abc");
        assert_eq!(
            clean_query(&"q".repeat(1000)).chars().count(),
            MAX_QUERY_CHARS
        );
        assert_eq!(clean_query(""), "");
    }

    #[test]
    fn statuses_say_what_went_wrong() {
        use ProviderError as E;
        use ProviderKind::*;
        for kind in [DuckDuckGo, Brave, Tavily, Searxng] {
            assert_eq!(status_error(kind, 200, ""), None);
            assert_eq!(status_error(kind, 429, ""), Some(E::RateLimited));
            assert_eq!(status_error(kind, 500, ""), Some(E::Http(500)));
            assert_eq!(status_error(kind, 503, ""), Some(E::Http(503)));
            assert_eq!(status_error(kind, 301, ""), Some(E::Http(301)));
            assert_eq!(status_error(kind, 404, ""), Some(E::Http(404)));
        }
        assert_eq!(status_error(Brave, 401, ""), Some(E::BadKey));
        assert_eq!(status_error(Brave, 403, ""), Some(E::BadKey));
        assert_eq!(
            status_error(
                Brave,
                422,
                include_str!("../../tests/fixtures/brave_error.json")
            ),
            Some(E::BadKey)
        );
        assert!(matches!(
            status_error(Brave, 422, "{}"),
            Some(E::Unreadable(_))
        ));
        assert_eq!(status_error(Tavily, 401, ""), Some(E::BadKey));
        assert_eq!(status_error(Tavily, 432, ""), Some(E::RateLimited));
        assert_eq!(status_error(Tavily, 433, ""), Some(E::RateLimited));
        // DuckDuckGo answers a search it takes for automated with 202 and a challenge.
        assert_eq!(status_error(DuckDuckGo, 202, ""), Some(E::Refused));
        assert_eq!(status_error(DuckDuckGo, 403, ""), Some(E::Refused));
        // A SearXNG instance without the JSON format answers 403.
        assert_eq!(status_error(Searxng, 403, ""), Some(E::JsonOff));
        assert_eq!(status_error(Searxng, 401, ""), Some(E::Refused));
    }

    /// Every failure says what happened and what to do, and names the service.
    #[test]
    fn failures_are_explained() {
        use ProviderError as E;
        let no_key = E::NoKey.explain(ProviderKind::Brave);
        assert!(
            no_key.contains("Brave Search") && no_key.contains("API key"),
            "{no_key}"
        );
        assert!(no_key.contains("Settings → Search"), "{no_key}");
        assert!(
            no_key.contains("DuckDuckGo"),
            "names the choice without a key: {no_key}"
        );
        let bad = E::BadKey.explain(ProviderKind::Tavily);
        assert!(
            bad.contains("Tavily") && bad.contains("Settings → Search"),
            "{bad}"
        );
        let limited = E::RateLimited.explain(ProviderKind::DuckDuckGo);
        assert!(
            limited.contains("DuckDuckGo") && limited.contains("again"),
            "{limited}"
        );
        let refused = E::Refused.explain(ProviderKind::DuckDuckGo);
        assert!(
            refused.contains("another") && refused.contains("Settings → Search"),
            "{refused}"
        );
        let timeout = E::Timeout.explain(ProviderKind::Searxng);
        assert!(
            timeout.contains("SearXNG") && timeout.contains("10 seconds"),
            "{timeout}"
        );
        let json = E::JsonOff.explain(ProviderKind::Searxng);
        assert!(json.contains("JSON") && json.contains("formats"), "{json}");
        let none = E::NoAddress.explain(ProviderKind::Searxng);
        assert!(
            none.contains("address") && none.contains("Settings → Search"),
            "{none}"
        );
        let net = E::Network("connection refused".into()).explain(ProviderKind::Searxng);
        assert!(
            net.contains("could not be reached") && net.contains("address"),
            "{net}"
        );
        let net = E::Network("dns".into()).explain(ProviderKind::Brave);
        assert!(net.contains("internet connection"), "{net}");
        let http = E::Http(503).explain(ProviderKind::Brave);
        assert!(http.contains("503"), "{http}");
        let unreadable = E::Unreadable("expected value".into()).explain(ProviderKind::Tavily);
        assert!(unreadable.contains("Tavily"), "{unreadable}");
        for e in [
            E::NoKey,
            E::BadKey,
            E::RateLimited,
            E::Refused,
            E::Timeout,
            E::Http(500),
        ] {
            for kind in [ProviderKind::DuckDuckGo, ProviderKind::Brave] {
                let text = e.explain(kind);
                assert!(!text.contains('!') && text.ends_with('.'), "{text}");
            }
        }
    }

    #[test]
    fn a_provider_is_not_built_without_what_it_needs() {
        let config = |kind| ProviderConfig::new(kind, "app/1.0");
        assert!(build(config(ProviderKind::DuckDuckGo)).is_ok());
        assert_eq!(
            build(config(ProviderKind::Brave)).err(),
            Some(ProviderError::NoKey)
        );
        assert_eq!(
            build(config(ProviderKind::Tavily)).err(),
            Some(ProviderError::NoKey)
        );
        assert_eq!(
            build(config(ProviderKind::Searxng)).err(),
            Some(ProviderError::NoAddress)
        );
        let mut blank = config(ProviderKind::Brave);
        blank.api_key = Some("   ".into());
        assert_eq!(build(blank).err(), Some(ProviderError::NoKey));
        let mut brave = config(ProviderKind::Brave);
        brave.api_key = Some("BSA-test".into());
        assert_eq!(build(brave).unwrap().kind(), ProviderKind::Brave);
        let mut searx = config(ProviderKind::Searxng);
        searx.searxng_url = Some(Url::parse("http://127.0.0.1:8888").unwrap());
        assert_eq!(build(searx).unwrap().kind(), ProviderKind::Searxng);
        let mut bad = config(ProviderKind::Searxng);
        bad.searxng_url = Some(Url::parse("ftp://example.org").unwrap());
        assert_eq!(build(bad).err(), Some(ProviderError::NoAddress));
    }

    #[test]
    fn the_key_is_never_printed() {
        let mut config = ProviderConfig::new(ProviderKind::Brave, "app/1.0");
        config.api_key = Some("BSA-SUPERSECRET".into());
        let shown = format!("{config:?}");
        assert!(!shown.contains("SUPERSECRET"), "{shown}");
        assert!(shown.contains("(set)"), "{shown}");
    }

    #[test]
    fn addresses_on_this_computer() {
        let local = |u: &str| is_loopback_url(&Url::parse(u).unwrap());
        assert!(local("http://127.0.0.1:8888/"));
        assert!(local("http://localhost:8080/searxng"));
        assert!(local("http://[::1]:8080/"));
        assert!(local("http://127.8.9.1/"));
        assert!(!local("http://192.168.1.10:8080/"));
        assert!(!local("https://searx.example.org/"));
        assert!(!local("http://localhost.example.org/"));
    }
}
