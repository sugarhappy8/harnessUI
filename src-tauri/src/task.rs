//! The chat's task state (0.0.4, "visible, resumable task state").
//!
//! Goal, constraints, steps, changed files, completed checks and the next action of the
//! work a chat is doing, kept so it can be shown, paused, corrected and resumed.
//!
//! Owned by the task feature. This file starts as the agreed entry point only.

use mcp_host::ToolOutput;
use serde_json::{Map, Value};
use tauri::AppHandle;

/// Called after every tool call of a turn: the place to note changed files and checks.
pub async fn after_call(
    _app: &AppHandle,
    _chat_id: i64,
    _tool: &str,
    _args: &Map<String, Value>,
    _action: &policy::Action,
    _out: &ToolOutput,
) {
}
