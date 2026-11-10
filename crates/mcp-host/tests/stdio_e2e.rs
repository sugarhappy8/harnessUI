//! End-to-end tests of stdio MCP servers through the host, and the fake server itself.
//!
//! This test target has `harness = false`, so this file's `main` runs for it. Started with
//! [`FAKE_FLAG`] it *is* a small MCP server built on rmcp's server side, with a dynamic
//! tool list. Started any other way (by `cargo test`) it starts copies of itself through
//! the host and checks:
//!
//! 1. tools are listed and called (text, images, tool errors, the 1 MiB cap, progress);
//! 2. the child's environment holds only the allowlist and the config's `env` (a canary
//!    set in this process does not leak), with secret placeholders resolved;
//! 3. a tool-list change notification refreshes the registry;
//! 4. a slow call cancelled through the token returns within 1 s, the server's whole
//!    process tree (a shell and the server it started) is gone afterwards, and the next
//!    call restarts it; a call past the timeout is handled the same way;
//! 5. a server that prints garbage, or exits at once, is Failed with its stderr tail;
//! 6. a modern-only server is reached by the fallback to the 2026-07-28 lifecycle, the
//!    remembered lifecycle skips the failed attempt, and `input_required` rounds are
//!    answered (elicitation declined, roots empty);
//! 7. shutdown kills every server.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mcp_host::{
    CallContext, ChatContext, Content, Described, Host, HostEvent, HostOptions, Lifecycle,
    ServerConfig, ServerKind, ServerState, ToolOutput, ToolPrefs,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

/// Hidden flag that turns this binary into the fake MCP server; the next argument is the
/// mode (`normal`, `modern-only`, `garbage`, `exit`).
const FAKE_FLAG: &str = "--mcp-host-test-fake-server";

/// Set in this process only; must never reach a server.
const CANARY: &str = "MCP_HOST_TEST_CANARY";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(FAKE_FLAG) {
        fake::run(
            args.get(2).map(String::as_str).unwrap_or("normal"),
            args.get(3).map(String::as_str),
        );
        return;
    }
    if args.iter().any(|a| a == "--list") {
        println!("stdio_e2e: test");
        return;
    }
    // SAFETY: no other thread exists yet; the runtime is built after this.
    unsafe { std::env::set_var(CANARY, "leaked-from-parent") };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    runtime.block_on(async {
        run("lists_and_calls_tools", lists_and_calls_tools()).await;
        run("environment_is_scrubbed", environment_is_scrubbed()).await;
        run(
            "tool_list_changes_refresh_the_registry",
            tool_list_changes(),
        )
        .await;
        #[cfg(unix)]
        run(
            "cancel_kills_the_process_tree_and_restarts",
            cancel_kills_tree(),
        )
        .await;
        run("timeout_stops_the_call_and_restarts", timeout_restarts()).await;
        run(
            "stop_during_a_slow_restart_returns_promptly",
            stop_during_restart(),
        )
        .await;
        run(
            "stop_while_the_server_is_starting_returns_promptly",
            stop_while_starting(),
        )
        .await;
        run("crash_after_ready_fails_and_restarts", crash_after_ready()).await;
        run("garbage_server_fails_with_stderr_tail", garbage_fails()).await;
        run("exiting_server_fails_with_exit_code", exiting_fails()).await;
        run("modern_only_server_uses_the_fallback", modern_only()).await;
        run("shutdown_stops_every_server", shutdown_stops()).await;
    });
}

async fn run(name: &str, check: impl std::future::Future<Output = ()>) {
    let started = Instant::now();
    check.await;
    println!(
        "stdio_e2e: {name} ... ok ({:.1} s)",
        started.elapsed().as_secs_f64()
    );
}

fn exe() -> String {
    std::env::current_exe()
        .expect("current exe")
        .to_string_lossy()
        .into_owned()
}

fn fake(mode: &str) -> ServerConfig {
    ServerConfig::Stdio {
        command: exe(),
        args: vec![FAKE_FLAG.into(), mode.into()],
        env: BTreeMap::new(),
        cwd: None,
    }
}

