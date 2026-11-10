//! `OpenAiClient` against a canned local HTTP server.

mod common;

use std::time::{Duration, Instant};

use agent::{
    ArgumentsStatus, ChatMessage, ChatRequest, ChatStream, ClientConfig, ContentPart, FinishReason,
    ImageUrl, ModelInfo, OpenAiClient, ProviderError, ProviderKind, StreamDecoder, StreamEvent,
    ToolCallAccumulator, ToolChoice, ToolSpec,
};
use common::{MockServer, Recorded, Reply, StreamEnd};
use futures_util::StreamExt;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const TOOL_CALL: &[u8] = include_bytes!("fixtures/stream_tool_call.sse");
const OVERFLOW: &[u8] = include_bytes!("fixtures/overflow_error.json");
const INPUT_TOKENS: &[u8] = include_bytes!("fixtures/input_tokens.json");

fn client(server: &MockServer, kind: ProviderKind) -> OpenAiClient {
    let mut config = ClientConfig::new(server.url(), kind);
    config.api_key = Some("test-key".into());
    OpenAiClient::new(config).unwrap()
}

fn weather_request() -> ChatRequest {
    let mut request = ChatRequest::new(
        "x",
        vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("What is the weather in Paris? Use the tool."),
        ],
    );
    request.tools = vec![ToolSpec::function(
        "get_weather",
        "Get the weather for a city",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    )];
    request.tool_choice = Some(ToolChoice::Auto);
    request.parallel_tool_calls = Some(false);
    request.max_tokens = Some(256);
    request.set_chat_template_kwarg("enable_thinking", json!(false));
    request
}

async fn collect(mut stream: ChatStream) -> (Vec<StreamEvent>, Option<ProviderError>) {
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        match item {
            Ok(event) => events.push(event),
            Err(err) => {
                assert!(stream.next().await.is_none(), "nothing may follow an error");
                return (events, Some(err));
            }
        }
    }
    (events, None)
}

fn fixture_events() -> Vec<StreamEvent> {
    let mut decoder = StreamDecoder::new();
    let mut out = Vec::new();
    decoder.push(TOOL_CALL, &mut out).unwrap();
    decoder.finish(&mut out).unwrap();
    out
}

fn route(handler: fn(&Recorded) -> Reply) -> fn(&Recorded) -> Reply {
    handler
}

#[tokio::test]
async fn streams_a_tool_call_sent_in_small_pieces() {
    let server =
        MockServer::start(|_| Reply::sse(TOOL_CALL, 7, Duration::from_millis(1), StreamEnd::Clean))
            .await;
    let client = client(&server, ProviderKind::LlamaServer);
    let stream = client
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(err, None);
    assert_eq!(events, fixture_events());

    let mut accumulator = ToolCallAccumulator::new();
    events.iter().for_each(|event| accumulator.push(event));
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].call.function.name, "get_weather");
    assert_eq!(calls[0].status, ArgumentsStatus::Object);

    let requests = server.requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path(), "/v1/chat/completions");
    assert_eq!(request.header("authorization"), Some("Bearer test-key"));
    assert_eq!(request.header("accept"), Some("text/event-stream"));
    let body = request.json();
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"], json!({"include_usage": true}));
    assert_eq!(body["parallel_tool_calls"], json!(false));
    assert_eq!(body["tool_choice"], json!("auto"));
    assert_eq!(
        body["chat_template_kwargs"],
        json!({"enable_thinking": false})
    );
    assert_eq!(body["tools"][0]["function"]["name"], json!("get_weather"));
    assert_eq!(body["messages"][1]["role"], json!("user"));
}

