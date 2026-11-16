//! [0.0.7] The tools that are on in a chat, for the system message's tool list and call guide
//! (`agent::prompt::ToolGuide`, ARCHITECTURE.md 8.5 and 8.4 "Which tools a turn shows").
//!
//! A turn shows the model only some of the chat's tools; the list names all of them, so the
//! model knows what it can reach through `call_tool`. It is made from the tools that are on,
//! never from the ones a turn shows, so it stays the same from turn to turn until the user
//! changes which tools are on, and the model server can keep its cached prompt.
//!
//! The list and the guide take part of the system message, so the room left for tool
//! definitions is smaller by what they take ([`tokens`]): the selection and the Tools panel
//! count them, as the loop's own check does.

use std::collections::BTreeSet;

use agent::prompt::{GuideOs, ListedTool, Sanitizer, ToolGroup, ToolGuide};
use mcp_host::RegisteredTool;

use super::finder;
use crate::tool_catalog;

/// The list for `tools`, the tools that are on in a chat (shown in the turn or not), in the
/// chat's order with `run_subagent` last, as the turn's own tool list has it. `find_tools` and
/// `call_tool` are not listed: the list's first lines say what they are for. A built-in tool
/// is described by the tool catalog's purpose, the same sentence `find_tools` prints; a tool
/// of an added server by the first sentence of its own description, which the agent cleans
/// and cuts as untrusted text. `n_ctx` decides how much of it fits
/// (`agent::prompt::tool_guide_budget`).
pub fn tool_guide(tools: &[RegisteredTool], builtin: &BTreeSet<String>, n_ctx: u32) -> ToolGuide {
    let mut ordered: Vec<&RegisteredTool> = tools.iter().collect();
    // Stable: only `run_subagent` moves, as in `turns::assemble`.
    ordered.sort_by_key(|tool| crate::subagent::is_tool(tool));
    let mut groups: Vec<(String, ToolGroup)> = Vec::new();
    let mut call_tool = false;
    for tool in ordered {
        let own = builtin.contains(&tool.server);
        if own && finder::is_own(tool) {
            call_tool |= tool.original == finder::CALL_TOOL;
            continue;
        }
        let purpose = match tool_catalog::entry_for(tool, own) {
            Some(entry) => entry.purpose.to_owned(),
            None => finder::first_sentence(&tool.def.description),
        };
        let listed = ListedTool {
            name: tool.exposed.clone(),
            purpose,
        };
        match groups.iter_mut().find(|(server, _)| *server == tool.server) {
            Some((_, group)) => group.tools.push(listed),
            None => groups.push((
                tool.server.clone(),
                ToolGroup {
                    label: tool_catalog::label(&tool.server),
                    own,
                    tools: vec![listed],
                },
            )),
        }
    }
    ToolGuide {
        groups: groups.into_iter().map(|(_, group)| group).collect(),
        finder: call_tool,
        os: GuideOs::this_os(),
        budget: agent::prompt::tool_guide_budget(n_ctx),
    }
}

/// About how many tokens the list and the guide take in the system message, by the loop's
/// own estimate (the blocks and the blank lines before them).
pub fn tokens(guide: &ToolGuide) -> u32 {
    let blocks = agent::prompt::tool_guide_blocks(guide, &Sanitizer::generic());
    if blocks.is_empty() {
        return 0;
    }
    agent::estimate_tokens(&format!("\n\n{}", blocks.join("\n\n")))
}

