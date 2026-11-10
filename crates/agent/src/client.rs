//! A streaming client for OpenAI-compatible chat completions, with the llama-server, Ollama,
//! LM Studio and vLLM extras the app needs (ARCHITECTURE.md 7.5 and 8.4).
//!
//! Proxy rules (ARCHITECTURE.md 13.6): a loopback client (the managed llama-server, a local
//! Ollama or LM Studio) is built with proxies and redirects disabled, so a system proxy never
//! sees local chat or its key. Every other endpoint uses the system proxy.

use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::error::{ProviderError, as_u32, bounded, error_from_body};
use crate::stream::{ChunkParser, StreamDecoder, StreamEvent};
use crate::types::ChatRequest;

/// The largest non-streaming response body read, in bytes. `/api/show` carries a model's
/// licence and template, which stay well below this.
const MAX_JSON_BODY: usize = 8 * 1024 * 1024;
/// The largest error body read from a failed request.
const MAX_ERROR_BODY: usize = 64 * 1024;
/// The largest body accepted when a server answers a streaming request with plain JSON.
const MAX_COMPLETION_BODY: usize = 32 * 1024 * 1024;

/// Which kind of server an endpoint is. Decides where context length and token counts come
/// from; the chat protocol is the same for all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    /// The app's managed llama-server: exact token counts, `/props`, timings.
    LlamaServer,
    /// Ollama through its OpenAI-compatible API.
    Ollama,
    /// LM Studio's local server.
    LmStudio,
    /// vLLM's OpenAI-compatible server.
    Vllm,
    /// Any other OpenAI-compatible endpoint.
    Generic,
}

/// How to reach an endpoint.
#[derive(Clone)]
pub struct ClientConfig {
    /// The server's base URL, for example `http://127.0.0.1:41234` or
    /// `https://api.example.com/v1`. `/v1` is added when the path has no version segment, and
    /// a pasted `/chat/completions` suffix is removed (see [`OpenAiClient::chat_completions_url`]).
    pub base_url: String,
    /// The API key, sent as `Authorization: Bearer`. Never logged: `Debug` redacts it.
    pub api_key: Option<String>,
    /// The kind of server.
    pub kind: ProviderKind,
    /// True for a server on this machine: the HTTP client then uses no proxy and follows no
    /// redirects. [`ClientConfig::new`] sets it from the URL's host.
    pub loopback: bool,
    /// Limit on establishing a connection.
    pub connect_timeout: Duration,
    /// For non-streaming requests, a limit on the whole request. For [`OpenAiClient::stream_chat`],
    /// a limit on the wait for the response headers and on each silence between body chunks,
    /// so a long generation is not cut off but a stalled server is. `None` waits forever; use
    /// that for the local llama-server, which reads the whole prompt before it sends its
    /// headers and can take minutes on a long prompt.
    pub request_timeout: Option<Duration>,
}

impl fmt::Debug for ClientConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .field("kind", &self.kind)
            .field("loopback", &self.loopback)
            .field("connect_timeout", &self.connect_timeout)
            .field("request_timeout", &self.request_timeout)
            .finish()
    }
}

impl ClientConfig {
    /// Defaults for an endpoint: no key, `loopback` from the URL's host (127.0.0.0/8, `::1`
    /// or `localhost`), a 10 s connect limit and a 300 s silence limit.
    pub fn new(base_url: impl Into<String>, kind: ProviderKind) -> Self {
        let base_url = base_url.into();
        let loopback = is_loopback_url(&base_url);
        Self {
            base_url,
            api_key: None,
            kind,
            loopback,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Some(Duration::from_secs(300)),
        }
    }

    /// The managed llama-server on `127.0.0.1:port` with its per-launch key: loopback, a 5 s
    /// connect limit and no request limit (cancellation stops a turn instead).
    pub fn llama_server(port: u16, api_key: impl Into<String>) -> Self {
        Self {
            base_url: format!("http://127.0.0.1:{port}"),
            api_key: Some(api_key.into()),
            kind: ProviderKind::LlamaServer,
            loopback: true,
            connect_timeout: Duration::from_secs(5),
            request_timeout: None,
        }
    }
}

