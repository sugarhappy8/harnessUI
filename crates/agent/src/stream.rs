//! Turning chat-completion chunks into stream events, and folding tool-call events back into
//! whole calls.
//!
//! The reader is tolerant on purpose (ARCHITECTURE.md 8.2, "Tolerant stream reader"):
//!
//! - reasoning in `delta.reasoning_content` (llama.cpp, LM Studio) or `delta.reasoning`
//!   (Ollama, vLLM);
//! - tool-call arguments in fragments (llama.cpp, LM Studio) or whole in one chunk (Ollama);
//!   [`ToolCallAccumulator`] replaces rather than appends when a later chunk for the same
//!   index carries a complete JSON object;
//! - missing or duplicate tool-call ids are replaced with `call_<uuid>`;
//! - a usage chunk with `choices: []`;
//! - `data: {"error":...}` mid-stream ends the stream with an error;
//! - a stream that ends without `[DONE]` still ends with [`StreamEvent::Done`]; when it also
//!   had no finish reason, [`StreamEvent::Unfinished`] comes first.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{ProviderError, as_u32, error_from_body, error_from_value};
use crate::sse::{SseDecoder, SseEvent};
use crate::types::ToolCall;

/// Token counts reported by the server for one response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    /// Tokens in the prompt, including cached ones.
    pub prompt_tokens: u32,
    /// Tokens generated.
    pub completion_tokens: u32,
    /// Prompt tokens served from the server's cache, when reported.
    pub cached_tokens: Option<u32>,
}

/// llama-server's `timings` object. Every field is optional because other servers send none
/// of it and llama-server may drop fields between builds.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timings {
    /// Prompt tokens evaluated (not counting cached ones).
    pub prompt_n: Option<u32>,
    /// Time spent on the prompt, in milliseconds.
    pub prompt_ms: Option<f64>,
    /// Prompt reading speed in tokens per second.
    pub prompt_per_second: Option<f64>,
    /// Tokens generated.
    pub predicted_n: Option<u32>,
    /// Time spent generating, in milliseconds.
    pub predicted_ms: Option<f64>,
    /// Generation speed in tokens per second.
    pub predicted_per_second: Option<f64>,
    /// Prompt tokens reused from the cache.
    pub cache_n: Option<u32>,
}

impl Timings {
    /// Tokens now in the context: `prompt_n + cache_n + predicted_n` (llama-server README,
    /// "Timings and context usage"). `None` unless all three were reported.
    pub fn context_used(&self) -> Option<u32> {
        self.prompt_n?
            .checked_add(self.cache_n?)?
            .checked_add(self.predicted_n?)
    }
}

/// Why the model stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FinishReason {
    /// End of turn or a stop sequence.
    Stop,
    /// The output limit (`max_tokens`) or the context ran out.
    Length,
    /// The model called tools. (Tool calls count whenever any arrived, whatever the finish
    /// reason says.)
    ToolCalls,
    /// The server filtered the output.
    ContentFilter,
    /// Anything else, as sent.
    Other(String),
}

impl FinishReason {
    /// Maps the wire value. `function_call` (legacy OpenAI) and `tool` (stale llama.cpp docs)
    /// count as tool calls.
    pub fn from_wire(value: &str) -> Self {
        match value {
            "stop" | "eos" | "end_turn" => Self::Stop,
            "length" | "max_tokens" => Self::Length,
            "tool_calls" | "function_call" | "tool" | "tool_use" => Self::ToolCalls,
            "content_filter" => Self::ContentFilter,
            other => Self::Other(other.to_owned()),
        }
    }
}

/// One event from a streamed chat completion.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// Answer text.
    Content(String),
    /// Reasoning ("thinking") text.
    Reasoning(String),
    /// A tool call began. `id` is unique within the response (synthesised when the server
    /// sent none or a duplicate). `name` is empty only if the server never sent one.
    ToolCallStarted {
        /// The call's position in the response.
        index: usize,
        /// The call id to answer in the tool result.
        id: String,
        /// The tool name.
        name: String,
    },
    /// More argument text for the call at `index`. Fold these with [`ToolCallAccumulator`],
    /// which also handles servers that resend a call's arguments whole.
    ToolCallArgs {
        /// The call's position in the response.
        index: usize,
        /// Argument text as sent.
        fragment: String,
    },
    /// Token counts.
    Usage(Usage),
    /// llama-server timings (tokens per second, cache use).
    Timings(Timings),
    /// The model stopped. Sent at most once.
    Finished {
        /// Why it stopped.
        finish_reason: FinishReason,
    },
    /// An event stream's input ended with neither a finish reason nor `[DONE]`: the server
    /// or something between closed the connection early, so the reply may be cut short.
    /// Sent just before [`StreamEvent::Done`].
    Unfinished,
    /// The stream ended normally. Always the last event of a stream that did not fail.
    Done,
}

