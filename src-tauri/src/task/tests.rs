//! Tests of what the app records from tool calls, the note at the start of a turn, and
//! what the Task tab is sent.

use mcp_host::{BuiltinServer, CallContext, ChatContext, Content, Described};
use serde_json::json;
use store::{TaskNote, TaskStep, UserMarks};
use tokio_util::sync::CancellationToken;

use super::*;

fn action(server: &str, tool: &str, scope: policy::Scope, payload: Value) -> policy::Action {
    policy::Action {
        server: server.into(),
        tool: tool.into(),
        summary: String::new(),
        preview: String::new(),
        scope,
        risk: policy::Risk::Normal,
        read_only: false,
        outbound: false,
        keep_asking: None,
        network: false,
        hard_limit: None,
        payload,
    }
}

fn file_action(tool: &str, payload: Value) -> policy::Action {
    action(
        "files",
        tool,
        policy::Scope::FileWrite {
            folder: "/work".into(),
        },
        payload,
    )
}

fn command_action(command: &str) -> policy::Action {
    action(
        "terminal",
        "run_command",
        policy::Scope::Command {
            program: "x".into(),
            args: Vec::new(),
            cwd: "/work".into(),
        },
        json!({"cwd": "/work", "root": 0, "exists": true, "wasSymlink": false,
            "command": command, "timeoutSeconds": 120}),
    )
}

fn text(text: &str) -> ToolOutput {
    ToolOutput::text(text)
}

fn failed(text: &str) -> ToolOutput {
    ToolOutput::error(text)
}

async fn store_with_chat() -> (Store, i64) {
    let store = Store::open_in_memory().expect("open");
    let chat = store
        .create_chat(store::NewChat {
            title: "work".into(),
            ..Default::default()
        })
        .await
        .expect("chat");
    (store, chat.0)
}

// ---- Facts -----------------------------------------------------------------------------------

#[test]
fn a_file_call_that_succeeded_is_a_changed_file() {
    let path = "/work/src/a.rs";
    let payload = |extra: Value| {
        let mut p = json!({"canonical": path, "root": 0, "exists": true, "wasSymlink": false});
        p.as_object_mut()
            .expect("object")
            .extend(extra.as_object().expect("object").clone());
        p
    };
    let table: Vec<(&str, Value, FileChange)> = vec![
        // write_file: the file did not exist when the call was described.
        (
            "write_file",
            json!({"canonical": path, "root": 0, "exists": false, "wasSymlink": false, "expectedSha256": null}),
            FileChange::Created,
        ),
        (
            "write_file",
            payload(json!({"expectedSha256": "ab12"})),
            FileChange::Edited,
        ),
        // Without the hash field, whether the path existed decides.
        (
            "write_file",
            json!({"canonical": path, "root": 0, "exists": false, "wasSymlink": false}),
            FileChange::Created,
        ),
        ("write_file", payload(json!({})), FileChange::Edited),
        (
            "edit_file",
            payload(json!({"expectedSha256": "ab12"})),
            FileChange::Edited,
        ),
        (
            "delete_file",
            payload(json!({"expectedSha256": "ab12"})),
            FileChange::Deleted,
        ),
    ];
    for (tool, payload, kind) in table {
        let a = file_action(tool, payload.clone());
        // The wording of the result plays no part.
        for said in ["Wrote 12 bytes", "anything else at all", ""] {
            assert_eq!(
                fact_of(&a, &text(said), 5),
                Some(Fact::File {
                    path: path.into(),
                    kind
                }),
                "{tool} {payload}"
            );
        }
        // A call that failed changed nothing, whatever its text says.
        assert_eq!(fact_of(&a, &failed("Wrote 12 bytes"), 5), None, "{tool}");
    }
}

