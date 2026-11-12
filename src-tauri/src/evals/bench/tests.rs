//! Every benchmark task's verification, proven on the scripted endpoint: the task passes
//! when the model solves it and fails when it does not.

use serde_json::{Value, json};

use super::*;
use crate::evals::tasks::{self, Toolchain};
use crate::evals::testing::{Endpoint, Say, audit_rows, model, text, tool_results};

/// The command the task's prompt asks the model to run, as the scripted model would.
fn command_in(prompt: &str, toolchains: &Toolchains, needs: Toolchain) -> Option<String> {
    let program = toolchains.command(needs)?;
    prompt
        .split('`')
        .skip(1)
        .step_by(2)
        .find(|part| part.starts_with(&format!("{program} ")))
        .map(str::to_owned)
}

/// A scripted model that writes `files` (all in one reply), then runs the task's own
/// command when it has one, then says it is done.
fn player(files: Vec<(String, String)>, command: Option<String>) -> impl Fn(&Value) -> Say {
    move |request| match tool_results(request).len() {
        0 if !files.is_empty() => Say::Calls(
            files
                .iter()
                .map(|(path, content)| {
                    (
                        "write_file".to_owned(),
                        json!({"path": path, "content": content}),
                    )
                })
                .collect(),
        ),
        n if n == files.len() && command.is_some() => Say::Calls(vec![(
            "run_command".to_owned(),
            json!({"command": command.clone().unwrap()}),
        )]),
        _ => text("Done."),
    }
}

async fn play(task: &Task, files: Vec<(String, String)>, toolchains: &Toolchains) -> TaskResult {
    let command = task
        .spec
        .needs
        .and_then(|needs| command_in(&prompt_for(task, toolchains), toolchains, needs));
    let endpoint = Endpoint::start(player(files, command)).await;
    let model = model(&endpoint);
    let (audit, _) = audit_rows();
    let setup = Setup {
        model: &model,
        settings: Settings::default(),
        toolchains,
        terminal: tools::TerminalConfig::default(),
        forbidden: &[],
        audit,
    };
    run_task(&setup, task, &CancellationToken::new()).await
}

fn as_text(files: Vec<(String, &'static [u8])>) -> Vec<(String, String)> {
    files
        .into_iter()
        .map(|(p, c)| (p, String::from_utf8(c.to_vec()).unwrap()))
        .collect()
}

/// Every task: solved, it passes; left as it was, it fails; answered wrongly (where a
/// plausible wrong answer is shipped), it fails. A task whose toolchain is missing on
/// this computer is reported as skipped.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_task_passes_when_solved_and_fails_when_not() {
    let toolchains = Toolchains::detect(None).await;
    for task in tasks::all() {
        let solution = as_text(tasks::answer(&task.name, "solution").unwrap());
        if let Some(needs) = task.spec.needs
            && toolchains.get(needs).is_none()
        {
            let result = play(&task, solution, &toolchains).await;
            assert_eq!(result.status, "skipped", "{}", task.name);
            eprintln!("{} skipped: {} is missing", task.name, needs.name());
            continue;
        }
        let solved = play(&task, solution, &toolchains).await;
        assert_eq!(solved.status, "passed", "{} solved: {solved:#?}", task.name);
        assert_eq!(solved.broken_edits, 0, "{}", task.name);
        assert_eq!(solved.blocked, 0, "{} solved: {:#?}", task.name, solved);
        // The task's own command ran under the rule (it was not blocked).
        if task.spec.needs.is_some() {
            assert!(solved.tool_failures <= 1, "{}: {solved:#?}", task.name);
        }

        let untouched = play(&task, Vec::new(), &toolchains).await;
        assert_eq!(
            untouched.status, "failed",
            "{} untouched: {untouched:#?}",
            task.name
        );
        assert!(untouched.checks.iter().any(|c| !c.passed), "{}", task.name);

        if let Some(wrong) = tasks::answer(&task.name, "wrong") {
            let wrong = play(&task, as_text(wrong), &toolchains).await;
            assert_eq!(wrong.status, "failed", "{} wrong: {wrong:#?}", task.name);
        }
    }
}

/// Code that prints the success line itself does not pass: it cannot know the nonce.
#[tokio::test]
async fn printing_the_success_line_does_not_pass() {
    let toolchains = Toolchains::detect(None).await;
    let task = tasks::all()
        .into_iter()
        .find(|t| t.name == "py-fix-failing-test")
        .unwrap();
    if toolchains.get(Toolchain::Python).is_none() {
        eprintln!("python is missing; skipped");
        return;
    }
    let cheat = "import sys\nprint('VERIFIED ' + (sys.argv[1:] or ['x'])[0])\nprint('VERIFIED 0123456789abcdef0123456789abcdef')\nsys.stdout.flush()\nimport os\nos._exit(0)\n";
    let result = play(&task, vec![("stats.py".into(), cheat.into())], &toolchains).await;
    assert_eq!(result.status, "failed", "{result:#?}");
}

