//! One agent turn (ARCHITECTURE.md 8.2), from the user's message to the final answer.
//!
//! ```text
//! normalise history → count, compact → stream → classify response
//!    ├─ final text                         → done
//!    ├─ tool calls (native)                ┐
//!    ├─ tool calls recovered from text     ┼→ for each call, in order:
//!    │                                     │    repair → validate → policy → (approval) → run → cap → append result
//!    │                                     ┘  → next step
//!    ├─ empty reply, or server parse error → one retry with thinking disabled → next step
//!    └─ reply cut off at the output limit  → handled by what it held → next step
//! ```
//!
//! [`run_turn`] talks to the model through [`OpenAiClient`], to tools through a [`ToolHost`],
//! to the policy engine through a [`PolicyGate`] (a read-only view: nothing here can change
//! grants), to the user through an [`Approver`], and reports everything through a
//! [`TurnSink`]. Every message it appends is reported as [`TurnEvent::MessageCompleted`] as
//! soon as it exists, so a crash loses at most the tokens in flight, and the same messages
//! come back in [`TurnOutcome::messages`].
//!
//! Rules (8.2, 8.3):
//!
//! - the user's pinned instructions ([`TurnInput::pinned`]) are part of the system message
//!   of every request, the summary requests included; a turn whose pins do not fit their
//!   share of the context does not start;
//! - `parallel_tool_calls: false` and `max_tokens` on every request; calls run one at a time,
//!   in order; a denied or cancelled call stops the rest of that response (`not run`);
//! - step cap (default 25 model calls): calls in the response that reaches it are not run,
//!   and one wrap-up request follows with `tool_choice: "none"` and the tool list unchanged;
//!   tool-call markup in the wrap-up reply is stripped;
//! - mistake budget: 3 consecutive invalid calls to one tool, or 6 consecutive mistakes
//!   overall, end the turn with the same wrap-up; a success resets both; denials and tool
//!   errors are not mistakes;
//! - loop detection: the same tool with the same arguments three times in a row gets a note,
//!   five times ends the turn with the wrap-up; a call identical to one denied earlier in the
//!   turn is refused with the earlier note;
//! - output limit: a reply that only thought is retried once with thinking disabled; a
//!   cut-off call is not run and is stored with `{}` arguments, and the model is told to
//!   write smaller pieces; a second cut-off of the same tool ends the turn;
//! - an empty reply or a server parse error is retried once with thinking disabled, its
//!   streamed output discarded ([`TurnEvent::AttemptDiscarded`]); a second failure ends the
//!   turn with "The model returned no answer";
//! - a call found in reasoning is never run; untrusted content (a tool result, an attachment)
//!   makes outbound actions ask (`untrusted_in_turn`); recovered calls reach the policy gate
//!   with `recovered = true`;
//! - cancellation is checked between stages and passed to the stream and to every tool call;
//!   a pending approval is resolved as cancelled.

mod runner;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::client::OpenAiClient;
use crate::context::ContextBudget;
use crate::estimate::TokenCalibrator;
use crate::history::{CallRecord, HistoryMessage, ResultRecord};
use crate::parse::ToolInfo;
use crate::prompt::{LoadedSkill, Preamble, Sanitizer};
use crate::stream::{Timings, Usage};
use crate::thinking::ThinkingLimit;

/// The default number of model calls in one turn.
pub const DEFAULT_STEP_CAP: u32 = 25;
/// Consecutive invalid calls to one tool that end the turn.
pub const MAX_INVALID_CALLS_PER_TOOL: u32 = 3;
/// Consecutive mistakes overall that end the turn.
pub const MAX_MISTAKES: u32 = 6;
/// The same call this many times in a row gets a note.
pub const LOOP_WARNING_AT: u32 = 3;
/// The same call this many times in a row ends the turn.
pub const LOOP_STOP_AT: u32 = 5;
/// The message when the model gives no answer twice.
pub const NO_ANSWER_MESSAGE: &str = "The model returned no answer";
/// The note ([`TurnEvent::AppNote`]) when a summary of earlier messages could not be written
/// (the request did not fit, the model returned nothing, or the server failed). Reported once
/// per turn; the turn goes on with the other stages.
pub const SUMMARY_FAILED_NOTE: &str = "Could not summarise earlier messages. The reply goes on, \
                                       but this chat may run out of room soon.";
