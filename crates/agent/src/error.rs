//! Provider errors, and the mapping from server error bodies to them.

use serde_json::Value;

use crate::context::tokens_with_words;

/// The longest server message kept in an error, in characters. Longer messages are cut so a
/// misbehaving server cannot fill the UI or the log with one error.
const MAX_MESSAGE_CHARS: usize = 2_000;

/// Text in a llama-server error that means its tool-call parser could not read the model's
/// output (ARCHITECTURE.md 8.3, step 3).
const PARSE_ERROR_MARKER: &str = "does not match the expected";

/// A turn's message when the chat no longer fits the app's own model, even after older tool
/// results were shortened and earlier messages summarised (codes `context_full` and
/// `context_overflow`). How much the model remembers is set in its Model settings.
pub const CHAT_TOO_LONG_LOCAL: &str = "This chat is too long for the model to remember, even \
     after summarising. Start a new chat, or let this model remember more in Model settings.";

/// The same for a model on an endpoint, whose size the app does not choose.
pub const CHAT_TOO_LONG_REMOTE: &str =
    "This chat is too long for the model to remember, even after summarising. Start a new chat.";

/// A turn's message when the chat no longer fits, even after the context stages: `local` for
/// the app's own model ([`CHAT_TOO_LONG_LOCAL`]), else [`CHAT_TOO_LONG_REMOTE`].
pub fn chat_too_long(local: bool) -> &'static str {
    if local {
        CHAT_TOO_LONG_LOCAL
    } else {
        CHAT_TOO_LONG_REMOTE
    }
}

/// An error from a model server or from talking to one.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The server answered with an error status (or a mid-stream error object) that has no
    /// more specific variant.
    #[error("HTTP {status}: {message}")]
    Http {
        /// The HTTP status, or the error object's `code` for a mid-stream error.
        status: u16,
        /// The server's message, cut to a bounded length.
        message: String,
    },
    /// The request does not fit the model's context window. From llama-server's
    /// `exceed_context_size_error`, or the equivalent message of other servers.
    #[error("request exceeds the context window ({n_prompt_tokens:?} tokens, {n_ctx:?} available)")]
    ContextOverflow {
        /// Tokens in the rejected request, when the server said.
        n_prompt_tokens: Option<u32>,
        /// The context size, when the server said.
        n_ctx: Option<u32>,
    },
    /// The server rejected the API key (HTTP 401).
    #[error("the server rejected the API key")]
    Unauthorized,
    /// llama-server could not parse the model's output in the template's tool-call format
    /// ("The model produced output that does not match the expected ... format"), as an
    /// HTTP 500 or a mid-stream error.
    #[error("the server could not parse the model output: {0}")]
    ServerParseError(String),
    /// Connecting, sending or receiving failed, or the server went silent for too long.
    #[error("network error: {0}")]
    Network(String),
    /// The response was not in a form this client understands.
    #[error("could not decode the server response: {0}")]
    Decode(String),
    /// The request was cancelled by the caller.
    #[error("cancelled")]
    Cancelled,
    /// The request or the client configuration was refused before anything was sent (for
    /// example an image that is not an inline data URI, or a base URL that is not http(s)).
    #[error("invalid request: {0}")]
    InvalidRequest(String),
}

