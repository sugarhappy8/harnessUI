//! Request and message types for OpenAI-compatible chat completions.
//!
//! These mirror the wire format that llama-server, Ollama, LM Studio and vLLM accept on
//! `/v1/chat/completions` (ARCHITECTURE.md 7.5 and 8.2). Optional fields are left out of the
//! JSON rather than sent as `null`, because several servers treat an explicit `null`
//! differently from an absent field.

use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// The author of a chat message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    /// The single system message at the start of the conversation (ARCHITECTURE.md 8.5).
    System,
    /// The person using the app.
    User,
    /// The model.
    Assistant,
    /// The result of a tool call, answering one assistant tool call by id.
    Tool,
}

/// An image reference inside a message.
///
/// The app only ever sends inline `data:` URIs: llama-server fetches http(s) URLs itself,
/// even with `--offline` (ARCHITECTURE.md 7.3). [`ChatRequest::validate_media`] rejects any
/// other form before a request leaves the process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUrl {
    /// A `data:image/<type>;base64,<data>` URI.
    pub url: String,
}

impl ImageUrl {
    /// Builds a data URI from a MIME type such as `image/png` and base64-encoded bytes.
    ///
    /// # Errors
    /// Returns a description of the problem when the MIME type is not an image type or the
    /// payload is empty or not plain base64 text.
    pub fn from_base64(mime: &str, base64_data: &str) -> Result<Self, String> {
        let url = format!("data:{mime};base64,{base64_data}");
        validate_data_uri(&url)?;
        Ok(Self { url })
    }

    /// Checks that this is an inline image data URI in the form llama-server accepts.
    ///
    /// # Errors
    /// Returns a description of the problem otherwise.
    pub fn validate(&self) -> Result<(), String> {
        validate_data_uri(&self.url)
    }
}

/// Validates the data-URI shape llama-server's media loader accepts: a `data:image/` prefix,
/// a `;base64` marker before the single comma, and a non-empty base64 payload.
fn validate_data_uri(url: &str) -> Result<(), String> {
    let Some(rest) = url.strip_prefix("data:") else {
        return Err("only inline data: image URIs are sent to a model server".to_owned());
    };
    let Some((header, payload)) = rest.split_once(',') else {
        return Err("image data URI has no comma".to_owned());
    };
    if payload.contains(',') {
        return Err("image data URI has more than one comma".to_owned());
    }
    let Some(mime) = header.strip_suffix(";base64") else {
        return Err("image data URI is not base64-encoded".to_owned());
    };
    let Some(subtype) = mime.strip_prefix("image/") else {
        return Err(format!("data URI type '{mime}' is not an image type"));
    };
    if subtype.is_empty() {
        return Err("image data URI has no image subtype".to_owned());
    }
    if payload.is_empty() {
        return Err("image data URI is empty".to_owned());
    }
    // llama-server's decoder accepts only the standard alphabet and stops silently at the
    // first other character, so URL-safe `-`/`_` or an `=` before the end would send a
    // truncated image. Padding is at most two `=` at the very end.
    let unpadded = payload
        .strip_suffix("==")
        .or_else(|| payload.strip_suffix('='))
        .unwrap_or(payload);
    if unpadded.is_empty()
        || !unpadded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/'))
    {
        return Err("image data URI payload is not standard base64 text".to_owned());
    }
    Ok(())
}

/// One part of a multi-part message: text or an image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    /// A run of text.
    Text {
        /// The text.
        text: String,
    },
    /// An inline image.
    ImageUrl {
        /// The image as a data URI.
        image_url: ImageUrl,
    },
}

impl ContentPart {
    /// A text part.
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// An image part.
    pub fn image(image_url: ImageUrl) -> Self {
        Self::ImageUrl { image_url }
    }
}

/// The content of a message: plain text, or a list of parts when images are involved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    /// Plain text, serialised as a JSON string.
    Text(String),
    /// Text and image parts, serialised as a JSON array.
    Parts(Vec<ContentPart>),
}

impl MessageContent {
    /// The text of the content, with the text parts of a multi-part message joined by
    /// newlines and images left out.
    pub fn text(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::Parts(parts) => parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    ContentPart::ImageUrl { .. } => None,
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// The images in the content, in order.
    pub fn images(&self) -> impl Iterator<Item = &ImageUrl> {
        let parts: &[ContentPart] = match self {
            Self::Text(_) => &[],
            Self::Parts(parts) => parts,
        };
        parts.iter().filter_map(|part| match part {
            ContentPart::ImageUrl { image_url } => Some(image_url),
            ContentPart::Text { .. } => None,
        })
    }

    /// True when there is no text and no image.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Text(text) => text.is_empty(),
            Self::Parts(parts) => parts.iter().all(|part| match part {
                ContentPart::Text { text } => text.is_empty(),
                ContentPart::ImageUrl { .. } => false,
            }),
        }
    }
}

