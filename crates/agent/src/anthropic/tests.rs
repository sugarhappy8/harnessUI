//! Request translation (golden JSON) and event-stream reading, with streams shaped like the
//! examples in Anthropic's streaming documentation.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::request::messages_body;
use super::{
    MessagesDecoder, ReplayKey, ThinkingReplay, events_from_message, guess, parse_model,
    parse_version,
};
use crate::cloud::{Fix, ModelFacts, ParamFixes, ThinkingModes};
use crate::error::ProviderError;
use crate::stream::{ArgumentsStatus, FinishReason, StreamEvent, ToolCallAccumulator, Usage};
use crate::types::{
    ChatMessage, ChatRequest, ContentPart, ImageUrl, ToolCall, ToolChoice, ToolSpec,
};

const PNG: &str = "iVBORw0KGgo=";

fn weather_tool() -> ToolSpec {
    ToolSpec::function(
        "get_weather",
        "Get the weather for a city",
        json!({"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}),
    )
}

fn adaptive_facts() -> ModelFacts {
    ModelFacts {
        context: Some(1_000_000),
        max_output: Some(128_000),
        thinking: Some(ThinkingModes {
            adaptive: true,
            enabled: false,
            disabled: true,
            between_tools: false,
        }),
    }
}

fn body(req: &ChatRequest, facts: &ModelFacts, replay: Option<&ThinkingReplay>) -> Value {
    messages_body(req, facts, replay, &ParamFixes::default())
}

/// A one-message request to `model`.
fn simple(model: &str) -> ChatRequest {
    let mut request = ChatRequest::new(model, vec![ChatMessage::user("Hi")]);
    request.max_tokens = Some(2_000);
    request
}

/// A tool-using conversation: an earlier turn, then this turn's call and its failed result.
fn tool_conversation() -> ChatRequest {
    let image = ImageUrl::from_base64("image/png", PNG).unwrap();
    let mut failed = ChatMessage::tool_result(
        "toolu_01",
        vec![
            ContentPart::text("no such city"),
            ContentPart::image(image.clone()),
        ],
    );
    failed.name = Some("get_weather".into());
    failed.is_error = true;
    let mut request = ChatRequest::new(
        "claude-sonnet-4-6",
        vec![
            ChatMessage::system("You are a helpful assistant."),
            ChatMessage::user("Hi"),
            ChatMessage::assistant("Hello."),
            ChatMessage::user(vec![
                ContentPart::text("Weather where this is?"),
                ContentPart::image(image),
            ]),
            ChatMessage::assistant_tool_calls(
                Some("Let me check.".into()),
                vec![ToolCall::function(
                    "toolu_01",
                    "get_weather",
                    r#"{"city":"Atlantis"}"#,
                )],
            ),
            failed,
        ],
    );
    request.tools = vec![weather_tool()];
    request.tool_choice = Some(ToolChoice::Auto);
    request.parallel_tool_calls = Some(false);
    request.max_tokens = Some(500_000);
    request.temperature = Some(0.7);
    request.top_k = Some(20);
    request.thinking = Some(true);
    request
}

#[test]
fn translates_a_tool_using_conversation() {
    let body = body(&tool_conversation(), &adaptive_facts(), None);
    assert_eq!(
        body,
        json!({
            "model": "claude-sonnet-4-6",
            // The app's limit, held to what the model accepts.
            "max_tokens": 128_000,
            // The tools and the system prompt stay cached for an hour; the newest message
            // for five minutes.
            "system": [{"type": "text", "text": "You are a helpful assistant.", "cache_control": {"type": "ephemeral", "ttl": "1h"}}],
            "messages": [
                {"role": "user", "content": [{"type": "text", "text": "Hi"}]},
                {"role": "assistant", "content": [{"type": "text", "text": "Hello."}]},
                {"role": "user", "content": [
                    {"type": "text", "text": "Weather where this is?"},
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": PNG}}
                ]},
                {"role": "assistant", "content": [
                    {"type": "text", "text": "Let me check."},
                    {"type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": {"city": "Atlantis"}}
                ]},
                {"role": "user", "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_01",
                    "content": [
                        {"type": "text", "text": "no such city"},
                        {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": PNG}}
                    ],
                    "is_error": true,
                    "cache_control": {"type": "ephemeral"}
                }]}
            ],
            "tools": [{
                "name": "get_weather",
                "description": "Get the weather for a city",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}
            }],
            "tool_choice": {"type": "auto", "disable_parallel_tool_use": true},
            "thinking": {"type": "adaptive", "display": "summarized"},
            "stream": true
        })
    );
}

