//! Agent turns in the app (ARCHITECTURE.md 6.1, 8, 13.3–13.7): connects the agent loop
//! to the MCP host, the policy engine, approval cards, the store and the audit log.
//!
//! - Policy state is read-only to the loop. Grants change only when the user answers a
//!   card with "Allow for this chat" (here, in Rust), never through a tool.
//! - An approval is resolved by a random id created in Rust and sent only to the main
//!   window.
//! - Every tool call gets one audit row.
//! - `run_subagent` runs here, not in a tool server: [`AppTools`] starts a helper turn
//!   (`subagent.rs`) with this chat's tools minus that one, behind a policy gate, an
//!   approver and an audit trail built for the same chat, so a helper's calls are decided,
//!   shown and logged exactly like the turn's own.

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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
///
/// This is the only copy: the policy gate of a running turn reads it on every decision.
#[derive(Default)]
pub struct Grants(Mutex<HashMap<i64, BTreeSet<policy::Scope>>>);

impl Grants {
    /// One chat's grants.
    pub fn of_chat(&self, chat: i64) -> BTreeSet<policy::Scope> {
        self.0
            .lock()
            .ok()
            .and_then(|g| g.get(&chat).cloned())
            .unwrap_or_default()
    }

    /// Replaces one chat's grants.
    pub fn replace(&self, chat: i64, grants: &BTreeSet<policy::Scope>) {
        if let Ok(mut g) = self.0.lock() {
            if grants.is_empty() {
                g.remove(&chat);
            } else {
                g.insert(chat, grants.clone());
            }
        }
    }

    /// Adds grants to one chat.
    pub fn add(&self, chat: i64, scopes: Vec<policy::Scope>) {
        if scopes.is_empty() {
            return;
        }
        if let Ok(mut g) = self.0.lock() {
            g.entry(chat).or_default().extend(scopes);
        }
    }

    /// Removes every grant for one external tool, in every chat.
    pub fn revoke_external(&self, server: &str, tool: &str) {
        if let Ok(mut all) = self.0.lock() {
            for grants in all.values_mut() {
                grants.retain(|g| {
                    !matches!(g,
                        policy::Scope::ExternalExact { server: s, tool: t, .. } | policy::Scope::ExternalAny { server: s, tool: t }
                        if s == server && t == tool)
                });
            }
        }
    }
}

/// Replaces this chat's grants.
pub async fn save_grants(
    state: &AppState,
    chat: i64,
    grants: &BTreeSet<policy::Scope>,
) -> AppResult<()> {
    state.grants.replace(chat, grants);
    Ok(())
}

/// Removes every grant for one external tool, in every chat.
pub fn revoke_external_tool(state: &AppState, server: &str, tool: &str) {
    state.grants.revoke_external(server, tool);
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

/// Stop: every card still waiting (including ones the browser raised outside a call) is
/// answered as cancelled.
pub fn cancel_all_pending(app: &AppHandle) {
    let cards: Vec<PendingCard> = app
        .state::<PendingApprovals>()
        .0
        .lock()
        .map(|mut m| m.drain().map(|(_, c)| c).collect())
        .unwrap_or_default();
    for card in cards {
        let _ = card.tx.send(Approval::Cancelled);
    }
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
    /// Set for a helper's approver: the id of the `run_subagent` call the helper works
    /// for. Its cards say they come from a subagent and belong to that call's tool card.
    helper_of: Option<String>,
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
    // A navigation held while a helper works in this chat came from the helper: the card
    // and the audit row say so.
    let helper = crate::subagent::running_in(chat_id);
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
        summary: match helper {
            Some(_) => crate::subagent::card_summary(&summary),
            None => summary.clone(),
        },
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
    let mut entry = store::AuditEntry {
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
    };
    if let Some(row) = helper {
        crate::subagent::mark_audit(&mut entry, row);
    }
    let _ = state.store.append_audit(entry).await;
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
        let (call_id, summary) = match &self.helper_of {
            Some(parent_call) => (
                parent_call.clone(),
                crate::subagent::card_summary(&req.action.summary),
            ),
            None => (req.call_id.clone(), req.action.summary.clone()),
        };
        let dto = ApprovalDto {
            id: id.clone(),
            chat_id: self.chat_id,
            call_id,
            server: req.action.server.clone(),
            tool: req.tool.clone(),
            summary,
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
            let scopes = match &req.action.scope {
                policy::Scope::Commands { parts } => parts.clone(),
                s => vec![s.clone()],
            };
            // Added under the lock, so a "Forget" at the same moment is not undone. The
            // policy gate reads the app state, so this applies to the turn's next call.
            self.app
                .state::<AppState>()
                .grants
                .add(self.chat_id, scopes);
        }
        answer
    }
}

