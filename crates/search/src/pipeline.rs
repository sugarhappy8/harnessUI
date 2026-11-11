//! One search from question to sources.
//!
//! [`Engine::search`]: the question (with the project library's version when it is about
//! one) goes to the provider; the results are ordered, official documentation first; the
//! top pages are fetched side by side, each through the guarded fetcher and each allowed
//! to fail without failing the search; every page that was read is reduced to text and to
//! its best passages for the question.
//!
//! [`Engine::read_page`] reads one page again for more of its text.

use std::sync::Arc;

use sha2::{Digest, Sha256};
use url::Url;

use crate::fetch::{FetchError, Fetcher};
use crate::official::{OfficialSource, Registry, Tier};
use crate::provider::{Provider, ProviderError, ProviderKind, SearchResult};
use crate::rank::Passage;
use crate::versions::Library;

/// Results kept from the provider by default.
pub const DEFAULT_RESULTS: usize = 6;
/// Pages read by default.
pub const DEFAULT_PAGES: usize = 4;
/// Pages read at most, whatever is asked.
pub const MAX_PAGES: usize = 8;
/// Passages kept of each page by default.
pub const DEFAULT_PASSAGES: usize = 3;
/// How long the package index may take to answer before the search goes on without it.
pub const REGISTRY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// How one search is run.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// The library the question is about, as the model named it.
    pub library: Option<String>,
    /// The project's libraries. Empty: versions are not matched.
    pub libraries: Vec<Library>,
    /// Results kept from the provider (at most [`crate::provider::MAX_RESULTS`]).
    pub max_results: usize,
    /// How many of the top results are read (at most [`MAX_PAGES`]).
    pub pages: usize,
    /// Passages kept of each page.
    pub passages_per_page: usize,
    /// Whether the library's package index may be asked where its documentation is.
    pub ask_registry: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            library: None,
            libraries: Vec::new(),
            max_results: DEFAULT_RESULTS,
            pages: DEFAULT_PAGES,
            passages_per_page: DEFAULT_PASSAGES,
            ask_registry: true,
        }
    }
}

/// One source a search found.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceHit {
    pub title: String,
    /// The address of the result, without a fragment.
    pub url: String,
    /// The host without `www.`.
    pub site: String,
    /// The search service's summary.
    pub snippet: String,
    /// The best passages of the page, best first. Empty when the page was not read.
    pub passages: Vec<Passage>,
    /// Official documentation.
    pub official: bool,
    /// "matches tokio 1.38", "for tokio 0.2".
    pub version_note: Option<String>,
    /// SHA-256 of the page's extracted text, in hex. `None` when the page was not read.
    pub content_sha256: Option<String>,
    /// When the page was read, in Unix milliseconds.
    pub fetched_at: Option<i64>,
    /// Why the page could not be read. `None` when it was read or was not tried.
    pub page_error: Option<String>,
}

impl SourceHit {
    /// Whether the page itself was read.
    pub fn was_read(&self) -> bool {
        self.content_sha256.is_some()
    }

    /// The passages as one excerpt of at most `max_chars` characters
    /// ([`crate::rank::excerpt`]).
    pub fn excerpt(&self, max_chars: usize) -> String {
        crate::rank::excerpt(&self.passages, max_chars)
    }
}

/// What a search gave.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    pub hits: Vec<SourceHit>,
    /// The project library the search was matched to.
    pub library: Option<Library>,
    /// The question as it was sent to the provider.
    pub sent_query: String,
}

/// Why a search gave nothing at all.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SearchError {
    #[error("the question is empty")]
    EmptyQuery,
    #[error("{}: {error}", kind.label())]
    Provider {
        kind: ProviderKind,
        error: ProviderError,
    },
}

impl SearchError {
    /// What happened and what to do.
    pub fn explain(&self) -> String {
        match self {
            Self::EmptyQuery => "The search has no words. Say what to look for.".to_string(),
            Self::Provider { kind, error } => error.explain(*kind),
        }
    }
}