#[derive(Debug, Default)]
struct CallState {
    started: bool,
    id: Option<String>,
    name: Option<String>,
    pending_args: String,
}

/// Maps decoded chunks (JSON objects) to [`StreamEvent`]s, keeping the per-response state
/// that tool-call ids and names need.
#[derive(Debug, Default)]
pub struct ChunkParser {
    calls: BTreeMap<usize, CallState>,
    used_ids: HashSet<String>,
    last_index: Option<usize>,
    finished_sent: bool,
    done: bool,
}

impl ChunkParser {
    /// A parser for one response.
    pub fn new() -> Self {
        Self::default()
    }

    /// True after `[DONE]`, [`ChunkParser::finish`] or a terminal error. Later input is
    /// ignored.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Handles one SSE event. Events produced before an error stay in `out`.
    ///
    /// # Errors
    /// A mid-stream error object, an `event: error` event, or data that is not JSON.
    pub fn push_event(
        &mut self,
        event: &SseEvent,
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        if self.done {
            return Ok(());
        }
        let is_error_event = match event.event.as_deref() {
            // An empty event type means "message" (WHATWG event-stream rules).
            None | Some("" | "message") => false,
            Some("error") => true,
            // Vendor events such as `ping` carry nothing for the chat.
            Some(_) => return Ok(()),
        };
        if event.is_done() {
            self.finish_into(out);
            return Ok(());
        }
        let data = event.data.trim();
        if data.is_empty() {
            if is_error_event {
                self.done = true;
                return Err(error_from_body(None, b"", "the server reported an error"));
            }
            return Ok(());
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(_) if is_error_event => {
                // `event: error` with a plain-text message: report the server's words.
                self.done = true;
                return Err(error_from_body(None, data.as_bytes(), ""));
            }
            Err(err) => {
                self.done = true;
                return Err(ProviderError::Decode(format!(
                    "a stream event is not JSON ({err})"
                )));
            }
        };
        if is_error_event {
            self.done = true;
            return Err(error_from_value(&value, None));
        }
        self.push_chunk(&value, out)
    }

