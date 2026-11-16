//! Cloud endpoints (0.0.7 P5, ARCHITECTURE.md 7.5): where an endpoint's server is, when a
//! key may be sent to it, the request rules of OpenAI-compatible cloud servers, and what the
//! app learns about an endpoint's models while it runs.
//!
//! - **Where a server is** ([`endpoint_place`]) comes from its address only: this computer,
//!   the local network (private and link-local addresses, `.local`-style names, one-word
//!   host names), or the internet. The chat header marks chats that go to the internet.
//! - **A key never travels in clear text over the internet** ([`check_key_transport`]): an
//!   endpoint on the internet with a key must use `https://`.
//! - **Requests a server accepts** ([`shape_chat_body`]): llama-server's own fields are never
//!   sent to another server; OpenAI and abliteration.ai get only fields of the OpenAI API
//!   (both reject unknown ones), OpenAI gets `max_completion_tokens` instead of the
//!   deprecated `max_tokens` and no sampling values on its reasoning models.
//! - **What a server refused once is not sent again** ([`fixes_for_error`]): an HTTP 400 or
//!   422 that names an unsupported or unknown parameter, or an output limit that is too
//!   large, is retried once without it (or within the limit), and the fix is kept for that
//!   endpoint and model until the app quits.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde_json::{Map, Value};
use url::Url;

use crate::client::ProviderKind;
use crate::error::ProviderError;

/// Where an endpoint's server is, as far as its address says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EndpointPlace {
    /// This computer: `localhost`, 127.0.0.0/8, `::1`.
    ThisComputer,
    /// The local network: private, shared (100.64.0.0/10) and link-local addresses, IPv6
    /// unique-local addresses, names ending in `.local`, `.lan`, `.home.arpa`, `.internal`
    /// or `.localdomain`, and one-word host names.
    LocalNetwork,
    /// Anywhere else. What is sent there leaves this computer and the local network.
    Internet,
}

/// Names that only resolve inside a local network.
const LOCAL_SUFFIXES: &[&str] = &[".local", ".lan", ".home.arpa", ".internal", ".localdomain"];

/// The error when a key would be sent over plain `http://` to the internet.
pub const KEY_OVER_HTTP: &str = "An API key is sent to a server on the internet only over \
     https://, so that no one on the way can read it. Change the address to start with \
     https://.";

fn parse(raw: &str) -> Option<Url> {
    let trimmed = raw.trim();
    let url = if trimmed.contains("://") {
        Url::parse(trimmed).ok()?
    } else {
        Url::parse(&format!("http://{trimmed}")).ok()?
    };
    url.host()?;
    Some(url)
}

/// Where the server at `base_url` is. An address that cannot be read counts as the internet,
/// so it is never treated as safer than it is.
pub fn endpoint_place(base_url: &str) -> EndpointPlace {
    let Some(url) = parse(base_url) else {
        return EndpointPlace::Internet;
    };
    match url.host() {
        Some(url::Host::Domain(domain)) => domain_place(domain),
        Some(url::Host::Ipv4(ip)) => ipv4_place(ip),
        Some(url::Host::Ipv6(ip)) => ipv6_place(ip),
        None => EndpointPlace::Internet,
    }
}

fn domain_place(domain: &str) -> EndpointPlace {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain == "localhost" || domain.ends_with(".localhost") {
        EndpointPlace::ThisComputer
    } else if !domain.contains('.') || LOCAL_SUFFIXES.iter().any(|s| domain.ends_with(s)) {
        EndpointPlace::LocalNetwork
    } else {
        EndpointPlace::Internet
    }
}

fn ipv4_place(ip: Ipv4Addr) -> EndpointPlace {
    let [a, b, ..] = ip.octets();
    if ip.is_loopback() || ip.is_unspecified() {
        EndpointPlace::ThisComputer
    } else if ip.is_private() || ip.is_link_local() || (a == 100 && (b & 0xc0) == 64) {
        EndpointPlace::LocalNetwork
    } else {
        EndpointPlace::Internet
    }
}

fn ipv6_place(ip: Ipv6Addr) -> EndpointPlace {
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_place(v4);
    }
    let first = ip.segments()[0];
    if ip.is_loopback() || ip.is_unspecified() {
        EndpointPlace::ThisComputer
    } else if (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 {
        EndpointPlace::LocalNetwork
    } else {
        EndpointPlace::Internet
    }
}

/// The host of `base_url` as the user wrote it (`api.anthropic.com`), for "Messages … go to
/// <host>".
pub fn endpoint_host(base_url: &str) -> Option<String> {
    parse(base_url).and_then(|url| url.host_str().map(str::to_owned))
}

/// Whether an API key may be sent to `base_url`: not over plain `http://` to the internet.
/// This computer and the local network are allowed over `http://` (Ollama, LM Studio or a
/// vLLM on a home server rarely have https), and so is every `https://` address.
///
/// # Errors
/// [`KEY_OVER_HTTP`] for an `http://` address on the internet.
pub fn check_key_transport(base_url: &str) -> Result<(), String> {
    match parse(base_url) {
        Some(url)
            if url.scheme() == "http" && endpoint_place(base_url) == EndpointPlace::Internet =>
        {
            Err(KEY_OVER_HTTP.to_owned())
        }
        _ => Ok(()),
    }
}

/// The context size the app plans with for a model of a cloud company when its server does
/// not say (ARCHITECTURE.md 8.4): Claude models have at least 200,000 tokens (the current
/// ones 1,000,000, which Anthropic's model list reports); OpenAI's chat models at least
/// 128,000 (OpenAI's model list reports none); OpenRouter and abliteration.ai report each
/// model's size, so their default is only for a model listed without one. `None` for
/// servers that are not a cloud company's: the user's setting applies.
pub fn default_context(kind: ProviderKind) -> Option<u32> {
    match kind {
        ProviderKind::Anthropic => Some(200_000),
        ProviderKind::OpenAi => Some(128_000),
        ProviderKind::OpenRouter | ProviderKind::Abliteration => Some(32_768),
        ProviderKind::LlamaServer
        | ProviderKind::Ollama
        | ProviderKind::LmStudio
        | ProviderKind::Vllm
        | ProviderKind::Generic => None,
    }
}

