//! A streaming client for OpenAI-compatible chat completions, with the llama-server, Ollama,
//! LM Studio and vLLM extras the app needs (ARCHITECTURE.md 7.5 and 8.4), and (0.0.7) for
//! Claude's own Messages API: an endpoint of kind [`ProviderKind::Anthropic`] gets the same
//! [`ChatRequest`] translated by [`crate::anthropic`] and gives back the same
//! [`StreamEvent`]s, so the turn runner works with either protocol unchanged.
//!
//! Proxy rules (ARCHITECTURE.md 13.6): a loopback client (the managed llama-server, a local
//! Ollama or LM Studio) is built with proxies and redirects disabled, so a system proxy never
//! sees local chat or its key; (0.0.7) neither does a server at a local network address.
//! Every other endpoint uses the system proxy. Every client but the loopback one follows
//! redirects only within its own origin. A key is never sent over plain `http://` to a server
//! on the internet ([`crate::cloud`]).

use std::collections::VecDeque;
use std::fmt;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::anthropic::{MessagesDecoder, ThinkingReplay};
use crate::cloud;
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
/// Requests sent again for one chat request after a server refused a parameter (0.0.7).
const MAX_FIX_ROUNDS: usize = 3;

/// Which kind of server an endpoint is. Decides where context length and token counts come
/// from, which request fields the server accepts, and (0.0.7) the chat protocol: Chat
/// Completions for all but [`ProviderKind::Anthropic`], which speaks Claude's Messages API.
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
    /// Anthropic (Claude) through its own Messages API (0.0.7).
    Anthropic,
    /// OpenAI's Chat Completions API (0.0.7): only OpenAI's own fields,
    /// `max_completion_tokens`, chat models only in the model list.
    OpenAi,
    /// OpenRouter's OpenAI-compatible API (0.0.7).
    OpenRouter,
    /// abliteration.ai's OpenAI-compatible API (0.0.7): only the fields it lists.
    Abliteration,
}

/// How to reach an endpoint.
#[derive(Clone)]
pub struct ClientConfig {
    /// The server's base URL, for example `http://127.0.0.1:41234` or
    /// `https://api.example.com/v1`. `/v1` is added when the path has no version segment, and
    /// a pasted `/chat/completions` suffix is removed (see [`OpenAiClient::chat_completions_url`]).
    pub base_url: String,
    /// The API key, sent as `Authorization: Bearer` (to Anthropic as `x-api-key`). Never
    /// logged: `Debug` redacts it.
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

/// A model in an endpoint's list, with what the list says about it (0.0.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedModel {
    /// The id to put in `ChatRequest::model`.
    pub id: String,
    /// A name for people, when the list has one (Anthropic's `display_name`, OpenRouter's and
    /// abliteration.ai's `name`).
    pub display_name: Option<String>,
    /// The context the server reports for it, as in [`ModelInfo::context_length`].
    pub context_length: Option<u32>,
}

/// One token from [`OpenAiClient::tokenize`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// The token id in the model's vocabulary.
    pub id: i64,
    /// The token's text, when the server sent it and it is valid UTF-8.
    pub piece: Option<String>,
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

/// What sending a streaming request gave.
pub(crate) enum Sent {
    /// The server accepted it; the body is still unread.
    Accepted(reqwest::Response),
    /// The server refused it with an error status (after any retries).
    Refused {
        /// The HTTP status.
        status: u16,
        /// The start of the error body.
        body: Vec<u8>,
        /// The status's canonical phrase.
        reason: String,
        /// How long the server asked to wait (`retry-after`), when it said.
        retry_after: Option<Duration>,
    },
}

/// How a response's events are read: chat-completion chunks or Claude's event stream.
pub(crate) enum WireDecoder {
    /// Chat Completions.
    Chat(StreamDecoder),
    /// Claude's Messages API.
    Messages(MessagesDecoder),
}

