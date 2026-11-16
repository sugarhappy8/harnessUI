//! 0.0.7 P5: cloud endpoints against local fake servers that answer in the documented
//! formats: Claude's Messages API (headers, translated requests, event streams, thinking
//! sent back within a tool-use loop, retries, errors, model list) and OpenAI-compatible
//! servers that refuse parameters they do not know. No API key or network needed.

mod common;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use agent::{
    Approval, ApprovalRequest, Approver, ChatMessage, ChatRequest, ClientConfig, HistoryMessage,
    ModelCaps, OpenAiClient, PolicyGate, ProviderError, ProviderKind, RequestTemplate, Sanitizer,
    StreamEvent, ToolChoice, ToolHost, ToolInfo, ToolSpec, TurnDeps, TurnEvent, TurnInput,
    TurnSink, TurnStatus, run_turn,
};
use async_trait::async_trait;
use common::{MockServer, Recorded, Reply, StreamEnd};
use futures_util::StreamExt;
use policy::{Action, Decision, Risk, Scope};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

const KEY: &str = "sk-ant-test-0123456789abcdef";

// ---- fakes for a whole turn ------------------------------------------------------------------

struct Weather {
    executed: Mutex<Vec<Map<String, Value>>>,
}

fn weather_info() -> ToolInfo {
    ToolInfo {
        name: "get_weather".into(),
        description: "Get the weather for a city.".into(),
        input_schema: json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
        read_only_hint: Some(true),
        destructive_hint: None,
    }
}

#[async_trait]
impl ToolHost for Weather {
    fn tools(&self) -> Vec<ToolInfo> {
        vec![weather_info()]
    }
    fn validate(&self, _: &str, args: &Value) -> Result<(), Vec<String>> {
        if args.get("city").and_then(Value::as_str).is_some() {
            Ok(())
        } else {
            Err(vec!["city is required".into()])
        }
    }
    fn llama_schema(&self, _: &str) -> Option<Value> {
        None
    }
    async fn describe(&self, name: &str, _: &Map<String, Value>) -> mcp_host::Described {
        mcp_host::Described::Action(Action {
            server: "weather".into(),
            tool: name.into(),
            summary: "weather".into(),
            preview: String::new(),
            scope: Scope::FileRead {
                folder: "/p".into(),
            },
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: json!({}),
        })
    }
    async fn execute(
        &self,
        _: &str,
        args: &Map<String, Value>,
        _: &Action,
        _: CancellationToken,
    ) -> mcp_host::ToolOutput {
        self.executed.lock().unwrap().push(args.clone());
        mcp_host::ToolOutput::text("Sunny, 21 °C")
    }
}

struct Allow;

impl PolicyGate for Allow {
    fn decide(&self, _: &str, _: &Action, _: bool, _: bool) -> Decision {
        Decision::AllowedByFullAccess
    }
}

#[async_trait]
impl Approver for Allow {
    async fn ask(&self, _: ApprovalRequest) -> Approval {
        Approval::AllowOnce
    }
}

#[derive(Default)]
struct Events(Mutex<Vec<TurnEvent>>);

impl TurnSink for Events {
    fn event(&self, e: TurnEvent) {
        self.0.lock().unwrap().push(e);
    }
}

async fn run(client: &OpenAiClient, caps: ModelCaps, template: RequestTemplate) -> (agent::TurnOutcome, Weather, Events) {
    let tools = Weather {
        executed: Mutex::new(Vec::new()),
    };
    let events = Events::default();
    let sanitizer = Sanitizer::generic();
    let deps = TurnDeps {
        client,
        tools: &tools,
        policy: &Allow,
        approver: &Allow,
        sink: &events,
        sanitizer: &sanitizer,
    };
    let mut input = TurnInput::new(
        vec![HistoryMessage::user("What is the weather in Paris?")],
        caps,
        template,
    );
    input.system_prompt = "You are a helpful assistant.".into();
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        run_turn(deps, input, CancellationToken::new()),
    )
    .await
    .expect("the turn did not finish");
    (outcome, tools, events)
}

