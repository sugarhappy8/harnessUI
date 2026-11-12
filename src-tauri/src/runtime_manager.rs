//! Owns the local llama-server: picking a backend, loading a model, reporting
//! progress to the webview, restarting after a crash and stepping down to the next
//! backend when one fails (ARCHITECTURE.md 7.2–7.4). The restart and step-down
//! policy itself lives in `runtime::Supervisor`; this file connects it to the app.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use runtime::llama::server::{Candidate, Supervisor, SupervisorExit};
use runtime::{
    Backend, BackendKind, Device, GpuUse, LaunchConfig, LoadFailure, LoadStage, ProbeResult,
    RuntimeLayout, ServerEvent, ServerState, StartSpec,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use crate::dto::{BackendDto, DeviceDto, RuntimeStateDto, RuntimeStatusDto};
use crate::error::{AppError, AppResult};
use crate::load_settings::{Footprint, Freed, ResolvedLoad};
use crate::load_tracker::{ChangeGate, LoadTicket, LoadTracker};
use crate::state::{AppState, BusyCounter, BusyGuard, BusyKind};

/// stderr lines kept for error details.
const TAIL_LINES: usize = 80;

/// How long a model may take to load before the attempt is abandoned:
/// 60 s plus 30 s per GB, capped at 15 minutes.
fn load_timeout(model: &Path) -> Duration {
    let size = std::fs::metadata(model).map(|m| m.len()).unwrap_or(0);
    let secs = 60 + (size / 1_000_000_000) * 30;
    Duration::from_secs(secs.min(900))
}

/// What chat needs to talk to the loaded model.
#[derive(Debug, Clone)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: String,
    pub model: PathBuf,
    pub n_ctx: u32,
}

struct Session {
    /// The load this server belongs to (see [`LoadTracker`]).
    seq: u64,
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<SupervisorExit>,
}

/// The model being loaded or loaded, as it was resolved (0.0.5 D2b): what the "already
/// loaded" check and the "older settings" flag compare, and what Automatic counts as free
/// when a load replaces it.
#[derive(Clone, Debug)]
struct Loaded {
    path: PathBuf,
    resolved: ResolvedLoad,
    footprint: Footprint,
}

/// How long "Stop and restart" waits for a stopped reply or model check to wind down
/// before the server stops anyway.
const STOP_WAIT: Duration = Duration::from_secs(10);

pub struct RuntimeManager {
    app: AppHandle,
    layout: RuntimeLayout,
    status: StdMutex<RuntimeStatusDto>,
    endpoint: StdMutex<Option<Endpoint>>,
    probes: Mutex<Option<Vec<(Backend, ProbeResult)>>>,
    session: Mutex<Option<Session>>,
    tail: StdMutex<VecDeque<String>>,
    loads: LoadTracker,
    /// Held while a load replaces the previous server and starts its own, so that two
    /// loads never do that at the same time. Not held while the model loads.
    load_setup: Mutex<()>,
    /// The model being loaded or loaded, as resolved.
    current: StdMutex<Option<Loaded>>,
    /// The model on a graphics device while the backends were last probed: (backend,
    /// device, estimated bytes). Its memory was not free then, but is for the next load.
    probed_with: StdMutex<Option<(String, String, u64)>>,
    /// The change that waits for the model to be free, and the users "Stop and restart"
    /// stops (D3).
    gate: Arc<ChangeGate>,
}

impl RuntimeManager {
    pub fn new(app: AppHandle, layout: RuntimeLayout) -> Self {
        Self {
            app,
            layout,
            status: StdMutex::new(RuntimeStatusDto::default()),
            endpoint: StdMutex::new(None),
            probes: Mutex::new(None),
            session: Mutex::new(None),
            tail: StdMutex::new(VecDeque::new()),
            loads: LoadTracker::default(),
            load_setup: Mutex::new(()),
            current: StdMutex::new(None),
            probed_with: StdMutex::new(None),
            gate: Arc::new(ChangeGate::default()),
        }
    }

