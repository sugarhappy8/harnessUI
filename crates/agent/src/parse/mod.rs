//! The fallback tool-call parser and argument repair (ARCHITECTURE.md 8.3, steps 2 and 4).
//!
//! [`recover_tool_calls`] runs only when tools were offered and no native call arrived. It is
//! a pure function over the message text (never the reasoning) and tries, in order:
//!
//! 1. the known markers in [`MARKER_FORMATS`] (the list is data: a new model family is one
//!    more entry);
//! 2. a fenced `json` block;
//! 3. bare JSON, only when the message starts with `{` or `[`.
//!
//! Rules that keep it from running something the model merely described:
//!
//! - a candidate counts only when it is the last thing in the message, and marker text inside
//!   a code fence or inline code is ignored;
//! - inside the JSON, `name` + `arguments`, `name` + `parameters` and `function.name` +
//!   `function.arguments` are accepted; arguments are repaired ([`repair_arguments`]) before
//!   they are validated;
//! - a fenced or bare-JSON candidate counts only when it names an enabled tool and its
//!   arguments validate, otherwise it stays text;
//! - a marker candidate that names an enabled tool but fails validation is a failed call (not
//!   run, the error goes back to the model, a mistake); one naming an unknown tool gets
//!   "no such tool; available: …";
//! - a candidate whose name and arguments match call-shaped text in a tool result, attachment
//!   or user message still in context ([`RecoveryContext`]) is rejected and stays text.

mod repair;
mod scan;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub use repair::{MAX_DEPTH as REPAIR_MAX_DEPTH, repair_arguments};
use scan::{CodeMap, content_end, find_outside_code, floor_boundary, json_value_at, skip_ws};

/// The most bytes of JSON read for one candidate.
const MAX_CANDIDATE_BYTES: usize = 1024 * 1024;
/// How far before a tool-name occurrence in context text the echo check looks for the start
/// of a call shape.
const ECHO_LOOKBACK_BYTES: usize = 4096;
/// How many occurrences of a tool name per context text the echo check examines.
const ECHO_MAX_OCCURRENCES: usize = 64;
/// How many possible call starts before one occurrence the echo check tries.
const ECHO_MAX_STARTS: usize = 16;
/// The most bytes the echo check reads for one call shape in context text.
const ECHO_WINDOW_BYTES: usize = 64 * 1024;

/// A tool offered to the model, as the parser and the loop see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    /// The name the model calls.
    pub name: String,
    /// What the tool does, for the model.
    pub description: String,
    /// The full JSON Schema of the arguments (validated by the tool host).
    pub input_schema: Value,
    /// The server's read-only hint (never used to skip a prompt).
    #[serde(default)]
    pub read_only_hint: Option<bool>,
    /// The server's destructive hint.
    #[serde(default)]
    pub destructive_hint: Option<bool>,
}

/// Validates arguments for a tool by name: `Ok` or one message per problem.
pub type ArgsValidator<'a> = dyn Fn(&str, &Value) -> Result<(), Vec<String>> + Send + Sync + 'a;

/// What the parser needs besides the text: the validator (schemas are compiled once, in the
/// tool host) and the untrusted texts still in context, which a candidate must not echo.
pub struct RecoveryContext<'a> {
    texts: Vec<&'a str>,
    validator: &'a ArgsValidator<'a>,
}

impl<'a> RecoveryContext<'a> {
    /// A context with this validator and no texts.
    pub fn new(validator: &'a ArgsValidator<'a>) -> Self {
        Self {
            texts: Vec::new(),
            validator,
        }
    }

    /// Adds a text from a tool result, an attachment or a user message still in context.
    #[must_use]
    pub fn with_text(mut self, text: &'a str) -> Self {
        self.add_text(text);
        self
    }

    /// Adds a text from a tool result, an attachment or a user message still in context.
    pub fn add_text(&mut self, text: &'a str) {
        if !text.is_empty() {
            self.texts.push(text);
        }
    }

    fn validate(&self, tool: &str, args: &Value) -> Result<(), Vec<String>> {
        (self.validator)(tool, args)
    }
}

/// One call recovered from text.
#[derive(Debug, Clone, PartialEq)]
pub enum Recovered {
    /// Names an enabled tool and the (repaired) arguments validate: it goes through policy.
    Valid {
        /// The tool name.
        name: String,
        /// The repaired arguments.
        arguments: Map<String, Value>,
    },
    /// A marker call that cannot run: not run, `error` returned to the model, counted as a
    /// mistake.
    Failed {
        /// The tool name as written (may be empty or unknown).
        name: String,
        /// The arguments as far as they could be read (`{}` otherwise).
        arguments: Map<String, Value>,
        /// What is wrong, for the model.
        error: String,
    },
}

