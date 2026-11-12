//! Adaptive tool selection (0.0.4).
//!
//! A chat can have many tools switched on. Sending all of them with every request costs
//! context and makes small models choose worse, so each turn shows the model a small set
//! that fits the task and lets it find the rest (`find_tools`) and run them (`call_tool`).
//!
//! - The Tools panel still decides which tools are ON in a chat. This module only chooses,
//!   among those, which ones a turn SHOWS. It never switches a tool on.
//! - [`select`] is the whole decision, as a pure function: the tools that always come
//!   along, the core file tools, what the chat was shown before, what it used or found,
//!   and the groups the user's last messages are about ([`matching_groups`]), up to a cap.
//! - The list is decided when a turn starts and does not change during it. Across the
//!   turns of a chat it only grows, in the fixed order of the chat's tools, because the
//!   model server reuses its cached prompt only for an unchanged beginning
//!   (ARCHITECTURE.md 8.5).
//! - A tool that is on but not shown stays reachable: [`finder`] has `find_tools` and
//!   `call_tool`, and the rules that make `call_tool` exactly as safe as a direct call.

pub mod finder;

use std::collections::BTreeSet;

use agent::HistoryMessage;
use mcp_host::RegisteredTool;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::State;

pub use finder::ToolFinderServer;

use crate::error::AppResult;
use crate::state::AppState;
use crate::tool_catalog;

/// The setting: `{auto, max}`.
pub const SETTING: &str = "toolSelection";
/// How many tools a turn shows unless the setting says otherwise.
pub const DEFAULT_MAX: usize = 14;
/// The smallest cap accepted: the tools that always come along and the core file tools
/// need about this much.
pub const MIN_MAX: usize = 6;
/// The largest cap accepted.
pub const MAX_MAX: usize = 64;
/// How much of one message is read when a request is matched, in characters.
pub const MESSAGE_CHARS: usize = 4000;
/// How many tool names are remembered per chat.
pub const REMEMBERED_NAMES: usize = 200;
/// How many found tools wait for the next turn.
pub const PENDING_NAMES: usize = 32;

/// Built-in servers whose tools come along whenever another tool is on in the chat. They
/// are not switches in the Tools panel.
pub const COMPANION_SERVERS: &[&str] = &[finder::SERVER_NAME, "outputs", "task"];

/// Whether a server is one of the companions: one of the app's own servers of that name.
/// An external server the user gave such a name is an ordinary group.
pub fn is_companion_server(server: &str, builtin: &BTreeSet<String>) -> bool {
    builtin.contains(server) && COMPANION_SERVERS.contains(&server)
}

/// The file tools every turn shows when they are on.
const CORE_FILE_TOOLS: &[&str] = &[
    "read_file",
    "list_dir",
    "search_files",
    "edit_file",
    "write_file",
];

/// The `toolSelection` setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSelection {
    /// Choose the tools for each task. Off: every tool that is on is shown, as before.
    #[serde(default = "yes")]
    pub auto: bool,
    /// The most tools a turn shows.
    #[serde(default = "default_max")]
    pub max: usize,
}

fn yes() -> bool {
    true
}

fn default_max() -> usize {
    DEFAULT_MAX
}

impl Default for ToolSelection {
    fn default() -> Self {
        Self {
            auto: true,
            max: DEFAULT_MAX,
        }
    }
}

impl ToolSelection {
    /// The stored setting, or the default when it is missing or unreadable.
    pub fn from_stored(stored: Option<&Value>) -> Self {
        let mut setting = stored
            .filter(|value| value.is_object())
            .and_then(|value| serde_json::from_value::<Self>(value.clone()).ok())
            .unwrap_or_default();
        setting.max = setting.max.clamp(MIN_MAX, MAX_MAX);
        setting
    }
}

/// The setting as stored.
pub async fn settings(state: &AppState) -> ToolSelection {
    let stored: Option<Value> = state.store.get_setting(SETTING).await.ok().flatten();
    ToolSelection::from_stored(stored.as_ref())
}

/// The setting, for the Tools panel.
#[tauri::command]
pub async fn get_tool_selection(state: State<'_, AppState>) -> AppResult<ToolSelection> {
    Ok(settings(&state).await)
}

/// Switches "choose tools for each task" on or off. It changes which of the tools that
/// are on a turn shows, never which tools are on.
#[tauri::command]
pub async fn set_tool_selection(
    state: State<'_, AppState>,
    auto: bool,
) -> AppResult<ToolSelection> {
    let mut current = settings(&state).await;
    current.auto = auto;
    state.store.set_setting(SETTING, &current).await?;
    Ok(current)
}

