//! The user's own SearXNG instance, through its JSON API (`/search?format=json`).

use async_trait::async_trait;
use url::Url;

use super::{Http, Provider, ProviderError, ProviderKind, SearchResult};

/// The address a search is sent to: `<instance>/search?q=…&format=json`, whatever the
/// instance's address ends with.
pub fn search_url(base: &Url, query: &str) -> Url {
    let mut url = base.clone();
    url.set_fragment(None);
    url.set_query(None);
    if let Ok(mut segments) = url.path_segments_mut() {
        segments.pop_if_empty().push("search");
    }
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("format", "json");
    url
}

/// The results in a SearXNG answer.
pub fn parse(json: &str) -> Result<Vec<SearchResult>, ProviderError> {
    // The HTML results page instead of JSON: the instance does not allow the format.
    if json.trim_start().starts_with('<') {
        return Err(ProviderError::JsonOff);
    }
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ProviderError::Unreadable(e.to_string()))?;
    if !root.is_object() {
        return Err(ProviderError::Unreadable("not a JSON object".into()));
    }
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let results = root
        .get("results")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("url").is_some_and(serde_json::Value::is_string))
                .map(|item| SearchResult {
                    title: text(item, "title"),
                    url: text(item, "url"),
                    snippet: text(item, "content"),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(results)
}

pub(crate) struct Searxng {
    pub(crate) http: Http,
    pub(crate) base: Url,
}

#[async_trait]
impl Provider for Searxng {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Searxng
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let request = self
            .http
            .client()
            .get(search_url(&self.base, query))
            .header(reqwest::header::ACCEPT, "application/json");
        super::run(&self.http, self.kind(), request, parse, max_results).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::clean;

    #[test]
    fn an_answer_with_results() {
        let results = clean(
            parse(include_str!("../../tests/fixtures/searxng.json")).unwrap(),
            10,
        );
        let got: Vec<(&str, &str, &str)> = results
            .iter()
            .map(|r| (r.title.as_str(), r.url.as_str(), r.snippet.as_str()))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "spawn_blocking in tokio::task - Rust",
                    "https://docs.rs/tokio/latest/tokio/task/fn.spawn_blocking.html",
                    "Runs the provided closure on a thread where blocking is acceptable. In general, issuing a blocking call or performing a lot of compute in a future without yielding is problematic."
                ),
                (
                    "Spawning | Tokio - An asynchronous Rust runtime",
                    "https://tokio.rs/tokio/tutorial/spawning",
                    "A Tokio task is an asynchronous green thread."
                ),
                (
                    "A video result without text",
                    "https://www.youtube.com/watch?v=abc",
                    ""
                ),
            ]
        );
    }

    #[test]
    fn odd_answers() {
        assert_eq!(parse(r#"{"query":"x","results":[]}"#), Ok(Vec::new()));
        assert_eq!(parse(r#"{"query":"x"}"#), Ok(Vec::new()));
        // The HTML results page instead of JSON: the format is not switched on.
        assert_eq!(parse("<!DOCTYPE html><html>"), Err(ProviderError::JsonOff));
        assert!(matches!(
            parse("{ broken"),
            Err(ProviderError::Unreadable(_))
        ));
    }

    #[test]
    fn the_search_address() {
        let at = |base: &str| {
            search_url(&Url::parse(base).unwrap(), "tokio spawn_blocking & more").to_string()
        };
        assert_eq!(
            at("http://127.0.0.1:8888"),
            "http://127.0.0.1:8888/search?q=tokio+spawn_blocking+%26+more&format=json"
        );
        assert_eq!(
            at("https://example.org/searxng/"),
            "https://example.org/searxng/search?q=tokio+spawn_blocking+%26+more&format=json"
        );
        assert_eq!(
            at("https://example.org/searxng"),
            "https://example.org/searxng/search?q=tokio+spawn_blocking+%26+more&format=json"
        );
        // Whatever the saved address carries besides its path is not sent on.
        assert_eq!(
            at("https://example.org/?token=abc#frag"),
            "https://example.org/search?q=tokio+spawn_blocking+%26+more&format=json"
        );
    }
}