/// Why a candidate was found but not used (for logs and tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// Call-shaped text that is not the last thing in the message.
    NotLast,
    /// The candidate matches call-shaped text in a tool result, attachment or user message.
    Echoed,
    /// A fenced or bare-JSON candidate naming a tool that is not enabled.
    UnknownTool(String),
    /// A fenced or bare-JSON candidate whose arguments do not validate.
    InvalidArguments(Vec<String>),
}

/// The result of [`recover_tool_calls`].
#[derive(Debug, Clone, PartialEq)]
pub struct Recovery {
    /// The message text to show and store: the original text, minus the candidate when calls
    /// were recovered.
    pub text: String,
    /// The calls, in order. Empty when nothing was recovered.
    pub calls: Vec<Recovered>,
    /// The family of the format that matched, when calls were recovered.
    pub format: Option<&'static str>,
    /// Why a candidate was not used, when one was found.
    pub rejected: Option<Rejection>,
}

impl Recovery {
    fn nothing(text: &str, rejected: Option<Rejection>) -> Self {
        Self {
            text: text.to_owned(),
            calls: Vec::new(),
            format: None,
            rejected,
        }
    }
}

/// How the text inside a marker is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    /// A JSON call object, or an array of them.
    Json,
    /// `<function=name><parameter=key>value</parameter>…</function>`, one or more.
    XmlFunction,
    /// `name<arg_key>key</arg_key><arg_value>value</arg_value>…` (GLM).
    ArgKeyValue,
    /// `name[ARGS]{json}` (newer Mistral).
    NameArgs,
    /// `name.call(key="value", …)` or `name(key="value")` (Llama 3 built-in tools).
    PythonCall,
    /// After `<function=`: `name>{json}` or `name><parameter=…>…` (Functionary v3.1).
    FunctionTag,
}

/// One known tool-call marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MarkerFormat {
    /// The model families that write it.
    pub family: &'static str,
    /// The opening marker.
    pub open: &'static str,
    /// The closing marker; `None` when the call runs to the end of the message (or to the
    /// next opening marker).
    pub close: Option<&'static str>,
    /// How the body may be written, tried in order.
    pub bodies: &'static [BodyKind],
}

/// The marker formats, in the order they are tried. Earlier entries win when markers nest
/// (`<function=…>` inside `<tool_call>`).
pub const MARKER_FORMATS: &[MarkerFormat] = &[
    MarkerFormat {
        family: "Hermes / Qwen 2.5, Qwen 3, QwQ / Granite 4 / Qwen3-Coder / GLM",
        open: "<tool_call>",
        close: Some("</tool_call>"),
        bodies: &[BodyKind::Json, BodyKind::XmlFunction, BodyKind::ArgKeyValue],
    },
    MarkerFormat {
        family: "Mistral",
        open: "[TOOL_CALLS]",
        close: None,
        bodies: &[BodyKind::Json, BodyKind::NameArgs],
    },
    MarkerFormat {
        family: "Granite 3",
        open: "<|tool_call|>",
        close: None,
        bodies: &[BodyKind::Json],
    },
    MarkerFormat {
        family: "Llama 3.x",
        open: "<|python_tag|>",
        close: None,
        bodies: &[BodyKind::Json, BodyKind::PythonCall],
    },
    MarkerFormat {
        family: "LM Studio",
        open: "[TOOL_REQUEST]",
        close: Some("[END_TOOL_REQUEST]"),
        bodies: &[BodyKind::Json],
    },
    MarkerFormat {
        family: "Functionary v3.1",
        open: "<function=",
        close: Some("</function>"),
        bodies: &[BodyKind::FunctionTag],
    },
];

/// A call as written, before the tool lookup.
#[derive(Debug, Clone, PartialEq)]
struct RawCall {
    name: String,
    arguments: Value,
}

/// A marker block found in the text.
#[derive(Debug, Clone)]
struct Block {
    start: usize,
    end: usize,
    format: &'static MarkerFormat,
    calls: Result<Vec<RawCall>, String>,
    /// False when the format has a closing marker and it is missing.
    closed: bool,
}