#[test]
fn thinking_takes_each_models_own_form() {
    let mut request = ChatRequest::new("claude-haiku-4-5", vec![ChatMessage::user("Hi")]);
    request.max_tokens = Some(8_000);
    let thinking =
        |request: &ChatRequest, facts: &ModelFacts| body(request, facts, None)["thinking"].clone();
    let none = ModelFacts::default();
    request.thinking = Some(true);
    // Before 4.6: a budget of half the output limit, at least 1,024 and below the limit.
    assert_eq!(
        thinking(&request, &none),
        json!({"type": "enabled", "budget_tokens": 4_000, "display": "summarized"})
    );
    request.max_tokens = Some(1_500);
    assert_eq!(
        thinking(&request, &none),
        Value::Null,
        "no room for a budget"
    );
    request.max_tokens = Some(8_000);
    request.thinking = Some(false);
    assert_eq!(thinking(&request, &none), json!({"type": "disabled"}));
    request.thinking = None;
    assert_eq!(thinking(&request, &none), Value::Null);

    // Claude Opus 5.5 cannot stop thinking; Claude Sonnet 5.5 stops with between_tools.
    request.thinking = Some(false);
    request.model = "claude-opus-5-5".into();
    assert_eq!(thinking(&request, &none), Value::Null);
    request.model = "claude-sonnet-5-5".into();
    assert_eq!(thinking(&request, &none), json!({"type": "between_tools"}));
    request.thinking = Some(true);
    assert_eq!(
        thinking(&request, &none),
        json!({"type": "adaptive", "display": "summarized"})
    );

    // The model list wins over the name.
    let listed = ModelFacts {
        thinking: Some(ThinkingModes {
            adaptive: false,
            enabled: true,
            disabled: true,
            between_tools: false,
        }),
        ..ModelFacts::default()
    };
    assert_eq!(thinking(&request, &listed)["type"], json!("enabled"));
    // A model that does not think: off is accepted, on sends nothing.
    request.model = "claude-3-5-haiku-20241022".into();
    assert_eq!(thinking(&request, &none), Value::Null);
}

#[test]
fn sampling_values_are_never_sent() {
    let body = body(&tool_conversation(), &adaptive_facts(), None);
    for key in ["temperature", "top_p", "top_k"] {
        assert!(body.get(key).is_none(), "{key}");
    }
}

#[test]
fn output_limit_follows_the_app_and_the_model() {
    let mut request = ChatRequest::new("claude-opus-4-1-20250805", vec![ChatMessage::user("Hi")]);
    // No limit from the app: the recommended 64,000, within what the model takes (32,000).
    assert_eq!(
        body(&request, &ModelFacts::default(), None)["max_tokens"],
        json!(32_000)
    );
    request.model = "claude-opus-5-5".into();
    assert_eq!(
        body(&request, &ModelFacts::default(), None)["max_tokens"],
        json!(64_000)
    );
    request.max_tokens = Some(2_048);
    assert_eq!(
        body(&request, &ModelFacts::default(), None)["max_tokens"],
        json!(2_048)
    );
    // What the server said the model takes.
    let endpoint = "https://limits.example/v1";
    crate::cloud::remember_fix(endpoint, "claude-opus-5-5", &Fix::Cap(1_000));
    let fixes = crate::cloud::remembered_fixes(endpoint, "claude-opus-5-5");
    assert_eq!(
        messages_body(&request, &ModelFacts::default(), None, &fixes)["max_tokens"],
        json!(1_000)
    );
}

#[test]
fn refused_fields_are_left_out() {
    let endpoint = "https://refused.example/v1";
    crate::cloud::remember_fix(endpoint, "claude-x", &Fix::Drop("thinking".into()));
    crate::cloud::remember_fix(endpoint, "claude-x", &Fix::Drop("tool_choice".into()));
    // Never the fields a request needs.
    crate::cloud::remember_fix(endpoint, "claude-x", &Fix::Drop("max_tokens".into()));
    let fixes = crate::cloud::remembered_fixes(endpoint, "claude-x");
    let mut request = tool_conversation();
    request.model = "claude-x".into();
    let body = messages_body(&request, &adaptive_facts(), None, &fixes);
    assert!(body.get("thinking").is_none());
    assert!(body.get("tool_choice").is_none());
    assert!(body.get("max_tokens").is_some());
    assert!(body.get("tools").is_some());
}

#[test]
fn forced_tool_use_only_where_allowed() {
    let mut request = ChatRequest::new("claude-sonnet-4-6", vec![ChatMessage::user("Go")]);
    request.tools = vec![weather_tool()];
    request.tool_choice = Some(ToolChoice::Required);
    let choice =
        |request: &ChatRequest| body(request, &ModelFacts::default(), None)["tool_choice"].clone();
    assert_eq!(choice(&request), json!({"type": "any"}));
    request.thinking = Some(true);
    assert_eq!(
        choice(&request),
        json!({"type": "auto"}),
        "not with thinking on"
    );
    request.thinking = None;
    request.model = "claude-opus-5-5".into();
    assert_eq!(
        choice(&request),
        json!({"type": "auto"}),
        "Claude Opus 5.5 refuses it"
    );
    request.tool_choice = Some(ToolChoice::None);
    request.parallel_tool_calls = Some(false);
    assert_eq!(choice(&request), json!({"type": "none"}));
    // No tools: no tool_choice.
    request.tools.clear();
    assert!(
        body(&request, &ModelFacts::default(), None)
            .get("tool_choice")
            .is_none()
    );
}

fn call_key(ids: &[&str]) -> ReplayKey {
    ReplayKey::Calls(ids.iter().map(|id| (*id).to_owned()).collect())
}

/// The last assistant turn of a translated body.
fn last_assistant(body: &Value) -> Value {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .rfind(|m| m["role"] == "assistant")
        .unwrap()
        .clone()
}