fn options() -> HostOptions {
    HostOptions {
        start_timeout: Duration::from_secs(20),
        call_timeout: Duration::from_secs(20),
        secret_resolver: Arc::new(|name: &str| (name == "tok").then(|| "s3cret".to_owned())),
        client_name: "mcp-host-test".into(),
        ..HostOptions::default()
    }
}

fn ctx() -> CallContext {
    CallContext {
        chat: ChatContext::default(),
        cancel: CancellationToken::new(),
        progress: None,
    }
}

/// Validates, describes and calls a tool the way the agent loop does.
async fn call(host: &Host, exposed: &str, args: Value, ctx: CallContext) -> ToolOutput {
    let args = args.as_object().cloned().expect("object arguments");
    host.validate(exposed, &Value::Object(args.clone()))
        .unwrap_or_else(|e| panic!("{exposed}: {e:?}"));
    let Described::Action(action) = host
        .describe(
            exposed,
            &args,
            &ChatContext::default(),
            ToolPrefs::default(),
        )
        .await
    else {
        panic!("{exposed}: no action")
    };
    host.call(exposed, &args, &action, ctx).await
}

fn status(host: &Host, name: &str) -> mcp_host::ServerStatus {
    host.servers()
        .into_iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("no server {name}"))
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

/// Whether a process is still running (a zombie counts as gone).
#[cfg(unix)]
fn alive(pid: u32) -> bool {
    if std::path::Path::new("/proc/self/stat").exists() {
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

async fn lists_and_calls_tools() {
    let host = Host::new(options());
    host.add_external("fake", fake("normal"))
        .await
        .expect("starts");
    let s = status(&host, "fake");
    assert_eq!(s.state, ServerState::Ready);
    assert_eq!(s.kind, ServerKind::Stdio);
    assert_eq!(s.lifecycle, Some(Lifecycle::Legacy));
    assert_eq!(s.protocol_version.as_deref(), Some("2025-11-25"));
    assert!(s.pid.is_some());
    assert!(s.tool_count >= 9, "{s:?}");
    assert!(
        s.stderr_tail
            .iter()
            .any(|l| l.contains("fake server: ready")),
        "{:?}",
        s.stderr_tail
    );

    let tools = host.tools();
    let echo = tools
        .iter()
        .find(|t| t.exposed == "fake__echo")
        .expect("echo");
    assert_eq!(echo.original, "echo");
    assert_eq!(echo.def.description, "Echo the text back");
    assert!(host.tool_ready("fake__echo"));
    assert!(host.validate("fake__echo", &json!({})).is_err());

    let out = call(&host, "fake__echo", json!({"text": "hello"}), ctx()).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.joined_text(), "hello");

    let out = call(&host, "fake__image", json!({}), ctx()).await;
    assert_eq!(
        out.content,
        vec![Content::Image {
            mime: "image/png".into(),
            data_base64: "iVBORw0KGgo=".into()
        }]
    );

    let out = call(&host, "fake__fail", json!({}), ctx()).await;
    assert!(out.is_error);
    assert_eq!(out.joined_text(), "it broke");

    let out = call(&host, "fake__big", json!({}), ctx()).await;
    let text = out.joined_text();
    assert!(text.len() < 1024 * 1024 + 200, "{}", text.len());
    assert!(text.ends_with("the rest was discarded]"));

    let lines = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = lines.clone();
    let mut with_progress = ctx();
    with_progress.progress = Some(Arc::new(move |line: String| {
        sink.lock().unwrap().push(line)
    }));
    let out = call(&host, "fake__progress", json!({}), with_progress).await;
    assert_eq!(out.joined_text(), "progressed");
    assert!(
        eventually(Duration::from_secs(2), || lines.lock().unwrap().len() == 3).await
            || !lines.lock().unwrap().is_empty(),
        "no progress lines"
    );
    assert!(
        lines.lock().unwrap()[0].starts_with("step 1"),
        "{:?}",
        lines.lock().unwrap()
    );
    host.shutdown().await;
}

async fn environment_is_scrubbed() {
    let host = Host::new(options());
    let mut cfg = fake("normal");
    if let ServerConfig::Stdio { env, .. } = &mut cfg {
        env.insert("FROM_CONFIG".into(), "key=${secret:tok}".into());
    }
    host.add_external("envcheck", cfg).await.expect("starts");
    let out = call(&host, "envcheck__env", json!({}), ctx()).await;
    let env: BTreeMap<String, String> = serde_json::from_str(&out.joined_text()).expect("env JSON");
    assert_eq!(std::env::var(CANARY).as_deref(), Ok("leaked-from-parent"));
    assert!(!env.contains_key(CANARY), "the canary leaked: {env:?}");
    assert_eq!(
        env.get("FROM_CONFIG").map(String::as_str),
        Some("key=s3cret")
    );
    assert!(env.contains_key("PATH"));
    let allowed: Vec<&str> = if cfg!(windows) {
        runtime::spawn::WINDOWS_ENV_ALLOWLIST.to_vec()
    } else {
        runtime::spawn::UNIX_ENV_ALLOWLIST.to_vec()
    };
    for name in env.keys() {
        assert!(
            name == "FROM_CONFIG" || allowed.iter().any(|a| a.eq_ignore_ascii_case(name)),
            "unexpected variable {name}"
        );
    }
    host.shutdown().await;
}

async fn tool_list_changes() {
    let host = Host::new(options());
    host.add_external("dyn", fake("normal"))
        .await
        .expect("starts");
    let mut events = host.subscribe();
    assert!(!host.tools().iter().any(|t| t.exposed == "dyn__extra"));
    let out = call(&host, "dyn__add_tool", json!({}), ctx()).await;
    assert_eq!(out.joined_text(), "added");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match tokio::time::timeout(left, events.recv()).await {
            Ok(Ok(HostEvent::ToolsChanged { server })) if server == "dyn" => break,
            Ok(Ok(_)) => continue,
            other => panic!("no ToolsChanged event: {other:?}"),
        }
    }
    let extra = host
        .tools()
        .into_iter()
        .find(|t| t.exposed == "dyn__extra")
        .expect("extra");
    assert_eq!(extra.def.description, "Added later");
    host.shutdown().await;
}

