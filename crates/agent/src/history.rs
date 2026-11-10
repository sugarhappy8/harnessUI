//! The agent's message model and the history invariant (ARCHITECTURE.md 8.2).
//!
//! The app stores a chat as a list of [`HistoryMessage`]s. Before every request, and when a
//! chat is opened, [`normalize`] brings stored history into a shape every chat template
//! accepts:
//!
//! - every assistant tool call has exactly one result; where none exists a synthetic
//!   `interrupted` result is added (the loop itself writes `denied`, `cancelled` and `not run`
//!   results as it goes);
//! - tool-call arguments are always a JSON object (the type guarantees it; a cut-off call is
//!   stored with an empty object by the loop);
//! - no assistant message is empty: an empty one becomes `(stopped)`;
//! - a user message never directly follows a user or tool message: a `(stopped)` assistant
//!   message is put between them, so a turn that ended without an answer is closed;
//! - a call recovered from text is stored, and replayed, as a native call followed by a
//!   tool-role result (the [`CallRecord::recovered`] flag only labels it).
//!
//! A history that *ends* with tool results is left as it is: that is the state in the middle
//! of a turn, just before the model answers.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::types::Role;

/// The text of the assistant message that closes a turn which ended without an answer.
pub const STOPPED_TEXT: &str = "(stopped)";

/// The name given to a stored call whose name the server never sent.
pub const UNKNOWN_TOOL_NAME: &str = "unknown_tool";

/// What happened to a tool call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CallStatus {
    /// The tool ran and succeeded.
    Ok,
    /// The tool ran and reported an error, or the call was invalid (bad arguments, unknown
    /// tool, cut off); the model reads the error.
    Error,
    /// The user denied the call, optionally with a note for the model.
    Denied {
        /// The user's note, sent back to the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The user stopped the turn while the call was pending or running.
    Cancelled,
    /// The call was not run: an earlier call in the same response was denied or cancelled,
    /// or the turn reached a limit.
    NotRun,
    /// No result was recorded (the app quit or crashed mid-call); added by [`normalize`].
    Interrupted,
    /// Refused without asking: the tool is unavailable, a hard limit applies, or the same
    /// call was denied earlier in the turn.
    Refused,
}

impl CallStatus {
    /// A short lowercase label (`ok`, `error`, `denied`, ...) for prompts and logs.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Denied { .. } => "denied",
            Self::Cancelled => "cancelled",
            Self::NotRun => "not run",
            Self::Interrupted => "interrupted",
            Self::Refused => "refused",
        }
    }
}

/// One tool call made by the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRecord {
    /// The call id the result answers.
    pub id: String,
    /// The tool name.
    pub name: String,
    /// The arguments, always a JSON object (a cut-off or unreadable call is stored with `{}`).
    pub arguments: Map<String, Value>,
    /// The call was recovered from the model's text rather than sent as a native tool call
    /// (ARCHITECTURE.md 8.3). It is replayed as a native call either way.
    #[serde(default)]
    pub recovered: bool,
}

/// The result of one tool call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResultRecord {
    /// The id of the call this answers.
    pub call_id: String,
    /// The tool name.
    pub name: String,
    /// What happened.
    pub status: CallStatus,
    /// What the tool returned for the model (untrusted data; sanitised and wrapped in a
    /// delimiter when it enters a prompt). Empty for calls that did not run.
    pub text: String,
    /// Images the tool returned, as `(mime, base64)` pairs.
    #[serde(default)]
    pub images: Vec<(String, String)>,
    /// The path or URL the result came from, shown in the delimiter header.
    #[serde(default)]
    pub source: Option<String>,
    /// App notes for the model (a validation error, the loop warning, why a call was not run,
    /// the wrap-up instruction). Trusted text from the app, placed after the delimiter.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl ResultRecord {
    /// A result with no tool output, only a status and notes (a call that did not run).
    pub fn synthetic(call: &CallRecord, status: CallStatus, notes: Vec<String>) -> Self {
        Self {
            call_id: call.id.clone(),
            name: call.name.clone(),
            status,
            text: String::new(),
            images: Vec::new(),
            source: None,
            notes,
        }
    }
}

