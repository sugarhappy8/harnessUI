//! Context management (ARCHITECTURE.md 8.4): the budget, the per-result cap, the "tools do
//! not fit" check, compaction helpers and truncation.
//!
//! **Numbers.** For an allocated context `n_ctx`:
//!
//! - reply reserve = min(4,096, `n_ctx` / 4);
//! - safety margin = 64 + `n_ctx` / 256 (the "small margin"; it reproduces the 8.4 table);
//! - budget = `n_ctx` − reply reserve − margin;
//! - trigger = 80% of the budget;
//! - per-result cap = min(8,000, 20% of `n_ctx`, (budget − F) / 3), where F is the fixed
//!   overhead (system prompt, preamble, skills, tool schemas);
//! - tools fit when F ≤ 40% of the budget, not counting pinned instructions;
//! - pinned instructions fit when they take ≤ 25% of the budget. They are part of F (they
//!   are in the system message of every request), so compaction never removes them.
//!
//! **Compaction** happens in three stages when the next request would pass the trigger; the
//! loop ([`crate::turn`]) drives them with these helpers:
//!
//! 1. [`stub_results`] over earlier turns: tool results become one-line stubs;
//! 2. summarise the oldest turns ([`summary_instruction`], [`summarisable_prefix`]);
//! 3. [`stub_results`] over this turn's own results, oldest first, keeping the user's message
//!    and the latest call and result.
//!
//! A summary is cut to [`ContextBudget::summary_max_chars`]. One written when the model had a
//! larger context can be longer than a smaller context allows; before the turn measures
//! anything it is folded again under the smaller cap ([`fold_instruction`]), or, when the
//! model cannot do that, cut to it ([`cut_summary`]).
//!
//! A stubbed call keeps its name and arguments (long string values shortened) and its result
//! becomes a note with the status, the size and "call again to re-read"; a call and its
//! result are always replaced together. Independently, only the newest
//! [`KEPT_RESULT_IMAGES`] tool-result images stay in context ([`drop_old_result_images`]).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::estimate::estimate_tokens;
use crate::history::{CallRecord, CallStatus, HistoryMessage, Part};
use crate::types::{ChatRequest, MessageContent};

/// The largest reply reserve, in tokens.
pub const MAX_REPLY_RESERVE: u32 = 4_096;
/// The largest per-result cap, in tokens.
pub const MAX_RESULT_CAP: u32 = 8_000;
/// Compaction starts when a request would use more than this share of the budget (percent).
pub const TRIGGER_PERCENT: u32 = 80;
/// Tools are offered only when the fixed overhead is at most this share of the budget
/// (percent).
pub const TOOLS_FIT_PERCENT: u32 = 40;
/// The user's pinned instructions may take at most this share of the budget (percent).
/// They have a share of their own, beside the one for tools, so a pin never takes room
/// from the tool list that was chosen to fit.
pub const PINNED_FIT_PERCENT: u32 = 25;
/// Tokens counted for one image when estimating (the server counts exactly).
pub const IMAGE_TOKENS_ESTIMATE: u32 = 1_024;
/// Tokens counted per message for the template's own markup when estimating.
pub const MESSAGE_OVERHEAD_TOKENS: u32 = 4;
/// The beginning of every stub note, which also marks a result as already stubbed.
pub const STUB_PREFIX: &str = "Earlier result removed to save space:";
/// Results shorter than this many characters are not worth stubbing.
const MIN_STUB_CHARS: usize = 240;
/// String arguments longer than this many characters are shortened in a stubbed call.
const MAX_STUB_ARGUMENT_CHARS: usize = 160;

/// The reply reserve for a context: min(4,096, a quarter of the context).
pub fn reply_reserve(n_ctx: u32) -> u32 {
    MAX_REPLY_RESERVE.min(n_ctx / 4)
}

/// The safety margin kept free besides the reply reserve.
pub fn safety_margin(n_ctx: u32) -> u32 {
    64 + n_ctx / 256
}

fn percent(value: u32, percent: u32) -> u32 {
    u32::try_from(u64::from(value) * u64::from(percent) / 100).unwrap_or(u32::MAX)
}

/// The context numbers for one model and one set of tools (module documentation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextBudget {
    /// The allocated context, in tokens.
    pub n_ctx: u32,
    /// Tokens kept for the reply.
    pub reply_reserve: u32,
    /// The safety margin.
    pub margin: u32,
    /// What a prompt may use.
    pub budget: u32,
    /// The fixed overhead F: system prompt, preamble, skills and tool schemas.
    pub fixed_overhead: u32,
    /// Compaction starts above this.
    pub trigger: u32,
    /// The most tokens one tool result may take.
    pub result_cap: u32,
}

