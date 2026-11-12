//! The `task` tool server: `update_task`.
//!
//! The tool reads and writes one row of the app's own database: the task of the chat
//! whose turn calls it. The chat is the one the call comes from, never one named in the
//! arguments. Nothing outside the app is touched, so it never prompts
//! (`policy::Scope::AppData`); each call is still decided by the policy engine and
//! written to the audit log like any other.
//!
//! What a call can change is decided in `edits.rs`; what its result says, in `note.rs`.

use std::sync::Arc;

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use serde_json::{Map, Value, json};
use store::{ChatId, StoreError};
use tauri::{AppHandle, Emitter, Manager};

use super::{edits, note};
use crate::state::AppState;

/// The server's name. It is a companion: offered whenever another tool is on in the chat
/// (`tool_select::COMPANION_SERVERS`).
pub const SERVER_NAME: &str = "task";
/// The tool's name.
pub const TOOL_NAME: &str = "update_task";

/// What a helper is told when it calls the tool.
pub const HELPER_REFUSAL: &str = "The task belongs to the main conversation, and a helper cannot change it. Nothing was saved. Carry on with the work you were given.";

/// Kept short: this definition is sent with every request of a chat that has tools on.
const DESCRIPTION: &str = "Save the task state the user sees and work resumes from. Send only what changed; a list replaces the old one.";

/// The tool as offered to the model.
pub fn tool_def() -> ToolDef {
    ToolDef {
        name: TOOL_NAME.into(),
        description: DESCRIPTION.into(),
        input_schema: json!({
            "type": "object",
            "properties": {
                "goal": {"type": "string"},
                "constraints": {"type": "array", "items": {"type": "string"}},
                "steps": {"type": "array", "items": {
                    "type": "object",
                    "properties": {
                        "text": {"type": "string"},
                        "status": {"enum": ["todo", "doing", "done"]}
                    },
                    "required": ["text"]
                }},
                "next_action": {"type": "string"},
                "status": {"enum": ["active", "done"]},
                "note": {"type": "string"}
            },
            "additionalProperties": false
        }),
        destructive: Some(false),
        read_only: Some(false),
    }
}

/// Arguments into an action, or the reason they cannot be one. No side effects.
pub fn describe(tool: &str, args: &Map<String, Value>) -> Described {
    if tool != TOOL_NAME {
        return Described::Invalid(format!("There is no tool named {tool:?} in this group."));
    }
    let update = match edits::ModelUpdate::from_args(args) {
        Ok(update) => update,
        Err(why) => return Described::Invalid(why),
    };
    let mut parts: Vec<String> = Vec::new();
    let mut preview: Vec<String> = Vec::new();
    if let Some(goal) = &update.goal {
        parts.push("goal".into());
        preview.push(format!("Goal: {goal}"));
    }
    if let Some(constraints) = &update.constraints {
        parts.push(count(constraints.len(), "constraint"));
        preview.extend(constraints.iter().map(|c| format!("Constraint: {c}")));
    }
    if let Some(steps) = &update.steps {
        parts.push(count(steps.len(), "step"));
        preview.extend(steps.iter().map(|s| format!("Step: {}", s.text)));
    }
    if let Some(next) = &update.next_action {
        parts.push("next action".into());
        preview.push(format!("Next action: {next}"));
    }
    if let Some(status) = update.status {
        parts.push(format!("status {}", status.as_str()));
    }
    if let Some(note) = &update.note {
        parts.push("note".into());
        preview.push(format!("Note: {note}"));
    }
    let summary = if parts.is_empty() {
        "Read the task".to_owned()
    } else {
        format!("Update the task: {}", parts.join(", "))
    };
    let preview = if preview.is_empty() {
        summary.clone()
    } else {
        preview.join("\n")
    };
    Described::Action(policy::Action {
        server: SERVER_NAME.into(),
        tool: TOOL_NAME.into(),
        summary: policy::sanitize::one_line(&summary, 200),
        preview: policy::sanitize::visible(&preview),
        scope: policy::Scope::AppData,
        risk: policy::Risk::Normal,
        read_only: false,
        outbound: false,
        keep_asking: None,
        network: false,
        hard_limit: None,
        payload: Value::Null,
    })
}

