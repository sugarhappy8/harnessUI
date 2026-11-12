//! The context inspector (0.0.4): exactly what the next request of a chat would contain,
//! for the Context tab.
//!
//! What must hold:
//!
//! - It is built by the code that builds a real turn: `turns::assemble` (the input and the
//!   tool list) in its preview mode, then `agent::preview_turn` (the request, counted, fit
//!   checked and staged by the loop's own code). Nothing is sent to the model, and nothing
//!   is changed: the preview mode reads where a turn would record (the tools the chat was
//!   shown, the task's "sent" record, the artifacts mark, the computer-use profile, the
//!   skills server's turn settings), and a remote endpoint's reported context is not kept.
//! - What is shown is the body of the request only. The endpoint's address, the request's
//!   headers and any key are never part of it (they are not in what is built here).
//! - Everything is plain text for the tab: it holds tool results and file text, which are
//!   untrusted, and the tab never renders it as markup.
//! - Token numbers come from the loop's estimator and budget (`agent::estimate`,
//!   `ContextBudget`); without a usable model the tab says so instead of showing numbers.

use agent::turn::{NextCompaction, SystemBlockKind, TurnPreview, TurnStatus};
use agent::{HistoryMessage, OpenAiClient, Part, Role, ToolInfo};
use async_trait::async_trait;
use mcp_host::{Described, ToolOutput};
use serde::Serialize;
use serde_json::{Map, Value};
use tauri::{AppHandle, State};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::turns::{Assembly, TurnRequest};

/// Stands for the user's next message when the chat does not end with one, so the request
/// has the shape the next turn's will have (the task note goes with that message).
pub const NEXT_MESSAGE: &str = "(your next message)";

/// One section of the next request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    /// Stable id: `system`, `pinned`, `preamble`, `skills`, `loadedSkills`, `tools`,
    /// `summary`, `kept`, `leftOut`, `task`, `attachments`.
    pub id: &'static str,
    /// What the tab calls it.
    pub title: &'static str,
    /// One line under the title (counts, names). Plain text.
    pub detail: String,
    /// About how many tokens it takes; `None` when no numbers can be given.
    pub tokens: Option<u32>,
    /// Its exact text. Plain text, never markup.
    pub text: String,
}

/// A share of the context and how much of it is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Share {
    pub used: u32,
    pub allowed: u32,
}

/// The numbers, when there is a model to count for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Totals {
    /// The whole request as the loop counts it.
    pub tokens: u32,
    /// True when the model server counted it; otherwise an estimate.
    pub exact: bool,
    /// The model's context window.
    pub n_ctx: u32,
    /// What a request may use (the window less the room kept for the reply).
    pub budget: u32,
    /// Where the loop starts to make room.
    pub trigger: u32,
    /// The fixed part without the pins, against what it may be with tools on.
    pub tools: Option<Share>,
    /// The pinned instructions against their share.
    pub pinned: Share,
}

/// What the Context tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextView {
    /// One sentence about the model and its context size, or why there are no numbers.
    pub model: String,
    /// `None` when no model is usable or its context size is unknown.
    pub totals: Option<Totals>,
    pub sections: Vec<Section>,
    /// What the loop's context stages would remove next, in one or two sentences.
    pub next_drop: String,
    /// Why the next turn would not start, if it would not.
    pub refusal: Option<String>,
    /// Short notes (the placeholder for the next message, how the tool list is chosen).
    pub notes: Vec<String>,
}

/// The shown tools for the loop to list. Nothing is described or run.
struct Listed {
    infos: Vec<ToolInfo>,
    host: mcp_host::Host,
}

#[async_trait]
impl agent::turn::ToolHost for Listed {
    fn tools(&self) -> Vec<ToolInfo> {
        self.infos.clone()
    }
    fn validate(&self, _: &str, _: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
    fn llama_schema(&self, name: &str) -> Option<Value> {
        self.host.llama_schema(name)
    }
    async fn describe(&self, _: &str, _: &Map<String, Value>) -> Described {
        Described::Invalid("a preview runs nothing".into())
    }
    async fn execute(
        &self,
        _: &str,
        _: &Map<String, Value>,
        _: &policy::Action,
        _: CancellationToken,
    ) -> ToolOutput {
        ToolOutput::error("a preview runs nothing")
    }
}

fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window can look at what the model is sent.",
        ))
    }
}

/// A client that is never asked anything (a generic endpoint: no token count, no probe),
/// for building the request when the chat has no usable model.
fn idle_client() -> AppResult<OpenAiClient> {
    OpenAiClient::new(agent::ClientConfig::new(
        "http://127.0.0.1:9/v1".to_owned(),
        agent::ProviderKind::Generic,
    ))
    .map_err(AppError::internal)
}

