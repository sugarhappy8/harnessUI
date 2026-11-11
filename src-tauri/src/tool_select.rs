//! Adaptive tool selection (0.0.4).
//!
//! A chat can have many tools switched on. Sending all of them with every request costs
//! context and makes small models choose worse, so each turn shows the model a small set
//! that fits the task and lets it find the rest (`find_tools`) and run them (`call_tool`).
//!
//! Owned by the tool selection feature. This file starts as the agreed entry point only.

pub mod finder;

use agent::HistoryMessage;
use mcp_host::RegisteredTool;

pub use finder::ToolFinderServer;

use crate::state::AppState;

/// Splits the tools that are on in the chat into those shown to the model this turn and
/// those kept back. Until the feature lands, everything is shown.
pub async fn for_turn(
    _state: &AppState,
    _chat: &store::Chat,
    _history: &[HistoryMessage],
    enabled: Vec<RegisteredTool>,
) -> (Vec<RegisteredTool>, Vec<RegisteredTool>) {
    (enabled, Vec::new())
}
