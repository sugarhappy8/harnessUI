//! Subagents (ARCHITECTURE.md 8.2, 9.3, 13.3): the `run_subagent` tool. With the
//! "Subagents" group switched on for a chat, the model can hand a self-contained task to a
//! helper. The helper is a nested agent turn with a fresh history, so long side work
//! (reading many files, several pages) stays out of the chat's context; only its final
//! answer comes back, as the tool result.
//!
//! What must hold:
//!
//! - The helper's calls go through the same policy gate, grants, approval cards and audit
//!   log as the turn's own (the app builds them for the same chat, see
//!   `turns::AppTools::run_subagent`). Its cards and audit rows say they come from a
//!   subagent, and its audit rows point at the `run_subagent` call's row.
//! - A helper cannot start a helper: the tool is not offered to it, and a call to it is
//!   refused ([`NoNesting`]). Nor can it hand a task to a coding app (0.0.6), in any
//!   approval mode: those tools are left out and refused the same way.
//! - The helper starts where the turn is: if untrusted content already entered the turn,
//!   or the `run_subagent` call itself was recovered from text, the helper's calls are
//!   decided as such from its first one ([`InheritedGate`], D9 and 8.3).
//! - Stop ends the helper and whatever it is running: its token is a child of the call's.
//! - Nothing of the helper is stored as chat messages, and its token counts never reach
//!   the chat header: [`HelperSink`] forwards progress lines and audit rows only.
//! - The answer is ordinary tool-result text: the turn wraps, sanitises and caps it like
//!   any other result.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::history::{CallRecord, CallStatus, HistoryMessage, STOPPED_TEXT};
use agent::turn::{
    Approver, PolicyGate, ToolHost, TurnDeps, TurnEvent, TurnInput, TurnOutcome, TurnSink,
    TurnStatus,
};
use agent::{OpenAiClient, Role, Sanitizer, ToolInfo};
use async_trait::async_trait;
use mcp_host::{
    BuiltinServer, CallContext, ChatContext, Described, ProgressFn, ToolDef, ToolOutput,
};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// The built-in server that lists the tool (the "Subagents" group of the Tools panel).
pub const SERVER_NAME: &str = "subagents";
/// The tool's name.
pub const TOOL_NAME: &str = "run_subagent";

/// What a helper is told when it tries to start a helper.
pub const NESTED_REFUSAL: &str =
    "A helper cannot start another helper. Do the work yourself with the tools you have.";

/// The start of every approval card a helper raises and of every audit preview of its calls.
pub const MARK: &str = "Subagent: ";

const DESCRIPTION: &str = "Hand a self-contained task to a helper that works in its own fresh context and returns only its final answer. Use it for side work whose steps this chat does not need, such as reading many files or several web pages to answer one question. The helper remembers nothing from this chat: put everything it needs into `task` and `context`. It has the same tools as you except this one, and the user approves its actions in the same way.";

/// The app's framing of the helper's one user message.
const HANDOVER: &str = "[App note] The main assistant of this chat handed you the task below. You are its helper. You cannot see the chat, so work only from what is written here and from what your tools return. Do not ask questions: nobody can answer them. When you are done, reply with your final answer. It is the only thing the main assistant receives, so put every result it needs into it.";

/// Whether a registered tool is `run_subagent`.
pub fn is_tool(tool: &mcp_host::RegisteredTool) -> bool {
    tool.server == SERVER_NAME && tool.original == TOOL_NAME
}

/// Whether an action is a `run_subagent` call.
pub fn is_action(action: &policy::Action) -> bool {
    action.server == SERVER_NAME && action.tool == TOOL_NAME
}

/// Whether a tool of the chat is offered to a helper: every one but `run_subagent` and
/// the coding apps (0.0.6: a helper never hands a task to Claude Code or Codex).
pub fn offered_to_helper(tool: &mcp_host::RegisteredTool) -> bool {
    !is_tool(tool) && !crate::outside_agent::is_tool(tool)
}