/// What the next request of a chat would contain.
#[tauri::command]
pub async fn inspect_context(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
) -> AppResult<ContextView> {
    only_main_window(window.label())?;
    let chat = state
        .store
        .get_chat(store::ChatId(chat_id))
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let target = crate::chat::target_for(&state, chat.model_ref.as_deref(), false).await;
    let (client, caps, template, model_ref, model) = match target {
        Ok(target) => {
            let (caps, template) = crate::chat::turn_settings(&state, &target);
            let model = model_line(target.local, caps.n_ctx);
            (target.client, caps, template, target.model_ref, Ok(model))
        }
        Err(e) => {
            let caps = agent::ModelCaps {
                tools: true,
                n_ctx: crate::settings::DEFAULT_REMOTE_CONTEXT,
                ..agent::ModelCaps::default()
            };
            let why = format!("{} Token numbers appear once it is.", e.message);
            (
                idle_client()?,
                caps,
                agent::RequestTemplate::default(),
                chat.model_ref.clone().unwrap_or_default(),
                Err(why),
            )
        }
    };
    let req = TurnRequest {
        reply_to: chat.leaf_message_id.unwrap_or(store::MessageId(0)),
        chat,
        user_message_id: None,
        client,
        model_ref,
        caps,
        template,
    };
    let assembled = crate::turns::assemble(&app, &req, Assembly::Preview).await?;
    let task_status = crate::task::note_status_for_turn(&state, chat_id).await;
    let mut input = assembled.input;
    let task_note = input.app_notes.first().cloned();
    let placeholder = !input
        .history
        .last()
        .is_some_and(|m| m.role == Role::User);
    if placeholder {
        input.history.push(HistoryMessage::user(NEXT_MESSAGE));
    }
    let sanitizer = agent::prompt::Sanitizer::for_endpoint(&req.client).await;
    let listed = Listed {
        infos: crate::turns::tool_infos(&assembled.shown),
        host: state.host.clone(),
    };
    let preview = agent::preview_turn(&req.client, &listed, &sanitizer, input).await;
    Ok(view(&Inspected {
        preview: &preview,
        hidden: assembled.hidden.len(),
        task_note: task_note.as_deref(),
        task_status: task_status.as_deref(),
        placeholder,
        model,
    }))
}

fn model_line(local: bool, n_ctx: u32) -> String {
    let n = agent::context::group_digits(u64::from(n_ctx));
    if local {
        format!("This chat's model runs on this computer with a context of {n} tokens.")
    } else {
        format!(
            "This chat's model is on an endpoint. The app counts with a context of {n} tokens: \
             what the endpoint reports, or the size set in Settings."
        )
    }
}

/// Everything [`view`] is built from.
pub struct Inspected<'a> {
    pub preview: &'a TurnPreview,
    /// How many tools are on but not shown this turn.
    pub hidden: usize,
    /// The task note the request carries, if any.
    pub task_note: Option<&'a str>,
    /// The task state's sentence for the user.
    pub task_status: Option<&'a str>,
    /// [`NEXT_MESSAGE`] stands for the next message.
    pub placeholder: bool,
    /// The model sentence, or why there are no numbers.
    pub model: Result<String, String>,
}

fn role_name(role: Role) -> &'static str {
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
    }
}