/// What is wrong between the tool catalog and the built-in tools registered in this
/// running app (empty when all is well): a built-in tool without an entry, or an example
/// its schema does not accept. The unit tests check the servers they can build; this
/// checks all of them.
#[tauri::command]
pub async fn tool_catalog_problems(state: State<'_, AppState>) -> AppResult<Vec<String>> {
    let host = &state.host;
    Ok(tool_catalog::problems(
        &host.tools(),
        &tool_catalog::builtin_servers(host),
        &|name, args| host.validate(name, args),
    ))
}

// ---- Words ----------------------------------------------------------------------------

/// A text made ready for matching: its words with their base forms, and the text itself
/// in lower case with single spaces, for phrases.
#[derive(Debug, Clone, Default)]
pub struct Wording {
    words: BTreeSet<String>,
    spaced: String,
}

impl Wording {
    /// Prepares a text.
    pub fn of(text: &str) -> Self {
        let lower = text.to_lowercase();
        let mut words = BTreeSet::new();
        let mut spaced = String::with_capacity(lower.len() + 2);
        spaced.push(' ');
        for word in split_words(&lower) {
            spaced.push_str(word);
            spaced.push(' ');
            words.extend(forms(word));
        }
        Self { words, spaced }
    }

    /// Whether the text has this keyword: a word in its plain form (the text may have it
    /// as a plural, or with -ing or -ed), or a phrase exactly as written.
    pub fn has(&self, keyword: &str) -> bool {
        if keyword.contains(' ') {
            self.spaced.contains(&format!(" {keyword} "))
        } else {
            self.words.contains(keyword)
        }
    }

    /// Whether the text has a word taken from a name or a description, which may itself
    /// be a plural or an -ing or -ed form ("forms" in a skill's name, "form" in the text).
    fn has_form_of(&self, word: &str) -> bool {
        forms(word).iter().any(|form| self.words.contains(form))
    }

    /// How many of these keywords the text has.
    fn count(&self, keywords: &[&str]) -> u32 {
        keywords.iter().filter(|k| self.has(k)).count() as u32
    }
}

/// The words of a lower-case text: runs of letters and digits.
fn split_words(lower: &str) -> impl Iterator<Item = &str> {
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
}

/// A word and what it may be a form of: "commits", "committing" and "committed" all give
/// "commit" among their forms. Both sides of a comparison go through this, so a keyword
/// is written once, in its plain form.
fn forms(word: &str) -> Vec<String> {
    let mut out = vec![word.to_string()];
    if !word.is_ascii() {
        return out;
    }
    let n = word.len();
    let mut stems: Vec<&str> = Vec::new();
    if n > 4 && word.ends_with("ies") {
        out.push(format!("{}y", &word[..n - 3]));
    }
    if n > 4 && word.ends_with("es") {
        out.push(word[..n - 2].to_string());
    }
    if n > 3 && word.ends_with('s') && !word.ends_with("ss") {
        out.push(word[..n - 1].to_string());
    }
    if n > 5 && word.ends_with("ing") {
        stems.push(&word[..n - 3]);
    }
    if n > 4 && word.ends_with("ed") {
        stems.push(&word[..n - 2]);
    }
    for stem in stems {
        out.push(stem.to_string());
        // "merging" → "merge", "compiled" → "compile".
        out.push(format!("{stem}e"));
        // "committing" → "commit", "running" → "run".
        let bytes = stem.as_bytes();
        if let [.., a, b] = bytes
            && a == b
            && !b"aeiou".contains(b)
        {
            out.push(stem[..stem.len() - 1].to_string());
        }
    }
    out
}

/// Words that say nothing about what a request is about.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "at", "by", "for", "from", "with",
    "without", "into", "onto", "is", "are", "was", "were", "be", "been", "it", "its", "this",
    "that", "these", "those", "as", "if", "then", "than", "so", "not", "no", "yes", "can", "could",
    "will", "would", "should", "may", "might", "must", "do", "does", "did", "done", "has", "have",
    "had", "i", "me", "my", "we", "our", "you", "your", "he", "she", "they", "them", "their",
    "what", "which", "who", "when", "where", "why", "how", "all", "any", "some", "each", "every",
    "one", "two", "more", "most", "other", "such", "only", "also", "just", "very", "about",
    "after", "before", "over", "under", "between", "there", "here", "out", "up", "down", "off",
    "per", "via", "etc", "asked", "using", "used", "given",
];

/// Everyday words that are part of many tool names: one of them in a request says little
/// about which tool is meant, so it does not make a group match.
const COMMON_WORDS: &[&str] = &[
    "read", "write", "edit", "list", "find", "search", "type", "back", "key", "text", "file",
    "files", "dir", "run", "move", "load", "press", "status", "log", "form", "fill", "click",
    "get", "set", "create", "update", "delete", "remove", "make", "add", "new", "open", "close",
    "show", "source", "output", "tool", "tools", "task", "web", "data", "info", "name", "value",
    "item", "items", "call", "use", "send", "start", "stop", "check", "content", "current",
];

fn is_stopword(word: &str) -> bool {
    STOPWORDS.contains(&word)
}