/// What a helper is told for a tool it may not use, if it is one.
fn not_for_helpers(name: &str) -> Option<&'static str> {
    if name == TOOL_NAME {
        Some(NESTED_REFUSAL)
    } else if crate::outside_agent::is_tool_name(name) {
        Some(crate::outside_agent::HELPER_REFUSAL)
    } else {
        None
    }
}

/// The summary line of an approval card raised by a helper.
pub fn card_summary(summary: &str) -> String {
    format!("{MARK}{summary}")
}

/// Marks an audit row as a helper's: its preview says so, and it points at the row of the
/// `run_subagent` call (when that row could be written).
pub fn mark_audit(entry: &mut store::AuditEntry, parent: Option<store::AuditId>) {
    entry.preview = format!("{MARK}{}", entry.preview)
        .chars()
        .take(20_000)
        .collect();
    entry.parent_audit_id = parent;
}

// ---- The built-in server -----------------------------------------------------------

/// Lists `run_subagent` so it appears in tool lists, per-chat switches and token
/// estimates like every built-in tool, and describes a call for the policy engine. It
/// does not run the call: a helper needs the turn's client, policy gate, approver and
/// sink, so the turn's tool host runs it (`turns::AppTools::execute`).
pub struct SubagentsServer;

fn text_arg<'a>(args: &'a Map<String, Value>, key: &str) -> &'a str {
    args.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

#[async_trait]
impl BuiltinServer for SubagentsServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![ToolDef {
            name: TOOL_NAME.into(),
            description: DESCRIPTION.into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "task": {
                        "type": "string",
                        "minLength": 1,
                        "description": "The full instructions for the helper. It sees nothing else from this chat."
                    },
                    "context": {
                        "type": "string",
                        "description": "Facts the helper needs: paths, names, what is already known."
                    }
                },
                "required": ["task"],
                "additionalProperties": false
            }),
            destructive: Some(false),
            read_only: None,
        }]
    }

    async fn describe(
        &self,
        _tool: &str,
        args: &Map<String, Value>,
        _chat: &ChatContext,
    ) -> Described {
        let task = text_arg(args, "task");
        if task.is_empty() {
            return Described::Invalid(
                "task is empty: write the full instructions for the helper".into(),
            );
        }
        let mut preview = format!("Task:\n{}", policy::sanitize::visible(task));
        let context = text_arg(args, "context");
        if !context.is_empty() {
            preview.push_str("\n\nContext:\n");
            preview.push_str(&policy::sanitize::visible(context));
        }
        Described::Action(policy::Action {
            server: SERVER_NAME.into(),
            tool: TOOL_NAME.into(),
            summary: format!("Start a helper: {}", policy::sanitize::one_line(task, 120)),
            preview,
            // The policy engine's "never prompts" scope (D11). Starting a helper does
            // nothing by itself: every action the helper takes is decided on its own.
            scope: policy::Scope::Skill,
            risk: policy::Risk::Normal,
            read_only: false,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Value::Null,
        })
    }

    async fn call(
        &self,
        _tool: &str,
        _args: &Map<String, Value>,
        _action: &policy::Action,
        _ctx: CallContext,
    ) -> ToolOutput {
        ToolOutput::error("run_subagent can only be started from a chat turn; nothing was run.")
    }
}

// ---- Which chat has a helper at work -------------------------------------------------

/// The chat whose helper is at work, with the audit row of its `run_subagent` call. One
/// turn runs at a time (8.2) and calls run in order, so there is at most one.
static RUNNING: Mutex<Option<(i64, Option<store::AuditId>)>> = Mutex::new(None);

/// Clears the entry when the call ends, however it ends.
pub struct Running(i64);

impl Drop for Running {
    fn drop(&mut self) {
        if let Ok(mut running) = RUNNING.lock()
            && running.is_some_and(|(chat, _)| chat == self.0)
        {
            *running = None;
        }
    }
}