#[test]
fn unchanged_replies_go_back_with_their_thinking() {
    let thinking = json!({"type": "thinking", "thinking": "Check the city.", "signature": "sig-1"});
    let redacted = json!({"type": "redacted_thinking", "data": "opaque"});
    let mut replay = ThinkingReplay::default();
    replay.store(
        call_key(&["toolu_01"]),
        vec![
            thinking.clone(),
            json!({"type": "text", "text": "Let me check."}),
            redacted.clone(),
            json!({"type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": {"city": "Atlantis"}}),
        ],
    );
    // An earlier turn: a call, and a finished reply kept under its text.
    replay.store(
        call_key(&["toolu_00"]),
        vec![
            thinking.clone(),
            json!({"type": "tool_use", "id": "toolu_00", "name": "get_weather", "input": {}}),
        ],
    );
    replay.store(
        ReplayKey::text("Hello."),
        vec![
            json!({"type": "thinking", "thinking": "Greet.", "signature": "sig-0"}),
            json!({"type": "text", "text": "Hello."}),
        ],
    );
    let mut request = tool_conversation();
    request
        .messages
        .insert(3, ChatMessage::user("And the weather at home?"));
    request.messages.insert(
        4,
        ChatMessage::assistant_tool_calls(
            None,
            vec![ToolCall::function("toolu_00", "get_weather", "{}")],
        ),
    );
    request
        .messages
        .insert(5, ChatMessage::tool_result("toolu_00", "sunny"));
    let sent = body(&request, &adaptive_facts(), Some(&replay));
    let messages = sent["messages"].as_array().unwrap();
    // With thinking on, earlier turns go back as they were written: the model reads its
    // earlier thinking, and the prompt cache stays valid.
    assert_eq!(
        messages[1]["content"],
        json!([{"type": "thinking", "thinking": "Greet.", "signature": "sig-0"}, {"type": "text", "text": "Hello."}])
    );
    assert_eq!(messages[3]["content"][0], thinking, "{sent}");
    // This turn: exactly as received, thinking and order kept.
    let current = last_assistant(&sent);
    assert_eq!(current["content"][0], thinking);
    assert_eq!(current["content"][2], redacted);
    assert_eq!(current["content"].as_array().unwrap().len(), 4);
    // claude-sonnet-4-6 has no preserved thinking: no block_binding.
    assert!(sent["thinking"].get("block_binding").is_none(), "{sent}");

    // With thinking off, only this turn's steps go back with their thinking.
    request.thinking = Some(false);
    let off = body(&request, &adaptive_facts(), Some(&replay));
    let messages = off["messages"].as_array().unwrap();
    assert_eq!(
        messages[1]["content"],
        json!([{"type": "text", "text": "Hello."}])
    );
    assert_eq!(last_assistant(&off)["content"][0], thinking);
    request.thinking = Some(true);

    // A step the app changed (an old call's arguments shortened) goes as the app has it.
    if let Some(calls) = request.messages[4].tool_calls.as_mut() {
        calls[0].function.arguments = r#"{"city":"[shortened]"}"#.into();
    }
    let changed = body(&request, &adaptive_facts(), Some(&replay));
    assert_eq!(
        changed["messages"][3]["content"],
        json!([{"type": "tool_use", "id": "toolu_00", "name": "get_weather", "input": {"city": "[shortened]"}}])
    );
    // So does a reply whose text changed.
    request.messages[2] = ChatMessage::assistant("Hello!");
    let changed = body(&request, &adaptive_facts(), Some(&replay));
    assert_eq!(
        changed["messages"][1]["content"],
        json!([{"type": "text", "text": "Hello!"}])
    );

    // Without a replay store nothing is replayed.
    let plain = body(&request, &adaptive_facts(), None);
    assert_eq!(
        last_assistant(&plain)["content"][0],
        json!({"type": "text", "text": "Let me check."})
    );
}

#[test]
fn preserved_thinking_asks_for_drop_block_when_it_replays() {
    let mut replay = ThinkingReplay::default();
    replay.store(
        call_key(&["toolu_01"]),
        vec![
            json!({"type": "thinking", "thinking": "Check.", "signature": "sig-1"}),
            json!({"type": "text", "text": "Let me check."}),
            json!({"type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": {"city": "Atlantis"}}),
        ],
    );
    let always = ModelFacts {
        thinking: Some(ThinkingModes {
            adaptive: true,
            ..ThinkingModes::default()
        }),
        ..adaptive_facts()
    };
    for model in [
        "claude-opus-5-5",
        "claude-sonnet-5-5",
        "claude-haiku-5-5",
        "claude-fable-5-2",
    ] {
        let mut request = tool_conversation();
        request.model = model.into();
        let sent = body(&request, &always, Some(&replay));
        assert_eq!(
            sent["thinking"]["block_binding"],
            json!({"prefix_mismatch_behavior": "drop_block"}),
            "{model}"
        );
        // Nothing replayed: nothing to bind.
        let sent = body(&request, &always, None);
        assert!(sent["thinking"].get("block_binding").is_none(), "{model}");
        // Thinking "off" on a model that always thinks: low effort, and still the binding.
        request.thinking = Some(false);
        let sent = body(&request, &always, Some(&replay));
        assert_eq!(
            sent["thinking"],
            json!({"type": "adaptive", "block_binding": {"prefix_mismatch_behavior": "drop_block"}}),
            "{model}"
        );
        assert_eq!(sent["output_config"], json!({"effort": "low"}), "{model}");
    }
    // Models before preserved thinking, and a refused setting, go without it.
    let mut request = tool_conversation();
    request.model = "claude-opus-5".into();
    assert!(
        body(&request, &always, Some(&replay))["thinking"]
            .get("block_binding")
            .is_none()
    );
    request.model = "claude-opus-5-5".into();
    let mut fixes = ParamFixes::default();
    fixes.add(&Fix::Drop("block_binding".into()));
    let sent = messages_body(&request, &always, Some(&replay), &fixes);
    assert!(sent["thinking"].get("block_binding").is_none());
    assert_eq!(sent["thinking"]["type"], json!("adaptive"));
}