/// Whether pictures a tool returns may go inside the tool's result message on a remote
/// endpoint of this kind. OpenAI's tool messages take text only, so there (and at OpenRouter
/// and abliteration.ai, which pass messages on to such models) the pictures go in a user
/// message right after the results; Claude's `tool_result` blocks take pictures.
pub fn images_in_tool_results(kind: ProviderKind) -> bool {
    !matches!(
        kind,
        ProviderKind::OpenAi | ProviderKind::OpenRouter | ProviderKind::Abliteration
    )
}

/// The output limit sent to an OpenAI-compatible cloud server when its model list did not
/// say: enough for long answers and for reasoning models' thinking, and below what most
/// models accept. A server that answers "at most N" lowers it for that model
/// ([`fixes_for_error`]).
pub(crate) const CLOUD_OUTPUT_LIMIT: u32 = 32_768;

/// Whether the kind is one of the ready-made cloud companies.
pub(crate) fn is_cloud_company(kind: ProviderKind) -> bool {
    matches!(
        kind,
        ProviderKind::Anthropic
            | ProviderKind::OpenAi
            | ProviderKind::OpenRouter
            | ProviderKind::Abliteration
    )
}

// ---- request shaping (OpenAI-compatible servers) -------------------------------------------

/// llama-server's own request fields (its README, "POST /completion" and the chat extras).
/// No other server is sent them.
const LLAMA_ONLY: &[&str] = &[
    "cache_prompt",
    "reasoning_format",
    "samplers",
    "n_probs",
    "reasoning_budget_tokens",
    "thinking_budget_tokens",
    "id_slot",
    "slot_id",
    "n_predict",
    "n_keep",
    "n_indent",
    "n_cmpl",
    "typical_p",
    "min_keep",
    "dynatemp_range",
    "dynatemp_exponent",
    "xtc_probability",
    "xtc_threshold",
    "repeat_last_n",
    "penalty_last_n",
    "dry_multiplier",
    "dry_base",
    "dry_allowed_length",
    "dry_penalty_last_n",
    "dry_sequence_breakers",
    "mirostat",
    "mirostat_tau",
    "mirostat_eta",
    "grammar",
    "json_schema",
    "ignore_eos",
    "t_max_predict_ms",
    "return_tokens",
    "post_sampling_probs",
    "timings_per_token",
    "return_progress",
    "lora",
    "image_data",
    "parse_tool_calls",
    "thinking_forced_open",
];

/// The top-level fields of OpenAI's Chat Completions request (its OpenAPI description,
/// `CreateChatCompletionRequest`). OpenAI rejects any other field.
const OPENAI_FIELDS: &[&str] = &[
    "model",
    "messages",
    "stream",
    "stream_options",
    "max_completion_tokens",
    "max_tokens",
    "temperature",
    "top_p",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "reasoning_effort",
    "response_format",
    "stop",
    "seed",
    "n",
    "logprobs",
    "top_logprobs",
    "logit_bias",
    "frequency_penalty",
    "presence_penalty",
    "user",
    "metadata",
    "store",
    "service_tier",
    "modalities",
    "audio",
    "prediction",
    "verbosity",
    "web_search_options",
    "safety_identifier",
    "prompt_cache_key",
    "prompt_cache_retention",
    "prompt_cache_options",
    "moderation",
    "function_call",
    "functions",
];

/// The top-level fields of abliteration.ai's chat request (its OpenAPI description,
/// `ChatCompletionRequest`, which has `additionalProperties: false`).
const ABLITERATION_FIELDS: &[&str] = &[
    "model",
    "messages",
    "policy_ids",
    "stream",
    "max_tokens",
    "max_completion_tokens",
    "temperature",
    "top_p",
    "frequency_penalty",
    "presence_penalty",
    "logit_bias",
    "n",
    "seed",
    "stop",
    "stream_options",
    "thinking",
    "reasoning",
    "reasoning_effort",
    "include_reasoning",
    "safety_identifier",
    "user",
    "tools",
    "tool_choice",
    "parallel_tool_calls",
    "functions",
    "function_call",
    "response_format",
    "modalities",
    "verbosity",
    "audio",
    "prediction",
    "metadata",
    "store",
    "service_tier",
    "prompt_cache_key",
    "prompt_cache_retention",
    "logprobs",
    "top_logprobs",
    "web_search_options",
];

/// The fields that hold the output limit.
const OUTPUT_LIMIT_KEYS: &[&str] = &["max_tokens", "max_completion_tokens"];

/// Fields a retry never leaves out: without them the request means something else.
const ESSENTIAL: &[&str] = &[
    "model",
    "messages",
    "stream",
    "tools",
    "system",
    "max_tokens",
    "max_completion_tokens",
];

/// OpenAI's reasoning models (o-series, GPT-5 and later) accept only the default sampling
/// values.
fn openai_reasoning_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    let model = model.rsplit('/').next().unwrap_or(&model);
    if let Some(rest) = model.strip_prefix('o') {
        return rest.starts_with(|c: char| c.is_ascii_digit());
    }
    model
        .strip_prefix("gpt-")
        .and_then(|rest| rest.chars().next())
        .and_then(|c| c.to_digit(10))
        .is_some_and(|major| major >= 5)
        && !model.contains("-chat")
}

