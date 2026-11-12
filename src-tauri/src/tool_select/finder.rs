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

// ---- call_tool: one call standing for another ---------------------------------------------

/// The chat's tools as one turn sees them: what the model has in its list, and what is on
/// in the chat but kept back. Both are fixed when the turn starts.
#[derive(Clone, Copy)]
pub struct Routing<'a> {
    pub host: &'a mcp_host::Host,
    pub shown: &'a [RegisteredTool],
    pub hidden: &'a [RegisteredTool],
}

/// A `call_tool` call turned into the call it stands for.
#[derive(Debug, Clone, PartialEq)]
pub struct Unwrapped {
    /// The tool to run, by the name the model would call it by.
    pub name: String,
    /// Its server.
    pub server: String,
    /// Its arguments, repaired and checked against its own schema.
    pub args: Map<String, Value>,
    /// The tool is not in the model's list this turn.
    pub hidden: bool,
}

/// What the turn does with a call it is about to run.
#[derive(Debug, Clone, PartialEq)]
pub enum Route {
    /// An ordinary call: run it as it is.
    Direct,
    /// `find_tools`: the turn answers it from its own tool lists.
    Find,
    /// `call_tool`: run this tool with these arguments instead.
    Call(Unwrapped),
    /// `call_tool` for something it may not run; nothing runs.
    Refused(String),
}

impl<'a> Routing<'a> {
    /// The tool of this name, if it is on in the chat this turn, and whether it is in the
    /// model's list. The two lists are the only place a name is looked up: a tool the
    /// host has registered but the chat has switched off is not found here.
    fn on(&self, name: &str) -> Option<(&'a RegisteredTool, bool)> {
        let named = |list: &'a [RegisteredTool]| list.iter().find(|t| t.exposed == name);
        named(self.shown)
            .map(|tool| (tool, true))
            .or_else(|| named(self.hidden).map(|tool| (tool, false)))
    }