#[tokio::test]
async fn stream_flag_is_forced_on() {
    let server = MockServer::start(|_| {
        Reply::sse(b"data: [DONE]\n\n", 64, Duration::ZERO, StreamEnd::Clean)
    })
    .await;
    let mut request = weather_request();
    request.stream = false;
    let stream = client(&server, ProviderKind::Generic)
        .stream_chat(&request, CancellationToken::new())
        .await
        .unwrap();
    let (events, _) = collect(stream).await;
    assert_eq!(events, vec![StreamEvent::Done]);
    let body = server.requests()[0].json();
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["stream_options"]["include_usage"], json!(true));
}

#[tokio::test]
async fn overflow_status_is_mapped_before_streaming() {
    let server = MockServer::start(|_| Reply::raw_json(400, OVERFLOW)).await;
    let err = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(
        err,
        ProviderError::ContextOverflow {
            n_prompt_tokens: Some(5009),
            n_ctx: Some(4096)
        }
    );
}

#[tokio::test]
async fn unauthorized_and_parse_errors() {
    let server = MockServer::start(|request| {
        if request.header("authorization") == Some("Bearer test-key") {
            Reply::json(
                500,
                &json!({"error": {"code": 500, "message": "The model produced output that does not match the expected peg-native format", "type": "server_error"}}),
            )
        } else {
            Reply::json(
                401,
                &json!({"error": {"code": 401, "message": "Invalid API Key", "type": "authentication_error"}}),
            )
        }
    })
    .await;
    let err = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ServerParseError(ref m) if m.contains("peg-native")));

    let mut config = ClientConfig::new(server.url(), ProviderKind::LlamaServer);
    config.api_key = Some("wrong".into());
    let wrong = OpenAiClient::new(config).unwrap();
    let err = wrong
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Unauthorized);
    assert_eq!(
        wrong.list_models().await.unwrap_err(),
        ProviderError::Unauthorized
    );
}

#[tokio::test]
async fn mid_stream_error_ends_the_stream() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hi\"}}]}\n\n",
        "data: {\"error\":{\"code\":500,\"message\":\"The model produced output that does not match the expected peg-native format\",\"type\":\"server_error\"}}\n\n",
    );
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 1024, Duration::ZERO, StreamEnd::Clean)
    })
    .await;
    let stream = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(events, vec![StreamEvent::Content("Hi".into())]);
    assert!(matches!(err, Some(ProviderError::ServerParseError(_))));
}

#[tokio::test]
async fn a_stream_without_done_ends_with_done() {
    let body = concat!(
        ":\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    );
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 5, Duration::ZERO, StreamEnd::Clean)
    })
    .await;
    let stream = client(&server, ProviderKind::Ollama)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(err, None);
    assert_eq!(
        events,
        vec![
            StreamEvent::Content("Hello".into()),
            StreamEvent::Finished {
                finish_reason: FinishReason::Stop
            },
            StreamEvent::Done
        ]
    );
}

#[tokio::test]
async fn a_dropped_connection_is_a_network_error() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n";
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 1024, Duration::ZERO, StreamEnd::Abort)
    })
    .await;
    let stream = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(events, vec![StreamEvent::Content("Hel".into())]);
    assert!(matches!(err, Some(ProviderError::Network(_))), "{err:?}");
}

#[tokio::test]
async fn cancelling_mid_stream_is_prompt_and_closes_the_connection() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one\"}}]}\n\n";
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 1024, Duration::ZERO, StreamEnd::HoldOpen)
    })
    .await;
    let cancel = CancellationToken::new();
    let mut stream = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), cancel.clone())
        .await
        .unwrap();
    assert_eq!(
        stream.next().await,
        Some(Ok(StreamEvent::Content("one".into())))
    );
    // Cancel from another task while the stream is waiting for data that never comes.
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let next = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .expect("stream did not react to cancellation");
    let cancelled_at = Instant::now();
    assert_eq!(next, Some(Err(ProviderError::Cancelled)));
    // 100 ms of that is the canceller's sleep. The bound leaves room for slow CI runners
    // (these tests also run on the Windows and macOS builders) while staying far below the
    // 20 s the server would hold the connection open.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let closed = server
        .wait_closed(Duration::from_secs(5))
        .await
        .expect("the connection stayed open after cancellation");
    assert!(
        closed.saturating_duration_since(cancelled_at) < Duration::from_secs(2),
        "connection closed {:?} after cancel",
        closed.saturating_duration_since(cancelled_at)
    );
    assert_eq!(stream.next().await, None);
}

