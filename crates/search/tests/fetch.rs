//! The guarded fetcher against real sockets (ARCHITECTURE.md 13.6): every address class is
//! refused before anything is connected to, the connection goes to the address that was
//! checked, and every redirect hop is checked again.
//!
//! The servers listen on 127.0.0.1. A test opens exactly the ports it serves from
//! (`with_test_loopback_ports`) and gives them public-looking names (`with_test_dns`);
//! both hooks exist only with the `test-hooks` feature, which the app never enables.

mod common;

use std::net::IpAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use common::{Response, TestServer};
use search::fetch::MEDIA_TYPES;
use search::netguard::AddressClass;
use search::{Accept, FetchError, Fetcher};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

const UA: &str = "harnessUI-test/0.0.4";

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

/// A server whose address is never opened: whatever reaches it got past the guard.
async fn trap() -> TestServer {
    TestServer::start(|_| Response::html("SECRET FROM THIS COMPUTER")).await
}

fn blocked_as(result: Result<search::Fetched, FetchError>) -> AddressClass {
    match result {
        Err(FetchError::Blocked { class, .. }) => class,
        other => panic!("expected a blocked address, got {other:?}"),
    }
}

/// Each class, IPv4 and IPv6, in every spelling an address can have. The fetcher is the
/// one the app builds: no hook at all.
#[tokio::test]
async fn every_blocked_class_is_refused_before_connecting() {
    use AddressClass::*;
    let trap = trap().await;
    let p = trap.port;
    let fetcher = Fetcher::new(UA);
    let cases: Vec<(String, AddressClass)> = vec![
        (format!("http://127.0.0.1:{p}/"), Loopback),
        (format!("http://127.0.0.2:{p}/"), Loopback),
        (format!("http://127.255.255.254:{p}/"), Loopback),
        (format!("http://[::1]:{p}/"), Loopback),
        // 127.0.0.1 as one number, in hex, in octal.
        (format!("http://2130706433:{p}/"), Loopback),
        (format!("http://0x7f.1:{p}/"), Loopback),
        (format!("http://0177.0.0.1:{p}/"), Loopback),
        // IPv4 inside IPv6.
        (format!("http://[::ffff:127.0.0.1]:{p}/"), Loopback),
        (format!("http://[::ffff:7f00:1]:{p}/"), Loopback),
        (format!("http://[::127.0.0.1]:{p}/"), Loopback),
        ("http://[64:ff9b::7f00:1]/".into(), Loopback),
        ("http://[2002:7f00:1::]/".into(), Loopback),
        ("http://10.0.0.1/".into(), Private),
        ("http://10.255.255.255/".into(), Private),
        ("http://172.16.5.4/".into(), Private),
        ("http://172.31.255.255/".into(), Private),
        ("http://192.168.1.1/admin".into(), Private),
        ("https://192.168.0.10:8443/".into(), Private),
        ("http://[::ffff:10.0.0.1]/".into(), Private),
        ("http://[64:ff9b::a00:1]/".into(), Private),
        ("http://169.254.169.254/latest/meta-data/".into(), LinkLocal),
        ("http://169.254.0.1/".into(), LinkLocal),
        ("http://[fe80::1]/".into(), LinkLocal),
        ("http://[::ffff:169.254.169.254]/".into(), LinkLocal),
        ("http://100.64.0.1/".into(), CarrierGradeNat),
        ("http://100.127.255.254/".into(), CarrierGradeNat),
        ("http://[::ffff:100.64.0.1]/".into(), CarrierGradeNat),
        (format!("http://0.0.0.0:{p}/"), Unspecified),
        ("http://0.1.2.3/".into(), Unspecified),
        (format!("http://[::]:{p}/"), Unspecified),
        ("http://[fc00::1]/".into(), UniqueLocal),
        ("http://[fd12:3456:789a::1]/".into(), UniqueLocal),
        ("http://[fec0::1]/".into(), UniqueLocal),
        ("http://224.0.0.1/".into(), Multicast),
        ("http://255.255.255.255/".into(), Multicast),
        ("http://[ff02::1]/".into(), Multicast),
        ("http://240.0.0.1/".into(), Reserved),
        ("http://192.0.2.1/".into(), Reserved),
        ("http://[2001:db8::1]/".into(), Reserved),
    ];
    for (address, class) in cases {
        let started = Instant::now();
        let got = blocked_as(fetcher.get(&url(&address), Accept::Page).await);
        assert_eq!(got, class, "{address}");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "{address} was not refused at once"
        );
        // The check alone gives the same answer.
        assert!(
            matches!(fetcher.check(&url(&address)).await, Err(FetchError::Blocked { class: c, .. }) if c == class),
            "{address}"
        );
    }
    assert_eq!(trap.connections(), 0, "a refused address was connected to");
}

