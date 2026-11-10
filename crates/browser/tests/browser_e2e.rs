//! End-to-end tests of the browser server against a real Chromium in headless mode and
//! local HTTP servers started by each test.
//!
//! They need a browser: `HARNISS_BROWSER_PATH`, an installed Chrome/Edge/Chromium, or
//! Playwright's Chromium under `/opt/pw-browsers`. Without one, each test prints a note
//! and passes, so CI machines without a browser still pass. On Windows and macOS they run
//! only with `HARNISS_BROWSER_E2E=1`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use browser::{
    BrowserConfig, BrowserServer, ConnectionMode, HeldNavigation, HoldKind, NavigationApprover,
};
use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described, ToolOutput};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

// ----- a tiny HTTP server ----------------------------------------------------------------

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
}

fn html(body: &str) -> Response {
    Response {
        status: 200,
        headers: vec![("Content-Type".into(), "text/html; charset=utf-8".into())],
        body: body.to_string(),
    }
}

fn redirect(to: &str) -> Response {
    Response {
        status: 302,
        headers: vec![("Location".into(), to.to_string())],
        body: String::new(),
    }
}

type Handler = Arc<dyn Fn(&str, &str, &str) -> Response + Send + Sync>;

struct Site {
    port: u16,
    /// Every request seen: "METHOD /path body".
    log: Arc<Mutex<Vec<String>>>,
}

impl Site {
    fn origin(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.origin())
    }
    fn posts(&self) -> usize {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("POST "))
            .count()
    }
}

async fn start_site(handler: Handler) -> Site {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log: Arc<Mutex<Vec<String>>> = Arc::default();
    let log2 = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let handler = handler.clone();
            let log = log2.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let head_end = loop {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let mut lines = head.lines();
                let first = lines.next().unwrap_or("");
                let mut parts = first.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let path = parts.next().unwrap_or("/").to_string();
                let length: usize = lines
                    .filter_map(|l| l.split_once(':'))
                    .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .and_then(|(_, v)| v.trim().parse().ok())
                    .unwrap_or(0);
                let mut body = buf[head_end..].to_vec();
                while body.len() < length {
                    let Ok(n) = sock.read(&mut chunk).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    body.extend_from_slice(&chunk[..n]);
                }
                let body = String::from_utf8_lossy(&body).to_string();
                log.lock().unwrap().push(format!("{method} {path} {body}"));
                let r = handler(&method, &path, &body);
                let reason = if r.status == 200 { "OK" } else { "Found" };
                let mut out = format!(
                    "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n",
                    r.status,
                    r.body.len()
                );
                for (k, v) in &r.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&r.body);
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Site { port, log }
}

// ----- a scripted approver ---------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Allow,
    Deny,
    /// Never answers; returns false once cancelled.
    Never,
}

struct Approver {
    answer: Mutex<Answer>,
    seen: Mutex<Vec<HeldNavigation>>,
    /// The chat each card was shown in.
    chats: Mutex<Vec<i64>>,
    cancelled: AtomicUsize,
}

impl Approver {
    fn new(answer: Answer) -> Arc<Self> {
        Arc::new(Self {
            answer: Mutex::new(answer),
            seen: Mutex::new(Vec::new()),
            chats: Mutex::new(Vec::new()),
            cancelled: AtomicUsize::new(0),
        })
    }
    fn set(&self, a: Answer) {
        *self.answer.lock().unwrap() = a;
    }
    fn seen(&self) -> Vec<HeldNavigation> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait]
impl NavigationApprover for Approver {
    async fn approve(&self, chat: i64, request: HeldNavigation, cancel: CancellationToken) -> bool {
        self.chats.lock().unwrap().push(chat);
        self.seen.lock().unwrap().push(request);
        let answer = *self.answer.lock().unwrap();
        match answer {
            Answer::Allow => true,
            Answer::Deny => false,
            Answer::Never => {
                cancel.cancelled().await;
                self.cancelled.fetch_add(1, Ordering::SeqCst);
                false
            }
        }
    }
}

// ----- harness ---------------------------------------------------------------------------

fn have_browser() -> bool {
    // On Windows and macOS CI runners Chrome is usually installed; these tests run there
    // only when asked to, so a CI job opts in deliberately.
    if !cfg!(target_os = "linux") && std::env::var_os("HARNISS_BROWSER_E2E").is_none() {
        eprintln!(
            "skipping browser end-to-end test: set HARNISS_BROWSER_E2E=1 to run it against this computer's browser"
        );
        return false;
    }
    match browser::find_browser(None) {
        Ok(_) => true,
        Err(e) => {
            eprintln!("skipping browser end-to-end test: {e}");
            false
        }
    }
}

struct Harness {
    server: BrowserServer,
    approver: Arc<Approver>,
    _profile: tempfile::TempDir,
}

fn harness(ports: &[u16], answer: Answer, mode: ConnectionMode) -> Harness {
    harness_with(ports, answer, mode, |_| {})
}

fn harness_with(
    ports: &[u16],
    answer: Answer,
    mode: ConnectionMode,
    tweak: impl FnOnce(&mut BrowserConfig),
) -> Harness {
    let profile = tempfile::tempdir().unwrap();
    let mut config = BrowserConfig::new(profile.path().join("profile"));
    config.headless = true;
    config.no_sandbox = true;
    config.connection = mode;
    config.navigation_timeout = Duration::from_secs(15);
    config.approval_timeout = Duration::from_secs(20);
    config.test_allowed_loopback_ports = ports.to_vec();
    tweak(&mut config);
    let approver = Approver::new(answer);
    Harness {
        server: BrowserServer::new(config, approver.clone()),
        approver,
        _profile: profile,
    }
}

fn chat() -> ChatContext {
    ChatContext {
        chat_id: 7,
        result_cap_tokens: 4000,
        ..Default::default()
    }
}