/// `1 step`, `2 steps`.
fn count(n: usize, what: &str) -> String {
    if n == 1 {
        format!("1 {what}")
    } else {
        format!("{n} {what}s")
    }
}

/// What the server needs, without the app around it.
pub struct Core {
    pub store: store::Store,
    /// Tells the window that a chat's task changed.
    pub notify: Arc<dyn Fn(i64) + Send + Sync>,
    /// Whether a helper (`run_subagent`) is at work in a chat. A helper has the chat's
    /// tools, and its plan is not the chat's task.
    pub helper_at_work: Arc<dyn Fn(i64) -> bool + Send + Sync>,
    /// The time, in Unix milliseconds.
    pub now: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl Core {
    /// Runs one `update_task` call for the chat it came from.
    pub async fn call(&self, tool: &str, args: &Map<String, Value>, chat_id: i64) -> ToolOutput {
        if tool != TOOL_NAME {
            return ToolOutput::error(format!("There is no tool named {tool:?} in this group."));
        }
        if (self.helper_at_work)(chat_id) {
            return ToolOutput::error(HELPER_REFUSAL);
        }
        let update = match edits::ModelUpdate::from_args(args) {
            Ok(update) => update,
            Err(why) => return ToolOutput::error(format!("Nothing was saved. {why}")),
        };
        let shortened = update.shortened.clone();
        let now = (self.now)();
        let result = self
            .store
            .change_task(ChatId(chat_id), move |slot| {
                let before = slot.clone();
                let applied = edits::apply_model(slot, update, now);
                // The result shows the task as it now stands, with what the user changed;
                // from then on the model has seen it.
                let shown = slot.as_ref().map(note::for_result);
                edits::shown_in_result(slot);
                let stored = *slot != before;
                (applied, shown, stored)
            })
            .await;
        let (applied, shown, stored) = match result {
            Ok((_, r)) => r,
            Err(StoreError::NotFound { .. }) => {
                return ToolOutput::error(
                    "The chat this task belongs to no longer exists. Nothing was saved.",
                );
            }
            Err(StoreError::Json(_)) => {
                return ToolOutput::error(
                    "The saved task of this chat could not be read, so nothing was saved. The user can clear it in the Task tab.",
                );
            }
            Err(e) => {
                tracing::warn!("update_task: {e}");
                return ToolOutput::error("The task could not be saved. Nothing was changed.");
            }
        };
        if stored {
            (self.notify)(chat_id);
        }
        let Some(shown) = shown else {
            return ToolOutput::text(
                "No task is recorded for this chat yet. A goal or steps start one.",
            );
        };
        let mut lines = vec![if applied.changed {
            "Task saved.".to_owned()
        } else {
            "Nothing changed.".to_owned()
        }];
        lines.extend(applied.notices);
        if applied.changed && !shortened.is_empty() {
            lines.push(format!("Shortened to fit: {}.", shortened.join(", ")));
        }
        lines.push(shown);
        ToolOutput::text(lines.join("\n"))
    }
}

/// The built-in server registered with the tool host.
pub struct TaskServer {
    app: AppHandle,
}

impl TaskServer {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

#[async_trait]
impl BuiltinServer for TaskServer {
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
        // The server is registered before the app's state exists.
        let Some(state) = self.app.try_state::<AppState>() else {
            return ToolOutput::error("The app is still starting. Try the call again.");
        };
        let app = self.app.clone();
        let core = Core {
            store: state.store.clone(),
            notify: Arc::new(move |chat_id| {
                let _ = app.emit_to(crate::MAIN_WINDOW, super::CHANGED_EVENT, chat_id);
            }),
            helper_at_work: Arc::new(|chat_id| crate::subagent::running_in(chat_id).is_some()),
            now: Arc::new(super::now_ms),
        };
        core.call(tool, args, ctx.chat.chat_id).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

    use store::{FileChange, Task, TaskCheck, TaskStatus};

    use super::*;

    struct Fixture {
        core: Core,
        seen: Arc<Mutex<Vec<i64>>>,
        helper: Arc<AtomicBool>,
        clock: Arc<AtomicI64>,
        chat: i64,
        other: i64,
    }

