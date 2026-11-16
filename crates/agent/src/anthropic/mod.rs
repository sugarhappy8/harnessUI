//! Claude's own format: Anthropic's Messages API (0.0.7 P5, ARCHITECTURE.md 7.5).
//!
//! The turn runner builds the same [`ChatRequest`] for every server and reads the same
//! [`crate::stream::StreamEvent`]s back; for an endpoint of kind [`ProviderKind::Anthropic`]
//! the client translates the request ([`request::messages_body`]) and the event stream
//! ([`stream::MessagesDecoder`]):
//!
//! - **Request:** the system message becomes the `system` field; messages become content
//!   blocks (text, base64 images, `tool_use`, `tool_result` with `is_error`); tools carry
//!   `input_schema`; `tool_choice` is `auto`, `none` or `any` with
//!   `disable_parallel_tool_use`; `max_tokens` is always sent, at most what the model
//!   accepts. Sampling values are never sent (API version 2026-10-21 removed them). A
//!   picture Claude would refuse becomes a note: over 10 MB or 8,000 pixels a side; in a
//!   request with more than 20 pictures, over 3,000 pixels; beyond the most pictures one
//!   request may hold (100, or 600 for a model with a window over 200,000 tokens), the
//!   oldest.
//! - **Thinking:** with the chat's thinking on, the model's own form (`adaptive`, or
//!   `enabled` with a budget on models before 4.6), with `display: "summarized"` so the
//!   Thinking area shows it; off, `disabled` or `between_tools` where the model accepts one,
//!   else (Claude Opus 5.5, Fable, Mythos: thinking cannot stop) `output_config.effort: low`
//!   with room in `max_tokens` for a short thought. Which forms a model accepts comes from
//!   Anthropic's model list (`capabilities.thinking.types`), else from its name ([`guess`]).
//! - **Thinking blocks are sent back unchanged**, as the documentation requires within a
//!   tool-use loop and recommends across turns (models that keep earlier thinking read it,
//!   and an unchanged history keeps the prompt cache). Each conversation (a chat, named by
//!   [`crate::ClientConfig::conversation`]) has its own [`ThinkingReplay`] while the app
//!   runs; no other chat ever sees it, and the chat used least recently is forgotten whole.
//!   A response that thought is kept under everything its request sent before it and what
//!   it showed ([`ReplayKey`]), so a later request of the chat replays it block for block
//!   exactly where it was written, after the same history; an edited history replays
//!   nothing after the edit (its blocks would not be valid there). If the API still refuses
//!   a block as "bound to a different conversation", the request goes again once with
//!   `block_binding.prefix_mismatch_behavior: drop_block` (beta
//!   `thinking-binding-controls-2026-08-01`, models with preserved thinking: the documented
//!   recovery, not sent otherwise), else without thinking blocks, and the chat's kept blocks
//!   are forgotten.
//! - **Prompt caching:** a `cache_control` breakpoint on the system prompt (which caches the
//!   tools with it) with the one-hour lifetime, so a chat that waits for an approval or a
//!   reply keeps it, and one with the five-minute lifetime on the newest message, so each
//!   step of a long chat reads what the step before wrote.
//! - **Model list** from `GET /v1/models` with display names, context size
//!   (`max_input_tokens`) and output limit (`max_tokens`).

mod request;
mod stream;
#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::client::{ChatStream, ListedModel, OpenAiClient, ProviderKind, Sent, WireDecoder};
use crate::cloud::{self, ModelFacts, ThinkingModes};
use crate::error::{ProviderError, as_u32, error_from_body, error_from_value};
use crate::types::ChatRequest;

pub(crate) use stream::{MessagesDecoder, events_from_message};

/// The API version the app speaks (the `anthropic-version` header). The newest: it removed
/// `temperature`, `top_p` and `top_k`, which the app never sends to Claude.
pub(crate) const API_VERSION: &str = "2026-10-21";

