//! The first request of a turn, built by the loop's own code and not sent ([`super::super::preview_turn`]).
//!
//! The same [`Turn`] is set up from the same input; the fixed overhead is measured and the
//! fit checks run as in [`Turn::measure`]; the request is built as [`Turn::prepare`] builds
//! it, applying stage 1 when the request needs it (that only changes the copy being built).
//! Stage 2 needs the model to write a summary, so it is reported, never run. Policy,
//! approvals and the sink are inert: nothing is decided, asked or recorded.

use std::collections::HashSet;

use crate::estimate::estimate_tokens;
use crate::turn::{NextCompaction, SystemBlock, SystemBlockKind, TurnPreview};

use super::*;

struct Inert;

impl super::super::PolicyGate for Inert {
    fn decide(&self, _: &str, _: &policy::Action, _: bool, _: bool) -> policy::Decision {
        policy::Decision::Refused {
            reason: "a preview runs nothing".into(),
        }
    }
}

#[async_trait::async_trait]
impl super::super::Approver for Inert {
    async fn ask(&self, _: ApprovalRequest) -> Approval {
        Approval::Cancelled
    }
}

impl super::super::TurnSink for Inert {
    fn event(&self, _: TurnEvent) {}
}

/// See [`super::super::preview_turn`].
pub(in crate::turn) async fn preview(
    client: &crate::client::OpenAiClient,
    tools: &dyn super::super::ToolHost,
    sanitizer: &crate::prompt::Sanitizer,
    input: TurnInput,
) -> TurnPreview {
    // What the summary replaces: the stored messages it covers.
    let covered: HashSet<i64> = input
        .summary
        .as_ref()
        .map(|s| s.covered_ids.iter().copied().collect())
        .unwrap_or_default();
    let left_out: Vec<HistoryMessage> = normalize(input.history.clone())
        .into_iter()
        .filter(|m| m.id.is_some_and(|id| covered.contains(&id)))
        .collect();
    let deps = TurnDeps {
        client,
        tools,
        policy: &Inert,
        approver: &Inert,
        sink: &Inert,
        sanitizer,
    };
    let mut turn = Turn::new(deps, input, CancellationToken::new());
    turn.preview(left_out).await
}

