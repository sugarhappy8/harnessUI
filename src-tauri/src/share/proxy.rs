//! The sharing proxy (0.0.6 item 3, direction A; D10): a small HTTP/1 server that the app
//! owns, on `127.0.0.1:<kept port>`, in front of llama-server, which stays on its random
//! port with its private key.
//!
//! What must hold (each has unit tests here or in `share/tests.rs`):
//!
//! - **Loopback only.** The listener binds `127.0.0.1`, never another address.
//! - **Checks before anything is read or revealed.** In this order and before the body is
//!   read: the request target is a plain path (no scheme or host in it), `Host` is exactly
//!   `127.0.0.1:<port>` or `localhost:<port>` (DNS rebinding), there is no `Origin` header
//!   (web pages), and the sharing key (`x-api-key` or `Authorization: Bearer`, each at most
//!   once) is in one of them, compared in constant time. Refused requests are counted in
//!   memory only, never audited, and their connection is closed; over
//!   [`REFUSALS_PER_SECOND`] they are answered with a bare 429.
//! - **Connection slots** ([`MAX_CONNECTIONS`]): when all are taken, the oldest connection
//!   that never sent the key is closed to make room, so a local program without the key
//!   cannot keep apps out by opening connections and leaving them idle.
//! - **Exact paths only**: `POST /v1/messages`, `POST /v1/messages/count_tokens`,
//!   `POST /v1/responses`, `POST /v1/chat/completions`, and `GET /v1/models`, which the
//!   proxy answers itself with [`MODEL_ALIAS`]. The path is compared byte for byte, so
//!   `..`, doubled slashes and percent-encoding never match. A query string is ignored and
//!   never forwarded: the upstream path is a constant per API (Claude Code adds
//!   `?beta=true` to its Messages requests, and its model discovery asks for `?limit=`).
//! - **Requests are parsed and rebuilt**: at most [`MAX_BODY`] bytes, a JSON object, only
//!   the top-level fields each public API defines (llama.cpp extras such as `grammar`,
//!   `n_probs`, `id_slot`, `lora` or `cache_prompt` are dropped), the output limit capped
//!   at [`MAX_OUTPUT_TOKENS`] and the model's context, the model name replaced by the
//!   alias, and every image, audio, video or file inline: a `data:` URL or base64, never a
//!   link or a file id, in all three API shapes and in every field llama-server's media
//!   loader reads (round 1: `input_audio`, `input_video` and `video_url` too).
//! - **WebP pictures become JPEG or PNG** (0.0.6 webp-other-paths): llama-server reads WebP
//!   only through ffmpeg, which the app never lets it start, so every WebP picture (known
//!   by its first bytes, whatever type it is given) is turned into JPEG or PNG in the
//!   picture worker (`attachments.rs`: its own process, with its memory, size and time
//!   limits) after the checks above and before forwarding. At most [`MAX_WEBP_PICTURES`] a
//!   request and [`MAX_CONVERTED`] bytes of converted pictures in all; one that cannot be
//!   turned, or pictures too large together, refuse the request with a plain message in the
//!   API's own shape.
//! - **Forwarded with a fixed header list** (`authorization` with the private key,
//!   `content-type`, `accept`) through a client with no system proxy and no redirects.
//! - **Answers are cleaned**: `timings`, `system_fingerprint` and cached-token counters are
//!   removed from JSON and from every SSE event (they would show how much of a prompt
//!   matches what the in-app chat sent last), the model name is the alias, and the model
//!   file's path never appears. Errors come back in each API's own shape. The audit row's
//!   token counts come from the usage object, or else from the removed `timings`.
//! - **One audit row per authenticated generation request** (server `other-apps`, the
//!   path, what the caller says it is, token counts and the result; never the request
//!   text), `count_tokens` at most once a minute. The decision is the existing
//!   `allowed_without_prompt` (D9). A request the proxy answers itself also has its row
//!   (round 1), result `not_run` with the reason: `refused_by_policy` when one of its rules
//!   refused it (too large, too slow, not JSON, a link instead of inline data), else
//!   `allowed_without_prompt` (no model loaded, still loading).
//! - **The busy counter** ([`BusyKind::OtherApp`]) is held while a request runs (D3), from
//!   before the model is looked up, so turning its WebP pictures counts too; a model
//!   stopped or changed meanwhile is never sent the request. The in-app chat is never
//!   blocked by it. A caller that leaves a streamed answer unread for [`CALLER_STALL`] is
//!   cut off, so an app that hangs cannot hold it for ever.
//! - **The approval mode changes nothing here** (D12): the proxy is not a tool action, reads
//!   no mode, and its switch is changed only by the user.

use std::collections::VecDeque;
use std::convert::Infallible;
use std::io;
use std::net::Ipv4Addr;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{HeaderMap, HeaderValue, Method, Request, Response, StatusCode, Uri, header};
use http_body_util::channel::Channel;
use http_body_util::combinators::BoxBody;
use http_body_util::{BodyExt, Full, Limited};
use hyper::body::Incoming;
use hyper_util::rt::{TokioIo, TokioTimer};
use serde_json::{Map, Value, json};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::state::{BusyCounter, BusyKind};

/// The model id every answer reports, whatever was loaded (the file's path never leaves).
pub const MODEL_ALIAS: &str = runtime::MODEL_ALIAS;
/// The audit log's server name for requests from other apps.
pub const AUDIT_SERVER: &str = "other-apps";
/// The largest request body read, and the largest non-streamed answer passed back.
pub const MAX_BODY: usize = 32 * 1024 * 1024;
/// The highest output limit forwarded (also never more than the model's context).
pub const MAX_OUTPUT_TOKENS: u64 = 32_768;
/// Refused requests answered normally per second; the rest get a bare 429.
pub const REFUSALS_PER_SECOND: u32 = 20;
/// Connections served at once. When all are taken, the oldest one that never sent the
/// sharing key is closed to make room (so a program without the key cannot keep apps out
/// by holding every one); when every one has sent the key, a new one is closed at once.
/// Kept well under the 256 open files a macOS app gets by default.
pub const MAX_CONNECTIONS: usize = 64;
/// How long a client may take to send its request line and headers.
const HEADER_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a client may take to send its body.
const BODY_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a caller may leave a streamed answer unread before it is cut off. Without it, an
/// app that hangs with its connection open would hold the busy counter for ever, and "Restart
/// when it ends" (D3) would never come. Short in tests, which wait for it.
#[cfg(not(test))]
pub const CALLER_STALL: Duration = Duration::from_secs(60);
#[cfg(test)]
pub const CALLER_STALL: Duration = Duration::from_secs(2);
/// How long the proxy waits for llama-server to accept a connection.
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// `count_tokens` writes at most one audit row per this long.
const COUNT_TOKENS_AUDIT_EVERY: Duration = Duration::from_secs(60);
/// The longest line of an upstream event stream kept while waiting for its end.
const MAX_SSE_LINE: usize = MAX_BODY;

/// "No model is loaded" in the words of the spec (each API wraps it in its own shape).
pub const NO_MODEL: &str = "No model is loaded in harnessUI. Open harnessUI and load a model.";
/// "The model is still loading".
pub const LOADING: &str = "The model in harnessUI is still loading. Try again in a minute.";
/// The server a request was meant for did not answer, or was stopped before it was sent.
const NOT_ANSWERED: &str = "The model in harnessUI did not answer. Try again.";

type Body = BoxBody<Bytes, io::Error>;

// ---- APIs and paths ----------------------------------------------------------------

/// The APIs the proxy forwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Api {
    /// Anthropic Messages (`POST /v1/messages`).
    Messages,
    /// Anthropic token counting (`POST /v1/messages/count_tokens`).
    CountTokens,
    /// OpenAI Responses (`POST /v1/responses`).
    Responses,
    /// OpenAI Chat Completions (`POST /v1/chat/completions`).
    ChatCompletions,
}

impl Api {
    /// The API a path names, compared byte for byte.
    pub fn from_path(path: &str) -> Option<Self> {
        match path {
            "/v1/messages" => Some(Self::Messages),
            "/v1/messages/count_tokens" => Some(Self::CountTokens),
            "/v1/responses" => Some(Self::Responses),
            "/v1/chat/completions" => Some(Self::ChatCompletions),
            _ => None,
        }
    }

    /// The constant upstream path (never anything the client sent).
    pub fn upstream_path(self) -> &'static str {
        match self {
            Self::Messages => "/v1/messages",
            Self::CountTokens => "/v1/messages/count_tokens",
            Self::Responses => "/v1/responses",
            Self::ChatCompletions => "/v1/chat/completions",
        }
    }

    /// Errors in the Anthropic shape rather than the OpenAI one.
    fn anthropic(self) -> bool {
        matches!(self, Self::Messages | Self::CountTokens)
    }

    /// Generates text (as opposed to counting tokens).
    pub fn generates(self) -> bool {
        !matches!(self, Self::CountTokens)
    }

    /// A name for the audit preview.
    fn label(self) -> &'static str {
        match self {
            Self::Messages => "Messages",
            Self::CountTokens => "token counting",
            Self::Responses => "Responses",
            Self::ChatCompletions => "Chat Completions",
        }
    }

    /// The top-level request fields the public API defines and llama-server can use.
    /// Everything else is dropped: llama.cpp's own extras, and public fields that would
    /// only carry the caller's identifiers or references to stored state.
    fn fields(self) -> &'static [&'static str] {
        match self {
            Self::Messages => &[
                "model",
                "messages",
                "max_tokens",
                "system",
                "stop_sequences",
                "stream",
                "temperature",
                "top_k",
                "top_p",
                "tools",
                "tool_choice",
                "thinking",
            ],
            Self::CountTokens => &[
                "model",
                "messages",
                "system",
                "tools",
                "tool_choice",
                "thinking",
            ],
            Self::Responses => &[
                "model",
                "input",
                "instructions",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "stream",
                "max_output_tokens",
                "temperature",
                "top_p",
                "reasoning",
                "text",
                "top_logprobs",
            ],
            Self::ChatCompletions => &[
                "model",
                "messages",
                "max_tokens",
                "max_completion_tokens",
                "temperature",
                "top_p",
                "stop",
                "stream",
                "stream_options",
                "presence_penalty",
                "frequency_penalty",
                "logit_bias",
                "logprobs",
                "top_logprobs",
                "response_format",
                "seed",
                "tools",
                "tool_choice",
                "parallel_tool_calls",
                "reasoning_effort",
            ],
        }
    }

    /// The output-limit fields of this API; the first is added when none is sent.
    fn limit_fields(self) -> &'static [&'static str] {
        match self {
            Self::Messages => &["max_tokens"],
            Self::CountTokens => &[],
            Self::Responses => &["max_output_tokens"],
            Self::ChatCompletions => &["max_tokens", "max_completion_tokens"],
        }
    }
}

/// What a request asks for, after the checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Models,
    Api(Api),
    MethodNotAllowed,
    NotFound,
}

/// The route of a method and a path (exact match only).
pub fn route(method: &Method, path: &str) -> Route {
    if path == "/v1/models" {
        return if method == Method::GET {
            Route::Models
        } else {
            Route::MethodNotAllowed
        };
    }
    match Api::from_path(path) {
        Some(api) if method == Method::POST => Route::Api(api),
        Some(_) => Route::MethodNotAllowed,
        None => Route::NotFound,
    }
}

// ---- Checks before the body ----------------------------------------------------------

/// Why a request was refused before its body was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The request target names a scheme or a host, or `Host` is missing, repeated or not
    /// this port on the loopback address.
    Host,
    /// The request carries an `Origin` header (it comes from a web page).
    Origin,
    /// No sharing key, a wrong one, or more than one.
    Key,
}

/// The `Host` and `Origin` checks, and that the target is a plain path.
pub fn check_target(uri: &Uri, headers: &HeaderMap, port: u16) -> Result<(), Refused> {
    if uri.scheme().is_some() || uri.authority().is_some() {
        return Err(Refused::Host);
    }
    let mut hosts = headers.get_all(header::HOST).iter();
    let (Some(host), None) = (hosts.next(), hosts.next()) else {
        return Err(Refused::Host);
    };
    let Ok(host) = host.to_str() else {
        return Err(Refused::Host);
    };
    let loopback = format!("127.0.0.1:{port}");
    let named = format!("localhost:{port}");
    if host != loopback && !host.eq_ignore_ascii_case(&named) {
        return Err(Refused::Host);
    }
    if headers.contains_key(header::ORIGIN) {
        return Err(Refused::Origin);
    }
    Ok(())
}

/// Compares two byte strings in time that depends only on their lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

/// The sharing key check: `x-api-key: <key>` or `Authorization: Bearer <key>`, each header
/// at most once, and one of them must hold the key. The other may hold something else:
/// Claude Code sends both when the user also has `ANTHROPIC_API_KEY` set (that key goes in
/// `x-api-key`, the sharing key from `ANTHROPIC_AUTH_TOKEN` in `Authorization`; seen with
/// 2.1.322). That other value is never used, logged, audited or forwarded. An empty expected
/// key (none made yet) matches nothing.
pub fn key_matches(headers: &HeaderMap, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let mut sent = Vec::new();
    for name in [
        header::HeaderName::from_static("x-api-key"),
        header::AUTHORIZATION,
    ] {
        let mut values = headers.get_all(&name).iter();
        match (values.next(), values.next()) {
            (None, _) => {}
            (Some(value), None) => {
                let bytes = value.as_bytes();
                if name == header::AUTHORIZATION {
                    // Another scheme never holds the sharing key.
                    if let Some(token) = bearer_token(bytes) {
                        sent.push(token);
                    }
                } else {
                    sent.push(bytes);
                }
            }
            (Some(_), Some(_)) => return false,
        }
    }
    // Every comparison runs, so the time does not tell which header held the key.
    sent.iter().fold(false, |ok, key| {
        constant_time_eq(key, expected.as_bytes()) | ok
    })
}

