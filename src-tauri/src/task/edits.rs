//! Changes to a chat's task: what the model's `update_task` call may set, what the user's
//! edit in the Task tab sets, and the user's Pause, Resume and Mark done.
//!
//! These are plain functions over the stored record, used inside `Store::change_task`, so
//! every change is one step and the rules can be tested without the app.
//!
//! The rules that matter:
//!
//! - The model sets the goal, constraints, steps, next action and status, and adds notes.
//!   It can never set changed files, checks, the marks for what the user wrote, or "paused":
//!   those have no way in from a call's arguments.
//! - All text is cleaned on the way in (one line, nothing invisible, capped), whoever
//!   wrote it.
//! - What the user changed and the model has not been shown yet wins over what the model
//!   sends, once: the model is told, sees the user's text, and can then decide.
//! - Only the user pauses and resumes. While a task is paused the model's `status` is not
//!   applied.

use serde::Deserialize;
use serde_json::{Map, Value};
use store::{ChatId, StepStatus, Task, TaskNote, TaskState, TaskStatus, TaskStep, UserMarks};

/// Longest goal, in characters.
pub const GOAL_CHARS: usize = 400;
/// Longest step or constraint.
pub const ITEM_CHARS: usize = 200;
/// Longest next action.
pub const NEXT_ACTION_CHARS: usize = 300;
/// Longest note.
pub const NOTE_CHARS: usize = 400;
/// Most steps kept.
pub const MAX_STEPS: usize = 30;
/// Most constraints kept.
pub const MAX_CONSTRAINTS: usize = 12;
/// Most notes kept (the newest).
pub const MAX_NOTES: usize = 20;

/// The names used in `UserMarks::changed`.
pub const GOAL: &str = "goal";
pub const CONSTRAINTS: &str = "constraints";
pub const STEPS: &str = "steps";
pub const NEXT_ACTION: &str = "next action";

/// Text as it is stored: one line (every run of white space becomes one space), invisible
/// and control characters shown as escapes, at most `max` characters.
pub fn clean(text: &str, max: usize) -> String {
    let squeezed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    policy::sanitize::one_line(&squeezed, max)
}

/// `text` cleaned, and whether it had to be shortened to fit.
fn clean_noting(text: &str, max: usize) -> (String, bool) {
    let cleaned = clean(text, max);
    let whole = clean(text, usize::MAX);
    let cut = cleaned != whole;
    (cleaned, cut)
}

/// A list of texts cleaned: empty ones dropped, each kept once, at most `max_items`.
/// Returns the list and whether anything had to be shortened or left out.
fn clean_list(items: impl IntoIterator<Item = String>, max_chars: usize, max_items: usize) -> (Vec<String>, bool) {
    let mut out: Vec<String> = Vec::new();
    let mut cut = false;
    for item in items {
        let (text, shortened) = clean_noting(&item, max_chars);
        if text.is_empty() || out.contains(&text) {
            continue;
        }
        if out.len() == max_items {
            cut = true;
            break;
        }
        cut |= shortened;
        out.push(text);
    }
    (out, cut)
}

/// Steps cleaned: empty ones dropped, at most [`MAX_STEPS`]. Returns whether anything was
/// shortened or left out.
fn clean_steps(steps: impl IntoIterator<Item = TaskStep>) -> (Vec<TaskStep>, bool) {
    let mut out = Vec::new();
    let mut cut = false;
    for step in steps {
        let (text, shortened) = clean_noting(&step.text, ITEM_CHARS);
        if text.is_empty() {
            continue;
        }
        if out.len() == MAX_STEPS {
            cut = true;
            break;
        }
        cut |= shortened;
        out.push(TaskStep {
            text,
            status: step.status,
        });
    }
    (out, cut)
}

fn string_arg(args: &Map<String, Value>, name: &str) -> Result<Option<String>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        Some(_) => Err(format!("`{name}` must be a string.")),
    }
}

fn step_status(value: Option<&Value>) -> Result<StepStatus, String> {
    match value {
        None | Some(Value::Null) => Ok(StepStatus::Todo),
        Some(Value::String(s)) if s == "todo" => Ok(StepStatus::Todo),
        Some(Value::String(s)) if s == "doing" => Ok(StepStatus::Doing),
        Some(Value::String(s)) if s == "done" => Ok(StepStatus::Done),
        Some(_) => Err("A step's `status` is todo, doing or done.".into()),
    }
}

/// What an `update_task` call asks for, cleaned. A field that is `None` was not sent and
/// stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelUpdate {
    pub goal: Option<String>,
    pub constraints: Option<Vec<String>>,
    pub steps: Option<Vec<TaskStep>>,
    pub next_action: Option<String>,
    /// `Active` or `Done`; the model cannot pause.
    pub status: Option<TaskStatus>,
    pub note: Option<String>,
    /// What had to be shortened to fit, for the result.
    pub shortened: Vec<String>,
}