/// Recovers tool calls written as text (module documentation). `tools` are the enabled
/// tools; `text` is the message content only, never the reasoning.
pub fn recover_tool_calls(text: &str, tools: &[ToolInfo], context: &RecoveryContext) -> Recovery {
    let code = CodeMap::new(text);
    let end = content_end(text);
    let blocks = find_blocks(text, &code, tools);
    let mut rejected = None;
    if let Some(group) = trailing_group(text, &blocks, end) {
        return use_marker_group(text, &group, tools, context);
    }
    if !blocks.is_empty() {
        rejected = Some(Rejection::NotLast);
    }

    // A fenced json block that ends the message.
    if let Some(fence) = code.fences().last()
        && fence.closed
        && fence.info.to_ascii_lowercase().starts_with("json")
        && content_end(&text[..fence.range.end]) == end
    {
        let body = &text[fence.body.clone()];
        if let Some(calls) = strict_json_calls(body) {
            return use_strict_candidate(text, fence.range.start, calls, tools, context);
        }
    }

    // Bare JSON: only when the message starts with `{` or `[` and the value is all of it.
    let first = skip_ws(text, 0);
    if text[first..].starts_with(['{', '['])
        && let Some((_, value_end)) = json_value_at(text, first, MAX_CANDIDATE_BYTES)
        && skip_ws(text, value_end) >= end
        && let Some(calls) = strict_json_calls(&text[first..value_end])
    {
        return use_strict_candidate(text, first, calls, tools, context);
    }
    Recovery::nothing(text, rejected)
}

/// Removes tool-call markup (every marker block outside code, wherever it is) and trailing
/// end-of-turn tokens. Used for the wrap-up reply, whose calls are never run
/// (ARCHITECTURE.md 8.2, "Step cap").
pub fn strip_tool_markup(text: &str) -> String {
    let code = CodeMap::new(text);
    let mut blocks = find_blocks(text, &code, &[]);
    blocks.sort_by_key(|block| block.start);
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for block in blocks {
        if block.start < cursor {
            continue;
        }
        out.push_str(&text[cursor..block.start]);
        cursor = block.end;
    }
    out.push_str(&text[cursor..]);
    let end = content_end(&out);
    out.truncate(end);
    out.trim().to_owned()
}

/// A tool call written as text that the output limit cut off: an opening marker (outside
/// code) at the end of the message whose closing marker is missing or whose call cannot be
/// read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutOffCall {
    /// The tool name, when it was written before the cut.
    pub name: Option<String>,
    /// The text before the marker.
    pub text_before: String,
}

/// Finds a text-form tool call cut off by the output limit (used when a reply ends with
/// `finish_reason: length`).
pub fn cut_off_call(text: &str) -> Option<CutOffCall> {
    let code = CodeMap::new(text);
    let end = content_end(text);
    let blocks = find_blocks(text, &code, &[]);
    let last = blocks.iter().max_by_key(|block| block.start)?;
    if last.end < end || (last.calls.is_ok() && last.closed) {
        return None;
    }
    Some(CutOffCall {
        name: guess_name(&text[last.start..last.end]),
        text_before: text[..last.start].trim_end().to_owned(),
    })
}

/// The tool name in a partial call: a JSON `"name": "…"`, `<function=…>`, or the text before
/// `[ARGS]`.
fn guess_name(partial: &str) -> Option<String> {
    let valid = |name: &str| {
        let name = name.trim();
        (!name.is_empty() && name.len() <= 128 && name.chars().all(is_name_char))
            .then(|| name.to_owned())
    };
    if let Some(pos) = partial.find("\"name\"") {
        let rest = partial[pos + 6..]
            .trim_start()
            .strip_prefix(':')?
            .trim_start();
        let rest = rest.strip_prefix('"')?;
        return valid(&rest[..rest.find('"')?]);
    }
    if let Some(pos) = partial.find("<function=") {
        let rest = &partial[pos + 10..];
        return valid(&rest[..rest.find('>').unwrap_or(rest.len())]);
    }
    if let Some(pos) = partial.find("[ARGS]") {
        let before = &partial[..pos];
        let name = before.rsplit(']').next().unwrap_or(before);
        return valid(name);
    }
    None
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')
}

/// Finds every marker block outside code, in [`MARKER_FORMATS`] order, skipping markers
/// inside a block already found.
fn find_blocks(text: &str, code: &CodeMap, tools: &[ToolInfo]) -> Vec<Block> {
    let end = content_end(text);
    let mut blocks: Vec<Block> = Vec::new();
    for format in MARKER_FORMATS {
        let starts: Vec<usize> = find_outside_code(text, format.open, code)
            .filter(|pos| *pos < end)
            .collect();
        for (i, &start) in starts.iter().enumerate() {
            if blocks
                .iter()
                .any(|block| (block.start..block.end).contains(&start))
            {
                continue;
            }
            let next_open = starts.get(i + 1).copied().unwrap_or(end).min(end);
            if let Some(block) = parse_block(text, start, end, next_open, format, tools) {
                blocks.push(block);
            }
        }
    }
    blocks
}