#[test]
fn other_calls_are_not_changed_files() {
    let payload =
        json!({"canonical": "/work/a.rs", "root": 0, "exists": true, "wasSymlink": false});
    let read = policy::Scope::FileRead {
        folder: "/work".into(),
    };
    let write = policy::Scope::FileWrite {
        folder: "/work".into(),
    };
    let external = policy::Scope::ExternalAny {
        server: "files".into(),
        tool: "files__write_file".into(),
    };
    for a in [
        // Reading is not changing.
        action("files", "read_file", read.clone(), payload.clone()),
        action("files", "list_dir", read.clone(), payload.clone()),
        action("files", "search_files", read, payload.clone()),
        // An external server's tool of the same name is not the app's file tool.
        action("files", "write_file", external.clone(), payload.clone()),
        action(
            "files",
            "files__write_file",
            external,
            json!({"kind": "external", "originalTool": "write_file", "args": "{}"}),
        ),
        action("notes", "write_file", write.clone(), payload.clone()),
        // No checked path, no fact.
        action("files", "write_file", write.clone(), Value::Null),
        action(
            "files",
            "write_file",
            write.clone(),
            json!({"canonical": 12}),
        ),
        action(
            "files",
            "write_file",
            write.clone(),
            json!({"canonical": "  "}),
        ),
        // Other tools.
        action(
            "git",
            "git_commit",
            policy::Scope::GitWrite {
                repo: "/work".into(),
            },
            payload.clone(),
        ),
        action(
            "task",
            "update_task",
            policy::Scope::AppData,
            json!({"canonical": "/work/a.rs"}),
        ),
        action(
            "artifacts",
            "create_artifact",
            policy::Scope::AppData,
            Value::Null,
        ),
    ] {
        assert_eq!(fact_of(&a, &text("ok"), 5), None, "{} {}", a.server, a.tool);
    }
}

#[test]
fn a_check_command_is_a_check_and_its_result_says_how_it_went() {
    let passed = fact_of(
        &command_action("npm test"),
        &text("Exit code 0 (3.1 s)\n\n> test\n> vitest run\n\n Tests  12 passed (12)\n"),
        77,
    );
    assert_eq!(
        passed,
        Some(Fact::Check(TaskCheck {
            command: "npm test".into(),
            passed: true,
            at: 77,
            summary: "Tests  12 passed (12)".into(),
            exit_code: Some(0),
        }))
    );
    let broke = fact_of(
        &command_action("cd app && cargo clippy -- -D warnings"),
        &failed(
            "Exit code 101 (9.0 s)\nerror: could not compile `app` (lib) due to 2 previous errors",
        ),
        78,
    );
    let Some(Fact::Check(check)) = broke else {
        panic!("not a check: {broke:?}");
    };
    assert!(!check.passed);
    assert_eq!(check.exit_code, Some(101));
    assert_eq!(check.command, "cd app && cargo clippy -- -D warnings");

    // It did not run to its end: no verdict, whatever the result's wording.
    for said in [
        "Timed out after 120 s; the command was ended",
        "Stopped by the user",
        "worded in some new way",
    ] {
        let Some(Fact::Check(check)) = fact_of(&command_action("cargo test"), &failed(said), 79)
        else {
            panic!("not a check");
        };
        assert!(!check.passed && check.exit_code.is_none(), "{said}");
    }

    // Other commands are not checks, also when they succeed or fail.
    for command in [
        "ls -la",
        "git status",
        "npm install",
        "echo \"npm test\"",
        "npm test | tail -5",
    ] {
        assert_eq!(
            fact_of(&command_action(command), &text("Exit code 0 (0.1 s)\n"), 80),
            None,
            "{command}"
        );
        assert_eq!(
            fact_of(
                &command_action(command),
                &failed("Exit code 1 (0.1 s)\n"),
                80
            ),
            None,
            "{command}"
        );
    }

    // The command line is kept as one cleaned line of bounded length.
    let long = format!("npm   test --\t{}\u{202E}", "x".repeat(1000));
    let Some(Fact::Check(check)) = fact_of(&command_action(&long), &text("Exit code 0 (1 s)"), 81)
    else {
        panic!("not a check");
    };
    assert!(
        check.command.starts_with("npm test -- xxx"),
        "{}",
        check.command
    );
    assert_eq!(check.command.chars().count(), COMMAND_CHARS);

    // Not the terminal's tool, or no command in the approved action.
    let mut other = command_action("npm test");
    other.server = "shell".into();
    assert_eq!(fact_of(&other, &text("Exit code 0 (1 s)"), 82), None);
    let mut empty = command_action("npm test");
    empty.payload = Value::Null;
    assert_eq!(fact_of(&empty, &text("Exit code 0 (1 s)"), 82), None);
}

