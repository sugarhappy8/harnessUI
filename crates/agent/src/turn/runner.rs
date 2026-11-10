//! The state machine behind [`super::run_turn`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::{
    Approval, ApprovalRequest, CallDecision, CompactionSummary, LOOP_STOP_AT, LOOP_WARNING_AT,
    MAX_INVALID_CALLS_PER_TOOL, MAX_MISTAKES, NO_ANSWER_MESSAGE, TurnDeps, TurnEvent, TurnInput,
    TurnOutcome, TurnStatus, UsageTotals,
};
use crate::client::ProviderKind;
use crate::context::{
    ContextBudget, KEPT_RESULT_IMAGES, ToolsDoNotFit, call_groups, drop_old_result_images,
    estimate_message, estimate_request, group_digits, stub_results, summarisable_prefix,
    summary_instruction, truncate_head_tail, turn_starts,
};
use crate::error::ProviderError;
use crate::estimate::{BASELINE_CHARS_PER_TOKEN, TokenCalibrator};
use crate::history::{
    CallRecord, CallStatus, HistoryMessage, Part, ResultRecord, STOPPED_TEXT, UNKNOWN_TOOL_NAME,
    normalize,
};
use crate::parse::{
    Recovered, RecoveryContext, ToolInfo, cut_off_call, recover_tool_calls, repair_arguments,
    strip_tool_markup,
};
use crate::prompt::{
    Preamble, Projection, SystemParts, append_result_note, project, sanitize_schema, system_message,
};
use crate::stream::{
    AccumulatedToolCall, ArgumentsStatus, FinishReason, StreamEvent, Timings, ToolCallAccumulator,
    Usage,
};
use crate::types::{
    ChatMessage, ChatRequest, ContentPart, MessageContent, Role, ToolChoice, ToolSpec,
};

/// How long a tool may take to return after the turn is cancelled before the loop stops
/// waiting for it.
const CANCEL_GRACE: Duration = Duration::from_millis(250);
/// The longest tool description sent to the model, in characters.
const MAX_DESCRIPTION_CHARS: usize = 2_000;
/// Compaction summaries one request may trigger (stage 2 of ARCHITECTURE.md 8.4).
const MAX_SUMMARY_ROUNDS: u32 = 4;
/// Candidate summary requests counted before a round gives up on finding one that fits.
const MAX_SUMMARY_FIT_TRIES: usize = 6;

/// What to do after a response was handled.
enum Next {
    /// Make the next model call.
    Continue,
    /// End the turn now.
    End(TurnStatus),
    /// End the turn after one wrap-up request.
    WrapUp(TurnStatus),
}

/// One streamed response.
#[derive(Debug, Default)]
struct Reply {
    content: String,
    reasoning: String,
    calls: Vec<AccumulatedToolCall>,
    finish: Option<FinishReason>,
    usage: Option<Usage>,
}

enum Attempt {
    Done(Reply),
    /// The turn was cancelled; `content` is what streamed before.
    Cancelled {
        content: String,
        reasoning: String,
    },
    Failed(ProviderError),
}

/// The arguments of a call as they arrived.
#[derive(Debug, Clone)]
enum RawArgs {
    /// Parsed JSON (any type; repair turns a double-encoded string into an object).
    Value(Value),
    /// Not JSON at all.
    Unreadable(String),
    /// Cut off by the output limit.
    CutOff,
    /// A recovered marker call that cannot run.
    Failed(String),
}

/// A call waiting to be processed.
struct Pending {
    record: CallRecord,
    verdict: Verdict,
}

/// The result of checking a call before anything runs.
enum Verdict {
    /// Ready for policy.
    Ready,
    /// Unknown tool, unreadable or invalid arguments: a mistake.
    Invalid(String),
    /// Cut off by the output limit.
    CutOff,
}

/// What processing one call means for the rest of the response and the turn.
enum Flow {
    Continue,
    /// Do not run the rest of this response.
    StopRest(String),
    /// The turn was cancelled.
    Stopped,
    MistakeBudget,
    LoopDetected,
    /// The same tool was cut off twice.
    OutputLimit(String),
}

/// One processed call.
struct Processed {
    result: ResultRecord,
    decision: CallDecision,
    action: Option<policy::Action>,
    full_text: Option<String>,
    ui: Option<Value>,
    flow: Flow,
}

impl Processed {
    fn synthetic(call: &CallRecord, status: CallStatus, notes: Vec<String>, flow: Flow) -> Self {
        let decision = match &status {
            CallStatus::Cancelled => CallDecision::Cancelled,
            CallStatus::Refused => CallDecision::Refused {
                reason: notes.first().cloned().unwrap_or_default(),
            },
            _ => CallDecision::NotEvaluated,
        };
        Self {
            result: ResultRecord::synthetic(call, status, notes),
            decision,
            action: None,
            full_text: None,
            ui: None,
            flow,
        }
    }
}

pub(super) struct Turn<'a> {
    deps: TurnDeps<'a>,
    input: TurnInput,
    cancel: CancellationToken,
    tools: Vec<ToolInfo>,
    tools_on: bool,
    specs: Vec<ToolSpec>,
    exact_counting: bool,
    system: String,
    /// The chat before this turn (messages covered by the summary removed).
    earlier: Vec<HistoryMessage>,
    /// Leading messages of `earlier` summarised during this turn.
    earlier_skip: usize,
    /// Compaction stage 1 applied.
    stub_earlier: bool,
    /// This turn's messages: the user's message, then everything the turn appends.
    turn: Vec<HistoryMessage>,
    /// Where the appended messages start in `turn`.
    new_from: usize,
    /// Calls of this turn whose results are stubbed (stage 3).
    stubbed: HashSet<String>,
    summary: Option<CompactionSummary>,
    summary_failed: bool,
    budget: ContextBudget,
    calibrator: TokenCalibrator,
    /// Correction for counts after the server rejected a request as too long.
    overflow_scale: f64,
    usage: UsageTotals,
    step: u32,
    mistakes: u32,
    mistakes_by_tool: HashMap<String, u32>,
    last_call: Option<(String, Value)>,
    repeats: u32,
    denied: Vec<(String, Value, Option<String>)>,
    untrusted: bool,
    cut_offs: HashMap<String, u32>,
    last_max_tokens: u32,
    /// The wrap-up instruction, appended to the last result in the wrap-up request only.
    wrap_note: Option<String>,
    truncation_noted: bool,
}