fn is_common(word: &str) -> bool {
    COMMON_WORDS.contains(&word)
}

/// The words of a name or a sentence, in lower case.
fn words_of(text: &str) -> Vec<String> {
    split_words(&text.to_lowercase())
        .map(str::to_string)
        .collect()
}

/// The words of a description that say something: long enough, and not everyday ones.
fn telling_words(text: &str, min_len: usize) -> BTreeSet<String> {
    words_of(text)
        .into_iter()
        .filter(|w| w.chars().count() >= min_len && !is_stopword(w) && !is_common(w))
        .collect()
}

// ---- The selection ----------------------------------------------------------------------

/// One tool that is on in the chat, as the selection sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The name the model calls it by.
    pub name: String,
    /// Its server: the group.
    pub server: String,
    /// What it says it does.
    pub description: String,
    /// One of the app's own tools.
    pub builtin: bool,
    /// About how many tokens its definition takes in a request ([`definition_cost`]).
    pub cost: u32,
}

impl Candidate {
    /// A registered tool.
    pub fn of(tool: &RegisteredTool, builtin: bool) -> Self {
        Self {
            name: tool.exposed.clone(),
            server: tool.server.clone(),
            description: tool.def.description.clone(),
            builtin,
            cost: definition_cost(&tool.exposed, &tool.def.description, &tool.def.input_schema),
        }
    }
}

/// About how many tokens a tool's definition takes in a request: its name, description and
/// argument schema as JSON, and the wrapping around them, by the loop's own cautious
/// estimate (three characters to a token). The loop measures the real size before a turn
/// starts; this is for choosing a list that will pass that measurement.
pub fn definition_cost(name: &str, description: &str, schema: &Value) -> u32 {
    /// `{"type":"function","function":{"name":"","description":"","parameters":}}`.
    const WRAPPING_CHARS: usize = 72;
    let chars = name.chars().count()
        + description.chars().count()
        + schema.to_string().chars().count()
        + WRAPPING_CHARS;
    u32::try_from(chars.div_ceil(3)).unwrap_or(u32::MAX)
}

/// How many tokens the tool definitions of a turn may take in a context of `n_ctx` tokens,
/// when the rest of the fixed part of every request (the system message with its
/// additions) takes about `other_fixed`.
///
/// The loop refuses to start a turn whose fixed part is over
/// `agent::context::TOOLS_FIT_PERCENT` of what a prompt may use (ARCHITECTURE.md 8.4).
/// Before the selection knew about sizes, the tools that are on by default did not pass
/// that check in a 4,096-token context, and no message could be sent. Sizes here are
/// estimates, so the allowance stays a little under the loop's limit.
///
/// [0.0.6] `other_fixed` is counted for this context ([`FixedPart`]): the Tools panel reads
/// it before the context is known ([`other_fixed`]), a turn once it is.
pub fn allowance(n_ctx: u32, other_fixed: impl FixedPart) -> u32 {
    let budget = agent::context::ContextBudget::new(n_ctx, 0).budget;
    let limit = u64::from(budget) * u64::from(agent::context::TOOLS_FIT_PERCENT) / 100;
    let limit = limit * 97 / 100;
    u32::try_from(limit)
        .unwrap_or(u32::MAX)
        .saturating_sub(other_fixed.tokens_in(n_ctx))
}

/// [0.0.6] The fixed part of every request besides tool definitions and messages, counted
/// for a context of `n_ctx` tokens.
pub trait FixedPart {
    fn tokens_in(&self, n_ctx: u32) -> u32;
}

/// Tokens already counted for the context.
impl FixedPart for u32 {
    fn tokens_in(&self, _n_ctx: u32) -> u32 {
        *self
    }
}

/// [0.0.6] What [`other_fixed`] reads: the fixed text (the system prompt, the list of
/// skills, loaded skills) and the summary of earlier messages as stored. Counted for a
/// context, the summary is counted as a turn in that context sends it
/// ([`summary_as_sent`]): a summary written when the model could remember more is sent at
/// most this context's cap long, so the Tools panel shows the tools the next turn shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OtherFixed {
    text: String,
    summary: Option<String>,
}

impl FixedPart for OtherFixed {
    fn tokens_in(&self, n_ctx: u32) -> u32 {
        let summary = self.summary.as_deref().map(|s| summary_as_sent(s, n_ctx));
        fixed_tokens(&self.text, summary.as_deref())
    }
}

/// The app's own part of the system message: folders, the system, how to call tools.
const APP_LINES_TOKENS: u32 = 220;
/// [0.0.5 memory] The lines around the summary of earlier messages in the system message
/// (`agent::prompt::system_message`: its tags and one sentence), with a little to spare.
const SUMMARY_LINES_TOKENS: u32 = 40;