/// Responses kept for one conversation: a safety limit far beyond a long chat's replies.
/// Past it the conversation's kept responses start over: each one is found by everything
/// before it, the thinking of the replies before it included, so once the oldest went none
/// of the others would be found again anyway.
const MAX_REPLAY_ENTRIES: usize = 4_096;
/// Conversations (chats with one endpoint and model) whose responses are kept; the one used
/// least recently is forgotten whole.
const MAX_REPLAY_STORES: usize = 32;
/// How long a model lookup that failed is not tried again.
const FAILED_LOOKUP_PAUSE: Duration = Duration::from_secs(120);
/// The beta that lets a request say what to do with a thinking block whose earlier history
/// changed (Anthropic's preserved thinking documentation).
pub(crate) const BINDING_BETA: &str = "thinking-binding-controls-2026-08-01";
/// Fixes tried for one request (a parameter left out, thinking blocks dropped).
const MAX_FIX_ROUNDS: usize = 3;
/// How long the first request to a model waits for its entry in the model list.
const FACTS_LOOKUP: Duration = Duration::from_secs(10);
/// Pages of the model list read (1,000 models each).
const MAX_MODEL_PAGES: usize = 5;

/// What finds a kept response in a later request: a hash of everything the request that
/// produced it sent before it (the system prompt, the tools and every earlier block, without
/// cache markers), and a hash of what the reply showed (its text and its tool calls). The
/// same reply text at another place, in this chat or another one, has another key, and so
/// does a reply whose earlier history changed: such a block would not be valid there anyway.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ReplayKey {
    /// Everything before the reply ([`request::PrefixHasher`]).
    pub prefix: u64,
    /// What the reply showed ([`reply_hash`]).
    pub reply: u64,
}

/// The hash of what a reply showed: its whole text, then each tool call's id, name and
/// arguments, in order.
pub(crate) fn reply_hash<'a>(
    text: &str,
    calls: impl IntoIterator<Item = (&'a str, &'a str, &'a Value)>,
) -> u64 {
    let mut hasher = DefaultHasher::new();
    text.hash(&mut hasher);
    for (id, name, input) in calls {
        id.hash(&mut hasher);
        name.hash(&mut hasher);
        input.to_string().hash(&mut hasher);
    }
    hasher.finish()
}

/// The content blocks of one conversation's responses that thought, kept so its later
/// requests can send them back exactly as received.
#[derive(Debug, Default)]
pub(crate) struct ThinkingReplay {
    entries: HashMap<ReplayKey, Vec<Value>>,
}

impl ThinkingReplay {
    /// Keeps the blocks of a response; past [`MAX_REPLAY_ENTRIES`], after forgetting all the
    /// others.
    pub(crate) fn store(&mut self, key: ReplayKey, blocks: Vec<Value>) {
        if self.entries.len() >= MAX_REPLAY_ENTRIES && !self.entries.contains_key(&key) {
            self.clear();
        }
        self.entries.insert(key, blocks);
    }

    /// The blocks of the response with this key.
    pub(crate) fn find(&self, key: &ReplayKey) -> Option<&Vec<Value>> {
        self.entries.get(key)
    }

    /// How many responses are kept.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Forgets everything (after the API refused a replayed block of this conversation).
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
    }
}

/// The kept responses of each conversation, with when each was last used.
#[derive(Default)]
struct Conversations {
    stores: HashMap<String, (Arc<Mutex<ThinkingReplay>>, u64)>,
    clock: u64,
}

static REPLAYS: LazyLock<Mutex<Conversations>> = LazyLock::new(Default::default);

/// The kept responses of one conversation (a chat: `conversation`) with `model` at
/// `endpoint` (its API URL), for this run of the app, so a chat's next turn finds the
/// thinking of the turns before it and no other chat ever does. When more conversations are
/// kept than [`MAX_REPLAY_STORES`], the one used least recently is forgotten whole, never
/// part of a conversation that is still going.
pub(crate) fn replay_for(
    endpoint: &str,
    model: &str,
    conversation: &str,
) -> Arc<Mutex<ThinkingReplay>> {
    let Ok(mut kept) = REPLAYS.lock() else {
        return Arc::default();
    };
    kept.clock += 1;
    let now = kept.clock;
    let key = format!("{endpoint}\n{model}\n{conversation}");
    if !kept.stores.contains_key(&key)
        && kept.stores.len() >= MAX_REPLAY_STORES
        && let Some(idle) = kept
            .stores
            .iter()
            .min_by_key(|(_, (_, used))| *used)
            .map(|(key, _)| key.clone())
    {
        kept.stores.remove(&idle);
    }
    let entry = kept.stores.entry(key).or_default();
    entry.1 = now;
    entry.0.clone()
}