#[tokio::test]
async fn cancelling_before_the_headers_arrive() {
    let server = MockServer::start(|_| Reply::Silence(Duration::from_secs(10))).await;
    let cancel = CancellationToken::new();
    let canceller = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });
    let started = Instant::now();
    let err = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), cancel)
        .await
        .unwrap_err();
    assert_eq!(err, ProviderError::Cancelled);
    // The server stays silent for 10 s; 100 ms of this is the canceller's sleep.
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn dropping_the_stream_closes_the_connection() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one\"}}]}\n\n";
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 1024, Duration::ZERO, StreamEnd::HoldOpen)
    })
    .await;
    let mut stream = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    assert!(stream.next().await.is_some());
    drop(stream);
    assert!(server.wait_closed(Duration::from_secs(5)).await.is_some());
}

#[tokio::test]
async fn a_silent_server_hits_the_idle_limit() {
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one\"}}]}\n\n";
    let server = MockServer::start(move |_| {
        Reply::sse(body.as_bytes(), 1024, Duration::ZERO, StreamEnd::HoldOpen)
    })
    .await;
    let mut config = ClientConfig::new(server.url(), ProviderKind::Generic);
    config.request_timeout = Some(Duration::from_millis(300));
    let stream = OpenAiClient::new(config)
        .unwrap()
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(events, vec![StreamEvent::Content("one".into())]);
    assert!(matches!(err, Some(ProviderError::Network(_))), "{err:?}");
}

#[tokio::test]
async fn a_json_answer_to_a_streaming_request_is_accepted() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            &json!({
                "object": "chat.completion",
                "choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": "Bonjour"}}],
                "usage": {"prompt_tokens": 9, "completion_tokens": 2, "total_tokens": 11}
            }),
        )
    })
    .await;
    let stream = client(&server, ProviderKind::Generic)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert_eq!(err, None);
    assert_eq!(events[0], StreamEvent::Content("Bonjour".into()));
    assert_eq!(events.last(), Some(&StreamEvent::Done));
}

#[tokio::test]
async fn remote_image_urls_are_refused_before_sending() {
    let server = MockServer::start(|_| Reply::json(200, &json!({}))).await;
    let image = ImageUrl {
        url: "https://example.com/cat.png".into(),
    };
    let request = ChatRequest::new(
        "x",
        vec![ChatMessage::user(vec![ContentPart::image(image)])],
    );
    let client = client(&server, ProviderKind::LlamaServer);
    let err = client
        .stream_chat(&request, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::InvalidRequest(_)));
    assert!(matches!(
        client.count_tokens(&request).await,
        Err(ProviderError::InvalidRequest(_))
    ));
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn list_models_per_kind() {
    let server = MockServer::start(route(|request| match request.path() {
        "/v1/models" => Reply::json(
            200,
            &json!({
                "object": "list",
                "models": [{"name": "legacy"}],
                "data": [
                    {"id": "Qwen3-0.6B-Q8_0.gguf", "object": "model", "max_model_len": 8192,
                     "context_length": 1000,
                     "meta": {"n_ctx": 4096, "n_ctx_train": 40960}},
                    {"object": "model"}
                ]
            }),
        ),
        _ => Reply::not_found(),
    }))
    .await;
    let expect = |context_length| {
        vec![ModelInfo {
            id: "Qwen3-0.6B-Q8_0.gguf".into(),
            context_length,
        }]
    };
    assert_eq!(
        client(&server, ProviderKind::LlamaServer)
            .list_models()
            .await
            .unwrap(),
        expect(Some(4096))
    );
    assert_eq!(
        client(&server, ProviderKind::Vllm)
            .list_models()
            .await
            .unwrap(),
        expect(Some(8192))
    );
    assert_eq!(
        client(&server, ProviderKind::Generic)
            .list_models()
            .await
            .unwrap(),
        expect(Some(1000))
    );
    assert_eq!(
        client(&server, ProviderKind::Ollama)
            .list_models()
            .await
            .unwrap(),
        expect(None)
    );
}