fn is_loopback_url(base_url: &str) -> bool {
    let Ok((url, _)) = parse_base(base_url) else {
        return false;
    };
    match url.host() {
        Some(url::Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip).is_loopback(),
        Some(url::Host::Ipv6(ip)) => {
            IpAddr::V6(ip).is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        None => false,
    }
}

/// A model an endpoint offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// The id to put in `ChatRequest::model`.
    pub id: String,
    /// The context the server reports for it, when the listing says (llama-server's allocated
    /// context, vLLM's `max_model_len`). Never a trained maximum.
    pub context_length: Option<u32>,
}

/// A stream of chat events. Ends with [`StreamEvent::Done`], or with one error. Dropping it
/// closes the HTTP connection, which is how llama-server learns to stop generating.
pub struct ChatStream {
    inner: BoxStream<'static, Result<StreamEvent, ProviderError>>,
}

impl fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChatStream").finish_non_exhaustive()
    }
}

impl Stream for ChatStream {
    type Item = Result<StreamEvent, ProviderError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx)
    }
}

/// Client for one OpenAI-compatible endpoint.
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    config: ClientConfig,
    /// The OpenAI API root, ending in a version segment (`.../v1`).
    api: Url,
    /// The server root for vendor endpoints (`/props`, `/api/ps`, `/api/v1/models`).
    root: Url,
}