    pub fn status(&self) -> RuntimeStatusDto {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Records whether the loaded model's template can think.
    pub fn set_thinking(&self, thinking: bool) {
        self.update(|s| s.thinking = thinking);
    }

    fn update(&self, f: impl FnOnce(&mut RuntimeStatusDto)) {
        let snapshot = {
            let Ok(mut s) = self.status.lock() else {
                return;
            };
            f(&mut s);
            s.clone()
        };
        let _ = self
            .app
            .emit_to(crate::MAIN_WINDOW, "runtime-status", snapshot);
    }

    /// The running server's address and key, if a model is loaded and ready.
    pub async fn endpoint(&self) -> Option<(String, String, PathBuf, u32)> {
        self.endpoint
            .lock()
            .ok()
            .and_then(|e| e.clone())
            .map(|e| (e.base_url, e.api_key, e.model, e.n_ctx))
    }

    /// Times a short generation after a load (7.4). A GPU backend that generates very
    /// slowly has probably spilled into system memory (NVIDIA drivers do this instead of
    /// failing); that is reported as "loaded but slow".
    pub async fn speed_check(&self) {
        use futures_util::StreamExt;
        let Some((base_url, key, _, _)) = self.endpoint().await else {
            return;
        };
        let Ok(client) = local_client(base_url, key) else {
            return;
        };
        let mut request = agent::ChatRequest::new(
            "local".to_string(),
            vec![agent::ChatMessage::user("Say hello.")],
        );
        request.stream = true;
        request.max_tokens = Some(SPEED_TEST_TOKENS);
        request
            .extra
            .insert("cache_prompt".into(), serde_json::json!(false));
        request.extra.insert(
            "chat_template_kwargs".into(),
            serde_json::json!({"enable_thinking": false}),
        );
        let cancel = CancellationToken::new();
        let timer = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(60)).await;
            timer.cancel();
        });
        let mut tps = None;
        if let Ok(stream) = client.stream_chat(&request, cancel.clone()).await {
            futures_util::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(agent::StreamEvent::Timings(t)) => tps = t.predicted_per_second.or(tps),
                    Ok(agent::StreamEvent::Done) | Err(_) => break,
                    _ => {}
                }
            }
        }
        cancel.cancel();
        let Some(tps) = tps else { return };
        tracing::info!(tokens_per_second = tps, "speed check after load");
        self.update(|s| {
            let gpu = s.backend.as_deref().is_some_and(|b| b != "cpu");
            s.speed_tps = Some(tps);
            s.slow = gpu && tps < SLOW_GPU_TPS;
        });
    }

    /// Probes every backend found on disk (cached unless `reprobe`).
    pub async fn backends(&self, reprobe: bool) -> Vec<(Backend, ProbeResult)> {
        let mut cache = self.probes.lock().await;
        if !reprobe && let Some(found) = cache.as_ref() {
            return found.clone();
        }
        // The model that is loaded now takes memory the probe will not report as free.
        let loaded_on = self.loaded_on_device();
        let mut out = Vec::new();
        for backend in runtime::discover(&self.layout) {
            // 45 s: the first start of a new binary on macOS waits for the system's
            // malware scan, which took longer than 15 s on CI.
            let probe = runtime::probe(&backend, Duration::from_secs(45)).await;
            if let Some(e) = &probe.error {
                tracing::warn!(backend = %backend.kind, "backend probe failed: {e}");
                for line in probe.output.iter().take(40) {
                    tracing::warn!(backend = %backend.kind, "  llama-server: {line}");
                }
            }
            out.push((backend, probe));
        }
        *cache = Some(out.clone());
        if let Ok(mut p) = self.probed_with.lock() {
            *p = loaded_on;
        }
        out
    }

    /// The loaded (or loading) model's graphics device and estimated size, when it runs on
    /// one.
    fn loaded_on_device(&self) -> Option<(String, String, u64)> {
        let status = self.status();
        if !matches!(
            status.state,
            RuntimeStateDto::Ready | RuntimeStateDto::Starting
        ) {
            return None;
        }
        let backend = status.backend.filter(|b| b != "cpu")?;
        let device = status.device?;
        let needed = self.current.lock().ok()?.as_ref()?.footprint.needed;
        Some((backend, device, needed))
    }

    /// Memory a load would free by replacing the loaded model (D2b), for Automatic.
    pub fn freed_memory(&self) -> Freed {
        let gpu = self.probed_with.lock().ok().and_then(|p| p.clone());
        let status = self.status();
        let on_cpu = status.backend.as_deref() == Some("cpu")
            && matches!(
                status.state,
                RuntimeStateDto::Ready | RuntimeStateDto::Starting
            );
        let ram_bytes = if on_cpu {
            self.current
                .lock()
                .ok()
                .and_then(|c| c.as_ref().map(|l| l.footprint.anon))
                .unwrap_or(0)
        } else {
            0
        };
        Freed { gpu, ram_bytes }
    }

    /// What the loaded copy of `path` was started with, when `path` is the loaded model.
    pub fn loaded_choices(&self, path: &str) -> Option<ResolvedLoad> {
        let status = self.status();
        if status.state != RuntimeStateDto::Ready || status.model_path.as_deref() != Some(path) {
            return None;
        }
        let current = self.current.lock().ok()?;
        current
            .as_ref()
            .filter(|l| l.path.display().to_string() == path)
            .map(|l| l.resolved.clone())
    }

    /// "Settings changed. Restart the model to use them." (D3).
    pub fn set_settings_changed(&self, changed: bool) {
        if self.status().settings_changed != changed {
            self.update(|s| s.settings_changed = changed);
        }
    }

    /// Before a load, unload or reload (D3): cancels the change that waits; then with
    /// `when_idle` waits until nothing uses the model (`reload_waiting` on meanwhile;
    /// `Ok(true)`), else stops the chat turns and model checks that use it first
    /// (`Ok(false)`). A wait that a newer change or Cancel ends is `load_replaced`.
    pub async fn before_change(&self, busy: &BusyCounter, when_idle: bool) -> AppResult<bool> {
        self.gate
            .before_change(busy, when_idle, STOP_WAIT, |waiting| {
                self.update(|s| s.reload_waiting = waiting);
            })
            .await
            .map_err(|_| wait_cancelled_error())
    }

    /// Cancels the change that waits for the model to be free, if one does.
    pub fn cancel_waiting(&self) {
        if self.gate.cancel_waiting() {
            self.update(|s| s.reload_waiting = false);
        }
    }

    /// Counts one user of the local model (D3) until the returned guard is dropped;
    /// "Stop and restart" stops it with `stop`.
    pub fn hold(
        &self,
        busy: &Arc<BusyCounter>,
        kind: BusyKind,
        stop: CancellationToken,
    ) -> ModelUse {
        let id = self.gate.add_user(stop);
        ModelUse {
            _busy: busy.hold(kind),
            gate: Arc::clone(&self.gate),
            id,
        }
    }

    /// "Unload to free memory": unloads, and remembers which model it was for
    /// "Unloaded to free memory · Load".
    pub async fn unload_by_user(&self) {
        let path = self.status().model_path;
        self.unload().await;
        if let Some(path) = path {
            self.update(|s| s.unloaded_path = Some(path));
        }
    }

    pub async fn backend_dtos(&self, reprobe: bool) -> Vec<BackendDto> {
        let adapters = crate::gpu::adapters();
        self.backends(reprobe)
            .await
            .into_iter()
            .map(|(b, p)| BackendDto {
                kind: kind_name(b.kind).to_string(),
                source: match &b.source {
                    runtime::BackendSource::Installed => "installed".to_string(),
                    runtime::BackendSource::Pack { id } => format!("download ({id})"),
                    runtime::BackendSource::DevOverride => "development".to_string(),
                },
                build: b.build.clone(),
                available: p.is_usable(),
                devices: p
                    .devices
                    .iter()
                    .map(|d| DeviceDto {
                        name: d.name.clone(),
                        description: d.description.clone(),
                        total_mib: d.total_mib,
                        free_mib: d.free_mib,
                    })
                    .collect(),
                // A Vulkan backend that lists no device says nothing about why (7.2): tell
                // the user which cards Windows sees and what to do, not "no device found".
                error: if b.kind == runtime::BackendKind::Vulkan && p.devices.is_empty() {
                    match &p.error {
                        Some(e) if !matches!(e, runtime::ProbeError::NoMatchingDevice { .. }) => {
                            Some(e.to_string())
                        }
                        _ => Some(crate::gpu::vulkan_no_device_message(
                            vulkan_loader_present()
                                || b.dir
                                    .join(runtime::VULKAN_FALLBACK_DIR)
                                    .join(runtime::VULKAN_RUNTIME_FILE)
                                    .is_file(),
                            &adapters,
                        )),
                    }
                } else {
                    p.error.as_ref().map(|e| e.to_string())
                },
            })
            .collect()
    }

    /// Backends that passed their probe, each with the device to use, in the order Metal,
    /// Vulkan, CUDA, ROCm, CPU. An explicit backend choice is looked up in this list, and
    /// the supervisor steps down through it after a failed start. Auto picks only Metal,
    /// Vulkan or CPU from it (D3, [`auto_choice`]): CUDA and ROCm are manual choices, and
    /// so is Metal on a Mac with an Intel processor ([`auto_prefers_cpu_on`]).
    ///
    /// ROCm is launched with only the chosen card visible to the HIP runtime
    /// (`HIP_VISIBLE_DEVICES`), which then numbers it `ROCm0`: with the integrated GPU still
    /// visible ROCm can crash even when the card is passed by name (7.2).
    fn candidates(found: &[(Backend, ProbeResult)], wanted_device: Option<&str>) -> Vec<Candidate> {
        let order = [
            BackendKind::Metal,
            BackendKind::Vulkan,
            BackendKind::Cuda,
            BackendKind::Rocm,
            BackendKind::Cpu,
        ];
        let mut out = Vec::new();
        for kind in order {
            for (b, p) in found
                .iter()
                .filter(|(b, p)| b.kind == kind && p.is_usable())
            {
                let device: Option<&Device> = if kind == BackendKind::Cpu {
                    None
                } else {
                    wanted_device
                        .and_then(|name| p.devices.iter().find(|d| d.name == name))
                        .or_else(|| runtime::pick_device(&p.devices))
                };
                if kind != BackendKind::Cpu && device.is_none() {
                    continue;
                }
                let (device, hip_visible_devices) = match device {
                    Some(d) => {
                        let (name, hip) = launch_device(kind, d);
                        (Some(name), hip)
                    }
                    None => (None, None),
                };
                out.push(Candidate {
                    backend: b.clone(),
                    device,
                    hip_visible_devices,
                });
            }
        }
        out
    }

    /// The first candidate: the user's backend choice, or Auto.
    async fn choose(&self, settings: &ResolvedLoad) -> AppResult<(Candidate, Vec<Candidate>)> {
        let found = self.backends(false).await;
        if found.is_empty() {
            return Err(AppError::new(
                "no_runtime",
                "This build does not contain the llama.cpp runtime.",
                "Reinstall the app. On a development build, set APP_DEV_LLAMA_DIR to a folder with llama-server.",
                format!("{:?}", self.layout),
            ));
        }
        first_candidate(&found, settings)
    }

    fn tail_text(&self) -> String {
        self.tail
            .lock()
            .map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    /// Loads `model` (and its projector) as `settings` resolved it, replacing any model
    /// already loaded. Returns when the model is ready or has failed on every backend it
    /// may step down to. `footprint` is what it will take, for the next Automatic.
    pub async fn load(
        self: Arc<Self>,
        model: PathBuf,
        settings: ResolvedLoad,
        footprint: Footprint,
    ) -> AppResult<()> {
        if !model.is_file() {
            return Err(AppError::new(
                "model_missing",
                "The model file could not be found.",
                "Pick the file again; it may have been moved or deleted.",
                model.display().to_string(),
            ));
        }
        let setup = self.load_setup.lock().await;
        // The same model picked again while it loads: wait for that load, do not restart
        // it.
        if let Some(seq) = self.loads.same_in_progress(&model, &settings) {
            drop(setup);
            return self.wait_for_load(seq, &model).await;
        }
        // From here this is the current load. A newer one replaces it; the ticket marks
        // it as over on every way out of this function.
        let ticket = LoadTicket::begin(&self.loads, &model, &settings);
        let seq = ticket.seq;
        self.unload().await;
        let (first, candidates) = self.choose(&settings).await?;
        if let Ok(mut c) = self.current.lock() {
            *c = Some(Loaded {
                path: model.clone(),
                resolved: settings.clone(),
                footprint,
            });
        }
        let model_name = model
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Ok(mut t) = self.tail.lock() {
            t.clear();
        }
        self.update(|s| {
            *s = RuntimeStatusDto {
                state: RuntimeStateDto::Starting,
                backend: Some(kind_name(first.backend.kind).to_string()),
                device: shown_device(
                    first.device.as_deref(),
                    first.hip_visible_devices.as_deref(),
                ),
                model_path: Some(model.display().to_string()),
                model_name: Some(model_name.clone()),
                load_stage: Some(LoadStage::Starting),
                ctx_source: Some(settings.ctx_source),
                ..Default::default()
            };
        });

        let port = runtime::free_loopback_port()
            .map_err(|e| AppError::internal(format!("no free port: {e}")))?;
        let mmproj = settings
            .mmproj
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_file());
        let launch = launch_config(
            &settings,
            &first,
            model.clone(),
            mmproj,
            port,
            runtime::random_api_key(),
        );
        let spec = StartSpec {
            backend: first.backend.clone(),
            launch,
            load_timeout: load_timeout(&model),
        };

        // Events: progress and state for the UI; stderr kept in a small ring for details.
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<ServerEvent>();
        let me = Arc::clone(&self);
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                me.on_event(event);
            }
        });

        let cancel = CancellationToken::new();
        let (ready_tx, ready_rx) = oneshot::channel::<Result<(), ()>>();
        let ready_tx = StdMutex::new(Some(ready_tx));
        let me = Arc::clone(&self);
        let supervisor = Supervisor::local(candidates, tx);
        let token = cancel.clone();
        let task = tokio::spawn(async move {
            let me2 = Arc::clone(&me);
            let exit = supervisor
                .run(spec, token, move |server, running| {
                    // A newer load is about to stop this server; its state is not news.
                    if !me2.loads.is_current(seq) {
                        return;
                    }
                    let props = server.props().clone();
                    if let Some(pid) = server.pid() {
                        crate::children::track(&me2.app, pid, &running.backend.server);
                    }
                    if let Ok(mut e) = me2.endpoint.lock() {
                        *e = Some(Endpoint {
                            base_url: server.base_url(),
                            api_key: server.api_key().to_string(),
                            model: running.launch.model.clone(),
                            n_ctx: props.n_ctx,
                        });
                    }
                    me2.update(|s| {
                        s.state = RuntimeStateDto::Ready;
                        s.progress = Some(1.0);
                        s.backend = Some(kind_name(running.backend.kind).to_string());
                        s.device = shown_device(
                            running.launch.device.as_deref(),
                            running.launch.hip_visible_devices.as_deref(),
                        );
                        s.n_ctx = Some(props.n_ctx);
                        s.supports_tools = props.chat_template_caps.supports_tool_calls;
                        s.preserve_reasoning = props.chat_template_caps.supports_preserve_reasoning;
                        s.vision = props.modalities.vision;
                        s.error = None;
                        s.load_stage = None;
                    });
                    if let Ok(mut slot) = ready_tx.lock()
                        && let Some(tx) = slot.take()
                    {
                        let _ = tx.send(Ok(()));
                    }
                })
                .await;
            // A newer load publishes its own server; do not wipe that.
            if me.loads.is_current(seq) {
                if let Ok(mut e) = me.endpoint.lock() {
                    *e = None;
                }
                crate::children::untrack(&me.app);
            }
            exit
        });
        *self.session.lock().await = Some(Session { seq, cancel, task });
        drop(setup);

        // Wait for the first ready server, or for the supervisor to give up.
        let outcome = ready_rx.await;
        if matches!(outcome, Ok(Ok(()))) {
            tracing::info!(model = %model_name, "model ready");
            return Ok(());
        }
        // This load's own session only: by now the slot may hold a newer load's server,
        // which must be left alone. `None`: an unload or a newer load took this one and
        // stopped it.
        let mine = {
            let mut slot = self.session.lock().await;
            if slot.as_ref().is_some_and(|s| s.seq == seq) {
                slot.take()
            } else {
                None
            }
        };
        let exit = match mine {
            Some(s) => s.task.await.map_err(|e| e.to_string()),
            None => Ok(SupervisorExit::Stopped),
        };
        if !ticket.is_current() {
            // Another model was picked meanwhile. That load owns the status now, and the
            // user must not be told that this one "was cancelled".
            return Err(replaced_error());
        }
        let err = match exit {
            Ok(SupervisorExit::Failed(failure)) => load_error(&failure, self.tail_text()),
            // Stopped by an unload (the model or its backend was removed). The unload has
            // reset the status; there is nothing to mark as failed.
            Ok(SupervisorExit::Stopped) => return Err(cancelled_error()),
            // The task that runs the server ended by itself, which only a bug does. It
            // used to be reported as "Loading was cancelled".
            Err(why) => loader_stopped_error(why, self.tail_text()),
        };
        self.update(|s| mark_failed(s, err.clone()));
        Err(err)
    }

    /// When `model` is already being loaded with the same choices: waits for that load
    /// and returns how it ended. `None` when no such load is in progress.
    pub async fn join_load(&self, model: &Path, settings: &ResolvedLoad) -> Option<AppResult<()>> {
        let seq = self.loads.same_in_progress(model, settings)?;
        Some(self.wait_for_load(seq, model).await)
    }

    /// Waits until load `seq` (of `model`) is over and returns how it ended.
    async fn wait_for_load(&self, seq: u64, model: &Path) -> AppResult<()> {
        self.loads.finished(seq).await;
        let now = self.status();
        let wanted = model.display().to_string();
        if now.model_path.as_deref() != Some(wanted.as_str()) {
            return Err(replaced_error());
        }
        if now.state == RuntimeStateDto::Ready {
            Ok(())
        } else {
            Err(now.error.unwrap_or_else(cancelled_error))
        }
    }

    fn on_event(&self, event: ServerEvent) {
        match event {
            ServerEvent::Progress { stage, value } => self.update(|s| {
                if let Some(name) = &stage {
                    s.load_stage = Some(LoadStage::from_llama_stage(name));
                }
                if stage.is_some() {
                    s.stage = stage;
                }
                s.progress = value;
            }),
            ServerEvent::Offloaded { layers, total } => self.update(|s| {
                s.offloaded_layers = Some(layers);
                s.total_layers = Some(total);
            }),
            ServerEvent::Log(line) => {
                if let Ok(mut t) = self.tail.lock() {
                    if t.len() >= TAIL_LINES {
                        t.pop_front();
                    }
                    t.push_back(line);
                }
            }
            ServerEvent::Restarting { attempt, max } => {
                tracing::warn!(
                    attempt,
                    max,
                    "llama-server stopped unexpectedly; restarting"
                );
                self.update(|s| {
                    s.state = RuntimeStateDto::Starting;
                    s.stage = Some(format!("Restarting after a crash ({attempt} of {max})"));
                    s.load_stage = Some(LoadStage::Restarting { attempt, max });
                    s.progress = None;
                });
            }
            ServerEvent::SteppedDown { from, to, reason } => {
                tracing::warn!(%from, %to, ?reason, "stepping down to another backend");
                let stage = LoadStage::SteppingDown {
                    from,
                    to,
                    reason: reason.clone(),
                };
                self.update(|s| {
                    s.state = RuntimeStateDto::Starting;
                    s.backend = Some(kind_name(to).to_string());
                    s.stage = Some(format!(
                        "{} failed ({}); trying {}",
                        kind_name(from),
                        reason.user_message(),
                        kind_name(to)
                    ));
                    s.adjustments.push(stage.clone());
                    s.load_stage = Some(stage);
                });
            }
            // A typed transition with no event of its own (WP1a's retries): shown as the
            // current stage, and kept as an adjustment of this load.
            ServerEvent::Stage(stage) => self.update(|s| {
                if matches!(
                    stage,
                    LoadStage::RetryingWithoutSaveMemory { .. }
                        | LoadStage::RetryingWithLessGpu { .. }
                        | LoadStage::SteppingDown { .. }
                ) {
                    s.adjustments.push(stage.clone());
                }
                s.load_stage = Some(stage);
            }),
            ServerEvent::State(ServerState::Failed(failure)) => {
                let err = load_error(&failure, self.tail_text());
                self.update(|s| mark_failed(s, err));
            }
            ServerEvent::State(_) | ServerEvent::Exited { .. } => {}
        }
    }

    pub async fn unload(&self) {
        let taken = self.session.lock().await.take();
        if let Some(session) = taken {
            self.update(|s| s.state = RuntimeStateDto::Stopping);
            session.cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(15), session.task).await;
            tracing::info!("model unloaded");
        }
        if let Ok(mut e) = self.endpoint.lock() {
            *e = None;
        }
        if let Ok(mut c) = self.current.lock() {
            *c = None;
        }
        self.update(|s| *s = RuntimeStatusDto::default());
    }

    /// Stops the server on app exit.
    pub async fn shutdown(&self) {
        let taken = self.session.lock().await.take();
        if let Some(session) = taken {
            session.cancel.cancel();
            let _ = tokio::time::timeout(Duration::from_secs(5), session.task).await;
        }
    }
}