/// Records that a helper is at work in `chat` until the returned guard is dropped.
pub fn enter(chat: i64, audit_row: Option<store::AuditId>) -> Running {
    if let Ok(mut running) = RUNNING.lock() {
        *running = Some((chat, audit_row));
    }
    Running(chat)
}

/// `Some` while a helper is at work in `chat`: the audit row of its `run_subagent` call,
/// if that row could be written. For approvals raised outside the agent loop (a
/// navigation the browser holds, `turns::ask_during_call`).
pub fn running_in(chat: i64) -> Option<Option<store::AuditId>> {
    let running = RUNNING.lock().ok()?;
    running.filter(|(c, _)| *c == chat).map(|(_, row)| row)
}

// ---- The helper's view of the chat's tools and policy ---------------------------------

/// The chat's tools without `run_subagent` and the coding apps (0.0.6): they are not
/// offered to a helper, and a call to one is refused whatever the wrapped host would do.
struct NoNesting(Arc<dyn ToolHost>);

#[async_trait]
impl ToolHost for NoNesting {
    fn tools(&self) -> Vec<ToolInfo> {
        self.0
            .tools()
            .into_iter()
            .filter(|tool| not_for_helpers(&tool.name).is_none())
            .collect()
    }
    fn validate(&self, name: &str, args: &Value) -> Result<(), Vec<String>> {
        if let Some(refusal) = not_for_helpers(name) {
            return Err(vec![refusal.to_owned()]);
        }
        self.0.validate(name, args)
    }
    fn llama_schema(&self, name: &str) -> Option<Value> {
        if not_for_helpers(name).is_some() {
            return None;
        }
        self.0.llama_schema(name)
    }
    async fn describe(&self, name: &str, args: &Map<String, Value>) -> Described {
        if let Some(refusal) = not_for_helpers(name) {
            return Described::Invalid(refusal.to_owned());
        }
        self.0.describe(name, args).await
    }
    async fn execute(
        &self,
        name: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        if name == TOOL_NAME || is_action(action) {
            return ToolOutput::error(NESTED_REFUSAL);
        }
        // Also through `call_tool`, which is described as the tool it names.
        if crate::outside_agent::is_tool_name(name) || crate::outside_agent::is_action(action) {
            return ToolOutput::error(crate::outside_agent::HELPER_REFUSAL);
        }
        self.0.execute(name, args, action, cancel).await
    }
}

/// What a helper inherits from the turn that starts it, for the policy engine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Inherited {
    /// Untrusted content had entered the turn when the helper was started (D9). The task
    /// was written by a model that had read it, so the helper's outbound actions ask.
    pub untrusted: bool,
    /// The `run_subagent` call was recovered from text (8.3), so the helper's calls are
    /// decided like recovered ones: anything but a read shows a card, grant or not.
    pub recovered: bool,
}

/// The chat's policy gate, with what the helper inherits added to every decision.
struct InheritedGate {
    inner: Arc<dyn PolicyGate>,
    inherited: Inherited,
}

impl PolicyGate for InheritedGate {
    fn decide(
        &self,
        tool: &str,
        action: &policy::Action,
        recovered: bool,
        untrusted_in_turn: bool,
    ) -> policy::Decision {
        if tool == TOOL_NAME || is_action(action) {
            return policy::Decision::Refused {
                reason: NESTED_REFUSAL.to_owned(),
            };
        }
        if crate::outside_agent::is_tool_name(tool) || crate::outside_agent::is_action(action) {
            return policy::Decision::Refused {
                reason: crate::outside_agent::HELPER_REFUSAL.to_owned(),
            };
        }
        self.inner.decide(
            tool,
            action,
            recovered || self.inherited.recovered,
            untrusted_in_turn || self.inherited.untrusted,
        )
    }
}

// ---- Events of the helper's turn ------------------------------------------------------

/// Where a helper's audit rows go (the turn's persister).
pub type AuditFn = Arc<dyn Fn(store::AuditEntry) + Send + Sync>;

/// Streamed answer text is sent to the card about this often.
const TEXT_BATCH: Duration = Duration::from_millis(30);

