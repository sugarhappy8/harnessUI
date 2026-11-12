//! Brave Search API: a key in the `X-Subscription-Token` header, JSON back.

use async_trait::async_trait;

use super::{Http, Provider, ProviderError, ProviderKind, SearchResult};

/// Where the search goes.
pub(crate) const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

/// The web results in Brave's answer. An error answer (Brave sends those as JSON too)
/// becomes the matching [`ProviderError`].
pub fn parse(json: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let root: serde_json::Value =
        serde_json::from_str(json).map_err(|e| ProviderError::Unreadable(e.to_string()))?;
    if !root.is_object() {
        return Err(ProviderError::Unreadable("not a JSON object".into()));
    }
    if root.get("type").and_then(serde_json::Value::as_str) == Some("ErrorResponse") {
        let code = root
            .pointer("/error/code")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        return Err(match code {
            "SUBSCRIPTION_TOKEN_INVALID" | "SUBSCRIPTION_NOT_FOUND" | "RESOURCE_NOT_ALLOWED" => {
                ProviderError::BadKey
            }
            "RATE_LIMITED" | "QUOTA_LIMITED" => ProviderError::RateLimited,
            other => ProviderError::Unreadable(format!("error {other}")),
        });
    }
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let results = root
        .pointer("/web/results")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| item.get("url").is_some_and(serde_json::Value::is_string))
                // Brave marks the words of the question with tags and writes character
                // references; both are taken out here.
                .map(|item| SearchResult {
                    title: crate::extract::fragment_to_line(&text(item, "title")),
                    url: text(item, "url"),
                    snippet: crate::extract::fragment_to_line(&text(item, "description")),
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(results)
}

pub(crate) struct Brave {
    pub(crate) http: Http,
    pub(crate) key: String,
}

#[async_trait]
impl Provider for Brave {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Brave
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let count = max_results.clamp(1, super::MAX_RESULTS).to_string();
        let request = self
            .http
            .client()
            .get(ENDPOINT)
            .query(&[("q", query), ("count", count.as_str())])
            .header(reqwest::header::ACCEPT, "application/json")
            .header("X-Subscription-Token", &self.key);
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
            parse(include_str!("../../tests/fixtures/brave.json")).unwrap(),
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
                    "Runs the provided closure on a thread where blocking is acceptable. In general, issuing a blocking call & waiting on it is problematic."
                ),
                (
                    "Spawning | Tokio - An asynchronous Rust runtime",
                    "https://tokio.rs/tokio/tutorial/spawning",
                    "A Tokio task is an asynchronous green thread."
                ),
            ]
        );
    }

    #[test]
    fn error_answers_and_odd_answers() {
        assert_eq!(
            parse(include_str!("../../tests/fixtures/brave_error.json")),
            Err(ProviderError::BadKey)
        );
        let limited = r#"{"type":"ErrorResponse","error":{"status":429,"code":"RATE_LIMITED","detail":"Request rate limit exceeded for plan."}}"#;
        assert_eq!(parse(limited), Err(ProviderError::RateLimited));
        // A search with no web results is an empty list, not an error.
        assert_eq!(
            parse(r#"{"type":"search","query":{"original":"x"}}"#),
            Ok(Vec::new())
        );
        assert_eq!(
            parse(r#"{"type":"search","web":{"results":[]}}"#),
            Ok(Vec::new())
        );
        assert!(matches!(
            parse("not json"),
            Err(ProviderError::Unreadable(_))
        ));
        assert!(matches!(parse("[1, 2]"), Err(ProviderError::Unreadable(_))));
        assert_eq!(parse(r#"{"web":{"results":"nope"}}"#), Ok(Vec::new()));
    }
}
