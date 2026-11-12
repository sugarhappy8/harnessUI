//! Integration tests against a real llama-server (the pinned build) and a tiny model.
//!
//! They read `LLAMA_SERVER_BIN` (the `llama-server` executable) and `TEST_GGUF` (a small
//! GGUF model with tool-call support, such as Qwen3 0.6B) and skip with a note when either
//! is unset, so a machine without them still passes. CI sets both on Windows and macOS.

use std::path::{Path, PathBuf};
use std::time::Duration;

use runtime::llama::server::loopback_client_builder;
use runtime::{
    Backend, BackendKind, GpuLayers, LaunchConfig, LoadFailure, RuntimeLayout, ServerEvent,
    ServerState, StartError, StartSpec, discover, free_loopback_port, probe, process_alive,
    random_api_key, start,
};
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

fn launch(model: PathBuf) -> LaunchConfig {
    LaunchConfig {
        model,
        mmproj: None,
        ctx: 2048,
        device: None,
        gpu_layers: GpuLayers::Auto,
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