impl From<String> for MessageContent {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<&str> for MessageContent {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<Vec<ContentPart>> for MessageContent {
    fn from(parts: Vec<ContentPart>) -> Self {
        Self::Parts(parts)
    }
}

/// The function a tool call names, with its arguments as JSON text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    /// The tool name.
    pub name: String,
    /// The arguments as JSON text. Deserialisation also accepts a JSON object (some servers
    /// send one) and stores it as text, so this field always holds text.
    #[serde(default, deserialize_with = "arguments_as_text")]
    pub arguments: String,
}

/// Accepts tool-call arguments given as a JSON string (the OpenAI form) or as any other JSON
/// value (llama-server and Ollama accept and sometimes emit an object), returning text.
fn arguments_as_text<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(match value {
        Value::String(text) => text,
        Value::Null => String::new(),
        other => other.to_string(),
    })
}

fn function_kind() -> String {
    "function".to_owned()
}

/// A tool call made by the model, as stored in an assistant message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    /// The call's id; the tool result names it in `tool_call_id`.
    pub id: String,
    /// Always `"function"`.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// The function name and arguments.
    pub function: FunctionCall,
}

impl ToolCall {
    /// A function tool call.
    pub fn function(
        id: impl Into<String>,
        name: impl Into<String>,
        arguments: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: function_kind(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }
}

/// One message in a chat request.
///
/// Serialised without `null` fields. `reasoning_content` is an addition to the OpenAI shape
/// that llama-server accepts for templates that preserve reasoning; leave it `None` unless
/// `/props` reports `supports_preserve_reasoning` (ARCHITECTURE.md 8.2, "Reasoning").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    /// Who wrote the message.
    pub role: Role,
    /// Text or parts. An assistant message that only calls tools has none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    /// The model's reasoning, sent back only when the template supports that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    /// Tool calls made by an assistant message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// For a tool message: the id of the call it answers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Optional author name (for a tool message, some templates render the tool name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

impl ChatMessage {
    fn bare(role: Role, content: Option<MessageContent>) -> Self {
        Self {
            role,
            content,
            reasoning_content: None,
            tool_calls: None,
            tool_call_id: None,
            name: None,
        }
    }

    /// A system message.
    pub fn system(text: impl Into<String>) -> Self {
        Self::bare(Role::System, Some(MessageContent::Text(text.into())))
    }

    /// A user message with text or parts.
    pub fn user(content: impl Into<MessageContent>) -> Self {
        Self::bare(Role::User, Some(content.into()))
    }

    /// An assistant message with text.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::bare(Role::Assistant, Some(MessageContent::Text(text.into())))
    }

    /// An assistant message that calls tools, with optional text before the calls. Empty
    /// text is left out, because templates render an empty content field inconsistently,
    /// and so is an empty call list, which OpenAI-style servers reject (`tool_calls: []`).
    pub fn assistant_tool_calls(text: Option<String>, tool_calls: Vec<ToolCall>) -> Self {
        let content = text
            .filter(|text| !text.is_empty())
            .map(MessageContent::Text);
        Self {
            tool_calls: (!tool_calls.is_empty()).then_some(tool_calls),
            ..Self::bare(Role::Assistant, content)
        }
    }

    /// A tool result answering the call with `tool_call_id`.
    pub fn tool_result(
        tool_call_id: impl Into<String>,
        content: impl Into<MessageContent>,
    ) -> Self {
        Self {
            tool_call_id: Some(tool_call_id.into()),
            ..Self::bare(Role::Tool, Some(content.into()))
        }
    }
}

/// The function half of a [`ToolSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FunctionSpec {
    /// The tool name the model calls.
    pub name: String,
    /// What the tool does, for the model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// The JSON Schema of the arguments object.
    pub parameters: Value,
}

/// A tool offered to the model, in the OpenAI shape
/// `{"type":"function","function":{"name","description","parameters"}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    /// Always `"function"`.
    #[serde(rename = "type", default = "function_kind")]
    pub kind: String,
    /// The function's name, description and parameter schema.
    pub function: FunctionSpec,
}