/// The messages after the system message, as sent, in plain text.
fn messages_text(request: &agent::ChatRequest) -> String {
    request
        .messages
        .iter()
        .skip(1)
        .map(|m| {
            let mut out = format!("[{}]", role_name(m.role));
            if let Some(name) = m.name.as_deref().filter(|_| m.role == Role::Tool) {
                out.push_str(&format!(" {name}"));
            }
            if let Some(content) = &m.content {
                let text = content.text();
                if !text.is_empty() {
                    out.push('\n');
                    out.push_str(&text);
                }
                let images = content.images().count();
                if images > 0 {
                    out.push_str(&format!("\n({images} image{})", if images == 1 { "" } else { "s" }));
                }
            }
            for call in m.tool_calls.iter().flatten() {
                out.push_str(&format!(
                    "\ncalls {}({})",
                    call.function.name, call.function.arguments
                ));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// A stored message in plain text: its role and its text parts.
fn history_text(messages: &[(HistoryMessage, u32)]) -> String {
    messages
        .iter()
        .map(|(m, _)| {
            let mut out = format!("[{}]", role_name(m.role));
            for part in &m.parts {
                match part {
                    Part::Text(text) => {
                        out.push('\n');
                        out.push_str(text);
                    }
                    Part::Attachment { name, .. } => out.push_str(&format!("\n(attached: {name})")),
                    Part::Image { .. } => out.push_str("\n(an image)"),
                    Part::ToolCalls(calls) => {
                        for call in calls {
                            out.push_str(&format!("\ncalls {}", call.name));
                        }
                    }
                    Part::ToolResult(result) => {
                        out.push_str(&format!("\nresult of {}:\n{}", result.name, result.text));
                    }
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

/// What the loop's stages would remove next, as the tab says it.
fn next_drop(preview: &TurnPreview) -> String {
    let what = match &preview.next {
        NextCompaction::ShortenEarlierResults { results } => format!(
            "the results of {} in earlier messages would be shortened to one-line notes.",
            plural(*results, "tool call", "tool calls")
        ),
        NextCompaction::SummariseEarlier { messages } => format!(
            "the oldest {} would be replaced by a summary the model writes.",
            plural(*messages, "message", "messages")
        ),
        NextCompaction::ShortenTurnResults => {
            "older tool results of the current turn would be shortened to one-line notes."
                .to_owned()
        }
        NextCompaction::Nothing => String::new(),
    };
    let mut out = String::new();
    if preview.applied.contains(&1) {
        out.push_str(
            "Results of tool calls in earlier messages are already shortened to one-line notes \
             so that the request fits. ",
        );
    }
    if what.is_empty() {
        if preview.over_budget {
            out.push_str(
                "The conversation no longer fits the model's context and nothing is left to \
                 shorten. Start a new chat or raise the context size.",
            );
        } else {
            out.push_str("Nothing would be left out: there is nothing earlier to shorten.");
        }
    } else if preview.over_trigger {
        out.push_str("Before the next request is sent, ");
        out.push_str(&what);
    } else {
        out.push_str(&format!(
            "Nothing is left out now. Once the request passes about {} tokens, ",
            agent::context::group_digits(u64::from(preview.budget.trigger))
        ));
        out.push_str(&what);
    }
    out
}

fn block_text(preview: &TurnPreview, kind: &SystemBlockKind) -> (String, u32) {
    let blocks: Vec<_> = preview
        .system_blocks
        .iter()
        .filter(|b| match (kind, &b.kind) {
            (SystemBlockKind::LoadedSkill { .. }, SystemBlockKind::LoadedSkill { .. }) => true,
            (a, b) => a == b,
        })
        .collect();
    (
        blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n"),
        blocks.iter().map(|b| b.tokens).sum(),
    )
}

/// The Context tab's view of a preview.
pub fn view(inspected: &Inspected) -> ContextView {
    let p = inspected.preview;
    let numbers = inspected.model.is_ok();
    let tokens = |n: u32| numbers.then_some(n);
    let mut sections = Vec::new();
    let mut system = |id, title, kind: SystemBlockKind, detail: String| {
        let (text, n) = block_text(p, &kind);
        let detail = if text.is_empty() {
            "None".to_owned()
        } else {
            detail
        };
        sections.push(Section {
            id,
            title,
            detail,
            tokens: tokens(n),
            text,
        });
    };
    system(
        "system",
        "System prompt",
        SystemBlockKind::Prompt,
        "The chat's own, or the default from Settings".to_owned(),
    );
    let pins = p
        .system_blocks
        .iter()
        .find(|b| b.kind == SystemBlockKind::Pinned)
        .map(|b| b.text.lines().filter(|l| l.split_once(". ").is_some_and(|(n, _)| n.parse::<u32>().is_ok())).count())
        .unwrap_or(0);
    system(
        "pinned",
        "Pinned instructions",
        SystemBlockKind::Pinned,
        plural(pins, "pin", "pins"),
    );
    system(
        "preamble",
        "Tool instructions",
        SystemBlockKind::Preamble,
        "How to use tools, the approved folders and the system".to_owned(),
    );
    system(
        "skills",
        "Skills list",
        SystemBlockKind::SkillsIndex,
        "The enabled skills, by name and description".to_owned(),
    );
    let loaded: Vec<String> = p
        .system_blocks
        .iter()
        .filter_map(|b| match &b.kind {
            SystemBlockKind::LoadedSkill { name } => Some(name.clone()),
            _ => None,
        })
        .collect();
    system(
        "loadedSkills",
        "Loaded skills",
        SystemBlockKind::LoadedSkill {
            name: String::new(),
        },
        loaded.join(", "),
    );
    let names: Vec<&str> = p
        .request
        .tools
        .iter()
        .map(|t| t.function.name.as_str())
        .collect();
    let tools_text = p
        .request
        .tools
        .iter()
        .map(|t| serde_json::to_string_pretty(t).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n\n");
    let mut detail = if names.is_empty() {
        "No tools are offered".to_owned()
    } else {
        format!("{} shown: {}", names.len(), names.join(", "))
    };
    if inspected.hidden > 0 {
        detail.push_str(&format!(
            ". {} more {} on but not shown; the model can find them with find_tools",
            inspected.hidden,
            if inspected.hidden == 1 { "is" } else { "are" }
        ));
    }
    if !p.withheld_tools.is_empty() {
        detail.push_str(&format!(
            ". Not offered because their settings contain chat markup: {}",
            p.withheld_tools.join(", ")
        ));
    }
    sections.push(Section {
        id: "tools",
        title: "Tool definitions",
        detail,
        tokens: tokens(p.tool_tokens),
        text: tools_text,
    });
    let mut system = |id, title, kind: SystemBlockKind, detail: String| {
        let (text, n) = block_text(p, &kind);
        let detail = if text.is_empty() {
            "None".to_owned()
        } else {
            detail
        };
        sections.push(Section {
            id,
            title,
            detail,
            tokens: tokens(n),
            text,
        });
    };
    system(
        "summary",
        "Summary of earlier messages",
        SystemBlockKind::Summary,
        "Written by the model to stand for the messages left out".to_owned(),
    );
    let kept_count = p.kept.len();
    let mut kept_detail = plural(kept_count, "message", "messages");
    if inspected.placeholder {
        kept_detail.push_str(&format!(", ending with {NEXT_MESSAGE}"));
    }
    sections.push(Section {
        id: "kept",
        title: "Messages kept",
        detail: kept_detail,
        tokens: tokens(p.kept.iter().map(|(_, n)| *n).sum()),
        text: messages_text(&p.request),
    });
    sections.push(Section {
        id: "leftOut",
        title: "Messages left out",
        detail: if p.left_out.is_empty() {
            "None".to_owned()
        } else {
            format!(
                "{}, replaced by the summary",
                plural(p.left_out.len(), "message", "messages")
            )
        },
        tokens: tokens(p.left_out.iter().map(|(_, n)| *n).sum()),
        text: history_text(&p.left_out),
    });
    let note = inspected.task_note.unwrap_or("");
    sections.push(Section {
        id: "task",
        title: "Task note",
        detail: inspected
            .task_status
            .map(str::to_owned)
            .unwrap_or_else(|| {
                if note.is_empty() {
                    "None".to_owned()
                } else {
                    "Sent with the next message".to_owned()
                }
            }),
        tokens: tokens(agent::estimate_tokens(note)),
        text: note.to_owned(),
    });
    let attachments: Vec<(&str, &str)> = p
        .kept
        .iter()
        .flat_map(|(m, _)| m.parts.iter())
        .filter_map(|part| match part {
            Part::Attachment { name, text } => Some((name.as_str(), text.as_str())),
            _ => None,
        })
        .collect();
    sections.push(Section {
        id: "attachments",
        title: "Attachments",
        detail: if attachments.is_empty() {
            "None".to_owned()
        } else {
            format!(
                "{}: {}",
                plural(attachments.len(), "file", "files"),
                attachments
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        },
        tokens: tokens(attachments.iter().map(|(_, t)| agent::estimate_tokens(t)).sum()),
        text: attachments
            .iter()
            .map(|(name, text)| format!("[{name}]\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    });

    let totals = numbers.then(|| Totals {
        tokens: p.prompt_tokens,
        exact: p.exact,
        n_ctx: p.budget.n_ctx,
        budget: p.budget.budget,
        trigger: p.budget.trigger,
        tools: p.tools_on.then_some(Share {
            used: p.unpinned_overhead,
            allowed: p.tools_allowance,
        }),
        pinned: Share {
            used: p.pinned_tokens,
            allowed: p.pinned_allowance,
        },
    });
    let refusal = numbers
        .then(|| match &p.refusal {
            Some(TurnStatus::Failed { message, .. }) => Some(message.clone()),
            _ => None,
        })
        .flatten();
    let mut notes = Vec::new();
    if inspected.placeholder {
        notes.push(format!(
            "{NEXT_MESSAGE} stands for what you write next. The tools shown can change with \
             what it asks for."
        ));
    }
    ContextView {
        model: match &inspected.model {
            Ok(line) | Err(line) => line.clone(),
        },
        totals,
        sections,
        next_drop: if numbers {
            next_drop(p)
        } else {
            "Shown once a model is ready for this chat.".to_owned()
        },
        refusal,
        notes,
    }
}

#[cfg(test)]
mod tests;