#[tokio::test]
async fn facts_are_recorded_and_start_the_record_when_there_is_none() {
    let (store, chat) = store_with_chat().await;
    let file = |path: &str, kind| Fact::File {
        path: path.into(),
        kind,
    };
    assert!(
        record(&store, chat, file("/work/a.rs", FileChange::Created), 10)
            .await
            .expect("record")
    );
    let task = store
        .get_task(ChatId(chat))
        .await
        .expect("get")
        .expect("created");
    assert_eq!((task.status, task.updated_at), (TaskStatus::Active, 10));
    assert_eq!(task.state.changed_files.len(), 1);
    assert_eq!(task.state.goal, "");

    // Created, then edited, is still a new file; a check is listed once with its latest result.
    record(&store, chat, file("/work/a.rs", FileChange::Edited), 11)
        .await
        .expect("record");
    let check = |passed: bool, at: i64| {
        Fact::Check(TaskCheck {
            command: "npm test".into(),
            passed,
            at,
            summary: String::new(),
            exit_code: Some(i32::from(!passed)),
        })
    };
    record(&store, chat, check(false, 12), 12)
        .await
        .expect("record");
    record(&store, chat, check(true, 13), 13)
        .await
        .expect("record");
    let task = store
        .get_task(ChatId(chat))
        .await
        .expect("get")
        .expect("task");
    assert_eq!(task.state.changed_files.len(), 1);
    assert_eq!(task.state.changed_files[0].kind, FileChange::Created);
    assert_eq!(task.state.checks.len(), 1);
    assert!(task.state.checks[0].passed);
    assert_eq!(task.updated_at, 13);

    // Created and deleted again: as if nothing happened to it.
    record(&store, chat, file("/work/a.rs", FileChange::Deleted), 14)
        .await
        .expect("record");
    let task = store
        .get_task(ChatId(chat))
        .await
        .expect("get")
        .expect("task");
    assert!(task.state.changed_files.is_empty());

    // What the model and the user wrote is untouched by facts, also on a done task.
    store
        .change_task(ChatId(chat), |slot| {
            let task = slot.as_mut().expect("task");
            task.state.goal = "g".into();
            task.state.user.goal = true;
            task.status = TaskStatus::Done;
        })
        .await
        .expect("change");
    record(&store, chat, file("/work/b.rs", FileChange::Edited), 15)
        .await
        .expect("record");
    let task = store
        .get_task(ChatId(chat))
        .await
        .expect("get")
        .expect("task");
    assert_eq!(
        (task.state.goal.as_str(), task.state.user.goal, task.status),
        ("g", true, TaskStatus::Done)
    );
    assert_eq!(task.state.changed_files.len(), 1);

    // A chat that is gone: an error for the caller to drop, and nothing is left behind.
    let gone = record(&store, 4040, file("/work/a.rs", FileChange::Created), 16).await;
    assert!(matches!(gone, Err(StoreError::NotFound { .. })), "{gone:?}");
}

// ---- The real tools ----------------------------------------------------------------------------

/// A folder of its own under the system's temporary folder.
fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "harness-task-{name}-{}-{}",
        std::process::id(),
        now_ms()
    ));
    std::fs::create_dir_all(&dir).expect("create");
    dir.canonicalize().expect("canonical")
}

fn chat_in(dir: &std::path::Path) -> ChatContext {
    ChatContext {
        chat_id: 1,
        approved_folders: vec![dir.display().to_string()],
        working_folder: Some(dir.display().to_string()),
        result_cap_tokens: 2000,
        ..ChatContext::default()
    }
}

