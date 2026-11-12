//! Stored tool output (0.0.4, "manageable tool output").
//!
//! When a tool shortens what the model sees, it returns the whole text in
//! `ToolOutput::full_text` and a note that says which lines it showed
//! (`tools::shorten::Shown`). [`after_call`] keeps the whole text in the chat's
//! `tool_outputs` and appends one line that names it ([`shortened_line`]), so that:
//!
//! - the model can read more of it with `read_output` (the `outputs` server below), and
//! - the user can open it from the tool card ("Full output", [`get_tool_output`]).
//!
//! Security: a stored output is another tool's result, so what `read_output` returns is
//! outside content. It is returned from `call` as an ordinary result, which the agent loop
//! wraps in the untrusted-data markers like any tool result; nothing of it is ever in an
//! app note (`describe` only checks the arguments' shape). Every read names the chat of
//! the turn (or of the card), and the store answers an id of another chat as "not found".
//! The tool touches only the app's own data for the chat: `Scope::AppData`, read-only, no
//! network, so it never prompts and is audited like every call.

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described, ToolDef, ToolOutput};
use serde::Serialize;
use serde_json::{Map, Value, json};
use store::{ChatId, NewToolOutput, Store, output_lines};
use tauri::{AppHandle, Manager, State};
use tools::shorten::Shown;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The server's name (fixed in the spec; tool selection offers it as a companion).
pub const SERVER_NAME: &str = "outputs";
/// The one tool of the server.
pub const TOOL_NAME: &str = "read_output";

/// Lines of context around a `grep` match when the call gives none.
const DEFAULT_CONTEXT: usize = 2;
/// The most lines of context a call may ask for.
const MAX_CONTEXT: usize = 20;
/// Saved output ids named when an id is unknown.
const MAX_IDS_LISTED: usize = 10;
/// Lines a "read next" range covers at most.
const MAX_NEXT_LINES: usize = 200;
/// Bytes of the result budget kept free for the closing line and the loop's own markers.
const FOOTER_ROOM: usize = 300;
/// Characters of the `grep` text shown in a summary or a message.
const GREP_SHOWN: usize = 120;

// ---- After a call: store and say so ---------------------------------------------------

/// The line appended to a shortened result. Plain digits, so the tool card can find the
/// id on the last line.
pub fn shortened_line(id: i64, shown: usize, total: u64, next: (usize, usize)) -> String {
    let (a, b) = next;
    format!(
        "[Shortened: {shown} of {total} lines shown. Full output is saved as output #{id}. Read more with read_output {{\"id\": {id}, \"lines\": \"{a}-{b}\"}} or {{\"id\": {id}, \"grep\": \"text\"}}.]"
    )
}