/// About how many tokens a turn in this chat sends besides tool definitions and messages:
/// the system prompt, the list of skills, loaded skills, the summary of earlier messages of
/// the branch the chat shows, and the app's own lines around them. [0.0.6] Read before the
/// context is known; [`allowance`] counts it for the context, with the summary as a turn in
/// that context sends it ([`OtherFixed`]).
pub async fn other_fixed(state: &AppState, chat: &store::Chat) -> OtherFixed {
    // [0.0.5 memory] The summary is part of every request's system message, so it takes
    // from the tools' share like the rest (a summary written under a larger context
    // could otherwise push the fixed part past it and refuse the turn).
    let summary = match chat.leaf_message_id {
        Some(leaf) => state
            .store
            .summary_for_branch(chat.id, leaf)
            .await
            .ok()
            .flatten()
            .map(|s| s.text),
        None => None,
    };
    OtherFixed {
        text: fixed_text(state, chat).await,
        summary,
    }
}

/// [0.0.5 memory] [`other_fixed`] for a turn that answers the end of `history` (which may
/// be on another branch than the one the chat shows, after an edit): its summary is the one
/// of that branch, as `turns::assemble` reads it.
async fn other_fixed_for_turn(
    state: &AppState,
    chat: &store::Chat,
    history: &[HistoryMessage],
) -> OtherFixed {
    let ids: Vec<store::MessageId> = history
        .iter()
        .filter_map(|m| m.id.map(store::MessageId))
        .collect();
    let summary = state
        .store
        .latest_summary_on_path(chat.id, &ids)
        .await
        .ok()
        .flatten()
        .map(|s| s.text);
    OtherFixed {
        text: fixed_text(state, chat).await,
        summary,
    }
}

/// [0.0.5 memory] The summary of earlier messages as a turn in a context of `n_ctx` tokens
/// sends it. One written when the model could remember more is folded again under this
/// context's cap before the loop measures anything, or cut to that cap when the model
/// cannot fold it (`agent::context::cut_summary`): either way it is at most
/// `ContextBudget::summary_max_chars` long. A summary that fits is sent as it is (trimmed).
pub(crate) fn summary_as_sent(summary: &str, n_ctx: u32) -> String {
    let cap = agent::context::ContextBudget::new(n_ctx, 0).summary_max_chars();
    agent::context::cut_summary(summary, cap)
}

/// [0.0.5 memory] What [`other_fixed`] counts, from the fixed text and the summary (as sent).
pub(crate) fn fixed_tokens(text: &str, summary: Option<&str>) -> u32 {
    let mut tokens = agent::estimate::estimate_tokens(text).saturating_add(APP_LINES_TOKENS);
    if let Some(summary) = summary.map(str::trim).filter(|s| !s.is_empty()) {
        tokens = tokens
            .saturating_add(agent::estimate::estimate_tokens(summary))
            .saturating_add(SUMMARY_LINES_TOKENS);
    }
    tokens
}

/// The fixed text of a chat's requests: the system prompt, the list of skills and the
/// skills loaded in the chat.
async fn fixed_text(state: &AppState, chat: &store::Chat) -> String {
    let prompt = match &chat.system_prompt {
        Some(prompt) => prompt.clone(),
        None => state
            .store
            .get_setting::<String>(crate::settings::DEFAULT_SYSTEM_PROMPT)
            .await
            .ok()
            .flatten()
            .unwrap_or_default(),
    };
    let mut text = prompt;
    let skills = state.skills.enabled();
    if !skills.is_empty() {
        let refs: Vec<&tools::Skill> = skills.iter().collect();
        text.push_str(&tools::skills::index_block(&refs));
    }
    for skill in state.skills.loaded_in(chat.id.0) {
        text.push_str(&skill.body);
    }
    text
}

/// Everything [`select`] decides on.
#[derive(Debug, Clone, Copy)]
pub struct Selecting<'a> {
    /// The tools that are on in the chat, in the order they are offered.
    pub tools: &'a [Candidate],
    /// The user's latest message and the two before it.
    pub messages: &'a [String],
    /// What this chat was shown before.
    pub shown: &'a [String],
    /// What the chat used or found, most wanted first.
    pub wanted: &'a [String],
    /// The enabled skills: name and description.
    pub skills: &'a [(String, String)],
    /// A skill is loaded in the chat.
    pub skill_loaded: bool,
    /// The most tools to show.
    pub max: usize,
    /// The most tokens their definitions may take together ([`allowance`]); `None` when
    /// the model's context is not known.
    pub allowance: Option<u32>,
}

/// A tool that is in every turn's list when it is on: the companions, `run_subagent`
/// (it cannot be reached through `call_tool`) and `load_skill` (the system message tells
/// the model to call it by name).
pub fn is_always_shown(tool: &Candidate) -> bool {
    tool.builtin
        && (COMPANION_SERVERS.contains(&tool.server.as_str())
            || (tool.server == crate::subagent::SERVER_NAME
                && tool.name == crate::subagent::TOOL_NAME)
            || (tool.server == "skills" && tool.name == "load_skill"))
}