impl OpenAiClient {
    /// Builds a client and its HTTP connection pool.
    ///
    /// # Errors
    /// [`ProviderError::InvalidRequest`] for a base URL that is not http(s);
    /// [`ProviderError::Network`] if the HTTP client cannot be built (for example no TLS
    /// backend).
    pub fn new(config: ClientConfig) -> Result<Self, ProviderError> {
        let mut builder = reqwest::Client::builder().connect_timeout(config.connect_timeout);
        if config.loopback {
            builder = builder
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none());
        }
        let http = builder
            .build()
            .map_err(|err| ProviderError::Network(error_chain(&err)))?;
        Self::with_http_client(config, http)
    }

    /// Uses an HTTP client built elsewhere (for example by a central network module). The
    /// caller is responsible for its proxy and redirect settings.
    ///
    /// # Errors
    /// [`ProviderError::InvalidRequest`] for a base URL that is not http(s).
    pub fn with_http_client(
        config: ClientConfig,
        http: reqwest::Client,
    ) -> Result<Self, ProviderError> {
        let (api, root) = parse_base(&config.base_url).map_err(ProviderError::InvalidRequest)?;
        Ok(Self {
            http,
            config,
            api,
            root,
        })
    }

    /// The configuration this client was built from.
    pub fn config(&self) -> &ClientConfig {
        &self.config
    }

    /// The chat-completions URL requests go to.
    pub fn chat_completions_url(&self) -> Url {
        join(&self.api, "chat/completions")
    }

    /// Streams a chat completion. The request is always sent with `stream: true` and
    /// `stream_options.include_usage`.
    ///
    /// The HTTP status is checked before the body is read as a stream, so an overflow or a
    /// bad key comes back here as an error rather than inside the stream. Cancelling `cancel`
    /// makes the stream yield [`ProviderError::Cancelled`] and drops the connection at once.
    /// A server that answers with a plain JSON completion instead of a stream is accepted.
    ///
    /// # Errors
    /// Any [`ProviderError`]; the stream's own items carry errors that happen later.
    pub async fn stream_chat(
        &self,
        req: &ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        req.validate_media()
            .map_err(ProviderError::InvalidRequest)?;
        let body = req.to_body(true);
        let request = self
            .authorize(self.http.post(self.chat_completions_url()))
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body);
        let limit = self.config.request_timeout;
        let send = within(limit, "the server sent no response", async {
            request.send().await.map_err(network_error)
        });
        let response = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(ProviderError::Cancelled),
            response = send => response?,
        };
        let status = response.status();
        if !status.is_success() {
            let reason = status.canonical_reason().unwrap_or("").to_owned();
            let read = within(
                limit,
                "the server did not finish its error response",
                read_capped(response, MAX_ERROR_BODY, true),
            );
            let body = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                body = read => body?,
            };
            return Err(error_from_body(Some(status.as_u16()), &body, &reason));
        }
        let is_json = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.trim_start().starts_with("application/json"));
        if is_json {
            let read = within(
                limit,
                "the server did not finish its response",
                read_capped(response, MAX_COMPLETION_BODY, false),
            );
            let body = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                body = read => body?,
            };
            let events = events_from_completion(&body)?;
            // Same state machine as a live stream, already finished, so cancelling still
            // yields `Cancelled` instead of the queued events.
            let state = StreamState {
                body: None,
                decoder: StreamDecoder::new(),
                pending: events.into_iter().map(Ok).collect(),
                cancel,
                idle_limit: None,
                finished: true,
            };
            return Ok(ChatStream {
                inner: futures_util::stream::unfold(state, next_item).boxed(),
            });
        }
        let state = StreamState {
            body: Some(response.bytes_stream().boxed()),
            decoder: StreamDecoder::new(),
            pending: VecDeque::new(),
            cancel,
            idle_limit: self.config.request_timeout,
            finished: false,
        };
        Ok(ChatStream {
            inner: futures_util::stream::unfold(state, next_item).boxed(),
        })
    }

    /// Lists the endpoint's models from `/v1/models`. Context length comes from llama-server's
    /// `meta.n_ctx` and vLLM's `max_model_len`; for a generic endpoint from `context_length`
    /// or `context_window` when present. Ollama and LM Studio listings carry none.
    ///
    /// # Errors
    /// Any [`ProviderError`].
    pub async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        let value = self.get_json(join(&self.api, "models")).await?;
        let entries = value
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| value.get("models").and_then(Value::as_array))
            .ok_or_else(|| {
                ProviderError::Decode("the model list has no 'data' array".to_owned())
            })?;
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let id = ["id", "name", "model"]
                    .iter()
                    .find_map(|key| entry.get(*key).and_then(Value::as_str))
                    .filter(|id| !id.is_empty())?;
                Some(ModelInfo {
                    id: id.to_owned(),
                    context_length: self.listed_context_length(entry),
                })
            })
            .collect())
    }

    fn listed_context_length(&self, entry: &Value) -> Option<u32> {
        match self.config.kind {
            ProviderKind::LlamaServer => entry.pointer("/meta/n_ctx").and_then(as_u32),
            ProviderKind::Vllm => entry.get("max_model_len").and_then(as_u32),
            ProviderKind::Ollama | ProviderKind::LmStudio => None,
            ProviderKind::Generic => ["context_length", "context_window", "max_model_len"]
                .iter()
                .find_map(|key| entry.get(*key).and_then(as_u32)),
        }
    }

    /// The context window the server actually uses for `model`, never a trained maximum
    /// (ARCHITECTURE.md 8.4):
    ///
    /// - llama-server: `/props` → `default_generation_settings.n_ctx`;
    /// - Ollama: the running model's `context_length` from `GET /api/ps`, else `num_ctx` in
    ///   `/api/show`'s parameters;
    /// - LM Studio: `loaded_instances[].config.context_length` from `/api/v1/models`; on an
    ///   older LM Studio without that API, `loaded_context_length` from `/api/v0/models` when
    ///   present (its `max_context_length` is the trained maximum and is not used);
    /// - vLLM and generic: the model list.
    ///
    /// `Ok(None)` means the server does not say; the app then uses the user's setting
    /// (default 4,096).
    ///
    /// # Errors
    /// Any [`ProviderError`] other than "endpoint not found", which gives `Ok(None)`.
    pub async fn context_length(&self, model: &str) -> Result<Option<u32>, ProviderError> {
        match self.config.kind {
            ProviderKind::LlamaServer => {
                let props = self.get_json(join(&self.root, "props")).await?;
                Ok(props
                    .pointer("/default_generation_settings/n_ctx")
                    .and_then(as_u32))
            }
            ProviderKind::Ollama => self.ollama_context_length(model).await,
            ProviderKind::LmStudio => self.lm_studio_context_length(model).await,
            ProviderKind::Vllm | ProviderKind::Generic => match self.list_models().await {
                Ok(models) => Ok(models
                    .into_iter()
                    .find(|info| info.id == model)
                    .and_then(|info| info.context_length)),
                Err(err) if is_not_found(&err) => Ok(None),
                Err(err) => Err(err),
            },
        }
    }

    async fn ollama_context_length(&self, model: &str) -> Result<Option<u32>, ProviderError> {
        // A server without `/api/ps` (an old Ollama, or not Ollama at all) falls through to
        // `/api/show`, and then to "does not say".
        let running = match self.get_json(join(&self.root, "api/ps")).await {
            Ok(running) => running,
            Err(err) if is_not_found(&err) => Value::Null,
            Err(err) => return Err(err),
        };
        if let Some(models) = running.get("models").and_then(Value::as_array) {
            for entry in models {
                let names = ["name", "model"]
                    .iter()
                    .filter_map(|key| entry.get(*key).and_then(Value::as_str));
                if names.into_iter().any(|name| ollama_same_model(name, model))
                    && let Some(length) = entry.get("context_length").and_then(as_u32)
                {
                    return Ok(Some(length));
                }
            }
        }
        let shown = match self
            .post_json(join(&self.root, "api/show"), &json!({ "model": model }))
            .await
        {
            Ok(shown) => shown,
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => return Err(err),
        };
        Ok(shown
            .get("parameters")
            .and_then(Value::as_str)
            .and_then(num_ctx_parameter))
    }

    async fn lm_studio_context_length(&self, model: &str) -> Result<Option<u32>, ProviderError> {
        match self.get_json(join(&self.root, "api/v1/models")).await {
            Ok(listing) => {
                let models = listing
                    .get("models")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                for entry in models {
                    let instances = entry
                        .get("loaded_instances")
                        .and_then(Value::as_array)
                        .map_or(&[][..], Vec::as_slice);
                    let key_matches = entry.get("key").and_then(Value::as_str) == Some(model);
                    for instance in instances {
                        let instance_matches =
                            instance.get("id").and_then(Value::as_str) == Some(model);
                        if (key_matches || instance_matches)
                            && let Some(length) =
                                instance.pointer("/config/context_length").and_then(as_u32)
                        {
                            return Ok(Some(length));
                        }
                    }
                }
                Ok(None)
            }
            Err(err) if is_not_found(&err) => {
                let listing = match self.get_json(join(&self.root, "api/v0/models")).await {
                    Ok(listing) => listing,
                    Err(err) if is_not_found(&err) => return Ok(None),
                    Err(err) => return Err(err),
                };
                let models = listing
                    .get("data")
                    .and_then(Value::as_array)
                    .map_or(&[][..], Vec::as_slice);
                Ok(models
                    .iter()
                    .find(|entry| entry.get("id").and_then(Value::as_str) == Some(model))
                    .filter(|entry| entry.get("state").and_then(Value::as_str) == Some("loaded"))
                    .and_then(|entry| entry.get("loaded_context_length"))
                    .and_then(as_u32))
            }
            Err(err) => Err(err),
        }
    }

    /// Counts the prompt tokens of `req` exactly, without running it. llama-server only:
    /// `POST /v1/chat/completions/input_tokens` with `stream` removed, falling back to
    /// `/apply-template` plus `/tokenize` on a build without that endpoint. `Ok(None)` for
    /// every other kind of server.
    ///
    /// # Errors
    /// Any [`ProviderError`].
    pub async fn count_tokens(&self, req: &ChatRequest) -> Result<Option<u32>, ProviderError> {
        if self.config.kind != ProviderKind::LlamaServer {
            return Ok(None);
        }
        req.validate_media()
            .map_err(ProviderError::InvalidRequest)?;
        let mut body = req.to_body(false);
        if let Value::Object(map) = &mut body {
            map.remove("stream");
            map.remove("stream_options");
        }
        match self
            .post_json(join(&self.api, "chat/completions/input_tokens"), &body)
            .await
        {
            Ok(value) => value
                .get("input_tokens")
                .and_then(as_u32)
                .map(Some)
                .ok_or_else(|| {
                    ProviderError::Decode("the token count has no 'input_tokens'".to_owned())
                }),
            Err(err) if is_not_found(&err) => self.count_by_template(&body).await.map(Some),
            Err(err) => Err(err),
        }
    }

    async fn count_by_template(&self, body: &Value) -> Result<u32, ProviderError> {
        let applied = self
            .post_json(join(&self.root, "apply-template"), body)
            .await?;
        let prompt = applied
            .get("prompt")
            .and_then(Value::as_str)
            .ok_or_else(|| ProviderError::Decode("apply-template returned no prompt".to_owned()))?;
        let tokens = self
            .post_json(
                join(&self.root, "tokenize"),
                &json!({ "content": prompt, "add_special": false, "parse_special": true }),
            )
            .await?;
        let count = tokens
            .get("tokens")
            .and_then(Value::as_array)
            .map(Vec::len)
            .ok_or_else(|| ProviderError::Decode("tokenize returned no tokens".to_owned()))?;
        u32::try_from(count).map_err(|_| ProviderError::Decode("token count overflow".to_owned()))
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.config.api_key.as_deref().filter(|key| !key.is_empty()) {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    async fn get_json(&self, url: Url) -> Result<Value, ProviderError> {
        self.send_json(self.http.get(url)).await
    }

    async fn post_json(&self, url: Url, body: &Value) -> Result<Value, ProviderError> {
        self.send_json(self.http.post(url).json(body)).await
    }

    async fn send_json(&self, request: reqwest::RequestBuilder) -> Result<Value, ProviderError> {
        let mut request = self.authorize(request);
        if let Some(limit) = self.config.request_timeout {
            request = request.timeout(limit);
        }
        let response = request.send().await.map_err(network_error)?;
        let status = response.status();
        if !status.is_success() {
            let reason = status.canonical_reason().unwrap_or("").to_owned();
            let body = read_capped(response, MAX_ERROR_BODY, true).await?;
            return Err(error_from_body(Some(status.as_u16()), &body, &reason));
        }
        let body = read_capped(response, MAX_JSON_BODY, false).await?;
        serde_json::from_slice(&body)
            .map_err(|err| ProviderError::Decode(format!("the response is not JSON ({err})")))
    }
}

/// Splits a user-typed base URL into the OpenAI API root (ending in a version segment) and
/// the server root.
///
/// - A missing scheme gets `http://`; only http and https are accepted.
/// - A pasted `/chat/completions` or `/models` suffix is removed.
/// - If no path segment looks like a version (`v1`, `v1beta`, `v4`), `/v1` is appended:
///   `http://127.0.0.1:8080` → `http://127.0.0.1:8080/v1`. Otherwise the path is kept, so
///   `https://host/v1beta/openai` stays as typed.
/// - The server root is the path without a trailing version segment.
fn parse_base(raw: &str) -> Result<(Url, Url), String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("the base URL is empty".to_owned());
    }
    let mut url = match Url::parse(trimmed) {
        Ok(url) if url.scheme() == "http" || url.scheme() == "https" => url,
        Ok(_) | Err(_) if !trimmed.contains("://") => Url::parse(&format!("http://{trimmed}"))
            .map_err(|err| format!("'{trimmed}' is not a valid URL ({err})"))?,
        Ok(url) => {
            return Err(format!(
                "the base URL must start with http:// or https://, not {}://",
                url.scheme()
            ));
        }
        Err(err) => return Err(format!("'{trimmed}' is not a valid URL ({err})")),
    };
    if url.host().is_none() {
        return Err(format!("'{trimmed}' has no host"));
    }
    url.set_fragment(None);
    let mut segments: Vec<String> = url
        .path_segments()
        .map(|segments| {
            segments
                .filter(|segment| !segment.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    if segments.ends_with(&["chat".to_owned(), "completions".to_owned()]) {
        segments.truncate(segments.len() - 2);
    } else if segments.last().is_some_and(|last| last == "models") {
        segments.pop();
    }
    let mut api_segments = segments.clone();
    if !segments.iter().any(|segment| is_version_segment(segment)) {
        api_segments.push("v1".to_owned());
    }
    let mut root_segments = segments;
    if root_segments
        .last()
        .is_some_and(|segment| is_version_segment(segment))
    {
        root_segments.pop();
    }
    let mut api = url.clone();
    api.set_path(&format!("/{}", api_segments.join("/")));
    let mut root = url;
    root.set_path(&format!("/{}", root_segments.join("/")));
    Ok((api, root))
}

/// `v` followed by a digit and then only letters or digits: `v1`, `v1beta`, `v4`.
fn is_version_segment(segment: &str) -> bool {
    let mut chars = segment.chars();
    chars.next() == Some('v')
        && chars.next().is_some_and(|c| c.is_ascii_digit())
        && chars.all(|c| c.is_ascii_alphanumeric())
}

/// Appends a relative path to a base URL, keeping its query string.
fn join(base: &Url, path: &str) -> Url {
    let mut url = base.clone();
    let joined = format!("{}/{}", base.path().trim_end_matches('/'), path);
    url.set_path(&joined);
    url
}

/// Runs `future`, failing with a [`ProviderError::Network`] that starts with `what` when a
/// `limit` is set and passes first.
async fn within<T>(
    limit: Option<Duration>,
    what: &str,
    future: impl Future<Output = Result<T, ProviderError>>,
) -> Result<T, ProviderError> {
    match limit {
        Some(limit) => tokio::time::timeout(limit, future)
            .await
            .map_err(|_| ProviderError::Network(format!("{what} within {}", describe(limit))))?,
        None => future.await,
    }
}

/// A duration for a message: whole seconds, or milliseconds below one second.
fn describe(duration: Duration) -> String {
    if duration < Duration::from_secs(1) {
        format!("{} ms", duration.as_millis())
    } else {
        format!("{} s", duration.as_secs())
    }
}

fn ollama_same_model(listed: &str, requested: &str) -> bool {
    fn normalise(name: &str) -> &str {
        name.strip_suffix(":latest").unwrap_or(name)
    }
    normalise(listed) == normalise(requested)
}

/// The value of a `num_ctx` line in Ollama's `parameters` text (`num_ctx    8192`).
fn num_ctx_parameter(parameters: &str) -> Option<u32> {
    parameters.lines().find_map(|line| {
        let mut words = line.split_whitespace();
        (words.next() == Some("num_ctx"))
            .then(|| words.next())
            .flatten()
            .and_then(|value| value.parse().ok())
    })
}

fn is_not_found(err: &ProviderError) -> bool {
    matches!(
        err,
        ProviderError::Http {
            status: 404 | 405 | 501,
            ..
        }
    )
}

/// The full error chain of a reqwest error with URLs removed (a URL can carry credentials).
fn network_error(err: reqwest::Error) -> ProviderError {
    if err.is_timeout() {
        return ProviderError::Network("the request timed out".to_owned());
    }
    ProviderError::Network(error_chain(&err.without_url()))
}

fn error_chain(err: &dyn std::error::Error) -> String {
    let mut message = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        let text = cause.to_string();
        if !message.contains(&text) {
            message.push_str(": ");
            message.push_str(&text);
        }
        source = cause.source();
    }
    bounded(&message)
}

/// Reads a response body up to `cap` bytes. With `truncate`, the rest is dropped (enough of
/// an error body to classify it); otherwise a longer body is an error.
async fn read_capped(
    mut response: reqwest::Response,
    cap: usize,
    truncate: bool,
) -> Result<Vec<u8>, ProviderError> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len() + chunk.len() > cap {
            if truncate {
                body.extend_from_slice(&chunk[..cap - body.len()]);
                return Ok(body);
            }
            return Err(ProviderError::Decode(format!(
                "the response is larger than {} MiB",
                cap / (1024 * 1024)
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Events for a server that answered a streaming request with a whole JSON completion.
fn events_from_completion(body: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| ProviderError::Decode(format!("the response is not JSON ({err})")))?;
    let has_error = value.get("error").is_some_and(|e| !e.is_null())
        || value.get("object").and_then(Value::as_str) == Some("error");
    if !has_error && !value.get("choices").is_some_and(Value::is_array) {
        // Not a completion (for example `{"detail": ...}` with status 200): it must not look
        // like a model that answered with nothing.
        return Err(ProviderError::Decode(
            "the JSON response is not a chat completion (it has no 'choices')".to_owned(),
        ));
    }
    let mut parser = ChunkParser::new();
    let mut events = Vec::new();
    parser.push_chunk(&value, &mut events)?;
    parser.finish(&mut events);
    Ok(events)
}

struct StreamState {
    /// The response body; `None` once the stream finished, so the connection is released.
    body: Option<BoxStream<'static, reqwest::Result<Bytes>>>,
    decoder: StreamDecoder,
    pending: VecDeque<Result<StreamEvent, ProviderError>>,
    cancel: CancellationToken,
    idle_limit: Option<Duration>,
    finished: bool,
}

impl StreamState {
    /// Ends the stream: drops the connection and any queued events.
    fn end(&mut self) {
        self.body = None;
        self.finished = true;
    }

    fn fail(&mut self, err: ProviderError) {
        self.end();
        self.pending.clear();
        self.pending.push_back(Err(err));
    }
}

enum Next {
    Chunk(Option<reqwest::Result<Bytes>>),
    Silent(Duration),
}

async fn next_item(
    mut state: StreamState,
) -> Option<(Result<StreamEvent, ProviderError>, StreamState)> {
    loop {
        if state.cancel.is_cancelled() {
            if state.finished && state.pending.is_empty() {
                // Everything was delivered (or Cancelled already was).
                return None;
            }
            // Cancellation wins over queued events: the caller has stopped listening.
            state.end();
            state.pending.clear();
            return Some((Err(ProviderError::Cancelled), state));
        }
        if let Some(item) = state.pending.pop_front() {
            return Some((item, state));
        }
        if state.finished {
            return None;
        }
        let Some(body) = state.body.as_mut() else {
            state.finished = true;
            continue;
        };
        let idle_limit = state.idle_limit;
        let next = async {
            match idle_limit {
                Some(limit) => match tokio::time::timeout(limit, body.next()).await {
                    Ok(chunk) => Next::Chunk(chunk),
                    Err(_) => Next::Silent(limit),
                },
                None => Next::Chunk(body.next().await),
            }
        };
        let next = tokio::select! {
            biased;
            () = state.cancel.cancelled() => continue,
            next = next => next,
        };
        let mut events = Vec::new();
        match next {
            Next::Silent(limit) => state.fail(ProviderError::Network(format!(
                "the server sent nothing for {}",
                describe(limit)
            ))),
            Next::Chunk(Some(Ok(bytes))) => {
                let result = state.decoder.push(&bytes, &mut events);
                state.pending.extend(events.into_iter().map(Ok));
                if let Err(err) = result {
                    state.end();
                    state.pending.push_back(Err(err));
                } else if state.decoder.is_done() {
                    state.end();
                }
            }
            Next::Chunk(Some(Err(err))) => {
                state.end();
                state.pending.push_back(Err(network_error(err)));
            }
            Next::Chunk(None) => {
                let result = state.decoder.finish(&mut events);
                state.pending.extend(events.into_iter().map(Ok));
                if let Err(err) = result {
                    state.pending.push_back(Err(err));
                }
                state.end();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(raw: &str) -> (String, String) {
        let (api, root) = parse_base(raw).unwrap();
        (api.to_string(), root.to_string())
    }

    #[test]
    fn base_url_normalisation() {
        assert_eq!(
            urls("http://127.0.0.1:8080"),
            (
                "http://127.0.0.1:8080/v1".into(),
                "http://127.0.0.1:8080/".into()
            )
        );
        assert_eq!(
            urls("http://127.0.0.1:8080/v1/"),
            (
                "http://127.0.0.1:8080/v1".into(),
                "http://127.0.0.1:8080/".into()
            )
        );
        assert_eq!(
            urls(" localhost:11434 "),
            (
                "http://localhost:11434/v1".into(),
                "http://localhost:11434/".into()
            )
        );
        assert_eq!(
            urls("https://api.example.com/v1/chat/completions"),
            (
                "https://api.example.com/v1".into(),
                "https://api.example.com/".into()
            )
        );
        assert_eq!(
            urls("https://openrouter.ai/api/v1"),
            (
                "https://openrouter.ai/api/v1".into(),
                "https://openrouter.ai/api".into()
            )
        );
        assert_eq!(
            urls("https://generativelanguage.googleapis.com/v1beta/openai/"),
            (
                "https://generativelanguage.googleapis.com/v1beta/openai".into(),
                "https://generativelanguage.googleapis.com/v1beta/openai".into()
            )
        );
        assert_eq!(
            urls("https://proxy.example.com/llm"),
            (
                "https://proxy.example.com/llm/v1".into(),
                "https://proxy.example.com/llm".into()
            )
        );
        assert_eq!(
            urls("http://host:8000/v1/models"),
            ("http://host:8000/v1".into(), "http://host:8000/".into())
        );
        assert_eq!(
            urls("https://host/v1?api-version=2#frag"),
            (
                "https://host/v1?api-version=2".into(),
                "https://host/?api-version=2".into()
            )
        );
    }

    #[test]
    fn join_keeps_query() {
        let (api, _) = parse_base("https://host/v1?x=1").unwrap();
        assert_eq!(
            join(&api, "chat/completions").to_string(),
            "https://host/v1/chat/completions?x=1"
        );
    }

    #[test]
    fn bad_base_urls() {
        assert!(parse_base("").is_err());
        assert!(parse_base("ftp://host/v1").is_err());
        assert!(parse_base("file:///etc").is_err());
        assert!(parse_base("http://").is_err());
    }

    #[test]
    fn version_segments() {
        for yes in ["v1", "v1beta", "v4", "v2alpha1"] {
            assert!(is_version_segment(yes), "{yes}");
        }
        for no in ["v", "vx", "api", "v1-ish", "openai", "V1"] {
            assert!(!is_version_segment(no), "{no}");
        }
    }

    #[test]
    fn loopback_detection() {
        assert!(ClientConfig::new("http://127.0.0.1:11434", ProviderKind::Ollama).loopback);
        assert!(ClientConfig::new("http://localhost:1234", ProviderKind::LmStudio).loopback);
        assert!(ClientConfig::new("http://[::1]:8000", ProviderKind::Vllm).loopback);
        assert!(ClientConfig::new("http://[::ffff:127.0.0.1]:8000", ProviderKind::Vllm).loopback);
        assert!(!ClientConfig::new("http://[::ffff:10.0.0.1]:8000", ProviderKind::Vllm).loopback);
        assert!(ClientConfig::new("127.0.0.2:8080", ProviderKind::Generic).loopback);
        assert!(!ClientConfig::new("http://192.168.1.5:11434", ProviderKind::Ollama).loopback);
        assert!(!ClientConfig::new("https://api.example.com/v1", ProviderKind::Generic).loopback);
        assert!(!ClientConfig::new("http://localhost.example.com", ProviderKind::Generic).loopback);
    }

    #[test]
    fn debug_redacts_the_key() {
        let config = ClientConfig::llama_server(1234, "secret-key");
        let text = format!("{config:?}");
        assert!(!text.contains("secret-key"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn ollama_parameters() {
        assert_eq!(
            num_ctx_parameter(
                "stop \"<|im_end|>\"\nnum_ctx                        8192\ntemperature 0.6"
            ),
            Some(8192)
        );
        assert_eq!(num_ctx_parameter("temperature 0.6"), None);
        assert!(ollama_same_model("qwen3:latest", "qwen3"));
        assert!(ollama_same_model("qwen3:8b", "qwen3:8b"));
        assert!(!ollama_same_model("qwen3:8b", "qwen3"));
    }

    fn assert_send<T: Send>(_: &T) {}
    fn assert_stream_bounds<T: Stream + Send + Unpin + 'static>() {}

    /// Tauri commands and spawned tasks need `Send` futures; this fails to compile otherwise.
    #[test]
    fn futures_and_stream_are_send() {
        let client = OpenAiClient::new(ClientConfig::llama_server(1, "k")).unwrap();
        let request = ChatRequest::new("m", vec![]);
        assert_send(&client.stream_chat(&request, CancellationToken::new()));
        assert_send(&client.list_models());
        assert_send(&client.context_length("m"));
        assert_send(&client.count_tokens(&request));
        assert_stream_bounds::<ChatStream>();
    }

    #[test]
    fn json_without_choices_is_not_an_empty_reply() {
        assert!(matches!(
            events_from_completion(br#"{"detail":"Not Found"}"#),
            Err(ProviderError::Decode(_))
        ));
        assert!(matches!(
            events_from_completion(br#"{"error":{"message":"boom","code":400}}"#),
            Err(ProviderError::Http { status: 400, .. })
        ));
        assert_eq!(
            events_from_completion(br#"{"choices":[]}"#).unwrap(),
            vec![StreamEvent::Done]
        );
    }

    #[test]
    fn invalid_config_is_rejected() {
        let err =
            OpenAiClient::new(ClientConfig::new("gopher://x", ProviderKind::Generic)).unwrap_err();
        assert!(matches!(err, ProviderError::InvalidRequest(_)));
    }
}