impl ModelUpdate {
    /// Reads a call's arguments. Only the six known fields are read: anything else in the
    /// arguments (`changed_files`, `checks`, …) has no effect.
    pub fn from_args(args: &Map<String, Value>) -> Result<Self, String> {
        let mut update = Self::default();
        let mut cut_notes: Vec<String> = Vec::new();

        if let Some(goal) = string_arg(args, "goal")? {
            let (goal, cut) = clean_noting(&goal, GOAL_CHARS);
            if cut {
                cut_notes.push(format!("goal (to {GOAL_CHARS} characters)"));
            }
            // An empty goal is not a goal.
            update.goal = (!goal.is_empty()).then_some(goal);
        }
        match args.get("constraints") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                let texts = items
                    .iter()
                    .map(|v| v.as_str().map(str::to_owned))
                    .collect::<Option<Vec<String>>>()
                    .ok_or("`constraints` must be a list of strings.")?;
                let (list, cut) = clean_list(texts, ITEM_CHARS, MAX_CONSTRAINTS);
                if cut {
                    cut_notes.push(format!(
                        "constraints (at most {MAX_CONSTRAINTS}, {ITEM_CHARS} characters each)"
                    ));
                }
                update.constraints = Some(list);
            }
            Some(_) => return Err("`constraints` must be a list of strings.".into()),
        }
        match args.get("steps") {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                let mut steps = Vec::new();
                for item in items {
                    let Some(object) = item.as_object() else {
                        return Err("`steps` must be a list of objects with `text` and `status`.".into());
                    };
                    let Some(text) = object.get("text").and_then(Value::as_str) else {
                        return Err("Each step needs a `text` string.".into());
                    };
                    steps.push(TaskStep {
                        text: text.to_owned(),
                        status: step_status(object.get("status"))?,
                    });
                }
                let (steps, cut) = clean_steps(steps);
                if cut {
                    cut_notes.push(format!(
                        "steps (at most {MAX_STEPS}, {ITEM_CHARS} characters each)"
                    ));
                }
                update.steps = Some(steps);
            }
            Some(_) => {
                return Err("`steps` must be a list of objects with `text` and `status`.".into());
            }
        }
        if let Some(next) = string_arg(args, "next_action")? {
            let (next, cut) = clean_noting(&next, NEXT_ACTION_CHARS);
            if cut {
                cut_notes.push(format!("next_action (to {NEXT_ACTION_CHARS} characters)"));
            }
            update.next_action = Some(next);
        }
        match args.get("status") {
            None | Some(Value::Null) => {}
            Some(Value::String(s)) if s == "active" => update.status = Some(TaskStatus::Active),
            Some(Value::String(s)) if s == "done" => update.status = Some(TaskStatus::Done),
            Some(_) => {
                return Err("`status` is active or done. Only the user pauses a task.".into());
            }
        }
        if let Some(note) = string_arg(args, "note")? {
            let (note, cut) = clean_noting(&note, NOTE_CHARS);
            if cut {
                cut_notes.push(format!("note (to {NOTE_CHARS} characters)"));
            }
            update.note = (!note.is_empty()).then_some(note);
        }
        update.shortened = cut_notes;
        Ok(update)
    }

    /// The call sets nothing.
    pub fn is_empty(&self) -> bool {
        self.goal.is_none()
            && self.constraints.is_none()
            && self.steps.is_none()
            && self.next_action.is_none()
            && self.status.is_none()
            && self.note.is_none()
    }
}

/// What applying a change did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    /// The stored task differs.
    pub changed: bool,
    /// Lines for the model about what was not applied as sent, and why.
    pub notices: Vec<String>,
}

/// Applies the model's update to the chat's task, creating the task when there is none.
pub fn apply_model(slot: &mut Option<Task>, update: ModelUpdate, now: i64) -> Applied {
    if update.is_empty() {
        return Applied::default();
    }
    let existed = slot.is_some();
    let before = slot.clone();
    let mut task = slot.clone().unwrap_or_else(|| Task::new(ChatId(0)));
    let mut notices = Vec::new();
    let user_changed = |name: &str| task.state.user.changed.iter().any(|c| c == name);

    // What the user changed and the model has not been shown yet stays, once.
    let mut goal = update.goal;
    if goal.as_ref().is_some_and(|g| *g != task.state.goal) && user_changed(GOAL) {
        goal = None;
        notices.push(not_changed("goal"));
    }
    let mut constraints = update.constraints;
    if constraints
        .as_ref()
        .is_some_and(|c| *c != task.state.constraints)
        && user_changed(CONSTRAINTS)
    {
        constraints = None;
        notices.push(not_changed("constraints"));
    }
    let mut steps = update.steps;
    if steps.as_ref().is_some_and(|s| *s != task.state.steps) && user_changed(STEPS) {
        steps = None;
        notices.push(not_changed("steps"));
    }
    let mut next_action = update.next_action;
    if next_action
        .as_ref()
        .is_some_and(|n| *n != task.state.next_action)
        && user_changed(NEXT_ACTION)
    {
        next_action = None;
        notices.push(not_changed("next action"));
    }

    // Who holds the status: the user, while the task is paused or was marked done by them
    // and the model has not been told.
    let paused = task.status == TaskStatus::Paused;
    let marked_done = task.status == TaskStatus::Done && task.state.user.done;
    let mut status = update.status;
    if status.is_some() && paused {
        status = None;
        notices.push(
            "Not changed: the status. The user paused this task; it stays paused until the user resumes it."
                .into(),
        );
    } else if status.is_some() && marked_done {
        status = None;
        notices.push("Not changed: the status. The user marked this task done.".into());
    }

    // A done task that gets a different goal is new work.
    if task.status == TaskStatus::Done
        && !marked_done
        && goal.as_ref().is_some_and(|g| *g != task.state.goal)
    {
        let user = std::mem::take(&mut task.state.user);
        let kept: Vec<String> = task
            .state
            .constraints
            .iter()
            .filter(|c| user.constraints.contains(c))
            .cloned()
            .collect();
        task.state = TaskState {
            constraints: kept.clone(),
            user: UserMarks {
                constraints: kept,
                ..UserMarks::default()
            },
            ..TaskState::default()
        };
        task.status = TaskStatus::Active;
        notices.push(
            "The earlier task was done, so this starts a new one: its steps, notes, changed files and checks were cleared."
                .into(),
        );
    }

    let content_before = task.state.clone();
    let state = &mut task.state;
    if let Some(goal) = goal
        && goal != state.goal
    {
        state.goal = goal;
        state.user.goal = false;
    }
    if let Some(list) = constraints
        && list != state.constraints
    {
        state.user.constraints.retain(|c| list.contains(c));
        state.constraints = list;
    }
    if let Some(list) = steps
        && list != state.steps
    {
        state.user.steps.retain(|t| list.iter().any(|s| s.text == *t));
        state.steps = list;
    }
    if let Some(next) = next_action
        && next != state.next_action
    {
        state.next_action = next;
        state.user.next_action = false;
    }
    if let Some(note) = update.note {
        state.notes.push(TaskNote { text: note, at: now });
        let extra = state.notes.len().saturating_sub(MAX_NOTES);
        state.notes.drain(..extra);
    }
    let content_changed = task.state != content_before;
    match status {
        Some(status) => task.status = status,
        // Work on a done task opens it again.
        None if content_changed && task.status == TaskStatus::Done && !marked_done => {
            task.status = TaskStatus::Active;
        }
        None => {}
    }

    if !existed && !has_content(&task.state) {
        return Applied {
            changed: false,
            notices,
        };
    }
    let mut after = Some(task);
    let changed = after != before;
    if changed && let Some(task) = &mut after {
        task.updated_at = now;
    }
    *slot = after;
    Applied { changed, notices }
}