#[cfg(unix)]
async fn cancel_kills_tree() {
    let host = Host::new(options());
    // A shell that starts the server in the background and waits for it, like an npx or
    // uvx shim: the server is a grandchild of the app, in the shell's process group.
    let sh = runtime::spawn::resolve_program("sh", None).expect("sh");
    let cfg = ServerConfig::Stdio {
        command: sh.to_string_lossy().into_owned(),
        args: vec![
            "-c".into(),
            format!("exec 3<&0; \"$0\" {FAKE_FLAG} normal <&3 3<&- & wait"),
            exe(),
        ],
        env: BTreeMap::new(),
        cwd: None,
    };
    host.add_external("tree", cfg)
        .await
        .expect("starts through the shell");
    let shell_pid = status(&host, "tree").pid.expect("pid");
    let server_pid: u32 = call(&host, "tree__pid", json!({}), ctx())
        .await
        .joined_text()
        .parse()
        .expect("server pid");
    assert_ne!(shell_pid, server_pid);
    assert!(alive(shell_pid) && alive(server_pid));

    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let mut call_ctx = ctx();
    call_ctx.cancel = cancel;
    let started = Instant::now();
    let out = call(&host, "tree__slow", json!({"seconds": 30}), call_ctx).await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "the cancelled call took {elapsed:?}"
    );
    assert!(out.is_error);
    assert!(
        out.joined_text().starts_with("cancelled"),
        "{}",
        out.joined_text()
    );
    assert!(status(&host, "tree").restart_on_next_use);

    // 500 ms later the whole tree is killed; the server ignored the cancellation.
    assert!(
        eventually(Duration::from_secs(3), || !alive(shell_pid)
            && !alive(server_pid))
        .await,
        "process tree still alive: shell {} server {}",
        alive(shell_pid),
        alive(server_pid)
    );
    assert_eq!(
        status(&host, "tree").state,
        ServerState::Ready,
        "a cancel is not a failure"
    );

    // The next call restarts the server.
    let out = call(&host, "tree__echo", json!({"text": "back"}), ctx()).await;
    assert_eq!(out.joined_text(), "back");
    let s = status(&host, "tree");
    assert!(!s.restart_on_next_use);
    assert_ne!(s.pid, Some(shell_pid));
    let new_pid = s.pid.expect("new pid");
    host.shutdown().await;
    assert!(eventually(Duration::from_secs(3), || !alive(new_pid)).await);
}