/// The note ([`TurnEvent::AppNote`]) while the model writes a summary of earlier messages.
pub const SUMMARISING_NOTE: &str = "Summarising earlier messages to make room.";

/// The stages that make room in a request (ARCHITECTURE.md 8.4), in the order the loop tries
/// them. [`TurnEvent::Compacted`] carries the stage as its number
/// ([`CompactionStage::number`]), so the app can say what happened in plain words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompactionStage {
    /// 1: the results of tool calls in earlier turns became one-line notes.
    ShortenEarlierResults,
    /// 2: the oldest earlier messages were replaced by a summary the model wrote, or the
    /// summary that stands for them was rewritten shorter to fit a smaller context.
    SummariseEarlier,
    /// 3: older tool results of the turn itself became one-line notes.
    ShortenTurnResults,
}

impl CompactionStage {
    /// The stage's number in [`TurnEvent::Compacted`] and [`TurnPreview::applied`].
    pub const fn number(self) -> u8 {
        match self {
            Self::ShortenEarlierResults => 1,
            Self::SummariseEarlier => 2,
            Self::ShortenTurnResults => 3,
        }
    }

    /// The stage with this number, if there is one.
    pub const fn from_number(number: u8) -> Option<Self> {
        match number {
            1 => Some(Self::ShortenEarlierResults),
            2 => Some(Self::SummariseEarlier),
            3 => Some(Self::ShortenTurnResults),
            _ => None,
        }
    }
}

/// The tools of the chat, as the loop needs them. Implemented by the app over the MCP host.
#[async_trait]
pub trait ToolHost: Send + Sync {
    /// The tools enabled for this chat, in a fixed order.
    fn tools(&self) -> Vec<ToolInfo>;
    /// Validates arguments against the tool's full schema (compiled once in the host).
    ///
    /// # Errors
    /// One short message per problem, naming the field and what was expected.
    fn validate(&self, name: &str, args: &Value) -> Result<(), Vec<String>>;
    /// The simplified schema sent to llama-server (ARCHITECTURE.md 8.3, "Schema hand-off"),
    /// or `None` to send the full schema.
    fn llama_schema(&self, name: &str) -> Option<Value>;
    /// Turns validated arguments into a policy action (no side effects).
    async fn describe(&self, name: &str, args: &Map<String, Value>) -> mcp_host::Described;
    /// Runs the approved action. `cancel` is a child of the turn's token.
    async fn execute(
        &self,
        name: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> mcp_host::ToolOutput;
}

/// The policy engine as the loop sees it: read-only. The app wraps [`policy::evaluate`] with
/// the chat's grants and enabled-tool state.
pub trait PolicyGate: Send + Sync {
    /// Decides one call. `recovered` is true for a call recovered from text;
    /// `untrusted_in_turn` once untrusted content entered the turn.
    fn decide(
        &self,
        tool: &str,
        action: &policy::Action,
        recovered: bool,
        untrusted_in_turn: bool,
    ) -> policy::Decision;
}

/// An approval card to show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalRequest {
    /// The call being approved.
    pub call_id: String,
    /// The tool name.
    pub tool: String,
    /// What the call would do, as the policy engine sees it.
    pub action: policy::Action,
    /// The buttons to offer.
    pub card: policy::CardKind,
    /// Why the card is shown.
    pub reason: policy::AskReason,
}

/// The user's answer to an approval card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Approval {
    /// Run this call.
    AllowOnce,
    /// Run this call and grant its scope for the chat (the app records the grant).
    AllowForChat,
    /// Do not run it; the note goes back to the model.
    Deny {
        /// The user's note for the model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The card was dismissed or the turn stopped.
    Cancelled,
}

