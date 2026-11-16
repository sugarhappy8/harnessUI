//! A [`ChatRequest`] (Chat Completions shape) in the form of Claude's Messages API.

use serde_json::{Map, Value, json};

use super::{ReplayKey, ThinkingReplay, guess};
use crate::cloud::{ModelFacts, ParamFixes, ThinkingModes};
use crate::history::STOPPED_TEXT;
use crate::types::{ChatMessage, ChatRequest, ContentPart, MessageContent, Role, ToolChoice};

/// The output limit when the request has none: Anthropic's recommendation for streamed
/// requests (64,000), within what the model accepts.
const DEFAULT_MAX_TOKENS: u32 = 64_000;
/// The smallest thinking budget the API accepts (`budget_tokens` ≥ 1,024).
const MIN_THINKING_BUDGET: u32 = 1_024;
/// The image types the Messages API reads.
const IMAGE_TYPES: &[&str] = &["image/jpeg", "image/png", "image/gif", "image/webp"];
/// Top-level fields a request may go without when the API refuses them.
const OPTIONAL_FIELDS: &[&str] = &[
    "thinking",
    "tool_choice",
    "temperature",
    "top_p",
    "top_k",
    "output_config",
];
/// The output limit at least, when the model thinks although the app asked for no thinking
/// (a model whose thinking cannot stop): room for a short thought and the answer, so a
/// small request (a summary) does not come back empty. Anthropic: size `max_tokens` for the
/// thinking as well as the reply.
const ROOM_WITH_THINKING: u32 = 8_192;
/// The largest picture Claude takes: 10 MB of base64 (Anthropic's vision documentation).
const MAX_IMAGE_BASE64: usize = 10_000_000;
/// The longest side of a picture Claude takes, in pixels.
const MAX_IMAGE_SIDE: u32 = 8_000;

/// The Messages API body for `req`, streamed. `facts` is what the model list said about the
/// model (the name decides what it did not say); `replay` holds the thinking blocks of this
/// tool-use loop's earlier steps (`None` sends none); `fixes` is what the API refused earlier
/// for this model.
pub(crate) fn messages_body(
    req: &ChatRequest,
    facts: &ModelFacts,
    replay: Option<&ThinkingReplay>,
    fixes: &ParamFixes,
) -> Value {
    let guessed = guess(&req.model);
    let cap = fixes
        .output_cap()
        .or(facts.max_output)
        .unwrap_or(guessed.max_output)
        .max(1);
    let modes = facts.thinking.unwrap_or(guessed.thinking);
    // A model whose thinking cannot stop thinks a little at `effort: low` when the app wants
    // no thinking (Claude Opus 5.5, Fable, Mythos).
    let cannot_stop = modes.adaptive && !modes.disabled && !modes.between_tools;
    let lowered = req.thinking == Some(false) && cannot_stop;
    let mut max_tokens = req.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS).clamp(1, cap);
    if lowered {
        max_tokens = max_tokens.max(ROOM_WITH_THINKING.min(cap));
    }
    let with_tools = !req.tools.is_empty();
    // Earlier turns' thinking goes back too while the model thinks.
    let replay_earlier = req.thinking == Some(true) || lowered;
    let converted = convert(&req.messages, with_tools, replay, replay_earlier);
    let mut messages = converted.messages;
    mark_newest_block(&mut messages);

    let mut body = Map::new();
    body.insert("model".into(), Value::String(req.model.clone()));
    body.insert("max_tokens".into(), Value::from(max_tokens));
    let system = system_text(&req.messages);
    if !system.trim().is_empty() {
        // The one-hour lifetime: the tools and the system prompt stay cached while a chat
        // waits for an approval or a reply (Anthropic: for gaps of 5 to 60 minutes). Longer
        // lifetimes come first, as required: the newest message's marker is five minutes.
        body.insert(
            "system".into(),
            json!([{"type": "text", "text": system, "cache_control": {"type": "ephemeral", "ttl": "1h"}}]),
        );
    }
    body.insert("messages".into(), Value::Array(messages));
    let mut thinking = thinking_param(req.thinking, modes, max_tokens, converted.unreplayed);
    let thinking_on = thinking
        .as_ref()
        .and_then(|t| t.get("type"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kind == "adaptive" || kind == "enabled");
    // Preserved thinking: a replayed block whose earlier history changed (the app shortened
    // an old tool result, a summary replaced old turns) is dropped instead of failing the
    // request. Only with thinking on (Anthropic refuses it beside `between_tools`); a model
    // that thinks at low effort says `adaptive` for it, which is what it does anyway.
    let binding = json!({"prefix_mismatch_behavior": "drop_block"});
    if converted.replayed_thinking && guessed.preserved_thinking && !fixes.drops("block_binding") {
        match thinking.as_mut() {
            Some(Value::Object(param)) if thinking_on => {
                param.insert("block_binding".into(), binding);
            }
            None if lowered => {
                thinking = Some(json!({"type": "adaptive", "block_binding": binding}));
            }
            _ => {}
        }
    }
    if with_tools {
        body.insert("tools".into(), tools(req));
        if let Some(choice) = tool_choice(req, thinking_on || !guessed.forced_tool_use) {
            body.insert("tool_choice".into(), choice);
        }
    }
    if let Some(thinking) = thinking {
        body.insert("thinking".into(), thinking);
    }
    if lowered {
        body.insert("output_config".into(), json!({"effort": "low"}));
    }
    body.insert("stream".into(), Value::Bool(true));
    for key in OPTIONAL_FIELDS {
        if fixes.drops(key) {
            body.remove(*key);
        }
    }
    Value::Object(body)
}

