//! A whole search against local servers: a SearXNG instance, the pages it lists, and a
//! package index. No test here needs the network.

mod common;

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use common::{Request, Response, TestServer};
use search::provider::{self, ProviderConfig, ProviderError, ProviderKind};
use search::versions::{Ecosystem, Library};
use search::{Engine, FetchError, Fetcher, Registry, SearchError, SearchOptions};
use url::Url;

const UA: &str = "harnessUI-test/0.0.4";

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn tokio_lib() -> Library {
    Library {
        name: "tokio".into(),
        version: "1.38.2".into(),
        ecosystem: Ecosystem::Cargo,
    }
}

fn filler(topic: &str, sentences: usize) -> String {
    (0..sentences)
        .map(|i| format!("The {topic} part number {i} explains scheduling, timers and wakers in general terms."))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The documentation page: the passage that answers the question sits in the middle.
fn docs_page() -> String {
    format!(
        "<!DOCTYPE html><html><head><title>spawn_blocking in tokio::task - Rust</title>\
         <script>var tracking = 'SCRIPT';</script></head><body>\
         <nav>NAVIGATION LINKS</nav><main>\
         <h1>Function spawn_blocking</h1>\
         <p>{}</p><p>{}</p>\
         <h2>Blocking work</h2>\
         <p>Runs the provided closure on a thread where blocking is acceptable. \
         The spawn_blocking function moves the closure to a dedicated blocking thread pool, \
         so the closure may block without stalling the async workers.</p>\
         <pre><code>let result = tokio::task::spawn_blocking(|| heavy()).await?;</code></pre>\
         <p>{}</p><p>{}</p>\
         </main><footer>FOOTER</footer></body></html>",
        filler("overview", 9),
        filler("runtime", 9),
        filler("cancellation", 9),
        filler("shutdown", 9),
    )
}

fn blog_page() -> String {
    format!(
        "<html><head><title>My notes on async Rust</title></head><body><article>\
         <p>{}</p><p>I used spawn_blocking for hashing passwords and it worked well.</p></article></body></html>",
        filler("blog", 6)
    )
}

struct World {
    pages: TestServer,
    searx: TestServer,
    registry: TestServer,
    engine: Engine,
}

/// What the SearXNG instance lists, in its order: a blog first, the official pages after.
fn listing(port: u16) -> String {
    serde_json::json!({
        "query": "x",
        "results": [
            {"url": format!("http://blog.test:{port}/post"), "title": "My notes on async Rust",
             "content": "I used spawn_blocking for hashing passwords."},
            {"url": format!("http://intranet.test:{port}/wiki/tokio"), "title": "Team wiki: Tokio",
             "content": "Internal notes about our Tokio setup."},
            {"url": format!("http://docs.rs:{port}/tokio/1.38.0/tokio/task/fn.spawn_blocking.html"),
             "title": "spawn_blocking in tokio::task - Rust",
             "content": "Runs the provided closure on a thread where blocking is acceptable."},
            {"url": format!("http://missing.test:{port}/gone"), "title": "A page that is gone",
             "content": "This used to explain spawn_blocking."},
            {"url": format!("http://pdf.test:{port}/paper.pdf"), "title": "A paper",
             "content": "A PDF about runtimes."},
            {"url": format!("http://tokio.rs:{port}/tokio/tutorial/spawning"), "title": "Spawning | Tokio",
             "content": "A Tokio task is an asynchronous green thread."}
        ]
    })
    .to_string()
}

async fn world_with(
    searx_answer: impl Fn(&Request, u16) -> Response + Send + Sync + 'static,
) -> World {
    let pages = TestServer::start(|request| {
        let host = request.header("host").unwrap_or("").split(':').next().unwrap_or("").to_string();
        match (host.as_str(), request.path()) {
            ("docs.rs", "/tokio/1.38.0/tokio/task/fn.spawn_blocking.html") => Response::html(docs_page()),
            ("blog.test", "/post") => Response::html(blog_page()),
            ("tokio.rs", "/tokio/tutorial/spawning") => Response::html(
                "<html><head><title>Spawning | Tokio</title></head><body><main><p>A Tokio task is an asynchronous green thread. Tasks are spawned with tokio::spawn.</p></main></body></html>",
            ),
            ("pdf.test", _) => Response::new(200, "application/pdf", "%PDF-1.7"),
            _ => Response::new(404, "text/html", "<h1>Not found</h1>"),
        }
    })
    .await;
    let pages_port = pages.port;
    let searx = TestServer::start(move |request| searx_answer(request, pages_port)).await;
    let registry = TestServer::start(|request| match request.path() {
        "/api/v1/crates/tokio" => Response::json(include_str!("fixtures/crates_tokio.json")),
        _ => Response::new(404, "application/json", "{\"errors\": []}"),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[pages.port, registry.port])
        .with_test_dns(&[
            ("docs.rs", ip("127.0.0.1")),
            ("blog.test", ip("127.0.0.1")),
            ("tokio.rs", ip("127.0.0.1")),
            ("missing.test", ip("127.0.0.1")),
            ("pdf.test", ip("127.0.0.1")),
            ("registry.test", ip("127.0.0.1")),
            ("intranet.test", ip("10.0.0.9")),
        ]);
    let mut config = ProviderConfig::new(ProviderKind::Searxng, UA);
    config.searxng_url = Some(searx.url("127.0.0.1", "/"));
    config.timeout = Duration::from_millis(1500);
    let base = |path: &str| registry.url("registry.test", path);
    let engine = Engine::new(
        provider::build(config).unwrap(),
        Arc::new(fetcher),
        Arc::new(Registry::with_test_bases(
            base("/npm/"),
            base("/pypi/"),
            base("/api/v1/crates/"),
        )),
    );
    World {
        pages,
        searx,
        registry,
        engine,
    }
}

async fn world() -> World {
    world_with(|request, port| match request.path() {
        "/search" => Response::json(listing(port)),
        _ => Response::status(404),
    })
    .await
}

fn project() -> SearchOptions {
    SearchOptions {
        libraries: vec![tokio_lib()],
        ..SearchOptions::default()
    }
}

#[tokio::test]
async fn a_search_puts_official_pages_first_and_keeps_their_best_passages() {
    let w = world().await;
    let outcome = w
        .engine
        .search("tokio spawn_blocking thread pool", &project())
        .await
        .unwrap();

    // The project's version went to the provider, as a plain JSON search.
    let sent = &w.searx.requests()[0];
    assert_eq!(
        sent.query("q").as_deref(),
        Some("tokio spawn_blocking thread pool 1.38")
    );
    assert_eq!(sent.query("format").as_deref(), Some("json"));
    assert_eq!(sent.header("user-agent"), Some(UA));
    assert_eq!(sent.header("cookie"), None);
    assert_eq!(outcome.sent_query, "tokio spawn_blocking thread pool 1.38");
    assert_eq!(outcome.library, Some(tokio_lib()));

    // Official documentation first (the library's own pages), the rest in the provider's order.
    let sites: Vec<&str> = outcome.hits.iter().map(|h| h.site.as_str()).collect();
    assert_eq!(
        sites,
        [
            "docs.rs",
            "tokio.rs",
            "blog.test",
            "intranet.test",
            "missing.test",
            "pdf.test"
        ]
    );
    let official: Vec<bool> = outcome.hits.iter().map(|h| h.official).collect();
    assert_eq!(official, [true, true, false, false, false, false]);

    // The documentation page was read.
    let docs = &outcome.hits[0];
    assert_eq!(docs.title, "spawn_blocking in tokio::task - Rust");
    assert!(
        docs.url
            .ends_with("/tokio/1.38.0/tokio/task/fn.spawn_blocking.html")
    );
    assert_eq!(docs.version_note.as_deref(), Some("matches tokio 1.38"));
    assert!(docs.was_read() && docs.page_error.is_none());
    let text = search::extract::html_to_text(&docs_page()).text;
    assert_eq!(
        docs.content_sha256.as_deref(),
        Some(search::pipeline::sha256_hex(&text).as_str())
    );
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    assert!((now - docs.fetched_at.unwrap()).abs() < 60_000);
    // Its best passage is the one about the question, not the top of the page.
    assert!(docs.passages.len() <= 3 && !docs.passages.is_empty());
    assert!(
        docs.passages[0]
            .text
            .contains("dedicated blocking thread pool"),
        "{:?}",
        docs.passages[0]
    );
    assert!(
        docs.passages[0].index > 0,
        "the page's first passage is filler"
    );
    let excerpt = docs.excerpt(1200);
    assert!(
        excerpt.contains("Runs the provided closure on a thread where blocking is acceptable.")
    );
    assert!(excerpt.chars().count() <= 1200);
    // Text only: nothing of the page's markup, scripts or navigation.
    for passage in &docs.passages {
        for gone in ["<p>", "<code>", "SCRIPT", "NAVIGATION", "FOOTER"] {
            assert!(!passage.text.contains(gone), "{gone} in {passage:?}");
        }
    }

    // The fourth of the four pages read is inside the user's network: listed, never fetched.
    let wiki = &outcome.hits[3];
    assert!(!wiki.was_read() && wiki.passages.is_empty());
    assert_eq!(wiki.snippet, "Internal notes about our Tokio setup.");
    let why = wiki.page_error.as_deref().unwrap();
    assert!(
        why.contains("private network") && why.contains("10.0.0.9"),
        "{why}"
    );

    // Results past the fourth are listed with their snippets and were not tried.
    for hit in &outcome.hits[4..] {
        assert!(!hit.was_read() && hit.page_error.is_none(), "{hit:?}");
        assert!(!hit.snippet.is_empty());
    }
    let fetched: Vec<String> = w
        .pages
        .requests()
        .iter()
        .map(|r| {
            r.header("host")
                .unwrap()
                .split(':')
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(fetched.len(), 3, "{fetched:?}");
    for host in ["docs.rs", "tokio.rs", "blog.test"] {
        assert!(
            fetched.contains(&host.to_string()),
            "{host} was not fetched"
        );
    }
    // The package index was asked once, about the library, through the same fetcher.
    let asked = w.registry.requests();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].target, "/api/v1/crates/tokio");
    assert_eq!(asked[0].header("user-agent"), Some(UA));
}

#[tokio::test]
async fn pages_that_cannot_be_read_stay_in_the_list_with_the_reason() {
    let w = world().await;
    let options = SearchOptions {
        pages: 6,
        ..project()
    };
    let outcome = w
        .engine
        .search("tokio spawn_blocking", &options)
        .await
        .unwrap();
    assert_eq!(outcome.hits.len(), 6);
    let by_site = |site: &str| outcome.hits.iter().find(|h| h.site == site).unwrap();
    assert!(by_site("docs.rs").was_read());
    assert!(by_site("blog.test").was_read());
    let gone = by_site("missing.test");
    assert!(!gone.was_read());
    assert!(
        gone.page_error.as_deref().unwrap().contains("404"),
        "{:?}",
        gone.page_error
    );
    assert_eq!(gone.snippet, "This used to explain spawn_blocking.");
    let pdf = by_site("pdf.test");
    assert!(
        pdf.page_error
            .as_deref()
            .unwrap()
            .contains("not a web page"),
        "{:?}",
        pdf.page_error
    );
    assert!(by_site("intranet.test").page_error.is_some());
}

#[tokio::test]
async fn the_model_can_name_the_library() {
    let w = world().await;
    let options = SearchOptions {
        library: Some("Tokio".into()),
        ..project()
    };
    let outcome = w
        .engine
        .search("spawn_blocking example", &options)
        .await
        .unwrap();
    assert_eq!(
        w.searx.requests()[0].query("q").as_deref(),
        Some("spawn_blocking example tokio 1.38")
    );
    assert_eq!(outcome.library, Some(tokio_lib()));
    assert_eq!(outcome.hits[0].site, "docs.rs");
    // Passages are ranked for what was asked, not for the version that was added.
    assert!(outcome.hits[0].passages[0].text.contains("spawn_blocking"));

    // A library the project does not use: its name is added, no version, no package index.
    let options = SearchOptions {
        library: Some("django".into()),
        ..project()
    };
    let before = w.registry.requests().len();
    let outcome = w.engine.search("forms validation", &options).await.unwrap();
    assert_eq!(
        w.searx.requests()[1].query("q").as_deref(),
        Some("forms validation django")
    );
    assert_eq!(outcome.library, None);
    assert_eq!(w.registry.requests().len(), before);
    assert!(outcome.hits.iter().all(|h| h.version_note.is_none()));
}

#[tokio::test]
async fn without_the_projects_libraries_nothing_is_added_and_no_index_is_asked() {
    let w = world().await;
    let outcome = w
        .engine
        .search("tokio spawn_blocking", &SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(
        w.searx.requests()[0].query("q").as_deref(),
        Some("tokio spawn_blocking")
    );
    assert_eq!(outcome.library, None);
    assert_eq!(w.registry.connections(), 0);
    // Documentation hosts the app knows by itself still come first.
    assert_eq!(outcome.hits[0].site, "docs.rs");
    assert!(outcome.hits[0].official);
    assert_eq!(outcome.hits[0].version_note, None);

    // With the libraries but the index switched off: the version is added, nobody is asked.
    let options = SearchOptions {
        ask_registry: false,
        ..project()
    };
    let outcome = w
        .engine
        .search("tokio spawn_blocking", &options)
        .await
        .unwrap();
    assert_eq!(outcome.sent_query, "tokio spawn_blocking 1.38");
    assert_eq!(w.registry.connections(), 0);
    assert_eq!(
        outcome.hits[0].version_note.as_deref(),
        Some("matches tokio 1.38")
    );
}

#[tokio::test]
async fn the_package_index_is_asked_once_per_library() {
    let w = world().await;
    for _ in 0..3 {
        w.engine
            .search("tokio spawn_blocking", &project())
            .await
            .unwrap();
    }
    assert_eq!(w.registry.requests().len(), 1);
}

#[tokio::test]
async fn how_many_results_and_pages() {
    let w = world().await;
    let options = SearchOptions {
        max_results: 2,
        pages: 1,
        passages_per_page: 1,
        ..SearchOptions::default()
    };
    let outcome = w.engine.search("spawn_blocking", &options).await.unwrap();
    // Two results are kept (after official pages were put first); one page is read.
    assert_eq!(outcome.hits.len(), 2);
    assert_eq!(outcome.hits.iter().filter(|h| h.was_read()).count(), 1);
    assert_eq!(outcome.hits[0].passages.len(), 1);
    assert_eq!(w.pages.requests().len(), 1);
    // Limits hold whatever is asked.
    let greedy = SearchOptions {
        max_results: 500,
        pages: 500,
        passages_per_page: 500,
        ..SearchOptions::default()
    };
    let outcome = w.engine.search("spawn_blocking", &greedy).await.unwrap();
    assert_eq!(outcome.hits.len(), 6);
    assert!(outcome.hits.len() <= provider::MAX_RESULTS);
    // No pages at all: only the provider's list.
    let none = SearchOptions {
        pages: 0,
        ..SearchOptions::default()
    };
    let before = w.pages.requests().len();
    let outcome = w.engine.search("spawn_blocking", &none).await.unwrap();
    assert!(
        outcome
            .hits
            .iter()
            .all(|h| !h.was_read() && h.page_error.is_none())
    );
    assert_eq!(w.pages.requests().len(), before);
}

#[tokio::test]
async fn an_empty_question_asks_nobody() {
    let w = world().await;
    for query in ["", "   ", "\n\t"] {
        assert_eq!(
            w.engine.search(query, &project()).await,
            Err(SearchError::EmptyQuery)
        );
    }
    assert_eq!(
        w.searx.connections() + w.pages.connections() + w.registry.connections(),
        0
    );
}

#[tokio::test]
async fn a_provider_that_fails_says_what_happened() {
    let failing = |status: u16, body: &'static str| async move {
        let w = world_with(move |_, _| Response::new(status, "application/json", body)).await;
        let err = w
            .engine
            .search("tokio", &SearchOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            w.pages.connections(),
            0,
            "no page is read after a failed search"
        );
        match err {
            SearchError::Provider { kind, error } => {
                assert_eq!(kind, ProviderKind::Searxng);
                error
            }
            other => panic!("{other:?}"),
        }
    };
    assert_eq!(failing(403, "").await, ProviderError::JsonOff);
    assert_eq!(failing(429, "").await, ProviderError::RateLimited);
    assert_eq!(failing(500, "").await, ProviderError::Http(500));
    assert_eq!(
        failing(200, "<!DOCTYPE html><html>").await,
        ProviderError::JsonOff
    );
    assert!(matches!(
        failing(200, "{ broken").await,
        ProviderError::Unreadable(_)
    ));

    // No results is not a failure.
    let w = world_with(|_, _| Response::json("{\"query\": \"x\", \"results\": []}")).await;
    let outcome = w
        .engine
        .search("qzxjv", &SearchOptions::default())
        .await
        .unwrap();
    assert!(outcome.hits.is_empty());

    // Too slow.
    let w = world_with(|_, _| Response::json("{}").after(Duration::from_secs(20))).await;
    let started = std::time::Instant::now();
    let err = w
        .engine
        .search("tokio", &SearchOptions::default())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        SearchError::Provider {
            kind: ProviderKind::Searxng,
            error: ProviderError::Timeout
        }
    );
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(err.explain().contains("SearXNG"));

    // Nothing listens at the address.
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let mut config = ProviderConfig::new(ProviderKind::Searxng, UA);
    config.searxng_url = Some(Url::parse(&format!("http://127.0.0.1:{port}")).unwrap());
    let engine = Engine::new(
        provider::build(config).unwrap(),
        Arc::new(Fetcher::new(UA)),
        Arc::new(Registry::new()),
    );
    let err = engine
        .search("tokio", &SearchOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            SearchError::Provider {
                error: ProviderError::Network(_),
                ..
            }
        ),
        "{err:?}"
    );
}