impl ToolSpec {
    /// A function tool. An empty description is left out.
    pub fn function(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
    ) -> Self {
        let description = description.into();
        Self {
            kind: function_kind(),
            function: FunctionSpec {
                name: name.into(),
                description: (!description.is_empty()).then_some(description),
                parameters,
            },
        }
    }
}

/// Whether the model may, must or must not call tools.
///
/// Always serialised as a string: llama-server reads `tool_choice` as a string only and
/// silently turns the OpenAI object form into `auto`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolChoice {
    /// The model decides.
    Auto,
    /// No tool calls. The tool list stays in the request so the cached prompt stays valid.
    None,
    /// The model must call a tool.
    Required,
}

/// Keys that [`ChatRequest`] writes itself; entries in `extra` with these names are ignored so
/// the JSON never carries a key twice.
const RESERVED_KEYS: &[&str] = &[
    "model",
    "messages",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "max_tokens",
    "temperature",
    "top_p",
    "top_k",
    "stream",
    "stream_options",
];

/// A chat-completions request.
///
/// Serialisation rules:
/// - `tools` is left out when empty, and so are `tool_choice` and `parallel_tool_calls`
///   (OpenAI-style servers reject them without tools).
/// - When `stream` is true, `stream_options: {"include_usage": true}` is added.
/// - `extra` is flattened into the top level for server-specific fields such as
///   `chat_template_kwargs`, `cache_prompt` or `reasoning_format`. An `extra` key that
///   collides with a field above is ignored.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatRequest {
    /// The model id. llama-server ignores it (one model per process); remote servers need it.
    pub model: String,
    /// The conversation.
    pub messages: Vec<ChatMessage>,
    /// Tools offered to the model.
    pub tools: Vec<ToolSpec>,
    /// Tool choice; `None` leaves it to the server.
    pub tool_choice: Option<ToolChoice>,
    /// Whether the model may return several calls at once. The agent loop always sends
    /// `Some(false)` (ARCHITECTURE.md 8.2, "Order").
    pub parallel_tool_calls: Option<bool>,
    /// Output limit in tokens.
    pub max_tokens: Option<u32>,
    /// Sampling temperature; `None` lets the server apply the model's own default.
    pub temperature: Option<f32>,
    /// Nucleus sampling.
    pub top_p: Option<f32>,
    /// Top-k sampling (not part of the OpenAI API; llama-server, LM Studio and vLLM accept it).
    pub top_k: Option<u32>,
    /// Stream the response as server-sent events.
    pub stream: bool,
    /// Server-specific fields, flattened into the JSON object.
    pub extra: Map<String, Value>,
}

impl ChatRequest {
    /// A streaming request for `model` with `messages` and no tools.
    pub fn new(model: impl Into<String>, messages: Vec<ChatMessage>) -> Self {
        Self {
            model: model.into(),
            messages,
            stream: true,
            ..Self::default()
        }
    }

    /// Sets a server-specific top-level field.
    pub fn set_extra(&mut self, key: impl Into<String>, value: Value) -> &mut Self {
        self.extra.insert(key.into(), value);
        self
    }

    /// Sets one entry of llama-server's `chat_template_kwargs` object, creating the object
    /// or replacing a non-object value. Example: `("enable_thinking", false)`.
    pub fn set_chat_template_kwarg(&mut self, key: impl Into<String>, value: Value) -> &mut Self {
        let entry = self
            .extra
            .entry("chat_template_kwargs")
            .or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Value::Object(map) = entry {
            map.insert(key.into(), value);
        }
        self
    }

    /// Checks every image in the request is an inline data URI (ARCHITECTURE.md 7.3: the
    /// app never forwards an http(s) or `file:` image URL).
    ///
    /// # Errors
    /// Returns a description of the first offending image.
    pub fn validate_media(&self) -> Result<(), String> {
        for (index, message) in self.messages.iter().enumerate() {
            if let Some(content) = &message.content {
                for image in content.images() {
                    image
                        .validate()
                        .map_err(|problem| format!("message {index}: {problem}"))?;
                }
            }
        }
        Ok(())
    }