/// The text of every system message, joined.
fn system_text(messages: &[ChatMessage]) -> String {
    messages
        .iter()
        .filter(|message| message.role == Role::System)
        .filter_map(|message| message.content.as_ref().map(MessageContent::text))
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The `thinking` parameter for what the app wants (`wanted`) and what the model accepts:
/// on, `adaptive` with summarised thinking text, else `enabled` with half the output limit
/// as its budget (models before 4.6, which also need every assistant step of the tool-use
/// loop to start with its thinking block, so the budget form is left out when one was not
/// kept); off, `disabled`, else `between_tools`, else nothing (a model that always thinks).
fn thinking_param(
    wanted: Option<bool>,
    modes: ThinkingModes,
    max_tokens: u32,
    unreplayed: bool,
) -> Option<Value> {
    if wanted? {
        if modes.adaptive {
            return Some(json!({"type": "adaptive", "display": "summarized"}));
        }
        if modes.enabled && max_tokens >= 2 * MIN_THINKING_BUDGET && !unreplayed {
            let budget = (max_tokens / 2)
                .max(MIN_THINKING_BUDGET)
                .min(max_tokens - 1);
            return Some(
                json!({"type": "enabled", "budget_tokens": budget, "display": "summarized"}),
            );
        }
        return None;
    }
    if modes.disabled {
        Some(json!({"type": "disabled"}))
    } else if modes.between_tools {
        Some(json!({"type": "between_tools"}))
    } else {
        None
    }
}

/// The tool definitions: name, description and `input_schema` (an object schema).
fn tools(req: &ChatRequest) -> Value {
    Value::Array(
        req.tools
            .iter()
            .map(|tool| {
                let mut schema = tool.function.parameters.clone();
                match &mut schema {
                    Value::Object(map) => {
                        map.entry("type").or_insert_with(|| json!("object"));
                    }
                    _ => schema = json!({"type": "object", "properties": {}}),
                }
                let mut definition = Map::new();
                definition.insert("name".into(), Value::String(tool.function.name.clone()));
                if let Some(description) = tool
                    .function
                    .description
                    .as_ref()
                    .filter(|d| !d.trim().is_empty())
                {
                    definition.insert("description".into(), Value::String(description.clone()));
                }
                definition.insert("input_schema".into(), schema);
                Value::Object(definition)
            })
            .collect(),
    )
}

/// `tool_choice`: `auto`, `none`, or `any` where forced tool use is allowed (never with
/// thinking on, and never on models that refuse it); one call at a time when the request
/// says so.
fn tool_choice(req: &ChatRequest, no_forcing: bool) -> Option<Value> {
    let choice = match (req.tool_choice, req.parallel_tool_calls) {
        (Some(choice), _) => choice,
        (None, Some(false)) => ToolChoice::Auto,
        (None, _) => return None,
    };
    let mut value = match choice {
        ToolChoice::None => return Some(json!({"type": "none"})),
        ToolChoice::Required if !no_forcing => json!({"type": "any"}),
        ToolChoice::Auto | ToolChoice::Required => json!({"type": "auto"}),
    };
    if req.parallel_tool_calls == Some(false) {
        value["disable_parallel_tool_use"] = Value::Bool(true);
    }
    Some(value)
}

/// The converted conversation.
struct Converted {
    messages: Vec<Value>,
    /// An assistant step of the current tool-use loop goes without the thinking blocks it
    /// was answered with.
    unreplayed: bool,
    /// A kept response with thinking blocks is sent back.
    replayed_thinking: bool,
}

/// The kept blocks of an assistant step, when the app's copy of it is what the model wrote:
/// the same text and the same tool calls (id, name and arguments). A step the app changed
/// (an old call's long arguments shortened) goes as the app has it, without its thinking.
fn kept_blocks(
    message: &ChatMessage,
    with_tools: bool,
    replay: &ThinkingReplay,
) -> Option<Vec<Value>> {
    let calls = message.tool_calls.as_deref().unwrap_or_default();
    let text = message
        .content
        .as_ref()
        .map(MessageContent::text)
        .unwrap_or_default();
    let key = if calls.is_empty() {
        if text.is_empty() {
            return None;
        }
        ReplayKey::text(&text)
    } else {
        if !with_tools {
            return None;
        }
        ReplayKey::Calls(calls.iter().map(|call| call.id.clone()).collect())
    };
    let blocks = replay.find(&key)?;
    let kept_text: String = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect();
    let kept_calls: Vec<&Value> = blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .collect();
    let same_calls = kept_calls.len() == calls.len()
        && kept_calls.iter().zip(calls).all(|(kept, call)| {
            let input = serde_json::from_str::<Value>(&call.function.arguments)
                .ok()
                .filter(Value::is_object)
                .unwrap_or_else(|| json!({}));
            kept.get("id").and_then(Value::as_str) == Some(call.id.as_str())
                && kept.get("name").and_then(Value::as_str) == Some(call.function.name.as_str())
                && kept.get("input") == Some(&input)
        });
    (kept_text == text && same_calls).then(|| blocks.clone())
}

/// The messages after the system message, as Messages API turns. With `replay`, kept
/// responses go back block for block: the steps of the current tool-use loop always, and
/// with `replay_earlier` the earlier turns too.
fn convert(
    chat: &[ChatMessage],
    with_tools: bool,
    replay: Option<&ThinkingReplay>,
    replay_earlier: bool,
) -> Converted {
    let body: Vec<&ChatMessage> = chat.iter().filter(|m| m.role != Role::System).collect();
    // The current tool-use loop starts after the last user message that is not part of
    // answering tool calls (the images message after tool results is part of it).
    let loop_start = body
        .iter()
        .enumerate()
        .rev()
        .find(|(index, message)| {
            message.role == Role::User && (*index == 0 || body[index - 1].role != Role::Tool)
        })
        .map_or(0, |(index, _)| index + 1);
    let mut messages: Vec<Value> = Vec::new();
    let mut unreplayed = false;
    let mut replayed_thinking = false;
    for (index, message) in body.iter().enumerate() {
        match message.role {
            Role::System => {}
            Role::User => push(&mut messages, "user", user_blocks(message)),
            Role::Assistant => {
                let calls = message.tool_calls.as_deref().unwrap_or_default();
                let in_loop = with_tools && index >= loop_start && !calls.is_empty();
                let kept = replay
                    .filter(|_| in_loop || replay_earlier)
                    .and_then(|replay| kept_blocks(message, with_tools, replay));
                let blocks = match kept {
                    Some(blocks) => {
                        replayed_thinking |= blocks.iter().any(|block| {
                            matches!(
                                block.get("type").and_then(Value::as_str),
                                Some("thinking" | "redacted_thinking")
                            )
                        });
                        blocks
                    }
                    None => {
                        unreplayed |= in_loop;
                        assistant_blocks(message, with_tools)
                    }
                };
                push(&mut messages, "assistant", blocks);
            }
            Role::Tool => push(&mut messages, "user", result_blocks(message, with_tools)),
        }
    }
    // The conversation must start with a user turn.
    if messages
        .first()
        .is_some_and(|first| first.get("role").and_then(Value::as_str) != Some("user"))
    {
        messages.insert(
            0,
            json!({"role": "user", "content": [text_block("(continued)")]}),
        );
    }
    Converted {
        messages,
        unreplayed,
        replayed_thinking,
    }
}

/// Appends a turn, joining it to the previous one when the role is the same (tool results
/// and the user text after them become one user turn, results first).
fn push(messages: &mut Vec<Value>, role: &str, mut blocks: Vec<Value>) {
    if blocks.is_empty() {
        blocks.push(text_block(if role == "assistant" {
            STOPPED_TEXT
        } else {
            "."
        }));
    }
    if let Some(last) = messages.last_mut()
        && last.get("role").and_then(Value::as_str) == Some(role)
        && let Some(Value::Array(content)) = last.get_mut("content")
    {
        content.extend(blocks);
        return;
    }
    messages.push(json!({"role": role, "content": blocks}));
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

/// Text and image blocks of a message's content; empty text is left out (the API refuses
/// empty text blocks).
fn content_blocks(content: Option<&MessageContent>) -> Vec<Value> {
    match content {
        None => Vec::new(),
        Some(MessageContent::Text(text)) if text.is_empty() => Vec::new(),
        Some(MessageContent::Text(text)) => vec![text_block(text)],
        Some(MessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } if text.is_empty() => None,
                ContentPart::Text { text } => Some(text_block(text)),
                ContentPart::ImageUrl { image_url } => Some(image_block(&image_url.url)),
            })
            .collect(),
    }
}