/// One of the core file tools.
pub fn is_core(tool: &Candidate) -> bool {
    tool.builtin && tool.server == "files" && CORE_FILE_TOOLS.contains(&tool.name.as_str())
}

/// The groups a request is about, best match first: (server, score). `tools` are the
/// tools still to be decided on; a group none of them belongs to is not looked at.
pub fn matching_groups(
    tools: &[Candidate],
    wording: &Wording,
    skills: &[(String, String)],
) -> Vec<(String, u32)> {
    let mut servers: Vec<&str> = Vec::new();
    for tool in tools {
        if !servers.contains(&tool.server.as_str()) {
            servers.push(&tool.server);
        }
    }
    let mut out: Vec<(String, u32)> = servers
        .into_iter()
        .filter_map(|server| {
            let members: Vec<&Candidate> = tools.iter().filter(|t| t.server == server).collect();
            let score = group_score(server, &members, wording, skills);
            (score >= MATCH_AT).then(|| (server.to_string(), score))
        })
        .collect();
    // Stable: groups with the same score stay in the order of the tools.
    out.sort_by_key(|(_, score)| std::cmp::Reverse(*score));
    out
}

/// What a keyword that means its group by itself is worth, and one that only counts
/// together with another.
const STRONG: u32 = 2;
const WEAK: u32 = 1;
/// A group matches a request from this score on: one strong word, or two weak ones.
const MATCH_AT: u32 = 2;

/// How much a request points at one group.
fn group_score(
    server: &str,
    members: &[&Candidate],
    wording: &Wording,
    skills: &[(String, String)],
) -> u32 {
    let builtin = members.iter().all(|t| t.builtin);
    let own = words_of(server);
    let mut score = match tool_catalog::group(server).filter(|_| builtin) {
        Some(group) => {
            let mut score = STRONG * wording.count(group.strong) + WEAK * wording.count(group.weak);
            if server == "skills" {
                score += skills_score(skills, wording);
            }
            score
        }
        // An external server: its own name, and what its tools say they do.
        None => {
            let named = own.iter().any(|w| {
                w.chars().count() >= 3 && !is_stopword(w) && !is_common(w) && wording.has_form_of(w)
            });
            let described: BTreeSet<String> = members
                .iter()
                .flat_map(|t| telling_words(&t.description, 5))
                .filter(|w| !own.contains(w))
                .collect();
            let said = described
                .iter()
                .filter(|w| wording.has_form_of(w))
                .count()
                .min(2) as u32;
            if named {
                STRONG + WEAK * said
            } else {
                WEAK * said
            }
        }
    };
    // The words of the tools' names, for the tool the request points at most.
    score += members
        .iter()
        .map(|tool| {
            STRONG
                * name_words(tool, &own)
                    .iter()
                    .filter(|w| !is_common(w) && wording.has_form_of(w))
                    .count() as u32
        })
        .max()
        .unwrap_or(0);
    score
}

/// How much a request points at one of the enabled skills: its name as written, and the
/// telling words of its name and description (each word once).
fn skills_score(skills: &[(String, String)], wording: &Wording) -> u32 {
    skills
        .iter()
        .map(|(name, description)| {
            let name_words = words_of(name);
            let phrase = name_words.join(" ");
            let named = !phrase.is_empty() && wording.spaced.contains(&format!(" {phrase} "));
            let mut telling: BTreeSet<String> = name_words
                .into_iter()
                .filter(|w| w.chars().count() >= 3 && !is_stopword(w) && !is_common(w))
                .collect();
            telling.extend(telling_words(description, 5));
            let said = telling
                .iter()
                .filter(|w| wording.has_form_of(w))
                .count()
                .min(3) as u32;
            if named {
                STRONG + WEAK * said
            } else {
                WEAK * said
            }
        })
        .max()
        .unwrap_or(0)
}

/// The words of a tool's name without its server's own (`git_commit` → `commit`).
fn name_words(tool: &Candidate, own: &[String]) -> Vec<String> {
    words_of(&tool.name)
        .into_iter()
        .filter(|w| w.chars().count() >= 3 && !is_stopword(w) && !own.contains(w))
        .collect()
}

/// A built-in tool's catalog entry.
fn entry_of(tool: &Candidate) -> Option<&'static tool_catalog::Entry> {
    tool_catalog::entry(&tool.name).filter(|e| tool.builtin && e.group == tool.server)
}

/// How much a text points at one tool: the words of its name, the words listed for it,
/// and what it says it does. Used to order the tools of a matching group, and by
/// `find_tools`.
pub(crate) fn tool_evidence(tool: &Candidate, wording: &Wording) -> u32 {
    let own = words_of(&tool.server);
    let entry = entry_of(tool);
    let mut score = 0;
    for word in name_words(tool, &own) {
        if wording.has_form_of(&word) {
            score += if is_common(&word) { 1 } else { 3 };
        }
    }
    if let Some(entry) = entry {
        score += 2 * wording.count(entry.keywords);
    }
    let text = entry.map_or(tool.description.as_str(), |e| e.purpose);
    score += telling_words(text, 4)
        .iter()
        .filter(|w| wording.has_form_of(w))
        .count()
        .min(3) as u32;
    score
}

