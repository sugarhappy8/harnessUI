//! The chat's task state (0.0.4, "visible, resumable task state").
//!
//! Goal, constraints, steps, changed files, completed checks and the next action of the
//! work a chat is doing, kept so it can be shown, paused, corrected and resumed after an
//! interruption or a restart. One record per chat, in the app's database
//! (`store::tasks`).
//!
//! Who writes what:
//!
//! - **The model**, through the tool `update_task` (`server.rs`): goal, constraints, steps,
//!   next action, status (active or done) and notes. Nothing else.
//! - **The app**, from the tool calls that ran ([`after_call`]): changed files, from
//!   successful `write_file`, `edit_file` and `delete_file` calls, and checks, from
//!   `run_command` calls whose command is a test, build, lint or type check
//!   (`checks.rs`). The model has no way to write either.
//! - **The user**, in the Task tab (the commands below): corrections to the goal,
//!   constraints, steps and next action; Pause, Resume, Mark done and Clear.
//!
//! At the start of every turn the task goes to the model as one app note
//! ([`begin_turn`], `note.rs`). Everything in the record is data: text the model wrote,
//! text the user wrote, and paths and command lines from tool calls. The note says it as
//! state, in quotes, capped and cleaned, and never includes a command's output.
//!
//! Nothing here touches approval settings, grants, approved folders or tool switches.
//! "Resume" in the Task tab sends an ordinary message through the ordinary send path.

mod checks;
mod edits;
mod note;
mod server;
mod when;

use mcp_host::ToolOutput;
use serde::Serialize;
use serde_json::{Map, Value};
use store::{ChatId, FileChange, StepStatus, Store, StoreError, Task, TaskCheck, TaskStatus};
use tauri::{AppHandle, Emitter, Manager, State};

pub use server::TaskServer;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Sent to the main window when a chat's task changed. The payload is the chat's id.
pub const CHANGED_EVENT: &str = "task-changed";

/// Longest command line kept for a check, in characters.
const COMMAND_CHARS: usize = 300;

/// The time, in Unix milliseconds.
pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

fn changed(app: &AppHandle, chat_id: i64) {
    let _ = app.emit_to(crate::MAIN_WINDOW, CHANGED_EVENT, chat_id);
}

// ---- What the app records from tool calls ------------------------------------------------

/// Something a finished tool call says about the work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fact {
    /// A file was created, edited or deleted.
    File { path: String, kind: FileChange },
    /// A test, build, lint or type check ran.
    Check(TaskCheck),
}

/// What a finished call says about the work, if anything.
///
/// It is read from the action the policy engine approved (the file tool's checked path,
/// the terminal's command line) and from whether the call failed, never from the wording
/// of a result. A call made through `call_tool` arrives here as the tool it ran.
pub fn fact_of(action: &policy::Action, out: &ToolOutput, now: i64) -> Option<Fact> {
    match (action.server.as_str(), action.tool.as_str()) {
        ("files", tool @ ("write_file" | "edit_file" | "delete_file")) => {
            // A change happened only when the call succeeded and was the write the policy
            // approved: an `edit_file` that only reads the file to explain why it failed
            // is described as a read, and answered as an error.
            if out.is_error
                || action.read_only
                || !matches!(action.scope, policy::Scope::FileWrite { .. })
            {
                return None;
            }
            let path = action.payload.get("canonical")?.as_str()?;
            if path.trim().is_empty() {
                return None;
            }
            let kind = match tool {
                "write_file" => match action.payload.get("expectedSha256") {
                    Some(Value::Null) => FileChange::Created,
                    Some(_) => FileChange::Edited,
                    None if action.payload.get("exists") == Some(&Value::Bool(false)) => {
                        FileChange::Created
                    }
                    None => FileChange::Edited,
                },
                "edit_file" => FileChange::Edited,
                _ => FileChange::Deleted,
            };
            Some(Fact::File {
                path: path.to_owned(),
                kind,
            })
        }
        ("terminal", "run_command") => {
            if !matches!(
                action.scope,
                policy::Scope::Command { .. }
                    | policy::Scope::Commands { .. }
                    | policy::Scope::ReadOnlyProgram { .. }
            ) {
                return None;
            }
            let command = action.payload.get("command")?.as_str()?;
            if !checks::is_check(command) {
                return None;
            }
            let outcome = checks::outcome(out);
            // Without an exit code a result that is not an error says nothing about how
            // the check went.
            if outcome.exit_code.is_none() && !out.is_error {
                return None;
            }
            Some(Fact::Check(TaskCheck {
                command: edits::clean(command, COMMAND_CHARS),
                passed: outcome.passed,
                at: now,
                summary: outcome.summary,
                exit_code: outcome.exit_code,
            }))
        }
        _ => None,
    }
}