/// Shows approval cards. Implemented by the app.
#[async_trait]
pub trait Approver: Send + Sync {
    /// Shows the card and waits for the answer. When the turn is cancelled the loop stops
    /// waiting and reports [`TurnEvent::ApprovalResolved`] with [`Approval::Cancelled`].
    async fn ask(&self, req: ApprovalRequest) -> Approval;
}

/// Receives the turn's events (UI streaming, persistence, audit). Must not block.
pub trait TurnSink: Send + Sync {
    /// Handles one event.
    fn event(&self, e: TurnEvent);
}

/// How a call was decided, for the audit log (ARCHITECTURE.md 13.7).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CallDecision {
    /// The user allowed it once.
    AllowedOnce,
    /// The user allowed it for the chat.
    AllowedForChat,
    /// A grant for the chat covered it.
    AllowedByGrant,
    /// The chat is in Allow all and the action is not one that keeps asking.
    AllowedByMode,
    /// The chat is in Full access: nothing asks (0.0.5).
    AllowedByFullAccess,
    /// The user denied it.
    Denied {
        /// The user's note.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The turn stopped while the call was pending or running.
    Cancelled,
    /// Refused by policy or by the loop without asking.
    Refused {
        /// Why.
        reason: String,
    },
    /// The call never reached the policy engine (invalid, cut off, not run).
    NotEvaluated,
}

/// One event of a turn.
// Events are moved once into the sink, a few per step; boxing the larger variants would only
// make them clumsier to match on.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TurnEvent {
    /// A model call (step) begins. Retries and the wrap-up are not steps.
    StepStarted {
        /// 1-based.
        step: u32,
    },
    /// The context numbers, after the fixed overhead was measured (and again when it
    /// changes). Tools size their output to `budget.result_cap`.
    Budget {
        /// The numbers.
        budget: ContextBudget,
    },
    /// Answer text streamed.
    ContentDelta {
        /// The new text.
        text: String,
    },
    /// Reasoning text streamed (the Thinking block).
    ReasoningDelta {
        /// The new text.
        text: String,
    },
    /// The text streamed since the last step start is discarded: the attempt failed and is
    /// retried (an empty reply, a server parse error, a reply that only thought).
    AttemptDiscarded {
        /// Why, for the log.
        reason: String,
    },
    /// A message was appended to the chat: store it. Assistant messages (with their tool
    /// calls), tool-result messages and the closing `(stopped)` message all come this way.
    MessageCompleted {
        /// The message.
        message: HistoryMessage,
    },
    /// A tool call is being processed (for its card).
    ToolCallStarted {
        /// The call.
        call: CallRecord,
    },
    /// An approval card should be shown.
    ApprovalPending {
        /// The card.
        request: ApprovalRequest,
    },
    /// An approval card was answered or cancelled.
    ApprovalResolved {
        /// The call.
        call_id: String,
        /// The answer.
        approval: Approval,
    },
    /// A tool call finished, ran or not.
    ToolCallFinished {
        /// What the model sees.
        result: ResultRecord,
        /// How it was decided.
        decision: CallDecision,
        /// The action, when the call was described.
        action: Option<policy::Action>,
        /// The full output, when the model saw a shortened version.
        full_text: Option<String>,
        /// Structured details for the card.
        ui: Option<Value>,
        /// Time from the start of processing to the result.
        duration_ms: u64,
    },
    /// A note for the UI (and the log), not for the model. A summary that could not be
    /// written (stage 2) is reported this way, with [`SUMMARY_FAILED_NOTE`]; the turn goes on
    /// with the other stages.
    AppNote {
        /// The note.
        text: String,
    },
    /// Token counts and timings of one response.
    Usage {
        /// The counts.
        usage: Usage,
        /// llama-server's timings, when sent.
        timings: Option<Timings>,
    },
    /// The context was compacted: one event per stage applied, each naming its stage.
    Compacted {
        /// The [`CompactionStage`], by its number ([`CompactionStage::from_number`]):
        /// 1: earlier results stubbed; 2: earlier turns summarised, or the summary that
        /// stands for them rewritten shorter for a smaller context (then `covered_ids` is
        /// what it covered before); 3: this turn's results stubbed.
        stage: u8,
        /// The new summary (stage 2, always set): store it in place of the earlier one.
        summary: Option<String>,
        /// Ids of every stored message the summary now covers (stage 2).
        covered_ids: Vec<i64>,
    },
    /// The turn ended.
    Finished {
        /// The outcome.
        outcome: TurnOutcome,
    },
}