    async fn fixture() -> Fixture {
        let store = store::Store::open_in_memory().expect("open");
        let mut chats = Vec::new();
        for title in ["mine", "other"] {
            let id = store
                .create_chat(store::NewChat {
                    title: title.into(),
                    ..Default::default()
                })
                .await
                .expect("chat");
            chats.push(id.0);
        }
        let seen: Arc<Mutex<Vec<i64>>> = Arc::default();
        let sink = seen.clone();
        let helper = Arc::new(AtomicBool::new(false));
        let at_work = helper.clone();
        let clock = Arc::new(AtomicI64::new(1000));
        let time = clock.clone();
        Fixture {
            core: Core {
                store,
                notify: Arc::new(move |chat| sink.lock().unwrap().push(chat)),
                helper_at_work: Arc::new(move |_| at_work.load(Ordering::Relaxed)),
                now: Arc::new(move || time.load(Ordering::Relaxed)),
            },
            seen,
            helper,
            clock,
            chat: chats[0],
            other: chats[1],
        }
    }

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().expect("an object").clone()
    }

    impl Fixture {
        async fn update(&self, value: Value) -> ToolOutput {
            self.core.call(TOOL_NAME, &args(value), self.chat).await
        }
        async fn task(&self, chat: i64) -> Option<Task> {
            self.core.store.get_task(ChatId(chat)).await.expect("get")
        }
        fn notified(&self) -> Vec<i64> {
            std::mem::take(&mut *self.seen.lock().unwrap())
        }
    }

    /// The name and the group other parts of the app already use for this tool.
    #[test]
    fn the_tool_is_the_one_the_rest_of_the_app_expects() {
        let def = tool_def();
        assert_eq!((SERVER_NAME, def.name.as_str()), ("task", "update_task"));
        assert!(crate::tool_select::COMPANION_SERVERS.contains(&SERVER_NAME));
        let entry = crate::tool_catalog::entry(TOOL_NAME).expect("a catalog entry");
        assert_eq!(entry.group, SERVER_NAME);
        // The catalog's examples fit the real schema.
        let schema = mcp_host::CompiledSchema::compile(&def.input_schema).expect("compiles");
        for example in entry.examples {
            let value: Value = serde_json::from_str(example).expect("json");
            assert_eq!(schema.validate(&value), Ok(()), "{example}");
        }
        // The schema the local model server gets is the same shape.
        assert_eq!(
            schema.llama()["properties"]["steps"]["items"]["properties"]["status"]["enum"],
            json!(["todo", "doing", "done"])
        );
    }

    /// Small models get this definition with every request: it has to stay short.
    #[test]
    fn the_definition_is_short() {
        let def = tool_def();
        let chars = def.description.chars().count() + def.input_schema.to_string().chars().count();
        let tokens =
            crate::tool_select::definition_cost(&def.name, &def.description, &def.input_schema);
        assert!(
            def.description.chars().count() <= 180,
            "{}",
            def.description.chars().count()
        );
        assert!(
            chars <= 560,
            "description and schema take {chars} characters"
        );
        assert!(tokens <= 215, "the definition costs about {tokens} tokens");
        // Nothing in it names what the app tracks by itself as something to send.
        let schema = def.input_schema.to_string();
        assert!(!schema.contains("changed_files") && !schema.contains("checks"));
    }

    #[test]
    fn the_schema_takes_the_agreed_arguments_and_nothing_else() {
        let schema = mcp_host::CompiledSchema::compile(&tool_def().input_schema).expect("compiles");
        for good in [
            json!({}),
            json!({"goal": "g"}),
            json!({"constraints": ["a", "b"], "next_action": "n", "note": "x", "status": "done"}),
            json!({"steps": [{"text": "a"}, {"text": "b", "status": "doing"}], "status": "active"}),
        ] {
            assert_eq!(schema.validate(&good), Ok(()), "{good}");
        }
        for bad in [
            // What the app tracks is not an argument.
            json!({"changed_files": [{"path": "/x", "kind": "edited", "at": 1}]}),
            json!({"checks": [{"command": "npm test", "passed": true}]}),
            json!({"goal": "g", "user": {"paused": false}}),
            json!({"chat_id": 2, "goal": "g"}),
            // Only the user pauses.
            json!({"status": "paused"}),
            json!({"steps": [{"status": "done"}]}),
            json!({"steps": [{"text": "a", "status": "finished"}]}),
            json!({"steps": "a, b"}),
            json!({"goal": 1}),
        ] {
            assert!(schema.validate(&bad).is_err(), "{bad} was accepted");
        }
    }

