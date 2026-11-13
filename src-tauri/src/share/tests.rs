//! The sharing proxy against a fake llama-server, and the sharing state machine with an
//! in-memory store and keychain.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http_body_util::BodyExt;
use http_body_util::channel::Channel;
use http_body_util::combinators::BoxBody;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::proxy::{self, Counters, ModelSource, Upstream};
use super::*;
use crate::state::{BusyCounter, BusyKind};

const PRIVATE: &str = "private-llama-key-1234567890";
const MODEL_PATH: &str = "/home/someone/Private Models/Secret-Model-Q4.gguf";
const SHARE_KEY: &str = "harnessui-share-0123456789abcdef0123456789abcdef";
/// What Claude Code also sends (in `x-api-key`) when the user has ANTHROPIC_API_KEY set.
const USERS_OWN_KEY: &str = "sk-ant-api03-the-users-own-key";

// ---- A fake llama-server -----------------------------------------------------------

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Value,
}

#[derive(Clone)]
enum Reply {
    Json(u16, Value),
    Text(u16, &'static str, String),
    /// Event lines (each sent as its own chunk), with a pause before each.
    Sse(Vec<String>, Duration),
}

#[derive(Clone, Default)]
struct FakeLlama {
    seen: Arc<Mutex<Vec<Seen>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
}

impl FakeLlama {
    async fn start(&self) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let fake = self.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let fake = fake.clone();
                tokio::spawn(async move {
                    let service = hyper::service::service_fn(move |req| {
                        let fake = fake.clone();
                        async move { Ok::<_, Infallible>(fake.answer(req).await) }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(hyper_util::rt::TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        addr
    }

    fn reply(&self, reply: Reply) {
        self.replies.lock().unwrap().push_back(reply);
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    async fn answer(
        &self,
        req: http::Request<hyper::body::Incoming>,
    ) -> http::Response<BoxBody<Bytes, Infallible>> {
        let method = req.method().to_string();
        let target = req.uri().to_string();
        let headers = req
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.to_string(),
                    String::from_utf8_lossy(v.as_bytes()).into_owned(),
                )
            })
            .collect();
        let bytes = req.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        self.seen.lock().unwrap().push(Seen {
            method,
            target,
            headers,
            body,
        });
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Reply::Json(200, json!({"ok": true})));
        let respond = |status: u16, kind: &str, body: BoxBody<Bytes, Infallible>| {
            http::Response::builder()
                .status(status)
                .header("content-type", kind)
                .body(body)
                .unwrap()
        };
        match reply {
            Reply::Json(status, value) => respond(
                status,
                "application/json; charset=utf-8",
                http_body_util::Full::new(Bytes::from(value.to_string())).boxed(),
            ),
            Reply::Text(status, kind, text) => respond(
                status,
                kind,
                http_body_util::Full::new(Bytes::from(text)).boxed(),
            ),
            Reply::Sse(lines, pause) => {
                let (mut tx, body) = Channel::<Bytes, Infallible>::new(4);
                tokio::spawn(async move {
                    for line in lines {
                        tokio::time::sleep(pause).await;
                        if tx.send_data(Bytes::from(line)).await.is_err() {
                            return;
                        }
                    }
                });
                respond(200, "text/event-stream", body.boxed())
            }
        }
    }
}

/// The app's loaded model, as a test sets it.
struct FakeModel(Mutex<Upstream>);

#[async_trait::async_trait]
impl ModelSource for FakeModel {
    async fn current(&self) -> Upstream {
        self.0.lock().unwrap().clone()
    }
}

impl FakeModel {
    fn ready(addr: SocketAddr, n_ctx: u32) -> Upstream {
        Upstream::Ready {
            base_url: format!("http://{addr}"),
            key: PRIVATE.into(),
            model: MODEL_PATH.into(),
            n_ctx,
        }
    }

    fn set(&self, upstream: Upstream) {
        *self.0.lock().unwrap() = upstream;
    }
}

// ---- A proxy in front of the fake --------------------------------------------------

struct Harness {
    port: u16,
    _running: proxy::Running,
    store: store::Store,
    busy: Arc<BusyCounter>,
    counters: Arc<Counters>,
    model: Arc<FakeModel>,
    key: Arc<RwLock<String>>,
    llama: FakeLlama,
    client: reqwest::Client,
}

impl Harness {
    async fn new() -> Self {
        // This test binary as the picture worker (`attachments::tests`).
        Self::with_pictures(crate::attachments::tests::test_worker("convert")).await
    }

    async fn with_pictures(pictures: crate::attachments::PictureWorker) -> Self {
        let llama = FakeLlama::default();
        let addr = llama.start().await;
        let model = Arc::new(FakeModel(Mutex::new(FakeModel::ready(addr, 8192))));
        let store = store::Store::open_in_memory().unwrap();
        let busy = Arc::new(BusyCounter::default());
        let counters = Arc::new(Counters::default());
        let key = Arc::new(RwLock::new(SHARE_KEY.to_owned()));
        let listener = proxy::bind(0).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let running = proxy::serve(
            listener,
            proxy::Deps {
                port,
                key: key.clone(),
                model: model.clone(),
                store: store.clone(),
                busy: busy.clone(),
                counters: counters.clone(),
                pictures,
            },
        )
        .unwrap();
        Self {
            port,
            _running: running,
            store,
            busy,
            counters,
            model,
            key,
            llama,
            client: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    fn post(&self, path: &str, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(self.url(path))
            .header("x-api-key", SHARE_KEY)
            .header("content-type", "application/json")
            .body(body.to_string())
    }

    async fn audit(&self) -> Vec<store::AuditRow> {
        self.store
            .list_audit(store::AuditQuery {
                server: Some(proxy::AUDIT_SERVER.into()),
                ..Default::default()
            })
            .await
            .unwrap()
    }

    /// The audit rows once every one has its result (they are completed after the answer).
    async fn audit_done(&self, n: usize) -> Vec<store::AuditRow> {
        for _ in 0..100 {
            let rows = self.audit().await;
            if rows.len() >= n && rows.iter().all(|r| r.result_status.is_some()) {
                return rows;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        self.audit().await
    }
}

/// Sends raw bytes and reads one response (status line, headers and a body of the
/// announced length), waiting at most `wait`.
async fn raw(port: u16, request: &[u8], wait: Duration) -> Option<(u16, String)> {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).await.ok()?;
    stream.write_all(request).await.ok()?;
    let deadline = Instant::now() + wait;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let length = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            if buf.len() >= end + 4 + length {
                let status = head.split(' ').nth(1)?.parse().ok()?;
                let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + length]).to_string();
                return Some((status, body));
            }
        }
        let left = deadline.checked_duration_since(Instant::now())?;
        match tokio::time::timeout(left, stream.read(&mut chunk)).await {
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => return None,
            Ok(Ok(n)) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

fn anthropic_answer() -> Value {
    json!({
        "id": "chatcmpl-1", "type": "message", "role": "assistant",
        "content": [{"type": "text", "text": "Hello"}],
        "model": MODEL_PATH, "stop_reason": "end_turn", "stop_sequence": null,
        "usage": {"cache_read_input_tokens": 9000, "input_tokens": 88, "output_tokens": 5}
    })
}

fn no_secret(text: &str) {
    for part in [
        "Private Models",
        "Secret-Model",
        "/home/someone",
        PRIVATE,
        "cache_n",
        "cache_read_input_tokens",
        "timings",
        "system_fingerprint",
    ] {
        assert!(!text.contains(part), "{part:?} leaked: {text}");
    }
}

// ---- Proxy tests -------------------------------------------------------------------

#[tokio::test]
async fn forwards_a_rebuilt_request_with_a_fixed_header_list() {
    let h = Harness::new().await;
    h.llama.reply(Reply::Json(200, anthropic_answer()));
    let response = h
        .client
        .post(h.url("/v1/messages?beta=true"))
        .header("authorization", format!("Bearer {SHARE_KEY}"))
        // Claude Code adds the user's own key when ANTHROPIC_API_KEY is also set.
        .header("x-api-key", USERS_OWN_KEY)
        .header("content-type", "application/json")
        .header("anthropic-version", "2023-06-01")
        .header("anthropic-beta", "something")
        .header("cookie", "a=b")
        .header("x-forwarded-for", "10.0.0.1")
        .header("user-agent", "claude-cli/2.1.322 (external, cli)")
        .body(
            json!({
                "model": "claude-sonnet-x",
                "max_tokens": 64000,
                "messages": [{"role": "user", "content": "the secret prompt"}],
                "metadata": {"user_id": "abc"},
                "grammar": "root ::= x",
                "cache_prompt": false,
                "stream": false
            })
            .to_string(),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "application/json");
    let text = response.text().await.unwrap();
    no_secret(&text);
    let answer: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(answer["model"], json!(proxy::MODEL_ALIAS));
    assert_eq!(
        answer["usage"],
        json!({"input_tokens": 9088, "output_tokens": 5})
    );

    let seen = h.llama.seen();
    assert_eq!(seen.len(), 1);
    let up = &seen[0];
    assert_eq!(up.method, "POST");
    // The constant path, never the query.
    assert_eq!(up.target, "/v1/messages");
    let mut names: Vec<&str> = up.headers.iter().map(|(k, _)| k.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "accept",
            "authorization",
            "content-length",
            "content-type",
            "host"
        ]
    );
    let header = |name: &str| {
        up.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(
        header("authorization"),
        Some(format!("Bearer {PRIVATE}").as_str())
    );
    assert_eq!(header("content-type"), Some("application/json"));
    assert_eq!(
        up.body,
        json!({
            "model": proxy::MODEL_ALIAS,
            "max_tokens": 8192,
            "messages": [{"role": "user", "content": "the secret prompt"}],
            "stream": false
        })
    );

    // One audit row: who it says it is and the counts, never the request text.
    let rows = h.audit_done(1).await;
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.tool, "/v1/messages");
    assert_eq!(row.decision, store::AuditDecision::AllowedWithoutPrompt);
    assert_eq!(row.chat_id, None);
    assert_eq!(row.result_status.as_deref(), Some("ok"));
    assert_eq!(
        row.result_summary.as_deref(),
        Some("9,088 tokens in, 5 out")
    );
    assert_eq!(
        row.arguments["says"],
        json!("claude-cli/2.1.322 (external, cli)")
    );
    assert!(
        row.preview.contains("It says it is claude-cli/2.1.322"),
        "{}",
        row.preview
    );
    let all = serde_json::to_string(row).unwrap();
    assert!(!all.contains("secret prompt") && !all.contains(SHARE_KEY) && !all.contains(PRIVATE));
    assert!(!all.contains(USERS_OWN_KEY));
    assert!(up.headers.iter().all(|(_, v)| !v.contains(USERS_OWN_KEY)));
}

#[tokio::test]
async fn each_api_goes_to_its_own_constant_path() {
    let h = Harness::new().await;
    for (path, body) in [
        (
            "/v1/messages/count_tokens",
            json!({"messages": [], "max_tokens": 3}),
        ),
        ("/v1/responses?x=1", json!({"input": "hi", "store": true})),
        (
            "/v1/chat/completions",
            json!({"messages": [], "n_probs": 2}),
        ),
    ] {
        h.llama.reply(Reply::Json(200, json!({"ok": 1})));
        let response = h.post(path, &body).send().await.unwrap();
        assert_eq!(response.status(), 200, "{path}");
    }
    let targets: Vec<String> = h.llama.seen().iter().map(|s| s.target.clone()).collect();
    assert_eq!(
        targets,
        [
            "/v1/messages/count_tokens",
            "/v1/responses",
            "/v1/chat/completions"
        ]
    );
    let seen = h.llama.seen();
    assert_eq!(
        seen[0].body,
        json!({"model": proxy::MODEL_ALIAS, "messages": []})
    );
    assert_eq!(
        seen[1].body,
        json!({"model": proxy::MODEL_ALIAS, "input": "hi", "max_output_tokens": 8192})
    );
    assert_eq!(
        seen[2].body,
        json!({"model": proxy::MODEL_ALIAS, "messages": [], "max_tokens": 8192})
    );
}

#[tokio::test]
async fn the_model_list_is_answered_by_the_proxy() {
    let h = Harness::new().await;
    for path in ["/v1/models", "/v1/models?limit=1000"] {
        let response = h
            .client
            .get(h.url(path))
            .bearer_auth(SHARE_KEY)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["data"][0]["id"], json!(proxy::MODEL_ALIAS));
    }
    // Without the key it is refused like everything else.
    let response = h.client.get(h.url("/v1/models")).send().await.unwrap();
    assert_eq!(response.status(), 401);
    assert!(h.llama.seen().is_empty());
}

#[tokio::test]
async fn unknown_paths_and_methods_are_refused_after_the_key() {
    let h = Harness::new().await;
    for path in [
        "/v1/messages/",
        "/v1/%6Dessages",
        "/v1/messages/../models",
        "//v1/messages",
        "/props",
        "/health",
        "/completion",
        "/v1/x/../messages",
    ] {
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nx-api-key: {SHARE_KEY}\r\nContent-Length: 2\r\n\r\n{{}}",
            h.port
        );
        let (status, body) = raw(h.port, request.as_bytes(), Duration::from_secs(3))
            .await
            .unwrap();
        assert_eq!(status, 404, "{path}");
        no_secret(&body);
    }
    let response = h
        .client
        .get(h.url("/v1/messages"))
        .header("x-api-key", SHARE_KEY)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 405);
    assert!(h.llama.seen().is_empty());
}

