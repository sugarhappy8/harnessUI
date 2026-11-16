//! Claude's event stream (Anthropic's streaming documentation) read into the same
//! [`StreamEvent`]s a chat completion gives:
//!
//! - `message_start` carries the prompt's token counts; `message_delta` the stop reason and
//!   the final (cumulative) counts; `message_stop` ends the stream;
//! - `content_block_start` / `content_block_delta` / `content_block_stop` for each block:
//!   `text_delta` → [`StreamEvent::Content`], `thinking_delta` → [`StreamEvent::Reasoning`],
//!   `signature_delta` completes a thinking block, `input_json_delta` → argument fragments of
//!   the `tool_use` block at that index;
//! - `ping` and event types the app does not know are ignored, as the documentation asks;
//! - `error` ends the stream with that error; a `refusal` stop ends it with
//!   [`ProviderError::Refused`], so nothing from the refused reply is used.
//!
//! A response that called tools keeps its blocks (thinking with signatures, text, tool calls)
//! in the [`ThinkingReplay`], so the next step of the loop sends them back unchanged.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};

use super::{ThinkingReplay, error_from};
use crate::error::{ProviderError, as_u32, bounded};
use crate::sse::{SseDecoder, SseEvent};
use crate::stream::{FinishReason, StreamEvent, Usage};

/// One content block as it streams.
#[derive(Debug)]
enum Block {
    Text(String),
    Thinking {
        thinking: String,
        signature: String,
    },
    /// Arrives whole; sent back exactly as received.
    Redacted(Value),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
    /// A block type the app does not use (server tools); a response with one is not kept.
    Other,
}

/// Token counts as the stream reports them; `message_delta`'s are cumulative.
#[derive(Debug, Default, Clone, Copy)]
struct Counts {
    input: Option<u32>,
    cache_creation: Option<u32>,
    cache_read: Option<u32>,
    output: Option<u32>,
}

impl Counts {
    fn read(&mut self, usage: &Value) {
        let count = |key: &str| usage.get(key).and_then(as_u32);
        self.input = count("input_tokens").or(self.input);
        self.cache_creation = count("cache_creation_input_tokens").or(self.cache_creation);
        self.cache_read = count("cache_read_input_tokens").or(self.cache_read);
        self.output = count("output_tokens").or(self.output);
    }

    fn known(&self) -> bool {
        self.input.is_some() || self.output.is_some()
    }

    /// The whole prompt is the uncached part plus what was written to and read from the
    /// cache (Anthropic's usage documentation).
    fn usage(&self) -> Usage {
        let prompt = self
            .input
            .unwrap_or(0)
            .saturating_add(self.cache_creation.unwrap_or(0))
            .saturating_add(self.cache_read.unwrap_or(0));
        Usage {
            prompt_tokens: prompt,
            completion_tokens: self.output.unwrap_or(0),
            cached_tokens: self.cache_read,
        }
    }
}

/// Maps a stop reason to the app's finish reasons. `model_context_window_exceeded` stops
/// like the output limit.
fn finish_reason(stop: &str) -> FinishReason {
    match stop {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" | "model_context_window_exceeded" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        other => FinishReason::Other(other.to_owned()),
    }
}

/// Bytes in, stream events out, for one Messages API response.
#[derive(Debug)]
pub(crate) struct MessagesDecoder {
    sse: SseDecoder,
    replay: Option<Arc<Mutex<ThinkingReplay>>>,
    blocks: BTreeMap<usize, Block>,
    counts: Counts,
    stop_reason: Option<String>,
    finished_sent: bool,
    saw_event: bool,
    done: bool,
}

impl MessagesDecoder {
    /// A decoder that keeps a response that called tools in `replay`.
    pub(crate) fn new(replay: Option<Arc<Mutex<ThinkingReplay>>>) -> Self {
        Self {
            sse: SseDecoder::new(),
            replay,
            blocks: BTreeMap::new(),
            counts: Counts::default(),
            stop_reason: None,
            finished_sent: false,
            saw_event: false,
            done: false,
        }
    }