async fn timeout_restarts() {
    let mut opts = options();
    opts.call_timeout = Duration::from_millis(800);
    let host = Host::new(opts);
    host.add_external("slowpoke", fake("normal"))
        .await
        .expect("starts");
    let first_pid = status(&host, "slowpoke").pid.expect("pid");
    let started = Instant::now();
    let out = call(&host, "slowpoke__slow", json!({"seconds": 10}), ctx()).await;
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert!(out.is_error);
    assert!(
        out.joined_text().contains("did not finish within"),
        "{}",
        out.joined_text()
    );
    assert!(status(&host, "slowpoke").restart_on_next_use);
    let out = call(&host, "slowpoke__echo", json!({"text": "again"}), ctx()).await;
    assert_eq!(out.joined_text(), "again");
    assert_ne!(status(&host, "slowpoke").pid, Some(first_pid));
    host.shutdown().await;
}

/// A call context whose token is cancelled (Stop) after `delay`.
fn cancelled_after(delay: Duration) -> CallContext {
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(delay).await;
        trigger.cancel();
    });
    CallContext { cancel, ..ctx() }
}

/// A fake server that starts at once the first time and takes 5 s on every later start
/// (a marker file in `dir` remembers the first start).
fn slow_restarting(dir: &std::path::Path) -> ServerConfig {
    let mut cfg = fake("slow-restart");
    if let ServerConfig::Stdio { args, .. } = &mut cfg {
        args.push(dir.join("started-once").to_string_lossy().into_owned());
    }
    cfg
}

/// 4.2: Stop ends a call within 1 s even while the server is being restarted after an
/// earlier cancel, and the restart is not abandoned half-way.
async fn stop_during_restart() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = Host::new(options());
    host.add_external("phoenix", slow_restarting(dir.path()))
        .await
        .expect("starts");
    let out = call(
        &host,
        "phoenix__slow",
        json!({"seconds": 30}),
        cancelled_after(Duration::from_millis(200)),
    )
    .await;
    assert!(out.joined_text().starts_with("cancelled"), "{out:?}");
    assert!(status(&host, "phoenix").restart_on_next_use);

    // The next call restarts the server, which now takes 5 s to start; Stop after 300 ms.
    let started = Instant::now();
    let out = call(
        &host,
        "phoenix__echo",
        json!({"text": "x"}),
        cancelled_after(Duration::from_millis(300)),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "the stopped call took {elapsed:?}"
    );
    assert!(
        out.joined_text().starts_with("cancelled"),
        "{}",
        out.joined_text()
    );
    // The restart carries on in the background and the server comes back by itself.
    assert!(
        eventually(Duration::from_secs(15), || {
            let s = status(&host, "phoenix");
            s.state == ServerState::Ready && !s.restart_on_next_use
        })
        .await,
        "{:?}",
        status(&host, "phoenix")
    );
    let out = call(&host, "phoenix__echo", json!({"text": "back"}), ctx()).await;
    assert_eq!(out.joined_text(), "back");
    host.shutdown().await;
}

/// Stop also ends a call that is waiting for a server restarted from the UI.
async fn stop_while_starting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let host = Host::new(options());
    host.add_external("lazarus", slow_restarting(dir.path()))
        .await
        .expect("starts");
    let restarter = host.clone();
    let restart = tokio::spawn(async move { restarter.restart("lazarus").await });
    assert!(
        eventually(Duration::from_secs(2), || status(&host, "lazarus").state
            == ServerState::Starting)
        .await
    );
    let started = Instant::now();
    let out = call(
        &host,
        "lazarus__echo",
        json!({"text": "x"}),
        cancelled_after(Duration::from_millis(300)),
    )
    .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(1),
        "the stopped call took {elapsed:?}"
    );
    assert!(
        out.joined_text().starts_with("cancelled"),
        "{}",
        out.joined_text()
    );
    restart.await.expect("join").expect("restarts");
    assert_eq!(status(&host, "lazarus").state, ServerState::Ready);
    host.shutdown().await;
}