/// Forgets what `conversation` kept at `endpoint`, with every model: a conversation that
/// does not go on (a helper that has answered).
pub(crate) fn forget_conversation(endpoint: &str, conversation: &str) {
    let Ok(mut kept) = REPLAYS.lock() else {
        return;
    };
    let (start, end) = (format!("{endpoint}\n"), format!("\n{conversation}"));
    kept.stores.retain(|key, _| {
        key.strip_prefix(&start)
            .and_then(|rest| rest.strip_suffix(&end))
            .is_none_or(|model| model.contains('\n'))
    });
}

/// A Claude model's family, from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Line {
    Opus,
    Sonnet,
    Haiku,
    /// Claude Fable and Claude Mythos: thinking is always on.
    Frontier,
}

/// What a Claude model's name says about it, for a model the model list did not describe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Guess {
    /// The thinking settings it accepts.
    pub thinking: ThinkingModes,
    /// The largest output limit it accepts.
    pub max_output: u32,
    /// It accepts `tool_choice: any` (Claude Opus 5.5, Claude Sonnet 5.5 and Fable 5.1 or
    /// later refuse forced tool use).
    pub forced_tool_use: bool,
    /// Its thinking blocks are bound to the conversation before them ("preserved thinking":
    /// Fable and Mythos 5.1 and later, Opus, Sonnet and Haiku 5.5 and later), so the request
    /// can ask for `drop_block`.
    pub preserved_thinking: bool,
}

/// The family and version in a model id: `claude-opus-4-5-20251101` → (Opus, 4, 5),
/// `claude-3-7-sonnet-latest` → (Sonnet, 3, 7), `claude-sonnet-5` → (Sonnet, 5, 0).
fn parse_version(model: &str) -> Option<(Line, u32, u32)> {
    let model = model.to_ascii_lowercase();
    let start = model.find("claude-")?;
    let parts: Vec<&str> = model[start + "claude-".len()..].split('-').collect();
    let line = |word: &str| match word {
        "opus" => Some(Line::Opus),
        "sonnet" => Some(Line::Sonnet),
        "haiku" => Some(Line::Haiku),
        "fable" | "mythos" => Some(Line::Frontier),
        _ => None,
    };
    // A number of one or two digits; dates (20250514) are not versions.
    let number = |word: Option<&&str>| {
        word.and_then(|w| w.parse::<u32>().ok())
            .filter(|n| *n < 100)
    };
    if let Some(found) = parts.first().and_then(|w| line(w)) {
        let major = number(parts.get(1)).unwrap_or(0);
        let minor = number(parts.get(2)).unwrap_or(0);
        return Some((found, major, minor));
    }
    // Older names: claude-3-5-sonnet, claude-3-opus.
    let major = number(parts.first())?;
    let (minor, at) = match number(parts.get(1)) {
        Some(minor) => (minor, 2),
        None => (0, 1),
    };
    Some((line(parts.get(at)?)?, major, minor))
}

/// What the model's name says about thinking and its output limit (Anthropic's thinking and
/// models documentation): Fable and Mythos always think; 4.6 and later think adaptively,
/// and Claude Opus 5.5 cannot stop thinking while Claude Sonnet 5.5 stops with
/// `between_tools`; 3.7 to 4.5 take a thinking budget; older models do not think.
pub(crate) fn guess(model: &str) -> Guess {
    let modern = Guess {
        thinking: ThinkingModes {
            adaptive: true,
            enabled: false,
            disabled: true,
            between_tools: false,
        },
        max_output: 32_000,
        forced_tool_use: true,
        preserved_thinking: false,
    };
    let Some((line, major, minor)) = parse_version(model) else {
        return modern;
    };
    let version = (major, minor);
    if line == Line::Frontier {
        let always_on = ThinkingModes {
            adaptive: true,
            ..ThinkingModes::default()
        };
        return Guess {
            thinking: always_on,
            max_output: 128_000,
            forced_tool_use: version < (5, 1),
            preserved_thinking: version >= (5, 1),
        };
    }
    if version >= (4, 6) {
        let newest = version >= (5, 5) && matches!(line, Line::Opus | Line::Sonnet);
        return Guess {
            thinking: ThinkingModes {
                adaptive: true,
                enabled: version == (4, 6),
                disabled: !newest,
                between_tools: newest && line == Line::Sonnet,
            },
            max_output: 128_000,
            forced_tool_use: !newest,
            preserved_thinking: version >= (5, 5),
        };
    }
    let budget = ThinkingModes {
        adaptive: false,
        enabled: true,
        disabled: true,
        between_tools: false,
    };
    if version >= (3, 7) {
        let max_output = if line == Line::Opus && version < (4, 5) {
            32_000
        } else {
            64_000
        };
        return Guess {
            thinking: budget,
            max_output,
            forced_tool_use: true,
            preserved_thinking: false,
        };
    }
    Guess {
        thinking: ThinkingModes {
            disabled: true,
            ..ThinkingModes::default()
        },
        max_output: if version >= (3, 5) { 8_192 } else { 4_096 },
        forced_tool_use: true,
        preserved_thinking: false,
    }
}

