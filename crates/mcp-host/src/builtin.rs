//! The interface built-in tool servers implement (ARCHITECTURE.md 9.1, 9.3, 13.3).
//!
//! Built-in servers (files, terminal, git, browser, computer, skills) are connected
//! in memory with typed values and no serialisation. From the registry upward they look
//! exactly like external servers: same tool list, toggles, policy, approval cards,
//! audit log and cancellation.
//!
//! Every call happens in two steps:
//!
//! 1. [`BuiltinServer::describe`] turns the model's arguments into a
//!    [`policy::Action`]: the exact thing the call touches (canonical path, resolved
//!    program, punycode host), a preview for the approval card, the grant scope, the
//!    risk and any hard limit. It has no side effects.
//! 2. [`BuiltinServer::call`] runs the *approved* action. It re-checks the action
//!    immediately before the side effect (13.3: "what runs is what was shown") and
//!    refuses if anything changed since the preview.

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

/// One tool as offered to the model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDef {
    /// Short fixed name for built-ins (`read_file`); `<server>__<tool>` for external tools.
    pub name: String,
    /// What the tool does, as offered to the model.
    pub description: String,
    /// JSON Schema of the arguments (an object schema).
    pub input_schema: Value,
    /// The server marks the tool destructive (raises it to always-ask).
    #[serde(default)]
    pub destructive: Option<bool>,
    /// The server marks the tool read-only. Never used to skip a prompt (13.3).
    #[serde(default)]
    pub read_only: Option<bool>,
}

/// One block of a tool result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum Content {
    /// Plain text.
    Text {
        /// The text.
        text: String,
    },
    /// An image, base64-encoded.
    Image {
        /// MIME type, e.g. `image/png`.
        mime: String,
        /// The image bytes in base64.
        data_base64: String,
    },
}

/// What a tool call returns. Failures are results the model can read
/// (`is_error: true`), not protocol errors (9.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutput {
    /// What the model sees (already sized to the per-result cap by the tool).
    pub content: Vec<Content>,
    /// The call failed; `content` explains why.
    pub is_error: bool,
    /// The full, untruncated output for the tool card and storage, when the model
    /// saw a shortened version.
    #[serde(default)]
    pub full_text: Option<String>,
    /// Structured details for the tool card (a diff, a listing…).
    #[serde(default)]
    pub ui: Option<Value>,
}

impl ToolOutput {
    /// A successful result with one text block.
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            ..Default::default()
        }
    }
    /// An error result with one text block.
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            content: vec![Content::Text { text: text.into() }],
            is_error: true,
            ..Default::default()
        }
    }
    /// The text blocks joined, for summaries and tests.
    pub fn joined_text(&self) -> String {
        self.content
            .iter()
            .filter_map(|c| match c {
                Content::Text { text } => Some(text.as_str()),
                Content::Image { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// What a describer may need about the chat. Read-only: nothing here can change
/// permissions (13.5).
#[derive(Debug, Clone, Default)]
pub struct ChatContext {
    /// The chat's database id.
    pub chat_id: i64,
    /// Approved folders for this chat (global plus the chat's own), as stored.
    pub approved_folders: Vec<String>,
    /// Relative paths resolve against this; the first approved folder by default.
    pub working_folder: Option<String>,
    /// Never reachable whatever was approved: the app's data, install and browser-profile
    /// folders (13.4).
    pub forbidden_folders: Vec<String>,
    /// Folders of enabled skills (a terminal working directory may be one of them).
    pub skill_folders: Vec<String>,
    /// The per-result cap in tokens (8.4); tools size what the model sees to it.
    pub result_cap_tokens: usize,
}

/// Progress lines streamed into the tool card while a call runs.
pub type ProgressFn = Arc<dyn Fn(String) + Send + Sync>;

/// Per-call context for [`BuiltinServer::call`].
#[derive(Clone)]
pub struct CallContext {
    /// The chat the call belongs to.
    pub chat: ChatContext,
    /// Child of the turn's token; Stop cancels it (4.2).
    pub cancel: CancellationToken,
    /// Where progress lines for the tool card go, if anywhere.
    pub progress: Option<ProgressFn>,
}

/// Result of [`BuiltinServer::describe`].
// One describe per tool call; the size difference does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum Described {
    /// Ready for the policy engine.
    Action(policy::Action),
    /// The arguments cannot be turned into an action (missing file for an edit, bad
    /// offset…). Returned to the model as an error result; not a policy decision.
    Invalid(String),
}

/// A built-in tool server.
#[async_trait]
pub trait BuiltinServer: Send + Sync {
    /// Server name (`files`, `terminal`, …).
    fn name(&self) -> &str;
    /// The tools this server offers, in a fixed order.
    fn tools(&self) -> Vec<ToolDef>;
    /// Arguments → action (no side effects). Arguments were already validated against
    /// the tool's schema.
    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described;
    /// Runs the approved action. Must re-check it before any side effect.
    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput;
}
