//! Contract tests for what the 0.0.6 sharing proxy (`src-tauri/src/share/proxy.rs`) relies
//! on in the pinned llama-server: the Anthropic Messages API (`/v1/messages`,
//! `/v1/messages/count_tokens`), the OpenAI Responses API (`/v1/responses`) and Chat
//! Completions, the fixed model alias as the model id, a comma-separated list of keys in
//! `LLAMA_API_KEY`, and the fields the proxy removes (cached-token counters, `timings`,
//! `system_fingerprint`), so a llama.cpp update that renames or moves one fails here
//! (ARCHITECTURE.md 7.1: every llama.cpp bump passes the contract tests).
//!
//! Like `llama_server.rs`, they read `LLAMA_SERVER_BIN` and `TEST_GGUF` and skip with a note
//! when either is unset. Answers are kept to a few tokens: these check shapes, not quality.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtime::llama::launch::API_KEY_ENV;
use runtime::llama::server::loopback_client_builder;
use runtime::spawn::{SpawnSpec, StdioMode, spawn};
use runtime::{
    Backend, BackendKind, LaunchConfig, MODEL_ALIAS, RunningServer, RuntimeLayout, StartSpec,
    build_args, discover, free_loopback_port, random_api_key, start,
};
use serde_json::{Value, json};
use tokio::sync::mpsc::unbounded_channel;

const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

