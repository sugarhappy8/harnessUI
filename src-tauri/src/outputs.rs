//! Stored tool output (0.0.4, "manageable tool output").
//!
//! When a tool shortens what the model sees, the full text is kept here so the model can
//! ask for more of it (`read_output`) and the user can open it from the tool card.
//!
//! Owned by the outputs feature. This file starts as the agreed entry point only.

use mcp_host::ToolOutput;
use tauri::AppHandle;

/// Called after every tool call of a turn, before the result goes back to the model.
pub async fn after_call(
    _app: &AppHandle,
    _chat_id: i64,
    _call_id: &str,
    _tool: &str,
    _out: &mut ToolOutput,
) {
}