/// One user of the local model (a chat turn, a model check): counted in the busy counter
/// while it lives, and stopped by "Stop and restart" (D3).
pub struct ModelUse {
    _busy: BusyGuard,
    gate: Arc<ChangeGate>,
    id: u64,
}

impl Drop for ModelUse {
    fn drop(&mut self) {
        self.gate.remove_user(self.id);
    }
}

/// For the one line where a chat turn or a model check starts (D3): while it uses the
/// local model (`model_ref` starts with `local:`) it is counted as busy, and "Stop and
/// restart" stops it with `stop`. `None` for a model of an endpoint.
pub fn use_local_model(
    state: &AppState,
    model_ref: &str,
    kind: BusyKind,
    stop: &CancellationToken,
) -> Option<ModelUse> {
    model_ref
        .starts_with("local:")
        .then(|| state.runtime.hold(&state.model_busy, kind, stop.clone()))
}

/// The launch of `model` on candidate `first` as `settings` resolved it: the context,
/// "Use the graphics card" (always off on the processor), Save memory and Keep in memory.
/// The old `gpu_layers` field is left at its default: the arguments read `gpu` only.
fn launch_config(
    settings: &ResolvedLoad,
    first: &Candidate,
    model: PathBuf,
    mmproj: Option<PathBuf>,
    port: u16,
    api_key: String,
) -> LaunchConfig {
    let gpu = if first.backend.kind == BackendKind::Cpu {
        GpuUse::Off
    } else {
        settings.gpu
    };
    LaunchConfig {
        mmproj,
        device: first.device.clone(),
        gpu,
        save_memory: settings.save_memory,
        keep_in_memory: settings.keep_in_memory,
        hip_visible_devices: first.hip_visible_devices.clone(),
        ..LaunchConfig::new(model, settings.ctx, port, api_key)
    }
}

