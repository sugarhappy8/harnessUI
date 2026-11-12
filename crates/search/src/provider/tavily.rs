//! Tavily's search API: a key as a bearer token, JSON back.

use async_trait::async_trait;

use super::{Http, Provider, ProviderError, ProviderKind, SearchResult};

/// Where the search goes.
pub(crate) const ENDPOINT: &str = "https://api.tavily.com/search";

/// What is sent: the question and how many results. No generated answer, no page
/// contents and no images are asked for: this app reads the pages itself.
pub(crate) fn request_body(query: &str, max_results: usize) -> serde_json::Value {
    serde_json::json!({
        "query": query,
        "max_results": max_results.clamp(1, super::MAX_RESULTS),
        "search_depth": "basic",
        "include_answer": false,
        "include_raw_content": false,
        "include_images": false,
    })
}

/// The results in Tavily's answer.
pub fn parse(json: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ProviderError::Unreadable(e.to_string()))?;
    if !root.is_object() {
        return Err(ProviderError::Unreadable("not a JSON object".into()));
    }
    // An error that came with a success status: {"detail": {"error": "…"}}.
    if root.get("results").is_none()
        && let Some(detail) = root.get("detail")
    {
        let message = detail.to_string().to_lowercase();
        return Err(
            if message.contains("unauthorized") || message.contains("api key") {
                ProviderError::BadKey
            } else {
                ProviderError::Unreadable("an error instead of results".into())
            },
        );
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

pub(crate) struct Tavily {
    pub(crate) http: Http,
    pub(crate) key: String,
}

#[async_trait]
impl Provider for Tavily {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Tavily
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let request = self
            .http
            .client()
            .post(ENDPOINT)
            .bearer_auth(&self.key)
            .json(&request_body(query, max_results));
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
            parse(include_str!("../../tests/fixtures/tavily.json")).unwrap(),
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
                    "A Tokio task is an asynchronous green thread. They are created by passing an async block to tokio::spawn."
                ),
                (
                    "example.org/untitled",
                    "https://example.org/untitled",
                    "A result without a title is shown under its address."
                ),
            ]
        );
    }

    #[test]
    fn odd_answers() {
        assert_eq!(parse(r#"{"query":"x","results":[]}"#), Ok(Vec::new()));
        assert_eq!(parse(r#"{"query":"x"}"#), Ok(Vec::new()));
        assert!(matches!(parse("<html>"), Err(ProviderError::Unreadable(_))));
        let detail = r#"{"detail":{"error":"Unauthorized: missing or invalid API key."}}"#;
        assert_eq!(parse(detail), Err(ProviderError::BadKey));
    }

    #[test]
    fn the_request_asks_only_for_results() {
        let body = request_body("tokio spawn_blocking", 6);
        assert_eq!(body["query"], "tokio spawn_blocking");
        assert_eq!(body["max_results"], 6);
        assert_eq!(body["include_answer"], false);
        assert_eq!(body["include_raw_content"], false);
        assert_eq!(body["include_images"], false);
        // The key travels in a header, never in the body.
        assert!(body.get("api_key").is_none());
    }
}