/// The output id named by a result's last line, when it is the [`shortened_line`] (the
/// rule the tool card follows in `ToolCallCard.tsx`, kept here to test the line against).
#[cfg(test)]
pub fn shortened_id(text: &str) -> Option<i64> {
    let last = text.trim_end().lines().last()?;
    let rest = last.strip_prefix("[Shortened: ")?;
    if !last.ends_with(".]") {
        return None;
    }
    let (_, after) = rest.split_once(" Full output is saved as output #")?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// How many lines at the start of `view` are the first lines of `full`: what a tool that
/// left no note showed, counted as stored lines.
fn leading_lines_shown(view: &str, full: &str) -> usize {
    output_lines(view)
        .iter()
        .zip(output_lines(full).iter())
        .take_while(|(a, b)| a.text == b.text)
        .count()
}

/// Called after every tool call of a turn, before the result goes back to the model.
/// See [`record`].
pub async fn after_call(
    app: &AppHandle,
    chat_id: i64,
    call_id: &str,
    tool: &str,
    out: &mut ToolOutput,
) {
    match app.try_state::<AppState>() {
        Some(state) => record(&state.store, chat_id, call_id, tool, out).await,
        // Not started yet: nothing can be stored, but the note never leaves the app.
        None => {
            Shown::take(&mut out.ui);
        }
    }
}

/// The work of [`after_call`]: when the tool shortened its result (`full_text` is set),
/// the whole text is stored for the chat, the "Shortened" line is appended to what the
/// model sees, and `full_text` is cleared, so the card shows exactly what the model saw
/// (and its "Full output" button). The tool's note is always taken out of `ui`.
pub async fn record(store: &Store, chat_id: i64, call_id: &str, tool: &str, out: &mut ToolOutput) {
    let note = Shown::take(&mut out.ui);
    // Which output a call saved is the app's to say, never a tool's or a server's.
    take_key(&mut out.ui, SAVED_KEY);
    // A part of a stored output is not stored again.
    if tool == TOOL_NAME {
        return;
    }
    let Some(full) = out.full_text.take() else {
        return;
    };
    let view = out.joined_text();
    let shown = note.map_or_else(|| leading_lines_shown(&view, &full), |n| n.lines);
    let next_from_note = note.and_then(|n| n.next);
    let added = store
        .add_tool_output(NewToolOutput {
            chat_id: ChatId(chat_id),
            call_id: call_id.to_string(),
            tool: tool.to_string(),
            content: full.clone(),
        })
        .await;
    let info = match added {
        Ok(info) => info,
        Err(e) => {
            tracing::warn!(
                chat_id,
                tool,
                "could not store a shortened tool output: {e}"
            );
            out.full_text = Some(full);
            return;
        }
    };
    let total = usize::try_from(info.lines).unwrap_or(usize::MAX);
    let shown = shown.min(total);
    let next = next_from_note
        .filter(|&(a, b)| a >= 1 && a <= b && b <= total)
        .or_else(|| Shown::next_range(shown + 1, shown, total))
        .unwrap_or((1, total.clamp(1, MAX_NEXT_LINES)));
    let line = shortened_line(info.id, shown, info.lines, next);
    let mut ui = match out.ui.take() {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    ui.insert(SAVED_KEY.into(), json!(info.id));
    out.ui = Some(Value::Object(ui));
    match out.content.iter_mut().rev().find_map(|c| match c {
        Content::Text { text } => Some(text),
        _ => None,
    }) {
        Some(text) => {
            if !text.is_empty() && !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&line);
        }
        None => out.content.push(Content::Text { text: line }),
    }
}

/// The key in `ToolOutput::ui` under which the app names the output a call saved. Set only
/// by [`record`], which drops any value a tool put there.
pub const SAVED_KEY: &str = "savedOutput";

/// Removes `key` from a result's `ui` object; a `ui` left empty becomes `None`.
fn take_key(ui: &mut Option<Value>, key: &str) {
    if let Some(Value::Object(map)) = ui.as_mut() {
        map.remove(key);
        if map.is_empty() {
            *ui = None;
        }
    }
}

/// The output the app saved for one call of the chat, if it saved one. Call ids are
/// unique within a chat (the agent loop makes them so); a helper's calls have none.
pub async fn saved_for_call(store: &Store, chat: ChatId, call_id: &str) -> Option<i64> {
    if call_id.is_empty() {
        return None;
    }
    let list = store.list_tool_outputs(chat).await.ok()?;
    list.iter().find(|o| o.call_id == call_id).map(|o| o.id)
}

// ---- The `outputs` server ---------------------------------------------------------------

/// The definition the model sees. Short on purpose: small models get a tight allowance.
pub fn tool_def() -> ToolDef {
    ToolDef {
        name: TOOL_NAME.into(),
        description: "Read a shortened tool result saved as output #id: a line range, or the lines containing grep text.".into(),
        input_schema: json!({"type":"object","properties":{
            "id": {"type":"integer"},
            "lines": {"type":"string","description":"A-B"},
            "grep": {"type":"string"},
            "context": {"type":"integer","minimum":0}
        },"required":["id"],"additionalProperties":false}),
        destructive: Some(false),
        read_only: Some(true),
    }
}

/// What a `read_output` call asks for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// Lines `a` to `b`, 1-based and inclusive (`b` may be past the end).
    Lines(usize, usize),
    /// The lines containing the text, with this many lines around each.
    Grep(String, usize),
}

