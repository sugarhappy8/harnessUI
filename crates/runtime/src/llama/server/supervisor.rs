//! Restart and step-down policy (ARCHITECTURE.md 7.4).
//!
//! - **Crash while Ready:** restart with the same arguments, up to
//!   [`RestartPolicy::max_restarts`] times within [`RestartPolicy::window`].
//! - **Retries on the same backend (0.0.6)**, when the server exits while loading or the
//!   crash limit is reached, before any step-down, each at most once per load (a process
//!   that could not be created, a load timeout or an unusable `/props` reply changes no
//!   setting):
//!   1. **Save memory off** ([`LoadStage::RetryingWithoutSaveMemory`]) when it was on and
//!      either the log shows the model refusing it ([`save_memory_refused`]: a q8_0 block
//!      that does not divide the head size, a model without flash attention; on any
//!      backend, the processor included), or a graphics card is in use and the failure
//!      is one it can cause ([`LoadFailure::save_memory_may_be_the_cause`], 0.0.6 D2c). It
//!      stays off for the rest of the load. Without a card in use, a plain failure is
//!      never retried without it: that would need twice the memory and blame the card.
//!   2. **Fast attention off** ([`LoadStage::RetryingWithoutFastAttention`], item 6) when
//!      fast attention was chosen On in Fine control with Save memory off, a graphics card
//!      is in use and the failure is one Save memory can also cause
//!      ([`LoadFailure::save_memory_may_be_the_cause`]): a card or driver that cannot do it.
//!      It stays off for the rest of the load.
//!   3. **As much as fits** ([`LoadStage::RetryingWithLessGpu`]) when a graphics card ran
//!      out of memory ([`LoadFailure::is_out_of_memory`]) with All of it ([`GpuUse::All`])
//!      or with layers or expert parts chosen in Fine control (a custom placement, item 6)
//!      that put part of the model on the card. The chosen placement is set aside for the
//!      rest of the load (the stage says so, with the chosen number of layers); the margin
//!      As much as fits leaves free and where the conversation memory lives are kept. A
//!      placement of 0 layers puts none of the model on the card, so As much as fits would
//!      put more there than chosen: such a load steps down instead.
//! - **Stepping down:** when a GPU backend still fails, and a lower backend passed its
//!   probe, try the next one down (ROCm or CUDA → Vulkan → CPU) and report
//!   [`ServerEvent::SteppedDown`]. The processor runs with [`GpuUse::Off`]; another
//!   graphics backend keeps the card choice reached so far. The chain is finite and every
//!   retry happens once, so this ends. Without a card in use, running out of memory does
//!   not step down: the processor build would repeat the same load.
//! - **Port taken:** pick a new port, up to [`RestartPolicy::port_retries`] times.
//! - **Events:** [`ServerState::Failed`] is reported once, when the supervisor gives up. An
//!   attempt it retries does not report it ([`LocalStarter`]), so a retry on the same
//!   backend reads `Starting`, the [`ServerEvent::Stage`], `Starting`, `Ready`, and a
//!   step-down reads `Starting`, [`ServerEvent::SteppedDown`], `Starting`, `Ready`.
//!   [`LoadStage::RetryingWithoutSaveMemory`] names where the model runs: the processor
//!   when no graphics card is in use, whatever the backend's label.
//!
//! "A graphics card in use" means what the command line does, not the backend's label: a
//! graphics backend with a device and a card choice other than Don't use it
//! ([`LaunchConfig::uses_device`](crate::llama::launch::LaunchConfig::uses_device)).
//! Don't use it on Vulkan runs on the processor (`--device none`) and is treated so.
//!
//! So a graphics card that runs out of memory goes: Save memory off (if it was on and the
//! context failed), fast attention off (if it was chosen On and the context failed), then
//! As much as fits, then the processor. A failure to allocate the weights skips the first
//! two steps, since Save memory and fast attention only change the conversation memory and
//! the working space, which are made after the weights. The saved settings never change:
//! every step is for this load only.
//!
//! The policy talks to servers only through the [`Starter`] and [`Supervised`] traits, so
//! tests drive it with scripted fakes and no real process.

use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{RunningServer, ServerEvent, ServerState, StartError, StartSpec, start};
use crate::llama::backend::{Backend, BackendKind};
use crate::llama::failure::{LoadFailure, classify, save_memory_refused};
use crate::llama::launch::{ExpertsOnCpu, FlashAttn, GpuUse, MAX_LAYER_COUNT, free_loopback_port};
use crate::llama::stage::LoadStage;

/// Starts a server. Implemented by [`LocalStarter`] for real llama-server processes and by
/// fakes in tests.
pub trait Starter: Send + Sync {
    /// The handle of a started server.
    type Server: Supervised;

    /// Starts one server for `spec`.
    fn start(
        &self,
        spec: StartSpec,
    ) -> impl Future<Output = Result<Self::Server, StartError>> + Send;
}

/// A started server as the supervisor sees it.
pub trait Supervised: Send {
    /// Resolves when the server exits on its own, with its exit code.
    fn wait_exit(&mut self) -> impl Future<Output = Option<i32>> + Send;

    /// Stops the server and waits for it to exit.
    fn stop(self) -> impl Future<Output = ()> + Send;

    /// Why the server crashed, from whatever it logged. Unknown by default.
    fn crash_reason(&self, exit_code: Option<i32>) -> LoadFailure {
        let _ = exit_code;
        LoadFailure::Unknown
    }
}

/// Starts real llama-server processes with [`start`].
///
/// Each start is one attempt of a load. What the attempt reports is passed on to the
/// starter's events in the order it was sent, except the attempt's own
/// [`ServerState::Failed`]: the supervisor may still retry (a new port, a setting turned
/// down, a backend lower), and it reports [`ServerState::Failed`] itself, once, when it
/// gives up. So the app never sees a load as failed while it is being retried, and a load
/// that fails for good is reported once.
#[derive(Clone, Debug)]
pub struct LocalStarter {
    events: UnboundedSender<ServerEvent>,
}

impl LocalStarter {
    /// A starter that reports to `events`.
    pub fn new(events: UnboundedSender<ServerEvent>) -> Self {
        Self { events }
    }
}

impl Starter for LocalStarter {
    type Server = LocalServer;

    fn start(
        &self,
        spec: StartSpec,
    ) -> impl Future<Output = Result<LocalServer, StartError>> + Send {
        let events = self.events.clone();
        async move {
            let (attempt_events, mut from_server) = unbounded_channel();
            // Every reader of a failed attempt has finished by the time `start` returns,
            // so nothing of it is left behind in `from_server`.
            relay_until(start(spec, attempt_events), &mut from_server, &events)
                .await
                .map(|server| LocalServer {
                    server,
                    from_server,
                    events,
                })
        }
    }
}

/// A llama-server started by [`LocalStarter`]: the [`RunningServer`], whose methods it
/// offers through `Deref`, and the events it still sends. Those are passed on while the
/// supervisor waits for the server to exit, and when it stops: [`RunningServer`] reports
/// everything up to its exit (the last log lines, then [`ServerEvent::Exited`]) before
/// either returns, so the supervisor's own events still come after them.
pub struct LocalServer {
    server: RunningServer,
    from_server: UnboundedReceiver<ServerEvent>,
    events: UnboundedSender<ServerEvent>,
}

impl LocalServer {
    /// Stops the server ([`RunningServer::stop`]) and passes on what it reported, ending
    /// with [`ServerState::Stopped`].
    pub async fn stop(self) {
        let Self {
            server,
            mut from_server,
            events,
        } = self;
        server.stop().await;
        while let Ok(event) = from_server.try_recv() {
            relay(&events, event);
        }
    }
}

impl std::ops::Deref for LocalServer {
    type Target = RunningServer;

    fn deref(&self) -> &RunningServer {
        &self.server
    }
}

impl std::fmt::Debug for LocalServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.server.fmt(f)
    }
}

impl Supervised for LocalServer {
    fn wait_exit(&mut self) -> impl Future<Output = Option<i32>> + Send {
        let Self {
            server,
            from_server,
            events,
        } = self;
        relay_until(server.wait_exit(), from_server, events)
    }

    fn stop(self) -> impl Future<Output = ()> + Send {
        LocalServer::stop(self)
    }

    fn crash_reason(&self, exit_code: Option<i32>) -> LoadFailure {
        classify(&self.server.stderr_tail(), exit_code)
    }
}

/// Awaits `work` (a start, or a wait for the exit) while passing on what the server
/// reports, then whatever it reported before `work` ended, all in the order it was sent.
async fn relay_until<T>(
    work: impl Future<Output = T>,
    from_server: &mut UnboundedReceiver<ServerEvent>,
    events: &UnboundedSender<ServerEvent>,
) -> T {
    let mut work = std::pin::pin!(work);
    // `work` first, so a server that logs without pause cannot keep it from being polled;
    // what is still queued when it ends is passed on below.
    let result = loop {
        tokio::select! {
            biased;
            result = &mut work => break result,
            Some(event) = from_server.recv() => relay(events, event),
        }
    };
    while let Ok(event) = from_server.try_recv() {
        relay(events, event);
    }
    result
}

/// Passes one event of a server on, except its own [`ServerState::Failed`] (see
/// [`LocalStarter`]).
fn relay(events: &UnboundedSender<ServerEvent>, event: ServerEvent) {
    if !matches!(event, ServerEvent::State(ServerState::Failed(_))) {
        let _ = events.send(event);
    }
}

impl Supervised for RunningServer {
    fn wait_exit(&mut self) -> impl Future<Output = Option<i32>> + Send {
        RunningServer::wait_exit(self)
    }

    fn stop(self) -> impl Future<Output = ()> + Send {
        RunningServer::stop(self)
    }

    fn crash_reason(&self, exit_code: Option<i32>) -> LoadFailure {
        classify(&self.stderr_tail(), exit_code)
    }
}

/// The supervisor's limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RestartPolicy {
    /// Restarts allowed within [`window`](Self::window) after unexpected exits.
    pub max_restarts: u32,
    /// The sliding window for counting crashes.
    pub window: Duration,
    /// New ports tried when the chosen one turns out to be taken.
    pub port_retries: u32,
}

impl Default for RestartPolicy {
    /// Three restarts in five minutes; three new ports.
    fn default() -> Self {
        Self {
            max_restarts: 3,
            window: Duration::from_secs(5 * 60),
            port_retries: 3,
        }
    }
}