impl<'a> Turn<'a> {
    pub(super) fn new(deps: TurnDeps<'a>, mut input: TurnInput, cancel: CancellationToken) -> Self {
        let history = normalize(std::mem::take(&mut input.history));
        let covered: HashSet<i64> = input
            .summary
            .as_ref()
            .map(|summary| summary.covered_ids.iter().copied().collect())
            .unwrap_or_default();
        let split = history
            .iter()
            .rposition(|message| message.role == Role::User)
            .unwrap_or(history.len());
        let mut earlier = history;
        let turn = earlier.split_off(split);
        let earlier: Vec<HistoryMessage> = earlier
            .into_iter()
            .filter(|message| message.id.is_none_or(|id| !covered.contains(&id)))
            .collect();
        // Untrusted content already in the turn: an attachment, or the output of a call made
        // before the turn was interrupted and resumed (D9).
        let untrusted = turn.iter().any(|message| {
            message.parts.iter().any(|part| match part {
                Part::Attachment { .. } => true,
                Part::ToolResult(result) => !result.text.is_empty() || !result.images.is_empty(),
                _ => false,
            })
        });
        let tools = if input.caps.tools {
            deps.tools.tools()
        } else {
            Vec::new()
        };
        let tools_on = !tools.is_empty();
        let exact_counting =
            input.caps.local && deps.client.config().kind == ProviderKind::LlamaServer;
        let calibrator = input.calibrator.clone();
        let new_from = turn.len();
        let mut this = Self {
            deps,
            cancel,
            tools,
            tools_on,
            specs: Vec::new(),
            exact_counting,
            system: String::new(),
            earlier,
            earlier_skip: 0,
            stub_earlier: false,
            turn,
            new_from,
            stubbed: HashSet::new(),
            summary: input.summary.clone(),
            summary_failed: false,
            budget: ContextBudget::new(input.caps.n_ctx, 0),
            calibrator,
            overflow_scale: 1.0,
            usage: UsageTotals::default(),
            step: 0,
            mistakes: 0,
            mistakes_by_tool: HashMap::new(),
            last_call: None,
            repeats: 0,
            denied: Vec::new(),
            untrusted,
            cut_offs: HashMap::new(),
            last_max_tokens: 0,
            wrap_note: None,
            truncation_noted: false,
            input,
        };
        this.specs = this.tool_specs();
        this.rebuild_system();
        this
    }

    pub(super) async fn run(mut self) -> TurnOutcome {
        let status = self.run_inner().await;
        self.finish(status)
    }

    async fn run_inner(&mut self) -> TurnStatus {
        // The turn starts at the last user message; anything after it may only be the calls
        // and results of an interrupted turn, never an answer (that turn is over).
        let answered = self
            .turn
            .last()
            .is_some_and(|m| m.role == Role::Assistant && !m.has_tool_calls());
        if answered
            || self
                .turn
                .first()
                .is_none_or(|message| message.role != Role::User)
        {
            return failed(
                "no_user_message",
                "There is no message to answer: the chat does not end with a user message.",
            );
        }
        if let Err(status) = self.measure().await {
            return status;
        }
        loop {
            if self.cancel.is_cancelled() {
                return TurnStatus::Stopped;
            }
            self.step += 1;
            self.emit(TurnEvent::StepStarted { step: self.step });
            let reply = match self.request(ToolChoice::Auto, false).await {
                Ok(reply) => reply,
                Err(status) => return status,
            };
            match self.handle_reply(reply).await {
                Next::Continue => {}
                Next::End(status) => return status,
                Next::WrapUp(status) => return self.wrap_up(status).await,
            }
        }
    }

    fn finish(mut self, status: TurnStatus) -> TurnOutcome {
        let answered = self
            .turn
            .last()
            .is_some_and(|m| m.role == Role::Assistant && !m.has_tool_calls() && !m.is_blank());
        let has_user = self.turn.first().is_some_and(|m| m.role == Role::User);
        if !answered && has_user {
            self.append(HistoryMessage::stopped());
        }
        let outcome = TurnOutcome {
            status,
            messages: self.turn[self.new_from.min(self.turn.len())..].to_vec(),
            usage: self.usage,
            summary: self.summary.clone(),
            budget: Some(self.budget),
            calibrator: self.calibrator.clone(),
        };
        self.emit(TurnEvent::Finished {
            outcome: outcome.clone(),
        });
        outcome
    }

    // ----- events and messages -------------------------------------------------------------

    fn emit(&self, event: TurnEvent) {
        self.deps.sink.event(event);
    }

    fn append(&mut self, message: HistoryMessage) {
        self.emit(TurnEvent::MessageCompleted {
            message: message.clone(),
        });
        self.turn.push(message);
    }

    fn chars_per_token(&self) -> f64 {
        if self.exact_counting {
            BASELINE_CHARS_PER_TOKEN
        } else {
            self.calibrator.chars_per_token()
        }
    }

    // ----- prompt --------------------------------------------------------------------------

    fn tool_specs(&self) -> Vec<ToolSpec> {
        let llama = self.deps.client.config().kind == ProviderKind::LlamaServer;
        self.tools
            .iter()
            .map(|tool| {
                let schema = if llama {
                    self.deps
                        .tools
                        .llama_schema(&tool.name)
                        .unwrap_or_else(|| tool.input_schema.clone())
                } else {
                    tool.input_schema.clone()
                };
                let description: String = tool
                    .description
                    .chars()
                    .take(MAX_DESCRIPTION_CHARS)
                    .collect();
                ToolSpec::function(
                    tool.name.clone(),
                    self.deps.sanitizer.clean(&description),
                    sanitize_schema(&schema, self.deps.sanitizer),
                )
            })
            .collect()
    }

    fn rebuild_system(&mut self) {
        let preamble = self.tools_on.then(|| {
            self.input
                .preamble
                .clone()
                .unwrap_or_else(|| Preamble::for_this_os(Vec::new()))
        });
        self.system = system_message(
            &SystemParts {
                chat_prompt: &self.input.system_prompt,
                preamble: preamble.as_ref(),
                skills_index: self.input.skills_index.as_deref(),
                loaded_skills: &self.input.loaded_skills,
                summary: self.summary.as_ref().map(|s| s.text.as_str()),
            },
            self.deps.sanitizer,
        );
    }

    fn projection(&self) -> Projection {
        Projection {
            tools: self.input.caps.tools,
            vision: self.input.caps.vision,
            preserve_reasoning: self.input.caps.preserve_reasoning,
            images_in_tool_results: self.input.caps.images_in_tool_results,
        }
    }

    /// The messages to send: earlier turns (after the summary, stage-1 stubs applied) and
    /// this turn (stage-3 stubs applied), normalised.
    fn view(&self) -> Vec<HistoryMessage> {
        let cpt = self.chars_per_token();
        let mut messages = self.earlier[self.earlier_skip..].to_vec();
        if self.stub_earlier {
            stub_results(&mut messages, |_| true, cpt);
        }
        let mut turn = self.turn.clone();
        if !self.stubbed.is_empty() {
            stub_results(&mut turn, |call| self.stubbed.contains(&call.id), cpt);
        }
        // App notes for the turn (a regenerate's earlier side effects) belong to the turn's own
        // user message, after its attachments and outside every delimiter. Attached here, not
        // to "the last user-role message" of the projection, they stay in one place for the
        // whole turn even when flattened results or image messages follow (8.5 "Stable
        // prefix").
        if !self.input.app_notes.is_empty()
            && let Some(user) = turn.first_mut().filter(|m| m.role == Role::User)
        {
            let notes = self
                .input
                .app_notes
                .iter()
                .map(|note| format!("[App note] {}", self.deps.sanitizer.clean(note.trim())))
                .collect::<Vec<_>>()
                .join("\n");
            user.parts.push(Part::Text(notes));
        }
        messages.extend(turn);
        drop_old_result_images(&mut messages, KEPT_RESULT_IMAGES);
        normalize(messages)
    }