/// Makes a Chat Completions body (from [`crate::types::ChatRequest::to_body`]) one that
/// `kind`'s server accepts, then applies what the server refused earlier in this run
/// (`fixes`) and the model's output limit (`max_output`). llama-server's requests are left
/// exactly as they are.
pub(crate) fn shape_chat_body(
    kind: ProviderKind,
    body: &mut Map<String, Value>,
    fixes: &ParamFixes,
    max_output: Option<u32>,
) {
    match kind {
        ProviderKind::LlamaServer | ProviderKind::Anthropic => return,
        ProviderKind::OpenAi => body.retain(|key, _| OPENAI_FIELDS.contains(&key.as_str())),
        ProviderKind::Abliteration => {
            body.retain(|key, _| ABLITERATION_FIELDS.contains(&key.as_str()));
        }
        // vLLM reads `chat_template_kwargs` too; an unknown server may be one of them.
        ProviderKind::Vllm | ProviderKind::Generic => {
            for key in LLAMA_ONLY {
                body.remove(*key);
            }
        }
        ProviderKind::Ollama | ProviderKind::LmStudio | ProviderKind::OpenRouter => {
            for key in LLAMA_ONLY {
                body.remove(*key);
            }
            body.remove("chat_template_kwargs");
        }
    }
    if kind == ProviderKind::OpenAi {
        if let Some(limit) = body.remove("max_tokens") {
            body.entry("max_completion_tokens").or_insert(limit);
        }
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if openai_reasoning_model(&model) {
            body.remove("temperature");
            body.remove("top_p");
        }
        // A tool message is `role`, `content` and `tool_call_id` in OpenAI's API.
        if let Some(Value::Array(messages)) = body.get_mut("messages") {
            for message in messages {
                if message.get("role").and_then(Value::as_str) == Some("tool")
                    && let Value::Object(map) = message
                {
                    map.remove("name");
                }
            }
        }
    }
    if is_cloud_company(kind) {
        cap_output(body, max_output.unwrap_or(CLOUD_OUTPUT_LIMIT));
    }
    fixes.apply(body);
}

fn cap_output(body: &mut Map<String, Value>, cap: u32) {
    for key in OUTPUT_LIMIT_KEYS {
        if body
            .get(*key)
            .and_then(Value::as_u64)
            .is_some_and(|n| n > u64::from(cap))
        {
            body.insert((*key).to_owned(), Value::from(cap.max(1)));
        }
    }
}

// ---- what a server refused ------------------------------------------------------------------

/// What to change in a request after the server refused it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fix {
    /// Leave this top-level field out.
    Drop(String),
    /// Send this field's value under another name (`max_tokens` → `max_completion_tokens`).
    Rename {
        /// The name the server refused.
        from: String,
        /// The name it wants.
        to: String,
    },
    /// Ask for at most this many output tokens.
    Cap(u32),
}

/// What an endpoint refused for a model, kept until the app quits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ParamFixes {
    dropped: BTreeSet<String>,
    renamed: BTreeMap<String, String>,
    output_cap: Option<u32>,
}

impl ParamFixes {
    /// Applies the fixes to a request body.
    pub(crate) fn apply(&self, body: &mut Map<String, Value>) {
        for (from, to) in &self.renamed {
            if let Some(value) = body.remove(from) {
                body.entry(to.clone()).or_insert(value);
            }
        }
        for key in &self.dropped {
            body.remove(key);
        }
        if let Some(cap) = self.output_cap {
            cap_output(body, cap);
        }
    }

    /// Whether `key` is left out.
    pub(crate) fn drops(&self, key: &str) -> bool {
        self.dropped.contains(key)
    }

    /// The output limit the server set, if it set one.
    pub(crate) fn output_cap(&self) -> Option<u32> {
        self.output_cap
    }

    /// Adds a fix; false when it changes nothing (the server refused the same thing again).
    fn add(&mut self, fix: &Fix) -> bool {
        match fix {
            Fix::Drop(key) => self.dropped.insert(key.clone()),
            Fix::Rename { from, to } => {
                if self.renamed.get(from) == Some(to) {
                    return false;
                }
                // `a → b` replaces an earlier `b → a`: the server's latest word wins.
                self.renamed.remove(to);
                self.renamed.insert(from.clone(), to.clone());
                true
            }
            Fix::Cap(cap) => {
                if self.output_cap.is_some_and(|old| old <= *cap) {
                    return false;
                }
                self.output_cap = Some(*cap);
                true
            }
        }
    }
}

/// Endpoints and models seen in one run of the app; a handful at most.
const MAX_REMEMBERED: usize = 256;

static FIXES: LazyLock<Mutex<HashMap<String, ParamFixes>>> = LazyLock::new(Default::default);

fn memory_key(endpoint: &str, model: &str) -> String {
    format!("{endpoint}\n{model}")
}

/// What `endpoint` refused for `model` earlier in this run.
pub(crate) fn remembered_fixes(endpoint: &str, model: &str) -> ParamFixes {
    FIXES
        .lock()
        .ok()
        .and_then(|map| map.get(&memory_key(endpoint, model)).cloned())
        .unwrap_or_default()
}

/// Keeps a fix for `endpoint` and `model` until the app quits. False when it was already
/// kept, so a server that refuses the fixed request again is not asked a third time.
pub(crate) fn remember_fix(endpoint: &str, model: &str, fix: &Fix) -> bool {
    let Ok(mut map) = FIXES.lock() else {
        return false;
    };
    if map.len() >= MAX_REMEMBERED {
        map.clear();
    }
    map.entry(memory_key(endpoint, model)).or_default().add(fix)
}

/// Words with which servers say a field is not accepted.
const REFUSED_WORDS: &[&str] = &[
    "not supported",
    "unsupported",
    "unknown parameter",
    "unknown field",
    "unrecognized",
    "unrecognised",
    "not permitted",
    "not allowed",
    "extra inputs",
    "extra fields",
    "does not support",
    "cannot both be specified",
    "only one of",
];