    /// Handles one chunk object (a streaming `chat.completion.chunk`, or a whole
    /// non-streaming `chat.completion`, whose `message` is read like a delta).
    ///
    /// # Errors
    /// A chunk that carries an error object, or one that is not a JSON object.
    pub fn push_chunk(
        &mut self,
        chunk: &Value,
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        if self.done {
            return Ok(());
        }
        let Some(object) = chunk.as_object() else {
            self.done = true;
            return Err(ProviderError::Decode(
                "a stream chunk is not a JSON object".to_owned(),
            ));
        };
        if object.get("error").is_some_and(|e| !e.is_null())
            || object.get("object").and_then(Value::as_str) == Some("error")
        {
            self.done = true;
            return Err(error_from_value(chunk, None));
        }
        if let Some(choices) = object.get("choices").and_then(Value::as_array) {
            for choice in choices {
                // Only one choice is ever requested; ignore any other.
                if choice.get("index").and_then(Value::as_u64).unwrap_or(0) != 0 {
                    continue;
                }
                let delta = choice
                    .get("delta")
                    .filter(|d| d.is_object())
                    .or_else(|| choice.get("message"));
                if let Some(Value::Object(delta)) = delta {
                    self.on_delta(delta, out);
                }
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    self.flush_unnamed_calls(out);
                    if !self.finished_sent {
                        self.finished_sent = true;
                        out.push(StreamEvent::Finished {
                            finish_reason: FinishReason::from_wire(reason),
                        });
                    }
                }
            }
        }
        if let Some(Value::Object(usage)) = object.get("usage") {
            out.push(StreamEvent::Usage(parse_usage(usage)));
        }
        if let Some(Value::Object(timings)) = object.get("timings") {
            out.push(StreamEvent::Timings(parse_timings(timings)));
        }
        Ok(())
    }

    /// Ends the response: announces any call that never got a name, then [`StreamEvent::Done`]
    /// unless the parser already finished.
    pub fn finish(&mut self, out: &mut Vec<StreamEvent>) {
        if !self.done {
            self.finish_into(out);
        }
    }

    fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        self.flush_unnamed_calls(out);
        self.done = true;
        out.push(StreamEvent::Done);
    }

    fn on_delta(&mut self, delta: &Map<String, Value>, out: &mut Vec<StreamEvent>) {
        let reasoning = non_empty_str(delta.get("reasoning_content"))
            .or_else(|| non_empty_str(delta.get("reasoning")));
        if let Some(text) = reasoning {
            out.push(StreamEvent::Reasoning(text.to_owned()));
        }
        if let Some(text) = non_empty_str(delta.get("content")) {
            out.push(StreamEvent::Content(text.to_owned()));
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for (position, call) in calls.iter().enumerate() {
                self.on_tool_call(position, call, out);
            }
        } else if let Some(function) = delta.get("function_call").filter(|f| f.is_object()) {
            // Legacy OpenAI single function call.
            let call = serde_json::json!({ "index": 0, "function": function });
            self.on_tool_call(0, &call, out);
        }
    }

    fn on_tool_call(&mut self, position: usize, call: &Value, out: &mut Vec<StreamEvent>) {
        let id = non_empty_str(call.get("id"));
        let function = call.get("function");
        let name = non_empty_str(function.and_then(|f| f.get("name")));
        let arguments = function
            .and_then(|f| f.get("arguments"))
            .and_then(|args| match args {
                Value::String(text) => Some(text.clone()),
                Value::Null => None,
                other => Some(other.to_string()),
            })
            .filter(|args| !args.is_empty());
        let index = match call
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|i| usize::try_from(i).ok())
        {
            Some(index) => index,
            None => self.infer_index(id, position),
        };
        self.last_index = Some(index);

        let state = self.calls.entry(index).or_default();
        if state.id.is_none() {
            state.id = id.map(str::to_owned);
        }
        if state.name.is_none() {
            state.name = name.map(str::to_owned);
        }
        if state.started {
            if let Some(fragment) = arguments {
                out.push(StreamEvent::ToolCallArgs { index, fragment });
            }
            return;
        }
        if let Some(fragment) = arguments {
            state.pending_args.push_str(&fragment);
        }
        if state.name.is_some() {
            self.start_call(index, out);
        }
    }

    /// The index for a call the server sent without one: the call with the same id if
    /// there is one, and a new index for a new id. Without an id, a later entry of the same
    /// `tool_calls` array (`position > 0`) is always another call, while the first entry
    /// continues the call last written to.
    fn infer_index(&self, id: Option<&str>, position: usize) -> usize {
        let next_free = self.calls.keys().next_back().map_or(0, |last| last + 1);
        if let Some(id) = id {
            if let Some((&index, _)) = self
                .calls
                .iter()
                .find(|(_, state)| state.id.as_deref() == Some(id))
            {
                return index;
            }
            return next_free;
        }
        if position > 0 {
            return next_free;
        }
        self.last_index.unwrap_or(0)
    }

    fn start_call(&mut self, index: usize, out: &mut Vec<StreamEvent>) {
        let Some(state) = self.calls.get_mut(&index) else {
            return;
        };
        state.started = true;
        let id = match state.id.take() {
            Some(id) if !self.used_ids.contains(&id) => id,
            _ => format!("call_{}", uuid::Uuid::new_v4().simple()),
        };
        self.used_ids.insert(id.clone());
        state.id = Some(id.clone());
        let name = state.name.clone().unwrap_or_default();
        let pending = std::mem::take(&mut state.pending_args);
        out.push(StreamEvent::ToolCallStarted { index, id, name });
        if !pending.is_empty() {
            out.push(StreamEvent::ToolCallArgs {
                index,
                fragment: pending,
            });
        }
    }

    /// Announces calls that got arguments or an id but never a name, so nothing the server
    /// sent is dropped. The agent loop treats the empty name as an unknown tool.
    fn flush_unnamed_calls(&mut self, out: &mut Vec<StreamEvent>) {
        let unnamed: Vec<usize> = self
            .calls
            .iter()
            .filter(|(_, state)| !state.started)
            .map(|(&index, _)| index)
            .collect();
        for index in unnamed {
            self.start_call(index, out);
        }
    }
}