/// The policy gate of one turn. It reads the chat's grants from the app state on every
/// decision and keeps no copy, so a grant given during the turn applies to its next call,
/// and "Forget everything I allowed in this chat" or a revoked tool apply at once, also in
/// a turn that is already running (D12).
struct AppPolicy {
    grants: Arc<Grants>,
    chat_id: i64,
    ready: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    /// What the turn's other pieces need to know about its calls (see [`TurnShared`]).
    shared: Arc<TurnShared>,
}

impl AppPolicy {
    fn for_chat(
        grants: Arc<Grants>,
        chat_id: i64,
        ready: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        Self {
            grants,
            chat_id,
            ready,
            shared: Arc::default(),
        }
    }

    fn sharing(mut self, shared: Arc<TurnShared>) -> Self {
        self.shared = shared;
        self
    }
}

/// What the pieces of one turn (policy gate, tool host, sink) tell each other. Calls run
/// one at a time, so "the current call" is well defined.
#[derive(Default)]
struct TurnShared {
    /// Untrusted content has entered the turn (D9), as the loop last told the policy gate.
    untrusted: AtomicBool,
    /// The `run_subagent` call being decided was recovered from text (8.3).
    subagent_recovered: AtomicBool,
    /// The id of the call being processed.
    current_call: Mutex<String>,
    /// The audit row already written for the running `run_subagent` call, by call id. Its
    /// result is filled in when the call finishes (instead of a second row).
    open_audit: Mutex<Option<(String, store::AuditId)>>,
}

impl TurnShared {
    fn current_call(&self) -> String {
        self.current_call
            .lock()
            .map(|c| c.clone())
            .unwrap_or_default()
    }

    /// What a helper started now inherits from the turn.
    fn inherited(&self) -> crate::subagent::Inherited {
        crate::subagent::Inherited {
            untrusted: self.untrusted.load(Ordering::Relaxed),
            recovered: self.subagent_recovered.load(Ordering::Relaxed),
        }
    }

    /// The audit row opened for `call_id`, if any. Taken once.
    fn take_open_audit(&self, call_id: &str) -> Option<store::AuditId> {
        let mut open = self.open_audit.lock().ok()?;
        match open.take() {
            Some((id, row)) if id == call_id => Some(row),
            other => {
                *open = other;
                None
            }
        }
    }
}