impl WireDecoder {
    fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        match self {
            Self::Chat(decoder) => decoder.push(bytes, out),
            Self::Messages(decoder) => decoder.push(bytes, out),
        }
    }

    fn finish(&mut self, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        match self {
            Self::Chat(decoder) => decoder.finish(out),
            Self::Messages(decoder) => decoder.finish(out),
        }
    }

    fn is_done(&self) -> bool {
        match self {
            Self::Chat(decoder) => decoder.is_done(),
            Self::Messages(decoder) => decoder.is_done(),
        }
    }

    /// Events for a server that answered a streaming request with one JSON body.
    fn whole(&self, body: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
        match self {
            Self::Chat(_) => events_from_completion(body),
            Self::Messages(_) => crate::anthropic::events_from_message(body),
        }
    }
}

/// Client for one endpoint: OpenAI-compatible, or (0.0.7) Claude's Messages API for
/// [`ProviderKind::Anthropic`].
#[derive(Debug, Clone)]
pub struct OpenAiClient {
    http: reqwest::Client,
    config: ClientConfig,
    /// The OpenAI API root, ending in a version segment (`.../v1`).
    api: Url,
    /// The server root for vendor endpoints (`/props`, `/api/ps`, `/api/v1/models`).
    root: Url,
    /// Claude's thinking blocks to send back within a tool-use loop (shared by clones).
    replay: Arc<Mutex<ThinkingReplay>>,
}

