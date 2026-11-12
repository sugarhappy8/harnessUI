//! End-to-end tests of the app's own managed server ([`mcp_host::ManagedServer`], rea's
//! client; item5-spec.md WP9b), and the fake server itself.
//!
//! This test target has `harness = false`, so this file's `main` runs for it. Started with
//! [`FAKE_FLAG`] it *is* a small MCP server written by hand on JSON lines, not on rmcp's
//! server side: rmcp (like rea's own SDK) drops the answer to a cancelled call, and the
//! "answers the cancel" case needs a server that answers anyway. It appends every line it
//! receives to the file named by `FAKE_REA_RECORD`, so a test can show that a refused call
//! never reached it. Started any other way (by `cargo test`) it starts copies of itself
//! through `ManagedServer` and checks:
//!
//! 1. the child's environment is exactly the given one (a canary set in this process does
//!    not leak), `PATH` is the fixed one, and the working folder and arguments are exact;
//! 2. a paged tool list is read, calls work, stderr reaches the tail and the named log file;
//! 3. a name outside the allow-list is refused and the server receives nothing;
//! 4. progress keeps a call alive past its limit, and the server's progress text is never
//!    returned or passed on;
//! 5. a cancelled call that the server answers within the grace returns within 1 s (3 s on
//!    Windows) and the server keeps running; one it ignores stops the server after the
//!    grace, and the next start is a new process; a timeout is handled the same way;
//! 6. `stop` lets a server that exits when its standard input closes end by itself, and
//!    kills one that ignores it after the grace;
//! 7. the start limit is kept; a crash after start is noticed.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mcp_host::{
    CANCEL_GRACE, CallOutcome, HostError, Lifecycle, ManagedServer, ManagedSpec, ProgressFn,
    STOP_AFTER_CANCEL, StopOutcome, ToolAllowList,
};
use runtime::EnvSpec;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

/// Hidden flag that turns this binary into the fake server; the next argument is the mode:
/// `normal`, `answers-cancel`, `ignores-cancel`, `lingers` (ignores its standard input
/// closing), `stubborn` (both), `no-handshake`.
const FAKE_FLAG: &str = "--mcp-host-test-fake-managed";

/// Set in this process only; must never reach the server.
const CANARY: &str = "MCP_HOST_MANAGED_TEST_CANARY";

/// The fixed `PATH` the tests give the server (no folder on it exists).
const FIXED_PATH: &str = if cfg!(windows) {
    r"C:\harnessui-fixed\bin"
} else {
    "/harnessui-fixed/bin"
};

/// Every name the fake lists, over two pages.
const FAKE_TOOLS: [&str; 9] = [
    "echo",
    "env",
    "cwd",
    "args",
    "pid",
    "slow",
    "progress",
    "crash",
    "start_gdb_session",
];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(FAKE_FLAG) {
        fake::run(args.get(2).map(String::as_str).unwrap_or("normal"));
        return;
    }
    if args.iter().any(|a| a == "--list") {
        println!("managed_e2e: test");
        return;
    }
    // SAFETY: no other thread exists yet; the runtime is built after this.
    unsafe { std::env::set_var(CANARY, "leaked-from-parent") };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        run(
            "environment_arguments_and_folder_are_exact",
            exact_environment(),
        )
        .await;
        run("lists_pages_calls_and_logs_stderr", lists_and_calls()).await;
        run("names_outside_the_allow_list_never_reach_it", allow_list()).await;
        run(
            "progress_keeps_a_call_alive_and_its_text_stays_out",
            progress(),
        )
        .await;
        run("an_answered_cancel_keeps_the_server", answered_cancel()).await;
        run("an_ignored_cancel_stops_the_server", ignored_cancel()).await;
        run("a_server_that_ignores_everything_is_killed", stubborn()).await;
        run("timeouts_are_handled_like_cancels", timeouts()).await;
        run("stop_lets_a_server_end_on_stdin_close", stop_by_itself()).await;
        run("stop_kills_a_server_that_ignores_stdin_close", stop_kills()).await;
        run("the_start_limit_is_kept", start_limit()).await;
        run("a_crash_after_start_is_noticed", crash()).await;
    });
}

