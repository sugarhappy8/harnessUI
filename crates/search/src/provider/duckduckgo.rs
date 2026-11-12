//! DuckDuckGo's HTML endpoint: no key, an ordinary results page that is parsed here.

use async_trait::async_trait;
use url::Url;

use super::{Http, Provider, ProviderError, ProviderKind, SearchResult};

/// Where the search goes.
pub(crate) const ENDPOINT: &str = "https://html.duckduckgo.com/html/";

/// The address a result links to. DuckDuckGo wraps it in a redirect link
/// (`//duckduckgo.com/l/?uddg=<address>`); advertisements go through `/y.js` and are not
/// results.
fn target(href: &str) -> Option<String> {
    let href = href.trim();
    let absolute = match href.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => href.to_string(),
    };
    let url = Url::parse(&absolute).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if host == "duckduckgo.com" || host.ends_with(".duckduckgo.com") {
        if url.path() != "/l/" {
            return None;
        }
        let wrapped = url
            .query_pairs()
            .find(|(key, _)| key == "uddg")?
            .1
            .into_owned();
        let inner = Url::parse(&wrapped).ok()?;
        return matches!(inner.scheme(), "http" | "https").then(|| inner.to_string());
    }
    Some(url.to_string())
}

fn has_class(node: &dom_query::NodeRef<'_>, class: &str) -> bool {
    node.attr("class")
        .is_some_and(|classes| classes.split_whitespace().any(|c| c == class))
}

/// The results on a DuckDuckGo HTML results page: the title, the address the result links
/// to (taken out of DuckDuckGo's redirect link) and the snippet. Advertisements are left
/// out. A page that asks for a challenge instead of answering is [`ProviderError::Refused`].
pub fn parse(html: &str) -> Result<Vec<SearchResult>, ProviderError> {
    let document = crate::extract::parse_html(html);
    let mut results = Vec::new();
    for block in document.select("div.result").iter() {
        let Some(block) = block.nodes().first().copied() else {
            continue;
        };
        if has_class(&block, "result--ad") {
            continue;
        }
        let selection = dom_query::Selection::from(block);
        let link = selection.select("a.result__a");
        let Some(link) = link.nodes().first() else {
            continue;
        };
        let Some(url) = link.attr("href").and_then(|href| target(&href)) else {
            continue;
        };
        let snippet = selection
            .select(".result__snippet")
            .nodes()
            .first()
            .map(|node| node.text().to_string())
            .unwrap_or_default();
        results.push(SearchResult {
            title: link.text().to_string(),
            url,
            snippet,
        });
    }
    if results.is_empty()
        && (document
            .select(".anomaly-modal__mask, #challenge-form")
            .exists()
            || html.contains("anomaly.js"))
    {
        return Err(ProviderError::Refused);
    }
    Ok(results)
}

pub(crate) struct DuckDuckGo {
    pub(crate) http: Http,
}

#[async_trait]
impl Provider for DuckDuckGo {
    fn kind(&self) -> ProviderKind {
        ProviderKind::DuckDuckGo
    }

    async fn search(
        &self,
        query: &str,
        max_results: usize,
    ) -> Result<Vec<SearchResult>, ProviderError> {
        let request = self
            .http
            .client()
            .get(ENDPOINT)
            .query(&[("q", query)])
            .header(reqwest::header::ACCEPT, "text/html");
        super::run(&self.http, self.kind(), request, parse, max_results).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::clean;

    #[test]
    fn a_results_page() {
        let results = clean(
            parse(include_str!("../../tests/fixtures/duckduckgo.html")).unwrap(),
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
                    "Runs the provided closure on a thread where blocking is acceptable. In general, issuing a blocking call or performing a lot of compute in a future without yielding is problematic, as it may prevent the executor from driving other futures forward."
                ),
                (
                    "Spawning | Tokio - An asynchronous Rust runtime",
                    "https://tokio.rs/tokio/tutorial/spawning",
                    "A Tokio task is an asynchronous green thread. They are created by passing an async block to tokio::spawn & return a JoinHandle."
                ),
                (
                    "rust - When should you use Tokio's `spawn_blocking`? - Stack Overflow",
                    "https://stackoverflow.com/questions/61752896/how-to-use-spawn_blocking?x=1&y=2",
                    "Use it for CPU-bound work <like hashing> or for calling synchronous APIs."
                ),
                (
                    "A direct link without the redirect wrapper",
                    "https://example.org/direct-link",
                    ""
                ),
            ]
        );
    }

    #[test]
    fn advertisements_are_never_results() {
        let results = parse(include_str!("../../tests/fixtures/duckduckgo.html")).unwrap();
        for r in &results {
            assert!(!r.title.contains("Sponsored"), "{r:?}");
            assert!(
                !r.url.contains("y.js") && !r.url.contains("learn-rust.example"),
                "{r:?}"
            );
            assert!(!r.snippet.contains("advertisement"), "{r:?}");
        }
    }

    #[test]
    fn a_page_without_results_and_a_challenge_page() {
        assert_eq!(
            parse(include_str!("../../tests/fixtures/duckduckgo_none.html")),
            Ok(Vec::new())
        );
        assert_eq!(
            parse(include_str!(
                "../../tests/fixtures/duckduckgo_challenge.html"
            )),
            Err(ProviderError::Refused)
        );
        assert_eq!(parse(""), Ok(Vec::new()));
        assert_eq!(
            parse("<html><body><p>Something else entirely.</p></body></html>"),
            Ok(Vec::new())
        );
    }

    /// A results page cannot smuggle in an address through the redirect link.
    #[test]
    fn only_web_addresses_come_out_of_the_redirect_link() {
        let page = |href: &str| {
            format!(
                "<div class=\"result web-result\"><a class=\"result__a\" href=\"{href}\">T</a></div>"
            )
        };
        let urls = |href: &str| -> Vec<String> {
            clean(parse(&page(href)).unwrap(), 10)
                .into_iter()
                .map(|r| r.url)
                .collect()
        };
        assert_eq!(
            urls("//duckduckgo.com/l/?uddg=https%3A%2F%2Fa.example%2Fp%3Fq%3D1&amp;rut=x"),
            ["https://a.example/p?q=1"]
        );
        assert_eq!(
            urls("https://duckduckgo.com/l/?uddg=http%3A%2F%2Fb.example%2F"),
            ["http://b.example/"]
        );
        assert!(urls("//duckduckgo.com/l/?uddg=javascript%3Aalert(1)").is_empty());
        assert!(urls("//duckduckgo.com/l/?uddg=file%3A%2F%2F%2Fetc%2Fpasswd").is_empty());
        assert!(urls("//duckduckgo.com/l/?rut=x").is_empty());
        assert!(urls("//duckduckgo.com/y.js?u3=https%3A%2F%2Fad.example%2F").is_empty());
        assert!(urls("").is_empty());
        // A look-alike host is an ordinary link, not a redirect to unwrap.
        assert_eq!(
            urls("https://duckduckgo.com.evil.example/l/?uddg=https%3A%2F%2Fa.example%2F"),
            ["https://duckduckgo.com.evil.example/l/?uddg=https%3A%2F%2Fa.example%2F"]
        );
    }
}