    /// The JSON body to send, with `stream` overridden. `stream_options` is added only when
    /// streaming.
    pub(crate) fn to_body(&self, stream: bool) -> Value {
        let mut map = Map::new();
        map.insert("model".into(), Value::String(self.model.clone()));
        map.insert("messages".into(), json_value(&self.messages));
        if !self.tools.is_empty() {
            map.insert("tools".into(), json_value(&self.tools));
            if let Some(choice) = self.tool_choice {
                map.insert("tool_choice".into(), json_value(&choice));
            }
            if let Some(parallel) = self.parallel_tool_calls {
                map.insert("parallel_tool_calls".into(), Value::Bool(parallel));
            }
        }
        if let Some(max_tokens) = self.max_tokens {
            map.insert("max_tokens".into(), Value::from(max_tokens));
        }
        if let Some(temperature) = self.temperature.and_then(f32_value) {
            map.insert("temperature".into(), temperature);
        }
        if let Some(top_p) = self.top_p.and_then(f32_value) {
            map.insert("top_p".into(), top_p);
        }
        if let Some(top_k) = self.top_k {
            map.insert("top_k".into(), Value::from(top_k));
        }
        map.insert("stream".into(), Value::Bool(stream));
        if stream {
            map.insert(
                "stream_options".into(),
                serde_json::json!({ "include_usage": true }),
            );
        }
        for (key, value) in &self.extra {
            if !RESERVED_KEYS.contains(&key.as_str()) {
                map.insert(key.clone(), value.clone());
            }
        }
        Value::Object(map)
    }
}

/// A JSON number for an `f32` with its shortest decimal form (`0.7`, not the widened
/// `0.699999988079071`). `None` for NaN or infinity, which JSON cannot carry.
fn f32_value(value: f32) -> Option<Value> {
    let widened: f64 = value.to_string().parse().ok()?;
    serde_json::Number::from_f64(widened).map(Value::Number)
}

/// Converts a serialisable value to JSON. The types used here (strings, numbers, derived
/// structs without maps keyed by non-strings) cannot fail; `Null` is the defensive fallback.
fn json_value<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