fn args(v: Value) -> Map<String, Value> {
    v.as_object().cloned().unwrap_or_default()
}

impl Harness {
    async fn describe(&self, tool: &str, a: Value) -> policy::Action {
        match self.server.describe(tool, &args(a), &chat()).await {
            Described::Action(action) => action,
            Described::Invalid(why) => panic!("{tool} was not describable: {why}"),
        }
    }

    async fn call_action(
        &self,
        tool: &str,
        a: Value,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        self.server
            .call(
                tool,
                &args(a),
                action,
                CallContext {
                    chat: chat(),
                    cancel,
                    progress: None,
                },
            )
            .await
    }

    /// describe + call, as if the user allowed the card.
    async fn run(&self, tool: &str, a: Value) -> (policy::Action, ToolOutput) {
        let action = self.describe(tool, a.clone()).await;
        assert!(
            action.hard_limit.is_none(),
            "{tool}: hard limit {:?}",
            action.hard_limit
        );
        let out = self
            .call_action(tool, a, &action, CancellationToken::new())
            .await;
        (action, out)
    }

    async fn ok(&self, tool: &str, a: Value) -> String {
        let (_, out) = self.run(tool, a).await;
        assert!(!out.is_error, "{tool} failed: {}", out.joined_text());
        out.joined_text()
    }
}

fn ref_of(text: &str, needle: &str) -> String {
    let line = text
        .lines()
        .find(|l| l.contains(needle) && l.contains("[ref="))
        .unwrap_or_else(|| panic!("no element {needle:?} with a ref in:\n{text}"));
    let start = line.find("[ref=").unwrap() + 5;
    let end = line[start..].find(']').unwrap() + start;
    line[start..end].to_string()
}

