//! The task record of a chat (app 0.0.4): the goal, constraints, steps, changed files,
//! completed checks and next action of the work a chat is doing, kept so the work can be
//! shown, paused, corrected and picked up again after an interruption or a restart.
//!
//! - One record per chat (`tasks`), deleted with the chat. `status` and `updated_at` are
//!   columns; everything else is the JSON `state`.
//! - Every change goes through [`Store::change_task`], which reads the record, lets the
//!   caller alter it and stores the result in one step on the database thread. A tool call
//!   and the user's edit arriving together therefore both take effect; neither overwrites
//!   the other.
//! - Changed files and checks are kept as facts with fixed rules ([`TaskState::record_file`],
//!   [`TaskState::record_check`]): one entry per file or command, the newest last, and
//!   never more than [`MAX_CHANGED_FILES`] and [`MAX_CHECKS`].
//! - What the user wrote is marked apart from what the model wrote ([`UserMarks`]).

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::chats::ensure_chat;
use crate::error::StoreError;
use crate::ids::{ChatId, sql_enum};

/// Changed files listed per task (the most recently changed).
pub const MAX_CHANGED_FILES: usize = 200;

/// Checks kept per task (the most recently run), one per command.
pub const MAX_CHECKS: usize = 30;

sql_enum!(
    /// Where a chat's task stands.
    TaskStatus, "task status" {
        /// Being worked on. Also after a turn that ended early: nothing is lost.
        Active => "active",
        /// Put on hold by the user.
        Paused => "paused",
        /// Finished.
        Done => "done",
    }
);

/// How far one step of the plan is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StepStatus {
    /// Not started.
    #[default]
    Todo,
    /// Being worked on.
    Doing,
    /// Finished.
    Done,
}

/// One step of the plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStep {
    /// What the step is.
    pub text: String,
    /// How far it is.
    #[serde(default)]
    pub status: StepStatus,
}

/// How a file differs from when the task started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileChange {
    /// It did not exist and now does.
    Created,
    /// It existed and its content changed.
    Edited,
    /// It existed and is gone.
    Deleted,
}

/// A file the work changed. Recorded by the app from tool results, never by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangedFile {
    /// The file's full path.
    pub path: String,
    /// How it differs from when the task started.
    pub kind: FileChange,
    /// When it last changed (Unix ms).
    pub at: i64,
}

/// A test, build, lint or type check that ran to its end. Recorded by the app from the
/// command's result, never by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskCheck {
    /// The command line as it was run.
    pub command: String,
    /// It ended with success.
    pub passed: bool,
    /// When it ended (Unix ms).
    pub at: i64,
    /// One line about the result, for the user. It comes from the command's output, so it
    /// is never sent to the model outside a tool result.
    #[serde(default)]
    pub summary: String,
    /// The exit code, when the command reported one.
    #[serde(default)]
    pub exit_code: Option<i32>,
}

/// A short remark the model attached to the task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskNote {
    /// The remark.
    pub text: String,
    /// When it was added (Unix ms).
    pub at: i64,
}

/// What the user wrote or did in a task, kept apart from what the model wrote. Only the
/// user's own actions in the app set these; nothing a model outputs can.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserMarks {
    /// The goal as it stands is the user's wording.
    pub goal: bool,
    /// The next action as it stands is the user's wording.
    pub next_action: bool,
    /// The constraints (by their text) the user wrote.
    pub constraints: Vec<String>,
    /// The steps (by their text) the user wrote.
    pub steps: Vec<String>,
    /// What the user changed since the model was last sent the task state: "goal",
    /// "constraints", "steps", "next action".
    pub changed: Vec<String>,
    /// The user paused the task since the model was last sent the task state.
    pub paused: bool,
}

/// The JSON `state` of a task. Every field has a default, so a state written by an older
/// version still loads.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TaskState {
    /// What the work should achieve.
    pub goal: String,
    /// Rules the work must keep to.
    pub constraints: Vec<String>,
    /// The plan, in order.
    pub steps: Vec<TaskStep>,
    /// Files the work changed, the most recently changed last.
    pub changed_files: Vec<ChangedFile>,
    /// Changed files no longer listed because the list is full.
    pub files_not_listed: u32,
    /// Checks that ran, the most recent last, one per command.
    pub checks: Vec<TaskCheck>,
    /// The very next thing to do.
    pub next_action: String,
    /// Remarks the model attached, oldest first.
    pub notes: Vec<TaskNote>,
    /// What the user wrote or did.
    pub user: UserMarks,
}