async fn run(name: &str, check: impl std::future::Future<Output = ()>) {
    let started = Instant::now();
    check.await;
    println!(
        "managed_e2e: {name} ... ok ({:.1} s)",
        started.elapsed().as_secs_f64()
    );
}

/// A scratch folder with `work/` (the server's working folder) and the files the fake
/// writes.
struct Scratch {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Scratch {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        std::fs::create_dir_all(root.join("work")).unwrap();
        Self { _dir: dir, root }
    }
    fn record(&self) -> PathBuf {
        self.root.join("record.jsonl")
    }
    fn eof_marker(&self) -> PathBuf {
        self.root.join("eof-marker")
    }
    fn pid_file(&self) -> PathBuf {
        self.root.join("pid")
    }
    fn log_file(&self) -> PathBuf {
        self.root.join("logs").join("mcp-reverse.log")
    }
    /// Every line the fake received, parsed.
    fn received(&self) -> Vec<Value> {
        std::fs::read_to_string(self.record())
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
    /// The names of the tools the fake was asked to run.
    fn called(&self) -> Vec<String> {
        self.received()
            .iter()
            .filter(|m| m["method"] == "tools/call")
            .filter_map(|m| m["params"]["name"].as_str().map(str::to_owned))
            .collect()
    }
    fn spec(&self, mode: &str) -> ManagedSpec {
        let mut env = EnvSpec::empty();
        // Windows needs these to start a program at all.
        if cfg!(windows) {
            env = env.inherit("SYSTEMROOT").inherit("WINDIR");
        }
        let env = env
            .set("PATH", FIXED_PATH)
            .set("FAKE_REA_RECORD", self.record().to_string_lossy())
            .set("FAKE_REA_EOF_MARKER", self.eof_marker().to_string_lossy())
            .set("FAKE_REA_PID", self.pid_file().to_string_lossy())
            .set("REA_LOG_LEVEL", "error");
        ManagedSpec {
            name: "reverse".into(),
            program: std::env::current_exe().expect("current exe"),
            args: vec![
                FAKE_FLAG.into(),
                mode.into(),
                "a b \"c\" %d".into(),
                "--not-a-flag".into(),
            ],
            env,
            cwd: self.root.join("work"),
            log_file: Some(self.log_file()),
            start_timeout: Duration::from_secs(20),
        }
    }
}

fn allow_all() -> ToolAllowList {
    ToolAllowList::new(FAKE_TOOLS)
}

fn args(value: Value) -> Map<String, Value> {
    value.as_object().cloned().expect("object arguments")
}

async fn call(server: &ManagedServer, name: &str, arguments: Value) -> CallOutcome {
    server
        .call_outcome(
            &allow_all(),
            name,
            args(arguments),
            Duration::from_secs(20),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

async fn text(server: &ManagedServer, name: &str, arguments: Value) -> String {
    match call(server, name, arguments).await {
        CallOutcome::Done(out) => {
            assert!(!out.is_error, "{name}: {out:?}");
            out.joined_text()
        }
        other => panic!("{name}: {other:?}"),
    }
}

async fn started(spec: ManagedSpec) -> ManagedServer {
    let server = ManagedServer::new(spec);
    server.start().await.expect("starts");
    server
}

async fn eventually(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    condition()
}

/// Whether a process is still running (a zombie counts as gone). Unix only.
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    if Path::new("/proc/self/stat").exists() {
        return match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(stat) => stat
                .rsplit_once(')')
                .is_some_and(|(_, rest)| !rest.trim_start().starts_with('Z')),
            Err(_) => false,
        };
    }
    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

/// How long a cancel the server answers may take to come back.
fn answered_cancel_allowance() -> Duration {
    if cfg!(windows) {
        Duration::from_secs(3)
    } else {
        Duration::from_secs(1)
    }
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

async fn exact_environment() {
    let scratch = Scratch::new();
    let spec = scratch.spec("normal");
    let server = started(spec.clone()).await;
    let env: BTreeMap<String, String> =
        serde_json::from_str(&text(&server, "env", json!({})).await).expect("env JSON");
    assert_eq!(std::env::var(CANARY).as_deref(), Ok("leaked-from-parent"));
    assert!(!env.contains_key(CANARY), "the canary leaked: {env:?}");
    // Exactly what the spec resolves to: nothing captured, nothing added.
    let expected: Vec<(String, String)> = spec
        .env
        .resolve()
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_string_lossy().into_owned(),
                v.to_string_lossy().into_owned(),
            )
        })
        .collect();
    // macOS adds __CF_USER_TEXT_ENCODING to every process it starts.
    let set_by_os = |name: &str| cfg!(target_os = "macos") && name == "__CF_USER_TEXT_ENCODING";
    for (name, value) in &env {
        if set_by_os(name) {
            continue;
        }
        let wanted = expected.iter().find(|(k, _)| same_name(k, name));
        assert_eq!(
            wanted.map(|(_, v)| v.as_str()),
            Some(value.as_str()),
            "unexpected variable {name}"
        );
    }
    for (name, _) in &expected {
        assert!(
            env.keys().any(|k| same_name(k, name)),
            "{name} is missing: {env:?}"
        );
    }
    let path = env
        .iter()
        .find(|(k, _)| same_name(k, "PATH"))
        .map(|(_, v)| v);
    assert_eq!(path.map(String::as_str), Some(FIXED_PATH));
    // Working folder and argument list, exactly.
    let cwd = text(&server, "cwd", json!({})).await;
    assert_eq!(
        std::fs::canonicalize(&cwd).unwrap(),
        std::fs::canonicalize(&spec.cwd).unwrap()
    );
    let argv: Vec<String> =
        serde_json::from_str(&text(&server, "args", json!({})).await).expect("args JSON");
    assert_eq!(argv[1..], spec.args[..]);
    server.stop(STOP_AFTER_CANCEL).await;
}