/// An image as a base64 `source`, from the inline data URI the app sends. A type the API
/// does not read becomes a note instead.
fn image_block(url: &str) -> Value {
    let parsed = url
        .strip_prefix("data:")
        .and_then(|rest| rest.split_once(','))
        .and_then(|(header, data)| Some((header.strip_suffix(";base64")?, data)));
    let Some((mime, data)) = parsed else {
        return text_block("[image: could not be read]");
    };
    let mime = mime.to_ascii_lowercase();
    let mime = if mime == "image/jpg" {
        "image/jpeg".to_owned()
    } else {
        mime
    };
    if !IMAGE_TYPES.contains(&mime.as_str()) {
        return text_block(&format!(
            "[image of type {mime}: Claude reads only JPEG, PNG, GIF and WebP images]"
        ));
    }
    // A picture Claude refuses would fail every later request of the chat, so it goes as a
    // note (the app accepts attachments up to 20 MB).
    if data.len() > MAX_IMAGE_BASE64 {
        return text_block(
            "[A picture was left out here: it is larger than the 10 MB that Claude takes.]",
        );
    }
    if let Some((width, height)) = picture_size(&head_bytes(data))
        && width.max(height) > MAX_IMAGE_SIDE
    {
        return text_block(&format!(
            "[A picture was left out here: at {width} x {height} pixels it is larger than the \
             8,000 pixels a side that Claude takes.]"
        ));
    }
    json!({"type": "image", "source": {"type": "base64", "media_type": mime, "data": data}})
}