    fn build_request(
        &self,
        messages: Vec<ChatMessage>,
        choice: ToolChoice,
        thinking_off: bool,
    ) -> ChatRequest {
        let template = &self.input.template;
        let mut request = ChatRequest::new(template.model.clone(), messages);
        request.temperature = template.temperature;
        request.top_p = template.top_p;
        request.top_k = template.top_k;
        request.extra = template.extra.clone();
        if self.tools_on {
            request.tools = self.specs.clone();
            request.tool_choice = Some(choice);
            request.parallel_tool_calls = Some(false);
        }
        if self.input.caps.thinking && (thinking_off || !self.input.thinking) {
            disable_thinking(&mut request, self.deps.client.config().kind);
        }
        request
    }

    fn request_for_view(
        &self,
        view: &[HistoryMessage],
        choice: ToolChoice,
        thinking_off: bool,
    ) -> ChatRequest {
        let sanitizer = self.deps.sanitizer;
        let mut messages = project(view, &self.system, &self.projection(), sanitizer);
        if let Some(note) = &self.wrap_note {
            append_result_note(&mut messages, note, sanitizer);
        }
        self.build_request(messages, choice, thinking_off)
    }

    // ----- counting and compaction ---------------------------------------------------------

    /// Counts a request: exactly on the local llama-server, otherwise estimated and
    /// calibrated. Returns the count to budget with and the baseline estimate.
    async fn count(&self, request: &ChatRequest) -> Result<(u32, u32), TurnStatus> {
        let baseline = estimate_request(request);
        let mut count = None;
        if self.exact_counting {
            let counted = tokio::select! {
                biased;
                () = self.cancel.cancelled() => return Err(TurnStatus::Stopped),
                counted = self.deps.client.count_tokens(request) => counted,
            };
            // A failed count falls back to the estimate; the request itself reports real
            // problems.
            count = counted.ok().flatten();
        }
        let count = count.unwrap_or_else(|| self.calibrator.calibrate(baseline));
        Ok((scale(count, self.overflow_scale), baseline))
    }

    /// Measures the fixed overhead F and checks that the tools fit (ARCHITECTURE.md 8.4).
    async fn measure(&mut self) -> Result<(), TurnStatus> {
        let fixed = self.fixed_overhead(true).await?;
        self.budget = ContextBudget::new(self.input.caps.n_ctx, fixed);
        if self.tools_on && !self.budget.tools_fit() {
            let without = self.fixed_overhead(false).await?;
            let refusal = ToolsDoNotFit {
                tool_count: self.tools.len(),
                tool_tokens: fixed.saturating_sub(without),
                fixed_overhead: fixed,
                n_ctx: self.budget.n_ctx,
                budget: self.budget.budget,
            };
            return Err(failed("tools_do_not_fit", refusal.message()));
        }
        self.emit(TurnEvent::Budget {
            budget: self.budget,
        });
        Ok(())
    }

    async fn fixed_overhead(&self, with_tools: bool) -> Result<u32, TurnStatus> {
        // Some templates refuse a conversation without a user message; a one-character one
        // costs a token or two.
        let messages = vec![
            ChatMessage::system(self.system.clone()),
            ChatMessage::user("."),
        ];
        let mut probe = self.build_request(messages, ToolChoice::Auto, false);
        if !with_tools {
            probe.tools.clear();
        }
        Ok(self.count(&probe).await?.0)
    }

    /// Builds the request for the next model call, compacting until it fits under the
    /// trigger (or at least under the budget). Returns the request and its baseline estimate.
    async fn prepare(
        &mut self,
        choice: ToolChoice,
        thinking_off: bool,
    ) -> Result<(ChatRequest, u32, u32), TurnStatus> {
        let mut summaries = 0;
        loop {
            if self.cancel.is_cancelled() {
                return Err(TurnStatus::Stopped);
            }
            let view = self.view();
            let mut request = self.request_for_view(&view, choice, thinking_off);
            let (count, baseline) = self.count(&request).await?;
            if count > self.budget.trigger {
                if !self.stub_earlier && has_results(&self.earlier[self.earlier_skip..]) {
                    self.stub_earlier = true;
                    self.emit(TurnEvent::Compacted {
                        stage: 1,
                        summary: None,
                        covered_ids: Vec::new(),
                    });
                    continue;
                }
                // Each round summarises the oldest turns that fit in one summary request and
                // folds in the summary before it, so a long chat on a small window takes a
                // few rounds instead of one request that cannot fit.
                if summaries < MAX_SUMMARY_ROUNDS && self.summarise().await? {
                    summaries += 1;
                    continue;
                }
                if self.stub_next_turn_pair() {
                    self.emit(TurnEvent::Compacted {
                        stage: 3,
                        summary: None,
                        covered_ids: Vec::new(),
                    });
                    continue;
                }
                if count > self.budget.budget {
                    return Err(failed(
                        "context_full",
                        format!(
                            "This conversation no longer fits the model's context window of {} \
                             tokens, even after older tool results were shortened and earlier \
                             messages summarised (it needs about {}). Start a new chat, raise the \
                             context size, or turn some tools off.",
                            group_digits(u64::from(self.budget.n_ctx)),
                            group_digits(u64::from(count))
                        ),
                    ));
                }
            }
            let max_tokens = self.budget.max_tokens(count);
            request.max_tokens = Some(max_tokens);
            self.last_max_tokens = max_tokens;
            self.limit_thinking(&mut request, max_tokens, thinking_off);
            return Ok((request, baseline, count));
        }
    }

    /// Gives thinking its own budget, half the output limit, so a reasoning model cannot spend
    /// the whole reply allowance on thought (ARCHITECTURE.md 8.2, "Reasoning"). llama-server
    /// only; a budget set in the request template wins.
    fn limit_thinking(&self, request: &mut ChatRequest, max_tokens: u32, thinking_off: bool) {
        let thinking = self.input.caps.thinking && self.input.thinking && !thinking_off;
        if thinking
            && self.deps.client.config().kind == ProviderKind::LlamaServer
            && !request.extra.contains_key("reasoning_budget_tokens")
            && !request.extra.contains_key("thinking_budget_tokens")
        {
            request.set_extra(
                "reasoning_budget_tokens",
                Value::from((max_tokens / 2).max(128)),
            );
        }
    }

    /// Stage 3: stubs the oldest call group of this turn that is not stubbed yet, never the
    /// latest one. Returns false when there is none.
    fn stub_next_turn_pair(&mut self) -> bool {
        let groups = call_groups(&self.turn, 0);
        let Some((_, older)) = groups.split_last() else {
            return false;
        };
        for group in older {
            if group.iter().all(|id| self.stubbed.contains(id)) {
                continue;
            }
            let worth_it = self
                .turn
                .iter()
                .flat_map(|m| m.tool_results())
                .any(|result| {
                    group.contains(&result.call_id)
                        && (result.text.chars().count() >= 240 || !result.images.is_empty())
                })
                || self.turn.iter().flat_map(|m| m.tool_calls()).any(|call| {
                    group.contains(&call.id)
                        && Value::Object(call.arguments.clone()).to_string().len() > 400
                });
            self.stubbed.extend(group.iter().cloned());
            if worth_it {
                return true;
            }
        }
        false
    }

