//! One headless agent turn in a run's folder, with everything it did recorded.
//!
//! The real agent loop (`agent::turn::run_turn`) talks to the model; the tools are the
//! real file tools (and, for benchmarks, the real terminal) of a tool host made for this
//! run only, whose one approved folder is the run's [`Workspace`]. Every call is described
//! by its tool and decided by [`sandbox::Gate`] (which calls `policy::evaluate`); nothing
//! is ever asked ([`sandbox::NobodyToAsk`]). Every finished call becomes an audit row.
//! Stop and the time limit end the turn through its cancellation token.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent::history::{CallStatus, HistoryMessage};
use agent::turn::{
    CallDecision, ModelCaps, RequestTemplate, ToolHost, TurnDeps, TurnEvent, TurnInput,
    TurnSink, TurnStatus,
};
use agent::{OpenAiClient, Role, Sanitizer, ToolInfo};
use async_trait::async_trait;
use mcp_host::{CallContext, ChatContext, Described, Host, HostOptions, ToolOutput, ToolPrefs};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use super::sandbox::{Gate, NobodyToAsk, Rules, Verdict};
use super::workspace::Workspace;

/// Where the run's requests go, and what that model can do.
#[derive(Clone)]
pub struct Model {
    pub client: OpenAiClient,
    pub caps: ModelCaps,
    pub template: RequestTemplate,
    pub sanitizer: Sanitizer,
}

/// The request settings a run varies (the capability check's variants).
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Thinking off for this run (only means something when the model can think).
    #[serde(default)]
    pub thinking_off: bool,
    /// The sampling temperature; `None` keeps the model's own default.
    #[serde(default)]
    pub temperature: Option<f32>,
}

/// What one run is asked to do.
pub struct Spec {
    /// The task, the run's one user message.
    pub prompt: String,
    /// The terminal, for benchmarks. `None` offers the file tools only.
    pub terminal: Option<tools::TerminalConfig>,
    pub settings: Settings,
    pub step_cap: u32,
    /// The run is stopped after this long.
    pub time_limit: Duration,
    /// The app's own folders, never reachable.
    pub forbidden: Vec<String>,
}

/// Where audit rows go.
pub type AuditFn = Arc<dyn Fn(store::AuditEntry) + Send + Sync>;

/// One tool call of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CallRec {
    pub tool: String,
    /// `ok`, `error`, `refused`, ... ([`CallStatus::label`]).
    pub status: String,
    /// Refused by the run's rule (or by anything else that decides without running).
    pub blocked: bool,
    /// Read from the reply's text rather than sent as a native tool call.
    pub recovered: bool,
    /// The start of the result or of the reason, for the results table.
    pub detail: String,
    pub time_ms: u64,
}

impl CallRec {
    /// The call ran (or was looked at by its tool) and failed. A blocked call is not a
    /// tool failure.
    pub fn failed(&self) -> bool {
        self.status == CallStatus::Error.label()
    }
}

/// How the run's tool calls arrived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Arrival {
    /// No tool calls.
    None,
    /// Every call came as a native tool call.
    Native,
    /// Every call was read from the reply's text.
    Text,
    /// Some of each.
    Mixed,
}

/// Everything one run did.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    /// `completed`, `stopped`, `timeLimit`, `stepCap`, `mistakes`, `loop` or `failed`.
    pub status: String,
    /// Why the turn could not continue, for `failed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub steps: u32,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub time_ms: u64,
    pub calls: Vec<CallRec>,
    /// The model's final answer.
    pub answer: String,
}

impl Record {
    /// Edit and write calls that failed: an edit whose target was not found or was
    /// ambiguous, a write that was refused by its tool. Counted from each finished call's
    /// status, never from the wording of its error.
    pub fn failed_edits(&self) -> u32 {
        self.calls
            .iter()
            .filter(|c| matches!(c.tool.as_str(), "edit_file" | "write_file") && c.failed())
            .count() as u32
    }

    pub fn tool_failures(&self) -> u32 {
        self.calls.iter().filter(|c| c.failed()).count() as u32
    }

    pub fn blocked(&self) -> u32 {
        self.calls.iter().filter(|c| c.blocked).count() as u32
    }

    /// Calls of `tool` that ran and succeeded.
    pub fn succeeded(&self, tool: &str) -> u32 {
        self.calls
            .iter()
            .filter(|c| c.tool == tool && c.status == CallStatus::Ok.label())
            .count() as u32
    }