impl ContextBudget {
    /// The numbers for a context of `n_ctx` tokens with a fixed overhead of
    /// `fixed_overhead` tokens.
    pub fn new(n_ctx: u32, fixed_overhead: u32) -> Self {
        let reply_reserve = reply_reserve(n_ctx);
        let margin = safety_margin(n_ctx);
        let budget = n_ctx.saturating_sub(reply_reserve).saturating_sub(margin);
        let trigger = percent(budget, TRIGGER_PERCENT);
        let result_cap = MAX_RESULT_CAP
            .min(n_ctx / 5)
            .min(budget.saturating_sub(fixed_overhead) / 3);
        Self {
            n_ctx,
            reply_reserve,
            margin,
            budget,
            fixed_overhead,
            trigger,
            result_cap,
        }
    }

    /// True when the fixed overhead is at most 40% of the budget, so a tool turn can start.
    pub fn tools_fit(&self) -> bool {
        u64::from(self.fixed_overhead) * 100
            <= u64::from(self.budget) * u64::from(TOOLS_FIT_PERCENT)
    }

    /// The output limit for a request whose prompt has `prompt_tokens` tokens: the context
    /// left after the prompt, never less than the reply reserve.
    pub fn max_tokens(&self, prompt_tokens: u32) -> u32 {
        self.n_ctx
            .saturating_sub(prompt_tokens)
            .max(self.reply_reserve)
            .max(1)
    }

    /// The output limit for a compaction summary: a capped length well inside the reserve.
    pub fn summary_max_tokens(&self) -> u32 {
        (self.reply_reserve / 2).clamp(128, 1_024)
    }

    /// The most characters a summary may keep in this context: [`SUMMARY_CHARS_PER_TOKEN`]
    /// per token of [`ContextBudget::summary_max_tokens`]. A summary written for a larger
    /// context can be longer; the loop folds it again before it is sent here.
    pub fn summary_max_chars(&self) -> usize {
        usize::try_from(self.summary_max_tokens())
            .unwrap_or(usize::MAX)
            .saturating_mul(SUMMARY_CHARS_PER_TOKEN)
    }
}

/// Characters a summary may keep per token of its cap (summaries are cut to this length).
pub const SUMMARY_CHARS_PER_TOKEN: usize = 4;

/// A summary cut to `max_chars` characters: its head and its tail (the headings that start
/// and end it), with a note where text was left out. Unchanged when it already fits. Used
/// when the summary of earlier messages is too long for a smaller context and the model
/// could not rewrite it shorter, so the turn can still go on.
pub fn cut_summary(text: &str, max_chars: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max_chars {
        return text.to_owned();
    }
    let max_tokens = u32::try_from(max_chars / SUMMARY_CHARS_PER_TOKEN.max(1)).unwrap_or(u32::MAX);
    #[allow(clippy::cast_precision_loss)]
    let chars_per_token = SUMMARY_CHARS_PER_TOKEN as f64;
    truncate_head_tail(text, max_tokens, chars_per_token)
        .map_or_else(|| text.to_owned(), |cut| cut.text)
}

/// About how many words fit in `tokens` tokens: three quarters of them, rounded down to a
/// whole thousand from 1,000 words up, else to a whole hundred (the app's one rule, as
/// `aboutWords` in the webview).
pub fn about_words(tokens: u32) -> u64 {
    let words = u64::from(tokens) * 3 / 4;
    if words >= 1_000 {
        words / 1_000 * 1_000
    } else {
        words / 100 * 100
    }
}

/// A token count with words, as the app writes them: "8,192 tokens, about 6,000 words".
pub fn tokens_with_words(tokens: u32) -> String {
    format!(
        "{} tokens, about {} words",
        group_digits(u64::from(tokens)),
        group_digits(about_words(tokens))
    )
}

/// The "tools do not fit" refusal, with the numbers for the message
/// (ARCHITECTURE.md 8.4 and 15).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolsDoNotFit {
    /// How many tools were enabled.
    pub tool_count: usize,
    /// About how many tokens their schemas take.
    pub tool_tokens: u32,
    /// The whole fixed overhead.
    pub fixed_overhead: u32,
    /// The model's context window.
    pub n_ctx: u32,
    /// The prompt budget.
    pub budget: u32,
}

impl ToolsDoNotFit {
    /// "These 23 tools need about 3,500 tokens; this model's window is 8,192."
    pub fn message(&self) -> String {
        let tools = if self.tool_count == 1 {
            "This tool needs".to_owned()
        } else {
            format!("These {} tools need", self.tool_count)
        };
        format!(
            "{tools} about {} tokens; this model's window is {}. Turn some tool groups off or \
             raise the context size.",
            group_digits(u64::from(self.tool_tokens)),
            group_digits(u64::from(self.n_ctx))
        )
    }
}

/// How many tokens the pinned instructions may take with this budget.
pub fn pinned_allowance(budget: &ContextBudget) -> u32 {
    percent(budget.budget, PINNED_FIT_PERCENT)
}