    /// Stage 2: summarises the oldest earlier turns with the same model. Returns whether a
    /// summary was made.
    async fn summarise(&mut self) -> Result<bool, TurnStatus> {
        if self.summary_failed {
            return Ok(false);
        }
        let cpt = self.chars_per_token();
        let mut earlier = self.earlier[self.earlier_skip..].to_vec();
        if earlier.is_empty() {
            return Ok(false);
        }
        if self.stub_earlier {
            stub_results(&mut earlier, |_| true, cpt);
        }
        let calibrate = |message: &HistoryMessage| -> u32 {
            if self.exact_counting {
                estimate_message(message)
            } else {
                self.calibrator.calibrate(estimate_message(message))
            }
        };
        let turn_tokens: u32 = self.turn.iter().map(calibrate).sum();
        let summary_cap = self.budget.summary_max_tokens();
        let keep = self
            .budget
            .trigger
            .saturating_sub(self.budget.fixed_overhead)
            .saturating_sub(turn_tokens)
            .saturating_sub(summary_cap);
        let wanted = summarisable_prefix(&earlier, keep, calibrate);
        if wanted == 0 {
            return Ok(false);
        }
        // The summary request carries the fixed overhead and the messages it summarises, so it
        // must fit the budget itself. Summarise the longest run of whole turns that does; the
        // next round (see `prepare`) folds this summary into one covering the rest.
        let room = u64::from(
            self.budget
                .budget
                .saturating_sub(self.budget.fixed_overhead),
        );
        let mut cuts: Vec<usize> = turn_starts(&earlier)
            .into_iter()
            .filter(|cut| *cut > 0 && *cut < wanted)
            .chain(std::iter::once(wanted))
            .collect();
        let mut tokens_before = 0u64;
        let mut next = 0;
        cuts.retain(|cut| {
            tokens_before += earlier[next..*cut]
                .iter()
                .map(|m| u64::from(calibrate(m)))
                .sum::<u64>();
            next = *cut;
            tokens_before <= room
        });
        let instruction = summary_instruction(summary_cap * 2 / 3, self.summary.is_some());
        let mut chosen = None;
        // The estimate picked the candidates; the count (exact on the local server) decides.
        for cut in cuts.into_iter().rev().take(MAX_SUMMARY_FIT_TRIES) {
            let request = self.summary_request(&earlier[..cut], &instruction, summary_cap);
            if self.count(&request).await?.0 <= self.budget.budget {
                chosen = Some((cut, request));
                break;
            }
        }
        let Some((prefix, request)) = chosen else {
            // Not even the oldest turn fits in a summary request.
            self.summary_failed = true;
            return Ok(false);
        };
        self.emit(TurnEvent::AppNote {
            text: "Summarising earlier messages to make room in the context window.".to_owned(),
        });
        let text = match self.stream(&request, false).await {
            Attempt::Done(reply) => strip_tool_markup(&reply.content),
            Attempt::Cancelled { .. } => return Err(TurnStatus::Stopped),
            Attempt::Failed(_) => String::new(),
        };
        if text.trim().is_empty() {
            self.summary_failed = true;
            return Ok(false);
        }
        let max_chars = usize::try_from(summary_cap)
            .unwrap_or(usize::MAX)
            .saturating_mul(4);
        let text: String = text.trim().chars().take(max_chars).collect();
        let mut covered: Vec<i64> = self
            .summary
            .as_ref()
            .map(|s| s.covered_ids.clone())
            .unwrap_or_default();
        covered.extend(
            self.earlier[self.earlier_skip..self.earlier_skip + prefix]
                .iter()
                .filter_map(|m| m.id),
        );
        self.earlier_skip += prefix;
        self.summary = Some(CompactionSummary {
            text: text.clone(),
            covered_ids: covered.clone(),
        });
        self.rebuild_system();
        self.emit(TurnEvent::Compacted {
            stage: 2,
            summary: Some(text),
            covered_ids: covered,
        });
        // The summary is part of the fixed overhead now.
        let fixed = self.fixed_overhead(true).await?;
        self.budget = ContextBudget::new(self.budget.n_ctx, fixed);
        self.emit(TurnEvent::Budget {
            budget: self.budget,
        });
        Ok(true)
    }

    /// The stage-2 summary request for `messages` (the oldest earlier messages): the same
    /// system message and tool list as the conversation (`tool_choice: none`, thinking off),
    /// the messages, then the fixed summary instruction in a user message.
    fn summary_request(
        &self,
        messages: &[HistoryMessage],
        instruction: &str,
        max_tokens: u32,
    ) -> ChatRequest {
        let to_summarise = normalize(messages.to_vec());
        let sanitizer = self.deps.sanitizer;
        let mut chat = project(&to_summarise, &self.system, &self.projection(), sanitizer);
        if chat.last().is_some_and(|m| m.role == Role::Tool) {
            chat.push(ChatMessage::assistant(STOPPED_TEXT));
        }
        match chat.last_mut() {
            Some(last) if last.role == Role::User => match &mut last.content {
                Some(MessageContent::Text(text)) => {
                    text.push_str("\n\n");
                    text.push_str(instruction);
                }
                // Keep the images of a user message; the instruction is one more part.
                Some(MessageContent::Parts(parts)) => parts.push(ContentPart::text(instruction)),
                None => last.content = Some(MessageContent::Text(instruction.to_owned())),
            },
            _ => chat.push(ChatMessage::user(instruction)),
        }
        let mut request = self.build_request(chat, ToolChoice::None, true);
        request.max_tokens = Some(max_tokens);
        request
    }

    fn on_overflow(&mut self, n_prompt_tokens: Option<u32>, n_ctx: Option<u32>, sent: u32) {
        if let Some(server_ctx) = n_ctx.filter(|n| *n > 0 && *n < self.budget.n_ctx) {
            self.budget = ContextBudget::new(server_ctx, self.budget.fixed_overhead);
            self.emit(TurnEvent::Budget {
                budget: self.budget,
            });
        }
        let factor = match n_prompt_tokens {
            Some(actual) if sent > 0 && actual > sent => f64::from(actual) / f64::from(sent),
            _ => 1.25,
        };
        self.overflow_scale = (self.overflow_scale * factor.max(1.1)).min(8.0);
    }

    // ----- model calls ---------------------------------------------------------------------