/// What to change after a server refused a request with HTTP 400 or 422, read from its error
/// body: OpenAI's `error.param` and `error.code` (`unsupported_parameter`,
/// `unsupported_value`, `unknown_parameter`), "Unrecognized request argument supplied: x",
/// FastAPI's `detail[].loc` with "Extra inputs are not permitted", Anthropic's
/// "x: Extra inputs are not permitted" and "x is not supported", and "at most N" for an
/// output limit. Only fields that were `sent` are named, and never one the request needs.
/// Empty when the error is about something else.
pub(crate) fn fixes_for_error(status: u16, body: &[u8], sent: &Map<String, Value>) -> Vec<Fix> {
    if status != 400 && status != 422 {
        return Vec::new();
    }
    let value: Value = serde_json::from_slice(body).unwrap_or_else(|_| Value::String(lossy(body)));
    let error = value
        .get("error")
        .filter(|error| error.is_object())
        .unwrap_or(&value);
    let message = match error.get("message").or_else(|| value.get("message")) {
        Some(Value::String(text)) => text.clone(),
        _ => match &value {
            Value::String(text) => text.clone(),
            other => other.to_string(),
        },
    };
    let lower = message.to_lowercase();
    let param = error
        .get("param")
        .and_then(Value::as_str)
        .map(top_level_field);
    let code = error
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or_default();

    // An output limit above what the model allows.
    let names_limit = OUTPUT_LIMIT_KEYS
        .iter()
        .any(|key| lower.contains(key) || param.as_deref() == Some(*key))
        || lower.contains("output tokens");
    if names_limit && let Some(cap) = output_cap_in(&lower) {
        let sent_limit = OUTPUT_LIMIT_KEYS
            .iter()
            .filter_map(|key| sent.get(*key).and_then(Value::as_u64))
            .max();
        if sent_limit.is_some_and(|sent| sent > u64::from(cap)) {
            return vec![Fix::Cap(cap)];
        }
    }
    // `max_tokens` against `max_completion_tokens`, in either direction.
    for (from, to) in [
        ("max_tokens", "max_completion_tokens"),
        ("max_completion_tokens", "max_tokens"),
    ] {
        let named = param.as_deref() == Some(from) || mentions(&lower, from);
        let refused = REFUSED_WORDS.iter().any(|w| lower.contains(w))
            || code.starts_with("unsupported")
            || code == "unknown_parameter";
        if sent.contains_key(from)
            && named
            && refused
            && (mentions(&lower, to) || from != "max_tokens")
        {
            return vec![Fix::Rename {
                from: from.to_owned(),
                to: to.to_owned(),
            }];
        }
    }
    let droppable = |key: &str| sent.contains_key(key) && !ESSENTIAL.contains(&key);
    let mut fixes = Vec::new();
    let push_drop = |key: String, fixes: &mut Vec<Fix>| {
        if droppable(&key) && !fixes.contains(&Fix::Drop(key.clone())) {
            fixes.push(Fix::Drop(key));
        }
    };
    // FastAPI / Pydantic: `{"detail": [{"loc": ["body", "top_k"], "type": "extra_forbidden"}]}`.
    if let Some(details) = value.get("detail").and_then(Value::as_array) {
        for detail in details {
            let extra = detail.get("type").and_then(Value::as_str) == Some("extra_forbidden")
                || detail
                    .get("msg")
                    .and_then(Value::as_str)
                    .is_some_and(|msg| msg.to_lowercase().contains("extra inputs"));
            let loc: Vec<&str> = detail
                .get("loc")
                .and_then(Value::as_array)
                .map(|loc| loc.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if extra && loc.len() == 2 && loc[0] == "body" {
                push_drop(loc[1].to_owned(), &mut fixes);
            }
        }
    }
    // "Unrecognized request argument(s) supplied: top_k, min_p".
    if let Some(at) = lower.find("unrecognized request argument") {
        let rest = &message[at..];
        if let Some(colon) = rest.find(':') {
            for name in rest[colon + 1..].split(',') {
                let name = name
                    .trim()
                    .trim_matches(|c: char| c == '\'' || c == '"' || c == '`' || c == '.');
                push_drop(name.to_owned(), &mut fixes);
            }
        }
    }
    // OpenAI names the field.
    if let Some(param) = param.clone() {
        let refused = code.starts_with("unsupported")
            || code == "unknown_parameter"
            || REFUSED_WORDS.iter().any(|w| lower.contains(w));
        if refused {
            push_drop(param, &mut fixes);
        }
    }
    // A message that names a field it does not accept.
    if fixes.is_empty() && REFUSED_WORDS.iter().any(|w| lower.contains(w)) {
        let mut named: Vec<(usize, &String)> = sent
            .keys()
            .filter(|key| droppable(key))
            .filter_map(|key| first_mention(&lower, key).map(|at| (at, key)))
            .collect();
        named.sort();
        // "`temperature` and `top_p` cannot both be specified": keep the first.
        let pick = if lower.contains("cannot both") || lower.contains("only one of") {
            named.last()
        } else {
            named.first()
        };
        if let Some((_, key)) = pick {
            push_drop((*key).clone(), &mut fixes);
        }
    }
    fixes
}

fn lossy(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

/// `messages[0].content` → `messages`; `thinking.display` → `thinking`.
fn top_level_field(param: &str) -> String {
    param
        .split(['.', '['])
        .next()
        .unwrap_or(param)
        .trim_matches(|c: char| c == '\'' || c == '"' || c == '`')
        .to_owned()
}

/// Where `word` first appears in `text` as a whole name (not inside a longer identifier).
fn first_mention(text: &str, word: &str) -> Option<usize> {
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(found) = text[from..].find(word) {
        let at = from + found;
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        if !before.is_some_and(ident) && !after.is_some_and(ident) {
            return Some(at);
        }
        from = at + word.len();
    }
    None
}

fn mentions(text: &str, word: &str) -> bool {
    first_mention(text, word).is_some()
}

/// The largest output a server says it allows: "at most 16384", "> 128000, which is the
/// maximum", "maximum of 8192", "less than or equal to 4096".
fn output_cap_in(lower: &str) -> Option<u32> {
    [
        "at most ",
        "maximum of ",
        "maximum is ",
        "less than or equal to ",
        "up to ",
    ]
    .iter()
    .find_map(|marker| number_after(lower, marker))
    .or_else(|| {
        let at = lower.find(", which is the maximum")?;
        let before = &lower[..at];
        let start = before.rfind("> ")? + 2;
        before[start..].trim().replace(',', "").parse().ok()
    })
    .filter(|cap| *cap > 0)
}

fn number_after(text: &str, marker: &str) -> Option<u32> {
    let start = text.find(marker)? + marker.len();
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',')
        .filter(char::is_ascii_digit)
        .collect();
    digits.parse().ok()
}

// ---- retrying ---------------------------------------------------------------------------------

/// Retries of one request to a cloud company before its error is shown.
const MAX_RETRIES: u32 = 2;
/// The longest wait a server may ask for before the app gives up and says when to try again.
const LONGEST_WAIT: Duration = Duration::from_secs(30);

/// How long to wait before trying a failed request to a cloud company again, or `None` to
/// give up: the companies' documentation says to retry rate limits (with the `retry-after`
/// header they send), overload (HTTP 529) and server errors with backoff. A rate limit
/// without `retry-after` (a spending limit, which keeps failing until access resumes) is not
/// retried, and neither is a wait longer than 30 seconds.
pub(crate) fn retry_delay(
    status: u16,
    retry_after: Option<Duration>,
    attempt: u32,
) -> Option<Duration> {
    if attempt >= MAX_RETRIES {
        return None;
    }
    match status {
        429 => retry_after.filter(|wait| *wait <= LONGEST_WAIT),
        408 | 500 | 502 | 503 | 504 | 529 => match retry_after {
            Some(wait) if wait <= LONGEST_WAIT => Some(wait),
            Some(_) => None,
            None => Some(Duration::from_secs(1u64 << attempt.min(4))),
        },
        _ => None,
    }
}

/// A `retry-after` header in seconds (the form the companies send).
pub(crate) fn parse_retry_after(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|secs| secs.is_finite() && *secs >= 0.0 && *secs < 86_400.0)
        .map(Duration::from_secs_f64)
}

