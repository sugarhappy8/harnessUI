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
//! while `/props` still names the launched file, and a failure Save memory may cause is
//! retried once without it. What a graphics card does with them is on the owner checklist.

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
    assert!(events.contains(&ServerEvent::Restarting { attempt: 1, max: 3 }));
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

/// A context far too large for any computer fails to allocate, through real llama-server
/// processes and the real supervisor: on a graphics backend with Save memory on, it retries
/// once without it on the same backend, then steps down to the processor, each with a typed
/// event, and reports the failure. On the processor nothing is retried without Save memory
/// (it can always do it, so the failure is memory, and turning it off needs more).
///
/// The Linux build has no graphics backend, so the "graphics" backend is the same
/// processor build labelled Vulkan with no device (`--device none`): the supervisor decides
/// by the label, while the processes, their log and its classification are the pinned
/// build's. Linux only, and only while the kernel refuses impossible allocations
/// (`vm.overcommit_memory` is not 1): elsewhere the allocation of several terabytes might
/// be granted lazily and then touched.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn a_failure_save_memory_may_cause_is_retried_once_without_it() {
    use runtime::Supervisor;
    use runtime::llama::server::Candidate;

    let Some((bin, gguf)) = env_paths("a_failure_save_memory_may_cause_is_retried_once_without_it")
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
    let candidate = |backend: &Backend| Candidate {
        backend: backend.clone(),
        device: None,
        hip_visible_devices: None,
    };
    let too_large = |backend: Backend, gpu: GpuUse| StartSpec {
        backend,
        launch: LaunchConfig {
            ctx: 50_000_128,
            gpu,
            save_memory: true,
            ..launch(gguf.clone())
        },
        load_timeout: LOAD_TIMEOUT,
    };
    /// The stages and step-downs, in order.
    fn changes(events: &[ServerEvent]) -> Vec<&ServerEvent> {
        events
            .iter()
            .filter(|e| matches!(e, ServerEvent::Stage(_) | ServerEvent::SteppedDown { .. }))
            .collect()
    }
    let starts = |events: &[ServerEvent]| {
        events
            .iter()
            .filter(|e| **e == ServerEvent::State(ServerState::Starting))
            .count()
    };

    // A graphics backend: Save memory off, then the processor.
    let (tx, mut rx) = unbounded_channel();
    let mut supervisor = Supervisor::local(vec![candidate(&graphics), candidate(&cpu)], tx);
    let error = supervisor
        .start(too_large(graphics.clone(), GpuUse::All))
        .await
        .expect_err("cannot allocate");
    assert_eq!(
        error.failure(),
        LoadFailure::ContextTooLarge,
        "{}",
        error.stderr_tail().join("\n")
    );
    let events = drain(&mut rx);
    assert_eq!(
        changes(&events),
        vec![
            &ServerEvent::Stage(LoadStage::RetryingWithoutSaveMemory {
                backend: BackendKind::Vulkan
            }),
            &ServerEvent::SteppedDown {
                from: BackendKind::Vulkan,
                to: BackendKind::Cpu,
                reason: LoadFailure::ContextTooLarge,
            },
        ]
    );
    assert_eq!(
        starts(&events),
        3,
        "with Save memory, without it, then on the processor"
    );

    // The processor alone: one start, no retry.
    let (tx, mut rx) = unbounded_channel();
    let mut supervisor = Supervisor::local(vec![candidate(&cpu)], tx);
    let error = supervisor
        .start(too_large(cpu, GpuUse::Off))
        .await
        .expect_err("cannot allocate");
    assert_eq!(error.failure(), LoadFailure::ContextTooLarge);
    let events = drain(&mut rx);
    assert!(changes(&events).is_empty(), "{events:?}");
    assert_eq!(starts(&events), 1);
}