fn not_changed(what: &str) -> String {
    format!(
        "Not changed: the {what}. The user changed it since you last saw the task; the lines below show the user's version."
    )
}

/// Who wrote each item of a list the user edited: an item the model wrote and the user
/// left as it was stays the model's; a new text, or one the user wrote before, is theirs.
fn users_items(new: &[String], old: &[String], old_users: &[String]) -> Vec<String> {
    new.iter()
        .filter(|t| old_users.contains(t) || !old.contains(t))
        .cloned()
        .collect()
}

fn mark_changed(user: &mut UserMarks, name: &str) {
    if !user.changed.iter().any(|c| c == name) {
        user.changed.push(name.to_owned());
    }
}

/// A step as the Task tab sends it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct StepInput {
    pub text: String,
    #[serde(default)]
    pub status: StepStatus,
}

/// The user's correction from the Task tab. A field that is `None` stays as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserEdit {
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub constraints: Option<Vec<String>>,
    #[serde(default)]
    pub steps: Option<Vec<StepInput>>,
    #[serde(default)]
    pub next_action: Option<String>,
}

/// Applies the user's correction, creating the task when there is none. What the user
/// wrote is marked as theirs, and what changed is remembered until the model has been
/// shown it. Returns whether the stored task differs.
pub fn apply_user(slot: &mut Option<Task>, edit: UserEdit, now: i64) -> bool {
    let existed = slot.is_some();
    let before = slot.clone();
    let mut task = slot.clone().unwrap_or_else(|| Task::new(ChatId(0)));
    let state = &mut task.state;
    if let Some(goal) = edit.goal {
        let goal = clean(&goal, GOAL_CHARS);
        if goal != state.goal {
            state.user.goal = !goal.is_empty();
            state.goal = goal;
            mark_changed(&mut state.user, GOAL);
        }
    }
    if let Some(list) = edit.constraints {
        let (list, _) = clean_list(list, ITEM_CHARS, MAX_CONSTRAINTS);
        if list != state.constraints {
            state.user.constraints = users_items(&list, &state.constraints, &state.user.constraints);
            state.constraints = list;
            mark_changed(&mut state.user, CONSTRAINTS);
        }
    }
    if let Some(steps) = edit.steps {
        let (steps, _) = clean_steps(steps.into_iter().map(|s| TaskStep {
            text: s.text,
            status: s.status,
        }));
        if steps != state.steps {
            let new: Vec<String> = steps.iter().map(|s| s.text.clone()).collect();
            let old: Vec<String> = state.steps.iter().map(|s| s.text.clone()).collect();
            state.user.steps = users_items(&new, &old, &state.user.steps);
            state.steps = steps;
            mark_changed(&mut state.user, STEPS);
        }
    }
    if let Some(next) = edit.next_action {
        let next = clean(&next, NEXT_ACTION_CHARS);
        if next != state.next_action {
            state.user.next_action = !next.is_empty();
            state.next_action = next;
            mark_changed(&mut state.user, NEXT_ACTION);
        }
    }
    if !existed && !has_content(&task.state) {
        return false;
    }
    let mut after = Some(task);
    let changed = after != before;
    if changed && let Some(task) = &mut after {
        task.updated_at = now;
    }
    *slot = after;
    changed
}

/// The user's Pause (`Paused`), Resume or Reopen (`Active`) and Mark done (`Done`).
/// Returns whether the stored task differs; there is nothing to do without a task.
pub fn set_status(slot: &mut Option<Task>, status: TaskStatus, now: i64) -> bool {
    let Some(task) = slot.as_mut() else {
        return false;
    };
    if task.status == status {
        return false;
    }
    match status {
        TaskStatus::Paused => {
            task.state.user.paused = true;
            task.state.user.done = false;
        }
        TaskStatus::Active => task.state.user.done = false,
        TaskStatus::Done => {
            task.state.user.done = true;
            task.state.user.paused = false;
        }
    }
    task.status = status;
    task.updated_at = now;
    true
}

/// A turn started and its note carried the task: the model has now been shown what the
/// user changed, and a paused task is being worked on again. Does not move "last updated".
pub fn turn_started(slot: &mut Option<Task>) {
    let Some(task) = slot.as_mut() else {
        return;
    };
    if task.status == TaskStatus::Paused {
        task.status = TaskStatus::Active;
    }
    task.state.user.changed.clear();
    task.state.user.paused = false;
    task.state.user.done = false;
}