/// A server that dies after it was Ready is Failed with its exit code and last words, and
/// a restart brings it back.
async fn crash_after_ready() {
    let host = Host::new(options());
    host.add_external("fragile", fake("normal"))
        .await
        .expect("starts");
    let out = call(&host, "fragile__crash", json!({}), ctx()).await;
    assert!(out.is_error, "{out:?}");
    assert!(
        eventually(Duration::from_secs(5), || matches!(
            status(&host, "fragile").state,
            ServerState::Failed { .. }
        ))
        .await,
        "{:?}",
        status(&host, "fragile")
    );
    let ServerState::Failed { reason } = status(&host, "fragile").state else {
        unreachable!()
    };
    assert!(reason.contains("exited with code 7"), "{reason}");
    assert!(reason.contains("fake server: crashing"), "{reason}");
    assert!(!host.tool_ready("fragile__echo"));
    let out = call(&host, "fragile__echo", json!({"text": "x"}), ctx()).await;
    assert!(
        out.is_error && out.joined_text().contains("not available"),
        "{}",
        out.joined_text()
    );
    host.restart("fragile").await.expect("restarts");
    assert!(host.tool_ready("fragile__echo"));
    let out = call(&host, "fragile__echo", json!({"text": "again"}), ctx()).await;
    assert_eq!(out.joined_text(), "again");
    host.shutdown().await;
}

async fn garbage_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut opts = options();
    opts.stderr_log_dir = Some(dir.path().to_path_buf());
    let host = Host::new(opts);
    let error = host
        .add_external("garbage", fake("garbage"))
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("exited with code 1"), "{text}");
    assert!(text.contains("fake server: about to fail"), "{text}");
    let s = status(&host, "garbage");
    let ServerState::Failed { reason } = &s.state else {
        panic!("not failed: {s:?}")
    };
    assert!(reason.contains("about to fail"), "{reason}");
    assert!(
        s.stderr_tail
            .iter()
            .any(|l| l == "fake server: about to fail"),
        "{:?}",
        s.stderr_tail
    );
    let log = std::fs::read_to_string(dir.path().join("mcp-server-garbage.log")).expect("log file");
    assert!(log.contains("fake server: about to fail"), "{log}");
    host.shutdown().await;
}

async fn exiting_fails() {
    let host = Host::new(options());
    let error = host
        .add_external("quitter", fake("exit"))
        .await
        .unwrap_err();
    let text = error.to_string();
    assert!(text.contains("exited with code 3"), "{text}");
    assert!(text.contains("fatal: missing API key"), "{text}");
    assert!(matches!(
        status(&host, "quitter").state,
        ServerState::Failed { .. }
    ));
    // A restart tries again and fails the same way.
    assert!(host.restart("quitter").await.is_err());
    host.shutdown().await;
}

async fn modern_only() {
    let host = Host::new(options());
    host.add_external("modern", fake("modern-only"))
        .await
        .expect("starts");
    let s = status(&host, "modern");
    assert_eq!(s.state, ServerState::Ready);
    assert_eq!(s.lifecycle, Some(Lifecycle::Modern));
    assert_eq!(s.protocol_version.as_deref(), Some("2026-07-28"));
    assert!(
        s.stderr_tail
            .iter()
            .any(|l| l.contains("did not accept the initialize")),
        "{:?}",
        s.stderr_tail
    );
    let out = call(&host, "modern__echo", json!({"text": "new era"}), ctx()).await;
    assert_eq!(out.joined_text(), "new era");
    // input_required: the elicitation is declined and roots is empty.
    let out = call(&host, "modern__ask", json!({}), ctx()).await;
    assert!(!out.is_error, "{out:?}");
    assert_eq!(out.joined_text(), "elicitation: decline; roots: 0");

    // With the remembered lifecycle the first attempt succeeds.
    host.add_external_with_lifecycle("remembered", fake("modern-only"), Some(Lifecycle::Modern))
        .await
        .expect("starts");
    let s = status(&host, "remembered");
    assert_eq!(s.lifecycle, Some(Lifecycle::Modern));
    assert!(
        !s.stderr_tail.iter().any(|l| l.contains("did not accept")),
        "{:?}",
        s.stderr_tail
    );
    // A server remembered as modern that turns out legacy falls back the other way.
    host.add_external_with_lifecycle("downgraded", fake("normal"), Some(Lifecycle::Modern))
        .await
        .expect("starts");
    assert_eq!(status(&host, "downgraded").state, ServerState::Ready);
    host.shutdown().await;
}