    /// Turns the arguments of a `call_tool` call into the tool it names and that tool's
    /// arguments, or says why it cannot run. Nothing is run and nothing is changed here;
    /// the same call always gives the same answer within a turn, so validating,
    /// describing and running a call all see the same tool and the same arguments.
    pub fn unwrap_call(&self, args: &Map<String, Value>) -> Result<Unwrapped, String> {
        let name = args
            .get("name")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if name.is_empty() {
            return Err(
                "call_tool needs \"name\": the name of the tool to run, as find_tools printed it."
                    .into(),
            );
        }
        // The name is the model's text: shown on one line and cut short.
        let said = policy::sanitize::one_line(name, 80);
        if NOT_THROUGH_CALL_TOOL.contains(&name) {
            return Err(format!(
                "call_tool cannot run {said}. If {said} is in your tool list, call it directly."
            ));
        }
        let Some((tool, in_list)) = self.on(name) else {
            return Err(self.not_on(name, &said));
        };
        if let Some(reason) = &tool.schema_error {
            return Err(format!("The tool {said} is disabled: {reason}"));
        }
        let arguments = match args.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return Err(format!(
                    "\"arguments\" must be a JSON object with the arguments for {said}."
                ));
            }
        };
        // What the agent loop repairs for a direct call (ARCHITECTURE.md 8.3, step 4) is
        // repaired here, against the tool's own schema; then the tool's full schema decides.
        let Value::Object(arguments) =
            agent::repair_arguments(Value::Object(arguments), &tool.def.input_schema)
        else {
            return Err(format!(
                "\"arguments\" must be a JSON object with the arguments for {said}."
            ));
        };
        if let Err(problems) = self
            .host
            .validate(&tool.exposed, &Value::Object(arguments.clone()))
        {
            return Err(format!(
                "The arguments do not fit {said}: {}",
                problems.join("; ")
            ));
        }
        Ok(Unwrapped {
            name: tool.exposed.clone(),
            server: tool.server.clone(),
            args: arguments,
            hidden: !in_list,
        })
    }

    /// What to tell the model about a name that is not on in the chat: switched off (the
    /// host knows the tool) or unknown, with close names of tools that are on. Tools
    /// that are off are never listed.
    fn not_on(&self, name: &str, said: &str) -> String {
        let registered = self.host.tools().iter().any(|t| t.exposed == name);
        let on: Vec<&str> = self
            .shown
            .iter()
            .chain(self.hidden)
            .map(|t| t.exposed.as_str())
            .filter(|n| !NOT_THROUGH_CALL_TOOL.contains(n))
            .collect();
        let close = close_names(name, &on);
        let mut message = if registered {
            format!(
                "{said} is switched off in this chat, so it was not run. The user can turn it on in the Tools panel."
            )
        } else {
            format!("There is no tool named \"{said}\" in this chat.")
        };
        if !close.is_empty() {
            message.push_str(&format!(
                " Tools that are on with a similar name: {}.",
                close.join(", ")
            ));
        }
        if !registered {
            message.push_str(" Use find_tools to search.");
        }
        message
    }

    /// Validation as the agent loop asks for it: an ordinary call against its tool's
    /// schema; a `call_tool` call against its own shape and then, as the tool it names,
    /// against that tool's schema. A name that is off, unknown or not allowed fails here,
    /// as a call to a tool that is not offered does.
    pub fn validate(&self, name: &str, args: &Value) -> Result<(), Vec<String>> {
        if name != CALL_TOOL {
            return self.host.validate(name, args);
        }
        self.host.validate(CALL_TOOL, args)?;
        let Some(call) = args.as_object() else {
            return Err(vec!["the arguments must be a JSON object".into()]);
        };
        // The agent loop puts the problems into a sentence of its own and ends it.
        self.unwrap_call(call)
            .map(|_| ())
            .map_err(|why| vec![why.trim_end_matches('.').to_string()])
    }

    /// The policy action of a call. A `call_tool` call is described as the tool it names,
    /// by that tool's own describer and with the user's switches for that tool, so the
    /// gate, the approval card and the audit row see the real action.
    pub async fn describe(
        &self,
        name: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
        prefs: &HashMap<String, ToolPrefs>,
    ) -> Described {
        let switches = |tool: &str| prefs.get(tool).copied().unwrap_or_default();
        if name != CALL_TOOL {
            return self.host.describe(name, args, chat, switches(name)).await;
        }
        let target = match self.unwrap_call(args) {
            Ok(target) => target,
            Err(why) => return Described::Invalid(why),
        };
        // A direct call to a tool whose server is not running is refused by the policy
        // engine before anything else (13.3, step 1). The same holds here.
        if !self.host.tool_ready(&target.name) {
            return Described::Invalid(format!(
                "The tool is not available: {} is not available right now",
                target.server
            ));
        }
        match self
            .host
            .describe(&target.name, &target.args, chat, switches(&target.name))
            .await
        {
            // The card and the audit row take the tool's name from the action: an action
            // that names anything else is not one to decide on.
            Described::Action(action) if !describes(&action, &target) => {
                Described::Invalid(format!(
                    "{} cannot be run through call_tool: its description of the call names another tool. Nothing was run.",
                    target.name
                ))
            }
            described => described,
        }
    }

    /// What to do with an approved call. For `call_tool` the tool and the arguments are
    /// worked out again from the same call, and the action must be the one made for them.
    pub fn route(&self, name: &str, args: &Map<String, Value>, action: &policy::Action) -> Route {
        match name {
            FIND_TOOLS => Route::Find,
            CALL_TOOL => match self.unwrap_call(args) {
                Ok(target) if describes(action, &target) => Route::Call(target),
                Ok(_) => Route::Refused(
                    "this call does not match the action that was approved, so it was not run"
                        .into(),
                ),
                Err(why) => Route::Refused(why),
            },
            _ => Route::Direct,
        }
    }
}

/// Whether an action is the one a tool's describer makes for that tool.
fn describes(action: &policy::Action, target: &Unwrapped) -> bool {
    action.tool == target.name && action.server == target.server
}

/// The name of the tool a call really runs: for `call_tool`, the tool its action
/// describes. For the approval card.
pub fn real_tool(name: &str, action: Option<&policy::Action>) -> String {
    match action {
        Some(action) if name == CALL_TOOL => action.tool.clone(),
        _ => name.to_string(),
    }
}

/// The tool and the arguments an audit row records: a call made through `call_tool` is
/// logged as the tool it ran, with that tool's arguments. A `call_tool` call that never
/// became an action (an unknown or switched-off name) is logged as what it was.
pub fn audited(name: &str, arguments: Value, action: Option<&policy::Action>) -> (String, Value) {
    match action {
        Some(action) if name == CALL_TOOL => {
            let inner = match arguments.get("arguments") {
                Some(Value::Object(map)) => map.clone(),
                _ => Map::new(),
            };
            (action.tool.clone(), Value::Object(inner))
        }
        _ => (name.to_string(), arguments),
    }
}