    #[test]
    fn a_call_is_the_apps_own_data_and_never_asks() {
        let Described::Action(action) = describe(
            TOOL_NAME,
            &args(json!({
                "goal": "Fix the\ndate tests\u{202E}",
                "steps": [{"text": "Run the tests"}, {"text": "Fix the parser"}],
                "next_action": "Run the tests"
            })),
        ) else {
            panic!("not an action");
        };
        assert_eq!(
            (action.server.as_str(), action.tool.as_str()),
            ("task", "update_task")
        );
        assert_eq!(action.scope, policy::Scope::AppData);
        assert_eq!(action.risk, policy::Risk::Normal);
        assert!(!action.read_only && !action.outbound && !action.network);
        assert_eq!(action.keep_asking, None);
        assert_eq!(action.hard_limit, None);
        assert_eq!(action.payload, Value::Null);
        // One line for the audit log, with nothing invisible in it.
        assert!(
            !action.summary.contains('\n') && !action.summary.contains('\u{202E}'),
            "{}",
            action.summary
        );
        assert!(
            action.summary.contains("goal") && action.summary.contains("2 steps"),
            "{}",
            action.summary
        );
        assert!(!action.preview.contains('\u{202E}'));

        // The engine lets it run without a card in every kind of turn, and still refuses
        // it when the tool is off.
        let grants = BTreeSet::<policy::Scope>::new();
        for (recovered, untrusted, allow_all) in [
            (false, false, false),
            (true, true, false),
            (false, true, true),
        ] {
            let turn = policy::TurnContext {
                tool_enabled: true,
                server_ready: true,
                recovered_from_text: recovered,
                untrusted_in_turn: untrusted,
                allow_all,
                full_access: false,
            };
            assert_eq!(
                policy::evaluate(&action, &turn, &grants),
                policy::Decision::AllowedByGrant
            );
        }
        let off = policy::TurnContext {
            tool_enabled: false,
            server_ready: true,
            ..Default::default()
        };
        assert!(matches!(
            policy::evaluate(&action, &off, &grants),
            policy::Decision::Refused { .. }
        ));

        // A call that cannot be read is refused before it runs, with the reason.
        for bad in [json!({"steps": "a, b"}), json!({"status": "paused"})] {
            assert!(
                matches!(
                    describe(TOOL_NAME, &args(bad.clone())),
                    Described::Invalid(_)
                ),
                "{bad}"
            );
        }
        assert!(matches!(
            describe("delete_task", &Map::new()),
            Described::Invalid(_)
        ));
    }

    #[tokio::test]
    async fn a_call_saves_the_task_and_answers_with_it() {
        let f = fixture().await;
        let out = f
            .update(json!({
                "goal": "Fix the failing date tests",
                "steps": [{"text": "Run the tests", "status": "doing"}, {"text": "Fix the parser"}],
                "next_action": "Run the tests"
            }))
            .await;
        assert!(!out.is_error, "{out:?}");
        assert_eq!(
            out.joined_text(),
            [
                "Task saved.",
                "- status: active",
                "- goal: \"Fix the failing date tests\"",
                "- steps (0 of 2 done): 1 [doing] \"Run the tests\"; 2 [todo] \"Fix the parser\"",
                "- next action: \"Run the tests\"",
            ]
            .join("\n")
        );
        let task = f.task(f.chat).await.expect("saved");
        assert_eq!((task.status, task.updated_at), (TaskStatus::Active, 1000));
        assert_eq!(task.state.goal, "Fix the failing date tests");
        assert_eq!(f.notified(), [f.chat]);
        // Only this chat has a task.
        assert_eq!(f.task(f.other).await, None);

        // A call that changes nothing says so, shows the task, and tells no one.
        f.clock.store(2000, Ordering::Relaxed);
        let out = f
            .update(json!({"goal": "Fix the failing date tests"}))
            .await;
        assert!(!out.is_error);
        assert!(
            out.joined_text()
                .starts_with("Nothing changed.\n- status: active\n"),
            "{}",
            out.joined_text()
        );
        assert_eq!(f.task(f.chat).await.expect("task").updated_at, 1000);
        assert_eq!(f.notified(), Vec::<i64>::new());

        // An empty call reads the task.
        let out = f.update(json!({})).await;
        assert!(
            out.joined_text()
                .contains("- goal: \"Fix the failing date tests\"")
        );

        // Marking it done.
        let out = f.update(json!({"status": "done", "next_action": ""})).await;
        assert!(
            out.joined_text()
                .starts_with("Task saved.\n- status: done\n"),
            "{}",
            out.joined_text()
        );
        assert_eq!(f.task(f.chat).await.expect("task").status, TaskStatus::Done);
        assert_eq!(f.notified(), [f.chat]);
    }