#[test]
fn thinking_off_on_a_model_that_cannot_stop_asks_for_low_effort() {
    // Claude Opus 5.5 accepts neither `disabled` nor `between_tools`: "off" is low effort,
    // and a small request gets room for a short thought.
    let always = ModelFacts {
        thinking: Some(ThinkingModes {
            adaptive: true,
            ..ThinkingModes::default()
        }),
        ..adaptive_facts()
    };
    let mut request = simple("claude-opus-5-5");
    request.thinking = Some(false);
    request.max_tokens = Some(1_024);
    let sent = body(&request, &always, None);
    assert_eq!(sent["output_config"], json!({"effort": "low"}));
    assert!(sent.get("thinking").is_none(), "{sent}");
    assert_eq!(sent["max_tokens"], json!(8_192));
    // Within the model's own limit.
    let small = ModelFacts {
        max_output: Some(4_096),
        ..always.clone()
    };
    assert_eq!(body(&request, &small, None)["max_tokens"], json!(4_096));
    // A model that can stop thinking stops, with the app's limit.
    let sent = body(&request, &adaptive_facts(), None);
    assert_eq!(sent["thinking"], json!({"type": "disabled"}));
    assert!(sent.get("output_config").is_none());
    assert_eq!(sent["max_tokens"], json!(1_024));
    // Thinking on: no effort setting.
    request.thinking = Some(true);
    assert!(body(&request, &always, None).get("output_config").is_none());
    // A server that refused `output_config` gets none.
    request.thinking = Some(false);
    let mut fixes = ParamFixes::default();
    fixes.add(&Fix::Drop("output_config".into()));
    assert!(
        messages_body(&request, &always, None, &fixes)
            .get("output_config")
            .is_none()
    );
}

#[test]
fn pictures_larger_than_claude_takes_become_a_note() {
    use base64::Engine as _;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);
    // PNG with a 9,000 x 100 header.
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&9_000u32.to_be_bytes());
    png.extend_from_slice(&100u32.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    // JPEG: SOI, an APP0 segment, then a start of frame of 8,001 x 50.
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x4A, 0x46];
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x32, 0x1F, 0x41, 0x03]);
    // GIF of 7,000 x 7,000 is fine.
    let mut gif = b"GIF89a".to_vec();
    gif.extend_from_slice(&7_000u16.to_le_bytes());
    gif.extend_from_slice(&7_000u16.to_le_bytes());
    // WebP (extended): a 12,000 x 10 canvas.
    let mut webp = b"RIFF\0\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0".to_vec();
    webp.extend_from_slice(&(12_000u32 - 1).to_le_bytes()[..3]);
    webp.extend_from_slice(&(10u32 - 1).to_le_bytes()[..3]);
    let picture = |mime: &str, bytes: &[u8]| {
        let mut request = simple("claude-opus-5-5");
        request.messages = vec![ChatMessage::user(vec![
            ContentPart::text("Look"),
            ContentPart::image(ImageUrl::from_base64(mime, &encode(bytes)).unwrap()),
        ])];
        body(&request, &adaptive_facts(), None)["messages"][0]["content"][1].clone()
    };
    for (mime, bytes, size) in [
        ("image/png", &png, "9000 x 100"),
        ("image/jpeg", &jpeg, "8001 x 50"),
        ("image/webp", &webp, "12000 x 10"),
    ] {
        let block = picture(mime, bytes);
        assert_eq!(block["type"], json!("text"), "{mime}");
        assert!(
            block["text"]
                .as_str()
                .unwrap()
                .contains(&format!("at {size} pixels")),
            "{block}"
        );
    }
    assert_eq!(picture("image/gif", &gif)["type"], json!("image"));
    // More than 10 MB of base64.
    let mut request = simple("claude-opus-5-5");
    let big = "A".repeat(10_000_004);
    request.messages = vec![ChatMessage::user(vec![ContentPart::image(ImageUrl {
        url: format!("data:image/png;base64,{big}"),
    })])];
    let block = body(&request, &adaptive_facts(), None)["messages"][0]["content"][0].clone();
    assert!(
        block["text"].as_str().unwrap().contains("10 MB"),
        "{}",
        block["type"]
    );
}

#[test]
fn budget_thinking_needs_its_blocks_back() {
    // Models before 4.6 refuse a thinking request whose tool-use loop lost its thinking
    // block, so thinking is left out of that request.
    let mut request = tool_conversation();
    request.model = "claude-sonnet-4-5".into();
    request.max_tokens = Some(10_000);
    let body = body(&request, &ModelFacts::default(), None);
    assert!(body.get("thinking").is_none(), "{body}");
    let mut replay = ThinkingReplay::default();
    replay.store(
        call_key(&["toolu_01"]),
        vec![
            json!({"type": "thinking", "thinking": "", "signature": "sig"}),
            json!({"type": "text", "text": "Let me check."}),
            json!({"type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": {"city": "Atlantis"}}),
        ],
    );
    let body = super::request::messages_body(
        &request,
        &ModelFacts::default(),
        Some(&replay),
        &ParamFixes::default(),
    );
    assert_eq!(body["thinking"]["type"], json!("enabled"));
}