    /// True once the stream ended (`message_stop`, an error, or [`MessagesDecoder::finish`]).
    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// Feeds bytes. Events produced before an error stay in `out`.
    ///
    /// # Errors
    /// An `error` event, a refusal, data that is not JSON, or an event over the size limit.
    pub(crate) fn push(
        &mut self,
        bytes: &[u8],
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        if self.done {
            return Ok(());
        }
        let events = self.sse.push(bytes);
        self.saw_event |= !events.is_empty();
        self.check_overflow()?;
        for event in &events {
            self.on_event(event, out)?;
            if self.done {
                break;
            }
        }
        Ok(())
    }

    /// Ends the input. A stream that ends without `message_stop` was cut short: its events
    /// end with [`StreamEvent::Unfinished`] (when no stop reason came) and
    /// [`StreamEvent::Done`], and nothing is kept for replay.
    ///
    /// # Errors
    /// As for [`MessagesDecoder::push`]; [`ProviderError::Decode`] when the body held no
    /// event at all (not a Messages API stream: a wrong address answering with a page).
    pub(crate) fn finish(&mut self, out: &mut Vec<StreamEvent>) -> Result<(), ProviderError> {
        if self.done {
            return Ok(());
        }
        let events = self.sse.finish();
        self.saw_event |= !events.is_empty();
        self.check_overflow()?;
        for event in &events {
            self.on_event(event, out)?;
            if self.done {
                return Ok(());
            }
        }
        self.done = true;
        if !self.saw_event {
            return Err(ProviderError::Decode(
                "the response held no server-sent events, so it is not a Messages API stream \
                 (check the base URL)"
                    .to_owned(),
            ));
        }
        if !self.finished_sent {
            out.push(StreamEvent::Unfinished);
        }
        if self.counts.known() {
            out.push(StreamEvent::Usage(self.counts.usage()));
        }
        out.push(StreamEvent::Done);
        Ok(())
    }

    fn check_overflow(&mut self) -> Result<(), ProviderError> {
        if self.sse.overflowed() {
            self.done = true;
            return Err(ProviderError::Decode(format!(
                "the server sent an event larger than {} MiB",
                SseDecoder::DEFAULT_MAX_BYTES / (1024 * 1024)
            )));
        }
        Ok(())
    }

    fn fail(&mut self, err: ProviderError) -> Result<(), ProviderError> {
        self.done = true;
        Err(err)
    }

