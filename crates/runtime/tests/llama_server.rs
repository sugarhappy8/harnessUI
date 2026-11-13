//! Integration tests against a real llama-server (the pinned build) and a tiny model.
//!
//! They read `LLAMA_SERVER_BIN` (the `llama-server` executable) and `TEST_GGUF` (a small
//! GGUF model with tool-call support, such as Qwen3 0.6B) and skip with a note when either
//! is unset, so a machine without them still passes. CI sets both on Windows and macOS.
//!
//! Every server here runs on the processor, so the 0.0.6 load settings are checked for
//! what the processor can show: Save memory allocates a q8_0 memory, Keep the model in
//! memory starts (and locks the model where the system allows it), As much as fits starts
//! with `--fit on` and keeps the explicit context, the alias is the model id in every API
//! while `/props` still names the launched file, a model that refuses Save memory is
//! retried once without it, and a plain lack of memory without a graphics card in use is
//! not retried at all. What a graphics card does with them is on the owner checklist.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtime::llama::launch::build_env_on;
use runtime::llama::protocol::CHILD_STATE_PREFIX;
use runtime::llama::server::loopback_client_builder;
use runtime::{
    Backend, BackendKind, GpuUse, LaunchConfig, LoadFailure, LoadStage, MODEL_ALIAS, RuntimeLayout,
    ServerEvent, ServerState, SpawnSpec, StartError, StartSpec, StdioMode, build_args, discover,
    free_loopback_port, probe, process_alive, random_api_key, start,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

const LOAD_TIMEOUT: Duration = Duration::from_secs(180);

fn env_paths(test: &str) -> Option<(PathBuf, PathBuf)> {
    let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
    let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
    match (bin, gguf) {
        (Some(bin), Some(gguf)) => Some((PathBuf::from(bin), PathBuf::from(gguf))),
        _ => {
            eprintln!("skipping {test}: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            None
        }
    }
}

/// The CPU backend of the directory holding `LLAMA_SERVER_BIN`, found the way the app
/// finds a development runtime.
fn backend(bin: &Path, kind: BackendKind) -> Backend {
    let dir = bin
        .parent()
        .expect("LLAMA_SERVER_BIN has a parent directory")
        .to_path_buf();
    let layout = RuntimeLayout {
        installed_dir: None,
        packs_dir: dir.join("no-packs-here"),
        dev_override: Some(dir),
    };
    discover(&layout)
        .into_iter()
        .find(|backend| backend.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} backend next to {}", bin.display()))
}

/// What the app launches on the processor: no device and `-ngl 0`.
fn launch(model: PathBuf) -> LaunchConfig {
    LaunchConfig {
        model,
        mmproj: None,
        ctx: 2048,
        device: None,
        gpu: GpuUse::Off,
        cache_ram_mib: 128,
        reasoning_budget: None,
        port: free_loopback_port().expect("free port"),
        api_key: random_api_key(),
        hip_visible_devices: None,
        ..LaunchConfig::default()
    }
}

fn drain(rx: &mut UnboundedReceiver<ServerEvent>) -> Vec<ServerEvent> {
    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    events
}

async fn wait_gone(pid: u32) -> bool {
    for _ in 0..100 {
        if !process_alive(pid) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
async fn starts_on_cpu_reports_progress_and_stops() {
    let Some((bin, gguf)) = env_paths("starts_on_cpu_reports_progress_and_stops") else {
        return;
    };
    let (tx, mut rx) = unbounded_channel();
    let spec = StartSpec {
        backend: backend(&bin, BackendKind::Cpu),
        launch: launch(gguf.clone()),
        load_timeout: LOAD_TIMEOUT,
    };
    let port = spec.launch.port;
    let server = match start(spec, tx).await {
        Ok(server) => server,
        Err(error) => panic!("start failed: {error}\n{}", error.stderr_tail().join("\n")),
    };

    assert_eq!(
        server.props().n_ctx,
        2048,
        "the explicit -c is what the server allocated"
    );
    assert!(server.props().chat_template_caps.supports_tool_calls);
    assert_eq!(server.base_url(), format!("http://127.0.0.1:{port}"));
    let info = server.log_info();
    assert!(info.model_loaded, "the log said 'model loaded'");
    assert!(
        info.listening_url
            .as_deref()
            .is_some_and(|url| url.ends_with(&format!(":{port}")))
    );

    let events = drain(&mut rx);
    assert_eq!(
        events.first(),
        Some(&ServerEvent::State(ServerState::Starting))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ServerEvent::Progress { value: Some(_), .. })),
        "no progress event"
    );
    assert_eq!(
        events.iter().rfind(|e| matches!(e, ServerEvent::State(_))),
        Some(&ServerEvent::State(ServerState::Ready))
    );

    // The key is enforced; /health is open.
    let client = loopback_client_builder().build().expect("client");
    let props = client
        .get(format!("{}/props", server.base_url()))
        .send()
        .await
        .expect("props");
    assert_eq!(props.status(), 401);
    let health = client
        .get(format!("{}/health", server.base_url()))
        .send()
        .await
        .expect("health");
    assert_eq!(health.status(), 200);
    let authed = client
        .get(format!("{}/props", server.base_url()))
        .bearer_auth(server.api_key())
        .send()
        .await
        .expect("props");
    assert_eq!(authed.status(), 200);

    // The key travels in the environment only: never in the log or in Debug output.
    let key = server.api_key().to_owned();
    assert!(
        !server.stderr_tail().iter().any(|line| line.contains(&key)),
        "the API key appeared in llama-server's log"
    );
    assert!(!format!("{server:?}").contains(&key));

    let pid = server.pid().expect("pid");
    assert!(process_alive(pid));
    server.stop().await;
    assert!(
        wait_gone(pid).await,
        "llama-server {pid} still running after stop"
    );
    let events: Vec<ServerEvent> = drain(&mut rx)
        .into_iter()
        .filter(|e| !matches!(e, ServerEvent::Log(_)))
        .collect();
    assert_eq!(
        events.first(),
        Some(&ServerEvent::State(ServerState::Stopping))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ServerEvent::Exited { .. }))
    );
    assert_eq!(
        events.last(),
        Some(&ServerEvent::State(ServerState::Stopped))
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_broken_model_file_is_classified() {
    let Some((bin, _)) = env_paths("a_broken_model_file_is_classified") else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let broken = dir.path().join("broken.gguf");
    std::fs::write(&broken, vec![0x5a_u8; 64 * 1024]).expect("write");
    for model in [broken, dir.path().join("missing.gguf")] {
        let (tx, mut rx) = unbounded_channel();
        let spec = StartSpec {
            backend: backend(&bin, BackendKind::Cpu),
            launch: launch(model.clone()),
            load_timeout: Duration::from_secs(60),
        };
        let started = tokio::time::Instant::now();
        let error = start(spec, tx)
            .await
            .expect_err("a broken model cannot load");
        assert!(started.elapsed() < Duration::from_secs(60));
        match &error {
            StartError::Exited {
                failure,
                exit_code,
                stderr_tail,
            } => {
                assert_eq!(
                    *failure,
                    LoadFailure::ModelFileInvalid,
                    "{}: {stderr_tail:#?}",
                    model.display()
                );
                assert_eq!(*exit_code, Some(1));
                assert!(!stderr_tail.is_empty());
            }
            other => panic!("unexpected error for {}: {other:?}", model.display()),
        }
        assert_eq!(
            drain(&mut rx)
                .iter()
                .rfind(|e| matches!(e, ServerEvent::State(_))),
            Some(&ServerEvent::State(ServerState::Failed(
                LoadFailure::ModelFileInvalid
            )))
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cpu_probe_reports_the_synthetic_cpu_device() {
    let Some((bin, _)) = env_paths("cpu_probe_reports_the_synthetic_cpu_device") else {
        return;
    };
    // The first start of a new binary on macOS is slow (the system scans it), so the
    // probe gets the same allowance as the app's.
    let result = probe(&backend(&bin, BackendKind::Cpu), Duration::from_secs(45)).await;
    assert_eq!(result.error, None);
    assert_eq!(result.backend_kind, BackendKind::Cpu);
    assert_eq!(result.devices, vec![runtime::llama::devices::cpu_device()]);

    if cfg!(target_os = "linux") {
        // The Linux dev build is CPU only: asking it for Vulkan finds no device.
        let mut vulkan = backend(&bin, BackendKind::Cpu);
        vulkan.kind = BackendKind::Vulkan;
        let result = probe(&vulkan, Duration::from_secs(15)).await;
        assert_eq!(
            result.error,
            Some(runtime::ProbeError::NoMatchingDevice {
                backend: BackendKind::Vulkan
            })
        );
        assert!(result.devices.is_empty());
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_taken_port_is_reported_as_port_in_use() {
    let Some((bin, gguf)) = env_paths("a_taken_port_is_reported_as_port_in_use") else {
        return;
    };
    // Unix only: on Windows the server sets SO_REUSEADDR and may bind a taken port anyway.
    let taken = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
    let mut config = launch(gguf);
    config.port = taken.local_addr().expect("addr").port();
    let (tx, _rx) = unbounded_channel();
    let spec = StartSpec {
        backend: backend(&bin, BackendKind::Cpu),
        launch: config,
        load_timeout: Duration::from_secs(60),
    };
    let error = start(spec, tx).await.expect_err("port is taken");
    assert_eq!(error.failure(), LoadFailure::PortInUse, "{error:?}");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn supervisor_restarts_a_crashed_server_and_stops_on_cancel() {
    use std::sync::{Arc, Mutex};

    use runtime::Supervisor;
    use runtime::llama::server::{Candidate, SupervisorExit};
    use tokio_util::sync::CancellationToken;

    let Some((bin, gguf)) = env_paths("supervisor_restarts_a_crashed_server_and_stops_on_cancel")
    else {
        return;
    };
    let cpu = backend(&bin, BackendKind::Cpu);
    let candidates = vec![Candidate {
        backend: cpu.clone(),
        device: None,
        hip_visible_devices: None,
    }];
    let (tx, mut rx) = unbounded_channel();
    let supervisor = Supervisor::local(candidates, tx);
    let cancel = CancellationToken::new();
    let pids = Arc::new(Mutex::new(Vec::new()));

    let task = {
        let cancel = cancel.clone();
        let pids = pids.clone();
        let spec = StartSpec {
            backend: cpu,
            launch: launch(gguf),
            load_timeout: LOAD_TIMEOUT,
        };
        tokio::spawn(async move {
            supervisor
                .run(spec, cancel.clone(), move |server, _spec| {
                    let mut pids = pids.lock().expect("pids");
                    pids.push(server.pid().expect("pid"));
                    if pids.len() == 1 {
                        // Simulate a crash of the first server.
                        let pid = libc::pid_t::try_from(pids[0]).expect("pid fits");
                        // SAFETY: plain signal to the child we just started.
                        unsafe { libc::kill(pid, libc::SIGKILL) };
                    } else {
                        cancel.cancel();
                    }
                })
                .await
        })
    };
    let exit = tokio::time::timeout(Duration::from_secs(300), task)
        .await
        .expect("supervisor finishes")
        .expect("join");
    assert_eq!(exit, SupervisorExit::Stopped);
    let pids = pids.lock().expect("pids").clone();
    assert_eq!(pids.len(), 2, "one restart");
    for pid in pids {
        assert!(wait_gone(pid).await, "llama-server {pid} still running");
    }
    let events = drain(&mut rx);
    let restart = events
        .iter()
        .position(|e| *e == ServerEvent::Restarting { attempt: 1, max: 3 })
        .expect("the restart is reported");
    let exited = events
        .iter()
        .position(|e| matches!(e, ServerEvent::Exited { .. }))
        .expect("the crash is reported");
    assert!(
        exited < restart,
        "the crashed server's exit comes before the restart"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ServerEvent::State(ServerState::Failed(_)))),
        "a restarted server is not a failed load"
    );
    assert_eq!(
        events.last(),
        Some(&ServerEvent::State(ServerState::Stopped))
    );
}

/// Starts a server for `config` on the processor with [`start`], the app's start path
/// (which also checks that `/props` names the launched file), or fails with its log.
async fn start_cpu(bin: &Path, config: LaunchConfig) -> runtime::RunningServer {
    let (tx, _rx) = unbounded_channel();
    let spec = StartSpec {
        backend: backend(bin, BackendKind::Cpu),
        launch: config,
        load_timeout: LOAD_TIMEOUT,
    };
    match start(spec, tx).await {
        Ok(server) => server,
        Err(error) => panic!("start failed: {error}\n{}", error.stderr_tail().join("\n")),
    }
}

/// Starts llama-server with exactly the app's arguments and environment for `config`,
/// returns the payload of its managed-child `ready` line and stops it.
///
/// That line carries what [`start`] does not keep: the model id the server reports and the
/// memory it allocated per device (`memory.devices[].context` is the conversation memory
/// in bytes).
async fn ready_payload(bin: &Path, config: &LaunchConfig) -> Value {
    let backend = backend(bin, BackendKind::Cpu);
    let spec = SpawnSpec::new(&backend.server, &backend.dir)
        .args(build_args(config))
        .env(build_env_on(backend.base_env(), config))
        .stdin(StdioMode::Piped)
        .stdout(StdioMode::Piped)
        .stderr(StdioMode::Piped);
    let mut child = runtime::spawn(spec).expect("spawn llama-server");
    // Managed-child mode exits when stdin closes: keep it open until the end.
    let _stdin = child.take_stdin();
    let stderr = child.take_stderr().expect("stderr");
    let log = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).lines();
        let mut kept = Vec::new();
        while let Ok(Some(line)) = lines.next_line().await {
            kept.push(line);
        }
        kept
    });
    let mut lines = BufReader::new(child.take_stdout().expect("stdout")).lines();
    let payload = tokio::time::timeout(LOAD_TIMEOUT, async {
        while let Ok(Some(line)) = lines.next_line().await {
            let Some(json) = line.strip_prefix(CHILD_STATE_PREFIX) else {
                continue;
            };
            let value: Value = serde_json::from_str(json).expect("state line is JSON");
            if value["state"] == "ready" {
                return Some(value["payload"].clone());
            }
        }
        None
    })
    .await
    .expect("ready in time");
    let _ = child.kill().await;
    let log = log.await.unwrap_or_default();
    payload.unwrap_or_else(|| panic!("no ready line; log:\n{}", log.join("\n")))
}

/// The conversation memory the server allocated, in bytes, over all devices.
fn context_bytes(payload: &Value) -> u64 {
    payload["memory"]["devices"]
        .as_array()
        .expect("memory.devices")
        .iter()
        .map(|device| device["context"].as_u64().unwrap_or(0))
        .sum()
}

/// A short request to `path` with the server's key; returns the JSON reply.
async fn post(server: &runtime::RunningServer, path: &str, body: Value) -> Value {
    let client = loopback_client_builder().build().expect("client");
    let reply = client
        .post(format!("{}{path}", server.base_url()))
        .bearer_auth(server.api_key())
        .json(&body)
        .send()
        .await
        .expect("request");
    assert_eq!(reply.status(), 200, "{path}");
    reply.json().await.expect("JSON reply")
}

fn chat_request(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": "Say hello. /no_think"}],
        "max_tokens": 4,
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn save_memory_starts_and_allocates_a_q8_0_memory() {
    let Some((bin, gguf)) = env_paths("save_memory_starts_and_allocates_a_q8_0_memory") else {
        return;
    };
    let save = |config: LaunchConfig| LaunchConfig {
        save_memory: true,
        ..config
    };
    let args = build_args(&save(launch(gguf.clone())));
    assert!(
        args.windows(6)
            .any(|w| w == ["-fa", "on", "-ctk", "q8_0", "-ctv", "q8_0"]),
        "{args:?}"
    );

    // The app's start path accepts it and the model answers with flash attention on.
    let server = start_cpu(&bin, save(launch(gguf.clone()))).await;
    assert_eq!(server.props().n_ctx, 2048);
    let reply = post(&server, "/v1/chat/completions", chat_request("x")).await;
    assert!(reply["choices"][0]["message"].is_object(), "{reply}");
    server.stop().await;

    // The memory it allocated: q8_0 takes 34 bytes where f16 takes 64, plus a little
    // padding (b12110 with Qwen3 0.6B at 2,048 tokens: 127,074,304 against 234,881,024
    // bytes, 54.1 %).
    let plain = context_bytes(&ready_payload(&bin, &launch(gguf.clone())).await);
    let saved = context_bytes(&ready_payload(&bin, &save(launch(gguf))).await);
    assert!(plain > 0, "the ready line reports the conversation memory");
    assert!(
        saved * 100 >= plain * 53 && saved * 100 <= plain * 56,
        "Save memory should allocate a q8_0 memory: {saved} bytes against {plain} bytes of f16"
    );
}

/// Whether this process may lock `bytes` of memory: the `CAP_IPC_LOCK` capability (root
/// usually has it) or a large enough `RLIMIT_MEMLOCK`.
#[cfg(target_os = "linux")]
fn may_lock(bytes: u64) -> bool {
    const CAP_IPC_LOCK: u32 = 14;
    let capable = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("CapEff:"))
                .and_then(|hex| u64::from_str_radix(hex.trim(), 16).ok())
        })
        .is_some_and(|caps| caps & (1 << CAP_IPC_LOCK) != 0);
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: getrlimit writes one struct we own.
    let limited = unsafe { libc::getrlimit(libc::RLIMIT_MEMLOCK, &raw mut limit) } == 0
        && limit.rlim_cur != libc::RLIM_INFINITY
        && limit.rlim_cur < bytes;
    capable || !limited
}

/// Locked memory of a process in KiB (`VmLck`).
#[cfg(target_os = "linux")]
fn locked_kib(pid: u32) -> u64 {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .expect("status")
        .lines()
        .find_map(|line| line.strip_prefix("VmLck:"))
        .and_then(|rest| rest.trim().trim_end_matches("kB").trim().parse().ok())
        .expect("VmLck")
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_in_memory_starts_and_locks_the_model_where_allowed() {
    let Some((bin, gguf)) = env_paths("keep_in_memory_starts_and_locks_the_model_where_allowed")
    else {
        return;
    };
    let keep = LaunchConfig {
        keep_in_memory: true,
        ..launch(gguf.clone())
    };
    let args = build_args(&keep);
    assert!(
        args.windows(2).any(|w| w == ["-lm", "mmap+mlock"]),
        "{args:?}"
    );
    let server = start_cpu(&bin, keep).await;
    assert_eq!(server.props().n_ctx, 2048);
    let reply = post(&server, "/v1/chat/completions", chat_request("x")).await;
    assert!(reply["choices"][0]["message"].is_object(), "{reply}");
    #[cfg(target_os = "linux")]
    {
        let model_bytes = std::fs::metadata(&gguf).expect("model").len();
        let locked = locked_kib(server.pid().expect("pid"));
        if may_lock(model_bytes) {
            assert!(
                locked * 1024 >= model_bytes * 9 / 10,
                "only {locked} KiB locked of a {model_bytes}-byte model"
            );
        } else {
            eprintln!(
                "note: this system's mlock limit is below the model size, so this test proves \
                 that the flag is accepted, not that memory is locked ({locked} KiB locked)"
            );
        }
    }
    server.stop().await;

    // Without it, nothing is locked.
    #[cfg(target_os = "linux")]
    {
        let server = start_cpu(&bin, launch(gguf)).await;
        assert_eq!(locked_kib(server.pid().expect("pid")), 0);
        server.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn as_much_as_fits_starts_with_fit_on_and_keeps_the_context() {
    let Some((bin, gguf)) = env_paths("as_much_as_fits_starts_with_fit_on_and_keeps_the_context")
    else {
        return;
    };
    let fit = LaunchConfig {
        gpu: GpuUse::Fit,
        ..launch(gguf)
    };
    let args = build_args(&fit);
    assert!(args.windows(2).any(|w| w == ["--fit", "on"]), "{args:?}");
    assert!(!args.iter().any(|a| a == "-ngl"), "{args:?}");
    let server = start_cpu(&bin, fit).await;
    assert_eq!(
        server.props().n_ctx,
        2048,
        "fitting changes only the layers, never an explicit -c"
    );
    let reply = post(&server, "/v1/chat/completions", chat_request("x")).await;
    assert!(reply["choices"][0]["message"].is_object(), "{reply}");
    server.stop().await;
}

/// All of it (`-ngl all --fit off`), the default on a graphics card, is accepted by the
/// pinned build. Here there is no graphics card (`--device none`), so this proves the words
/// parse and the context stays as given; what a card does with them is on the owner
/// checklist.
#[tokio::test(flavor = "multi_thread")]
async fn all_of_it_starts_with_fit_off() {
    let Some((bin, gguf)) = env_paths("all_of_it_starts_with_fit_off") else {
        return;
    };
    let all = LaunchConfig {
        gpu: GpuUse::All,
        ..launch(gguf)
    };
    let args = build_args(&all);
    assert!(
        args.windows(4)
            .any(|w| w == ["-ngl", "all", "--fit", "off"]),
        "{args:?}"
    );
    let server = start_cpu(&bin, all).await;
    assert_eq!(server.props().n_ctx, 2048);
    let reply = post(&server, "/v1/chat/completions", chat_request("x")).await;
    assert!(reply["choices"][0]["message"].is_object(), "{reply}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_alias_is_the_model_id_in_every_api() {
    let Some((bin, gguf)) = env_paths("the_alias_is_the_model_id_in_every_api") else {
        return;
    };
    let file_name = gguf
        .file_name()
        .expect("file name")
        .to_string_lossy()
        .into_owned();
    // `start` succeeding means the start-up check passed: /props names the launched file.
    let server = start_cpu(&bin, launch(gguf.clone())).await;
    let client = loopback_client_builder().build().expect("client");

    let models = client
        .get(format!("{}/v1/models", server.base_url()))
        .bearer_auth(server.api_key())
        .send()
        .await
        .expect("models")
        .text()
        .await
        .expect("body");
    let parsed: Value = serde_json::from_str(&models).expect("JSON");
    let ids: Vec<&str> = parsed["data"]
        .as_array()
        .expect("data")
        .iter()
        .filter_map(|m| m["id"].as_str())
        .collect();
    assert_eq!(ids, vec![MODEL_ALIAS]);
    assert!(
        !models.contains(&file_name),
        "the model list names the file: {models}"
    );

    // Whatever model a request names, replies carry the alias.
    let chat = post(&server, "/v1/chat/completions", chat_request("any-name")).await;
    assert_eq!(chat["model"], MODEL_ALIAS, "{chat}");
    let stream = client
        .post(format!("{}/v1/chat/completions", server.base_url()))
        .bearer_auth(server.api_key())
        .json(&json!({
            "model": "any-name",
            "messages": [{"role": "user", "content": "Say hello. /no_think"}],
            "max_tokens": 4,
            "stream": true,
        }))
        .send()
        .await
        .expect("stream")
        .text()
        .await
        .expect("events");
    let chunks: Vec<Value> = stream
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter(|data| *data != "[DONE]")
        .map(|data| serde_json::from_str(data).expect("chunk JSON"))
        .collect();
    assert!(!chunks.is_empty(), "{stream}");
    assert!(chunks.iter().all(|c| c["model"] == MODEL_ALIAS), "{stream}");
    let messages = client
        .post(format!("{}/v1/messages", server.base_url()))
        .header("x-api-key", server.api_key())
        .json(&json!({
            "model": "any-name",
            "max_tokens": 4,
            "messages": [{"role": "user", "content": "Say hello. /no_think"}],
        }))
        .send()
        .await
        .expect("messages");
    assert_eq!(messages.status(), 200);
    let messages: Value = messages.json().await.expect("JSON");
    assert_eq!(messages["model"], MODEL_ALIAS, "{messages}");

    // /props still names the file the app launched, and the alias beside it.
    let props: Value = client
        .get(format!("{}/props", server.base_url()))
        .bearer_auth(server.api_key())
        .send()
        .await
        .expect("props")
        .json()
        .await
        .expect("JSON");
    assert_eq!(
        props["model_path"].as_str().map(PathBuf::from),
        Some(gguf.clone())
    );
    assert_eq!(props["model_alias"], MODEL_ALIAS);
    server.stop().await;

    // The managed-child ready line reports the same id.
    let ready = ready_payload(&bin, &launch(gguf)).await;
    assert_eq!(ready["id"], MODEL_ALIAS, "{ready}");
}

/// Writes a tiny llama-architecture GGUF model whose attention heads are 80 values wide (two
/// heads of a 160-wide layer, one layer, a byte-level vocabulary of 259 tokens, all weights
/// zero, about 700 KB) to `path`.
///
/// Families with that head size exist; the pinned build refuses Save memory for them on
/// every backend, because a q8_0 block of 32 values does not divide 80 ("K cache type q8_0
/// with block size 32 does not divide n_embd_head_k=80"), and loads them without it.
fn write_head_size_80_model(path: &Path) {
    const EMBD: u64 = 160;
    const FF: u64 = 64;
    fn text(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u64).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }
    // GGUF value types.
    const U32: u32 = 4;
    const I32: u32 = 5;
    const F32: u32 = 6;
    const STRING: u32 = 8;
    const ARRAY: u32 = 9;
    let mut kvs: Vec<Vec<u8>> = Vec::new();
    let mut kv = |key: &str, kind: u32, value: Vec<u8>| {
        let mut out = Vec::new();
        text(&mut out, key);
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&value);
        kvs.push(out);
    };
    let string = |s: &str| {
        let mut out = Vec::new();
        text(&mut out, s);
        out
    };
    let u32_value = |n: u32| n.to_le_bytes().to_vec();
    let array = |kind: u32, items: Vec<Vec<u8>>| {
        let mut out = kind.to_le_bytes().to_vec();
        out.extend_from_slice(&(items.len() as u64).to_le_bytes());
        out.extend(items.into_iter().flatten());
        out
    };
    kv("general.architecture", STRING, string("llama"));
    kv("llama.context_length", U32, u32_value(2048));
    kv("llama.embedding_length", U32, u32_value(EMBD as u32));
    kv("llama.block_count", U32, u32_value(1));
    kv("llama.feed_forward_length", U32, u32_value(FF as u32));
    kv("llama.attention.head_count", U32, u32_value(2));
    kv("llama.attention.head_count_kv", U32, u32_value(2));
    kv(
        "llama.attention.layer_norm_rms_epsilon",
        F32,
        1e-5_f32.to_le_bytes().to_vec(),
    );
    let tokens: Vec<String> = ["<unk>", "<s>", "</s>"]
        .into_iter()
        .map(str::to_owned)
        .chain((0..=255_u8).map(|b| format!("<0x{b:02X}>")))
        .collect();
    // Unknown, control, control, then byte tokens.
    let types: Vec<i32> = [2, 3, 3].into_iter().chain([6; 256]).collect();
    let vocab = tokens.len() as u64;
    kv("tokenizer.ggml.model", STRING, string("llama"));
    kv(
        "tokenizer.ggml.tokens",
        ARRAY,
        array(STRING, tokens.iter().map(|t| string(t)).collect()),
    );
    kv(
        "tokenizer.ggml.scores",
        ARRAY,
        array(
            F32,
            tokens
                .iter()
                .map(|_| 0_f32.to_le_bytes().to_vec())
                .collect(),
        ),
    );
    kv(
        "tokenizer.ggml.token_type",
        ARRAY,
        array(
            I32,
            types.iter().map(|t| t.to_le_bytes().to_vec()).collect(),
        ),
    );
    kv("tokenizer.ggml.unknown_token_id", U32, u32_value(0));
    kv("tokenizer.ggml.bos_token_id", U32, u32_value(1));
    kv("tokenizer.ggml.eos_token_id", U32, u32_value(2));

    // (name, dimensions with the fastest first, value of every element), all f32.
    let mut tensors: Vec<(String, Vec<u64>, f32)> = vec![
        ("token_embd.weight".into(), vec![EMBD, vocab], 0.0),
        ("output_norm.weight".into(), vec![EMBD], 1.0),
    ];
    for (name, dims, value) in [
        ("attn_norm", vec![EMBD], 1.0),
        ("attn_q", vec![EMBD, EMBD], 0.0),
        ("attn_k", vec![EMBD, EMBD], 0.0),
        ("attn_v", vec![EMBD, EMBD], 0.0),
        ("attn_output", vec![EMBD, EMBD], 0.0),
        ("ffn_norm", vec![EMBD], 1.0),
        ("ffn_gate", vec![EMBD, FF], 0.0),
        ("ffn_down", vec![FF, EMBD], 0.0),
        ("ffn_up", vec![EMBD, FF], 0.0),
    ] {
        tensors.push((format!("blk.0.{name}.weight"), dims, value));
    }
    const ALIGN: usize = 32;
    let pad = |buf: &mut Vec<u8>| buf.resize(buf.len().div_ceil(ALIGN) * ALIGN, 0);
    let (mut infos, mut data) = (Vec::new(), Vec::new());
    for (name, dims, value) in &tensors {
        pad(&mut data);
        text(&mut infos, name);
        infos.extend_from_slice(&(dims.len() as u32).to_le_bytes());
        for dim in dims {
            infos.extend_from_slice(&dim.to_le_bytes());
        }
        infos.extend_from_slice(&0_u32.to_le_bytes()); // GGML_TYPE_F32
        infos.extend_from_slice(&(data.len() as u64).to_le_bytes());
        let count: u64 = dims.iter().product();
        for _ in 0..count {
            data.extend_from_slice(&value.to_le_bytes());
        }
    }
    let mut file = b"GGUF".to_vec();
    file.extend_from_slice(&3_u32.to_le_bytes());
    file.extend_from_slice(&(tensors.len() as u64).to_le_bytes());
    file.extend_from_slice(&(kvs.len() as u64).to_le_bytes());
    file.extend(kvs.into_iter().flatten());
    file.extend_from_slice(&infos);
    pad(&mut file);
    file.extend_from_slice(&data);
    std::fs::write(path, file).expect("write the test model");
}

/// The lifecycle states and typed stages, in the order the app receives them.
fn states_and_stages(events: &[ServerEvent]) -> Vec<&ServerEvent> {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                ServerEvent::State(_) | ServerEvent::Stage(_) | ServerEvent::SteppedDown { .. }
            )
        })
        .collect()
}