fn env_paths(test: &str) -> Option<(PathBuf, PathBuf)> {
    let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
    let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
    match (bin, gguf) {
        (Some(bin), Some(gguf)) => Some((PathBuf::from(bin), PathBuf::from(gguf))),
        _ => {
            eprintln!("skipping {test}: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            None
        }
    }
}

fn cpu_backend(bin: &Path) -> Backend {
    let dir = bin
        .parent()
        .expect("LLAMA_SERVER_BIN has a parent directory")
        .to_path_buf();
    let layout = RuntimeLayout {
        installed_dir: None,
        packs_dir: dir.join("no-packs-here"),
        dev_override: Some(dir),
    };
    discover(&layout)
        .into_iter()
        .find(|backend| backend.kind == BackendKind::Cpu)
        .unwrap_or_else(|| panic!("no CPU backend next to {}", bin.display()))
}

fn launch(model: PathBuf) -> LaunchConfig {
    let mut cfg = LaunchConfig::new(
        model,
        2048,
        free_loopback_port().expect("free port"),
        random_api_key(),
    );
    cfg.cache_ram_mib = 128;
    cfg
}

/// The server as the app starts it (the start-up check included).
async fn app_server(bin: &Path, gguf: &Path) -> RunningServer {
    let (tx, _rx) = unbounded_channel();
    let spec = StartSpec {
        backend: cpu_backend(bin),
        launch: launch(gguf.to_path_buf()),
        load_timeout: LOAD_TIMEOUT,
    };
    match start(spec, tx).await {
        Ok(server) => server,
        Err(error) => panic!("start failed: {error}\n{}", error.stderr_tail().join("\n")),
    }
}

fn client() -> reqwest::Client {
    loopback_client_builder().build().expect("client")
}

async fn post(server: &RunningServer, path: &str, body: Value) -> (u16, String) {
    let response = client()
        .post(format!("{}{path}", server.base_url()))
        .header("x-api-key", server.api_key())
        .json(&body)
        .send()
        .await
        .expect("request");
    let status = response.status().as_u16();
    (status, response.text().await.expect("body"))
}

fn json_of(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// The JSON of every `data:` line of an event stream, and the `event:` names.
fn events(text: &str) -> (Vec<String>, Vec<Value>) {
    let names = text
        .lines()
        .filter_map(|l| l.strip_prefix("event: "))
        .map(str::to_owned)
        .collect();
    let data = text
        .lines()
        .filter_map(|l| l.strip_prefix("data: "))
        .filter(|d| *d != "[DONE]")
        .map(json_of)
        .collect();
    (names, data)
}

fn hi() -> Value {
    json!([{"role": "user", "content": "Say hi."}])
}

#[tokio::test(flavor = "multi_thread")]
async fn messages_api_contract() {
    let Some((bin, gguf)) = env_paths("messages_api_contract") else {
        return;
    };
    let server = app_server(&bin, &gguf).await;

    let (status, text) = post(
        &server,
        "/v1/messages",
        json!({"model": "claude-x", "max_tokens": 4, "messages": hi()}),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let answer = json_of(&text);
    assert_eq!(answer["type"], json!("message"));
    assert_eq!(answer["role"], json!("assistant"));
    assert!(answer["content"].is_array(), "{answer}");
    // Whatever model name was asked for, the alias answers (never the file's path).
    assert_eq!(answer["model"], json!(MODEL_ALIAS));
    assert!(!text.contains(&*gguf.to_string_lossy()));
    let usage = &answer["usage"];
    assert!(
        usage["input_tokens"].is_u64() && usage["output_tokens"].is_u64(),
        "{usage}"
    );
    // The counter the proxy folds back into input_tokens. A second identical request
    // reuses the cached prompt, and input_tokens then counts only the rest: this is the
    // leak the proxy closes.
    assert!(usage.get("cache_read_input_tokens").is_some(), "{usage}");
    let first_input = usage["input_tokens"].as_u64().unwrap()
        + usage["cache_read_input_tokens"].as_u64().unwrap();
    let (_, again) = post(
        &server,
        "/v1/messages",
        json!({"model": "x", "max_tokens": 4, "messages": hi()}),
    )
    .await;
    let again = json_of(&again)["usage"].clone();
    assert!(
        again["cache_read_input_tokens"].as_u64().unwrap() > 0,
        "{again}"
    );
    assert_eq!(
        again["input_tokens"].as_u64().unwrap()
            + again["cache_read_input_tokens"].as_u64().unwrap(),
        first_input
    );

    // Streaming: the events the proxy cleans one by one.
    let (status, text) = post(
        &server,
        "/v1/messages",
        json!({"model": "x", "max_tokens": 4, "stream": true, "messages": hi()}),
    )
    .await;
    assert_eq!(status, 200);
    let (names, data) = events(&text);
    assert_eq!(names.first().map(String::as_str), Some("message_start"));
    assert_eq!(names.last().map(String::as_str), Some("message_stop"));
    assert!(
        names.iter().any(|n| n == "content_block_delta"),
        "{names:?}"
    );
    assert!(names.iter().any(|n| n == "message_delta"), "{names:?}");
    assert_eq!(data[0]["message"]["model"], json!(MODEL_ALIAS));
    assert!(
        data[0]["message"]["usage"]
            .get("cache_read_input_tokens")
            .is_some()
    );
    let delta = data
        .iter()
        .find(|d| d["type"] == json!("message_delta"))
        .unwrap();
    assert!(delta["usage"]["output_tokens"].is_u64(), "{delta}");

    // Token counting.
    let (status, text) = post(
        &server,
        "/v1/messages/count_tokens",
        json!({"model": "x", "messages": hi()}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(
        json_of(&text)["input_tokens"].as_u64().unwrap() > 0,
        "{text}"
    );

    // Both key headers work; none is refused in the API's error shape.
    let bearer = client()
        .post(format!("{}/v1/messages", server.base_url()))
        .bearer_auth(server.api_key())
        .json(&json!({"model": "x", "max_tokens": 1, "messages": hi()}))
        .send()
        .await
        .unwrap();
    assert_eq!(bearer.status(), 200);
    let none = client()
        .post(format!("{}/v1/messages", server.base_url()))
        .json(&json!({"model": "x", "max_tokens": 1, "messages": hi()}))
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 401);

    // A request larger than the context: the error the proxy turns into each client's
    // "too long" wording.
    let long = "word ".repeat(3000);
    let (status, text) = post(
        &server,
        "/v1/messages",
        json!({"model": "x", "max_tokens": 1, "messages": [{"role": "user", "content": long}]}),
    )
    .await;
    assert_eq!(status, 400, "{text}");
    let error = &json_of(&text)["error"];
    assert_eq!(error["type"], json!("exceed_context_size_error"), "{error}");
    assert!(error["n_prompt_tokens"].as_u64().unwrap() > 2048);
    assert_eq!(error["n_ctx"], json!(2048));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn responses_api_contract() {
    let Some((bin, gguf)) = env_paths("responses_api_contract") else {
        return;
    };
    let server = app_server(&bin, &gguf).await;

    let (status, text) = post(
        &server,
        "/v1/responses",
        json!({"model": "x", "max_output_tokens": 3, "input": "Count to ten."}),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let answer = json_of(&text);
    assert_eq!(answer["object"], json!("response"));
    assert_eq!(answer["model"], json!(MODEL_ALIAS));
    assert!(answer["output"].is_array());
    // max_output_tokens is honoured (the proxy caps it).
    assert!(
        answer["usage"]["output_tokens"].as_u64().unwrap() <= 3,
        "{answer}"
    );
    assert!(
        answer["usage"]["input_tokens_details"]
            .get("cached_tokens")
            .is_some(),
        "{answer}"
    );

    // A message list with a function tool, streamed, as Codex sends it.
    let body = json!({
        "model": "x", "max_output_tokens": 16, "stream": true,
        "instructions": "Answer briefly.",
        "input": [{"type": "message", "role": "user", "content": [{"type": "input_text", "text": "What is the weather in Paris?"}]}],
        "tools": [{"type": "function", "name": "get_weather", "description": "Weather for a city.",
                   "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}]
    });
    let (status, text) = post(&server, "/v1/responses", body).await;
    assert_eq!(status, 200, "{text}");
    let (names, data) = events(&text);
    assert_eq!(names.first().map(String::as_str), Some("response.created"));
    assert_eq!(names.last().map(String::as_str), Some("response.completed"));
    let done = data.last().unwrap();
    assert_eq!(done["response"]["model"], json!(MODEL_ALIAS));
    assert!(
        done["response"]["usage"]["input_tokens_details"]
            .get("cached_tokens")
            .is_some()
    );
    // llama-server adds its timings at the top of the last event; the proxy removes them.
    assert!(done.get("timings").is_some(), "{done}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn chat_completions_contract() {
    let Some((bin, gguf)) = env_paths("chat_completions_contract") else {
        return;
    };
    let server = app_server(&bin, &gguf).await;
    let (status, text) = post(
        &server,
        "/v1/chat/completions",
        json!({"model": "x", "max_tokens": 3, "messages": hi()}),
    )
    .await;
    assert_eq!(status, 200, "{text}");
    let answer = json_of(&text);
    assert_eq!(answer["model"], json!(MODEL_ALIAS));
    // The fields the proxy removes or zeroes. Before removing `timings` it reads the counts
    // for the audit row from them (a streamed answer without `include_usage` has no usage).
    let timings = &answer["timings"];
    for counter in ["cache_n", "prompt_n", "predicted_n"] {
        assert!(timings[counter].is_u64(), "{counter}: {answer}");
    }
    assert_eq!(
        timings["cache_n"].as_u64().unwrap() + timings["prompt_n"].as_u64().unwrap(),
        answer["usage"]["prompt_tokens"].as_u64().unwrap(),
        "{answer}"
    );
    assert_eq!(
        timings["predicted_n"], answer["usage"]["completion_tokens"],
        "{answer}"
    );
    assert!(answer.get("system_fingerprint").is_some(), "{answer}");
    assert!(
        answer["usage"]["prompt_tokens_details"]
            .get("cached_tokens")
            .is_some(),
        "{answer}"
    );

    // Streamed, the last chunk carries timings, and usage when asked for.
    let (status, text) = post(
        &server,
        "/v1/chat/completions",
        json!({"model": "x", "max_tokens": 3, "stream": true, "stream_options": {"include_usage": true}, "messages": hi()}),
    )
    .await;
    assert_eq!(status, 200);
    assert!(text.trim_end().ends_with("data: [DONE]"));
    let (_, data) = events(&text);
    assert!(data.iter().all(|d| d["model"] == json!(MODEL_ALIAS)));
    assert!(data.iter().any(|d| d["timings"]["predicted_n"].is_u64()));
    assert!(data.iter().any(|d| d["usage"]["prompt_tokens"].is_u64()));
    server.stop().await;
}

/// The alias is the model id everywhere, while `/props` still names the launched file
/// (the app's start-up check compares that path).
#[tokio::test(flavor = "multi_thread")]
async fn the_alias_is_the_model_id() {
    let Some((bin, gguf)) = env_paths("the_alias_is_the_model_id") else {
        return;
    };
    let server = app_server(&bin, &gguf).await;
    let models: Value = client()
        .get(format!("{}/v1/models", server.base_url()))
        .bearer_auth(server.api_key())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let ids: Vec<&str> = models["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, [MODEL_ALIAS]);
    let props: Value = client()
        .get(format!("{}/props", server.base_url()))
        .bearer_auth(server.api_key())
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let launched = std::fs::canonicalize(&gguf).unwrap();
    let reported = std::fs::canonicalize(props["model_path"].as_str().unwrap()).unwrap();
    assert_eq!(reported, launched);
    server.stop().await;
}

/// `LLAMA_API_KEY` takes a comma-separated list, each key valid on its own: a second key
/// could be given to other apps without exposing the app's own (the proxy does not need
/// it, but a llama.cpp update that drops it would change what the private key means).
#[tokio::test(flavor = "multi_thread")]
async fn several_keys_are_each_accepted() {
    let Some((bin, gguf)) = env_paths("several_keys_are_each_accepted") else {
        return;
    };
    let backend = cpu_backend(&bin);
    let cfg = launch(gguf.clone());
    let env = runtime::llama::launch::build_env_on(backend.base_env(), &cfg)
        .set(API_KEY_ENV, "appkey111,sharekey222");
    let spec = SpawnSpec::new(&backend.server, &backend.dir)
        .args(build_args(&cfg))
        .env(env)
        // Managed-child mode exits when stdin closes: keep it open.
        .stdin(StdioMode::Piped)
        .stdout(StdioMode::Null)
        .stderr(StdioMode::Null);
    let mut child = spawn(spec).expect("llama-server starts");
    let _stdin = child.take_stdin();
    let base = format!("http://127.0.0.1:{}", cfg.port);
    let client = client();
    let mut ready = false;
    for _ in 0..600 {
        if let Ok(r) = client.get(format!("{base}/health")).send().await
            && r.status() == 200
        {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(ready, "llama-server did not get ready");
    for (key, expected) in [
        ("appkey111", 200),
        ("sharekey222", 200),
        ("wrong", 401),
        ("appkey111,sharekey222", 401),
    ] {
        let status = client
            .get(format!("{base}/v1/models"))
            .header("x-api-key", key)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, expected, "{key}");
        let status = client
            .get(format!("{base}/v1/models"))
            .bearer_auth(key)
            .send()
            .await
            .unwrap()
            .status();
        assert_eq!(status, expected, "{key}");
    }
    let none = client
        .get(format!("{base}/v1/models"))
        .send()
        .await
        .unwrap();
    assert_eq!(none.status(), 401);
    let _ = child.kill().await;
}