/// The candidate a load starts on and the list it may step down through: the backend the
/// user chose, or Auto's pick (D3), from the probes. Only the settings decide it, never
/// the backend the last load ended up on, so Reload after a step-down tries the chosen
/// backend (and the whole graphics card) again.
fn first_candidate(
    found: &[(Backend, ProbeResult)],
    settings: &ResolvedLoad,
) -> AppResult<(Candidate, Vec<Candidate>)> {
    let candidates = RuntimeManager::candidates(found, settings.device.as_deref());
    let first = match settings.backend.as_deref() {
        Some(choice) => match explicit_choice(&candidates, choice) {
            Some(c) => c.clone(),
            None => {
                let why = found
                    .iter()
                    .find(|(b, _)| kind_name(b.kind) == choice)
                    .and_then(|(_, p)| p.error.as_ref().map(|e| e.to_string()))
                    .unwrap_or_else(|| "not installed".to_string());
                return Err(AppError::new(
                    "backend_unavailable",
                    format!("The {choice} backend is not usable on this computer."),
                    "Pick Auto in Settings → Local models, or run the backend check in Settings → Diagnostics.",
                    why,
                ));
            }
        },
        None => auto_choice(&candidates).cloned().ok_or_else(|| {
            AppError::new(
                "no_usable_backend",
                "No llama.cpp backend could start on this computer.",
                if candidates.is_empty() {
                    "Run the backend check in Settings → Diagnostics and copy the report into a bug report."
                } else {
                    // Only CUDA or ROCm work here; Auto never picks them (D3).
                    "Pick CUDA or ROCm in Settings → Local models, or run the backend check in Settings → Diagnostics."
                },
                found
                    .iter()
                    .map(|(b, p)| format!("{}: {:?}", b.kind, p.error))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )
        })?,
    };
    Ok((first, candidates))
}