/// Up to five names from `on` that look like `name`, closest first.
pub fn close_names(name: &str, on: &[&str]) -> Vec<String> {
    let wanted: String = name.trim().to_lowercase().chars().take(80).collect();
    if wanted.is_empty() {
        return Vec::new();
    }
    let wanted_words = super::words_of(&wanted);
    let squeezed =
        |text: &str| -> String { text.chars().filter(|c| c.is_alphanumeric()).collect() };
    let mut scored: Vec<(u32, &str)> = on
        .iter()
        .filter_map(|candidate| {
            let lower = candidate.to_lowercase();
            let mut score = 0;
            if wanted.chars().count() >= 3 && (lower.contains(&wanted) || wanted.contains(&lower)) {
                score += 4;
            }
            score += 2 * super::words_of(&lower)
                .iter()
                .filter(|w| w.chars().count() >= 3 && wanted_words.contains(w))
                .count() as u32;
            let distance = edit_distance(&wanted, &lower);
            if distance <= 2 {
                score += 4;
            } else if distance <= (lower.chars().count() / 3).max(1) {
                score += 2;
            }
            // Written without its separators: "readfile" for "read_file".
            if squeezed(&wanted) == squeezed(&lower) {
                score += 6;
            }
            (score > 0).then_some((score, *candidate))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    scored
        .into_iter()
        .take(5)
        .map(|(_, name)| name.to_string())
        .collect()
}

/// How many single-character changes turn one short name into another.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut previous = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            let next = (previous + cost).min(row[j] + 1).min(row[j + 1] + 1);
            previous = row[j + 1];
            row[j + 1] = next;
        }
    }
    row[b.len()]
}

// ---- find_tools ---------------------------------------------------------------------------

/// One tool as `find_tools` lists it.
#[derive(Debug, Clone, Copy)]
pub struct Listed<'a> {
    pub tool: &'a RegisteredTool,
    /// In the model's list this turn.
    pub in_list: bool,
    /// One of the app's own tools.
    pub builtin: bool,
}

/// A turn's tools as `find_tools` lists them.
fn listed<'t>(
    tools: &'t [RegisteredTool],
    in_list: bool,
    builtin: &BTreeSet<String>,
) -> Vec<Listed<'t>> {
    tools
        .iter()
        .map(|tool| Listed {
            tool,
            in_list,
            builtin: builtin.contains(&tool.server),
        })
        .collect()
}

/// The tools a search finds, best first, at most [`MAX_MATCHES`]. A tool is found by the
/// words of its name, the words listed for it, what it says it does, and the words of its
/// group; anything that scores less than half of the best match is left out, so a search
/// for one tool's name returns that tool. The two finder tools are never listed.
pub fn search<'a>(query: &str, tools: &[Listed<'a>]) -> Vec<Listed<'a>> {
    let wording = super::Wording::of(query);
    let mut scored: Vec<(u32, usize)> = tools
        .iter()
        .enumerate()
        .filter(|(_, listed)| !is_own(listed.tool) && listed.tool.schema_error.is_none())
        .filter_map(|(index, listed)| {
            let score = find_score(listed, &wording);
            (score > 0).then_some((score, index))
        })
        .collect();
    let Some(best) = scored.iter().map(|(score, _)| *score).max() else {
        return Vec::new();
    };
    scored.retain(|(score, _)| score * 2 >= best);
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    scored
        .into_iter()
        .take(MAX_MATCHES)
        .map(|(_, index)| tools[index])
        .collect()
}

/// How much a search points at one tool. 0: not at all.
fn find_score(listed: &Listed<'_>, wording: &super::Wording) -> u32 {
    let tool = listed.tool;
    let candidate = super::Candidate::of(tool, listed.builtin);
    let own = super::words_of(&tool.server);
    let entry = tool_catalog::entry_for(tool, listed.builtin);
    let mut score = 0;
    // The tool's own name, as written ("git_commit" or "git commit").
    let name = super::words_of(&tool.exposed).join(" ");
    if !name.is_empty() && wording.spaced.contains(&format!(" {name} ")) {
        score += 100;
    }
    for word in super::name_words(&candidate, &own) {
        if wording.has_form_of(&word) {
            score += 10;
        }
    }
    if let Some(entry) = entry {
        score += 6 * wording.count(entry.keywords);
    }
    let text = entry.map_or(tool.def.description.as_str(), |e| e.purpose);
    score += 2 * super::telling_words(text, 4)
        .iter()
        .filter(|w| wording.has_form_of(w))
        .count()
        .min(3) as u32;
    // Its group: the words that mean the group, and the group's name.
    let group = tool_catalog::group(&tool.server).filter(|_| listed.builtin);
    if let Some(group) = group {
        score += (3 * wording.count(group.strong)).min(6) + wording.count(group.weak).min(2);
    }
    let label = group.map(|g| super::words_of(g.label)).unwrap_or_default();
    if own
        .iter()
        .chain(&label)
        .any(|w| w.chars().count() >= 3 && wording.has_form_of(w))
    {
        score += 4;
    }
    if score == 0 {
        return 0;
    }
    // Among equals, the tool its group needs most comes first.
    score + u32::from(3u8.saturating_sub(super::rank_of(&candidate)))
}

