//! Remote (Streamable HTTP) servers through the host, against a small hand-written MCP
//! server on a loopback port: the legacy handshake, listing and calling tools, secret
//! placeholders in the URL and headers, redaction in the server log, a refused
//! cross-origin redirect, the sign-in (OAuth) message and the legacy-SSE message.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use mcp_host::{
    CallContext, ChatContext, Described, Host, HostError, HostOptions, Lifecycle, ServerConfig,
    ServerKind, ServerState, ToolPrefs,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_util::sync::CancellationToken;

/// One request the fake server received.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    rpc_method: Option<String>,
}

#[derive(Clone, Copy)]
enum Mode {
    /// A working MCP server (stateless, JSON responses).
    Mcp,
    /// Every request: 401 with a `WWW-Authenticate` challenge.
    SignIn,
    /// Every request: 404.
    NotFound,
    /// Every request: 307 to the given base URL.
    Redirect,
    /// Reads the request and never answers.
    Silent,
}

struct FakeServer {
    base: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

async fn start(mode: Mode, redirect_to: Option<String>) -> FakeServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("addr"));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let log = log.clone();
            let redirect_to = redirect_to.clone();
            tokio::spawn(async move { handle(socket, mode, redirect_to, log).await });
        }
    });
    FakeServer { base, seen }
}

async fn handle(
    mut socket: TcpStream,
    mode: Mode,
    redirect_to: Option<String>,
    log: Arc<Mutex<Vec<Seen>>>,
) {
    let Some((method, path, headers, body)) = read_request(&mut socket).await else {
        return;
    };
    let rpc: Option<Value> = serde_json::from_slice(&body).ok();
    let rpc_method = rpc
        .as_ref()
        .and_then(|v| v.get("method"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    log.lock().unwrap().push(Seen {
        method: method.clone(),
        path: path.clone(),
        headers,
        rpc_method: rpc_method.clone(),
    });
    let response = match mode {
        Mode::Silent => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            return;
        }
        Mode::SignIn => reply(
            401,
            "Unauthorized",
            &[("www-authenticate", "Bearer realm=\"mcp\"")],
            "",
        ),
        Mode::NotFound => reply(404, "Not Found", &[], ""),
        Mode::Redirect => {
            let location = format!("{}/mcp", redirect_to.unwrap_or_default());
            reply(307, "Temporary Redirect", &[("location", &location)], "")
        }
        Mode::Mcp => match (method.as_str(), rpc) {
            ("POST", Some(rpc)) => mcp_reply(&rpc),
            ("POST", None) => reply(400, "Bad Request", &[], ""),
            ("DELETE", _) => reply(200, "OK", &[], ""),
            _ => reply(405, "Method Not Allowed", &[], ""),
        },
    };
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn mcp_reply(rpc: &Value) -> String {
    let Some(id) = rpc.get("id").cloned() else {
        // A notification.
        return reply(202, "Accepted", &[], "");
    };
    let method = rpc
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let body = match method {
        "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
            "protocolVersion": "2025-11-25",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "http-fake", "version": "1.0"}
        }}),
        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {"tools": [{
            "name": "greet",
            "description": "Say hello",
            "inputSchema": {"type": "object", "properties": {"name": {"type": "string"}}, "required": ["name"]}
        }]}}),
        "tools/call" => {
            let name = rpc
                .pointer("/params/arguments/name")
                .and_then(Value::as_str)
                .unwrap_or("?");
            json!({"jsonrpc": "2.0", "id": id, "result": {
                "content": [{"type": "text", "text": format!("hello {name}")}]
            }})
        }
        _ => {
            json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "method not found"}})
        }
    };
    reply(
        200,
        "OK",
        &[("content-type", "application/json")],
        &body.to_string(),
    )
}