    /// One model call with its recovery: a retry with thinking disabled after an empty reply,
    /// a server parse error or a reply that only thought; a forced compaction and one retry
    /// after a context overflow.
    async fn request(&mut self, choice: ToolChoice, wrap_up: bool) -> Result<Reply, TurnStatus> {
        let mut thinking_off = false;
        let mut retried = false;
        let mut overflow_retried = false;
        loop {
            let (request, baseline, sent) = self.prepare(choice, thinking_off).await?;
            // The wrap-up reply is shown only once its tool-call markup is stripped.
            match self.stream(&request, !wrap_up).await {
                Attempt::Cancelled { content, reasoning } => {
                    // A wrap-up reply is never shown or stored with its tool-call markup.
                    let content = if wrap_up {
                        strip_tool_markup(&content)
                    } else {
                        content
                    };
                    if !content.trim().is_empty() {
                        let mut message = HistoryMessage::assistant(content);
                        message.reasoning = non_empty(reasoning);
                        self.append(message);
                    }
                    return Err(TurnStatus::Stopped);
                }
                Attempt::Failed(ProviderError::ContextOverflow {
                    n_prompt_tokens,
                    n_ctx,
                }) if !overflow_retried => {
                    overflow_retried = true;
                    self.on_overflow(n_prompt_tokens, n_ctx, sent);
                    self.emit(TurnEvent::AppNote {
                        text: "The server said the request was too long; compacting and trying once more."
                            .to_owned(),
                    });
                }
                Attempt::Failed(ProviderError::ServerParseError(detail)) => {
                    if !wrap_up {
                        self.mistakes += 1;
                    }
                    if retried {
                        return Err(failed("no_answer", NO_ANSWER_MESSAGE));
                    }
                    retried = true;
                    thinking_off = true;
                    self.emit(TurnEvent::AttemptDiscarded {
                        reason: format!("the server could not parse the model output: {detail}"),
                    });
                }
                Attempt::Failed(err) => return Err(provider_failure(&err)),
                Attempt::Done(reply) => {
                    if let Some(usage) = reply.usage {
                        self.observe(baseline, usage);
                    }
                    let no_content = reply.content.trim().is_empty() && reply.calls.is_empty();
                    if !no_content {
                        return Ok(reply);
                    }
                    if retried {
                        return Err(failed("no_answer", NO_ANSWER_MESSAGE));
                    }
                    retried = true;
                    thinking_off = true;
                    let reason = if reply.finish == Some(FinishReason::Length) {
                        "the reply hit the output limit while thinking"
                    } else {
                        "the model returned an empty reply"
                    };
                    self.emit(TurnEvent::AttemptDiscarded {
                        reason: reason.to_owned(),
                    });
                }
            }
        }
    }

    /// Learns the endpoint's characters per token from its reported prompt size.
    fn observe(&mut self, baseline: u32, usage: Usage) {
        if self.exact_counting || usage.prompt_tokens == 0 {
            return;
        }
        let truncated = TokenCalibrator::looks_truncated(baseline, usage.prompt_tokens);
        if truncated && !self.truncation_noted {
            self.truncation_noted = true;
            self.emit(TurnEvent::AppNote {
                text: format!(
                    "The server reported a prompt of {} tokens for about {} sent; it may be \
                     dropping old messages because its context window is too small.",
                    group_digits(u64::from(usage.prompt_tokens)),
                    group_digits(u64::from(self.calibrator.calibrate(baseline)))
                ),
            });
        }
        self.calibrator
            .observe(baseline, usage.prompt_tokens, truncated);
    }

    /// Streams one response. With `visible`, deltas go to the sink.
    async fn stream(&mut self, request: &ChatRequest, visible: bool) -> Attempt {
        let token = self.cancel.child_token();
        let opened = tokio::select! {
            biased;
            () = self.cancel.cancelled() => return Attempt::Cancelled { content: String::new(), reasoning: String::new() },
            opened = self.deps.client.stream_chat(request, token.clone()) => opened,
        };
        self.usage.requests += 1;
        let mut stream = match opened {
            Ok(stream) => stream,
            Err(ProviderError::Cancelled) => {
                return Attempt::Cancelled {
                    content: String::new(),
                    reasoning: String::new(),
                };
            }
            Err(err) => return Attempt::Failed(err),
        };
        let mut reply = Reply::default();
        let mut accumulator = ToolCallAccumulator::new();
        let mut timings: Option<Timings> = None;
        loop {
            let item = tokio::select! {
                biased;
                () = self.cancel.cancelled() => None,
                item = stream.next() => Some(item),
            };
            let Some(item) = item else {
                return Attempt::Cancelled {
                    content: reply.content,
                    reasoning: reply.reasoning,
                };
            };
            match item {
                None | Some(Ok(StreamEvent::Done)) => break,
                Some(Ok(StreamEvent::Content(text))) => {
                    if visible {
                        self.emit(TurnEvent::ContentDelta { text: text.clone() });
                    }
                    reply.content.push_str(&text);
                }
                Some(Ok(StreamEvent::Reasoning(text))) => {
                    if visible {
                        self.emit(TurnEvent::ReasoningDelta { text: text.clone() });
                    }
                    reply.reasoning.push_str(&text);
                }
                Some(Ok(StreamEvent::Usage(usage))) => reply.usage = Some(usage),
                Some(Ok(StreamEvent::Timings(t))) => timings = Some(t),
                Some(Ok(event)) => accumulator.push(&event),
                Some(Err(ProviderError::Cancelled)) => {
                    return Attempt::Cancelled {
                        content: reply.content,
                        reasoning: reply.reasoning,
                    };
                }
                Some(Err(err)) => return Attempt::Failed(err),
            }
        }
        drop(stream);
        reply.finish = accumulator.finish_reason().cloned();
        reply.calls = accumulator.finish();
        if let Some(usage) = reply.usage {
            self.usage.prompt_tokens += u64::from(usage.prompt_tokens);
            self.usage.completion_tokens += u64::from(usage.completion_tokens);
            self.usage.cached_tokens += u64::from(usage.cached_tokens.unwrap_or(0));
            self.usage.last_prompt_tokens = Some(usage.prompt_tokens);
        }
        if timings.is_some() {
            self.usage.last_timings = timings;
        }
        if let Some(usage) = reply.usage {
            self.emit(TurnEvent::Usage { usage, timings });
        }
        Attempt::Done(reply)
    }

    // ----- responses -----------------------------------------------------------------------

    async fn handle_reply(&mut self, reply: Reply) -> Next {
        let Reply {
            content,
            reasoning,
            calls,
            finish,
            usage,
        } = reply;
        let reasoning = non_empty(reasoning);
        let output_tokens = usage
            .map(|u| u.completion_tokens)
            .filter(|n| *n > 0)
            .unwrap_or(self.last_max_tokens);
        if !calls.is_empty() {
            let pending: Vec<Pending> = calls
                .into_iter()
                .map(|call| self.check_native(call))
                .collect();
            return self
                .run_calls(content, reasoning, pending, output_tokens)
                .await;
        }
        if self.tools_on {
            if finish == Some(FinishReason::Length)
                && let Some(cut) = cut_off_call(&content)
            {
                let record = CallRecord {
                    id: new_call_id(),
                    name: cut.name.unwrap_or_else(|| UNKNOWN_TOOL_NAME.to_owned()),
                    arguments: Map::new(),
                    recovered: true,
                };
                let pending = vec![Pending {
                    record,
                    verdict: Verdict::CutOff,
                }];
                return self
                    .run_calls(cut.text_before, reasoning, pending, output_tokens)
                    .await;
            }
            let recovery = self.recover(&content);
            if !recovery.calls.is_empty() {
                let pending: Vec<Pending> = recovery
                    .calls
                    .into_iter()
                    .map(|recovered| self.check_recovered(recovered))
                    .collect();
                return self
                    .run_calls(recovery.text, reasoning, pending, output_tokens)
                    .await;
            }
        }
        let mut message = HistoryMessage::assistant(content);
        message.reasoning = reasoning;
        self.append(message);
        Next::End(TurnStatus::Completed)
    }