async fn lists_and_calls() {
    let scratch = Scratch::new();
    let server = ManagedServer::new(scratch.spec("normal"));
    let tools = server.start().await.expect("starts");
    let names: Vec<&str> = tools.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, FAKE_TOOLS, "both pages are read, in order");
    let echo = &tools[0].1;
    assert_eq!(echo["properties"]["text"]["type"], "string");
    assert_eq!(
        scratch
            .received()
            .iter()
            .filter(|m| m["method"] == "tools/list")
            .count(),
        2
    );
    assert!(server.is_running() && server.pid().is_some());
    assert_eq!(server.lifecycle(), Some(Lifecycle::Legacy));
    assert_eq!(
        text(&server, "echo", json!({"text": "hello"})).await,
        "hello"
    );
    // A tool error stays a result the model can read.
    match call(&server, "echo", json!({"fail": true})).await {
        CallOutcome::Done(out) => {
            assert!(out.is_error);
            assert_eq!(out.joined_text(), "it broke");
        }
        other => panic!("{other:?}"),
    }
    // Stderr reaches the tail and exactly the named log file.
    assert!(
        eventually(Duration::from_secs(5), || server
            .stderr_tail()
            .iter()
            .any(|l| l == "fake rea: ready (normal)"))
        .await,
        "{:?}",
        server.stderr_tail()
    );
    assert!(
        eventually(Duration::from_secs(5), || std::fs::read_to_string(
            scratch.log_file()
        )
        .is_ok_and(|t| t.contains("fake rea: ready (normal)")))
        .await
    );
    server.stop(STOP_AFTER_CANCEL).await;
    assert!(!server.is_running() && !server.ended_unexpectedly());
}