fn non_empty_str(value: Option<&Value>) -> Option<&str> {
    value
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

fn parse_usage(usage: &Map<String, Value>) -> Usage {
    Usage {
        prompt_tokens: usage.get("prompt_tokens").and_then(as_u32).unwrap_or(0),
        completion_tokens: usage.get("completion_tokens").and_then(as_u32).unwrap_or(0),
        cached_tokens: usage
            .get("prompt_tokens_details")
            .and_then(|details| details.get("cached_tokens"))
            .and_then(as_u32),
    }
}

fn parse_timings(timings: &Map<String, Value>) -> Timings {
    let count = |key: &str| timings.get(key).and_then(as_u32);
    let number = |key: &str| {
        timings
            .get(key)
            .and_then(Value::as_f64)
            .filter(|n| n.is_finite())
    };
    Timings {
        prompt_n: count("prompt_n"),
        prompt_ms: number("prompt_ms"),
        prompt_per_second: number("prompt_per_second"),
        predicted_n: count("predicted_n"),
        predicted_ms: number("predicted_ms"),
        predicted_per_second: number("predicted_per_second"),
        cache_n: count("cache_n"),
    }
}

/// Bytes in, stream events out: an [`SseDecoder`] feeding a [`ChunkParser`].
#[derive(Debug, Default)]
pub struct StreamDecoder {
    sse: SseDecoder,
    parser: ChunkParser,
    /// At least one server-sent event arrived. A body that ends without any (an empty body,
    /// or a web page served with status 200 by a wrong base URL) is not a chat stream.
    saw_event: bool,
}

impl StreamDecoder {
    /// A decoder for one response.
    pub fn new() -> Self {
        Self::default()
    }

    /// True once the stream finished (`[DONE]`, a terminal error, or [`StreamDecoder::finish`]).
    pub fn is_done(&self) -> bool {
        self.parser.is_done()
    }

    /// Feeds bytes. Events produced before an error stay in `out`; after an error or
    /// `[DONE]` further input is ignored.
    ///
    /// # Errors
    /// See [`ChunkParser::push_event`]; also an event larger than the decoder's limit.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.parser.is_done() {
            return Ok(());
        }
        let events = self.sse.push(bytes);
        self.saw_event |= !events.is_empty();
        self.check_overflow()?;
        for event in &events {
            self.parser.push_event(event, out)?;
            if self.parser.is_done() {
                break;
            }
        }
        Ok(())
    }

    /// Ends the input: delivers an unterminated last event, then [`StreamEvent::Done`] if the
    /// server never sent `[DONE]`.
    ///
    /// # Errors
    /// As for [`StreamDecoder::push`]; also [`ProviderError::Decode`] when the whole input
    /// held no server-sent event at all (an empty body, or an HTML page answering with
    /// status 200), which is not a chat-completion stream.
    pub fn finish(&mut self, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.parser.is_done() {
            return Ok(());
        }
        let events = self.sse.finish();
        self.saw_event |= !events.is_empty();
        self.check_overflow()?;
        for event in &events {
            self.parser.push_event(event, out)?;
        }
        if !self.saw_event && !self.parser.is_done() {
            // Mark the parser finished so nothing else is read.
            let mut ignored = Vec::new();
            self.parser.finish(&mut ignored);
            return Err(ProviderError::Decode(
                "the response held no server-sent events, so it is not a chat-completion \
                 stream (check the base URL)"
                    .to_owned(),
            ));
        }
        if !self.parser.is_done() && !self.parser.finished_sent {
            out.push(StreamEvent::Unfinished);
        }
        self.parser.finish(out);
        Ok(())
    }

    fn check_overflow(&mut self) -> Result<(), ProviderError> {
        if self.sse.overflowed() {
            // Mark the parser finished so nothing else is read.
            let mut ignored = Vec::new();
            self.parser.finish(&mut ignored);
            return Err(ProviderError::Decode(format!(
                "the server sent an event larger than {} MiB",
                SseDecoder::DEFAULT_MAX_BYTES / (1024 * 1024)
            )));
        }
        Ok(())
    }
}

/// Whether a call's accumulated arguments can be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentsStatus {
    /// The arguments parse as a JSON object. (Empty arguments count as `{}`.)
    Object,
    /// The arguments are not a JSON object: invalid JSON, or valid JSON of another type
    /// (for example a double-encoded string, which the agent loop's repair step handles).
    NotObject {
        /// What is wrong, for the error returned to the model.
        reason: String,
    },
    /// The response hit the output limit while this call was being written: the call is cut
    /// off and must not run, whether or not its partial arguments happen to parse.
    Truncated,
}

/// A tool call assembled from stream events, with the state of its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccumulatedToolCall {
    /// The call, with arguments exactly as received (empty arguments become `{}`).
    pub call: ToolCall,
    /// Whether the arguments are usable.
    pub status: ArgumentsStatus,
}

impl AccumulatedToolCall {
    /// The arguments as a JSON object, when [`ArgumentsStatus::Object`].
    pub fn arguments_object(&self) -> Option<Map<String, Value>> {
        if self.status != ArgumentsStatus::Object {
            return None;
        }
        match serde_json::from_str(&self.call.function.arguments) {
            Ok(Value::Object(map)) => Some(map),
            _ => None,
        }
    }
}

#[derive(Debug, Default)]
struct PartialCall {
    id: String,
    name: String,
    arguments: String,
}