/// The token of `Bearer <token>` (the scheme in any case, one space).
fn bearer_token(value: &[u8]) -> Option<&[u8]> {
    let (scheme, token) = (value.get(..7)?, value.get(7..)?);
    scheme.eq_ignore_ascii_case(b"bearer ").then_some(token)
}

// ---- Rebuilding requests -------------------------------------------------------------

/// Why a request body was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadRequest {
    NotJson,
    NotAnObject,
    /// An image or file given as a link, a file id or anything but inline data.
    NotInline,
    /// An output limit that is not a whole number.
    BadLimit,
    /// `stream` that is not true or false.
    BadStream,
}

impl BadRequest {
    fn message(self) -> &'static str {
        match self {
            Self::NotJson => "The request body is not valid JSON.",
            Self::NotAnObject => "The request body must be a JSON object.",
            Self::NotInline => {
                "Images and files must be sent inline (a data: URL or base64), not as a link or a file id."
            }
            Self::BadLimit => "The output token limit must be a whole number.",
            Self::BadStream => "stream must be true or false.",
        }
    }

    /// What the audit row says (never anything from the request itself).
    fn audit_summary(self) -> &'static str {
        match self {
            Self::NotJson => "Refused: the request was not valid JSON.",
            Self::NotAnObject => "Refused: the request was not a JSON object.",
            Self::NotInline => {
                "Refused: an image, audio, video or file was sent as a link or a file id, not inline."
            }
            Self::BadLimit => "Refused: the output token limit was not a whole number.",
            Self::BadStream => "Refused: stream was not true or false.",
        }
    }
}

/// A request ready to forward.
#[derive(Debug, Clone, PartialEq)]
pub struct Rebuilt {
    pub body: Value,
    pub stream: bool,
}

/// Parses `body` and rebuilds it for `api` (see the module header). `n_ctx` is the loaded
/// model's context, the second cap of the output limit.
pub fn rebuild_request(api: Api, body: &[u8], n_ctx: u32) -> Result<Rebuilt, BadRequest> {
    let parsed: Value = serde_json::from_slice(body).map_err(|_| BadRequest::NotJson)?;
    let Value::Object(mut sent) = parsed else {
        return Err(BadRequest::NotAnObject);
    };
    let mut out = Map::new();
    for field in api.fields() {
        if let Some(value) = sent.remove(*field) {
            out.insert((*field).to_owned(), value);
        }
    }
    check_inline(api, &out)?;
    let stream = match out.get("stream") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(BadRequest::BadStream),
    };
    let cap = MAX_OUTPUT_TOKENS.min(u64::from(n_ctx.max(1)));
    let mut limited = false;
    for field in api.limit_fields() {
        match out.get(*field) {
            None | Some(Value::Null) => {
                out.remove(*field);
            }
            Some(value) => {
                let n = value.as_u64().ok_or(BadRequest::BadLimit)?;
                out.insert((*field).to_owned(), json!(n.min(cap)));
                limited = true;
            }
        }
    }
    if !limited && let Some(first) = api.limit_fields().first() {
        out.insert((*first).to_owned(), json!(cap));
    }
    out.insert("model".into(), json!(MODEL_ALIAS));
    Ok(Rebuilt {
        body: Value::Object(out),
        stream,
    })
}

/// Every image and file in the request must be inline.
fn check_inline(api: Api, req: &Map<String, Value>) -> Result<(), BadRequest> {
    match api {
        Api::Messages | Api::CountTokens => {
            check_parts(req.get("system"))?;
            for message in array(req.get("messages")) {
                check_parts(message.get("content"))?;
            }
        }
        Api::ChatCompletions => {
            for message in array(req.get("messages")) {
                check_parts(message.get("content"))?;
            }
        }
        Api::Responses => check_parts(req.get("input"))?,
    }
    Ok(())
}

fn array(value: Option<&Value>) -> &[Value] {
    value.and_then(Value::as_array).map_or(&[], Vec::as_slice)
}

/// Checks a content value: a string (nothing to check) or an array of parts.
fn check_parts(content: Option<&Value>) -> Result<(), BadRequest> {
    for part in array(content) {
        check_part(part)?;
    }
    Ok(())
}

/// One content part, item or block, in any of the three shapes:
/// - OpenAI Chat: `{"type":"image_url","image_url":{"url":…}}`, `{"type":"file","file":{…}}`;
/// - OpenAI Responses: `{"type":"input_image","image_url":…}`, `{"type":"input_file",…}`,
///   messages with `content`, `function_call_output` with an `output` array;
/// - Anthropic: `{"type":"image"|"document","source":{"type":…}}`, `tool_result` with a
///   `content` array, a document whose source is `content`.
///
/// Tool-call arguments (`input`, `arguments`) are never looked into: an address there is
/// the model's own text, not something llama-server would fetch.
fn check_part(part: &Value) -> Result<(), BadRequest> {
    let Some(obj) = part.as_object() else {
        return Ok(());
    };
    if obj.contains_key("file_id") || obj.contains_key("file_url") {
        return Err(BadRequest::NotInline);
    }
    for key in MEDIA_KEYS {
        if let Some(value) = obj.get(key) {
            check_media(value)?;
        }
    }
    if let Some(file) = obj.get("file").and_then(Value::as_object)
        && (file.contains_key("file_id") || file.contains_key("file_url"))
    {
        return Err(BadRequest::NotInline);
    }
    if let Some(source) = obj.get("source").and_then(Value::as_object) {
        match source.get("type").and_then(Value::as_str) {
            Some("base64" | "text") => {}
            Some("content") => check_parts(source.get("content"))?,
            _ => return Err(BadRequest::NotInline),
        }
    }
    for key in ["content", "output"] {
        check_parts(obj.get(key))?;
    }
    Ok(())
}

/// The keys of a content part that hold an image, audio or video. llama-server's media
/// loader (b12110, `oaicompat_content_load_media`) reads `image_url.url`, and `data`, else
/// `url`, of `input_audio`, `input_video` and `video_url`, and downloads any value that
/// starts with `http` (its `--offline` flag does not stop that). `audio_url` is no shape it
/// reads, and is held to the same rule.
const MEDIA_KEYS: [&str; 5] = [
    "image_url",
    "input_audio",
    "input_video",
    "video_url",
    "audio_url",
];

/// One media value: a string, or an object whose `url` and `data` (at least one) are
/// strings. Every string llama-server may read must be inline. A `url` or `data` that is
/// not a string is refused too: llama-server would fall back to the other one.
fn check_media(value: &Value) -> Result<(), BadRequest> {
    match value {
        Value::String(s) if is_inline(s) => Ok(()),
        Value::Object(obj) => {
            let mut found = false;
            for key in ["url", "data"] {
                match obj.get(key) {
                    None => {}
                    Some(Value::String(s)) if is_inline(s) => found = true,
                    Some(_) => return Err(BadRequest::NotInline),
                }
            }
            if found {
                Ok(())
            } else {
                Err(BadRequest::NotInline)
            }
        }
        _ => Err(BadRequest::NotInline),
    }
}

/// A `data:` URL, or plain base64 (llama-server decodes such a value as it is; it fetches
/// only a value with `://` in it, which base64 never has).
fn is_inline(value: &str) -> bool {
    is_data_url(value)
        || (!value.trim().is_empty()
            && value.bytes().all(|b| {
                b.is_ascii_alphanumeric() || b"+/=".contains(&b) || b.is_ascii_whitespace()
            }))
}

fn is_data_url(url: &str) -> bool {
    url.get(..5)
        .is_some_and(|s| s.eq_ignore_ascii_case("data:"))
}

// ---- WebP pictures (0.0.6 webp-other-paths) ------------------------------------------

/// The most WebP pictures one request may hold; each is turned into JPEG or PNG in the
/// picture worker before the request is forwarded.
pub const MAX_WEBP_PICTURES: usize = 32;
/// What one request's converted pictures may add up to, as base64.
pub const MAX_CONVERTED: usize = MAX_BODY;
/// How long turning one request's WebP pictures into JPEG or PNG may take in all (each
/// picture also has the worker's own time limit).
const PICTURES_TIMEOUT: Duration = Duration::from_secs(120);
/// Requests whose WebP pictures are turned at the same time; the others wait their turn, so
/// the workers cannot take the computer's memory between them.
const PICTURES_AT_ONCE: usize = 2;

/// Limits on turning one request's WebP pictures (smaller in tests).
#[derive(Debug, Clone, Copy)]
struct PictureLimits {
    count: usize,
    total: usize,
    time: Duration,
}

const PICTURE_LIMITS: PictureLimits = PictureLimits {
    count: MAX_WEBP_PICTURES,
    total: MAX_CONVERTED,
    time: PICTURES_TIMEOUT,
};

/// Why a request's WebP pictures were not turned into JPEG or PNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PictureRefusal {
    /// More than [`MAX_WEBP_PICTURES`].
    TooMany,
    /// Not valid base64, broken, larger than the picture worker's limits, too slow, or the
    /// worker failed.
    Unreadable,
    /// One picture larger than 20 MB once turned.
    TooLarge,
    /// The turned pictures over [`MAX_CONVERTED`] in all (one alone never is, as below).
    TooLargeTogether,
}

// One picture the worker gives back, as base64, always fits the request's total, so
// passing the total always takes more than one picture.
const _: () = assert!(crate::attachments::MAX_IMAGE_BYTES.div_ceil(3) * 4 < MAX_CONVERTED);

impl PictureRefusal {
    fn message(self) -> &'static str {
        match self {
            Self::TooMany => {
                "This request has more than 32 WebP pictures. Send PNG or JPEG pictures instead."
            }
            Self::Unreadable => {
                "A WebP picture in this request could not be read. Send a PNG or JPEG instead."
            }
            Self::TooLarge => {
                "A WebP picture in this request is too large to turn into PNG or JPEG. Send a smaller picture, or a PNG or JPEG."
            }
            Self::TooLargeTogether => {
                "The WebP pictures in this request are too large together once turned into PNG or JPEG. Send fewer or smaller pictures."
            }
        }
    }

    /// What the audit row says (never anything from the request itself).
    fn audit_summary(self) -> &'static str {
        match self {
            Self::TooMany => "Refused: more than 32 WebP pictures.",
            Self::Unreadable => "Refused: a WebP picture could not be read.",
            Self::TooLarge => "Refused: a WebP picture was too large to turn into PNG or JPEG.",
            Self::TooLargeTogether => {
                "Refused: the WebP pictures were too large together once turned into PNG or JPEG."
            }
        }
    }
}

/// A place in a request where llama-server reads a picture from.
enum PictureSlot<'a> {
    /// `image_url` (OpenAI Chat's `url` in it, or OpenAI Responses' string): a `data:` URL
    /// or plain base64.
    Url(&'a mut String),
    /// An Anthropic `source` of type `base64` (`media_type` and `data`).
    Source(&'a mut Map<String, Value>),
}

impl PictureSlot<'_> {
    /// The base64 of the picture here when it is a WebP file (by its bytes).
    fn webp(&self) -> Option<&str> {
        let data = match self {
            Self::Url(url) if is_data_url(url) => {
                let (head, data) = url.split_once(',')?;
                let base64 = head
                    .get(head.len().saturating_sub(6)..)
                    .is_some_and(|end| end.eq_ignore_ascii_case("base64"));
                base64.then_some(data)?
            }
            Self::Url(url) => url.as_str(),
            Self::Source(source) => source.get("data")?.as_str()?,
        };
        crate::attachments::base64_is_webp(data).then_some(data)
    }

    /// Puts the turned picture here.
    fn replace(self, mime: &str, data: String) {
        match self {
            Self::Url(url) => *url = format!("data:{mime};base64,{data}"),
            Self::Source(source) => {
                source.insert("media_type".into(), json!(mime));
                source.insert("data".into(), Value::String(data));
            }
        }
    }
}