impl ProviderError {
    /// A plain-language message for the UI (ARCHITECTURE.md 15). Technical detail stays in
    /// the `Display` form for Copy details and the log.
    pub fn user_message(&self) -> String {
        match self {
            Self::Http { status: 503, .. } => {
                "The model server is still starting or busy. Try again in a moment.".to_owned()
            }
            Self::Http {
                status: 404,
                message,
            } => format!(
                "The server does not have what was asked for (HTTP 404). Check the base URL and \
                 the model name. The server said: {message}"
            ),
            Self::Http { status, message } => {
                format!("The model server returned an error (HTTP {status}): {message}")
            }
            Self::ContextOverflow {
                n_prompt_tokens,
                n_ctx,
            } => {
                let needs = n_prompt_tokens
                    .map(|prompt| format!(" It needs {}.", tokens_with_words(prompt)));
                match (needs, n_ctx) {
                    (needs, Some(ctx)) => format!(
                        "This chat is too long for the model to remember.{} The model can \
                         remember {}.",
                        needs.unwrap_or_default(),
                        tokens_with_words(*ctx)
                    ),
                    (Some(needs), None) => {
                        format!("This chat is too long for the model to remember.{needs}")
                    }
                    (None, None) => "This chat is too long for the model to remember.".to_owned(),
                }
            }
            Self::Unauthorized => {
                "The server rejected the API key. Check the key in Settings.".to_owned()
            }
            Self::ServerParseError(_) => "The model produced a reply the server could not read \
                                          as an answer or a tool call."
                .to_owned(),
            Self::Network(detail) => format!("Could not reach the model server: {detail}"),
            Self::Decode(_) => "The server sent a response this app could not read.".to_owned(),
            Self::Cancelled => "Stopped.".to_owned(),
            Self::InvalidRequest(detail) => format!("The request was not sent: {detail}"),
        }
    }
}

/// Cuts a server message to a bounded length on a character boundary.
pub(crate) fn bounded(message: &str) -> String {
    let message = message.trim();
    match message.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((cut, _)) => format!("{}…", &message[..cut]),
        None => message.to_owned(),
    }
}

/// Maps an error response body to an error. `status` is the HTTP status when there was one
/// (`None` for a mid-stream error event); `reason` is the status's canonical phrase, used when
/// the body says nothing.
pub(crate) fn error_from_body(status: Option<u16>, body: &[u8], reason: &str) -> ProviderError {
    if let Ok(value) = serde_json::from_slice::<Value>(body) {
        return error_from_value(&value, status);
    }
    let text = String::from_utf8_lossy(body);
    // Some servers send an error status with an event-stream body.
    for line in text.lines() {
        if let Some(data) = line.strip_prefix("data:")
            && let Ok(value) = serde_json::from_str::<Value>(data.trim())
            && value.is_object()
        {
            return error_from_value(&value, status);
        }
    }
    let message = if text.trim().is_empty() {
        reason.to_owned()
    } else {
        text.into_owned()
    };
    classify(ErrorParts {
        status,
        message: bounded(&message),
        ..ErrorParts::default()
    })
}

/// Maps a JSON error value to an error. Accepts the OpenAI form `{"error":{...}}`, a bare
/// string `{"error":"..."}` (Ollama native), vLLM's `{"object":"error","message":...}` and
/// FastAPI's `{"detail":...}`.
pub(crate) fn error_from_value(value: &Value, status: Option<u16>) -> ProviderError {
    let inner = value.get("error").filter(|e| !e.is_null()).unwrap_or(value);
    let mut parts = ErrorParts {
        status,
        ..ErrorParts::default()
    };
    match inner {
        Value::String(text) => parts.message = bounded(text),
        Value::Object(map) => {
            let message = map
                .get("message")
                .or_else(|| map.get("detail"))
                .or_else(|| map.get("msg"));
            parts.message = match message {
                Some(Value::String(text)) => bounded(text),
                Some(other) => bounded(&other.to_string()),
                None => bounded(&inner.to_string()),
            };
            parts.kind = map.get("type").and_then(Value::as_str).map(str::to_owned);
            match map.get("code") {
                Some(Value::Number(code)) => {
                    if parts.status.is_none() {
                        parts.status = code.as_u64().and_then(|c| u16::try_from(c).ok());
                    }
                }
                Some(Value::String(code)) => parts.code = Some(code.clone()),
                _ => {}
            }
            parts.n_prompt_tokens = map.get("n_prompt_tokens").and_then(as_u32);
            parts.n_ctx = map.get("n_ctx").and_then(as_u32);
        }
        other => parts.message = bounded(&other.to_string()),
    }
    classify(parts)
}