/// Reads and checks the arguments. The errors name the argument and what to send.
fn parse(args: &Map<String, Value>) -> Result<(i64, Request), String> {
    let id = match args.get("id") {
        Some(v) => v.as_i64().ok_or(
            "id must be a whole number: the number after \"output #\" in a [Shortened: …] line.",
        )?,
        None => {
            return Err(
                "id is required: the number after \"output #\" in a [Shortened: …] line.".into(),
            );
        }
    };
    let lines = args.get("lines").and_then(Value::as_str);
    let grep = args.get("grep").and_then(Value::as_str);
    match (lines, grep) {
        (Some(_), Some(_)) => Err("Pass lines or grep, not both: lines reads a range, grep finds the lines that contain some text.".into()),
        (None, Some(text)) => {
            if text.trim().is_empty() {
                return Err("grep is empty: pass the text to look for.".into());
            }
            let context = args
                .get("context")
                .and_then(Value::as_u64)
                .map_or(DEFAULT_CONTEXT, |c| usize::try_from(c).unwrap_or(MAX_CONTEXT))
                .min(MAX_CONTEXT);
            Ok((id, Request::Grep(text.to_string(), context)))
        }
        (Some(range), None) => parse_range(range).map(|(a, b)| (id, Request::Lines(a, b))),
        (None, None) => Ok((id, Request::Lines(1, usize::MAX))),
    }
}

/// "A-B" (or "A" alone), 1-based, A ≤ B.
fn parse_range(range: &str) -> Result<(usize, usize), String> {
    let bad = || {
        format!(
            "lines \"{}\" is not a range: write it as \"A-B\" with line numbers from 1, for example \"1-80\".",
            policy::sanitize::one_line(range, 40)
        )
    };
    let number = |s: &str| s.trim().parse::<usize>().ok();
    let (a, b) = match range.split_once('-') {
        Some((a, b)) => (number(a).ok_or_else(bad)?, number(b).ok_or_else(bad)?),
        None => {
            let a = number(range).ok_or_else(bad)?;
            (a, a)
        }
    };
    if a == 0 {
        return Err(format!(
            "lines start at 1, not 0: for example \"1-{}\".",
            b.max(1)
        ));
    }
    if a > b {
        return Err(format!(
            "lines \"{a}-{b}\" is backwards: the first line comes first, as in \"{b}-{a}\"."
        ));
    }
    Ok((a, b))
}

/// What `read_output` reports to the policy: the app's own data for this chat, read only.
fn describe(tool: &str, args: &Map<String, Value>) -> Described {
    if tool != TOOL_NAME {
        return Described::Invalid(format!("the outputs server has no tool named {tool}"));
    }
    let (id, request) = match parse(args) {
        Ok(v) => v,
        Err(why) => return Described::Invalid(why),
    };
    let summary = match &request {
        Request::Lines(1, usize::MAX) => format!("Read saved output #{id}"),
        Request::Lines(a, b) => format!("Read lines {a}-{b} of saved output #{id}"),
        Request::Grep(text, _) => format!(
            "Find \"{}\" in saved output #{id}",
            policy::sanitize::one_line(text, GREP_SHOWN)
        ),
    };
    Described::Action(policy::Action {
        server: SERVER_NAME.into(),
        tool: TOOL_NAME.into(),
        preview: summary.clone(),
        summary,
        // The app's own data for this chat: never prompts, in any mode.
        scope: policy::Scope::AppData,
        risk: policy::Risk::Normal,
        read_only: true,
        outbound: false,
        keep_asking: None,
        network: false,
        hard_limit: None,
        payload: json!({ "id": id }),
    })
}

/// One numbered line as `read_output` shows it.
fn numbered(n: usize, sep: char, text: &str) -> String {
    format!("{n:>6}{sep} {text}\n")
}