async fn eventually(mut f: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn routes(pages: HashMap<&'static str, String>) -> Handler {
    Arc::new(move |method, path, _body| {
        if method == "POST" && path == "/submit" {
            return html("<title>Done</title><h1>submitted</h1>");
        }
        if method == "POST" && path == "/api" {
            return html("saved");
        }
        if path.starts_with("/search") {
            return html("<title>Results</title><h1>results</h1>");
        }
        match pages.get(path) {
            Some(body) => html(body),
            None => Response {
                status: 404,
                headers: vec![],
                body: "not found".into(),
            },
        }
    })
}

// ----- tests -----------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn navigate_snapshot_click_and_type() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        r#"<title>Home</title><h1>Welcome</h1>
        <input id=name aria-label="Your name">
        <button onclick="document.getElementById('out').textContent='Hello, '+document.getElementById('name').value">Greet</button>
        <button onclick="this.textContent='Counted'">Count</button>
        <p id=out>nothing yet</p>
        <div style="height:3000px"></div><button>Far below</button>"#
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);

    let text = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    assert!(text.contains("heading \"Welcome\""), "{text}");
    assert!(text.contains("textbox \"Your name\""), "{text}");
    assert!(text.contains("button \"Greet\""), "{text}");
    assert!(
        !text.contains("Far below"),
        "the snapshot is limited to the viewport:\n{text}"
    );
    assert!(text.contains("outside the view"), "{text}");

    let found = h.ok("browser_find", json!({"query": "far below"})).await;
    assert!(found.contains("button \"Far below\""), "{found}");

    // A click before typing on a neutral button needs no submit card.
    let count = ref_of(&text, "button \"Count\"");
    let (action, out) = h.run("browser_click", json!({"ref": count})).await;
    assert_eq!(action.risk, policy::Risk::Normal);
    assert_eq!(
        action.scope,
        policy::Scope::Site {
            origin: site.origin()
        }
    );
    assert!(action.outbound && !action.read_only);
    assert!(!out.is_error, "{}", out.joined_text());

    let name = ref_of(&text, "textbox \"Your name\"");
    let typed = h
        .ok("browser_type", json!({"ref": name, "text": "Ada"}))
        .await;
    assert!(typed.contains("Typed 3 character(s)"), "{typed}");

    // D8: a button click after the agent typed on the page always asks.
    let greet = ref_of(&text, "button \"Greet\"");
    let (action, out) = h.run("browser_click", json!({"ref": greet})).await;
    assert_eq!(action.risk, policy::Risk::AlwaysAsk);
    assert!(!out.is_error, "{}", out.joined_text());
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(snap.contains("Hello, Ada"), "{snap}");
    assert!(snap.contains("[value=\"Ada\"]"), "{snap}");

    let text = h.ok("browser_read_text", json!({})).await;
    assert!(text.contains("Hello, Ada"), "{text}");

    let (action, out) = h.run("browser_screenshot", json!({})).await;
    assert!(action.read_only && !action.outbound);
    assert!(
        out.content
            .iter()
            .any(|c| matches!(c, Content::Image { mime, .. } if mime == "image/png"))
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn redirects_to_private_and_loopback_addresses_are_blocked_at_the_request() {
    if !have_browser() {
        return;
    }
    // A loopback port nobody is allowed to reach.
    let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let closed_port = closed.local_addr().unwrap().port();
    let handler: Handler = Arc::new(move |_, path, _| match path {
        "/to-private" => redirect("http://10.255.255.1/router"),
        "/to-loopback" => redirect(&format!("http://127.0.0.1:{closed_port}/llama")),
        _ => html(&format!(
            "<title>Start</title><a href=\"http://127.0.0.1:{closed_port}/x\">local link</a>"
        )),
    });
    let site = start_site(handler).await;
    let h = harness(&[site.port], Answer::Allow, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;

    let (_, out) = h
        .run("browser_navigate", json!({"url": site.url("/to-private")}))
        .await;
    assert!(out.is_error, "{}", out.joined_text());
    let text = out.joined_text();
    assert!(
        text.contains("10.255.255.1") && text.contains("private"),
        "{text}"
    );

    let (_, out) = h
        .run("browser_navigate", json!({"url": site.url("/to-loopback")}))
        .await;
    assert!(out.is_error, "{}", out.joined_text());
    assert!(
        out.joined_text().contains("loopback"),
        "{}",
        out.joined_text()
    );

    // Typing the blocked address directly is a hard limit: no card at all.
    let action = h
        .describe(
            "browser_navigate",
            json!({"url": format!("http://127.0.0.1:{closed_port}/")}),
        )
        .await;
    assert!(
        action
            .hard_limit
            .as_deref()
            .is_some_and(|w| w.contains("loopback"))
    );

    // A link on the page to the blocked address is blocked at the request too.
    let snap = h.ok("browser_snapshot", json!({})).await;
    let link = ref_of(&snap, "link \"local link\"");
    let (_, out) = h.run("browser_click", json!({"ref": link})).await;
    assert!(
        out.joined_text().contains("Blocked a request"),
        "{}",
        out.joined_text()
    );
    let url = h.server.active_url().await.unwrap();
    assert!(url.starts_with(&site.origin()), "{url}");
    drop(closed);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_chrome_and_data_urls_are_blocked() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Links</title><a href=\"file:///etc/hostname\">local file</a><a href=\"chrome://version\">chrome page</a>".to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Allow, ConnectionMode::Pipe);
    for url in [
        "file:///etc/hostname",
        "chrome://version",
        "devtools://devtools/x",
        "data:text/html,hi",
    ] {
        let action = h.describe("browser_navigate", json!({"url": url})).await;
        let why = action.hard_limit.unwrap_or_default();
        assert!(why.contains("blocked"), "{url}: {why}");
    }
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    for name in ["link \"local file\"", "link \"chrome page\""] {
        let r = ref_of(&snap, name);
        let (_, _out) = h.run("browser_click", json!({"ref": r})).await;
        let url = h.server.active_url().await.unwrap();
        assert!(url.starts_with("http://"), "{name}: the tab went to {url}");
    }
    h.server.shutdown().await;
}

fn form_pages() -> HashMap<&'static str, String> {
    let mut pages = HashMap::new();
    pages.insert(
        "/script-form",
        r#"<title>Script form</title>
        <form method=post action=/submit><input name=q aria-label=Query value=secret></form>
        <div onclick="document.forms[0].submit()" style="padding:20px">Continue here</div>"#
            .to_string(),
    );
    pages.insert(
        "/plain-form",
        r#"<title>Plain form</title>
        <form method=post action=/submit><input name=q aria-label=Query><button>Go</button></form>"#
            .to_string(),
    );
    pages
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_post_submission_is_held_and_failed_when_the_user_says_no() {
    if !have_browser() {
        return;
    }
    let site = start_site(routes(form_pages())).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/script-form")}))
        .await;
    let found = h
        .ok("browser_find", json!({"query": "Continue here"}))
        .await;
    let r = ref_of(&found, "Continue here");
    // The div has no submit intent: the click itself needs no always-ask card...
    let (action, out) = h.run("browser_click", json!({"ref": r})).await;
    assert_eq!(action.risk, policy::Risk::Normal);
    // ...but the POST it causes is held, and failed on Deny.
    let seen = h.approver.seen();
    assert_eq!(seen.len(), 1, "{}", out.joined_text());
    assert_eq!(seen[0].kind, HoldKind::Submission);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].origin, site.origin());
    assert_eq!(seen[0].during_tool.as_deref(), Some("browser_click"));
    assert_eq!(site.posts(), 0, "the server never saw the POST");
    assert!(
        out.joined_text().contains("did not allow"),
        "{}",
        out.joined_text()
    );
    assert_eq!(h.server.held_requests().await, 0);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_post_submission_is_released_when_the_user_says_yes() {
    if !have_browser() {
        return;
    }
    let site = start_site(routes(form_pages())).await;
    let h = harness(&[site.port], Answer::Allow, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/script-form")}))
        .await;
    let found = h
        .ok("browser_find", json!({"query": "Continue here"}))
        .await;
    let r = ref_of(&found, "Continue here");
    let text = h.ok("browser_click", json!({"ref": r})).await;
    assert_eq!(h.approver.seen().len(), 1);
    eventually(|| site.posts() == 1, "the POST to arrive").await;
    assert!(
        site.log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("q=secret"))
    );
    assert!(text.contains("submitted"), "{text}");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_approved_submit_intent_releases_its_post_without_a_second_prompt() {
    if !have_browser() {
        return;
    }
    let site = start_site(routes(form_pages())).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/plain-form")}))
        .await;
    let go = ref_of(&snap, "button \"Go\"");
    let (action, out) = h.run("browser_click", json!({"ref": go})).await;
    assert_eq!(
        action.risk,
        policy::Risk::AlwaysAsk,
        "a submit control always asks"
    );
    assert!(action.preview.contains("POST"), "{}", action.preview);
    assert!(!out.is_error, "{}", out.joined_text());
    assert!(h.approver.seen().is_empty(), "no second prompt");
    eventually(|| site.posts() == 1, "the POST to arrive").await;
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn held_requests_are_released_on_stop_within_a_second() {
    if !have_browser() {
        return;
    }
    let site = start_site(routes(form_pages())).await;
    let h = harness(&[site.port], Answer::Never, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/script-form")}))
        .await;
    let found = h
        .ok("browser_find", json!({"query": "Continue here"}))
        .await;
    let r = ref_of(&found, "Continue here");
    let a = json!({"ref": r});
    let action = h.describe("browser_click", a.clone()).await;
    let cancel = CancellationToken::new();
    let stopper = {
        let cancel = cancel.clone();
        let approver = h.approver.clone();
        tokio::spawn(async move {
            eventually(|| !approver.seen().is_empty(), "the hold").await;
            let at = Instant::now();
            cancel.cancel();
            at
        })
    };
    let out = h.call_action("browser_click", a, &action, cancel).await;
    let returned = Instant::now();
    let stopped_at = stopper.await.unwrap();
    assert!(
        out.is_error && out.joined_text().contains("Stopped"),
        "{}",
        out.joined_text()
    );
    assert!(returned.duration_since(stopped_at) < Duration::from_secs(1));
    eventually(
        || h.approver.cancelled.load(Ordering::SeqCst) == 1,
        "the card to be cancelled",
    )
    .await;
    let mut held = usize::MAX;
    for _ in 0..100 {
        held = h.server.held_requests().await;
        if held == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(held, 0, "the held request was released");
    assert_eq!(site.posts(), 0);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pop_up_is_adopted_and_back_closes_it() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/opener",
        "<title>Opener</title><a href=\"/popup\" target=\"_blank\">Open the pop-up</a>".to_string(),
    );
    pages.insert(
        "/popup",
        "<title>Popup</title><h1>Inside the pop-up</h1>".to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/opener")}))
        .await;
    let link = ref_of(&snap, "link \"Open the pop-up\"");
    let out = h.ok("browser_click", json!({"ref": link})).await;
    assert!(out.contains("pop-up"), "{out}");
    eventually_async(&h, |url| url.ends_with("/popup")).await;
    assert_eq!(h.server.tab_count().await, 2);
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(snap.contains("Inside the pop-up"), "{snap}");
    let back = h.ok("browser_back", json!({})).await;
    assert!(back.contains("Closed the pop-up"), "{back}");
    assert!(h.server.active_url().await.unwrap().ends_with("/opener"));
    h.server.shutdown().await;
}

async fn eventually_async(h: &Harness, f: impl Fn(&str) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(url) = h.server.active_url().await
            && f(&url)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out; tab at {:?}",
            h.server.active_url().await
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_site_recheck_refuses_a_call_whose_frame_moved_to_another_origin() {
    if !have_browser() {
        return;
    }
    let mut a_pages = HashMap::new();
    a_pages.insert("/", "<title>A</title><button>Press me</button>".to_string());
    let a = start_site(routes(a_pages)).await;
    let mut b_pages = HashMap::new();
    b_pages.insert("/", "<title>B</title><button>Press me</button>".to_string());
    let b = start_site(routes(b_pages)).await;
    let h = harness(&[a.port, b.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h.ok("browser_navigate", json!({"url": a.url("/")})).await;
    let press = ref_of(&snap, "button \"Press me\"");
    let click_args = json!({"ref": press});
    let approved = h.describe("browser_click", click_args.clone()).await;
    assert_eq!(approved.scope, policy::Scope::Site { origin: a.origin() });
    // The tab moves to another site before the approved click runs.
    let to_b = h
        .describe("browser_navigate", json!({"url": b.url("/")}))
        .await;
    assert_eq!(
        to_b.scope,
        policy::Scope::Site { origin: b.origin() },
        "a new site scope"
    );
    assert!(
        to_b.preview.contains("Leaves the current site"),
        "{}",
        to_b.preview
    );
    let out = h
        .call_action(
            "browser_navigate",
            json!({"url": b.url("/")}),
            &to_b,
            CancellationToken::new(),
        )
        .await;
    assert!(!out.is_error, "{}", out.joined_text());
    let out = h
        .call_action(
            "browser_click",
            click_args,
            &approved,
            CancellationToken::new(),
        )
        .await;
    assert!(out.is_error);
    assert!(
        out.joined_text().contains("Refused: the page is now on"),
        "{}",
        out.joined_text()
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_that_leaves_the_granted_site_is_held_for_approval() {
    if !have_browser() {
        return;
    }
    let mut b_pages = HashMap::new();
    b_pages.insert(
        "/",
        "<title>Other site</title><h1>Elsewhere</h1>".to_string(),
    );
    let b = start_site(routes(b_pages)).await;
    let mut a_pages = HashMap::new();
    a_pages.insert(
        "/",
        format!(
            "<title>A</title><a href=\"{}\">Go elsewhere</a>",
            b.url("/")
        ),
    );
    let a = start_site(routes(a_pages)).await;
    let h = harness(&[a.port, b.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h.ok("browser_navigate", json!({"url": a.url("/")})).await;
    let link = ref_of(&snap, "link \"Go elsewhere\"");
    let (action, out) = h.run("browser_click", json!({"ref": link})).await;
    assert_eq!(action.risk, policy::Risk::Normal);
    let seen = h.approver.seen();
    assert_eq!(seen.len(), 1, "{}", out.joined_text());
    assert_eq!(seen[0].kind, HoldKind::LeavesSite);
    assert_eq!(seen[0].origin, b.origin());
    assert!(
        h.server
            .active_url()
            .await
            .unwrap()
            .starts_with(&a.origin())
    );
    // Allowed, the same click goes through.
    h.approver.set(Answer::Allow);
    let (_, out) = h.run("browser_click", json!({"ref": link})).await;
    assert!(
        out.joined_text().contains("Elsewhere"),
        "{}",
        out.joined_text()
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn file_choosers_are_cancelled_with_a_message() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Upload</title><input type=file aria-label=\"Upload a file\">".to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let r = ref_of(&snap, "Upload a file");
    let (_, out) = h.run("browser_click", json!({"ref": r})).await;
    assert!(
        out.joined_text().contains("file chooser"),
        "{}",
        out.joined_text()
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_loopback_port_mode_works_too() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert("/", "<title>Port</title><h1>Over a port</h1>".to_string());
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::LoopbackPort);
    let text = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    assert!(text.contains("Over a port"), "{text}");
    h.server.shutdown().await;
    assert!(!h.server.is_running().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cross_site_frame_is_attached_enforced_and_clickable() {
    if !have_browser() {
        return;
    }
    let mut b_pages = HashMap::new();
    b_pages.insert(
        "/frame",
        "<title>Inner</title><button onclick=\"this.textContent='Inside clicked'\">Inside button</button>\
         <a href=\"http://10.255.255.1/\">private link</a>"
            .to_string(),
    );
    let b = start_site(routes(b_pages)).await;
    let frame_origin = format!("http://frame.test:{}", b.port);
    let frame_url = format!("{frame_origin}/frame");
    let mut a_pages = HashMap::new();
    a_pages.insert(
        "/",
        format!(
            "<title>Outer</title><h1>Outer page</h1>\
             <iframe src=\"{}\" style=\"width:400px;height:200px;margin-left:60px;margin-top:40px;border:5px solid black\"></iframe>",
            frame_url
        ),
    );
    let a = start_site(routes(a_pages)).await;
    // A different host name (mapped to 127.0.0.1) makes the frame cross-site, so the
    // browser runs it in its own process and attaches it as its own target.
    let h = harness_with(&[a.port, b.port], Answer::Deny, ConnectionMode::Pipe, |c| {
        c.test_extra_args
            .push("--host-resolver-rules=MAP frame.test 127.0.0.1".into());
        c.test_extra_args.push("--site-per-process".into());
        c.test_dns_overrides
            .push(("frame.test".into(), "127.0.0.1".parse().unwrap()));
    });
    let snap = h.ok("browser_navigate", json!({"url": a.url("/")})).await;
    assert!(snap.contains("Outer page"), "{snap}");
    assert!(
        snap.contains(&format!("- iframe \"{frame_origin}\"")),
        "{snap}"
    );
    assert_eq!(
        h.server.frame_sessions().await,
        1,
        "the frame runs in its own attached session"
    );
    let inside = ref_of(&snap, "button \"Inside button\"");
    let (action, out) = h.run("browser_click", json!({"ref": inside})).await;
    // The click touches the frame's site, not the page's.
    assert_eq!(
        action.scope,
        policy::Scope::Site {
            origin: frame_origin.clone()
        }
    );
    assert!(!out.is_error, "{}", out.joined_text());
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(
        snap.contains("Inside clicked"),
        "the click landed inside the frame:\n{snap}"
    );
    // Request enforcement runs in the frame's own session too.
    let link = ref_of(&snap, "link \"private link\"");
    let (_, out) = h.run("browser_click", json!({"ref": link})).await;
    let text = out.joined_text();
    assert!(
        text.contains("Blocked a request") && text.contains("10.255.255.1"),
        "{text}"
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn background_posts_are_held_only_in_strict_mode() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Draft</title><button onclick=\"fetch('/api',{method:'POST',body:'x'}).then(r=>r.text()).then(t=>document.getElementById('o').textContent=t).catch(e=>document.getElementById('o').textContent='failed')\">Save draft</button><p id=o>idle</p>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    // Default: background requests go through without a card.
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let save = ref_of(&snap, "button \"Save draft\"");
    h.ok("browser_click", json!({"ref": save})).await;
    eventually(|| site.posts() == 1, "the background POST").await;
    assert!(h.approver.seen().is_empty());
    h.server.shutdown().await;

    // Strict mode for the site: the same POST is held and failed on Deny.
    let origin = site.origin();
    let h = harness_with(&[site.port], Answer::Deny, ConnectionMode::Pipe, |c| {
        c.strict_sites.push(origin);
    });
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let save = ref_of(&snap, "button \"Save draft\"");
    h.ok("browser_click", json!({"ref": save})).await;
    eventually(|| !h.approver.seen().is_empty(), "the hold").await;
    assert_eq!(h.approver.seen()[0].kind, HoldKind::Background);
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(snap.contains("failed"), "{snap}");
    assert_eq!(site.posts(), 1, "no second POST reached the server");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_get_form_submission_by_script_is_held() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Search</title><form method=get action=/search><input name=q aria-label=Search value=cats></form>\
         <div onclick=\"document.forms[0].submit()\" style=\"padding:20px\">Search now</div>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let found = h.ok("browser_find", json!({"query": "Search now"})).await;
    let r = ref_of(&found, "Search now");
    let (action, _) = h.run("browser_click", json!({"ref": r})).await;
    assert_eq!(action.risk, policy::Risk::Normal);
    let seen = h.approver.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].kind, HoldKind::Submission);
    assert_eq!(seen[0].method, "GET");
    assert!(
        !site
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("/search"))
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn same_site_frames_vision_clicks_dialogs_and_downloads() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Mixed</title><h1>Top</h1>\
         <button style=\"position:absolute;left:100px;top:100px;width:200px;height:50px\" onclick=\"this.textContent='Hit by pixels'\">Target</button>\
         <button style=\"position:absolute;left:100px;top:200px\" onclick=\"alert('hello from the page')\">Alert me</button>\
         <a style=\"position:absolute;left:100px;top:260px\" href=\"/download\" download>Get file</a>\
         <iframe style=\"position:absolute;left:400px;top:100px\" src=\"/inner\"></iframe>\
         <iframe style=\"position:absolute;left:400px;top:3000px\" src=\"/far\"></iframe>"
            .to_string(),
    );
    pages.insert("/inner", "<p>Same site inner text</p>".to_string());
    pages.insert("/far", "<p>Far frame text</p>".to_string());
    pages.insert("/download", "file body".to_string());
    let site = start_site(routes(pages)).await;
    let h = harness_with(&[site.port], Answer::Deny, ConnectionMode::Pipe, |c| {
        c.test_extra_args
            .push("--force-device-scale-factor=2".into());
    });
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    assert_eq!(
        snap.matches("Same site inner text").count(),
        1,
        "listed once:\n{snap}"
    );
    assert!(
        !snap.contains("Far frame text"),
        "frames outside the view are left out:\n{snap}"
    );

    // A screenshot is in device pixels; the click is divided by the pixel ratio (2).
    let (_, shot) = h.run("browser_screenshot", json!({})).await;
    assert!(
        shot.joined_text().contains("device pixel ratio 2"),
        "{}",
        shot.joined_text()
    );
    let (action, out) = h.run("browser_click", json!({"x": 400, "y": 250})).await;
    assert!(action.preview.contains("(200, 125)"), "{}", action.preview);
    assert!(!out.is_error, "{}", out.joined_text());
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(snap.contains("Hit by pixels"), "{snap}");

    let alert = ref_of(&snap, "button \"Alert me\"");
    let out = h.ok("browser_click", json!({"ref": alert})).await;
    assert!(
        out.contains("alert dialog") && out.contains("hello from the page"),
        "{out}"
    );

    let file = ref_of(&snap, "link \"Get file\"");
    let out = h.ok("browser_click", json!({"ref": file})).await;
    assert!(out.contains("download"), "{out}");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_missing_browser_is_reported_with_a_next_step() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = BrowserConfig::new(dir.path().join("profile"));
    config.browser_path = Some(dir.path().join("no-such-browser"));
    let server = BrowserServer::new(config, Approver::new(Answer::Deny));
    assert!(server.availability().is_err());
    match server
        .describe(
            "browser_navigate",
            &args(json!({"url": "https://example.com"})),
            &chat(),
        )
        .await
    {
        Described::Invalid(why) => {
            assert!(
                why.contains("unavailable") && why.contains("Settings"),
                "{why}"
            );
        }
        Described::Action(a) => panic!("expected the browser to be unavailable, got {a:?}"),
    }
    match server
        .describe("browser_snapshot", &args(json!({})), &chat())
        .await
    {
        Described::Invalid(why) => assert!(why.contains("browser_navigate"), "{why}"),
        Described::Action(a) => panic!("expected no page, got {a:?}"),
    }
}

/// The browser processes started with `profile` (Linux: from /proc).
#[cfg(target_os = "linux")]
fn processes_using(profile: &std::path::Path) -> usize {
    let needle = format!("--user-data-dir={}", profile.display());
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_name()
                .to_string_lossy()
                .chars()
                .all(|c| c.is_ascii_digit())
        })
        .filter_map(|e| std::fs::read(e.path().join("cmdline")).ok())
        .filter(|c| {
            String::from_utf8_lossy(c)
                .split('\0')
                .any(|arg| arg == needle)
        })
        .count()
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_browser_dies_with_shutdown_and_with_the_server() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert("/", "<title>Alive</title><h1>Alive</h1>".to_string());
    let site = start_site(routes(pages)).await;
    for drop_instead in [false, true] {
        let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
        let profile = h._profile.path().join("profile");
        h.ok("browser_navigate", json!({"url": site.url("/")}))
            .await;
        assert!(
            processes_using(&profile) > 0,
            "the browser runs with the agent profile"
        );
        if drop_instead {
            drop(h.server);
        } else {
            h.server.shutdown().await;
        }
        eventually(
            || processes_using(&profile) == 0,
            "the browser tree to exit",
        )
        .await;
    }
}

// ----- regression tests from the review ---------------------------------------------------

/// The harness configuration for a page that embeds a cross-site frame from `frame.test`.
fn cross_site(c: &mut BrowserConfig) {
    c.test_extra_args
        .push("--host-resolver-rules=MAP frame.test 127.0.0.1".into());
    c.test_extra_args.push("--site-per-process".into());
    c.test_dns_overrides
        .push(("frame.test".into(), "127.0.0.1".parse().unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_element_on_a_scrolled_page_is_clickable_by_reference() {
    // Review fix: the cover check hit-tested viewport coordinates as document
    // coordinates, so every click below the first screen failed.
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Long</title><div style=\"height:3000px\"></div>\
         <button onclick=\"this.textContent='Far clicked'\">Far below</button>\
         <div style=\"height:3000px\"></div>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let found = h.ok("browser_find", json!({"query": "Far below"})).await;
    let r = ref_of(&found, "Far below");
    let (_, out) = h.run("browser_click", json!({"ref": r})).await;
    assert!(!out.is_error, "{}", out.joined_text());
    let snap = h.ok("browser_snapshot", json!({"full": true})).await;
    assert!(snap.contains("Far clicked"), "{snap}");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clicks_and_keys_into_a_cross_site_frame_are_judged_inside_it() {
    // Review fix: a vision click on a cross-site frame was judged on the <iframe> element
    // of the outer page (outer site's scope, no D8 intent), and a key press with the focus
    // inside such a frame was scoped to the outer site.
    if !have_browser() {
        return;
    }
    let mut b_pages = HashMap::new();
    b_pages.insert(
        "/frame",
        "<title>Inner</title><body style=\"margin:0\">\
         <button style=\"position:absolute;left:10px;top:10px;width:200px;height:50px\" onclick=\"this.textContent='Paid'\">Pay now</button>\
         <input aria-label=\"Card\" style=\"position:absolute;left:10px;top:100px\"></body>"
            .to_string(),
    );
    let b = start_site(routes(b_pages)).await;
    let frame_origin = format!("http://frame.test:{}", b.port);
    let mut a_pages = HashMap::new();
    a_pages.insert(
        "/",
        format!(
            "<title>Outer</title><body style=\"margin:0\"><iframe src=\"{frame_origin}/frame\" style=\"position:absolute;left:100px;top:100px;width:400px;height:200px;border:0\"></iframe></body>"
        ),
    );
    let a = start_site(routes(a_pages)).await;
    let h = harness_with(
        &[a.port, b.port],
        Answer::Deny,
        ConnectionMode::Pipe,
        cross_site,
    );
    let snap = h.ok("browser_navigate", json!({"url": a.url("/")})).await;
    assert_eq!(h.server.frame_sessions().await, 1, "{snap}");
    h.ok("browser_screenshot", json!({})).await;
    // (150, 125) is inside the frame's "Pay now" button.
    let (action, out) = h.run("browser_click", json!({"x": 150, "y": 125})).await;
    assert_eq!(
        action.scope,
        policy::Scope::Site {
            origin: frame_origin.clone()
        }
    );
    assert_eq!(action.risk, policy::Risk::AlwaysAsk, "{}", action.preview);
    assert!(action.preview.contains("Pay now"), "{}", action.preview);
    assert!(!out.is_error, "{}", out.joined_text());
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(snap.contains("button \"Paid\""), "{snap}");

    // Focus in the frame's field: a key press goes to the frame's site.
    let card = ref_of(&snap, "textbox \"Card\"");
    h.ok("browser_click", json!({"ref": card})).await;
    let action = h.describe("browser_press_key", json!({"key": "4"})).await;
    assert_eq!(
        action.scope,
        policy::Scope::Site {
            origin: frame_origin.clone()
        }
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn key_presses_are_judged_as_pressed_and_never_use_the_clipboard() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Keys</title>\
         <input aria-label=\"Source\">\
         <input aria-label=\"Dest\">\
         <div contenteditable=true aria-label=\"Chat\" style=\"width:200px;height:40px;border:1px solid\"></div>\
         <button onclick=\"this.textContent='Sent'\">Next</button>\
         <form method=post action=/submit><input name=q aria-label=Query></form>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;

    // Review fix: " Enter" was pressed as Enter but judged as an unknown key, so Enter in
    // a form field went without a card.
    let q = ref_of(&snap, "textbox \"Query\"");
    h.ok("browser_click", json!({"ref": q})).await;
    for key in ["Enter", " Enter", "Enter ", "Shift + Enter"] {
        let action = h.describe("browser_press_key", json!({"key": key})).await;
        assert_eq!(action.risk, policy::Risk::AlwaysAsk, "{key:?}");
    }

    // Review fix: characters typed with browser_press_key are typing too (D8: a button
    // after typing asks).
    let chat_box = ref_of(&snap, "Chat");
    h.ok("browser_click", json!({"ref": chat_box})).await;
    let next = ref_of(&snap, "button \"Next\"");
    let before = h.describe("browser_click", json!({"ref": next})).await;
    assert_eq!(before.risk, policy::Risk::Normal);
    h.ok("browser_press_key", json!({"key": "h"})).await;
    h.ok("browser_press_key", json!({"key": "i"})).await;
    let after = h.describe("browser_click", json!({"ref": next})).await;
    assert_eq!(after.risk, policy::Risk::AlwaysAsk);

    // Review fix: Control+V pasted the user's own clipboard into the page. Put something
    // on the clipboard through the page, then check no shortcut can use it.
    let src = ref_of(&snap, "textbox \"Source\"");
    h.ok(
        "browser_type",
        json!({"ref": src, "text": "clipboard-secret"}),
    )
    .await;
    for key in [
        "Control+c",
        "Control+x",
        "Control+v",
        "Meta+v",
        "Shift+Insert",
        "Control+Insert",
        "Shift+Delete",
    ] {
        match h
            .server
            .describe("browser_press_key", &args(json!({"key": key})), &chat())
            .await
        {
            Described::Invalid(why) => assert!(why.contains("clipboard"), "{key}: {why}"),
            Described::Action(a) => panic!("{key} was described as {a:?}"),
        }
    }
    let dest = ref_of(&snap, "textbox \"Dest\"");
    h.ok("browser_click", json!({"ref": dest})).await;
    // An action forged for a paste is refused at call time too.
    let mut forged = h.describe("browser_press_key", json!({"key": "a"})).await;
    forged.payload["key"] = json!("Control+v");
    let out = h
        .call_action(
            "browser_press_key",
            json!({"key": "Control+v"}),
            &forged,
            CancellationToken::new(),
        )
        .await;
    assert!(
        out.is_error && out.joined_text().contains("clipboard"),
        "{}",
        out.joined_text()
    );
    let snap = h.ok("browser_snapshot", json!({})).await;
    assert!(
        !snap.contains("textbox \"Dest\" [focused] [value="),
        "nothing was pasted:\n{snap}"
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fields_are_rechecked_before_a_fill_and_any_submit_button_is_one() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Agree</title>\
         <form method=post action=/submit>\
           <input type=checkbox id=agree aria-label=\"Agree\">\
           <button type=bogus>Proceed</button>\
         </form>\
         <button onclick=\"const c=document.getElementById('agree'); c.type='submit'; c.value='Agree'\">Toggle</button>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;

    // Review fix: a button whose type the browser does not know is a submit button.
    let proceed = ref_of(&snap, "button \"Proceed\"");
    let action = h.describe("browser_click", json!({"ref": proceed})).await;
    assert_eq!(action.risk, policy::Risk::AlwaysAsk, "{}", action.preview);

    // Review fix: the page turns the approved checkbox into a submit button before the
    // fill runs; the fill is refused instead of clicking it.
    let agree = ref_of(&snap, "Agree");
    let fill_args = json!({"fields": [{"ref": agree, "value": "true"}]});
    let approved = h.describe("browser_fill_form", fill_args.clone()).await;
    assert_eq!(approved.risk, policy::Risk::Normal);
    let toggle = ref_of(&snap, "button \"Toggle\"");
    h.ok("browser_click", json!({"ref": toggle})).await;
    let out = h
        .call_action(
            "browser_fill_form",
            fill_args,
            &approved,
            CancellationToken::new(),
        )
        .await;
    assert!(
        out.is_error && out.joined_text().contains("Refused"),
        "{}",
        out.joined_text()
    );
    assert!(h.approver.seen().is_empty(), "nothing was submitted");
    assert_eq!(site.posts(), 0);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_get_form_aimed_at_a_frame_is_held() {
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Targets</title>\
         <form id=f1 method=get action=/search target=inner><input name=q value=one></form>\
         <iframe name=inner src=/blank></iframe>\
         <div onclick=\"document.getElementById('f1').submit()\" style=\"padding:10px\">Into frame</div>"
            .to_string(),
    );
    pages.insert("/blank", "<p>blank</p>".to_string());
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let found = h.ok("browser_find", json!({"query": "Into frame"})).await;
    let r = ref_of(&found, "Into frame");
    h.ok("browser_click", json!({"ref": r})).await;
    let seen = h.approver.seen();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].kind, HoldKind::Submission);
    assert!(
        !site
            .log
            .lock()
            .unwrap()
            .iter()
            .any(|l| l.contains("/search"))
    );
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_card_raised_outside_a_call_goes_to_the_chat_that_used_the_browser() {
    // Review fix: describe (which may run for any chat, and for calls never approved)
    // recorded its chat as the one that gets the cards for held requests.
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Later</title><form method=post action=/submit><input name=q value=x></form>\
         <script>setTimeout(() => document.forms[0].submit(), 1500)</script>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    // Chat 7 opens the page.
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    // Chat 8 only asks to describe something, which the user may never allow.
    let other = ChatContext {
        chat_id: 8,
        ..chat()
    };
    assert!(matches!(
        h.server
            .describe("browser_snapshot", &args(json!({})), &other)
            .await,
        Described::Action(_)
    ));
    eventually(|| !h.approver.seen().is_empty(), "the page's own POST").await;
    assert_eq!(*h.approver.chats.lock().unwrap(), vec![7]);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_fails_requests_held_in_other_tabs_of_the_chat() {
    // Review fix: Stop failed only the requests of the call's own tab; a request the call
    // made another tab send stayed held until the approval timeout.
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/opener",
        "<title>Opener</title><form method=post action=/submit><input name=q value=x></form>\
         <button onclick=\"window.open('/popup')\">Open the pop-up</button>"
            .to_string(),
    );
    pages.insert(
        "/popup",
        "<title>Popup</title><div onclick=\"window.opener.document.forms[0].submit()\" style=\"padding:20px\">Send from the opener</div>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Never, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/opener")}))
        .await;
    let open = ref_of(&snap, "button \"Open the pop-up\"");
    h.ok("browser_click", json!({"ref": open})).await;
    eventually_async(&h, |url| url.ends_with("/popup")).await;
    let found = h
        .ok("browser_find", json!({"query": "Send from the opener"}))
        .await;
    let r = ref_of(&found, "Send from the opener");
    let a = json!({"ref": r});
    let action = h.describe("browser_click", a.clone()).await;
    let cancel = CancellationToken::new();
    let stopper = {
        let cancel = cancel.clone();
        let approver = h.approver.clone();
        tokio::spawn(async move {
            eventually(|| !approver.seen().is_empty(), "the hold").await;
            cancel.cancel();
        })
    };
    let out = h.call_action("browser_click", a, &action, cancel).await;
    stopper.await.unwrap();
    assert!(
        out.joined_text().contains("Stopped"),
        "{}",
        out.joined_text()
    );
    let mut held = usize::MAX;
    for _ in 0..20 {
        held = h.server.held_requests().await;
        if held == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        held, 0,
        "the request held in the opener tab was failed on Stop"
    );
    assert_eq!(site.posts(), 0);
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_crashed_tab_recovers_by_navigating_again() {
    // Review fix: after a crash every tool, browser_navigate included, answered "Navigate
    // again to reload it", so the tab could never be used again.
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert("/", "<title>Fine</title><h1>Fine again</h1>".to_string());
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    h.ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    h.server.crash_active_tab().await;
    let crashed_at = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        match h
            .server
            .describe("browser_snapshot", &args(json!({})), &chat())
            .await
        {
            Described::Invalid(why) if why.contains("crashed") => break,
            _ => {}
        }
        assert!(Instant::now() < deadline, "the crash was never noticed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // Commands in flight to the dead renderer fail at once instead of waiting 15 s.
    assert!(
        crashed_at.elapsed() < Duration::from_secs(8),
        "noticing the crash took {:?}",
        crashed_at.elapsed()
    );
    let text = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    assert!(text.contains("Fine again"), "{text}");
    h.server.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acting_in_a_sandboxed_frame_always_asks() {
    // Review fix: a sandboxed srcdoc frame has the opaque origin "null"; a grant for
    // that scope would have covered such frames on every site.
    if !have_browser() {
        return;
    }
    let mut pages = HashMap::new();
    pages.insert(
        "/",
        "<title>Sandbox</title><iframe sandbox=\"allow-scripts\" srcdoc=\"<button onclick='this.textContent=1'>Inner</button>\"></iframe>"
            .to_string(),
    );
    let site = start_site(routes(pages)).await;
    let h = harness(&[site.port], Answer::Deny, ConnectionMode::Pipe);
    let snap = h
        .ok("browser_navigate", json!({"url": site.url("/")}))
        .await;
    let inner = ref_of(&snap, "button \"Inner\"");
    let action = h.describe("browser_click", json!({"ref": inner})).await;
    assert_eq!(
        action.scope,
        policy::Scope::Site {
            origin: "null".into()
        }
    );
    assert_eq!(action.risk, policy::Risk::AlwaysAsk, "{}", action.preview);
    h.server.shutdown().await;
}