/// What the model can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCaps {
    /// The template supports tool calls.
    pub tools: bool,
    /// The model can see images.
    pub vision: bool,
    /// The allocated context, in tokens.
    pub n_ctx: u32,
    /// The app's own llama-server: exact token counts.
    pub local: bool,
    /// The model can think and thinking can be switched off per request.
    pub thinking: bool,
    /// The template accepts reasoning in earlier assistant messages.
    pub preserve_reasoning: bool,
    /// Images from tools go inside the tool message (else in a user message after it).
    pub images_in_tool_results: bool,
}

impl Default for ModelCaps {
    fn default() -> Self {
        Self {
            tools: true,
            vision: false,
            n_ctx: 4_096,
            local: false,
            thinking: false,
            preserve_reasoning: false,
            images_in_tool_results: true,
        }
    }
}

/// The fixed parts of every request.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTemplate {
    /// The model id (llama-server ignores it).
    pub model: String,
    /// Sampling temperature; `None` uses the model's own default.
    pub temperature: Option<f32>,
    /// Nucleus sampling.
    pub top_p: Option<f32>,
    /// Top-k sampling.
    pub top_k: Option<u32>,
    /// Server-specific fields such as `cache_prompt`.
    #[serde(default)]
    pub extra: Map<String, Value>,
}

/// A summary of earlier messages (ARCHITECTURE.md 8.4, stage 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompactionSummary {
    /// The summary text.
    pub text: String,
    /// Ids of the stored messages it replaces in requests.
    pub covered_ids: Vec<i64>,
}

/// Everything a turn starts from.
#[derive(Debug, Clone)]
pub struct TurnInput {
    /// The chat so far, ending with the new user message.
    pub history: Vec<HistoryMessage>,
    /// The chat's system prompt.
    pub system_prompt: String,
    /// The user's pinned instructions that apply to this chat, in order. They follow the
    /// system prompt in every request of the turn and are never trimmed or summarised
    /// away; a turn whose pins do not fit does not start (`pinned_do_not_fit`).
    pub pinned: Vec<String>,
    /// What the preamble says (approved folders, OS); used when tools are on. `None` uses
    /// this OS and no folders.
    pub preamble: Option<Preamble>,
    /// The skills index.
    pub skills_index: Option<String>,
    /// Skills whose text is loaded.
    pub loaded_skills: Vec<LoadedSkill>,
    /// The current compaction summary, if any.
    pub summary: Option<CompactionSummary>,
    /// What the model can do.
    pub caps: ModelCaps,
    /// Model calls per turn.
    pub step_cap: u32,
    /// The request template.
    pub template: RequestTemplate,
    /// Whether thinking is on (ignored when the model cannot think).
    pub thinking: bool,
    /// How long the model may think in one request (0.0.5, "How long it may think"): each
    /// request with thinking on gets a budget of min(the choice's tokens, half its output
    /// limit) ([`ThinkingLimit::budget`]); llama-server only. The app fills it from the
    /// model's load settings; [`ThinkingLimit::NoLimit`] keeps half the output limit.
    pub thinking_limit: ThinkingLimit,
    /// App notes for the start of the turn (for example the side effects of a replaced turn).
    pub app_notes: Vec<String>,
    /// The token calibrator for this model (remote endpoints).
    pub calibrator: TokenCalibrator,
}