    /// The fallback parser over the content (never the reasoning), with the untrusted texts
    /// still in context.
    fn recover(&self, content: &str) -> crate::parse::Recovery {
        let tools = self.deps.tools;
        let validate = move |name: &str, args: &Value| tools.validate(name, args);
        let mut context = RecoveryContext::new(&validate);
        for message in self.earlier[self.earlier_skip..]
            .iter()
            .chain(self.turn.iter())
        {
            for part in &message.parts {
                match part {
                    Part::Text(text) if message.role == Role::User => context.add_text(text),
                    Part::Attachment { text, .. } => context.add_text(text),
                    Part::ToolResult(result) => context.add_text(&result.text),
                    _ => {}
                }
            }
        }
        recover_tool_calls(content, &self.tools, &context)
    }

    fn tool(&self, name: &str) -> Option<&ToolInfo> {
        self.tools.iter().find(|tool| tool.name == name)
    }

    /// Checks a native call: known tool, readable arguments, repair, validation.
    fn check_native(&self, call: AccumulatedToolCall) -> Pending {
        let raw = match &call.status {
            ArgumentsStatus::Truncated => RawArgs::CutOff,
            ArgumentsStatus::Object | ArgumentsStatus::NotObject { .. } => {
                match serde_json::from_str::<Value>(&call.call.function.arguments) {
                    Ok(value) => RawArgs::Value(value),
                    Err(err) => RawArgs::Unreadable(format!(
                        "The arguments are not valid JSON ({err}). Send a JSON object that \
                         matches the tool's schema."
                    )),
                }
            }
        };
        // Ids must be unique within the chat; some servers number calls per response.
        let taken = self
            .earlier
            .iter()
            .chain(self.turn.iter())
            .flat_map(HistoryMessage::tool_calls)
            .any(|existing| existing.id == call.call.id);
        let id = if taken || call.call.id.is_empty() {
            new_call_id()
        } else {
            call.call.id
        };
        self.check(id, call.call.function.name, raw, false)
    }

    fn check_recovered(&self, recovered: Recovered) -> Pending {
        match recovered {
            Recovered::Valid { name, arguments } => self.check(
                new_call_id(),
                name,
                RawArgs::Value(Value::Object(arguments)),
                true,
            ),
            Recovered::Failed {
                name,
                arguments,
                error,
            } => {
                let mut pending = self.check(new_call_id(), name, RawArgs::Failed(error), true);
                pending.record.arguments = arguments;
                pending
            }
        }
    }

    fn check(&self, id: String, name: String, raw: RawArgs, recovered: bool) -> Pending {
        let stored_name = if name.trim().is_empty() {
            UNKNOWN_TOOL_NAME.to_owned()
        } else {
            name.clone()
        };
        let mut record = CallRecord {
            id,
            name: stored_name.clone(),
            arguments: Map::new(),
            recovered,
        };
        let verdict = match raw {
            RawArgs::CutOff => Verdict::CutOff,
            RawArgs::Failed(error) => Verdict::Invalid(error),
            RawArgs::Unreadable(error) => match self.tool(&name) {
                Some(_) => Verdict::Invalid(error),
                None => Verdict::Invalid(self.no_such_tool(&stored_name)),
            },
            RawArgs::Value(value) => match self.tool(&name) {
                None => {
                    record.arguments = value.as_object().cloned().unwrap_or_default();
                    Verdict::Invalid(self.no_such_tool(&stored_name))
                }
                Some(tool) => match repair_arguments(value, &tool.input_schema) {
                    Value::Object(arguments) => {
                        let value = Value::Object(arguments.clone());
                        record.arguments = arguments;
                        match self.deps.tools.validate(&name, &value) {
                            Ok(()) => Verdict::Ready,
                            Err(errors) => Verdict::Invalid(format!(
                                "Invalid arguments for {name}: {}. The tool was not run; fix the \
                                 arguments and call it again.",
                                errors.join("; ")
                            )),
                        }
                    }
                    other => Verdict::Invalid(format!(
                        "The arguments of {name} are a JSON {}, not an object. Send a JSON object \
                         that matches the tool's schema.",
                        json_kind(&other)
                    )),
                },
            },
        };
        Pending { record, verdict }
    }

    fn no_such_tool(&self, name: &str) -> String {
        let names: Vec<&str> = self.tools.iter().map(|tool| tool.name.as_str()).collect();
        format!("No such tool: {name}; available: {}.", names.join(", "))
    }

    /// Runs the calls of one response in order.
    async fn run_calls(
        &mut self,
        text: String,
        reasoning: Option<String>,
        pending: Vec<Pending>,
        output_tokens: u32,
    ) -> Next {
        let records: Vec<CallRecord> = pending.iter().map(|p| p.record.clone()).collect();
        let mut message = HistoryMessage::assistant_calls(text, records);
        message.reasoning = reasoning;
        self.append(message);

        let at_cap = self.step >= self.input.step_cap.max(1);
        let mut next = if at_cap {
            Next::WrapUp(TurnStatus::StepCapReached)
        } else {
            Next::Continue
        };
        let mut skip_reason: Option<String> = at_cap.then(|| {
            format!(
                "Not run: this turn reached its limit of {} model calls.",
                self.input.step_cap
            )
        });
        if skip_reason.is_none() && self.mistakes >= MAX_MISTAKES {
            skip_reason = Some("Not run: the turn is ending after too many mistakes.".to_owned());
            next = Next::WrapUp(TurnStatus::MistakeBudget);
        }
        for Pending { record, verdict } in pending {
            self.emit(TurnEvent::ToolCallStarted {
                call: record.clone(),
            });
            let started = Instant::now();
            let processed = if let Some(reason) = &skip_reason {
                Processed::synthetic(
                    &record,
                    CallStatus::NotRun,
                    vec![reason.clone()],
                    Flow::Continue,
                )
            } else if self.cancel.is_cancelled() {
                Processed::synthetic(&record, CallStatus::Cancelled, Vec::new(), Flow::Stopped)
            } else {
                self.process(&record, verdict, output_tokens).await
            };
            let Processed {
                result,
                decision,
                action,
                full_text,
                ui,
                flow,
            } = processed;
            self.emit(TurnEvent::ToolCallFinished {
                result: result.clone(),
                decision,
                action,
                full_text,
                ui,
                duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            });
            self.append(HistoryMessage::tool_result(result));
            match flow {
                Flow::Continue => {}
                Flow::StopRest(reason) => {
                    skip_reason = Some(reason);
                }
                Flow::Stopped => {
                    skip_reason = Some("Not run: the turn was stopped.".to_owned());
                    next = Next::End(TurnStatus::Stopped);
                }
                Flow::MistakeBudget => {
                    skip_reason = Some(
                        "Not run: the turn is ending after too many invalid calls.".to_owned(),
                    );
                    next = Next::WrapUp(TurnStatus::MistakeBudget);
                }
                Flow::LoopDetected => {
                    skip_reason = Some(
                        "Not run: the turn is ending because the same call was repeated."
                            .to_owned(),
                    );
                    next = Next::WrapUp(TurnStatus::LoopDetected);
                }
                Flow::OutputLimit(message) => {
                    skip_reason = Some("Not run: the turn ended at the output limit.".to_owned());
                    next = Next::End(failed("output_limit", message));
                }
            }
        }
        next
    }