async fn allow_list() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("normal")).await;
    let only_echo = ToolAllowList::new(["echo"]);
    for name in ["start_gdb_session", "slow", "instrument_with_frida"] {
        let refused = server
            .call_outcome(
                &only_echo,
                name,
                args(json!({"seconds": 0})),
                Duration::from_secs(5),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(
            matches!(refused, Err(HostError::NotFound(_))),
            "{name}: {refused:?}"
        );
        // The plain call refuses it the same way.
        let refused = server
            .call(
                &only_echo,
                name,
                Map::new(),
                Duration::from_secs(5),
                &CancellationToken::new(),
                None,
            )
            .await;
        assert!(matches!(refused, Err(HostError::NotFound(_))), "{name}");
    }
    assert!(scratch.called().is_empty(), "{:?}", scratch.received());
    let out = server
        .call_outcome(
            &only_echo,
            "echo",
            args(json!({"text": "ok"})),
            Duration::from_secs(5),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(matches!(out, CallOutcome::Done(ref o) if o.joined_text() == "ok"));
    assert_eq!(scratch.called(), vec!["echo".to_owned()]);
    server.stop(STOP_AFTER_CANCEL).await;
}

async fn progress() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("normal")).await;
    let ticks = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = ticks.clone();
    let tick: ProgressFn = Arc::new(move |line| sink.lock().unwrap().push(line));
    let begun = Instant::now();
    // Ten notes 300 ms apart: 3 s in all, three times the 1 s limit.
    let out = server
        .call_outcome(
            &allow_all(),
            "progress",
            args(json!({"steps": 10, "every_ms": 300})),
            Duration::from_secs(1),
            &CancellationToken::new(),
            Some(tick),
        )
        .await
        .expect("call");
    let took = begun.elapsed();
    let CallOutcome::Done(out) = out else {
        panic!("not kept alive: {out:?}")
    };
    assert_eq!(out.joined_text(), "progressed");
    assert!(took >= Duration::from_millis(2500), "{took:?}");
    let ticks = ticks.lock().unwrap().clone();
    assert!(!ticks.is_empty(), "no sign of life was passed on");
    assert!(ticks.iter().all(String::is_empty), "{ticks:?}");
    // At most one a second.
    assert!(ticks.len() <= 5, "{ticks:?}");
    let all = format!("{out:?} {:?}", server.stderr_tail());
    assert!(!all.contains("SECRET-PROGRESS"), "{all}");
    assert!(server.is_running());
    server.stop(STOP_AFTER_CANCEL).await;
}

/// Starts a 30 s call and cancels it after 300 ms; returns the outcome and how long it
/// took to come back after the cancel.
async fn cancel_slow_call(server: &ManagedServer) -> (CallOutcome, Duration) {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let cancelled_at = Arc::new(Mutex::new(None::<Instant>));
    let stamp = cancelled_at.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        *stamp.lock().unwrap() = Some(Instant::now());
        trigger.cancel();
    });
    let out = server
        .call_outcome(
            &allow_all(),
            "slow",
            args(json!({"seconds": 30})),
            Duration::from_secs(60),
            &cancel,
            None,
        )
        .await
        .expect("call");
    let at = cancelled_at.lock().unwrap().expect("cancelled");
    (out, at.elapsed())
}

async fn answered_cancel() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("answers-cancel")).await;
    let pid = text(&server, "pid", json!({})).await;
    let (out, took) = cancel_slow_call(&server).await;
    assert_eq!(out, CallOutcome::Cancelled { stopped: false });
    assert!(took < answered_cancel_allowance(), "{took:?}");
    // The server got the cancellation for that call, and keeps running.
    let received = scratch.received();
    let call_id = received
        .iter()
        .find(|m| m["method"] == "tools/call" && m["params"]["name"] == "slow")
        .map(|m| m["id"].clone())
        .expect("slow call");
    assert!(
        received.iter().any(
            |m| m["method"] == "notifications/cancelled" && m["params"]["requestId"] == call_id
        ),
        "{received:?}"
    );
    assert!(server.is_running());
    assert_eq!(text(&server, "pid", json!({})).await, pid);
    server.stop(STOP_AFTER_CANCEL).await;
}