/// An `update_task` result showed the task: the model has now been shown what the user
/// changed, and that the user marked the task done. A pause is not forgotten here: the
/// turn that follows it says so.
pub fn shown_in_result(slot: &mut Option<Task>) {
    let Some(task) = slot.as_mut() else {
        return;
    };
    task.state.user.changed.clear();
    task.state.user.done = false;
}

/// Whether the record holds anything the model or the user wrote. A record with only
/// changed files and checks is not yet a task to tell the model about.
pub fn has_content(state: &TaskState) -> bool {
    !state.goal.is_empty()
        || !state.constraints.is_empty()
        || !state.steps.is_empty()
        || !state.next_action.is_empty()
        || !state.notes.is_empty()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use store::{FileChange, TaskCheck};

    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().expect("an object").clone()
    }

    fn update(value: Value) -> ModelUpdate {
        ModelUpdate::from_args(&args(value)).expect("valid arguments")
    }

    fn step(text: &str, status: StepStatus) -> TaskStep {
        TaskStep {
            text: text.into(),
            status,
        }
    }

    fn texts(steps: &[TaskStep]) -> Vec<(&str, StepStatus)> {
        steps.iter().map(|s| (s.text.as_str(), s.status)).collect()
    }

    /// A task with facts the app recorded and marks only the user sets.
    fn task_with_facts() -> Task {
        let mut task = Task::new(ChatId(1));
        task.state.goal = "Add a dark mode switch".into();
        task.state.steps = vec![step("Read the settings page", StepStatus::Done)];
        task.state
            .record_file("/work/src/Switch.tsx", FileChange::Created, 10);
        task.state.record_check(TaskCheck {
            command: "npm test".into(),
            passed: true,
            at: 11,
            summary: "12 passed".into(),
            exit_code: Some(0),
        });
        task.updated_at = 11;
        task
    }

    #[test]
    fn the_models_update_sets_and_replaces() {
        let mut slot = None;
        let applied = apply_model(
            &mut slot,
            update(json!({
                "goal": "Fix the failing date tests",
                "constraints": ["No new dependencies"],
                "steps": [
                    {"text": "Run the tests", "status": "doing"},
                    {"text": "Fix the parser"}
                ],
                "next_action": "Run the tests",
                "note": "Two tests fail in test_dates.py"
            })),
            100,
        );
        assert!(applied.changed);
        assert_eq!(applied.notices, Vec::<String>::new());
        let task = slot.clone().expect("created");
        assert_eq!((task.status, task.updated_at), (TaskStatus::Active, 100));
        assert_eq!(task.state.goal, "Fix the failing date tests");
        assert_eq!(task.state.constraints, ["No new dependencies"]);
        assert_eq!(
            texts(&task.state.steps),
            [
                ("Run the tests", StepStatus::Doing),
                ("Fix the parser", StepStatus::Todo)
            ]
        );
        assert_eq!(task.state.next_action, "Run the tests");
        assert_eq!(
            task.state.notes,
            [TaskNote {
                text: "Two tests fail in test_dates.py".into(),
                at: 100
            }]
        );
        assert_eq!(task.state.user, UserMarks::default());

        // Only what is sent changes; a list replaces the old one; a note is added.
        let applied = apply_model(
            &mut slot,
            update(json!({
                "steps": [
                    {"text": "Run the tests", "status": "done"},
                    {"text": "Fix the parser", "status": "doing"},
                    {"text": "Run the tests again", "status": "todo"}
                ],
                "next_action": "Fix the parser",
                "note": "The parser drops the time zone"
            })),
            200,
        );
        assert!(applied.changed);
        let task = slot.clone().expect("still there");
        assert_eq!(task.state.goal, "Fix the failing date tests");
        assert_eq!(task.state.constraints, ["No new dependencies"]);
        assert_eq!(task.state.steps.len(), 3);
        assert_eq!(task.state.steps[0].status, StepStatus::Done);
        assert_eq!(task.state.next_action, "Fix the parser");
        assert_eq!(task.state.notes.len(), 2);
        assert_eq!(task.updated_at, 200);

        // An empty next action clears it; an empty goal is not a goal and changes nothing.
        apply_model(
            &mut slot,
            update(json!({"goal": "  ", "next_action": "", "status": "done"})),
            300,
        );
        let task = slot.clone().expect("still there");
        assert_eq!(task.state.goal, "Fix the failing date tests");
        assert_eq!(task.state.next_action, "");
        assert_eq!(task.status, TaskStatus::Done);

        // The same call again changes nothing, and "last updated" stays.
        let applied = apply_model(&mut slot, update(json!({"status": "done"})), 400);
        assert!(!applied.changed);
        assert_eq!(slot.expect("still there").updated_at, 300);
    }

    #[test]
    fn an_empty_update_creates_nothing() {
        for value in [json!({}), json!({"status": "done"}), json!({"goal": ""})] {
            let mut slot = None;
            let applied = apply_model(&mut slot, update(value.clone()), 5);
            assert!(!applied.changed, "{value}");
            assert_eq!(slot, None, "{value}");
        }
        // And leaves an existing task exactly as it is.
        let mut slot = Some(task_with_facts());
        let applied = apply_model(&mut slot, update(json!({})), 5);
        assert!(!applied.changed);
        assert_eq!(slot, Some(task_with_facts()));
    }

    #[test]
    fn text_is_cleaned_and_capped() {
        assert_eq!(
            clean("  two\n\nlines\tand   spaces ", 100),
            "two lines and spaces"
        );
        assert_eq!(clean("a\u{2028}b\u{00A0}c", 100), "a b c");
        assert_eq!(
            clean("evil\u{202E}txt\u{7}", 100),
            "evil\\u{202E}txt\\u{0007}"
        );
        assert_eq!(clean("abcdefghij", 5), "abcd…");
        assert_eq!(clean("日本語のテキスト", 4), "日本語…");
        assert_eq!(clean("", 5), "");

        let long = "x".repeat(1000);
        let many_steps: Vec<Value> = (0..40)
            .map(|i| json!({"text": format!("step {i}")}))
            .collect();
        let many_constraints: Vec<Value> = (0..20).map(|i| json!(format!("rule {i}"))).collect();
        let u = update(json!({
            "goal": long,
            "constraints": many_constraints,
            "steps": many_steps,
            "next_action": long,
            "note": long,
        }));
        assert_eq!(u.goal.as_ref().map(|g| g.chars().count()), Some(GOAL_CHARS));
        assert_eq!(
            u.next_action.as_ref().map(|g| g.chars().count()),
            Some(NEXT_ACTION_CHARS)
        );
        assert_eq!(u.note.as_ref().map(|g| g.chars().count()), Some(NOTE_CHARS));
        assert_eq!(u.steps.as_ref().map(Vec::len), Some(MAX_STEPS));
        assert_eq!(u.constraints.as_ref().map(Vec::len), Some(MAX_CONSTRAINTS));
        // The result can say what was cut.
        let said = u.shortened.join(" | ");
        for part in ["goal", "next_action", "note", "steps", "constraints"] {
            assert!(said.contains(part), "{part} is missing in: {said}");
        }

        // A long step or constraint is shortened, an empty one is dropped, a repeated
        // constraint is kept once.
        let u = update(json!({
            "constraints": ["  ", "Keep it small", "Keep it small", long],
            "steps": [{"text": ""}, {"text": long, "status": "doing"}],
        }));
        let constraints = u.constraints.expect("constraints");
        assert_eq!(constraints.len(), 2);
        assert_eq!(constraints[1].chars().count(), ITEM_CHARS);
        let steps = u.steps.expect("steps");
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].text.chars().count(), ITEM_CHARS);
        assert_eq!(steps[0].status, StepStatus::Doing);

        // Notes: the newest are kept.
        let mut slot = None;
        for i in 0..MAX_NOTES + 5 {
            apply_model(
                &mut slot,
                update(json!({"note": format!("note {i}")})),
                i as i64,
            );
        }
        let notes = slot.expect("task").state.notes;
        assert_eq!(notes.len(), MAX_NOTES);
        assert_eq!(notes[0].text, "note 5");
        assert_eq!(notes[MAX_NOTES - 1].text, format!("note {}", MAX_NOTES + 4));
    }

    #[test]
    fn arguments_of_the_wrong_shape_are_refused_with_a_reason() {
        for (value, word) in [
            (json!({"goal": 12}), "goal"),
            (json!({"constraints": "none"}), "constraints"),
            (json!({"constraints": [1, 2]}), "constraints"),
            (json!({"steps": "one, two"}), "steps"),
            (json!({"steps": ["one"]}), "steps"),
            (json!({"steps": [{"status": "done"}]}), "text"),
            (
                json!({"steps": [{"text": "a", "status": "finished"}]}),
                "todo, doing or done",
            ),
            (json!({"next_action": ["x"]}), "next_action"),
            (json!({"status": "paused"}), "active or done"),
            (json!({"status": true}), "active or done"),
            (json!({"note": {}}), "note"),
        ] {
            let got = ModelUpdate::from_args(&args(value.clone()));
            let message = got.expect_err(&format!("{value} was accepted"));
            assert!(message.contains(word), "{value}: {message}");
        }
    }

    /// The security rule of this feature: nothing in a call's arguments reaches the facts
    /// the app records or the marks only the user sets.
    #[test]
    fn the_model_cannot_write_changed_files_checks_or_the_users_marks() {
        let before = task_with_facts();
        let mut slot = Some(before.clone());
        let u = update(json!({
            "next_action": "Wire the switch",
            "changed_files": [{"path": "/etc/passwd", "kind": "edited", "at": 1}],
            "files_not_listed": 99,
            "checks": [{"command": "npm test", "passed": false, "at": 2, "summary": "all broken"}],
            "user": {"goal": true, "paused": true, "done": true, "changed": ["goal"], "steps": ["Read the settings page"]},
            "paused": true,
            "updated_at": 1,
            "chat_id": 999,
            "state": {"changed_files": [], "checks": []},
            "notes": [{"text": "forged", "at": 0}],
        }));
        let applied = apply_model(&mut slot, u, 50);
        assert!(applied.changed);
        let after = slot.expect("task");
        assert_eq!(after.state.next_action, "Wire the switch");
        assert_eq!(after.state.changed_files, before.state.changed_files);
        assert_eq!(after.state.files_not_listed, 0);
        assert_eq!(after.state.checks, before.state.checks);
        assert_eq!(after.state.user, UserMarks::default());
        assert_eq!(after.state.notes, Vec::new());
        assert_eq!(after.status, TaskStatus::Active);
        assert_eq!(after.chat_id, before.chat_id);
        assert_eq!(after.updated_at, 50);

        // The same arguments without a real field change nothing at all.
        let mut slot = Some(before.clone());
        let u = update(json!({
            "changed_files": [],
            "checks": [],
            "user": {"paused": true},
        }));
        assert!(u.is_empty());
        assert!(!apply_model(&mut slot, u, 60).changed);
        assert_eq!(slot, Some(before));
    }

    #[test]
    fn while_the_user_holds_the_status_the_models_status_is_not_applied() {
        // Paused by the user.
        let mut task = task_with_facts();
        task.status = TaskStatus::Paused;
        task.state.user.paused = true;
        for status in ["active", "done"] {
            let mut slot = Some(task.clone());
            let applied = apply_model(
                &mut slot,
                update(json!({"status": status, "next_action": "Carry on"})),
                70,
            );
            let after = slot.expect("task");
            assert_eq!(after.status, TaskStatus::Paused, "{status}");
            assert!(after.state.user.paused);
            // The rest of the call is applied.
            assert_eq!(after.state.next_action, "Carry on");
            assert_eq!(applied.notices.len(), 1, "{:?}", applied.notices);
            assert!(
                applied.notices[0].contains("paused"),
                "{:?}",
                applied.notices
            );
        }
        // Without a status in the call there is nothing to say.
        let mut slot = Some(task.clone());
        let applied = apply_model(&mut slot, update(json!({"next_action": "Carry on"})), 70);
        assert_eq!(applied.notices, Vec::<String>::new());
        assert_eq!(slot.expect("task").status, TaskStatus::Paused);

        // Marked done by the user, and the model has not been told yet: a change does not
        // reopen it, and a new goal does not start a new task.
        let mut task = task_with_facts();
        task.status = TaskStatus::Done;
        task.state.user.done = true;
        let mut slot = Some(task.clone());
        let applied = apply_model(
            &mut slot,
            update(json!({"goal": "Something else", "status": "active"})),
            80,
        );
        let after = slot.expect("task");
        assert_eq!(after.status, TaskStatus::Done);
        assert_eq!(after.state.changed_files.len(), 1);
        assert_eq!(applied.notices.len(), 1, "{:?}", applied.notices);
        assert!(
            applied.notices[0].contains("marked"),
            "{:?}",
            applied.notices
        );
    }

    #[test]
    fn a_done_task_that_gets_a_new_goal_starts_fresh() {
        let mut done = task_with_facts();
        done.status = TaskStatus::Done;
        done.state.constraints = vec!["Keep the tests green".into(), "Use tabs".into()];
        done.state.user.constraints = vec!["Keep the tests green".into()];
        done.state.user.goal = true;
        done.state.next_action = "Nothing".into();
        done.state.notes = vec![TaskNote {
            text: "old".into(),
            at: 1,
        }];

        // A different goal: new work. The user's constraints stay; the rest starts empty.
        let mut slot = Some(done.clone());
        let applied = apply_model(
            &mut slot,
            update(json!({"goal": "Write the release notes", "steps": [{"text": "Read the log"}]})),
            500,
        );
        assert!(applied.changed);
        let task = slot.expect("task");
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.state.goal, "Write the release notes");
        assert_eq!(
            texts(&task.state.steps),
            [("Read the log", StepStatus::Todo)]
        );
        assert_eq!(task.state.constraints, ["Keep the tests green"]);
        assert_eq!(task.state.user.constraints, ["Keep the tests green"]);
        assert!(!task.state.user.goal);
        assert_eq!(task.state.next_action, "");
        assert!(task.state.notes.is_empty());
        assert!(task.state.changed_files.is_empty() && task.state.checks.is_empty());
        assert_eq!(applied.notices.len(), 1, "{:?}", applied.notices);

        // The same goal with other changes: the task is open again, with its facts.
        let mut slot = Some(done.clone());
        apply_model(
            &mut slot,
            update(json!({"goal": "Add a dark mode switch", "next_action": "One more thing"})),
            500,
        );
        let task = slot.expect("task");
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.state.changed_files.len(), 1);
        assert_eq!(task.state.checks.len(), 1);
        assert!(task.state.user.goal);

        // A change that says "done" leaves it done.
        let mut slot = Some(done.clone());
        apply_model(
            &mut slot,
            update(json!({"note": "Checked once more", "status": "done"})),
            500,
        );
        assert_eq!(slot.expect("task").status, TaskStatus::Done);

        // A new goal on a task that is not done only replaces the goal.
        let mut active = done.clone();
        active.status = TaskStatus::Active;
        let mut slot = Some(active);
        apply_model(&mut slot, update(json!({"goal": "A sharper goal"})), 500);
        let task = slot.expect("task");
        assert_eq!(task.state.changed_files.len(), 1);
        assert_eq!(task.state.notes.len(), 1);
        assert!(!task.state.user.goal);
    }

    #[test]
    fn what_the_user_changed_and_the_model_has_not_seen_wins_once() {
        let mut task = task_with_facts();
        task.state.goal = "Put the switch in the header".into();
        task.state.user.goal = true;
        task.state.steps = vec![
            step("Read the settings page", StepStatus::Done),
            step("Ask before touching the footer", StepStatus::Todo),
        ];
        task.state.user.steps = vec!["Ask before touching the footer".into()];
        task.state.user.changed = vec![GOAL.into(), STEPS.into()];
        let stale = json!({
            "goal": "Put the switch in the footer",
            "steps": [{"text": "Read the settings page", "status": "done"}, {"text": "Edit the footer", "status": "doing"}],
            "next_action": "Edit the footer",
        });

        let mut slot = Some(task.clone());
        let applied = apply_model(&mut slot, update(stale.clone()), 600);
        let after = slot.clone().expect("task");
        assert_eq!(after.state.goal, "Put the switch in the header");
        assert_eq!(after.state.steps, task.state.steps);
        assert!(after.state.user.goal);
        // What the user did not touch is applied.
        assert_eq!(after.state.next_action, "Edit the footer");
        let said = applied.notices.join(" | ");
        assert!(said.contains("goal") && said.contains("steps"), "{said}");
        assert!(!said.contains("next"), "{said}");
        // `apply_model` does not decide when the model has seen the change.
        assert_eq!(after.state.user.changed, [GOAL, STEPS]);

        // The result showed the task: from now on the model's word counts again.
        shown_in_result(&mut slot);
        assert!(slot.as_ref().expect("task").state.user.changed.is_empty());
        let applied = apply_model(&mut slot, update(stale), 700);
        let after = slot.expect("task");
        assert_eq!(after.state.goal, "Put the switch in the footer");
        assert!(!after.state.user.goal);
        assert_eq!(after.state.steps.len(), 2);
        assert_eq!(after.state.steps[1].text, "Edit the footer");
        assert!(after.state.user.steps.is_empty());
        assert_eq!(applied.notices, Vec::<String>::new());

        // Sending the user's own text back is not a conflict.
        let mut slot = Some(task.clone());
        let applied = apply_model(
            &mut slot,
            update(json!({"goal": "Put the switch in the header"})),
            600,
        );
        assert_eq!(applied.notices, Vec::<String>::new());
    }

    #[test]
    fn what_the_user_wrote_stays_marked_as_long_as_its_text_stays() {
        let mut task = task_with_facts();
        task.state.constraints = vec!["No new dependencies".into()];
        task.state.user.constraints = vec!["No new dependencies".into()];
        task.state.steps = vec![step("Ask first", StepStatus::Todo)];
        task.state.user.steps = vec!["Ask first".into()];
        task.state.next_action = "Ask the user".into();
        task.state.user.next_action = true;

        let mut slot = Some(task);
        apply_model(
            &mut slot,
            update(json!({
                "constraints": ["No new dependencies", "Keep it small"],
                "steps": [{"text": "Ask first", "status": "done"}, {"text": "Then edit"}],
                "next_action": "Ask the user",
            })),
            10,
        );
        let user = slot.as_ref().expect("task").state.user.clone();
        assert_eq!(user.constraints, ["No new dependencies"]);
        assert_eq!(user.steps, ["Ask first"]);
        assert!(user.next_action);

        apply_model(
            &mut slot,
            update(json!({
                "constraints": ["Keep it small"],
                "steps": [{"text": "Then edit", "status": "doing"}],
                "next_action": "Edit the file",
            })),
            20,
        );
        let user = slot.expect("task").state.user;
        assert!(user.constraints.is_empty() && user.steps.is_empty());
        assert!(!user.next_action);
    }

    #[test]
    fn the_users_edit_is_marked_as_theirs_and_remembered_as_changed() {
        let mut slot = Some(task_with_facts());
        let changed = apply_user(
            &mut slot,
            UserEdit {
                goal: Some("  Put the switch\nin the header ".into()),
                ..UserEdit::default()
            },
            900,
        );
        assert!(changed);
        let task = slot.clone().expect("task");
        assert_eq!(task.state.goal, "Put the switch in the header");
        assert!(task.state.user.goal);
        assert_eq!(task.state.user.changed, [GOAL]);
        assert_eq!(task.updated_at, 900);
        // The facts are not the user's to edit either.
        assert_eq!(task.state.changed_files.len(), 1);
        assert_eq!(task.state.checks.len(), 1);

        // The same value again is no change.
        let changed = apply_user(
            &mut slot,
            UserEdit {
                goal: Some("Put the switch in the header".into()),
                ..UserEdit::default()
            },
            950,
        );
        assert!(!changed);
        assert_eq!(slot.as_ref().expect("task").updated_at, 900);

        // Steps: a new text is the user's; ticking a step changes the steps but not who
        // wrote it. Constraints likewise. Each name is listed once.
        let changed = apply_user(
            &mut slot,
            UserEdit {
                steps: Some(vec![
                    StepInput {
                        text: "Read the settings page".into(),
                        status: StepStatus::Todo,
                    },
                    StepInput {
                        text: "Ask before touching the footer".into(),
                        status: StepStatus::Todo,
                    },
                    StepInput {
                        text: " ".into(),
                        status: StepStatus::Todo,
                    },
                ]),
                constraints: Some(vec!["No new dependencies".into()]),
                next_action: Some("Ask about the footer".into()),
                goal: Some("Put the switch in the header, on the right".into()),
            },
            1000,
        );
        assert!(changed);
        let task = slot.clone().expect("task");
        assert_eq!(
            texts(&task.state.steps),
            [
                ("Read the settings page", StepStatus::Todo),
                ("Ask before touching the footer", StepStatus::Todo)
            ]
        );
        assert_eq!(task.state.user.steps, ["Ask before touching the footer"]);
        assert_eq!(task.state.user.constraints, ["No new dependencies"]);
        assert!(task.state.user.next_action);
        assert_eq!(
            task.state.user.changed,
            [GOAL, CONSTRAINTS, STEPS, NEXT_ACTION]
        );

        // Clearing a field the user wrote takes the mark away.
        apply_user(
            &mut slot,
            UserEdit {
                next_action: Some(String::new()),
                constraints: Some(Vec::new()),
                ..UserEdit::default()
            },
            1100,
        );
        let task = slot.expect("task");
        assert_eq!(task.state.next_action, "");
        assert!(!task.state.user.next_action);
        assert!(task.state.constraints.is_empty() && task.state.user.constraints.is_empty());
    }

    #[test]
    fn the_users_edit_is_cleaned_like_the_models() {
        let mut slot = None;
        apply_user(
            &mut slot,
            UserEdit {
                goal: Some("g".repeat(1000)),
                constraints: Some((0..20).map(|i| format!("rule {i}\u{202E}")).collect()),
                steps: Some(
                    (0..40)
                        .map(|i| StepInput {
                            text: format!("step {i}"),
                            status: StepStatus::Todo,
                        })
                        .collect(),
                ),
                next_action: Some("n".repeat(1000)),
            },
            1,
        );
        let state = slot.expect("created").state;
        assert_eq!(state.goal.chars().count(), GOAL_CHARS);
        assert_eq!(state.next_action.chars().count(), NEXT_ACTION_CHARS);
        assert_eq!(state.constraints.len(), MAX_CONSTRAINTS);
        assert_eq!(state.constraints[0], "rule 0\\u{202E}");
        assert_eq!(state.steps.len(), MAX_STEPS);
    }

    #[test]
    fn the_users_edit_can_start_a_task_but_not_an_empty_one() {
        let mut slot = None;
        assert!(!apply_user(&mut slot, UserEdit::default(), 1));
        assert_eq!(slot, None);
        assert!(!apply_user(
            &mut slot,
            UserEdit {
                goal: Some("   ".into()),
                steps: Some(Vec::new()),
                ..UserEdit::default()
            },
            1
        ));
        assert_eq!(slot, None);

        assert!(apply_user(
            &mut slot,
            UserEdit {
                goal: Some("Tidy the notes folder".into()),
                ..UserEdit::default()
            },
            2
        ));
        let task = slot.expect("created");
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(task.state.goal, "Tidy the notes folder");
        assert!(task.state.user.goal);
        assert_eq!(task.updated_at, 2);
    }

    #[test]
    fn pause_resume_and_mark_done_are_the_users() {
        // Nothing to pause without a task.
        let mut slot = None;
        assert!(!set_status(&mut slot, TaskStatus::Paused, 1));
        assert_eq!(slot, None);

        let mut slot = Some(task_with_facts());
        assert!(set_status(&mut slot, TaskStatus::Paused, 20));
        let task = slot.clone().expect("task");
        assert_eq!((task.status, task.updated_at), (TaskStatus::Paused, 20));
        assert!(task.state.user.paused);
        // Pausing a paused task changes nothing.
        assert!(!set_status(&mut slot, TaskStatus::Paused, 21));

        // Resume: active again, and the next turn still says there was a pause.
        assert!(set_status(&mut slot, TaskStatus::Active, 30));
        let task = slot.clone().expect("task");
        assert_eq!(task.status, TaskStatus::Active);
        assert!(task.state.user.paused);

        // Mark done; reopening takes the "marked done" back.
        assert!(set_status(&mut slot, TaskStatus::Done, 40));
        let task = slot.clone().expect("task");
        assert_eq!(task.status, TaskStatus::Done);
        assert!(task.state.user.done && !task.state.user.paused);
        assert!(set_status(&mut slot, TaskStatus::Active, 50));
        let task = slot.clone().expect("task");
        assert!(!task.state.user.done);
        assert_eq!((task.status, task.updated_at), (TaskStatus::Active, 50));

        // The facts are untouched by all of it.
        assert_eq!(
            task.state.changed_files,
            task_with_facts().state.changed_files
        );
        assert_eq!(task.state.checks, task_with_facts().state.checks);
    }

    #[test]
    fn a_turn_start_makes_a_paused_task_active_and_forgets_the_marks() {
        let mut task = task_with_facts();
        task.status = TaskStatus::Paused;
        task.state.user = UserMarks {
            goal: true,
            next_action: false,
            constraints: vec!["x".into()],
            steps: Vec::new(),
            changed: vec![GOAL.into()],
            paused: true,
            done: false,
        };
        let mut slot = Some(task);
        turn_started(&mut slot);
        let task = slot.clone().expect("task");
        assert_eq!(task.status, TaskStatus::Active);
        assert!(task.state.user.changed.is_empty());
        assert!(!task.state.user.paused);
        // Who wrote what is not forgotten, and "last updated" does not move.
        assert!(task.state.user.goal);
        assert_eq!(task.state.user.constraints, ["x"]);
        assert_eq!(task.updated_at, 11);

        // A done task stays done; the "marked done" has been said.
        let mut done = task_with_facts();
        done.status = TaskStatus::Done;
        done.state.user.done = true;
        let mut slot = Some(done);
        turn_started(&mut slot);
        let task = slot.expect("task");
        assert_eq!(task.status, TaskStatus::Done);
        assert!(!task.state.user.done);

        // A result keeps the pause for the turn that follows.
        let mut paused = task_with_facts();
        paused.status = TaskStatus::Paused;
        paused.state.user.paused = true;
        paused.state.user.changed = vec![STEPS.into()];
        let mut slot = Some(paused);
        shown_in_result(&mut slot);
        let task = slot.expect("task");
        assert_eq!(task.status, TaskStatus::Paused);
        assert!(task.state.user.paused);
        assert!(task.state.user.changed.is_empty());

        // Nothing to do without a task.
        let mut slot = None;
        turn_started(&mut slot);
        shown_in_result(&mut slot);
        assert_eq!(slot, None);
    }

    #[test]
    fn a_record_with_only_facts_has_no_content() {
        let mut state = TaskState::default();
        assert!(!has_content(&state));
        state.record_file("/w/a.txt", FileChange::Edited, 1);
        state.record_check(TaskCheck {
            command: "npm test".into(),
            passed: true,
            at: 2,
            summary: String::new(),
            exit_code: Some(0),
        });
        assert!(!has_content(&state));
        for fill in [
            (|s: &mut TaskState| s.goal = "g".into()) as fn(&mut TaskState),
            |s| s.constraints.push("c".into()),
            |s| {
                s.steps.push(TaskStep {
                    text: "s".into(),
                    status: StepStatus::Todo,
                })
            },
            |s| s.next_action = "n".into(),
            |s| {
                s.notes.push(TaskNote {
                    text: "n".into(),
                    at: 1,
                })
            },
        ] {
            let mut with = state.clone();
            fill(&mut with);
            assert!(has_content(&with));
        }
    }
}