pub(crate) fn as_u32(value: &Value) -> Option<u32> {
    value.as_u64().and_then(|n| u32::try_from(n).ok())
}

#[derive(Default)]
struct ErrorParts {
    status: Option<u16>,
    message: String,
    kind: Option<String>,
    code: Option<String>,
    n_prompt_tokens: Option<u32>,
    n_ctx: Option<u32>,
}

fn classify(parts: ErrorParts) -> ProviderError {
    let kind = parts.kind.as_deref().unwrap_or("");
    let code = parts.code.as_deref().unwrap_or("");
    let lower = parts.message.to_lowercase();
    if kind == "exceed_context_size_error"
        || code == "context_length_exceeded"
        || is_overflow_message(&lower)
    {
        return ProviderError::ContextOverflow {
            n_prompt_tokens: parts.n_prompt_tokens.or_else(|| prompt_tokens_in(&lower)),
            n_ctx: parts.n_ctx.or_else(|| context_size_in(&lower)),
        };
    }
    if parts.status == Some(401) || kind == "authentication_error" {
        return ProviderError::Unauthorized;
    }
    if parts.message.contains(PARSE_ERROR_MARKER) {
        return ProviderError::ServerParseError(parts.message);
    }
    ProviderError::Http {
        status: parts.status.unwrap_or(500),
        message: parts.message,
    }
}

/// Context-overflow wording of servers that do not send a typed error: llama-server, vLLM
/// and OpenAI ("maximum context length"), LM Studio ("context length of only").
fn is_overflow_message(lower: &str) -> bool {
    lower.contains("exceeds the available context size")
        || lower.contains("maximum context length")
        || lower.contains("context length of only")
        || lower.contains("context_length_exceeded")
}

fn prompt_tokens_in(lower: &str) -> Option<u32> {
    ["request (", "requested ", "resulted in "]
        .iter()
        .find_map(|marker| number_after(lower, marker))
}

fn context_size_in(lower: &str) -> Option<u32> {
    [
        "available context size (",
        "maximum context length is ",
        "context length of only ",
    ]
    .iter()
    .find_map(|marker| number_after(lower, marker))
}