/// Removes the API key from an error's text, in case a server or a proxy echoed it.
pub(crate) fn scrub_key(err: ProviderError, key: Option<&str>) -> ProviderError {
    let Some(key) = key.map(str::trim).filter(|key| key.len() >= 8) else {
        return err;
    };
    let clean = |text: String| text.replace(key, "[API key]");
    match err {
        ProviderError::Http { status, message } => ProviderError::Http {
            status,
            message: clean(message),
        },
        ProviderError::ServerParseError(text) => ProviderError::ServerParseError(clean(text)),
        ProviderError::Network(text) => ProviderError::Network(clean(text)),
        ProviderError::Decode(text) => ProviderError::Decode(clean(text)),
        ProviderError::InvalidRequest(text) => ProviderError::InvalidRequest(clean(text)),
        ProviderError::Refused { explanation } => ProviderError::Refused {
            explanation: explanation.map(clean),
        },
        other @ (ProviderError::ContextOverflow { .. }
        | ProviderError::Unauthorized
        | ProviderError::Cancelled) => other,
    }
}

// ---- models -----------------------------------------------------------------------------------

/// Which thinking settings a model accepts (Anthropic's model list,
/// `capabilities.thinking.types`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ThinkingModes {
    /// `{"type": "adaptive"}`.
    pub adaptive: bool,
    /// `{"type": "enabled", "budget_tokens": N}`.
    pub enabled: bool,
    /// `{"type": "disabled"}`.
    pub disabled: bool,
    /// `{"type": "between_tools"}` (thinking off, short notes between tool calls).
    pub between_tools: bool,
}

/// What a server's model list says about one model.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ModelFacts {
    /// The context size in tokens.
    pub context: Option<u32>,
    /// The largest output limit the model accepts.
    pub max_output: Option<u32>,
    /// The thinking settings it accepts (Anthropic).
    pub thinking: Option<ThinkingModes>,
}

static FACTS: LazyLock<Mutex<HashMap<String, ModelFacts>>> = LazyLock::new(Default::default);

/// Keeps what a model list said about `model` at `endpoint`.
pub(crate) fn remember_facts(endpoint: &str, model: &str, facts: ModelFacts) {
    if let Ok(mut map) = FACTS.lock() {
        if map.len() >= MAX_REMEMBERED * 4 {
            map.clear();
        }
        map.insert(memory_key(endpoint, model), facts);
    }
}

/// What a model list said about `model` at `endpoint` earlier in this run.
pub(crate) fn known_facts(endpoint: &str, model: &str) -> Option<ModelFacts> {
    FACTS
        .lock()
        .ok()
        .and_then(|map| map.get(&memory_key(endpoint, model)).cloned())
}