impl Turn<'_> {
    /// A text's estimate as this turn counts.
    fn tokens_of(&self, text: &str) -> u32 {
        let estimate = estimate_tokens(text);
        if self.exact_counting {
            estimate
        } else {
            self.calibrator.calibrate(estimate)
        }
    }

    /// A count that cannot be cancelled here (the token is never cancelled); the estimate if
    /// it fails all the same.
    async fn count_or_estimate(&self, request: &ChatRequest) -> u32 {
        match self.count(request).await {
            Ok((count, _)) => count,
            Err(_) => estimate_request(request),
        }
    }

    async fn overhead_or_estimate(&self, system: String, with_tools: bool) -> u32 {
        self.fixed_overhead_of(system, with_tools)
            .await
            .unwrap_or_default()
    }

    /// The blocks of the system message, each rendered alone by the same code.
    fn system_blocks(&self) -> Vec<SystemBlock> {
        let empty = SystemParts {
            chat_prompt: "",
            pinned: &[],
            preamble: None,
            skills_index: None,
            loaded_skills: &[],
            summary: None,
        };
        let preamble = self.tools_on.then(|| {
            self.input
                .preamble
                .clone()
                .unwrap_or_else(|| Preamble::for_this_os(Vec::new()))
        });
        let mut parts: Vec<(SystemBlockKind, SystemParts)> = vec![
            (
                SystemBlockKind::Prompt,
                SystemParts {
                    chat_prompt: &self.input.system_prompt,
                    ..empty
                },
            ),
            (
                SystemBlockKind::Pinned,
                SystemParts {
                    pinned: &self.input.pinned,
                    ..empty
                },
            ),
            (
                SystemBlockKind::Preamble,
                SystemParts {
                    preamble: preamble.as_ref(),
                    ..empty
                },
            ),
            (
                SystemBlockKind::SkillsIndex,
                SystemParts {
                    skills_index: self.input.skills_index.as_deref(),
                    ..empty
                },
            ),
        ];
        for skill in &self.input.loaded_skills {
            parts.push((
                SystemBlockKind::LoadedSkill {
                    name: skill.name.clone(),
                },
                SystemParts {
                    loaded_skills: std::slice::from_ref(skill),
                    ..empty
                },
            ));
        }
        parts.push((
            SystemBlockKind::Summary,
            SystemParts {
                summary: self.summary.as_ref().map(|s| s.text.as_str()),
                ..empty
            },
        ));
        parts
            .into_iter()
            .filter_map(|(kind, part)| {
                let text = system_message(&part, self.deps.sanitizer);
                (!text.is_empty()).then(|| SystemBlock {
                    tokens: self.tokens_of(&text),
                    kind,
                    text,
                })
            })
            .collect()
    }

    /// What the stages of [`Turn::prepare`] would remove next, in their order.
    fn next_compaction(&self) -> NextCompaction {
        let earlier = &self.earlier[self.earlier_skip..];
        if !self.stub_earlier && has_results(earlier) {
            return NextCompaction::ShortenEarlierResults {
                results: earlier.iter().map(|m| m.tool_results().count()).sum(),
            };
        }
        if !earlier.is_empty() && !self.summary_failed {
            let mut earlier = earlier.to_vec();
            if self.stub_earlier {
                stub_results(&mut earlier, |_| true, self.chars_per_token());
            }
            // What the loop would choose now, or (while everything fits) the oldest whole
            // turn, the least a summary replaces.
            let messages = match self.summarisable(&earlier) {
                0 => turn_starts(&earlier)
                    .into_iter()
                    .find(|cut| *cut > 0)
                    .unwrap_or(earlier.len()),
                wanted => wanted,
            };
            return NextCompaction::SummariseEarlier { messages };
        }
        if call_groups(&self.turn, 0).len() > 1 {
            return NextCompaction::ShortenTurnResults;
        }
        NextCompaction::Nothing
    }

    async fn preview(&mut self, left_out: Vec<HistoryMessage>) -> TurnPreview {
        // The fit checks, with the loop's own refusal.
        let refusal = self.measure().await.err();
        let fixed = self.budget.fixed_overhead;
        let without_tools = if self.tools_on {
            self.overhead_or_estimate(self.system.clone(), false).await
        } else {
            fixed
        };
        let plain = self.system_text(false);
        let unpinned = if plain == self.system {
            fixed
        } else {
            self.overhead_or_estimate(plain, true).await
        };
        // The request as `prepare` builds it, stage 1 applied when it needs it.
        let mut applied = Vec::new();
        let (request, count, view) = loop {
            let view = self.view();
            let mut request = self.request_for_view(&view, ToolChoice::Auto, false);
            let count = self.count_or_estimate(&request).await;
            if count > self.budget.trigger
                && refusal.is_none()
                && !self.stub_earlier
                && has_results(&self.earlier[self.earlier_skip..])
            {
                self.stub_earlier = true;
                applied.push(1);
                continue;
            }
            let max_tokens = self.budget.max_tokens(count);
            request.max_tokens = Some(max_tokens);
            self.limit_thinking(&mut request, max_tokens, false);
            break (request, count, view);
        };
        let kept = view
            .into_iter()
            .map(|m| {
                let tokens = self.calibrated(&m);
                (m, tokens)
            })
            .collect();
        let left_out = left_out
            .into_iter()
            .map(|m| {
                let tokens = self.calibrated(&m);
                (m, tokens)
            })
            .collect();
        TurnPreview {
            system_blocks: self.system_blocks(),
            budget: self.budget,
            prompt_tokens: count,
            exact: self.exact_counting,
            refusal,
            tools_on: self.tools_on,
            withheld_tools: self.withheld_tools.clone(),
            tool_tokens: fixed.saturating_sub(without_tools),
            tools_allowance: u32::try_from(
                u64::from(self.budget.budget) * u64::from(crate::context::TOOLS_FIT_PERCENT) / 100,
            )
            .unwrap_or(u32::MAX),
            unpinned_overhead: unpinned,
            pinned_tokens: fixed.saturating_sub(unpinned),
            pinned_allowance: pinned_allowance(&self.budget),
            kept,
            left_out,
            applied,
            over_trigger: count > self.budget.trigger,
            over_budget: count > self.budget.budget,
            next: self.next_compaction(),
            request,
        }
    }
}