#[test]
fn without_tools_the_tool_history_is_text() {
    let mut request = tool_conversation();
    request.tools.clear();
    request.tool_choice = None;
    request.parallel_tool_calls = None;
    let body = body(&request, &adaptive_facts(), None);
    let text = body.to_string();
    assert!(
        !text.contains("tool_use") && !text.contains("tool_result"),
        "{text}"
    );
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[3]["content"][1],
        json!({"type": "text", "text": "[Called the tool get_weather with {\"city\":\"Atlantis\"}]"})
    );
    assert_eq!(
        messages[4]["content"][0],
        json!({"type": "text", "text": "[Result of the tool get_weather]"})
    );
}

#[test]
fn images_and_ids_the_api_accepts() {
    let jpg = ImageUrl::from_base64("image/jpg", "AAAA").unwrap();
    let bmp = ImageUrl::from_base64("image/bmp", "AAAA").unwrap();
    let mut request = ChatRequest::new(
        "claude-opus-5-5",
        vec![
            ChatMessage::user(vec![
                ContentPart::image(jpg),
                ContentPart::image(bmp),
                ContentPart::text(""),
            ]),
            ChatMessage::assistant_tool_calls(
                None,
                vec![ToolCall::function("call.1:x", "f", "not json")],
            ),
            ChatMessage::tool_result("call.1:x", ""),
        ],
    );
    request.tools = vec![ToolSpec::function("f", "", json!({"properties": {}}))];
    let body = body(&request, &ModelFacts::default(), None);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(
        messages[0]["content"][0]["source"]["media_type"],
        json!("image/jpeg")
    );
    assert_eq!(messages[0]["content"][1]["type"], json!("text"));
    assert_eq!(
        messages[0]["content"].as_array().unwrap().len(),
        2,
        "empty text is left out"
    );
    assert_eq!(messages[1]["content"][0]["id"], json!("call_1_x"));
    assert_eq!(messages[1]["content"][0]["input"], json!({}));
    assert_eq!(messages[2]["content"][0]["tool_use_id"], json!("call_1_x"));
    assert_eq!(
        messages[2]["content"][0]["content"],
        json!([{"type": "text", "text": "(no output)"}])
    );
    assert_eq!(
        body["tools"][0]["input_schema"],
        json!({"type": "object", "properties": {}})
    );
    assert!(body["tools"][0].get("description").is_none());
}

#[test]
fn conversations_start_with_a_user_turn_and_join_same_role_turns() {
    let request = ChatRequest::new(
        "claude-opus-5-5",
        vec![
            ChatMessage::assistant("Earlier answer."),
            ChatMessage::assistant_tool_calls(None, vec![ToolCall::function("a", "f", "{}")]),
        ],
    );
    let body = body(&request, &ModelFacts::default(), None);
    let messages = body["messages"].as_array().unwrap();
    assert_eq!(messages[0]["role"], json!("user"));
    assert_eq!(messages[1]["role"], json!("assistant"));
    assert_eq!(messages.len(), 2, "two assistant messages become one turn");
    assert!(body.get("system").is_none());
}

#[test]
fn model_names() {
    use super::Line;
    assert_eq!(
        parse_version("claude-opus-4-5-20251101"),
        Some((Line::Opus, 4, 5))
    );
    assert_eq!(
        parse_version("claude-opus-4-20250514"),
        Some((Line::Opus, 4, 0))
    );
    assert_eq!(
        parse_version("claude-3-5-haiku-20241022"),
        Some((Line::Haiku, 3, 5))
    );
    assert_eq!(
        parse_version("claude-3-opus-20240229"),
        Some((Line::Opus, 3, 0))
    );
    assert_eq!(parse_version("claude-sonnet-5"), Some((Line::Sonnet, 5, 0)));
    assert_eq!(
        parse_version("claude-fable-5-2"),
        Some((Line::Frontier, 5, 2))
    );
    assert_eq!(
        parse_version("anthropic/claude-haiku-5-5"),
        Some((Line::Haiku, 5, 5))
    );
    assert_eq!(parse_version("gpt-4o"), None);
    assert!(!guess("claude-fable-5-2").thinking.disabled);
    assert!(!guess("claude-opus-5-5").forced_tool_use);
    assert!(guess("claude-haiku-5-5").thinking.disabled);
    assert!(guess("claude-sonnet-4-6").thinking.enabled);
    assert_eq!(guess("claude-sonnet-4-5").max_output, 64_000);
    assert_eq!(guess("claude-3-5-sonnet-latest").max_output, 8_192);
    assert!(!guess("claude-3-5-sonnet-latest").thinking.adaptive);
}