    fn on_event(
        &mut self,
        event: &SseEvent,
        out: &mut Vec<StreamEvent>,
    ) -> Result<(), ProviderError> {
        let named_error = event.event.as_deref() == Some("error");
        let data = event.data.trim();
        if data.is_empty() {
            if named_error {
                return self.fail(ProviderError::Http {
                    status: 500,
                    message: "the server reported an error".to_owned(),
                });
            }
            return Ok(());
        }
        let value: Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(_) if named_error => {
                return self.fail(ProviderError::Http {
                    status: 500,
                    message: bounded(data),
                });
            }
            Err(err) => {
                return self.fail(ProviderError::Decode(format!(
                    "a stream event is not JSON ({err})"
                )));
            }
        };
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .or(event.event.as_deref())
            .unwrap_or_default()
            .to_owned();
        match kind.as_str() {
            "message_start" => {
                if let Some(usage) = value.pointer("/message/usage") {
                    self.counts.read(usage);
                }
            }
            "content_block_start" => self.on_block_start(&value, out),
            "content_block_delta" => self.on_block_delta(&value, out),
            "content_block_stop" => {}
            "message_delta" => {
                if let Some(usage) = value.get("usage") {
                    self.counts.read(usage);
                }
                if let Some(stop) = value.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    if stop == "refusal" {
                        let explanation = value
                            .pointer("/delta/stop_details/explanation")
                            .and_then(Value::as_str)
                            .map(bounded)
                            .filter(|text| !text.is_empty());
                        return self.fail(ProviderError::Refused { explanation });
                    }
                    self.stop_reason = Some(stop.to_owned());
                    if !self.finished_sent {
                        self.finished_sent = true;
                        out.push(StreamEvent::Finished {
                            finish_reason: finish_reason(stop),
                        });
                    }
                }
            }
            "message_stop" => {
                self.keep_for_replay();
                if self.counts.known() {
                    out.push(StreamEvent::Usage(self.counts.usage()));
                }
                out.push(StreamEvent::Done);
                self.done = true;
            }
            "error" => {
                let err = error_from(&value, None);
                return self.fail(err);
            }
            // `ping`, and types added later.
            _ => {}
        }
        Ok(())
    }

    fn on_block_start(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
        let Some(index) = block_index(value) else {
            return;
        };
        let Some(block) = value.get("content_block") else {
            return;
        };
        let text_of = |key: &str| {
            block
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        let started = match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = text_of("text");
                if !text.is_empty() {
                    out.push(StreamEvent::Content(text.clone()));
                }
                Block::Text(text)
            }
            Some("thinking") => {
                let thinking = text_of("thinking");
                if !thinking.is_empty() {
                    out.push(StreamEvent::Reasoning(thinking.clone()));
                }
                Block::Thinking {
                    thinking,
                    signature: text_of("signature"),
                }
            }
            Some("redacted_thinking") => Block::Redacted(block.clone()),
            Some("tool_use") => {
                let id = text_of("id");
                let name = text_of("name");
                out.push(StreamEvent::ToolCallStarted {
                    index,
                    id: id.clone(),
                    name: name.clone(),
                });
                // Normally `{}` with the input in deltas; a proxy may send it whole.
                let input = match block.get("input") {
                    Some(Value::Object(map)) if !map.is_empty() => {
                        let text = Value::Object(map.clone()).to_string();
                        out.push(StreamEvent::ToolCallArgs {
                            index,
                            fragment: text.clone(),
                        });
                        text
                    }
                    _ => String::new(),
                };
                Block::ToolUse { id, name, input }
            }
            _ => Block::Other,
        };
        self.blocks.insert(index, started);
    }

    fn on_block_delta(&mut self, value: &Value, out: &mut Vec<StreamEvent>) {
        let Some(index) = block_index(value) else {
            return;
        };
        let Some(delta) = value.get("delta") else {
            return;
        };
        let text_of = |key: &str| delta.get(key).and_then(Value::as_str).unwrap_or_default();
        match (
            delta.get("type").and_then(Value::as_str),
            self.blocks.get_mut(&index),
        ) {
            (Some("text_delta"), block) => {
                let text = text_of("text");
                if text.is_empty() {
                    return;
                }
                out.push(StreamEvent::Content(text.to_owned()));
                match block {
                    Some(Block::Text(kept)) => kept.push_str(text),
                    None => {
                        self.blocks.insert(index, Block::Text(text.to_owned()));
                    }
                    Some(_) => {}
                }
            }
            (Some("thinking_delta"), block) => {
                let text = text_of("thinking");
                if !text.is_empty() {
                    out.push(StreamEvent::Reasoning(text.to_owned()));
                }
                if let Some(Block::Thinking { thinking, .. }) = block {
                    thinking.push_str(text);
                }
            }
            (Some("signature_delta"), Some(Block::Thinking { signature, .. })) => {
                signature.push_str(text_of("signature"));
            }
            (Some("input_json_delta"), Some(Block::ToolUse { input, .. })) => {
                let fragment = text_of("partial_json");
                if !fragment.is_empty() {
                    input.push_str(fragment);
                    out.push(StreamEvent::ToolCallArgs {
                        index,
                        fragment: fragment.to_owned(),
                    });
                }
            }
            // `citations_delta`, and types added later.
            _ => {}
        }
    }

    /// Keeps a finished response that called tools and thought, exactly as received, for the
    /// next request of the tool-use loop. Not kept: a reply cut off, one with a block the app
    /// cannot send back, one with arguments that are not a JSON object.
    fn keep_for_replay(&mut self) {
        let Some(replay) = &self.replay else {
            return;
        };
        if self.stop_reason.as_deref() != Some("tool_use") {
            return;
        }
        let mut blocks = Vec::new();
        let mut call_ids = Vec::new();
        let mut thought = false;
        for block in self.blocks.values() {
            match block {
                Block::Text(text) if text.is_empty() => {}
                Block::Text(text) => blocks.push(json!({"type": "text", "text": text})),
                Block::Thinking { signature, .. } if signature.is_empty() => return,
                Block::Thinking {
                    thinking,
                    signature,
                } => {
                    thought = true;
                    blocks.push(
                        json!({"type": "thinking", "thinking": thinking, "signature": signature}),
                    );
                }
                Block::Redacted(block) => {
                    thought = true;
                    blocks.push(block.clone());
                }
                Block::ToolUse { id, name, input } => {
                    let input = if input.trim().is_empty() {
                        Value::Object(Map::new())
                    } else {
                        match serde_json::from_str::<Value>(input) {
                            Ok(Value::Object(map)) => Value::Object(map),
                            _ => return,
                        }
                    };
                    call_ids.push(id.clone());
                    blocks
                        .push(json!({"type": "tool_use", "id": id, "name": name, "input": input}));
                }
                Block::Other => return,
            }
        }
        if thought && let Ok(mut replay) = replay.lock() {
            replay.store(call_ids, blocks);
        }
    }
}

