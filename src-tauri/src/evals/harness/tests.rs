//! The harness on the scripted endpoint: what a run records, what it refuses, and that it
//! stops when asked.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;
use crate::evals::testing::{Endpoint, Say, audit_rows, call, model, spec, text, tool_results};

fn tool_names(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn a_solved_run_records_its_calls_tokens_and_audit_rows() {
    let ws = Workspace::create().unwrap();
    ws.write_files([("notes.txt", b"one\ntwo\n".as_slice())])
        .unwrap();
    let endpoint = Endpoint::playing(vec![
        call("read_file", json!({"path": "notes.txt"})),
        call(
            "edit_file",
            json!({"path": "notes.txt", "old_text": "two", "new_text": "three"}),
        ),
        call(
            "write_file",
            json!({"path": "new.txt", "content": "made\n"}),
        ),
        text("I changed two to three and made new.txt."),
    ])
    .await;
    let (audit, rows) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("change two to three", false),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.status, "completed", "{record:?}");
    assert_eq!(ws.read("notes.txt").as_deref(), Some("one\nthree\n"));
    assert_eq!(ws.read("new.txt").as_deref(), Some("made\n"));
    assert_eq!(record.calls.len(), 3);
    assert!(record.calls.iter().all(|c| c.status == "ok" && !c.blocked));
    assert_eq!(record.arrival(), Arrival::Native);
    assert_eq!(record.steps, 4);
    assert_eq!(record.prompt_tokens, 4 * 500);
    assert_eq!(record.completion_tokens, 4 * 10);
    assert_eq!(record.answer, "I changed two to three and made new.txt.");
    assert_eq!(
        (
            record.failed_edits(),
            record.tool_failures(),
            record.blocked()
        ),
        (0, 0, 0)
    );
    // One audit row per call, decided by the rule, belonging to no chat.
    let rows = rows.lock().unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().all(|r| r.chat_id.is_none()));
    assert!(rows.iter().all(|r| r.decided_by == "policy"));
    assert_eq!(
        rows.iter().map(|r| r.tool.as_str()).collect::<Vec<_>>(),
        ["read_file", "edit_file", "write_file"]
    );
    // The model was offered the file tools and nothing else: no delete, no terminal.
    let offered = tool_names(&endpoint.requests()[0]);
    assert!(offered.contains(&"edit_file".to_owned()), "{offered:?}");
    assert!(
        !offered
            .iter()
            .any(|t| t == "delete_file" || t == "run_command"),
        "{offered:?}"
    );
}

#[tokio::test]
async fn refused_calls_are_counted_as_blocked_and_the_run_goes_on() {
    let ws = Workspace::create().unwrap();
    let outside = Workspace::create_private().unwrap();
    outside
        .write_files([("secret.txt", b"keep\n".as_slice())])
        .unwrap();
    ws.write_files([("notes.txt", b"one\n".as_slice())])
        .unwrap();
    let secret = outside.path().join("secret.txt").display().to_string();
    let endpoint = Endpoint::playing(vec![
        call("write_file", json!({"path": secret, "content": "gone"})),
        call("delete_file", json!({"path": "notes.txt"})),
        call("write_file", json!({"path": "notes.txt", "content": ""})),
        call("run_command", json!({"command": "ls"})),
        text("Done."),
    ])
    .await;
    let (audit, rows) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("try things", false),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.status, "completed", "{record:?}");
    assert_eq!(outside.read("secret.txt").as_deref(), Some("keep\n"));
    assert_eq!(ws.read("notes.txt").as_deref(), Some("one\n"));
    let statuses: Vec<(&str, bool)> = record
        .calls
        .iter()
        .map(|c| (c.tool.as_str(), c.blocked))
        .collect();
    // `delete_file` and `run_command` are not offered in this run: the agent loop refuses
    // them as unknown tools before any describer or tool sees them (an invalid call, not
    // a blocked one). Were one to reach the gate, the rule refuses it too (sandbox tests).
    assert_eq!(
        statuses,
        [
            ("write_file", true),
            ("delete_file", false),
            ("write_file", true),
            ("run_command", false)
        ]
    );
    assert_eq!(record.calls[1].status, "error");
    assert_eq!(record.blocked(), 2);
    // The model was told why, and the reply went on.
    let results = tool_results(endpoint.requests().last().unwrap());
    assert!(results.len() >= 3, "{results:?}");
    assert_eq!(rows.lock().unwrap().len(), 4);
    assert!(
        rows.lock().unwrap()[..3]
            .iter()
            .all(|r| r.decision == store::AuditDecision::RefusedByPolicy)
    );
}