/// The context the app plans with for a Claude model on "Automatic" (at most what the model
/// takes): 200,000 tokens, and 100,000 for Claude Haiku 5.5 and later Haiku models, which
/// cost five times as much per token above 100,000 (Anthropic's model overview).
pub(crate) fn default_context(model: &str) -> u32 {
    match parse_version(model) {
        Some((Line::Haiku, major, minor)) if (major, minor) >= (5, 5) => 100_000,
        _ => 200_000,
    }
}

/// [0.0.7 P5] Whether the chat's Thinking switch can stop the thinking of Claude `model` at
/// the endpoint with this address: the model accepts `disabled` or `between_tools`
/// (Anthropic's model list, when the app has read it this run, else the model's name). When
/// not, the switch's "off" asks for less thinking (`effort: low`).
pub fn thinking_can_stop(base_url: &str, model: &str) -> bool {
    let known = crate::client::api_url_of(base_url)
        .and_then(|endpoint| cloud::known_facts(&endpoint, model))
        .and_then(|facts| facts.thinking);
    let modes = known.unwrap_or_else(|| guess(model).thinking);
    modes.disabled || modes.between_tools || !modes.adaptive
}

/// A model entry of `GET /v1/models` (or `GET /v1/models/{id}`).
fn parse_model(entry: &Value) -> Option<(ListedModel, ModelFacts)> {
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())?
        .to_owned();
    let display_name = entry
        .get("display_name")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned);
    // 0 stands for "not known".
    let count = |key: &str| entry.get(key).and_then(as_u32).filter(|n| *n > 0);
    let context = count("max_input_tokens");
    let thinking = entry.pointer("/capabilities/thinking").map(|thinking| {
        let on = thinking
            .get("supported")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let accepts = |kind: &str| {
            thinking
                .pointer(&format!("/types/{kind}/supported"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        };
        ThinkingModes {
            adaptive: on && accepts("adaptive"),
            enabled: on && accepts("enabled"),
            disabled: accepts("disabled"),
            between_tools: accepts("between_tools"),
        }
    });
    Some((
        ListedModel {
            id,
            display_name,
            context_length: context,
        },
        ModelFacts {
            context,
            max_output: count("max_tokens"),
            thinking,
        },
    ))
}

static FAILED_LOOKUPS: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Default::default);

/// What the model list says about `model`: from this run's memory, else from
/// `GET /v1/models/{id}`. A model the list does not describe gets nothing, and is not asked
/// for again this run; the request then goes by its name ([`guess`]). A lookup that failed
/// (no answer, a server error) is tried again after two minutes.
async fn facts(client: &OpenAiClient, model: &str) -> ModelFacts {
    let endpoint = client.api_url().as_str().to_owned();
    if let Some(known) = cloud::known_facts(&endpoint, model) {
        return known;
    }
    let lookup_key = format!("{endpoint}\n{model}");
    let failed_lately = FAILED_LOOKUPS.lock().ok().is_some_and(|failed| {
        failed
            .get(&lookup_key)
            .is_some_and(|at| at.elapsed() < FAILED_LOOKUP_PAUSE)
    });
    if failed_lately {
        return ModelFacts::default();
    }
    let mut url = crate::client::join(client.api_url(), "models");
    if let Ok(mut segments) = url.path_segments_mut() {
        segments.push(model);
    }
    let found = match tokio::time::timeout(FACTS_LOOKUP, client.get_json(url)).await {
        Ok(Ok(value)) => Some(
            parse_model(&value)
                .map(|(_, facts)| facts)
                .unwrap_or_default(),
        ),
        Ok(Err(err)) if crate::client::is_not_found(&err) => Some(ModelFacts::default()),
        _ => None,
    };
    match found {
        Some(facts) => {
            cloud::remember_facts(&endpoint, model, facts.clone());
            facts
        }
        None => {
            if let Ok(mut failed) = FAILED_LOOKUPS.lock() {
                if failed.len() >= MAX_REPLAY_STORES {
                    failed.clear();
                }
                failed.insert(lookup_key, Instant::now());
            }
            ModelFacts::default()
        }
    }
}

