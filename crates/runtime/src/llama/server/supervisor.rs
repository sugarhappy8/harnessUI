//! Restart and step-down policy (ARCHITECTURE.md 7.4).
//!
//! - **Crash while Ready:** restart with the same arguments, up to
//!   [`RestartPolicy::max_restarts`] times within [`RestartPolicy::window`].
//! - **Stepping down:** when a GPU backend fails to load or reaches the crash limit, and a
//!   lower backend passed its probe, try the next one down (ROCm or CUDA → Vulkan → CPU)
//!   and report [`ServerEvent::SteppedDown`]. The chain is finite, so this ends.
//! - **Port taken:** pick a new port, up to [`RestartPolicy::port_retries`] times.
//!
//! The policy talks to servers only through the [`Starter`] and [`Supervised`] traits, so
//! tests drive it with scripted fakes and no real process.

use std::collections::VecDeque;
use std::future::Future;
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{RunningServer, ServerEvent, ServerState, StartError, StartSpec, start};
use crate::llama::backend::{Backend, BackendKind};
use crate::llama::failure::{LoadFailure, classify};
use crate::llama::launch::free_loopback_port;

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
    type Server = RunningServer;

    fn start(
        &self,
        spec: StartSpec,
    ) -> impl Future<Output = Result<RunningServer, StartError>> + Send {
        start(spec, self.events.clone())
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

    /// Starts `spec`, trying a new port when the port is taken and the next backend down
    /// when the failure could be backend-specific. Returns the server and the spec it
    /// actually runs with.
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

    /// The spec for the next backend down, and the event that tells the user.
    fn step_down_spec(&self, spec: &StartSpec, reason: &LoadFailure) -> Option<StartSpec> {
        let backends: Vec<Backend> = self.candidates.iter().map(|c| c.backend.clone()).collect();
        let lower = step_down(spec.backend.kind, &backends)?;
        let candidate = self.candidates.iter().find(|c| c.backend == lower)?;
        let mut next = spec.clone();
        next.backend = lower;
        next.launch.device = if candidate.backend.kind == BackendKind::Cpu {
            None
        } else {
            candidate.device.clone()
        };
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
    use crate::llama::launch::{GpuLayers, LaunchConfig};

    /// What a scripted start does.
    #[derive(Clone, Debug)]
    enum Script {
        /// Fails to load with this reason.
        Fail(LoadFailure),
        /// Becomes ready, then exits after this long (`None`: runs until stopped).
        Run(Option<Duration>),
        /// Never finishes starting (a model that loads forever).
        Hang,
        /// Refuses the settings before starting anything.
        Invalid,
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
                Script::Run(exit_after) => Ok(FakeServer {
                    exit_after,
                    log: self.log.clone(),
                }),
                Script::Hang => std::future::pending().await,
                Script::Invalid => Err(StartError::InvalidConfig("bad settings".into())),
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
                gpu_layers: GpuLayers::Auto,
                cache_ram_mib: 256,
                reasoning_budget: None,
                port: 41000,
                api_key: "k".into(),
                hip_visible_devices: (kind == BackendKind::Rocm).then(|| "1".to_owned()),
            },
            load_timeout: Duration::from_secs(60),
        }
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
        let (_server, running) = sup
            .start(spec(BackendKind::Rocm, Some("ROCm0")))
            .await
            .expect("starts");
        assert_eq!(running.backend.kind, BackendKind::Vulkan);
        assert_eq!(running.launch.device.as_deref(), Some("Vulkan1"));
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

    #[test]
    fn default_policy_matches_the_design() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 3);
        assert_eq!(policy.window, Duration::from_secs(300));
    }
}