/// The call: the part of the chat's stored output `id` that was asked for, at most
/// `budget` bytes. Only outputs of `chat` can be read.
pub async fn read(
    store: &Store,
    chat: ChatId,
    args: &Map<String, Value>,
    budget: usize,
) -> ToolOutput {
    let (id, request) = match parse(args) {
        Ok(v) => v,
        Err(why) => return ToolOutput::error(why),
    };
    let stored = match store.get_tool_output(chat, id).await {
        Ok(Some(stored)) => stored,
        Ok(None) => return ToolOutput::error(unknown_id(store, chat, id).await),
        Err(e) => return ToolOutput::error(format!("The saved outputs could not be read: {e}")),
    };
    let lines = output_lines(&stored.content);
    let total = lines.len();
    let room = budget.saturating_sub(FOOTER_ROOM).max(200);
    match request {
        Request::Lines(a, b) => {
            if total == 0 {
                return ToolOutput::text(format!("Output #{id} is empty."));
            }
            if a > total {
                return ToolOutput::error(format!(
                    "Output #{id} has {total} line{}, so there is no line {a}. Ask for lines from 1 to {total}.",
                    if total == 1 { "" } else { "s" }
                ));
            }
            let end = b.min(total);
            let mut out = String::new();
            let mut last = a - 1;
            for (i, line) in lines.iter().enumerate().take(end).skip(a - 1) {
                let row = numbered(i + 1, ':', line.text);
                if last >= a && out.len() + row.len() > room {
                    break;
                }
                if row.len() > room {
                    // One line larger than a result: its start.
                    let mut cut = room.min(line.text.len());
                    while !line.text.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    out.push_str(&numbered(i + 1, ':', &format!("{}…", &line.text[..cut])));
                } else {
                    out.push_str(&row);
                }
                last = i + 1;
            }
            let cut = last < end;
            out.push_str(&if last < total {
                let next = (last + 1, (last + (last + 1 - a).clamp(20, MAX_NEXT_LINES)).min(total));
                format!(
                    "[Showing lines {a}-{last} of {total}{}. Next: read_output {{\"id\": {id}, \"lines\": \"{}-{}\"}}.]",
                    if cut { " (as many as fit in one result)" } else { "" },
                    next.0,
                    next.1
                )
            } else if b > total && b != usize::MAX {
                format!("[Showing lines {a}-{last}: output #{id} ends at line {total}.]")
            } else {
                format!("[Showing lines {a}-{last} of {total}: the end of output #{id}.]")
            });
            ToolOutput::text(out)
        }
        Request::Grep(text, context) => {
            let wanted = text.to_lowercase();
            let shown_text = policy::sanitize::one_line(&text, GREP_SHOWN);
            let hits: Vec<usize> = lines
                .iter()
                .enumerate()
                .filter(|(_, l)| l.text.to_lowercase().contains(&wanted))
                .map(|(i, _)| i)
                .collect();
            if hits.is_empty() {
                return ToolOutput::text(format!(
                    "No line of output #{id} contains \"{shown_text}\" (letter case ignored). It has {total} line{}; read_output {{\"id\": {id}, \"lines\": \"1-{}\"}} shows the first ones.",
                    if total == 1 { "" } else { "s" },
                    total.clamp(1, 50)
                ));
            }
            let mut out = format!(
                "{} line{} of output #{id} contain{} \"{shown_text}\" ({total} lines in all; matching lines are marked with \":\", the lines around them with \"-\"):\n",
                hits.len(),
                if hits.len() == 1 { "" } else { "s" },
                if hits.len() == 1 { "s" } else { "" },
            );
            let mut shown_hits = 0;
            let mut printed_to = 0usize; // lines before this index are printed
            let mut cut_at = None;
            for (k, &hit) in hits.iter().enumerate() {
                let from = hit.saturating_sub(context).max(printed_to);
                let to = (hit + context + 1).min(total);
                let mut block = String::new();
                if printed_to > 0 && from > printed_to {
                    block.push_str("--\n");
                }
                for (i, line) in lines.iter().enumerate().take(to).skip(from) {
                    let sep = if hits.binary_search(&i).is_ok() {
                        ':'
                    } else {
                        '-'
                    };
                    block.push_str(&numbered(i + 1, sep, line.text));
                }
                if shown_hits > 0 && out.len() + block.len() > room {
                    cut_at = Some(k);
                    break;
                }
                out.push_str(&block);
                printed_to = to;
                // Later hits printed as context of this one count as shown.
                shown_hits = hits.iter().take_while(|&&h| h < to).count();
            }
            if cut_at.is_some() && shown_hits < hits.len() {
                let next = hits[shown_hits] + 1;
                out.push_str(&format!(
                    "[Showing {shown_hits} of {} matching lines. The next one is line {next}: read_output {{\"id\": {id}, \"lines\": \"{next}-{}\"}} reads on from there.]",
                    hits.len(),
                    (next + 49).min(total)
                ));
            }
            ToolOutput::text(out)
        }
    }
}