/// Host, Origin and key are checked before the body is read: a request that announces a
/// body and never sends it is answered at once (the body wait is a minute).
#[tokio::test]
async fn refusals_come_before_the_body_is_read() {
    let h = Harness::new().await;
    let port = h.port;
    let cases = [
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nx-api-key: {SHARE_KEY}\r\nOrigin: https://example.com\r\nContent-Length: 1000\r\n\r\n"
            ),
            403,
        ),
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: evil.example:{port}\r\nx-api-key: {SHARE_KEY}\r\nContent-Length: 1000\r\n\r\n"
            ),
            403,
        ),
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nx-api-key: {SHARE_KEY}\r\nContent-Length: 1000\r\n\r\n",
                port + 1
            ),
            403,
        ),
        (
            format!(
                "POST http://127.0.0.1:{port}/v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nx-api-key: {SHARE_KEY}\r\nContent-Length: 1000\r\n\r\n"
            ),
            403,
        ),
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 1000\r\n\r\n"
            ),
            401,
        ),
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nx-api-key: harnessui-share-ffffffffffffffffffffffffffffffff\r\nContent-Length: 1000\r\n\r\n"
            ),
            401,
        ),
        (
            format!(
                "POST /v1/messages HTTP/1.1\r\nHost: localhost:{port}\r\nAuthorization: Bearer nope\r\nContent-Length: 1000\r\n\r\n"
            ),
            401,
        ),
        (
            format!(
                "OPTIONS /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: null\r\nAccess-Control-Request-Method: POST\r\n\r\n"
            ),
            403,
        ),
    ];
    for (request, expected) in &cases {
        let started = Instant::now();
        let (status, body) = raw(port, request.as_bytes(), Duration::from_secs(5))
            .await
            .unwrap_or_else(|| panic!("no answer to {request:?}"));
        assert_eq!(status, *expected, "{request:?}");
        assert!(started.elapsed() < Duration::from_secs(3), "{request:?}");
        // Nothing about the app, the model or the key in a refusal.
        no_secret(&body);
        assert!(
            !body.contains("harnessUI") && !body.contains(SHARE_KEY),
            "{body}"
        );
    }
    assert!(h.llama.seen().is_empty());
    assert!(h.audit().await.is_empty());
    assert_eq!(h.counters.refused(), cases.len() as u64);
}

#[tokio::test]
async fn bodies_over_the_cap_are_refused() {
    let h = Harness::new().await;
    // Announced too large: answered without reading.
    let request = format!(
        "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nx-api-key: {SHARE_KEY}\r\nContent-Length: {}\r\n\r\n",
        h.port,
        proxy::MAX_BODY + 1
    );
    let (status, body) = raw(h.port, request.as_bytes(), Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(status, 413);
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["error"]["type"],
        json!("request_too_large")
    );
    // Sent in pieces with no length: cut off at the cap.
    let piece = Bytes::from(vec![b' '; 1 << 20]);
    let pieces =
        (0..(proxy::MAX_BODY >> 20) + 2).map(move |_| Ok::<_, std::io::Error>(piece.clone()));
    let response = h
        .client
        .post(h.url("/v1/chat/completions"))
        .header("x-api-key", SHARE_KEY)
        .body(reqwest::Body::wrap_stream(futures_util::stream::iter(
            pieces,
        )))
        .send()
        .await;
    match response {
        Ok(r) => assert_eq!(r.status(), 413),
        // The proxy may close the connection before the client has sent everything.
        Err(e) => assert!(e.is_request() || e.is_body(), "{e}"),
    }
    assert!(h.llama.seen().is_empty());
    // Round 1: both are authenticated requests, refused by the size rule, each with its row.
    let rows = h.audit_done(2).await;
    assert_eq!(rows.len(), 2, "{rows:?}");
    for row in &rows {
        assert_eq!(row.decision, store::AuditDecision::RefusedByPolicy);
        assert_eq!(row.result_status.as_deref(), Some("not_run"));
        assert_eq!(
            row.result_summary.as_deref(),
            Some("Refused: the request was too large.")
        );
    }
}

