//! The usage line under the model name (0.0.6 item 6, `docs/dev/0.0.6/item6-spec.md`
//! sections 2.4 and 5): how busy this computer's graphics card and processor are and how
//! much memory is in use, read on this computer only, and only while the webview asks
//! (every 1.5 seconds while the window is visible). Nothing is sent anywhere and the model
//! is never told these figures (I6-17).
//!
//! One reader thread owns the system's [`usage::Sampler`] (its counter handles live on that
//! thread). It is started by the first request and ends [`IDLE_END`] after the last one,
//! dropping the sampler and so closing every handle. A request is answered within
//! [`ANSWER_WITHIN`]; a sample that runs longer marks the reader stuck: that request and
//! every later one answer "Couldn't read this just now" for every figure at once, without
//! queuing behind it, and at most one replacement thread starts per app run (a call that
//! hangs for good never piles up threads). A stuck reader whose call returns is used again.
//!
//! Computer memory is added here from `models::hardware::system_memory()` (used = total −
//! available), so `crates/usage` does not depend on `models`. The focus device (which card
//! Windows reads) is the loaded model's device, else the one a load would use, by the
//! backend probe's description and total; the probe is never started from here.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use runtime::{Backend, ProbeResult};
use serde::Serialize;
use tauri::State;
use usage::{Focus, Gap, Reading, Sample, Sampler};

use crate::dto::{LocalModelSettings, RuntimeStateDto, RuntimeStatusDto};
use crate::error::AppResult;
use crate::state::AppState;

/// The reader thread ends this long after the last request.
pub const IDLE_END: Duration = Duration::from_secs(30);

/// A request is answered within this; a sample that takes longer marks the reader stuck.
pub const ANSWER_WITHIN: Duration = Duration::from_secs(2);

const MIB: u64 = 1024 * 1024;

/// What `usage_now` answers (contract 2.4). JSON: the sample's fields (`graphics`,
/// `processorPercent`, `threads`, `unified`) and `memory`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageNow {
    #[serde(flatten)]
    pub sample: Sample,
    pub memory: Reading<MemUse>,
}

/// Computer memory in use by all programs, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemUse {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// Makes the system's sampler, on the reader thread (a fake in tests).
pub type SamplerFactory = Arc<dyn Fn() -> Box<dyn Sampler + Send> + Send + Sync>;

struct Request {
    focus: Focus,
    reply: tokio::sync::oneshot::Sender<Sample>,
}

/// One reader thread, as the requests see it.
struct Worker {
    id: u64,
    requests: mpsc::Sender<Request>,
    /// When the sample being taken started; `None` between samples.
    sampling_since: Arc<Mutex<Option<Instant>>>,
    /// Samples finished so far.
    finished: Arc<AtomicU64>,
    /// `finished` when a request to this thread ran out of time: stuck until another
    /// sample finishes.
    timed_out_at: Option<u64>,
}

impl Worker {
    /// A request ran out of time and no sample finished since, or a sample has run longer
    /// than `limit`.
    fn stuck(&self, limit: Duration) -> bool {
        if self
            .timed_out_at
            .is_some_and(|at| self.finished.load(Ordering::SeqCst) == at)
        {
            return true;
        }
        self.sampling_since
            .lock()
            .ok()
            .and_then(|since| *since)
            .is_some_and(|since| since.elapsed() > limit)
    }
}

#[derive(Default)]
struct ReaderState {
    worker: Option<Worker>,
    /// The one replacement of a stuck reader was started this run.
    replaced: bool,
    /// Threads started this run.
    started: u64,
}

/// The usage line's reader (`AppState::usage`).
pub struct UsageReader {
    factory: SamplerFactory,
    idle_end: Duration,
    answer_within: Duration,
    state: Mutex<ReaderState>,
}

impl Default for UsageReader {
    fn default() -> Self {
        Self::with_factory(Arc::new(usage::system_sampler), IDLE_END, ANSWER_WITHIN)
    }
}

