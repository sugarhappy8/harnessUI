//! End-to-end tests against a real llama-server.
//!
//! Gated on two environment variables, and skipped (with a note) when either is unset so CI
//! without a model passes:
//!
//! - `LLAMA_SERVER_BIN`: path to the pinned llama-server (b12110);
//! - `TEST_GGUF`: a small chat model with tool support, e.g. Qwen3-0.6B-Q8_0.gguf.
//!
//! The server is started in managed mode (`LLAMA_SERVER_ROUTER_PORT` set, stdin piped), as the
//! runtime crate starts it, on a free port with a per-run API key, and killed at the end, also
//! on panic.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use agent::{
    ArgumentsStatus, ChatMessage, ChatRequest, ChatStream, ClientConfig, OpenAiClient,
    ProviderError, ProviderKind, StreamEvent, ToolCallAccumulator, ToolChoice, ToolSpec, Usage,
};
use futures_util::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const CONTEXT: u32 = 2048;

/// Kills the server when dropped, so a failing assertion does not leave it running.
struct ServerGuard {
    child: Child,
    log: PathBuf,
    _dir: tempfile::TempDir,
}

impl ServerGuard {
    fn log_tail(&self) -> String {
        let mut text = String::new();
        if let Ok(mut file) = std::fs::File::open(&self.log) {
            let _ = file.read_to_string(&mut text);
        }
        let lines: Vec<&str> = text.lines().collect();
        lines[lines.len().saturating_sub(40)..].join("\n")
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        // Managed mode: ask politely first (only honoured once the model has loaded).
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(b"cmd_router_to_child:exit\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Variables passed through to the server after the environment is cleared, mirroring the
/// runtime crate's allowlist. Windows needs `SYSTEMROOT` (and friends): without it Winsock
/// cannot initialise and the server cannot open its port.
#[cfg(windows)]
const ENV_ALLOWLIST: &[&str] = &[
    "PATH",
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "LOCALAPPDATA",
];
#[cfg(not(windows))]
const ENV_ALLOWLIST: &[&str] = &["PATH", "HOME", "TMPDIR"];

fn free_port() -> u16 {
    loop {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        if port != 9931 && port != 18080 {
            return port;
        }
    }
}

// Tests are exempt from the spawn-helper rule (scripts/check-spawn.sh); production code
// starts llama-server through crates/runtime.
#[allow(clippy::disallowed_methods)]
fn start_server(bin: &Path, model: &Path, port: u16, key: &str) -> ServerGuard {
    let dir = tempfile::tempdir().expect("temp dir");
    let log = dir.path().join("llama-server.log");
    let log_file = std::fs::File::create(&log).expect("log file");
    let mut command = std::process::Command::new(bin); // spawn-ok: test
    command
        .args(["--host", "127.0.0.1", "--port", &port.to_string()])
        .args(["--no-webui", "--offline", "--jinja"])
        .arg("-m")
        .arg(model)
        .args(["-c", &CONTEXT.to_string(), "-np", "1", "--device", "none"])
        .env_clear()
        .env("LLAMA_API_KEY", key)
        .env("LLAMA_SERVER_ROUTER_PORT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file));
    for name in ENV_ALLOWLIST {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    if let Some(dir) = bin.parent() {
        command.current_dir(dir);
    }
    let child = command.spawn().expect("start llama-server");
    ServerGuard {
        child,
        log,
        _dir: dir,
    }
}

async fn wait_healthy(guard: &mut ServerGuard, port: u16) {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        if let Ok(Some(status)) = guard.child.try_wait() {
            panic!(
                "llama-server exited with {status} before it was ready:\n{}",
                guard.log_tail()
            );
        }
        if let Ok(response) = http
            .get(format!("http://127.0.0.1:{port}/health"))
            .send()
            .await
            && response.status() == reqwest::StatusCode::OK
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "llama-server did not become healthy:\n{}",
            guard.log_tail()
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

async fn collect(mut stream: ChatStream) -> Vec<StreamEvent> {
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item.expect("stream error"));
    }
    events
}

fn no_thinking(mut request: ChatRequest) -> ChatRequest {
    request.set_chat_template_kwarg("enable_thinking", json!(false));
    request
}

fn weather_request() -> ChatRequest {
    let mut request = no_thinking(ChatRequest::new(
        "x",
        vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("What is the weather in Paris? Use the tool."),
        ],
    ));
    request.tools = vec![ToolSpec::function(
        "get_weather",
        "Get the weather for a city",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    )];
    request.tool_choice = Some(ToolChoice::Required);
    request.parallel_tool_calls = Some(false);
    request.max_tokens = Some(256);
    request
}

#[tokio::test(flavor = "multi_thread")]
async fn real_llama_server_end_to_end() {
    let (Some(bin), Some(model)) = (
        std::env::var_os("LLAMA_SERVER_BIN"),
        std::env::var_os("TEST_GGUF"),
    ) else {
        eprintln!("skipping real_llama_server_end_to_end: set LLAMA_SERVER_BIN and TEST_GGUF");
        return;
    };
    let port = free_port();
    let key = format!("test-{}", uuid::Uuid::new_v4().simple());
    let mut guard = start_server(Path::new(&bin), Path::new(&model), port, &key);
    wait_healthy(&mut guard, port).await;
    let client = OpenAiClient::new(ClientConfig::llama_server(port, key.clone())).unwrap();

    // Context and model list.
    assert_eq!(client.context_length("x").await.unwrap(), Some(CONTEXT));
    let models = client.list_models().await.unwrap();
    assert_eq!(models.len(), 1, "{models:?}");
    assert_eq!(models[0].context_length, Some(CONTEXT));

    // (a) Plain chat: content and timings.
    let mut plain = no_thinking(ChatRequest::new(
        "x",
        vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Say hello in five words."),
        ],
    ));
    plain.max_tokens = Some(48);
    let events = collect(
        client
            .stream_chat(&plain, CancellationToken::new())
            .await
            .unwrap(),
    )
    .await;
    let content: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Content(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!content.trim().is_empty(), "no content in {events:?}");
    let timings = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Timings(t) => Some(*t),
            _ => None,
        })
        .expect("no timings event");
    assert!(timings.predicted_n.unwrap_or(0) > 0, "{timings:?}");
    assert!(
        timings.predicted_per_second.unwrap_or(0.0) > 0.0,
        "{timings:?}"
    );
    assert!(events.iter().any(|e| matches!(e, StreamEvent::Usage(_))));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::Finished { .. }))
    );
    assert_eq!(events.last(), Some(&StreamEvent::Done));
    eprintln!(
        "plain chat: {content:?}, {:.1} tokens/s",
        timings.predicted_per_second.unwrap_or(0.0)
    );

    // (a2) Thinking on: the server streams reasoning separately from the answer (the
    // `reasoning_content` field this reader maps to Reasoning events).
    let mut thinking = ChatRequest::new("x", vec![ChatMessage::user("What is 2 + 3?")]);
    thinking.set_chat_template_kwarg("enable_thinking", json!(true));
    thinking.max_tokens = Some(48);
    let events = collect(
        client
            .stream_chat(&thinking, CancellationToken::new())
            .await
            .unwrap(),
    )
    .await;
    let reasoning: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::Reasoning(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(!reasoning.trim().is_empty(), "no reasoning in {events:?}");
    assert!(
        !reasoning.contains("<think>"),
        "reasoning markup leaked: {reasoning:?}"
    );
    assert_eq!(events.last(), Some(&StreamEvent::Done));

    // (b) One tool: one complete call with valid JSON arguments.
    let request = weather_request();
    let events = collect(
        client
            .stream_chat(&request, CancellationToken::new())
            .await
            .unwrap(),
    )
    .await;
    let mut accumulator = ToolCallAccumulator::new();
    events.iter().for_each(|event| accumulator.push(event));
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1, "{events:?}");
    assert_eq!(calls[0].call.function.name, "get_weather");
    assert_eq!(calls[0].status, ArgumentsStatus::Object, "{:?}", calls[0]);
    let arguments = calls[0].arguments_object().unwrap();
    assert!(arguments["city"].is_string(), "{arguments:?}");
    assert!(!calls[0].call.id.is_empty());
    let usage: Usage = events
        .iter()
        .find_map(|e| match e {
            StreamEvent::Usage(usage) => Some(*usage),
            _ => None,
        })
        .expect("no usage event");
    eprintln!("tool call: {:?}, usage {usage:?}", calls[0].call);

    // (c) Exact count of the same request equals what the server reported using.
    let counted = client.count_tokens(&request).await.unwrap();
    assert!(counted.is_some_and(|n| n > 0), "{counted:?}");
    assert_eq!(counted, Some(usage.prompt_tokens));

    // (d) A prompt over the context is a typed overflow.
    let long = no_thinking(ChatRequest::new(
        "x",
        vec![ChatMessage::user("hello ".repeat(3000))],
    ));
    let err = client
        .stream_chat(&long, CancellationToken::new())
        .await
        .unwrap_err();
    match err {
        ProviderError::ContextOverflow {
            n_prompt_tokens,
            n_ctx,
        } => {
            assert_eq!(n_ctx, Some(CONTEXT));
            assert!(
                n_prompt_tokens.is_some_and(|n| n > CONTEXT),
                "{n_prompt_tokens:?}"
            );
        }
        other => panic!("expected ContextOverflow, got {other:?}"),
    }

    // (e) Cancelling mid-stream returns promptly, and the server stops generating: with one
    // slot, the next request would otherwise wait for the long answer to finish.
    let mut counting = no_thinking(ChatRequest::new(
        "x",
        vec![ChatMessage::user(
            "Count from 1 to 1000, one number per line, with no other text.",
        )],
    ));
    counting.max_tokens = Some(1500);
    let cancel = CancellationToken::new();
    let mut stream = client.stream_chat(&counting, cancel.clone()).await.unwrap();
    loop {
        match stream.next().await {
            Some(Ok(StreamEvent::Content(_))) => break,
            Some(Ok(_)) => {}
            other => panic!("stream ended before any content: {other:?}"),
        }
    }
    let started = Instant::now();
    cancel.cancel();
    let next = tokio::time::timeout(Duration::from_secs(1), stream.next())
        .await
        .expect("cancellation took longer than 1 s");
    assert_eq!(next, Some(Err(ProviderError::Cancelled)));
    assert_eq!(stream.next().await, None);
    let cancel_latency = started.elapsed();
    assert!(cancel_latency < Duration::from_secs(1));
    drop(stream);

    let mut short = no_thinking(ChatRequest::new("x", vec![ChatMessage::user("Hi")]));
    short.max_tokens = Some(4);
    let follow_up = Instant::now();
    let events = tokio::time::timeout(
        Duration::from_secs(15),
        collect(
            client
                .stream_chat(&short, CancellationToken::new())
                .await
                .unwrap(),
        ),
    )
    .await
    .expect("the server kept generating after the client cancelled");
    assert_eq!(events.last(), Some(&StreamEvent::Done));
    eprintln!(
        "cancel returned in {cancel_latency:?}; follow-up request took {:?}",
        follow_up.elapsed()
    );

    // A wrong key is reported as such.
    let mut wrong = ClientConfig::llama_server(port, "wrong-key");
    wrong.kind = ProviderKind::LlamaServer;
    let wrong = OpenAiClient::new(wrong).unwrap();
    assert_eq!(
        wrong
            .stream_chat(&short, CancellationToken::new())
            .await
            .unwrap_err(),
        ProviderError::Unauthorized
    );
    assert_eq!(
        wrong.context_length("x").await.unwrap_err(),
        ProviderError::Unauthorized
    );

    drop(guard);
}