/// Folds tool-call events into whole calls.
///
/// Argument fragments are appended, except that a fragment which is itself a complete JSON
/// object, arriving when the arguments so far are already a complete JSON object, replaces
/// them (a server resending a call whole, Ollama style). Tool calls count if any arrived,
/// whatever the finish reason says.
#[derive(Debug, Default)]
pub struct ToolCallAccumulator {
    calls: BTreeMap<usize, PartialCall>,
    finish_reason: Option<FinishReason>,
}

impl ToolCallAccumulator {
    /// An empty accumulator.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds one event. Events other than tool calls and [`StreamEvent::Finished`] are
    /// ignored.
    pub fn push(&mut self, event: &StreamEvent) {
        match event {
            StreamEvent::ToolCallStarted { index, id, name } => {
                let call = self.calls.entry(*index).or_default();
                if call.id.is_empty() {
                    call.id.clone_from(id);
                }
                if call.name.is_empty() {
                    call.name.clone_from(name);
                }
            }
            StreamEvent::ToolCallArgs { index, fragment } => {
                let call = self.calls.entry(*index).or_default();
                if is_json_object(fragment) && is_json_object(&call.arguments) {
                    call.arguments.clone_from(fragment);
                } else {
                    call.arguments.push_str(fragment);
                }
            }
            StreamEvent::Finished { finish_reason } => {
                self.finish_reason = Some(finish_reason.clone());
            }
            _ => {}
        }
    }

    /// True when no tool call arrived.
    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// The finish reason seen, if any.
    pub fn finish_reason(&self) -> Option<&FinishReason> {
        self.finish_reason.as_ref()
    }

    /// The calls in index order. When the response ended on the output limit, the last call
    /// is marked [`ArgumentsStatus::Truncated`]; earlier calls were complete, because the
    /// model had moved on to the next one.
    pub fn finish(&self) -> Vec<AccumulatedToolCall> {
        let truncated_index = match self.finish_reason {
            Some(FinishReason::Length) => self.calls.keys().next_back().copied(),
            _ => None,
        };
        self.calls
            .iter()
            .map(|(index, partial)| {
                let id = if partial.id.is_empty() {
                    format!("call_{}", uuid::Uuid::new_v4().simple())
                } else {
                    partial.id.clone()
                };
                let mut arguments = partial.arguments.clone();
                let status = if Some(*index) == truncated_index {
                    ArgumentsStatus::Truncated
                } else if arguments.trim().is_empty() {
                    arguments = "{}".to_owned();
                    ArgumentsStatus::Object
                } else {
                    match serde_json::from_str::<Value>(&arguments) {
                        Ok(Value::Object(_)) => ArgumentsStatus::Object,
                        Ok(other) => ArgumentsStatus::NotObject {
                            reason: format!("arguments are a JSON {}", json_type(&other)),
                        },
                        Err(err) => ArgumentsStatus::NotObject {
                            reason: format!("arguments are not valid JSON: {err}"),
                        },
                    }
                };
                AccumulatedToolCall {
                    call: ToolCall::function(id, partial.name.clone(), arguments),
                    status,
                }
            })
            .collect()
    }
}

