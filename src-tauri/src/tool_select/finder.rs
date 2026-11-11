//! `find_tools` and `call_tool` (0.0.4, adaptive tool selection): how the model reaches a
//! tool that is on in the chat but not in its tool list this turn.
//!
//! What must hold:
//!
//! - `call_tool` is exactly as safe as calling the tool it names. [`Routing`] turns the
//!   call into that tool and its arguments before anything looks at it: the arguments are
//!   checked against the tool's own schema, the tool's own describer makes the action,
//!   the policy gate decides on that action, the approval card and the audit row name the
//!   real tool, and the tool's own code runs.
//! - Only a tool that is on in the chat can be reached: the two lists a turn starts with
//!   (shown and hidden) are the only places a name is looked up. Nothing here switches a
//!   tool on.
//! - `call_tool` cannot run itself, `find_tools` or `run_subagent`.
//! - `find_tools` reads the chat's tool list and nothing else, so it never asks. It may
//!   say that a group is switched off; it cannot switch it on.

use std::collections::{BTreeSet, HashMap};

use async_trait::async_trait;
use mcp_host::{
    BuiltinServer, CallContext, ChatContext, Described, RegisteredTool, ToolDef, ToolOutput,
    ToolPrefs,
};
use serde_json::{Map, Value, json};

use crate::tool_catalog;

/// The built-in server that lists the two tools.
pub const SERVER_NAME: &str = "toolfinder";
/// Searches the tools that are on in the chat.
pub const FIND_TOOLS: &str = "find_tools";
/// Runs a tool that is on in the chat but not in the model's list this turn.
pub const CALL_TOOL: &str = "call_tool";

/// Tools `call_tool` never runs: itself and `find_tools` (no recursion), and
/// `run_subagent` (a helper is started by the turn itself, with its own gate and audit
/// row; it is always in the list when it is on).
pub const NOT_THROUGH_CALL_TOOL: &[&str] = &[CALL_TOOL, FIND_TOOLS, crate::subagent::TOOL_NAME];

/// The most matches one search returns.
pub const MAX_MATCHES: usize = 8;

const FIND_DESCRIPTION: &str = "Search the tools that are switched on in this chat, including the ones that are not in your tool list right now. Each match comes with what it does, its arguments and example arguments. Use it when none of your tools fits what you need to do.";

const CALL_DESCRIPTION: &str = "Run a tool that is on in this chat but is not in your tool list this turn. Get its name and arguments from find_tools first. The user approves it exactly as if you had called that tool yourself.";

/// Lists `find_tools` and `call_tool` so they appear in tool lists and token estimates like
/// every built-in tool, and describes a `find_tools` call for the policy engine. It runs
/// neither: both need the tool lists of the turn, so the turn's tool host runs them
/// (`turns::AppTools`, through [`Routing`]).
pub struct ToolFinderServer;

#[async_trait]
impl BuiltinServer for ToolFinderServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef {
                name: FIND_TOOLS.into(),
                description: FIND_DESCRIPTION.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "query": {
                            "type": "string",
                            "minLength": 1,
                            "description": "What you want to do, or part of a tool's name, for example \"commit\" or \"take a screenshot\""
                        }
                    },
                    "required": ["query"],
                    "additionalProperties": false
                }),
                destructive: Some(false),
                read_only: Some(true),
            },
            ToolDef {
                name: CALL_TOOL.into(),
                description: CALL_DESCRIPTION.into(),
                input_schema: json!({
                    "type": "object",
                    "properties": {
                        "name": {
                            "type": "string",
                            "minLength": 1,
                            "description": "The tool's name, as find_tools printed it"
                        },
                        "arguments": {
                            "type": "object",
                            "description": "The arguments for that tool, as a JSON object"
                        }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }),
                destructive: None,
                read_only: None,
            },
        ]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _chat: &ChatContext,
    ) -> Described {
        if tool != FIND_TOOLS {
            // `call_tool` is described as the tool it names, by the turn. A call that
            // reaches this server was not unwrapped, so it is not run.
            return Described::Invalid(
                "call_tool can only be used from a chat turn; nothing was run.".into(),
            );
        }
        let query = args
            .get("query")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        if query.is_empty() {
            return Described::Invalid(
                "query is empty: say what you want to do, or give part of a tool's name".into(),
            );
        }
        let summary = format!(
            "Search this chat's tools for \"{}\"",
            policy::sanitize::one_line(query, 80)
        );
        Described::Action(policy::Action {
            server: SERVER_NAME.into(),
            tool: FIND_TOOLS.into(),
            summary: summary.clone(),
            preview: summary,
            // Reads the chat's own tool list and nothing else: never prompts.
            scope: policy::Scope::AppData,
            risk: policy::Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Value::Null,
        })
    }

    async fn call(
        &self,
        tool: &str,
        _args: &Map<String, Value>,
        _action: &policy::Action,
        _ctx: CallContext,
    ) -> ToolOutput {
        ToolOutput::error(format!(
            "{tool} can only be used from a chat turn; nothing was run."
        ))
    }
}

/// Whether a registered tool is one of the two tools of this server.
pub fn is_own(tool: &RegisteredTool) -> bool {
    tool.server == SERVER_NAME && (tool.original == FIND_TOOLS || tool.original == CALL_TOOL)
}

// Used by the routing and the search added below.
#[allow(dead_code)]
fn unused() -> (BTreeSet<String>, HashMap<String, ToolPrefs>) {
    let _ = tool_catalog::ENTRIES;
    (BTreeSet::new(), HashMap::new())
}