#[test]
fn model_list_entries() {
    // The shape of the models documentation's example, with real numbers.
    let entry = json!({
        "id": "claude-opus-5-5",
        "type": "model",
        "display_name": "Claude Opus 5.5",
        "created_at": "2026-09-21T00:00:00Z",
        "max_input_tokens": 1_000_000,
        "max_tokens": 128_000,
        "capabilities": {
            "image_input": {"supported": true},
            "thinking": {"supported": true, "types": {
                "adaptive": {"supported": true},
                "between_tools": {"supported": false},
                "disabled": {"supported": false},
                "enabled": {"supported": false}
            }}
        }
    });
    let (listed, facts) = parse_model(&entry).unwrap();
    assert_eq!(listed.id, "claude-opus-5-5");
    assert_eq!(listed.display_name.as_deref(), Some("Claude Opus 5.5"));
    assert_eq!(listed.context_length, Some(1_000_000));
    assert_eq!(facts.max_output, Some(128_000));
    assert_eq!(
        facts.thinking,
        Some(ThinkingModes {
            adaptive: true,
            enabled: false,
            disabled: false,
            between_tools: false
        })
    );
    // 0 means "not known".
    let (listed, facts) =
        parse_model(&json!({"id": "x", "max_input_tokens": 0, "max_tokens": 0})).unwrap();
    assert_eq!(
        (listed.context_length, facts.max_output, facts.thinking),
        (None, None, None)
    );
    assert!(parse_model(&json!({"id": ""})).is_none());
}

// ---- the event stream ------------------------------------------------------------------------

fn decode(
    body: &str,
    replay: Option<Arc<Mutex<ThinkingReplay>>>,
) -> (Vec<StreamEvent>, Option<ProviderError>) {
    let mut decoder = MessagesDecoder::new(replay);
    let mut out = Vec::new();
    // Byte by byte: events split anywhere still decode.
    for byte in body.as_bytes() {
        if let Err(err) = decoder.push(std::slice::from_ref(byte), &mut out) {
            return (out, Some(err));
        }
    }
    match decoder.finish(&mut out) {
        Ok(()) => (out, None),
        Err(err) => (out, Some(err)),
    }
}

/// The text example of the streaming documentation.
const TEXT_STREAM: &str = r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_1nZdL29xx5MUA1yADyHTEsnR8uuvGzszyY", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 25, "output_tokens": 1}}}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}

event: ping
data: {"type": "ping"}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "Hello"}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "!"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "end_turn", "stop_sequence":null}, "usage": {"output_tokens": 15}}

event: message_stop
data: {"type": "message_stop"}

"#;

#[test]
fn reads_a_text_stream() {
    let (events, err) = decode(TEXT_STREAM, None);
    assert_eq!(err, None);
    assert_eq!(
        events,
        vec![
            StreamEvent::Content("Hello".into()),
            StreamEvent::Content("!".into()),
            StreamEvent::Finished {
                finish_reason: FinishReason::Stop
            },
            StreamEvent::Usage(Usage {
                prompt_tokens: 25,
                completion_tokens: 15,
                cached_tokens: None
            }),
            StreamEvent::Done,
        ]
    );
}

/// The tool-use example of the streaming documentation (input in small JSON pieces), with
/// cache counts.
const TOOL_STREAM: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_014p7gG3wDgGV9EUtLvnow3U","type":"message","role":"assistant","model":"claude-opus-5","stop_sequence":null,"usage":{"input_tokens":472,"cache_creation_input_tokens":100,"cache_read_input_tokens":2000,"output_tokens":2},"content":[],"stop_reason":null}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Okay, let's check the weather for San Francisco, CA:"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_01T1x1fJ34qAmk2tNTrN7Up6","name":"get_weather","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"location\":"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" \"San"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" Francisc"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"o,"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" CA\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":89}}

event: message_stop
data: {"type":"message_stop"}
"#;

#[test]
fn reads_a_tool_call_in_pieces() {
    let replay = Arc::new(Mutex::new(ThinkingReplay::default()));
    let (events, err) = decode(TOOL_STREAM, Some(replay.clone()));
    assert_eq!(err, None);
    assert_eq!(
        events[1],
        StreamEvent::ToolCallStarted {
            index: 1,
            id: "toolu_01T1x1fJ34qAmk2tNTrN7Up6".into(),
            name: "get_weather".into()
        }
    );
    let mut accumulator = ToolCallAccumulator::new();
    events.iter().for_each(|event| accumulator.push(event));
    let calls = accumulator.finish();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].call.function.arguments,
        r#"{"location": "San Francisco, CA"}"#
    );
    assert_eq!(calls[0].status, ArgumentsStatus::Object);
    assert_eq!(accumulator.finish_reason(), Some(&FinishReason::ToolCalls));
    // The whole prompt counts what was cached; the cached part is reported apart.
    assert!(events.contains(&StreamEvent::Usage(Usage {
        prompt_tokens: 2_572,
        completion_tokens: 89,
        cached_tokens: Some(2_000)
    })));
    assert_eq!(events.last(), Some(&StreamEvent::Done));
    // No thinking: nothing to send back.
    assert!(
        replay
            .lock()
            .unwrap()
            .find(&call_key(&["toolu_01T1x1fJ34qAmk2tNTrN7Up6"]))
            .is_none()
    );
}

/// Thinking with its signature, then a tool call.
const THINKING_TOOL_STREAM: &str = r#"event: message_start
data: {"type": "message_start", "message": {"id": "msg_01", "type": "message", "role": "assistant", "content": [], "model": "claude-opus-5-5", "stop_reason": null, "stop_sequence": null, "usage": {"input_tokens": 50, "output_tokens": 1}}}