impl TurnInput {
    /// Defaults: no system prompt, no pinned instructions, no skills, 25 steps, thinking on.
    pub fn new(history: Vec<HistoryMessage>, caps: ModelCaps, template: RequestTemplate) -> Self {
        Self {
            history,
            system_prompt: String::new(),
            pinned: Vec::new(),
            preamble: None,
            skills_index: None,
            loaded_skills: Vec::new(),
            summary: None,
            caps,
            step_cap: DEFAULT_STEP_CAP,
            template,
            thinking: true,
            thinking_limit: ThinkingLimit::NoLimit,
            app_notes: Vec::new(),
            calibrator: TokenCalibrator::new(),
        }
    }
}

/// What the turn needs from the app. All read-only.
#[derive(Clone, Copy)]
pub struct TurnDeps<'a> {
    /// The model endpoint.
    pub client: &'a OpenAiClient,
    /// The chat's tools.
    pub tools: &'a dyn ToolHost,
    /// The policy engine.
    pub policy: &'a dyn PolicyGate,
    /// Approval cards.
    pub approver: &'a dyn Approver,
    /// Events.
    pub sink: &'a dyn TurnSink,
    /// The sanitiser for this model ([`Sanitizer::for_endpoint`]).
    pub sanitizer: &'a Sanitizer,
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TurnStatus {
    /// The model answered.
    Completed,
    /// The user stopped it.
    Stopped,
    /// The step cap was reached (after the wrap-up).
    StepCapReached,
    /// Too many invalid calls (after the wrap-up).
    MistakeBudget,
    /// The same call repeated (after the wrap-up).
    LoopDetected,
    /// The turn could not continue.
    Failed {
        /// A stable code (`no_answer`, `tools_do_not_fit`, `pinned_do_not_fit`,
        /// `context_full`, `output_limit`, or a provider error code).
        code: String,
        /// A plain-language message for the UI.
        message: String,
    },
}

/// Token totals for a turn.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageTotals {
    /// Requests made (steps, retries, wrap-up, summaries).
    pub requests: u32,
    /// Prompt tokens over all requests.
    pub prompt_tokens: u64,
    /// Generated tokens over all requests.
    pub completion_tokens: u64,
    /// Prompt tokens served from the cache.
    pub cached_tokens: u64,
    /// The prompt size of the last request (context in use).
    pub last_prompt_tokens: Option<u32>,
    /// The last timings llama-server sent.
    pub last_timings: Option<Timings>,
}

/// The result of a turn.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnOutcome {
    /// How it ended.
    pub status: TurnStatus,
    /// The messages the turn appended, in order (the same ones reported as
    /// [`TurnEvent::MessageCompleted`]). Always ends with an assistant message without calls.
    pub messages: Vec<HistoryMessage>,
    /// Token totals.
    pub usage: UsageTotals,
    /// The compaction summary after the turn (new or unchanged). When a summary too long for
    /// this context could not be rewritten, the one the turn used: cut to fit, and not
    /// reported as a new summary.
    pub summary: Option<CompactionSummary>,
    /// The context numbers used.
    pub budget: Option<ContextBudget>,
    /// The calibrator after the turn's observations.
    pub calibrator: TokenCalibrator,
}

/// One block of the system message, as it appears in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemBlock {
    /// Which part of the system message it is.
    pub kind: SystemBlockKind,
    /// Its text, exactly as in the system message.
    pub text: String,
    /// About how many tokens it takes.
    pub tokens: u32,
}

/// The parts of the system message ([`crate::prompt::system_message`]), in their order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SystemBlockKind {
    /// The chat's system prompt.
    Prompt,
    /// The user's pinned instructions.
    Pinned,
    /// The agent preamble (tools on).
    Preamble,
    /// The skills index.
    SkillsIndex,
    /// The text of one loaded skill.
    LoadedSkill {
        /// The skill's name.
        name: String,
    },
    /// The summary of earlier messages.
    Summary,
}