/// A backend that passed its probe, with the device to use on it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The backend.
    pub backend: Backend,
    /// The device to pass to `--device` (`None` for CPU).
    pub device: Option<String>,
    /// `HIP_VISIBLE_DEVICES` for ROCm.
    pub hip_visible_devices: Option<String>,
}

/// How [`Supervisor::run`] ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SupervisorExit {
    /// The cancellation token fired and the server was stopped.
    Stopped,
    /// The start failed, or the restart policy gave up, with the last reason.
    Failed(LoadFailure),
}

/// The next backend down from `kind` that is in `available`: ROCm or CUDA → Vulkan → CPU,
/// Vulkan or Metal → CPU, CPU → none. `available` should hold only backends that passed
/// their probe; the first match in its order wins.
pub fn step_down(kind: BackendKind, available: &[Backend]) -> Option<Backend> {
    let order: &[BackendKind] = match kind {
        BackendKind::Rocm | BackendKind::Cuda => &[BackendKind::Vulkan, BackendKind::Cpu],
        BackendKind::Vulkan | BackendKind::Metal => &[BackendKind::Cpu],
        BackendKind::Cpu => &[],
    };
    order.iter().find_map(|wanted| {
        available
            .iter()
            .find(|backend| backend.kind == *wanted)
            .cloned()
    })
}

/// Whether a graphics card does part of the work of `spec`: a graphics backend whose
/// command line names a device ([`LaunchConfig::uses_device`]). Don't use it, or no device,
/// runs on the processor whatever the backend's label.
///
/// [`LaunchConfig::uses_device`]: crate::llama::launch::LaunchConfig::uses_device
fn uses_graphics_card(spec: &StartSpec) -> bool {
    spec.backend.kind != BackendKind::Cpu && spec.launch.uses_device()
}

/// Whether As much as fits puts less of `spec`'s model on the graphics card than it asked
/// for: All of it, or a custom placement (Fine control, item 6) that puts part of the model
/// on the card. A placement of 0 layers puts none there (expert flags then have nothing on
/// the card to move), so As much as fits would put more there than the user chose.
fn turned_down_by_fit(spec: &StartSpec) -> bool {
    let fine = &spec.launch.fine;
    if fine.custom_placement() {
        fine.gpu_layers != Some(0)
    } else {
        spec.launch.gpu == GpuUse::All
    }
}

/// Runs one model with the restart, step-down and new-port policy.
#[derive(Debug)]
pub struct Supervisor<S: Starter> {
    starter: S,
    policy: RestartPolicy,
    candidates: Vec<Candidate>,
    events: UnboundedSender<ServerEvent>,
    crashes: VecDeque<Instant>,
}

impl Supervisor<LocalStarter> {
    /// A supervisor for real llama-server processes.
    pub fn local(candidates: Vec<Candidate>, events: UnboundedSender<ServerEvent>) -> Self {
        Self::new(LocalStarter::new(events.clone()), candidates, events)
    }
}

impl<S: Starter> Supervisor<S> {
    /// A supervisor with the default [`RestartPolicy`]. `candidates` are the backends that
    /// passed their probe, used for stepping down.
    pub fn new(
        starter: S,
        candidates: Vec<Candidate>,
        events: UnboundedSender<ServerEvent>,
    ) -> Self {
        Self {
            starter,
            policy: RestartPolicy::default(),
            candidates,
            events,
            crashes: VecDeque::new(),
        }
    }