/// The "pinned instructions do not fit" refusal, with the numbers for the message. Pins
/// are never shortened or left out to make a request fit: the turn does not start, and
/// the user decides what to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedDoNotFit {
    /// How many instructions are pinned.
    pub pin_count: usize,
    /// About how many tokens they take in a request.
    pub pinned_tokens: u32,
    /// The most they may take ([`pinned_allowance`]).
    pub allowed_tokens: u32,
    /// The model's context window.
    pub n_ctx: u32,
}

impl PinnedDoNotFit {
    /// "Your 20 pinned instructions need about 6,700 tokens; this model's window is 4,096,
    /// and pinned instructions may take up to 748 of it."
    pub fn message(&self) -> String {
        let pins = if self.pin_count == 1 {
            "Your pinned instruction needs".to_owned()
        } else {
            format!("Your {} pinned instructions need", self.pin_count)
        };
        format!(
            "{pins} about {} tokens; this model's window is {}, and pinned instructions may \
             take up to {} of it. Remove or shorten pins in the Context tab, or raise the \
             context size.",
            group_digits(u64::from(self.pinned_tokens)),
            group_digits(u64::from(self.n_ctx)),
            group_digits(u64::from(self.allowed_tokens))
        )
    }
}

/// `12345` → `12,345`.
pub fn group_digits(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn tokens_for_chars(chars: usize, chars_per_token: f64) -> u32 {
    #[allow(clippy::cast_precision_loss)]
    let tokens = (chars as f64 / chars_per_token.max(0.5)).ceil();
    if tokens >= f64::from(u32::MAX) {
        u32::MAX
    } else {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let tokens = tokens.max(0.0) as u32;
        tokens
    }
}

/// Text cut to fit a token budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Truncated {
    /// The shortened text, with a note where text was left out.
    pub text: String,
    /// Characters left out.
    pub omitted_chars: usize,
    /// About how many tokens were left out.
    pub omitted_tokens: u32,
}

/// Keeps the head and the tail of `text` with a note in the middle,
/// `[... 48,210 characters (about 12,000 tokens) omitted ...]`, so the result fits
/// `max_tokens` at `chars_per_token`. Cuts at line breaks when one is close. `None` when the
/// text already fits.
pub fn truncate_head_tail(text: &str, max_tokens: u32, chars_per_token: f64) -> Option<Truncated> {
    let total = text.chars().count();
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    let allowed = (f64::from(max_tokens) * chars_per_token.max(0.5))
        .floor()
        .max(0.0) as usize;
    if total <= allowed {
        return None;
    }
    // Room for the note itself.
    let keep = allowed.saturating_sub(80);
    let head_chars = keep / 2;
    let tail_chars = keep - head_chars;
    let head_end = byte_at_char(text, head_chars);
    let tail_start = byte_at_char(text, total - tail_chars);
    // Prefer whole lines when a break is within 200 bytes.
    let head_end = text[..head_end]
        .rfind('\n')
        .filter(|pos| head_end - pos < 200)
        .map_or(head_end, |pos| pos + 1);
    let tail_start = text[tail_start..]
        .find('\n')
        .filter(|pos| *pos < 200)
        .map_or(tail_start, |pos| tail_start + pos + 1)
        .max(head_end);
    let omitted_chars = text[head_end..tail_start].chars().count();
    let omitted_tokens = tokens_for_chars(omitted_chars, chars_per_token);
    let note = format!(
        "[... {} characters (about {} tokens) omitted ...]",
        group_digits(omitted_chars as u64),
        group_digits(u64::from(omitted_tokens))
    );
    let head = &text[..head_end];
    let tail = &text[tail_start..];
    let mut out = String::with_capacity(head.len() + tail.len() + note.len() + 2);
    out.push_str(head);
    if !head.is_empty() && !head.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&note);
    if !tail.is_empty() {
        out.push('\n');
        out.push_str(tail);
    }
    Some(Truncated {
        text: out,
        omitted_chars,
        omitted_tokens,
    })
}

fn byte_at_char(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(i, _)| i)
}