/// What the loop's context stages would remove next, in their order (ARCHITECTURE.md 8.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NextCompaction {
    /// Stage 1: the results of tool calls in earlier turns become one-line notes.
    ShortenEarlierResults {
        /// How many results.
        results: usize,
    },
    /// Stage 2: the oldest earlier messages are replaced by a summary the model writes.
    SummariseEarlier {
        /// How many of the oldest messages the loop's next summary would replace, chosen by
        /// the loop's own code (while everything fits, the oldest whole turn: the least a
        /// summary replaces).
        messages: usize,
    },
    /// Stage 3: older tool results of the turn itself become one-line notes.
    ShortenTurnResults,
    /// Nothing is left to remove.
    Nothing,
}

/// The first request of a turn, built by the loop's own code and not sent: for showing the
/// user what the model would receive. Building it sends nothing to the model and changes
/// nothing; the local model server may be asked to count tokens, as a turn does.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnPreview {
    /// The request as the turn's first model call would send it (`max_tokens` set).
    pub request: crate::types::ChatRequest,
    /// The blocks of its system message, in order; joined with a blank line they are the
    /// system message.
    pub system_blocks: Vec<SystemBlock>,
    /// The context numbers (fixed overhead measured as the turn measures it).
    pub budget: ContextBudget,
    /// The request's size in tokens as the loop counts it.
    pub prompt_tokens: u32,
    /// True when that count is the model server's own (local), not an estimate.
    pub exact: bool,
    /// The turn would not start: pinned instructions or tools do not fit (the loop's own
    /// status and message).
    pub refusal: Option<TurnStatus>,
    /// Tools are offered.
    pub tools_on: bool,
    /// Tools left out because their settings contain chat markup.
    pub withheld_tools: Vec<String>,
    /// What the tool definitions take of the fixed overhead.
    pub tool_tokens: u32,
    /// The most the fixed overhead may be, pins left out, with tools on (40% of the budget).
    pub tools_allowance: u32,
    /// The fixed overhead without the pinned instructions (what the tools' share is checked
    /// against).
    pub unpinned_overhead: u32,
    /// What the pinned instructions take.
    pub pinned_tokens: u32,
    /// The most they may take (25% of the budget).
    pub pinned_allowance: u32,
    /// The messages sent after the system message, before projection (stage-1 notes in
    /// place when the request needs them), with their token estimates.
    pub kept: Vec<(HistoryMessage, u32)>,
    /// Stored messages replaced by the summary of earlier messages, with their estimates.
    pub left_out: Vec<(HistoryMessage, u32)>,
    /// Stages this request already needs that the preview can apply, by number
    /// ([`CompactionStage`]): 1 when the results of earlier tool calls are shortened, as the
    /// turn does; 2 when the stored summary is too long for this context: the turn first has
    /// the model rewrite it shorter, which a preview never asks for, so the preview cuts it
    /// to that length instead (the summary block, the counts and the fit checks use the cut
    /// text, which is what the turn falls back to when the model cannot rewrite it).
    pub applied: Vec<u8>,
    /// The request is over the compaction trigger even so: [`TurnPreview::next`] happens
    /// before it is sent.
    pub over_trigger: bool,
    /// The request is over the whole budget.
    pub over_budget: bool,
    /// What the context stages would remove next.
    pub next: NextCompaction,
}

/// Builds the first request of a turn exactly as [`run_turn`] would, with the same
/// counting, fit checks and context stages, and sends nothing: stage 2 (a summary written
/// by the model) is reported in [`TurnPreview::next`], never run. `tools` is only listed,
/// never described or run.
pub async fn preview_turn(
    client: &OpenAiClient,
    tools: &dyn ToolHost,
    sanitizer: &Sanitizer,
    input: TurnInput,
) -> TurnPreview {
    runner::preview(client, tools, sanitizer, input).await
}