/// The user's own SearXNG may run on this computer; the pages it lists may not be read
/// from there. This is the fetcher the app builds: no test hook.
#[tokio::test]
async fn results_on_this_computer_are_listed_but_never_fetched() {
    let trap = TestServer::start(|_| Response::html("SECRET FROM THIS COMPUTER")).await;
    let trap_port = trap.port;
    let searx = TestServer::start(move |_| {
        Response::json(
            serde_json::json!({"results": [
                {"url": format!("http://127.0.0.1:{trap_port}/admin"), "title": "Local admin", "content": "A page on this computer."},
                {"url": format!("http://localhost:{trap_port}/x"), "title": "Localhost", "content": "Another one."},
                {"url": "http://192.168.1.1/", "title": "Router", "content": "The router's page."},
                {"url": "http://169.254.169.254/latest/meta-data/", "title": "Metadata", "content": "Cloud metadata."}
            ]})
            .to_string(),
        )
    })
    .await;
    let mut config = ProviderConfig::new(ProviderKind::Searxng, UA);
    config.searxng_url = Some(searx.url("127.0.0.1", "/"));
    let engine = Engine::new(
        provider::build(config).unwrap(),
        Arc::new(Fetcher::new(UA)),
        Arc::new(Registry::new()),
    );
    let outcome = engine
        .search("admin page", &SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.hits.len(), 4);
    for hit in &outcome.hits {
        assert!(!hit.was_read(), "{hit:?}");
        assert!(!hit.snippet.is_empty());
        let why = hit.page_error.as_deref().unwrap();
        assert!(why.contains("never read"), "{why}");
        assert!(
            hit.passages.is_empty() && hit.content_sha256.is_none() && hit.fetched_at.is_none()
        );
    }
    assert!(
        outcome.hits[0]
            .page_error
            .as_deref()
            .unwrap()
            .contains("this computer")
    );
    assert!(
        outcome.hits[2]
            .page_error
            .as_deref()
            .unwrap()
            .contains("private network")
    );
    assert!(
        outcome.hits[3]
            .page_error
            .as_deref()
            .unwrap()
            .contains("link-local")
    );
    assert_eq!(trap.connections(), 0);
    assert_eq!(searx.connections(), 1);
}