/// The log lines of a run, for a failure message.
fn log_lines(events: &[ServerEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            ServerEvent::Log(line) => Some(line.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A model that refuses Save memory (0.0.6 D2c), through real llama-server processes and
/// the real supervisor as the app runs it ([`runtime::Supervisor::run`]): the first start
/// fails with the pinned build's refusal line, the supervisor retries once without Save
/// memory on the same backend with a typed event, and the model loads. On the processor
/// this is the only retry without Save memory; it is decided by the log, not by the
/// backend.
///
/// The events are what the app gets for every retry on the same backend: `Starting`, the
/// stage, the new attempt's `Starting`, `Ready` (then `Stopping`, `Stopped` for the stop).
/// The attempt that failed reports no `Failed`: the load is not over, so the app keeps
/// showing it as loading, with the stage.
///
/// The stage names where the model runs. On Linux the same processor build also runs
/// labelled Vulkan with Don't use it (`--device none`), the owner's setup when a model
/// is set to Don't use it: the stage still names the processor, so the app says the model
/// does not take Save memory instead of blaming a graphics card.
#[tokio::test(flavor = "multi_thread")]
async fn a_model_that_refuses_save_memory_is_retried_once_without_it() {
    use runtime::Supervisor;
    use runtime::llama::server::{Candidate, SupervisorExit};
    use tokio_util::sync::CancellationToken;

    let Some((bin, _)) = env_paths("a_model_that_refuses_save_memory_is_retried_once_without_it")
    else {
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    let model = dir.path().join("head80.gguf");
    write_head_size_80_model(&model);
    let cpu = backend(&bin, BackendKind::Cpu);
    let mut cases = vec![(cpu.clone(), None)];
    if cfg!(target_os = "linux") {
        let mut graphics = cpu;
        graphics.kind = BackendKind::Vulkan;
        cases.push((graphics, Some("Vulkan0".to_owned())));
    }
    for (first, device) in cases {
        let kind = first.kind;
        let (tx, mut rx) = unbounded_channel();
        let supervisor = Supervisor::local(
            vec![Candidate {
                backend: first.clone(),
                device: device.clone(),
                hip_visible_devices: None,
            }],
            tx,
        );
        let spec = StartSpec {
            backend: first,
            launch: LaunchConfig {
                save_memory: true,
                device,
                ..launch(model.clone())
            },
            load_timeout: LOAD_TIMEOUT,
        };
        let cancel = CancellationToken::new();
        let mut loaded = None;
        let exit = {
            let stop = cancel.clone();
            supervisor
                .run(spec, cancel, |server, running| {
                    loaded = Some((
                        running.backend.kind,
                        running.launch.save_memory,
                        server.props().n_ctx,
                    ));
                    stop.cancel();
                })
                .await
        };
        let events = drain(&mut rx);
        assert_eq!(
            exit,
            SupervisorExit::Stopped,
            "{kind}: {}",
            log_lines(&events)
        );
        assert_eq!(
            loaded,
            Some((kind, false, 2048)),
            "{kind}: the load runs without Save memory on the same backend"
        );
        assert!(
            log_lines(&events).contains("does not divide n_embd_head_k=80"),
            "{kind}: the pinned build's refusal line"
        );
        assert_eq!(
            states_and_stages(&events),
            vec![
                &ServerEvent::State(ServerState::Starting),
                &ServerEvent::Stage(LoadStage::RetryingWithoutSaveMemory {
                    backend: BackendKind::Cpu
                }),
                &ServerEvent::State(ServerState::Starting),
                &ServerEvent::State(ServerState::Ready),
                &ServerEvent::State(ServerState::Stopping),
                &ServerEvent::State(ServerState::Stopped),
            ],
            "{kind}"
        );
    }
}

/// A context far too large for any computer fails to allocate, through real llama-server
/// processes and the real supervisor. Without a graphics card in use that is a plain lack
/// of memory: no retry without Save memory (it would need more), no step-down (the
/// processor build would repeat the same load), one start and the failure, reported once
/// as the app runs it ([`runtime::Supervisor::run`]). That holds on
/// the processor and on a graphics backend with Don't use it, so the backend's label
/// changes nothing; on this Linux build the "graphics" backend is the same processor build
/// labelled Vulkan, which with Don't use it runs as `--device none`, exactly as on a real
/// one. What a real graphics card does is covered by the supervisor's unit tests and the
/// owner checklist.
///
/// Linux only, and only while the kernel refuses impossible allocations
/// (`vm.overcommit_memory` is not 1): elsewhere the allocation of several terabytes might
/// be granted lazily and then touched.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_memory_failure_without_a_graphics_card_is_not_retried() {
    use runtime::Supervisor;
    use runtime::llama::server::{Candidate, SupervisorExit};
    use tokio_util::sync::CancellationToken;

    let Some((bin, gguf)) =
        env_paths("a_plain_memory_failure_without_a_graphics_card_is_not_retried")
    else {
        return;
    };
    let overcommit = std::fs::read_to_string("/proc/sys/vm/overcommit_memory").unwrap_or_default();
    if overcommit.trim() == "1" {
        eprintln!("skipping: vm.overcommit_memory is 1, so a huge allocation might be granted");
        return;
    }
    let cpu = backend(&bin, BackendKind::Cpu);
    let mut graphics = cpu.clone();
    graphics.kind = BackendKind::Vulkan;
    let candidate = |backend: &Backend, device: Option<&str>| Candidate {
        backend: backend.clone(),
        device: device.map(str::to_owned),
        hip_visible_devices: None,
    };
    for (first, device, candidates) in [
        (cpu.clone(), None, vec![candidate(&cpu, None)]),
        (
            graphics.clone(),
            Some("Vulkan0"),
            vec![candidate(&graphics, Some("Vulkan0")), candidate(&cpu, None)],
        ),
    ] {
        let kind = first.kind;
        let (tx, mut rx) = unbounded_channel();
        let supervisor = Supervisor::local(candidates, tx);
        let spec = StartSpec {
            backend: first,
            launch: LaunchConfig {
                ctx: 50_000_128,
                gpu: GpuUse::Off,
                device: device.map(str::to_owned),
                save_memory: true,
                ..launch(gguf.clone())
            },
            load_timeout: LOAD_TIMEOUT,
        };
        assert!(
            build_args(&spec.launch)
                .windows(2)
                .any(|w| w == ["--device", "none"]),
            "Don't use it names no device"
        );
        let exit = supervisor
            .run(spec, CancellationToken::new(), |_, _| {
                panic!("{kind}: a context of 50 million tokens loaded")
            })
            .await;
        let events = drain(&mut rx);
        assert_eq!(
            exit,
            SupervisorExit::Failed(LoadFailure::ContextTooLarge),
            "{kind}: {}",
            log_lines(&events)
        );
        assert_eq!(
            states_and_stages(&events),
            vec![
                &ServerEvent::State(ServerState::Starting),
                &ServerEvent::State(ServerState::Failed(LoadFailure::ContextTooLarge)),
            ],
            "{kind}: one start, no retry, no step-down, one failure"
        );
    }
}

/// Fine control words a processor can show (item 6): a server with `-t 2 -fa off`, and one
/// with `-fa on`, reaches Ready through the app's start path and answers. The expert flags
/// and `-nkvo` add nothing without a device (the launch leaves them out), so their server
/// start is on the owner checklist; fit-params accepts them (`tests/fit_params.rs`).
#[tokio::test(flavor = "multi_thread")]
async fn fine_control_flags_start() {
    use runtime::{ExpertsOnCpu, FineLaunch, FlashAttn};

    let Some((bin, gguf)) = env_paths("fine_control_flags_start") else {
        return;
    };
    let with = |fine: FineLaunch| LaunchConfig {
        fine,
        ..launch(gguf.clone())
    };
    let threads_off = with(FineLaunch {
        threads: Some(2),
        flash_attn: FlashAttn::Off,
        // Without a device these change nothing on the command line.
        experts_on_cpu: ExpertsOnCpu::All,
        kv_in_computer_memory: true,
        ..FineLaunch::default()
    });
    let args = build_args(&threads_off).join(" ");
    assert!(
        args.contains("--device none -ngl 0 --fit off -t 2 -fa off --cache-ram"),
        "{args}"
    );
    assert!(!args.contains("--cpu-moe") && !args.contains("-nkvo"), "{args}");
    let fa_on = with(FineLaunch {
        flash_attn: FlashAttn::On,
        ..FineLaunch::default()
    });
    assert!(
        build_args(&fa_on).join(" ").contains("--fit off -fa on --cache-ram"),
        "{:?}",
        build_args(&fa_on)
    );
    for config in [threads_off, fa_on] {
        let server = start_cpu(&bin, config).await;
        assert_eq!(server.props().n_ctx, 2048);
        let reply = post(&server, "/v1/chat/completions", chat_request("x")).await;
        assert!(reply["choices"][0]["message"].is_object(), "{reply}");
        server.stop().await;
    }
}

/// Where the model went (item 6, I6-7): the start path sends one memory report from the
/// managed-child ready line, in bytes, with the processor's rows (Host and the repacked
/// weights' buffer type), the allocated context and the conversation memory a 2,048-token
/// f16 memory takes.
#[tokio::test(flavor = "multi_thread")]
async fn ready_reports_where_the_model_went() {
    use runtime::MemoryRowKind;

    let Some((bin, gguf)) = env_paths("ready_reports_where_the_model_went") else {
        return;
    };
    let (tx, mut rx) = unbounded_channel();
    let spec = StartSpec {
        backend: backend(&bin, BackendKind::Cpu),
        launch: launch(gguf.clone()),
        load_timeout: LOAD_TIMEOUT,
    };
    let server = match start(spec, tx).await {
        Ok(server) => server,
        Err(error) => panic!("start failed: {error}\n{}", error.stderr_tail().join("\n")),
    };
    // The stdout reader may still be passing the line on.
    let mut events = drain(&mut rx);
    for _ in 0..50 {
        if events.iter().any(|e| matches!(e, ServerEvent::Memory(_))) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        events.extend(drain(&mut rx));
    }
    server.stop().await;
    events.extend(drain(&mut rx));
    let reports: Vec<&runtime::MemoryReport> = events
        .iter()
        .filter_map(|e| match e {
            ServerEvent::Memory(report) => Some(report),
            _ => None,
        })
        .collect();
    assert_eq!(reports.len(), 1, "one report per ready line: {reports:?}");
    let report = reports[0];
    eprintln!("note: the ready line's report: {report:#?}");
    let ready_at = events
        .iter()
        .position(|e| *e == ServerEvent::State(ServerState::Ready));
    let memory_at = events
        .iter()
        .position(|e| matches!(e, ServerEvent::Memory(_)));
    eprintln!("note: Ready at {ready_at:?}, the memory report at {memory_at:?}");
    assert_eq!(report.n_ctx, Some(2048));
    assert!(report.rows.len() <= 16);
    let host = report
        .rows
        .iter()
        .find(|r| r.kind == MemoryRowKind::Host)
        .expect("a Host row");
    assert_eq!(host.name, "Host");
    assert_eq!(host.description, "", "the ready line carries no description");
    assert!(
        report
            .rows
            .iter()
            .all(|r| r.kind != MemoryRowKind::Device && r.total_bytes.is_none()),
        "the processor has no device row: {report:?}"
    );
    let model_size = std::fs::metadata(&gguf).expect("model").len();
    let weights: u64 = report.rows.iter().map(|r| r.model_bytes).sum();
    assert!(
        weights >= model_size / 2 && weights <= model_size.saturating_mul(2),
        "{weights} bytes of weights for a {model_size}-byte file"
    );
    // 2,048 tokens of f16 conversation memory (234,881,024 bytes for this model on b12110,
    // see save_memory_starts_and_allocates_a_q8_0_memory).
    let context: u64 = report.rows.iter().map(|r| r.context_bytes).sum();
    assert!(context > 0, "{report:?}");
    assert!(report.rows.iter().all(|r| r.compute_bytes < (1 << 40)));
}