/// Reads one block opening at `start`. `end` is the content end; `next_open` the next opening
/// of the same marker (the end of an unterminated block).
fn parse_block(
    text: &str,
    start: usize,
    end: usize,
    next_open: usize,
    format: &'static MarkerFormat,
    tools: &[ToolInfo],
) -> Option<Block> {
    let body_start = start + format.open.len();
    if body_start > end {
        return None;
    }
    // A JSON body is read as JSON first, so a closing marker inside a string is not taken
    // for the end of the block.
    if format.bodies.contains(&BodyKind::Json) {
        let value_start = skip_ws(text, body_start);
        if text[value_start..].starts_with(['{', '['])
            && let Some((value, value_end)) = json_value_at(text, value_start, MAX_CANDIDATE_BYTES)
            && value_end <= end
        {
            let after = skip_ws(text, value_end);
            let block_end = match format.close {
                Some(close) if text[after..].starts_with(close) => {
                    Some((after + close.len(), true))
                }
                // A missing closing marker is accepted at the end of the message (the model
                // stopped right after the call).
                Some(_) if after >= end => Some((end, false)),
                Some(_) => None,
                // Without a closing marker the call ends with its JSON. What follows is the
                // next call, or other text that makes this block "not last"; it is never part
                // of the call. (Reading on to the next opening marker would also cut a call
                // whose string value contains that marker in two.)
                None => Some((if after >= end { end } else { value_end }, true)),
            };
            if let Some((block_end, closed)) = block_end {
                return Some(Block {
                    start,
                    end: block_end,
                    format,
                    calls: json_calls(&value, false),
                    closed,
                });
            }
        }
    }
    let (body_end, block_end, closed) = match format.close {
        Some(close) => match text[body_start..end].find(close) {
            Some(offset) => (body_start + offset, body_start + offset + close.len(), true),
            None => (end, end, false),
        },
        None => (next_open, next_open, true),
    };
    let body = &text[body_start..body_end];
    let mut first_error = None;
    for kind in format.bodies {
        match parse_body(*kind, body, tools) {
            Ok(calls) if !calls.is_empty() => {
                return Some(Block {
                    start,
                    end: block_end,
                    format,
                    calls: Ok(calls),
                    closed,
                });
            }
            Ok(_) => {}
            Err(err) => {
                first_error.get_or_insert(err);
            }
        }
    }
    Some(Block {
        start,
        end: block_end,
        format,
        calls: Err(first_error.unwrap_or_else(|| "the tool call could not be read".to_owned())),
        closed,
    })
}

/// The run of blocks of one format at the end of the message: the last block must end where
/// the content ends, and blocks before it count when only whitespace separates them.
fn trailing_group<'b>(text: &str, blocks: &'b [Block], end: usize) -> Option<Vec<&'b Block>> {
    let last = blocks.iter().max_by_key(|block| block.start)?;
    if last.end < end {
        return None;
    }
    let mut group = vec![last];
    let mut cursor = last.start;
    loop {
        let previous = blocks
            .iter()
            .filter(|block| {
                std::ptr::eq(block.format, last.format)
                    && block.end <= cursor
                    && text[block.end..cursor].trim().is_empty()
            })
            .max_by_key(|block| block.end);
        match previous {
            Some(block) => {
                cursor = block.start;
                group.push(block);
            }
            None => break,
        }
    }
    group.reverse();
    Some(group)
}

fn use_marker_group(
    text: &str,
    group: &[&Block],
    tools: &[ToolInfo],
    context: &RecoveryContext,
) -> Recovery {
    let mut calls = Vec::new();
    for block in group {
        match &block.calls {
            Ok(raw_calls) => {
                for raw in raw_calls {
                    calls.push(check_marker_call(raw, tools, context));
                }
            }
            Err(error) => calls.push(Recovered::Failed {
                name: String::new(),
                arguments: Map::new(),
                error: format!("{error}. {}", available(tools)),
            }),
        }
    }
    let echoed = group.iter().any(|block| match &block.calls {
        Ok(raw_calls) => raw_calls.iter().any(|raw| is_echo(raw, tools, context)),
        Err(_) => false,
    });
    if echoed {
        return Recovery::nothing(text, Some(Rejection::Echoed));
    }
    let start = group.first().map_or(0, |block| block.start);
    Recovery {
        text: text[..start].trim_end().to_owned(),
        calls,
        format: group.first().map(|block| block.format.family),
        rejected: None,
    }
}

fn check_marker_call(raw: &RawCall, tools: &[ToolInfo], context: &RecoveryContext) -> Recovered {
    let Some(tool) = tools.iter().find(|tool| tool.name == raw.name) else {
        let name = if raw.name.is_empty() {
            "(no name)".to_owned()
        } else {
            raw.name.clone()
        };
        return Recovered::Failed {
            name: raw.name.clone(),
            arguments: object_or_empty(&raw.arguments),
            error: format!("no such tool: {name}; {}", available(tools)),
        };
    };
    let repaired = repair_arguments(raw.arguments.clone(), &tool.input_schema);
    let Value::Object(arguments) = repaired else {
        return Recovered::Failed {
            name: raw.name.clone(),
            arguments: Map::new(),
            error: format!(
                "the arguments of {} are not a JSON object; send an object that matches the \
                 tool's schema",
                raw.name
            ),
        };
    };
    let value = Value::Object(arguments);
    match context.validate(&raw.name, &value) {
        Ok(()) => Recovered::Valid {
            name: raw.name.clone(),
            arguments: object_or_empty(&value),
        },
        Err(errors) => Recovered::Failed {
            name: raw.name.clone(),
            arguments: object_or_empty(&value),
            error: format!("invalid arguments for {}: {}", raw.name, errors.join("; ")),
        },
    }
}