/// What `find_tools` answers: each match with what it does, its arguments, example
/// arguments and how to run it, then the groups the search is about that are switched off.
///
/// The answer never holds a whole call (a name together with its arguments as one JSON
/// object): the agent loop refuses a call written as text when it repeats call-shaped text
/// from a tool result, so a call copied from here must not look like an echo.
pub fn render(
    query: &str,
    found: &[Listed<'_>],
    all: &[Listed<'_>],
    off_groups: &[String],
    skills: &[String],
    validate: &tool_catalog::Validate<'_>,
) -> String {
    let asked = policy::sanitize::one_line(query.trim(), 80);
    let wording = super::Wording::of(query);
    let (about, other): (Vec<&String>, Vec<&String>) = off_groups
        .iter()
        .partition(|server| group_is_about(server, &wording));
    let labels = |servers: &[&String]| -> Vec<String> {
        servers.iter().map(|s| tool_catalog::label(s)).collect()
    };
    let off_sentence = format!(
        "{} tools are off in this chat; the user can turn them on in the Tools panel.",
        join_with_and(&labels(&about))
    );
    if found.is_empty() {
        let mut out = format!("No tool that is on in this chat matches \"{asked}\".");
        if !about.is_empty() {
            out.push('\n');
            out.push_str(&off_sentence);
        }
        let groups = groups_on(all);
        if !groups.is_empty() {
            let list: Vec<String> = groups
                .iter()
                .map(|(label, count)| {
                    format!(
                        "{label} ({count} tool{})",
                        if *count == 1 { "" } else { "s" }
                    )
                })
                .collect();
            out.push_str(&format!(
                "\nOn in this chat: {}. Search again with a word for what you want to do.",
                list.join(", ")
            ));
        }
        if !other.is_empty() {
            out.push_str(&format!(
                "\nAlso off in this chat: {}.",
                labels(&other).join(", ")
            ));
        }
        return out;
    }
    let mut out = format!("Tools that match \"{asked}\" ({}):", found.len());
    for (place, listed) in found.iter().enumerate() {
        out.push_str("\n\n");
        // Two examples for the best two matches, one for the others.
        let examples = if place < 2 { 2 } else { 1 };
        out.push_str(&describe_match(listed, examples, skills, validate));
    }
    if !about.is_empty() {
        out.push_str("\n\n");
        out.push_str(&off_sentence);
    }
    out
}

/// Whether a search is about a group: its words (for a built-in group) or its name.
fn group_is_about(server: &str, wording: &super::Wording) -> bool {
    let own = super::words_of(server);
    let group = tool_catalog::group(server);
    let label = group.map(|g| super::words_of(g.label)).unwrap_or_default();
    let named = own
        .iter()
        .chain(&label)
        .any(|w| w.chars().count() >= 3 && wording.has_form_of(w));
    let meant = group.is_some_and(|g| 2 * wording.count(g.strong) + wording.count(g.weak) >= 2);
    named || meant
}

/// The groups that have a tool on, in the order of the tools, with how many. The
/// companions are not groups.
fn groups_on(all: &[Listed<'_>]) -> Vec<(String, usize)> {
    let mut groups: Vec<(&str, usize)> = Vec::new();
    for listed in all {
        let server = listed.tool.server.as_str();
        if listed.builtin && super::COMPANION_SERVERS.contains(&server) {
            continue;
        }
        match groups.iter_mut().find(|(s, _)| *s == server) {
            Some(group) => group.1 += 1,
            None => groups.push((server, 1)),
        }
    }
    groups
        .into_iter()
        .map(|(server, count)| (tool_catalog::label(server), count))
        .collect()
}

/// "A", "A and B", "A, B and C".
fn join_with_and(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

/// One match, as a few lines.
fn describe_match(
    listed: &Listed<'_>,
    max_examples: usize,
    skills: &[String],
    validate: &tool_catalog::Validate<'_>,
) -> String {
    let tool = listed.tool;
    let name = &tool.exposed;
    let entry = tool_catalog::entry_for(tool, listed.builtin);
    let purpose = match entry {
        Some(entry) => entry.purpose.to_string(),
        None => first_sentence(&tool.def.description),
    };
    let mut lines = vec![
        format!("{name} [{}]: {purpose}", tool_catalog::label(&tool.server)),
        format!(
            "  Arguments: {}",
            tool_catalog::arguments_line(&tool.def.input_schema)
        ),
    ];
    for example in examples_for(tool, entry, skills, validate)
        .into_iter()
        .take(max_examples)
    {
        lines.push(format!("  Example arguments: {example}"));
    }
    lines.push(if listed.in_list {
        format!("  In your tool list: call {name} directly.")
    } else {
        format!(
            "  Not in your tool list this turn. Run it with call_tool: name \"{name}\", and its arguments in \"arguments\"."
        )
    });
    lines.join("\n")
}

/// Example arguments for a tool, as JSON text: the catalog's, as far as the tool's schema
/// accepts them as it is registered now; otherwise one made from the schema. An example
/// that does not validate is never shown.
fn examples_for(
    tool: &RegisteredTool,
    entry: Option<&tool_catalog::Entry>,
    skills: &[String],
    validate: &tool_catalog::Validate<'_>,
) -> Vec<String> {
    let fits = |value: &Value| validate(&tool.exposed, value).is_ok();
    // A skill tool's example names a skill that is enabled.
    let with_real_skill = |text: &str| match (tool.server.as_str(), skills.first()) {
        ("skills", Some(skill)) => text.replace(
            &format!("\"{}\"", tool_catalog::EXAMPLE_SKILL),
            &Value::String(skill.clone()).to_string(),
        ),
        _ => text.to_string(),
    };
    let mut out = match entry {
        Some(entry) => tool_catalog::fitting_examples(entry, &with_real_skill, &fits),
        None => Vec::new(),
    };
    if out.is_empty()
        && let Some(made) = tool_catalog::example_from_schema(&tool.def.input_schema)
    {
        let value = Value::Object(made);
        if fits(&value) {
            out.push(value.to_string());
        }
    }
    out
}

/// The first sentence of a description, on one line. External descriptions are a server's
/// text: they were made safe when the tool was registered, and are cut short here.
fn first_sentence(text: &str) -> String {
    let trimmed = text.trim();
    let end = trimmed
        .char_indices()
        .find(|(index, c)| {
            let after = &trimmed[index + c.len_utf8()..];
            *c == '\n'
                || (matches!(c, '.' | '!' | '?')
                    && after.chars().next().is_none_or(char::is_whitespace))
        })
        .map(|(index, c)| {
            if c == '\n' {
                index
            } else {
                index + c.len_utf8()
            }
        })
        .unwrap_or(trimmed.len());
    let sentence = policy::sanitize::one_line(trimmed[..end].trim(), 200);
    if sentence.is_empty() {
        "No description.".into()
    } else {
        sentence
    }
}

/// Runs one `find_tools` call for a turn: searches the tools that are on in the chat,
/// names the groups that are off, and notes what was found so the next turn can show it.
/// It reads the chat's tool list and nothing else, and it switches nothing on.
pub async fn find(
    app: &tauri::AppHandle,
    chat_id: i64,
    routing: Routing<'_>,
    args: &Map<String, Value>,
) -> ToolOutput {
    use tauri::Manager;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if query.is_empty() {
        return ToolOutput::error(
            "query is empty: say what you want to do, or give part of a tool's name",
        );
    }
    let state = app.state::<crate::state::AppState>();
    let builtin = tool_catalog::builtin_servers(routing.host);
    let mut all = listed(routing.shown, true, &builtin);
    all.extend(listed(routing.hidden, false, &builtin));
    let found = search(query, &all);
    let off = crate::mcp_cmd::off_groups(&state, store::ChatId(chat_id)).await;
    let skills: Vec<String> = state.skills.enabled().into_iter().map(|s| s.name).collect();
    let text = render(query, &found, &all, &off, &skills, &|name, value| {
        routing.host.validate(name, value)
    });
    // What was found and is not in the list waits for the next turn.
    let waiting: Vec<String> = found
        .iter()
        .filter(|listed| !listed.in_list)
        .map(|listed| listed.tool.exposed.clone())
        .collect();
    super::note_wanted(&state, chat_id, &waiting, false).await;
    ToolOutput::text(text)
}

#[cfg(test)]
mod tests;