    pub fn arrival(&self) -> Arrival {
        let text = self.calls.iter().filter(|c| c.recovered).count();
        match (self.calls.len(), text) {
            (0, _) => Arrival::None,
            (_, 0) => Arrival::Native,
            (n, t) if n == t => Arrival::Text,
            _ => Arrival::Mixed,
        }
    }

    pub fn completed(&self) -> bool {
        self.status == "completed"
    }
}

/// The system prompt of a run.
const SYSTEM_PROMPT: &str = "You are a careful coding assistant. You work in one project folder with the tools you are given. Nobody can answer questions while you work: do the task, check your work where you can, then reply with a short summary of what you did.";

/// Runs one turn on `spec` in `workspace` and records it. Never panics on what the model,
/// the server or a tool does; a failure ends up in [`Record::status`].
pub async fn run(
    model: &Model,
    spec: Spec,
    workspace: &Workspace,
    cancel: CancellationToken,
    audit: AuditFn,
) -> Record {
    let started = Instant::now();
    let folder = workspace.display();
    let rules = Rules::new(
        folder.clone(),
        spec.forbidden.clone(),
        spec.terminal.is_some(),
    );
    let host = Host::new(HostOptions::default());
    host.add_builtin(Arc::new(tools::FilesServer::default()));
    if let Some(config) = spec.terminal {
        host.add_builtin(Arc::new(tools::TerminalServer::new(config)));
    }
    let result_cap = Arc::new(AtomicUsize::new(2000));
    let tools = RunTools {
        host,
        rules: rules.clone(),
        chat: ChatContext {
            chat_id: 0,
            approved_folders: vec![folder.clone()],
            working_folder: Some(folder.clone()),
            forbidden_folders: spec.forbidden,
            skill_folders: Vec::new(),
            skill_roots: Vec::new(),
            result_cap_tokens: 2000,
        },
        result_cap: result_cap.clone(),
    };
    let gate = Gate::new(rules);
    let approver = NobodyToAsk::default();
    let sink = Recorder {
        audit,
        result_cap,
        state: Mutex::default(),
    };

    let mut template = model.template.clone();
    if spec.settings.temperature.is_some() {
        template.temperature = spec.settings.temperature;
    }
    let mut input = TurnInput::new(
        vec![HistoryMessage::user(spec.prompt)],
        model.caps,
        template,
    );
    input.system_prompt = SYSTEM_PROMPT.into();
    input.step_cap = spec.step_cap;
    input.thinking = !spec.settings.thinking_off;
    input.preamble = Some(agent::prompt::Preamble {
        approved_folders: vec![folder],
        os: if cfg!(windows) {
            "Windows".into()
        } else if cfg!(target_os = "macos") {
            "macOS".into()
        } else {
            "Linux".into()
        },
        path_separator: if cfg!(windows) { "\\" } else { "/" }.into(),
    });

    // Stop, or the time limit, whichever comes first.
    let token = cancel.child_token();
    let timed_out = Arc::new(AtomicBool::new(false));
    let timer = {
        let token = token.clone();
        let timed_out = timed_out.clone();
        let limit = spec.time_limit;
        tokio::spawn(async move {
            tokio::select! {
                _ = token.cancelled() => {}
                _ = tokio::time::sleep(limit) => {
                    timed_out.store(true, Ordering::Relaxed);
                    token.cancel();
                }
            }
        })
    };
    let deps = TurnDeps {
        client: &model.client,
        tools: &tools,
        policy: &gate,
        approver: &approver,
        sink: &sink,
        sanitizer: &model.sanitizer,
    };
    let outcome = agent::turn::run_turn(deps, input, token.clone()).await;
    timer.abort();
    token.cancel();

    let mut record = sink.state.lock().map(|s| s.record.clone()).unwrap_or_default();
    if approver.asked() > 0 {
        // The gate never asks; should anything have, it was refused and is counted.
        tracing::warn!(asked = approver.asked(), "a headless run asked for approval");
    }
    record.prompt_tokens = outcome.usage.prompt_tokens;
    record.completion_tokens = outcome.usage.completion_tokens;
    record.answer = outcome
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::Assistant && !m.has_tool_calls())
        .map(HistoryMessage::text)
        .unwrap_or_default();
    record.status = match &outcome.status {
        TurnStatus::Completed => "completed",
        TurnStatus::Stopped if timed_out.load(Ordering::Relaxed) => "timeLimit",
        TurnStatus::Stopped => "stopped",
        TurnStatus::StepCapReached => "stepCap",
        TurnStatus::MistakeBudget => "mistakes",
        TurnStatus::LoopDetected => "loop",
        TurnStatus::Failed { message, .. } => {
            record.failure = Some(message.clone());
            "failed"
        }
    }
    .into();
    record.time_ms = started.elapsed().as_millis() as u64;
    record
}