/// Records a fact in the chat's task, creating the record when the chat has none: the
/// Task tab shows changed files and checks also when the model never wrote a goal.
/// Returns whether the stored task differs.
pub async fn record(store: &Store, chat_id: i64, fact: Fact, now: i64) -> Result<bool, StoreError> {
    let (_, changed) = store
        .change_task(ChatId(chat_id), move |slot| {
            let before = slot.clone();
            let task = slot.get_or_insert_with(|| Task::new(ChatId(chat_id)));
            match fact {
                Fact::File { path, kind } => task.state.record_file(&path, kind, now),
                Fact::Check(check) => task.state.record_check(check),
            }
            // A file created and deleted again in a chat with no task leaves no record.
            if before.is_none() && task.state == store::TaskState::default() {
                *slot = None;
            }
            let changed = *slot != before;
            if changed && let Some(task) = slot.as_mut() {
                task.updated_at = now;
            }
            changed
        })
        .await?;
    Ok(changed)
}

/// Called after every tool call of a turn: notes changed files and checks.
pub async fn after_call(
    app: &AppHandle,
    chat_id: i64,
    _tool: &str,
    _args: &Map<String, Value>,
    action: &policy::Action,
    out: &ToolOutput,
) {
    let now = now_ms();
    let Some(fact) = fact_of(action, out, now) else {
        return;
    };
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    match record(&state.store, chat_id, fact, now).await {
        Ok(true) => changed(app, chat_id),
        Ok(false) => {}
        // The chat was deleted while its turn ran, or its task cannot be read.
        Err(e) => tracing::debug!("a task fact was not recorded: {e}"),
    }
}

// ---- The note at the start of a turn -------------------------------------------------------

/// The task note the next real turn of a chat would start with, or `None` when it would
/// start without one: no task, nothing to tell, or the model already has the task state
/// from the conversation (`when.rs`). Read-only: for the Context inspector, and the same
/// text [`begin_turn`] gives the turn.
#[allow(
    dead_code,
    reason = "for the Context inspector (feature I), which calls it"
)]
pub async fn note_for_turn(state: &AppState, chat_id: i64) -> Option<String> {
    let summary = summary_marker(&state.store, chat_id).await;
    note_in(&state.store, &when::MEMORY, chat_id, summary).await
}

/// For the Context inspector: `None` when the chat has no task, otherwise one plain
/// sentence saying whether the task state goes with the next message, and why.
/// Read-only.
#[allow(
    dead_code,
    reason = "for the Context inspector (feature I), which calls it"
)]
pub async fn note_status_for_turn(state: &AppState, chat_id: i64) -> Option<String> {
    let summary = summary_marker(&state.store, chat_id).await;
    status_in(&state.store, &when::MEMORY, chat_id, summary).await
}

/// The chat's summary of earlier messages that the next turn uses (its row id), read the
/// way `turns::run` reads it: the latest summary on the branch the chat shows.
async fn summary_marker(store: &Store, chat_id: i64) -> Option<i64> {
    let path = store.current_path(ChatId(chat_id)).await.ok()?;
    let ids: Vec<store::MessageId> = path.iter().map(|m| m.id).collect();
    let summary = store
        .latest_summary_on_path(ChatId(chat_id), &ids)
        .await
        .ok()
        .flatten()?;
    Some(summary.id.0)
}

async fn note_in(
    store: &Store,
    memory: &when::TurnMemory,
    chat_id: i64,
    summary: Option<i64>,
) -> Option<String> {
    note_from(
        store.get_task(ChatId(chat_id)).await,
        memory.get(chat_id),
        summary,
    )
}