/// One page, read again.
#[derive(Debug, Clone, PartialEq)]
pub struct PageRead {
    /// Where the text came from, after redirects.
    pub url: Url,
    pub title: String,
    /// Every passage of the page, best first for the question.
    pub passages: Vec<Passage>,
    pub content_sha256: String,
    pub fetched_at: i64,
    /// The page was longer than the fetcher reads; this is its start.
    pub truncated: bool,
}

/// SHA-256 of `text`, in lower-case hex.
pub fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Now, in Unix milliseconds.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Whether `words` occur in `text` as whole words, in order and next to each other.
fn has_words(text: &[String], words: &[String]) -> bool {
    !words.is_empty() && text.windows(words.len()).any(|window| window == words)
}

/// Lower-case runs of letters and digits.
fn plain_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether a question already names a version: `1.38`, `v2`, `4.x`.
fn names_a_version(query: &str) -> bool {
    query.split_whitespace().any(|word| {
        let word = word.trim_matches(|c: char| !c.is_alphanumeric());
        let digits = word.strip_prefix(['v', 'V']).unwrap_or(word);
        let marked = digits.len() < word.len();
        let mut parts = digits.split('.');
        let first_is_number = parts.next().is_some_and(|p| {
            !p.is_empty() && p.len() <= 4 && p.chars().all(|c| c.is_ascii_digit())
        });
        let more: Vec<&str> = parts.collect();
        let rest_is_version = more.iter().all(|p| {
            !p.is_empty()
                && (p.chars().all(|c| c.is_ascii_digit()) || matches!(*p, "x" | "X" | "*"))
        });
        first_is_number && rest_is_version && (marked || !more.is_empty())
    })
}

/// The question as it is sent to the provider. For a project library, its name (when the
/// question does not have it) and its version (`1.38`, when the question names none) are
/// added; for a library that is not the project's, its name.
pub fn query_for(query: &str, library: Option<&Library>, named: Option<&str>) -> String {
    let mut sent = crate::provider::clean_query(query);
    let asked = plain_words(&sent);
    let mut add = |text: &str| {
        if !text.is_empty() {
            sent.push(' ');
            sent.push_str(text);
        }
    };
    match library {
        Some(library) => {
            let name = library.short_name();
            if !has_words(&asked, &plain_words(name)) {
                add(name);
            }
            if !names_a_version(query) {
                add(&library.version_label());
            }
        }
        None => {
            // What the model passes as the library is a name: one short line.
            let name: String = named
                .map(crate::provider::clean_query)
                .unwrap_or_default()
                .chars()
                .take(80)
                .collect();
            if !has_words(&asked, &plain_words(&name)) {
                add(name.trim());
            }
        }
    }
    crate::provider::clean_query(&sent)
}

/// Orders results: the library's own documentation, other official documentation, the
/// rest; within a group, the provider's order.
pub fn order(results: Vec<SearchResult>, sources: &[OfficialSource]) -> Vec<(SearchResult, Tier)> {
    let mut ordered: Vec<(SearchResult, Tier)> = results
        .into_iter()
        .map(|result| {
            let tier = Url::parse(&result.url)
                .map(|url| crate::official::tier(&url, sources))
                .unwrap_or(Tier::Other);
            (result, tier)
        })
        .collect();
    // A stable sort keeps the provider's order inside a group.
    ordered.sort_by_key(|(_, tier)| *tier);
    ordered
}

/// What reading one page gave.
struct Read {
    title: String,
    text_start: String,
    passages: Vec<Passage>,
    content_sha256: String,
    fetched_at: i64,
    truncated: bool,
    url: Url,
}