#[tokio::test]
async fn localhost_names_are_refused_without_a_lookup() {
    let trap = trap().await;
    let p = trap.port;
    let fetcher = Fetcher::new(UA);
    for address in [
        format!("http://localhost:{p}/"),
        format!("http://LOCALHOST:{p}/"),
        format!("http://localhost.:{p}/"),
        format!("http://app.localhost:{p}/"),
        format!("http://a.b.LocalHost.:{p}/"),
    ] {
        let err = fetcher.get(&url(&address), Accept::Page).await.unwrap_err();
        assert!(
            matches!(err, FetchError::LocalName(_)),
            "{address}: {err:?}"
        );
        assert!(err.is_refusal());
    }
    assert_eq!(trap.connections(), 0);
}

/// A name is judged by what it resolves to. One blocked address among several refuses the
/// name: otherwise a name with a public and a private answer could reach the private one.
#[tokio::test]
async fn names_that_resolve_to_blocked_addresses_are_refused() {
    use AddressClass::*;
    let trap = trap().await;
    let p = trap.port;
    let fetcher = Fetcher::new(UA).with_test_dns(&[
        ("intranet.test", ip("10.1.2.3")),
        ("meta.test", ip("169.254.169.254")),
        ("six.test", ip("fd00::1")),
        ("self.test", ip("127.0.0.1")),
        ("self6.test", ip("::1")),
        ("shared.test", ip("100.64.3.4")),
        ("nowhere.test", ip("0.0.0.0")),
        ("mixed.test", ip("93.184.216.34")),
        ("mixed.test", ip("127.0.0.1")),
        ("mapped.test", ip("::ffff:192.168.1.1")),
    ]);
    for (address, class) in [
        ("http://intranet.test/".to_string(), Private),
        ("https://intranet.test/wiki".to_string(), Private),
        ("http://meta.test/latest/meta-data/".to_string(), LinkLocal),
        ("http://six.test/".to_string(), UniqueLocal),
        (format!("http://self.test:{p}/"), Loopback),
        (format!("http://SELF.test.:{p}/"), Loopback),
        (format!("http://self6.test:{p}/"), Loopback),
        ("http://shared.test/".to_string(), CarrierGradeNat),
        (format!("http://nowhere.test:{p}/"), Unspecified),
        (format!("http://mixed.test:{p}/"), Loopback),
        ("http://mapped.test/".to_string(), Private),
    ] {
        let started = Instant::now();
        assert_eq!(
            blocked_as(fetcher.get(&url(&address), Accept::Page).await),
            class,
            "{address}"
        );
        assert!(started.elapsed() < Duration::from_secs(2), "{address}");
    }
    assert_eq!(trap.connections(), 0);
}

/// `docs.test` does not exist anywhere but in this fetcher's checked answer. The page
/// arrives, so the connection went to the checked address and no other resolver was asked.
#[tokio::test]
async fn a_page_is_fetched_from_the_address_that_was_checked() {
    let server = TestServer::start(|_| Response::html("<p>Hello from the page.</p>")).await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("docs.test", ip("127.0.0.1"))]);
    let address = server.url("docs.test", "/guide?x=1");
    assert_eq!(
        fetcher.check(&address).await.unwrap(),
        vec![std::net::SocketAddr::new(ip("127.0.0.1"), server.port)]
    );
    let page = fetcher.get(&address, Accept::Page).await.unwrap();
    assert_eq!(page.text, "<p>Hello from the page.</p>");
    assert_eq!(page.content_type, "text/html");
    assert_eq!(page.url, address);
    assert_eq!(page.redirects, 0);
    assert!(!page.truncated);
    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].target, "/guide?x=1");
    assert_eq!(
        requests[0].header("host"),
        Some(format!("docs.test:{}", server.port).as_str())
    );
    // The open port opens only itself: the same server under another loopback address,
    // and another port, stay refused.
    let other = trap().await;
    assert_eq!(
        blocked_as(
            fetcher
                .get(&other.url("127.0.0.1", "/"), Accept::Page)
                .await
        ),
        AddressClass::Loopback
    );
    assert_eq!(other.connections(), 0);
}