impl PolicyGate for AppPolicy {
    fn decide(
        &self,
        tool: &str,
        action: &policy::Action,
        recovered: bool,
        untrusted_in_turn: bool,
    ) -> policy::Decision {
        // A helper started by this turn starts where the turn is: with untrusted content
        // already read, and from a call written as text, its actions ask as the turn's
        // own would (D9, 8.3).
        if untrusted_in_turn {
            self.shared.untrusted.store(true, Ordering::Relaxed);
        }
        if crate::subagent::is_action(action) {
            self.shared
                .subagent_recovered
                .store(recovered, Ordering::Relaxed);
        }
        // [0.0.4 allow all] the chat's approval mode goes into the turn context here.
        let turn = policy::TurnContext {
            // Only enabled tools are offered; a call to anything else never gets here.
            tool_enabled: true,
            server_ready: (self.ready)(tool),
            recovered_from_text: recovered,
            untrusted_in_turn,
        };
        let grants = self.grants.of_chat(self.chat_id);
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
    /// What `run_subagent` needs to start a helper. `None` in a helper's own tool host: a
    /// helper cannot start another.
    helper: Option<HelperSetup>,
    /// The app, for the hooks that store or track what a call did.
    app: AppHandle,
    /// Tools that are on in the chat but not shown to the model this turn (adaptive tool
    /// selection, `tool_select.rs`): `find_tools` lists them and `call_tool` runs them.
    hidden: Vec<mcp_host::RegisteredTool>,
    /// Shared with the policy gate and the sink: the id of the call in progress.
    shared: Arc<TurnShared>,
}

/// What a helper takes over from the turn that starts it (see `subagent.rs`).
struct HelperSetup {
    app: AppHandle,
    client: agent::OpenAiClient,
    sanitizer: agent::Sanitizer,
    /// The turn's input without its history: system prompt, preamble, skills, model
    /// capabilities and request template.
    input: agent::turn::TurnInput,
    channel: Channel<TurnStreamEvent>,
    persist: tokio::sync::mpsc::UnboundedSender<Persist>,
    grants: Arc<Grants>,
    shared: Arc<TurnShared>,
}

impl AppTools {
    /// Runs one `run_subagent` call: a helper turn in the same chat, with the chat's
    /// tools minus this one, behind the same policy gate, grants, approval cards and
    /// audit log. `cancel` is the call's token, so Stop ends the helper and whatever it
    /// is running.
    async fn run_subagent(
        &self,
        setup: &HelperSetup,
        args: &Map<String, Value>,
        action: &policy::Action,
        cancel: CancellationToken,
    ) -> ToolOutput {
        let state = setup.app.state::<AppState>();
        let chat_id = self.chat.chat_id;
        let call_id = setup.shared.current_call();
        // The call's audit row is written before the helper starts, so the rows of the
        // helper's own calls can point at it; its result is filled in when it finishes.
        let row = state
            .store
            .append_audit(store::AuditEntry {
                ts: None,
                chat_id: Some(store::ChatId(chat_id)),
                server: action.server.clone(),
                tool: action.tool.clone(),
                arguments: Value::Object(args.clone()),
                preview: action.preview.chars().take(20_000).collect(),
                risk: format!("{:?}", action.risk).to_lowercase(),
                decision: store::AuditDecision::AllowedWithoutPrompt,
                decided_by: "policy".into(),
                note: None,
                parent_audit_id: None,
                result: None,
            })
            .await;
        let row = match row {
            Ok(row) => {
                if let Ok(mut open) = setup.shared.open_audit.lock() {
                    *open = Some((call_id.clone(), row));
                }
                Some(row)
            }
            Err(e) => {
                tracing::error!("could not write the audit log: {e}");
                None
            }
        };
        let _running = crate::subagent::enter(chat_id, row);

        // The helper's own budget sets its per-result cap; the turn's stays as it is.
        let result_cap = Arc::new(AtomicUsize::new(self.result_cap.load(Ordering::Relaxed)));
        let tools = AppTools {
            host: self.host.clone(),
            enabled: self
                .enabled
                .iter()
                .filter(|t| !crate::subagent::is_tool(t))
                .cloned()
                .collect(),
            chat: self.chat.clone(),
            result_cap: result_cap.clone(),
            prefs: self.prefs.clone(),
            // The card shows one short line per call of the helper, not each tool's output.
            progress: None,
            helper: None,
            app: setup.app.clone(),
            hidden: self
                .hidden
                .iter()
                .filter(|t| !crate::subagent::is_tool(t))
                .cloned()
                .collect(),
            shared: Arc::default(),
        };
        let ready_host = self.host.clone();
        let policy = AppPolicy::for_chat(
            setup.grants.clone(),
            chat_id,
            Arc::new(move |tool: &str| ready_host.tool_ready(tool)),
        );
        let approver = AppApprover {
            app: setup.app.clone(),
            chat_id,
            cancel: cancel.clone(),
            helper_of: Some(call_id.clone()),
        };
        let mut input = setup.input.clone();
        // Skills loaded in the chat so far, also during this turn: `load_skill` answers
        // "already loaded" for them, so their text must be in the helper's system message.
        input.loaded_skills = state
            .skills
            .loaded_in(chat_id)
            .into_iter()
            .map(|s| agent::prompt::LoadedSkill {
                name: s.name.clone(),
                text: s.body.clone(),
            })
            .collect();
        let channel = setup.channel.clone();
        let persist = setup.persist.clone();
        let helper = crate::subagent::Helper {
            chat_id,
            client: setup.client.clone(),
            sanitizer: setup.sanitizer.clone(),
            tools: Arc::new(tools),
            policy: Arc::new(policy),
            approver: Arc::new(approver),
            input,
            inherited: setup.shared.inherited(),
            progress: Arc::new(move |text: String| {
                let _ = channel.send(TurnStreamEvent::ToolProgress {
                    call_id: call_id.clone(),
                    text: text.chars().take(4000).collect(),
                });
            }),
            audit: Arc::new(move |entry: store::AuditEntry| {
                let _ = persist.send(Persist::Audit(Box::new(entry)));
            }),
            parent_audit: row,
            result_cap,
        };
        crate::subagent::run(helper, args, cancel).await
    }
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
        // Built-in names are fixed (an external tool can never hold one), so this is the
        // app's own tool. It runs here, not in its server: a helper needs the turn's
        // client, policy gate, approver and sink.
        if name == crate::subagent::TOOL_NAME {
            let on = self.enabled.iter().any(crate::subagent::is_tool);
            return match &self.helper {
                Some(setup) if on => self.run_subagent(setup, args, action, cancel).await,
                _ => ToolOutput::error(crate::subagent::NESTED_REFUSAL),
            };
        }
        // [0.0.4 tool selection] `find_tools` and `call_tool` are the app's own tools too and
        // are run from here (see `tool_select.rs`).
        let mut chat = self.chat.clone();
        chat.result_cap_tokens = self.result_cap.load(Ordering::Relaxed);
        let chat_id = chat.chat_id;
        let mut out = self
            .host
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
            .await;
        // ---- After a call. One line per feature, each a call into that feature's module. ----
        let call_id = self.shared.current_call();
        // [0.0.4 outputs] keeps the full text of a shortened result and says how to read it.
        crate::outputs::after_call(&self.app, chat_id, &call_id, name, &mut out).await;
        // [0.0.4 task] notes changed files and completed checks in the chat's task.
        crate::task::after_call(&self.app, chat_id, name, args, action, &out).await;
        out
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
    /// Fills in the result of an audit row written when its call started.
    CompleteAudit(store::AuditId, store::AuditResult),
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
    /// The agent loop's end-of-turn placeholder, held back until the turn's status is
    /// known (see [`ending`]).
    filler: Mutex<Option<(HistoryMessage, MessageUsage)>>,
    deltas: Mutex<DeltaBuffer>,
    shared: Arc<TurnShared>,
}

/// True for the "(stopped)" placeholder the agent loop appends when a turn ends without an
/// answer. It is not an answer: storing it as one would show "Stopped" above every error
/// and throw away the text streamed before a failure.
fn is_end_filler(message: &HistoryMessage) -> bool {
    message.role == Role::Assistant
        && matches!(message.parts.as_slice(), [Part::Text(text)] if text == agent::history::STOPPED_TEXT)
}

/// What is stored when a turn ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ending {
    /// One last assistant message with this status and the text streamed so far.
    Final(store::MessageStatus),
    /// The partial answer is already stored: mark it stopped.
    MarkStopped,
    /// No answer and no error to show: the placeholder keeps the history valid.
    StoreFiller,
    Nothing,
}