impl TaskState {
    /// Notes that a file was created, edited or deleted. There is one entry per path, and
    /// it says how the file differs from when the task started: a file created and then
    /// edited is still new, a file created and then deleted is not listed at all, and a
    /// deleted file written again counts as edited. The entry moves to the end of the
    /// list; beyond [`MAX_CHANGED_FILES`] the oldest entry is dropped and counted in
    /// [`TaskState::files_not_listed`].
    pub fn record_file(&mut self, path: &str, kind: FileChange, at: i64) {
        use FileChange::{Created, Deleted, Edited};
        if path.trim().is_empty() {
            return;
        }
        let before = self
            .changed_files
            .iter()
            .position(|f| f.path == path)
            .map(|i| self.changed_files.remove(i).kind);
        let now = match (before, kind) {
            // New in this task and gone again: as if nothing had happened to it.
            (Some(Created), Deleted) => return,
            (Some(Created), _) => Created,
            // It existed when the task started, so writing it again is a change to it.
            (Some(Edited | Deleted), Created | Edited) => Edited,
            (Some(Edited | Deleted), Deleted) => Deleted,
            (None, kind) => kind,
        };
        self.changed_files.push(ChangedFile {
            path: path.to_owned(),
            kind: now,
            at,
        });
        while self.changed_files.len() > MAX_CHANGED_FILES {
            self.changed_files.remove(0);
            self.files_not_listed = self.files_not_listed.saturating_add(1);
        }
    }

    /// Notes the result of a check. A command that ran before keeps only its latest
    /// result, at the end of the list; beyond [`MAX_CHECKS`] the oldest is dropped.
    pub fn record_check(&mut self, check: TaskCheck) {
        if check.command.trim().is_empty() {
            return;
        }
        self.checks.retain(|c| c.command != check.command);
        self.checks.push(check);
        while self.checks.len() > MAX_CHECKS {
            self.checks.remove(0);
        }
    }
}

/// A chat's task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// Where it stands.
    pub status: TaskStatus,
    /// When its content last changed (Unix ms). Set by whoever changes it.
    pub updated_at: i64,
    /// Everything else.
    pub state: TaskState,
}

impl Task {
    /// An empty, active task for a chat.
    pub fn new(chat_id: ChatId) -> Self {
        Self {
            chat_id,
            status: TaskStatus::Active,
            updated_at: 0,
            state: TaskState::default(),
        }
    }
}