    #[tokio::test]
    async fn an_empty_call_without_a_task_creates_nothing() {
        let f = fixture().await;
        let out = f.update(json!({})).await;
        assert!(!out.is_error);
        assert!(
            out.joined_text()
                .starts_with("No task is recorded for this chat yet."),
            "{}",
            out.joined_text()
        );
        assert_eq!(f.task(f.chat).await, None);
        assert_eq!(f.notified(), Vec::<i64>::new());
    }

    /// The model cannot write what the app tracks, even with a call that got past the
    /// schema (the server is the last line, not the first).
    #[tokio::test]
    async fn changed_files_and_checks_in_the_arguments_change_neither() {
        let f = fixture().await;
        // What the app recorded from real tool calls.
        f.core
            .store
            .change_task(ChatId(f.chat), |slot| {
                let mut task = Task::new(ChatId(0));
                task.state.goal = "Add a switch".into();
                task.state
                    .record_file("/work/src/Switch.tsx", FileChange::Created, 10);
                task.state.record_check(TaskCheck {
                    command: "npm test".into(),
                    passed: false,
                    at: 11,
                    summary: "1 failed".into(),
                    exit_code: Some(1),
                });
                *slot = Some(task);
            })
            .await
            .expect("seed");
        let before = f.task(f.chat).await.expect("task");

        let forged = json!({
            "changed_files": [
                {"path": "/work/src/Other.tsx", "kind": "created", "at": 99},
                {"path": "/etc/passwd", "kind": "edited", "at": 99}
            ],
            "files_not_listed": 7,
            "checks": [{"command": "npm test", "passed": true, "at": 99, "summary": "all green", "exit_code": 0}],
            "user": {"goal": true, "paused": true},
            "state": {"changed_files": [], "checks": []}
        });
        // Alone: nothing at all changes.
        let out = f.update(forged.clone()).await;
        assert!(
            out.joined_text().starts_with("Nothing changed."),
            "{}",
            out.joined_text()
        );
        assert_eq!(f.task(f.chat).await.expect("task"), before);
        assert_eq!(f.notified(), Vec::<i64>::new());

        // Together with a real field: the field is saved, the rest is still ignored.
        let mut with_goal = forged.as_object().expect("object").clone();
        with_goal.insert("next_action".into(), json!("Run the tests again"));
        let out = f.update(Value::Object(with_goal)).await;
        assert!(!out.is_error);
        let after = f.task(f.chat).await.expect("task");
        assert_eq!(after.state.next_action, "Run the tests again");
        assert_eq!(after.state.changed_files, before.state.changed_files);
        assert_eq!(after.state.files_not_listed, 0);
        assert_eq!(after.state.checks, before.state.checks);
        assert_eq!(after.state.user, before.state.user);
        // The result shows the facts as the app has them, not as the call claimed.
        let text = out.joined_text();
        assert!(
            text.contains("created \"Switch.tsx\"")
                && text.contains("failed \"npm test\" (exit code 1)"),
            "{text}"
        );
        assert!(
            !text.contains("passwd") && !text.contains("Other.tsx") && !text.contains("all green"),
            "{text}"
        );
        // And never a line of the command's output.
        assert!(!text.contains("1 failed"), "{text}");
    }