/// The first 64 KiB or so of base64 `data`, decoded (enough for a picture's header).
fn head_bytes(data: &str) -> Vec<u8> {
    use base64::Engine as _;
    let take = data.len().min(87_380) / 4 * 4;
    let head = data.get(..take).unwrap_or_default();
    base64::engine::general_purpose::STANDARD
        .decode(head)
        .unwrap_or_default()
}

/// The width and height a picture's header gives (PNG, GIF, WebP, JPEG); `None` when the
/// header is not in `bytes` or cannot be read.
fn picture_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let be16 = |at: usize| {
        Some(u32::from(u16::from_be_bytes([
            *bytes.get(at)?,
            *bytes.get(at + 1)?,
        ])))
    };
    let le16 = |at: usize| {
        Some(u32::from(u16::from_le_bytes([
            *bytes.get(at)?,
            *bytes.get(at + 1)?,
        ])))
    };
    let le24 = |at: usize| {
        Some(u32::from_le_bytes([
            *bytes.get(at)?,
            *bytes.get(at + 1)?,
            *bytes.get(at + 2)?,
            0,
        ]))
    };
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        let width = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
        let height = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
        return Some((width, height));
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some((le16(6)?, le16(8)?));
    }
    if bytes.get(0..4) == Some(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        return match bytes.get(12..16)? {
            // Lossy: frame tag, start code, then 14-bit sides.
            b"VP8 " => Some((le16(26)? & 0x3FFF, le16(28)? & 0x3FFF)),
            // Lossless: a signature byte, then 14 bits each of side − 1.
            b"VP8L" => {
                let bits = u32::from_le_bytes(bytes.get(21..25)?.try_into().ok()?);
                Some(((bits & 0x3FFF) + 1, ((bits >> 14) & 0x3FFF) + 1))
            }
            // Extended: 24 bits each of the canvas side − 1.
            b"VP8X" => Some((le24(24)? + 1, le24(27)? + 1)),
            _ => None,
        };
    }
    if bytes.starts_with(&[0xFF, 0xD8]) {
        let mut at = 2;
        loop {
            if *bytes.get(at)? != 0xFF {
                return None;
            }
            let marker = *bytes.get(at + 1)?;
            match marker {
                // Fill bytes, and markers without a length.
                0xFF => at += 1,
                0x01 | 0xD0..=0xD8 => at += 2,
                // A start of frame: precision, then height and width.
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                    return Some((be16(at + 7)?, be16(at + 5)?));
                }
                _ => {
                    let length = usize::try_from(be16(at + 2)?).ok()?;
                    if length < 2 {
                        return None;
                    }
                    at += 2 + length;
                }
            }
        }
    }
    None
}