/// The redirect a search result can hide behind: a public-looking page that sends the
/// reader to an address inside the user's network. Each hop is checked before it is
/// connected to.
#[tokio::test]
async fn a_redirect_to_a_private_address_is_refused_before_connecting() {
    use AddressClass::*;
    let trap = trap().await;
    let t = trap.port;
    let public = TestServer::start(move |request| {
        let target = match request.path() {
            "/to-loopback" => format!("http://127.0.0.1:{t}/secret"),
            "/to-loopback-v6" => format!("http://[::1]:{t}/secret"),
            "/to-decimal" => format!("http://2130706433:{t}/secret"),
            "/to-mapped" => format!("http://[::ffff:127.0.0.1]:{t}/secret"),
            "/to-zero" => format!("http://0.0.0.0:{t}/secret"),
            "/to-private" => "http://10.0.0.5/router".to_string(),
            "/to-private-https" => "https://192.168.1.1/".to_string(),
            "/to-metadata" => "http://169.254.169.254/latest/meta-data/".to_string(),
            "/to-shared" => "http://100.64.0.9/".to_string(),
            "/to-unique-local" => "http://[fd00::1]/".to_string(),
            "/to-name" => "http://intranet.test/wiki".to_string(),
            "/to-self-name" => format!("http://self.test:{t}/secret"),
            "/to-localhost" => format!("http://localhost:{t}/secret"),
            "/then-private" => "/to-private".to_string(),
            other => return Response::html(format!("no such test page {other}")),
        };
        let status = match request.query("status").as_deref() {
            Some(s) => s.parse().unwrap(),
            None => 302,
        };
        Response::redirect(status, &target)
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[public.port])
        .with_test_dns(&[
            ("public.test", ip("127.0.0.1")),
            ("intranet.test", ip("192.168.7.7")),
            ("self.test", ip("127.0.0.1")),
        ]);
    let cases = [
        ("/to-loopback", Loopback),
        ("/to-loopback-v6", Loopback),
        ("/to-decimal", Loopback),
        ("/to-mapped", Loopback),
        ("/to-zero", Unspecified),
        ("/to-private", Private),
        ("/to-private-https", Private),
        ("/to-metadata", LinkLocal),
        ("/to-shared", CarrierGradeNat),
        ("/to-unique-local", UniqueLocal),
        ("/to-name", Private),
        ("/to-self-name", Loopback),
        // Two hops: the second one is checked like the first.
        ("/then-private", Private),
    ];
    for (path, class) in cases {
        let started = Instant::now();
        let got = blocked_as(
            fetcher
                .get(&public.url("public.test", path), Accept::Page)
                .await,
        );
        assert_eq!(got, class, "{path}");
        assert!(started.elapsed() < Duration::from_secs(2), "{path}");
    }
    // Whatever kind of redirect it is.
    for status in [301, 302, 303, 307, 308] {
        let address = public.url("public.test", &format!("/to-loopback?status={status}"));
        assert_eq!(
            blocked_as(fetcher.get(&address, Accept::Page).await),
            Loopback,
            "{status}"
        );
    }
    let err = fetcher
        .get(&public.url("public.test", "/to-localhost"), Accept::Page)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::LocalName(_)), "{err:?}");
    assert_eq!(trap.connections(), 0, "a redirect reached this computer");
    assert_eq!(
        public.connections(),
        cases.len() + 1 + 5 + 1,
        "one request per hop"
    );
}