event: content_block_start
data: {"type": "content_block_start", "index": 0, "content_block": {"type": "thinking", "thinking": "", "signature": ""}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": "I need the weather."}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "thinking_delta", "thinking": " Use the tool."}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 0, "delta": {"type": "signature_delta", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 0}

event: content_block_start
data: {"type": "content_block_start", "index": 1, "content_block": {"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpPkNRj2YfWXGmKDxH4mPnZ5sQ7vB9URj2pLmN3kF8/dW5hR7xJ0aP1oLs9yTcMnKVf2wRpEGjH9XZaBt4UvDcPrQ"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 1}

event: content_block_start
data: {"type": "content_block_start", "index": 2, "content_block": {"type": "tool_use", "id": "toolu_02", "name": "get_weather", "input": {}}}

event: content_block_delta
data: {"type": "content_block_delta", "index": 2, "delta": {"type": "input_json_delta", "partial_json": "{\"city\": \"Paris\"}"}}

event: content_block_stop
data: {"type": "content_block_stop", "index": 2}

event: message_delta
data: {"type": "message_delta", "delta": {"stop_reason": "tool_use", "stop_sequence": null}, "usage": {"output_tokens": 40}}

event: message_stop
data: {"type": "message_stop"}

"#;

#[test]
fn thinking_streams_as_reasoning_and_is_kept_with_its_signature() {
    let replay = Arc::new(Mutex::new(ThinkingReplay::default()));
    let (events, err) = decode(THINKING_TOOL_STREAM, Some(replay.clone()));
    assert_eq!(err, None);
    assert_eq!(
        events[0],
        StreamEvent::Reasoning("I need the weather.".into())
    );
    assert_eq!(events[1], StreamEvent::Reasoning(" Use the tool.".into()));
    let kept = replay
        .lock()
        .unwrap()
        .find(&call_key(&["toolu_02"]))
        .cloned()
        .unwrap();
    assert_eq!(
        kept,
        vec![
            json!({"type": "thinking", "thinking": "I need the weather. Use the tool.", "signature": "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds"}),
            json!({"type": "redacted_thinking", "data": "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpPkNRj2YfWXGmKDxH4mPnZ5sQ7vB9URj2pLmN3kF8/dW5hR7xJ0aP1oLs9yTcMnKVf2wRpEGjH9XZaBt4UvDcPrQ"}),
            json!({"type": "tool_use", "id": "toolu_02", "name": "get_weather", "input": {"city": "Paris"}}),
        ]
    );
}

#[test]
fn a_finished_reply_that_thought_is_kept_under_its_text() {
    let stream = [
        r#"{"type":"message_start","message":{"usage":{"input_tokens":5}}}"#,
        r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Greet."}}"#,
        r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-0"}}"#,
        r#"{"type":"content_block_stop","index":0}"#,
        r#"{"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Hel"}}"#,
        r#"{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"lo."}}"#,
        r#"{"type":"content_block_stop","index":1}"#,
        r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":9}}"#,
        r#"{"type":"message_stop"}"#,
    ]
    .iter()
    .map(|data| format!("event: x\ndata: {data}\n\n"))
    .collect::<String>();
    let replay = Arc::new(Mutex::new(ThinkingReplay::default()));
    let (_, err) = decode(&stream, Some(replay.clone()));
    assert_eq!(err, None);
    assert_eq!(
        replay
            .lock()
            .unwrap()
            .find(&ReplayKey::text("Hello."))
            .cloned(),
        Some(vec![
            json!({"type": "thinking", "thinking": "Greet.", "signature": "sig-0"}),
            json!({"type": "text", "text": "Hello."}),
        ])
    );
    // A reply cut by the output limit is not kept.
    let cut = stream.replace("end_turn", "max_tokens");
    let replay = Arc::new(Mutex::new(ThinkingReplay::default()));
    decode(&cut, Some(replay.clone()));
    assert!(
        replay
            .lock()
            .unwrap()
            .find(&ReplayKey::text("Hello."))
            .is_none()
    );
}

#[test]
fn a_cut_stream_is_unfinished_and_not_kept() {
    let replay = Arc::new(Mutex::new(ThinkingReplay::default()));
    let cut = THINKING_TOOL_STREAM
        .split("event: message_delta")
        .next()
        .unwrap();
    let (events, err) = decode(cut, Some(replay.clone()));
    assert_eq!(err, None);
    assert_eq!(
        &events[events.len() - 3..],
        &[
            StreamEvent::Unfinished,
            StreamEvent::Usage(Usage {
                prompt_tokens: 50,
                completion_tokens: 1,
                cached_tokens: None
            }),
            StreamEvent::Done
        ]
    );
    assert!(
        replay
            .lock()
            .unwrap()
            .find(&call_key(&["toolu_02"]))
            .is_none()
    );
}

#[test]
fn errors_and_refusals_end_the_stream() {
    let overloaded = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3}}}\n\n\
        event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
        event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hi\"}}\n\n\
        event: error\ndata: {\"type\": \"error\", \"error\": {\"type\": \"overloaded_error\", \"message\": \"Overloaded\"}}\n\n";
    let (events, err) = decode(overloaded, None);
    assert_eq!(events, vec![StreamEvent::Content("Hi".into())]);
    assert_eq!(
        err,
        Some(ProviderError::Http {
            status: 529,
            message: "Overloaded".into()
        })
    );
    let refused = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"refusal\",\"stop_details\":{\"type\":\"refusal\",\"category\":\"cyber\",\"explanation\":\"This could enable harm.\"}},\"usage\":{\"output_tokens\":5}}\n\n";
    let (_, err) = decode(refused, None);
    assert_eq!(
        err,
        Some(ProviderError::Refused {
            explanation: Some("This could enable harm.".into())
        })
    );
    let plain = "event: error\ndata: upstream connect error\n\n";
    assert!(matches!(
        decode(plain, None).1,
        Some(ProviderError::Http { status: 500, .. })
    ));
    let broken = "event: content_block_delta\ndata: {not json\n\n";
    assert!(matches!(
        decode(broken, None).1,
        Some(ProviderError::Decode(_))
    ));
}

