//! Turning a reverse tool call into a policy action (item5-spec.md WP9c `describe.rs`,
//! section 6): paths through the sandbox (the whole computer in Full access, the app's
//! folders never), folders through [`crate::walk`], session calls against the chat's
//! binding, result ids against the chat's ledger, and what this computer offers first.
//! Opening or checking a file: scope `FileAnalysis { folder }`; session calls:
//! `FileAnalysisSession`. No side effects.
//!
//! TODO(0.0.6): WP9c builds [`describe`]. This is WP9a's contract stub: the types and
//! signature are frozen; the stub refuses every call with a plain message.

use mcp_host::{ChatContext, Described};
use serde_json::{Map, Value};

use crate::session::SessionView;
use crate::walk::WalkFs;
use crate::{AppleTools, Decompiler};

/// What this computer offers right now, for the availability messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    /// rea is installed (whatever its state).
    pub rea_installed: bool,
    /// The decompiler rea uses, if one is installed and chosen.
    pub decompiler: Option<Decompiler>,
    /// Apple's command line tools (checked fresh on a Mac).
    pub apple_tools: AppleTools,
    /// rea or the chosen decompiler needs an update (R15).
    pub needs_update: bool,
    /// Allow-listed tools whose schema changed in the installed rea (switched off).
    pub changed_tools: Vec<String>,
}

/// What `describe` reads.
pub struct DescribeEnv<'a> {
    /// The file system (fake in tests).
    pub fs: &'a dyn WalkFs,
    /// The session snapshot.
    pub session: &'a SessionView,
    /// What this computer offers.
    pub availability: &'a Availability,
}

/// The action for one reverse tool call, or why it cannot be one.
///
/// TODO(0.0.6): WP9c. The stub refuses names outside the allow-list with "there is no tool
/// named …" and every other call as not ready yet.
pub fn describe(
    tool: &str,
    args: &Map<String, Value>,
    chat: &ChatContext,
    env: &DescribeEnv<'_>,
) -> Described {
    let _ = (args, chat, env);
    if !crate::allow::MODEL_TOOLS.contains(&tool) {
        return Described::Invalid(format!("there is no tool named {tool}"));
    }
    Described::Invalid("Reverse engineering is not ready in this build yet.".into())
}