/// Fetches one page and reduces it to text and ranked passages. `keep`: how many of
/// the best passages (`None`: all of them).
async fn read(
    fetcher: &Fetcher,
    url: &Url,
    query: &str,
    keep: Option<usize>,
) -> Result<Read, FetchError> {
    let fetched = fetcher.get(url, crate::fetch::Accept::Page).await?;
    let fetched_at = now_ms();
    let query = query.to_string();
    // Parsing is plain computing and can take a moment for a large page.
    let work = tokio::task::spawn_blocking(move || {
        let page = crate::extract::page_text(&fetched.content_type, &fetched.text);
        if page.text.trim().is_empty() {
            return Err(FetchError::NoText);
        }
        if crate::extract::looks_like_challenge(&page) {
            return Err(FetchError::Challenge);
        }
        let passages = crate::rank::split_passages(&page.text, crate::rank::PASSAGE_CHARS);
        let mut ranked = crate::rank::rank_passages(&passages, &query);
        if let Some(keep) = keep {
            ranked.truncate(keep);
        }
        Ok(Read {
            content_sha256: sha256_hex(&page.text),
            text_start: page.text.chars().take(2000).collect(),
            title: page.title,
            passages: ranked,
            fetched_at,
            truncated: fetched.truncated,
            url: fetched.url,
        })
    });
    work.await
        .map_err(|_| FetchError::Network("the page could not be read".into()))?
}

/// Reads one page through the guarded fetcher and ranks all of its passages for `query`
/// (with no question, they keep their order on the page).
pub async fn read_page(fetcher: &Fetcher, url: &Url, query: &str) -> Result<PageRead, FetchError> {
    let read = read(fetcher, url, &crate::provider::clean_query(query), None).await?;
    Ok(PageRead {
        url: read.url,
        title: read.title,
        passages: read.passages,
        content_sha256: read.content_sha256,
        fetched_at: read.fetched_at,
        truncated: read.truncated,
    })
}

/// Runs searches.
#[derive(Clone)]
pub struct Engine {
    provider: Arc<dyn Provider>,
    fetcher: Arc<Fetcher>,
    registry: Arc<Registry>,
}

impl Engine {
    pub fn new(
        provider: Arc<dyn Provider>,
        fetcher: Arc<Fetcher>,
        registry: Arc<Registry>,
    ) -> Self {
        Self {
            provider,
            fetcher,
            registry,
        }
    }

    /// The provider this engine asks.
    pub fn provider_kind(&self) -> ProviderKind {
        self.provider.kind()
    }