    /// Counts a mistake against `tool`; true when the budget is spent.
    fn mistake(&mut self, tool: &str) -> bool {
        self.mistakes += 1;
        let by_tool = self.mistakes_by_tool.entry(tool.to_owned()).or_insert(0);
        *by_tool += 1;
        *by_tool >= MAX_INVALID_CALLS_PER_TOOL || self.mistakes >= MAX_MISTAKES
    }

    fn mistake_flow(
        &mut self,
        tool: &str,
        mut notes: Vec<String>,
        record: &CallRecord,
    ) -> Processed {
        let spent = self.mistake(tool);
        if spent {
            notes.push("Too many invalid calls in a row; the turn is ending.".to_owned());
        }
        Processed::synthetic(
            record,
            CallStatus::Error,
            notes,
            if spent {
                Flow::MistakeBudget
            } else {
                Flow::Continue
            },
        )
    }

    async fn process(
        &mut self,
        record: &CallRecord,
        verdict: Verdict,
        output_tokens: u32,
    ) -> Processed {
        let name = record.name.clone();
        match verdict {
            Verdict::Invalid(error) => return self.mistake_flow(&name, vec![error], record),
            Verdict::CutOff => {
                let count = self.cut_offs.entry(name.clone()).or_insert(0);
                *count += 1;
                if *count >= 2 {
                    let message = format!(
                        "The model's reply hit the output limit ({} tokens) twice while writing a \
                         call to {name}, so the turn ended. Ask for smaller steps, or raise the \
                         context size.",
                        group_digits(u64::from(output_tokens))
                    );
                    return Processed::synthetic(
                        record,
                        CallStatus::Error,
                        vec![format!(
                            "Your call was cut off again after {} tokens and was not run.",
                            group_digits(u64::from(output_tokens))
                        )],
                        Flow::OutputLimit(message),
                    );
                }
                let note = format!(
                    "Your call was cut off after {} tokens, so it was not run. Write the file in \
                     smaller pieces or use edit_file.",
                    group_digits(u64::from(output_tokens))
                );
                return self.mistake_flow(&name, vec![note], record);
            }
            Verdict::Ready => {}
        }
        let args = record.arguments.clone();
        let key = (name.clone(), Value::Object(args.clone()));

        // A call identical to one the user denied earlier in the turn.
        if let Some((_, _, note)) = self
            .denied
            .iter()
            .find(|(n, a, _)| *n == key.0 && *a == key.1)
        {
            let mut text =
                "You made this exact call earlier in this turn and the user denied it.".to_owned();
            if let Some(note) = note {
                text.push_str(&format!(" Their note: {note}"));
            }
            let mut processed =
                Processed::synthetic(record, CallStatus::Refused, vec![text], Flow::Continue);
            processed.decision = CallDecision::Refused {
                reason: "denied earlier in this turn".to_owned(),
            };
            processed.flow = Flow::StopRest("Not run: the call before it was refused.".to_owned());
            return processed;
        }

        // Loop detection.
        if self.last_call.as_ref() == Some(&key) {
            self.repeats += 1;
        } else {
            self.last_call = Some(key.clone());
            self.repeats = 1;
        }
        if self.repeats >= LOOP_STOP_AT {
            return Processed::synthetic(
                record,
                CallStatus::NotRun,
                vec![format!(
                    "Not run: this is the same call with the same arguments {} times in a row, so \
                     the turn is ending.",
                    self.repeats
                )],
                Flow::LoopDetected,
            );
        }
        let loop_note = (self.repeats >= LOOP_WARNING_AT).then(|| {
            format!(
                "You have made this same call with the same arguments {} times in a row. Do \
                 something different, or answer with what you have.",
                self.repeats
            )
        });

        // Describe.
        let described = tokio::select! {
            biased;
            () = self.cancel.cancelled() => {
                return Processed::synthetic(record, CallStatus::Cancelled, Vec::new(), Flow::Stopped);
            }
            described = self.deps.tools.describe(&name, &args) => described,
        };
        let action = match described {
            mcp_host::Described::Action(action) => action,
            mcp_host::Described::Invalid(message) => {
                let mut notes = vec![message];
                notes.extend(loop_note);
                return Processed::synthetic(record, CallStatus::Error, notes, Flow::Continue);
            }
        };

        // Policy.
        let decision = self
            .deps
            .policy
            .decide(&name, &action, record.recovered, self.untrusted);
        let decision = match decision {
            policy::Decision::Refused { reason } => {
                let mut processed = Processed::synthetic(
                    record,
                    CallStatus::Refused,
                    vec![format!("The tool is not available: {reason}")],
                    Flow::Continue,
                );
                processed.decision = CallDecision::Refused { reason };
                processed.action = Some(action);
                return processed;
            }
            policy::Decision::Denied { reason } => {
                let mut processed = Processed::synthetic(
                    record,
                    CallStatus::Refused,
                    vec![format!("Blocked without asking: {reason}")],
                    Flow::Continue,
                );
                processed.decision = CallDecision::Refused { reason };
                processed.action = Some(action);
                return processed;
            }
            policy::Decision::AllowedByGrant => CallDecision::AllowedByGrant,
            policy::Decision::Ask { card, reason } => {
                let request = ApprovalRequest {
                    call_id: record.id.clone(),
                    tool: name.clone(),
                    action: action.clone(),
                    card,
                    reason,
                };
                self.emit(TurnEvent::ApprovalPending {
                    request: request.clone(),
                });
                let approval = tokio::select! {
                    biased;
                    () = self.cancel.cancelled() => None,
                    approval = self.deps.approver.ask(request) => Some(approval),
                };
                let approval = match approval.unwrap_or(Approval::Cancelled) {
                    // A once-only card (always-ask risk, a recovered call) has no "for this
                    // chat" button: such an answer can only be a bug or a forged reply, and
                    // it must never be recorded as a grant.
                    Approval::AllowForChat if card == policy::CardKind::OnceOnly => {
                        Approval::AllowOnce
                    }
                    other => other,
                };
                self.emit(TurnEvent::ApprovalResolved {
                    call_id: record.id.clone(),
                    approval: approval.clone(),
                });
                match approval {
                    Approval::AllowOnce => CallDecision::AllowedOnce,
                    Approval::AllowForChat => CallDecision::AllowedForChat,
                    Approval::Deny { note } => {
                        let note = note.filter(|n| !n.trim().is_empty());
                        self.denied.push((key.0, key.1, note.clone()));
                        let mut processed = Processed::synthetic(
                            record,
                            CallStatus::Denied { note: note.clone() },
                            Vec::new(),
                            Flow::StopRest("Not run: the call before it was denied.".to_owned()),
                        );
                        processed.decision = CallDecision::Denied { note };
                        processed.action = Some(action);
                        return processed;
                    }
                    Approval::Cancelled => {
                        let flow = if self.cancel.is_cancelled() {
                            Flow::Stopped
                        } else {
                            Flow::StopRest("Not run: the call before it was cancelled.".to_owned())
                        };
                        let mut processed =
                            Processed::synthetic(record, CallStatus::Cancelled, Vec::new(), flow);
                        processed.action = Some(action);
                        return processed;
                    }
                }
            }
        };

        // Run.
        if self.cancel.is_cancelled() {
            let mut processed =
                Processed::synthetic(record, CallStatus::Cancelled, Vec::new(), Flow::Stopped);
            processed.action = Some(action);
            return processed;
        }
        let (output, cancelled) = {
            let token = self.cancel.child_token();
            let mut execution =
                std::pin::pin!(self.deps.tools.execute(&name, &args, &action, token));
            tokio::select! {
                biased;
                output = &mut execution => (Some(output), false),
                () = self.cancel.cancelled() => {
                    // The tool got the cancellation through its token; give it a moment to
                    // return what it has, then stop waiting.
                    (tokio::time::timeout(CANCEL_GRACE, &mut execution).await.ok(), true)
                }
            }
        };
        // A tool that honours its token returns as the turn is cancelled: still a cancellation.
        let cancelled = cancelled || self.cancel.is_cancelled();
        self.untrusted = true;
        let cpt = self.chars_per_token();
        let cap = self.budget.result_cap;
        let mut result = ResultRecord::synthetic(
            record,
            if cancelled {
                CallStatus::Cancelled
            } else {
                CallStatus::Ok
            },
            Vec::new(),
        );
        result.source = source_of(&action);
        let mut full_text = None;
        let mut ui = None;
        if let Some(output) = output {
            let mut text = String::new();
            for block in &output.content {
                match block {
                    mcp_host::Content::Text { text: block } => {
                        if !text.is_empty() {
                            text.push('\n');
                        }
                        text.push_str(block);
                    }
                    mcp_host::Content::Image { mime, data_base64 } => {
                        result.images.push((mime.clone(), data_base64.clone()));
                    }
                }
            }
            // The per-result cap holds for every tool; one that shortened its own output keeps
            // its full copy for the card.
            full_text.clone_from(&output.full_text);
            if let Some(cut) = truncate_head_tail(&text, cap, cpt) {
                let original = std::mem::replace(&mut text, cut.text);
                full_text.get_or_insert(original);
            }
            result.text = text;
            ui = output.ui;
            if !cancelled {
                if output.is_error {
                    result.status = CallStatus::Error;
                } else {
                    // A successful call resets the mistake budget.
                    self.mistakes = 0;
                    self.mistakes_by_tool.clear();
                }
            }
        }
        result.notes.extend(loop_note);
        Processed {
            result,
            decision: if cancelled {
                CallDecision::Cancelled
            } else {
                decision
            },
            action: Some(action),
            full_text,
            ui,
            flow: if cancelled {
                Flow::Stopped
            } else {
                Flow::Continue
            },
        }
    }