fn is_json_object(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('{') && matches!(serde_json::from_str::<Value>(text), Ok(Value::Object(_)))
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse_chunks(chunks: &[Value]) -> Vec<StreamEvent> {
        let mut parser = ChunkParser::new();
        let mut out = Vec::new();
        for chunk in chunks {
            parser.push_chunk(chunk, &mut out).unwrap();
        }
        parser.finish(&mut out);
        out
    }

    fn accumulate(events: &[StreamEvent]) -> Vec<AccumulatedToolCall> {
        let mut acc = ToolCallAccumulator::new();
        for event in events {
            acc.push(event);
        }
        acc.finish()
    }

    #[test]
    fn finish_reasons_map() {
        assert_eq!(FinishReason::from_wire("stop"), FinishReason::Stop);
        assert_eq!(FinishReason::from_wire("length"), FinishReason::Length);
        assert_eq!(
            FinishReason::from_wire("tool_calls"),
            FinishReason::ToolCalls
        );
        assert_eq!(FinishReason::from_wire("tool"), FinishReason::ToolCalls);
        assert_eq!(
            FinishReason::from_wire("content_filter"),
            FinishReason::ContentFilter
        );
        assert_eq!(
            FinishReason::from_wire("weird"),
            FinishReason::Other("weird".into())
        );
    }

    #[test]
    fn ollama_style_whole_calls_and_reasoning_field() {
        // Ollama: `reasoning`, each call whole in one chunk with a `call_` id, usage in its own
        // chunk, finish chunk with an empty delta.
        let events = parse_chunks(&[
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"","reasoning":"Let me check."}}]}),
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"","tool_calls":[
                {"id":"call_ab12cd34","index":0,"type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Paris\"}"}}
            ]}}]}),
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":"","tool_calls":[
                {"id":"call_ef56gh78","index":1,"type":"function","function":{"name":"get_time","arguments":"{\"tz\":\"CET\"}"}}
            ]}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":50,"completion_tokens":20,"total_tokens":70,"prompt_tokens_details":{"cached_tokens":10}}}),
        ]);
        assert_eq!(events[0], StreamEvent::Reasoning("Let me check.".into()));
        assert!(events.contains(&StreamEvent::Usage(Usage {
            prompt_tokens: 50,
            completion_tokens: 20,
            cached_tokens: Some(10)
        })));
        assert_eq!(events.last(), Some(&StreamEvent::Done));
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].call.id, "call_ab12cd34");
        assert_eq!(calls[0].call.function.name, "get_weather");
        assert_eq!(calls[0].call.function.arguments, r#"{"city":"Paris"}"#);
        assert_eq!(calls[0].status, ArgumentsStatus::Object);
        assert_eq!(calls[1].call.function.name, "get_time");
    }

    #[test]
    fn lm_studio_style_fragments_with_empty_header_arguments() {
        let events = parse_chunks(&[
            json!({"choices":[{"index":0,"delta":{"role":"assistant","content":null}}]}),
            json!({"choices":[{"index":0,"delta":{"reasoning_content":"thinking"}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"814890118","type":"function","function":{"name":"read_file","arguments":""}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"notes.txt\"}"}}]}}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}),
        ]);
        assert_eq!(events[0], StreamEvent::Reasoning("thinking".into()));
        assert_eq!(
            events[1],
            StreamEvent::ToolCallStarted {
                index: 0,
                id: "814890118".into(),
                name: "read_file".into()
            }
        );
        let calls = accumulate(&events);
        assert_eq!(calls[0].call.function.arguments, r#"{"path":"notes.txt"}"#);
        assert_eq!(calls[0].status, ArgumentsStatus::Object);
    }

    #[test]
    fn whole_object_resent_replaces() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":"{\"x\":1}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"x\":2}"}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 1);
        // Replaced (not appended into `{"x":1}{"x":2}`, and not the stale first copy).
        assert_eq!(calls[0].call.function.arguments, r#"{"x":2}"#);
        assert_eq!(calls[0].status, ArgumentsStatus::Object);
    }

    #[test]
    fn nested_object_fragment_appends() {
        // A fragment that is a complete object but arrives mid-arguments must be appended.
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":"{\"opts\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"deep\":true}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(
            calls[0].call.function.arguments,
            r#"{"opts":{"deep":true}}"#
        );
    }

    #[test]
    fn missing_and_duplicate_ids_are_synthesised() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"a","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"dup","function":{"name":"b","arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":2,"id":"dup","function":{"name":"c","arguments":"{}"}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 3);
        assert!(calls[0].call.id.starts_with("call_"));
        assert_eq!(calls[1].call.id, "dup");
        assert!(calls[2].call.id.starts_with("call_"));
        assert_ne!(calls[0].call.id, calls[2].call.id);
    }

    #[test]
    fn calls_without_index_are_separated_by_id() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"id":"x1","function":{"name":"a","arguments":"{\"n\":1}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"id":"x2","function":{"name":"b","arguments":"{\"n\":2}"}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[1].call.function.name, "b");
    }

    #[test]
    fn whole_calls_in_one_chunk_without_index_or_id_stay_separate() {
        // Two complete calls in one `tool_calls` array, with neither `index` nor `id`: they
        // must not be folded into one call (the second's arguments under the first's name).
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[
                {"function":{"name":"read_file","arguments":"{\"path\":\"a.txt\"}"}},
                {"function":{"name":"delete_file","arguments":"{\"path\":\"b.txt\"}"}}
            ]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 2, "{events:?}");
        assert_eq!(calls[0].call.function.name, "read_file");
        assert_eq!(calls[0].call.function.arguments, r#"{"path":"a.txt"}"#);
        assert_eq!(calls[1].call.function.name, "delete_file");
        assert_eq!(calls[1].call.function.arguments, r#"{"path":"b.txt"}"#);
        assert_ne!(calls[0].call.id, calls[1].call.id);
    }

    #[test]
    fn fragments_without_index_or_id_continue_the_last_call() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"function":{"name":"f","arguments":"{\"a\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"1}"}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.function.arguments, r#"{"a":1}"#);
        assert_eq!(calls[0].status, ArgumentsStatus::Object);
    }

    #[test]
    fn arguments_before_name_are_held() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"q","function":{"arguments":"{\"a\""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"f","arguments":":1}"}}]}}]}),
        ]);
        assert_eq!(
            events[0],
            StreamEvent::ToolCallStarted {
                index: 0,
                id: "q".into(),
                name: "f".into()
            }
        );
        let calls = accumulate(&events);
        assert_eq!(calls[0].call.function.arguments, r#"{"a":1}"#);
    }

    #[test]
    fn nameless_call_is_flushed_at_finish() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{}"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].call.function.name, "");
    }

    #[test]
    fn length_marks_last_call_truncated() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":"{\"x\":1}"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"write_file","arguments":"{\"content\":\"lorem ip"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"length"}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls[0].status, ArgumentsStatus::Object);
        assert_eq!(calls[1].status, ArgumentsStatus::Truncated);
        assert!(calls[1].arguments_object().is_none());
    }

    #[test]
    fn invalid_and_non_object_arguments() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":"{\"x\":"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":1,"id":"b","function":{"name":"g","arguments":"\"{\\\"x\\\":1}\""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":2,"id":"c","function":{"name":"h"}}]}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ]);
        let calls = accumulate(&events);
        assert!(
            matches!(&calls[0].status, ArgumentsStatus::NotObject { reason } if reason.contains("not valid JSON"))
        );
        assert!(
            matches!(&calls[1].status, ArgumentsStatus::NotObject { reason } if reason.contains("string"))
        );
        assert_eq!(calls[2].status, ArgumentsStatus::Object);
        assert_eq!(calls[2].call.function.arguments, "{}");
        assert_eq!(calls[2].arguments_object(), Some(Map::new()));
    }

    #[test]
    fn object_arguments_are_accepted() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"f","arguments":{"x":1}}}]}}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls[0].call.function.arguments, r#"{"x":1}"#);
    }

    #[test]
    fn legacy_function_call() {
        let events = parse_chunks(&[
            json!({"choices":[{"delta":{"function_call":{"name":"f","arguments":"{}"}}}]}),
            json!({"choices":[{"delta":{},"finish_reason":"function_call"}]}),
        ]);
        let calls = accumulate(&events);
        assert_eq!(calls[0].call.function.name, "f");
        assert!(events.contains(&StreamEvent::Finished {
            finish_reason: FinishReason::ToolCalls
        }));
    }

    #[test]
    fn mid_stream_error_is_terminal() {
        let mut parser = ChunkParser::new();
        let mut out = Vec::new();
        parser
            .push_chunk(&json!({"choices":[{"delta":{"content":"Hi"}}]}), &mut out)
            .unwrap();
        let err = parser
            .push_chunk(
                &json!({"error":{"code":500,"message":"The model produced output that does not match the expected peg-native format","type":"server_error"}}),
                &mut out,
            )
            .unwrap_err();
        assert!(matches!(err, ProviderError::ServerParseError(_)));
        assert_eq!(out, vec![StreamEvent::Content("Hi".into())]);
        assert!(parser.is_done());
        // Nothing after the error.
        parser
            .push_chunk(&json!({"choices":[{"delta":{"content":"x"}}]}), &mut out)
            .unwrap();
        parser.finish(&mut out);
        assert_eq!(out.len(), 1);
    }

    #[test]
    fn vllm_top_level_error_object() {
        let mut parser = ChunkParser::new();
        let err = parser
            .push_chunk(
                &json!({"object":"error","message":"boom","type":"BadRequestError","code":400}),
                &mut Vec::new(),
            )
            .unwrap_err();
        assert_eq!(
            err,
            ProviderError::Http {
                status: 400,
                message: "boom".into()
            }
        );
    }

    #[test]
    fn finished_is_sent_once_and_other_choices_ignored() {
        let events = parse_chunks(&[
            json!({"choices":[{"index":1,"delta":{"content":"other"}}]}),
            json!({"choices":[{"index":0,"delta":{"content":"a"},"finish_reason":"stop"}]}),
            json!({"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}),
        ]);
        assert_eq!(
            events,
            vec![
                StreamEvent::Content("a".into()),
                StreamEvent::Finished {
                    finish_reason: FinishReason::Stop
                },
                StreamEvent::Done
            ]
        );
    }

    #[test]
    fn non_streaming_completion_is_read_like_a_delta() {
        let events = parse_chunks(&[json!({
            "choices":[{"index":0,"finish_reason":"tool_calls","message":{
                "role":"assistant","content":"ok","reasoning_content":"why",
                "tool_calls":[{"id":"t","type":"function","function":{"name":"f","arguments":"{}"}}]
            }}],
            "usage":{"prompt_tokens":3,"completion_tokens":4}
        })]);
        assert_eq!(events[0], StreamEvent::Reasoning("why".into()));
        assert_eq!(events[1], StreamEvent::Content("ok".into()));
        assert!(matches!(&events[2], StreamEvent::ToolCallStarted { name, .. } if name == "f"));
        assert!(events.contains(&StreamEvent::Usage(Usage {
            prompt_tokens: 3,
            completion_tokens: 4,
            cached_tokens: None
        })));
    }

    #[test]
    fn timings_context_used() {
        let timings = Timings {
            prompt_n: Some(13),
            cache_n: Some(1),
            predicted_n: Some(120),
            ..Timings::default()
        };
        assert_eq!(timings.context_used(), Some(134));
        assert_eq!(Timings::default().context_used(), None);
    }

    #[test]
    fn non_json_data_is_a_decode_error() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let err = decoder.push(b"data: not json\n\n", &mut out).unwrap_err();
        assert!(matches!(err, ProviderError::Decode(_)));
        assert!(decoder.is_done());
    }

    #[test]
    fn vendor_events_are_ignored() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder
            .push(b"event: ping\ndata: {}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n", &mut out)
            .unwrap();
        decoder.finish(&mut out).unwrap();
        // No finish reason and no [DONE]: the reply may be cut short.
        assert_eq!(
            out,
            vec![
                StreamEvent::Content("a".into()),
                StreamEvent::Unfinished,
                StreamEvent::Done
            ]
        );
    }

    #[test]
    fn event_error_type_is_terminal() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let err = decoder
            .push(
                b"event: error\ndata: {\"message\":\"overloaded\",\"code\":529}\n\n",
                &mut out,
            )
            .unwrap_err();
        assert_eq!(
            err,
            ProviderError::Http {
                status: 529,
                message: "overloaded".into()
            }
        );
    }

    #[test]
    fn empty_event_type_is_a_message() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder
            .push(
                b"event:\ndata: {\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n\n",
                &mut out,
            )
            .unwrap();
        decoder.finish(&mut out).unwrap();
        // No finish reason and no [DONE]: the reply may be cut short.
        assert_eq!(
            out,
            vec![
                StreamEvent::Content("a".into()),
                StreamEvent::Unfinished,
                StreamEvent::Done
            ]
        );
    }

    #[test]
    fn error_event_with_plain_text_reports_the_text() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        let err = decoder
            .push(b"event: error\ndata: upstream overloaded\n\n", &mut out)
            .unwrap_err();
        assert_eq!(
            err,
            ProviderError::Http {
                status: 500,
                message: "upstream overloaded".into()
            }
        );
        assert!(decoder.is_done());
        let mut decoder = StreamDecoder::new();
        let err = decoder
            .push(b"event: error\ndata: The model produced output that does not match the expected peg-native format\n\n", &mut out)
            .unwrap_err();
        assert!(matches!(err, ProviderError::ServerParseError(_)), "{err:?}");
    }

    #[test]
    fn a_body_without_any_event_is_a_decode_error() {
        // An HTML page served with status 200 by a wrong base URL, an empty body, or only
        // keep-alive comments: none of them is a chat stream, and none may look like an
        // empty but successful reply.
        for body in [
            &b"<!doctype html><html><body>Open WebUI</body></html>"[..],
            b"",
            b":\n\n: ping\n\n",
        ] {
            let mut decoder = StreamDecoder::new();
            let mut out = Vec::new();
            decoder.push(body, &mut out).unwrap();
            let err = decoder.finish(&mut out).unwrap_err();
            assert!(matches!(err, ProviderError::Decode(_)), "{err:?}");
            assert!(out.is_empty(), "{out:?}");
            assert!(decoder.is_done());
            // Finishing again is a no-op, not a second error.
            decoder.finish(&mut out).unwrap();
            assert!(out.is_empty());
        }
    }

    #[test]
    fn done_alone_is_a_valid_empty_stream() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder.push(b"data: [DONE]\n\n", &mut out).unwrap();
        decoder.finish(&mut out).unwrap();
        assert_eq!(out, vec![StreamEvent::Done]);
    }

    #[test]
    fn stream_without_done_still_ends_with_done() {
        let mut decoder = StreamDecoder::new();
        let mut out = Vec::new();
        decoder
            .push(
                b"data: {\"choices\":[{\"delta\":{\"content\":\"a\"},\"finish_reason\":\"stop\"}]}",
                &mut out,
            )
            .unwrap();
        decoder.finish(&mut out).unwrap();
        assert_eq!(out.last(), Some(&StreamEvent::Done));
        assert_eq!(out[0], StreamEvent::Content("a".into()));
    }
}