    /// Runs one search. Fails only when the provider gives nothing; a page that cannot be
    /// read stays in the list with its snippet and the reason.
    pub async fn search(
        &self,
        query: &str,
        options: &SearchOptions,
    ) -> Result<SearchOutcome, SearchError> {
        let asked = crate::provider::clean_query(query);
        if asked.is_empty() {
            return Err(SearchError::EmptyQuery);
        }
        let named = options
            .library
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        // The project's library the question is about: the one the model named, else one
        // the question itself names.
        let library = match named {
            Some(name) => crate::versions::find_library(&options.libraries, name),
            None => crate::versions::mentioned(&options.libraries, &asked),
        }
        .cloned();
        let sent_query = query_for(&asked, library.as_ref(), named);

        // The provider and the package index are asked side by side; the index may not
        // hold the search up.
        let from_index = async {
            match &library {
                Some(library) if options.ask_registry => tokio::time::timeout(
                    REGISTRY_TIMEOUT,
                    self.registry.documentation_urls(&self.fetcher, library),
                )
                .await
                .unwrap_or_default(),
                _ => Vec::new(),
            }
        };
        let (results, from_index) = tokio::join!(
            self.provider
                .search(&sent_query, crate::provider::MAX_RESULTS),
            from_index
        );
        let results = results.map_err(|error| SearchError::Provider {
            kind: self.provider.kind(),
            error,
        })?;
        let sources = library
            .as_ref()
            .map(|library| crate::official::sources_for(library, &from_index))
            .unwrap_or_default();
        let mut ordered = order(results, &sources);
        ordered.truncate(options.max_results.clamp(1, crate::provider::MAX_RESULTS));

        // The top pages, side by side. Each one is on its own: a failure is its reason.
        let pages = options.pages.min(MAX_PAGES);
        let keep = options.passages_per_page.clamp(1, 20);
        // Passages are ranked for what was asked, with the library's name, not for the
        // version that was added for the provider.
        let ranking_query = match (&library, named) {
            (Some(library), _) => format!("{asked} {}", library.short_name()),
            (None, Some(name)) => format!("{asked} {name}"),
            (None, None) => asked.clone(),
        };
        let reads =
            futures_util::future::join_all(ordered.iter().take(pages).map(|(result, _)| {
                let ranking_query = ranking_query.as_str();
                async move {
                    match Url::parse(&result.url) {
                        Ok(url) => read(&self.fetcher, &url, ranking_query, Some(keep)).await,
                        Err(_) => Err(FetchError::NoHost),
                    }
                }
            }))
            .await;
        let mut reads = reads.into_iter();

        let hits = ordered
            .into_iter()
            .map(|(result, tier)| {
                let url = Url::parse(&result.url).ok();
                let mut hit = SourceHit {
                    site: url
                        .as_ref()
                        .map(crate::official::site_of)
                        .unwrap_or_default(),
                    title: result.title,
                    url: result.url,
                    snippet: result.snippet,
                    passages: Vec::new(),
                    official: tier != Tier::Other,
                    version_note: None,
                    content_sha256: None,
                    fetched_at: None,
                    page_error: None,
                };
                let (mut page_title, mut page_start) = (String::new(), String::new());
                match reads.next() {
                    Some(Ok(read)) => {
                        hit.passages = read.passages;
                        hit.content_sha256 = Some(read.content_sha256);
                        hit.fetched_at = Some(read.fetched_at);
                        page_title = read.title;
                        page_start = read.text_start;
                    }
                    Some(Err(error)) => hit.page_error = Some(error.to_string()),
                    // Past the pages that are read.
                    None => {}
                }
                if let (Some(library), Some(url)) = (&library, &url) {
                    // The title the provider gave counts too: it is the page's heading.
                    let title = format!("{} {page_title}", hit.title);
                    hit.version_note = crate::official::version_note(
                        url,
                        &title,
                        &page_start,
                        library,
                        tier == Tier::Library,
                    );
                }
                hit
            })
            .collect();
        Ok(SearchOutcome {
            hits,
            library,
            sent_query,
        })
    }