/// Calls `f` on every picture slot of `req`, wherever [`check_inline`] looks, in the same
/// order every time.
fn each_picture(api: Api, req: &mut Value, f: &mut dyn FnMut(PictureSlot<'_>)) {
    let Some(req) = req.as_object_mut() else {
        return;
    };
    if api == Api::Responses {
        picture_parts(req.get_mut("input"), f);
        return;
    }
    if api.anthropic() {
        picture_parts(req.get_mut("system"), f);
    }
    if let Some(Value::Array(messages)) = req.get_mut("messages") {
        for message in messages {
            picture_parts(message.get_mut("content"), f);
        }
    }
}

fn picture_parts(content: Option<&mut Value>, f: &mut dyn FnMut(PictureSlot<'_>)) {
    if let Some(Value::Array(parts)) = content {
        for part in parts {
            picture_part(part, f);
        }
    }
}

/// One content part, item or block ([`check_part`] lists the shapes).
fn picture_part(part: &mut Value, f: &mut dyn FnMut(PictureSlot<'_>)) {
    let Some(obj) = part.as_object_mut() else {
        return;
    };
    match obj.get_mut("image_url") {
        Some(Value::String(url)) => f(PictureSlot::Url(url)),
        Some(Value::Object(image)) => {
            if let Some(Value::String(url)) = image.get_mut("url") {
                f(PictureSlot::Url(url));
            }
        }
        _ => {}
    }
    if let Some(Value::Object(source)) = obj.get_mut("source") {
        let kind = source.get("type").and_then(Value::as_str);
        let (base64, nested) = (kind == Some("base64"), kind == Some("content"));
        if base64 {
            f(PictureSlot::Source(source));
        } else if nested {
            picture_parts(source.get_mut("content"), f);
        }
    }
    for key in ["content", "output"] {
        picture_parts(obj.get_mut(key), f);
    }
}

/// Turns every WebP picture in `req` into JPEG or PNG with the picture worker, in place
/// (see the module header); other pictures are left as they are. `at_once` holds back the
/// requests over [`PICTURES_AT_ONCE`]; a request without WebP pictures never waits.
pub async fn convert_webp(
    api: Api,
    req: &mut Value,
    worker: &crate::attachments::PictureWorker,
    at_once: &tokio::sync::Semaphore,
) -> Result<(), PictureRefusal> {
    convert_webp_within(api, req, worker, at_once, PICTURE_LIMITS).await
}

async fn convert_webp_within(
    api: Api,
    req: &mut Value,
    worker: &crate::attachments::PictureWorker,
    at_once: &tokio::sync::Semaphore,
    limits: PictureLimits,
) -> Result<(), PictureRefusal> {
    use crate::attachments::NotConverted;
    use base64::Engine;
    use base64::engine::general_purpose::STANDARD;
    // The WebP pictures, in order: their bytes, or `None` when the base64 does not decode.
    let mut found = Vec::new();
    let mut count = 0usize;
    each_picture(api, req, &mut |slot| {
        let Some(data) = slot.webp() else {
            return;
        };
        count += 1;
        if count <= limits.count {
            let compact: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            found.push(STANDARD.decode(compact).ok());
        }
    });
    if count > limits.count {
        return Err(PictureRefusal::TooMany);
    }
    if found.is_empty() {
        return Ok(());
    }
    let _turn = at_once.acquire().await;
    let turning = async {
        let mut turned = Vec::with_capacity(found.len());
        let mut total = 0usize;
        for bytes in found {
            let bytes = bytes.ok_or(PictureRefusal::Unreadable)?;
            let (mime, picture) = worker.convert(&bytes).await.map_err(|e| match e {
                NotConverted::TooLarge => PictureRefusal::TooLarge,
                NotConverted::Unreadable(_) => PictureRefusal::Unreadable,
            })?;
            let data = STANDARD.encode(picture);
            total = total.saturating_add(data.len());
            if total > limits.total {
                return Err(PictureRefusal::TooLargeTogether);
            }
            turned.push((mime, data));
        }
        Ok::<_, PictureRefusal>(turned)
    };
    let turned = tokio::time::timeout(limits.time, turning)
        .await
        .map_err(|_| PictureRefusal::Unreadable)??;
    // The same walk over the same request meets the same pictures in the same order.
    let mut turned = turned.into_iter();
    each_picture(api, req, &mut |slot| {
        if slot.webp().is_some()
            && let Some((mime, data)) = turned.next()
        {
            slot.replace(&mime, data);
        }
    });
    Ok(())
}

// ---- Cleaning answers --------------------------------------------------------------

/// Token counts seen in an answer, for the audit row.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: Option<u64>,
    pub output: Option<u64>,
}

impl Usage {
    fn merge(&mut self, other: Usage) {
        self.input = other.input.or(self.input);
        self.output = other.output.or(self.output);
    }

    /// These counts, and `other`'s where these have none.
    fn or(self, other: Usage) -> Usage {
        Usage {
            input: self.input.or(other.input),
            output: self.output.or(other.output),
        }
    }

    /// "1,234 tokens in, 56 out", or nothing when no count was seen.
    pub fn summary(self) -> Option<String> {
        match (self.input, self.output) {
            (None, None) => None,
            // Token counting: only what came in.
            (Some(input), None) => Some(format!("{} tokens in", thousands(input))),
            (input, output) => Some(format!(
                "{} tokens in, {} out",
                input.map_or_else(|| "?".to_owned(), thousands),
                output.map_or_else(|| "?".to_owned(), thousands)
            )),
        }
    }
}

/// 1234567 → "1,234,567".
pub fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Cleans one JSON answer or event in place and returns the token counts it carried.
/// Only known places are touched: the top level and the `message` (Anthropic
/// `message_start`) and `response` (OpenAI Responses events) objects.
pub fn clean_answer(value: &mut Value) -> Usage {
    let counts = clean_counted(value);
    counts.usage.or(counts.timed)
}

/// The counts one answer or event carried, kept apart until the end of a stream: a usage
/// object always wins over `timings`, also when they come in different events.
#[derive(Debug, Clone, Copy, Default)]
struct Counts {
    /// From usage objects.
    usage: Usage,
    /// From llama-server's `timings` (removed from the answer).
    timed: Usage,
}

fn clean_counted(value: &mut Value) -> Counts {
    let mut counts = Counts::default();
    let Some(obj) = value.as_object_mut() else {
        return counts;
    };
    clean_object(obj, &mut counts);
    for key in ["message", "response"] {
        if let Some(Value::Object(inner)) = obj.get_mut(key) {
            clean_object(inner, &mut counts);
        }
    }
    counts
}

fn clean_object(obj: &mut Map<String, Value>, counts: &mut Counts) {
    if let Some(Value::Object(timings)) = obj.remove("timings") {
        counts.timed.merge(timings_usage(&timings));
    }
    obj.remove("system_fingerprint");
    if let Some(model) = obj.get_mut("model")
        && model.is_string()
    {
        *model = json!(MODEL_ALIAS);
    }
    if let Some(Value::Object(usage)) = obj.get_mut("usage") {
        counts.usage.merge(clean_usage(usage));
    }
}

/// The counts in llama-server's `timings`, for the audit row only (the object itself never
/// goes back): the prompt is the cached part plus the rest, the answer is what was predicted.
fn timings_usage(timings: &Map<String, Value>) -> Usage {
    let number = |key: &str| timings.get(key).and_then(Value::as_u64);
    let input = match (number("cache_n"), number("prompt_n")) {
        (None, None) => None,
        (cached, rest) => Some(cached.unwrap_or(0) + rest.unwrap_or(0)),
    };
    Usage {
        input,
        output: number("predicted_n"),
    }
}

/// Removes what tells how much of the prompt was already cached. Anthropic counts cached
/// tokens apart from `input_tokens`, so they are added back in; OpenAI counts them inside
/// the total and in a details object, whose counter is set to 0.
fn clean_usage(counts: &mut Map<String, Value>) -> Usage {
    let mut cached = 0u64;
    let mut anthropic = false;
    for key in ["cache_read_input_tokens", "cache_creation_input_tokens"] {
        if let Some(value) = counts.remove(key) {
            anthropic = true;
            cached += value.as_u64().unwrap_or(0);
        }
    }
    counts.remove("cache_creation");
    if anthropic && let Some(input) = counts.get("input_tokens").and_then(Value::as_u64) {
        counts.insert("input_tokens".into(), json!(input + cached));
    }
    for details in ["prompt_tokens_details", "input_tokens_details"] {
        if let Some(Value::Object(d)) = counts.get_mut(details)
            && d.contains_key("cached_tokens")
        {
            d.insert("cached_tokens".into(), json!(0));
        }
    }
    let number = |keys: [&str; 2]| keys.iter().find_map(|k| counts.get(*k)?.as_u64());
    Usage {
        input: number(["input_tokens", "prompt_tokens"]),
        output: number(["output_tokens", "completion_tokens"]),
    }
}

/// Replaces the model file's path (and its file name) in text that goes back to the
/// caller, wherever llama-server might have put it.
#[derive(Debug, Clone, Default)]
pub struct Scrub {
    needles: Vec<String>,
}

impl Scrub {
    pub fn new(model: &Path) -> Self {
        let mut needles = Vec::new();
        let mut add = |text: &str| {
            if text.len() >= 3 {
                // As it appears inside a JSON string, and as it is.
                if let Ok(quoted) = serde_json::to_string(text) {
                    let inner = quoted.trim_matches('"').to_owned();
                    if inner != text {
                        needles.push(inner);
                    }
                }
                needles.push(text.to_owned());
            }
        };
        add(&model.to_string_lossy());
        if let Some(name) = model.file_name() {
            add(&name.to_string_lossy());
        }
        // Longest first, so the whole path goes before its file name.
        needles.sort_by_key(|n| std::cmp::Reverse(n.len()));
        needles.dedup();
        Self { needles }
    }

    pub fn apply(&self, text: String) -> String {
        let mut text = text;
        for needle in &self.needles {
            if text.contains(needle.as_str()) {
                text = text.replace(needle.as_str(), MODEL_ALIAS);
            }
        }
        text
    }
}

/// Cleans an upstream event stream line by line: `data:` lines are parsed as JSON and
/// cleaned (or passed when they are `[DONE]`), `event:` lines are kept when they are a
/// plain event name, blank lines end events, and everything else is dropped.
pub struct SseFilter {
    buf: Vec<u8>,
    scrub: Scrub,
    counts: Counts,
}

impl SseFilter {
    pub fn new(scrub: Scrub) -> Self {
        Self {
            buf: Vec::new(),
            scrub,
            counts: Counts::default(),
        }
    }

    /// The counts seen so far (from usage objects, else from the timings).
    pub fn usage(&self) -> Usage {
        self.counts.usage.or(self.counts.timed)
    }

    /// Takes the next chunk and returns the cleaned complete lines in it. Fails when a
    /// line grows beyond [`MAX_SSE_LINE`].
    pub fn push(&mut self, chunk: &[u8]) -> Result<String, ()> {
        self.buf.extend_from_slice(chunk);
        let mut out = String::new();
        while let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            self.line(&line[..line.len() - 1], &mut out);
        }
        if self.buf.len() > MAX_SSE_LINE {
            return Err(());
        }
        Ok(out)
    }

    /// The last line, if the stream did not end with a line break.
    pub fn finish(&mut self) -> String {
        let mut out = String::new();
        if !self.buf.is_empty() {
            let line = std::mem::take(&mut self.buf);
            self.line(&line, &mut out);
        }
        out
    }

    fn line(&mut self, line: &[u8], out: &mut String) {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let Ok(line) = std::str::from_utf8(line) else {
            return;
        };
        if line.is_empty() {
            out.push('\n');
        } else if let Some(data) = line.strip_prefix("data:") {
            let data = data.strip_prefix(' ').unwrap_or(data);
            if data == "[DONE]" {
                out.push_str("data: [DONE]\n");
            } else if let Ok(mut value) = serde_json::from_str::<Value>(data) {
                let counts = clean_counted(&mut value);
                self.counts.usage.merge(counts.usage);
                self.counts.timed.merge(counts.timed);
                out.push_str("data: ");
                out.push_str(&self.scrub.apply(value.to_string()));
                out.push('\n');
            }
        } else if let Some(name) = line.strip_prefix("event:") {
            let name = name.trim_start();
            if !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
            {
                out.push_str("event: ");
                out.push_str(name);
                out.push('\n');
            }
        }
    }
}

// ---- Error answers -----------------------------------------------------------------

/// An error body in the shape of `api` (OpenAI's when the path is not one of them).
pub fn error_body(
    api: Option<Api>,
    status: StatusCode,
    message: &str,
    code: Option<&str>,
) -> Value {
    if api.is_some_and(Api::anthropic) {
        let kind = match status.as_u16() {
            400 | 405 | 408 => "invalid_request_error",
            401 => "authentication_error",
            403 => "permission_error",
            404 => "not_found_error",
            413 => "request_too_large",
            429 => "rate_limit_error",
            503 | 529 => "overloaded_error",
            _ => "api_error",
        };
        json!({"type": "error", "error": {"type": kind, "message": message}})
    } else {
        let kind = match status.as_u16() {
            400..=499 => "invalid_request_error",
            _ => "server_error",
        };
        json!({"error": {"message": message, "type": kind, "param": null, "code": code}})
    }
}

/// An upstream error rebuilt in the shape of `api`: only its message passes, sanitised,
/// cut and with the model's path removed. A full context is reported in the wording each
/// client recognises, so it can shorten its conversation and try again.
pub fn upstream_error(
    api: Api,
    status: StatusCode,
    body: &[u8],
    scrub: &Scrub,
) -> (StatusCode, Value) {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let error = parsed.get("error").unwrap_or(&parsed);
    let kind = error
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let status = if status.is_client_error() || status.is_server_error() {
        status
    } else {
        StatusCode::BAD_GATEWAY
    };
    if kind == "exceed_context_size_error" {
        let prompt = error
            .get("n_prompt_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let ctx = error.get("n_ctx").and_then(Value::as_u64).unwrap_or(0);
        let message = if api.anthropic() {
            format!("prompt is too long: {prompt} tokens > {ctx} maximum")
        } else {
            format!(
                "This model's maximum context length is {ctx} tokens. However, your messages resulted in {prompt} tokens."
            )
        };
        let body = error_body(
            Some(api),
            StatusCode::BAD_REQUEST,
            &message,
            Some("context_length_exceeded"),
        );
        return (StatusCode::BAD_REQUEST, body);
    }
    let message = error
        .get("message")
        .and_then(Value::as_str)
        .filter(|m| !m.trim().is_empty())
        .map(|m| policy::sanitize::one_line(&scrub.apply(m.to_owned()), 500))
        .unwrap_or_else(|| "The model could not answer this request.".to_owned());
    (status, error_body(Some(api), status, &message, None))
}

fn json_response(status: StatusCode, value: &Value) -> Response<Body> {
    let mut response = Response::new(full(Bytes::from(value.to_string())));
    *response.status_mut() = status;
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn full(bytes: Bytes) -> Body {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

/// The answer of `GET /v1/models`, readable as an OpenAI and as an Anthropic model list.
pub fn models_body() -> Value {
    json!({
        "object": "list",
        "data": [{
            "id": MODEL_ALIAS,
            "object": "model",
            "type": "model",
            "display_name": "The model loaded in harnessUI",
            "created": 0,
            "created_at": "1970-01-01T00:00:00Z",
            "owned_by": "harnessui"
        }],
        "has_more": false,
        "first_id": MODEL_ALIAS,
        "last_id": MODEL_ALIAS
    })
}

// ---- What the proxy needs from the app ----------------------------------------------

/// Where the loaded model is, or why there is none.
#[derive(Debug, Clone, PartialEq)]
pub enum Upstream {
    Ready {
        base_url: String,
        /// llama-server's private key (never the sharing key).
        key: String,
        model: std::path::PathBuf,
        n_ctx: u32,
    },
    Loading,
    NotLoaded,
}

/// The app's loaded model (the runtime manager; a fake in tests).
#[async_trait::async_trait]
pub trait ModelSource: Send + Sync + 'static {
    async fn current(&self) -> Upstream;
}

/// Counts kept in memory only.
#[derive(Default)]
pub struct Counters {
    refused: AtomicU64,
    active: AtomicU32,
    /// The second refusals are counted in and how many were answered in it.
    window: Mutex<Option<(Instant, u32)>>,
    last_count_audit: Mutex<Option<Instant>>,
    last_refusal_log: Mutex<Option<Instant>>,
}

impl Counters {
    /// Requests refused since the counter was last reset.
    pub fn refused(&self) -> u64 {
        self.refused.load(Ordering::Relaxed)
    }

    /// Requests being answered now.
    pub fn active(&self) -> u32 {
        self.active.load(Ordering::Relaxed)
    }

    /// Back to zero (sharing switched on).
    pub fn reset_refused(&self) {
        self.refused.store(0, Ordering::Relaxed);
    }

    /// Counts one refusal; false when this second's allowance is used up.
    fn refuse(&self) -> bool {
        self.refused.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let mut window = self.window.lock().unwrap_or_else(|e| e.into_inner());
        match window.as_mut() {
            Some((start, n)) if now.duration_since(*start) < Duration::from_secs(1) => {
                *n += 1;
                *n <= REFUSALS_PER_SECOND
            }
            _ => {
                *window = Some((now, 1));
                true
            }
        }
    }

    /// True once per `every`, for rate-limited logging and audit rows.
    fn due(slot: &Mutex<Option<Instant>>, every: Duration) -> bool {
        let now = Instant::now();
        let mut last = slot.lock().unwrap_or_else(|e| e.into_inner());
        if last.is_some_and(|t| now.duration_since(t) < every) {
            return false;
        }
        *last = Some(now);
        true
    }
}

/// Counts one request being answered while it lives.
struct Active(Arc<Counters>);

impl Active {
    fn new(counters: &Arc<Counters>) -> Self {
        counters.active.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(counters))
    }
}

impl Drop for Active {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Everything one listening proxy uses.
pub struct Deps {
    pub port: u16,
    /// The sharing key now; replacing it takes effect on the next request.
    pub key: Arc<RwLock<String>>,
    pub model: Arc<dyn ModelSource>,
    pub store: store::Store,
    pub busy: Arc<BusyCounter>,
    pub counters: Arc<Counters>,
    /// Turns WebP pictures into JPEG or PNG (0.0.6 webp-other-paths).
    pub pictures: crate::attachments::PictureWorker,
}

struct Shared {
    deps: Deps,
    client: reqwest::Client,
    /// Requests whose WebP pictures are being turned ([`PICTURES_AT_ONCE`]).
    pictures_at_once: tokio::sync::Semaphore,
}

impl Shared {
    fn key(&self) -> String {
        self.deps.key.read().map(|k| k.clone()).unwrap_or_default()
    }

    /// Counts a refusal and answers it (a bare 429 over the per-second allowance).
    fn refuse(&self, api: Option<Api>, why: Refused) -> Response<Body> {
        if Counters::due(
            &self.deps.counters.last_refusal_log,
            Duration::from_secs(60),
        ) {
            tracing::warn!(reason = ?why, "the sharing port refused a request");
        }
        if !self.deps.counters.refuse() {
            let mut response = Response::new(full(Bytes::new()));
            *response.status_mut() = StatusCode::TOO_MANY_REQUESTS;
            response
                .headers_mut()
                .insert(header::CONNECTION, HeaderValue::from_static("close"));
            return response;
        }
        let (status, message) = match why {
            Refused::Host | Refused::Origin => (StatusCode::FORBIDDEN, "Forbidden."),
            Refused::Key => (StatusCode::UNAUTHORIZED, "Invalid API key."),
        };
        let code = (why == Refused::Key).then_some("invalid_api_key");
        let mut response = json_response(status, &error_body(api, status, message, code));
        // A refused caller does not keep its connection (round 1).
        response
            .headers_mut()
            .insert(header::CONNECTION, HeaderValue::from_static("close"));
        response
    }
}

// ---- The server --------------------------------------------------------------------

/// Binds the loopback port (never another address).
pub async fn bind(port: u16) -> io::Result<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await
}

/// A listening proxy.
pub struct Running {
    port: u16,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Running {
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The accept loop ended by itself (it never should).
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Stops listening and closes every connection; the port is free when this returns.
    pub async fn stop(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

/// Serves `listener` until stopped.
pub fn serve(listener: TcpListener, deps: Deps) -> io::Result<Running> {
    let port = listener.local_addr()?.port();
    let client = runtime::llama::server::loopback_client_builder()
        .connect_timeout(UPSTREAM_CONNECT_TIMEOUT)
        .build()
        .map_err(io::Error::other)?;
    let shared = Arc::new(Shared {
        deps,
        client,
        pictures_at_once: tokio::sync::Semaphore::new(PICTURES_AT_ONCE),
    });
    let cancel = CancellationToken::new();
    let task = tokio::spawn(accept_loop(listener, shared, cancel.clone()));
    Ok(Running { port, cancel, task })
}

/// One connection the accept loop serves, as the loop sees it.
struct Slot {
    /// Set once a request on it carried the sharing key.
    keyed: Arc<AtomicBool>,
    /// Set when it has ended.
    done: Arc<AtomicBool>,
    /// Ends it.
    cancel: CancellationToken,
}

/// Makes room for one more connection: forgets the ones that ended and, when every slot is
/// taken, closes the oldest one that never sent the sharing key. False when every slot
/// holds a connection that sent the key (the new one is then closed).
fn make_room(slots: &mut VecDeque<Slot>) -> bool {
    slots.retain(|s| !s.done.load(Ordering::Relaxed));
    if slots.len() < MAX_CONNECTIONS {
        return true;
    }
    match slots.iter().position(|s| !s.keyed.load(Ordering::Relaxed)) {
        Some(oldest) => {
            if let Some(slot) = slots.remove(oldest) {
                slot.cancel.cancel();
            }
            true
        }
        None => false,
    }
}

async fn accept_loop(listener: TcpListener, shared: Arc<Shared>, cancel: CancellationToken) {
    let mut slots = VecDeque::new();
    let mut connections = tokio::task::JoinSet::new();
    loop {
        let accepted = tokio::select! {
            () = cancel.cancelled() => break,
            accepted = listener.accept() => accepted,
        };
        while connections.try_join_next().is_some() {}
        let (stream, peer) = match accepted {
            Ok(pair) => pair,
            Err(e) => {
                // Out of file handles and similar: wait a moment instead of spinning.
                tracing::warn!("the sharing port could not accept a connection: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        if !peer.ip().is_loopback() {
            continue;
        }
        if !make_room(&mut slots) {
            continue;
        }
        let slot = Slot {
            keyed: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicBool::new(false)),
            cancel: cancel.child_token(),
        };
        let (keyed, done, cancel) = (
            Arc::clone(&slot.keyed),
            Arc::clone(&slot.done),
            slot.cancel.clone(),
        );
        slots.push_back(slot);
        let shared = Arc::clone(&shared);
        connections.spawn(async move {
            let service = hyper::service::service_fn(move |req| {
                let shared = Arc::clone(&shared);
                let keyed = Arc::clone(&keyed);
                async move { Ok::<_, Infallible>(handle(req, &shared, &keyed).await) }
            });
            let mut builder = hyper::server::conn::http1::Builder::new();
            builder
                .timer(TokioTimer::new())
                .header_read_timeout(HEADER_TIMEOUT)
                .max_buf_size(64 * 1024)
                .max_headers(64);
            let connection = builder.serve_connection(TokioIo::new(stream), service);
            tokio::select! {
                _ = connection => {}
                () = cancel.cancelled() => {}
            }
            done.store(true, Ordering::Relaxed);
        });
    }
    drop(listener);
    connections.abort_all();
    while connections.join_next().await.is_some() {}
}

async fn handle(req: Request<Incoming>, shared: &Shared, keyed: &AtomicBool) -> Response<Body> {
    let path = req.uri().path().to_owned();
    let api = Api::from_path(&path);
    let port = shared.deps.port;
    if let Err(why) = check_target(req.uri(), req.headers(), port) {
        return shared.refuse(api, why);
    }
    if !key_matches(req.headers(), &shared.key()) {
        return shared.refuse(api, Refused::Key);
    }
    // This connection is now never closed to make room for another.
    keyed.store(true, Ordering::Relaxed);
    match route(req.method(), &path) {
        Route::Models => json_response(StatusCode::OK, &models_body()),
        Route::Api(api) => forward(req, api, shared).await,
        Route::MethodNotAllowed => {
            let status = StatusCode::METHOD_NOT_ALLOWED;
            json_response(
                status,
                &error_body(api, status, "This method is not allowed here.", None),
            )
        }
        Route::NotFound => {
            let status = StatusCode::NOT_FOUND;
            json_response(
                status,
                &error_body(api, status, "There is nothing at this address.", None),
            )
        }
    }
}

/// What the caller says it is: its `User-Agent`, sanitised, at most 40 characters.
pub fn says(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .map(|v| String::from_utf8_lossy(v.as_bytes()).trim().to_owned())
        .filter(|s| !s.is_empty())
        .map(|s| policy::sanitize::one_line(&s, 40))
        .unwrap_or_default()
}

/// [0.0.6 webp-other-paths, last round] How often a request whose WebP pictures are being
/// turned, or which waits its turn, looks again whether its model is still the loaded one.
const MODEL_RECHECK: Duration = Duration::from_millis(500);

/// [0.0.6 webp-other-paths, last round] The loaded model is no longer the one a request was
/// looked up on: it was stopped, or another one was loaded, while the request waited.
enum ModelMoved {
    Loading,
    NotLoaded,
    Changed,
}

impl ModelMoved {
    /// `now` against the server `base_url` and `key` name, or `None` while it is still that
    /// same server with that same key.
    fn seen(now: Upstream, base_url: &str, key: &str) -> Option<Self> {
        match now {
            Upstream::Ready {
                base_url: at,
                key: with,
                ..
            } if at == base_url && with == key => None,
            Upstream::Ready { .. } => Some(Self::Changed),
            Upstream::Loading => Some(Self::Loading),
            Upstream::NotLoaded => Some(Self::NotLoaded),
        }
    }

    /// What the caller is told, and what its audit row says.
    fn reply(self) -> (StatusCode, &'static str, Option<&'static str>, NotForwarded) {
        match self {
            Self::Changed => (
                StatusCode::BAD_GATEWAY,
                NOT_ANSWERED,
                None,
                NotForwarded::NotRun("The model was changed while the request waited."),
            ),
            Self::Loading => (
                StatusCode::SERVICE_UNAVAILABLE,
                LOADING,
                Some("model_loading"),
                NotForwarded::NotRun("The model was still loading."),
            ),
            Self::NotLoaded => (
                StatusCode::SERVICE_UNAVAILABLE,
                NO_MODEL,
                Some("model_not_loaded"),
                NotForwarded::NotRun("No model was loaded."),
            ),
        }
    }
}

async fn forward(req: Request<Incoming>, api: Api, shared: &Shared) -> Response<Body> {
    let started = Instant::now();
    let caller = says(req.headers());
    // An authenticated request the proxy answers itself still has its audit row (round 1),
    // saying why it never reached the model.
    let unforwarded = async |status: StatusCode, message: &str, code, why: NotForwarded| {
        AuditRow::not_forwarded(shared, api, &caller, why, started).await;
        json_response(status, &error_body(Some(api), status, message, code))
    };
    let too_large = NotForwarded::Refused("Refused: the request was too large.");
    let declared = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    if declared.is_some_and(|n| n > MAX_BODY as u64) {
        return unforwarded(
            StatusCode::PAYLOAD_TOO_LARGE,
            "The request is too large.",
            None,
            too_large,
        )
        .await;
    }
    let body = match tokio::time::timeout(
        BODY_TIMEOUT,
        Limited::new(req.into_body(), MAX_BODY).collect(),
    )
    .await
    {
        Ok(Ok(collected)) => collected.to_bytes(),
        Ok(Err(e)) if e.is::<http_body_util::LengthLimitError>() => {
            return unforwarded(
                StatusCode::PAYLOAD_TOO_LARGE,
                "The request is too large.",
                None,
                too_large,
            )
            .await;
        }
        Ok(Err(_)) => {
            return unforwarded(
                StatusCode::BAD_REQUEST,
                "The request body could not be read.",
                None,
                NotForwarded::NotRun("The request could not be read."),
            )
            .await;
        }
        Err(_) => {
            return unforwarded(
                StatusCode::REQUEST_TIMEOUT,
                "The request body took too long to arrive.",
                None,
                NotForwarded::Refused("Refused: the request took too long to arrive."),
            )
            .await;
        }
    };
    // [0.0.6 webp-other-paths, final round] The busy counter (D3) and the request count are
    // held from before the model is looked up until the answer ends, so turning WebP
    // pictures (seconds, or a wait behind other requests' pictures) counts too: a change of
    // model asks about this request ("Another app") or waits for it.
    let busy = shared.deps.busy.hold(BusyKind::OtherApp);
    let active = Active::new(&shared.deps.counters);
    let (base_url, upstream_key, model, n_ctx) = match shared.deps.model.current().await {
        Upstream::Ready {
            base_url,
            key,
            model,
            n_ctx,
        } => (base_url, key, model, n_ctx),
        Upstream::Loading => {
            return unforwarded(
                StatusCode::SERVICE_UNAVAILABLE,
                LOADING,
                Some("model_loading"),
                NotForwarded::NotRun("The model was still loading."),
            )
            .await;
        }
        Upstream::NotLoaded => {
            return unforwarded(
                StatusCode::SERVICE_UNAVAILABLE,
                NO_MODEL,
                Some("model_not_loaded"),
                NotForwarded::NotRun("No model was loaded."),
            )
            .await;
        }
    };
    let mut rebuilt = match rebuild_request(api, &body, n_ctx) {
        Ok(rebuilt) => rebuilt,
        Err(bad) => {
            return unforwarded(
                StatusCode::BAD_REQUEST,
                bad.message(),
                None,
                NotForwarded::Refused(bad.audit_summary()),
            )
            .await;
        }
    };
    drop(body);
    // [0.0.6 webp-other-paths] llama-server cannot read WebP: such pictures become JPEG or PNG.
    // [last round] Turning them, and waiting for a turn, ends as soon as the model is stopped
    // or changed ([`MODEL_RECHECK`]), so a request the user chose not to wait for stops
    // counting as in use then and not when its pictures are done; dropping the work kills
    // its picture worker.
    let pictures = &shared.deps.pictures;
    let turning = convert_webp(api, &mut rebuilt.body, pictures, &shared.pictures_at_once);
    let watch = async {
        loop {
            tokio::time::sleep(MODEL_RECHECK).await;
            if let Some(moved) =
                ModelMoved::seen(shared.deps.model.current().await, &base_url, &upstream_key)
            {
                return moved;
            }
        }
    };
    let turned = tokio::select! {
        turned = turning => turned.map(|()| None),
        moved = watch => Ok(Some(moved)),
    };
    let moved = match turned {
        Ok(moved) => moved,
        Err(refusal) => {
            return unforwarded(
                StatusCode::BAD_REQUEST,
                refusal.message(),
                None,
                NotForwarded::Refused(refusal.audit_summary()),
            )
            .await;
        }
    };
    // A change the user chose not to wait for may also have come just as the pictures were
    // done, or before the next look: the request never goes to its old address, which
    // another program may hold by then.
    let moved = match moved {
        Some(moved) => Some(moved),
        None => ModelMoved::seen(shared.deps.model.current().await, &base_url, &upstream_key),
    };
    if let Some(moved) = moved {
        let (status, message, code, why) = moved.reply();
        return unforwarded(status, message, code, why).await;
    }
    let fail = |status: StatusCode, message: &str| {
        json_response(status, &error_body(Some(api), status, message, None))
    };

    let audit = AuditRow::open(shared, api, rebuilt.stream, &caller).await;
    let scrub = Scrub::new(&model);
    let sent = shared
        .client
        .post(format!("{base_url}{}", api.upstream_path()))
        .header(header::AUTHORIZATION, format!("Bearer {upstream_key}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::ACCEPT,
            if rebuilt.stream {
                "text/event-stream"
            } else {
                "application/json"
            },
        )
        .body(rebuilt.body.to_string())
        .send()
        .await;
    let mut upstream = match sent {
        Ok(response) => response,
        Err(e) => {
            tracing::warn!(
                "the sharing proxy could not reach the model: {}",
                without_url(&e)
            );
            audit
                .finish("error", Some("The model did not answer.".into()))
                .await;
            return fail(StatusCode::BAD_GATEWAY, NOT_ANSWERED);
        }
    };
    let status = upstream.status();
    let event_stream = upstream
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));

    if status.is_success() && event_stream {
        let (mut sender, body) = Channel::<Bytes, io::Error>::new(16);
        tokio::spawn(async move {
            let _busy = busy;
            let _active = active;
            let mut filter = SseFilter::new(scrub);
            let outcome = loop {
                match upstream.chunk().await {
                    Ok(Some(chunk)) => match filter.push(&chunk) {
                        Ok(out) if out.is_empty() => {}
                        Ok(out) => {
                            let sent = tokio::time::timeout(
                                CALLER_STALL,
                                sender.send_data(Bytes::from(out)),
                            )
                            .await;
                            match sent {
                                Ok(Ok(())) => {}
                                // The caller went away.
                                Ok(Err(_)) => break "cancelled",
                                // The caller stopped reading: end its answer, free the model.
                                Err(_) => {
                                    sender.abort(io::Error::other("the caller stopped reading"));
                                    break "cancelled";
                                }
                            }
                        }
                        Err(()) => {
                            sender.abort(io::Error::other("an event was too large"));
                            break "error";
                        }
                    },
                    Ok(None) => {
                        let rest = filter.finish();
                        if !rest.is_empty() {
                            let _ = tokio::time::timeout(
                                CALLER_STALL,
                                sender.send_data(Bytes::from(rest)),
                            )
                            .await;
                        }
                        break "ok";
                    }
                    Err(_) => {
                        sender.abort(io::Error::other("the model stopped answering"));
                        break "error";
                    }
                }
            };
            audit.finish(outcome, filter.usage().summary()).await;
        });
        let mut response = Response::new(body.boxed());
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/event-stream"),
        );
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
        return response;
    }

    // A whole answer (or an error, which llama-server never streams).
    let mut bytes = Vec::new();
    let read = loop {
        match upstream.chunk().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_BODY {
                    break Err(());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => break Ok(()),
            Err(_) => break Err(()),
        }
    };
    drop(upstream);
    drop(busy);
    drop(active);
    if read.is_err() {
        audit
            .finish("error", Some("The model's answer was cut off.".into()))
            .await;
        return fail(
            StatusCode::BAD_GATEWAY,
            "The model in harnessUI did not finish its answer. Try again.",
        );
    }
    if !status.is_success() {
        let (status, body) = upstream_error(api, status, &bytes, &scrub);
        audit
            .finish(
                "error",
                Some(format!(
                    "The model answered with an error ({}).",
                    status.as_u16()
                )),
            )
            .await;
        return json_response(status, &body);
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
        audit
            .finish("error", Some("The model's answer was not JSON.".into()))
            .await;
        return fail(
            StatusCode::BAD_GATEWAY,
            "The model in harnessUI gave an answer that could not be read.",
        );
    };
    let mut usage = clean_answer(&mut value);
    if api == Api::CountTokens {
        // `{"input_tokens": n}`, with no usage object.
        usage.input = value.get("input_tokens").and_then(Value::as_u64);
    }
    audit.finish("ok", usage.summary()).await;
    let text = scrub.apply(value.to_string());
    let mut response = Response::new(full(Bytes::from(text)));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// A transport error without the address in it (it holds only the private port, but the
/// log has no use for it).
fn without_url(e: &reqwest::Error) -> String {
    let mut text = e.to_string();
    if let Some(url) = e.url() {
        text = text.replace(url.as_str(), "<llama-server>");
    }
    text
}

// ---- Audit -------------------------------------------------------------------------

/// The audit row of one request: opened before forwarding, completed with the result.
/// A request dropped half way (the caller went away) is completed as cancelled.
struct AuditRow {
    store: store::Store,
    id: Option<store::AuditId>,
    started: Instant,
    done: bool,
}

/// Why an authenticated request was answered by the proxy and never reached the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NotForwarded {
    /// A rule of the proxy refused it (too large, too slow, a link to an image, not JSON).
    Refused(&'static str),
    /// It could not be passed on (no model, still loading, the body could not be read).
    NotRun(&'static str),
}

/// Whether a request of `api` gets an audit row: every generation request, token counting
/// at most once a minute.
fn audit_wanted(shared: &Shared, api: Api) -> bool {
    api.generates()
        || Counters::due(
            &shared.deps.counters.last_count_audit,
            COUNT_TOKENS_AUDIT_EVERY,
        )
}

/// The row of one authenticated request, without its result. Never the request's text.
fn audit_entry(
    api: Api,
    caller: &str,
    stream: Option<bool>,
    forwarded: bool,
    decision: store::AuditDecision,
) -> store::AuditEntry {
    let says = if caller.is_empty() {
        "It did not say which program it is.".to_owned()
    } else {
        format!("It says it is {caller}.")
    };
    let preview = if forwarded {
        format!(
            "Another app used the loaded model ({}). {says}",
            api.label()
        )
    } else {
        format!(
            "Another app sent a request ({}) that was not passed to the model. {says}",
            api.label()
        )
    };
    let mut arguments = json!({"path": api.upstream_path(), "says": caller});
    if let Some(stream) = stream {
        arguments["stream"] = json!(stream);
    }
    store::AuditEntry {
        ts: None,
        chat_id: None,
        server: AUDIT_SERVER.into(),
        tool: api.upstream_path().into(),
        arguments,
        preview,
        risk: "normal".into(),
        decision,
        decided_by: match decision {
            store::AuditDecision::RefusedByPolicy => "sharing proxy".into(),
            _ => "sharing key".into(),
        },
        note: None,
        parent_audit_id: None,
        result: None,
    }
}

impl AuditRow {
    async fn open(shared: &Shared, api: Api, stream: bool, caller: &str) -> Self {
        let store = shared.deps.store.clone();
        let started = Instant::now();
        let id = if audit_wanted(shared, api) {
            store
                .append_audit(audit_entry(
                    api,
                    caller,
                    Some(stream),
                    true,
                    store::AuditDecision::AllowedWithoutPrompt,
                ))
                .await
                .map_err(|e| tracing::error!("could not write the audit log: {e}"))
                .ok()
        } else {
            None
        };
        Self {
            store,
            id,
            started,
            done: false,
        }
    }

    /// The whole row of an authenticated request the proxy answered itself: refused by one
    /// of its rules (decision `refused_by_policy`) or not passed on (no model), result
    /// `not_run` with the reason.
    async fn not_forwarded(
        shared: &Shared,
        api: Api,
        caller: &str,
        why: NotForwarded,
        started: Instant,
    ) {
        if !audit_wanted(shared, api) {
            return;
        }
        let (decision, summary) = match why {
            NotForwarded::Refused(summary) => (store::AuditDecision::RefusedByPolicy, summary),
            NotForwarded::NotRun(summary) => (store::AuditDecision::AllowedWithoutPrompt, summary),
        };
        let store = &shared.deps.store;
        let written = store
            .append_audit(audit_entry(api, caller, None, false, decision))
            .await;
        let result = match written {
            Ok(id) => {
                let duration = started.elapsed().as_millis() as u64;
                store
                    .complete_audit(id, "not_run".into(), Some(summary.into()), Some(duration))
                    .await
            }
            Err(e) => Err(e),
        };
        if let Err(e) = result {
            tracing::error!("could not write the audit log: {e}");
        }
    }

    async fn finish(mut self, status: &str, summary: Option<String>) {
        self.done = true;
        if let Some(id) = self.id {
            let duration = self.started.elapsed().as_millis() as u64;
            if let Err(e) = self
                .store
                .complete_audit(id, status.to_owned(), summary, Some(duration))
                .await
            {
                tracing::error!("could not complete the audit row: {e}");
            }
        }
    }
}

impl Drop for AuditRow {
    fn drop(&mut self) {
        if self.done {
            return;
        }
        let (Some(id), Ok(runtime)) = (self.id, tokio::runtime::Handle::try_current()) else {
            return;
        };
        let store = self.store.clone();
        let duration = self.started.elapsed().as_millis() as u64;
        runtime.spawn(async move {
            let _ = store
                .complete_audit(id, "cancelled".into(), None, Some(duration))
                .await;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(
                header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    #[test]
    fn only_the_exact_paths_route() {
        assert_eq!(route(&Method::GET, "/v1/models"), Route::Models);
        assert_eq!(
            route(&Method::POST, "/v1/messages"),
            Route::Api(Api::Messages)
        );
        assert_eq!(
            route(&Method::POST, "/v1/messages/count_tokens"),
            Route::Api(Api::CountTokens)
        );
        assert_eq!(
            route(&Method::POST, "/v1/responses"),
            Route::Api(Api::Responses)
        );
        assert_eq!(
            route(&Method::POST, "/v1/chat/completions"),
            Route::Api(Api::ChatCompletions)
        );
        assert_eq!(route(&Method::GET, "/v1/messages"), Route::MethodNotAllowed);
        assert_eq!(route(&Method::POST, "/v1/models"), Route::MethodNotAllowed);
        for path in [
            "/v1/messages/",
            "//v1/messages",
            "/v1//messages",
            "/v1/./messages",
            "/v1/x/../messages",
            "/v1/messages/../models",
            "/v1/%6Dessages",
            "/v1/messages%2Fcount_tokens",
            "/V1/messages",
            "/v1/messages/count_tokens/",
            "/v1/completions",
            "/completion",
            "/props",
            "/health",
            "/slots",
            "/v1/embeddings",
            "/",
            "",
        ] {
            assert_eq!(route(&Method::POST, path), Route::NotFound, "{path}");
            assert_eq!(Api::from_path(path), None, "{path}");
        }
    }

    #[test]
    fn host_must_be_this_loopback_port() {
        let uri: Uri = "/v1/messages".parse().unwrap();
        for ok in ["127.0.0.1:50000", "localhost:50000", "LocalHost:50000"] {
            assert_eq!(
                check_target(&uri, &headers(&[("host", ok)]), 50000),
                Ok(()),
                "{ok}"
            );
        }
        for bad in [
            "127.0.0.1",
            "localhost",
            "127.0.0.1:50001",
            "evil.example:50000",
            "127.0.0.1.evil.example:50000",
            "[::1]:50000",
            "0.0.0.0:50000",
            "127.0.0.2:50000",
            " 127.0.0.1:50000",
            "localhost:50000.",
        ] {
            assert_eq!(
                check_target(&uri, &headers(&[("host", bad)]), 50000),
                Err(Refused::Host),
                "{bad}"
            );
        }
        // Missing or repeated.
        assert_eq!(
            check_target(&uri, &HeaderMap::new(), 50000),
            Err(Refused::Host)
        );
        assert_eq!(
            check_target(
                &uri,
                &headers(&[("host", "127.0.0.1:50000"), ("host", "127.0.0.1:50000")]),
                50000
            ),
            Err(Refused::Host)
        );
        // A target with a scheme or a host of its own.
        let absolute: Uri = "http://127.0.0.1:50000/v1/messages".parse().unwrap();
        assert_eq!(
            check_target(&absolute, &headers(&[("host", "127.0.0.1:50000")]), 50000),
            Err(Refused::Host)
        );
    }

    #[test]
    fn any_origin_is_refused() {
        let uri: Uri = "/v1/messages".parse().unwrap();
        for origin in ["null", "http://127.0.0.1:50000", "https://example.com", ""] {
            assert_eq!(
                check_target(
                    &uri,
                    &headers(&[("host", "127.0.0.1:50000"), ("origin", origin)]),
                    50000
                ),
                Err(Refused::Origin),
                "{origin:?}"
            );
        }
    }

    const KEY: &str = "harnessui-share-0123456789abcdef0123456789abcdef";

    #[test]
    fn the_key_in_either_header() {
        assert!(key_matches(&headers(&[("x-api-key", KEY)]), KEY));
        assert!(key_matches(
            &headers(&[("authorization", &format!("Bearer {KEY}"))]),
            KEY
        ));
        assert!(key_matches(
            &headers(&[("authorization", &format!("bearer {KEY}"))]),
            KEY
        ));
        // Both, the same.
        assert!(key_matches(
            &headers(&[
                ("x-api-key", KEY),
                ("authorization", &format!("Bearer {KEY}"))
            ]),
            KEY
        ));
        // The key in one, something else in the other: Claude Code with the user's own
        // ANTHROPIC_API_KEY set sends that key in x-api-key next to the sharing key.
        assert!(key_matches(
            &headers(&[
                ("x-api-key", "sk-ant-api03-the-users-own-key"),
                ("authorization", &format!("Bearer {KEY}"))
            ]),
            KEY
        ));
        assert!(key_matches(
            &headers(&[("x-api-key", KEY), ("authorization", "Bearer nope")]),
            KEY
        ));
        assert!(key_matches(
            &headers(&[
                ("x-api-key", KEY),
                ("authorization", "Basic aGFybmVzc3VpOg==")
            ]),
            KEY
        ));
    }

    #[test]
    fn a_wrong_missing_or_doubtful_key_is_refused() {
        let wrong = "harnessui-share-0123456789abcdef0123456789abcdee";
        for set in [
            vec![],
            vec![("x-api-key", wrong)],
            vec![("x-api-key", "")],
            vec![("x-api-key", &KEY[..KEY.len() - 1])],
            vec![("authorization", KEY)],
            vec![("authorization", "Basic aGFybmVzc3VpOg==")],
            vec![("authorization", "Bearer")],
            vec![(
                "authorization",
                "Bearer  harnessui-share-0123456789abcdef0123456789abcdef",
            )],
            // Both wrong, or the key only in a scheme that is not Bearer.
            vec![("x-api-key", wrong), ("authorization", "Bearer nope")],
            vec![
                ("x-api-key", "sk-ant-api03-the-users-own-key"),
                ("authorization", &format!("Basic {KEY}")),
            ],
            // Repeated, even when one of the copies is right.
            vec![("x-api-key", KEY), ("x-api-key", KEY)],
            vec![("x-api-key", wrong), ("x-api-key", KEY)],
            vec![
                ("authorization", &format!("Bearer {KEY}")),
                ("authorization", &format!("Bearer {KEY}")),
            ],
            vec![
                ("x-api-key", KEY),
                ("authorization", &format!("Bearer {KEY}")),
                ("authorization", "Bearer nope"),
            ],
        ] {
            assert!(!key_matches(&headers(&set), KEY), "{set:?}");
        }
        // No key made yet matches nothing, not even an empty one.
        assert!(!key_matches(&headers(&[("x-api-key", "")]), ""));
        assert!(!key_matches(&headers(&[("authorization", "Bearer ")]), ""));
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
        // Every byte is compared: a difference at the start or at the end is found alike.
        assert!(!constant_time_eq(b"xbcdef", b"abcdef"));
        assert!(!constant_time_eq(b"abcdex", b"abcdef"));
        // Different lengths never match, whichever side is longer.
        assert!(!constant_time_eq(b"abcd", b"abc"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(!constant_time_eq(b"a", b""));
        // The same on long keys.
        let long = vec![7u8; 1 << 16];
        let mut first = long.clone();
        first[0] = 8;
        let mut last = long.clone();
        *last.last_mut().unwrap() = 8;
        assert!(constant_time_eq(&long, &long.clone()));
        assert!(!constant_time_eq(&long, &first));
        assert!(!constant_time_eq(&long, &last));
        assert!(!constant_time_eq(&long, &long[1..]));
    }

    /// The comparison takes about as long whether the first or the last byte differs.
    /// A loose bound (a short-circuiting compare differs by far more on 64 KiB keys).
    /// Wall-clock timing fails at random on a busy machine, so it is run by hand
    /// (`--ignored --nocapture`; the numbers are reported).
    #[test]
    #[ignore = "timing measurement, run by hand"]
    fn constant_time_compare_does_not_stop_at_the_first_difference() {
        let a = vec![7u8; 1 << 16];
        let mut early = a.clone();
        early[0] = 8;
        let mut late = a.clone();
        *late.last_mut().unwrap() = 8;
        let time = |b: &[u8]| {
            let start = Instant::now();
            for _ in 0..200 {
                assert!(!constant_time_eq(&a, std::hint::black_box(b)));
            }
            start.elapsed()
        };
        // Warm up, then take the best of three of each.
        time(&early);
        let early_t = (0..3).map(|_| time(&early)).min().unwrap();
        let late_t = (0..3).map(|_| time(&late)).min().unwrap();
        let ratio = late_t.as_secs_f64() / early_t.as_secs_f64().max(1e-9);
        eprintln!("early {early_t:?}, late {late_t:?}, ratio {ratio:.2}");
        assert!(
            (0.25..4.0).contains(&ratio),
            "early {early_t:?}, late {late_t:?}"
        );
    }

    #[test]
    fn unknown_and_llama_fields_are_dropped_and_the_model_is_the_alias() {
        let body = json!({
            "model": "claude-sonnet-x",
            "messages": [{"role": "user", "content": "hi"}],
            "max_tokens": 100,
            "metadata": {"user_id": "someone"},
            "grammar": "root ::= \"x\"",
            "n_probs": 5,
            "id_slot": 0,
            "lora": [{"id": 0, "scale": 1.0}],
            "cache_prompt": false,
            "chat_template_kwargs": {"enable_thinking": true},
            "samplers": ["top_k"],
            "json_schema": {},
            "temperature": 0.5,
        });
        let out = rebuild_request(Api::Messages, body.to_string().as_bytes(), 8192).unwrap();
        assert_eq!(
            out.body,
            json!({
                "model": MODEL_ALIAS,
                "messages": [{"role": "user", "content": "hi"}],
                "max_tokens": 100,
                "temperature": 0.5,
            })
        );
        assert!(!out.stream);

        let chat = json!({"model": "x", "messages": [], "stream": true, "n_probs": 3,
                          "cache_prompt": true, "user": "u", "store": true, "seed": 3});
        let out = rebuild_request(Api::ChatCompletions, chat.to_string().as_bytes(), 8192).unwrap();
        assert_eq!(
            out.body,
            json!({"model": MODEL_ALIAS, "messages": [], "stream": true, "seed": 3, "max_tokens": 8192})
        );
        assert!(out.stream);

        let responses = json!({"model": "x", "input": "hi", "store": false, "previous_response_id": "r",
                               "include": ["x"], "prompt_cache_key": "k", "metadata": {}, "id_slot": 1});
        let out = rebuild_request(Api::Responses, responses.to_string().as_bytes(), 4096).unwrap();
        assert_eq!(
            out.body,
            json!({"model": MODEL_ALIAS, "input": "hi", "max_output_tokens": 4096})
        );

        let count = json!({"model": "x", "messages": [], "max_tokens": 5, "stream": true});
        let out = rebuild_request(Api::CountTokens, count.to_string().as_bytes(), 4096).unwrap();
        assert_eq!(out.body, json!({"model": MODEL_ALIAS, "messages": []}));
    }

    #[test]
    fn the_output_limit_is_capped() {
        let limit = |api: Api, body: Value, n_ctx: u32| {
            rebuild_request(api, body.to_string().as_bytes(), n_ctx).map(|r| r.body)
        };
        // At the fixed cap, and at the model's context when that is smaller.
        assert_eq!(
            limit(Api::Messages, json!({"max_tokens": 1_000_000}), 131_072).unwrap()["max_tokens"],
            json!(MAX_OUTPUT_TOKENS)
        );
        assert_eq!(
            limit(Api::Messages, json!({"max_tokens": 32_000}), 8192).unwrap()["max_tokens"],
            json!(8192)
        );
        assert_eq!(
            limit(Api::Messages, json!({"max_tokens": 50}), 8192).unwrap()["max_tokens"],
            json!(50)
        );
        // Added when missing.
        assert_eq!(
            limit(Api::Messages, json!({}), 2048).unwrap()["max_tokens"],
            json!(2048)
        );
        // Both Chat fields are capped; none is added when one is sent.
        let both = limit(
            Api::ChatCompletions,
            json!({"max_tokens": 99_999, "max_completion_tokens": 99_999}),
            16_384,
        )
        .unwrap();
        assert_eq!(both["max_tokens"], json!(16_384));
        assert_eq!(both["max_completion_tokens"], json!(16_384));
        let one = limit(
            Api::ChatCompletions,
            json!({"max_completion_tokens": 10}),
            16_384,
        )
        .unwrap();
        assert_eq!(one.get("max_tokens"), None);
        assert_eq!(
            limit(Api::Responses, json!({"max_output_tokens": 70_000}), 65_536).unwrap()["max_output_tokens"],
            json!(MAX_OUTPUT_TOKENS)
        );
        // Not a whole number.
        for bad in [json!(-1), json!(1.5), json!("100"), json!([1])] {
            assert_eq!(
                limit(Api::Messages, json!({"max_tokens": bad}), 8192),
                Err(BadRequest::BadLimit)
            );
        }
    }

    #[test]
    fn bodies_that_are_not_json_objects_are_refused() {
        assert_eq!(
            rebuild_request(Api::Messages, b"{", 8192),
            Err(BadRequest::NotJson)
        );
        assert_eq!(
            rebuild_request(Api::Messages, b"", 8192),
            Err(BadRequest::NotJson)
        );
        assert_eq!(
            rebuild_request(Api::Messages, b"[1]", 8192),
            Err(BadRequest::NotAnObject)
        );
        assert_eq!(
            rebuild_request(Api::Messages, b"\"x\"", 8192),
            Err(BadRequest::NotAnObject)
        );
        assert_eq!(
            rebuild_request(Api::Messages, br#"{"stream":"yes"}"#, 8192),
            Err(BadRequest::BadStream)
        );
    }

    fn refused(api: Api, body: Value) -> bool {
        rebuild_request(api, body.to_string().as_bytes(), 8192) == Err(BadRequest::NotInline)
    }

    const PNG: &str = "data:image/png;base64,iVBORw0KGgo=";

    #[test]
    fn links_to_images_and_files_are_refused_in_chat_completions() {
        let msg = |part: Value| json!({"messages": [{"role": "user", "content": [part]}]});
        for url in [
            "http://127.0.0.1:9/x.png",
            "https://example.com/x.png",
            "file:///etc/passwd",
            "ftp://x/y",
            "",
        ] {
            assert!(
                refused(
                    Api::ChatCompletions,
                    msg(json!({"type": "image_url", "image_url": {"url": url}}))
                ),
                "{url}"
            );
            assert!(
                refused(
                    Api::ChatCompletions,
                    msg(json!({"type": "image_url", "image_url": url}))
                ),
                "{url}"
            );
        }
        assert!(refused(
            Api::ChatCompletions,
            msg(json!({"type": "image_url", "image_url": {}}))
        ));
        assert!(refused(
            Api::ChatCompletions,
            msg(json!({"type": "file", "file": {"file_id": "file-1"}}))
        ));
        assert!(refused(
            Api::ChatCompletions,
            msg(json!({"type": "input_audio", "audio_url": "https://x/a.wav"}))
        ));
        // Audio and video in the shapes llama-server reads (its media loader takes `data`
        // first, then `url`, for input_audio, input_video and video_url, and downloads any
        // value that starts with http).
        let metadata = "http://169.254.169.254/latest/meta-data/";
        for part in [
            json!({"type": "input_audio", "input_audio": {"data": metadata, "format": "wav"}}),
            json!({"type": "input_audio", "input_audio": {"url": metadata, "format": "wav"}}),
            json!({"type": "input_audio", "input_audio": {"data": "UklGRg==", "url": metadata}}),
            json!({"type": "input_video", "input_video": {"url": metadata}}),
            json!({"type": "input_video", "input_video": {"data": metadata}}),
            json!({"type": "video_url", "video_url": {"url": "data:video/mp4;base64,AAAA", "data": metadata}}),
            json!({"type": "video_url", "video_url": {"url": metadata}}),
            json!({"type": "video_url", "video_url": metadata}),
            json!({"type": "input_audio", "input_audio": metadata}),
            json!({"type": "input_audio", "input_audio": {"data": "file:///etc/passwd"}}),
            json!({"type": "input_audio", "input_audio": {"data": ""}}),
            json!({"type": "input_audio", "input_audio": {"format": "wav"}}),
            json!({"type": "input_audio", "input_audio": {"data": 7, "url": metadata}}),
            json!({"type": "input_video", "input_video": {"data": ["x"]}}),
            json!({"type": "image_url", "image_url": {"url": PNG, "data": metadata}}),
        ] {
            assert!(refused(Api::ChatCompletions, msg(part.clone())), "{part}");
            // Also inside a tool message, which llama-server loads the same way.
            assert!(
                refused(
                    Api::ChatCompletions,
                    json!({"messages": [{"role": "tool", "tool_call_id": "1", "content": [part.clone()]}]})
                ),
                "{part}"
            );
        }
        // Audio and video sent inline: a data: URL, or plain base64 as OpenAI's input_audio
        // sends it.
        for part in [
            json!({"type": "input_audio", "input_audio": {"data": "UklGRiQAAABXQVZF", "format": "wav"}}),
            json!({"type": "input_audio", "input_audio": {"data": "UklG\nRiQA AABX=="}}),
            json!({"type": "input_audio", "input_audio": {"url": "data:audio/wav;base64,UklGRg=="}}),
            json!({"type": "input_video", "input_video": {"data": "data:video/mp4;base64,AAAA"}}),
            json!({"type": "video_url", "video_url": {"url": "data:video/mp4;base64,AAAA"}}),
            json!({"type": "image_url", "image_url": {"url": "iVBORw0KGgo="}}),
        ] {
            assert!(!refused(Api::ChatCompletions, msg(part.clone())), "{part}");
        }
        // Inline data is fine, and so is an address in text or in a tool call.
        assert!(!refused(
            Api::ChatCompletions,
            msg(json!({"type": "image_url", "image_url": {"url": PNG}}))
        ));
        assert!(!refused(
            Api::ChatCompletions,
            msg(json!({"type": "image_url", "image_url": {"url": "DATA:image/png;base64,AA=="}}))
        ));
        assert!(!refused(
            Api::ChatCompletions,
            msg(
                json!({"type": "file", "file": {"file_data": "data:application/pdf;base64,AA==", "filename": "a.pdf"}})
            )
        ));
        assert!(!refused(
            Api::ChatCompletions,
            msg(json!({"type": "text", "text": "see https://example.com/x.png"}))
        ));
        assert!(!refused(
            Api::ChatCompletions,
            json!({"messages": [{"role": "assistant", "content": null,
            "tool_calls": [{"id": "1", "type": "function", "function": {"name": "fetch", "arguments": "{\"url\":\"https://example.com\"}"}}]}]})
        ));
    }

    #[test]
    fn links_to_images_and_files_are_refused_in_responses() {
        let input = |part: Value| json!({"input": [{"role": "user", "content": [part]}]});
        assert!(refused(
            Api::Responses,
            input(json!({"type": "input_image", "image_url": "https://example.com/x.png"}))
        ));
        assert!(refused(
            Api::Responses,
            input(json!({"type": "input_image", "image_url": "http://localhost/x.png"}))
        ));
        assert!(refused(
            Api::Responses,
            input(json!({"type": "input_image", "file_id": "file-1"}))
        ));
        assert!(refused(
            Api::Responses,
            input(json!({"type": "input_file", "file_url": "https://example.com/a.pdf"}))
        ));
        assert!(refused(
            Api::Responses,
            input(json!({"type": "input_file", "file_id": "file-1"}))
        ));
        // In a typed message item, and in a tool's output.
        assert!(refused(
            Api::Responses,
            json!({"input": [{"type": "message", "role": "user",
            "content": [{"type": "input_image", "image_url": "https://example.com/x.png"}]}]})
        ));
        assert!(refused(
            Api::Responses,
            json!({"input": [{"type": "function_call_output", "call_id": "c",
            "output": [{"type": "input_image", "image_url": "https://example.com/x.png"}]}]})
        ));
        // Inline data, plain text, a string input, and a URL inside function arguments are fine.
        assert!(!refused(
            Api::Responses,
            input(json!({"type": "input_image", "image_url": PNG}))
        ));
        assert!(!refused(
            Api::Responses,
            input(
                json!({"type": "input_file", "file_data": "data:application/pdf;base64,AA==", "filename": "a.pdf"})
            )
        ));
        assert!(!refused(
            Api::Responses,
            json!({"input": "look at https://example.com/x.png"})
        ));
        assert!(!refused(
            Api::Responses,
            json!({"input": [{"type": "function_call", "call_id": "c", "name": "fetch",
            "arguments": "{\"url\":\"https://example.com\"}"}]})
        ));
        assert!(!refused(
            Api::Responses,
            json!({"input": [{"type": "function_call_output", "call_id": "c", "output": "https://example.com"}]})
        ));
    }

    #[test]
    fn links_to_images_and_files_are_refused_in_messages() {
        let user = |block: Value| json!({"messages": [{"role": "user", "content": [block]}]});
        for api in [Api::Messages, Api::CountTokens] {
            assert!(refused(
                api,
                user(
                    json!({"type": "image", "source": {"type": "url", "url": "https://example.com/x.png"}})
                )
            ));
            assert!(refused(
                api,
                user(json!({"type": "image", "source": {"type": "file", "file_id": "file_1"}}))
            ));
            assert!(refused(
                api,
                user(
                    json!({"type": "document", "source": {"type": "url", "url": "https://example.com/a.pdf"}})
                )
            ));
            assert!(refused(api, user(json!({"type": "image", "source": {}}))));
            // Inside a tool result, inside a document made of content, and in the system prompt.
            assert!(refused(
                api,
                user(json!({"type": "tool_result", "tool_use_id": "t",
                "content": [{"type": "image", "source": {"type": "url", "url": "https://example.com/x.png"}}]}))
            ));
            assert!(refused(
                api,
                user(json!({"type": "document", "source": {"type": "content",
                "content": [{"type": "image", "source": {"type": "url", "url": "https://x/y.png"}}]}}))
            ));
            assert!(refused(
                api,
                json!({"system": [{"type": "image", "source": {"type": "url", "url": "https://x/y.png"}}], "messages": []})
            ));
            // Inline data, text, and an address in a tool call's input are fine.
            assert!(!refused(
                api,
                user(
                    json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgo="}})
                )
            ));
            assert!(!refused(
                api,
                user(
                    json!({"type": "document", "source": {"type": "text", "media_type": "text/plain", "data": "hello"}})
                )
            ));
            assert!(!refused(
                api,
                json!({"messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "t", "name": "WebFetch", "input": {"url": "https://example.com", "image_url": "https://x"}}]}]})
            ));
            assert!(!refused(
                api,
                user(
                    json!({"type": "tool_result", "tool_use_id": "t", "content": "fetched https://example.com"})
                )
            ));
        }
    }

    #[test]
    fn cached_token_counters_timings_and_the_path_are_removed() {
        // Anthropic, as llama-server answers after a cache hit.
        let mut message = json!({"id": "x", "type": "message", "model": "/home/someone/models/m.gguf",
            "usage": {"cache_read_input_tokens": 9, "input_tokens": 1, "output_tokens": 8}});
        let usage = clean_answer(&mut message);
        assert_eq!(
            message["usage"],
            json!({"input_tokens": 10, "output_tokens": 8})
        );
        assert_eq!(message["model"], json!(MODEL_ALIAS));
        assert_eq!(
            usage,
            Usage {
                input: Some(10),
                output: Some(8)
            }
        );

        let mut start = json!({"type": "message_start", "message": {"model": "m", "usage":
            {"cache_read_input_tokens": 3, "cache_creation_input_tokens": 2, "cache_creation": {}, "input_tokens": 5, "output_tokens": 0}}});
        clean_answer(&mut start);
        assert_eq!(
            start["message"]["usage"],
            json!({"input_tokens": 10, "output_tokens": 0})
        );

        // Chat Completions.
        let mut chat = json!({"model": "m", "system_fingerprint": "b12110-cd959bae9",
            "usage": {"completion_tokens": 8, "prompt_tokens": 10, "total_tokens": 18, "prompt_tokens_details": {"cached_tokens": 9}},
            "timings": {"cache_n": 9, "prompt_n": 1}});
        let usage = clean_answer(&mut chat);
        assert_eq!(
            chat,
            json!({"model": MODEL_ALIAS,
            "usage": {"completion_tokens": 8, "prompt_tokens": 10, "total_tokens": 18, "prompt_tokens_details": {"cached_tokens": 0}}})
        );
        assert_eq!(
            usage,
            Usage {
                input: Some(10),
                output: Some(8)
            }
        );

        // Responses, the completed event.
        let mut done = json!({"type": "response.completed", "response": {"model": "m",
            "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14, "input_tokens_details": {"cached_tokens": 9}}},
            "timings": {"cache_n": 9}});
        let usage = clean_answer(&mut done);
        assert_eq!(
            done,
            json!({"type": "response.completed", "response": {"model": MODEL_ALIAS,
            "usage": {"input_tokens": 10, "output_tokens": 4, "total_tokens": 14, "input_tokens_details": {"cached_tokens": 0}}}})
        );
        assert_eq!(
            usage,
            Usage {
                input: Some(10),
                output: Some(4)
            }
        );
    }

    /// A streamed Chat Completions answer reports no usage unless the caller asked for it,
    /// but llama-server's `timings` still carry the counts: the audit row takes them from
    /// there before they are removed.
    #[test]
    fn counts_come_from_the_timings_when_there_is_no_usage() {
        let mut filter = SseFilter::new(Scrub::default());
        let mut out = filter
            .push(b"data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\n")
            .unwrap();
        out.push_str(&filter.push(b"data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}],\"timings\":{\"cache_n\":9,\"prompt_n\":3,\"prompt_ms\":1.5,\"predicted_n\":4,\"predicted_ms\":2.0}}\n\ndata: [DONE]\n\n").unwrap());
        assert!(
            !out.contains("timings") && !out.contains("cache_n"),
            "{out}"
        );
        assert_eq!(
            filter.usage(),
            Usage {
                input: Some(12),
                output: Some(4)
            }
        );
        // A usage object, when there is one, wins over the timings.
        let mut both = json!({"usage": {"prompt_tokens": 20, "completion_tokens": 5},
            "timings": {"cache_n": 1, "prompt_n": 1, "predicted_n": 1}});
        assert_eq!(
            clean_answer(&mut both),
            Usage {
                input: Some(20),
                output: Some(5)
            }
        );
        assert_eq!(both.get("timings"), None);
        // Timings that are not an object, or lack the counts, give nothing.
        let mut odd = json!({"timings": "x"});
        assert_eq!(clean_answer(&mut odd), Usage::default());
        assert_eq!(odd, json!({}));
        let mut partial = json!({"timings": {"predicted_n": 2}});
        assert_eq!(
            clean_answer(&mut partial),
            Usage {
                input: None,
                output: Some(2)
            }
        );
    }

    /// A key named like a counter inside the model's own output (a tool call's input) is
    /// left alone: only known places are cleaned.
    #[test]
    fn the_models_output_is_not_rewritten() {
        let mut message = json!({"content": [{"type": "tool_use", "input": {"timings": 1, "model": "keep", "usage": {"cache_read_input_tokens": 1}}}]});
        let before = message.clone();
        clean_answer(&mut message);
        assert_eq!(message, before);
    }

    #[test]
    fn the_event_stream_is_cleaned_event_by_event() {
        let scrub = Scrub::new(Path::new("/home/someone/models/Secret-Model.gguf"));
        let mut filter = SseFilter::new(scrub);
        let upstream = concat!(
            "event: message_start\n",
            "data: {\"type\":\"message_start\",\"message\":{\"model\":\"/home/someone/models/Secret-Model.gguf\",\"usage\":{\"cache_read_input_tokens\":9,\"input_tokens\":1,\"output_tokens\":0}}}\n",
            "\n",
            ": a comment\n",
            "id: 7\n",
            "retry: 10\n",
            "event: evil\r\n",
            "event: bad name\n",
            "data: not json\n",
            "data: {\"type\":\"content_block_delta\",\"delta\":{\"text\":\"hi\"}}\r\n",
            "\r\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":4,\"prompt_tokens_details\":{\"cached_tokens\":9}},\"timings\":{\"cache_n\":9}}\n",
            "\n",
            "data: [DONE]\n",
            "\n",
        );
        // Fed in awkward pieces, as a socket would.
        let mut out = String::new();
        for piece in upstream.as_bytes().chunks(7) {
            out.push_str(&filter.push(piece).unwrap());
        }
        out.push_str(&filter.finish());
        // Data lines compared as JSON (key order depends on serde_json's features).
        let lines: Vec<Value> = out
            .split_inclusive('\n')
            .map(
                |line| match line.strip_prefix("data: ").filter(|d| !d.starts_with('[')) {
                    Some(data) => serde_json::from_str(data.trim_end()).unwrap(),
                    None => json!(line),
                },
            )
            .collect();
        assert_eq!(
            lines,
            vec![
                json!("event: message_start\n"),
                json!({"type": "message_start", "message": {"model": MODEL_ALIAS, "usage": {"input_tokens": 10, "output_tokens": 0}}}),
                json!("\n"),
                json!("event: evil\n"),
                json!({"type": "content_block_delta", "delta": {"text": "hi"}}),
                json!("\n"),
                json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 4, "prompt_tokens_details": {"cached_tokens": 0}}}),
                json!("\n"),
                json!("data: [DONE]\n"),
                json!("\n"),
            ]
        );
        assert_eq!(
            filter.usage(),
            Usage {
                input: Some(10),
                output: Some(4)
            }
        );
        assert!(!out.contains("Secret-Model"));
        assert!(!out.contains("cache_n"));
    }

    #[test]
    fn an_endless_line_is_an_error() {
        let mut filter = SseFilter::new(Scrub::default());
        let chunk = vec![b'a'; 1 << 20];
        let mut failed = false;
        for _ in 0..40 {
            if filter.push(&chunk).is_err() {
                failed = true;
                break;
            }
        }
        assert!(failed);
    }

    #[test]
    fn the_path_is_scrubbed_from_any_text() {
        let windows = Scrub::new(Path::new(r"C:\Users\Someone\Models\m.gguf"));
        let text =
            serde_json::to_string(&json!({"error": r"cannot read C:\Users\Someone\Models\m.gguf"}))
                .unwrap();
        let out = windows.apply(text);
        assert!(!out.contains("Someone"), "{out}");
        assert!(out.contains(MODEL_ALIAS));
        let unix = Scrub::new(Path::new("/home/someone/m.gguf"));
        assert_eq!(
            unix.apply("load m.gguf from /home/someone/m.gguf".into()),
            "load harnessui-local from harnessui-local"
        );
    }

    #[test]
    fn upstream_errors_come_back_in_the_apis_shape() {
        let scrub = Scrub::new(Path::new("/home/someone/m.gguf"));
        let full = br#"{"error":{"code":400,"message":"request (6009 tokens) exceeds the available context size (4096 tokens), try increasing it","type":"exceed_context_size_error","n_prompt_tokens":6009,"n_ctx":4096}}"#;
        let (status, body) = upstream_error(Api::Messages, StatusCode::BAD_REQUEST, full, &scrub);
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(
            body,
            json!({"type": "error", "error": {"type": "invalid_request_error", "message": "prompt is too long: 6009 tokens > 4096 maximum"}})
        );
        let (_, body) = upstream_error(Api::Responses, StatusCode::BAD_REQUEST, full, &scrub);
        assert_eq!(body["error"]["code"], json!("context_length_exceeded"));
        assert_eq!(body["error"]["type"], json!("invalid_request_error"));

        let other = br#"{"error":{"code":500,"message":"failed to open /home/someone/m.gguf\u202e","type":"server_error","extra":"x"}}"#;
        let (status, body) = upstream_error(
            Api::ChatCompletions,
            StatusCode::INTERNAL_SERVER_ERROR,
            other,
            &scrub,
        );
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            body,
            json!({"error": {"message": "failed to open harnessui-local\\u{202E}", "type": "server_error", "param": null, "code": null}})
        );
        let (status, body) = upstream_error(Api::Messages, StatusCode::OK, b"<html>", &scrub);
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(
            body["error"]["message"],
            json!("The model could not answer this request.")
        );
    }

    #[test]
    fn error_shapes() {
        assert_eq!(
            error_body(
                Some(Api::Messages),
                StatusCode::SERVICE_UNAVAILABLE,
                NO_MODEL,
                None
            ),
            json!({"type": "error", "error": {"type": "overloaded_error", "message": NO_MODEL}})
        );
        assert_eq!(
            error_body(
                Some(Api::CountTokens),
                StatusCode::UNAUTHORIZED,
                "Invalid API key.",
                Some("invalid_api_key")
            ),
            json!({"type": "error", "error": {"type": "authentication_error", "message": "Invalid API key."}})
        );
        assert_eq!(
            error_body(
                Some(Api::ChatCompletions),
                StatusCode::SERVICE_UNAVAILABLE,
                LOADING,
                Some("model_loading")
            ),
            json!({"error": {"message": LOADING, "type": "server_error", "param": null, "code": "model_loading"}})
        );
        assert_eq!(
            error_body(None, StatusCode::NOT_FOUND, "x", None),
            json!({"error": {"message": "x", "type": "invalid_request_error", "param": null, "code": null}})
        );
    }

    #[test]
    fn what_the_caller_says_it_is() {
        assert_eq!(
            says(&headers(&[(
                "user-agent",
                "claude-cli/2.1.322 (external, cli)"
            )])),
            "claude-cli/2.1.322 (external, cli)"
        );
        assert_eq!(says(&HeaderMap::new()), "");
        let long = says(&headers(&[("user-agent", &"x".repeat(100))]));
        assert_eq!(long.chars().count(), 40);
        let mut map = HeaderMap::new();
        map.insert(
            header::USER_AGENT,
            HeaderValue::from_bytes(b"evil\xe2\x80\xae\tapp").unwrap(),
        );
        let s = says(&map);
        assert!(!s.contains('\u{202e}') && !s.contains('\t'), "{s:?}");
    }

    #[test]
    fn usage_summary_and_thousands() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(11_088), "11,088");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(Usage::default().summary(), None);
        assert_eq!(
            Usage {
                input: Some(1_200),
                output: None
            }
            .summary()
            .as_deref(),
            Some("1,200 tokens in")
        );
        assert_eq!(
            Usage {
                input: Some(11_088),
                output: Some(56)
            }
            .summary()
            .as_deref(),
            Some("11,088 tokens in, 56 out")
        );
        assert_eq!(
            Usage {
                input: None,
                output: Some(3)
            }
            .summary()
            .as_deref(),
            Some("? tokens in, 3 out")
        );
    }

    #[test]
    fn refusals_over_the_allowance_are_cut_short() {
        let counters = Counters::default();
        let answered = (0..100).filter(|_| counters.refuse()).count();
        assert_eq!(answered, REFUSALS_PER_SECOND as usize);
        assert_eq!(counters.refused(), 100);
        counters.reset_refused();
        assert_eq!(counters.refused(), 0);
    }

    #[test]
    fn the_model_list_names_only_the_alias() {
        let body = models_body();
        assert_eq!(body["data"][0]["id"], json!(MODEL_ALIAS));
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
    }

    // ---- WebP pictures (0.0.6 webp-other-paths) --------------------------------------

    fn b64(bytes: &[u8]) -> String {
        base64::Engine::encode(&base64::engine::general_purpose::STANDARD, bytes)
    }

    fn webp_count(api: Api, body: &Value) -> usize {
        let mut body = body.clone();
        let mut n = 0;
        each_picture(api, &mut body, &mut |slot| {
            n += usize::from(slot.webp().is_some())
        });
        n
    }

    /// [0.0.6 webp-other-paths] Every place llama-server reads a picture from is looked at,
    /// in all three shapes, and a WebP file is known by its bytes whatever type it is given;
    /// other pictures, text and tool-call arguments are not touched.
    #[test]
    fn webp_pictures_are_found_wherever_llama_server_reads_a_picture() {
        use crate::attachments::tests::LOSSY_WEBP;
        let webp = b64(LOSSY_WEBP);
        let mut spaced = webp.clone();
        spaced.insert(6, '\n');
        let url = format!("data:image/webp;base64,{webp}");
        let as_png = format!("data:image/png;base64,{spaced}");
        let png = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAA";
        let source =
            |data: &str| json!({"type": "base64", "media_type": "image/png", "data": data});

        let chat = json!({"messages": [
            {"role": "system", "content": "data:image/webp;base64,UklGRlwAAABXRUJQ"},
            {"role": "user", "content": [
                {"type": "text", "text": url},
                {"type": "image_url", "image_url": {"url": url}},
                {"type": "image_url", "image_url": {"url": as_png}},
                {"type": "image_url", "image_url": webp},
                {"type": "image_url", "image_url": {"url": png}},
                {"type": "image_url", "image_url": {"url": format!("data:image/webp,{webp}")}},
                {"type": "input_audio", "input_audio": {"data": webp, "format": "wav"}},
            ]},
            {"role": "assistant", "tool_calls": [{"id": "c", "type": "function",
                "function": {"name": "f", "arguments": json!({"image_url": url}).to_string()}}]},
            {"role": "tool", "tool_call_id": "c", "content": [{"type": "image_url", "image_url": {"url": url}}]},
        ]});
        assert_eq!(webp_count(Api::ChatCompletions, &chat), 4);

        let responses = json!({"input": [
            {"role": "user", "content": [{"type": "input_image", "image_url": url}, {"type": "input_image", "image_url": png}]},
            {"type": "function_call", "call_id": "c", "name": "f", "arguments": json!({"image_url": url}).to_string()},
            {"type": "function_call_output", "call_id": "c", "output": [{"type": "input_image", "image_url": as_png}]},
        ]});
        assert_eq!(webp_count(Api::Responses, &responses), 2);
        assert_eq!(webp_count(Api::Responses, &json!({"input": url})), 0);

        let messages = json!({
            "system": [{"type": "image", "source": source(&webp)}],
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": source(&spaced)},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBORw0KGgoAAAANSUhEUgAA"}},
                {"type": "tool_use", "id": "t", "name": "f", "input": {"image_url": url, "source": source(&webp)}},
                {"type": "tool_result", "tool_use_id": "t", "content": [{"type": "image", "source": source(&webp)}]},
                {"type": "document", "source": {"type": "content", "content": [{"type": "image", "source": source(&webp)}]}},
                {"type": "image", "source": {"type": "url", "url": url}},
            ]}],
        });
        assert_eq!(webp_count(Api::Messages, &messages), 4);
        assert_eq!(webp_count(Api::CountTokens, &messages), 4);
        // OpenAI shapes have no Anthropic system blocks.
        assert_eq!(
            webp_count(Api::ChatCompletions, &json!({"system": messages["system"]})),
            0
        );
    }

    /// [0.0.6 webp-other-paths] The limits on turning one request's WebP pictures hold, and a
    /// refused request is left as it came: more pictures than allowed (refused before any is
    /// decoded), converted pictures over the total, too slow, and base64 that does not
    /// decode past a WebP start. Within them, every picture is turned.
    #[tokio::test]
    async fn the_limits_on_turning_webp_pictures_hold() {
        use crate::attachments::tests::{LOSSY_WEBP, test_worker};
        let worker = test_worker("convert");
        let no_worker = test_worker("hang");
        let at_once = tokio::sync::Semaphore::new(PICTURES_AT_ONCE);
        let url = format!("data:image/webp;base64,{}", b64(LOSSY_WEBP));
        let part = |url: &str| json!({"type": "image_url", "image_url": {"url": url}});
        let two = json!({"messages": [{"role": "user", "content": [part(&url), part(&url)]}]});
        let limits = |count, total, time| PictureLimits { count, total, time };
        let minute = Duration::from_secs(60);
        for (worker, limits, body, refusal) in [
            (
                &no_worker,
                limits(1, MAX_CONVERTED, minute),
                &two,
                PictureRefusal::TooMany,
            ),
            (
                &worker,
                limits(2, 100, minute),
                &two,
                PictureRefusal::TooLargeTogether,
            ),
            (
                &no_worker,
                limits(2, MAX_CONVERTED, Duration::from_millis(300)),
                &two,
                PictureRefusal::Unreadable,
            ),
            (
                &worker,
                limits(2, MAX_CONVERTED, minute),
                &json!({"messages": [{"role": "user", "content": [part("data:image/webp;base64,UklGRlwAAABXRUJQ!!!!")]}]}),
                PictureRefusal::Unreadable,
            ),
        ] {
            let mut sent = body.clone();
            let got =
                convert_webp_within(Api::ChatCompletions, &mut sent, worker, &at_once, limits)
                    .await;
            assert_eq!(got, Err(refusal));
            assert_eq!(&sent, body, "{refusal:?}");
        }
        let mut sent = two.clone();
        let got = convert_webp_within(
            Api::ChatCompletions,
            &mut sent,
            &worker,
            &at_once,
            limits(2, MAX_CONVERTED, minute),
        )
        .await;
        assert_eq!(got, Ok(()));
        for n in 0..2 {
            let url = sent["messages"][0]["content"][n]["image_url"]["url"]
                .as_str()
                .unwrap();
            assert!(url.starts_with("data:image/jpeg;base64,/9j/"), "{url:.40}");
        }
        // Nothing to turn: no worker is started (this one would never answer).
        let mut plain = json!({"messages": [{"role": "user", "content": [part("data:image/png;base64,iVBORw0KGgo=")]}]});
        let started = std::time::Instant::now();
        assert_eq!(
            convert_webp(Api::ChatCompletions, &mut plain, &no_worker, &at_once).await,
            Ok(())
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    /// [0.0.6 webp-other-paths, final round] WebP pictures that each turn within the limits
    /// but pass the request's total together are refused in words that say so and how to
    /// fix it (fewer or smaller pictures), not as one picture that is too large.
    #[tokio::test]
    async fn webp_pictures_too_large_together_are_refused_as_such() {
        use crate::attachments::tests::{LOSSY_WEBP, test_worker};
        let worker = test_worker("convert");
        let at_once = tokio::sync::Semaphore::new(PICTURES_AT_ONCE);
        let url = format!("data:image/webp;base64,{}", b64(LOSSY_WEBP));
        let part = json!({"type": "image_url", "image_url": {"url": url}});
        let request =
            |n: usize| json!({"messages": [{"role": "user", "content": vec![part.clone(); n]}]});
        // How large one picture is once turned, as base64.
        let mut one = request(1);
        convert_webp(Api::ChatCompletions, &mut one, &worker, &at_once)
            .await
            .unwrap();
        let turned = one["messages"][0]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        let size = turned.split_once(',').unwrap().1.len();
        let limits = PictureLimits {
            count: 3,
            total: size * 2 + size / 2,
            time: Duration::from_secs(60),
        };
        let mut two = request(2);
        let got =
            convert_webp_within(Api::ChatCompletions, &mut two, &worker, &at_once, limits).await;
        assert_eq!(got, Ok(()));
        let mut three = request(3);
        let refusal =
            convert_webp_within(Api::ChatCompletions, &mut three, &worker, &at_once, limits)
                .await
                .unwrap_err();
        assert_eq!(three, request(3));
        assert_eq!(
            refusal.message(),
            "The WebP pictures in this request are too large together once turned into PNG or JPEG. Send fewer or smaller pictures."
        );
        assert_eq!(
            refusal.audit_summary(),
            "Refused: the WebP pictures were too large together once turned into PNG or JPEG."
        );
    }

    /// [0.0.6 webp-other-paths] The refusals are in plain words and name the real limit.
    #[test]
    fn the_webp_refusals_are_plain_and_name_the_limit() {
        let all = [
            PictureRefusal::TooMany,
            PictureRefusal::Unreadable,
            PictureRefusal::TooLarge,
            PictureRefusal::TooLargeTogether,
        ];
        for refusal in all {
            for text in [refusal.message(), refusal.audit_summary()] {
                assert!(
                    text.contains("WebP") && text.ends_with('.') && !text.contains('!'),
                    "{text}"
                );
            }
        }
        let limit = MAX_WEBP_PICTURES.to_string();
        assert!(PictureRefusal::TooMany.message().contains(&limit));
        assert!(PictureRefusal::TooMany.audit_summary().contains(&limit));
        assert!(
            PictureRefusal::Unreadable
                .message()
                .ends_with("Send a PNG or JPEG instead.")
        );
    }
}