/// [0.0.7 P5] The redirect rule of a client for a server that is not on this machine: at most
/// ten redirects, each to the scheme, host and port the request started with. So the chat and
/// the API key never reach a host the user did not add, nor plain `http://` after `https://`
/// (reqwest drops `Authorization` on a redirect to another host, but not a key in a header of
/// the API's own, such as Anthropic's `x-api-key`). A redirect elsewhere ends the request with
/// the redirect's own status.
fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let origin = |url: &reqwest::Url| {
            (
                url.scheme().to_owned(),
                url.host_str().map(str::to_ascii_lowercase),
                url.port_or_known_default(),
            )
        };
        let home = attempt.previous().first().map(origin);
        if home == Some(origin(attempt.url())) && attempt.previous().len() <= 10 {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

impl OpenAiClient {
    /// Builds a client and its HTTP connection pool.
    ///
    /// # Errors
    /// [`ProviderError::InvalidRequest`] for a base URL that is not http(s), or for a key
    /// that would travel over plain `http://` to a server on the internet;
    /// [`ProviderError::Network`] if the HTTP client cannot be built (for example no TLS
    /// backend).
    pub fn new(config: ClientConfig) -> Result<Self, ProviderError> {
        let mut builder = reqwest::Client::builder().connect_timeout(config.connect_timeout);
        if config.loopback {
            builder = builder
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none());
        } else {
            builder = builder.redirect(same_origin_redirects());
            // [0.0.7 P5] A server at a local network address is reached directly: a system
            // proxy would see its plain-http traffic, key included.
            if cloud::endpoint_place(&config.base_url) == cloud::EndpointPlace::LocalNetwork {
                builder = builder.no_proxy();
            }
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
    /// [`ProviderError::InvalidRequest`] for a base URL that is not http(s), or for a key
    /// that would travel over plain `http://` to a server on the internet.
    pub fn with_http_client(
        config: ClientConfig,
        http: reqwest::Client,
    ) -> Result<Self, ProviderError> {
        let (api, root) = parse_base(&config.base_url).map_err(ProviderError::InvalidRequest)?;
        if config
            .api_key
            .as_deref()
            .is_some_and(|key| !key.trim().is_empty())
        {
            cloud::check_key_transport(&config.base_url).map_err(ProviderError::InvalidRequest)?;
        }
        Ok(Self {
            http,
            config,
            api,
            root,
            replay: Arc::default(),
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

    /// The API root (`.../v1`).
    pub(crate) fn api_url(&self) -> &Url {
        &self.api
    }

    /// Claude's thinking blocks kept for this endpoint's tool-use loops.
    pub(crate) fn replay(&self) -> &Arc<Mutex<ThinkingReplay>> {
        &self.replay
    }

    /// Streams a chat completion. The request is always sent with `stream: true` and
    /// `stream_options.include_usage`. (0.0.7) An endpoint of kind
    /// [`ProviderKind::Anthropic`] is sent the same request in Claude's Messages API form;
    /// any other endpoint but llama-server gets only the fields its server accepts, and a
    /// request refused for an unsupported parameter is sent again once without it. Requests
    /// to a cloud company are tried again on a rate limit (when the server says how long to
    /// wait), on overload and on server errors.
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
        let result = if self.config.kind == ProviderKind::Anthropic {
            crate::anthropic::stream(self, req, cancel).await
        } else {
            self.stream_completion(req, cancel).await
        };
        let key = self.config.api_key.clone();
        match result {
            // [0.0.7 P5] Errors inside the stream (an `error` event, an error object in a
            // chunk) can echo the key too.
            Ok(stream) => Ok(ChatStream {
                inner: stream
                    .inner
                    .map(move |item| item.map_err(|err| cloud::scrub_key(err, key.as_deref())))
                    .boxed(),
            }),
            Err(err) => Err(cloud::scrub_key(err, key.as_deref())),
        }
    }

    async fn stream_completion(
        &self,
        req: &ChatRequest,
        cancel: CancellationToken,
    ) -> Result<ChatStream, ProviderError> {
        req.validate_media()
            .map_err(ProviderError::InvalidRequest)?;
        let kind = self.config.kind;
        let endpoint = self.api.as_str().to_owned();
        let max_output = cloud::known_facts(&endpoint, &req.model).and_then(|f| f.max_output);
        let mut rounds = 0;
        // Fixes found for this request: kept for the model once a request with them is
        // accepted, not before.
        let mut pending: Vec<cloud::Fix> = Vec::new();
        loop {
            let mut fixes = cloud::remembered_fixes(&endpoint, &req.model);
            for fix in &pending {
                fixes.add(fix);
            }
            let mut body = req.to_body(true);
            if kind != ProviderKind::LlamaServer
                && let Value::Object(map) = &mut body
            {
                cloud::shape_chat_body(kind, map, &fixes, max_output);
            }
            match self
                .send_stream_request(self.chat_completions_url(), &body, &cancel, None)
                .await?
            {
                Sent::Accepted(response) => {
                    for fix in &pending {
                        cloud::remember_fix(&endpoint, &req.model, fix);
                    }
                    return self
                        .open_stream(response, cancel, WireDecoder::Chat(StreamDecoder::new()))
                        .await;
                }
                Sent::Refused {
                    status,
                    body: error_body,
                    reason,
                    retry_after,
                } => {
                    // A server other than llama-server that refused a parameter: once more
                    // without it, and never again for this model while the app runs.
                    if kind != ProviderKind::LlamaServer
                        && rounds < MAX_FIX_ROUNDS
                        && let Value::Object(sent) = &body
                    {
                        rounds += 1;
                        let mut changed = false;
                        for fix in cloud::fixes_for_error(status, &error_body, sent) {
                            if fixes.add(&fix) {
                                pending.push(fix);
                                changed = true;
                            }
                        }
                        if changed {
                            continue;
                        }
                    }
                    let err = error_from_body(Some(status), &error_body, &reason);
                    return Err(with_wait(err, retry_after));
                }
            }
        }
    }

    /// Sends a streaming request and reads the status. Requests to a cloud company are tried
    /// again, up to twice, on a rate limit with `retry-after`, on overload (529), on server
    /// errors and when the connection fails; other servers get one try, as before.
    ///
    /// # Errors
    /// [`ProviderError::Network`] when sending fails, [`ProviderError::Cancelled`].
    pub(crate) async fn send_stream_request(
        &self,
        url: Url,
        body: &Value,
        cancel: &CancellationToken,
        beta: Option<&str>,
    ) -> Result<Sent, ProviderError> {
        let retries = cloud::is_cloud_company(self.config.kind);
        let limit = self.config.request_timeout;
        let mut attempt = 0;
        loop {
            let mut request = self
                .authorize(self.http.post(url.clone()))
                .header(reqwest::header::ACCEPT, "text/event-stream");
            // [0.0.7 P5] An API feature in beta that this request uses (`anthropic-beta`).
            if let Some(beta) = beta {
                request = request.header("anthropic-beta", beta);
            }
            let request = request.json(body);
            let send = within(limit, "the server sent no response", async {
                request.send().await.map_err(network_error)
            });
            let sent = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                sent = send => sent,
            };
            let response = match sent {
                Ok(response) => response,
                // A connection that failed (not a server that went silent) is tried again.
                Err(ProviderError::Network(detail))
                    if retries
                        && attempt < 2
                        && !detail.contains("timed out")
                        && !detail.contains(" within ") =>
                {
                    wait(Duration::from_secs(1u64 << attempt), cancel).await?;
                    attempt += 1;
                    continue;
                }
                Err(err) => return Err(err),
            };
            let status = response.status();
            if status.is_success() {
                return Ok(Sent::Accepted(response));
            }
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(cloud::parse_retry_after);
            let reason = status.canonical_reason().unwrap_or("").to_owned();
            let read = within(
                limit,
                "the server did not finish its error response",
                read_capped(response, MAX_ERROR_BODY, true),
            );
            let error_body = tokio::select! {
                biased;
                () = cancel.cancelled() => return Err(ProviderError::Cancelled),
                body = read => body?,
            };
            if retries
                && let Some(delay) = cloud::retry_delay(status.as_u16(), retry_after, attempt)
            {
                wait(delay, cancel).await?;
                attempt += 1;
                continue;
            }
            return Ok(Sent::Refused {
                status: status.as_u16(),
                body: error_body,
                reason,
                retry_after,
            });
        }
    }

    /// Turns an accepted response into a [`ChatStream`] read with `decoder`; a whole JSON
    /// body instead of an event stream is accepted too.
    pub(crate) async fn open_stream(
        &self,
        response: reqwest::Response,
        cancel: CancellationToken,
        decoder: WireDecoder,
    ) -> Result<ChatStream, ProviderError> {
        let limit = self.config.request_timeout;
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
            let events = decoder.whole(&body)?;
            // Same state machine as a live stream, already finished, so cancelling still
            // yields `Cancelled` instead of the queued events.
            let state = StreamState {
                body: None,
                decoder,
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
            decoder,
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
        Ok(self
            .list_model_entries()
            .await?
            .into_iter()
            .map(|model| ModelInfo {
                id: model.id,
                context_length: model.context_length,
            })
            .collect())
    }

    /// The model list with what it says about each model (0.0.7): display names from
    /// Anthropic, OpenRouter and abliteration.ai; Anthropic's list from its own
    /// `GET /v1/models`; OpenAI's without the models that cannot chat (embedding, speech,
    /// image, moderation and the like). What a cloud company's list says about a model's
    /// output limit is kept for this run, so requests stay within it.
    ///
    /// # Errors
    /// Any [`ProviderError`].
    pub async fn list_model_entries(&self) -> Result<Vec<ListedModel>, ProviderError> {
        let kind = self.config.kind;
        if kind == ProviderKind::Anthropic {
            return crate::anthropic::list_models(self).await;
        }
        let value = self.get_json(join(&self.api, "models")).await?;
        let entries = value
            .get("data")
            .and_then(Value::as_array)
            .or_else(|| value.get("models").and_then(Value::as_array))
            .ok_or_else(|| {
                ProviderError::Decode("the model list has no 'data' array".to_owned())
            })?;
        let endpoint = self.api.as_str().to_owned();
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let id = ["id", "name", "model"]
                    .iter()
                    .find_map(|key| entry.get(*key).and_then(Value::as_str))
                    .filter(|id| !id.is_empty())?;
                if kind == ProviderKind::OpenAi && !cloud::openai_can_chat(id) {
                    return None;
                }
                // A model that cannot write text (an image or video generator).
                let writes_text = |modalities: Option<&Value>| {
                    modalities
                        .and_then(Value::as_array)
                        .is_none_or(|list| list.iter().any(|m| m.as_str() == Some("text")))
                };
                if matches!(kind, ProviderKind::OpenRouter | ProviderKind::Abliteration)
                    && !(writes_text(entry.pointer("/architecture/output_modalities"))
                        && writes_text(entry.get("output_modalities")))
                {
                    return None;
                }
                let context_length = self.listed_context_length(entry);
                let display_name =
                    matches!(kind, ProviderKind::OpenRouter | ProviderKind::Abliteration)
                        .then(|| {
                            ["display_name", "name"]
                                .iter()
                                .find_map(|key| entry.get(*key).and_then(Value::as_str))
                                .map(str::trim)
                                .filter(|name| !name.is_empty() && *name != id)
                                .map(str::to_owned)
                        })
                        .flatten();
                if cloud::is_cloud_company(kind) {
                    let max_output = ["/top_provider/max_completion_tokens", "/max_output_length"]
                        .iter()
                        .find_map(|path| entry.pointer(path).and_then(as_u32))
                        .filter(|n| *n > 0);
                    cloud::remember_facts(
                        &endpoint,
                        id,
                        cloud::ModelFacts {
                            context: context_length,
                            max_output,
                            thinking: None,
                        },
                    );
                }
                Some(ListedModel {
                    id: id.to_owned(),
                    display_name,
                    context_length,
                })
            })
            .collect())
    }

    fn listed_context_length(&self, entry: &Value) -> Option<u32> {
        match self.config.kind {
            ProviderKind::LlamaServer => entry.pointer("/meta/n_ctx").and_then(as_u32),
            ProviderKind::Vllm => entry.get("max_model_len").and_then(as_u32),
            ProviderKind::Ollama | ProviderKind::LmStudio => None,
            ProviderKind::Generic | ProviderKind::OpenAi => {
                ["context_length", "context_window", "max_model_len"]
                    .iter()
                    .find_map(|key| entry.get(*key).and_then(as_u32))
            }
            // OpenRouter and abliteration.ai report `context_length` for every model.
            ProviderKind::OpenRouter | ProviderKind::Abliteration => entry
                .get("context_length")
                .or_else(|| entry.pointer("/top_provider/context_length"))
                .and_then(as_u32)
                .filter(|n| *n > 0),
            ProviderKind::Anthropic => entry
                .get("max_input_tokens")
                .and_then(as_u32)
                .filter(|n| *n > 0),
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
    /// - vLLM, generic, OpenAI, OpenRouter and abliteration.ai: the model list;
    /// - Anthropic: `max_input_tokens` from its model list (0.0.7).
    ///
    /// `Ok(None)` means the server does not say; the app then uses the user's setting
    /// (default 4,096), or for a cloud company the size it documents
    /// ([`crate::cloud::default_context`]).
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
            ProviderKind::Anthropic => crate::anthropic::context_length(self, model).await,
            ProviderKind::Vllm
            | ProviderKind::Generic
            | ProviderKind::OpenAi
            | ProviderKind::OpenRouter
            | ProviderKind::Abliteration => match self.list_models().await {
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

    /// llama-server's `GET /props` (chat template, special tokens, capabilities) as JSON.
    /// `Ok(None)` for every other kind of server.
    ///
    /// # Errors
    /// Any [`ProviderError`].
    pub async fn props(&self) -> Result<Option<Value>, ProviderError> {
        if self.config.kind != ProviderKind::LlamaServer {
            return Ok(None);
        }
        self.get_json(join(&self.root, "props")).await.map(Some)
    }

    /// Tokenizes `content` with llama-server's `POST /tokenize`, with or without special-token
    /// parsing, and returns each token with its text piece. `Ok(None)` for every other kind of
    /// server.
    ///
    /// # Errors
    /// Any [`ProviderError`]; [`ProviderError::Decode`] when the reply has no token list.
    pub async fn tokenize(
        &self,
        content: &str,
        parse_special: bool,
    ) -> Result<Option<Vec<Token>>, ProviderError> {
        if self.config.kind != ProviderKind::LlamaServer {
            return Ok(None);
        }
        let reply = self
            .post_json(
                join(&self.root, "tokenize"),
                &json!({
                    "content": content,
                    "add_special": false,
                    "parse_special": parse_special,
                    "with_pieces": true,
                }),
            )
            .await?;
        let tokens = reply
            .get("tokens")
            .and_then(Value::as_array)
            .ok_or_else(|| ProviderError::Decode("tokenize returned no tokens".to_owned()))?;
        Ok(Some(
            tokens
                .iter()
                .filter_map(|token| match token {
                    // `with_pieces`: `{"id": 1, "piece": "text"}`; a piece that is not valid
                    // UTF-8 comes as a byte array.
                    Value::Object(map) => Some(Token {
                        id: map.get("id").and_then(Value::as_i64)?,
                        piece: map.get("piece").and_then(Value::as_str).map(str::to_owned),
                    }),
                    // An older build without `with_pieces`: ids only.
                    Value::Number(id) => Some(Token {
                        id: id.as_i64()?,
                        piece: None,
                    }),
                    _ => None,
                })
                .collect(),
        ))
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
        let key = self.config.api_key.as_deref().filter(|key| !key.is_empty());
        if self.config.kind == ProviderKind::Anthropic {
            // Claude's API: the key in `x-api-key` (marked sensitive, so it never appears in
            // a debug print) and the API version on every request.
            let request = request.header("anthropic-version", crate::anthropic::API_VERSION);
            return match key.map(|key| reqwest::header::HeaderValue::from_str(key.trim())) {
                Some(Ok(mut value)) => {
                    value.set_sensitive(true);
                    request.header("x-api-key", value)
                }
                // A key with characters a header cannot carry is not sent at all; the server
                // then says the key is missing.
                Some(Err(_)) | None => request,
            };
        }
        match key {
            Some(key) => request.bearer_auth(key),
            None => request,
        }
    }

    pub(crate) async fn get_json(&self, url: Url) -> Result<Value, ProviderError> {
        self.send_json(self.http.get(url)).await
    }

    async fn post_json(&self, url: Url, body: &Value) -> Result<Value, ProviderError> {
        self.send_json(self.http.post(url).json(body)).await
    }

    async fn send_json(&self, request: reqwest::RequestBuilder) -> Result<Value, ProviderError> {
        self.send_json_unscrubbed(request)
            .await
            .map_err(|err| cloud::scrub_key(err, self.config.api_key.as_deref()))
    }

    async fn send_json_unscrubbed(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<Value, ProviderError> {
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
    // [0.0.7 P5] The same parse as the key check and the "Cloud" mark.
    let mut url = cloud::endpoint_url(raw)?;
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
    } else if segments.len() >= 2
        && segments.last().is_some_and(|last| last == "messages")
        && is_version_segment(&segments[segments.len() - 2])
    {
        // A pasted Messages API address (`https://api.anthropic.com/v1/messages`, 0.0.7).
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
pub(crate) fn join(base: &Url, path: &str) -> Url {
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

/// Waits `delay`, unless the request is cancelled first.
async fn wait(delay: Duration, cancel: &CancellationToken) -> Result<(), ProviderError> {
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ProviderError::Cancelled),
        () = tokio::time::sleep(delay) => Ok(()),
    }
}

/// Adds how long the server asked to wait to a rate-limit or overload error that was not
/// retried because the wait is long, so the message says when to try again (0.0.7). Other
/// errors are returned unchanged.
pub(crate) fn with_wait(err: ProviderError, retry_after: Option<Duration>) -> ProviderError {
    match (err, retry_after) {
        (
            ProviderError::Http {
                status: status @ (429 | 503 | 529),
                message,
            },
            Some(after),
        ) if after > Duration::from_secs(30) => {
            let secs = after.as_secs();
            let when = if secs >= 120 {
                format!("about {} minutes", secs.div_ceil(60))
            } else {
                format!("{secs} seconds")
            };
            ProviderError::Http {
                status,
                message: format!("{message} (it asks to try again in {when})"),
            }
        }
        (err, _) => err,
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
    decoder: WireDecoder,
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