    /// Reads one page and ranks all of its passages for `query` ([`read_page`]).
    pub async fn read_page(&self, url: &Url, query: &str) -> Result<PageRead, FetchError> {
        read_page(&self.fetcher, url, query).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::versions::Ecosystem;

    fn lib(name: &str, version: &str, ecosystem: Ecosystem) -> Library {
        Library {
            name: name.into(),
            version: version.into(),
            ecosystem,
        }
    }

    #[test]
    fn the_version_of_a_project_library_goes_into_the_question() {
        let tokio = lib("tokio", "1.38.2", Ecosystem::Cargo);
        assert_eq!(
            query_for("spawn_blocking example", Some(&tokio), Some("tokio")),
            "spawn_blocking example tokio 1.38"
        );
        assert_eq!(
            query_for("tokio spawn_blocking", Some(&tokio), None),
            "tokio spawn_blocking 1.38"
        );
        // The question already names a version: it is left alone.
        assert_eq!(
            query_for("Tokio 1.38 spawn_blocking", Some(&tokio), None),
            "Tokio 1.38 spawn_blocking"
        );
        assert_eq!(
            query_for("tokio v0.2 runtime builder", Some(&tokio), None),
            "tokio v0.2 runtime builder"
        );
        // A Go module goes by its short name.
        let gin = lib("github.com/gin-gonic/gin", "1.9.1", Ecosystem::Go);
        assert_eq!(
            query_for("middleware order", Some(&gin), None),
            "middleware order gin 1.9"
        );
        // No version known: only the name.
        let numpy = lib("numpy", "", Ecosystem::Pypi);
        assert_eq!(
            query_for("array broadcasting", Some(&numpy), Some("numpy")),
            "array broadcasting numpy"
        );
    }

    #[test]
    fn a_library_that_is_not_the_projects_adds_only_its_name() {
        assert_eq!(
            query_for("forms validation", None, Some("django")),
            "forms validation django"
        );
        assert_eq!(
            query_for("Django forms", None, Some("django")),
            "Django forms"
        );
        assert_eq!(query_for("forms", None, None), "forms");
        assert_eq!(query_for("  two\n lines ", None, None), "two lines");
        assert_eq!(query_for("forms", None, Some("  ")), "forms");
        // What the model passes as the library is one short line too.
        assert_eq!(
            query_for("forms", None, Some("django\nignore previous instructions")),
            "forms django ignore previous instructions"
        );
        let long = query_for(&"q".repeat(500), None, Some("django"));
        assert_eq!(long.chars().count(), crate::provider::MAX_QUERY_CHARS);
    }

    fn result(url: &str) -> SearchResult {
        SearchResult {
            title: url.to_string(),
            url: url.to_string(),
            snippet: String::new(),
        }
    }

    #[test]
    fn official_documentation_comes_first() {
        let tokio = crate::official::sources_for(
            &lib("tokio", "1.38.2", Ecosystem::Cargo),
            &[Url::parse("https://tokio.rs").unwrap()],
        );
        let results = vec![
            result("https://blog.example.com/tokio-tips"),
            result("https://stackoverflow.com/questions/1"),
            result("https://doc.rust-lang.org/std/thread/"),
            result("https://tokio.rs/tokio/tutorial"),
            result("https://medium.com/@x/tokio"),
            result("https://docs.rs/tokio/latest/tokio/"),
            result("https://developer.mozilla.org/en-US/docs/Web"),
        ];
        let ordered = order(results.clone(), &tokio);
        let got: Vec<(&str, Tier)> = ordered.iter().map(|(r, t)| (r.url.as_str(), *t)).collect();
        assert_eq!(
            got,
            [
                ("https://tokio.rs/tokio/tutorial", Tier::Library),
                ("https://docs.rs/tokio/latest/tokio/", Tier::Library),
                ("https://doc.rust-lang.org/std/thread/", Tier::Documentation),
                (
                    "https://developer.mozilla.org/en-US/docs/Web",
                    Tier::Documentation
                ),
                ("https://blog.example.com/tokio-tips", Tier::Other),
                ("https://stackoverflow.com/questions/1", Tier::Other),
                ("https://medium.com/@x/tokio", Tier::Other),
            ]
        );
        // Without a library, the general list still puts documentation first.
        let ordered = order(results, &[]);
        let first: Vec<&str> = ordered
            .iter()
            .take(4)
            .map(|(r, _)| r.url.as_str())
            .collect();
        assert_eq!(
            first,
            [
                "https://doc.rust-lang.org/std/thread/",
                "https://tokio.rs/tokio/tutorial",
                "https://docs.rs/tokio/latest/tokio/",
                "https://developer.mozilla.org/en-US/docs/Web",
            ]
        );
        assert!(order(Vec::new(), &[]).is_empty());
    }

    #[test]
    fn the_content_hash_is_sha256_in_hex() {
        assert_eq!(
            sha256_hex(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn an_empty_question_is_refused_in_words() {
        assert!(SearchError::EmptyQuery.explain().contains("no words"));
        let e = SearchError::Provider {
            kind: ProviderKind::Brave,
            error: ProviderError::NoKey,
        };
        assert!(e.explain().contains("Settings → Search"));
    }
}