fn use_strict_candidate(
    text: &str,
    start: usize,
    raw_calls: Vec<RawCall>,
    tools: &[ToolInfo],
    context: &RecoveryContext,
) -> Recovery {
    let mut calls = Vec::new();
    for raw in &raw_calls {
        let Some(tool) = tools.iter().find(|tool| tool.name == raw.name) else {
            return Recovery::nothing(text, Some(Rejection::UnknownTool(raw.name.clone())));
        };
        let repaired = repair_arguments(raw.arguments.clone(), &tool.input_schema);
        if !repaired.is_object() {
            return Recovery::nothing(
                text,
                Some(Rejection::InvalidArguments(vec![
                    "arguments are not a JSON object".to_owned(),
                ])),
            );
        }
        if let Err(errors) = context.validate(&raw.name, &repaired) {
            return Recovery::nothing(text, Some(Rejection::InvalidArguments(errors)));
        }
        calls.push(Recovered::Valid {
            name: raw.name.clone(),
            arguments: object_or_empty(&repaired),
        });
    }
    if raw_calls.iter().any(|raw| is_echo(raw, tools, context)) {
        return Recovery::nothing(text, Some(Rejection::Echoed));
    }
    Recovery {
        text: text[..start].trim_end().to_owned(),
        calls,
        format: Some(if text[start..].trim_start().starts_with(['`', '~']) {
            "fenced json"
        } else {
            "bare json"
        }),
        rejected: None,
    }
}

fn available(tools: &[ToolInfo]) -> String {
    if tools.is_empty() {
        return "no tools are available".to_owned();
    }
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    format!("available: {}", names.join(", "))
}

fn object_or_empty(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// Calls in a fenced or bare-JSON candidate: every element must be a strict call object.
fn strict_json_calls(body: &str) -> Option<Vec<RawCall>> {
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    json_calls(&value, true)
        .ok()
        .filter(|calls| !calls.is_empty())
}

/// Reads a call object or an array of them. Strict mode (fenced or bare JSON) requires the
/// arguments key and allows no other keys than a call object has.
fn json_calls(value: &Value, strict: bool) -> Result<Vec<RawCall>, String> {
    match value {
        Value::Array(items) => {
            if items.is_empty() {
                return Err("the tool-call list is empty".to_owned());
            }
            items
                .iter()
                .map(|item| {
                    call_from_json(item, strict)
                        .ok_or_else(|| "an entry of the tool-call list is not a call".to_owned())
                })
                .collect()
        }
        Value::Object(_) => call_from_json(value, strict)
            .map(|call| vec![call])
            .ok_or_else(|| {
                "the tool call is not an object with \"name\" and \"arguments\"".to_owned()
            }),
        _ => Err("the tool call is not a JSON object".to_owned()),
    }
}

fn call_from_json(value: &Value, strict: bool) -> Option<RawCall> {
    const CALL_KEYS: &[&str] = &["name", "arguments", "parameters", "id", "type", "function"];
    let object = value.as_object()?;
    if strict && object.keys().any(|key| !CALL_KEYS.contains(&key.as_str())) {
        return None;
    }
    let holder = match object.get("function") {
        Some(Value::Object(function)) => function,
        _ => object,
    };
    let name = holder.get("name")?.as_str()?.trim();
    if name.is_empty() {
        return None;
    }
    let arguments = holder
        .get("arguments")
        .or_else(|| holder.get("parameters"))
        .cloned();
    let arguments = match arguments {
        Some(arguments) => arguments,
        None if strict => return None,
        None => Value::Object(Map::new()),
    };
    Some(RawCall {
        name: name.to_owned(),
        arguments,
    })
}

fn parse_body(kind: BodyKind, body: &str, tools: &[ToolInfo]) -> Result<Vec<RawCall>, String> {
    match kind {
        BodyKind::Json => {
            let value: Value = serde_json::from_str(body.trim())
                .map_err(|err| format!("the tool call is not valid JSON ({err})"))?;
            json_calls(&value, false)
        }
        BodyKind::XmlFunction => {
            let trimmed = body.trim();
            if !trimmed.starts_with("<function=") {
                return Ok(Vec::new());
            }
            let mut calls = Vec::new();
            let mut rest = trimmed;
            while let Some(pos) = rest.find("<function=") {
                let after = &rest[pos + "<function=".len()..];
                let (call, consumed) = parse_function_tag(after, tools)?;
                calls.push(call);
                rest = &after[consumed..];
            }
            Ok(calls)
        }
        BodyKind::FunctionTag => {
            let (call, _) = parse_function_tag(body, tools)?;
            Ok(vec![call])
        }
        BodyKind::ArgKeyValue => {
            parse_arg_key_value(body, tools).map(|call| call.into_iter().collect())
        }
        BodyKind::NameArgs => parse_name_args(body).map(|call| vec![call]),
        BodyKind::PythonCall => parse_python_call(body, tools).map(|call| vec![call]),
    }
}

/// Reads `name>BODY</function>` (the text after `<function=`), where BODY is a JSON object
/// or `<parameter=…>` children. Returns the call and the bytes consumed.
fn parse_function_tag(after: &str, tools: &[ToolInfo]) -> Result<(RawCall, usize), String> {
    let close = after
        .find('>')
        .ok_or_else(|| "the function tag is not closed with '>'".to_owned())?;
    let name = after[..close].trim();
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err(format!("'{name}' is not a tool name"));
    }
    let inner_start = close + 1;
    let (inner_end, consumed) = match after[inner_start..].find("</function>") {
        Some(offset) => (
            inner_start + offset,
            inner_start + offset + "</function>".len(),
        ),
        None => (after.len(), after.len()),
    };
    let inner = after[inner_start..inner_end].trim();
    let schema = tool_schema(tools, name);
    let arguments = if inner.is_empty() {
        Value::Object(Map::new())
    } else if inner.starts_with("<parameter=") {
        Value::Object(parse_parameters(inner, schema))
    } else {
        serde_json::from_str::<Value>(inner)
            .map_err(|err| format!("the arguments of {name} are not valid JSON ({err})"))?
    };
    Ok((
        RawCall {
            name: name.to_owned(),
            arguments,
        },
        consumed,
    ))
}