fn block_index(value: &Value) -> Option<usize> {
    value
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
}

/// Events for a server that answered a streaming request with a whole Message object.
///
/// # Errors
/// An error object, a refusal, or JSON that is not a Message.
pub(crate) fn events_from_message(body: &[u8]) -> Result<Vec<StreamEvent>, ProviderError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|err| ProviderError::Decode(format!("the response is not JSON ({err})")))?;
    if value.get("type").and_then(Value::as_str) == Some("error") {
        return Err(error_from(&value, None));
    }
    let Some(content) = value.get("content").and_then(Value::as_array) else {
        return Err(ProviderError::Decode(
            "the JSON response is not a message (it has no 'content')".to_owned(),
        ));
    };
    let stop = value.get("stop_reason").and_then(Value::as_str);
    if stop == Some("refusal") {
        let explanation = value
            .pointer("/stop_details/explanation")
            .and_then(Value::as_str)
            .map(bounded)
            .filter(|text| !text.is_empty());
        return Err(ProviderError::Refused { explanation });
    }
    let mut events = Vec::new();
    for (index, block) in content.iter().enumerate() {
        let text_of = |key: &str| block.get(key).and_then(Value::as_str).unwrap_or_default();
        match block.get("type").and_then(Value::as_str) {
            Some("text") if !text_of("text").is_empty() => {
                events.push(StreamEvent::Content(text_of("text").to_owned()));
            }
            Some("thinking") if !text_of("thinking").is_empty() => {
                events.push(StreamEvent::Reasoning(text_of("thinking").to_owned()));
            }
            Some("tool_use") => {
                events.push(StreamEvent::ToolCallStarted {
                    index,
                    id: text_of("id").to_owned(),
                    name: text_of("name").to_owned(),
                });
                let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                events.push(StreamEvent::ToolCallArgs {
                    index,
                    fragment: input.to_string(),
                });
            }
            _ => {}
        }
    }
    if let Some(stop) = stop {
        events.push(StreamEvent::Finished {
            finish_reason: finish_reason(stop),
        });
    }
    if let Some(usage) = value.get("usage") {
        let mut counts = Counts::default();
        counts.read(usage);
        events.push(StreamEvent::Usage(counts.usage()));
    }
    events.push(StreamEvent::Done);
    Ok(events)
}