#[tokio::test]
async fn a_failed_edit_is_counted_from_its_status() {
    let ws = Workspace::create().unwrap();
    ws.write_files([("a.txt", b"x = 1\nx = 1\n".as_slice())])
        .unwrap();
    let endpoint = Endpoint::playing(vec![
        call(
            "edit_file",
            json!({"path": "a.txt", "old_text": "not there", "new_text": "y"}),
        ),
        call(
            "edit_file",
            json!({"path": "a.txt", "old_text": "x = 1", "new_text": "x = 2"}),
        ),
        call("read_file", json!({"path": "missing.txt"})),
        text("Done."),
    ])
    .await;
    let (audit, _) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("edit", false),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.failed_edits(), 2, "{:?}", record.calls);
    assert_eq!(record.tool_failures(), 3, "{:?}", record.calls);
    assert_eq!(record.blocked(), 0);
    assert_eq!(ws.read("a.txt").as_deref(), Some("x = 1\nx = 1\n"));
}

#[tokio::test]
async fn calls_read_from_text_are_recorded_as_such() {
    let ws = Workspace::create().unwrap();
    ws.write_files([("n.txt", b"heron\n".as_slice())]).unwrap();
    let endpoint = Endpoint::playing(vec![
        Say::TextCall("read_file".into(), json!({"path": "n.txt"})),
        text("heron"),
    ])
    .await;
    let (audit, _) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("read", false),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.calls.len(), 1, "{record:?}");
    assert!(record.calls[0].recovered);
    assert_eq!(record.calls[0].status, "ok");
    assert_eq!(record.arrival(), Arrival::Text);
}