/// Reads `<parameter=key>value</parameter>` pairs. A missing closing tag ends the value at
/// the next parameter or at the end.
fn parse_parameters(inner: &str, schema: Option<&Value>) -> Map<String, Value> {
    let mut map = Map::new();
    let mut rest = inner;
    while let Some(pos) = rest.find("<parameter=") {
        let after = &rest[pos + "<parameter=".len()..];
        let Some(close) = after.find('>') else {
            break;
        };
        let key = after[..close].trim().to_owned();
        let value_area = &after[close + 1..];
        let value_end = [
            value_area.find("</parameter>"),
            value_area.find("<parameter="),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(value_area.len());
        let raw = &value_area[..value_end];
        map.insert(key.clone(), param_value(raw, property_schema(schema, &key)));
        rest = value_area[value_end..]
            .strip_prefix("</parameter>")
            .unwrap_or(&value_area[value_end..]);
    }
    map
}

/// `name<arg_key>k</arg_key><arg_value>v</arg_value>…` (GLM 4.6/4.7). `Ok(None)` when the body
/// is not in this format.
fn parse_arg_key_value(body: &str, tools: &[ToolInfo]) -> Result<Option<RawCall>, String> {
    let trimmed = body.trim();
    let Some(first) = trimmed.find("<arg_key>") else {
        // A name alone is a call without arguments; anything else is not this format.
        return Ok(
            (!trimmed.is_empty() && trimmed.chars().all(is_name_char)).then(|| RawCall {
                name: trimmed.to_owned(),
                arguments: Value::Object(Map::new()),
            }),
        );
    };
    let name = trimmed[..first].trim();
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err(format!("'{name}' is not a tool name"));
    }
    let schema = tool_schema(tools, name);
    let mut map = Map::new();
    let mut rest = &trimmed[first..];
    while let Some(pos) = rest.find("<arg_key>") {
        let after = &rest[pos + "<arg_key>".len()..];
        let key_end = after
            .find("</arg_key>")
            .ok_or_else(|| "an <arg_key> is not closed".to_owned())?;
        let key = after[..key_end].trim().to_owned();
        let after_key = &after[key_end + "</arg_key>".len()..];
        let value_start = after_key
            .find("<arg_value>")
            .ok_or_else(|| format!("the argument {key} has no <arg_value>"))?;
        let value_area = &after_key[value_start + "<arg_value>".len()..];
        let value_end = value_area.find("</arg_value>").unwrap_or(value_area.len());
        map.insert(
            key.clone(),
            param_value(&value_area[..value_end], property_schema(schema, &key)),
        );
        rest = &value_area[value_end..];
    }
    Ok(Some(RawCall {
        name: name.to_owned(),
        arguments: Value::Object(map),
    }))
}