fn caps(thinking: bool) -> ModelCaps {
    ModelCaps {
        tools: true,
        vision: true,
        n_ctx: 200_000,
        local: false,
        thinking,
        preserve_reasoning: false,
        images_in_tool_results: true,
    }
}

// ---- Claude's event streams --------------------------------------------------------------------

fn event(kind: &str, data: &Value) -> String {
    format!("event: {kind}\ndata: {data}\n\n")
}

fn start(input_tokens: u32, cache_read: u32) -> String {
    event(
        "message_start",
        &json!({"type": "message_start", "message": {"id": "msg_1", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "usage": {"input_tokens": input_tokens, "cache_read_input_tokens": cache_read, "output_tokens": 1}}}),
    )
}

fn stop(reason: &str, output_tokens: u32) -> String {
    event(
        "message_delta",
        &json!({"type": "message_delta", "delta": {"stop_reason": reason, "stop_sequence": null}, "usage": {"output_tokens": output_tokens}}),
    ) + &event("message_stop", &json!({"type": "message_stop"}))
}

fn text_stream(text: &str) -> Vec<u8> {
    let mut out = start(1_200, 900);
    out += &event("content_block_start", &json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}));
    out += &event("ping", &json!({"type": "ping"}));
    out += &event("content_block_delta", &json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}}));
    out += &event("content_block_stop", &json!({"type": "content_block_stop", "index": 0}));
    out += &stop("end_turn", 12);
    out.into_bytes()
}

const SIGNATURE: &str = "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds";

fn thinking_tool_stream(id: &str) -> Vec<u8> {
    let mut out = start(1_000, 0);
    out += &event("content_block_start", &json!({"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}));
    out += &event("content_block_delta", &json!({"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "I need the weather."}}));
    out += &event("content_block_delta", &json!({"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": SIGNATURE}}));
    out += &event("content_block_stop", &json!({"type": "content_block_stop", "index": 0}));
    out += &event("content_block_start", &json!({"type": "content_block_start", "index": 1, "content_block": {"type": "tool_use", "id": id, "name": "get_weather", "input": {}}}));
    out += &event("content_block_delta", &json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "{\"city\": "}}));
    out += &event("content_block_delta", &json!({"type": "content_block_delta", "index": 1, "delta": {"type": "input_json_delta", "partial_json": "\"Paris\"}"}}));
    out += &event("content_block_stop", &json!({"type": "content_block_stop", "index": 1}));
    out += &stop("tool_use", 40);
    out.into_bytes()
}

fn sse(body: Vec<u8>) -> Reply {
    Reply::sse(&body, 64, Duration::ZERO, StreamEnd::Clean)
}

fn model_entry(id: &str) -> Value {
    json!({
        "id": id, "type": "model", "display_name": "Claude Opus 5.5",
        "created_at": "2026-09-21T00:00:00Z",
        "max_input_tokens": 1_000_000, "max_tokens": 128_000,
        "capabilities": {"thinking": {"supported": true, "types": {
            "adaptive": {"supported": true}, "enabled": {"supported": false},
            "disabled": {"supported": false}, "between_tools": {"supported": false}
        }}}
    })
}

/// A fake Messages API: `/v1/models/{id}` answers with an entry; `/v1/messages` replies are
/// taken from `replies` in order.
async fn claude_server(replies: Vec<Reply>) -> MockServer {
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(replies)));
    MockServer::start(move |request: &Recorded| {
        let path = request.path();
        if let Some(id) = path.strip_prefix("/v1/models/") {
            return Reply::json(200, &model_entry(id));
        }
        match path {
            "/v1/messages" => queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| sse(text_stream("(no more replies)"))),
            _ => Reply::not_found(),
        }
    })
    .await
}

fn claude(server: &MockServer) -> OpenAiClient {
    let mut config = ClientConfig::new(server.url(), ProviderKind::Anthropic);
    config.api_key = Some(KEY.into());
    OpenAiClient::new(config).unwrap()
}