/// The list as a helper started by `run_subagent` gets it: without the tools a helper is
/// not offered (`run_subagent` itself and the coding apps, `subagent::offered_to_helper`).
pub fn for_helper(guide: &ToolGuide) -> ToolGuide {
    let not_offered: Vec<String> = guide
        .groups
        .iter()
        .flat_map(|group| &group.tools)
        .map(|tool| tool.name.clone())
        .filter(|name| {
            name == crate::subagent::TOOL_NAME || crate::outside_agent::is_tool_name(name)
        })
        .collect();
    guide.without(&not_offered)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use mcp_host::{RegisteredTool, ToolDef};
    use serde_json::json;

    use super::*;

    fn registered(server: &str, name: &str, description: &str) -> RegisteredTool {
        RegisteredTool {
            exposed: name.into(),
            server: server.into(),
            original: name.rsplit("__").next().unwrap_or(name).into(),
            def: ToolDef {
                name: name.into(),
                description: description.into(),
                input_schema: json!({"type": "object"}),
                destructive: None,
                read_only: None,
            },
            fingerprint: String::new(),
            schema_error: None,
        }
    }

    fn builtin() -> BTreeSet<String> {
        ["files", "terminal", "subagents", "toolfinder", "computer"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    }

    fn on() -> Vec<RegisteredTool> {
        vec![
            registered("files", "read_file", "Read a text file. Long files are returned in parts."),
            registered("terminal", "run_command", "Run one shell command."),
            registered("subagents", "run_subagent", "Hand a task to a helper."),
            registered("toolfinder", "find_tools", "Search the tools."),
            registered("toolfinder", "call_tool", "Run a tool."),
            registered("files", "write_file", "Create a file."),
            registered(
                "weather",
                "weather__forecast",
                "Get the forecast for a city. Uses the national service.\nSecond line.",
            ),
        ]
    }

    #[test]
    fn every_tool_that_is_on_is_listed_by_group_with_run_subagent_last() {
        let guide = tool_guide(&on(), &builtin(), 32_768);
        assert!(guide.finder);
        let groups: Vec<(&str, bool, Vec<&str>)> = guide
            .groups
            .iter()
            .map(|g| {
                (
                    g.label.as_str(),
                    g.own,
                    g.tools.iter().map(|t| t.name.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            groups,
            [
                ("Files", true, vec!["read_file", "write_file"]),
                ("Terminal", true, vec!["run_command"]),
                ("weather", false, vec!["weather__forecast"]),
                ("Subagents", true, vec!["run_subagent"]),
            ]
        );
        // The catalog's purpose for a built-in tool; the first sentence of an added
        // server's own description.
        let purpose = |name: &str| {
            guide
                .groups
                .iter()
                .flat_map(|g| &g.tools)
                .find(|t| t.name == name)
                .unwrap()
                .purpose
                .clone()
        };
        assert_eq!(
            purpose("read_file"),
            tool_catalog::entry("read_file").unwrap().purpose
        );
        assert_eq!(purpose("weather__forecast"), "Get the forecast for a city.");
        assert_eq!(guide.budget, agent::prompt::tool_guide_budget(32_768));
        assert_eq!(guide.os, GuideOs::this_os());
    }

    #[test]
    fn without_call_tool_there_is_no_finder_and_nothing_on_lists_nothing() {
        let mut tools = on();
        tools.retain(|t| t.server != "toolfinder");
        assert!(!tool_guide(&tools, &builtin(), 32_768).finder);
        let empty = tool_guide(&[], &builtin(), 32_768);
        assert!(empty.groups.is_empty());
        assert_eq!(tokens(&empty), 0);
        // An added server named like a built-in one is not the app's own.
        let lookalike = vec![registered("terminal", "terminal__run_command", "Runs anything.")];
        let guide = tool_guide(&lookalike, &BTreeSet::new(), 32_768);
        assert!(!guide.groups[0].own);
    }

    #[test]
    fn the_list_is_the_same_whatever_a_turn_shows_and_its_size_is_counted() {
        let a = tool_guide(&on(), &builtin(), 32_768);
        let mut reordered = on();
        // The order of the chat's tools is the order of the list: the same tools in the same
        // order give the same list, whichever of them a turn shows.
        let b = tool_guide(&reordered, &builtin(), 32_768);
        assert_eq!(a, b);
        reordered.swap(0, 5);
        assert_ne!(tool_guide(&reordered, &builtin(), 32_768), a);
        let size = tokens(&a);
        let text = agent::prompt::tool_guide_blocks(&a, &Sanitizer::generic()).join("\n\n");
        assert_eq!(size, agent::estimate_tokens(&format!("\n\n{text}")));
        assert!(size > 50 && size <= a.budget, "{size}");
    }

    #[test]
    fn a_helper_gets_the_list_without_the_tools_it_is_not_offered() {
        let mut tools = on();
        tools.push(registered("coding_apps", "ask_codex", "Ask Codex."));
        let guide = tool_guide(&tools, &builtin(), 32_768);
        let helper = for_helper(&guide);
        assert!(guide.lists("run_subagent") && guide.lists("ask_codex"));
        assert!(!helper.lists("run_subagent") && !helper.lists("ask_codex"));
        assert!(helper.lists("read_file") && helper.lists("weather__forecast"));
    }
}