fn reply(status: u16, reason: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut out = format!("HTTP/1.1 {status} {reason}\r\n");
    for (name, value) in headers {
        out.push_str(&format!("{name}: {value}\r\n"));
    }
    out.push_str(&format!(
        "content-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    ));
    out
}

type Request = (String, String, HashMap<String, String>, Vec<u8>);

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > 64 * 1024 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_owned();
    let path = first.next()?.to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut body = buf[header_end + 4..].to_vec();
    while body.len() < length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    Some((method, path, headers, body))
}

fn options() -> HostOptions {
    HostOptions {
        start_timeout: Duration::from_secs(10),
        call_timeout: Duration::from_secs(10),
        secret_resolver: Arc::new(|name: &str| match name {
            "tok" => Some("tok-123".to_owned()),
            "key" => Some("key-456".to_owned()),
            _ => None,
        }),
        ..HostOptions::default()
    }
}

#[tokio::test]
async fn connects_lists_and_calls_over_http() {
    let server = start(Mode::Mcp, None).await;
    let host = Host::new(options());
    let cfg = ServerConfig::Http {
        url: format!("{}/mcp?key=${{secret:key}}", server.base),
        headers: BTreeMap::from([(
            "Authorization".to_owned(),
            "Bearer ${secret:tok}".to_owned(),
        )]),
    };
    host.add_external("web", cfg).await.expect("connects");
    let status = host
        .servers()
        .into_iter()
        .find(|s| s.name == "web")
        .expect("status");
    assert_eq!(status.state, ServerState::Ready);
    assert_eq!(status.kind, ServerKind::Http);
    assert_eq!(status.lifecycle, Some(Lifecycle::Legacy));
    assert_eq!(status.protocol_version.as_deref(), Some("2025-11-25"));
    assert_eq!(status.pid, None);
    // The log names the server without its query string or secrets.
    let log = status.stderr_tail.join("\n");
    assert!(
        log.contains(&format!("connecting to {}/mcp", server.base)),
        "{log}"
    );
    assert!(
        !log.contains("key-456") && !log.contains("tok-123"),
        "{log}"
    );

    let tool = host
        .tools()
        .into_iter()
        .find(|t| t.exposed == "web__greet")
        .expect("tool");
    assert_eq!(tool.def.description, "Say hello");
    let args = json!({"name": "Ada"}).as_object().cloned().expect("object");
    let Described::Action(action) = host
        .describe(
            "web__greet",
            &args,
            &ChatContext::default(),
            ToolPrefs::default(),
        )
        .await
    else {
        panic!("no action")
    };
    let ctx = CallContext {
        chat: ChatContext::default(),
        cancel: CancellationToken::new(),
        progress: None,
    };
    let out = host.call("web__greet", &args, &action, ctx).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.joined_text(), "hello Ada");

    let seen = server.seen.lock().unwrap().clone();
    let methods: Vec<&str> = seen
        .iter()
        .filter_map(|s| s.rpc_method.as_deref())
        .collect();
    assert_eq!(
        methods,
        vec![
            "initialize",
            "notifications/initialized",
            "tools/list",
            "tools/call"
        ]
    );
    for request in &seen {
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/mcp?key=key-456");
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer tok-123")
        );
    }
    host.shutdown().await;
}

#[tokio::test]
async fn cross_origin_redirects_are_not_followed() {
    let elsewhere = start(Mode::Mcp, None).await;
    let redirector = start(Mode::Redirect, Some(elsewhere.base.clone())).await;
    let host = Host::new(options());
    let cfg = ServerConfig::Http {
        url: format!("{}/mcp", redirector.base),
        headers: BTreeMap::from([("X-Api-Key".to_owned(), "${secret:tok}".to_owned())]),
    };
    let error = host.add_external("hop", cfg).await.unwrap_err();
    assert!(matches!(error, HostError::Connect(_)), "{error:?}");
    assert_eq!(redirector.seen.lock().unwrap().len(), 1);
    assert!(
        elsewhere.seen.lock().unwrap().is_empty(),
        "the headers were replayed to another origin"
    );
}

#[tokio::test]
async fn sign_in_required_is_explained() {
    let server = start(Mode::SignIn, None).await;
    let host = Host::new(options());
    let cfg = ServerConfig::Http {
        url: format!("{}/mcp", server.base),
        headers: BTreeMap::new(),
    };
    assert_eq!(
        host.add_external("oauth", cfg).await,
        Err(HostError::AuthRequired)
    );
    let status = host.servers().into_iter().next().expect("status");
    assert!(
        matches!(&status.state, ServerState::Failed { reason } if reason.contains("signing in"))
    );
}

#[tokio::test]
async fn failing_legacy_sse_entries_point_to_the_bridge() {
    let server = start(Mode::NotFound, None).await;
    let host = Host::new(options());
    let cfg = ServerConfig::LegacySse {
        url: format!("{}/sse", server.base),
        headers: BTreeMap::new(),
    };
    assert_eq!(
        host.add_external("old", cfg).await,
        Err(HostError::LegacySseUnsupported)
    );
    let status = host.servers().into_iter().next().expect("status");
    let ServerState::Failed { reason } = &status.state else {
        panic!("not failed: {status:?}")
    };
    assert!(
        reason.contains("legacy SSE transport") && reason.contains("mcp-remote"),
        "{reason}"
    );
    // It was tried as Streamable HTTP first.
    assert!(!server.seen.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_legacy_sse_entry_that_speaks_streamable_http_just_works() {
    let server = start(Mode::Mcp, None).await;
    let host = Host::new(options());
    let cfg = ServerConfig::LegacySse {
        url: format!("{}/mcp", server.base),
        headers: BTreeMap::new(),
    };
    host.add_external("relabelled", cfg)
        .await
        .expect("connects");
    assert!(host.tool_ready("relabelled__greet"));
    host.shutdown().await;
}

#[tokio::test]
async fn removing_a_server_that_is_still_connecting_stops_the_attempt() {
    let server = start(Mode::Silent, None).await;
    let mut opts = options();
    opts.start_timeout = Duration::from_secs(30);
    let host = Host::new(opts);
    let cfg = ServerConfig::Http {
        url: format!("{}/mcp", server.base),
        headers: BTreeMap::new(),
    };
    let adder = host.clone();
    let adding = tokio::spawn(async move { adder.add_external("hung", cfg).await });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while server.seen.lock().unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "no request arrived");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let started = std::time::Instant::now();
    host.remove("hung").await.expect("removed");
    let result = tokio::time::timeout(Duration::from_secs(5), adding)
        .await
        .expect("the start ended")
        .expect("join");
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(result, Err(HostError::NotFound("hung".into())));
    assert!(host.servers().is_empty());
}