#[tokio::test]
async fn a_page_is_read_again_for_more_of_its_text() {
    let w = world().await;
    let address = w
        .pages
        .url("docs.rs", "/tokio/1.38.0/tokio/task/fn.spawn_blocking.html");
    let page = w
        .engine
        .read_page(&address, "dedicated blocking thread pool")
        .await
        .unwrap();
    assert_eq!(page.title, "spawn_blocking in tokio::task - Rust");
    assert_eq!(page.url, address);
    let text = search::extract::html_to_text(&docs_page()).text;
    assert_eq!(page.content_sha256, search::pipeline::sha256_hex(&text));
    // Every passage of the page, best first.
    let all = search::rank::split_passages(&text, search::rank::PASSAGE_CHARS);
    assert_eq!(page.passages.len(), all.len());
    assert!(page.passages.len() >= 4);
    assert!(
        page.passages[0]
            .text
            .contains("dedicated blocking thread pool")
    );
    assert!(page.passages.windows(2).all(|w| w[0].score >= w[1].score));
    assert!(!page.truncated);

    // The same rules as for any page.
    let private = Url::parse("http://intranet.test/wiki").unwrap();
    assert!(matches!(
        w.engine.read_page(&private, "x").await,
        Err(FetchError::Blocked { .. })
    ));
    let gone = w.pages.url("missing.test", "/gone");
    assert_eq!(
        w.engine.read_page(&gone, "x").await,
        Err(FetchError::Status(404))
    );
}