    #[tokio::test]
    async fn the_task_is_the_calling_chats_whatever_the_arguments_say() {
        let f = fixture().await;
        let out = f
            .update(json!({"goal": "Mine", "chat_id": f.other, "chatId": f.other}))
            .await;
        assert!(!out.is_error);
        assert_eq!(f.task(f.chat).await.expect("task").state.goal, "Mine");
        assert_eq!(f.task(f.other).await, None);
    }

    #[tokio::test]
    async fn a_helper_cannot_change_the_task() {
        let f = fixture().await;
        f.update(json!({"goal": "The chat's goal"})).await;
        f.notified();
        f.helper.store(true, Ordering::Relaxed);
        let out = f
            .update(json!({"goal": "The helper's own plan", "status": "done"}))
            .await;
        assert!(out.is_error);
        assert_eq!(out.joined_text(), HELPER_REFUSAL);
        let task = f.task(f.chat).await.expect("task");
        assert_eq!(
            (task.state.goal.as_str(), task.status),
            ("The chat's goal", TaskStatus::Active)
        );
        assert_eq!(f.notified(), Vec::<i64>::new());
    }

    #[tokio::test]
    async fn what_was_not_applied_is_said_in_the_result() {
        let f = fixture().await;
        f.update(
            json!({"goal": "Put the switch in the footer", "steps": [{"text": "Edit the footer"}]}),
        )
        .await;
        // The user corrects the goal and pauses, in the Task tab.
        f.core
            .store
            .change_task(ChatId(f.chat), |slot| {
                edits::apply_user(
                    slot,
                    edits::UserEdit {
                        goal: Some("Put the switch in the header".into()),
                        ..Default::default()
                    },
                    1500,
                );
                edits::set_status(slot, TaskStatus::Paused, 1500);
            })
            .await
            .expect("user edit");

        let out = f
            .update(json!({"goal": "Put the switch in the footer", "status": "done", "note": "Footer edited"}))
            .await;
        assert!(!out.is_error);
        let text = out.joined_text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[0], "Task saved.");
        assert!(lines[1].starts_with("Not changed: the goal."), "{text}");
        assert!(lines[2].starts_with("Not changed: the status."), "{text}");
        assert!(
            lines[3].starts_with("- status: paused (the user put this task on hold"),
            "{text}"
        );
        assert!(
            text.contains("- since you last saw this, the user: changed the goal"),
            "{text}"
        );
        assert!(
            text.contains("- goal (the user wrote it): \"Put the switch in the header\""),
            "{text}"
        );
        let task = f.task(f.chat).await.expect("task");
        assert_eq!(task.status, TaskStatus::Paused);
        assert_eq!(task.state.goal, "Put the switch in the header");
        assert_eq!(task.state.notes.len(), 1);
        // The result showed the user's change: it is not announced a second time, and
        // the pause still is, for the turn that follows it.
        assert!(task.state.user.changed.is_empty());
        assert!(task.state.user.paused);

        // Text that was cut to fit is said too.
        let out = f.update(json!({"next_action": "n".repeat(1000)})).await;
        let text = out.joined_text();
        assert!(
            text.lines()
                .nth(1)
                .is_some_and(|l| l.starts_with("Shortened to fit: ")),
            "{text}"
        );
    }

    #[tokio::test]
    async fn a_call_that_cannot_be_saved_says_why() {
        let f = fixture().await;
        // A chat that does not exist (it was deleted while its turn ran).
        let out = f
            .core
            .call(TOOL_NAME, &args(json!({"goal": "g"})), 4040)
            .await;
        assert!(out.is_error);
        assert!(
            out.joined_text().contains("no longer exists"),
            "{}",
            out.joined_text()
        );
        // Arguments of the wrong shape.
        let out = f.update(json!({"steps": "a, b"})).await;
        assert!(out.is_error);
        assert!(out.joined_text().contains("steps"), "{}", out.joined_text());
        // Another tool name.
        let out = f.core.call("delete_task", &Map::new(), f.chat).await;
        assert!(out.is_error);
        assert_eq!(f.task(f.chat).await, None);
        assert_eq!(f.notified(), Vec::<i64>::new());
    }
}