    /// Replaces the policy.
    pub fn with_policy(mut self, policy: RestartPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Starts `spec`, trying a new port when the port is taken, the retries on the same
    /// backend (Save memory off, As much as fits) and the next backend down when the
    /// failure could be backend-specific. Returns the server and the spec it actually runs
    /// with, so the caller can tell which load settings are in effect. A failure is
    /// returned, not sent as [`ServerState::Failed`]: [`run`](Self::run) sends that.
    pub async fn start(&mut self, spec: StartSpec) -> Result<(S::Server, StartSpec), StartError> {
        let mut spec = spec;
        let mut port_attempts = 0;
        loop {
            let error = match self.starter.start(spec.clone()).await {
                Ok(server) => return Ok((server, spec)),
                Err(error) => error,
            };
            if matches!(error, StartError::InvalidConfig(_)) {
                // The same settings are invalid on every backend.
                return Err(error);
            }
            let failure = error.failure();
            if failure == LoadFailure::PortInUse && port_attempts < self.policy.port_retries {
                port_attempts += 1;
                spec.launch.port = free_loopback_port().map_err(StartError::Spawn)?;
                continue;
            }
            // Only a server that exited while loading can have failed because of a load
            // setting. A process that could not be created, a load that ran out of time and
            // an unusable `/props` reply are unknown failures too, but turning a setting
            // down for them would blame it wrongly (and repeat a whole load timeout).
            if matches!(error, StartError::Exited { .. })
                && let Some(retry) = self.retry_spec(&spec, &failure, error.stderr_tail())
            {
                spec = retry;
                port_attempts = 0;
                continue;
            }
            if failure.may_succeed_on_another_backend()
                && let Some(lower) = self.step_down_spec(&spec, &failure)
            {
                spec = lower;
                port_attempts = 0;
                continue;
            }
            return Err(error);
        }
    }

    /// Starts `spec` and keeps it running until `cancel` fires (the server is stopped and
    /// the result is [`SupervisorExit::Stopped`]) or the policy gives up
    /// ([`SupervisorExit::Failed`], also emitted as [`ServerState::Failed`]).
    ///
    /// `on_ready` is called with every server that becomes ready, the first one and each
    /// replacement, together with the spec it runs with (port, backend and device may
    /// differ from `spec`), so the app can publish the current endpoint.
    pub async fn run<F>(
        mut self,
        spec: StartSpec,
        cancel: CancellationToken,
        mut on_ready: F,
    ) -> SupervisorExit
    where
        F: FnMut(&S::Server, &StartSpec) + Send,
    {
        if cancel.is_cancelled() {
            return SupervisorExit::Stopped;
        }
        let started = tokio::select! {
            biased;
            // Dropping the start kills the half-started process tree.
            () = cancel.cancelled() => return self.stopped_while_starting(),
            started = self.start(spec) => started,
        };
        let (mut server, mut spec) = match started {
            Ok(running) => running,
            Err(error) => return self.failed(error.failure()),
        };
        loop {
            on_ready(&server, &spec);
            let code = tokio::select! {
                biased;
                () = cancel.cancelled() => {
                    server.stop().await;
                    return SupervisorExit::Stopped;
                }
                code = server.wait_exit() => code,
            };
            let reason = server.crash_reason(code);
            drop(server);

            let next = if let Some(attempt) = self.record_crash(Instant::now()) {
                let _ = self.events.send(ServerEvent::Restarting {
                    attempt,
                    max: self.policy.max_restarts,
                });
                spec.clone()
            } else if let Some(retry) = self.retry_spec(&spec, &reason, &[]) {
                // No log lines: this server became ready, so its context was made and the
                // model did not refuse Save memory; only a card in use retries here.
                self.crashes.clear();
                retry
            } else if let Some(lower) = self.step_down_spec(&spec, &reason) {
                self.crashes.clear();
                lower
            } else {
                return self.failed(reason);
            };

            let restarted = tokio::select! {
                biased;
                () = cancel.cancelled() => return self.stopped_while_starting(),
                restarted = self.start(next) => restarted,
            };
            match restarted {
                Ok((replacement, replacement_spec)) => {
                    server = replacement;
                    spec = replacement_spec;
                }
                Err(error) => return self.failed(error.failure()),
            }
        }
    }

    /// Records a crash at `now`. Returns the restart number within the window when another
    /// restart is allowed, `None` when the limit is reached.
    fn record_crash(&mut self, now: Instant) -> Option<u32> {
        while self
            .crashes
            .front()
            .is_some_and(|&first| now.duration_since(first) > self.policy.window)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        let count = u32::try_from(self.crashes.len()).unwrap_or(u32::MAX);
        (count <= self.policy.max_restarts).then_some(count)
    }

    /// The spec for the next retry on the same backend after `reason` (with the failed
    /// load's last log lines), and the event that tells the user; `None` when no retry
    /// applies (see the module documentation).
    ///
    /// Each retry turns a setting down for the rest of the load, so it can happen only once:
    /// Save memory is then off, fast attention is then Off, and the card is then at As much
    /// as fits with no custom placement.
    fn retry_spec(
        &self,
        spec: &StartSpec,
        reason: &LoadFailure,
        stderr_tail: &[String],
    ) -> Option<StartSpec> {
        let backend = spec.backend.kind;
        let card = uses_graphics_card(spec);
        let mut next = spec.clone();
        // The model refusing Save memory is retried on any backend. Otherwise only a card
        // in use is: a device or driver may lack what a q8_0 memory needs, while on the
        // processor a plain failure is a lack of memory, which turning it off makes worse.
        let fine = &spec.launch.fine;
        let custom = fine.custom_placement();
        let stage = if spec.launch.save_memory
            && (save_memory_refused(stderr_tail) || (card && reason.save_memory_may_be_the_cause()))
        {
            next.launch.save_memory = false;
            // Where the model runs: without a card in use only the model can refuse it, and
            // the processor is what the user is told about, not a card the load never used.
            let backend = if card { backend } else { BackendKind::Cpu };
            LoadStage::RetryingWithoutSaveMemory { backend }
        } else if !spec.launch.save_memory
            && fine.flash_attn == FlashAttn::On
            && card
            && reason.save_memory_may_be_the_cause()
        {
            // Item 6, section 3.3 step 2: fast attention chosen On fails the way Save memory
            // can fail on a graphics card that cannot do it. Off for the rest of the load.
            next.launch.fine.flash_attn = FlashAttn::Off;
            LoadStage::RetryingWithoutFastAttention { backend }
        } else if card && turned_down_by_fit(spec) && reason.is_out_of_memory() {
            // All of it, or a placement chosen in Fine control (item 6, section 3.3 step 3),
            // ran out of graphics memory: As much as fits for the rest of the load. The
            // chosen layers and expert parts are set aside; the margin As much as fits
            // leaves free and the conversation memory's place are kept.
            next.launch.gpu = GpuUse::Fit;
            next.launch.fine.gpu_layers = None;
            next.launch.fine.experts_on_cpu = ExpertsOnCpu::None;
            LoadStage::RetryingWithLessGpu {
                backend,
                custom,
                // The count the command line named (above its limit it said `all`).
                chosen_layers: if custom {
                    fine.gpu_layers.filter(|&n| n <= MAX_LAYER_COUNT)
                } else {
                    None
                },
            }
        } else {
            return None;
        };
        tracing::warn!(?stage, ?reason, "retrying the load on the same backend");
        let _ = self.events.send(ServerEvent::Stage(stage));
        Some(next)
    }

    /// The spec for the next backend down, and the event that tells the user.
    ///
    /// Without a graphics card in use the load already runs on the processor, so the
    /// processor build would repeat it: for running out of memory that cannot help, and
    /// its note would say the graphics card ran out of memory. A failure of the backend
    /// itself (a crash, a missing library) still steps down.
    fn step_down_spec(&self, spec: &StartSpec, reason: &LoadFailure) -> Option<StartSpec> {
        if !uses_graphics_card(spec) && reason.is_out_of_memory() {
            return None;
        }
        let backends: Vec<Backend> = self.candidates.iter().map(|c| c.backend.clone()).collect();
        let lower = step_down(spec.backend.kind, &backends)?;
        let candidate = self.candidates.iter().find(|c| c.backend == lower)?;
        let mut next = spec.clone();
        next.backend = lower;
        if candidate.backend.kind == BackendKind::Cpu {
            next.launch.device = None;
            next.launch.gpu = GpuUse::Off;
        } else {
            next.launch.device = candidate.device.clone();
        }
        next.launch.hip_visible_devices = candidate.hip_visible_devices.clone();
        let _ = self.events.send(ServerEvent::SteppedDown {
            from: spec.backend.kind,
            to: next.backend.kind,
            reason: reason.clone(),
        });
        Some(next)
    }

    /// A start was abandoned because `cancel` fired: report the terminal state that a
    /// running server's `stop` would have reported.
    fn stopped_while_starting(&self) -> SupervisorExit {
        let _ = self.events.send(ServerEvent::State(ServerState::Stopped));
        SupervisorExit::Stopped
    }

    fn failed(&self, reason: LoadFailure) -> SupervisorExit {
        let _ = self
            .events
            .send(ServerEvent::State(ServerState::Failed(reason.clone())));
        SupervisorExit::Failed(reason)
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::*;
    use crate::llama::backend::BackendSource;
    use crate::llama::launch::{ExpertsOnCpu, FineLaunch, FlashAttn, GpuUse, LaunchConfig};
    use crate::llama::stage::LoadStage;

    /// What a scripted start does.
    #[derive(Clone, Debug)]
    enum Script {
        /// Fails to load with this reason.
        Fail(LoadFailure),
        /// Fails to load with this reason and these last lines of its log.
        FailLog(LoadFailure, Vec<String>),
        /// Becomes ready, then exits after this long (`None`: runs until stopped).
        Run(Option<Duration>),
        /// Becomes ready, then crashes after this long with this reason in its log.
        Crash(Duration, LoadFailure),
        /// Never finishes starting (a model that loads forever).
        Hang,
        /// Refuses the settings before starting anything.
        Invalid,
        /// The process could not be created (a missing or blocked executable).
        SpawnFails,
        /// Did not become ready within the load timeout, nothing recognisable logged.
        TimesOut,
        /// Became ready, but `/props` was unusable.
        PropsFail,
    }

    #[derive(Debug, Default)]
    struct Log {
        started: Vec<StartSpec>,
        stopped: usize,
    }

    struct FakeStarter {
        script: Mutex<VecDeque<Script>>,
        log: Arc<Mutex<Log>>,
    }

    #[derive(Debug)]
    struct FakeServer {
        exit_after: Option<Duration>,
        reason: LoadFailure,
        log: Arc<Mutex<Log>>,
    }

    impl Starter for FakeStarter {
        type Server = FakeServer;

        async fn start(&self, spec: StartSpec) -> Result<FakeServer, StartError> {
            self.log.lock().expect("log").started.push(spec);
            let step = self
                .script
                .lock()
                .expect("script")
                .pop_front()
                .unwrap_or(Script::Run(None));
            match step {
                Script::Fail(failure) => Err(StartError::Exited {
                    failure,
                    exit_code: Some(1),
                    stderr_tail: vec!["boom".into()],
                }),
                Script::FailLog(failure, stderr_tail) => Err(StartError::Exited {
                    failure,
                    exit_code: Some(1),
                    stderr_tail,
                }),
                Script::Run(exit_after) => Ok(FakeServer {
                    exit_after,
                    reason: LoadFailure::Unknown,
                    log: self.log.clone(),
                }),
                Script::Crash(after, reason) => Ok(FakeServer {
                    exit_after: Some(after),
                    reason,
                    log: self.log.clone(),
                }),
                Script::Hang => std::future::pending().await,
                Script::Invalid => Err(StartError::InvalidConfig("bad settings".into())),
                Script::SpawnFails => Err(StartError::Spawn(std::io::Error::from(
                    std::io::ErrorKind::PermissionDenied,
                ))),
                Script::TimesOut => Err(StartError::Timeout {
                    after: Duration::from_secs(60),
                    failure: LoadFailure::Unknown,
                    stderr_tail: vec!["load_tensors: loading".into()],
                }),
                Script::PropsFail => Err(StartError::Props {
                    message: "no model_path".into(),
                    stderr_tail: Vec::new(),
                }),
            }
        }
    }

    impl Supervised for FakeServer {
        async fn wait_exit(&mut self) -> Option<i32> {
            match self.exit_after {
                Some(after) => {
                    tokio::time::sleep(after).await;
                    Some(134)
                }
                None => std::future::pending().await,
            }
        }

        async fn stop(self) {
            self.log.lock().expect("log").stopped += 1;
        }

        fn crash_reason(&self, _exit_code: Option<i32>) -> LoadFailure {
            self.reason.clone()
        }
    }

    fn backend(kind: BackendKind) -> Backend {
        Backend {
            kind,
            dir: PathBuf::from(format!("/rt/{}", kind.as_str())),
            server: PathBuf::from(format!("/rt/{}/llama-server", kind.as_str())),
            fit_params: None,
            build: "b1".into(),
            source: BackendSource::Installed,
        }
    }

    fn candidate(kind: BackendKind, device: Option<&str>) -> Candidate {
        Candidate {
            backend: backend(kind),
            device: device.map(str::to_owned),
            hip_visible_devices: None,
        }
    }

    fn spec(kind: BackendKind, device: Option<&str>) -> StartSpec {
        StartSpec {
            backend: backend(kind),
            launch: LaunchConfig {
                model: PathBuf::from("/m/model.gguf"),
                mmproj: None,
                ctx: 4096,
                device: device.map(str::to_owned),
                cache_ram_mib: 256,
                reasoning_budget: None,
                port: 41000,
                api_key: "k".into(),
                hip_visible_devices: (kind == BackendKind::Rocm).then(|| "1".to_owned()),
                gpu: if kind == BackendKind::Cpu {
                    GpuUse::Off
                } else {
                    GpuUse::All
                },
                ..LaunchConfig::default()
            },
            load_timeout: Duration::from_secs(60),
        }
    }

    /// [`spec`] with these load settings.
    fn spec_with(
        kind: BackendKind,
        device: Option<&str>,
        gpu: GpuUse,
        save_memory: bool,
    ) -> StartSpec {
        let mut spec = spec(kind, device);
        spec.launch.gpu = gpu;
        spec.launch.save_memory = save_memory;
        spec
    }

    /// Backend, graphics card use and Save memory of every start, in order.
    fn attempts(log: &Arc<Mutex<Log>>) -> Vec<(BackendKind, GpuUse, bool)> {
        log.lock()
            .expect("log")
            .started
            .iter()
            .map(|s| (s.backend.kind, s.launch.gpu, s.launch.save_memory))
            .collect()
    }

    fn gpu_cpu() -> Vec<Candidate> {
        vec![
            candidate(BackendKind::Vulkan, Some("Vulkan0")),
            candidate(BackendKind::Cpu, None),
        ]
    }

    fn without_save_memory(backend: BackendKind) -> ServerEvent {
        ServerEvent::Stage(LoadStage::RetryingWithoutSaveMemory { backend })
    }

    fn with_less_gpu(backend: BackendKind) -> ServerEvent {
        ServerEvent::Stage(LoadStage::with_less_gpu(backend))
    }

    fn supervisor(
        script: Vec<Script>,
        candidates: Vec<Candidate>,
    ) -> (
        Supervisor<FakeStarter>,
        Arc<Mutex<Log>>,
        UnboundedReceiver<ServerEvent>,
    ) {
        let log = Arc::new(Mutex::new(Log::default()));
        let (tx, rx) = unbounded_channel();
        let starter = FakeStarter {
            script: Mutex::new(script.into()),
            log: log.clone(),
        };
        (Supervisor::new(starter, candidates, tx), log, rx)
    }

    fn drain(rx: &mut UnboundedReceiver<ServerEvent>) -> Vec<ServerEvent> {
        let mut out = Vec::new();
        while let Ok(event) = rx.try_recv() {
            out.push(event);
        }
        out
    }

    fn minutes(n: u64) -> Duration {
        Duration::from_secs(n * 60)
    }

    #[test]
    fn step_down_order() {
        let all = vec![
            backend(BackendKind::Cpu),
            backend(BackendKind::Vulkan),
            backend(BackendKind::Rocm),
        ];
        assert_eq!(
            step_down(BackendKind::Rocm, &all).map(|b| b.kind),
            Some(BackendKind::Vulkan)
        );
        assert_eq!(
            step_down(BackendKind::Cuda, &all).map(|b| b.kind),
            Some(BackendKind::Vulkan)
        );
        assert_eq!(
            step_down(BackendKind::Vulkan, &all).map(|b| b.kind),
            Some(BackendKind::Cpu)
        );
        assert_eq!(
            step_down(BackendKind::Metal, &all).map(|b| b.kind),
            Some(BackendKind::Cpu)
        );
        assert_eq!(step_down(BackendKind::Cpu, &all), None);
        let no_vulkan = vec![backend(BackendKind::Cpu)];
        assert_eq!(
            step_down(BackendKind::Rocm, &no_vulkan).map(|b| b.kind),
            Some(BackendKind::Cpu)
        );
        assert_eq!(step_down(BackendKind::Vulkan, &[]), None);
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_three_times_in_five_minutes_then_fails() {
        let crash = Script::Run(Some(Duration::from_secs(10)));
        let (sup, log, mut rx) = supervisor(
            vec![crash.clone(), crash.clone(), crash.clone(), crash],
            vec![],
        );
        let mut ready = 0;
        let exit = sup
            .run(
                spec(BackendKind::Vulkan, Some("Vulkan0")),
                CancellationToken::new(),
                |_, _| ready += 1,
            )
            .await;
        assert_eq!(exit, SupervisorExit::Failed(LoadFailure::Unknown));
        assert_eq!(ready, 4);
        assert_eq!(log.lock().expect("log").started.len(), 4);
        let events = drain(&mut rx);
        let restarts: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                ServerEvent::Restarting { attempt, max } => Some((*attempt, *max)),
                _ => None,
            })
            .collect();
        assert_eq!(restarts, vec![(1, 3), (2, 3), (3, 3)]);
        assert_eq!(
            events.last(),
            Some(&ServerEvent::State(ServerState::Failed(
                LoadFailure::Unknown
            )))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn crashes_spread_beyond_the_window_keep_restarting() {
        let crash = Script::Run(Some(minutes(6)));
        let script = vec![
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash,
            Script::Run(None),
        ];
        let (sup, log, mut rx) = supervisor(script, vec![]);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let mut ready = 0;
        let exit = sup
            .run(spec(BackendKind::Cpu, None), cancel, move |_, _| {
                ready += 1;
                if ready == 6 {
                    stopper.cancel();
                }
            })
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        let log = log.lock().expect("log");
        assert_eq!(log.started.len(), 6);
        assert_eq!(log.stopped, 1);
        let attempts: Vec<u32> = drain(&mut rx)
            .iter()
            .filter_map(|e| match e {
                ServerEvent::Restarting { attempt, .. } => Some(*attempt),
                _ => None,
            })
            .collect();
        assert_eq!(
            attempts,
            vec![1, 1, 1, 1, 1],
            "each crash is alone in its window"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn crash_limit_steps_down_to_cpu() {
        let crash = Script::Run(Some(Duration::from_secs(1)));
        let script = vec![
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash,
            Script::Run(None),
        ];
        let candidates = vec![
            candidate(BackendKind::Vulkan, Some("Vulkan0")),
            candidate(BackendKind::Cpu, None),
        ];
        let (sup, log, mut rx) = supervisor(script, candidates);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let exit = sup
            .run(
                spec(BackendKind::Vulkan, Some("Vulkan0")),
                cancel,
                move |_, spec| {
                    if spec.backend.kind == BackendKind::Cpu {
                        stopper.cancel();
                    }
                },
            )
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        let started = log.lock().expect("log").started.clone();
        assert_eq!(started.len(), 5);
        let last = started.last().expect("last");
        assert_eq!(last.backend.kind, BackendKind::Cpu);
        assert_eq!(last.launch.device, None);
        assert_eq!(
            last.launch.gpu,
            GpuUse::Off,
            "the processor uses no graphics card"
        );
        assert!(drain(&mut rx).contains(&ServerEvent::SteppedDown {
            from: BackendKind::Vulkan,
            to: BackendKind::Cpu,
            reason: LoadFailure::Unknown,
        }));
    }

    #[tokio::test(start_paused = true)]
    async fn load_failure_on_rocm_steps_down_to_vulkan_with_its_device() {
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let candidates = vec![
            candidate(BackendKind::Rocm, Some("ROCm0")),
            candidate(BackendKind::Vulkan, Some("Vulkan1")),
            candidate(BackendKind::Cpu, None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, candidates);
        // Already As much as fits, so running out of memory steps down at once (with All
        // of it, the supervisor would first retry ROCm with less on the card).
        let (_server, running) = sup
            .start(spec_with(
                BackendKind::Rocm,
                Some("ROCm0"),
                GpuUse::Fit,
                false,
            ))
            .await
            .expect("starts");
        assert_eq!(running.backend.kind, BackendKind::Vulkan);
        assert_eq!(running.launch.device.as_deref(), Some("Vulkan1"));
        assert_eq!(
            running.launch.gpu,
            GpuUse::Fit,
            "the same card on another backend keeps As much as fits"
        );
        assert_eq!(
            running.launch.hip_visible_devices, None,
            "HIP masking is dropped with ROCm"
        );
        assert_eq!(log.lock().expect("log").started.len(), 2);
        assert_eq!(
            drain(&mut rx),
            vec![ServerEvent::SteppedDown {
                from: BackendKind::Rocm,
                to: BackendKind::Vulkan,
                reason: LoadFailure::OutOfMemory
            }]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn save_memory_failure_retries_once_without_it_on_the_same_backend() {
        let script = vec![
            Script::Fail(LoadFailure::ContextTooLarge),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect("starts");
        assert_eq!(running.backend.kind, BackendKind::Vulkan);
        assert_eq!(running.launch.device.as_deref(), Some("Vulkan0"));
        assert!(!running.launch.save_memory);
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Vulkan, GpuUse::All, false),
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![without_save_memory(BackendKind::Vulkan)]
        );
    }

    /// The order of 0.0.6 for a graphics card that runs out of memory: Save memory off,
    /// then As much as fits, then the processor. Each is tried once.
    #[tokio::test(start_paused = true)]
    async fn graphics_card_memory_failure_order() {
        let script = vec![
            Script::Fail(LoadFailure::ContextTooLarge),
            Script::Fail(LoadFailure::ContextTooLarge),
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect("starts on the processor");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Vulkan, GpuUse::All, false),
                (BackendKind::Vulkan, GpuUse::Fit, false),
                (BackendKind::Cpu, GpuUse::Off, false),
            ]
        );
        assert_eq!(running.launch.device, None);
        assert_eq!(
            drain(&mut rx),
            vec![
                without_save_memory(BackendKind::Vulkan),
                with_less_gpu(BackendKind::Vulkan),
                ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::OutOfMemory,
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn every_retry_happens_once_and_the_chain_ends() {
        let script = vec![Script::Fail(LoadFailure::ContextTooLarge); 6];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let error = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect_err("nothing fits");
        assert_eq!(error.failure(), LoadFailure::ContextTooLarge);
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Vulkan, GpuUse::All, false),
                (BackendKind::Vulkan, GpuUse::Fit, false),
                (BackendKind::Cpu, GpuUse::Off, false),
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                without_save_memory(BackendKind::Vulkan),
                with_less_gpu(BackendKind::Vulkan),
                ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::ContextTooLarge,
                },
            ]
        );
    }

    /// The weights did not fit: Save memory cannot be the cause (it only changes the
    /// conversation memory, made after the weights), and it helps more of the model fit,
    /// so it stays on while less goes on the card.
    #[tokio::test(start_paused = true)]
    async fn weights_that_do_not_fit_keep_save_memory_and_put_less_on_the_card() {
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect("starts");
        assert_eq!(
            (
                running.backend.kind,
                running.launch.gpu,
                running.launch.save_memory
            ),
            (BackendKind::Vulkan, GpuUse::Fit, true)
        );
        assert_eq!(attempts(&log).len(), 2);
        assert_eq!(drain(&mut rx), vec![with_less_gpu(BackendKind::Vulkan)]);
    }

    #[tokio::test(start_paused = true)]
    async fn an_unknown_failure_with_save_memory_retries_without_it_then_steps_down() {
        let script = vec![
            Script::Fail(LoadFailure::Unknown),
            Script::Fail(LoadFailure::Unknown),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_with(
            BackendKind::Vulkan,
            Some("Vulkan0"),
            GpuUse::All,
            true,
        ))
        .await
        .expect("starts");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Vulkan, GpuUse::All, false),
                (BackendKind::Cpu, GpuUse::Off, false),
            ],
            "an unknown failure is not a memory failure: no As much as fits"
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                without_save_memory(BackendKind::Vulkan),
                ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::Unknown,
                },
            ]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn failures_save_memory_cannot_cause_keep_it_on() {
        // A missing device steps down at once, Save memory still on.
        let script = vec![
            Script::Fail(LoadFailure::InvalidDevice("Vulkan0".into())),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_with(
            BackendKind::Vulkan,
            Some("Vulkan0"),
            GpuUse::All,
            true,
        ))
        .await
        .expect("starts");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Cpu, GpuUse::Off, true),
            ]
        );
        assert!(
            drain(&mut rx)
                .iter()
                .all(|e| matches!(e, ServerEvent::SteppedDown { .. }))
        );

        // A broken file fails at once.
        let (mut sup, log, mut rx) =
            supervisor(vec![Script::Fail(LoadFailure::ModelFileInvalid)], gpu_cpu());
        let error = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect_err("broken");
        assert_eq!(error.failure(), LoadFailure::ModelFileInvalid);
        assert_eq!(attempts(&log).len(), 1);
        assert!(drain(&mut rx).is_empty());

        // A taken port only changes the port.
        let script = vec![Script::Fail(LoadFailure::PortInUse), Script::Run(None)];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_with(
            BackendKind::Vulkan,
            Some("Vulkan0"),
            GpuUse::All,
            true,
        ))
        .await
        .expect("starts");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Vulkan, GpuUse::All, true),
            ]
        );
        assert!(drain(&mut rx).is_empty());
    }

    /// On the processor, a failure whose log does not show the model refusing Save memory
    /// is a lack of memory: no retry without it, which would need twice the memory for the
    /// conversation and tell the user that Save memory does not work with their graphics
    /// card. Neither when the load started on the processor, nor after a step-down to it
    /// with Save memory still on, nor when it keeps crashing there.
    #[tokio::test(start_paused = true)]
    async fn a_plain_failure_on_the_processor_keeps_save_memory() {
        for failure in [LoadFailure::ContextTooLarge, LoadFailure::Unknown] {
            let (mut sup, log, mut rx) = supervisor(
                vec![Script::Fail(failure.clone())],
                vec![candidate(BackendKind::Cpu, None)],
            );
            let error = sup
                .start(spec_with(BackendKind::Cpu, None, GpuUse::Off, true))
                .await
                .expect_err("fails");
            assert_eq!(error.failure(), failure);
            assert_eq!(attempts(&log), vec![(BackendKind::Cpu, GpuUse::Off, true)]);
            assert!(drain(&mut rx).is_empty(), "{failure:?}");
        }

        // A missing graphics device steps down with Save memory on; the processor then
        // fails for lack of memory and that is the answer.
        let script = vec![
            Script::Fail(LoadFailure::InvalidDevice("Vulkan0".into())),
            Script::Fail(LoadFailure::ContextTooLarge),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let error = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect_err("fails");
        assert_eq!(error.failure(), LoadFailure::ContextTooLarge);
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::All, true),
                (BackendKind::Cpu, GpuUse::Off, true),
            ]
        );
        assert!(
            drain(&mut rx)
                .iter()
                .all(|e| matches!(e, ServerEvent::SteppedDown { .. })),
            "only the step-down"
        );

        // Crashing on the processor with Save memory on: the restarts, then the failure.
        let crash = Script::Run(Some(Duration::from_secs(1)));
        let (sup, log, mut rx) =
            supervisor(vec![crash; 4], vec![candidate(BackendKind::Cpu, None)]);
        let exit = sup
            .run(
                spec_with(BackendKind::Cpu, None, GpuUse::Off, true),
                CancellationToken::new(),
                |_, _| {},
            )
            .await;
        assert_eq!(exit, SupervisorExit::Failed(LoadFailure::Unknown));
        assert!(attempts(&log).iter().all(|a| a.2), "Save memory stays on");
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|e| matches!(e, ServerEvent::Stage(_))),
            "no retry without Save memory"
        );
    }

    /// What the pinned build logs when the model itself cannot take Save memory's settings,
    /// whatever the backend (llama.cpp's `llama_init_from_model`): a q8_0 block that does
    /// not divide the attention head size, and a model that cannot use flash attention.
    fn refusal(line: &str) -> Vec<String> {
        vec![
            "I llama_context: constructing llama_context".to_owned(),
            format!("E llama_init_from_model: {line}"),
            "E common_init_result: failed to create context with model 'm.gguf'".to_owned(),
            "E srv    load_model: failed to load model, 'm.gguf'".to_owned(),
        ]
    }

    const REFUSALS: [&str; 4] = [
        "K cache type q8_0 with block size 32 does not divide n_embd_head_k=80",
        "V cache type q8_0 with block size 32 does not divide n_embd_head_v=80",
        "quantized V cache requires flash_attn to be enabled",
        "failed to initialize the context: quantized V cache was requested, but this requires Flash Attention",
    ];

    /// The model refuses Save memory (0.0.6 D2c): retried once without it on the same
    /// backend, on the processor too, where nothing else could make it load. A plain
    /// memory failure there still keeps it on (see the test above).
    ///
    /// The event names where the model runs: the processor when no graphics card is in
    /// use (Don't use it, or no device), whatever the backend's label, so the app says the
    /// model does not take Save memory instead of blaming a card the load never used.
    #[tokio::test(start_paused = true)]
    async fn a_model_that_refuses_save_memory_is_retried_without_it_on_any_backend() {
        for line in REFUSALS {
            for (kind, device, gpu, runs_on) in [
                (BackendKind::Cpu, None, GpuUse::Off, BackendKind::Cpu),
                (
                    BackendKind::Vulkan,
                    Some("Vulkan0"),
                    GpuUse::Off,
                    BackendKind::Cpu,
                ),
                (BackendKind::Vulkan, None, GpuUse::All, BackendKind::Cpu),
                (
                    BackendKind::Metal,
                    Some("MTL0"),
                    GpuUse::Off,
                    BackendKind::Cpu,
                ),
                (
                    BackendKind::Vulkan,
                    Some("Vulkan0"),
                    GpuUse::All,
                    BackendKind::Vulkan,
                ),
                (
                    BackendKind::Metal,
                    Some("MTL0"),
                    GpuUse::Fit,
                    BackendKind::Metal,
                ),
            ] {
                let script = vec![
                    Script::FailLog(LoadFailure::ContextTooLarge, refusal(line)),
                    Script::Run(None),
                ];
                let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
                let (_server, running) = sup
                    .start(spec_with(kind, device, gpu, true))
                    .await
                    .expect("starts without Save memory");
                assert_eq!(
                    (
                        running.backend.kind,
                        running.launch.gpu,
                        running.launch.save_memory
                    ),
                    (kind, gpu, false),
                    "{line}"
                );
                assert_eq!(
                    attempts(&log),
                    vec![(kind, gpu, true), (kind, gpu, false)],
                    "{line}"
                );
                assert_eq!(
                    drain(&mut rx),
                    vec![without_save_memory(runs_on)],
                    "{kind} {device:?} {gpu:?}: {line}"
                );
            }
        }

        // Only once: the same lines again without Save memory change nothing more.
        let script = vec![
            Script::FailLog(LoadFailure::ContextTooLarge, refusal(REFUSALS[0])),
            Script::FailLog(LoadFailure::ContextTooLarge, refusal(REFUSALS[0])),
        ];
        let (mut sup, log, mut rx) = supervisor(script, vec![candidate(BackendKind::Cpu, None)]);
        sup.start(spec_with(BackendKind::Cpu, None, GpuUse::Off, true))
            .await
            .expect_err("fails");
        assert_eq!(attempts(&log).len(), 2);
        assert_eq!(drain(&mut rx), vec![without_save_memory(BackendKind::Cpu)]);

        // Not a refusal: the flash attention warning alone, and Save memory already off.
        for (lines, save_memory) in [
            (
                vec![
                    "W llama_init_from_model: flash_attn is not compatible with Grok - forcing off"
                        .to_owned(),
                ],
                true,
            ),
            (refusal(REFUSALS[0]), false),
        ] {
            let script = vec![Script::FailLog(LoadFailure::ContextTooLarge, lines)];
            let (mut sup, log, mut rx) =
                supervisor(script, vec![candidate(BackendKind::Cpu, None)]);
            sup.start(spec_with(BackendKind::Cpu, None, GpuUse::Off, save_memory))
                .await
                .expect_err("fails");
            assert_eq!(attempts(&log).len(), 1);
            assert!(drain(&mut rx).is_empty());
        }
    }

    /// "Don't use it" on a graphics backend runs everything on the processor
    /// (`--device none`), like a graphics backend with no device: the backend's label does
    /// not make a graphics card. So a plain failure is not retried without Save memory
    /// (no note that it does not work with the graphics card), and running out of memory
    /// does not step down to the processor build, which would repeat the same load and
    /// say the graphics card ran out of memory. A failure of the backend itself (a crash,
    /// a missing library) still steps down, since the processor build differs there.
    #[tokio::test(start_paused = true)]
    async fn without_a_graphics_card_in_use_the_label_changes_nothing() {
        for (device, gpu) in [
            (Some("Vulkan0"), GpuUse::Off),
            (None, GpuUse::All),
            (Some(""), GpuUse::Fit),
        ] {
            for failure in [LoadFailure::ContextTooLarge, LoadFailure::OutOfMemory] {
                let (mut sup, log, mut rx) = supervisor(
                    vec![Script::Fail(failure.clone()), Script::Run(None)],
                    gpu_cpu(),
                );
                let error = sup
                    .start(spec_with(BackendKind::Vulkan, device, gpu, true))
                    .await
                    .expect_err("does not fit");
                assert_eq!(error.failure(), failure);
                assert_eq!(
                    attempts(&log),
                    vec![(BackendKind::Vulkan, gpu, true)],
                    "{device:?} {gpu:?} {failure:?}"
                );
                assert!(drain(&mut rx).is_empty(), "{device:?} {gpu:?} {failure:?}");
            }

            for failure in [LoadFailure::Unknown, LoadFailure::MissingRuntimeDll] {
                let (mut sup, log, mut rx) = supervisor(
                    vec![Script::Fail(failure.clone()), Script::Run(None)],
                    gpu_cpu(),
                );
                sup.start(spec_with(BackendKind::Vulkan, device, gpu, true))
                    .await
                    .expect("starts on the processor build");
                assert_eq!(
                    attempts(&log),
                    vec![
                        (BackendKind::Vulkan, gpu, true),
                        (BackendKind::Cpu, GpuUse::Off, true),
                    ],
                    "{device:?} {gpu:?} {failure:?}"
                );
                assert_eq!(
                    drain(&mut rx),
                    vec![ServerEvent::SteppedDown {
                        from: BackendKind::Vulkan,
                        to: BackendKind::Cpu,
                        reason: failure,
                    }]
                );
            }
        }

        // Crashing repeatedly with Don't use it and Save memory: the restarts, then the
        // processor build with Save memory still on, never "without Save memory".
        let crash = Script::Run(Some(Duration::from_secs(1)));
        let script = vec![
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash,
            Script::Run(None),
        ];
        let (sup, log, mut rx) = supervisor(script, gpu_cpu());
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let exit = sup
            .run(
                spec_with(BackendKind::Vulkan, Some("Vulkan0"), GpuUse::Off, true),
                cancel,
                move |_, spec| {
                    if spec.backend.kind == BackendKind::Cpu {
                        stopper.cancel();
                    }
                },
            )
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        let tries = attempts(&log);
        assert_eq!(tries.len(), 5);
        assert!(tries.iter().all(|t| t.2), "Save memory stays on: {tries:?}");
        assert_eq!(tries[4], (BackendKind::Cpu, GpuUse::Off, true));
        assert!(
            !drain(&mut rx)
                .iter()
                .any(|e| matches!(e, ServerEvent::Stage(_))),
            "no retry without Save memory"
        );
    }

    /// Only a server that exited while loading can have failed because of a load setting.
    /// A process that could not be created, a load that ran out of time and an unusable
    /// `/props` reply are reported as unknown failures too, but retrying them without Save
    /// memory would tell the user that Save memory does not work with their graphics card
    /// (and a second timeout would double the wait). They step down as in 0.0.4, with the
    /// settings unchanged.
    #[tokio::test(start_paused = true)]
    async fn errors_that_are_not_a_failed_load_change_no_setting() {
        for (name, first) in [
            ("spawn", Script::SpawnFails),
            ("timeout", Script::TimesOut),
            ("props", Script::PropsFail),
        ] {
            let (mut sup, log, mut rx) = supervisor(vec![first, Script::Run(None)], gpu_cpu());
            sup.start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::All,
                true,
            ))
            .await
            .expect("starts on the processor");
            assert_eq!(
                attempts(&log),
                vec![
                    (BackendKind::Vulkan, GpuUse::All, true),
                    (BackendKind::Cpu, GpuUse::Off, true),
                ],
                "{name}"
            );
            assert_eq!(
                drain(&mut rx),
                vec![ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::Unknown,
                }],
                "{name}"
            );
        }
    }

    #[tokio::test(start_paused = true)]
    async fn less_on_the_card_is_tried_only_from_all_of_it_on_a_graphics_card() {
        // Already As much as fits: straight to the processor.
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_with(
            BackendKind::Vulkan,
            Some("Vulkan0"),
            GpuUse::Fit,
            false,
        ))
        .await
        .expect("starts");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Vulkan, GpuUse::Fit, false),
                (BackendKind::Cpu, GpuUse::Off, false),
            ]
        );
        assert_eq!(drain(&mut rx).len(), 1, "only the step-down");

        // Don't use it: the processor already does all the work, so the processor build
        // would repeat the same load. Running out of memory is the answer.
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let error = sup
            .start(spec_with(
                BackendKind::Vulkan,
                Some("Vulkan0"),
                GpuUse::Off,
                false,
            ))
            .await
            .expect_err("does not fit");
        assert_eq!(error.failure(), LoadFailure::OutOfMemory);
        assert_eq!(
            attempts(&log),
            vec![(BackendKind::Vulkan, GpuUse::Off, false)]
        );
        assert!(
            drain(&mut rx).is_empty(),
            "no note blames the graphics card"
        );

        // The processor: nothing to put on a card, nothing below it.
        let (mut sup, log, mut rx) = supervisor(
            vec![Script::Fail(LoadFailure::OutOfMemory)],
            vec![candidate(BackendKind::Cpu, None)],
        );
        let error = sup
            .start(spec(BackendKind::Cpu, None))
            .await
            .expect_err("does not fit");
        assert_eq!(error.failure(), LoadFailure::OutOfMemory);
        assert_eq!(attempts(&log), vec![(BackendKind::Cpu, GpuUse::Off, false)]);
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn out_of_memory_on_rocm_puts_less_on_the_card_then_tries_vulkan_then_the_processor() {
        let script = vec![
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Run(None),
        ];
        let candidates = vec![
            candidate(BackendKind::Rocm, Some("ROCm0")),
            candidate(BackendKind::Vulkan, Some("Vulkan1")),
            candidate(BackendKind::Cpu, None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, candidates);
        sup.start(spec(BackendKind::Rocm, Some("ROCm0")))
            .await
            .expect("starts");
        assert_eq!(
            attempts(&log),
            vec![
                (BackendKind::Rocm, GpuUse::All, false),
                (BackendKind::Rocm, GpuUse::Fit, false),
                (BackendKind::Vulkan, GpuUse::Fit, false),
                (BackendKind::Cpu, GpuUse::Off, false),
            ]
        );
        let devices: Vec<Option<String>> = log
            .lock()
            .expect("log")
            .started
            .iter()
            .map(|s| s.launch.device.clone())
            .collect();
        assert_eq!(
            devices,
            vec![
                Some("ROCm0".to_owned()),
                Some("ROCm0".to_owned()),
                Some("Vulkan1".to_owned()),
                None
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                with_less_gpu(BackendKind::Rocm),
                ServerEvent::SteppedDown {
                    from: BackendKind::Rocm,
                    to: BackendKind::Vulkan,
                    reason: LoadFailure::OutOfMemory,
                },
                ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::OutOfMemory,
                },
            ]
        );
    }

    /// A server that keeps crashing with Save memory on gets it turned off before any
    /// step-down (0.0.6 D2c), on the same backend.
    #[tokio::test(start_paused = true)]
    async fn crash_limit_with_save_memory_turns_it_off_before_stepping_down() {
        let crash = Script::Run(Some(Duration::from_secs(1)));
        let script = vec![
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash,
            Script::Run(None),
        ];
        let (sup, log, mut rx) = supervisor(script, gpu_cpu());
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let exit = sup
            .run(
                spec_with(BackendKind::Vulkan, Some("Vulkan0"), GpuUse::All, true),
                cancel,
                move |_, spec| {
                    // Stop at the first server without Save memory, or on the processor
                    // (where a wrong order would end up).
                    if !spec.launch.save_memory || spec.backend.kind == BackendKind::Cpu {
                        stopper.cancel();
                    }
                },
            )
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        let tries = attempts(&log);
        assert_eq!(tries.len(), 5);
        assert_eq!(tries[4], (BackendKind::Vulkan, GpuUse::All, false));
        let events = drain(&mut rx);
        assert!(events.contains(&without_save_memory(BackendKind::Vulkan)));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ServerEvent::SteppedDown { .. }))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn crash_limit_out_of_memory_puts_less_on_the_card() {
        let crash = Script::Crash(Duration::from_secs(1), LoadFailure::OutOfMemory);
        let script = vec![
            crash.clone(),
            crash.clone(),
            crash.clone(),
            crash,
            Script::Run(None),
        ];
        let (sup, log, mut rx) = supervisor(script, gpu_cpu());
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let exit = sup
            .run(
                spec(BackendKind::Vulkan, Some("Vulkan0")),
                cancel,
                move |_, spec| {
                    if spec.launch.gpu == GpuUse::Fit || spec.backend.kind == BackendKind::Cpu {
                        stopper.cancel();
                    }
                },
            )
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        assert_eq!(attempts(&log)[4], (BackendKind::Vulkan, GpuUse::Fit, false));
        assert!(drain(&mut rx).contains(&with_less_gpu(BackendKind::Vulkan)));
    }

    #[tokio::test(start_paused = true)]
    async fn step_down_continues_until_the_chain_ends() {
        let script = vec![
            Script::Fail(LoadFailure::InvalidDevice("ROCm0".into())),
            Script::Fail(LoadFailure::Unknown),
            Script::Fail(LoadFailure::Unknown),
        ];
        let candidates = vec![
            candidate(BackendKind::Vulkan, Some("Vulkan0")),
            candidate(BackendKind::Cpu, None),
        ];
        let (mut sup, log, _rx) = supervisor(script, candidates);
        let error = sup
            .start(spec(BackendKind::Rocm, Some("ROCm0")))
            .await
            .expect_err("all fail");
        assert_eq!(error.failure(), LoadFailure::Unknown);
        let kinds: Vec<_> = log
            .lock()
            .expect("log")
            .started
            .iter()
            .map(|s| s.backend.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![BackendKind::Rocm, BackendKind::Vulkan, BackendKind::Cpu]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn invalid_settings_do_not_step_down() {
        let candidates = vec![
            candidate(BackendKind::Vulkan, Some("Vulkan0")),
            candidate(BackendKind::Cpu, None),
        ];
        let (mut sup, log, mut rx) = supervisor(vec![Script::Invalid], candidates);
        let error = sup
            .start(spec(BackendKind::Vulkan, Some("Vulkan0")))
            .await
            .expect_err("invalid");
        assert!(matches!(error, StartError::InvalidConfig(_)), "{error:?}");
        assert_eq!(log.lock().expect("log").started.len(), 1);
        assert!(drain(&mut rx).is_empty(), "no step-down was announced");
    }

    #[tokio::test(start_paused = true)]
    async fn a_broken_model_file_does_not_step_down() {
        let script = vec![Script::Fail(LoadFailure::ModelFileInvalid)];
        let candidates = vec![candidate(BackendKind::Cpu, None)];
        let (sup, log, mut rx) = supervisor(script, candidates);
        let exit = sup
            .run(
                spec(BackendKind::Vulkan, Some("Vulkan0")),
                CancellationToken::new(),
                |_, _| {},
            )
            .await;
        assert_eq!(exit, SupervisorExit::Failed(LoadFailure::ModelFileInvalid));
        assert_eq!(log.lock().expect("log").started.len(), 1);
        assert_eq!(
            drain(&mut rx),
            vec![ServerEvent::State(ServerState::Failed(
                LoadFailure::ModelFileInvalid
            ))]
        );
    }

    #[tokio::test(start_paused = true)]
    async fn port_in_use_retries_on_new_ports_then_gives_up() {
        let script = vec![
            Script::Fail(LoadFailure::PortInUse),
            Script::Fail(LoadFailure::PortInUse),
            Script::Fail(LoadFailure::PortInUse),
            Script::Fail(LoadFailure::PortInUse),
        ];
        let (mut sup, log, _rx) = supervisor(script, vec![candidate(BackendKind::Cpu, None)]);
        let error = sup
            .start(spec(BackendKind::Vulkan, Some("Vulkan0")))
            .await
            .expect_err("gives up");
        assert_eq!(error.failure(), LoadFailure::PortInUse);
        let started = log.lock().expect("log").started.clone();
        assert_eq!(started.len(), 4, "the first try plus three new ports");
        assert!(
            started[1..]
                .iter()
                .all(|s| s.launch.port != 41000 && s.launch.port != 0)
        );
        assert!(
            started
                .iter()
                .all(|s| s.backend.kind == BackendKind::Vulkan),
            "a port problem is not a backend problem"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_stops_the_running_server() {
        let (sup, log, mut rx) = supervisor(vec![Script::Run(None)], vec![]);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let exit = sup
            .run(spec(BackendKind::Cpu, None), cancel, move |_, _| {
                stopper.cancel()
            })
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        assert_eq!(log.lock().expect("log").stopped, 1);
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_during_a_start_reports_stopped() {
        // First start: a crash after a second. The restart hangs while loading.
        let script = vec![Script::Run(Some(Duration::from_secs(1))), Script::Hang];
        let (sup, log, mut rx) = supervisor(script, vec![]);
        let cancel = CancellationToken::new();
        let stopper = cancel.clone();
        let task = tokio::spawn(sup.run(spec(BackendKind::Cpu, None), cancel, |_, _| {}));
        // Let the first server crash and the restart begin, then cancel it.
        tokio::time::sleep(Duration::from_secs(5)).await;
        stopper.cancel();
        let exit = task.await.expect("join");
        assert_eq!(exit, SupervisorExit::Stopped);
        assert_eq!(log.lock().expect("log").started.len(), 2);
        let events = drain(&mut rx);
        assert!(events.contains(&ServerEvent::Restarting { attempt: 1, max: 3 }));
        assert_eq!(
            events.last(),
            Some(&ServerEvent::State(ServerState::Stopped))
        );
    }

    #[tokio::test(start_paused = true)]
    async fn cancel_before_start_starts_nothing() {
        let (sup, log, _rx) = supervisor(vec![], vec![]);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let exit = sup
            .run(spec(BackendKind::Cpu, None), cancel, |_, _| {})
            .await;
        assert_eq!(exit, SupervisorExit::Stopped);
        assert!(log.lock().expect("log").started.is_empty());
    }

    /// What a real server reports goes through [`relay_until`] ([`LocalStarter`] and
    /// [`LocalServer`]): passed on while the attempt runs (load progress is live), all of it
    /// before the call returns (so the supervisor's own events come after), in the order it
    /// was sent, except the attempt's own `Failed`. The supervisor may still retry; it
    /// reports `Failed` itself when it gives up, so the app never shows a load as failed
    /// while it is retried.
    #[tokio::test]
    async fn a_server_is_relayed_in_order_without_its_own_failure() {
        let progress = ServerEvent::Progress {
            stage: Some("text_model".into()),
            value: Some(0.5),
        };
        let starting = ServerEvent::State(ServerState::Starting);
        let (to_relay, mut from_server) = unbounded_channel();
        let (events, mut app) = unbounded_channel();

        // A failed attempt: it reports its start and progress, then waits until the app
        // has seen them, then logs, exits and fails.
        let (seen, wait_seen) = tokio::sync::oneshot::channel::<()>();
        let attempt = {
            let to_relay = to_relay.clone();
            let (starting, progress) = (starting.clone(), progress.clone());
            async move {
                let _ = to_relay.send(starting);
                let _ = to_relay.send(progress);
                let _ = wait_seen.await;
                for event in [
                    ServerEvent::Log("E failed to create context".into()),
                    ServerEvent::Exited { code: Some(1) },
                    ServerEvent::State(ServerState::Failed(LoadFailure::ContextTooLarge)),
                ] {
                    let _ = to_relay.send(event);
                }
                "failed"
            }
        };
        let app_side = async {
            let early = vec![app.recv().await, app.recv().await];
            let _ = seen.send(());
            early
        };
        let (result, early) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(relay_until(attempt, &mut from_server, &events), app_side)
        })
        .await
        .expect("events are passed on while the attempt runs");
        assert_eq!(result, "failed");
        assert_eq!(early, vec![Some(starting.clone()), Some(progress)]);
        assert_eq!(
            drain(&mut app),
            vec![
                ServerEvent::Log("E failed to create context".into()),
                ServerEvent::Exited { code: Some(1) },
            ],
            "everything up to the exit, before the supervisor decides; no Failed"
        );

        // A ready server, then its wait for the exit and its stop: the other states pass.
        let ready = async {
            let _ = to_relay.send(starting.clone());
            let _ = to_relay.send(ServerEvent::State(ServerState::Ready));
        };
        relay_until(ready, &mut from_server, &events).await;
        let exit = async {
            for event in [
                ServerEvent::Log("last line".into()),
                ServerEvent::Exited { code: Some(134) },
                ServerEvent::State(ServerState::Stopping),
                ServerEvent::State(ServerState::Stopped),
            ] {
                let _ = to_relay.send(event);
            }
            Some(134)
        };
        assert_eq!(
            relay_until(exit, &mut from_server, &events).await,
            Some(134)
        );
        assert_eq!(
            drain(&mut app),
            vec![
                starting,
                ServerEvent::State(ServerState::Ready),
                ServerEvent::Log("last line".into()),
                ServerEvent::Exited { code: Some(134) },
                ServerEvent::State(ServerState::Stopping),
                ServerEvent::State(ServerState::Stopped),
            ]
        );
    }

    #[test]
    fn default_policy_matches_the_design() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 3);
        assert_eq!(policy.window, Duration::from_secs(300));
    }

    /// [`spec_with`] with these Fine control values (item 6).
    fn spec_fine(gpu: GpuUse, save_memory: bool, fine: FineLaunch) -> StartSpec {
        let mut spec = spec_with(BackendKind::Vulkan, Some("Vulkan0"), gpu, save_memory);
        spec.launch.fine = fine;
        spec
    }

    /// Fast attention, layers and expert parts of every start, in order.
    fn fine_attempts(log: &Arc<Mutex<Log>>) -> Vec<(BackendKind, GpuUse, FineLaunch)> {
        log.lock()
            .expect("log")
            .started
            .iter()
            .map(|s| (s.backend.kind, s.launch.gpu, s.launch.fine.clone()))
            .collect()
    }

    fn without_fast_attention(backend: BackendKind) -> ServerEvent {
        ServerEvent::Stage(LoadStage::RetryingWithoutFastAttention { backend })
    }

    /// Item 6, section 3.3 step 2: fast attention chosen On fails the way Save memory can
    /// fail on a graphics card (a failed context, or an unknown crash): one retry on the same
    /// backend with it Off, the rest of Fine control kept. Without a card in use, with Save
    /// memory on (its own `-fa on` is what the card refused, step 1), with Automatic or Off,
    /// and for weights that did not fit, there is no such retry.
    #[tokio::test(start_paused = true)]
    async fn fast_attention_failure_retries_once_with_off() {
        let on = FineLaunch {
            flash_attn: FlashAttn::On,
            threads: Some(4),
            kv_in_computer_memory: true,
            ..FineLaunch::default()
        };
        for reason in [LoadFailure::ContextTooLarge, LoadFailure::Unknown] {
            let script = vec![Script::Fail(reason.clone()), Script::Run(None)];
            let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
            let (_server, running) = sup
                .start(spec_fine(GpuUse::All, false, on.clone()))
                .await
                .expect("starts");
            let off = FineLaunch {
                flash_attn: FlashAttn::Off,
                ..on.clone()
            };
            assert_eq!(running.backend.kind, BackendKind::Vulkan, "{reason:?}");
            assert_eq!(running.launch.fine, off, "{reason:?}");
            assert_eq!(
                fine_attempts(&log),
                vec![
                    (BackendKind::Vulkan, GpuUse::All, on.clone()),
                    (BackendKind::Vulkan, GpuUse::All, off),
                ],
                "{reason:?}"
            );
            assert_eq!(
                drain(&mut rx),
                vec![without_fast_attention(BackendKind::Vulkan)],
                "{reason:?}"
            );
            let args = crate::llama::launch::build_args(&running.launch);
            assert!(args.windows(2).any(|w| w == ["-fa", "off"]), "{args:?}");
        }

        // Once only: a second such failure goes on down the chain (As much as fits for a
        // context that did not fit, then the processor).
        let script = vec![Script::Fail(LoadFailure::ContextTooLarge); 3]
            .into_iter()
            .chain([Script::Run(None)])
            .collect();
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_fine(GpuUse::All, false, on.clone()))
            .await
            .expect("starts on the processor");
        let flash: Vec<FlashAttn> = fine_attempts(&log)
            .iter()
            .map(|(_, _, f)| f.flash_attn)
            .collect();
        assert_eq!(
            flash,
            [
                FlashAttn::On,
                FlashAttn::Off,
                FlashAttn::Off,
                FlashAttn::Off
            ]
        );
        assert_eq!(
            drain(&mut rx),
            vec![
                without_fast_attention(BackendKind::Vulkan),
                with_less_gpu(BackendKind::Vulkan),
                ServerEvent::SteppedDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::ContextTooLarge,
                },
            ]
        );

        // No fast-attention retry for these.
        let cases: Vec<(&str, StartSpec, LoadFailure)> = vec![
            (
                "weights that did not fit",
                spec_fine(GpuUse::Fit, false, on.clone()),
                LoadFailure::OutOfMemory,
            ),
            (
                "Automatic",
                spec_fine(GpuUse::Fit, false, FineLaunch::default()),
                LoadFailure::ContextTooLarge,
            ),
            (
                "Off",
                spec_fine(
                    GpuUse::Fit,
                    false,
                    FineLaunch {
                        flash_attn: FlashAttn::Off,
                        ..FineLaunch::default()
                    },
                ),
                LoadFailure::ContextTooLarge,
            ),
            (
                "Don't use it on a graphics backend",
                spec_fine(GpuUse::Off, false, on.clone()),
                LoadFailure::ContextTooLarge,
            ),
        ];
        for (name, start, reason) in cases {
            let script = vec![Script::Fail(reason.clone()), Script::Run(None)];
            let (mut sup, _log, mut rx) = supervisor(script, gpu_cpu());
            let _ = sup.start(start).await;
            assert!(
                !drain(&mut rx).contains(&without_fast_attention(BackendKind::Vulkan)),
                "{name}"
            );
        }
        // The processor: fast attention always works there; a plain failure is no reason.
        let (mut sup, log, mut rx) = supervisor(
            vec![Script::Fail(LoadFailure::ContextTooLarge)],
            vec![candidate(BackendKind::Cpu, None)],
        );
        let mut cpu = spec(BackendKind::Cpu, None);
        cpu.launch.fine = on.clone();
        sup.start(cpu).await.expect_err("does not fit");
        assert_eq!(fine_attempts(&log).len(), 1);
        assert!(drain(&mut rx).is_empty());

        // Save memory on: step 1 turns Save memory off; its `-fa on` was the one in use.
        // A second failure then retries with the chosen fast attention Off.
        let script = vec![
            Script::Fail(LoadFailure::ContextTooLarge),
            Script::Fail(LoadFailure::ContextTooLarge),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_fine(GpuUse::Fit, true, on.clone()))
            .await
            .expect("starts");
        assert!(!running.launch.save_memory);
        assert_eq!(running.launch.fine.flash_attn, FlashAttn::Off);
        assert_eq!(fine_attempts(&log).len(), 3);
        assert_eq!(
            drain(&mut rx),
            vec![
                without_save_memory(BackendKind::Vulkan),
                without_fast_attention(BackendKind::Vulkan),
            ]
        );
    }

    /// Item 6, section 3.3 step 3: a placement chosen in Fine control that runs out of
    /// graphics memory is retried once with As much as fits, whatever the plain choice
    /// said; the chosen layers and expert parts are set aside for this load, the stage says
    /// so with the chosen number of layers, and the conversation memory's place and the
    /// margin As much as fits leaves free are kept. Then the existing order.
    #[tokio::test(start_paused = true)]
    async fn custom_placement_out_of_memory_retries_with_as_much_as_fits() {
        let chosen = FineLaunch {
            gpu_layers: Some(40),
            experts_on_cpu: ExpertsOnCpu::FirstLayers(5),
            kv_in_computer_memory: true,
            fit_margin_mib: Some(2048),
            threads: Some(6),
            flash_attn: FlashAttn::Off,
        };
        let kept = FineLaunch {
            gpu_layers: None,
            experts_on_cpu: ExpertsOnCpu::None,
            ..chosen.clone()
        };
        for (gpu, reason) in [
            (GpuUse::All, LoadFailure::OutOfMemory),
            (GpuUse::Fit, LoadFailure::OutOfMemory),
            (GpuUse::Fit, LoadFailure::ContextTooLarge),
        ] {
            let script = vec![Script::Fail(reason.clone()), Script::Run(None)];
            let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
            let (_server, running) = sup
                .start(spec_fine(gpu, false, chosen.clone()))
                .await
                .expect("starts");
            assert_eq!(
                fine_attempts(&log),
                vec![
                    (BackendKind::Vulkan, gpu, chosen.clone()),
                    (BackendKind::Vulkan, GpuUse::Fit, kept.clone()),
                ],
                "{gpu:?} {reason:?}"
            );
            assert_eq!(
                drain(&mut rx),
                vec![ServerEvent::Stage(LoadStage::RetryingWithLessGpu {
                    backend: BackendKind::Vulkan,
                    custom: true,
                    chosen_layers: Some(40),
                })],
                "{gpu:?} {reason:?}"
            );
            let args = crate::llama::launch::build_args(&running.launch).join(" ");
            assert!(
                args.contains("--device Vulkan0 --fit on -fitt 2048 -nkvo -t 6 -fa off"),
                "{args}"
            );
        }

        // Expert parts only: the stage names no layer count.
        let experts = FineLaunch {
            experts_on_cpu: ExpertsOnCpu::All,
            ..FineLaunch::default()
        };
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, _log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_fine(GpuUse::All, false, experts))
            .await
            .expect("starts");
        assert!(!running.launch.fine.custom_placement());
        assert_eq!(running.launch.gpu, GpuUse::Fit);
        assert_eq!(
            drain(&mut rx),
            vec![ServerEvent::Stage(LoadStage::RetryingWithLessGpu {
                backend: BackendKind::Vulkan,
                custom: true,
                chosen_layers: None,
            })]
        );

        // Once: As much as fits that still runs out of memory steps down to the processor,
        // where the placement adds nothing.
        let script = vec![
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Fail(LoadFailure::OutOfMemory),
            Script::Run(None),
        ];
        let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
        let (_server, running) = sup
            .start(spec_fine(GpuUse::All, false, chosen.clone()))
            .await
            .expect("starts on the processor");
        assert_eq!(fine_attempts(&log).len(), 3);
        assert_eq!(running.backend.kind, BackendKind::Cpu);
        assert_eq!(drain(&mut rx).len(), 2, "the retry, then the step-down");

        // A failure that is not about memory keeps the chosen placement.
        let script = vec![Script::Fail(LoadFailure::Unknown), Script::Run(None)];
        let (mut sup, log, _rx) = supervisor(script, gpu_cpu());
        sup.start(spec_fine(GpuUse::All, false, chosen.clone()))
            .await
            .expect("starts");
        let attempts = fine_attempts(&log);
        assert_eq!(attempts.len(), 2);
        assert!(
            attempts.iter().all(|(_, _, f)| *f == chosen),
            "an unknown failure steps down; it does not drop the placement: {attempts:?}"
        );

        // Without a custom placement, All of it keeps the old stage (no `custom`).
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, _log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_fine(GpuUse::All, false, FineLaunch::default()))
            .await
            .expect("starts");
        assert_eq!(drain(&mut rx), vec![with_less_gpu(BackendKind::Vulkan)]);

        // 0 layers chosen puts none of the model on the card (`-ngl 0`; expert flags have
        // nothing there to move). As much as fits would put more on the card than chosen
        // and its note would blame the chosen layers, so the load steps down to the
        // processor, which is where the user put the layers.
        for gpu in [GpuUse::All, GpuUse::Fit] {
            for experts in [
                ExpertsOnCpu::None,
                ExpertsOnCpu::All,
                ExpertsOnCpu::FirstLayers(5),
            ] {
                let zero = FineLaunch {
                    gpu_layers: Some(0),
                    experts_on_cpu: experts,
                    ..FineLaunch::default()
                };
                for reason in [LoadFailure::OutOfMemory, LoadFailure::ContextTooLarge] {
                    let script = vec![Script::Fail(reason.clone()), Script::Run(None)];
                    let (mut sup, log, mut rx) = supervisor(script, gpu_cpu());
                    let (_server, running) = sup
                        .start(spec_fine(gpu, false, zero.clone()))
                        .await
                        .expect("starts on the processor");
                    let attempts = fine_attempts(&log);
                    assert_eq!(
                        attempts
                            .iter()
                            .map(|(backend, gpu, _)| (*backend, *gpu))
                            .collect::<Vec<_>>(),
                        vec![(BackendKind::Vulkan, gpu), (BackendKind::Cpu, GpuUse::Off)],
                        "{gpu:?} {experts:?} {reason:?}"
                    );
                    assert_eq!(running.backend.kind, BackendKind::Cpu);
                    assert_eq!(
                        drain(&mut rx),
                        vec![ServerEvent::SteppedDown {
                            from: BackendKind::Vulkan,
                            to: BackendKind::Cpu,
                            reason,
                        }],
                        "{gpu:?} {experts:?}: no As much as fits retry"
                    );
                }
            }
        }

        // A chosen count above the command line's limit launched as `-ngl all`
        // (`launch::MAX_LAYER_COUNT`), so the stage names no count.
        let script = vec![Script::Fail(LoadFailure::OutOfMemory), Script::Run(None)];
        let (mut sup, _log, mut rx) = supervisor(script, gpu_cpu());
        sup.start(spec_fine(
            GpuUse::All,
            false,
            FineLaunch {
                gpu_layers: Some(u32::MAX),
                ..FineLaunch::default()
            },
        ))
        .await
        .expect("starts");
        assert_eq!(
            drain(&mut rx),
            vec![ServerEvent::Stage(LoadStage::RetryingWithLessGpu {
                backend: BackendKind::Vulkan,
                custom: true,
                chosen_layers: None,
            })]
        );
    }

    /// Every retry happens once, with every setting on (Save memory, fast attention On, a
    /// custom placement, the conversation memory in computer memory): Save memory off, fast
    /// attention off, As much as fits, the processor, then the failure. The chain ends.
    #[tokio::test(start_paused = true)]
    async fn the_chain_still_ends() {
        let everything = FineLaunch {
            gpu_layers: Some(30),
            experts_on_cpu: ExpertsOnCpu::All,
            kv_in_computer_memory: true,
            threads: Some(2),
            flash_attn: FlashAttn::On,
            fit_margin_mib: Some(512),
        };
        for failure in [
            LoadFailure::ContextTooLarge,
            LoadFailure::Unknown,
            LoadFailure::OutOfMemory,
        ] {
            let script = vec![Script::Fail(failure.clone()); 12];
            let candidates = vec![
                candidate(BackendKind::Rocm, Some("ROCm0")),
                candidate(BackendKind::Vulkan, Some("Vulkan0")),
                candidate(BackendKind::Cpu, None),
            ];
            let (mut sup, log, mut rx) = supervisor(script, candidates);
            let mut start = spec_with(BackendKind::Rocm, Some("ROCm0"), GpuUse::All, true);
            start.launch.fine = everything.clone();
            let error = sup.start(start).await.expect_err("nothing loads");
            assert_eq!(error.failure(), failure);
            let started = log.lock().expect("log").started.len();
            assert!(started <= 6, "{failure:?}: {started} starts");
            let events = drain(&mut rx);
            for stage in [
                without_save_memory(BackendKind::Rocm),
                without_fast_attention(BackendKind::Rocm),
            ] {
                assert!(
                    events.iter().filter(|e| **e == stage).count() <= 1,
                    "{failure:?}: {events:?}"
                );
            }
            assert!(
                events
                    .iter()
                    .filter(|e| matches!(
                        e,
                        ServerEvent::Stage(LoadStage::RetryingWithLessGpu { .. })
                    ))
                    .count()
                    <= 2,
                "at most once per graphics backend: {events:?}"
            );
            let kinds: Vec<BackendKind> = log
                .lock()
                .expect("log")
                .started
                .iter()
                .map(|s| s.backend.kind)
                .collect();
            assert_eq!(
                kinds.last(),
                Some(&BackendKind::Cpu),
                "{failure:?}: {kinds:?}"
            );
            match failure {
                LoadFailure::ContextTooLarge => assert_eq!(
                    events,
                    vec![
                        without_save_memory(BackendKind::Rocm),
                        without_fast_attention(BackendKind::Rocm),
                        ServerEvent::Stage(LoadStage::RetryingWithLessGpu {
                            backend: BackendKind::Rocm,
                            custom: true,
                            chosen_layers: Some(30),
                        }),
                        ServerEvent::SteppedDown {
                            from: BackendKind::Rocm,
                            to: BackendKind::Vulkan,
                            reason: LoadFailure::ContextTooLarge,
                        },
                        ServerEvent::SteppedDown {
                            from: BackendKind::Vulkan,
                            to: BackendKind::Cpu,
                            reason: LoadFailure::ContextTooLarge,
                        },
                    ]
                ),
                LoadFailure::OutOfMemory => assert_eq!(
                    events,
                    vec![
                        ServerEvent::Stage(LoadStage::RetryingWithLessGpu {
                            backend: BackendKind::Rocm,
                            custom: true,
                            chosen_layers: Some(30),
                        }),
                        ServerEvent::SteppedDown {
                            from: BackendKind::Rocm,
                            to: BackendKind::Vulkan,
                            reason: LoadFailure::OutOfMemory,
                        },
                        ServerEvent::SteppedDown {
                            from: BackendKind::Vulkan,
                            to: BackendKind::Cpu,
                            reason: LoadFailure::OutOfMemory,
                        },
                    ]
                ),
                _ => {}
            }
        }
    }
}
