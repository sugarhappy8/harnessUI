//! Agent turns in the app (ARCHITECTURE.md 6.1, 8, 13.3–13.7): connects the agent loop
//! to the MCP host, the policy engine, approval cards, the store and the audit log.
//!
//! - Policy state is read-only to the loop. Grants change only when the user answers a
//!   card with "Allow for this chat" (here, in Rust), never through a tool.
//! - An approval is resolved by a random id created in Rust and sent only to the main
//!   window.
//! - Every tool call gets one audit row.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use agent::history::{CallRecord, CallStatus, HistoryMessage, Part, ResultRecord};
use agent::turn::{
    Approval, ApprovalRequest, Approver, CallDecision, PolicyGate, ToolHost, TurnEvent, TurnSink,
};
use agent::{Role, ToolInfo};
use async_trait::async_trait;
use mcp_host::{CallContext, ChatContext, Described, ToolOutput};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ---- Storage format ---------------------------------------------------------------
//
// Message content is a JSON array of parts:
//   {"type":"text","text"}            {"type":"image","attachment_id"}
//   {"type":"document","attachment_id","name"}
//   {"type":"tool_calls","calls":[CallRecord]}
//   {"type":"tool_result","result":ResultRecord}

/// Stored content for an agent message (images from tool results are kept inline).
pub fn content_of(msg: &HistoryMessage) -> Value {
    let parts: Vec<Value> = msg
        .parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => json!({"type": "text", "text": t}),
            Part::Image { mime, data_base64 } => {
                json!({"type": "inline_image", "mime": mime, "data": data_base64})
            }
            Part::ToolCalls(calls) => json!({"type": "tool_calls", "calls": calls}),
            Part::ToolResult(r) => json!({"type": "tool_result", "result": r}),
            Part::Attachment { name, text } => {
                json!({"type": "inline_document", "name": name, "text": text})
            }
        })
        .collect();
    Value::Array(parts)
}