#[tokio::test]
async fn no_model_and_loading_are_answered_in_each_apis_shape() {
    let h = Harness::new().await;
    for (state, message, code) in [
        (Upstream::NotLoaded, proxy::NO_MODEL, "model_not_loaded"),
        (Upstream::Loading, proxy::LOADING, "model_loading"),
    ] {
        h.model.set(state);
        for path in ["/v1/messages", "/v1/messages/count_tokens"] {
            let response = h.post(path, &json!({"messages": []})).send().await.unwrap();
            assert_eq!(response.status(), 503);
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body,
                json!({"type": "error", "error": {"type": "overloaded_error", "message": message}})
            );
        }
        for path in ["/v1/responses", "/v1/chat/completions"] {
            let response = h.post(path, &json!({"input": "x"})).send().await.unwrap();
            assert_eq!(response.status(), 503);
            let body: Value = response.json().await.unwrap();
            assert_eq!(
                body,
                json!({"error": {"message": message, "type": "server_error", "param": null, "code": code}})
            );
        }
    }
    assert_eq!(
        proxy::NO_MODEL,
        "No model is loaded in harnessUI. Open harnessUI and load a model."
    );
    assert_eq!(
        proxy::LOADING,
        "The model in harnessUI is still loading. Try again in a minute."
    );
    assert!(h.llama.seen().is_empty());
    // Round 1: each of these authenticated requests has its audit row, saying why it never
    // reached a model (token counting at most once a minute: 3 + 1 + 3).
    let rows = h.audit_done(7).await;
    assert_eq!(rows.len(), 7, "{rows:?}");
    assert_eq!(
        rows.iter()
            .filter(|r| r.tool == "/v1/messages/count_tokens")
            .count(),
        1
    );
    for row in &rows {
        assert_eq!(row.decision, store::AuditDecision::AllowedWithoutPrompt);
        assert_eq!(row.decided_by, "sharing key");
        assert_eq!(row.result_status.as_deref(), Some("not_run"), "{row:?}");
        assert!(
            matches!(
                row.result_summary.as_deref(),
                Some("No model was loaded." | "The model was still loading.")
            ),
            "{row:?}"
        );
        assert!(
            row.preview.contains("was not passed to the model"),
            "{row:?}"
        );
    }
    assert_eq!(
        rows.iter()
            .filter(|r| r.result_summary.as_deref() == Some("No model was loaded."))
            .count(),
        4
    );
}

#[tokio::test]
async fn image_links_are_refused_before_forwarding() {
    let h = Harness::new().await;
    for (path, body) in [
        (
            "/v1/chat/completions",
            json!({"messages": [{"role": "user", "content": [{"type": "image_url", "image_url": {"url": "http://127.0.0.1:1/x.png"}}]}]}),
        ),
        (
            "/v1/responses",
            json!({"input": [{"role": "user", "content": [{"type": "input_image", "image_url": "https://example.com/x.png"}]}]}),
        ),
        (
            "/v1/messages",
            json!({"messages": [{"role": "user", "content": [{"type": "image", "source": {"type": "url", "url": "https://example.com/x.png"}}]}]}),
        ),
        (
            "/v1/chat/completions",
            json!({"messages": [{"role": "user", "content": [{"type": "input_audio", "input_audio": {"data": "http://169.254.169.254/latest/meta-data/", "format": "wav"}}]}]}),
        ),
    ] {
        let response = h
            .post(path, &body)
            .header("user-agent", "curl/8.5")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 400, "{path}");
    }
    assert!(h.llama.seen().is_empty());
    // Round 1: the owner can see that an app tried to make the model fetch an address. The
    // row names the rule, never the address or anything else from the request.
    let rows = h.audit_done(4).await;
    assert_eq!(rows.len(), 4, "{rows:?}");
    for row in &rows {
        assert_eq!(
            row.decision,
            store::AuditDecision::RefusedByPolicy,
            "{row:?}"
        );
        assert_eq!(row.result_status.as_deref(), Some("not_run"), "{row:?}");
        assert_eq!(
            row.result_summary.as_deref(),
            Some(
                "Refused: an image, audio, video or file was sent as a link or a file id, not inline."
            )
        );
        assert!(row.preview.contains("It says it is curl/8.5."), "{row:?}");
        let text = serde_json::to_string(row).unwrap();
        for part in ["example.com", "169.254", "127.0.0.1:1", "x.png"] {
            assert!(!text.contains(part), "{part} in {text}");
        }
    }
}

// ---- WebP pictures (0.0.6 webp-other-paths) ------------------------------------------

fn b64(bytes: &[u8]) -> String {
    base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
}

/// The picture a `data:` URL or plain base64 holds: its format, read from its bytes, and
/// its size in pixels.
fn picture_in(data: &Value) -> (image::ImageFormat, (u32, u32)) {
    let data = data.as_str().unwrap_or_else(|| panic!("{data}"));
    let data = data.split_once(',').map_or(data, |(_, d)| d);
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data).unwrap();
    let picture = image::load_from_memory(&bytes).unwrap();
    (
        image::guess_format(&bytes).unwrap(),
        (picture.width(), picture.height()),
    )
}

/// [0.0.6 webp-other-paths] llama-server cannot read WebP (it would need ffmpeg, which the
/// app never lets it start), so a WebP picture another app sends is turned into JPEG
/// (opaque) or PNG (see-through parts) in the picture worker before the request is
/// forwarded: in all three API shapes, as a `data:` URL, as plain base64, as an Anthropic
/// base64 source, inside tool results, and under another type's name (llama-server goes by
/// the bytes). Other pictures are forwarded as they came.
#[tokio::test]
async fn webp_pictures_are_forwarded_as_jpeg_or_png() {
    use crate::attachments::tests::{LOSSLESS_WEBP, LOSSY_WEBP};
    use image::ImageFormat::{Jpeg, Png};
    let h = Harness::new().await;
    let png = "data:image/png;base64,iVBORw0KGgo=";
    let webp_url = |bytes: &[u8]| format!("data:image/webp;base64,{}", b64(bytes));
    let requests = [
        (
            "/v1/chat/completions",
            json!({"messages": [{"role": "user", "content": [
                {"type": "text", "text": "What is in these?"},
                {"type": "image_url", "image_url": {"url": webp_url(LOSSY_WEBP)}},
                {"type": "image_url", "image_url": {"url": png}},
            ]}]}),
        ),
        (
            "/v1/responses",
            json!({"input": [
                {"role": "user", "content": [{"type": "input_image", "image_url": webp_url(LOSSLESS_WEBP)}]},
                {"type": "function_call_output", "call_id": "c1", "output": [
                    {"type": "input_image", "image_url": b64(LOSSY_WEBP)}]},
            ]}),
        ),
        (
            "/v1/messages",
            json!({"messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/webp", "data": b64(LOSSLESS_WEBP)}},
                {"type": "tool_result", "tool_use_id": "t1", "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64(LOSSY_WEBP)}}]},
            ]}]}),
        ),
    ];
    for (path, body) in &requests {
        let response = h.post(path, body).send().await.unwrap();
        let status = response.status();
        let text = response.text().await.unwrap();
        assert_eq!(status, 200, "{path}: {text}");
    }
    let seen = h.llama.seen();
    assert_eq!(seen.len(), 3);
    for s in &seen {
        let text = s.body.to_string();
        // "UklGR" is "RIFF" in base64: no WebP file is forwarded, under any name.
        assert!(
            !text.contains("image/webp") && !text.contains("UklGR"),
            "{}: {}",
            s.target,
            &text[..text.len().min(300)]
        );
    }

    let chat = &seen[0].body["messages"][0]["content"];
    let url = &chat[1]["image_url"]["url"];
    assert!(url.as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    assert_eq!(picture_in(url), (Jpeg, (4, 3)));
    assert_eq!(chat[2]["image_url"]["url"], png);
    assert_eq!(chat[0]["text"], "What is in these?");

    let input = &seen[1].body["input"];
    let url = &input[0]["content"][0]["image_url"];
    assert!(url.as_str().unwrap().starts_with("data:image/png;base64,"));
    assert_eq!(picture_in(url), (Png, (3, 2)));
    let url = &input[1]["output"][0]["image_url"];
    assert!(url.as_str().unwrap().starts_with("data:image/jpeg;base64,"));
    assert_eq!(picture_in(url), (Jpeg, (4, 3)));

    let content = &seen[2].body["messages"][0]["content"];
    let source = &content[0]["source"];
    assert_eq!(source["type"], "base64");
    assert_eq!(source["media_type"], "image/png");
    assert_eq!(picture_in(&source["data"]), (Png, (3, 2)));
    let source = &content[1]["content"][0]["source"];
    assert_eq!(source["media_type"], "image/jpeg");
    assert_eq!(picture_in(&source["data"]), (Jpeg, (4, 3)));

    let rows = h.audit_done(3).await;
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert!(
        rows.iter()
            .all(|r| r.result_status.as_deref() == Some("ok")),
        "{rows:?}"
    );
}