/// The message for an id that is not one of the chat's saved outputs.
async fn unknown_id(store: &Store, chat: ChatId, id: i64) -> String {
    let ids: Vec<String> = match store.list_tool_outputs(chat).await {
        Ok(list) => list
            .iter()
            .take(MAX_IDS_LISTED + 1)
            .map(|o| format!("#{}", o.id))
            .collect(),
        Err(_) => Vec::new(),
    };
    match ids.len() {
        0 => format!(
            "There is no output #{id} in this chat: this chat has no saved outputs. Outputs are saved when a result is shortened; its last line names the id."
        ),
        n if n > MAX_IDS_LISTED => format!(
            "There is no output #{id} in this chat. The newest saved outputs are {} and older ones. The last line of a shortened result names its id.",
            ids[..MAX_IDS_LISTED].join(", ")
        ),
        _ => format!(
            "There is no output #{id} in this chat. Saved outputs: {} (newest first). The last line of a shortened result names its id.",
            ids.join(", ")
        ),
    }
}

/// The built-in server registered with the tool host.
pub struct OutputsServer {
    app: AppHandle,
}

impl OutputsServer {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl BuiltinServer for OutputsServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![tool_def()]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _chat: &ChatContext,
    ) -> Described {
        describe(tool, args)
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        _action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if tool != TOOL_NAME {
            return ToolOutput::error(format!("the outputs server has no tool named {tool}"));
        }
        let Some(state) = self.app.try_state::<AppState>() else {
            return ToolOutput::error("The app is still starting. Try the call again.");
        };
        let budget = tools::shorten::budget_bytes(ctx.chat.result_cap_tokens);
        read(&state.store, ChatId(ctx.chat.chat_id), args, budget).await
    }
}

// ---- The tool card ----------------------------------------------------------------------

/// A saved output, for the tool card's "Full output".
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputDto {
    pub id: i64,
    pub tool: String,
    pub lines: u64,
    pub bytes: u64,
    pub text: String,
}

/// The saved output of one of the chat's tool calls, if the app saved one: what the tool
/// card shows "Full output" for (never read from the result's text).
#[tauri::command]
pub async fn tool_output_for_call(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
    call_id: String,
) -> AppResult<Option<i64>> {
    if window.label() != crate::MAIN_WINDOW {
        return Err(AppError::bad_request(
            "Only the main window shows tool output.",
        ));
    }
    Ok(saved_for_call(&state.store, ChatId(chat_id), &call_id).await)
}

/// The whole text of one of the chat's saved outputs. Another chat's id is "not found".
#[tauri::command]
pub async fn get_tool_output(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
) -> AppResult<ToolOutputDto> {
    if window.label() != crate::MAIN_WINDOW {
        return Err(AppError::bad_request(
            "Only the main window shows tool output.",
        ));
    }
    let stored = state
        .store
        .get_tool_output(ChatId(chat_id), id)
        .await?
        .ok_or_else(|| AppError::not_found("saved output"))?;
    Ok(ToolOutputDto {
        id: stored.info.id,
        tool: stored.info.tool,
        lines: stored.info.lines,
        bytes: stored.info.bytes,
        text: stored.content,
    })
}

#[cfg(test)]
mod tests;