/// The context size of `model` from Anthropic's model list (`max_input_tokens`); `Ok(None)`
/// when the list does not say.
pub(crate) async fn context_length(
    client: &OpenAiClient,
    model: &str,
) -> Result<Option<u32>, ProviderError> {
    Ok(facts(client, model).await.context)
}

/// Every model the key may use, newest first, with display names, from `GET /v1/models`.
pub(crate) async fn list_models(client: &OpenAiClient) -> Result<Vec<ListedModel>, ProviderError> {
    let endpoint = client.api_url().as_str().to_owned();
    let mut models = Vec::new();
    let mut after: Option<String> = None;
    for _ in 0..MAX_MODEL_PAGES {
        let mut url = crate::client::join(client.api_url(), "models");
        url.query_pairs_mut().append_pair("limit", "1000");
        if let Some(after) = &after {
            url.query_pairs_mut().append_pair("after_id", after);
        }
        let page = client.get_json(url).await?;
        let entries = page.get("data").and_then(Value::as_array).ok_or_else(|| {
            ProviderError::Decode("the model list has no 'data' array".to_owned())
        })?;
        for entry in entries {
            if let Some((listed, facts)) = parse_model(entry) {
                cloud::remember_facts(&endpoint, &listed.id, facts);
                models.push(listed);
            }
        }
        match page.get("last_id").and_then(Value::as_str) {
            Some(last) if page.get("has_more").and_then(Value::as_bool) == Some(true) => {
                after = Some(last.to_owned());
            }
            _ => break,
        }
    }
    Ok(models)
}

/// The HTTP status an Anthropic error type stands for (its errors documentation), for an
/// error that arrives inside a stream without one.
fn status_of(error_type: &str) -> Option<u16> {
    Some(match error_type {
        "invalid_request_error" => 400,
        "authentication_error" => 401,
        "billing_error" => 402,
        "permission_error" => 403,
        "not_found_error" => 404,
        "request_too_large" => 413,
        "rate_limit_error" => 429,
        "api_error" => 500,
        "timeout_error" => 504,
        "overloaded_error" => 529,
        _ => return None,
    })
}

/// An Anthropic error object (`{"type": "error", "error": {"type", "message"}}`).
pub(crate) fn error_from(value: &Value, status: Option<u16>) -> ProviderError {
    let status = status.or_else(|| {
        value
            .pointer("/error/type")
            .and_then(Value::as_str)
            .and_then(status_of)
    });
    error_from_value(value, status)
}

/// The API refused a thinking block that was sent back: it was changed, or what came before
/// it changed (the history-editing check).
fn thinking_block_refused(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_lowercase();
    (text.contains("thinking") || text.contains("signature"))
        && (text.contains("cannot be modified")
            || text.contains("bound to a different conversation")
            || text.contains("invalid `signature`")
            || text.contains("invalid signature"))
}

/// The API refused a replayed thinking block because what came before it changed ("bound to
/// a different conversation"): the documented recovery is one request with `drop_block`.
fn bound_to_another_conversation(body: &[u8]) -> bool {
    String::from_utf8_lossy(body)
        .to_lowercase()
        .contains("bound to a different conversation")
}

/// The API refused the `block_binding` setting or its beta (an account or platform without
/// the beta: "block_binding: Extra inputs are not permitted", or the header's value). Not the
/// refusal of a thinking block, which names the setting as a way out.
fn binding_refused(body: &[u8]) -> bool {
    let text = String::from_utf8_lossy(body).to_lowercase();
    text.contains("anthropic-beta")
        || text.contains(BINDING_BETA)
        || (text.contains("block_binding")
            && [
                "extra inputs",
                "not permitted",
                "unknown",
                "unexpected",
                "not supported",
            ]
            .iter()
            .any(|words| text.contains(words)))
}