/// How much a tool's group needs it: 1 first. A tool without an entry is in the middle.
pub(crate) fn rank_of(tool: &Candidate) -> u8 {
    entry_of(tool).map_or(2, |e| e.rank)
}

/// The tools chosen so far, by position.
struct Picked {
    chosen: Vec<bool>,
    count: usize,
    max: usize,
}

impl Picked {
    /// Shown whatever the cap says.
    fn keep(&mut self, index: usize) {
        if !self.chosen[index] {
            self.chosen[index] = true;
            self.count += 1;
        }
    }

    /// Shown when there is room.
    fn add(&mut self, index: usize) {
        if self.count < self.max {
            self.keep(index);
        }
    }

    fn full(&self) -> bool {
        self.count >= self.max
    }
}

/// The names of the tools a turn shows, in the order of `tools`.
///
/// 1. A chat whose tools all fit under the cap is shown all of them.
/// 2. Otherwise, always: the companions, `run_subagent`, `load_skill`; and everything
///    this chat was shown before (the list never shrinks).
/// 3. Then, while there is room: the core file tools, what the chat used or found, and
///    the tools of the groups the request is about, best first, the groups taking turns.
///
/// When that list would take more of the model's context than tool definitions may
/// ([`Selecting::allowance`]), the list is made again by size ([`select_by_size`]).
pub fn select(input: &Selecting<'_>) -> Vec<String> {
    let chosen = select_by_count(input);
    let Some(allowance) = input.allowance else {
        return chosen;
    };
    let cost: u64 = input
        .tools
        .iter()
        .filter(|t| chosen.contains(&t.name))
        .map(|t| u64::from(t.cost))
        .sum();
    if cost <= u64::from(allowance) {
        chosen
    } else {
        select_by_size(input, &chosen, allowance)
    }
}

/// The list for a context too small for what [`select_by_count`] chose: tools are added
/// in order of need while their definitions fit in `allowance` tokens.
///
/// 1. The core file tools.
/// 2. What this chat was shown before, in that order, so the list changes as little as
///    the smaller room allows.
/// 3. `find_tools` and `call_tool`, both or neither: one is no use without the other.
/// 4. The rest of what the list by count had, in the order of `tools`.
///
/// A tool that does not fit is left out and the smaller ones after it are still tried.
/// Left-out tools stay on in the chat; without the finder pair the model cannot reach
/// them in this turn, which is what a turn in so small a context was like before 0.0.4.
fn select_by_size(input: &Selecting<'_>, by_count: &[String], allowance: u32) -> Vec<String> {
    let tools = input.tools;
    let mut chosen = vec![false; tools.len()];
    let mut spent: u32 = 0;
    let mut add = |indexes: &[usize], chosen: &mut Vec<bool>| {
        let new: Vec<usize> = indexes.iter().copied().filter(|i| !chosen[*i]).collect();
        let cost = new
            .iter()
            .fold(0u32, |sum, i| sum.saturating_add(tools[*i].cost));
        if spent.saturating_add(cost) <= allowance {
            spent = spent.saturating_add(cost);
            for i in new {
                chosen[i] = true;
            }
        }
    };
    for (index, tool) in tools.iter().enumerate() {
        if is_core(tool) {
            add(&[index], &mut chosen);
        }
    }
    for name in input.shown {
        if let Some(index) = tools.iter().position(|t| &t.name == name)
            && !is_finder(&tools[index])
        {
            add(&[index], &mut chosen);
        }
    }
    let finder: Vec<usize> = (0..tools.len()).filter(|i| is_finder(&tools[*i])).collect();
    if finder.iter().all(|i| by_count.contains(&tools[*i].name)) {
        add(&finder, &mut chosen);
    }
    for (index, tool) in tools.iter().enumerate() {
        if by_count.contains(&tool.name) && !is_finder(tool) {
            add(&[index], &mut chosen);
        }
    }
    tools
        .iter()
        .zip(&chosen)
        .filter(|(_, chosen)| **chosen)
        .map(|(tool, _)| tool.name.clone())
        .collect()
}

/// One of the two tools that reach the tools a turn does not show.
fn is_finder(tool: &Candidate) -> bool {
    tool.builtin && tool.server == finder::SERVER_NAME
}