async fn ignored_cancel() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("ignores-cancel")).await;
    let pid: u32 = text(&server, "pid", json!({})).await.parse().unwrap();
    let (out, took) = cancel_slow_call(&server).await;
    assert_eq!(out, CallOutcome::Cancelled { stopped: true });
    assert!(took >= CANCEL_GRACE, "{took:?}");
    assert!(
        took < CANCEL_GRACE + STOP_AFTER_CANCEL + Duration::from_secs(3),
        "{took:?}"
    );
    assert!(!server.is_running());
    assert!(
        !server.ended_unexpectedly(),
        "a stop on purpose is not a crash"
    );
    #[cfg(unix)]
    assert!(
        eventually(Duration::from_secs(5), || !alive(pid)).await,
        "the old process {pid} survived"
    );
    assert!(
        server
            .stderr_tail()
            .iter()
            .any(|l| l.contains("did not end the stopped call")),
        "{:?}",
        server.stderr_tail()
    );
    // Nothing runs until the next start, which is a new process.
    let refused = server
        .call_outcome(
            &allow_all(),
            "pid",
            Map::new(),
            Duration::from_secs(5),
            &CancellationToken::new(),
            None,
        )
        .await;
    assert!(
        matches!(refused, Err(HostError::NotReady(_))),
        "{refused:?}"
    );
    server.start().await.expect("restarts");
    let again: u32 = text(&server, "pid", json!({})).await.parse().unwrap();
    assert_ne!(again, pid);
    server.stop(STOP_AFTER_CANCEL).await;
}

async fn stubborn() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("stubborn")).await;
    let pid: u32 = text(&server, "pid", json!({})).await.parse().unwrap();
    let (out, took) = cancel_slow_call(&server).await;
    assert_eq!(out, CallOutcome::Cancelled { stopped: true });
    // The grace for the answer, then the grace after closing its input, then the kill.
    assert!(took >= CANCEL_GRACE + STOP_AFTER_CANCEL, "{took:?}");
    assert!(
        took < CANCEL_GRACE + STOP_AFTER_CANCEL + Duration::from_secs(3),
        "{took:?}"
    );
    assert!(!server.is_running());
    #[cfg(unix)]
    assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    #[cfg(not(unix))]
    let _ = pid;
    assert!(!scratch.eof_marker().exists(), "it never ended by itself");
    assert!(
        server
            .stderr_tail()
            .iter()
            .any(|l| l.contains("was still running after 3 seconds, so it was ended")),
        "{:?}",
        server.stderr_tail()
    );
}

async fn timeouts() {
    // Answered: the server keeps running.
    let scratch = Scratch::new();
    let server = started(scratch.spec("answers-cancel")).await;
    let begun = Instant::now();
    let out = server
        .call_outcome(
            &allow_all(),
            "slow",
            args(json!({"seconds": 30})),
            Duration::from_secs(1),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(out, CallOutcome::TimedOut { stopped: false });
    assert!(begun.elapsed() < Duration::from_secs(1) + answered_cancel_allowance());
    assert!(server.is_running());
    // The plain call reads it as an error result.
    let plain = server
        .call(
            &allow_all(),
            "slow",
            args(json!({"seconds": 30})),
            Duration::from_secs(1),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert!(
        plain.is_error
            && plain
                .joined_text()
                .contains("did not finish within 1 seconds")
    );
    server.stop(STOP_AFTER_CANCEL).await;
    // Ignored: the server is stopped after the grace.
    let scratch = Scratch::new();
    let server = started(scratch.spec("ignores-cancel")).await;
    let begun = Instant::now();
    let out = server
        .call_outcome(
            &allow_all(),
            "slow",
            args(json!({"seconds": 30})),
            Duration::from_secs(1),
            &CancellationToken::new(),
            None,
        )
        .await
        .unwrap();
    assert_eq!(out, CallOutcome::TimedOut { stopped: true });
    assert!(begun.elapsed() >= Duration::from_secs(1) + CANCEL_GRACE);
    assert!(!server.is_running());
}

async fn stop_by_itself() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("normal")).await;
    let begun = Instant::now();
    let outcome = server.stop_with(STOP_AFTER_CANCEL).await;
    assert_eq!(outcome, StopOutcome::Exited("exited with code 0".into()));
    assert!(
        begun.elapsed() < Duration::from_secs(2),
        "{:?}",
        begun.elapsed()
    );
    assert!(
        scratch.eof_marker().exists(),
        "the server saw its input close"
    );
    assert!(!server.is_running());
    assert!(
        server
            .stderr_tail()
            .iter()
            .any(|l| l == "--- stopped: the server exited with code 0 ---"),
        "{:?}",
        server.stderr_tail()
    );
}