/// A page that stands in for the real one until a browser passes a check, and a page with
/// no text at all (one built by scripts), are not sources: the reason is given instead.
#[tokio::test]
async fn browser_checks_and_empty_pages_are_not_read_as_pages() {
    let pages = TestServer::start(|request| match request.path() {
        "/wall" => Response::html(
            "<html><head><title>Just a moment...</title></head><body><h1>Checking your browser</h1>\
             <p>Enable JavaScript and cookies to continue</p></body></html>",
        ),
        "/app" => {
            Response::html("<html><body><div id=root></div><script>render()</script></body></html>")
        }
        _ => Response::html("<p>A real page about gardening with enough words to read.</p>"),
    })
    .await;
    let port = pages.port;
    let searx = TestServer::start(move |_| {
        Response::json(
            serde_json::json!({"results": [
                {"url": format!("http://site.test:{port}/wall"), "title": "Behind a check", "content": "Snippet one."},
                {"url": format!("http://site.test:{port}/app"), "title": "Built by scripts", "content": "Snippet two."},
                {"url": format!("http://site.test:{port}/real"), "title": "A real page", "content": "Snippet three."}
            ]})
            .to_string(),
        )
    })
    .await;
    let mut config = ProviderConfig::new(ProviderKind::Searxng, UA);
    config.searxng_url = Some(searx.url("127.0.0.1", "/"));
    let engine = Engine::new(
        provider::build(config).unwrap(),
        Arc::new(
            Fetcher::new(UA)
                .with_test_loopback_ports(&[port])
                .with_test_dns(&[("site.test", ip("127.0.0.1"))]),
        ),
        Arc::new(Registry::new()),
    );
    let outcome = engine
        .search("gardening", &SearchOptions::default())
        .await
        .unwrap();
    let wall = &outcome.hits[0];
    assert!(!wall.was_read() && wall.passages.is_empty());
    assert!(
        wall.page_error
            .as_deref()
            .unwrap()
            .contains("only a web browser"),
        "{:?}",
        wall.page_error
    );
    assert_eq!(wall.snippet, "Snippet one.");
    let app = &outcome.hits[1];
    assert!(!app.was_read());
    assert!(
        app.page_error.as_deref().unwrap().contains("no text"),
        "{:?}",
        app.page_error
    );
    assert!(outcome.hits[2].was_read());
    assert!(matches!(
        engine
            .read_page(&pages.url("site.test", "/wall"), "x")
            .await,
        Err(FetchError::Challenge)
    ));
    assert!(matches!(
        engine.read_page(&pages.url("site.test", "/app"), "x").await,
        Err(FetchError::NoText)
    ));
}