/// The list by count: see [`select`].
fn select_by_count(input: &Selecting<'_>) -> Vec<String> {
    let tools = input.tools;
    if tools.len() <= input.max {
        return tools.iter().map(|t| t.name.clone()).collect();
    }
    let position = |name: &str| tools.iter().position(|t| t.name == name);
    let mut picked = Picked {
        chosen: vec![false; tools.len()],
        count: 0,
        max: input.max,
    };
    for (index, tool) in tools.iter().enumerate() {
        if is_always_shown(tool) {
            picked.keep(index);
        }
    }
    for name in input.shown {
        if let Some(index) = position(name) {
            picked.keep(index);
        }
    }
    for (index, tool) in tools.iter().enumerate() {
        if is_core(tool) {
            picked.add(index);
        }
    }
    for name in input.wanted {
        if let Some(index) = position(name) {
            picked.add(index);
        }
    }
    if input.skill_loaded {
        // A loaded skill refers to its files and its scripts.
        for (server, name) in [("skills", "read_skill_file"), ("terminal", "run_command")] {
            if let Some(index) = tools
                .iter()
                .position(|t| t.builtin && t.server == server && t.name == name)
            {
                picked.add(index);
            }
        }
    }
    if !picked.full() {
        let rest_index: Vec<usize> = (0..tools.len()).filter(|i| !picked.chosen[*i]).collect();
        let rest: Vec<Candidate> = rest_index.iter().map(|i| tools[*i].clone()).collect();
        let wording = Wording::of(&input.messages.join("\n"));
        let mut queues: Vec<std::collections::VecDeque<usize>> =
            matching_groups(&rest, &wording, input.skills)
                .into_iter()
                .map(|(server, _)| {
                    let mut members: Vec<usize> = rest_index
                        .iter()
                        .copied()
                        .filter(|i| tools[*i].server == server)
                        .collect();
                    members.sort_by_key(|i| {
                        (
                            std::cmp::Reverse(tool_evidence(&tools[*i], &wording)),
                            rank_of(&tools[*i]),
                            *i,
                        )
                    });
                    members.into()
                })
                .collect();
        // The groups take turns, so a large one does not starve a small one.
        while !picked.full() && queues.iter().any(|q| !q.is_empty()) {
            for queue in &mut queues {
                if let Some(index) = queue.pop_front() {
                    picked.add(index);
                }
            }
        }
    }
    tools
        .iter()
        .zip(&picked.chosen)
        .filter(|(_, chosen)| **chosen)
        .map(|(tool, _)| tool.name.clone())
        .collect()
}