async fn stop_kills() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("lingers")).await;
    let pid = server.pid().expect("pid");
    let grace = Duration::from_secs(1);
    let begun = Instant::now();
    let outcome = server.stop_with(grace).await;
    let took = begun.elapsed();
    assert_eq!(outcome, StopOutcome::Killed);
    assert!(
        took >= grace && took < grace + Duration::from_secs(3),
        "{took:?}"
    );
    #[cfg(unix)]
    assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    #[cfg(not(unix))]
    let _ = pid;
    assert!(!scratch.eof_marker().exists());
}

async fn start_limit() {
    let scratch = Scratch::new();
    let mut spec = scratch.spec("no-handshake");
    spec.start_timeout = Duration::from_secs(2);
    let server = ManagedServer::new(spec);
    let begun = Instant::now();
    let error = server
        .start()
        .await
        .expect_err("never answers the handshake");
    let took = begun.elapsed();
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(8),
        "{took:?}"
    );
    let message = error.to_string();
    assert!(message.contains("within 2 seconds"), "{message}");
    assert!(
        message.contains("fake rea: ready (no-handshake)"),
        "{message}"
    );
    assert!(!server.is_running());
    #[cfg(unix)]
    {
        let pid: u32 = std::fs::read_to_string(scratch.pid_file())
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(eventually(Duration::from_secs(5), || !alive(pid)).await);
    }
}

async fn crash() {
    let scratch = Scratch::new();
    let server = started(scratch.spec("normal")).await;
    let out = server
        .call_outcome(
            &allow_all(),
            "crash",
            Map::new(),
            Duration::from_secs(10),
            &CancellationToken::new(),
            None,
        )
        .await;
    assert!(matches!(out, Err(HostError::NotReady(_))), "{out:?}");
    assert!(eventually(Duration::from_secs(5), || server.ended_unexpectedly()).await);
    assert!(!server.is_running());
    assert!(
        eventually(Duration::from_secs(5), || server
            .stderr_tail()
            .iter()
            .any(|l| l == "--- the server exited with code 7 ---"))
        .await,
        "{:?}",
        server.stderr_tail()
    );
    // The next start is a fresh process.
    server.start().await.expect("restarts");
    assert!(server.is_running() && !server.ended_unexpectedly());
    server.stop(STOP_AFTER_CANCEL).await;
}

/// The fake server: JSON lines on standard input and output, written by hand.
mod fake {
    use std::collections::{BTreeMap, HashMap};
    use std::io::{BufRead, Write};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::{Value, json};

    type Out = Arc<Mutex<std::io::Stdout>>;
    type Pending = Arc<Mutex<HashMap<String, mpsc::Sender<()>>>>;