/// Receives the helper's events. Only two things leave it: progress text for the
/// `run_subagent` tool card, and one audit row per call. Messages are not stored, and
/// usage and compaction events are dropped, so the chat's stored history, its summary and
/// the context numbers in its header stay the turn's own.
struct HelperSink {
    chat_id: i64,
    parent_audit: Option<store::AuditId>,
    progress: ProgressFn,
    audit: AuditFn,
    result_cap: Arc<AtomicUsize>,
    state: Mutex<SinkState>,
}

#[derive(Default)]
struct SinkState {
    started: HashMap<String, CallRecord>,
    /// Answer text streamed since the last completed message: what the helper "had so
    /// far" when a request fails.
    partial: String,
    /// Streamed text not yet sent to the card.
    buffered: String,
    buffered_since: Option<Instant>,
    /// The card's text does not end with a line break.
    mid_line: bool,
}

impl HelperSink {
    fn say(&self, st: &mut SinkState, text: &str) {
        if text.is_empty() {
            return;
        }
        st.mid_line = !text.ends_with('\n');
        (self.progress)(text.to_owned());
    }

    fn flush(&self, st: &mut SinkState) {
        st.buffered_since = None;
        let text = std::mem::take(&mut st.buffered);
        self.say(st, &text);
    }

    /// Ends the line in progress, if any.
    fn end_line(&self, st: &mut SinkState) {
        self.flush(st);
        if st.mid_line {
            self.say(st, "\n");
        }
    }

    fn partial(&self) -> String {
        self.state
            .lock()
            .map(|st| st.partial.clone())
            .unwrap_or_default()
    }

    fn finish(&self) {
        if let Ok(mut st) = self.state.lock() {
            self.end_line(&mut st);
        }
    }
}

/// The part of a call's arguments worth a few words on its progress line.
fn argument_hint(args: &Map<String, Value>) -> String {
    let named = ["path", "command", "query", "url", "name", "folder", "repo"]
        .iter()
        .find_map(|key| args.get(*key).and_then(Value::as_str));
    match named {
        Some(text) => policy::sanitize::one_line(text, 80),
        None if args.is_empty() => String::new(),
        None => policy::sanitize::one_line(&Value::Object(args.clone()).to_string(), 80),
    }
}

fn outcome_word(status: &CallStatus) -> &'static str {
    match status {
        CallStatus::Ok => "done",
        CallStatus::Error => "failed",
        CallStatus::Denied { .. } => "denied",
        CallStatus::Cancelled => "cancelled",
        CallStatus::NotRun => "not run",
        CallStatus::Interrupted => "interrupted",
        CallStatus::Refused => "not allowed",
    }
}

impl TurnSink for HelperSink {
    fn event(&self, e: TurnEvent) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        let st = &mut *st;
        match e {
            TurnEvent::Budget { budget } => {
                self.result_cap
                    .store(budget.result_cap as usize, Ordering::Relaxed);
            }
            TurnEvent::ContentDelta { text } => {
                st.partial.push_str(&text);
                st.buffered.push_str(&text);
                let since = *st.buffered_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= TEXT_BATCH {
                    self.flush(st);
                }
            }
            TurnEvent::AttemptDiscarded { .. } => {
                st.partial.clear();
                self.end_line(st);
                self.say(st, "(trying again)\n");
            }
            TurnEvent::MessageCompleted { .. } => {
                st.partial.clear();
                self.end_line(st);
            }
            TurnEvent::ToolCallStarted { call } => {
                self.end_line(st);
                let hint = argument_hint(&call.arguments);
                let line = if hint.is_empty() {
                    call.name.clone()
                } else {
                    format!("{} {hint}", call.name)
                };
                self.say(st, &line);
                st.started.insert(call.id.clone(), call);
            }
            TurnEvent::ToolCallFinished {
                result,
                decision,
                action,
                duration_ms,
                ..
            } => {
                let arguments = st
                    .started
                    .remove(&result.call_id)
                    .map(|call| Value::Object(call.arguments))
                    .unwrap_or(Value::Null);
                let (_, mut entry) = crate::turns::audit_entry(
                    self.chat_id,
                    arguments,
                    &result,
                    &decision,
                    action.as_ref(),
                    duration_ms,
                );
                mark_audit(&mut entry, self.parent_audit);
                (self.audit)(entry);
                self.flush(st);
                let word = outcome_word(&result.status);
                self.say(st, &format!(" — {word}\n"));
            }
            TurnEvent::AppNote { text } => {
                self.end_line(st);
                self.say(st, &format!("{text}\n"));
            }
            TurnEvent::Finished { .. } => self.end_line(st),
            // Thinking is not shown on the card. Approval cards are shown by the approver.
            // Usage and compaction belong to the helper's own context, not the chat's.
            TurnEvent::StepStarted { .. }
            | TurnEvent::ReasoningDelta { .. }
            | TurnEvent::ApprovalPending { .. }
            | TurnEvent::ApprovalResolved { .. }
            | TurnEvent::Usage { .. }
            | TurnEvent::Compacted { .. } => {}
        }
    }
}