/// Describes and runs one call of a built-in server, as the turn does, and returns what
/// `after_call` would be given.
async fn run_tool(
    server: &dyn BuiltinServer,
    chat: &ChatContext,
    tool: &str,
    args: Value,
) -> (policy::Action, ToolOutput) {
    let args = args.as_object().expect("object").clone();
    let action = match server.describe(tool, &args, chat).await {
        Described::Action(action) => action,
        Described::Invalid(why) => panic!("{tool} was not described: {why}"),
    };
    let out = server
        .call(
            tool,
            &args,
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    (action, out)
}

/// The file tools' own approved actions carry what this feature reads. If their shape
/// changes, this fails, not the Task tab in the field.
#[tokio::test]
async fn the_file_tools_calls_are_read_as_changed_files() {
    let dir = scratch("files");
    let chat = chat_in(&dir);
    let files = tools::FilesServer::default();
    let (store, chat_id) = store_with_chat().await;
    std::fs::write(dir.join("old.txt"), "one\ntwo\n").expect("seed");
    std::fs::write(dir.join("gone.txt"), "bye\n").expect("seed");

    let mut at = 0;
    for (tool, args) in [
        (
            "write_file",
            json!({"path": "new.txt", "content": "hello\n"}),
        ),
        (
            "edit_file",
            json!({"path": "new.txt", "old_text": "hello", "new_text": "hello there"}),
        ),
        (
            "edit_file",
            json!({"path": "old.txt", "old_text": "two", "new_text": "2"}),
        ),
        (
            "write_file",
            json!({"path": "old.txt", "content": "all new\n"}),
        ),
        ("delete_file", json!({"path": "gone.txt"})),
        // Reading changes nothing.
        ("read_file", json!({"path": "old.txt"})),
        ("list_dir", json!({"path": "."})),
    ] {
        let (action, out) = run_tool(&files, &chat, tool, args).await;
        assert!(!out.is_error, "{tool}: {}", out.joined_text());
        at += 1;
        if let Some(fact) = fact_of(&action, &out, at) {
            record(&store, chat_id, fact, at).await.expect("record");
        }
    }
    let task = store
        .get_task(ChatId(chat_id))
        .await
        .expect("get")
        .expect("a task");
    let listed: Vec<(String, FileChange)> = task
        .state
        .changed_files
        .iter()
        .map(|f| (f.path.clone(), f.kind))
        .collect();
    let full = |name: &str| dir.join(name).display().to_string();
    assert_eq!(
        listed,
        vec![
            (full("new.txt"), FileChange::Created),
            (full("old.txt"), FileChange::Edited),
            (full("gone.txt"), FileChange::Deleted),
        ]
    );

    // A write that fails when it runs (the file changed after it was described) is not
    // recorded.
    let args = json!({"path": "old.txt", "content": "again\n"});
    let map = args.as_object().expect("object").clone();
    let Described::Action(action) = files.describe("write_file", &map, &chat).await else {
        panic!("not described");
    };
    std::fs::write(dir.join("old.txt"), "changed behind its back\n").expect("write");
    let out = files
        .call(
            "write_file",
            &map,
            &action,
            CallContext {
                chat: chat.clone(),
                cancel: CancellationToken::new(),
                progress: None,
            },
        )
        .await;
    assert!(out.is_error, "{}", out.joined_text());
    assert_eq!(fact_of(&action, &out, 99), None);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The terminal's result: an error flag, and a first line with the exit code.
#[cfg(unix)]
#[tokio::test]
async fn the_terminals_calls_are_read_as_checks() {
    let dir = scratch("terminal");
    let chat = chat_in(&dir);
    let terminal = tools::TerminalServer::new(tools::TerminalConfig::default());
    std::fs::write(dir.join("run-tests.sh"), "echo \"3 passed\"\nexit 0\n").expect("script");
    std::fs::write(dir.join("lint.sh"), "echo \"2 problems\" >&2\nexit 3\n").expect("script");

    let (action, out) = run_tool(
        &terminal,
        &chat,
        "run_command",
        json!({"command": "sh ./run-tests.sh"}),
    )
    .await;
    let Some(Fact::Check(check)) = fact_of(&action, &out, 1) else {
        panic!("not a check: {}", out.joined_text());
    };
    assert_eq!(
        (
            check.command.as_str(),
            check.passed,
            check.exit_code,
            check.summary.as_str()
        ),
        ("sh ./run-tests.sh", true, Some(0), "3 passed")
    );

    let (action, out) = run_tool(
        &terminal,
        &chat,
        "run_command",
        json!({"command": "sh ./lint.sh"}),
    )
    .await;
    let Some(Fact::Check(check)) = fact_of(&action, &out, 2) else {
        panic!("not a check: {}", out.joined_text());
    };
    assert_eq!((check.passed, check.exit_code), (false, Some(3)));
    assert_eq!(check.summary, "2 problems");

    // A check that is cut off has no verdict.
    std::fs::write(dir.join("build.sh"), "sleep 30\n").expect("script");
    let (action, out) = run_tool(
        &terminal,
        &chat,
        "run_command",
        json!({"command": "sh ./build.sh", "timeout_seconds": 1}),
    )
    .await;
    let Some(Fact::Check(check)) = fact_of(&action, &out, 3) else {
        panic!("not a check: {}", out.joined_text());
    };
    assert_eq!((check.passed, check.exit_code), (false, None));

    // A command that is not a check is not recorded.
    let (action, out) = run_tool(&terminal, &chat, "run_command", json!({"command": "ls"})).await;
    assert!(!out.is_error);
    assert_eq!(fact_of(&action, &out, 4), None);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- The start of a turn ------------------------------------------------------------------------

fn written_task() -> Task {
    let mut task = Task::new(ChatId(0));
    task.state.goal = "Put the switch in the header".into();
    task.state.steps = vec![
        TaskStep {
            text: "Read the settings page".into(),
            status: StepStatus::Done,
        },
        TaskStep {
            text: "Add the switch".into(),
            status: StepStatus::Doing,
        },
    ];
    task.state.next_action = "Add the switch".into();
    task.state
        .record_file("/work/src/Settings.tsx", FileChange::Edited, 10);
    task.updated_at = 10;
    task
}

#[tokio::test]
async fn a_turn_starts_with_the_task_note_and_records_that_it_was_sent() {
    let (store, chat) = store_with_chat().await;
    // No task: no note, and nothing is created.
    assert_eq!(note_in(&store, chat).await, None);
    assert_eq!(start_turn(&store, chat).await, (None, false));
    assert_eq!(store.get_task(ChatId(chat)).await.expect("get"), None);

    // The user paused the task and corrected the goal.
    let mut task = written_task();
    task.status = TaskStatus::Paused;
    task.state.user = UserMarks {
        goal: true,
        changed: vec!["goal".into()],
        paused: true,
        ..UserMarks::default()
    };
    store
        .change_task(ChatId(chat), move |slot| *slot = Some(task))
        .await
        .expect("seed");

    // What the Context inspector would show is what the turn gets.
    let preview = note_in(&store, chat).await.expect("a note");
    let stored_before = store.get_task(ChatId(chat)).await.expect("get");
    assert_eq!(
        stored_before.as_ref().map(|t| t.status),
        Some(TaskStatus::Paused),
        "reading the note changes nothing"
    );
    let (note, stored) = start_turn(&store, chat).await;
    assert_eq!(note.as_deref(), Some(preview.as_str()));
    assert!(stored);
    assert!(preview.starts_with(note::HEADER));
    assert!(preview.contains("- status: active"), "{preview}");
    assert!(preview.contains("- since you last saw this, the user: paused this task and has now resumed it; changed the goal"), "{preview}");
    assert!(
        preview.contains("- goal (the user wrote it): \"Put the switch in the header\""),
        "{preview}"
    );
    assert!(
        preview.contains("- changed files (in \"/work/src\"): edited \"Settings.tsx\""),
        "{preview}"
    );

    // The turn is the work going on: the task is active, the marks are spent, and the
    // time of the last change did not move.
    let task = store
        .get_task(ChatId(chat))
        .await
        .expect("get")
        .expect("task");
    assert_eq!(task.status, TaskStatus::Active);
    assert!(task.state.user.changed.is_empty() && !task.state.user.paused);
    assert!(task.state.user.goal);
    assert_eq!(task.updated_at, 10);

    // The next turn gets the state without the news, and writes nothing.
    let (note, stored) = start_turn(&store, chat).await;
    let note = note.expect("a note");
    assert!(!stored);
    assert!(!note.contains("since you last saw this"), "{note}");
    assert!(
        note.contains("- goal (the user wrote it): \"Put the switch in the header\""),
        "{note}"
    );
    assert!(note::cost(&format!("[App note] {note}")) <= note::NOTE_TOKENS);

    // A done task sends nothing; one the user marked done says so once.
    user_status(&store, chat, TaskStatus::Done, 20)
        .await
        .expect("done");
    let (note, stored) = start_turn(&store, chat).await;
    assert!(stored);
    assert!(
        note.expect("a note")
            .contains("- status: done (the user marked this task done)")
    );
    assert_eq!(start_turn(&store, chat).await, (None, false));
    assert_eq!(note_in(&store, chat).await, None);
}

#[tokio::test]
async fn a_record_of_facts_only_sends_no_note() {
    let (store, chat) = store_with_chat().await;
    record(
        &store,
        chat,
        Fact::File {
            path: "/work/a.rs".into(),
            kind: FileChange::Edited,
        },
        5,
    )
    .await
    .expect("record");
    assert_eq!(note_in(&store, chat).await, None);
    assert_eq!(start_turn(&store, chat).await, (None, false));
    // Once the user writes a goal it is a task to tell the model about.
    user_edit(
        &store,
        chat,
        edits::UserEdit {
            goal: Some("Tidy up".into()),
            ..Default::default()
        },
        6,
    )
    .await
    .expect("edit");
    let note = note_in(&store, chat).await.expect("a note");
    assert!(
        note.contains("- goal (the user wrote it): \"Tidy up\""),
        "{note}"
    );
    assert!(note.contains("edited \"a.rs\""), "{note}");
}

/// A saved state that cannot be read is `StoreError::Json` from the store (its own test,
/// `a_state_with_other_fields_still_loads`, writes one and deletes it again). The app
/// crate cannot write such a row, so the error is built here the way the store builds it.
#[test]
fn a_task_that_cannot_be_read_sends_no_note_and_can_be_cleared() {
    let unreadable = || {
        StoreError::Json(
            serde_json::from_str::<store::TaskState>(r#"{"goal": 12}"#).expect_err("wrong shape"),
        )
    };
    // No note, and the turn start stores nothing.
    assert_eq!(note_from(Err(unreadable())), None);
    assert_eq!(turn_start_from(Err(unreadable())), (None, false));
    // The tab is told what happened and what to do: Clear task, which deletes the row
    // without reading it (`Store::delete_task`).
    let e = store_error(unreadable());
    assert_eq!(e.code, "task_unreadable");
}

// ---- The Task tab ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_users_commands_change_the_stored_task() {
    let (store, chat) = store_with_chat().await;
    // Nothing to pause, and an empty edit starts nothing.
    assert_eq!(
        user_status(&store, chat, TaskStatus::Paused, 1)
            .await
            .expect("pause"),
        (None, false)
    );
    assert_eq!(
        user_edit(&store, chat, edits::UserEdit::default(), 1)
            .await
            .expect("edit"),
        (None, false)
    );

    let (task, stored) = user_edit(
        &store,
        chat,
        edits::UserEdit {
            goal: Some("Tidy the notes folder".into()),
            steps: Some(vec![edits::StepInput {
                text: "List the notes".into(),
                status: StepStatus::Todo,
            }]),
            ..Default::default()
        },
        2,
    )
    .await
    .expect("edit");
    assert!(stored);
    let task = task.expect("created");
    assert_eq!(
        (task.chat_id, task.status, task.updated_at),
        (ChatId(chat), TaskStatus::Active, 2)
    );

    let (task, stored) = user_status(&store, chat, TaskStatus::Paused, 3)
        .await
        .expect("pause");
    assert!(stored);
    assert_eq!(task.expect("task").status, TaskStatus::Paused);
    let (_, stored) = user_status(&store, chat, TaskStatus::Paused, 4)
        .await
        .expect("pause again");
    assert!(!stored);
    let (task, _) = user_status(&store, chat, TaskStatus::Active, 5)
        .await
        .expect("resume");
    assert_eq!(task.expect("task").status, TaskStatus::Active);

    // A chat that does not exist.
    let gone = user_edit(
        &store,
        4040,
        edits::UserEdit {
            goal: Some("x".into()),
            ..Default::default()
        },
        6,
    )
    .await;
    assert_eq!(store_error(gone.expect_err("no chat")).code, "not_found");
}

#[test]
fn the_tab_gets_the_task_with_who_wrote_what_and_the_newest_first() {
    let mut task = written_task();
    task.chat_id = ChatId(7);
    task.status = TaskStatus::Paused;
    task.state.constraints = vec!["No new dependencies".into(), "Keep the tests green".into()];
    task.state.user = UserMarks {
        goal: true,
        next_action: false,
        constraints: vec!["Keep the tests green".into()],
        steps: vec!["Add the switch".into()],
        ..UserMarks::default()
    };
    task.state
        .record_file("/work/src/evil\u{202E}txt.exe", FileChange::Created, 12);
    task.state.files_not_listed = 3;
    task.state.record_check(TaskCheck {
        command: "npm test".into(),
        passed: true,
        at: 13,
        summary: "12 passed".into(),
        exit_code: Some(0),
    });
    task.state.record_check(TaskCheck {
        command: "cargo build".into(),
        passed: false,
        at: 14,
        summary: "Timed out".into(),
        exit_code: None,
    });
    task.state.notes = vec![
        TaskNote {
            text: "first".into(),
            at: 1,
        },
        TaskNote {
            text: "second".into(),
            at: 2,
        },
    ];
    let dto = TaskDto::from(task);
    assert_eq!(
        (dto.chat_id, dto.status, dto.updated_at),
        (7, TaskStatus::Paused, 10)
    );
    assert!(dto.goal_by_user && !dto.next_action_by_user);
    assert_eq!(
        dto.constraints,
        vec![
            ItemDto {
                text: "No new dependencies".into(),
                by_user: false
            },
            ItemDto {
                text: "Keep the tests green".into(),
                by_user: true
            },
        ]
    );
    assert_eq!(
        dto.steps
            .iter()
            .map(|s| (s.text.as_str(), s.status, s.by_user))
            .collect::<Vec<_>>(),
        vec![
            ("Read the settings page", StepStatus::Done, false),
            ("Add the switch", StepStatus::Doing, true)
        ]
    );
    // Newest first; the path to copy is the real one, the one shown hides nothing.
    assert_eq!(dto.changed_files.len(), 2);
    assert_eq!(dto.changed_files[0].path, "/work/src/evil\u{202E}txt.exe");
    assert_eq!(dto.changed_files[0].shown, "/work/src/evil\\u{202E}txt.exe");
    assert_eq!(dto.changed_files[0].kind, FileChange::Created);
    assert_eq!(dto.changed_files[1].path, "/work/src/Settings.tsx");
    assert_eq!(dto.files_not_listed, 3);
    assert_eq!(
        dto.checks
            .iter()
            .map(|c| (c.command.as_str(), c.passed, c.finished, c.exit_code))
            .collect::<Vec<_>>(),
        vec![
            ("cargo build", false, false, None),
            ("npm test", true, true, Some(0))
        ]
    );
    assert_eq!(dto.checks[1].summary, "12 passed");
    assert_eq!(
        dto.notes
            .iter()
            .map(|n| n.text.as_str())
            .collect::<Vec<_>>(),
        ["second", "first"]
    );

    // As the window receives it.
    let sent = serde_json::to_value(&dto).expect("json");
    assert_eq!(sent["chatId"], 7);
    assert_eq!(sent["status"], "paused");
    assert_eq!(sent["goalByUser"], true);
    assert_eq!(
        sent["steps"][1],
        json!({"text": "Add the switch", "status": "doing", "byUser": true})
    );
    assert_eq!(sent["changedFiles"][1]["kind"], "edited");
    assert_eq!(sent["checks"][0]["exitCode"], Value::Null);
    assert_eq!(sent["filesNotListed"], 3);
    assert_eq!(sent["nextAction"], "Add the switch");
}

#[test]
fn only_the_main_window_edits_the_task() {
    assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
    for other in [crate::OVERLAY_WINDOW, "indicator", ""] {
        assert!(only_main_window(other).is_err(), "{other:?}");
    }
}

/// The edit the window sends: camelCase, and nothing but the four fields.
#[test]
fn the_windows_edit_has_four_fields_and_no_others() {
    let edit: edits::UserEdit = serde_json::from_value(json!({
        "goal": "g",
        "constraints": ["c"],
        "steps": [{"text": "s", "status": "doing"}, {"text": "t"}],
        "nextAction": "n"
    }))
    .expect("an edit");
    assert_eq!(edit.goal.as_deref(), Some("g"));
    assert_eq!(edit.next_action.as_deref(), Some("n"));
    assert_eq!(edit.steps.as_ref().map(Vec::len), Some(2));
    let partial: edits::UserEdit =
        serde_json::from_value(json!({"nextAction": "n"})).expect("an edit");
    assert_eq!((partial.goal, partial.steps), (None, None));
    for forged in [
        json!({"changedFiles": []}),
        json!({"checks": []}),
        json!({"status": "done"}),
        json!({"user": {"goal": true}}),
        json!({"goal": "g", "changed_files": [{"path": "/x", "kind": "edited", "at": 1}]}),
    ] {
        assert!(
            serde_json::from_value::<edits::UserEdit>(forged.clone()).is_err(),
            "{forged}"
        );
    }
}

/// From the model's first call to the turn that resumes the work.
#[tokio::test]
async fn the_task_is_written_tracked_corrected_and_resumed() {
    let (store, chat) = store_with_chat().await;
    let core = server::Core {
        store: store.clone(),
        notify: std::sync::Arc::new(|_| {}),
        helper_at_work: std::sync::Arc::new(|_| false),
        now: std::sync::Arc::new(|| 100),
    };
    let args = json!({
        "goal": "Put the switch in the footer",
        "steps": [{"text": "Read the settings page", "status": "doing"}, {"text": "Add the switch"}],
        "next_action": "Read the settings page"
    });
    let out = core
        .call(server::TOOL_NAME, args.as_object().expect("object"), chat)
        .await;
    assert!(!out.is_error, "{}", out.joined_text());

    // The app records what the tools did.
    let wrote = file_action(
        "write_file",
        json!({"canonical": "/work/src/Switch.tsx", "root": 0, "exists": false, "wasSymlink": false, "expectedSha256": null}),
    );
    record(
        &store,
        chat,
        fact_of(&wrote, &text("ok"), 110).expect("a fact"),
        110,
    )
    .await
    .expect("record");
    let ran = command_action("npm test");
    let out = ToolOutput {
        content: vec![Content::Text {
            text: "Exit code 1 (2.0 s)\nFAIL src/Switch.test.tsx\n1 failed".into(),
        }],
        is_error: true,
        full_text: None,
        ui: None,
    };
    record(&store, chat, fact_of(&ran, &out, 120).expect("a fact"), 120)
        .await
        .expect("record");

    // The first turn after that gets it all. (Turns start here; the note is not stored.)
    let (note, _) = start_turn(&store, chat).await;
    let note = note.expect("a note");
    assert!(note.contains("created \"Switch.tsx\""), "{note}");
    assert!(note.contains("failed \"npm test\" (exit code 1)"), "{note}");
    assert!(
        !note.contains("FAIL src") && !note.contains("1 failed"),
        "{note}"
    );

    // The user pauses, corrects the goal, and resumes.
    user_status(&store, chat, TaskStatus::Paused, 130)
        .await
        .expect("pause");
    user_edit(
        &store,
        chat,
        edits::UserEdit {
            goal: Some("Put the switch in the header".into()),
            ..Default::default()
        },
        140,
    )
    .await
    .expect("edit");
    user_status(&store, chat, TaskStatus::Active, 150)
        .await
        .expect("resume");
    let (note, stored) = start_turn(&store, chat).await;
    let note = note.expect("a note");
    assert!(stored);
    assert!(note.contains("- status: active"), "{note}");
    assert!(
        note.contains("the user: paused this task and has now resumed it; changed the goal"),
        "{note}"
    );
    assert!(
        note.contains("- goal (the user wrote it): \"Put the switch in the header\""),
        "{note}"
    );
    assert!(
        note.contains("- next action: \"Read the settings page\""),
        "{note}"
    );
    assert!(note::cost(&format!("[App note] {note}")) <= note::NOTE_TOKENS);
}