/// `name[ARGS]{json}`, optionally with `[CALL_ID]id` before `[ARGS]` (newer Mistral).
fn parse_name_args(body: &str) -> Result<RawCall, String> {
    let trimmed = body.trim();
    let args_at = trimmed
        .find("[ARGS]")
        .ok_or_else(|| "the tool call has no [ARGS]".to_owned())?;
    let head = &trimmed[..args_at];
    let name = head.split("[CALL_ID]").next().unwrap_or(head).trim();
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err(format!("'{name}' is not a tool name"));
    }
    let json = trimmed[args_at + "[ARGS]".len()..].trim();
    let arguments = if json.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_str(json)
            .map_err(|err| format!("the arguments of {name} are not valid JSON ({err})"))?
    };
    Ok(RawCall {
        name: name.to_owned(),
        arguments,
    })
}

/// `name.call(key="value", n=1)` or `name(key="value")` with literal arguments only.
fn parse_python_call(body: &str, tools: &[ToolInfo]) -> Result<RawCall, String> {
    let trimmed = body.trim();
    let open = trimmed
        .find('(')
        .ok_or_else(|| "the tool call has no argument list".to_owned())?;
    if !trimmed.ends_with(')') {
        return Err("the argument list is not closed".to_owned());
    }
    let head = trimmed[..open].trim();
    let name = head.strip_suffix(".call").unwrap_or(head);
    if name.is_empty() || !name.chars().all(is_name_char) {
        return Err(format!("'{name}' is not a tool name"));
    }
    let schema = tool_schema(tools, name);
    let mut parser = PyArgs {
        text: &trimmed[open + 1..trimmed.len() - 1],
        pos: 0,
    };
    let mut map = Map::new();
    loop {
        parser.skip_ws();
        if parser.done() {
            break;
        }
        let key = parser.ident()?;
        parser.skip_ws();
        parser.expect('=')?;
        parser.skip_ws();
        let value = parser.literal()?;
        let value = match (value, property_schema(schema, &key).and_then(type_of)) {
            // `path=5` for a string parameter: keep the model's text.
            (Value::Number(n), Some("string")) => Value::String(n.to_string()),
            (value, _) => value,
        };
        map.insert(key, value);
        parser.skip_ws();
        if parser.done() {
            break;
        }
        parser.expect(',')?;
    }
    Ok(RawCall {
        name: name.to_owned(),
        arguments: Value::Object(map),
    })
}

struct PyArgs<'t> {
    text: &'t str,
    pos: usize,
}

impl PyArgs<'_> {
    fn rest(&self) -> &str {
        &self.text[self.pos..]
    }

    fn done(&self) -> bool {
        self.pos >= self.text.len()
    }

    fn skip_ws(&mut self) {
        self.pos = skip_ws(self.text, self.pos);
    }

    fn expect(&mut self, c: char) -> Result<(), String> {
        if self.rest().starts_with(c) {
            self.pos += c.len_utf8();
            Ok(())
        } else {
            Err(format!("expected '{c}' in the argument list"))
        }
    }

    fn ident(&mut self) -> Result<String, String> {
        let len = self
            .rest()
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(self.rest().len());
        if len == 0 {
            return Err("expected an argument name".to_owned());
        }
        let ident = self.rest()[..len].to_owned();
        self.pos += len;
        Ok(ident)
    }

    fn literal(&mut self) -> Result<Value, String> {
        let rest = self.rest();
        if let Some(quote) = rest.chars().next().filter(|c| *c == '"' || *c == '\'') {
            let mut out = String::new();
            let mut chars = rest.char_indices().skip(1);
            while let Some((i, c)) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some((_, 'n')) => out.push('\n'),
                        Some((_, 't')) => out.push('\t'),
                        Some((_, 'r')) => out.push('\r'),
                        Some((_, other)) => out.push(other),
                        None => break,
                    },
                    c if c == quote => {
                        self.pos += i + 1;
                        return Ok(Value::String(out));
                    }
                    c => out.push(c),
                }
            }
            return Err("a string argument is not closed".to_owned());
        }
        let len = rest
            .find(|c: char| c == ',' || c.is_whitespace())
            .unwrap_or(rest.len());
        let word = &rest[..len];
        let value = match word {
            "True" | "true" => Value::Bool(true),
            "False" | "false" => Value::Bool(false),
            "None" | "null" => Value::Null,
            _ => match serde_json::from_str::<Value>(word) {
                Ok(number @ Value::Number(_)) => number,
                _ => return Err(format!("'{word}' is not a literal value")),
            },
        };
        self.pos += len;
        Ok(value)
    }
}

fn tool_schema<'t>(tools: &'t [ToolInfo], name: &str) -> Option<&'t Value> {
    tools
        .iter()
        .find(|tool| tool.name == name)
        .map(|tool| &tool.input_schema)
}