#[tokio::test]
async fn redirects_are_followed_by_hand_at_most_five_times() {
    let second = TestServer::start(|_| Response::html("on the other site")).await;
    let other_port = second.port;
    let first = TestServer::start(move |request| {
        let path = request.path().to_string();
        if let Some(n) = path.strip_prefix("/hop/") {
            let n: u32 = n.parse().unwrap();
            return match n {
                0 => Response::html("arrived"),
                // Absolute path, relative path, full address: all are read against the
                // address that sent them.
                1 => Response::redirect(302, "/hop/0"),
                2 => Response::redirect(301, "1"),
                n => Response::redirect(
                    307,
                    &format!(
                        "http://first.test:{}/hop/{}",
                        request.header("host").unwrap().rsplit(':').next().unwrap(),
                        n - 1
                    ),
                ),
            };
        }
        match path.as_str() {
            "/elsewhere" => {
                Response::redirect(302, &format!("http://second.test:{other_port}/landing"))
            }
            _ => Response::status(404),
        }
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[first.port, second.port])
        .with_test_dns(&[
            ("first.test", ip("127.0.0.1")),
            ("second.test", ip("127.0.0.1")),
        ]);
    let page = fetcher
        .get(&first.url("first.test", "/hop/5"), Accept::Page)
        .await
        .unwrap();
    assert_eq!(page.text, "arrived");
    assert_eq!(page.redirects, 5);
    assert_eq!(page.url, first.url("first.test", "/hop/0"));
    assert_eq!(first.requests().len(), 6);
    assert_eq!(
        fetcher
            .get(&first.url("first.test", "/hop/6"), Accept::Page)
            .await,
        Err(FetchError::TooManyRedirects)
    );
    // To another site: allowed, and that site sees its own name, no referrer and no cookie.
    let page = fetcher
        .get(&first.url("first.test", "/elsewhere"), Accept::Page)
        .await
        .unwrap();
    assert_eq!(page.text, "on the other site");
    assert_eq!(page.url, second.url("second.test", "/landing"));
    let landed = &second.requests()[0];
    assert_eq!(
        landed.header("host"),
        Some(format!("second.test:{other_port}").as_str())
    );
    assert_eq!(landed.header("referer"), None);
}

#[tokio::test]
async fn redirects_to_other_schemes_or_nowhere_are_refused() {
    let server = TestServer::start(|request| match request.path() {
        "/file" => Response::redirect(302, "file:///etc/passwd"),
        "/ftp" => Response::redirect(302, "ftp://files.test/x"),
        "/script" => Response::redirect(302, "javascript:alert(1)"),
        "/data" => Response::redirect(302, "data:text/html,<p>hi</p>"),
        "/credentials" => Response::redirect(302, "http://user:pw@public.test/"),
        "/empty" => Response::redirect(302, ""),
        "/missing" => Response {
            status: 302,
            headers: Vec::new(),
            body: Vec::new(),
            delay: Duration::ZERO,
        },
        "/not-modified" => Response::status(304),
        "/choices" => Response::status(300),
        _ => Response::status(404),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let get = |path: &'static str| {
        let fetcher = fetcher.clone();
        let address = server.url("public.test", path);
        async move { fetcher.get(&address, Accept::Page).await.unwrap_err() }
    };
    for path in ["/file", "/ftp", "/script", "/data"] {
        assert!(matches!(get(path).await, FetchError::Scheme(_)), "{path}");
    }
    assert_eq!(get("/credentials").await, FetchError::Credentials);
    assert!(matches!(get("/empty").await, FetchError::BadRedirect(_)));
    assert!(matches!(get("/missing").await, FetchError::BadRedirect(_)));
    assert_eq!(get("/not-modified").await, FetchError::Status(304));
    assert_eq!(get("/choices").await, FetchError::Status(300));
}

#[tokio::test]
async fn addresses_that_are_not_plain_web_addresses_are_refused() {
    let server = TestServer::start(|_| Response::html("x")).await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    for (address, scheme) in [
        ("ftp://public.test/file", "ftp"),
        ("file:///etc/passwd", "file"),
        ("gopher://public.test/", "gopher"),
        ("data:text/html,hi", "data"),
        ("javascript:alert(1)", "javascript"),
        ("ws://public.test/", "ws"),
    ] {
        assert_eq!(
            fetcher.get(&url(address), Accept::Page).await,
            Err(FetchError::Scheme(scheme.to_string())),
            "{address}"
        );
    }
    let with_password = url(&format!("http://user:pw@public.test:{}/", server.port));
    assert_eq!(
        fetcher.get(&with_password, Accept::Page).await,
        Err(FetchError::Credentials)
    );
    let with_user = url(&format!("http://user@public.test:{}/", server.port));
    assert_eq!(
        fetcher.get(&with_user, Accept::Page).await,
        Err(FetchError::Credentials)
    );
    assert_eq!(server.connections(), 0);
}

#[tokio::test]
async fn only_web_pages_and_plain_text_are_read() {
    let server = TestServer::start(|request| match request.path() {
        "/html" => Response::new(200, "TEXT/HTML; Charset=UTF-8", "<p>page</p>"),
        "/plain" => Response::new(200, "text/plain", "plain words"),
        "/pdf" => Response::new(200, "application/pdf", "%PDF-1.7"),
        "/png" => Response::new(200, "image/png", vec![0x89, b'P', b'N', b'G']),
        "/json" => Response::json("{\"a\": 1}"),
        "/binary" => Response::new(200, "application/octet-stream", vec![0, 1, 2]),
        "/xhtml" => Response::new(200, "application/xhtml+xml", "<html/>"),
        "/untyped" => Response {
            status: 200,
            headers: Vec::new(),
            body: b"<p>no type</p>".to_vec(),
            delay: Duration::ZERO,
        },
        _ => Response::status(404),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let at = |path: &str| server.url("public.test", path);
    let html = fetcher.get(&at("/html"), Accept::Page).await.unwrap();
    assert_eq!(
        (html.content_type.as_str(), html.text.as_str()),
        ("text/html", "<p>page</p>")
    );
    let plain = fetcher.get(&at("/plain"), Accept::Page).await.unwrap();
    assert_eq!(
        (plain.content_type.as_str(), plain.text.as_str()),
        ("text/plain", "plain words")
    );
    for (path, kind) in [
        ("/pdf", "application/pdf"),
        ("/png", "image/png"),
        ("/json", "application/json"),
        ("/binary", "application/octet-stream"),
        ("/xhtml", "application/xhtml+xml"),
        ("/untyped", ""),
    ] {
        assert_eq!(
            fetcher.get(&at(path), Accept::Page).await,
            Err(FetchError::ContentType(kind.to_string())),
            "{path}"
        );
    }
    // A package index answers in JSON, and only that is taken from it.
    let json = fetcher.get(&at("/json"), Accept::Json).await.unwrap();
    assert_eq!(json.text, "{\"a\": 1}");
    assert_eq!(
        fetcher.get(&at("/html"), Accept::Json).await,
        Err(FetchError::ContentType("text/html".into()))
    );
}

#[tokio::test]
async fn error_statuses_are_reported() {
    let server = TestServer::start(|request| match request.path() {
        "/gone" => Response::new(404, "text/html", "<h1>Not found</h1>"),
        "/forbidden" => Response::new(403, "text/html", "no"),
        "/busy" => Response::new(429, "text/html", "slow down"),
        "/broken" => Response::new(500, "text/html", "oops"),
        _ => Response::html("ok"),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    for (path, status) in [
        ("/gone", 404),
        ("/forbidden", 403),
        ("/busy", 429),
        ("/broken", 500),
    ] {
        assert_eq!(
            fetcher
                .get(&server.url("public.test", path), Accept::Page)
                .await,
            Err(FetchError::Status(status)),
            "{path}"
        );
    }
}

#[tokio::test]
async fn a_long_body_is_read_up_to_the_limit() {
    let server = TestServer::start(|request| match request.path() {
        "/big" => Response::html(vec![b'a'; 3 * 1024 * 1024]),
        _ => Response::html(vec![b'b'; 5000]),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let big = fetcher
        .get(&server.url("public.test", "/big"), Accept::Page)
        .await
        .unwrap();
    assert_eq!(big.text.len(), search::fetch::MAX_BODY_BYTES);
    assert!(big.truncated);
    let small = fetcher
        .get(&server.url("public.test", "/small"), Accept::Page)
        .await
        .unwrap();
    assert_eq!(small.text.len(), 5000);
    assert!(!small.truncated);
    let capped = fetcher
        .clone()
        .with_max_bytes(1000)
        .get(&server.url("public.test", "/small"), Accept::Page)
        .await
        .unwrap();
    assert_eq!(capped.text.len(), 1000);
    assert!(capped.truncated);
}

#[tokio::test]
async fn a_site_that_does_not_answer_in_time_is_given_up() {
    let server = TestServer::start(|request| match request.path() {
        "/slow" => Response::html("late").after(Duration::from_secs(20)),
        _ => Response::redirect(302, "/slow"),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_timeout(Duration::from_millis(400))
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    for path in ["/slow", "/redirect-then-slow"] {
        let started = Instant::now();
        let err = fetcher
            .get(&server.url("public.test", path), Accept::Page)
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Timeout(_)), "{path}: {err:?}");
        // The limit covers the whole fetch, redirects included.
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{path}: {:?}",
            started.elapsed()
        );
    }
    assert_eq!(search::fetch::DEFAULT_TIMEOUT, Duration::from_secs(10));
}

/// The request names the app and its version, and carries nothing about the user: no
/// cookie, no referrer, no language, no credentials.
#[tokio::test]
async fn the_request_says_who_asks_and_nothing_about_the_user() {
    let server = TestServer::start(|request| match request.path() {
        "/start" => {
            Response::redirect(302, "/end").with_header("Set-Cookie", "sid=tracking; Path=/")
        }
        _ => Response::html("done").with_header("Set-Cookie", "sid2=more"),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    for _ in 0..2 {
        fetcher
            .get(&server.url("public.test", "/start"), Accept::Page)
            .await
            .unwrap();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    for request in &requests {
        assert_eq!(request.header("user-agent"), Some(UA));
        assert_eq!(request.header("accept-encoding"), Some("identity"));
        assert!(request.header("accept").unwrap().contains("text/html"));
        let mut names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["accept", "accept-encoding", "host", "user-agent"],
            "{:?}",
            request.headers
        );
    }
    let json = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let _ = json
        .get(&server.url("public.test", "/x"), Accept::Json)
        .await;
    assert_eq!(
        server.requests()[4].header("accept"),
        Some("application/json")
    );
}

#[tokio::test]
async fn text_is_decoded_and_packed_bodies_are_refused() {
    let server = TestServer::start(|request| match request.path() {
        "/latin1" => Response::new(
            200,
            "text/html; charset=iso-8859-1",
            b"<p>caf\xE9</p>".to_vec(),
        ),
        "/meta" => Response::new(
            200,
            "text/html",
            b"<meta charset=windows-1252><p>\x93q\x94</p>".to_vec(),
        ),
        "/gzip" => Response::html(vec![0x1f, 0x8b, 8, 0]).with_header("Content-Encoding", "gzip"),
        "/identity" => Response::html("plain").with_header("Content-Encoding", "identity"),
        _ => Response::html("naïve"),
    })
    .await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[server.port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let text = |path: &'static str| {
        let fetcher = fetcher.clone();
        let address = server.url("public.test", path);
        async move { fetcher.get(&address, Accept::Page).await.map(|p| p.text) }
    };
    assert_eq!(text("/latin1").await.unwrap(), "<p>café</p>");
    assert_eq!(
        text("/meta").await.unwrap(),
        "<meta charset=windows-1252><p>\u{201C}q\u{201D}</p>"
    );
    assert_eq!(text("/utf8").await.unwrap(), "naïve");
    assert_eq!(text("/identity").await.unwrap(), "plain");
    assert_eq!(
        text("/gzip").await,
        Err(FetchError::Encoding("gzip".into()))
    );
}

#[tokio::test]
async fn a_closed_port_is_a_connection_failure() {
    // A port that was free a moment ago: nothing listens there.
    let port = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[port])
        .with_test_dns(&[("public.test", ip("127.0.0.1"))]);
    let err = fetcher
        .get(&url(&format!("http://public.test:{port}/")), Accept::Page)
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Network(_)), "{err:?}");
    assert!(!err.is_refusal());
}

// Result files (Studio): a picture or video from a company's file host, read in binary,
// streamed to a file under the same address rules, and never with a key.

/// A fetcher that reaches `servers` under the names `files.test` and `other.test`.
fn file_fetcher(servers: &[&TestServer]) -> Fetcher {
    let ports: Vec<u16> = servers.iter().map(|s| s.port).collect();
    Fetcher::new(UA)
        .with_test_loopback_ports(&ports)
        .with_test_dns(&[
            ("files.test", ip("127.0.0.1")),
            ("other.test", ip("127.0.0.1")),
        ])
}

/// Bytes that are not text.
fn video_bytes(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// The file `fetch_to_file` writes while it reads.
fn part_of(dest: &Path) -> std::path::PathBuf {
    dest.with_extension("part")
}

/// Neither the result nor its partial file is on disk.
fn nothing_saved(dest: &Path) -> bool {
    !dest.exists() && !part_of(dest).exists()
}

/// A server for what `TestServer` cannot send: it reads the request, writes `answer` as
/// it is (a head, then some body), then closes the connection (a body without a length
/// ends there) or, with `hold`, keeps it open and sends nothing more.
async fn raw_server(answer: Vec<u8>, hold: bool) -> u16 {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((mut socket, _)) = listener.accept().await {
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                match socket.read(&mut chunk).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => request.extend_from_slice(&chunk[..n]),
                }
            }
            let _ = socket.write_all(&answer).await;
            if hold {
                held.push(socket);
            } else {
                let _ = socket.shutdown().await;
            }
        }
    });
    port
}

#[tokio::test]
async fn fetch_to_file_saves_a_video() {
    let body = video_bytes(300_000);
    let sent = body.clone();
    let server =
        TestServer::start(move |_| Response::new(200, "Video/MP4; codecs=avc1", sent.clone()))
            .await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("result.mp4");
    let address = server.url("files.test", "/v.mp4?sig=abc");
    let saved = file_fetcher(&[&server])
        .fetch_to_file(&address, 1 << 20, &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body);
    assert_eq!(saved.len, body.len() as u64);
    assert_eq!(saved.content_type, "video/mp4");
    assert_eq!(saved.url, address);
    assert_eq!(saved.redirects, 0);
    assert!(
        !part_of(&dest).exists(),
        "the partial file became the result"
    );
    assert_eq!(server.requests()[0].target, "/v.mp4?sig=abc");

    // Every kind of picture and video Studio keeps, and an earlier file of the same name
    // is replaced.
    let server = TestServer::start(|request| {
        let index: usize = request.path().trim_start_matches("/file/").parse().unwrap();
        let kind = MEDIA_TYPES[index];
        Response::new(200, kind, kind.as_bytes())
    })
    .await;
    let fetcher = file_fetcher(&[&server]);
    let dest = dir.path().join("each");
    for (index, kind) in MEDIA_TYPES.iter().enumerate() {
        let address = server.url("files.test", &format!("/file/{index}"));
        let saved = fetcher.fetch_to_file(&address, 1024, &dest).await.unwrap();
        assert_eq!(saved.content_type, *kind);
        assert_eq!(std::fs::read(&dest).unwrap(), kind.as_bytes());
        assert_eq!(saved.len, kind.len() as u64);
    }
}

/// The request carries the app's name and what it accepts, and nothing that could be a
/// key: no `Authorization`, no cookie, and an address with a user name or password is
/// refused before anything is sent.
#[tokio::test]
async fn fetch_to_file_never_sends_authorization() {
    let server = TestServer::start(|request| match request.path() {
        "/start" => {
            Response::redirect(302, "/v.webm").with_header("Set-Cookie", "sid=tracking; Path=/")
        }
        _ => Response::new(200, "video/webm", video_bytes(64)).with_header("Set-Cookie", "b=2"),
    })
    .await;
    let fetcher = file_fetcher(&[&server]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.webm");
    for _ in 0..2 {
        fetcher
            .fetch_to_file(&server.url("files.test", "/start"), 1 << 20, &dest)
            .await
            .unwrap();
    }
    let requests = server.requests();
    assert_eq!(requests.len(), 4);
    let accept = MEDIA_TYPES.join(", ");
    for request in &requests {
        assert_eq!(request.header("authorization"), None);
        assert_eq!(request.header("cookie"), None);
        assert_eq!(request.header("user-agent"), Some(UA));
        assert_eq!(request.header("accept"), Some(accept.as_str()));
        assert_eq!(request.header("accept-encoding"), Some("identity"));
        let mut names: Vec<&str> = request.headers.iter().map(|(n, _)| n.as_str()).collect();
        names.sort_unstable();
        assert_eq!(
            names,
            ["accept", "accept-encoding", "host", "user-agent"],
            "{:?}",
            request.headers
        );
    }
    for address in [
        format!("http://user:sk-secret@files.test:{}/v.webm", server.port),
        format!("http://sk-secret@files.test:{}/v.webm", server.port),
    ] {
        let dest = dir.path().join("keyed.webm");
        assert_eq!(
            fetcher.fetch_to_file(&url(&address), 1 << 20, &dest).await,
            Err(FetchError::Credentials)
        );
        assert!(nothing_saved(&dest));
    }
    assert_eq!(server.connections(), 4);
}

#[tokio::test]
async fn fetch_to_file_refuses_a_private_address() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.mp4");
    let err = Fetcher::new("t")
        .fetch_to_file(&url("http://127.0.0.1:9/v.mp4"), 1 << 20, &dest)
        .await
        .unwrap_err();
    assert!(err.is_refusal());
    assert!(!dest.exists());
    assert!(nothing_saved(&dest));

    // The app's fetcher, no hook: nothing on this computer or its network is reached.
    let trap = trap().await;
    let p = trap.port;
    let fetcher = Fetcher::new(UA);
    for address in [
        format!("http://127.0.0.1:{p}/v.mp4"),
        format!("http://[::1]:{p}/v.mp4"),
        format!("http://localhost:{p}/v.mp4"),
        "http://10.0.0.1/v.mp4".to_string(),
        "http://169.254.169.254/latest/meta-data/".to_string(),
        "file:///etc/passwd".to_string(),
    ] {
        let err = fetcher
            .fetch_to_file(&url(&address), 1 << 20, &dest)
            .await
            .unwrap_err();
        assert!(err.is_refusal(), "{address}: {err:?}");
        assert!(nothing_saved(&dest), "{address}");
    }
    assert_eq!(trap.connections(), 0);
}

/// Object stores (S3, GCS, R2) send a file uploaded without a type as
/// `application/octet-stream`, `binary/octet-stream` or with no type at all: it is saved
/// with that type, and the caller checks its bytes.
#[tokio::test]
async fn fetch_to_file_takes_a_file_whose_type_names_nothing() {
    let body = video_bytes(4096);
    let sent = body.clone();
    let server = TestServer::start(move |request| match request.path() {
        "/octet" => Response::new(200, "application/octet-stream", sent.clone()),
        "/binary" => Response::new(200, "Binary/Octet-Stream", sent.clone()),
        _ => Response {
            status: 200,
            headers: Vec::new(),
            body: sent.clone(),
            delay: Duration::ZERO,
        },
    })
    .await;
    let fetcher = file_fetcher(&[&server]);
    let dir = tempfile::tempdir().unwrap();
    for (path, kind) in [
        ("/octet", "application/octet-stream"),
        ("/binary", "binary/octet-stream"),
        ("/untyped", ""),
    ] {
        let dest = dir.path().join(path.trim_start_matches('/'));
        let saved = fetcher
            .fetch_to_file(&server.url("files.test", path), 1 << 20, &dest)
            .await
            .unwrap_or_else(|e| panic!("{path}: {e:?}"));
        assert_eq!(saved.content_type, kind, "{path}");
        assert_eq!(std::fs::read(&dest).unwrap(), body, "{path}");
        assert!(!part_of(&dest).exists(), "{path}");
    }
}

#[tokio::test]
async fn fetch_to_file_refuses_a_page() {
    let server = TestServer::start(|request| match request.path() {
        "/page" => Response::html("<p>Sign in to see this file</p>"),
        "/plain" => Response::new(200, "text/plain", "words"),
        "/json" => Response::json("{\"error\": \"expired\"}"),
        "/xhtml" => Response::new(200, "application/xhtml+xml", "<svg/>"),
        "/gzip" => Response::new(200, "image/png", vec![0x1f, 0x8b, 8, 0])
            .with_header("Content-Encoding", "gzip"),
        "/gone" => Response::new(404, "text/html", "<h1>Not found</h1>"),
        _ => Response::new(200, "image/png", video_bytes(16)),
    })
    .await;
    let fetcher = file_fetcher(&[&server]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("picture.png");
    let fetch = |path: &'static str| {
        let fetcher = fetcher.clone();
        let address = server.url("files.test", path);
        let dest = dest.clone();
        async move { fetcher.fetch_to_file(&address, 1 << 20, &dest).await }
    };
    assert_eq!(
        fetch("/page").await,
        Err(FetchError::NotMedia("text/html".into()))
    );
    assert!(nothing_saved(&dest));
    for (path, kind) in [
        ("/plain", "text/plain"),
        ("/json", "application/json"),
        ("/xhtml", "application/xhtml+xml"),
    ] {
        assert_eq!(
            fetch(path).await,
            Err(FetchError::NotMedia(kind.into())),
            "{path}"
        );
        assert!(nothing_saved(&dest), "{path}");
    }
    assert_eq!(
        fetch("/gzip").await,
        Err(FetchError::Encoding("gzip".into()))
    );
    assert_eq!(fetch("/gone").await, Err(FetchError::Status(404)));
    assert!(nothing_saved(&dest));
    for error in [
        FetchError::NotMedia("text/html".into()),
        FetchError::TooLarge(1024),
        FetchError::DiskFull,
        FetchError::Disk("denied".into()),
    ] {
        assert!(!error.is_refusal(), "{error:?}");
    }
    // Reading pages is unchanged: a picture is not a page.
    assert_eq!(
        fetcher
            .get(&server.url("files.test", "/png"), Accept::Page)
            .await,
        Err(FetchError::ContentType("image/png".into()))
    );
}

#[tokio::test]
async fn fetch_to_file_stops_at_the_cap() {
    let server = TestServer::start(|request| match request.path() {
        "/big" => Response::new(200, "video/mp4", video_bytes(4096)),
        _ => Response::new(200, "video/mp4", video_bytes(1024)),
    })
    .await;
    let fetcher = file_fetcher(&[&server]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.mp4");
    assert_eq!(
        fetcher
            .fetch_to_file(&server.url("files.test", "/big"), 1024, &dest)
            .await,
        Err(FetchError::TooLarge(1024))
    );
    assert!(nothing_saved(&dest));
    // Exactly the cap is allowed.
    let saved = fetcher
        .fetch_to_file(&server.url("files.test", "/exact"), 1024, &dest)
        .await
        .unwrap();
    assert_eq!(saved.len, 1024);
    assert_eq!(std::fs::read(&dest).unwrap(), video_bytes(1024));

    // A body that does not say how long it is is counted as it arrives.
    let mut answer =
        b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nConnection: close\r\n\r\n".to_vec();
    answer.extend_from_slice(&video_bytes(4096));
    let port = raw_server(answer, false).await;
    let fetcher = Fetcher::new(UA)
        .with_test_loopback_ports(&[port])
        .with_test_dns(&[("files.test", ip("127.0.0.1"))]);
    let address = url(&format!("http://files.test:{port}/v.mp4"));
    let dest = dir.path().join("unsized.mp4");
    assert_eq!(
        fetcher.fetch_to_file(&address, 1024, &dest).await,
        Err(FetchError::TooLarge(1024))
    );
    assert!(nothing_saved(&dest));
    let saved = fetcher.fetch_to_file(&address, 4096, &dest).await.unwrap();
    assert_eq!(saved.len, 4096);
    assert_eq!(std::fs::read(&dest).unwrap(), video_bytes(4096));
}

#[tokio::test]
async fn fetch_to_file_follows_redirects_by_hand() {
    let other = TestServer::start(|_| Response::new(200, "image/webp", video_bytes(512))).await;
    let other_port = other.port;
    let files = TestServer::start(move |request| match request.path() {
        "/to-other" => {
            Response::redirect(302, &format!("http://other.test:{other_port}/result.webp"))
        }
        "/to-private" => Response::redirect(307, "http://10.0.0.1/result.webp"),
        "/loop" => Response::redirect(302, "/loop"),
        _ => Response::status(404),
    })
    .await;
    let fetcher = file_fetcher(&[&files, &other]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("result.webp");
    let saved = fetcher
        .fetch_to_file(&files.url("files.test", "/to-other"), 1 << 20, &dest)
        .await
        .unwrap();
    assert_eq!(saved.redirects, 1);
    assert_eq!(saved.url, other.url("other.test", "/result.webp"));
    assert_eq!(saved.content_type, "image/webp");
    assert_eq!(std::fs::read(&dest).unwrap(), video_bytes(512));
    assert_eq!(
        other.requests()[0].header("host"),
        Some(format!("other.test:{other_port}").as_str())
    );

    let dest = dir.path().join("private.webp");
    let err = fetcher
        .fetch_to_file(&files.url("files.test", "/to-private"), 1 << 20, &dest)
        .await
        .unwrap_err();
    assert!(
        matches!(
            err,
            FetchError::Blocked {
                class: AddressClass::Private,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(nothing_saved(&dest));
    assert_eq!(
        fetcher
            .fetch_to_file(&files.url("files.test", "/loop"), 1 << 20, &dest)
            .await,
        Err(FetchError::TooManyRedirects)
    );
    assert!(nothing_saved(&dest));
}

/// A download that stops sending runs out of time, and what it wrote so far is removed.
#[tokio::test]
async fn fetch_to_file_gives_up_on_a_stalled_download_and_removes_the_partial_file() {
    let mut answer =
        b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nContent-Length: 100000\r\n\r\n".to_vec();
    answer.extend_from_slice(&video_bytes(2048));
    let port = raw_server(answer, true).await;
    let fetcher = Fetcher::new(UA)
        .with_timeout(Duration::from_millis(500))
        .with_test_loopback_ports(&[port])
        .with_test_dns(&[("files.test", ip("127.0.0.1"))]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("v.mp4");
    let started = Instant::now();
    let err = fetcher
        .fetch_to_file(
            &url(&format!("http://files.test:{port}/v.mp4")),
            1 << 20,
            &dest,
        )
        .await
        .unwrap_err();
    assert!(matches!(err, FetchError::Timeout(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(nothing_saved(&dest));
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}