/// The integer right after the first occurrence of `marker`, if any.
fn number_after(text: &str, marker: &str) -> Option<u32> {
    let start = text.find(marker)? + marker.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_server_overflow_fixture() {
        let body = include_bytes!("../tests/fixtures/overflow_error.json");
        assert_eq!(
            error_from_body(Some(400), body, "Bad Request"),
            ProviderError::ContextOverflow {
                n_prompt_tokens: Some(5009),
                n_ctx: Some(4096)
            }
        );
    }

    #[test]
    fn overflow_numbers_from_message_only() {
        let body = br#"{"error":{"message":"This model's maximum context length is 8192 tokens. However, you requested 9000 tokens.","type":"BadRequestError","code":400}}"#;
        assert_eq!(
            error_from_body(Some(400), body, ""),
            ProviderError::ContextOverflow {
                n_prompt_tokens: Some(9000),
                n_ctx: Some(8192)
            }
        );
        let vllm = br#"{"object":"error","message":"This model's maximum context length is 4096 tokens. However, your messages resulted in 5000 tokens.","type":"BadRequestError","code":400}"#;
        assert_eq!(
            error_from_body(Some(400), vllm, ""),
            ProviderError::ContextOverflow {
                n_prompt_tokens: Some(5000),
                n_ctx: Some(4096)
            }
        );
    }

    #[test]
    fn unauthorized() {
        let body =
            br#"{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}"#;
        assert_eq!(
            error_from_body(Some(401), body, "Unauthorized"),
            ProviderError::Unauthorized
        );
        assert_eq!(
            error_from_body(Some(401), b"", "Unauthorized"),
            ProviderError::Unauthorized
        );
    }

    #[test]
    fn parse_error_from_500_and_mid_stream() {
        let body = br#"{"error":{"code":500,"message":"The model produced output that does not match the expected peg-native format","type":"server_error"}}"#;
        assert!(matches!(
            error_from_body(Some(500), body, ""),
            ProviderError::ServerParseError(m) if m.contains("peg-native")
        ));
        let value: Value = serde_json::from_slice(body).unwrap();
        assert!(matches!(
            error_from_value(&value, None),
            ProviderError::ServerParseError(_)
        ));
    }

    #[test]
    fn mid_stream_error_takes_status_from_code() {
        let value = serde_json::json!({"error": {"code": 503, "message": "Loading model", "type": "unavailable_error"}});
        assert_eq!(
            error_from_value(&value, None),
            ProviderError::Http {
                status: 503,
                message: "Loading model".into()
            }
        );
    }

    #[test]
    fn plain_text_and_string_errors() {
        assert_eq!(
            error_from_body(Some(502), b"<html>Bad gateway</html>", "Bad Gateway"),
            ProviderError::Http {
                status: 502,
                message: "<html>Bad gateway</html>".into()
            }
        );
        assert_eq!(
            error_from_body(
                Some(404),
                br#"{"error":"model 'x' not found"}"#,
                "Not Found"
            ),
            ProviderError::Http {
                status: 404,
                message: "model 'x' not found".into()
            }
        );
        assert_eq!(
            error_from_body(Some(500), b"   ", "Internal Server Error"),
            ProviderError::Http {
                status: 500,
                message: "Internal Server Error".into()
            }
        );
        assert_eq!(
            error_from_body(Some(422), br#"{"detail":"bad field"}"#, ""),
            ProviderError::Http {
                status: 422,
                message: "bad field".into()
            }
        );
    }

    #[test]
    fn event_stream_error_body() {
        let body = b"data: {\"error\":{\"message\":\"boom\",\"code\":400}}\n\n";
        assert_eq!(
            error_from_body(Some(400), body, ""),
            ProviderError::Http {
                status: 400,
                message: "boom".into()
            }
        );
    }

    #[test]
    fn long_messages_are_bounded() {
        let long = "x".repeat(10_000);
        let message = bounded(&long);
        assert_eq!(message.chars().count(), MAX_MESSAGE_CHARS + 1);
    }

    #[test]
    fn user_messages_are_plain() {
        // 0.0.6: no "context window"; the model's size comes with words.
        assert_eq!(
            ProviderError::ContextOverflow {
                n_prompt_tokens: Some(5009),
                n_ctx: Some(4096)
            }
            .user_message(),
            "This chat is too long for the model to remember. It needs 5,009 tokens, about \
             3,000 words. The model can remember 4,096 tokens, about 3,000 words."
        );
        assert_eq!(
            ProviderError::ContextOverflow {
                n_prompt_tokens: None,
                n_ctx: Some(8192)
            }
            .user_message(),
            "This chat is too long for the model to remember. The model can remember 8,192 \
             tokens, about 6,000 words."
        );
        assert_eq!(
            ProviderError::ContextOverflow {
                n_prompt_tokens: Some(9000),
                n_ctx: None
            }
            .user_message(),
            "This chat is too long for the model to remember. It needs 9,000 tokens, about \
             6,000 words."
        );
        assert_eq!(
            ProviderError::ContextOverflow {
                n_prompt_tokens: None,
                n_ctx: None
            }
            .user_message(),
            "This chat is too long for the model to remember."
        );
        for message in [chat_too_long(true), chat_too_long(false)] {
            assert!(
                message.starts_with(
                    "This chat is too long for the model to remember, even after summarising. \
                     Start a new chat"
                ),
                "{message}"
            );
            assert!(!message.contains("context"), "{message}");
            assert!(!message.contains('!'), "{message}");
        }
        assert!(chat_too_long(true).ends_with("let this model remember more in Model settings."));
        assert!(!chat_too_long(false).contains("Model settings"));
        assert!(
            ProviderError::Unauthorized
                .user_message()
                .contains("API key")
        );
        assert_eq!(ProviderError::Cancelled.user_message(), "Stopped.");
    }
}