/// Whether an OpenAI model id is one that can chat: the model list also has embedding,
/// speech, transcription, image, video, moderation, completion-only and realtime models.
pub(crate) fn openai_can_chat(id: &str) -> bool {
    const NOT_CHAT: &[&str] = &[
        "embedding",
        "whisper",
        "tts",
        "transcribe",
        "dall-e",
        "gpt-image",
        "chatgpt-image",
        "moderation",
        "davinci",
        "babbage",
        "-instruct",
        "realtime",
        "sora",
        "computer-use",
    ];
    let id = id.to_ascii_lowercase();
    !NOT_CHAT.iter().any(|word| id.contains(word))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn place(url: &str) -> EndpointPlace {
        endpoint_place(url)
    }

    #[test]
    fn places_from_addresses() {
        use EndpointPlace::{Internet, LocalNetwork, ThisComputer};
        for (url, expected) in [
            ("http://127.0.0.1:11434", ThisComputer),
            ("http://localhost:1234/v1", ThisComputer),
            ("localhost:8080", ThisComputer),
            ("http://[::1]:8000", ThisComputer),
            ("http://[::ffff:127.0.0.1]:8000", ThisComputer),
            ("http://0.0.0.0:8000", ThisComputer),
            ("http://api.localhost", ThisComputer),
            ("http://192.168.1.5:11434", LocalNetwork),
            ("http://10.0.0.7:8000", LocalNetwork),
            ("http://172.16.4.2", LocalNetwork),
            ("http://169.254.10.1", LocalNetwork),
            ("http://100.101.102.103:11434", LocalNetwork),
            ("http://[fd12:3456::1]:8000", LocalNetwork),
            ("http://[fe80::1]:8000", LocalNetwork),
            ("http://[::ffff:10.0.0.1]:8000", LocalNetwork),
            ("http://nas:8000", LocalNetwork),
            ("http://studio.local:1234", LocalNetwork),
            ("http://gpu-box.lan", LocalNetwork),
            ("http://ollama.home.arpa", LocalNetwork),
            ("https://api.anthropic.com", Internet),
            ("https://api.openai.com/v1", Internet),
            ("https://openrouter.ai/api/v1", Internet),
            ("http://8.8.8.8:80", Internet),
            ("http://172.32.0.1", Internet),
            ("http://100.128.0.1", Internet),
            ("http://[2001:db8::1]", Internet),
            ("http://localhost.example.com", Internet),
            ("not a url at all", Internet),
            ("", Internet),
        ] {
            assert_eq!(place(url), expected, "{url}");
        }
    }

    #[test]
    fn hosts_for_the_cloud_mark() {
        assert_eq!(
            endpoint_host("https://api.anthropic.com").as_deref(),
            Some("api.anthropic.com")
        );
        assert_eq!(
            endpoint_host("openrouter.ai/api/v1").as_deref(),
            Some("openrouter.ai")
        );
        assert_eq!(endpoint_host("::"), None);
    }

    #[test]
    fn keys_never_go_over_plain_http_to_the_internet() {
        assert!(check_key_transport("https://api.openai.com/v1").is_ok());
        assert!(check_key_transport("http://127.0.0.1:11434").is_ok());
        assert!(check_key_transport("http://192.168.1.5:8000/v1").is_ok());
        assert!(check_key_transport("http://nas:8000").is_ok());
        assert_eq!(
            check_key_transport("http://api.example.com/v1"),
            Err(KEY_OVER_HTTP.to_owned())
        );
        assert!(check_key_transport("http://203.0.113.9:8000").is_err());
        // Without a scheme the client uses http://.
        assert!(check_key_transport("api.example.com/v1").is_err());
        assert!(!KEY_OVER_HTTP.contains('!'));
    }

    #[test]
    fn company_defaults() {
        assert_eq!(default_context(ProviderKind::Anthropic), Some(200_000));
        assert_eq!(default_context(ProviderKind::OpenAi), Some(128_000));
        assert_eq!(default_context(ProviderKind::OpenRouter), Some(32_768));
        assert_eq!(default_context(ProviderKind::Generic), None);
        assert_eq!(default_context(ProviderKind::Ollama), None);
        assert!(images_in_tool_results(ProviderKind::Anthropic));
        assert!(images_in_tool_results(ProviderKind::Generic));
        assert!(!images_in_tool_results(ProviderKind::OpenAi));
        assert!(!images_in_tool_results(ProviderKind::OpenRouter));
        assert!(!images_in_tool_results(ProviderKind::Abliteration));
    }

    fn body(value: Value) -> Map<String, Value> {
        match value {
            Value::Object(map) => map,
            _ => unreachable!(),
        }
    }

    fn shaped(kind: ProviderKind, value: Value) -> Value {
        let mut map = body(value);
        shape_chat_body(kind, &mut map, &ParamFixes::default(), None);
        Value::Object(map)
    }

    fn llama_style() -> Value {
        json!({
            "model": "gpt-4.1",
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                {"role": "tool", "content": "ok", "tool_call_id": "c1", "name": "f"}
            ],
            "max_tokens": 1000,
            "temperature": 0.5,
            "top_k": 20,
            "stream": true,
            "stream_options": {"include_usage": true},
            "cache_prompt": true,
            "chat_template_kwargs": {"enable_thinking": false},
            "reasoning_format": "deepseek",
            "samplers": ["top_k"],
            "n_probs": 2
        })
    }

    #[test]
    fn openai_gets_only_its_own_fields() {
        let body = shaped(ProviderKind::OpenAi, llama_style());
        assert_eq!(
            body,
            json!({
                "model": "gpt-4.1",
                "messages": [
                    {"role": "user", "content": "hi"},
                    {"role": "assistant", "tool_calls": [{"id": "c1", "type": "function", "function": {"name": "f", "arguments": "{}"}}]},
                    {"role": "tool", "content": "ok", "tool_call_id": "c1"}
                ],
                "max_completion_tokens": 1000,
                "temperature": 0.5,
                "stream": true,
                "stream_options": {"include_usage": true}
            })
        );
        // Reasoning models: no sampling values.
        let mut request = llama_style();
        request["model"] = json!("o4-mini");
        let body = shaped(ProviderKind::OpenAi, request);
        assert!(body.get("temperature").is_none());
        let mut request = llama_style();
        request["model"] = json!("gpt-5.5");
        request["top_p"] = json!(0.9);
        let body = shaped(ProviderKind::OpenAi, request);
        assert!(body.get("temperature").is_none() && body.get("top_p").is_none());
        assert!(openai_reasoning_model("o3"));
        assert!(openai_reasoning_model("gpt-6-astra"));
        assert!(!openai_reasoning_model("gpt-4o"));
        assert!(!openai_reasoning_model("gpt-5-chat-latest"));
        assert!(!openai_reasoning_model("omni-moderation-latest"));
    }

    #[test]
    fn llama_server_requests_are_untouched_and_others_lose_llama_fields() {
        assert_eq!(
            shaped(ProviderKind::LlamaServer, llama_style()),
            llama_style()
        );
        let body = shaped(ProviderKind::OpenRouter, llama_style());
        for gone in [
            "cache_prompt",
            "chat_template_kwargs",
            "reasoning_format",
            "samplers",
            "n_probs",
        ] {
            assert!(body.get(gone).is_none(), "{gone}");
        }
        assert_eq!(body["top_k"], json!(20), "OpenRouter takes top_k");
        assert_eq!(body["max_tokens"], json!(1000));
        let body = shaped(ProviderKind::Vllm, llama_style());
        assert_eq!(
            body["chat_template_kwargs"],
            json!({"enable_thinking": false})
        );
        assert!(body.get("cache_prompt").is_none());
        let body = shaped(ProviderKind::Abliteration, llama_style());
        assert!(body.get("top_k").is_none() && body.get("cache_prompt").is_none());
        assert_eq!(
            body["messages"][2]["name"],
            json!("f"),
            "abliteration.ai takes name"
        );
    }

    #[test]
    fn cloud_output_limits_are_capped() {
        let mut request = llama_style();
        request["max_tokens"] = json!(120_000);
        let body = shaped(ProviderKind::OpenAi, request.clone());
        assert_eq!(body["max_completion_tokens"], json!(CLOUD_OUTPUT_LIMIT));
        let mut map = self::body(request.clone());
        shape_chat_body(
            ProviderKind::OpenRouter,
            &mut map,
            &ParamFixes::default(),
            Some(65_536),
        );
        assert_eq!(map["max_tokens"], json!(65_536));
        // Servers that are not a cloud company's keep the app's limit.
        let body = shaped(ProviderKind::Generic, request);
        assert_eq!(body["max_tokens"], json!(120_000));
    }

    fn sent() -> Map<String, Value> {
        body(json!({
            "model": "m",
            "messages": [],
            "max_tokens": 50_000,
            "temperature": 0.7,
            "top_p": 0.9,
            "top_k": 20,
            "parallel_tool_calls": false,
            "thinking": {"type": "disabled"}
        }))
    }

    fn fixes(status: u16, body: Value) -> Vec<Fix> {
        fixes_for_error(status, body.to_string().as_bytes(), &sent())
    }

    #[test]
    fn openai_unsupported_parameter_errors() {
        // Recorded from api.openai.com (simonw/llm issue 724).
        assert_eq!(
            fixes(
                400,
                json!({"error": {
                    "message": "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.",
                    "type": "invalid_request_error", "param": "max_tokens", "code": "unsupported_parameter"
                }})
            ),
            vec![Fix::Rename {
                from: "max_tokens".into(),
                to: "max_completion_tokens".into()
            }]
        );
        assert_eq!(
            fixes(
                400,
                json!({"error": {
                    "message": "Unsupported value: 'temperature' does not support 0.7 with this model. Only the default (1) value is supported.",
                    "type": "invalid_request_error", "param": "temperature", "code": "unsupported_value"
                }})
            ),
            vec![Fix::Drop("temperature".into())]
        );
        assert_eq!(
            fixes(
                400,
                json!({"error": {
                    "message": "Unknown parameter: 'parallel_tool_calls'.",
                    "type": "invalid_request_error", "param": "parallel_tool_calls", "code": "unknown_parameter"
                }})
            ),
            vec![Fix::Drop("parallel_tool_calls".into())]
        );
        assert_eq!(
            fixes(
                400,
                json!({"error": {
                    "message": "Unrecognized request arguments supplied: top_k, parallel_tool_calls",
                    "type": "invalid_request_error", "param": null, "code": null
                }})
            ),
            vec![
                Fix::Drop("top_k".into()),
                Fix::Drop("parallel_tool_calls".into())
            ]
        );
        assert_eq!(
            fixes(
                400,
                json!({"error": {
                    "message": "max_tokens is too large: 50000. This model supports at most 16384 completion tokens, whereas you provided 50000.",
                    "type": "invalid_request_error", "param": "max_tokens", "code": "invalid_value"
                }})
            ),
            vec![Fix::Cap(16_384)]
        );
    }

    #[test]
    fn other_servers_unsupported_parameter_errors() {
        // FastAPI / Pydantic (abliteration.ai's server is FastAPI).
        assert_eq!(
            fixes(
                422,
                json!({"detail": [{"loc": ["body", "top_k"], "msg": "Extra inputs are not permitted", "type": "extra_forbidden"}]})
            ),
            vec![Fix::Drop("top_k".into())]
        );
        // Anthropic.
        assert_eq!(
            fixes(
                400,
                json!({"type": "error", "error": {"type": "invalid_request_error", "message": "top_k: Extra inputs are not permitted"}})
            ),
            vec![Fix::Drop("top_k".into())]
        );
        assert_eq!(
            fixes(
                400,
                json!({"type": "error", "error": {"type": "invalid_request_error",
                "message": "\"thinking.type.disabled\" is not supported for this model. Use \"thinking.type.adaptive\" and \"output_config.effort\" to control thinking behavior."}})
            ),
            vec![Fix::Drop("thinking".into())]
        );
        assert_eq!(
            fixes(
                400,
                json!({"type": "error", "error": {"type": "invalid_request_error",
                "message": "`temperature` and `top_p` cannot both be specified for this model. Please use only one."}})
            ),
            vec![Fix::Drop("top_p".into())]
        );
        assert_eq!(
            fixes(
                400,
                json!({"type": "error", "error": {"type": "invalid_request_error",
                "message": "max_tokens: 50000 > 32000, which is the maximum allowed number of output tokens for claude-opus-4-1"}})
            ),
            vec![Fix::Cap(32_000)]
        );
        // vLLM and plain text.
        assert_eq!(
            fixes(
                400,
                json!({"object": "error", "message": "top_k is not supported by this model", "code": 400})
            ),
            vec![Fix::Drop("top_k".into())]
        );
        assert_eq!(
            fixes_for_error(
                400,
                b"Unrecognized request argument supplied: top_k",
                &sent()
            ),
            vec![Fix::Drop("top_k".into())]
        );
    }

    #[test]
    fn errors_about_something_else_change_nothing() {
        for body in [
            json!({"error": {"message": "This model's maximum context length is 8192 tokens. However, you requested 9000 tokens.", "code": "context_length_exceeded"}}),
            json!({"type": "error", "error": {"type": "invalid_request_error", "message": "prompt is too long: 210000 tokens > 200000 maximum"}}),
            json!({"error": {"message": "messages: field required", "param": "messages", "code": "unsupported_value"}}),
            json!({"error": {"message": "Unsupported parameter: 'model'", "param": "model", "code": "unsupported_parameter"}}),
            json!({"error": {"message": "Invalid API key"}}),
        ] {
            assert_eq!(fixes(400, body.clone()), Vec::new(), "{body}");
        }
        // Only 400 and 422 are read.
        assert!(fixes(500, json!({"error": {"message": "top_k is not supported"}})).is_empty());
        // A field that was not sent is never named.
        let mut sent = sent();
        sent.remove("top_k");
        assert!(
            fixes_for_error(
                400,
                br#"{"error":{"message":"top_k is not supported"}}"#,
                &sent
            )
            .is_empty()
        );
    }

    #[test]
    fn fixes_are_kept_per_endpoint_and_model() {
        let endpoint = "https://fixes.example/v1";
        assert_eq!(remembered_fixes(endpoint, "a"), ParamFixes::default());
        assert!(remember_fix(endpoint, "a", &Fix::Drop("top_k".into())));
        assert!(
            !remember_fix(endpoint, "a", &Fix::Drop("top_k".into())),
            "kept once"
        );
        assert!(remember_fix(endpoint, "a", &Fix::Cap(16_384)));
        assert!(
            !remember_fix(endpoint, "a", &Fix::Cap(20_000)),
            "a higher cap changes nothing"
        );
        assert!(remember_fix(
            endpoint,
            "a",
            &Fix::Rename {
                from: "max_tokens".into(),
                to: "max_completion_tokens".into()
            }
        ));
        let fixes = remembered_fixes(endpoint, "a");
        assert!(fixes.drops("top_k"));
        assert_eq!(fixes.output_cap(), Some(16_384));
        assert_eq!(remembered_fixes(endpoint, "b"), ParamFixes::default());
        let mut map =
            body(json!({"model": "a", "max_tokens": 40_000, "top_k": 1, "temperature": 0.2}));
        fixes.apply(&mut map);
        assert_eq!(
            Value::Object(map),
            json!({"model": "a", "max_completion_tokens": 16_384, "temperature": 0.2})
        );
        // The reverse rename replaces the first.
        assert!(remember_fix(
            endpoint,
            "a",
            &Fix::Rename {
                from: "max_completion_tokens".into(),
                to: "max_tokens".into()
            }
        ));
        let mut map = body(json!({"max_completion_tokens": 10}));
        remembered_fixes(endpoint, "a").apply(&mut map);
        assert_eq!(Value::Object(map), json!({"max_tokens": 10}));
    }

    #[test]
    fn retry_rules() {
        let secs = Duration::from_secs;
        assert_eq!(retry_delay(529, None, 0), Some(secs(1)));
        assert_eq!(retry_delay(529, None, 1), Some(secs(2)));
        assert_eq!(retry_delay(529, None, 2), None);
        assert_eq!(retry_delay(500, Some(secs(3)), 0), Some(secs(3)));
        assert_eq!(retry_delay(503, Some(secs(300)), 0), None);
        assert_eq!(retry_delay(429, Some(secs(5)), 0), Some(secs(5)));
        assert_eq!(
            retry_delay(429, None, 0),
            None,
            "a spending limit is not retried"
        );
        assert_eq!(retry_delay(429, Some(secs(31)), 0), None);
        for status in [400, 401, 402, 403, 404, 413, 422] {
            assert_eq!(retry_delay(status, Some(secs(1)), 0), None, "{status}");
        }
        assert_eq!(parse_retry_after(" 12 "), Some(secs(12)));
        assert_eq!(parse_retry_after("0.5"), Some(Duration::from_millis(500)));
        assert_eq!(parse_retry_after("Wed, 21 Oct 2015 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("-1"), None);
    }

    #[test]
    fn keys_are_scrubbed_from_errors() {
        let key = "sk-ant-secret-0123456789";
        let err = scrub_key(
            ProviderError::Http {
                status: 400,
                message: format!("bad key {key} sent"),
            },
            Some(key),
        );
        assert_eq!(err.to_string(), "HTTP 400: bad key [API key] sent");
        let err = scrub_key(ProviderError::Network(format!("to {key}")), Some(key));
        assert!(!err.to_string().contains(key));
        // Short or missing keys change nothing.
        assert_eq!(
            scrub_key(ProviderError::Network("abc".into()), Some("abc")),
            ProviderError::Network("abc".into())
        );
    }

    #[test]
    fn openai_listing_keeps_chat_models_only() {
        for id in [
            "gpt-4o",
            "gpt-5.5",
            "o4-mini",
            "gpt-4o-search-preview",
            "gpt-6-astra",
        ] {
            assert!(openai_can_chat(id), "{id}");
        }
        for id in [
            "text-embedding-3-large",
            "whisper-1",
            "tts-1-hd",
            "gpt-4o-mini-tts",
            "gpt-4o-transcribe",
            "dall-e-3",
            "gpt-image-1",
            "omni-moderation-latest",
            "davinci-002",
            "babbage-002",
            "gpt-3.5-turbo-instruct",
            "gpt-realtime",
            "sora-2",
            "computer-use-preview",
        ] {
            assert!(!openai_can_chat(id), "{id}");
        }
    }
}