/// The backend a load would start on: the user's choice (`backend`), or Auto's pick.
/// `None` when it cannot start anywhere.
pub(crate) fn planned_backend(
    found: &[(Backend, ProbeResult)],
    backend: Option<&str>,
    device: Option<&str>,
) -> Option<BackendKind> {
    let candidates = RuntimeManager::candidates(found, device);
    match backend {
        Some(choice) => explicit_choice(&candidates, choice),
        None => auto_choice(&candidates),
    }
    .map(|c| c.backend.kind)
}

/// Marks the status as a failed load. The stage is cleared, as on Ready; the adjustments
/// stay, since they explain the failure.
fn mark_failed(s: &mut RuntimeStatusDto, err: AppError) {
    s.state = RuntimeStateDto::Failed;
    s.progress = None;
    s.load_stage = None;
    s.error = Some(err);
}

/// The code of the error a load ends with when a newer load replaced it. The UI shows
/// nothing for it.
pub const LOAD_REPLACED: &str = "load_replaced";

fn replaced_error() -> AppError {
    AppError::new(
        LOAD_REPLACED,
        "Another model was picked while this one was loading.",
        "Nothing to do: the model picked last is the one being loaded.",
        String::new(),
    )
}

/// A change that waited for the model to be free was cancelled (a newer change, or
/// Cancel). Same code as a replaced load: the UI shows nothing for it.
fn wait_cancelled_error() -> AppError {
    AppError::new(
        LOAD_REPLACED,
        "The change was cancelled before the model was free.",
        "Nothing to do.",
        String::new(),
    )
}

fn cancelled_error() -> AppError {
    AppError::new(
        "load_cancelled",
        "Loading was stopped.",
        "Pick the model again to load it.",
        String::new(),
    )
}

fn loader_stopped_error(why: String, tail: String) -> AppError {
    AppError::new(
        "load_crashed",
        "The model could not be loaded: the loader stopped unexpectedly.",
        "Try again. If it keeps happening, run the backend check in Settings → Diagnostics and send the report.",
        format!("{why}\n{tail}"),
    )
}

pub fn kind_name(kind: BackendKind) -> &'static str {
    kind.as_str()
}

/// Auto's pick (D3) on the computer this build runs on.
fn auto_choice(candidates: &[Candidate]) -> Option<&Candidate> {
    auto_choice_on(std::env::consts::OS, std::env::consts::ARCH, candidates)
}

/// Auto's pick (D3) for an operating system and processor (`std::env::consts` names):
/// the first of Metal, Vulkan or CPU. CUDA and ROCm are only used when the user picks
/// them, even when Vulkan does not work. Where [`auto_prefers_cpu_on`] holds, the CPU is
/// taken before Metal when both work.
fn auto_choice_on<'a>(os: &str, arch: &str, candidates: &'a [Candidate]) -> Option<&'a Candidate> {
    let auto = |skip_metal: bool| {
        candidates.iter().find(|c| match c.backend.kind {
            BackendKind::Cuda | BackendKind::Rocm => false,
            BackendKind::Metal => !skip_metal,
            BackendKind::Vulkan | BackendKind::Cpu => true,
        })
    };
    auto(auto_prefers_cpu_on(os, arch)).or_else(|| auto(false))
}