/// One part of a message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Part {
    /// Text written by the user or the model.
    Text(String),
    /// An inline image.
    Image {
        /// MIME type such as `image/png`.
        mime: String,
        /// The image bytes, base64-encoded (standard alphabet).
        data_base64: String,
    },
    /// Tool calls made by an assistant message.
    ToolCalls(Vec<CallRecord>),
    /// The result of one tool call (in a tool-role message).
    ToolResult(ResultRecord),
    /// Text extracted from an attached file (PDF, text). Untrusted: it enters the prompt with
    /// the same delimiter and sanitiser as tool results (ARCHITECTURE.md 8.6).
    Attachment {
        /// The file name shown in the delimiter header.
        name: String,
        /// The extracted text.
        text: String,
    },
}

/// One message of a chat, as the app stores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryMessage {
    /// Who wrote it. System messages are never stored: the one system message is assembled
    /// for each request (ARCHITECTURE.md 8.5), and [`normalize`] drops any it finds.
    pub role: Role,
    /// The content.
    pub parts: Vec<Part>,
    /// The model's reasoning, shown in the Thinking block.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// The database id, when stored.
    #[serde(default)]
    pub id: Option<i64>,
}

impl HistoryMessage {
    /// A message with these parts and no reasoning or id.
    pub fn new(role: Role, parts: Vec<Part>) -> Self {
        Self {
            role,
            parts,
            reasoning: None,
            id: None,
        }
    }