// ---- Running a helper -------------------------------------------------------------

/// Everything one helper needs. The app builds the tools, policy gate and approver for
/// the same chat as the turn's own, so the helper's calls are decided, shown and logged
/// the same way.
pub struct Helper {
    pub chat_id: i64,
    pub client: OpenAiClient,
    pub sanitizer: Sanitizer,
    /// The chat's tools. `run_subagent` is taken out and refused here, whatever it holds.
    pub tools: Arc<dyn ToolHost>,
    pub policy: Arc<dyn PolicyGate>,
    pub approver: Arc<dyn Approver>,
    /// The turn's input: system prompt, preamble, skills, model capabilities, request
    /// template and step cap. Its history and summary are replaced.
    pub input: TurnInput,
    pub inherited: Inherited,
    /// Progress text for the `run_subagent` tool card.
    pub progress: ProgressFn,
    pub audit: AuditFn,
    /// The audit row of the `run_subagent` call.
    pub parent_audit: Option<store::AuditId>,
    /// The helper's own per-result cap, which its tools read.
    pub result_cap: Arc<AtomicUsize>,
}

/// The helper's one user message: the app's framing, then the task and the context as the
/// model wrote them (cleaned of chat-template control tokens, like any model-made text
/// that goes back into a prompt).
///
/// The system prompt tells the model that lines starting with `[App note]` come from the
/// app, and the task is not inside a delimited block, so without care the main model,
/// steered by something it read, could pass an instruction to the helper as the app's own
/// `[App note]` marker. The task and context therefore go through the summary block's own
/// cleaning ([`agent::prompt::defang_app_notes`]): every `[App note]` marker (any letter
/// case, any spacing or hidden characters inside, full-width brackets too) gets round
/// brackets, with its hidden characters shown as visible escapes, and control and tag
/// characters are shown as escapes anywhere. The rest reaches the helper word for word, in
/// any script; text that only resembles the marker (`[App notes]`) is not changed.
fn handover_message(task: &str, context: &str, sanitizer: &Sanitizer) -> String {
    let clean = |text: &str| agent::prompt::defang_app_notes(text, sanitizer);
    let mut text = format!("{HANDOVER}\n\nTask:\n{}", clean(task));
    if !context.is_empty() {
        text.push_str("\n\nContext:\n");
        text.push_str(&clean(context));
    }
    text
}