/// [0.0.6 webp-other-paths] A WebP picture that cannot be turned into JPEG or PNG is refused
/// before anything is forwarded, with a plain message in the API's own error shape and an
/// audit row that names the rule (never anything from the request): a crafted picture far
/// larger than the limits (refused before it is decoded), a broken one, and more WebP
/// pictures than one request may hold.
#[tokio::test]
async fn webp_pictures_that_cannot_be_turned_are_refused_in_each_apis_shape() {
    use crate::attachments::tests::{LOSSY_WEBP, plain_webp};
    let unreadable =
        "A WebP picture in this request could not be read. Send a PNG or JPEG instead.";
    let too_many =
        "This request has more than 32 WebP pictures. Send PNG or JPEG pictures instead.";
    let h = Harness::new().await;
    let crafted = b64(&plain_webp(10_000, 8));
    let webp_part = |data: &str| json!({"type": "image_url", "image_url": {"url": format!("data:image/webp;base64,{data}")}});
    let many: Vec<Value> = (0..33).map(|_| webp_part(&b64(LOSSY_WEBP))).collect();
    let cases = [
        (
            "/v1/chat/completions",
            json!({"messages": [{"role": "user", "content": [webp_part(&crafted)]}]}),
            unreadable,
        ),
        (
            "/v1/messages",
            json!({"messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/webp", "data": b64(&LOSSY_WEBP[..40])}}]}]}),
            unreadable,
        ),
        (
            "/v1/responses",
            json!({"input": [{"role": "user", "content": [{"type": "input_image", "image_url": crafted}]}]}),
            unreadable,
        ),
        (
            "/v1/chat/completions",
            json!({"messages": [{"role": "user", "content": many}]}),
            too_many,
        ),
    ];
    for (path, body, message) in &cases {
        let response = h
            .post(path, body)
            .header("user-agent", "curl/8.5")
            .send()
            .await
            .unwrap();
        let status = response.status();
        let answer: Value = response.json().await.unwrap();
        assert_eq!(status, 400, "{path}: {answer}");
        assert_eq!(answer["error"]["message"], *message, "{path}: {answer}");
        assert_eq!(answer["error"]["type"], "invalid_request_error", "{answer}");
        if *path == "/v1/messages" {
            assert_eq!(answer["type"], "error", "{answer}");
        } else {
            assert!(answer["error"]["code"].is_null(), "{answer}");
        }
    }
    assert!(h.llama.seen().is_empty());
    let rows = h.audit_done(4).await;
    assert_eq!(rows.len(), 4, "{rows:?}");
    for row in &rows {
        assert_eq!(
            row.decision,
            store::AuditDecision::RefusedByPolicy,
            "{row:?}"
        );
        assert_eq!(row.result_status.as_deref(), Some("not_run"), "{row:?}");
        let summary = row.result_summary.as_deref().unwrap_or_default();
        assert!(
            summary.starts_with("Refused: ") && summary.contains("WebP"),
            "{row:?}"
        );
        let text = serde_json::to_string(row).unwrap();
        assert!(!text.contains("UklGR") && !text.contains("data:"), "{text}");
    }
}

/// Waits until a request with a WebP picture counts as using the model, while its picture
/// is still being turned (the slow worker takes 3 s), with `forwarded` requests seen so far.
async fn counted_while_turning(h: &Harness, forwarded: usize) {
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.busy.snapshot().other_apps == 0 && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let busy = h.busy.snapshot();
    assert_eq!(
        busy.other_apps, 1,
        "not counted while its picture is turned"
    );
    assert_eq!(h.counters.active(), 1);
    assert_eq!(
        h.llama.seen().len(),
        forwarded,
        "forwarded before it was turned"
    );
}

/// [0.0.6 webp-other-paths, final round] Turning a request's WebP pictures can take seconds,
/// or wait behind other requests' pictures. The busy counter (D3) and the request count are
/// held from before the model is looked up, so while the pictures are turned the busy
/// question names "Another app" and "Restart when it ends" waits for the request, which then
/// goes to the server it was looked up on, with that server's key. When the model is stopped
/// or changed meanwhile all the same (the user chose not to wait), the request is never sent
/// to the old address, which another program may hold by then.
#[tokio::test]
async fn the_busy_counter_is_held_while_webp_pictures_are_turned() {
    use crate::attachments::tests::{LOSSY_WEBP, test_worker};
    let h = Harness::with_pictures(test_worker("slow")).await;
    let ready = h.model.current().await;
    let body = json!({"messages": [{"role": "user", "content": [{"type": "image_url",
        "image_url": {"url": format!("data:image/webp;base64,{}", b64(LOSSY_WEBP))}}]}]});
    let send = || tokio::spawn(h.post("/v1/chat/completions", &body).send());

    let request = send();
    counted_while_turning(&h, 0).await;
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.status(), 200);
    response.bytes().await.unwrap();
    let seen = h.llama.seen();
    assert_eq!(seen.len(), 1);
    assert!(
        seen[0]
            .headers
            .contains(&("authorization".into(), format!("Bearer {PRIVATE}"))),
        "{:?}",
        seen[0].headers
    );
    let url = &seen[0].body["messages"][0]["content"][0]["image_url"]["url"];
    assert_eq!(picture_in(url).0, image::ImageFormat::Jpeg);
    for _ in 0..50 {
        if !h.busy.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!h.busy.is_busy());

    // The model is unloaded while the picture is turned.
    let request = send();
    counted_while_turning(&h, 1).await;
    h.model.set(Upstream::NotLoaded);
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.status(), 503);
    let answer: Value = response.json().await.unwrap();
    assert_eq!(answer["error"]["message"], proxy::NO_MODEL, "{answer}");

    // Another model is loaded while the picture is turned (a new server, port and key).
    let other = FakeLlama::default();
    let other_addr = other.start().await;
    h.model.set(ready);
    let request = send();
    counted_while_turning(&h, 1).await;
    h.model.set(FakeModel::ready(other_addr, 8192));
    let response = request.await.unwrap().unwrap();
    assert_eq!(response.status(), 502);
    let answer: Value = response.json().await.unwrap();
    assert_eq!(
        answer["error"]["message"], "The model in harnessUI did not answer. Try again.",
        "{answer}"
    );
    assert_eq!(h.llama.seen().len(), 1);
    assert!(other.seen().is_empty());
    assert!(!h.busy.is_busy());
    assert_eq!(h.counters.active(), 0);

    let rows = h.audit_done(3).await;
    let mut results: Vec<_> = rows
        .iter()
        .map(|r| {
            (
                r.result_status.clone().unwrap_or_default(),
                r.result_summary.clone().unwrap_or_default(),
            )
        })
        .collect();
    results.sort();
    assert_eq!(results.len(), 3, "{rows:?}");
    assert_eq!(results[0].0, "not_run");
    assert_eq!(results[0].1, "No model was loaded.");
    assert_eq!(results[1].0, "not_run");
    assert_eq!(
        results[1].1,
        "The model was changed while the request waited."
    );
    assert_eq!(results[2].0, "ok", "{rows:?}");
}

#[tokio::test]
async fn streams_come_back_cleaned_event_by_event() {
    let h = Harness::new().await;
    let path_json = serde_json::to_string(MODEL_PATH).unwrap();
    h.llama.reply(Reply::Sse(
        vec![
            "event: message_start\n".into(),
            format!("data: {{\"type\":\"message_start\",\"message\":{{\"model\":{path_json},\"usage\":{{\"cache_read_input_tokens\":100,\"input_tokens\":2,\"output_tokens\":0}}}}}}\n\n"),
            "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n".into(),
            "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7},\"timings\":{\"cache_n\":100}}\n\n".into(),
            "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".into(),
        ],
        Duration::from_millis(5),
    ));
    let response = h
        .post(
            "/v1/messages",
            &json!({"messages": [{"role": "user", "content": "hi"}], "stream": true}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["content-type"], "text/event-stream");
    let text = response.text().await.unwrap();
    no_secret(&text);
    let events: Vec<Value> = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .map(|d| serde_json::from_str(d).unwrap())
        .collect();
    assert_eq!(events.len(), 4);
    assert_eq!(events[0]["message"]["model"], json!(proxy::MODEL_ALIAS));
    assert_eq!(
        events[0]["message"]["usage"],
        json!({"input_tokens": 102, "output_tokens": 0})
    );
    assert_eq!(events[1]["delta"]["text"], json!("Hi"));
    assert_eq!(events[2].get("timings"), None);
    assert!(text.contains("event: message_stop\n"));
    let rows = h.audit_done(1).await;
    assert_eq!(rows[0].result_status.as_deref(), Some("ok"));
    assert_eq!(
        rows[0].result_summary.as_deref(),
        Some("102 tokens in, 7 out")
    );
    assert_eq!(rows[0].arguments["stream"], json!(true));
}

#[tokio::test]
async fn the_models_path_never_leaks() {
    let h = Harness::new().await;
    // In an answer's fields, in a field the proxy does not know, and in errors.
    h.llama.reply(Reply::Json(
        200,
        json!({
            "model": MODEL_PATH, "id": "x", "object": "chat.completion",
            "choices": [{"message": {"role": "assistant", "content": "ok"}}],
            "extra": {"path": MODEL_PATH, "note": format!("loaded {MODEL_PATH}")},
            "system_fingerprint": "b12110-cd959bae9",
            "timings": {"cache_n": 1}
        }),
    ));
    h.llama.reply(Reply::Json(500, json!({"error": {"code": 500, "message": format!("failed to read {MODEL_PATH}"), "type": "server_error", "path": MODEL_PATH}})));
    h.llama.reply(Reply::Text(
        500,
        "text/plain",
        format!("crash in {MODEL_PATH}"),
    ));
    h.llama.reply(Reply::Json(400, json!({"error": {"code": 400, "message": "request (9000 tokens) exceeds the available context size (8192 tokens), try increasing it", "type": "exceed_context_size_error", "n_prompt_tokens": 9000, "n_ctx": 8192}})));
    let mut statuses = Vec::new();
    for _ in 0..4 {
        let response = h
            .post("/v1/chat/completions", &json!({"messages": []}))
            .send()
            .await
            .unwrap();
        statuses.push(response.status().as_u16());
        let text = response.text().await.unwrap();
        no_secret(&text);
    }
    assert_eq!(statuses, [200, 500, 500, 400]);
    // The full context comes back in the words clients know.
    h.llama.reply(Reply::Json(400, json!({"error": {"code": 400, "message": "x", "type": "exceed_context_size_error", "n_prompt_tokens": 9000, "n_ctx": 8192}})));
    let response = h
        .post("/v1/messages", &json!({"messages": []}))
        .send()
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["error"]["message"],
        json!("prompt is too long: 9000 tokens > 8192 maximum")
    );
    let rows = h.audit_done(5).await;
    assert_eq!(
        rows.iter()
            .filter(|r| r.result_status.as_deref() == Some("error"))
            .count(),
        4
    );
}