impl Serialize for ChatRequest {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let Value::Object(body) = self.to_body(self.stream) else {
            return serializer.serialize_map(Some(0))?.end();
        };
        let mut map = serializer.serialize_map(Some(body.len()))?;
        for (key, value) in &body {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roles_serialise_lowercase() {
        assert_eq!(
            serde_json::to_value(Role::Assistant).unwrap(),
            json!("assistant")
        );
        assert_eq!(serde_json::to_value(Role::Tool).unwrap(), json!("tool"));
    }

    #[test]
    fn messages_serialise_without_nulls() {
        let message = ChatMessage::user("hi");
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({"role": "user", "content": "hi"})
        );
        let call = ToolCall::function("call_1", "read_file", r#"{"path":"a"}"#);
        let assistant = ChatMessage::assistant_tool_calls(Some(String::new()), vec![call]);
        assert_eq!(
            serde_json::to_value(&assistant).unwrap(),
            json!({
                "role": "assistant",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {"name": "read_file", "arguments": "{\"path\":\"a\"}"}
                }]
            })
        );
        // No calls: no `tool_calls: []`, which OpenAI-style servers reject.
        let no_calls = ChatMessage::assistant_tool_calls(Some("done".into()), vec![]);
        assert_eq!(
            serde_json::to_value(&no_calls).unwrap(),
            json!({"role": "assistant", "content": "done"})
        );
        let result = ChatMessage::tool_result("call_1", "contents");
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"role": "tool", "content": "contents", "tool_call_id": "call_1"})
        );
    }

    #[test]
    fn content_parts_use_openai_tags() {
        let image = ImageUrl::from_base64("image/png", "iVBORw0KGgo=").unwrap();
        let message = ChatMessage::user(vec![ContentPart::text("look"), ContentPart::image(image)]);
        assert_eq!(
            serde_json::to_value(&message).unwrap(),
            json!({
                "role": "user",
                "content": [
                    {"type": "text", "text": "look"},
                    {"type": "image_url", "image_url": {"url": "data:image/png;base64,iVBORw0KGgo="}}
                ]
            })
        );
    }

    #[test]
    fn content_round_trips_untagged() {
        let text: MessageContent = serde_json::from_value(json!("plain")).unwrap();
        assert_eq!(text, MessageContent::Text("plain".into()));
        let parts: MessageContent =
            serde_json::from_value(json!([{"type": "text", "text": "a"}])).unwrap();
        assert_eq!(parts, MessageContent::Parts(vec![ContentPart::text("a")]));
        assert_eq!(parts.text(), "a");
    }

    #[test]
    fn tool_call_arguments_accept_objects() {
        let call: ToolCall = serde_json::from_value(json!({
            "id": "x",
            "function": {"name": "f", "arguments": {"a": 1}}
        }))
        .unwrap();
        assert_eq!(call.kind, "function");
        assert_eq!(call.function.arguments, r#"{"a":1}"#);
    }

    #[test]
    fn data_uris_are_validated() {
        assert!(ImageUrl::from_base64("image/jpeg", "AAAA").is_ok());
        assert!(ImageUrl::from_base64("text/plain", "AAAA").is_err());
        assert!(ImageUrl::from_base64("image/png", "").is_err());
        assert!(ImageUrl::from_base64("image/png", "AA,AA").is_err());
        assert!(ImageUrl::from_base64("image/png", "AAA=").is_ok());
        assert!(ImageUrl::from_base64("image/png", "AA==").is_ok());
        for url in [
            "https://example.com/a.png",
            "http://127.0.0.1:9931/x.png",
            "file:///etc/passwd",
            "data:image/png,rawdata",
            "data:image/;base64,AAAA",
            "data:image/png;base64,AA AA",
            // URL-safe alphabet, interior or excess padding, padding only: llama-server's
            // decoder would stop early and load a truncated image.
            "data:image/png;base64,AB-_",
            "data:image/png;base64,AA=A",
            "data:image/png;base64,A===",
            "data:image/png;base64,==",
            "data:text/html;base64,PGI+",
        ] {
            let image = ImageUrl { url: url.into() };
            assert!(image.validate().is_err(), "{url} should be rejected");
        }
    }

    #[test]
    fn request_rejects_remote_images() {
        let image = ImageUrl {
            url: "http://example.com/cat.png".into(),
        };
        let request = ChatRequest::new(
            "m",
            vec![ChatMessage::user(vec![ContentPart::image(image)])],
        );
        let err = request.validate_media().unwrap_err();
        assert!(err.contains("message 0"), "{err}");
    }

    #[test]
    fn request_body_shape() {
        let mut request = ChatRequest::new(
            "qwen",
            vec![ChatMessage::system("s"), ChatMessage::user("u")],
        );
        request.tools = vec![ToolSpec::function(
            "get_weather",
            "Get the weather",
            json!({"type": "object", "properties": {"city": {"type": "string"}}}),
        )];
        request.tool_choice = Some(ToolChoice::Auto);
        request.parallel_tool_calls = Some(false);
        request.max_tokens = Some(256);
        request.temperature = Some(0.7);
        request.top_k = Some(20);
        request.set_chat_template_kwarg("enable_thinking", json!(false));
        request.set_extra("cache_prompt", json!(true));
        request.set_extra("stream", json!(false));
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(
            body,
            json!({
                "model": "qwen",
                "messages": [
                    {"role": "system", "content": "s"},
                    {"role": "user", "content": "u"}
                ],
                "tools": [{
                    "type": "function",
                    "function": {
                        "name": "get_weather",
                        "description": "Get the weather",
                        "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}
                    }
                }],
                "tool_choice": "auto",
                "parallel_tool_calls": false,
                "max_tokens": 256,
                "temperature": 0.7,
                "top_k": 20,
                "stream": true,
                "stream_options": {"include_usage": true},
                "chat_template_kwargs": {"enable_thinking": false},
                "cache_prompt": true
            })
        );
    }

    #[test]
    fn tool_fields_are_dropped_without_tools() {
        let mut request = ChatRequest::new("m", vec![ChatMessage::user("u")]);
        request.tool_choice = Some(ToolChoice::None);
        request.parallel_tool_calls = Some(false);
        request.stream = false;
        let body = serde_json::to_value(&request).unwrap();
        assert_eq!(
            body,
            json!({"model": "m", "messages": [{"role": "user", "content": "u"}], "stream": false})
        );
    }

    #[test]
    fn non_finite_sampling_values_are_left_out() {
        let mut request = ChatRequest::new("m", vec![]);
        request.temperature = Some(f32::NAN);
        request.top_p = Some(0.95);
        let body = serde_json::to_value(&request).unwrap();
        assert!(body.get("temperature").is_none());
        assert_eq!(body["top_p"], json!(0.95));
    }

    #[test]
    fn tool_choice_serialises_as_strings() {
        for (choice, text) in [
            (ToolChoice::Auto, "auto"),
            (ToolChoice::None, "none"),
            (ToolChoice::Required, "required"),
        ] {
            assert_eq!(serde_json::to_value(choice).unwrap(), json!(text));
        }
    }

    #[test]
    fn chat_template_kwargs_merge() {
        let mut request = ChatRequest::new("m", vec![]);
        request.set_extra("chat_template_kwargs", json!("bogus"));
        request.set_chat_template_kwarg("a", json!(1));
        request.set_chat_template_kwarg("b", json!(2));
        assert_eq!(
            request.extra["chat_template_kwargs"],
            json!({"a": 1, "b": 2})
        );
    }
}