/// A file left not parsing counts as a broken edit.
#[tokio::test]
async fn a_file_left_not_parsing_is_a_broken_edit() {
    let toolchains = Toolchains::detect(None).await;
    let task = tasks::all()
        .into_iter()
        .find(|t| t.name == "config-json-settings")
        .unwrap();
    let broken = "{\n  \"server\": {\"port\": 8080,\n";
    let result = play(
        &task,
        vec![("settings.json".into(), broken.into())],
        &toolchains,
    )
    .await;
    assert_eq!(result.status, "failed");
    assert_eq!(result.unparsable, ["settings.json"]);
    assert_eq!(result.broken_edits, 1);
}

#[tokio::test]
async fn a_task_without_its_toolchain_is_skipped() {
    let none = Toolchains::default();
    let task = tasks::all()
        .into_iter()
        .find(|t| t.spec.needs == Some(Toolchain::Node))
        .unwrap();
    let endpoint = Endpoint::playing(vec![]).await;
    let model = model(&endpoint);
    let (audit, _) = audit_rows();
    let setup = Setup {
        model: &model,
        settings: Settings::default(),
        toolchains: &none,
        terminal: tools::TerminalConfig::default(),
        forbidden: &[],
        audit,
    };
    let result = run_task(&setup, &task, &CancellationToken::new()).await;
    assert_eq!(result.status, "skipped");
    assert!(result.note.contains("Node.js"), "{}", result.note);
    assert!(endpoint.requests().is_empty(), "the model was asked anyway");
}

#[tokio::test]
async fn a_cancelled_benchmark_runs_no_more_tasks() {
    let toolchains = Toolchains::default();
    let endpoint = Endpoint::playing(vec![Say::Slow]).await;
    let model = model(&endpoint);
    let (audit, _) = audit_rows();
    let setup = Setup {
        model: &model,
        settings: Settings::default(),
        toolchains: &toolchains,
        terminal: tools::TerminalConfig::default(),
        forbidden: &[],
        audit,
    };
    let docs: Vec<Task> = tasks::all()
        .into_iter()
        .filter(|t| t.spec.needs.is_none())
        .collect();
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        stopper.cancel();
    });
    let started = std::time::Instant::now();
    let results = run_all(&setup, &docs, &cancel, Arc::new(|_, _, _| {})).await;
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].status, "stopped");
}

#[test]
fn totals_add_up() {
    let task = tasks::all().into_iter().next().unwrap();
    let mut a = TaskResult::skipped(&task, "x".into());
    a.status = "passed".into();
    a.steps = 3;
    a.prompt_tokens = 100;
    a.completion_tokens = 5;
    a.broken_edits = 1;
    let mut b = a.clone();
    b.status = "failed".into();
    b.tool_failures = 2;
    b.blocked = 1;
    b.failed_commands = 3;
    let c = TaskResult::skipped(&task, "x".into());
    let t = totals(&[a, b, c]);
    assert_eq!(
        t,
        Totals {
            tasks: 3,
            ran: 2,
            completed: 1,
            skipped: 1,
            broken_edits: 2,
            tool_failures: 2,
            failed_commands: 3,
            blocked: 1,
            steps: 6,
            tokens: 210,
            time_ms: 0,
        }
    );
}

#[test]
fn runs_are_saved_listed_and_loaded_by_safe_ids_only() {
    let dir = crate::evals::testing::TempFolder::new("bench-save");
    let id = new_id();
    assert!(valid_id(&id), "{id}");
    for bad in [
        "",
        "../x",
        "20260101-000000-zzzz",
        "20260101-000000-ABCD",
        "a/b",
        "20260101-000000-abcd/",
    ] {
        assert!(!valid_id(bad), "{bad}");
        assert!(load(dir.path(), bad).is_none());
    }
    let run = |id: &str, completed: u32| BenchRun {
        id: id.into(),
        started_at: 1,
        finished_at: 2,
        model: "fake-model".into(),
        model_ref: "remote:1:fake-model".into(),
        settings: Settings::default(),
        toolchains: vec!["Python".into()],
        cancelled: false,
        tasks: Vec::new(),
        totals: Totals {
            completed,
            ..Totals::default()
        },
    };
    save(dir.path(), &run("20260101-000000-aaaa", 1)).unwrap();
    save(dir.path(), &run("20260102-000000-bbbb", 2)).unwrap();
    assert!(save(dir.path(), &run("../evil", 0)).is_err());
    let listed = list(dir.path());
    assert_eq!(
        listed.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        ["20260102-000000-bbbb", "20260101-000000-aaaa"]
    );
    assert_eq!(
        load(dir.path(), "20260101-000000-aaaa")
            .unwrap()
            .totals
            .completed,
        1
    );
    for i in 0..(KEEP_RUNS + 3) {
        save(dir.path(), &run(&format!("20270101-{i:06}-cccc"), 0)).unwrap();
    }
    assert_eq!(list(dir.path()).len(), KEEP_RUNS);
    assert!(
        load(dir.path(), "20260101-000000-aaaa").is_none(),
        "the oldest run was kept"
    );
}