/// The headline check of the spec: refused requests are counted but never audited.
#[tokio::test]
async fn ten_thousand_unauthenticated_requests_write_no_audit_row() {
    let h = Harness::new().await;
    let mut answered = 0u32;
    let mut cut_short = 0u32;
    for i in 0..10_000u32 {
        let mut request = h
            .client
            .post(h.url("/v1/messages"))
            .header("content-type", "application/json")
            .body("{\"messages\":[]}");
        if i % 2 == 1 {
            request = request.header("x-api-key", format!("harnessui-share-{i:032x}"));
        }
        match request.send().await {
            Ok(r) if r.status() == 401 => answered += 1,
            Ok(r) if r.status() == 429 => cut_short += 1,
            Ok(r) => panic!("unexpected {}", r.status()),
            // A connection closed after a bare 429 can race the next request.
            Err(_) => cut_short += 1,
        }
    }
    assert_eq!(answered + cut_short, 10_000);
    assert!(cut_short > 0, "the per-second allowance never applied");
    assert!(h.audit().await.is_empty());
    assert!(h.llama.seen().is_empty());
    assert!(h.counters.refused() >= 9_990, "{}", h.counters.refused());
    // A good request still works right after.
    h.llama.reply(Reply::Json(200, anthropic_answer()));
    let response = h
        .post("/v1/messages", &json!({"messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(h.audit_done(1).await.len(), 1);
}

/// Round 1: a program without the key cannot keep apps out by holding every connection
/// slot. The oldest connection that never sent the key makes room for a new one, and a
/// refused request closes its connection.
#[tokio::test]
async fn connections_without_the_key_cannot_fill_every_slot() {
    use futures_util::StreamExt;
    let h = Harness::new().await;
    // A refused request: answered, then its connection is closed.
    let mut refused = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    refused
        .write_all(
            format!(
                "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n\r\n",
                h.port
            )
            .as_bytes(),
        )
        .await
        .unwrap();
    let mut answer = Vec::new();
    tokio::time::timeout(Duration::from_secs(3), refused.read_to_end(&mut answer))
        .await
        .expect("the connection stayed open after the refusal")
        .unwrap();
    let answer = String::from_utf8_lossy(&answer).to_lowercase();
    assert!(answer.starts_with("http/1.1 401"), "{answer}");
    assert!(answer.contains("connection: close"), "{answer}");

    // An app with the key is streaming an answer when every slot is taken by connections
    // that send nothing.
    let lines: Vec<String> = (0..8)
        .map(|i| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n"))
        .collect();
    h.llama.reply(Reply::Sse(lines, Duration::from_millis(100)));
    let streaming = h
        .post(
            "/v1/chat/completions",
            &json!({"messages": [], "stream": true}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(streaming.status(), 200);
    let mut idle = Vec::new();
    for _ in 0..proxy::MAX_CONNECTIONS + 8 {
        idle.push(TcpStream::connect(("127.0.0.1", h.port)).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    // A new app with the key still gets in, on a new connection, at once.
    let fresh = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = tokio::time::timeout(
        Duration::from_secs(3),
        fresh
            .get(h.url("/v1/models"))
            .header("x-api-key", SHARE_KEY)
            .send(),
    )
    .await
    .expect("no answer")
    .expect("the connection was refused");
    assert_eq!(response.status(), 200);
    // The answer being streamed was never cut: its connection had sent the key.
    let mut body = String::new();
    let mut stream = streaming.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.push_str(&String::from_utf8_lossy(
            &chunk.expect("the stream was cut"),
        ));
    }
    assert_eq!(body.matches("data: ").count(), 8, "{body}");
    // The idle connections that made room were closed.
    let mut closed = 0;
    for mut conn in idle {
        let mut byte = [0u8; 1];
        if let Ok(Ok(0)) =
            tokio::time::timeout(Duration::from_millis(50), conn.read(&mut byte)).await
        {
            closed += 1;
        }
    }
    assert!(closed >= 8, "{closed}");
}

#[tokio::test]
async fn the_busy_counter_is_held_while_a_request_runs() {
    let h = Harness::new().await;
    let lines: Vec<String> = (0..6)
        .map(|i| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n"))
        .collect();
    h.llama.reply(Reply::Sse(lines, Duration::from_millis(150)));
    // A chat turn in the app at the same time is not blocked and is counted apart.
    let turn = h.busy.hold(BusyKind::ChatTurn);
    let request = h
        .post(
            "/v1/chat/completions",
            &json!({"messages": [], "stream": true}),
        )
        .send();
    let response = tokio::time::timeout(Duration::from_secs(5), request)
        .await
        .unwrap()
        .unwrap();
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    let first = stream.next().await.unwrap().unwrap();
    assert!(!first.is_empty());
    let busy = h.busy.snapshot();
    assert_eq!((busy.other_apps, busy.chat_turns), (1, 1));
    assert_eq!(h.counters.active(), 1);
    while let Some(chunk) = stream.next().await {
        chunk.unwrap();
    }
    drop(turn);
    for _ in 0..50 {
        if !h.busy.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!h.busy.is_busy());
    assert_eq!(h.counters.active(), 0);
}

#[tokio::test]
async fn a_caller_that_goes_away_ends_the_request() {
    let h = Harness::new().await;
    let lines: Vec<String> = (0..40)
        .map(|i| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n"))
        .collect();
    h.llama.reply(Reply::Sse(lines, Duration::from_millis(100)));
    let response = h
        .post(
            "/v1/chat/completions",
            &json!({"messages": [], "stream": true}),
        )
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    stream.next().await.unwrap().unwrap();
    drop(stream);
    let rows = h.audit_done(1).await;
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
    for _ in 0..100 {
        if !h.busy.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!h.busy.is_busy());
}

/// A caller that keeps its connection open but stops reading a streamed answer is cut off
/// after [`proxy::CALLER_STALL`], so the busy counter goes back to zero and "Restart when
/// it ends" (D3) is not kept waiting by an app that hangs.
#[tokio::test]
async fn a_caller_that_stops_reading_is_cut_off() {
    let h = Harness::new().await;
    // About 32 MB of events: more than the socket buffers between the caller and the
    // proxy hold, so the proxy has to wait for the caller.
    let text = "x".repeat(4000);
    let lines: Vec<String> = (0..8000)
        .map(|_| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n"))
        .collect();
    h.llama.reply(Reply::Sse(lines, Duration::ZERO));
    let body = json!({"messages": [], "stream": true}).to_string();
    let request = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nx-api-key: {SHARE_KEY}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        h.port,
        body.len()
    );
    let mut stream = TcpStream::connect(("127.0.0.1", h.port)).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    // The request starts...
    let started = Instant::now();
    while !h.busy.is_busy() && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(h.busy.is_busy(), "the request never started");
    // ...and ends by itself although the caller neither reads nor closes.
    let wait = proxy::CALLER_STALL + Duration::from_secs(20);
    while h.busy.is_busy() && started.elapsed() < wait {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !h.busy.is_busy(),
        "still busy after {:?}",
        started.elapsed()
    );
    assert_eq!(h.counters.active(), 0);
    let rows = h.audit_done(1).await;
    assert_eq!(rows[0].result_status.as_deref(), Some("cancelled"));
    drop(stream);
}

#[tokio::test]
async fn token_counting_is_audited_at_most_once_a_minute() {
    let h = Harness::new().await;
    for _ in 0..3 {
        h.llama.reply(Reply::Json(200, json!({"input_tokens": 12})));
        let response = h
            .post("/v1/messages/count_tokens", &json!({"messages": []}))
            .send()
            .await
            .unwrap();
        assert_eq!(
            response.json::<Value>().await.unwrap(),
            json!({"input_tokens": 12})
        );
    }
    let rows = h.audit_done(1).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tool, "/v1/messages/count_tokens");
    // The count is the row's summary (the answer has no usage object).
    assert_eq!(rows[0].result_summary.as_deref(), Some("12 tokens in"));
    // Every generation request has its row.
    for _ in 0..2 {
        h.llama.reply(Reply::Json(200, anthropic_answer()));
        h.post("/v1/messages", &json!({"messages": []}))
            .send()
            .await
            .unwrap();
    }
    assert_eq!(h.audit_done(3).await.len(), 3);
}

#[tokio::test]
async fn a_model_that_does_not_answer_gives_a_clear_error() {
    let h = Harness::new().await;
    let closed = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    h.model.set(FakeModel::ready(closed, 8192));
    let response = h
        .post("/v1/responses", &json!({"input": "hi"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 502);
    let body: Value = response.json().await.unwrap();
    assert_eq!(
        body["error"]["message"],
        json!("The model in harnessUI did not answer. Try again.")
    );
    assert_eq!(
        h.audit_done(1).await[0].result_status.as_deref(),
        Some("error")
    );
    assert!(!h.busy.is_busy());
}

#[tokio::test]
async fn a_replaced_key_takes_effect_on_the_next_request() {
    let h = Harness::new().await;
    *h.key.write().unwrap() = "harnessui-share-11111111111111111111111111111111".into();
    let response = h
        .post("/v1/messages", &json!({"messages": []}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 401);
    h.llama.reply(Reply::Json(200, anthropic_answer()));
    let response = h
        .client
        .post(h.url("/v1/messages"))
        .header(
            "x-api-key",
            "harnessui-share-11111111111111111111111111111111",
        )
        .body("{}")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

/// Nothing the proxy logs holds the sharing key, the private key or the model's path,
/// whatever it was sent.
#[test]
fn the_keys_never_reach_the_log() {
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let buffer = Buffer::default();
    let writer = buffer.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let h = Harness::new().await;
        // Wrong keys, a right one, an upstream error, a model that is gone.
        for key in [
            "harnessui-share-ffffffffffffffffffffffffffffffff",
            SHARE_KEY,
        ] {
            h.llama.reply(Reply::Json(
                500,
                json!({"error": {"message": format!("bad {MODEL_PATH}")}}),
            ));
            let _ = h
                .client
                .post(h.url("/v1/messages"))
                .header("x-api-key", key)
                .body("{}")
                .send()
                .await;
            let _ = h
                .client
                .post(h.url("/v1/messages"))
                .bearer_auth(key)
                .header("origin", "null")
                .body("{}")
                .send()
                .await;
            h.llama.reply(Reply::Json(200, anthropic_answer()));
            let _ = h
                .client
                .post(h.url("/v1/messages"))
                .bearer_auth(key)
                .header("x-api-key", USERS_OWN_KEY)
                .body("{}")
                .send()
                .await;
        }
        let closed = TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        h.model.set(FakeModel::ready(closed, 8192));
        let _ = h.post("/v1/messages", &json!({})).send().await;

        // The state machine too: on, reveal, a new key, the setup, off.
        let env = test_env(&h).await;
        let sharing = Sharing::default();
        sharing.set_enabled(&env, true).await.unwrap();
        let first = sharing.reveal_key(&env).await.unwrap();
        sharing.replace_key(&env).await.unwrap();
        let second = sharing.reveal_key(&env).await.unwrap();
        sharing.setup(&env, ShareApp::ClaudeCode).await.unwrap();
        sharing.set_enabled(&env, false).await.unwrap();
        let log = String::from_utf8_lossy(&buffer.0.lock().unwrap()).to_string();
        assert!(log.contains("refused"), "nothing was logged at all: {log}");
        for secret in [
            SHARE_KEY,
            PRIVATE,
            first.as_str(),
            second.as_str(),
            "Secret-Model",
            "ffffffffffffffff",
            USERS_OWN_KEY,
        ] {
            assert!(!log.contains(secret), "{secret} is in the log:\n{log}");
        }
    });
}

// ---- The state machine -------------------------------------------------------------

async fn test_env(h: &Harness) -> Env {
    Env {
        store: h.store.clone(),
        secrets: Arc::new(store::secrets::MemorySecretStore::new()),
        model: h.model.clone(),
        busy: h.busy.clone(),
    }
}

fn env_with(model: Upstream) -> Env {
    Env {
        store: store::Store::open_in_memory().unwrap(),
        secrets: Arc::new(store::secrets::MemorySecretStore::new()),
        model: Arc::new(FakeModel(Mutex::new(model))),
        busy: Arc::new(BusyCounter::default()),
    }
}

async fn saved(env: &Env) -> Option<ShareSettings> {
    env.store.get_setting(SETTINGS_KEY).await.unwrap()
}

async fn port_open(port: u16) -> bool {
    TcpStream::connect(("127.0.0.1", port)).await.is_ok()
}

#[tokio::test]
async fn switching_on_picks_a_port_in_range_and_keeps_it() {
    let env = env_with(Upstream::NotLoaded);
    let sharing = Sharing::default();
    let off = sharing.status(&env).await;
    assert_eq!(
        off,
        ShareStatus {
            enabled: false,
            port: None,
            listening: false,
            error: None,
            refused: 0,
            in_use: false,
            model_ctx: None
        }
    );

    let on = sharing.set_enabled(&env, true).await.unwrap();
    let port = on.port.unwrap();
    assert!(PORT_RANGE.contains(&port), "{port}");
    assert!(on.enabled && on.listening && on.error.is_none());
    assert_eq!(
        saved(&env).await,
        Some(ShareSettings {
            enabled: true,
            port
        })
    );
    assert!(port_open(port).await);

    // The key is in the keychain, in its form, and nowhere in the settings.
    let key = env.secrets.get(KEY_SECRET).unwrap().unwrap();
    assert!(is_key(&key), "{key}");
    assert_eq!(sharing.reveal_key(&env).await.unwrap(), key);
    let all = env
        .store
        .get_setting::<Value>(SETTINGS_KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(!all.to_string().contains(&key));

    // Off: the port is closed, the port is kept for next time.
    let off = sharing.set_enabled(&env, false).await.unwrap();
    assert!(!off.enabled && !off.listening && off.error.is_none());
    assert_eq!(off.port, Some(port));
    assert!(!port_open(port).await);
    assert_eq!(
        saved(&env).await,
        Some(ShareSettings {
            enabled: false,
            port
        })
    );

    // On again: the same port and the same key.
    let again = sharing.set_enabled(&env, true).await.unwrap();
    assert_eq!(again.port, Some(port));
    assert!(again.listening);
    assert_eq!(sharing.reveal_key(&env).await.unwrap(), key);
    // On twice changes nothing.
    assert_eq!(
        sharing.set_enabled(&env, true).await.unwrap().port,
        Some(port)
    );
    sharing.shutdown().await;
    assert!(!port_open(port).await);
}

#[tokio::test]
async fn a_taken_kept_port_says_so_and_listens_nowhere() {
    let env = env_with(Upstream::NotLoaded);
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = taken.local_addr().unwrap().port();
    env.store
        .set_setting(
            SETTINGS_KEY,
            &ShareSettings {
                enabled: false,
                port,
            },
        )
        .await
        .unwrap();
    let sharing = Sharing::default();
    let status = sharing.set_enabled(&env, true).await.unwrap();
    assert!(status.enabled && !status.listening);
    assert_eq!(
        status.error,
        Some(format!(
            "Port {port} is used by another program. Choose another port."
        ))
    );
    assert_eq!(status.port, Some(port));
    // Choosing another port fixes it.
    let fixed = sharing.set_port(&env, 0).await.unwrap();
    assert!(fixed.listening && fixed.error.is_none());
    let new_port = fixed.port.unwrap();
    assert_ne!(new_port, port);
    assert!(PORT_RANGE.contains(&new_port));
    assert!(port_open(new_port).await);
    sharing.shutdown().await;
}

#[tokio::test]
async fn choosing_another_port() {
    let env = env_with(Upstream::NotLoaded);
    let sharing = Sharing::default();
    // While off: saved only, and checked.
    let free = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let status = sharing.set_port(&env, free).await.unwrap();
    assert_eq!(
        (status.enabled, status.port, status.listening),
        (false, Some(free), false)
    );
    assert!(!port_open(free).await);
    // Too low.
    assert_eq!(
        sharing.set_port(&env, 80).await.unwrap_err().message,
        "Choose a port from 1024 to 65535."
    );
    // On, then a taken port: nothing changes and the error says why.
    let on = sharing.set_enabled(&env, true).await.unwrap();
    assert_eq!(on.port, Some(free));
    let taken = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let taken_port = taken.local_addr().unwrap().port();
    let err = sharing.set_port(&env, taken_port).await.unwrap_err();
    assert_eq!(
        err.message,
        format!("Port {taken_port} is used by another program. Choose another port.")
    );
    assert_eq!(sharing.status(&env).await.port, Some(free));
    assert!(port_open(free).await);
    // A free one moves the proxy there.
    drop(taken);
    let moved = sharing.set_port(&env, taken_port).await.unwrap();
    assert_eq!(moved.port, Some(taken_port));
    assert!(moved.listening);
    assert!(port_open(taken_port).await);
    assert!(!port_open(free).await);
    // The port it already listens on is not "used by another program".
    let same = sharing.set_port(&env, taken_port).await.unwrap();
    assert_eq!(
        (same.port, same.listening, same.error),
        (Some(taken_port), true, None)
    );
    assert!(port_open(taken_port).await);
    sharing.shutdown().await;
}

#[tokio::test]
async fn a_new_key_replaces_the_old_one_at_once() {
    let llama = FakeLlama::default();
    let addr = llama.start().await;
    let env = env_with(FakeModel::ready(addr, 8192));
    let sharing = Sharing::default();
    let port = sharing.set_enabled(&env, true).await.unwrap().port.unwrap();
    let old = sharing.reveal_key(&env).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let send = |key: String| {
        client
            .post(format!("http://127.0.0.1:{port}/v1/messages"))
            .header("x-api-key", key)
            .body("{\"messages\":[]}")
            .send()
    };
    llama.reply(Reply::Json(200, anthropic_answer()));
    assert_eq!(send(old.clone()).await.unwrap().status(), 200);
    sharing.replace_key(&env).await.unwrap();
    let new = sharing.reveal_key(&env).await.unwrap();
    assert_ne!(new, old);
    assert!(is_key(&new));
    assert_eq!(env.secrets.get(KEY_SECRET).unwrap(), Some(new.clone()));
    assert_eq!(send(old).await.unwrap().status(), 401);
    llama.reply(Reply::Json(200, anthropic_answer()));
    assert_eq!(send(new).await.unwrap().status(), 200);
    // Refusals are counted on the page; switching on again starts the count anew.
    assert_eq!(sharing.status(&env).await.refused, 1);
    sharing.set_enabled(&env, false).await.unwrap();
    assert_eq!(sharing.set_enabled(&env, true).await.unwrap().refused, 0);
    sharing.shutdown().await;
}

/// What the Coding apps see (WP6): the kept port and the switch always, the listening port
/// and the sharing key the proxy checks only while sharing is on and listening.
#[tokio::test]
async fn the_coding_apps_get_the_key_only_while_sharing_listens() {
    let env = env_with(Upstream::NotLoaded);
    let sharing = Sharing::default();
    assert_eq!(sharing.now(&env).await, ShareNow::default());
    let port = sharing.set_enabled(&env, true).await.unwrap().port.unwrap();
    let key = sharing.reveal_key(&env).await.unwrap();
    let on = sharing.now(&env).await;
    assert_eq!(
        on,
        ShareNow {
            port: Some(port),
            enabled: true,
            listening: Some(port),
            key: Some(key.clone()),
        }
    );
    assert!(!format!("{on:?}").contains(&key), "{on:?}");
    // A new key: the new one, at once.
    sharing.replace_key(&env).await.unwrap();
    let new = sharing.reveal_key(&env).await.unwrap();
    assert_eq!(sharing.now(&env).await.key, Some(new));
    // Off: the kept port, nothing listening, no key.
    sharing.set_enabled(&env, false).await.unwrap();
    let off = ShareNow {
        port: Some(port),
        enabled: false,
        listening: None,
        key: None,
    };
    assert_eq!(sharing.now(&env).await, off);
    // On, but the kept port was taken meanwhile: not listening, no key.
    let taken = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    assert!(!sharing.set_enabled(&env, true).await.unwrap().listening);
    assert_eq!(
        sharing.now(&env).await,
        ShareNow {
            enabled: true,
            ..off
        }
    );
    // The user picks another port: that one is kept and listened on, with the key, and the
    // old one is neither (an app still set up for it is not asked:
    // outside_agent::tests::an_app_set_up_for_another_port_of_this_computer_is_not_asked).
    let moved = sharing.set_port(&env, 0).await.unwrap();
    let new_port = moved.port.unwrap();
    assert!(moved.listening);
    assert_ne!(new_port, port);
    assert_eq!(
        sharing.now(&env).await,
        ShareNow {
            port: Some(new_port),
            enabled: true,
            listening: Some(new_port),
            key: Some(sharing.reveal_key(&env).await.unwrap()),
        }
    );
    drop(taken);
    sharing.shutdown().await;
}

#[tokio::test]
async fn the_status_shows_the_loaded_models_context_and_use() {
    let llama = FakeLlama::default();
    let addr = llama.start().await;
    let env = env_with(FakeModel::ready(addr, 16_384));
    let sharing = Sharing::default();
    let status = sharing.set_enabled(&env, true).await.unwrap();
    assert_eq!(status.model_ctx, Some(16_384));
    assert!(!status.in_use);
    let port = status.port.unwrap();
    let lines: Vec<String> = (0..5)
        .map(|_| "data: {\"choices\":[]}\n\n".to_owned())
        .collect();
    llama.reply(Reply::Sse(lines, Duration::from_millis(150)));
    let key = sharing.reveal_key(&env).await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .bearer_auth(key)
        .body("{\"messages\":[],\"stream\":true}")
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    use futures_util::StreamExt;
    stream.next().await.unwrap().unwrap();
    assert!(sharing.status(&env).await.in_use);
    while stream.next().await.is_some() {}
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!sharing.status(&env).await.in_use);
    sharing.shutdown().await;
}

/// Switching sharing off while another app's answer streams closes the port at once and
/// ends that request; the busy counter goes back to zero (no "Restart when it ends" is kept
/// waiting by a request the user has cut off).
#[tokio::test]
async fn switching_off_ends_a_running_request() {
    use futures_util::StreamExt;
    let llama = FakeLlama::default();
    let addr = llama.start().await;
    let env = env_with(FakeModel::ready(addr, 8192));
    let sharing = Sharing::default();
    let port = sharing.set_enabled(&env, true).await.unwrap().port.unwrap();
    let key = sharing.reveal_key(&env).await.unwrap();
    // About 20 seconds of events.
    let lines: Vec<String> = (0..100)
        .map(|i| format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{i}\"}}}}]}}\n\n"))
        .collect();
    llama.reply(Reply::Sse(lines, Duration::from_millis(200)));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
        .bearer_auth(&key)
        .body("{\"messages\":[],\"stream\":true}")
        .send()
        .await
        .unwrap();
    let mut stream = response.bytes_stream();
    stream.next().await.unwrap().unwrap();
    assert!(env.busy.is_busy());

    let started = Instant::now();
    let off = sharing.set_enabled(&env, false).await.unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert!(!off.enabled && !off.listening, "{off:?}");
    assert!(!port_open(port).await);
    // The caller's answer is cut off instead of running on.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(chunk) = stream.next().await {
            if chunk.is_err() {
                return;
            }
        }
    })
    .await;
    assert!(
        ended.is_ok(),
        "the request ran on after sharing was switched off"
    );
    for _ in 0..150 {
        if !env.busy.is_busy() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!env.busy.is_busy());
    sharing.shutdown().await;
}

#[tokio::test]
async fn the_setup_needs_sharing_on_and_holds_the_port_key_and_context() {
    let llama = FakeLlama::default();
    let addr = llama.start().await;
    let env = env_with(FakeModel::ready(addr, 16_384));
    let sharing = Sharing::default();
    let err = sharing.setup(&env, ShareApp::ClaudeCode).await.unwrap_err();
    assert_eq!(
        err.message,
        "Switch sharing on first, so the setup has a port and a key."
    );
    let port = sharing.set_enabled(&env, true).await.unwrap().port.unwrap();
    let key = sharing.reveal_key(&env).await.unwrap();
    let setup = sharing.setup(&env, ShareApp::ClaudeCode).await.unwrap();
    let json = setup.ways[2].steps[0].code.clone().unwrap();
    let parsed: Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        parsed["env"]["ANTHROPIC_BASE_URL"],
        json!(format!("http://127.0.0.1:{port}"))
    );
    assert_eq!(parsed["env"]["ANTHROPIC_AUTH_TOKEN"], json!(key));
    assert_eq!(parsed["env"]["ANTHROPIC_MODEL"], json!("harnessui-local"));
    assert_eq!(
        parsed["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"],
        json!("16384")
    );
    let codex = sharing.setup(&env, ShareApp::Codex).await.unwrap();
    let toml: toml::Table =
        toml::from_str(codex.ways[0].steps[0].code.as_deref().unwrap()).unwrap();
    assert_eq!(toml["model_context_window"].as_integer(), Some(16_384));
    assert_eq!(
        toml["model_providers"]["harnessui"]["base_url"].as_str(),
        Some(format!("http://127.0.0.1:{port}/v1").as_str())
    );
    sharing.shutdown().await;
}

/// The switch never turns itself on: at start the proxy listens only if the user left
/// sharing on.
#[tokio::test]
async fn at_start_it_listens_only_if_left_on() {
    let env = env_with(Upstream::NotLoaded);
    let sharing = Sharing::default();
    sharing.start_saved(&env).await;
    assert!(!sharing.status(&env).await.enabled);
    assert!(
        env.secrets.get(KEY_SECRET).unwrap().is_none(),
        "a key was made without the switch"
    );

    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    env.store
        .set_setting(
            SETTINGS_KEY,
            &ShareSettings {
                enabled: false,
                port,
            },
        )
        .await
        .unwrap();
    sharing.start_saved(&env).await;
    let status = sharing.status(&env).await;
    assert!(!status.enabled && !status.listening);
    assert!(!port_open(port).await);

    env.store
        .set_setting(
            SETTINGS_KEY,
            &ShareSettings {
                enabled: true,
                port,
            },
        )
        .await
        .unwrap();
    sharing.start_saved(&env).await;
    let status = sharing.status(&env).await;
    assert!(status.enabled && status.listening, "{status:?}");
    assert!(port_open(port).await);
    sharing.shutdown().await;
}

/// D12: sharing is not a tool action, and no approval mode changes it. Whatever mode every
/// chat starts in (Ask first, Allow all, or a saved Full access), the switch stays as the
/// user left it at start, and the proxy asks for the key, refuses web pages and writes the
/// same audit row (never the Full access decision or note).
#[tokio::test]
async fn no_approval_mode_changes_sharing() {
    for mode in ["ask", "allowAll", "fullAccess"] {
        let llama = FakeLlama::default();
        let addr = llama.start().await;
        let env = env_with(FakeModel::ready(addr, 8192));
        env.store
            .set_setting(crate::approvals::APPROVAL_MODE_DEFAULT, &json!(mode))
            .await
            .unwrap();
        env.store
            .set_setting(crate::approvals::ALLOW_ALL_DEFAULT, &json!(mode != "ask"))
            .await
            .unwrap();
        let sharing = Sharing::default();
        sharing.start_saved(&env).await;
        let status = sharing.status(&env).await;
        assert!(!status.enabled && !status.listening, "{mode}: {status:?}");
        assert!(env.secrets.get(KEY_SECRET).unwrap().is_none(), "{mode}");

        let port = sharing.set_enabled(&env, true).await.unwrap().port.unwrap();
        let key = sharing.reveal_key(&env).await.unwrap();
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        let url = format!("http://127.0.0.1:{port}/v1/messages");
        let without = client.post(&url).body("{}").send().await.unwrap();
        assert_eq!(without.status(), 401, "{mode}");
        let page = client
            .post(&url)
            .header("x-api-key", &key)
            .header("origin", "http://127.0.0.1")
            .body("{}")
            .send()
            .await
            .unwrap();
        assert_eq!(page.status(), 403, "{mode}");
        llama.reply(Reply::Json(200, anthropic_answer()));
        let with = client
            .post(&url)
            .header("x-api-key", &key)
            .body("{\"messages\":[]}")
            .send()
            .await
            .unwrap();
        assert_eq!(with.status(), 200, "{mode}");
        // Round 1: an image given as a link is refused by the proxy's own rule in every
        // mode, Full access included, with its row.
        let link = client
            .post(&url)
            .header("x-api-key", &key)
            .body(
                json!({"messages": [{"role": "user", "content": [{"type": "image",
                "source": {"type": "url", "url": "http://example.com/a.png"}}]}]})
                .to_string(),
            )
            .send()
            .await
            .unwrap();
        assert_eq!(link.status(), 400, "{mode}");
        let mut rows = Vec::new();
        for _ in 0..100 {
            rows = env
                .store
                .list_audit(store::AuditQuery {
                    server: Some(proxy::AUDIT_SERVER.into()),
                    ..Default::default()
                })
                .await
                .unwrap();
            if rows.iter().all(|r| r.result_status.is_some()) && rows.len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(rows.len(), 2, "{mode}");
        let seen: Vec<(store::AuditDecision, Option<&str>)> = rows
            .iter()
            .map(|r| (r.decision, r.result_status.as_deref()))
            .collect();
        assert!(
            seen.contains(&(store::AuditDecision::AllowedWithoutPrompt, Some("ok")))
                && seen.contains(&(store::AuditDecision::RefusedByPolicy, Some("not_run"))),
            "{mode}: {seen:?}"
        );
        assert!(rows.iter().all(|r| r.note.is_none()), "{mode}");
        sharing.set_enabled(&env, false).await.unwrap();
        assert!(!port_open(port).await, "{mode}");
        sharing.shutdown().await;
    }
}

/// D12: the switch, the port and the key stay out of reach of every tool in every mode.
/// The generic `set_setting` command (the only settings write a page could reach without
/// its own command) never lists the sharing key, and nothing here reads an approval mode.
#[test]
fn the_sharing_switch_is_out_of_reach_of_tools() {
    let source = include_str!("../models_cmd.rs");
    let start = source
        .find("const ALLOWED: &[&str] = &[")
        .expect("the allow-list of set_setting");
    let list = &source[start..start + source[start..].find("];").expect("its end")];
    assert!(list.contains("\"theme\""), "this is the list: {list}");
    for word in [SETTINGS_KEY, "share", "Share"] {
        assert!(!list.contains(word), "{word} in {list}");
    }
    for (name, module) in [
        ("mod.rs", include_str!("mod.rs")),
        ("proxy.rs", include_str!("proxy.rs")),
        ("setup_text.rs", include_str!("setup_text.rs")),
    ] {
        for word in ["approvals::", "ApprovalMode", "full_access(", "allow_all("] {
            assert!(
                !module.contains(word),
                "{name} reads the approval mode ({word})"
            );
        }
    }
}

#[test]
fn keys_look_right_and_differ() {
    let a = new_key();
    let b = new_key();
    assert_ne!(a, b);
    for key in [&a, &b] {
        assert!(is_key(key), "{key}");
        assert_eq!(key.len(), KEY_PREFIX.len() + 32);
    }
    for bad in [
        "",
        "harnessui-share-",
        "harnessui-share-0123",
        "harnessui-share-0123456789ABCDEF0123456789abcdef",
        "harnessui-share-0123456789abcdef0123456789abcdeg",
        "x-0123456789abcdef0123456789abcdef",
    ] {
        assert!(!is_key(bad), "{bad}");
    }
}

/// Every command of this module checks the calling window first, like its neighbours.
#[test]
fn every_command_is_for_the_main_window_only() {
    let source = include_str!("mod.rs");
    let commands: Vec<&str> = source.split("#[tauri::command]").skip(1).collect();
    assert_eq!(commands.len(), 6);
    for command in commands {
        let body = command.split_once('{').unwrap().1;
        let first = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap();
        assert_eq!(
            first, "crate::commands::only_main_window(window.label())?;",
            "{command}"
        );
    }
    let capabilities = include_str!("../../capabilities/main.json");
    let list = include_str!("../../commands.list");
    for name in [
        "share_status",
        "set_share_enabled",
        "set_share_port",
        "reveal_share_key",
        "new_share_key",
        "share_setup",
    ] {
        assert!(
            capabilities.contains(&format!("\"allow-{}\"", name.replace('_', "-"))),
            "{name}"
        );
        assert!(list.lines().any(|l| l.trim() == name), "{name}");
    }
}

/// The webview reads exactly these shapes (src/lib/types.ts). Moved here from WP0's
/// stub, with the real setup text in place of the sample.
#[test]
fn shapes() {
    let status = ShareStatus {
        enabled: false,
        port: None,
        listening: false,
        error: None,
        refused: 0,
        in_use: false,
        model_ctx: None,
    };
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        json!({
            "enabled": false,
            "port": null,
            "listening": false,
            "error": null,
            "refused": 0,
            "inUse": false,
            "modelCtx": null
        })
    );
    assert_eq!(
        serde_json::to_value(ShareSettings {
            enabled: true,
            port: 50000
        })
        .unwrap(),
        json!({"enabled": true, "port": 50000})
    );
    let input = setup_text::SetupInput {
        port: 51_234,
        key: SHARE_KEY.into(),
        model: "harnessui-local".into(),
        ctx: Some(32_768),
    };
    for (app, name, first_system) in [
        (ShareApp::ClaudeCode, "claudeCode", "windows"),
        (ShareApp::Codex, "codex", "windows"),
        (ShareApp::ClaudeApp, "claudeApp", "any"),
    ] {
        assert_eq!(serde_json::to_value(app).unwrap(), json!(name));
        let setup = serde_json::to_value(setup_text::build(app, &input)).unwrap();
        assert_eq!(setup["app"], json!(name));
        assert_eq!(setup["ways"][0]["system"], json!(first_system));
        assert!(setup["ways"][0]["steps"][0].get("code").is_some());
        assert!(setup["ways"][0]["steps"][0].get("file").is_some());
        assert_eq!(setup["notTried"], json!(app == ShareApp::ClaudeApp));
    }
}