fn user_blocks(message: &ChatMessage) -> Vec<Value> {
    content_blocks(message.content.as_ref())
}

/// An assistant step: its text, then its tool calls (as `tool_use` blocks, or as text when
/// the request offers no tools, since the API refuses tool blocks without tools).
fn assistant_blocks(message: &ChatMessage, with_tools: bool) -> Vec<Value> {
    let mut blocks = content_blocks(message.content.as_ref());
    for call in message.tool_calls.as_deref().unwrap_or_default() {
        let input = match serde_json::from_str::<Value>(&call.function.arguments) {
            Ok(Value::Object(map)) => Value::Object(map),
            _ => json!({}),
        };
        if with_tools {
            let name = if call.function.name.is_empty() {
                crate::history::UNKNOWN_TOOL_NAME
            } else {
                &call.function.name
            };
            blocks.push(json!({
                "type": "tool_use",
                "id": safe_id(&call.id),
                "name": name,
                "input": input,
            }));
        } else {
            blocks.push(text_block(&format!(
                "[Called the tool {} with {}]",
                call.function.name, input
            )));
        }
    }
    blocks
}

/// A tool result: a `tool_result` block (with `is_error` for a call that failed, was refused
/// or did not run), or its text when the request offers no tools.
fn result_blocks(message: &ChatMessage, with_tools: bool) -> Vec<Value> {
    let mut content = content_blocks(message.content.as_ref());
    if !with_tools {
        let name = message.name.as_deref().unwrap_or("a tool");
        let mut blocks = vec![text_block(&format!("[Result of the tool {name}]"))];
        blocks.append(&mut content);
        return blocks;
    }
    if content.is_empty() {
        content.push(text_block("(no output)"));
    }
    let mut block = Map::new();
    block.insert("type".into(), json!("tool_result"));
    block.insert(
        "tool_use_id".into(),
        Value::String(safe_id(message.tool_call_id.as_deref().unwrap_or_default())),
    );
    block.insert("content".into(), Value::Array(content));
    if message.is_error {
        block.insert("is_error".into(), Value::Bool(true));
    }
    vec![Value::Object(block)]
}

/// A tool call id in the form the API accepts (`^[a-zA-Z0-9_-]+$`); the same id always
/// becomes the same text, so a call and its result still match.
fn safe_id(id: &str) -> String {
    let cleaned: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "call".to_owned()
    } else {
        cleaned
    }
}

/// A `cache_control` breakpoint on the newest message's last block, so the next request of
/// the chat reads everything before it from the cache.
fn mark_newest_block(messages: &mut [Value]) {
    let Some(Value::Array(content)) = messages
        .last_mut()
        .and_then(|message| message.get_mut("content"))
    else {
        return;
    };
    if let Some(Value::Object(block)) = content.last_mut() {
        let cacheable = matches!(
            block.get("type").and_then(Value::as_str),
            Some("text" | "image" | "tool_result" | "tool_use")
        );
        if cacheable {
            block.insert("cache_control".into(), json!({"type": "ephemeral"}));
        }
    }
}