/// The review's cheats (finding 2): code that patches the assertions, prints the success
/// line, or patches Node's assert does not pass, because the app compares values.
#[tokio::test]
async fn tampering_with_the_checking_process_does_not_pass() {
    let toolchains = Toolchains::detect(None).await;
    let all = tasks::all();
    let task = |name: &str| all.iter().find(|t| t.name == name).unwrap();
    let fixture =
        |t: &Task, file: &str| String::from_utf8(t.fixture(file).unwrap().to_vec()).unwrap();
    let mut cases = Vec::new();
    if toolchains.get(Toolchain::Python).is_some() {
        let t = task("py-fix-failing-test");
        let buggy = fixture(t, "stats.py");
        cases.push((
            t,
            "stats.py",
            format!(
                "{buggy}\nimport unittest as _u; _u.TestCase.assertEqual = lambda *a, **k: None\n"
            ),
        ));
        cases.push((t, "stats.py", format!(
            "{buggy}\nimport __main__, os, sys\nprint('VERIFIED ' + str(getattr(__main__, 'NONCE', 'x')))\nprint('RESULT ' + '{{}}')\nsys.stdout.flush()\nos._exit(0)\n"
        )));
    }
    if toolchains.get(Toolchain::Node).is_some() {
        let t = task("js-fix-failing-test");
        let buggy = fixture(t, "slugify.js");
        cases.push((
            t,
            "slugify.js",
            format!("{buggy}\nrequire(\"node:assert\").strictEqual = () => {{}};\n"),
        ));
    }
    for (t, file, content) in cases {
        let result = play(t, vec![(file.to_owned(), content)], &toolchains).await;
        assert_eq!(result.status, "failed", "{}: {result:#?}", t.name);
    }
}

/// Each script check is written to the audit log: the fixed command and its result.
#[tokio::test]
async fn every_script_check_gets_an_audit_row() {
    let toolchains = Toolchains::detect(None).await;
    if toolchains.get(Toolchain::Python).is_none() {
        eprintln!("python is missing; skipped");
        return;
    }
    let task = tasks::all()
        .into_iter()
        .find(|t| t.name == "py-fix-failing-test")
        .unwrap();
    let endpoint = Endpoint::playing(vec![]).await;
    let model = model(&endpoint);
    let (audit, rows) = audit_rows();
    let setup = Setup {
        model: &model,
        settings: Settings::default(),
        toolchains: &toolchains,
        terminal: tools::TerminalConfig::default(),
        forbidden: &[],
        audit,
    };
    let result = run_task(&setup, &task, &CancellationToken::new()).await;
    assert_eq!(result.status, "failed");
    let rows = rows.lock().unwrap();
    let verify: Vec<_> = rows.iter().filter(|r| r.tool == "verify").collect();
    assert_eq!(verify.len(), 1, "{rows:#?}");
    assert_eq!(verify[0].server, "evals");
    assert!(
        verify[0].preview.contains("verify.py"),
        "{}",
        verify[0].preview
    );
    assert_eq!(verify[0].result.as_ref().unwrap().status, "failed");
    assert!(verify[0].chat_id.is_none());
}

/// A run saved before failed commands were counted still loads, with 0 for them.
#[test]
fn a_run_saved_before_failed_commands_loads() {
    let dir = crate::evals::testing::TempFolder::new("bench-old");
    let old = r#"{"id":"20261112-100000-abcd","startedAt":1,"finishedAt":2,"model":"m","modelRef":"remote:1:m",
      "settings":{"thinkingOff":false,"temperature":null},"toolchains":[],"cancelled":false,
      "tasks":[{"name":"t","title":"T","kind":"python","status":"failed","note":"","checks":[],"failedEdits":0,
        "unparsable":[],"brokenEdits":0,"toolFailures":1,"blocked":0,"steps":2,"promptTokens":1,"completionTokens":1,
        "timeMs":5,"arrival":"native","runStatus":"completed"}],
      "totals":{"tasks":1,"ran":1,"completed":0,"skipped":0,"brokenEdits":0,"toolFailures":1,"blocked":0,"steps":2,"tokens":2,"timeMs":5}}"#;
    std::fs::write(dir.path().join("20261112-100000-abcd.json"), old).unwrap();
    let run = load(dir.path(), "20261112-100000-abcd").expect("an old run loads");
    assert_eq!(run.tasks[0].failed_commands, 0);
    assert_eq!(run.totals.failed_commands, 0);
    assert_eq!(list(dir.path()).len(), 1);
    let saved = serde_json::to_value(&run).unwrap();
    assert_eq!(saved["tasks"][0]["failedCommands"], 0);
    assert_eq!(saved["totals"]["failedCommands"], 0);
}