/// Runs one helper to its end and returns its final answer as the tool result. Failures
/// (stopped, step cap, repeated or invalid calls, a failed request) come back as error
/// results that say what happened and carry what the helper had so far.
///
/// The helper's turn runs in its own task. When Stop is pressed, the agent loop waits only
/// a moment for a tool before it moves on; the helper then still ends by itself (its token
/// is cancelled) and writes the audit rows of the calls it had started.
pub async fn run(
    helper: Helper,
    args: &Map<String, Value>,
    cancel: CancellationToken,
) -> ToolOutput {
    let task = text_arg(args, "task");
    if task.is_empty() {
        return ToolOutput::error("task is empty: write the full instructions for the helper");
    }
    let Helper {
        chat_id,
        client,
        sanitizer,
        tools,
        policy,
        approver,
        mut input,
        inherited,
        progress,
        audit,
        parent_audit,
        result_cap,
    } = helper;
    input.history = vec![HistoryMessage::user(handover_message(
        task,
        text_arg(args, "context"),
        &sanitizer,
    ))];
    input.summary = None;
    input.app_notes.clear();
    let step_cap = input.step_cap;
    // A child of the call's token: Stop reaches the helper's stream and its running tool.
    // Cancelled as well if this call is dropped, so a helper never works on unattended.
    let token = cancel.child_token();
    let _stop_with_call = token.clone().drop_guard();
    let work = tokio::spawn(async move {
        let tools = NoNesting(tools);
        let policy = InheritedGate {
            inner: policy,
            inherited,
        };
        let sink = HelperSink {
            chat_id,
            parent_audit,
            progress,
            audit,
            result_cap,
            state: Mutex::default(),
        };
        let deps = TurnDeps {
            client: &client,
            tools: &tools,
            policy: &policy,
            approver: approver.as_ref(),
            sink: &sink,
            sanitizer: &sanitizer,
        };
        let outcome = agent::turn::run_turn(deps, input, token).await;
        sink.finish();
        tracing::info!(
            status = ?outcome.status,
            requests = outcome.usage.requests,
            prompt_tokens = outcome.usage.prompt_tokens,
            completion_tokens = outcome.usage.completion_tokens,
            "a helper finished"
        );
        output(&outcome, &sink.partial(), step_cap)
    });
    work.await.unwrap_or_else(|_| {
        ToolOutput::error("The helper stopped unexpectedly; nothing came back.")
    })
}

/// The tool result for a finished helper turn.
fn output(outcome: &TurnOutcome, partial: &str, step_cap: u32) -> ToolOutput {
    let said = |with_calls: bool| {
        outcome
            .messages
            .iter()
            .rev()
            .filter(|m| m.role == Role::Assistant && (with_calls || !m.has_tool_calls()))
            .map(HistoryMessage::text)
            .find(|text| !text.trim().is_empty() && text.trim() != STOPPED_TEXT)
    };
    // What it had when it was cut short: the text of the reply in flight, else the last
    // thing it said.
    let so_far = || {
        let text = if partial.trim().is_empty() {
            said(true).unwrap_or_default()
        } else {
            partial.to_owned()
        };
        if text.trim().is_empty() {
            String::new()
        } else {
            format!("\n\nWhat it had so far:\n{}", text.trim())
        }
    };
    let reported = || match said(false) {
        Some(text) => format!("\n\nWhat it reported:\n{}", text.trim()),
        None => String::new(),
    };
    match &outcome.status {
        TurnStatus::Completed => match said(false) {
            Some(answer) => ToolOutput::text(answer),
            None => ToolOutput::error("The helper finished without an answer."),
        },
        TurnStatus::Stopped => ToolOutput::error(format!(
            "The helper was stopped before it finished.{}",
            so_far()
        )),
        TurnStatus::StepCapReached => ToolOutput::error(format!(
            "The helper did not finish: it reached its limit of {step_cap} model calls.{}",
            reported()
        )),
        TurnStatus::MistakeBudget => ToolOutput::error(format!(
            "The helper did not finish: it made too many invalid tool calls.{}",
            reported()
        )),
        TurnStatus::LoopDetected => ToolOutput::error(format!(
            "The helper did not finish: it kept repeating the same tool call.{}",
            reported()
        )),
        TurnStatus::Failed { message, .. } => ToolOutput::error(format!(
            "The helper could not finish: {message}{}",
            so_far()
        )),
    }
}

#[cfg(test)]
mod tests;