impl UsageReader {
    /// A reader whose thread uses samplers from `factory`, ends `idle_end` after the last
    /// request and answers within `answer_within`.
    pub fn with_factory(
        factory: SamplerFactory,
        idle_end: Duration,
        answer_within: Duration,
    ) -> Self {
        Self {
            factory,
            idle_end,
            answer_within,
            state: Mutex::new(ReaderState::default()),
        }
    }

    /// Threads started so far this run.
    #[cfg(test)]
    fn threads_started(&self) -> u64 {
        self.state.lock().map_or(0, |s| s.started)
    }

    /// Starts a reader thread; `false` when the system refused one.
    fn start(&self, state: &mut ReaderState) -> bool {
        let (requests, received) = mpsc::channel::<Request>();
        let sampling_since = Arc::new(Mutex::new(None));
        let finished = Arc::new(AtomicU64::new(0));
        let factory = Arc::clone(&self.factory);
        let idle_end = self.idle_end;
        let since = Arc::clone(&sampling_since);
        let done = Arc::clone(&finished);
        let started = std::thread::Builder::new()
            .name("usage-reader".into())
            .spawn(move || read_until_idle(&received, &*factory, idle_end, &since, &done));
        if let Err(e) = started {
            tracing::warn!("could not start the usage reader: {e}");
            return false;
        }
        state.started = state.started.saturating_add(1);
        state.worker = Some(Worker {
            id: state.started,
            requests,
            sampling_since,
            finished,
            timed_out_at: None,
        });
        true
    }

    /// One sample focused on `focus`, or `None` when the reader is stuck, could not start,
    /// or took longer than [`ANSWER_WITHIN`] (every figure is then "readFailed").
    pub async fn sample(&self, focus: Focus) -> Option<Sample> {
        // Twice at most: a thread that ended after its idle time while the request was on
        // its way is started again once.
        for _ in 0..2 {
            let (reply, answer) = tokio::sync::oneshot::channel();
            let id = {
                let Ok(mut state) = self.state.lock() else {
                    return None;
                };
                if state
                    .worker
                    .as_ref()
                    .is_some_and(|w| w.stuck(self.answer_within))
                {
                    if !state.replaced {
                        // Left to finish its call; it ends when it finds its requests gone.
                        state.worker = None;
                        state.replaced = true;
                        self.start(&mut state);
                    }
                    return None;
                }
                if state.worker.is_none() && !self.start(&mut state) {
                    return None;
                }
                let worker = state.worker.as_ref()?;
                let request = Request {
                    focus: focus.clone(),
                    reply,
                };
                if worker.requests.send(request).is_err() {
                    // It ended after its idle time: start another.
                    state.worker = None;
                    continue;
                }
                worker.id
            };
            match tokio::time::timeout(self.answer_within, answer).await {
                Ok(Ok(sample)) => return Some(sample),
                Ok(Err(_)) => {
                    // The thread ended before answering (its idle time ran out as the
                    // request arrived): forget it and try once more.
                    if let Ok(mut state) = self.state.lock()
                        && state.worker.as_ref().is_some_and(|w| w.id == id)
                    {
                        state.worker = None;
                    }
                }
                // Past the limit: the reader is stuck until its call returns.
                Err(_) => {
                    if let Ok(mut state) = self.state.lock()
                        && let Some(worker) = state.worker.as_mut().filter(|w| w.id == id)
                    {
                        worker.timed_out_at = Some(worker.finished.load(Ordering::SeqCst));
                    }
                    return None;
                }
            }
        }
        None
    }
}