/// Whether Auto leaves Metal to an explicit choice: on a Mac with an Intel processor.
/// llama.cpp's Metal backend is unreliable on Intel integrated graphics, so such a Mac
/// runs models on the CPU unless the user picks Metal in Settings → Local models (worth
/// trying with a Radeon GPU). The universal app runs natively on both kinds of Mac, so
/// the processor this build was compiled for is the processor it runs on.
pub fn auto_prefers_cpu_on(os: &str, arch: &str) -> bool {
    os == "macos" && arch == "x86_64"
}

/// [`auto_prefers_cpu_on`] for the computer this build runs on.
pub fn auto_prefers_cpu() -> bool {
    auto_prefers_cpu_on(std::env::consts::OS, std::env::consts::ARCH)
}

/// The candidate for a backend the user picked in Settings, by its lower-case name.
fn explicit_choice<'a>(candidates: &'a [Candidate], choice: &str) -> Option<&'a Candidate> {
    candidates
        .iter()
        .find(|c| kind_name(c.backend.kind) == choice)
}

/// The `--device` name and `HIP_VISIBLE_DEVICES` for a probed device. The probe's
/// `ROCm<N>` comes from the unmasked HIP enumeration, the numbering `HIP_VISIBLE_DEVICES`
/// uses; with only that card visible, llama.cpp names it `ROCm0`.
fn launch_device(kind: BackendKind, device: &Device) -> (String, Option<String>) {
    if kind == BackendKind::Rocm
        && let Some(index) = device.name.strip_prefix("ROCm")
        && !index.is_empty()
        && index.bytes().all(|b| b.is_ascii_digit())
        && let Ok(index) = index.parse::<u32>()
    {
        return ("ROCm0".to_string(), Some(index.to_string()));
    }
    (device.name.clone(), None)
}

/// The device as the probe and Settings name it: a ROCm launch masked to one card passes
/// it as `ROCm0`, but the user picked it by its unmasked number.
fn shown_device(device: Option<&str>, hip_visible_devices: Option<&str>) -> Option<String> {
    match (device, hip_visible_devices) {
        (Some("ROCm0"), Some(index)) => Some(format!("ROCm{index}")),
        (device, _) => device.map(str::to_string),
    }
}

fn load_error(failure: &LoadFailure, tail: String) -> AppError {
    AppError::new(
        "load_failed",
        failure.user_message(),
        failure.next_step(),
        format!("{failure:?}\n\n{tail}"),
    )
}

/// Where backends live for this build (ARCHITECTURE.md 7.1).
pub fn layout(app: &AppHandle) -> RuntimeLayout {
    use tauri::Manager;
    let installed_dir = if cfg!(target_os = "macos") {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_path_buf))
    } else if cfg!(windows) {
        app.path()
            .resource_dir()
            .ok()
            .map(|d| d.join("llama").join("main"))
    } else {
        None
    };
    let packs_dir = app
        .path()
        .app_local_data_dir()
        .map(|d| d.join("backends"))
        .unwrap_or_else(|_| PathBuf::from("backends"));
    RuntimeLayout::with_env_override(installed_dir, packs_dir)
}

/// A client for the local llama-server: loopback (no proxy), the per-launch key,
/// and no overall request limit (prompt processing can take minutes).
/// Whether the Vulkan loader library is installed (the GPU driver ships it).
fn vulkan_loader_present() -> bool {
    if cfg!(windows) {
        let root = std::env::var_os("SystemRoot")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        root.join("System32").join("vulkan-1.dll").is_file()
    } else if cfg!(target_os = "macos") {
        true // Metal is used on macOS; the Vulkan backend is not shipped there.
    } else {
        [
            "/usr/lib/x86_64-linux-gnu/libvulkan.so.1",
            "/usr/lib64/libvulkan.so.1",
            "/usr/lib/libvulkan.so.1",
        ]
        .iter()
        .any(|p| std::path::Path::new(p).exists())
    }
}

/// Tokens generated by the speed check after a load.
const SPEED_TEST_TOKENS: u32 = 24;
/// Below this, a model on a GPU backend is reported as "loaded but slow". Even an 8B
/// model at Q4 on a modest GPU generates well above it; a spill into system memory
/// typically drops it to 1–3 tokens/s.
const SLOW_GPU_TPS: f64 = 5.0;