async fn status_in(
    store: &Store,
    memory: &when::TurnMemory,
    chat_id: i64,
    summary: Option<i64>,
) -> Option<String> {
    let task = store.get_task(ChatId(chat_id)).await.ok().flatten()?;
    Some(when::status_sentence(&task, memory.get(chat_id), summary))
}

/// The note for a task as it was read, if one is due. A task that cannot be read sends no
/// note: the Task tab says so and offers to clear it.
fn note_from(
    loaded: Result<Option<Task>, StoreError>,
    seen: Option<when::Seen>,
    summary: Option<i64>,
) -> Option<String> {
    match loaded {
        Ok(task) => task
            .as_ref()
            .and_then(|t| when::note_if_due(t, seen, summary)),
        Err(e) => {
            tracing::debug!("the task note was left out: {e}");
            None
        }
    }
}

/// For `turns::run`: the task note for the turn that starts now (exactly what
/// [`note_for_turn`] returns at this moment, often nothing), and the record that the turn
/// started: what the user changed is not announced again, a paused task is active again,
/// and the chat's memory of notes moves on.
pub async fn begin_turn(app: &AppHandle, chat_id: i64) -> Option<String> {
    let state = app.try_state::<AppState>()?;
    let summary = summary_marker(&state.store, chat_id).await;
    let (note, stored) = start_turn(&state.store, &when::MEMORY, chat_id, summary).await;
    if stored {
        changed(app, chat_id);
    }
    note
}

/// The note, and whether the stored task changed. Updates `memory` for the chat.
async fn start_turn(
    store: &Store,
    memory: &when::TurnMemory,
    chat_id: i64,
    summary: Option<i64>,
) -> (Option<String>, bool) {
    let seen = memory.get(chat_id);
    let result = store
        .change_task(ChatId(chat_id), move |slot| {
            // The same decision and renderer as `note_in`, over the same record, before it
            // changes.
            let note = slot
                .as_ref()
                .and_then(|t| when::note_if_due(t, seen, summary));
            let before = slot.clone();
            edits::turn_started(slot);
            (note, *slot != before)
        })
        .await;
    let (note, stored) = turn_start_from(result.map(|(_, r)| r));
    memory.set(chat_id, when::after_turn(seen, note.is_some(), summary));
    (note, stored)
}

/// The note and whether the record changed; nothing at all when the task cannot be read.
fn turn_start_from(result: Result<(Option<String>, bool), StoreError>) -> (Option<String>, bool) {
    match result {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!("the task note was left out: {e}");
            (None, false)
        }
    }
}

// ---- The Task tab ---------------------------------------------------------------------------

/// A constraint as the Task tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ItemDto {
    pub text: String,
    /// The user wrote it.
    pub by_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepDto {
    pub text: String,
    pub status: StepStatus,
    pub by_user: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDto {
    /// The file's path as it is, for copying.
    pub path: String,
    /// The path for showing: invisible characters as escapes.
    pub shown: String,
    pub kind: FileChange,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDto {
    pub command: String,
    pub passed: bool,
    /// It ran to its end. A check that did not (timed out, stopped) has no verdict.
    pub finished: bool,
    pub at: i64,
    /// One line of the command's output. Text from outside: shown as plain text.
    pub summary: String,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteDto {
    pub text: String,
    pub at: i64,
}

/// A chat's task as the Task tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDto {
    pub chat_id: i64,
    pub status: TaskStatus,
    pub updated_at: i64,
    pub goal: String,
    pub goal_by_user: bool,
    pub constraints: Vec<ItemDto>,
    pub steps: Vec<StepDto>,
    /// The most recently changed first.
    pub changed_files: Vec<FileDto>,
    pub files_not_listed: u32,
    /// The most recent first.
    pub checks: Vec<CheckDto>,
    pub next_action: String,
    pub next_action_by_user: bool,
    /// The newest first.
    pub notes: Vec<NoteDto>,
}

impl From<Task> for TaskDto {
    fn from(task: Task) -> Self {
        let state = task.state;
        let user = state.user;
        Self {
            chat_id: task.chat_id.0,
            status: task.status,
            updated_at: task.updated_at,
            goal: state.goal,
            goal_by_user: user.goal,
            constraints: state
                .constraints
                .into_iter()
                .map(|text| ItemDto {
                    by_user: user.constraints.contains(&text),
                    text,
                })
                .collect(),
            steps: state
                .steps
                .into_iter()
                .map(|s| StepDto {
                    by_user: user.steps.contains(&s.text),
                    text: s.text,
                    status: s.status,
                })
                .collect(),
            changed_files: state
                .changed_files
                .into_iter()
                .rev()
                .map(|f| FileDto {
                    shown: policy::sanitize::visible(&f.path),
                    path: f.path,
                    kind: f.kind,
                    at: f.at,
                })
                .collect(),
            files_not_listed: state.files_not_listed,
            checks: state
                .checks
                .into_iter()
                .rev()
                .map(|c| CheckDto {
                    finished: c.exit_code.is_some(),
                    command: c.command,
                    passed: c.passed,
                    at: c.at,
                    summary: c.summary,
                    exit_code: c.exit_code,
                })
                .collect(),
            next_action: state.next_action,
            next_action_by_user: user.next_action,
            notes: state
                .notes
                .into_iter()
                .rev()
                .map(|n| NoteDto {
                    text: n.text,
                    at: n.at,
                })
                .collect(),
        }
    }
}

/// The Task tab belongs to the main window (the capability files grant these commands to
/// it alone; this is the same rule checked again).
fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window shows the task.",
        ))
    }
}