fn property_schema<'s>(schema: Option<&'s Value>, key: &str) -> Option<&'s Value> {
    schema?.get("properties")?.get(key)
}

fn type_of(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(Value::as_str)
}

/// A parameter value written as text: kept as text for a string parameter, otherwise read as
/// JSON when it is JSON. One newline right after the opening tag and one right before the
/// closing tag belong to the format, not to the value.
fn param_value(raw: &str, schema: Option<&Value>) -> Value {
    let raw = raw.strip_prefix('\n').unwrap_or(raw);
    let raw = raw.strip_suffix('\n').unwrap_or(raw);
    if schema.and_then(type_of) == Some("string") {
        return Value::String(raw.to_owned());
    }
    serde_json::from_str::<Value>(raw.trim()).unwrap_or_else(|_| Value::String(raw.to_owned()))
}

/// True when `raw` matches call-shaped text in any context text: the same tool name and the
/// same arguments, as written or after repair.
fn is_echo(raw: &RawCall, tools: &[ToolInfo], context: &RecoveryContext) -> bool {
    let schema = tool_schema(tools, &raw.name);
    let repaired = schema.map(|schema| repair_arguments(raw.arguments.clone(), schema));
    let matches = |shape: &RawCall| {
        if shape.name != raw.name {
            return false;
        }
        if shape.arguments == raw.arguments {
            return true;
        }
        match (schema, &repaired) {
            (Some(schema), Some(repaired)) => {
                &repair_arguments(shape.arguments.clone(), schema) == repaired
            }
            _ => false,
        }
    };
    context
        .texts
        .iter()
        .any(|text| call_shapes_near_name(text, &raw.name, tools).any(|shape| matches(&shape)))
}

/// Call shapes in `text` that could contain an occurrence of `name`: for each occurrence,
/// JSON objects and markers starting shortly before it, and a Python-style call at it.
fn call_shapes_near_name<'t>(
    text: &'t str,
    name: &'t str,
    tools: &'t [ToolInfo],
) -> impl Iterator<Item = RawCall> + 't {
    let code = CodeMap::default();
    text.match_indices(name)
        .take(ECHO_MAX_OCCURRENCES)
        .flat_map(move |(pos, _)| {
            let from = floor_boundary(text, pos.saturating_sub(ECHO_LOOKBACK_BYTES));
            let window = &text[from..pos];
            let mut starts: Vec<usize> = window
                .char_indices()
                .filter(|(i, c)| {
                    *c == '{'
                        || *c == '['
                        || MARKER_FORMATS
                            .iter()
                            .any(|format| window[*i..].starts_with(format.open))
                })
                .map(|(i, _)| from + i)
                .collect();
            starts.reverse();
            starts.truncate(ECHO_MAX_STARTS);
            starts.push(pos);
            let mut shapes = Vec::new();
            for start in starts {
                shapes.extend(shapes_at(text, start, tools, &code));
            }
            shapes
        })
}

/// Call shapes starting at `start`: a JSON value, a marker block, or a Python-style call.
/// Reads at most [`ECHO_WINDOW_BYTES`] after `start`.
fn shapes_at(text: &str, start: usize, tools: &[ToolInfo], code: &CodeMap) -> Vec<RawCall> {
    let window_end = floor_boundary(text, start.saturating_add(ECHO_WINDOW_BYTES));
    let window = &text[..window_end];
    let rest = &window[start..];
    if rest.starts_with(['{', '[']) {
        return json_value_at(window, start, ECHO_WINDOW_BYTES)
            .and_then(|(value, _)| json_calls(&value, false).ok())
            .unwrap_or_default();
    }
    if let Some(format) = MARKER_FORMATS
        .iter()
        .find(|format| rest.starts_with(format.open))
    {
        let end = window.len();
        let next_open = find_outside_code(window, format.open, code)
            .find(|pos| *pos > start)
            .unwrap_or(end);
        return parse_block(window, start, end, next_open, format, tools)
            .and_then(|block| block.calls.ok())
            .unwrap_or_default();
    }
    // `name.call(…)` or `name(…)` at the occurrence itself.
    match python_call_end(rest) {
        Some(close) => parse_python_call(&rest[..close], tools)
            .map(|call| vec![call])
            .unwrap_or_default(),
        None => Vec::new(),
    }
}

/// The byte just after the `)` closing the first argument list in `text`, skipping quoted
/// strings. `None` when there is no complete argument list.
fn python_call_end(text: &str) -> Option<usize> {
    let open = text.find('(')?;
    // Only an identifier (with dots) may come before the list.
    if !text[..open].trim().chars().all(is_name_char) {
        return None;
    }
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (i, c) in text[open + 1..].char_indices() {
        match quote {
            Some(q) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                ')' => return Some(open + 1 + i + 1),
                _ => {}
            },
        }
    }
    None
}

#[cfg(test)]
mod tests;
