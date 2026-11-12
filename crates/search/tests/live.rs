//! Checks against the real services. They need the network, so every test here is
//! ignored by default and nothing in CI runs them. To run them by hand:
//!
//! ```text
//! cargo test -p search --test live -- --ignored --nocapture
//! ```
//!
//! The provider is DuckDuckGo unless `SEARCH_LIVE_PROVIDER` says `brave`, `tavily` (with
//! the key in `SEARCH_LIVE_KEY`) or `searxng` (with the address in `SEARCH_LIVE_SEARXNG`).

use std::sync::Arc;

use search::provider::{self, ProviderConfig, ProviderKind};
use search::versions::{Ecosystem, Library};
use search::{Accept, Engine, Fetcher, Registry, SearchOptions};
use url::Url;

const UA: &str = concat!("harnessUI/", env!("CARGO_PKG_VERSION"));

fn live_provider() -> ProviderConfig {
    let kind = match std::env::var("SEARCH_LIVE_PROVIDER").as_deref() {
        Ok("brave") => ProviderKind::Brave,
        Ok("tavily") => ProviderKind::Tavily,
        Ok("searxng") => ProviderKind::Searxng,
        _ => ProviderKind::DuckDuckGo,
    };
    let mut config = ProviderConfig::new(kind, UA);
    config.api_key = std::env::var("SEARCH_LIVE_KEY").ok();
    config.searxng_url = std::env::var("SEARCH_LIVE_SEARXNG")
        .ok()
        .and_then(|address| Url::parse(&address).ok());
    config
}

fn library(name: &str, version: &str, ecosystem: Ecosystem) -> Library {
    Library {
        name: name.into(),
        version: version.into(),
        ecosystem,
    }
}

#[tokio::test]
#[ignore = "needs the network"]
async fn the_provider_answers() {
    let provider = provider::build(live_provider()).expect("the provider's key or address");
    let results = provider
        .search("tokio spawn_blocking", 6)
        .await
        .unwrap_or_else(|e| panic!("{}", e.explain(provider.kind())));
    for result in &results {
        println!("{}\n  {}\n  {}", result.title, result.url, result.snippet);
    }
    assert!(!results.is_empty(), "no results");
    assert!(results.iter().all(|r| r.url.starts_with("http")));
}

#[tokio::test]
#[ignore = "needs the network"]
async fn the_package_indexes_answer() {
    let fetcher = Fetcher::new(UA);
    let registry = Registry::new();
    for lib in [
        library("react", "18.3.1", Ecosystem::Npm),
        library("@tauri-apps/api", "2.12.4", Ecosystem::Npm),
        library("django", "4.2.11", Ecosystem::Pypi),
        library("numpy", "", Ecosystem::Pypi),
        library("tokio", "1.38.2", Ecosystem::Cargo),
    ] {
        let address = search::official::registry_url(&lib).unwrap();
        let direct = fetcher.get(&address, Accept::Json).await;
        let urls = registry.documentation_urls(&fetcher, &lib).await;
        println!(
            "{}: {} -> {:?} ({:?})",
            lib.name,
            address,
            urls.iter().map(Url::as_str).collect::<Vec<_>>(),
            direct
                .as_ref()
                .map(|f| f.text.len())
                .map_err(ToString::to_string),
        );
        assert!(!urls.is_empty(), "{}: nothing from {address}", lib.name);
    }
}

#[tokio::test]
#[ignore = "needs the network"]
async fn a_page_is_read() {
    let address = std::env::var("SEARCH_LIVE_URL")
        .unwrap_or_else(|_| "https://pypi.org/project/Django/".to_string());
    let engine = Engine::new(
        provider::build(ProviderConfig::new(ProviderKind::DuckDuckGo, UA)).unwrap(),
        Arc::new(Fetcher::new(UA)),
        Arc::new(Registry::new()),
    );
    let page = engine
        .read_page(&Url::parse(&address).unwrap(), "web framework install")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    println!(
        "{} ({} passages, sha256 {})",
        page.title,
        page.passages.len(),
        page.content_sha256
    );
    println!("{}", page.passages[0].text);
    assert!(!page.title.is_empty() && !page.passages.is_empty());
}

#[tokio::test]
#[ignore = "needs the network"]
async fn a_whole_search() {
    let engine = Engine::new(
        provider::build(live_provider()).expect("the provider's key or address"),
        Arc::new(Fetcher::new(UA)),
        Arc::new(Registry::new()),
    );
    let options = SearchOptions {
        libraries: vec![library("tokio", "1.38.2", Ecosystem::Cargo)],
        ..SearchOptions::default()
    };
    let outcome = engine
        .search("tokio spawn_blocking", &options)
        .await
        .unwrap_or_else(|e| panic!("{}", e.explain()));
    println!("sent: {}", outcome.sent_query);
    for hit in &outcome.hits {
        println!(
            "{} — {} official={} note={:?} read={} error={:?}\n  {}",
            hit.title,
            hit.site,
            hit.official,
            hit.version_note,
            hit.was_read(),
            hit.page_error,
            hit.excerpt(300).replace('\n', " ")
        );
    }
    assert!(!outcome.hits.is_empty());
}