fn store_error(e: StoreError) -> AppError {
    match e {
        StoreError::NotFound { .. } => AppError::not_found("chat"),
        StoreError::Json(e) => AppError::new(
            "task_unreadable",
            "The saved task of this chat could not be read.",
            "Choose Clear task to remove it. The chat itself is not affected.",
            e.to_string(),
        ),
        other => other.into(),
    }
}

/// The user's correction, stored. Returns the task as it now stands (none when there was
/// no task and the edit was empty) and whether it changed.
async fn user_edit(
    store: &Store,
    chat_id: i64,
    edit: edits::UserEdit,
    now: i64,
) -> Result<(Option<Task>, bool), StoreError> {
    store
        .change_task(ChatId(chat_id), move |slot| {
            edits::apply_user(slot, edit, now)
        })
        .await
}

/// The user's Pause, Resume, Reopen or Mark done, stored.
async fn user_status(
    store: &Store,
    chat_id: i64,
    status: TaskStatus,
    now: i64,
) -> Result<(Option<Task>, bool), StoreError> {
    store
        .change_task(ChatId(chat_id), move |slot| {
            edits::set_status(slot, status, now)
        })
        .await
}

/// The chat's task, or nothing when it has none.
#[tauri::command]
pub async fn get_task(state: State<'_, AppState>, chat_id: i64) -> AppResult<Option<TaskDto>> {
    let task = state
        .store
        .get_task(ChatId(chat_id))
        .await
        .map_err(store_error)?;
    Ok(task.map(TaskDto::from))
}

/// The user corrects the goal, the constraints, the steps or the next action. Fields left
/// out stay as they are. Starts a task when the chat has none.
#[tauri::command]
pub async fn edit_task(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    edit: edits::UserEdit,
) -> AppResult<Option<TaskDto>> {
    only_main_window(window.label())?;
    let (task, stored) = user_edit(&state.store, chat_id, edit, now_ms())
        .await
        .map_err(store_error)?;
    if stored {
        changed(&app, chat_id);
    }
    Ok(task.map(TaskDto::from))
}

/// The user pauses the task (`paused`), resumes or reopens it (`active`), or marks it done
/// (`done`). Stopping a running turn and sending the message that resumes the work are
/// the window's part, through the same commands as the Stop button and the message box.
#[tauri::command]
pub async fn set_task_status(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    status: TaskStatus,
) -> AppResult<Option<TaskDto>> {
    only_main_window(window.label())?;
    let (task, stored) = user_status(&state.store, chat_id, status, now_ms())
        .await
        .map_err(store_error)?;
    if stored {
        changed(&app, chat_id);
    }
    Ok(task.map(TaskDto::from))
}

/// The user clears the chat's task. Works also when the saved task cannot be read.
#[tauri::command]
pub async fn clear_task(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
) -> AppResult<()> {
    only_main_window(window.label())?;
    // The next turn starts as if the app had just started.
    when::MEMORY.forget(chat_id);
    if state.store.delete_task(ChatId(chat_id)).await? {
        changed(&app, chat_id);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