    /// The wrap-up after an early stop: tools stay offered but `tool_choice` is `none`, and
    /// the model is asked to say what it did and what is left.
    async fn wrap_up(&mut self, status: TurnStatus) -> TurnStatus {
        if self.cancel.is_cancelled() {
            return TurnStatus::Stopped;
        }
        let why = match status {
            TurnStatus::StepCapReached => format!(
                "This turn reached its limit of {} model calls.",
                self.input.step_cap
            ),
            TurnStatus::MistakeBudget => {
                "This turn is ending because of too many invalid tool calls.".to_owned()
            }
            TurnStatus::LoopDetected => {
                "This turn is ending because the same tool call was repeated.".to_owned()
            }
            _ => "This turn is ending.".to_owned(),
        };
        self.wrap_note = Some(format!(
            "{why} Do not call any more tools. Tell the user what you did, what is left to do, \
             and anything they should check."
        ));
        let reply = self.request(ToolChoice::None, true).await;
        self.wrap_note = None;
        match reply {
            Ok(reply) => {
                let text = strip_tool_markup(&reply.content);
                if !reply.reasoning.trim().is_empty() {
                    self.emit(TurnEvent::ReasoningDelta {
                        text: reply.reasoning.clone(),
                    });
                }
                if !text.is_empty() {
                    self.emit(TurnEvent::ContentDelta { text: text.clone() });
                }
                let mut message = HistoryMessage::assistant(if text.is_empty() {
                    STOPPED_TEXT.to_owned()
                } else {
                    text
                });
                message.reasoning = non_empty(reply.reasoning);
                self.append(message);
                status
            }
            Err(TurnStatus::Stopped) => TurnStatus::Stopped,
            // The wrap-up failing does not hide why the turn stopped.
            Err(_) => status,
        }
    }
}

fn failed(code: &str, message: impl Into<String>) -> TurnStatus {
    TurnStatus::Failed {
        code: code.to_owned(),
        message: message.into(),
    }
}

fn provider_failure(err: &ProviderError) -> TurnStatus {
    let code = match err {
        ProviderError::Http { .. } => "http",
        ProviderError::ContextOverflow { .. } => "context_overflow",
        ProviderError::Unauthorized => "unauthorized",
        ProviderError::ServerParseError(_) => "model_output_unparseable",
        ProviderError::Network(_) => "network",
        ProviderError::Decode(_) => "decode",
        ProviderError::Cancelled => "cancelled",
        ProviderError::InvalidRequest(_) => "invalid_request",
    };
    failed(code, err.user_message())
}

/// Switches thinking off for one request, in the form the server understands
/// (ARCHITECTURE.md 8.3, step 3).
fn disable_thinking(request: &mut ChatRequest, kind: ProviderKind) {
    match kind {
        ProviderKind::LlamaServer => {
            request.set_chat_template_kwarg("enable_thinking", Value::Bool(false));
        }
        ProviderKind::Ollama => {
            request.set_extra("reasoning_effort", Value::String("none".to_owned()));
        }
        // Elsewhere a plain retry.
        ProviderKind::LmStudio | ProviderKind::Vllm | ProviderKind::Generic => {}
    }
}

fn scale(count: u32, factor: f64) -> u32 {
    if factor <= 1.0 {
        return count;
    }
    let scaled = (f64::from(count) * factor).ceil();
    if scaled >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let scaled = scaled as u32;
        scaled
    }
}

fn non_empty(text: String) -> Option<String> {
    (!text.trim().is_empty()).then_some(text)
}

fn new_call_id() -> String {
    format!("call_{}", uuid::Uuid::new_v4().simple())
}

fn has_results(messages: &[HistoryMessage]) -> bool {
    messages.iter().any(|m| m.tool_results().next().is_some())
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The path or URL a result came from, as the describer recorded it in the action's payload.
fn source_of(action: &policy::Action) -> Option<String> {
    ["source", "path", "url", "canonicalPath", "canonical_path"]
        .iter()
        .find_map(|key| action.payload.get(*key).and_then(Value::as_str))
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