fn messages_requests(server: &MockServer) -> Vec<Recorded> {
    server
        .requests()
        .into_iter()
        .filter(|r| r.path() == "/v1/messages")
        .collect()
}

async fn collect(client: &OpenAiClient, request: &ChatRequest) -> Result<Vec<StreamEvent>, ProviderError> {
    let mut stream = client.stream_chat(request, CancellationToken::new()).await?;
    let mut events = Vec::new();
    while let Some(item) = stream.next().await {
        events.push(item?);
    }
    Ok(events)
}

fn simple_request(model: &str) -> ChatRequest {
    let mut request = ChatRequest::new(
        model,
        vec![ChatMessage::system("Be brief."), ChatMessage::user("Hi")],
    );
    request.max_tokens = Some(1_000);
    request
}

// ---- Claude ------------------------------------------------------------------------------------

#[tokio::test]
async fn claude_gets_its_own_headers_and_format() {
    let server = claude_server(vec![sse(text_stream("Hello."))]).await;
    let client = claude(&server);
    let mut request = simple_request("claude-opus-5-5");
    request.tools = vec![ToolSpec::function("get_weather", "Weather", json!({"type": "object", "properties": {}}))];
    request.tool_choice = Some(ToolChoice::Auto);
    request.parallel_tool_calls = Some(false);
    request.temperature = Some(0.5);
    let events = collect(&client, &request).await.unwrap();
    assert_eq!(events[0], StreamEvent::Content("Hello.".into()));
    assert_eq!(events.last(), Some(&StreamEvent::Done));

    let sent = &messages_requests(&server)[0];
    assert_eq!(sent.header("x-api-key"), Some(KEY));
    assert_eq!(sent.header("anthropic-version"), Some("2026-10-21"));
    assert_eq!(sent.header("authorization"), None, "the key goes in x-api-key only");
    let body = sent.json();
    assert_eq!(body["model"], json!("claude-opus-5-5"));
    assert_eq!(body["max_tokens"], json!(1_000));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["system"][0]["text"], json!("Be brief."));
    assert_eq!(body["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(body["messages"], json!([{"role": "user", "content": [{"type": "text", "text": "Hi", "cache_control": {"type": "ephemeral"}}]}]));
    assert_eq!(body["tools"][0]["input_schema"], json!({"type": "object", "properties": {}}));
    assert_eq!(body["tool_choice"], json!({"type": "auto", "disable_parallel_tool_use": true}));
    assert!(body.get("temperature").is_none());
    // The model was looked up once, in Anthropic's model list.
    let lookups: Vec<String> = server.requests().iter().filter(|r| r.method == "GET").map(|r| r.path().to_owned()).collect();
    assert_eq!(lookups, vec!["/v1/models/claude-opus-5-5".to_owned()]);
    collect(&client, &request).await.unwrap();
    assert_eq!(server.requests().iter().filter(|r| r.method == "GET").count(), 1);
}

#[tokio::test]
async fn a_tool_using_turn_against_claude() {
    let server = claude_server(vec![
        sse(thinking_tool_stream("toolu_01A")),
        sse(text_stream("It is sunny in Paris, 21 °C.")),
    ])
    .await;
    let client = claude(&server);
    let template = RequestTemplate {
        model: "claude-opus-5-5".into(),
        ..RequestTemplate::default()
    };
    let (outcome, tools, events) = run(&client, caps(true), template).await;
    assert_eq!(outcome.status, TurnStatus::Completed);
    assert_eq!(
        outcome.messages.last().unwrap().text(),
        "It is sunny in Paris, 21 °C."
    );
    assert_eq!(*tools.executed.lock().unwrap(), vec![json!({"city": "Paris"}).as_object().unwrap().clone()]);

    // The thinking streamed into the Thinking area like a local model's.
    let reasoning: String = events
        .0
        .lock()
        .unwrap()
        .iter()
        .filter_map(|e| match e {
            TurnEvent::ReasoningDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(reasoning, "I need the weather.");
    // Token counts include what was read from the cache.
    assert_eq!(outcome.usage.requests, 2);
    assert_eq!(outcome.usage.prompt_tokens, 1_000 + 2_100);
    assert_eq!(outcome.usage.cached_tokens, 900);

    let requests = messages_requests(&server);
    assert_eq!(requests.len(), 2);
    let first = requests[0].json();
    assert_eq!(first["thinking"], json!({"type": "adaptive", "display": "summarized"}));
    let second = requests[1].json();
    let messages = second["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "{second}");
    // The thinking block goes back exactly as received, before the call it led to.
    assert_eq!(
        messages[1],
        json!({"role": "assistant", "content": [
            {"type": "thinking", "thinking": "I need the weather.", "signature": SIGNATURE},
            {"type": "tool_use", "id": "toolu_01A", "name": "get_weather", "input": {"city": "Paris"}}
        ]})
    );
    let result = &messages[2]["content"][0];
    assert_eq!(result["type"], json!("tool_result"));
    assert_eq!(result["tool_use_id"], json!("toolu_01A"));
    assert!(result["content"][0]["text"].as_str().unwrap().contains("Sunny, 21 °C"));
    assert!(result.get("is_error").is_none());
    // The newest block is the cache breakpoint; the system prompt is the other one.
    assert_eq!(result["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(second["system"][0]["cache_control"], json!({"type": "ephemeral"}));
    assert_eq!(first["system"], second["system"], "the cached prefix stays the same");
    assert_eq!(first["tools"], second["tools"]);
}

#[tokio::test]
async fn claude_thinking_off_and_failed_calls() {
    let server = claude_server(vec![
        sse(thinking_tool_stream("toolu_02")),
        sse(text_stream("Done.")),
    ])
    .await;
    let client = claude(&server);
    let template = RequestTemplate {
        model: "claude-haiku-5-5".into(),
        ..RequestTemplate::default()
    };
    let tools = Weather {
        executed: Mutex::new(Vec::new()),
    };
    let events = Events::default();
    let sanitizer = Sanitizer::generic();
    let deps = TurnDeps {
        client: &client,
        tools: &tools,
        policy: &Refuse,
        approver: &Allow,
        sink: &events,
        sanitizer: &sanitizer,
    };
    let mut input = TurnInput::new(vec![HistoryMessage::user("Weather?")], caps(true), template);
    input.thinking = false;
    let outcome = run_turn(deps, input, CancellationToken::new()).await;
    assert_eq!(outcome.status, TurnStatus::Completed);
    let requests = messages_requests(&server);
    // The model list says this model cannot switch thinking off: the parameter is left out.
    assert!(requests[0].json().get("thinking").is_none());
    // A refused call goes back with is_error.
    let second = requests[1].json();
    let result = &second["messages"][2]["content"][0];
    assert_eq!(result["is_error"], json!(true), "{second}");
}

struct Refuse;

impl PolicyGate for Refuse {
    fn decide(&self, _: &str, _: &Action, _: bool, _: bool) -> Decision {
        Decision::Refused {
            reason: "not in this test".into(),
        }
    }
}

fn error(kind: &str, message: &str) -> Value {
    json!({"type": "error", "error": {"type": kind, "message": message}, "request_id": "req_1"})
}

fn with_retry_after(status: u16, body: &Value, after: &str) -> Reply {
    Reply::WithHeaders {
        status,
        headers: vec![("retry-after".into(), after.into())],
        content_type: "application/json".into(),
        body: body.to_string().into_bytes(),
    }
}

#[tokio::test]
async fn claude_rate_limits_and_overload_are_retried_as_documented() {
    // A rate limit with retry-after, then an overload, then the answer.
    let server = claude_server(vec![
        with_retry_after(429, &error("rate_limit_error", "Number of requests exceeded"), "0"),
        with_retry_after(529, &error("overloaded_error", "Overloaded"), "0"),
        sse(text_stream("Finally.")),
    ])
    .await;
    let events = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap();
    assert_eq!(events[0], StreamEvent::Content("Finally.".into()));
    assert_eq!(messages_requests(&server).len(), 3);

    // Overloaded every time: two retries, then the error in plain words.
    let overloaded = || with_retry_after(529, &error("overloaded_error", "Overloaded"), "0");
    let server = claude_server(vec![overloaded(), overloaded(), overloaded(), overloaded()]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    assert!(matches!(err, ProviderError::Http { status: 529, .. }), "{err:?}");
    assert!(err.user_message().starts_with("The company's servers are overloaded"));
    assert_eq!(messages_requests(&server).len(), 3);

    // A spending limit (no retry-after) is not retried; a long wait is said in words.
    let server = claude_server(vec![Reply::json(429, &error("rate_limit_error", "Usage cap reached"))]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    assert!(matches!(err, ProviderError::Http { status: 429, .. }));
    assert_eq!(messages_requests(&server).len(), 1);
    let server = claude_server(vec![with_retry_after(429, &error("rate_limit_error", "Slow down"), "600")]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    assert!(err.to_string().contains("try again in about 10 minutes"), "{err}");
    assert_eq!(messages_requests(&server).len(), 1);
}

#[tokio::test]
async fn claude_bad_keys_and_errors_never_show_the_key() {
    let server = claude_server(vec![Reply::json(401, &error("authentication_error", &format!("invalid x-api-key {KEY}")))]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    assert_eq!(err, ProviderError::Unauthorized);
    // A server that echoes the key in an error message: the key is removed.
    let server = claude_server(vec![Reply::json(400, &error("invalid_request_error", &format!("bad header {KEY}")))]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    let text = format!("{err} {err:?} {}", err.user_message());
    assert!(!text.contains(KEY), "{text}");
    assert!(text.contains("[API key]"), "{text}");
    // Too long for the model: the app's own words, with the numbers.
    let server = claude_server(vec![Reply::json(400, &error("invalid_request_error", "prompt is too long: 210000 tokens > 200000 maximum"))]).await;
    let err = collect(&claude(&server), &simple_request("claude-opus-5-5")).await.unwrap_err();
    assert_eq!(err, ProviderError::ContextOverflow { n_prompt_tokens: Some(210_000), n_ctx: Some(200_000) });
}

#[tokio::test]
async fn claude_refused_thinking_blocks_are_dropped_once() {
    let refusal = error(
        400,
        "invalid_request_error",
        "messages.1.content.0: Invalid `signature` in `thinking` block. The block is bound to a different conversation. Remove the block, or set `thinking.block_binding.prefix_mismatch_behavior` to \"drop_block\".",
    );
    let server = claude_server(vec![
        sse(thinking_tool_stream("toolu_03")),
        Reply::json(400, &refusal),
        sse(text_stream("Answer.")),
    ])
    .await;
    let client = claude(&server);
    let template = RequestTemplate {
        model: "claude-opus-5-5".into(),
        ..RequestTemplate::default()
    };
    let (outcome, _, _) = run(&client, caps(true), template).await;
    assert_eq!(outcome.status, TurnStatus::Completed);
    let requests = messages_requests(&server);
    assert_eq!(requests.len(), 3);
    let replayed = requests[1].json();
    assert_eq!(replayed["messages"][1]["content"][0]["type"], json!("thinking"));
    let retried = requests[2].json();
    assert_eq!(
        retried["messages"][1]["content"],
        json!([{"type": "tool_use", "id": "toolu_03", "name": "get_weather", "input": {"city": "Paris"}}])
    );
}

#[tokio::test]
async fn claude_refused_parameters_are_left_out_and_remembered() {
    let server = {
        let calls = Arc::new(Mutex::new(0));
        MockServer::start(move |request: &Recorded| {
            if request.method == "GET" {
                return Reply::not_found();
            }
            *calls.lock().unwrap() += 1;
            if request.json()["thinking"]["type"] == json!("disabled") {
                return Reply::json(400, &error("invalid_request_error", "\"thinking.type.disabled\" is not supported for this model. Use \"thinking.type.adaptive\" and \"output_config.effort\" to control thinking behavior."));
            }
            sse(text_stream("Fine."))
        })
        .await
    };
    let client = claude(&server);
    let mut request = simple_request("claude-future-9");
    request.thinking = Some(false);
    collect(&client, &request).await.unwrap();
    let posts = messages_requests(&server);
    assert_eq!(posts.len(), 2);
    assert!(posts[1].json().get("thinking").is_none());
    // The next request leaves it out from the start.
    collect(&client, &request).await.unwrap();
    assert_eq!(messages_requests(&server).len(), 3);
}

#[tokio::test]
async fn claude_model_list_with_display_names_and_sizes() {
    let server = MockServer::start(|request: &Recorded| {
        if request.path() != "/v1/models" {
            return Reply::not_found();
        }
        let after = request.target.contains("after_id=claude-sonnet-5-5");
        assert!(request.target.contains("limit=1000"), "{}", request.target);
        if after {
            Reply::json(200, &json!({"data": [model_entry("claude-haiku-5-5")], "has_more": false, "first_id": "claude-haiku-5-5", "last_id": "claude-haiku-5-5"}))
        } else {
            Reply::json(200, &json!({"data": [model_entry("claude-opus-5-5"), model_entry("claude-sonnet-5-5")], "has_more": true, "first_id": "claude-opus-5-5", "last_id": "claude-sonnet-5-5"}))
        }
    })
    .await;
    let client = claude(&server);
    let models = client.list_model_entries().await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["claude-opus-5-5", "claude-sonnet-5-5", "claude-haiku-5-5"]);
    assert_eq!(models[0].display_name.as_deref(), Some("Claude Opus 5.5"));
    assert_eq!(models[0].context_length, Some(1_000_000));
    assert!(server.requests().iter().all(|r| r.header("x-api-key") == Some(KEY)));
    // The size of a listed model is known without asking again.
    let asked = server.requests().len();
    assert_eq!(client.context_length("claude-sonnet-5-5").await.unwrap(), Some(1_000_000));
    assert_eq!(server.requests().len(), asked);
}

// ---- OpenAI-compatible cloud servers ------------------------------------------------------------

/// The fields OpenAI's Chat Completions API takes (a subset is enough for the app's requests).
const STRICT_FIELDS: &[&str] = &[
    "model", "messages", "stream", "stream_options", "max_completion_tokens", "max_tokens",
    "temperature", "top_p", "tools", "tool_choice", "parallel_tool_calls",
];

fn openai_error(message: &str, param: Option<&str>, code: Option<&str>) -> Reply {
    Reply::json(400, &json!({"error": {"message": message, "type": "invalid_request_error", "param": param, "code": code}}))
}

/// A strict OpenAI-style server: unknown fields, `max_tokens`, sampling values on its
/// reasoning model, `parallel_tool_calls` and output limits above 16,384 are refused the way
/// OpenAI refuses them; anything else gets the next scripted reply.
async fn strict_openai(replies: Vec<Reply>) -> MockServer {
    let queue = Arc::new(Mutex::new(std::collections::VecDeque::from(replies)));
    MockServer::start(move |request: &Recorded| {
        if request.path() != "/v1/chat/completions" {
            return Reply::not_found();
        }
        let body = request.json();
        let map = body.as_object().unwrap();
        if let Some(unknown) = map.keys().find(|k| !STRICT_FIELDS.contains(&k.as_str())) {
            return openai_error(&format!("Unrecognized request argument supplied: {unknown}"), None, None);
        }
        if map.contains_key("max_tokens") {
            return openai_error("Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.", Some("max_tokens"), Some("unsupported_parameter"));
        }
        if map.contains_key("temperature") {
            return openai_error("Unsupported value: 'temperature' does not support 0.3 with this model. Only the default (1) value is supported.", Some("temperature"), Some("unsupported_value"));
        }
        if let Some(limit) = map.get("max_completion_tokens").and_then(Value::as_u64)
            && limit > 16_384
        {
            return openai_error(&format!("max_completion_tokens is too large: {limit}. This model supports at most 16384 completion tokens, whereas you provided {limit}."), Some("max_completion_tokens"), Some("invalid_value"));
        }
        if map.contains_key("parallel_tool_calls") {
            return openai_error("Unsupported parameter: 'parallel_tool_calls' is not supported with this model.", Some("parallel_tool_calls"), Some("unsupported_parameter"));
        }
        for message in body["messages"].as_array().unwrap() {
            if message["role"] == "tool" && message.get("name").is_some() {
                return openai_error("Additional properties are not allowed ('name' was unexpected) - 'messages.2'", None, None);
            }
        }
        queue.lock().unwrap().pop_front().unwrap_or_else(Reply::not_found)
    })
    .await
}

#[tokio::test]
async fn a_tool_using_turn_against_a_strict_openai_server() {
    let server = strict_openai(vec![
        common::sse_reply(common::sse::tool_call("call_1", "get_weather", json!({"city": "Paris"}))),
        common::sse_reply(common::sse::text("Sunny in Paris.")),
    ])
    .await;
    let mut config = ClientConfig::new(server.url(), ProviderKind::OpenAi);
    config.api_key = Some(KEY.into());
    let client = OpenAiClient::new(config).unwrap();
    let mut template = RequestTemplate {
        model: "o9-mini".into(),
        temperature: Some(0.3),
        top_k: Some(40),
        ..RequestTemplate::default()
    };
    template.extra.insert("cache_prompt".into(), json!(true));
    template.extra.insert("chat_template_kwargs".into(), json!({"enable_thinking": false}));
    let mut caps = caps(false);
    caps.n_ctx = 128_000;
    caps.images_in_tool_results = false;
    let (outcome, tools, _) = run(&client, caps, template).await;
    assert_eq!(outcome.status, TurnStatus::Completed, "{:?}", server.requests().iter().map(Recorded::json).collect::<Vec<_>>());
    assert_eq!(tools.executed.lock().unwrap().len(), 1);
    let bodies: Vec<Value> = server.requests().iter().map(Recorded::json).collect();
    // The output limit and parallel_tool_calls were refused once each, in the first step;
    // the second step was sent right the first time.
    assert_eq!(bodies.len(), 4, "{bodies:#?}");
    assert_eq!(bodies[0]["max_completion_tokens"], json!(32_768));
    assert_eq!(bodies[1]["max_completion_tokens"], json!(16_384));
    assert!(bodies[2].get("parallel_tool_calls").is_none());
    assert!(bodies[3].get("parallel_tool_calls").is_none());
    assert_eq!(bodies[3]["max_completion_tokens"], json!(16_384));
    for body in &bodies {
        for gone in ["max_tokens", "top_k", "temperature", "cache_prompt", "chat_template_kwargs"] {
            assert!(body.get(gone).is_none(), "{gone} in {body}");
        }
        assert_eq!(body["stream_options"], json!({"include_usage": true}));
    }
    assert!(server.requests().iter().all(|r| r.header("authorization") == Some(&format!("Bearer {KEY}"))));
}

#[tokio::test]
async fn a_server_that_refuses_a_parameter_gets_it_no_more() {
    let server = MockServer::start(|request: &Recorded| {
        if request.json().get("top_k").is_some() {
            return Reply::raw_json(400, b"{\"error\":{\"message\":\"Unrecognized request argument supplied: top_k\",\"type\":\"invalid_request_error\",\"param\":null,\"code\":null}}");
        }
        common::sse_reply(common::sse::text("ok"))
    })
    .await;
    let client = OpenAiClient::new(ClientConfig::new(server.url(), ProviderKind::Generic)).unwrap();
    let mut request = simple_request("some-model");
    request.top_k = Some(20);
    collect(&client, &request).await.unwrap();
    assert_eq!(server.requests().len(), 2);
    collect(&client, &request).await.unwrap();
    assert_eq!(server.requests().len(), 3, "remembered for this endpoint and model");
    // Another model on the same endpoint is asked with top_k again.
    let mut other = simple_request("other-model");
    other.top_k = Some(20);
    collect(&client, &other).await.unwrap();
    assert_eq!(server.requests().len(), 5);
    // llama-server never gets a request changed.
    let llama = OpenAiClient::new(ClientConfig::new(server.url(), ProviderKind::LlamaServer)).unwrap();
    let err = collect(&llama, &request).await.unwrap_err();
    assert!(matches!(err, ProviderError::Http { status: 400, .. }));
}

#[tokio::test]
async fn openai_model_list_has_chat_models_only() {
    let server = MockServer::start(|_| {
        Reply::json(200, &json!({"object": "list", "data": [
            {"id": "gpt-5.5", "object": "model", "owned_by": "openai"},
            {"id": "text-embedding-3-large", "object": "model"},
            {"id": "whisper-1", "object": "model"},
            {"id": "gpt-image-1", "object": "model"},
            {"id": "omni-moderation-latest", "object": "model"},
            {"id": "o4-mini", "object": "model"}
        ]}))
    })
    .await;
    let openai = OpenAiClient::new(ClientConfig::new(server.url(), ProviderKind::OpenAi)).unwrap();
    let ids: Vec<String> = openai.list_models().await.unwrap().into_iter().map(|m| m.id).collect();
    assert_eq!(ids, vec!["gpt-5.5", "o4-mini"]);
    // Another kind lists everything, as before.
    let generic = OpenAiClient::new(ClientConfig::new(server.url(), ProviderKind::Generic)).unwrap();
    assert_eq!(generic.list_models().await.unwrap().len(), 6);
}

#[tokio::test]
async fn openrouter_and_abliteration_lists_give_names_sizes_and_limits() {
    let server = MockServer::start(|request: &Recorded| {
        if request.path() == "/v1/chat/completions" {
            return common::sse_reply(common::sse::text("ok"));
        }
        Reply::json(200, &json!({"data": [
            {"id": "qwen/qwen4-max", "name": "Qwen: Qwen4 Max", "context_length": 1_000_000,
             "architecture": {"output_modalities": ["text"]},
             "top_provider": {"context_length": 1_000_000, "max_completion_tokens": 8_000}},
            {"id": "maker/image-only", "name": "Images", "context_length": 4_096,
             "architecture": {"output_modalities": ["image"]}},
            {"id": "abliterated-model", "display_name": "Abliterated", "context_length": 65_536,
             "max_output_length": 4_096, "output_modalities": ["text"]}
        ]}))
    })
    .await;
    let client = OpenAiClient::new(ClientConfig::new(server.url(), ProviderKind::OpenRouter)).unwrap();
    let models = client.list_model_entries().await.unwrap();
    assert_eq!(models.len(), 2, "a model that cannot write text is left out");
    assert_eq!(models[0].display_name.as_deref(), Some("Qwen: Qwen4 Max"));
    assert_eq!(models[0].context_length, Some(1_000_000));
    assert_eq!(models[1].display_name.as_deref(), Some("Abliterated"));
    // Requests stay within the output limit the list gave.
    let mut request = simple_request("qwen/qwen4-max");
    request.max_tokens = Some(50_000);
    collect(&client, &request).await.unwrap();
    let sent = server.requests().last().unwrap().json();
    assert_eq!(sent["max_tokens"], json!(8_000));
}

#[test]
fn keys_never_travel_in_clear_text_to_the_internet() {
    let with_key = |url: &str| {
        let mut config = ClientConfig::new(url, ProviderKind::Generic);
        config.api_key = Some(KEY.into());
        OpenAiClient::new(config)
    };
    let err = with_key("http://api.example.com/v1").unwrap_err();
    assert!(matches!(err, ProviderError::InvalidRequest(ref m) if m.contains("https://")), "{err:?}");
    assert!(with_key("https://api.example.com/v1").is_ok());
    assert!(with_key("http://192.168.1.20:8000/v1").is_ok());
    assert!(with_key("http://127.0.0.1:11434").is_ok());
    assert!(OpenAiClient::new(ClientConfig::new("http://api.example.com/v1", ProviderKind::Generic)).is_ok());
}