pub fn local_client(base_url: String, key: String) -> AppResult<agent::OpenAiClient> {
    let mut config = agent::ClientConfig::new(base_url, agent::ProviderKind::LlamaServer);
    config.api_key = Some(key);
    config.loopback = true;
    config.request_timeout = None;
    config.connect_timeout = Duration::from_secs(5);
    agent::OpenAiClient::new(config).map_err(AppError::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend(kind: BackendKind) -> Backend {
        Backend {
            kind,
            dir: PathBuf::from("/b"),
            server: PathBuf::from("/b/llama-server"),
            fit_params: None,
            build: "b1".into(),
            source: runtime::BackendSource::Installed,
        }
    }

    fn device(name: &str, description: &str, total_mib: u64) -> Device {
        Device {
            name: name.into(),
            description: description.into(),
            total_mib,
            free_mib: total_mib,
        }
    }

    fn usable(kind: BackendKind, devices: Vec<Device>) -> (Backend, ProbeResult) {
        (
            backend(kind),
            ProbeResult {
                backend_kind: kind,
                devices,
                error: None,
                output: Vec::new(),
            },
        )
    }

    fn broken(kind: BackendKind) -> (Backend, ProbeResult) {
        (
            backend(kind),
            ProbeResult {
                backend_kind: kind,
                devices: Vec::new(),
                error: Some(runtime::ProbeError::NoMatchingDevice { backend: kind }),
                output: Vec::new(),
            },
        )
    }

    /// 7.2: ROCm sees only the chosen card (HIP_VISIBLE_DEVICES), which is then ROCm0.
    #[test]
    fn rocm_is_masked_to_the_chosen_card() {
        let found = vec![usable(
            BackendKind::Rocm,
            vec![
                device("ROCm0", "AMD Radeon(TM) Graphics", 50246),
                device("ROCm1", "AMD Radeon RX 9070 XT", 16304),
            ],
        )];
        let rocm = RuntimeManager::candidates(&found, None);
        assert_eq!(rocm.len(), 1);
        assert_eq!(rocm[0].device.as_deref(), Some("ROCm0"));
        assert_eq!(rocm[0].hip_visible_devices.as_deref(), Some("1"));
        assert_eq!(
            shown_device(
                rocm[0].device.as_deref(),
                rocm[0].hip_visible_devices.as_deref()
            )
            .as_deref(),
            Some("ROCm1"),
            "the status names the card as the user picked it"
        );
        // A card the user picked in Settings is masked the same way.
        let picked = RuntimeManager::candidates(&found, Some("ROCm0"));
        assert_eq!(picked[0].device.as_deref(), Some("ROCm0"));
        assert_eq!(picked[0].hip_visible_devices.as_deref(), Some("0"));
    }

    #[test]
    fn other_backends_pass_the_device_unmasked() {
        let found = vec![
            usable(
                BackendKind::Vulkan,
                vec![
                    device("Vulkan0", "AMD Radeon(TM) Graphics", 512),
                    device("Vulkan1", "AMD Radeon RX 9070 XT", 16304),
                ],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let c = RuntimeManager::candidates(&found, None);
        assert_eq!(c[0].device.as_deref(), Some("Vulkan1"));
        assert_eq!(c[0].hip_visible_devices, None);
        assert_eq!(
            (c[1].device.as_deref(), c[1].hip_visible_devices.as_deref()),
            (None, None)
        );
        assert_eq!(
            launch_device(BackendKind::Rocm, &device("ROCm+1", "x", 1)),
            ("ROCm+1".to_string(), None),
            "an unexpected name is passed as it is"
        );
    }

    /// D3: Auto takes Metal, Vulkan or CPU; CUDA and ROCm only when the user picks them.
    #[test]
    fn auto_never_picks_cuda_or_rocm() {
        let gpu = || vec![device("D0", "NVIDIA GeForce RTX 4070", 12000)];
        // Vulkan has no driver, but CUDA and ROCm packs pass their probe.
        let found = vec![
            broken(BackendKind::Vulkan),
            usable(BackendKind::Cuda, gpu()),
            usable(
                BackendKind::Rocm,
                vec![device("ROCm0", "AMD Radeon RX 9070 XT", 16304)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let candidates = RuntimeManager::candidates(&found, None);
        let kinds: Vec<BackendKind> = candidates.iter().map(|c| c.backend.kind).collect();
        assert_eq!(
            kinds,
            vec![BackendKind::Cuda, BackendKind::Rocm, BackendKind::Cpu],
            "the explicit-choice and step-down order is unchanged"
        );
        assert_eq!(
            auto_choice(&candidates).map(|c| c.backend.kind),
            Some(BackendKind::Cpu)
        );
        // Vulkan, when it works, comes first.
        let found = vec![
            usable(
                BackendKind::Vulkan,
                vec![device("Vulkan0", "NVIDIA GeForce RTX 4070", 12000)],
            ),
            usable(BackendKind::Cuda, gpu()),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let candidates = RuntimeManager::candidates(&found, None);
        assert_eq!(
            auto_choice(&candidates).map(|c| c.backend.kind),
            Some(BackendKind::Vulkan)
        );
        // Only CUDA works: Auto finds nothing (the user can still pick CUDA).
        let found = vec![usable(BackendKind::Cuda, gpu())];
        let candidates = RuntimeManager::candidates(&found, None);
        assert!(auto_choice(&candidates).is_none());
        assert_eq!(candidates.len(), 1);
    }

    /// A Mac with an Intel processor runs models on the CPU unless the user picks Metal:
    /// Metal is unreliable on Intel integrated graphics. Every other computer keeps D3's
    /// order.
    #[test]
    fn auto_prefers_the_cpu_over_metal_only_on_an_intel_mac() {
        let mac = vec![
            usable(
                BackendKind::Metal,
                vec![device("MTL0", "Intel(R) Iris(TM) Plus Graphics 655", 1536)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let candidates = RuntimeManager::candidates(&mac, None);
        let kinds: Vec<BackendKind> = candidates.iter().map(|c| c.backend.kind).collect();
        assert_eq!(
            kinds,
            vec![BackendKind::Metal, BackendKind::Cpu],
            "the explicit-choice and step-down list is the same on every Mac"
        );
        for (os, arch, want) in [
            ("macos", "x86_64", BackendKind::Cpu),
            ("macos", "aarch64", BackendKind::Metal),
            ("windows", "x86_64", BackendKind::Metal),
            ("windows", "aarch64", BackendKind::Metal),
        ] {
            assert_eq!(
                auto_choice_on(os, arch, &candidates).map(|c| c.backend.kind),
                Some(want),
                "{os} {arch}"
            );
            assert_eq!(
                auto_prefers_cpu_on(os, arch),
                want == BackendKind::Cpu,
                "{os} {arch}"
            );
        }

        // Metal stays an explicit choice on an Intel Mac (it helps one with a Radeon GPU),
        // and CPU stays one on Apple silicon.
        let metal = explicit_choice(&candidates, "metal").expect("metal can be picked");
        assert_eq!(metal.backend.kind, BackendKind::Metal);
        assert_eq!(metal.device.as_deref(), Some("MTL0"));
        assert_eq!(
            explicit_choice(&candidates, "cpu").map(|c| c.backend.kind),
            Some(BackendKind::Cpu)
        );
        assert!(explicit_choice(&candidates, "vulkan").is_none());

        // With no CPU candidate, Auto on an Intel Mac still takes Metal over nothing.
        let metal_only = RuntimeManager::candidates(&mac[..1], None);
        assert_eq!(
            auto_choice_on("macos", "x86_64", &metal_only).map(|c| c.backend.kind),
            Some(BackendKind::Metal)
        );
    }

    /// The bundled runtime serves both backends on either kind of Mac. Without the Intel
    /// entry in the lock file an Intel Mac would be offered the CPU only, and Metal could
    /// not be picked.
    #[test]
    fn the_lock_file_offers_cpu_and_metal_on_both_kinds_of_mac() {
        let installer = &runtime::llama::lock::embedded().installer;
        let intel = installer.get("macos-x86_64").expect("Intel Mac entry");
        assert_eq!(intel.backends, vec![BackendKind::Cpu, BackendKind::Metal]);
        let arm = installer.get("macos-aarch64").expect("Apple silicon entry");
        assert_eq!(arm.backends, vec![BackendKind::Metal, BackendKind::Cpu]);
    }

    /// The Intel Mac rule is about Metal only: a Windows PC with the same processor
    /// family keeps Vulkan first, and CUDA and ROCm stay manual everywhere.
    #[test]
    fn the_intel_mac_rule_does_not_change_other_backends() {
        let pc = vec![
            usable(
                BackendKind::Vulkan,
                vec![device("Vulkan0", "AMD Radeon RX 9070 XT", 16304)],
            ),
            usable(
                BackendKind::Cuda,
                vec![device("CUDA0", "NVIDIA GeForce RTX 4070", 12000)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let candidates = RuntimeManager::candidates(&pc, None);
        for (os, arch) in [
            ("windows", "x86_64"),
            ("windows", "aarch64"),
            ("macos", "aarch64"),
            ("linux", "x86_64"),
        ] {
            assert_eq!(
                auto_choice_on(os, arch, &candidates).map(|c| c.backend.kind),
                Some(BackendKind::Vulkan),
                "{os} {arch}"
            );
        }
        // Only CUDA works: Auto finds nothing, on an Intel Mac as anywhere.
        let cuda_only = RuntimeManager::candidates(&pc[1..2], None);
        for (os, arch) in [("macos", "x86_64"), ("windows", "x86_64")] {
            assert!(
                auto_choice_on(os, arch, &cuda_only).is_none(),
                "{os} {arch}"
            );
        }
    }

    fn resolved(gpu: GpuUse, save_memory: bool, keep_in_memory: bool) -> ResolvedLoad {
        ResolvedLoad {
            ctx: 24_576,
            ctx_choice: crate::load_settings::ContextSize::Auto,
            ctx_source: crate::load_settings::ContextSource::Automatic,
            gpu,
            save_memory,
            keep_in_memory,
            thinking_limit: crate::load_settings::ThinkingLimit::Short,
            backend: None,
            device: None,
            mmproj: None,
        }
    }

    /// The resolved settings reach the launch: the context Automatic produced, the GPU
    /// choice (always off on the processor), Save memory and Keep in memory.
    #[test]
    fn the_launch_carries_the_resolved_settings() {
        let found = vec![
            usable(
                BackendKind::Vulkan,
                vec![device("Vulkan0", "AMD Radeon RX 9070 XT", 16304)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let candidates = RuntimeManager::candidates(&found, None);
        let (vulkan, cpu) = (&candidates[0], &candidates[1]);
        let model = PathBuf::from("/m/model.gguf");
        let launch = |r: &ResolvedLoad, c: &Candidate| {
            launch_config(r, c, model.clone(), None, 4242, "key".into())
        };
        for gpu in [GpuUse::All, GpuUse::Fit, GpuUse::Off] {
            let l = launch(&resolved(gpu, true, true), vulkan);
            assert_eq!(l.gpu, gpu);
            assert_eq!(l.ctx, 24_576);
            assert!(l.save_memory && l.keep_in_memory);
            assert_eq!(l.device.as_deref(), Some("Vulkan0"));
            assert_eq!(l.alias, runtime::MODEL_ALIAS);
            assert_eq!((l.port, l.api_key.as_str()), (4242, "key"));
        }
        // The processor: never the graphics card, whatever the choice.
        let l = launch(&resolved(GpuUse::All, false, false), cpu);
        assert_eq!(l.gpu, GpuUse::Off);
        assert_eq!(l.device, None);
        assert!(!l.save_memory && !l.keep_in_memory);
    }

    /// Reload after a step-down (D2c, D3): the load starts again on the backend the
    /// settings choose, with the graphics card choice they hold, and may step down through
    /// the rest of the list. Nothing of the last load's outcome (the processor it ended
    /// on) is an input: `first_candidate` reads only the probes and the settings.
    #[test]
    fn a_reload_starts_on_the_chosen_backend_again() {
        let found = vec![
            usable(
                BackendKind::Vulkan,
                vec![device("Vulkan0", "AMD Radeon RX 9070 XT", 16304)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        let auto = resolved(GpuUse::All, true, false);
        let (first, all) = first_candidate(&found, &auto).expect("a candidate");
        assert_eq!(first.backend.kind, BackendKind::Vulkan, "Auto's pick");
        assert_eq!(first.device.as_deref(), Some("Vulkan0"));
        let kinds: Vec<BackendKind> = all.iter().map(|c| c.backend.kind).collect();
        assert_eq!(
            kinds,
            [BackendKind::Vulkan, BackendKind::Cpu],
            "the step-down list"
        );
        let launch = launch_config(&auto, &first, "/m.gguf".into(), None, 1, "k".into());
        assert_eq!(launch.gpu, GpuUse::All, "the whole graphics card again");
        assert!(
            launch.save_memory,
            "Save memory as saved, even if a load turned it off"
        );
        let chosen = ResolvedLoad {
            backend: Some("vulkan".into()),
            ..auto.clone()
        };
        assert_eq!(
            first_candidate(&found, &chosen).unwrap().0.backend.kind,
            BackendKind::Vulkan
        );
        // A backend that is not usable is an error, never a quiet switch.
        let gone = ResolvedLoad {
            backend: Some("cuda".into()),
            ..auto
        };
        let e = first_candidate(&found, &gone).unwrap_err();
        assert_eq!(e.code, "backend_unavailable");
    }

    /// Where Automatic sizes for: the user's backend, else Auto's pick.
    #[test]
    fn the_planned_backend_is_the_choice_or_autos_pick() {
        let found = vec![
            usable(
                BackendKind::Vulkan,
                vec![device("Vulkan0", "AMD Radeon RX 9070 XT", 16304)],
            ),
            usable(
                BackendKind::Cuda,
                vec![device("CUDA0", "NVIDIA GeForce RTX 4070", 12000)],
            ),
            usable(BackendKind::Cpu, vec![device("CPU", "CPU", 0)]),
        ];
        assert_eq!(
            planned_backend(&found, None, None),
            Some(BackendKind::Vulkan)
        );
        assert_eq!(
            planned_backend(&found, Some("cuda"), None),
            Some(BackendKind::Cuda)
        );
        assert_eq!(
            planned_backend(&found, Some("cpu"), None),
            Some(BackendKind::Cpu)
        );
        assert_eq!(planned_backend(&found, Some("rocm"), None), None);
        assert_eq!(planned_backend(&[], None, None), None);
    }

    /// A failed load shows no stage any more (a stale "Reading the model" would look like a
    /// load in progress); the adjustments stay, since they explain the failure.
    #[test]
    fn a_failed_load_clears_its_stage_and_keeps_its_adjustments() {
        let step = LoadStage::SteppingDown {
            from: BackendKind::Vulkan,
            to: BackendKind::Cpu,
            reason: LoadFailure::OutOfMemory,
        };
        let mut s = RuntimeStatusDto {
            state: RuntimeStateDto::Starting,
            progress: Some(0.4),
            load_stage: Some(LoadStage::ReadingModel),
            adjustments: vec![step.clone()],
            ..Default::default()
        };
        mark_failed(&mut s, AppError::internal("sample"));
        assert_eq!(s.state, RuntimeStateDto::Failed);
        assert_eq!(s.progress, None);
        assert_eq!(s.load_stage, None);
        assert_eq!(s.adjustments, vec![step]);
        assert!(s.error.is_some());
    }
}