#[tokio::test]
async fn llama_server_context_length_from_props() {
    let server = MockServer::start(route(|request| match request.path() {
        "/props" => Reply::json(
            200,
            &json!({"default_generation_settings": {"n_ctx": 2048, "params": {}}, "total_slots": 1}),
        ),
        _ => Reply::not_found(),
    }))
    .await;
    let mut config = ClientConfig::new(format!("{}/v1", server.url()), ProviderKind::LlamaServer);
    config.api_key = Some("k".into());
    let client = OpenAiClient::new(config).unwrap();
    assert_eq!(client.context_length("any").await.unwrap(), Some(2048));
    assert_eq!(
        server.requests()[0].header("authorization"),
        Some("Bearer k")
    );
}

#[tokio::test]
async fn ollama_context_length_prefers_the_running_model() {
    let server = MockServer::start(route(|request| match (request.method.as_str(), request.path()) {
        ("GET", "/api/ps") => Reply::json(
            200,
            &json!({"models": [{"name": "qwen3:latest", "model": "qwen3:latest", "context_length": 8192}]}),
        ),
        ("POST", "/api/show") => match request.json()["model"].as_str() {
            Some("gemma4") => Reply::json(
                200,
                &json!({"parameters": "stop \"<end>\"\nnum_ctx                        16384", "model_info": {"gemma4.context_length": 131072}}),
            ),
            Some("bare") => Reply::json(
                200,
                &json!({"parameters": "temperature 0.6", "model_info": {"llama.context_length": 131072}}),
            ),
            _ => Reply::json(404, &json!({"error": "model 'missing' not found"})),
        },
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::Ollama);
    assert_eq!(client.context_length("qwen3").await.unwrap(), Some(8192));
    assert_eq!(client.context_length("gemma4").await.unwrap(), Some(16384));
    // The trained maximum in model_info is never used.
    assert_eq!(client.context_length("bare").await.unwrap(), None);
    assert_eq!(client.context_length("missing").await.unwrap(), None);
}

#[tokio::test]
async fn lm_studio_context_length_from_loaded_instances() {
    let server = MockServer::start(route(|request| match request.path() {
        "/api/v1/models" => Reply::json(
            200,
            &json!({"models": [
                {"type": "llm", "key": "qwen/qwen3-4b", "max_context_length": 262144,
                 "loaded_instances": [{"id": "qwen/qwen3-4b", "config": {"context_length": 4096}}]},
                {"type": "llm", "key": "other", "max_context_length": 32768, "loaded_instances": []}
            ]}),
        ),
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::LmStudio);
    assert_eq!(
        client.context_length("qwen/qwen3-4b").await.unwrap(),
        Some(4096)
    );
    assert_eq!(client.context_length("other").await.unwrap(), None);
}

#[tokio::test]
async fn lm_studio_falls_back_to_v0_without_using_the_maximum() {
    let server = MockServer::start(route(|request| match request.path() {
        "/api/v0/models" => Reply::json(
            200,
            &json!({"object": "list", "data": [
                {"id": "a", "state": "loaded", "max_context_length": 32768},
                {"id": "b", "state": "loaded", "max_context_length": 32768, "loaded_context_length": 8192},
                {"id": "c", "state": "not-loaded", "max_context_length": 32768, "loaded_context_length": 8192}
            ]}),
        ),
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::LmStudio);
    assert_eq!(client.context_length("a").await.unwrap(), None);
    assert_eq!(client.context_length("b").await.unwrap(), Some(8192));
    assert_eq!(client.context_length("c").await.unwrap(), None);
}

#[tokio::test]
async fn vllm_context_length_from_the_model_list() {
    let server = MockServer::start(route(|request| match request.path() {
        "/v1/models" => Reply::json(
            200,
            &json!({"data": [{"id": "meta/llama", "max_model_len": 32768}]}),
        ),
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::Vllm);
    assert_eq!(
        client.context_length("meta/llama").await.unwrap(),
        Some(32768)
    );
    assert_eq!(client.context_length("other").await.unwrap(), None);
}

#[tokio::test]
async fn count_tokens_uses_input_tokens_without_stream() {
    let server = MockServer::start(route(|request| match request.path() {
        "/v1/chat/completions/input_tokens" => Reply::raw_json(200, INPUT_TOKENS),
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::LlamaServer);
    assert_eq!(
        client.count_tokens(&weather_request()).await.unwrap(),
        Some(168)
    );
    let body = server.requests()[0].json();
    assert!(body.get("stream").is_none());
    assert!(body.get("stream_options").is_none());
    assert_eq!(body["tools"][0]["function"]["name"], json!("get_weather"));
}

#[tokio::test]
async fn count_tokens_falls_back_to_template_and_tokenize() {
    let server = MockServer::start(route(|request| match request.path() {
        "/apply-template" => Reply::json(200, &json!({"prompt": "<|im_start|>user\nhi<|im_end|>"})),
        "/tokenize" => {
            let body = request.json();
            assert_eq!(body["parse_special"], json!(true));
            Reply::json(200, &json!({"tokens": [1, 2, 3, 4, 5]}))
        }
        _ => Reply::not_found(),
    }))
    .await;
    let client = client(&server, ProviderKind::LlamaServer);
    assert_eq!(
        client.count_tokens(&weather_request()).await.unwrap(),
        Some(5)
    );
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| r.path().to_owned())
        .collect();
    assert_eq!(
        paths,
        [
            "/v1/chat/completions/input_tokens",
            "/apply-template",
            "/tokenize"
        ]
    );
}

#[tokio::test]
async fn count_tokens_is_none_for_other_servers() {
    let server = MockServer::start(|_| Reply::json(200, &json!({"input_tokens": 1}))).await;
    for kind in [
        ProviderKind::Ollama,
        ProviderKind::LmStudio,
        ProviderKind::Vllm,
        ProviderKind::Generic,
    ] {
        assert_eq!(
            client(&server, kind)
                .count_tokens(&weather_request())
                .await
                .unwrap(),
            None
        );
    }
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn connection_refused_is_a_network_error() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let client = OpenAiClient::new(ClientConfig::llama_server(port, "k")).unwrap();
    let err = client
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::Network(_)), "{err:?}");
    assert!(err.user_message().starts_with("Could not reach"));
}

#[tokio::test]
async fn loopback_clients_do_not_follow_redirects() {
    // ARCHITECTURE.md 7.3 and 13.6: the loopback client has redirects disabled, so a local
    // server cannot bounce the request (and its key) somewhere else.
    let server = MockServer::start(route(|request| match request.path() {
        "/v1/chat/completions" | "/v1/models" => Reply::Redirect {
            status: 307,
            location: format!("/elsewhere{}", request.path()),
        },
        _ => Reply::sse(TOOL_CALL, 1024, Duration::ZERO, StreamEnd::Clean),
    }))
    .await;
    let client = OpenAiClient::new(ClientConfig::llama_server(server.addr.port(), "k")).unwrap();
    let err = client
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::Http { status: 307, .. }),
        "{err:?}"
    );
    let err = client.list_models().await.unwrap_err();
    assert!(
        matches!(err, ProviderError::Http { status: 307, .. }),
        "{err:?}"
    );
    let paths: Vec<String> = server
        .requests()
        .iter()
        .map(|r| r.path().to_owned())
        .collect();
    assert_eq!(paths, ["/v1/chat/completions", "/v1/models"]);
}

#[tokio::test]
async fn an_html_page_with_status_200_is_not_an_empty_reply() {
    // A wrong base URL that serves a web page for any path must not look like a model that
    // answered with nothing.
    let server = MockServer::start(|_| Reply::Raw {
        status: 200,
        content_type: "text/html; charset=utf-8".into(),
        body: b"<!doctype html><html><body>Welcome</body></html>".to_vec(),
    })
    .await;
    let stream = client(&server, ProviderKind::Generic)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert!(events.is_empty(), "{events:?}");
    assert!(matches!(err, Some(ProviderError::Decode(_))), "{err:?}");
}

#[tokio::test]
async fn an_empty_200_body_is_not_an_empty_reply() {
    let server = MockServer::start(|_| Reply::sse(b"", 1, Duration::ZERO, StreamEnd::Clean)).await;
    let stream = client(&server, ProviderKind::LlamaServer)
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap();
    let (events, err) = collect(stream).await;
    assert!(events.is_empty(), "{events:?}");
    assert!(matches!(err, Some(ProviderError::Decode(_))), "{err:?}");
}

#[tokio::test]
async fn a_stalled_error_body_hits_the_request_limit() {
    // Headers of an error arrive, then the body stops: the request limit still applies.
    let server = MockServer::start(|_| Reply::Stream {
        status: 400,
        content_type: "application/json".into(),
        chunks: vec![b"{\"error\":".to_vec()],
        delay: Duration::ZERO,
        end: StreamEnd::HoldOpen,
    })
    .await;
    let mut config = ClientConfig::new(server.url(), ProviderKind::Generic);
    config.request_timeout = Some(Duration::from_millis(300));
    let started = Instant::now();
    let err = OpenAiClient::new(config)
        .unwrap()
        .stream_chat(&weather_request(), CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::Network(ref m) if m.contains("error response")),
        "{err:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn ollama_without_api_ps_falls_back_to_show() {
    let server = MockServer::start(route(|request| {
        match (request.method.as_str(), request.path()) {
            ("POST", "/api/show") => match request.json()["model"].as_str() {
                Some("qwen3") => Reply::json(200, &json!({"parameters": "num_ctx 8192"})),
                _ => Reply::not_found(),
            },
            _ => Reply::not_found(),
        }
    }))
    .await;
    let client = client(&server, ProviderKind::Ollama);
    assert_eq!(client.context_length("qwen3").await.unwrap(), Some(8192));
    assert_eq!(client.context_length("other").await.unwrap(), None);
}

#[tokio::test]
async fn context_length_errors_other_than_not_found_are_reported() {
    let server = MockServer::start(|_| {
        Reply::json(
            500,
            &json!({"error": {"code": 500, "message": "boom", "type": "server_error"}}),
        )
    })
    .await;
    for kind in [
        ProviderKind::LlamaServer,
        ProviderKind::Ollama,
        ProviderKind::LmStudio,
        ProviderKind::Vllm,
    ] {
        let err = client(&server, kind).context_length("m").await.unwrap_err();
        assert_eq!(
            err,
            ProviderError::Http {
                status: 500,
                message: "boom".into()
            },
            "{kind:?}"
        );
    }
}

#[tokio::test]
async fn cancelling_a_json_answer_yields_cancelled() {
    let server = MockServer::start(|_| {
        Reply::json(
            200,
            &json!({"choices": [{"index": 0, "finish_reason": "stop", "message": {"role": "assistant", "content": "Bonjour"}}]}),
        )
    })
    .await;
    let cancel = CancellationToken::new();
    let mut stream = client(&server, ProviderKind::Generic)
        .stream_chat(&weather_request(), cancel.clone())
        .await
        .unwrap();
    cancel.cancel();
    assert_eq!(stream.next().await, Some(Err(ProviderError::Cancelled)));
    assert_eq!(stream.next().await, None);
}