/// The reader thread: one sampler for its life, a sample per request, the end after
/// `idle_end` without one (or when the app no longer has its requests).
fn read_until_idle(
    requests: &mpsc::Receiver<Request>,
    factory: &(dyn Fn() -> Box<dyn Sampler + Send> + Send + Sync),
    idle_end: Duration,
    sampling_since: &Mutex<Option<Instant>>,
    finished: &AtomicU64,
) {
    let mut sampler = factory();
    while let Ok(request) = requests.recv_timeout(idle_end) {
        if let Ok(mut since) = sampling_since.lock() {
            *since = Some(Instant::now());
        }
        let sample = sampler.sample(&request.focus);
        if let Ok(mut since) = sampling_since.lock() {
            *since = None;
        }
        finished.fetch_add(1, Ordering::SeqCst);
        let _ = request.reply.send(sample);
    }
}

/// Every figure "Couldn't read this just now" (a stuck reader).
fn failed_sample() -> Sample {
    Sample {
        graphics: Reading::gap(Gap::ReadFailed),
        processor_percent: Reading::gap(Gap::ReadFailed),
        threads: usage::logical_processors(),
        unified: cfg!(all(target_os = "macos", target_arch = "aarch64")),
    }
}

/// The answer from a sample (`None`: the reader is stuck) and the system's memory.
fn answer(sample: Option<Sample>, memory: Option<models::hardware::SystemMemory>) -> UsageNow {
    let memory = match (&sample, memory) {
        (None, _) | (Some(_), None) => Reading::gap(Gap::ReadFailed),
        (Some(_), Some(m)) => Reading::value(MemUse {
            used_bytes: m.total_bytes.saturating_sub(m.available_bytes),
            total_bytes: m.total_bytes,
        }),
    };
    UsageNow {
        sample: sample.unwrap_or_else(failed_sample),
        memory,
    }
}

/// The device the usage line reads (section 5): the loaded model's device, else the one a
/// load would use with the defaults for all models, by the probe's description and total
/// (on a Mac the total is the Metal working set). Nothing when no graphics device is known.
fn focus(
    found: &[(Backend, ProbeResult)],
    status: &RuntimeStatusDto,
    defaults: &LocalModelSettings,
) -> Focus {
    let loaded = matches!(
        status.state,
        RuntimeStateDto::Ready | RuntimeStateDto::Starting
    )
    .then(|| status.backend.as_deref().zip(status.device.as_deref()))
    .flatten()
    .filter(|(backend, _)| *backend != "cpu");
    let device = match loaded {
        Some((backend, name)) => found
            .iter()
            .filter(|(b, p)| b.kind.as_str() == backend && p.is_usable())
            .flat_map(|(_, p)| p.devices.iter())
            .find(|d| d.name == name),
        None => crate::runtime_manager::planned_backend(
            found,
            defaults.backend.as_deref(),
            defaults.device.as_deref(),
        )
        .filter(|kind| *kind != runtime::BackendKind::Cpu)
        .and_then(|kind| {
            let probed = &found
                .iter()
                .find(|(b, p)| b.kind == kind && p.is_usable())?
                .1
                .devices;
            defaults
                .device
                .as_deref()
                .and_then(|name| probed.iter().find(|d| d.name == name))
                .or_else(|| runtime::pick_device(probed))
        }),
    };
    Focus {
        description: device.map(|d| d.description.clone()),
        total_hint_bytes: device
            .map(|d| d.total_mib.saturating_mul(MIB))
            .filter(|b| *b > 0),
    }
}

/// This computer's usage now (the usage line; main window only). Read here, on request,
/// never sent anywhere.
#[tauri::command]
pub async fn usage_now(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<UsageNow> {
    crate::commands::only_main_window(window.label())?;
    let found = state.runtime.cached_backends().unwrap_or_default();
    let defaults = crate::load_settings::read_defaults(&state.store)
        .await
        .settings;
    let focus = focus(&found, &state.runtime.status(), &defaults);
    let sample = state.usage.sample(focus).await;
    Ok(answer(sample, models::hardware::system_memory()))
}

#[cfg(test)]
mod tests;