fn load_task(conn: &Connection, chat_id: ChatId) -> Result<Option<Task>, StoreError> {
    let row: Option<(String, TaskStatus, i64)> = conn
        .query_row(
            "SELECT state, status, updated_at FROM tasks WHERE chat_id = ?1",
            [chat_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((state, status, updated_at)) = row else {
        return Ok(None);
    };
    Ok(Some(Task {
        chat_id,
        status,
        updated_at,
        state: serde_json::from_str(&state)?,
    }))
}

fn save_task(conn: &Connection, task: &Task) -> Result<(), StoreError> {
    ensure_chat(conn, task.chat_id)?;
    conn.execute(
        "INSERT INTO tasks (chat_id, state, status, updated_at) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(chat_id) DO UPDATE SET state = excluded.state, status = excluded.status, \
         updated_at = excluded.updated_at",
        params![
            task.chat_id,
            serde_json::to_string(&task.state)?,
            task.status,
            task.updated_at
        ],
    )?;
    Ok(())
}

fn delete_task(conn: &Connection, chat_id: ChatId) -> Result<bool, StoreError> {
    Ok(conn.execute("DELETE FROM tasks WHERE chat_id = ?1", [chat_id])? > 0)
}

impl Store {
    /// The chat's task, or `None` when it has none.
    pub async fn get_task(&self, chat_id: ChatId) -> Result<Option<Task>, StoreError> {
        self.call(move |c| load_task(c, chat_id)).await
    }

    /// Reads the chat's task, lets `change` alter it and stores the result, in one step
    /// on the database thread. `change` gets `None` when the chat has no task; it may
    /// set a task to create one, alter the one there, or set `None` to delete it.
    /// Nothing is written when it leaves the task as it was.
    ///
    /// Returns the task as it now stands, and what `change` returned. A task is always
    /// stored under `chat_id`, whatever its own `chat_id` field says. Creating a task for
    /// a chat that does not exist fails with [`StoreError::NotFound`].
    pub async fn change_task<R, F>(
        &self,
        chat_id: ChatId,
        change: F,
    ) -> Result<(Option<Task>, R), StoreError>
    where
        R: Send + 'static,
        F: FnOnce(&mut Option<Task>) -> R + Send + 'static,
    {
        self.call(move |c| {
            let tx = c.transaction()?;
            let before = load_task(&tx, chat_id)?;
            let mut after = before.clone();
            let result = change(&mut after);
            if let Some(task) = &mut after {
                task.chat_id = chat_id;
            }
            if after != before {
                match &after {
                    Some(task) => save_task(&tx, task)?,
                    None => {
                        delete_task(&tx, chat_id)?;
                    }
                }
            }
            tx.commit()?;
            Ok((after, result))
        })
        .await
    }

    /// Deletes the chat's task; returns whether it had one. Works even when the stored
    /// state can no longer be read.
    pub async fn delete_task(&self, chat_id: ChatId) -> Result<bool, StoreError> {
        self.call(move |c| delete_task(c, chat_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::NewChat;

    async fn chat(store: &Store, title: &str) -> ChatId {
        store
            .create_chat(NewChat {
                title: title.into(),
                ..NewChat::default()
            })
            .await
            .expect("create chat")
    }

    /// A task with every field filled in.
    fn full_task(chat_id: ChatId) -> Task {
        let mut state = TaskState {
            goal: "Add a dark mode switch".into(),
            constraints: vec!["No new dependencies".into(), "Keep the tests green".into()],
            steps: vec![
                TaskStep {
                    text: "Read the settings page".into(),
                    status: StepStatus::Done,
                },
                TaskStep {
                    text: "Add the switch".into(),
                    status: StepStatus::Doing,
                },
                TaskStep {
                    text: "Run the tests".into(),
                    status: StepStatus::Todo,
                },
            ],
            next_action: "Wire the switch into the theme store".into(),
            notes: vec![TaskNote {
                text: "The theme lives in src/lib/theme.ts".into(),
                at: 5,
            }],
            user: UserMarks {
                goal: true,
                next_action: false,
                constraints: vec!["Keep the tests green".into()],
                steps: vec!["Run the tests".into()],
                changed: vec!["goal".into(), "steps".into()],
                paused: true,
            },
            ..TaskState::default()
        };
        state.record_file("/work/src/Switch.tsx", FileChange::Created, 10);
        state.record_file("/work/src/Settings.tsx", FileChange::Edited, 11);
        state.record_file("/work/old.txt", FileChange::Deleted, 12);
        state.record_check(TaskCheck {
            command: "npm test".into(),
            passed: true,
            at: 13,
            summary: "exit code 0 · 12 passed".into(),
            exit_code: Some(0),
        });
        state.record_check(TaskCheck {
            command: "npm run lint".into(),
            passed: false,
            at: 14,
            summary: "exit code 1 · 2 problems".into(),
            exit_code: Some(1),
        });
        Task {
            chat_id,
            status: TaskStatus::Paused,
            updated_at: 1_700_000_000_000,
            state,
        }
    }

    /// Resumable for real: what was saved is what a new run of the app reads, field for
    /// field, from the database file.
    #[tokio::test]
    async fn a_task_survives_closing_and_reopening_the_database() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("app.db");
        let store = Store::open(&path).await.expect("open");
        let id = chat(&store, "work").await;
        let task = full_task(id);
        let wanted = task.clone();
        let (stored, ()) = store
            .change_task(id, move |slot| *slot = Some(task))
            .await
            .expect("save");
        assert_eq!(stored.as_ref(), Some(&wanted));
        assert_eq!(wanted.state.changed_files.len(), 3);
        assert_eq!(wanted.state.checks.len(), 2);
        drop(store);

        let store = Store::open(&path).await.expect("open again");
        let read = store.get_task(id).await.expect("get").expect("a task");
        assert_eq!(read, wanted);
        // The columns hold the status and the time; the rest is the JSON state.
        let (status, updated, state): (String, i64, String) = store
            .call(move |c| {
                Ok(c.query_row(
                    "SELECT status, updated_at, state FROM tasks WHERE chat_id = ?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?)
            })
            .await
            .expect("row");
        assert_eq!((status.as_str(), updated), ("paused", 1_700_000_000_000));
        let json: serde_json::Value = serde_json::from_str(&state).expect("json");
        assert_eq!(json["goal"], "Add a dark mode switch");
        assert_eq!(json["steps"][1]["status"], "doing");
        assert_eq!(json["changed_files"][0]["kind"], "created");
        assert_eq!(json["checks"][1]["passed"], false);
        assert_eq!(json["next_action"], "Wire the switch into the theme store");
        assert!(json.get("status").is_none() && json.get("updated_at").is_none());
    }

    #[tokio::test]
    async fn a_change_creates_alters_and_deletes_and_writes_only_when_something_changed() {
        let store = Store::open_in_memory().expect("open");
        let id = chat(&store, "c").await;
        assert_eq!(store.get_task(id).await.expect("get"), None);

        // Looking without changing writes nothing and creates nothing.
        let (task, seen) = store
            .change_task(id, |slot| slot.is_some())
            .await
            .expect("look");
        assert_eq!((task, seen), (None, false));
        assert_eq!(store.get_task(id).await.expect("get"), None);

        // Setting a task creates it. It is stored under the chat asked for, whatever the
        // task itself says.
        let (task, ()) = store
            .change_task(id, |slot| {
                let mut task = Task::new(ChatId(999));
                task.state.goal = "first".into();
                task.updated_at = 5;
                *slot = Some(task);
            })
            .await
            .expect("create");
        let task = task.expect("created");
        assert_eq!((task.chat_id, task.updated_at), (id, 5));
        assert_eq!(task.status, TaskStatus::Active);
        assert_eq!(store.get_task(id).await.expect("get"), Some(task.clone()));

        // Altering it in place, and getting a value back from the change.
        let (task, goal_before) = store
            .change_task(id, |slot| {
                let task = slot.as_mut().expect("exists");
                let before = std::mem::replace(&mut task.state.goal, "second".into());
                task.status = TaskStatus::Done;
                task.updated_at = 6;
                before
            })
            .await
            .expect("alter");
        assert_eq!(goal_before, "first");
        let task = task.expect("still there");
        assert_eq!(
            (task.state.goal.as_str(), task.status, task.updated_at),
            ("second", TaskStatus::Done, 6)
        );
        assert_eq!(store.get_task(id).await.expect("get"), Some(task));

        // Setting none deletes it; deleting again finds nothing.
        let (task, ()) = store
            .change_task(id, |slot| *slot = None)
            .await
            .expect("delete");
        assert_eq!(task, None);
        assert_eq!(store.get_task(id).await.expect("get"), None);
        assert!(!store.delete_task(id).await.expect("delete again"));
    }

    /// A tool call and the user's edit arriving together both take effect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn changes_made_at_the_same_time_are_all_kept() {
        let store = Store::open_in_memory().expect("open");
        let id = chat(&store, "c").await;
        let mut work = Vec::new();
        for n in 0..40_i64 {
            let store = store.clone();
            work.push(tokio::spawn(async move {
                store
                    .change_task(id, move |slot| {
                        let task = slot.get_or_insert_with(|| Task::new(id));
                        task.state.notes.push(TaskNote {
                            text: format!("note {n}"),
                            at: n,
                        });
                    })
                    .await
                    .expect("change");
            }));
        }
        for w in work {
            w.await.expect("join");
        }
        let task = store.get_task(id).await.expect("get").expect("a task");
        let mut seen: Vec<i64> = task.state.notes.iter().map(|n| n.at).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..40).collect::<Vec<_>>());
    }

    #[tokio::test]
    async fn a_task_belongs_to_one_chat_and_is_deleted_with_it() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store, "a").await;
        let b = chat(&store, "b").await;
        for (id, goal) in [(a, "goal a"), (b, "goal b")] {
            store
                .change_task(id, move |slot| {
                    let mut task = Task::new(id);
                    task.state.goal = goal.into();
                    *slot = Some(task);
                })
                .await
                .expect("create");
        }
        let goal = |t: Option<Task>| t.map(|t| t.state.goal);
        assert_eq!(
            goal(store.get_task(a).await.expect("get")).as_deref(),
            Some("goal a")
        );
        assert_eq!(
            goal(store.get_task(b).await.expect("get")).as_deref(),
            Some("goal b")
        );

        assert!(store.delete_task(a).await.expect("delete"));
        assert_eq!(store.get_task(a).await.expect("get"), None);
        assert!(store.get_task(b).await.expect("get").is_some());

        // Deleting the chat deletes its task.
        store.delete_chat(b).await.expect("delete chat");
        assert_eq!(store.get_task(b).await.expect("get"), None);
        let rows: i64 = store
            .call(|c| Ok(c.query_row("SELECT count(*) FROM tasks", [], |r| r.get(0))?))
            .await
            .expect("count");
        assert_eq!(rows, 0);

        // No task for a chat that does not exist, and nothing is left behind.
        let missing = ChatId(4040);
        assert_eq!(store.get_task(missing).await.expect("get"), None);
        let created = store
            .change_task(missing, move |slot| *slot = Some(Task::new(missing)))
            .await;
        assert!(
            matches!(created, Err(StoreError::NotFound { what: "chat", .. })),
            "{created:?}"
        );
        assert_eq!(store.get_task(missing).await.expect("get"), None);
    }

    /// A state written by an earlier build (fewer fields) or a later one (more fields)
    /// still loads; one that is not a task state at all is an error, and can be deleted.
    #[tokio::test]
    async fn a_state_with_other_fields_still_loads() {
        let store = Store::open_in_memory().expect("open");
        let id = chat(&store, "c").await;
        let write = |state: &'static str| {
            let store = store.clone();
            async move {
                store
                    .call(move |c| {
                        c.execute(
                            "INSERT OR REPLACE INTO tasks (chat_id, state, status, updated_at) \
                             VALUES (?1, ?2, 'active', 7)",
                            params![id, state],
                        )?;
                        Ok(())
                    })
                    .await
                    .expect("write");
            }
        };
        write(r#"{"goal": "only a goal", "steps": [{"text": "one"}], "from_the_future": [1, 2]}"#)
            .await;
        let task = store.get_task(id).await.expect("get").expect("a task");
        assert_eq!(task.state.goal, "only a goal");
        assert_eq!(
            task.state.steps,
            vec![TaskStep {
                text: "one".into(),
                status: StepStatus::Todo
            }]
        );
        assert_eq!(task.state.user, UserMarks::default());
        assert_eq!((task.status, task.updated_at), (TaskStatus::Active, 7));

        write(r#"{"goal": 12}"#).await;
        assert!(matches!(
            store.get_task(id).await,
            Err(StoreError::Json(_))
        ));
        assert!(matches!(
            store.change_task(id, |_| ()).await,
            Err(StoreError::Json(_))
        ));
        assert!(store.delete_task(id).await.expect("delete"));
        assert_eq!(store.get_task(id).await.expect("get"), None);
    }

    fn kinds(state: &TaskState) -> Vec<(&str, FileChange)> {
        state
            .changed_files
            .iter()
            .map(|f| (f.path.as_str(), f.kind))
            .collect()
    }

    /// One entry per file, saying how it differs from when the task started.
    #[test]
    fn changed_files_say_how_each_file_differs_from_the_start() {
        use FileChange::{Created, Deleted, Edited};
        // (what happened to one file, in order) → how it is listed (None: not at all).
        let table: &[(&[FileChange], Option<FileChange>)] = &[
            (&[Created], Some(Created)),
            (&[Edited], Some(Edited)),
            (&[Deleted], Some(Deleted)),
            (&[Created, Edited], Some(Created)),
            (&[Created, Edited, Edited], Some(Created)),
            (&[Created, Created], Some(Created)),
            (&[Edited, Edited], Some(Edited)),
            (&[Edited, Created], Some(Edited)),
            (&[Created, Deleted], None),
            (&[Created, Edited, Deleted], None),
            (&[Edited, Deleted], Some(Deleted)),
            (&[Deleted, Deleted], Some(Deleted)),
            (&[Deleted, Created], Some(Edited)),
            (&[Deleted, Edited], Some(Edited)),
            (&[Deleted, Created, Deleted], Some(Deleted)),
            (&[Created, Deleted, Created], Some(Created)),
            (&[Created, Deleted, Edited], Some(Edited)),
        ];
        for (events, listed) in table {
            let mut state = TaskState::default();
            for (i, kind) in events.iter().enumerate() {
                state.record_file("/w/a.txt", *kind, i as i64);
            }
            let got = state.changed_files.first().map(|f| f.kind);
            assert_eq!(got, *listed, "{events:?}");
            assert!(state.changed_files.len() <= 1, "{events:?}");
            if let Some(file) = state.changed_files.first() {
                assert_eq!(file.at, events.len() as i64 - 1, "{events:?}");
            }
        }
    }

    #[test]
    fn changed_files_are_listed_once_with_the_latest_change_last() {
        let mut state = TaskState::default();
        state.record_file("/w/a.txt", FileChange::Created, 1);
        state.record_file("/w/b.txt", FileChange::Edited, 2);
        state.record_file("/w/c.txt", FileChange::Deleted, 3);
        state.record_file("/w/a.txt", FileChange::Edited, 4);
        assert_eq!(
            kinds(&state),
            vec![
                ("/w/b.txt", FileChange::Edited),
                ("/w/c.txt", FileChange::Deleted),
                ("/w/a.txt", FileChange::Created),
            ]
        );
        assert_eq!(state.changed_files[2].at, 4);
        // Paths are compared exactly: these are two files.
        state.record_file("/w/A.txt", FileChange::Edited, 5);
        assert_eq!(state.changed_files.len(), 4);
        // An empty path is not a file.
        state.record_file("", FileChange::Edited, 6);
        state.record_file("   ", FileChange::Edited, 6);
        assert_eq!(state.changed_files.len(), 4);
        assert_eq!(state.files_not_listed, 0);
    }

    #[test]
    fn changed_files_are_capped_and_the_rest_is_counted() {
        let mut state = TaskState::default();
        for i in 0..MAX_CHANGED_FILES + 5 {
            state.record_file(&format!("/w/f{i}.txt"), FileChange::Edited, i as i64);
        }
        assert_eq!(state.changed_files.len(), MAX_CHANGED_FILES);
        assert_eq!(state.files_not_listed, 5);
        // The oldest were dropped; the newest is last.
        assert_eq!(state.changed_files[0].path, "/w/f5.txt");
        assert_eq!(
            state.changed_files[MAX_CHANGED_FILES - 1].path,
            format!("/w/f{}.txt", MAX_CHANGED_FILES + 4)
        );
        // A file already listed does not push another one out.
        state.record_file("/w/f5.txt", FileChange::Edited, 999);
        assert_eq!(state.changed_files.len(), MAX_CHANGED_FILES);
        assert_eq!(state.files_not_listed, 5);
        assert_eq!(state.changed_files[MAX_CHANGED_FILES - 1].path, "/w/f5.txt");
    }

    #[test]
    fn a_check_run_again_keeps_only_its_latest_result() {
        let check = |command: &str, passed: bool, at: i64| TaskCheck {
            command: command.into(),
            passed,
            at,
            summary: format!("run at {at}"),
            exit_code: Some(i32::from(!passed)),
        };
        let mut state = TaskState::default();
        state.record_check(check("cargo test", false, 1));
        state.record_check(check("npm run lint", true, 2));
        state.record_check(check("cargo test", true, 3));
        let got: Vec<(&str, bool, i64)> = state
            .checks
            .iter()
            .map(|c| (c.command.as_str(), c.passed, c.at))
            .collect();
        assert_eq!(
            got,
            vec![("npm run lint", true, 2), ("cargo test", true, 3)]
        );
        assert_eq!(state.checks[1].summary, "run at 3");
        // A check without a command is not a check.
        state.record_check(check("  ", true, 4));
        assert_eq!(state.checks.len(), 2);

        for i in 0..MAX_CHECKS + 3 {
            state.record_check(check(&format!("make test{i}"), true, 10 + i as i64));
        }
        assert_eq!(state.checks.len(), MAX_CHECKS);
        assert_eq!(state.checks[0].command, "make test3");
        assert_eq!(
            state.checks[MAX_CHECKS - 1].command,
            format!("make test{}", MAX_CHECKS + 2)
        );
    }

    #[test]
    fn statuses_are_stored_as_plain_words() {
        for (status, text) in [
            (TaskStatus::Active, "active"),
            (TaskStatus::Paused, "paused"),
            (TaskStatus::Done, "done"),
        ] {
            assert_eq!(status.as_str(), text);
            assert_eq!(
                serde_json::to_value(status).expect("json"),
                serde_json::json!(text)
            );
        }
        assert_eq!(
            serde_json::to_value(StepStatus::Doing).expect("json"),
            serde_json::json!("doing")
        );
        assert_eq!(
            serde_json::to_value(FileChange::Deleted).expect("json"),
            serde_json::json!("deleted")
        );
        assert_eq!(Task::new(ChatId(3)).status, TaskStatus::Active);
    }
}