/// Runs one turn. Never panics on bad input from the model, the server or a tool; every
/// failure ends in a [`TurnStatus`].
pub async fn run_turn(
    deps: TurnDeps<'_>,
    input: TurnInput,
    cancel: CancellationToken,
) -> TurnOutcome {
    runner::Turn::new(deps, input, cancel).run().await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>(_: &T) {}

    struct Nothing;

    #[async_trait]
    impl ToolHost for Nothing {
        fn tools(&self) -> Vec<ToolInfo> {
            Vec::new()
        }
        fn validate(&self, _: &str, _: &Value) -> Result<(), Vec<String>> {
            Ok(())
        }
        fn llama_schema(&self, _: &str) -> Option<Value> {
            None
        }
        async fn describe(&self, _: &str, _: &Map<String, Value>) -> mcp_host::Described {
            mcp_host::Described::Invalid("none".into())
        }
        async fn execute(
            &self,
            _: &str,
            _: &Map<String, Value>,
            _: &policy::Action,
            _: CancellationToken,
        ) -> mcp_host::ToolOutput {
            mcp_host::ToolOutput::error("none")
        }
    }

    impl PolicyGate for Nothing {
        fn decide(&self, _: &str, _: &policy::Action, _: bool, _: bool) -> policy::Decision {
            policy::Decision::Refused {
                reason: "none".into(),
            }
        }
    }

    #[async_trait]
    impl Approver for Nothing {
        async fn ask(&self, _: ApprovalRequest) -> Approval {
            Approval::Cancelled
        }
    }

    impl TurnSink for Nothing {
        fn event(&self, _: TurnEvent) {}
    }

    /// Tauri spawns the turn on a multi-threaded runtime: the future must be `Send`.
    #[test]
    fn run_turn_future_is_send() {
        let client = OpenAiClient::new(crate::ClientConfig::llama_server(1, "k")).unwrap();
        let sanitizer = Sanitizer::generic();
        let deps = TurnDeps {
            client: &client,
            tools: &Nothing,
            policy: &Nothing,
            approver: &Nothing,
            sink: &Nothing,
            sanitizer: &sanitizer,
        };
        let input = TurnInput::new(Vec::new(), ModelCaps::default(), RequestTemplate::default());
        assert_send(&run_turn(deps, input, CancellationToken::new()));
    }

    #[test]
    fn events_serialise() {
        let event = TurnEvent::ApprovalResolved {
            call_id: "c".into(),
            approval: Approval::Deny {
                note: Some("no".into()),
            },
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "approvalResolved");
        assert_eq!(value["approval"]["kind"], "deny");
        let status = serde_json::to_value(TurnStatus::Failed {
            code: "no_answer".into(),
            message: NO_ANSWER_MESSAGE.into(),
        })
        .unwrap();
        assert_eq!(status["kind"], "failed");
    }

    #[test]
    fn compaction_stages_have_fixed_numbers() {
        for (stage, number) in [
            (CompactionStage::ShortenEarlierResults, 1),
            (CompactionStage::SummariseEarlier, 2),
            (CompactionStage::ShortenTurnResults, 3),
        ] {
            assert_eq!(stage.number(), number);
            assert_eq!(CompactionStage::from_number(number), Some(stage));
        }
        assert_eq!(CompactionStage::from_number(0), None);
        assert_eq!(CompactionStage::from_number(4), None);
        let event = TurnEvent::Compacted {
            stage: CompactionStage::ShortenTurnResults.number(),
            summary: None,
            covered_ids: Vec::new(),
        };
        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value["type"], "compacted");
        assert_eq!(value["stage"], 3);
    }

    #[test]
    fn a_new_turn_has_no_thinking_limit() {
        let input = TurnInput::new(Vec::new(), ModelCaps::default(), RequestTemplate::default());
        assert_eq!(input.thinking_limit, ThinkingLimit::NoLimit);
    }
}
