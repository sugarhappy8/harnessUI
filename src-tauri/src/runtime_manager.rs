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
    Backend, BackendKind, Device, GpuLayers, LaunchConfig, LoadFailure, ProbeResult, RuntimeLayout,
    ServerEvent, ServerState, StartSpec,
};
use tauri::{AppHandle, Emitter};
use tokio::sync::{Mutex, oneshot};
use tokio_util::sync::CancellationToken;

use crate::dto::{BackendDto, DeviceDto, LocalModelSettings, RuntimeStateDto, RuntimeStatusDto};
use crate::error::{AppError, AppResult};

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
    cancel: CancellationToken,
    task: tokio::task::JoinHandle<SupervisorExit>,
}

pub struct RuntimeManager {
    app: AppHandle,
    layout: RuntimeLayout,
    status: StdMutex<RuntimeStatusDto>,
    endpoint: StdMutex<Option<Endpoint>>,
    probes: Mutex<Option<Vec<(Backend, ProbeResult)>>>,
    session: Mutex<Option<Session>>,
    tail: StdMutex<VecDeque<String>>,
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
        }
    }

    pub fn status(&self) -> RuntimeStatusDto {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
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

    /// Probes every backend found on disk (cached unless `reprobe`).
    pub async fn backends(&self, reprobe: bool) -> Vec<(Backend, ProbeResult)> {
        let mut cache = self.probes.lock().await;
        if !reprobe && let Some(found) = cache.as_ref() {
            return found.clone();
        }
        let mut out = Vec::new();
        for backend in runtime::discover(&self.layout) {
            let probe = runtime::probe(&backend, Duration::from_secs(20)).await;
            if let Some(e) = &probe.error {
                tracing::warn!(backend = %backend.kind, "backend probe failed: {e}");
            }
            out.push((backend, probe));
        }
        *cache = Some(out.clone());
        out
    }

    pub async fn backend_dtos(&self, reprobe: bool) -> Vec<BackendDto> {
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
                error: p.error.as_ref().map(|e| e.to_string()),
            })
            .collect()
    }

    /// Backends that passed their probe, each with the device to use, in Auto order:
    /// Metal, Vulkan, CUDA, ROCm, CPU (decision D3 puts Vulkan before ROCm).
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
                out.push(Candidate {
                    backend: b.clone(),
                    device: device.map(|d| d.name.clone()),
                    hip_visible_devices: None,
                });
            }
        }
        out
    }

    /// The first candidate: the user's backend choice, or Auto.
    async fn choose(
        &self,
        settings: &LocalModelSettings,
    ) -> AppResult<(Candidate, Vec<Candidate>)> {
        let found = self.backends(false).await;
        if found.is_empty() {
            return Err(AppError::new(
                "no_runtime",
                "This build does not contain the llama.cpp runtime.",
                "Reinstall the app. On a development build, set APP_DEV_LLAMA_DIR to a folder with llama-server.",
                format!("{:?}", self.layout),
            ));
        }
        let candidates = Self::candidates(&found, settings.device.as_deref());
        let first = match settings.backend.as_deref() {
            Some(choice) => match candidates.iter().find(|c| kind_name(c.backend.kind) == choice) {
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
            None => candidates.first().cloned().ok_or_else(|| {
                AppError::new(
                    "no_usable_backend",
                    "No llama.cpp backend could start on this computer.",
                    "Run the backend check in Settings → Diagnostics and copy the report into a bug report.",
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

    fn tail_text(&self) -> String {
        self.tail
            .lock()
            .map(|t| t.iter().cloned().collect::<Vec<_>>().join("\n"))
            .unwrap_or_default()
    }

    /// Loads `model` (and its projector), replacing any model already loaded. Returns
    /// when the model is ready or has failed on every backend it may step down to.
    pub async fn load(
        self: Arc<Self>,
        model: PathBuf,
        settings: LocalModelSettings,
    ) -> AppResult<()> {
        if !model.is_file() {
            return Err(AppError::new(
                "model_missing",
                "The model file could not be found.",
                "Pick the file again; it may have been moved or deleted.",
                model.display().to_string(),
            ));
        }
        self.unload().await;
        let (first, candidates) = self.choose(&settings).await?;
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
                device: first.device.clone(),
                model_path: Some(model.display().to_string()),
                model_name: Some(model_name.clone()),
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
        let launch = LaunchConfig {
            model: model.clone(),
            mmproj,
            ctx: settings.ctx,
            device: first.device.clone(),
            gpu_layers: if first.backend.kind == BackendKind::Cpu {
                GpuLayers::Count(0)
            } else {
                GpuLayers::All
            },
            cache_ram_mib: 512,
            reasoning_budget: None,
            port,
            api_key: runtime::random_api_key(),
            hip_visible_devices: first.hip_visible_devices.clone(),
        };
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
                        s.device = running.launch.device.clone();
                        s.n_ctx = Some(props.n_ctx);
                        s.supports_tools = props.chat_template_caps.supports_tool_calls;
                        s.vision = props.modalities.vision;
                        s.error = None;
                    });
                    if let Ok(mut slot) = ready_tx.lock()
                        && let Some(tx) = slot.take()
                    {
                        let _ = tx.send(Ok(()));
                    }
                })
                .await;
            if let Ok(mut e) = me.endpoint.lock() {
                *e = None;
            }
            crate::children::untrack(&me.app);
            exit
        });
        *self.session.lock().await = Some(Session { cancel, task });

        // Wait for the first ready server, or for the supervisor to give up.
        let outcome = ready_rx.await;
        if matches!(outcome, Ok(Ok(()))) {
            tracing::info!(model = %model_name, "model ready");
            return Ok(());
        }
        let exit = match self.session.lock().await.take() {
            Some(s) => s.task.await.unwrap_or(SupervisorExit::Stopped),
            None => SupervisorExit::Stopped,
        };
        let err = match exit {
            SupervisorExit::Failed(failure) => load_error(&failure, self.tail_text()),
            SupervisorExit::Stopped => AppError::new(
                "load_cancelled",
                "Loading was cancelled.",
                "Pick the model again to load it.",
                String::new(),
            ),
        };
        self.update(|s| {
            s.state = RuntimeStateDto::Failed;
            s.progress = None;
            s.error = Some(err.clone());
        });
        Err(err)
    }

    fn on_event(&self, event: ServerEvent) {
        match event {
            ServerEvent::Progress { stage, value } => self.update(|s| {
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
                    s.progress = None;
                });
            }
            ServerEvent::SteppedDown { from, to, reason } => {
                tracing::warn!(%from, %to, ?reason, "stepping down to another backend");
                self.update(|s| {
                    s.state = RuntimeStateDto::Starting;
                    s.backend = Some(kind_name(to).to_string());
                    s.stage = Some(format!(
                        "{} failed ({}); trying {}",
                        kind_name(from),
                        reason.user_message(),
                        kind_name(to)
                    ));
                });
            }
            ServerEvent::State(ServerState::Failed(failure)) => {
                let err = load_error(&failure, self.tail_text());
                self.update(|s| {
                    s.state = RuntimeStateDto::Failed;
                    s.progress = None;
                    s.error = Some(err);
                });
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

pub fn kind_name(kind: BackendKind) -> &'static str {
    kind.as_str()
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
pub fn local_client(base_url: String, key: String) -> AppResult<agent::OpenAiClient> {
    let mut config = agent::ClientConfig::new(base_url, agent::ProviderKind::LlamaServer);
    config.api_key = Some(key);
    config.loopback = true;
    config.request_timeout = None;
    config.connect_timeout = Duration::from_secs(5);
    agent::OpenAiClient::new(config).map_err(AppError::from)
}