/// Streams one reply in Claude's own format. The request is translated, sent with this
/// endpoint's key in `x-api-key`, and its event stream read into the same events as a chat
/// completion. A request refused for a parameter is sent again without it (and once that is
/// accepted, the parameter is left out for this model until the app quits). One refused
/// because a thinking block sent back is "bound to a different conversation" is sent again
/// once with `drop_block` (models with preserved thinking), else without thinking blocks;
/// either way the conversation's kept blocks are forgotten, so its later requests need
/// neither.
///
/// # Errors
/// Any [`ProviderError`]; [`ProviderError::Refused`] inside the stream when the company's
/// safety filter stops the reply.
pub(crate) async fn stream(
    client: &OpenAiClient,
    req: &ChatRequest,
    cancel: CancellationToken,
) -> Result<ChatStream, ProviderError> {
    debug_assert_eq!(client.config().kind, ProviderKind::Anthropic);
    req.validate_media()
        .map_err(ProviderError::InvalidRequest)?;
    let facts = tokio::select! {
        biased;
        () = cancel.cancelled() => return Err(ProviderError::Cancelled),
        facts = facts(client, &req.model) => facts,
    };
    let endpoint = client.api_url().as_str().to_owned();
    let url = crate::client::join(client.api_url(), "messages");
    // This conversation's kept responses: a chat's own, or this client's for a request that
    // belongs to no chat.
    let replay = match &client.config().conversation {
        Some(conversation) => replay_for(&endpoint, &req.model, conversation),
        None => client.own_replay().clone(),
    };
    let forget = || {
        if let Ok(mut kept) = replay.lock() {
            kept.clear();
        }
    };
    let mut replay_thinking = true;
    let mut recovering = false;
    let mut round = 0;
    // Fixes found for this request: kept for the model once a request with them is accepted.
    let mut pending: Vec<cloud::Fix> = Vec::new();
    loop {
        let mut fixes = cloud::remembered_fixes(&endpoint, &req.model);
        for fix in &pending {
            fixes.add(fix);
        }
        let body = {
            let kept = replay.lock().ok();
            let kept = kept.as_deref().filter(|_| replay_thinking);
            request::messages_body(req, &facts, kept, &fixes, recovering)
        };
        if recovering {
            // The recovery request carries the blocks; later requests of the conversation
            // carry none of them (the API needs `drop_block` no more).
            forget();
        }
        let beta = body
            .pointer("/thinking/block_binding")
            .is_some()
            .then_some(BINDING_BETA);
        // The reply is kept under the history as the conversation's next request sends it:
        // after a recovery, without the forgotten blocks.
        let prefix = if recovering {
            request::reply_prefix(&request::messages_body(req, &facts, None, &fixes, false))
        } else {
            request::reply_prefix(&body)
        };
        match client
            .send_stream_request(url.clone(), &body, &cancel, beta)
            .await?
        {
            Sent::Accepted(response) => {
                for fix in &pending {
                    cloud::remember_fix(&endpoint, &req.model, fix);
                }
                let decoder = MessagesDecoder::new(Some((replay.clone(), prefix)));
                return client
                    .open_stream(response, cancel, WireDecoder::Messages(decoder))
                    .await;
            }
            Sent::Refused {
                status,
                body: error_body,
                reason,
                retry_after,
            } => {
                if round < MAX_FIX_ROUNDS {
                    round += 1;
                    if status == 400 && replay_thinking && thinking_block_refused(&error_body) {
                        let can_drop = !recovering
                            && bound_to_another_conversation(&error_body)
                            && replay.lock().ok().is_some_and(|kept| {
                                request::messages_body(req, &facts, Some(&*kept), &fixes, true)
                                    .pointer("/thinking/block_binding")
                                    .is_some()
                            });
                        if can_drop {
                            recovering = true;
                        } else {
                            forget();
                            replay_thinking = false;
                        }
                        continue;
                    }
                    if status == 400 && beta.is_some() && binding_refused(&error_body) {
                        let binding = cloud::Fix::Drop("block_binding".to_owned());
                        if fixes.add(&binding) {
                            pending.push(binding);
                        }
                        forget();
                        replay_thinking = false;
                        continue;
                    }
                    if let Value::Object(sent) = &body {
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
                }
                let err = match serde_json::from_slice::<Value>(&error_body) {
                    Ok(value) if value.get("error").is_some() => error_from(&value, Some(status)),
                    _ => error_from_body(Some(status), &error_body, &reason),
                };
                return Err(crate::client::with_wait(err, retry_after));
            }
        }
    }
}