/// What to remember for the chat after a turn showed `shown`: everything remembered so
/// far plus what is new, at most [`REMEMBERED_NAMES`] names. Names of tools that are off
/// now are kept, so they are shown again when they are switched back on.
pub fn remembered(stored: &[String], shown: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(stored.len() + shown.len());
    for name in stored.iter().chain(shown) {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    // Bounded: the oldest names that are not shown now go first.
    let mut extra = out.len().saturating_sub(REMEMBERED_NAMES);
    out.retain(|name| {
        if extra > 0 && !shown.contains(name) {
            extra -= 1;
            false
        } else {
            true
        }
    });
    out.truncate(REMEMBERED_NAMES);
    out
}

/// The text of the user's latest message and the two before it, newest first. What the
/// user typed only: attachments and tool results are content, not the request.
pub fn recent_user_messages(history: &[HistoryMessage]) -> Vec<String> {
    history
        .iter()
        .rev()
        .filter(|message| message.role == agent::Role::User)
        .map(|message| {
            message
                .parts
                .iter()
                .filter_map(|part| match part {
                    agent::Part::Text(text) => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n")
        })
        .filter(|text| !text.trim().is_empty())
        .take(3)
        .map(|text| text.chars().take(MESSAGE_CHARS).collect())
        .collect()
}

/// The tools the chat has called, newest first, each once. A call made through
/// `call_tool` counts as the tool it ran.
pub fn used_tools(history: &[HistoryMessage]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for message in history.iter().rev() {
        let calls: Vec<_> = message.tool_calls().collect();
        for call in calls.into_iter().rev() {
            let name = if call.name == finder::CALL_TOOL {
                match call.arguments.get("name").and_then(Value::as_str) {
                    Some(name) => name.trim(),
                    None => continue,
                }
            } else {
                call.name.as_str()
            };
            if name.is_empty() || name == finder::CALL_TOOL || name == finder::FIND_TOOLS {
                continue;
            }
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
    }
    out
}

/// Splits the chat's tools into the shown ones and the rest, each in their order.
pub fn split(
    enabled: Vec<RegisteredTool>,
    shown: &[String],
) -> (Vec<RegisteredTool>, Vec<RegisteredTool>) {
    enabled
        .into_iter()
        .partition(|tool| shown.contains(&tool.exposed))
}

/// The enabled skills as the selection reads them: name and description.
pub fn skill_words(state: &AppState) -> Vec<(String, String)> {
    state
        .skills
        .enabled()
        .into_iter()
        .map(|skill| (skill.name, skill.description))
        .collect()
}

/// The tools the chat's next turn shows whatever the user writes: what [`select`] gives
/// before a message is known. The Tools panel marks these "shown" and the others "on
/// request".
pub fn shown_before_a_message(
    offered: &[Candidate],
    prefs: &crate::turns::ChatTools,
    skills: &[(String, String)],
    skill_loaded: bool,
    setting: ToolSelection,
    allowance: Option<u32>,
) -> BTreeSet<String> {
    if !setting.auto {
        return offered.iter().map(|t| t.name.clone()).collect();
    }
    select(&Selecting {
        tools: offered,
        messages: &[],
        shown: &prefs.shown,
        wanted: &prefs.found,
        skills,
        skill_loaded,
        max: setting.max,
        allowance,
    })
    .into_iter()
    .collect()
}

/// Adds tools the model found or ran through `call_tool` to the chat's waiting list: the
/// next turn shows them when there is room. `first` puts them before what already waits
/// (a tool that was run counts for more than one that was only found). This never
/// switches a tool on: the next turn still only chooses among the tools that are on.
pub async fn note_wanted(state: &AppState, chat_id: i64, names: &[String], first: bool) {
    if names.is_empty() {
        return;
    }
    let changed =
        crate::mcp_cmd::change_selection_memory(state, store::ChatId(chat_id), |_shown, found| {
            *found = waiting_with(found, names, first)
        })
        .await;
    if let Err(e) = changed {
        tracing::warn!("could not note the tools a chat found: {}", e.message);
    }
}

/// The waiting list with `names` added, each name once, at most [`PENDING_NAMES`].
pub fn waiting_with(waiting: &[String], names: &[String], first: bool) -> Vec<String> {
    let (before, after) = if first {
        (names, waiting)
    } else {
        (waiting, names)
    };
    let mut out: Vec<String> = Vec::new();
    for name in before.iter().chain(after) {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out.truncate(PENDING_NAMES);
    out
}

/// Splits the tools that are on in the chat into those shown to the model this turn and
/// those kept back (reachable through `find_tools` and `call_tool`). Decided once, when
/// the turn starts; what was shown is remembered for the chat, so the list only grows.
pub async fn for_turn(
    state: &AppState,
    chat: &store::Chat,
    history: &[HistoryMessage],
    enabled: Vec<RegisteredTool>,
    n_ctx: u32,
) -> (Vec<RegisteredTool>, Vec<RegisteredTool>) {
    choose(state, chat, history, enabled, n_ctx, true).await
}

/// [`for_turn`] without remembering anything: the same choice from the same inputs, for
/// the Context tab's view of the next request (0.0.4).
pub async fn preview_for_turn(
    state: &AppState,
    chat: &store::Chat,
    history: &[HistoryMessage],
    enabled: Vec<RegisteredTool>,
    n_ctx: u32,
) -> (Vec<RegisteredTool>, Vec<RegisteredTool>) {
    choose(state, chat, history, enabled, n_ctx, false).await
}

async fn choose(
    state: &AppState,
    chat: &store::Chat,
    history: &[HistoryMessage],
    enabled: Vec<RegisteredTool>,
    n_ctx: u32,
    remember: bool,
) -> (Vec<RegisteredTool>, Vec<RegisteredTool>) {
    let setting = settings(state).await;
    if !setting.auto || enabled.is_empty() {
        return (enabled, Vec::new());
    }
    // What the definitions may take of this model's context.
    let room = allowance(n_ctx, other_fixed_for_turn(state, chat, history).await);
    let builtin = tool_catalog::builtin_servers(&state.host);
    let candidates: Vec<Candidate> = enabled
        .iter()
        .map(|tool| Candidate::of(tool, builtin.contains(&tool.server)))
        .collect();
    let prefs: crate::turns::ChatTools =
        serde_json::from_value(chat.tool_prefs.clone()).unwrap_or_default();
    // What the chat used comes before what it only found.
    let mut wanted = used_tools(history);
    for name in &prefs.found {
        if !wanted.contains(name) {
            wanted.push(name.clone());
        }
    }
    let shown = select(&Selecting {
        tools: &candidates,
        messages: &recent_user_messages(history),
        shown: &prefs.shown,
        wanted: &wanted,
        skills: &skill_words(state),
        skill_loaded: !state.skills.loaded_in(chat.id.0).is_empty(),
        max: setting.max,
        allowance: Some(room),
    });
    if !remember {
        return split(enabled, &shown);
    }
    let stored = crate::mcp_cmd::change_selection_memory(state, chat.id, |was_shown, found| {
        *was_shown = remembered(was_shown, &shown);
        // What waited had its turn.
        found.clear();
    })
    .await;
    if let Err(e) = stored {
        // The turn goes on with what was chosen; the next one chooses again.
        tracing::warn!(
            "could not remember the tools a chat was shown: {}",
            e.message
        );
    }
    split(enabled, &shown)
}

#[cfg(test)]
mod tests;