    /// A user message with text.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(Role::User, vec![Part::Text(text.into())])
    }

    /// An assistant message with text.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(Role::Assistant, vec![Part::Text(text.into())])
    }

    /// An assistant message with optional text before its calls. Empty text is left out.
    pub fn assistant_calls(text: impl Into<String>, calls: Vec<CallRecord>) -> Self {
        let text = text.into();
        let mut parts = Vec::new();
        if !text.trim().is_empty() {
            parts.push(Part::Text(text));
        }
        if !calls.is_empty() {
            parts.push(Part::ToolCalls(calls));
        }
        Self::new(Role::Assistant, parts)
    }

    /// A tool-role message carrying one result.
    pub fn tool_result(result: ResultRecord) -> Self {
        Self::new(Role::Tool, vec![Part::ToolResult(result)])
    }

    /// The `(stopped)` assistant message.
    pub fn stopped() -> Self {
        Self::assistant(STOPPED_TEXT)
    }

    /// The text parts joined with blank lines (attachments and images left out).
    pub fn text(&self) -> String {
        self.parts
            .iter()
            .filter_map(|part| match part {
                Part::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The tool calls of an assistant message, in order.
    pub fn tool_calls(&self) -> impl Iterator<Item = &CallRecord> {
        self.parts.iter().flat_map(|part| match part {
            Part::ToolCalls(calls) => calls.as_slice(),
            _ => &[],
        })
    }

    /// The results carried by a tool message, in order.
    pub fn tool_results(&self) -> impl Iterator<Item = &ResultRecord> {
        self.parts.iter().filter_map(|part| match part {
            Part::ToolResult(result) => Some(result),
            _ => None,
        })
    }

    /// True when the message has tool calls.
    pub fn has_tool_calls(&self) -> bool {
        self.tool_calls().next().is_some()
    }

    /// True when there is nothing a template could render as content: no non-blank text, no
    /// image, no attachment and no tool call or result.
    pub fn is_blank(&self) -> bool {
        self.parts.iter().all(|part| match part {
            Part::Text(text) => text.trim().is_empty(),
            Part::ToolCalls(calls) => calls.is_empty(),
            Part::Image { .. } | Part::ToolResult(_) | Part::Attachment { .. } => false,
        })
    }
}

/// Applies the history invariant (module documentation). Idempotent: normalising twice gives
/// the same result as once.
pub fn normalize(msgs: Vec<HistoryMessage>) -> Vec<HistoryMessage> {
    let mut out: Vec<HistoryMessage> = Vec::with_capacity(msgs.len() + 2);
    let mut iter = msgs.into_iter().peekable();
    while let Some(message) = iter.next() {
        match message.role {
            // Only the request builder writes the system message; a tool message here was
            // not consumed by the assistant message before it, so it answers nothing.
            Role::System | Role::Tool => {}
            Role::User => {
                let Some(message) = clean_user(message) else {
                    continue;
                };
                if matches!(out.last().map(|m| m.role), Some(Role::User | Role::Tool)) {
                    out.push(HistoryMessage::stopped());
                }
                out.push(message);
            }
            Role::Assistant => {
                let mut results = Vec::new();
                while iter.peek().is_some_and(|next| next.role == Role::Tool) {
                    if let Some(tool_message) = iter.next() {
                        results.extend(tool_message.parts.into_iter().filter_map(
                            |part| match part {
                                Part::ToolResult(result) => Some(result),
                                _ => None,
                            },
                        ));
                    }
                }
                push_assistant(&mut out, message, results);
            }
        }
    }
    out
}

/// Keeps the parts a user message may carry; `None` when nothing is left.
fn clean_user(mut message: HistoryMessage) -> Option<HistoryMessage> {
    message.parts.retain(|part| {
        matches!(
            part,
            Part::Text(_) | Part::Image { .. } | Part::Attachment { .. }
        )
    });
    message.reasoning = None;
    if message.is_blank() {
        return None;
    }
    Some(message)
}

/// Appends an assistant message and exactly one result per call, in call order.
fn push_assistant(
    out: &mut Vec<HistoryMessage>,
    mut message: HistoryMessage,
    mut results: Vec<ResultRecord>,
) {
    // Merge the calls into one part placed after the content, with names and unique ids.
    let mut calls: Vec<CallRecord> = Vec::new();
    let mut content: Vec<Part> = Vec::new();
    for part in std::mem::take(&mut message.parts) {
        match part {
            Part::ToolCalls(list) => calls.extend(list),
            Part::Text(_) | Part::Image { .. } => content.push(part),
            // Results and attachments do not belong in an assistant message.
            Part::ToolResult(_) | Part::Attachment { .. } => {}
        }
    }
    let mut seen = HashSet::new();
    for call in &mut calls {
        if call.name.trim().is_empty() {
            call.name = UNKNOWN_TOOL_NAME.to_owned();
        }
        if call.id.is_empty() || !seen.insert(call.id.clone()) {
            // Deterministic, so the same stored history renders the same way on every request
            // (a random id would invalidate the server's cached prefix each time).
            let base = if call.id.is_empty() {
                "call"
            } else {
                call.id.as_str()
            };
            let mut n = 1;
            let mut id = format!("{base}_{n}");
            while seen.contains(&id) {
                n += 1;
                id = format!("{base}_{n}");
            }
            call.id = id;
            seen.insert(call.id.clone());
        }
    }
    message.parts = content;
    if calls.is_empty() {
        if message.is_blank() {
            message.parts = vec![Part::Text(STOPPED_TEXT.to_owned())];
        }
        // An assistant directly after an assistant without calls: merge, because several
        // templates require strictly alternating roles.
        if let Some(previous) = out.last_mut()
            && previous.role == Role::Assistant
            && !previous.has_tool_calls()
        {
            merge_assistant(previous, message);
            return;
        }
        out.push(message);
        return;
    }
    if out
        .last()
        .is_some_and(|previous| previous.role == Role::Assistant && !previous.has_tool_calls())
        && let Some(previous) = out.pop()
    {
        // Two assistant messages in a row: fold the earlier one's content in front of this
        // one (a `(stopped)` filler is simply dropped), keeping roles alternating.
        let is_filler = previous.text() == STOPPED_TEXT && previous.parts.len() == 1;
        if !is_filler {
            let mut parts = previous.parts;
            parts.append(&mut message.parts);
            message.parts = parts;
            message.reasoning = match (previous.reasoning, message.reasoning.take()) {
                (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
                (a, b) => a.or(b),
            };
            message.id = message.id.or(previous.id);
        }
    }
    message.parts.push(Part::ToolCalls(calls.clone()));
    out.push(message);
    for call in &calls {
        let result = match results.iter().position(|r| r.call_id == call.id) {
            Some(index) => {
                let mut result = results.remove(index);
                if result.name.trim().is_empty() || result.name != call.name {
                    result.name.clone_from(&call.name);
                }
                result
            }
            None => ResultRecord::synthetic(
                call,
                CallStatus::Interrupted,
                vec!["No result was recorded for this call.".to_owned()],
            ),
        };
        out.push(HistoryMessage::tool_result(result));
    }
}

fn merge_assistant(previous: &mut HistoryMessage, next: HistoryMessage) {
    let next_is_filler = next.text() == STOPPED_TEXT && next.parts.len() == 1;
    let previous_is_filler = previous.text() == STOPPED_TEXT && previous.parts.len() == 1;
    if next_is_filler {
        return;
    }
    if previous_is_filler {
        previous.parts.clear();
    }
    previous.parts.extend(next.parts);
    match (&mut previous.reasoning, next.reasoning) {
        (Some(existing), Some(more)) => {
            existing.push_str("\n\n");
            existing.push_str(&more);
        }
        (slot @ None, more) => *slot = more,
        (Some(_), None) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn call(id: &str, name: &str) -> CallRecord {
        CallRecord {
            id: id.into(),
            name: name.into(),
            arguments: Map::new(),
            recovered: false,
        }
    }

    fn ok_result(id: &str, name: &str, text: &str) -> ResultRecord {
        ResultRecord {
            call_id: id.into(),
            name: name.into(),
            status: CallStatus::Ok,
            text: text.into(),
            images: vec![],
            source: None,
            notes: vec![],
        }
    }

    fn roles(msgs: &[HistoryMessage]) -> Vec<Role> {
        msgs.iter().map(|m| m.role).collect()
    }

    #[test]
    fn missing_results_get_interrupted_and_results_follow_call_order() {
        let msgs = vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant_calls(
                "",
                vec![call("a", "read_file"), call("b", "list_dir")],
            ),
            HistoryMessage::tool_result(ok_result("b", "list_dir", "x")),
        ];
        let out = normalize(msgs);
        assert_eq!(
            roles(&out),
            vec![Role::User, Role::Assistant, Role::Tool, Role::Tool]
        );
        let first = out[2].tool_results().next().unwrap();
        assert_eq!(first.call_id, "a");
        assert_eq!(first.status, CallStatus::Interrupted);
        let second = out[3].tool_results().next().unwrap();
        assert_eq!(second.call_id, "b");
        assert_eq!(second.status, CallStatus::Ok);
    }

    #[test]
    fn duplicate_and_orphan_results_are_dropped() {
        let msgs = vec![
            HistoryMessage::user("go"),
            HistoryMessage::tool_result(ok_result("zz", "x", "orphan before any call")),
            HistoryMessage::assistant_calls("", vec![call("a", "read_file")]),
            HistoryMessage::tool_result(ok_result("a", "read_file", "first")),
            HistoryMessage::tool_result(ok_result("a", "read_file", "second")),
            HistoryMessage::tool_result(ok_result("nope", "read_file", "orphan")),
            HistoryMessage::assistant("done"),
        ];
        let out = normalize(msgs);
        assert_eq!(
            roles(&out),
            vec![Role::User, Role::Assistant, Role::Tool, Role::Assistant]
        );
        assert_eq!(out[2].tool_results().next().unwrap().text, "first");
    }

    #[test]
    fn user_never_follows_user_or_tool() {
        let msgs = vec![
            HistoryMessage::user("one"),
            HistoryMessage::user("two"),
            HistoryMessage::assistant_calls("", vec![call("a", "f")]),
            HistoryMessage::tool_result(ok_result("a", "f", "r")),
            HistoryMessage::user("three"),
        ];
        let out = normalize(msgs);
        assert_eq!(
            roles(&out),
            vec![
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Assistant,
                Role::User
            ]
        );
        assert_eq!(out[1].text(), STOPPED_TEXT);
        assert_eq!(out[5].text(), STOPPED_TEXT);
    }

    #[test]
    fn trailing_tool_results_are_left_for_the_model() {
        let msgs = vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant_calls("", vec![call("a", "f")]),
            HistoryMessage::tool_result(ok_result("a", "f", "r")),
        ];
        let out = normalize(msgs.clone());
        assert_eq!(out, msgs);
    }

    #[test]
    fn empty_assistant_becomes_stopped_and_consecutive_assistants_merge() {
        let mut empty = HistoryMessage::assistant("   ");
        empty.reasoning = Some("thinking only".into());
        let msgs = vec![
            HistoryMessage::user("go"),
            empty,
            HistoryMessage::assistant("an answer"),
        ];
        let out = normalize(msgs);
        assert_eq!(roles(&out), vec![Role::User, Role::Assistant]);
        assert_eq!(out[1].text(), "an answer");
        assert_eq!(out[1].reasoning.as_deref(), Some("thinking only"));

        let out = normalize(vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant(""),
        ]);
        assert_eq!(out[1].text(), STOPPED_TEXT);
    }

    #[test]
    fn system_messages_and_foreign_parts_are_dropped() {
        let mut user = HistoryMessage::user("hi");
        user.parts.push(Part::ToolCalls(vec![call("x", "f")]));
        let msgs = vec![
            HistoryMessage::new(Role::System, vec![Part::Text("old system".into())]),
            user,
            HistoryMessage::new(Role::User, vec![Part::Text("  ".into())]),
        ];
        let out = normalize(msgs);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].parts, vec![Part::Text("hi".into())]);
    }

    #[test]
    fn calls_get_names_and_unique_ids() {
        let msgs = vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant_calls("", vec![call("a", ""), call("a", "g")]),
            HistoryMessage::tool_result(ok_result("a", "", "r")),
        ];
        let out = normalize(msgs);
        let calls: Vec<_> = out[1].tool_calls().cloned().collect();
        assert_eq!(calls[0].name, UNKNOWN_TOOL_NAME);
        assert_eq!(calls[0].id, "a");
        assert_eq!(calls[1].id, "a_1", "renamed deterministically");
        assert_eq!(
            out[2].tool_results().next().unwrap().name,
            UNKNOWN_TOOL_NAME
        );
        assert_eq!(
            out[3].tool_results().next().unwrap().status,
            CallStatus::Interrupted
        );
    }

    #[test]
    fn recovered_calls_are_kept_as_native_calls() {
        let mut recovered = call("r1", "write_file");
        recovered.recovered = true;
        recovered.arguments = json!({"path": "a"}).as_object().cloned().unwrap();
        let msgs = vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant_calls("Writing it.", vec![recovered.clone()]),
            HistoryMessage::tool_result(ok_result("r1", "write_file", "done")),
            HistoryMessage::assistant("ok"),
        ];
        let out = normalize(msgs);
        assert_eq!(out[1].tool_calls().next(), Some(&recovered));
        assert_eq!(out[1].text(), "Writing it.");
    }

    #[test]
    fn stopped_filler_before_calls_is_removed_and_normalize_is_idempotent() {
        let msgs = vec![
            HistoryMessage::user("a"),
            HistoryMessage::stopped(),
            HistoryMessage::assistant_calls("", vec![call("c", "f")]),
            HistoryMessage::user("b"),
        ];
        let once = normalize(msgs);
        assert_eq!(
            roles(&once),
            vec![
                Role::User,
                Role::Assistant,
                Role::Tool,
                Role::Assistant,
                Role::User
            ]
        );
        assert_eq!(normalize(once.clone()), once);
    }

    #[test]
    fn serialises_for_storage() {
        let message = HistoryMessage::tool_result(ResultRecord {
            call_id: "c".into(),
            name: "f".into(),
            status: CallStatus::Denied {
                note: Some("not that one".into()),
            },
            text: String::new(),
            images: vec![("image/png".into(), "AAAA".into())],
            source: Some("/p/a".into()),
            notes: vec!["note".into()],
        });
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["parts"][0]["toolResult"]["status"]["kind"], "denied");
        assert_eq!(
            value["parts"][0]["toolResult"]["status"]["note"],
            "not that one"
        );
        let back: HistoryMessage = serde_json::from_value(value).unwrap();
        assert_eq!(back, message);
        let text: Part = serde_json::from_value(json!({"text": "hi"})).unwrap();
        assert_eq!(text, Part::Text("hi".into()));
    }
}