// ---- The run's tools ------------------------------------------------------------------

/// The run's own tool host: the file tools, plus the terminal for benchmarks. Only the
/// tools the rule offers are listed to the model; a call to any other tool the host knows
/// (`delete_file`) is still described and reaches the gate, which refuses it.
struct RunTools {
    host: Host,
    rules: Rules,
    chat: ChatContext,
    result_cap: Arc<AtomicUsize>,
}

impl RunTools {
    fn chat(&self) -> ChatContext {
        let mut chat = self.chat.clone();
        chat.result_cap_tokens = self.result_cap.load(Ordering::Relaxed);
        chat
    }
}

#[async_trait]
impl ToolHost for RunTools {
    fn tools(&self) -> Vec<ToolInfo> {
        self.host
            .tools()
            .into_iter()
            .filter(|t| t.schema_error.is_none() && self.rules.offers(&t.server, &t.original))
            .map(|t| ToolInfo {
                name: t.exposed,
                description: t.def.description,
                input_schema: t.def.input_schema,
                read_only_hint: t.def.read_only,
                destructive_hint: t.def.destructive,
            })
            .collect()
    }
    fn validate(&self, name: &str, args: &Value) -> Result<(), Vec<String>> {
        self.host.validate(name, args)
    }
    fn llama_schema(&self, name: &str) -> Option<Value> {
        self.host.llama_schema(name)
    }
    async fn describe(&self, name: &str, args: &Map<String, Value>) -> Described {
        self.host
            .describe(name, args, &self.chat(), ToolPrefs::default())
            .await
    }
    async fn execute(
        &self,
        name: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        // The gate already decided; the rule is asked once more right before the tool
        // runs, so nothing reaches a tool that the rule would refuse.
        if let Verdict::Blocked(why) = self.rules.judge(action, false) {
            return ToolOutput::error(format!("Blocked without asking: {why}"));
        }
        self.host
            .call(
                name,
                args,
                action,
                CallContext {
                    chat: self.chat(),
                    cancel,
                    progress: None,
                },
            )
            .await
    }
}

// ---- What the run did ---------------------------------------------------------------

struct Recorder {
    audit: AuditFn,
    result_cap: Arc<AtomicUsize>,
    state: Mutex<RecorderState>,
}

#[derive(Default)]
struct RecorderState {
    record: Record,
    started: std::collections::HashMap<String, agent::history::CallRecord>,
}

fn blocked(decision: &CallDecision, status: &CallStatus) -> bool {
    matches!(
        decision,
        CallDecision::Refused { .. } | CallDecision::Denied { .. }
    ) || matches!(status, CallStatus::Refused | CallStatus::Denied { .. })
}

impl TurnSink for Recorder {
    fn event(&self, e: TurnEvent) {
        let Ok(mut st) = self.state.lock() else {
            return;
        };
        match e {
            TurnEvent::Budget { budget } => {
                self.result_cap
                    .store(budget.result_cap as usize, Ordering::Relaxed);
            }
            TurnEvent::StepStarted { step } => st.record.steps = st.record.steps.max(step),
            TurnEvent::ToolCallStarted { call } => {
                st.started.insert(call.id.clone(), call);
            }
            TurnEvent::ToolCallFinished {
                result,
                decision,
                action,
                duration_ms,
                ..
            } => {
                let call = st.started.remove(&result.call_id);
                let recovered = call.as_ref().is_some_and(|c| c.recovered);
                let arguments = call
                    .map(|c| Value::Object(c.arguments))
                    .unwrap_or(Value::Null);
                let is_blocked = blocked(&decision, &result.status);
                let detail = match &decision {
                    CallDecision::Refused { reason } if is_blocked => reason.clone(),
                    _ if result.text.is_empty() => result.notes.join(" "),
                    _ => result.text.clone(),
                };
                st.record.calls.push(CallRec {
                    tool: result.name.clone(),
                    status: result.status.label().to_owned(),
                    blocked: is_blocked,
                    recovered,
                    detail: policy::sanitize::one_line(&detail, 200),
                    time_ms: duration_ms,
                });
                let (_, mut entry) = crate::turns::audit_entry(
                    0,
                    arguments,
                    &result,
                    &decision,
                    action.as_ref(),
                    duration_ms,
                );
                // Not a chat's call: the row has no chat, and the run's own row is its
                // parent (set by the audit function).
                entry.chat_id = None;
                (self.audit)(entry);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