/// The arguments of a stubbed call: string values longer than a few lines are shortened.
pub fn compact_arguments(arguments: &Map<String, Value>) -> Map<String, Value> {
    fn compact(value: &Value) -> Value {
        match value {
            Value::String(text) if text.chars().count() > MAX_STUB_ARGUMENT_CHARS => {
                let head: String = text.chars().take(MAX_STUB_ARGUMENT_CHARS - 40).collect();
                Value::String(format!(
                    "{head}… [{} characters in all]",
                    group_digits(text.chars().count() as u64)
                ))
            }
            Value::Array(items) => Value::Array(items.iter().map(compact).collect()),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(key, value)| (key.clone(), compact(value)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    arguments
        .iter()
        .map(|(key, value)| (key.clone(), compact(value)))
        .collect()
}

/// The stub note for a removed result.
pub fn stub_note(
    call: &CallRecord,
    status: &CallStatus,
    chars: usize,
    images: usize,
    chars_per_token: f64,
) -> String {
    let mut arguments = Value::Object(compact_arguments(&call.arguments)).to_string();
    if arguments.chars().count() > 400 {
        arguments = format!("{}…", arguments.chars().take(400).collect::<String>());
    }
    let images = match images {
        0 => String::new(),
        1 => " and 1 image".to_owned(),
        n => format!(" and {n} images"),
    };
    format!(
        "{STUB_PREFIX} {} {arguments} → {}, {} characters (about {} tokens){images}. Call the \
         tool again to re-read it.",
        call.name,
        status.label(),
        group_digits(chars as u64),
        group_digits(u64::from(tokens_for_chars(chars, chars_per_token)))
    )
}

/// True when a result was already replaced by a stub.
pub fn is_stub(parts: &[Part]) -> bool {
    parts.iter().any(|part| match part {
        Part::ToolResult(result) => {
            result.text.is_empty()
                && result
                    .notes
                    .first()
                    .is_some_and(|n| n.starts_with(STUB_PREFIX))
        }
        _ => false,
    })
}

/// Replaces the results of every call for which `select` is true with stubs, and shortens
/// those calls' long string arguments (a call and its result are replaced together).
/// Results too small to be worth it keep their text. Returns how many results were stubbed.
pub fn stub_results(
    messages: &mut [HistoryMessage],
    select: impl Fn(&CallRecord) -> bool,
    chars_per_token: f64,
) -> usize {
    let mut selected: Vec<CallRecord> = Vec::new();
    let mut stubbed = 0;
    for message in messages.iter_mut() {
        for part in &mut message.parts {
            match part {
                Part::ToolCalls(calls) => {
                    for call in calls.iter_mut() {
                        if select(call) {
                            selected.push(call.clone());
                            call.arguments = compact_arguments(&call.arguments);
                        }
                    }
                }
                Part::ToolResult(result) => {
                    let Some(call) = selected.iter().find(|call| call.id == result.call_id) else {
                        continue;
                    };
                    let already = result.text.is_empty()
                        && result
                            .notes
                            .first()
                            .is_some_and(|n| n.starts_with(STUB_PREFIX));
                    let chars = result.text.chars().count();
                    if already || (chars < MIN_STUB_CHARS && result.images.is_empty()) {
                        continue;
                    }
                    let note = stub_note(
                        call,
                        &result.status,
                        chars,
                        result.images.len(),
                        chars_per_token,
                    );
                    result.text.clear();
                    result.images.clear();
                    result.notes = vec![note];
                    stubbed += 1;
                }
                _ => {}
            }
        }
    }
    stubbed
}

/// How many tool-result images (screenshots) stay in context; older ones become stubs
/// (ARCHITECTURE.md 8.4, "Screenshots").
pub const KEPT_RESULT_IMAGES: usize = 2;

/// Replaces all but the newest `keep` images in tool results with a note. Returns how many
/// were removed.
pub fn drop_old_result_images(messages: &mut [HistoryMessage], keep: usize) -> usize {
    let total: usize = messages
        .iter()
        .flat_map(HistoryMessage::tool_results)
        .map(|result| result.images.len())
        .sum();
    let mut to_drop = total.saturating_sub(keep);
    let dropped = to_drop;
    for message in messages.iter_mut() {
        if to_drop == 0 {
            break;
        }
        for part in &mut message.parts {
            let Part::ToolResult(result) = part else {
                continue;
            };
            if result.images.is_empty() || to_drop == 0 {
                continue;
            }
            let removed = result.images.len().min(to_drop);
            result.images.drain(..removed);
            to_drop -= removed;
            result.notes.push(format!(
                "{} older image{} from this result {} removed to save space; call the tool again \
                 to see the current state.",
                removed,
                if removed == 1 { "" } else { "s" },
                if removed == 1 { "was" } else { "were" }
            ));
        }
    }
    dropped
}

/// The call ids of each assistant message with calls in `messages[from..]`, in order.
pub fn call_groups(messages: &[HistoryMessage], from: usize) -> Vec<Vec<String>> {
    messages
        .iter()
        .skip(from)
        .filter(|message| message.has_tool_calls())
        .map(|message| message.tool_calls().map(|call| call.id.clone()).collect())
        .collect()
}

/// The indices of the user messages that start turns.
pub fn turn_starts(messages: &[HistoryMessage]) -> Vec<usize> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == crate::types::Role::User)
        .map(|(i, _)| i)
        .collect()
}

/// How many leading messages of `earlier` (the turns before the current one) to summarise so
/// that what stays verbatim fits in `keep_tokens`: whole turns only, newest kept first.
/// `estimate` gives the tokens of one message. Returns 0 when nothing should be summarised.
pub fn summarisable_prefix(
    earlier: &[HistoryMessage],
    keep_tokens: u32,
    estimate: impl Fn(&HistoryMessage) -> u32,
) -> usize {
    let starts = turn_starts(earlier);
    let mut kept_tokens: u64 = 0;
    let mut cut = earlier.len();
    // Walk turns from the newest; keep each while it fits. Messages before the first user
    // message (left over after an earlier summary) are always summarised.
    for &start in starts.iter().rev() {
        let turn_tokens: u64 = earlier[start..cut]
            .iter()
            .map(|m| u64::from(estimate(m)))
            .sum();
        if kept_tokens + turn_tokens > u64::from(keep_tokens) {
            break;
        }
        kept_tokens += turn_tokens;
        cut = start;
    }
    cut
}

/// The fixed template for the compaction summary (ARCHITECTURE.md 8.4, stage 2).
pub fn summary_instruction(max_words: u32, has_previous_summary: bool) -> String {
    let previous = if has_previous_summary {
        " Fold in everything the earlier summary in the system message says."
    } else {
        ""
    };
    format!(
        "[App note] The conversation is getting too long for the model's context. Write a \
         summary of the conversation so far for an assistant who will continue it.{previous} \
         Use exactly these headings:\n\
         Goal:\nDecisions:\nWork done:\nWhat is left:\nExact paths and identifiers:\n\
         Keep exact file paths, commands, error messages, names and identifiers. Use at most \
         {max_words} words. Do not continue the task and do not call tools; reply with the \
         summary only."
    )
}

/// The instruction that folds the summary of earlier messages again, shorter, when it was
/// written for a larger context than the model has now (stage 2 with nothing new to
/// summarise). The request carries the system message, which holds the old summary.
pub fn fold_instruction(max_words: u32) -> String {
    format!(
        "[App note] The summary of earlier messages in the system message is too long for the \
         model's context now. Rewrite it more briefly for an assistant who will continue the \
         conversation. Use exactly these headings:\n\
         Goal:\nDecisions:\nWork done:\nWhat is left:\nExact paths and identifiers:\n\
         Keep exact file paths, commands, error messages, names and identifiers. Use at most \
         {max_words} words. Do not continue the task and do not call tools; reply with the \
         summary only."
    )
}

/// A baseline (3 characters per token) estimate of a request's prompt tokens: message text,
/// tool calls, images and tool schemas, plus a few tokens of template markup per message.
pub fn estimate_request(request: &ChatRequest) -> u32 {
    let mut total: u64 = 3;
    for message in &request.messages {
        total += u64::from(MESSAGE_OVERHEAD_TOKENS);
        if let Some(content) = &message.content {
            total += u64::from(estimate_tokens(&content.text()));
            if let MessageContent::Parts(_) = content {
                total += u64::from(IMAGE_TOKENS_ESTIMATE) * content.images().count() as u64;
            }
        }
        if let Some(reasoning) = &message.reasoning_content {
            total += u64::from(estimate_tokens(reasoning));
        }
        for call in message.tool_calls.iter().flatten() {
            total += 4
                + u64::from(estimate_tokens(&call.function.name))
                + u64::from(estimate_tokens(&call.function.arguments));
        }
    }
    if !request.tools.is_empty() {
        let schemas = serde_json::to_string(&request.tools).unwrap_or_default();
        total += u64::from(estimate_tokens(&schemas));
    }
    u32::try_from(total).unwrap_or(u32::MAX)
}

/// A baseline estimate of one stored message (for choosing what to summarise).
pub fn estimate_message(message: &HistoryMessage) -> u32 {
    let mut total = u64::from(MESSAGE_OVERHEAD_TOKENS);
    for part in &message.parts {
        total += u64::from(match part {
            Part::Text(text) => estimate_tokens(text),
            Part::Image { .. } => IMAGE_TOKENS_ESTIMATE,
            Part::Attachment { text, .. } => estimate_tokens(text),
            Part::ToolCalls(calls) => calls
                .iter()
                .map(|call| {
                    4 + estimate_tokens(&call.name)
                        + estimate_tokens(&Value::Object(call.arguments.clone()).to_string())
                })
                .sum(),
            Part::ToolResult(result) => {
                estimate_tokens(&result.text)
                    + result.notes.iter().map(|n| estimate_tokens(n)).sum::<u32>()
                    + IMAGE_TOKENS_ESTIMATE * u32::try_from(result.images.len()).unwrap_or(u32::MAX)
                    + 24
            }
        });
    }
    u32::try_from(total).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::ResultRecord;
    use serde_json::json;

    #[test]
    fn numbers_match_the_architecture_table() {
        // ARCHITECTURE.md 8.4: 8,192 → budget ~6,050, cap ~1,580, trigger ~4,840 (F ~1,300);
        // 32,768 → budget ~28,500, cap ~6,550, trigger ~22,800.
        let small = ContextBudget::new(8_192, 1_300);
        assert_eq!(small.reply_reserve, 2_048);
        assert_eq!(small.budget, 6_048);
        assert_eq!(small.result_cap, 1_582);
        assert_eq!(small.trigger, 4_838);
        assert!(small.budget.abs_diff(6_050) <= 50);
        assert!(small.result_cap.abs_diff(1_580) <= 20);
        assert!(small.trigger.abs_diff(4_840) <= 20);
        assert!(small.tools_fit());

        let large = ContextBudget::new(32_768, 1_300);
        assert_eq!(large.reply_reserve, 4_096);
        assert_eq!(large.budget, 28_480);
        assert_eq!(large.result_cap, 6_553);
        assert_eq!(large.trigger, 22_784);
        assert!(large.budget.abs_diff(28_500) <= 50);
        assert!(large.result_cap.abs_diff(6_550) <= 20);
        assert!(large.trigger.abs_diff(22_800) <= 50);

        // The 8,000 cap applies to large windows.
        assert_eq!(ContextBudget::new(131_072, 1_300).result_cap, 8_000);
    }

    #[test]
    fn tools_do_not_fit_above_forty_percent() {
        // 8k: budget 6,048; 40% = 2,419.
        assert!(ContextBudget::new(8_192, 2_419).tools_fit());
        assert!(!ContextBudget::new(8_192, 2_420).tools_fit());
        // A 4k window carries almost nothing.
        assert!(!ContextBudget::new(4_096, 1_300).tools_fit());
        let refusal = ToolsDoNotFit {
            tool_count: 23,
            tool_tokens: 3_500,
            fixed_overhead: 3_900,
            n_ctx: 8_192,
            budget: 6_048,
        };
        assert_eq!(
            refusal.message(),
            "These 23 tools need about 3,500 tokens; this model's window is 8,192. Turn some tool \
             groups off or raise the context size."
        );
    }

    #[test]
    fn pinned_instructions_get_a_quarter_of_the_budget() {
        // 4k: budget 2,992; 8k: 6,048; 32k: 28,480.
        assert_eq!(pinned_allowance(&ContextBudget::new(4_096, 0)), 748);
        assert_eq!(pinned_allowance(&ContextBudget::new(8_192, 0)), 1_512);
        assert_eq!(pinned_allowance(&ContextBudget::new(32_768, 0)), 7_120);
        // The allowance does not shrink as the fixed part grows.
        assert_eq!(pinned_allowance(&ContextBudget::new(8_192, 2_000)), 1_512);
        // Tools and pins together leave room for the conversation.
        const { assert!(TOOLS_FIT_PERCENT + PINNED_FIT_PERCENT < TRIGGER_PERCENT) };
        let refusal = PinnedDoNotFit {
            pin_count: 20,
            pinned_tokens: 6_741,
            allowed_tokens: 748,
            n_ctx: 4_096,
        };
        assert_eq!(
            refusal.message(),
            "Your 20 pinned instructions need about 6,741 tokens; this model's window is 4,096, \
             and pinned instructions may take up to 748 of it. Remove or shorten pins in the \
             Context tab, or raise the context size."
        );
        let one = PinnedDoNotFit {
            pin_count: 1,
            pinned_tokens: 900,
            ..refusal
        };
        assert!(
            one.message()
                .starts_with("Your pinned instruction needs about 900 tokens; "),
            "{}",
            one.message()
        );
    }

    #[test]
    fn max_tokens_is_what_is_left_but_at_least_the_reserve() {
        let budget = ContextBudget::new(8_192, 500);
        assert_eq!(budget.max_tokens(1_000), 7_192);
        assert_eq!(budget.max_tokens(7_000), 2_048);
        assert_eq!(budget.max_tokens(u32::MAX), 2_048);
        assert_eq!(ContextBudget::new(0, 0).max_tokens(0), 1);
    }

    #[test]
    fn digit_grouping() {
        assert_eq!(group_digits(0), "0");
        assert_eq!(group_digits(999), "999");
        assert_eq!(group_digits(1_000), "1,000");
        assert_eq!(group_digits(48_210), "48,210");
        assert_eq!(group_digits(1_234_567), "1,234,567");
    }

    #[test]
    fn head_and_tail_truncation() {
        let text: String = (0..2_000).map(|i| format!("line {i}\n")).collect();
        let cut = truncate_head_tail(&text, 300, 3.0).unwrap();
        assert!(cut.text.starts_with("line 0\n"));
        assert!(cut.text.ends_with("line 1999\n"));
        assert!(cut.text.contains("characters (about "));
        assert!(cut.text.contains("omitted ...]"));
        assert!(
            cut.text.chars().count() <= 900,
            "{}",
            cut.text.chars().count()
        );
        let note_chars = cut
            .text
            .lines()
            .find(|l| l.starts_with("[..."))
            .unwrap()
            .chars()
            .count();
        // Kept text plus omitted text is the original (the note and the line break after it
        // aside; the head already ends with a line break).
        assert_eq!(
            cut.text.chars().count() - note_chars - 1 + cut.omitted_chars,
            text.chars().count()
        );
        // Whole lines on both sides of the note.
        let note_line = cut.text.lines().find(|l| l.starts_with("[...")).unwrap();
        assert!(note_line.ends_with("omitted ...]"));
        assert!(truncate_head_tail("short", 100, 3.0).is_none());
        // Multibyte text without newlines.
        let wide = "日本語".repeat(1_000);
        let cut = truncate_head_tail(&wide, 50, 3.0).unwrap();
        assert!(cut.text.chars().count() <= 150);
    }

    fn call(id: &str, args: Value) -> CallRecord {
        CallRecord {
            id: id.into(),
            name: "read_file".into(),
            arguments: args.as_object().cloned().unwrap(),
            recovered: false,
        }
    }

    fn big_result(id: &str) -> ResultRecord {
        ResultRecord {
            call_id: id.into(),
            name: "read_file".into(),
            status: CallStatus::Ok,
            text: "x".repeat(3_000),
            images: vec![("image/png".into(), "AAAA".into())],
            source: Some("/p/a".into()),
            notes: vec![],
        }
    }

    #[test]
    fn stubs_replace_call_and_result_together() {
        let long_content = "y".repeat(5_000);
        let mut messages = vec![
            HistoryMessage::user("go"),
            HistoryMessage::assistant_calls(
                "",
                vec![call("a", json!({"path": "a.txt", "content": long_content}))],
            ),
            HistoryMessage::tool_result(big_result("a")),
            HistoryMessage::assistant_calls("", vec![call("b", json!({"path": "b.txt"}))]),
            HistoryMessage::tool_result(big_result("b")),
        ];
        let count = stub_results(&mut messages, |c| c.id == "a", 3.0);
        assert_eq!(count, 1);
        let stubbed_call = messages[1].tool_calls().next().unwrap();
        let content = stubbed_call.arguments["content"].as_str().unwrap();
        assert!(content.chars().count() < 200, "{content}");
        assert!(content.contains("5,000 characters in all"));
        let result = messages[2].tool_results().next().unwrap();
        assert!(result.text.is_empty());
        assert!(result.images.is_empty());
        assert!(result.notes[0].starts_with(STUB_PREFIX));
        assert!(result.notes[0].contains("read_file"));
        assert!(
            result.notes[0].contains("→ ok, 3,000 characters (about 1,000 tokens) and 1 image")
        );
        assert!(result.notes[0].contains("Call the tool again to re-read it."));
        assert!(is_stub(&messages[2].parts));
        // The other pair is untouched, and stubbing again changes nothing.
        assert_eq!(messages[4].tool_results().next().unwrap().text.len(), 3_000);
        assert_eq!(stub_results(&mut messages, |c| c.id == "a", 3.0), 0);
        assert_eq!(
            call_groups(&messages, 0),
            vec![vec!["a".to_owned()], vec!["b".to_owned()]]
        );
    }

    #[test]
    fn only_the_newest_screenshots_stay() {
        let shot = |id: &str| {
            HistoryMessage::tool_result(ResultRecord {
                text: "screenshot taken".into(),
                images: vec![("image/png".into(), "AAAA".into())],
                ..big_result(id)
            })
        };
        let mut messages = vec![shot("a"), shot("b"), shot("c")];
        assert_eq!(drop_old_result_images(&mut messages, KEPT_RESULT_IMAGES), 1);
        let first = messages[0].tool_results().next().unwrap();
        assert!(first.images.is_empty());
        assert!(first.notes[0].contains("1 older image from this result was removed"));
        assert_eq!(first.text, "screenshot taken");
        assert_eq!(messages[1].tool_results().next().unwrap().images.len(), 1);
        assert_eq!(messages[2].tool_results().next().unwrap().images.len(), 1);
        assert_eq!(drop_old_result_images(&mut messages, KEPT_RESULT_IMAGES), 0);
    }

    #[test]
    fn small_results_are_not_stubbed() {
        let mut messages = vec![
            HistoryMessage::assistant_calls("", vec![call("a", json!({"path": "a"}))]),
            HistoryMessage::tool_result(ResultRecord {
                text: "tiny".into(),
                images: vec![],
                ..big_result("a")
            }),
        ];
        assert_eq!(stub_results(&mut messages, |_| true, 3.0), 0);
        assert_eq!(messages[1].tool_results().next().unwrap().text, "tiny");
    }

    #[test]
    fn summarisable_prefix_keeps_newest_whole_turns() {
        let turn = |n: usize| {
            vec![
                HistoryMessage::user(format!("question {n}")),
                HistoryMessage::assistant("a".repeat(300)),
            ]
        };
        let earlier: Vec<HistoryMessage> = (0..4).flat_map(turn).collect();
        let per_turn: u32 = earlier[0..2].iter().map(estimate_message).sum();
        // Room for two turns: the first two are summarised.
        assert_eq!(
            summarisable_prefix(&earlier, per_turn * 2 + 1, estimate_message),
            4
        );
        // Room for everything: nothing to summarise.
        assert_eq!(summarisable_prefix(&earlier, u32::MAX, estimate_message), 0);
        // Room for nothing: everything.
        assert_eq!(summarisable_prefix(&earlier, 0, estimate_message), 8);
    }

    #[test]
    fn summary_instruction_has_the_fixed_headings() {
        let text = summary_instruction(300, true);
        for heading in [
            "Goal:",
            "Decisions:",
            "Work done:",
            "What is left:",
            "Exact paths and identifiers:",
        ] {
            assert!(text.contains(heading));
        }
        assert!(text.contains("300 words"));
        assert!(text.contains("earlier summary"));
        assert!(!summary_instruction(300, false).contains("earlier summary"));
    }

    #[test]
    fn the_fold_instruction_rewrites_the_summary_with_the_same_headings() {
        let text = fold_instruction(341);
        assert!(text.starts_with("[App note] The summary of earlier messages"));
        for heading in [
            "Goal:",
            "Decisions:",
            "Work done:",
            "What is left:",
            "Exact paths and identifiers:",
        ] {
            assert!(text.contains(heading));
        }
        assert!(text.contains("at most 341 words"));
        assert!(text.contains("do not call tools"));
    }

    #[test]
    fn the_summary_cap_in_characters_follows_the_context() {
        // Four characters per token of the summary cap: 1,024 tokens from 8k up, 512 at 4k,
        // 256 at 2k.
        assert_eq!(ContextBudget::new(32_768, 0).summary_max_chars(), 4_096);
        assert_eq!(ContextBudget::new(8_192, 0).summary_max_chars(), 4_096);
        assert_eq!(ContextBudget::new(4_096, 0).summary_max_chars(), 2_048);
        assert_eq!(ContextBudget::new(2_048, 0).summary_max_chars(), 1_024);
    }

    #[test]
    fn a_summary_is_cut_to_its_head_and_tail() {
        let summary = format!(
            "Goal: fix the build.\n{}Exact paths and identifiers: /p/src/main.rs",
            "Work done: a step.\n".repeat(300)
        );
        let cut = cut_summary(&summary, 2_048);
        assert!(cut.chars().count() <= 2_048, "{}", cut.chars().count());
        assert!(cut.starts_with("Goal: fix the build.\n"));
        assert!(cut.ends_with("Exact paths and identifiers: /p/src/main.rs"));
        assert!(cut.contains("omitted ...]"));
        // A summary that fits stays as it is (trimmed).
        assert_eq!(cut_summary("  Goal: x  ", 2_048), "Goal: x");
        assert_eq!(cut_summary(&summary, summary.chars().count()), summary);
    }

    #[test]
    fn token_counts_come_with_words() {
        assert_eq!(tokens_with_words(8_192), "8,192 tokens, about 6,000 words");
        assert_eq!(
            tokens_with_words(32_768),
            "32,768 tokens, about 24,000 words"
        );
        assert_eq!(tokens_with_words(4_096), "4,096 tokens, about 3,000 words");
        assert_eq!(tokens_with_words(600), "600 tokens, about 400 words");
        assert_eq!(about_words(0), 0);
    }

    #[test]
    fn request_estimates_count_text_calls_images_and_tools() {
        use crate::types::{ChatMessage, ContentPart, ImageUrl, ToolCall, ToolSpec};
        let mut request = ChatRequest::new(
            "m",
            vec![
                ChatMessage::system("s".repeat(300)),
                ChatMessage::user(vec![
                    ContentPart::text("look"),
                    ContentPart::image(ImageUrl::from_base64("image/png", "AAAA").unwrap()),
                ]),
                ChatMessage::assistant_tool_calls(
                    None,
                    vec![ToolCall::function("c", "read_file", "{\"path\":\"abc\"}")],
                ),
            ],
        );
        let base = estimate_request(&request);
        assert!(base >= 100 + IMAGE_TOKENS_ESTIMATE, "{base}");
        request.tools = vec![ToolSpec::function(
            "read_file",
            "Read a file",
            json!({"type": "object"}),
        )];
        assert!(estimate_request(&request) > base);
    }
}