/// The searchable text of a message.
pub fn search_text_of(msg: &HistoryMessage) -> String {
    msg.parts
        .iter()
        .filter_map(|p| match p {
            Part::Text(t) => Some(t.clone()),
            Part::ToolResult(r) => Some(r.text.chars().take(2000).collect()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn role_of(r: store::Role) -> Role {
    match r {
        store::Role::System => Role::System,
        store::Role::User => Role::User,
        store::Role::Assistant => Role::Assistant,
        store::Role::Tool => Role::Tool,
    }
}

pub fn store_role(r: Role) -> store::Role {
    match r {
        Role::System => store::Role::System,
        Role::User => store::Role::User,
        Role::Assistant => store::Role::Assistant,
        Role::Tool => store::Role::Tool,
    }
}

/// Loads the chat path as agent history (attachments resolved to their data).
pub async fn history_of(
    state: &AppState,
    messages: &[store::Message],
) -> AppResult<Vec<HistoryMessage>> {
    let mut out = Vec::new();
    for m in messages {
        let mut parts = Vec::new();
        if let Some(arr) = m.content.as_array() {
            for p in arr {
                match p.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(t) = p.get("text").and_then(Value::as_str) {
                            parts.push(Part::Text(t.to_string()));
                        }
                    }
                    Some("image") => {
                        if let Some(id) = p.get("attachment_id").and_then(Value::as_i64) {
                            match crate::attachments::load_base64(state, id).await {
                                Ok((mime, data_base64)) => {
                                    parts.push(Part::Image { mime, data_base64 })
                                }
                                Err(_) => parts.push(Part::Text(
                                    "[an attached image is no longer available]".into(),
                                )),
                            }
                        }
                    }
                    Some("document") => {
                        if let Some(id) = p.get("attachment_id").and_then(Value::as_i64) {
                            let name = p
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or("document")
                                .to_string();
                            let text = state
                                .store
                                .get_attachment(store::AttachmentId(id))
                                .await?
                                .and_then(|a| a.extracted_text)
                                .unwrap_or_default();
                            parts.push(Part::Attachment { name, text });
                        }
                    }
                    Some("inline_image") => {
                        let (Some(mime), Some(data)) = (
                            p.get("mime").and_then(Value::as_str),
                            p.get("data").and_then(Value::as_str),
                        ) else {
                            continue;
                        };
                        parts.push(Part::Image {
                            mime: mime.into(),
                            data_base64: data.into(),
                        });
                    }
                    Some("inline_document") => {
                        let name = p
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("document")
                            .to_string();
                        let text = p
                            .get("text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        parts.push(Part::Attachment { name, text });
                    }
                    Some("tool_calls") => {
                        if let Ok(calls) = serde_json::from_value::<Vec<CallRecord>>(
                            p.get("calls").cloned().unwrap_or_default(),
                        ) {
                            parts.push(Part::ToolCalls(calls));
                        }
                    }
                    Some("tool_result") => {
                        if let Ok(r) = serde_json::from_value::<ResultRecord>(
                            p.get("result").cloned().unwrap_or_default(),
                        ) {
                            parts.push(Part::ToolResult(r));
                        }
                    }
                    _ => {}
                }
            }
        } else if let Some(t) = m.content.as_str() {
            parts.push(Part::Text(t.to_string()));
        }
        out.push(HistoryMessage {
            role: role_of(m.role),
            parts,
            reasoning: m.reasoning.clone(),
            id: Some(m.id.0),
        });
    }
    Ok(out)
}

// ---- Per-chat tool state ---------------------------------------------------------

/// What a chat has switched on, stored in `chats.tool_prefs`.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTools {
    /// Tools switched on (exposed names). `None` = the defaults for a new chat.
    #[serde(default)]
    pub enabled: Option<Vec<String>>,
    /// Thinking off for this chat (when the model can think). Default on.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub thinking_off: bool,
}

/// "Allow for this chat" grants, per chat. They live in memory only: they end when the
/// app quits or the user revokes them (D12), and are never written to disk.
#[derive(Default)]
pub struct Grants(Mutex<HashMap<i64, BTreeSet<policy::Scope>>>);

/// This chat's grants.
pub async fn load_grants(state: &AppState, chat: i64) -> BTreeSet<policy::Scope> {
    state
        .grants
        .0
        .lock()
        .ok()
        .and_then(|g| g.get(&chat).cloned())
        .unwrap_or_default()
}

/// Replaces this chat's grants.
pub async fn save_grants(
    state: &AppState,
    chat: i64,
    grants: &BTreeSet<policy::Scope>,
) -> AppResult<()> {
    if let Ok(mut g) = state.grants.0.lock() {
        if grants.is_empty() {
            g.remove(&chat);
        } else {
            g.insert(chat, grants.clone());
        }
    }
    Ok(())
}

/// Removes every grant for one external tool, in every chat.
pub fn revoke_external_tool(state: &AppState, server: &str, tool: &str) {
    if let Ok(mut all) = state.grants.0.lock() {
        for grants in all.values_mut() {
            grants.retain(|g| {
                !matches!(g,
                    policy::Scope::ExternalExact { server: s, tool: t, .. } | policy::Scope::ExternalAny { server: s, tool: t }
                    if s == server && t == tool)
            });
        }
    }
}

// ---- Approvals ----------------------------------------------------------------------

/// Cards waiting for an answer, by the random id sent to the main window.
#[derive(Default)]
pub struct PendingApprovals(Mutex<HashMap<String, PendingCard>>);

/// A card waiting for an answer, and which buttons it offered.
pub struct PendingCard {
    tx: tokio::sync::oneshot::Sender<Approval>,
    grantable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDto {
    pub id: String,
    pub chat_id: i64,
    pub call_id: String,
    pub server: String,
    pub tool: String,
    pub summary: String,
    pub preview: String,
    pub card: policy::CardKind,
    pub reason: policy::AskReason,
}

/// Answers a card. Only the main window can call this, and only with an id Rust made.
#[tauri::command]
pub async fn resolve_approval(
    app: AppHandle,
    id: String,
    decision: String,
    note: Option<String>,
) -> AppResult<()> {
    let approval = match decision.as_str() {
        "allowOnce" => Approval::AllowOnce,
        "allowForChat" => Approval::AllowForChat,
        "deny" => Approval::Deny {
            note: note.map(|n| n.chars().take(2000).collect()),
        },
        _ => return Err(AppError::bad_request("Unknown decision.")),
    };
    let pending = app.state::<PendingApprovals>();
    let card = pending.0.lock().ok().and_then(|mut m| m.remove(&id));
    match card {
        Some(card) => {
            // A once-only card has no "for this chat" button: such an answer is a bug
            // or forged, and counts as Allow once.
            let approval = if approval == Approval::AllowForChat && !card.grantable {
                Approval::AllowOnce
            } else {
                approval
            };
            let _ = card.tx.send(approval);
            Ok(())
        }
        None => Err(AppError::new(
            "approval_expired",
            "That approval is no longer waiting.",
            "Nothing to do.",
            String::new(),
        )),
    }
}

/// Removes a card's pending entry and tells the window to close it, however `ask` ends.
struct CardGuard {
    app: AppHandle,
    id: String,
}

impl Drop for CardGuard {
    fn drop(&mut self) {
        if let Ok(mut m) = self.app.state::<PendingApprovals>().0.lock() {
            m.remove(&self.id);
        }
        let _ = self
            .app
            .emit_to(crate::MAIN_WINDOW, "approval-closed", self.id.clone());
    }
}

struct AppApprover {
    app: AppHandle,
    chat_id: i64,
    cancel: CancellationToken,
    /// Shared with the policy gate so a new grant applies to the rest of the turn.
    grants: Arc<Mutex<BTreeSet<policy::Scope>>>,
}

/// An always-ask card raised while a call is running (ARCHITECTURE.md 13.3, "Approvals
/// raised during a call"), e.g. a navigation the browser holds. Allow once / Deny only;
/// Deny, Stop and a timeout all refuse. Writes its own audit row.
pub struct DuringCall {
    pub chat_id: i64,
    pub server: String,
    pub tool: String,
    pub summary: String,
    pub preview: String,
}

pub async fn ask_during_call(
    app: &AppHandle,
    held: DuringCall,
    cancel: CancellationToken,
    timeout: std::time::Duration,
) -> bool {
    let DuringCall {
        chat_id,
        server,
        tool,
        summary,
        preview,
    } = held;
    let id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = tokio::sync::oneshot::channel();
    if let Ok(mut m) = app.state::<PendingApprovals>().0.lock() {
        m.insert(
            id.clone(),
            PendingCard {
                tx,
                grantable: false,
            },
        );
    }
    let dto = ApprovalDto {
        id: id.clone(),
        chat_id,
        call_id: String::new(),
        server: server.clone(),
        tool: tool.clone(),
        summary: summary.clone(),
        preview: preview.clone(),
        card: policy::CardKind::OnceOnly,
        reason: policy::AskReason::AlwaysAsk,
    };
    let _ = app.emit_to(crate::MAIN_WINDOW, "approval-request", dto);
    let guard = CardGuard {
        app: app.clone(),
        id,
    };
    let answer = tokio::select! {
        r = rx => r.unwrap_or(Approval::Cancelled),
        _ = cancel.cancelled() => Approval::Cancelled,
        _ = tokio::time::sleep(timeout) => Approval::Cancelled,
    };
    drop(guard);
    let allowed = matches!(answer, Approval::AllowOnce | Approval::AllowForChat);
    let (decision, decided_by, note) = match &answer {
        Approval::AllowOnce | Approval::AllowForChat => {
            (store::AuditDecision::AllowOnce, "user", None)
        }
        Approval::Deny { note } => (store::AuditDecision::Denied, "user", note.clone()),
        Approval::Cancelled => (store::AuditDecision::Cancelled, "stop", None),
    };
    let state = app.state::<AppState>();
    let _ = state
        .store
        .append_audit(store::AuditEntry {
            ts: None,
            chat_id: Some(store::ChatId(chat_id)),
            server,
            tool,
            arguments: json!({ "held": summary }),
            preview: preview.chars().take(20_000).collect(),
            risk: "alwaysAsk".into(),
            decision,
            decided_by: decided_by.into(),
            note,
            parent_audit_id: None,
            result: None,
        })
        .await;
    allowed
}

#[async_trait]
impl Approver for AppApprover {
    async fn ask(&self, req: ApprovalRequest) -> Approval {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Ok(mut m) = self.app.state::<PendingApprovals>().0.lock() {
            m.insert(
                id.clone(),
                PendingCard {
                    tx,
                    grantable: req.card == policy::CardKind::Grantable,
                },
            );
        }
        let dto = ApprovalDto {
            id: id.clone(),
            chat_id: self.chat_id,
            call_id: req.call_id.clone(),
            server: req.action.server.clone(),
            tool: req.tool.clone(),
            summary: req.action.summary.clone(),
            preview: req.action.preview.clone(),
            card: req.card,
            reason: req.reason,
        };
        let _ = self
            .app
            .emit_to(crate::MAIN_WINDOW, "approval-request", dto);
        // The loop may drop this future (Stop): the guard still closes the card.
        let guard = CardGuard {
            app: self.app.clone(),
            id,
        };
        let answer = tokio::select! {
            r = rx => r.unwrap_or(Approval::Cancelled),
            _ = self.cancel.cancelled() => Approval::Cancelled,
        };
        drop(guard);
        if answer == Approval::AllowForChat && req.card == policy::CardKind::Grantable {
            let state = self.app.state::<AppState>();
            let mut grants = load_grants(&state, self.chat_id).await;
            match &req.action.scope {
                policy::Scope::Commands { parts } => grants.extend(parts.iter().cloned()),
                s => {
                    grants.insert(s.clone());
                }
            }
            let _ = save_grants(&state, self.chat_id, &grants).await;
            if let Ok(mut shared) = self.grants.lock() {
                *shared = grants;
            }
        }
        answer
    }
}

struct AppPolicy {
    grants: Arc<Mutex<BTreeSet<policy::Scope>>>,
    ready: Arc<dyn Fn(&str) -> bool + Send + Sync>,
}

impl PolicyGate for AppPolicy {
    fn decide(
        &self,
        tool: &str,
        action: &policy::Action,
        recovered: bool,
        untrusted_in_turn: bool,
    ) -> policy::Decision {
        let turn = policy::TurnContext {
            // Only enabled tools are offered; a call to anything else never gets here.
            tool_enabled: true,
            server_ready: (self.ready)(tool),
            recovered_from_text: recovered,
            untrusted_in_turn,
        };
        let grants = self.grants.lock().map(|g| g.clone()).unwrap_or_default();
        policy::evaluate(action, &turn, &grants)
    }
}

// ---- Tool host ---------------------------------------------------------------------

struct AppTools {
    host: mcp_host::Host,
    enabled: Vec<mcp_host::RegisteredTool>,
    chat: ChatContext,
    result_cap: Arc<AtomicUsize>,
    prefs: HashMap<String, mcp_host::ToolPrefs>,
    progress: Option<mcp_host::ProgressFn>,
}

#[async_trait]
impl ToolHost for AppTools {
    fn tools(&self) -> Vec<ToolInfo> {
        self.enabled
            .iter()
            .filter(|t| t.schema_error.is_none())
            .map(|t| ToolInfo {
                name: t.exposed.clone(),
                description: t.def.description.clone(),
                input_schema: t.def.input_schema.clone(),
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
        let mut chat = self.chat.clone();
        chat.result_cap_tokens = self.result_cap.load(Ordering::Relaxed);
        let prefs = self.prefs.get(name).cloned().unwrap_or_default();
        self.host.describe(name, args, &chat, prefs).await
    }
    async fn execute(
        &self,
        name: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let mut chat = self.chat.clone();
        chat.result_cap_tokens = self.result_cap.load(Ordering::Relaxed);
        self.host
            .call(
                name,
                args,
                action,
                CallContext {
                    chat,
                    cancel,
                    progress: self.progress.clone(),
                },
            )
            .await
    }
}

// ---- Events: UI and persistence --------------------------------------------------

/// What the frontend receives while a turn runs.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum TurnStreamEvent {
    Started {
        user_message_id: Option<i64>,
    },
    Step {
        step: u32,
    },
    Content {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Discarded,
    MessageSaved {
        message_id: i64,
        role: String,
    },
    ToolStarted {
        call_id: String,
        name: String,
        arguments: Value,
        recovered: bool,
    },
    ToolProgress {
        call_id: String,
        text: String,
    },
    ToolFinished {
        call_id: String,
        name: String,
        status: String,
        text: String,
        decision: String,
        duration_ms: u64,
        ui: Option<Value>,
    },
    Usage {
        prompt_tokens: u64,
        completion_tokens: u64,
        tokens_per_second: Option<f64>,
        context_used: Option<u32>,
        n_ctx: Option<u32>,
    },
    Note {
        text: String,
    },
    Compacted {
        stage: u8,
    },
    Finished {
        status: String,
        message: Option<String>,
    },
}

/// Token counts of the request that produced a message.
#[derive(Clone, Copy, Default)]
struct MessageUsage {
    tokens_per_second: Option<f64>,
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
}

enum Persist {
    Message(HistoryMessage, MessageUsage),
    /// The end of a turn that stopped or failed before the model answered: the partial
    /// reply, so the chat shows what happened and the history stays valid.
    Final {
        text: String,
        reasoning: Option<String>,
        status: store::MessageStatus,
        error: Option<Value>,
    },
    /// The turn was stopped after its last message was stored: mark that message stopped.
    MarkStopped,
    Audit(Box<store::AuditEntry>),
    Summary(String, Vec<i64>),
}

struct AppSink {
    channel: Channel<TurnStreamEvent>,
    persist: tokio::sync::mpsc::UnboundedSender<Persist>,
    result_cap: Arc<AtomicUsize>,
    chat_id: i64,
    last_usage: Mutex<MessageUsage>,
    started: Mutex<HashMap<String, CallRecord>>,
    /// Text and reasoning streamed since the last stored message.
    partial: Mutex<(String, String)>,
    /// The last stored message is an assistant answer (no tool calls).
    answered: std::sync::atomic::AtomicBool,
    deltas: Mutex<DeltaBuffer>,
}

fn decision_name(d: &CallDecision) -> (&'static str, store::AuditDecision, Option<String>) {
    match d {
        CallDecision::AllowedOnce => ("allowed once", store::AuditDecision::AllowOnce, None),
        CallDecision::AllowedForChat => (
            "allowed for this chat",
            store::AuditDecision::AllowForChat,
            None,
        ),
        CallDecision::AllowedByGrant => (
            "allowed by an earlier grant",
            store::AuditDecision::AllowedByGrant,
            None,
        ),
        CallDecision::Denied { note } => ("denied", store::AuditDecision::Denied, note.clone()),
        CallDecision::Cancelled => ("cancelled", store::AuditDecision::Cancelled, None),
        CallDecision::Refused { reason } => (
            "refused",
            store::AuditDecision::RefusedByPolicy,
            Some(reason.clone()),
        ),
        CallDecision::NotEvaluated => ("not run", store::AuditDecision::RefusedByPolicy, None),
    }
}

/// Streamed text not yet sent to the webview.
struct DeltaBuffer {
    content: String,
    reasoning: String,
    since: std::time::Instant,
}

impl Default for DeltaBuffer {
    fn default() -> Self {
        Self {
            content: String::new(),
            reasoning: String::new(),
            since: std::time::Instant::now(),
        }
    }
}

const DELTA_BATCH: std::time::Duration = std::time::Duration::from_millis(30);

impl AppSink {
    fn buffer_delta(&self, text: &str, reasoning: bool) {
        let due = {
            let Ok(mut b) = self.deltas.lock() else {
                return;
            };
            // Thinking and answer text keep their order: switch kind → flush first.
            let other_pending = if reasoning {
                !b.content.is_empty()
            } else {
                !b.reasoning.is_empty()
            };
            if other_pending {
                drop(b);
                self.flush_deltas();
                b = match self.deltas.lock() {
                    Ok(b) => b,
                    Err(_) => return,
                };
            }
            if b.content.is_empty() && b.reasoning.is_empty() {
                b.since = std::time::Instant::now();
            }
            if reasoning {
                b.reasoning.push_str(text);
            } else {
                b.content.push_str(text);
            }
            b.since.elapsed() >= DELTA_BATCH
        };
        if due {
            self.flush_deltas();
        }
    }

    fn flush_deltas(&self) {
        let (content, reasoning) = match self.deltas.lock() {
            Ok(mut b) => (
                std::mem::take(&mut b.content),
                std::mem::take(&mut b.reasoning),
            ),
            Err(_) => return,
        };
        if !reasoning.is_empty() {
            let _ = self
                .channel
                .send(TurnStreamEvent::Reasoning { text: reasoning });
        }
        if !content.is_empty() {
            let _ = self
                .channel
                .send(TurnStreamEvent::Content { text: content });
        }
    }
}

impl TurnSink for AppSink {
    fn event(&self, e: TurnEvent) {
        let send = |ev: TurnStreamEvent| {
            let _ = self.channel.send(ev);
        };
        // Text deltas are batched (about every 30 ms); anything else first flushes them,
        // so the webview always sees events in order.
        match &e {
            TurnEvent::ContentDelta { .. } | TurnEvent::ReasoningDelta { .. } => {}
            _ => self.flush_deltas(),
        }
        match e {
            TurnEvent::StepStarted { step } => send(TurnStreamEvent::Step { step }),
            TurnEvent::Budget { budget } => {
                self.result_cap
                    .store(budget.result_cap as usize, Ordering::Relaxed);
            }
            TurnEvent::ContentDelta { text } => {
                if let Ok(mut p) = self.partial.lock() {
                    p.0.push_str(&text);
                }
                self.buffer_delta(&text, false);
            }
            TurnEvent::ReasoningDelta { text } => {
                if let Ok(mut p) = self.partial.lock() {
                    p.1.push_str(&text);
                }
                self.buffer_delta(&text, true);
            }
            TurnEvent::AttemptDiscarded { .. } => {
                if let Ok(mut p) = self.partial.lock() {
                    *p = Default::default();
                }
                send(TurnStreamEvent::Discarded)
            }
            TurnEvent::MessageCompleted { message } => {
                if let Ok(mut p) = self.partial.lock() {
                    *p = Default::default();
                }
                let answered = message.role == Role::Assistant
                    && !message
                        .parts
                        .iter()
                        .any(|p| matches!(p, Part::ToolCalls(_)));
                self.answered.store(answered, Ordering::Relaxed);
                let usage = self.last_usage.lock().map(|u| *u).unwrap_or_default();
                let _ = self.persist.send(Persist::Message(message, usage));
            }
            TurnEvent::ToolCallStarted { call } => {
                send(TurnStreamEvent::ToolStarted {
                    call_id: call.id.clone(),
                    name: call.name.clone(),
                    arguments: Value::Object(call.arguments.clone()),
                    recovered: call.recovered,
                });
                if let Ok(mut s) = self.started.lock() {
                    s.insert(call.id.clone(), call);
                }
            }
            TurnEvent::ApprovalPending { .. } | TurnEvent::ApprovalResolved { .. } => {}
            TurnEvent::ToolCallFinished {
                result,
                decision,
                action,
                full_text,
                ui,
                duration_ms,
            } => {
                let (mut label, mut audit_decision, note) = decision_name(&decision);
                // Skills never prompt (D11): record them as such, not as a grant.
                if audit_decision == store::AuditDecision::AllowedByGrant
                    && action
                        .as_ref()
                        .is_some_and(|a| a.scope == policy::Scope::Skill)
                {
                    audit_decision = store::AuditDecision::AllowedWithoutPrompt;
                    label = "no approval needed";
                }
                send(TurnStreamEvent::ToolFinished {
                    call_id: result.call_id.clone(),
                    name: result.name.clone(),
                    status: result.status.label().to_string(),
                    text: full_text.clone().unwrap_or_else(|| result.text.clone()),
                    decision: label.to_string(),
                    duration_ms,
                    ui,
                });
                let call = self
                    .started
                    .lock()
                    .ok()
                    .and_then(|mut s| s.remove(&result.call_id));
                let arguments = call
                    .map(|c| Value::Object(c.arguments))
                    .unwrap_or(Value::Null);
                let (server, preview, risk) = match &action {
                    Some(a) => (
                        a.server.clone(),
                        a.preview.clone(),
                        format!("{:?}", a.risk).to_lowercase(),
                    ),
                    None => (String::new(), String::new(), "unknown".into()),
                };
                let entry = store::AuditEntry {
                    ts: None,
                    chat_id: Some(store::ChatId(self.chat_id)),
                    server,
                    tool: result.name.clone(),
                    arguments,
                    preview: preview.chars().take(20_000).collect(),
                    risk,
                    decision: audit_decision,
                    decided_by: match decision {
                        CallDecision::AllowedByGrant
                        | CallDecision::Refused { .. }
                        | CallDecision::NotEvaluated => "policy".into(),
                        CallDecision::Cancelled => "stop".into(),
                        _ => "user".into(),
                    },
                    note: note.or_else(|| match &result.status {
                        CallStatus::Denied { note } => note.clone(),
                        _ => None,
                    }),
                    parent_audit_id: None,
                    result: Some(store::AuditResult {
                        status: result.status.label().to_string(),
                        summary: Some(result.text.chars().take(500).collect()),
                        duration_ms: Some(duration_ms),
                    }),
                };
                let _ = self.persist.send(Persist::Audit(Box::new(entry)));
            }
            TurnEvent::AppNote { text } => send(TurnStreamEvent::Note { text }),
            TurnEvent::Usage { usage, timings } => {
                let tps = timings.as_ref().and_then(|t| t.predicted_per_second);
                if let Ok(mut u) = self.last_usage.lock() {
                    *u = MessageUsage {
                        tokens_per_second: tps,
                        prompt_tokens: Some(u64::from(usage.prompt_tokens)),
                        completion_tokens: Some(u64::from(usage.completion_tokens)),
                    };
                }
                send(TurnStreamEvent::Usage {
                    prompt_tokens: u64::from(usage.prompt_tokens),
                    completion_tokens: u64::from(usage.completion_tokens),
                    tokens_per_second: tps,
                    context_used: timings.as_ref().and_then(|t| t.context_used()),
                    n_ctx: None,
                })
            }
            TurnEvent::Compacted {
                stage,
                summary,
                covered_ids,
            } => {
                if let Some(s) = summary {
                    let _ = self.persist.send(Persist::Summary(s, covered_ids));
                }
                send(TurnStreamEvent::Compacted { stage });
            }
            TurnEvent::Finished { .. } => {}
        }
    }
}

/// Writes messages, audit rows and summaries in order, chaining each message to the
/// previous one. Returns the id of the last message written.
async fn persist_loop(
    state_app: AppHandle,
    chat_id: store::ChatId,
    mut parent: store::MessageId,
    model_ref: String,
    channel: Channel<TurnStreamEvent>,
    mut rx: tokio::sync::mpsc::UnboundedReceiver<Persist>,
) -> store::MessageId {
    let state = state_app.state::<AppState>();
    while let Some(p) = rx.recv().await {
        match p {
            Persist::Message(msg, usage) => {
                let role = store_role(msg.role);
                let inserted = state
                    .store
                    .insert_message(store::NewMessage {
                        chat_id,
                        parent_id: Some(parent),
                        role,
                        content: content_of(&msg),
                        reasoning: msg.reasoning.clone(),
                        status: store::MessageStatus::Complete,
                        model_ref: (role == store::Role::Assistant).then(|| model_ref.clone()),
                        search_text: search_text_of(&msg),
                    })
                    .await;
                match inserted {
                    Ok(id) => {
                        parent = id;
                        if role == store::Role::Assistant && usage.prompt_tokens.is_some() {
                            let _ = state
                                .store
                                .update_message(
                                    id,
                                    store::MessagePatch {
                                        tokens_per_second: usage.tokens_per_second,
                                        prompt_tokens: usage.prompt_tokens,
                                        completion_tokens: usage.completion_tokens,
                                        ..Default::default()
                                    },
                                )
                                .await;
                        }
                        let _ = channel.send(TurnStreamEvent::MessageSaved {
                            message_id: id.0,
                            role: role.as_str().to_string(),
                        });
                    }
                    Err(e) => tracing::error!("could not store a message: {e}"),
                }
            }
            Persist::Final {
                text,
                reasoning,
                status,
                error,
            } => {
                let inserted = state
                    .store
                    .insert_message(store::NewMessage {
                        chat_id,
                        parent_id: Some(parent),
                        role: store::Role::Assistant,
                        content: json!([{ "type": "text", "text": text }]),
                        reasoning,
                        status,
                        model_ref: Some(model_ref.clone()),
                        search_text: text.clone(),
                    })
                    .await;
                match inserted {
                    Ok(id) => {
                        parent = id;
                        if error.is_some() {
                            let _ = state
                                .store
                                .update_message(
                                    id,
                                    store::MessagePatch {
                                        error,
                                        ..Default::default()
                                    },
                                )
                                .await;
                        }
                        let _ = channel.send(TurnStreamEvent::MessageSaved {
                            message_id: id.0,
                            role: "assistant".into(),
                        });
                    }
                    Err(e) => tracing::error!("could not store a message: {e}"),
                }
            }
            Persist::MarkStopped => {
                let _ = state
                    .store
                    .update_message(
                        parent,
                        store::MessagePatch {
                            status: Some(store::MessageStatus::Stopped),
                            ..Default::default()
                        },
                    )
                    .await;
            }
            Persist::Audit(entry) => {
                if let Err(e) = state.store.append_audit(*entry).await {
                    tracing::error!("could not write the audit log: {e}");
                }
            }
            Persist::Summary(text, covered) => {
                let last = covered.iter().max().copied().unwrap_or(parent.0);
                let _ = state
                    .store
                    .insert_summary(store::NewSummary {
                        chat_id,
                        last_message_id: store::MessageId(last),
                        text,
                    })
                    .await;
            }
        }
    }
    parent
}

/// Everything the turn needs that the chat command already resolved.
pub struct TurnRequest {
    pub chat: store::Chat,
    /// The message the reply answers (the user's message).
    pub reply_to: store::MessageId,
    pub user_message_id: Option<store::MessageId>,
    pub client: agent::OpenAiClient,
    pub model_ref: String,
    pub caps: agent::turn::ModelCaps,
    pub template: agent::turn::RequestTemplate,
}

/// Runs one agent turn for a chat and streams it to `channel`.
pub async fn run(
    app: &AppHandle,
    req: TurnRequest,
    channel: Channel<TurnStreamEvent>,
    cancel: CancellationToken,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let chat_id = req.chat.id;
    let path = state.store.path_to(chat_id, req.reply_to).await?;
    let history = history_of(&state, &path).await?;

    // Tools enabled for this chat.
    let host = state.host.clone();
    crate::computer_cmd::prepare_turn(app, chat_id.0, &req.model_ref).await;
    let enabled = crate::mcp_cmd::enabled_for_chat(&state, &req.chat).await;
    let mut tool_prefs = HashMap::new();
    for t in &enabled {
        tool_prefs.insert(
            t.exposed.clone(),
            crate::mcp_cmd::tool_prefs_for(&state, &t.server, &t.original).await,
        );
    }
    let folders: Vec<String> = state
        .store
        .approved_folders_for_chat(chat_id)
        .await?
        .into_iter()
        .map(|f| f.path)
        .collect();
    let skills_srv = state.skills.clone();
    // One turn runs at a time, so the skills server's per-turn settings are global.
    skills_srv.set_context_tokens(req.caps.n_ctx as usize);
    skills_srv.set_terminal_enabled(enabled.iter().any(|t| t.server == "terminal"));
    let chat_ctx = ChatContext {
        chat_id: chat_id.0,
        working_folder: req
            .chat
            .working_folder
            .clone()
            .or_else(|| folders.first().cloned()),
        approved_folders: folders.clone(),
        forbidden_folders: crate::permissions::forbidden_for_tools(app),
        skill_folders: skills_srv
            .enabled()
            .iter()
            .map(|s| s.dir.display().to_string())
            .collect(),
        result_cap_tokens: 2000,
    };
    let result_cap = Arc::new(AtomicUsize::new(2000));
    let progress_channel = channel.clone();
    let tools = AppTools {
        host: host.clone(),
        enabled,
        chat: chat_ctx,
        result_cap: result_cap.clone(),
        prefs: tool_prefs,
        progress: Some(Arc::new(move |text: String| {
            let _ = progress_channel.send(TurnStreamEvent::ToolProgress {
                call_id: String::new(),
                text: text.chars().take(4000).collect(),
            });
        })),
    };
    let ready_host = host.clone();
    let grants = Arc::new(Mutex::new(load_grants(&state, chat_id.0).await));
    let policy = AppPolicy {
        grants: grants.clone(),
        ready: Arc::new(move |tool: &str| ready_host.tool_ready(tool)),
    };
    let approver = AppApprover {
        app: app.clone(),
        chat_id: chat_id.0,
        cancel: cancel.clone(),
        grants,
    };
    let (ptx, prx) = tokio::sync::mpsc::unbounded_channel();
    let sink = AppSink {
        channel: channel.clone(),
        persist: ptx,
        result_cap,
        chat_id: chat_id.0,
        last_usage: Mutex::new(MessageUsage::default()),
        started: Mutex::new(HashMap::new()),
        partial: Mutex::new(Default::default()),
        answered: std::sync::atomic::AtomicBool::new(false),
        deltas: Mutex::new(DeltaBuffer::default()),
    };
    let persister = tokio::spawn(persist_loop(
        app.clone(),
        chat_id,
        req.reply_to,
        req.model_ref.clone(),
        channel.clone(),
        prx,
    ));

    let default_prompt: Option<String> = state
        .store
        .get_setting(crate::settings::DEFAULT_SYSTEM_PROMPT)
        .await
        .ok()
        .flatten();
    let mut input = agent::turn::TurnInput::new(history, req.caps, req.template);
    let chat_prefs: ChatTools =
        serde_json::from_value(req.chat.tool_prefs.clone()).unwrap_or_default();
    input.thinking = !chat_prefs.thinking_off;
    input.system_prompt = req
        .chat
        .system_prompt
        .clone()
        .or(default_prompt)
        .unwrap_or_else(|| "You are a helpful assistant.".into());
    input.preamble = Some(agent::prompt::Preamble {
        approved_folders: folders,
        os: if cfg!(windows) {
            "Windows".into()
        } else {
            "macOS".into()
        },
        path_separator: if cfg!(windows) {
            "\\".into()
        } else {
            "/".into()
        },
    });
    let enabled_skills = skills_srv.enabled();
    if !enabled_skills.is_empty() {
        let refs: Vec<&tools::Skill> = enabled_skills.iter().collect();
        input.skills_index = Some(tools::skills::index_block(&refs));
    }
    input.loaded_skills = skills_srv
        .loaded_in(chat_id.0)
        .into_iter()
        .map(|s| agent::prompt::LoadedSkill {
            name: s.name.clone(),
            text: s.body.clone(),
        })
        .collect();
    input.summary = latest_summary(&state, chat_id, &path).await;
    let sanitizer = agent::prompt::Sanitizer::for_endpoint(&req.client).await;
    let _ = channel.send(TurnStreamEvent::Started {
        user_message_id: req.user_message_id.map(|m| m.0),
    });
    let deps = agent::turn::TurnDeps {
        client: &req.client,
        tools: &tools,
        policy: &policy,
        approver: &approver,
        sink: &sink,
        sanitizer: &sanitizer,
    };
    let outcome = agent::turn::run_turn(deps, input, cancel).await;
    let (partial_text, partial_reasoning) =
        sink.partial.lock().map(|p| p.clone()).unwrap_or_default();
    let final_message = match &outcome.status {
        agent::turn::TurnStatus::Stopped
            if !sink.answered.load(Ordering::Relaxed) || !partial_text.is_empty() =>
        {
            Some((store::MessageStatus::Stopped, None))
        }
        agent::turn::TurnStatus::Failed { code, message } => {
            let err = AppError::new(
                code,
                message.clone(),
                failure_next_step(code),
                String::new(),
            );
            Some((store::MessageStatus::Error, serde_json::to_value(err).ok()))
        }
        agent::turn::TurnStatus::Stopped => {
            // The partial answer was already stored as a message.
            let _ = sink.persist.send(Persist::MarkStopped);
            None
        }
        _ => None,
    };
    if let Some((status, error)) = final_message {
        let _ = sink.persist.send(Persist::Final {
            text: partial_text,
            reasoning: (!partial_reasoning.is_empty()).then_some(partial_reasoning),
            status,
            error,
        });
    }
    sink.flush_deltas();
    crate::computer_cmd::end_turn(app);
    drop(sink);
    let _ = persister.await;
    let (status, message) = match &outcome.status {
        agent::turn::TurnStatus::Completed => ("complete", None),
        agent::turn::TurnStatus::Stopped => ("stopped", None),
        agent::turn::TurnStatus::StepCapReached => (
            "stepCap",
            Some("The step limit for one turn was reached.".to_string()),
        ),
        agent::turn::TurnStatus::MistakeBudget => (
            "mistakes",
            Some("The model made too many invalid tool calls in a row.".to_string()),
        ),
        agent::turn::TurnStatus::LoopDetected => (
            "loop",
            Some("The model kept repeating the same tool call.".to_string()),
        ),
        agent::turn::TurnStatus::Failed { message, .. } => ("error", Some(message.clone())),
    };
    let _ = channel.send(TurnStreamEvent::Usage {
        prompt_tokens: outcome.usage.prompt_tokens,
        completion_tokens: outcome.usage.completion_tokens,
        tokens_per_second: outcome
            .usage
            .last_timings
            .as_ref()
            .and_then(|t| t.predicted_per_second),
        context_used: outcome.usage.last_prompt_tokens,
        n_ctx: outcome.budget.as_ref().map(|b| b.n_ctx),
    });
    let _ = channel.send(TurnStreamEvent::Finished {
        status: status.to_string(),
        message,
    });
    Ok(())
}

fn failure_next_step(code: &str) -> &'static str {
    match code {
        "tools_do_not_fit" => {
            "Switch off some tools for this chat, or load the model with a larger context."
        }
        "context_full" => "Start a new chat, or load the model with a larger context.",
        "output_limit" => "Ask for a shorter answer, or regenerate.",
        "no_answer" => "Regenerate, or try a different model.",
        _ => "Check that the model is still running, then regenerate.",
    }
}

async fn latest_summary(
    state: &AppState,
    chat_id: store::ChatId,
    path: &[store::Message],
) -> Option<agent::turn::CompactionSummary> {
    let ids: Vec<store::MessageId> = path.iter().map(|m| m.id).collect();
    let s = state
        .store
        .latest_summary_on_path(chat_id, &ids)
        .await
        .ok()
        .flatten()?;
    let covered: Vec<i64> = path
        .iter()
        .map(|m| m.id.0)
        .filter(|id| *id <= s.last_message_id.0)
        .collect();
    Some(agent::turn::CompactionSummary {
        text: s.text,
        covered_ids: covered,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_messages_are_stored_with_their_parts() {
        let call = CallRecord {
            id: "c1".into(),
            name: "read_file".into(),
            arguments: serde_json::from_value(json!({"path": "a.txt"})).unwrap(),
            recovered: false,
        };
        let msg = HistoryMessage {
            role: Role::Assistant,
            parts: vec![
                Part::Text("Reading it.".into()),
                Part::ToolCalls(vec![call.clone()]),
            ],
            reasoning: None,
            id: None,
        };
        let content = content_of(&msg);
        assert_eq!(content[0], json!({"type": "text", "text": "Reading it."}));
        assert_eq!(content[1]["type"], "tool_calls");
        let back: Vec<CallRecord> = serde_json::from_value(content[1]["calls"].clone()).unwrap();
        assert_eq!(back, vec![call]);
        assert_eq!(search_text_of(&msg), "Reading it.");
    }

    #[test]
    fn decisions_map_to_audit_values() {
        assert_eq!(
            decision_name(&CallDecision::AllowedOnce).1,
            store::AuditDecision::AllowOnce
        );
        assert_eq!(
            decision_name(&CallDecision::AllowedByGrant).1,
            store::AuditDecision::AllowedByGrant
        );
        let (_, d, note) = decision_name(&CallDecision::Denied {
            note: Some("no".into()),
        });
        assert_eq!(
            (d, note.as_deref()),
            (store::AuditDecision::Denied, Some("no"))
        );
        assert_eq!(
            decision_name(&CallDecision::Cancelled).1,
            store::AuditDecision::Cancelled
        );
    }

    #[test]
    fn roles_round_trip() {
        for r in [Role::System, Role::User, Role::Assistant, Role::Tool] {
            assert_eq!(role_of(store_role(r)), r);
        }
    }
}