fn ending(
    status: &agent::turn::TurnStatus,
    answered: bool,
    has_partial: bool,
    has_filler: bool,
) -> Ending {
    match status {
        agent::turn::TurnStatus::Failed { .. } => Ending::Final(store::MessageStatus::Error),
        agent::turn::TurnStatus::Stopped if !answered || has_partial => {
            Ending::Final(store::MessageStatus::Stopped)
        }
        agent::turn::TurnStatus::Stopped => Ending::MarkStopped,
        _ if has_filler => Ending::StoreFiller,
        _ => Ending::Nothing,
    }
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

/// The audit row for one finished call (13.7), and the decision as its tool card words it.
/// Shared by the turn's sink and a helper's (`subagent.rs`), so both write the same rows.
pub(crate) fn audit_entry(
    chat_id: i64,
    arguments: Value,
    result: &ResultRecord,
    decision: &CallDecision,
    action: Option<&policy::Action>,
    duration_ms: u64,
) -> (&'static str, store::AuditEntry) {
    let (mut label, mut audit_decision, note) = decision_name(decision);
    // Skills and `run_subagent` never prompt (D11): record them as such, not as a grant.
    if audit_decision == store::AuditDecision::AllowedByGrant
        && action.is_some_and(|a| matches!(a.scope, policy::Scope::Skill | policy::Scope::AppData))
    {
        audit_decision = store::AuditDecision::AllowedWithoutPrompt;
        label = "no approval needed";
    }
    let (server, preview, risk) = match action {
        Some(a) => (
            a.server.clone(),
            a.preview.clone(),
            format!("{:?}", a.risk).to_lowercase(),
        ),
        None => (String::new(), String::new(), "unknown".into()),
    };
    let entry = store::AuditEntry {
        ts: None,
        chat_id: Some(store::ChatId(chat_id)),
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
    (label, entry)
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
            TurnEvent::MessageCompleted { message } if is_end_filler(&message) => {
                // Held back: whether it is stored depends on how the turn ended, and the
                // text streamed so far must survive for the error or stopped message.
                let usage = self.last_usage.lock().map(|u| *u).unwrap_or_default();
                if let Ok(mut f) = self.filler.lock() {
                    *f = Some((message, usage));
                }
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
                if let Ok(mut current) = self.shared.current_call.lock() {
                    current.clone_from(&call.id);
                }
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
                let call = self
                    .started
                    .lock()
                    .ok()
                    .and_then(|mut s| s.remove(&result.call_id));
                let arguments = call
                    .map(|c| Value::Object(c.arguments))
                    .unwrap_or(Value::Null);
                let (label, entry) = audit_entry(
                    self.chat_id,
                    arguments,
                    &result,
                    &decision,
                    action.as_ref(),
                    duration_ms,
                );
                send(TurnStreamEvent::ToolFinished {
                    call_id: result.call_id.clone(),
                    name: result.name.clone(),
                    status: result.status.label().to_string(),
                    text: full_text
                        .clone()
                        .unwrap_or_else(|| shown_result_text(&result)),
                    decision: label.to_string(),
                    duration_ms,
                    ui,
                });
                // A `run_subagent` call that ran wrote its row when it started: only its
                // result is filled in now.
                let opened = self.shared.take_open_audit(&result.call_id);
                let _ = self.persist.send(match (opened, entry.result.clone()) {
                    (Some(row), Some(outcome)) => Persist::CompleteAudit(row, outcome),
                    _ => Persist::Audit(Box::new(entry)),
                });
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
            Persist::CompleteAudit(row, outcome) => {
                if let Err(e) = state
                    .store
                    .complete_audit(row, outcome.status, outcome.summary, outcome.duration_ms)
                    .await
                {
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

/// The folder relative paths resolve against (ARCHITECTURE.md 9.3): the one the user
/// chose while it is still approved for the chat, otherwise the first approved folder.
fn working_folder_for(stored: Option<&str>, approved: &[String]) -> Option<String> {
    stored
        .filter(|w| approved.iter().any(|a| a == w))
        .or_else(|| approved.first().map(String::as_str))
        .map(str::to_string)
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
    let enabled = crate::mcp_cmd::enabled_for_chat(&state, &req.chat, Some(req.caps.n_ctx)).await;
    // [0.0.4 tool selection] Of the tools that are on in the chat, the ones shown to the
    // model this turn. The rest stay reachable through `find_tools` and `call_tool`.
    let (mut enabled, hidden) =
        crate::tool_select::for_turn(&state, &req.chat, &history, enabled).await;
    // `run_subagent` goes last, so a helper's tool list (the same without it) is a prefix
    // of the turn's and the model server can reuse more of its cached prompt (8.5).
    enabled.sort_by_key(crate::subagent::is_tool);
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
        working_folder: working_folder_for(req.chat.working_folder.as_deref(), &folders),
        approved_folders: folders.clone(),
        forbidden_folders: crate::permissions::forbidden_for_tools(app),
        skill_folders: skills_srv
            .enabled()
            .iter()
            .map(|s| s.dir.display().to_string())
            .collect(),
        // Where skills are loaded from (as in `toolset::reload_skills`): the file and git
        // tools never write there.
        skill_roots: {
            let shared: bool = state
                .store
                .get_setting(crate::toolset::SHARED_SKILLS)
                .await
                .ok()
                .flatten()
                .unwrap_or(false);
            let mut roots: Vec<String> = crate::toolset::skills_root(app)
                .map(|r| r.display().to_string())
                .into_iter()
                .collect();
            if shared && let Ok(home) = app.path().home_dir() {
                roots.push(home.join(".agents").join("skills").display().to_string());
            }
            roots
        },
        result_cap_tokens: 2000,
    };
    let result_cap = Arc::new(AtomicUsize::new(2000));
    let shared = Arc::new(TurnShared::default());
    let progress_channel = channel.clone();
    let subagents_on = enabled.iter().any(crate::subagent::is_tool);
    let mut tools = AppTools {
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
        // Set below, once the turn's input exists.
        helper: None,
        app: app.clone(),
        hidden,
        shared: shared.clone(),
    };
    let ready_host = host.clone();
    let policy = AppPolicy::for_chat(
        state.grants.clone(),
        chat_id.0,
        Arc::new(move |tool: &str| ready_host.tool_ready(tool)),
    )
    .sharing(shared.clone());
    let approver = AppApprover {
        app: app.clone(),
        chat_id: chat_id.0,
        cancel: cancel.clone(),
        helper_of: None,
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
        filler: Mutex::new(None),
        deltas: Mutex::new(DeltaBuffer::default()),
        shared: shared.clone(),
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
    let local_endpoint = req.caps.local;
    let mut input = agent::turn::TurnInput::new(history, req.caps, req.template);
    let stored_steps: Option<serde_json::Value> = state
        .store
        .get_setting(crate::settings::STEPS_PER_TURN)
        .await
        .ok()
        .flatten();
    input.step_cap = crate::settings::steps_per_turn(stored_steps.as_ref());
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
    let sanitizer = agent::prompt::Sanitizer::for_endpoint(&req.client).await;
    if subagents_on {
        // A helper gets the same system prompt, preamble and skills, the same model and
        // request settings, and none of the chat: no history and no summary.
        let history = std::mem::take(&mut input.history);
        let helper_input = input.clone();
        input.history = history;
        tools.helper = Some(HelperSetup {
            app: app.clone(),
            client: req.client.clone(),
            sanitizer: sanitizer.clone(),
            input: helper_input,
            channel: channel.clone(),
            persist: sink.persist.clone(),
            grants: state.grants.clone(),
            shared: shared.clone(),
        });
    }
    input.summary = latest_summary(&state, chat_id, &path).await;
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
    let filler = sink.filler.lock().ok().and_then(|mut f| f.take());
    match ending(
        &outcome.status,
        sink.answered.load(Ordering::Relaxed),
        !partial_text.is_empty(),
        filler.is_some(),
    ) {
        Ending::Final(status) => {
            let error = match &outcome.status {
                agent::turn::TurnStatus::Failed { code, message } => {
                    serde_json::to_value(AppError::new(
                        code,
                        message.clone(),
                        failure_next_step(code, local_endpoint),
                        String::new(),
                    ))
                    .ok()
                }
                _ => None,
            };
            let _ = sink.persist.send(Persist::Final {
                text: partial_text,
                reasoning: (!partial_reasoning.is_empty()).then_some(partial_reasoning),
                status,
                error,
            });
        }
        // The partial answer was already stored as a message.
        Ending::MarkStopped => {
            let _ = sink.persist.send(Persist::MarkStopped);
        }
        Ending::StoreFiller => {
            if let Some((message, usage)) = filler {
                let _ = sink.persist.send(Persist::Message(message, usage));
            }
        }
        Ending::Nothing => {}
    }
    sink.flush_deltas();
    crate::computer_cmd::end_turn(app);
    // The persister ends when every sender is gone: the sink's and the one kept for helpers.
    drop(tools);
    drop(sink);
    let _ = persister.await;
    let (status, message) = match &outcome.status {
        agent::turn::TurnStatus::Completed => ("complete", None),
        agent::turn::TurnStatus::Stopped => ("stopped", None),
        agent::turn::TurnStatus::StepCapReached => (
            "stepCap",
            Some("The model reached the step limit for one turn.".to_string()),
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

/// The advice shown under a failed turn. `local` is true for the app's own llama-server;
/// the app does not run a remote endpoint or choose its context size.
fn failure_next_step(code: &str, local: bool) -> &'static str {
    match (code, local) {
        ("tools_do_not_fit", true) => {
            "Switch off some tools for this chat, or load the model with a larger context."
        }
        ("tools_do_not_fit", false) => {
            "Switch off some tools for this chat. If the server's model has a larger context, set it in Settings → Endpoints."
        }
        ("context_full" | "context_overflow", true) => {
            "Start a new chat, or load the model with a larger context."
        }
        ("context_full" | "context_overflow", false) => {
            "Start a new chat. If the server's model has a larger context, set it in Settings → Endpoints."
        }
        ("unauthorized", _) => "Check the API key in Settings → Endpoints, then regenerate.",
        ("output_limit", _) => "Ask for a shorter answer, or regenerate.",
        ("no_answer", _) => "Regenerate, or try a different model.",
        (_, true) => "Check that the model is still running, then regenerate.",
        (_, false) => {
            "Check that the server is running and its address in Settings → Endpoints is right, then regenerate."
        }
    }
}

/// What a tool card shows as the result: the tool's output, or, for a call that did not
/// run (invalid arguments, refused, denied, switched off), the app's notes saying why.
pub fn shown_result_text(result: &ResultRecord) -> String {
    if result.text.trim().is_empty() {
        result.notes.join("\n")
    } else {
        result.text.clone()
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

    /// The agent loop ends a turn that has no answer with a "(stopped)" placeholder. It is
    /// not an answer: a failed turn stores one error message that keeps the text streamed
    /// so far, not the placeholder followed by an empty error.
    #[test]
    fn the_end_of_turn_placeholder_is_not_an_answer() {
        assert!(is_end_filler(&HistoryMessage::stopped()));
        assert!(!is_end_filler(&HistoryMessage::assistant("An answer.")));
        assert!(!is_end_filler(&HistoryMessage::assistant(
            "(stopped) early"
        )));
        assert!(!is_end_filler(&HistoryMessage::user("(stopped)")));

        let failed = agent::turn::TurnStatus::Failed {
            code: "http".into(),
            message: "boom".into(),
        };
        // A failure always stores exactly one message, the error, whatever was streamed.
        for (answered, partial, filler) in [
            (false, false, true),
            (false, true, true),
            (true, false, false),
        ] {
            assert_eq!(
                ending(&failed, answered, partial, filler),
                Ending::Final(store::MessageStatus::Error),
                "{answered} {partial} {filler}"
            );
        }
        let stopped = agent::turn::TurnStatus::Stopped;
        // Stopped before anything was stored: one stopped message with the partial text.
        assert_eq!(
            ending(&stopped, false, false, true),
            Ending::Final(store::MessageStatus::Stopped)
        );
        // Stopped after the partial answer was stored as a message: that one is marked.
        assert_eq!(ending(&stopped, true, false, false), Ending::MarkStopped);
        // Any other ending without an answer keeps the placeholder, so the next request
        // still has an assistant message after the user's.
        assert_eq!(
            ending(&agent::turn::TurnStatus::StepCapReached, false, false, true),
            Ending::StoreFiller
        );
        assert_eq!(
            ending(&agent::turn::TurnStatus::Completed, true, false, false),
            Ending::Nothing
        );
    }

    /// The advice under an error fits the error and where the model runs.
    #[test]
    fn the_next_step_fits_the_error_and_the_endpoint() {
        // A rejected key is fixed in the endpoint's settings, not by regenerating.
        let key = failure_next_step("unauthorized", false);
        assert!(
            key.contains("Settings → Endpoints") && key.contains("key"),
            "{key}"
        );
        // A remote endpoint's context is a setting; a local model is loaded with one.
        for code in ["tools_do_not_fit", "context_full", "context_overflow"] {
            let local = failure_next_step(code, true);
            let remote = failure_next_step(code, false);
            assert!(
                local.contains("load the model with a larger context"),
                "{code}: {local}"
            );
            assert!(!remote.contains("load the model"), "{code}: {remote}");
            assert!(remote.contains("Settings → Endpoints"), "{code}: {remote}");
        }
        // The app does not run a remote server, so it cannot say "still running".
        for code in ["network", "http"] {
            assert!(failure_next_step(code, true).contains("model is still running"));
            let remote = failure_next_step(code, false);
            assert!(
                remote.contains("server") && remote.contains("Settings → Endpoints"),
                "{remote}"
            );
        }
        assert!(failure_next_step("no_answer", false).contains("Regenerate"));
    }

    /// A call that did not run has no output; the card shows the app's reason instead of
    /// an empty box under "Failed".
    #[test]
    fn a_call_that_did_not_run_shows_its_reason() {
        let call = CallRecord {
            id: "c1".into(),
            name: "git_push".into(),
            arguments: Map::new(),
            recovered: false,
        };
        let refused = ResultRecord::synthetic(
            &call,
            CallStatus::Error,
            vec!["Invalid arguments for git_push: unexpected field \"path\".".into()],
        );
        assert_eq!(
            shown_result_text(&refused),
            "Invalid arguments for git_push: unexpected field \"path\"."
        );
        // Output wins when there is any; notes then are for the model (truncation and so on).
        let mut ran = refused.clone();
        ran.text = "pushed".into();
        assert_eq!(shown_result_text(&ran), "pushed");
        let mut two = refused;
        two.notes.push("Second note.".into());
        assert!(shown_result_text(&two).ends_with("\nSecond note."));
    }

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

    fn action_with(scope: policy::Scope) -> policy::Action {
        policy::Action {
            server: "files".into(),
            tool: "write_file".into(),
            summary: String::new(),
            preview: String::new(),
            scope,
            risk: policy::Risk::Normal,
            read_only: false,
            outbound: false,
            hard_limit: None,
            payload: Value::Null,
        }
    }

    /// D12: revoking grants takes effect from the next tool call, also in a turn that is
    /// already running ("Forget everything I allowed in this chat", a narrowed or changed
    /// external tool).
    #[test]
    fn revoked_grants_reach_a_running_turn() {
        let grants = Arc::new(Grants::default());
        let write = policy::Scope::FileWrite {
            folder: "/work".into(),
        };
        let external = policy::Scope::ExternalAny {
            server: "weather".into(),
            tool: "forecast".into(),
        };
        grants.replace(7, &BTreeSet::from([write.clone(), external.clone()]));
        grants.replace(8, &BTreeSet::from([write.clone()]));
        let gate = AppPolicy::for_chat(grants.clone(), 7, Arc::new(|_: &str| true));
        let asks = |d: policy::Decision| matches!(d, policy::Decision::Ask { .. });
        let decide =
            |scope: &policy::Scope| gate.decide("t", &action_with(scope.clone()), false, false);

        assert_eq!(decide(&write), policy::Decision::AllowedByGrant);
        assert_eq!(decide(&external), policy::Decision::AllowedByGrant);

        // One external tool's grants are revoked while the turn runs.
        grants.revoke_external("weather", "forecast");
        assert!(
            asks(decide(&external)),
            "a revoked tool grant still applied"
        );
        assert_eq!(decide(&write), policy::Decision::AllowedByGrant);

        // "Forget everything I allowed in this chat" while the turn runs.
        grants.replace(7, &BTreeSet::new());
        assert!(asks(decide(&write)), "a cleared grant still applied");
        // Another chat keeps its own grants.
        assert_eq!(grants.of_chat(8), BTreeSet::from([write.clone()]));

        // A grant given later in the turn ("Allow for this chat") applies to its next call.
        grants.add(7, vec![write.clone()]);
        assert_eq!(decide(&write), policy::Decision::AllowedByGrant);
        assert_eq!(grants.of_chat(8), BTreeSet::from([write.clone()]));
        // Nothing to add leaves no empty entry behind.
        grants.add(9, Vec::new());
        assert!(grants.0.lock().unwrap().get(&9).is_none());
    }

    fn subagent_action() -> policy::Action {
        policy::Action {
            server: crate::subagent::SERVER_NAME.into(),
            tool: crate::subagent::TOOL_NAME.into(),
            scope: policy::Scope::Skill,
            ..action_with(policy::Scope::Skill)
        }
    }

    /// The gate tells the rest of the turn what a helper started now must inherit: that
    /// untrusted content is in the turn (it stays so), and whether the `run_subagent`
    /// call being decided was recovered from text.
    #[test]
    fn the_gate_records_what_a_helper_inherits() {
        let shared = Arc::new(TurnShared::default());
        let gate = AppPolicy::for_chat(Arc::new(Grants::default()), 7, Arc::new(|_: &str| true))
            .sharing(shared.clone());
        let read = action_with(policy::Scope::FileRead {
            folder: "/work".into(),
        });
        let none = crate::subagent::Inherited::default();
        assert_eq!(shared.inherited(), none);

        // `run_subagent` itself never asks, recovered or not, untrusted or not.
        assert_eq!(
            gate.decide("run_subagent", &subagent_action(), false, false),
            policy::Decision::AllowedByGrant
        );
        assert_eq!(shared.inherited(), none);
        assert_eq!(
            gate.decide("run_subagent", &subagent_action(), true, false),
            policy::Decision::AllowedByGrant
        );
        assert!(shared.inherited().recovered);
        // The next, native, call to it clears that; another tool's call does not set it.
        gate.decide("read_file", &read, true, false);
        assert!(shared.inherited().recovered);
        gate.decide("run_subagent", &subagent_action(), false, false);
        assert_eq!(shared.inherited(), none);
        gate.decide("read_file", &read, true, false);
        assert_eq!(shared.inherited(), none);

        // Untrusted content stays in the turn once the loop has said so.
        gate.decide("read_file", &read, false, true);
        gate.decide("read_file", &read, false, false);
        assert_eq!(
            shared.inherited(),
            crate::subagent::Inherited {
                untrusted: true,
                recovered: false
            }
        );
    }

    /// `run_subagent` is audited like a skill call: allowed without a prompt, not as a
    /// grant. Its row is written when the helper starts and matched to the call by id.
    #[test]
    fn a_subagent_call_is_audited_as_needing_no_approval() {
        let call = CallRecord {
            id: "c9".into(),
            name: "run_subagent".into(),
            arguments: serde_json::from_value(json!({"task": "Read it."})).unwrap(),
            recovered: false,
        };
        let mut result = ResultRecord::synthetic(&call, CallStatus::Ok, Vec::new());
        result.text = "the secret word is heron".into();
        let (label, entry) = audit_entry(
            7,
            Value::Object(call.arguments.clone()),
            &result,
            &CallDecision::AllowedByGrant,
            Some(&subagent_action()),
            1200,
        );
        assert_eq!(label, "no approval needed");
        assert_eq!(entry.decision, store::AuditDecision::AllowedWithoutPrompt);
        assert_eq!(entry.server, "subagents");
        assert_eq!(entry.tool, "run_subagent");
        assert_eq!(entry.decided_by, "policy");
        assert_eq!(entry.arguments, json!({"task": "Read it."}));
        let outcome = entry.result.expect("a result");
        assert_eq!(outcome.status, "ok");
        assert_eq!(outcome.summary.as_deref(), Some("the secret word is heron"));
        // An ordinary grant is still a grant.
        let (label, entry) = audit_entry(
            7,
            Value::Null,
            &result,
            &CallDecision::AllowedByGrant,
            Some(&action_with(policy::Scope::Screenshots)),
            1,
        );
        assert_eq!(label, "allowed by an earlier grant");
        assert_eq!(entry.decision, store::AuditDecision::AllowedByGrant);

        // The row opened for a call is taken once, and only by that call.
        let shared = TurnShared::default();
        assert_eq!(shared.take_open_audit("c9"), None);
        *shared.open_audit.lock().unwrap() = Some(("c9".into(), store::AuditId(3)));
        assert_eq!(shared.take_open_audit("other"), None);
        assert_eq!(shared.take_open_audit("c9"), Some(store::AuditId(3)));
        assert_eq!(shared.take_open_audit("c9"), None);
    }

    /// ARCHITECTURE.md 9.3: the chosen working folder is used only while it is still
    /// approved; otherwise the first approved folder, as when none was chosen.
    #[test]
    fn a_revoked_working_folder_is_not_used() {
        let approved = vec!["/work/a".to_string(), "/work/b".to_string()];
        assert_eq!(
            working_folder_for(Some("/work/b"), &approved).as_deref(),
            Some("/work/b")
        );
        assert_eq!(
            working_folder_for(None, &approved).as_deref(),
            Some("/work/a")
        );
        assert_eq!(
            working_folder_for(Some("/work/revoked"), &approved).as_deref(),
            Some("/work/a"),
            "a folder that is no longer approved was used"
        );
        assert_eq!(working_folder_for(Some("/work/revoked"), &[]), None);
        assert_eq!(working_folder_for(None, &[]), None);
    }

    #[test]
    fn roles_round_trip() {
        for r in [Role::System, Role::User, Role::Assistant, Role::Tool] {
            assert_eq!(role_of(store_role(r)), r);
        }
    }
}