#[tokio::test]
async fn stop_ends_a_run_within_two_seconds() {
    let ws = Workspace::create().unwrap();
    let endpoint = Endpoint::playing(vec![Say::Slow]).await;
    let cancel = CancellationToken::new();
    let stopper = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        stopper.cancel();
    });
    let (audit, _) = audit_rows();
    let started = Instant::now();
    let record = run(&model(&endpoint), spec("go", false), &ws, cancel, audit).await;
    assert_eq!(record.status, "stopped", "{record:?}");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn the_time_limit_stops_a_run() {
    let ws = Workspace::create().unwrap();
    let endpoint = Endpoint::playing(vec![Say::Slow]).await;
    let (audit, _) = audit_rows();
    let mut spec = spec("go", false);
    spec.time_limit = Duration::from_millis(400);
    let started = Instant::now();
    let record = run(
        &model(&endpoint),
        spec,
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.status, "timeLimit", "{record:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn a_failed_request_ends_the_run_as_failed() {
    let ws = Workspace::create().unwrap();
    let endpoint =
        Endpoint::playing(vec![Say::Status(500), Say::Status(500), Say::Status(500)]).await;
    let (audit, _) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("go", false),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(record.status, "failed", "{record:?}");
    assert!(record.failure.is_some());
}

#[tokio::test]
async fn the_settings_reach_the_request() {
    let ws = Workspace::create().unwrap();
    let endpoint = Endpoint::playing(vec![text("ok")]).await;
    let (audit, _) = audit_rows();
    let mut spec = spec("go", false);
    spec.settings.temperature = Some(0.2);
    run(
        &model(&endpoint),
        spec,
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    let request = &endpoint.requests()[0];
    assert!(
        (request["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6,
        "{request}"
    );
}

/// Benchmarks have the terminal: plain commands run in the run's folder, and commands the
/// rule refuses do not.
#[cfg(unix)]
#[tokio::test]
async fn a_benchmark_run_has_the_terminal_under_the_rule() {
    if runtime::resolve_program("python3", None).is_none() {
        eprintln!("python3 is missing; skipped");
        return;
    }
    let ws = Workspace::create().unwrap();
    ws.write_files([(
        "hello.py",
        b"open('out.txt', 'w').write('hello from the run\\n')\n".as_slice(),
    )])
    .unwrap();
    let endpoint = Endpoint::playing(vec![
        call("run_command", json!({"command": "python3 hello.py"})),
        call("run_command", json!({"command": "rm hello.py"})),
        call(
            "run_command",
            json!({"command": "curl https://example.com"}),
        ),
        text("Done."),
    ])
    .await;
    let (audit, _) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("run it", true),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    assert_eq!(
        ws.read("out.txt").as_deref(),
        Some("hello from the run\n"),
        "{record:?}"
    );
    assert!(ws.read("hello.py").is_some());
    // The model is told up front what the terminal runs.
    let system = endpoint.requests()[0]["messages"][0]["content"].to_string();
    assert!(
        system.contains(crate::evals::sandbox::COMMANDS_ALLOWED),
        "{system}"
    );
    let blocked: Vec<bool> = record.calls.iter().map(|c| c.blocked).collect();
    assert_eq!(blocked, [false, true, true], "{:?}", record.calls);
    let offered = tool_names(&endpoint.requests()[0]);
    assert!(offered.contains(&"run_command".to_owned()), "{offered:?}");
    assert!(!offered.contains(&"delete_file".to_owned()), "{offered:?}");
}

fn rec(tool: &str, status: &str, blocked: bool, text: &str) -> CallRec {
    CallRec {
        tool: tool.into(),
        status: status.into(),
        blocked,
        recovered: false,
        detail: String::new(),
        time_ms: 0,
        exit_code: (tool == "run_command" && !blocked)
            .then(|| exit_code_of(text))
            .flatten(),
    }
}

/// A command that ran and exited with a failure is a failed command, not a tool failure.
/// A command that could not run or did not finish, and every other failed call, is.
#[test]
fn failed_commands_are_told_from_tool_failures() {
    let cases = [
        // (call, failed command, tool failure)
        (
            rec(
                "run_command",
                "error",
                false,
                "Exit code 1 (0.2 s)\nFAILED (failures=1)",
            ),
            true,
            false,
        ),
        (
            rec("run_command", "error", false, "Exit code 2 (0.0 s)\nusage"),
            true,
            false,
        ),
        (
            rec("run_command", "ok", false, "Exit code 0 (0.1 s)\nOK"),
            false,
            false,
        ),
        (
            rec(
                "run_command",
                "error",
                false,
                "Timed out after 600 s; the command was ended\n...",
            ),
            false,
            true,
        ),
        (
            rec(
                "run_command",
                "error",
                false,
                "The command was ended by a signal\n",
            ),
            false,
            true,
        ),
        (
            rec("run_command", "error", false, "Stopped by the user\n"),
            false,
            true,
        ),
        (rec("run_command", "refused", true, ""), false, false),
        (
            rec(
                "run_command",
                "error",
                false,
                "Exit code 127 (0.0 s)\nsh: 1: zzq: not found",
            ),
            false,
            true,
        ),
        (
            rec(
                "run_command",
                "error",
                false,
                "Exit code 126 (0.0 s)\npermission denied",
            ),
            false,
            true,
        ),
        (
            rec(
                "run_command",
                "error",
                false,
                "the working folder is not allowed",
            ),
            false,
            true,
        ),
        (
            rec("edit_file", "error", false, "Exit code 1 (0.0 s)"),
            false,
            true,
        ),
        (
            rec("write_file", "error", false, "old_text was not found"),
            false,
            true,
        ),
    ];
    for (call, failed_command, tool_failure) in cases {
        assert_eq!(call.failed_command(), failed_command, "{call:?}");
        assert_eq!(call.failed(), tool_failure, "{call:?}");
    }
    assert_eq!(exit_code_of("Exit code 1 (0.2 s)\nmore"), Some(1));
    assert_eq!(exit_code_of("Exit code -1 (0.2 s)"), Some(-1));
    assert_eq!(exit_code_of("Exit code 1"), None);
    assert_eq!(exit_code_of("exit code 1 (0.2 s)"), None);
    assert_eq!(exit_code_of("output\nExit code 1 (0.2 s)"), None);
    let record = Record {
        calls: cases_calls(),
        ..Record::default()
    };
    assert_eq!((record.failed_commands(), record.tool_failures()), (1, 2));
}

fn cases_calls() -> Vec<CallRec> {
    vec![
        rec("run_command", "error", false, "Exit code 1 (0.2 s)"),
        rec(
            "run_command",
            "error",
            false,
            "Timed out after 600 s; the command was ended",
        ),
        rec("edit_file", "error", false, "x"),
        rec("run_command", "ok", false, "Exit code 0 (0.1 s)"),
    ]
}

/// Through the real terminal: failing tests are a failed command; a program that is not
/// there is a tool failure.
#[cfg(unix)]
#[tokio::test]
async fn a_failing_test_run_is_a_failed_command_not_a_tool_failure() {
    if runtime::resolve_program("python3", None).is_none() {
        eprintln!("python3 is missing; skipped");
        return;
    }
    let ws = Workspace::create().unwrap();
    ws.write_files([
        (
            "fails.py",
            b"import sys\nprint('1 test failed')\nsys.exit(1)\n".as_slice(),
        ),
        ("passes.py", b"print('ok')\n".as_slice()),
    ])
    .unwrap();
    let endpoint = Endpoint::playing(vec![
        call("run_command", json!({"command": "python3 fails.py"})),
        call("run_command", json!({"command": "python3 passes.py"})),
        call("run_command", json!({"command": "python3 missing.py"})),
        text("Done."),
    ])
    .await;
    let (audit, _) = audit_rows();
    let record = run(
        &model(&endpoint),
        spec("run them", true),
        &ws,
        CancellationToken::new(),
        audit,
    )
    .await;
    let codes: Vec<Option<i32>> = record.calls.iter().map(|c| c.exit_code).collect();
    assert_eq!(codes, [Some(1), Some(0), Some(2)], "{:#?}", record.calls);
    // A missing script is Python's own exit code 2: the command ran and failed.
    assert_eq!(record.failed_commands(), 2, "{:#?}", record.calls);
    assert_eq!(record.tool_failures(), 0, "{:#?}", record.calls);
}