    fn send(out: &Out, message: &Value) {
        let mut out = out.lock().unwrap();
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn respond(out: &Out, id: &Value, result: Value) {
        send(out, &json!({"jsonrpc": "2.0", "id": id, "result": result}));
    }

    fn text(out: &Out, id: &Value, text: &str, is_error: bool) {
        respond(
            out,
            id,
            json!({"content": [{"type": "text", "text": text}], "isError": is_error}),
        );
    }

    fn tool(name: &str) -> Value {
        let schema = match name {
            "echo" => {
                json!({"type": "object", "properties": {"text": {"type": "string"}, "fail": {"type": "boolean"}}})
            }
            _ => json!({"type": "object"}),
        };
        json!({"name": name, "description": format!("fake {name}"), "inputSchema": schema})
    }

    pub fn run(mode: &str) {
        if let Ok(path) = std::env::var("FAKE_REA_PID") {
            let _ = std::fs::write(path, std::process::id().to_string());
        }
        eprintln!("fake rea: ready ({mode})");
        let record = std::env::var("FAKE_REA_RECORD").ok();
        let out: Out = Arc::new(Mutex::new(std::io::stdout()));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if let Some(path) = &record
                && let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
            {
                let _ = writeln!(f, "{line}");
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let method = message["method"].as_str().unwrap_or_default().to_owned();
            let id = message.get("id").cloned();
            match (method.as_str(), id) {
                ("initialize", Some(id)) => {
                    if mode != "no-handshake" {
                        let version = message["params"]["protocolVersion"].clone();
                        respond(
                            &out,
                            &id,
                            json!({
                                "protocolVersion": version,
                                "capabilities": {"tools": {}},
                                "serverInfo": {"name": "fake-rea", "version": "1.0"}
                            }),
                        );
                    }
                }
                ("tools/list", Some(id)) => {
                    let names = &super::FAKE_TOOLS;
                    let result = if message["params"]["cursor"] == "page2" {
                        json!({"tools": names[5..].iter().map(|n| tool(n)).collect::<Vec<_>>()})
                    } else {
                        json!({
                            "tools": names[..5].iter().map(|n| tool(n)).collect::<Vec<_>>(),
                            "nextCursor": "page2"
                        })
                    };
                    respond(&out, &id, result);
                }
                ("tools/call", Some(id)) => call(&out, &pending, &message, id),
                // `ignores-cancel` keeps the call running (and its sender alive).
                ("notifications/cancelled", None) if mode == "answers-cancel" => {
                    let request = message["params"]["requestId"].to_string();
                    if let Some(stop) = pending.lock().unwrap().remove(&request) {
                        let _ = stop.send(());
                    }
                }
                ("ping", Some(id)) => respond(&out, &id, json!({})),
                (_, Some(id)) => send(
                    &out,
                    &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown method"}}),
                ),
                _ => {}
            }
        }
        // Standard input closed.
        if mode == "lingers" || mode == "stubborn" {
            std::thread::sleep(Duration::from_secs(60));
        }
        if let Ok(path) = std::env::var("FAKE_REA_EOF_MARKER") {
            let _ = std::fs::write(path, b"eof");
        }
        std::process::exit(0);
    }

    fn call(out: &Out, pending: &Pending, message: &Value, id: Value) {
        let params = &message["params"];
        let args = &params["arguments"];
        match params["name"].as_str().unwrap_or_default() {
            "echo" if args["fail"] == true => text(out, &id, "it broke", true),
            "echo" => text(out, &id, args["text"].as_str().unwrap_or_default(), false),
            "env" => {
                let env: BTreeMap<String, String> = std::env::vars().collect();
                text(out, &id, &serde_json::to_string(&env).unwrap(), false);
            }
            "cwd" => {
                let cwd = std::env::current_dir().unwrap();
                text(out, &id, &cwd.to_string_lossy(), false);
            }
            "args" => {
                let argv: Vec<String> = std::env::args().collect();
                text(out, &id, &serde_json::to_string(&argv).unwrap(), false);
            }
            "pid" => text(out, &id, &std::process::id().to_string(), false),
            "crash" => {
                eprintln!("fake rea: crashing");
                std::process::exit(7);
            }
            "slow" => {
                let seconds = args["seconds"].as_f64().unwrap_or(30.0);
                let (stop, stopped) = mpsc::channel();
                pending.lock().unwrap().insert(id.to_string(), stop);
                let out = out.clone();
                std::thread::spawn(move || {
                    match stopped.recv_timeout(Duration::from_secs_f64(seconds)) {
                        Ok(()) => text(&out, &id, "stopped early", true),
                        Err(_) => text(&out, &id, "done", false),
                    }
                });
            }
            "progress" => {
                let steps = args["steps"].as_u64().unwrap_or(3);
                let every = Duration::from_millis(args["every_ms"].as_u64().unwrap_or(100));
                let token = params["_meta"]["progressToken"].clone();
                let out = out.clone();
                std::thread::spawn(move || {
                    for step in 1..=steps {
                        std::thread::sleep(every);
                        send(
                            &out,
                            &json!({
                                "jsonrpc": "2.0",
                                "method": "notifications/progress",
                                "params": {
                                    "progressToken": token,
                                    "progress": step,
                                    "total": steps,
                                    "message": format!("SECRET-PROGRESS step {step}")
                                }
                            }),
                        );
                    }
                    text(&out, &id, "progressed", false);
                });
            }
            other => send(
                out,
                &json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("no tool {other}")}}),
            ),
        }
    }
}