#[test]
fn unknown_events_and_stops_are_read_gracefully() {
    let body = "event: future_event\ndata: {\"type\":\"future_event\",\"x\":1}\n\n\
        event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"server_tool_use\",\"id\":\"srvtoolu_1\",\"name\":\"web_search\",\"input\":{}}}\n\n\
        event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"citations_delta\",\"citation\":{}}}\n\n\
        event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"model_context_window_exceeded\"}}\n\n\
        event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    let (events, err) = decode(body, None);
    assert_eq!(err, None);
    assert_eq!(
        events,
        vec![
            StreamEvent::Finished {
                finish_reason: FinishReason::Length
            },
            StreamEvent::Done
        ]
    );
}

#[test]
fn a_body_without_events_is_not_a_stream() {
    for body in ["", "<!doctype html><p>Sign in</p>", ": keep-alive\n\n"] {
        let (events, err) = decode(body, None);
        assert!(events.is_empty());
        assert!(
            matches!(err, Some(ProviderError::Decode(_))),
            "{body:?}: {err:?}"
        );
    }
}

#[test]
fn a_whole_message_instead_of_a_stream() {
    let body = json!({
        "id": "msg_1", "type": "message", "role": "assistant", "model": "claude-opus-5-5",
        "content": [
            {"type": "thinking", "thinking": "Hmm.", "signature": "s"},
            {"type": "text", "text": "Calling."},
            {"type": "tool_use", "id": "toolu_9", "name": "f", "input": {"a": 1}}
        ],
        "stop_reason": "tool_use",
        "usage": {"input_tokens": 10, "cache_read_input_tokens": 5, "output_tokens": 7}
    });
    let events = events_from_message(body.to_string().as_bytes()).unwrap();
    assert_eq!(
        events,
        vec![
            StreamEvent::Reasoning("Hmm.".into()),
            StreamEvent::Content("Calling.".into()),
            StreamEvent::ToolCallStarted {
                index: 2,
                id: "toolu_9".into(),
                name: "f".into()
            },
            StreamEvent::ToolCallArgs {
                index: 2,
                fragment: r#"{"a":1}"#.into()
            },
            StreamEvent::Finished {
                finish_reason: FinishReason::ToolCalls
            },
            StreamEvent::Usage(Usage {
                prompt_tokens: 15,
                completion_tokens: 7,
                cached_tokens: Some(5)
            }),
            StreamEvent::Done,
        ]
    );
    let error = br#"{"type":"error","error":{"type":"rate_limit_error","message":"slow down"}}"#;
    assert_eq!(
        events_from_message(error),
        Err(ProviderError::Http {
            status: 429,
            message: "slow down".into()
        })
    );
    assert!(matches!(
        events_from_message(br#"{"choices":[]}"#),
        Err(ProviderError::Decode(_))
    ));
}

#[test]
fn the_replay_store_is_bounded_and_exact() {
    let mut replay = ThinkingReplay::default();
    let last = super::MAX_REPLAY_ENTRIES + 4;
    for n in 0..=last {
        replay.store(call_key(&[&format!("c{n}")]), vec![json!({"n": n})]);
    }
    assert!(
        replay.find(&call_key(&["c0"])).is_none(),
        "the oldest are dropped"
    );
    let newest = format!("c{last}");
    let before = format!("c{}", last - 1);
    assert!(replay.find(&call_key(&[&newest])).is_some());
    assert!(
        replay.find(&call_key(&[&newest, &before])).is_none(),
        "ids must match exactly"
    );
    replay.store(ReplayKey::text("Hi"), vec![json!({"text": "Hi"})]);
    assert!(replay.find(&ReplayKey::text("Hi")).is_some());
    assert!(replay.find(&ReplayKey::text("Hi!")).is_none());
    replay.store(ReplayKey::Calls(Vec::new()), vec![json!({})]);
    assert!(replay.find(&ReplayKey::Calls(Vec::new())).is_none());
    replay.clear();
    assert!(replay.find(&call_key(&[&newest])).is_none());
    // One store for each endpoint and model, shared by the requests of a run.
    let a = super::replay_for("https://api.anthropic.com/v1", "claude-opus-5-5");
    let b = super::replay_for("https://api.anthropic.com/v1", "claude-opus-5-5");
    let c = super::replay_for("https://api.anthropic.com/v1", "claude-sonnet-5-5");
    assert!(Arc::ptr_eq(&a, &b));
    assert!(!Arc::ptr_eq(&a, &c));
}