async fn shutdown_stops() {
    let host = Host::new(options());
    host.add_external("one", fake("normal")).await.expect("one");
    host.add_external("two", fake("normal")).await.expect("two");
    let pids: Vec<u32> = host.servers().iter().filter_map(|s| s.pid).collect();
    assert_eq!(pids.len(), 2);
    let started = Instant::now();
    host.shutdown().await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    for s in host.servers() {
        assert_eq!(s.state, ServerState::Stopped);
        assert_eq!(s.pid, None);
    }
    #[cfg(unix)]
    for pid in pids {
        assert!(
            eventually(Duration::from_secs(3), || !alive(pid)).await,
            "{pid} alive"
        );
    }
    let out = call_after_shutdown(&host).await;
    assert!(out.is_error);
}

async fn call_after_shutdown(host: &Host) -> ToolOutput {
    let args = json!({"text": "x"}).as_object().cloned().expect("object");
    let Described::Action(action) = host
        .describe(
            "one__echo",
            &args,
            &ChatContext::default(),
            ToolPrefs::default(),
        )
        .await
    else {
        panic!("no action")
    };
    host.call("one__echo", &args, &action, ctx()).await
}

/// The fake MCP server. Roots is deprecated by SEP-2577 but still part of the protocol,
/// and the host must answer it.
#[allow(deprecated)]
mod fake {
    use std::borrow::Cow;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use rmcp::model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, ElicitRequest,
        ElicitRequestParams, ElicitationSchema, ErrorData, Implementation, InputRequest,
        InputRequiredResult, ListRootsRequest, ListToolsResult, PaginatedRequestParams,
        ProgressNotificationParam, ProtocolVersion, ServerCapabilities, ServerConfig, Tool,
    };
    use rmcp::service::RequestContext;
    use rmcp::{RoleServer, ServerHandler, ServiceExt};
    use serde_json::{Value, json};

    pub fn run(mode: &str, extra: Option<&str>) {
        match mode {
            "garbage" => {
                println!("this is not json");
                println!("{{\"jsonrpc\": broken");
                eprintln!("fake server: about to fail");
                std::process::exit(1);
            }
            "exit" => {
                eprintln!("fatal: missing API key");
                std::process::exit(3);
            }
            // First start at once (leaving a marker file), every later one after 5 s.
            "slow-restart" => {
                if let Some(marker) = extra {
                    if std::path::Path::new(marker).exists() {
                        eprintln!("fake server: slow start");
                        std::thread::sleep(Duration::from_secs(5));
                    } else {
                        let _ = std::fs::write(marker, b"started");
                    }
                }
            }
            _ => {}
        }
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async move {
            let server = Fake {
                tools: Arc::new(Mutex::new(tools())),
                modern_only: mode == "modern-only",
            };
            eprintln!("fake server: ready ({mode})");
            match server
                .serve((tokio::io::stdin(), tokio::io::stdout()))
                .await
            {
                Ok(running) => {
                    let _ = running.waiting().await;
                }
                Err(error) => {
                    eprintln!("fake server: handshake refused: {error}");
                    std::process::exit(2);
                }
            }
        });
    }

    fn tool(name: &'static str, description: &'static str, schema: Value) -> Tool {
        let schema = schema.as_object().cloned().unwrap_or_default();
        Tool::new(name, description, Arc::new(schema))
    }

    fn tools() -> Vec<Tool> {
        let empty = || json!({"type": "object"});
        vec![
            tool(
                "echo",
                "Echo the text back",
                json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}),
            ),
            tool("env", "The server's environment", empty()),
            tool("pid", "The server's process id", empty()),
            tool(
                "slow",
                "Sleeps, ignoring cancellation",
                json!({"type": "object", "properties": {"seconds": {"type": "number"}}}),
            ),
            tool("add_tool", "Adds a tool", empty()),
            tool("big", "Two megabytes of text", empty()),
            tool("image", "A tiny image", empty()),
            tool("fail", "Always fails", empty()),
            tool("progress", "Reports progress", empty()),
            tool("ask", "Needs input (2026-07-28 only)", empty()),
            tool("crash", "Exits the server with code 7", empty()),
        ]
    }

    #[derive(Clone)]
    struct Fake {
        tools: Arc<Mutex<Vec<Tool>>>,
        modern_only: bool,
    }

    fn text(s: impl Into<String>) -> Result<CallToolResponse, ErrorData> {
        Ok(CallToolResult::success(vec![ContentBlock::text(s.into())]).into())
    }

    impl ServerHandler for Fake {
        fn get_info(&self) -> ServerConfig {
            ServerConfig::new(
                ServerCapabilities::builder()
                    .enable_tools()
                    .enable_tool_list_changed()
                    .build(),
            )
            .with_server_info(Implementation::new("fake", "1.0"))
        }

        fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
            if self.modern_only {
                Cow::Owned(vec![ProtocolVersion::V_2026_07_28])
            } else {
                Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)
            }
        }

        async fn list_tools(
            &self,
            _request: Option<PaginatedRequestParams>,
            _context: RequestContext<RoleServer>,
        ) -> Result<ListToolsResult, ErrorData> {
            let tools = self.tools.lock().expect("tools").clone();
            Ok(ListToolsResult::with_all_items(tools))
        }

        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            let args = request.arguments.clone().unwrap_or_default();
            match request.name.as_ref() {
                "echo" => text(args.get("text").and_then(Value::as_str).unwrap_or_default()),
                "env" => {
                    let env: BTreeMap<String, String> = std::env::vars().collect();
                    text(serde_json::to_string(&env).unwrap_or_default())
                }
                "pid" => text(std::process::id().to_string()),
                "slow" => {
                    let seconds = args.get("seconds").and_then(Value::as_f64).unwrap_or(30.0);
                    tokio::time::sleep(Duration::from_secs_f64(seconds)).await;
                    text("done")
                }
                "add_tool" => {
                    self.tools.lock().expect("tools").push(tool(
                        "extra",
                        "Added later",
                        json!({"type": "object"}),
                    ));
                    let _ = context.peer.notify_tool_list_changed().await;
                    text("added")
                }
                "big" => text("x".repeat(2 * 1024 * 1024)),
                "image" => Ok(CallToolResult::success(vec![ContentBlock::image(
                    "iVBORw0KGgo=",
                    "image/png",
                )])
                .into()),
                "fail" => Ok(CallToolResult::error(vec![ContentBlock::text("it broke")]).into()),
                "crash" => {
                    eprintln!("fake server: crashing");
                    std::process::exit(7);
                }
                "progress" => {
                    if let Some(token) = context.meta.get_progress_token() {
                        for step in 1..=3 {
                            let _ = context
                                .peer
                                .notify_progress(
                                    ProgressNotificationParam::new(token.clone(), f64::from(step))
                                        .with_total(3.0)
                                        .with_message(format!("step {step}")),
                                )
                                .await;
                        }
                    }
                    text("progressed")
                }
                "ask" => match &request.input_responses {
                    Some(responses) => {
                        let action = responses
                            .get("q")
                            .and_then(|r| r.get("action"))
                            .and_then(Value::as_str)
                            .unwrap_or("missing")
                            .to_owned();
                        let roots = responses
                            .get("r")
                            .and_then(|r| r.get("roots"))
                            .and_then(Value::as_array)
                            .map_or(usize::MAX, Vec::len);
                        text(format!("elicitation: {action}; roots: {roots}"))
                    }
                    None => {
                        let elicit =
                            ElicitRequest::new(ElicitRequestParams::FormElicitationParams {
                                meta: None,
                                message: "Your name?".into(),
                                requested_schema: ElicitationSchema::builder().build_unchecked(),
                            });
                        let requests = BTreeMap::from([
                            ("q".to_owned(), InputRequest::Elicitation(elicit)),
                            (
                                "r".to_owned(),
                                InputRequest::ListRoots(ListRootsRequest::default()),
                            ),
                        ]);
                        Ok(InputRequiredResult::new(Some(requests), None).into())
                    }
                },
                other => Err(ErrorData::invalid_params(format!("no tool {other}"), None)),
            }
        }
    }
}
