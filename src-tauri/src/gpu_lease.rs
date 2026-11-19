//! 0.1.0 Studio: the graphics card has one tenant at a time, the chat model or a local Studio
//! job (studio-local-design.md, "The graphics-card lease").
//!
//! A job that fits beside the loaded chat model runs alongside it. One that does not waits
//! for a reply being written and unloads the chat model, asks first, or runs on the
//! processor, as Settings → Studio says ([`GpuPolicy`]); on a Mac's shared memory "Keep in
//! memory" on the chat model always unloads it. A job holds the card only from its grant to
//! the drop of its [`StudioHold`]: while it still waits (for a reply, a Model check or the
//! person's answer) it holds nothing, so a chat load goes first and the unload waits for it.
//! A chat load while a job holds the card waits for it to end (the [`StudioGate`] on
//! `RuntimeManager::load`), and the next chat load after a job probes the backends again.
//! A chat load that passed the gate counts as in flight until its server runs: a job's
//! grant waits for it ([`LeaseWait::ChatModelLoading`]) and reads the card again after it,
//! so the two never land together.
//! A job's unload stops the chat model only between two chat loads, never in the middle of
//! one, and not at all when one passed the gate since the job read the card: that load went
//! first, and the job decides again on the card as it is now.
//! A job's need is judged with the chat model as it was then ([`StudioNeed::judged_with`]):
//! when a chat model on the card is not the one it was judged with (one started, or another
//! took its place, while the job waited), the lease grants nothing and the job is judged
//! again ([`Refused::JudgeAgain`]), so it never runs alongside a chat model it was not
//! judged beside.
//! A hold never loads the chat model again: "Reload it after Studio" is
//! [`GpuLease::reload_chat`], called once when the local queue empties.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, TryAcquireError, watch};
use tokio_util::sync::CancellationToken;

use crate::dto::RuntimeStatusDto;
use crate::error::{AppError, AppResult};
use crate::runtime_manager::{RuntimeManager, StudioGate};
use crate::state::{AppState, BusyCounter};
use crate::studio::prefs::GpuPolicy;

/// What a local job needs of the graphics card, from `gen_fit` with and without the chat
/// model (the manager's local lane works it out for each job).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StudioNeed {
    /// Green, or one part at a time, with the memory free now (the chat model loaded).
    pub alongside: bool,
    /// Not red once the chat model is unloaded.
    pub after_unload: bool,
    /// A picture whose processor-only fit is not red.
    pub processor_ok: bool,
    /// Apple silicon: the processor and the graphics share one memory.
    pub shared_memory: bool,
    /// The chat model's bytes on the graphics device this need was judged with (`None`: no
    /// chat model was on one). A chat model on the card now that differs from it makes the
    /// lease ask for the job to be judged again ([`Refused::JudgeAgain`]).
    pub judged_with: Option<u64>,
}

/// How a job may use the card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Grant {
    /// Beside the chat model, or with none loaded ("Chat replies may be slower while this
    /// runs" when one is).
    Alongside,
    /// The chat model was unloaded to make room: `unloaded` is its path, for "Load it
    /// again" (`None` when it had gone by the time the unload ran).
    AfterUnload { unloaded: Option<String> },
    /// On the processor. `unloaded` as for [`Grant::AfterUnload`] when the chat model was
    /// unloaded first (a job that does not fit even after an unload, on shared memory,
    /// where the processor uses the same memory): "The chat model was unloaded to make
    /// room", and "Reload it after Studio". `None` when nothing was unloaded: the chat model
    /// stays loaded.
    UseProcessor { unloaded: Option<String> },
}

/// Why a job waits before its grant, shown on its card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseWait {
    /// A reply (or a Model check) uses the chat model: "Starts when the reply ends".
    ReplyRunning,
    /// Another Studio job holds the card. Unreachable in 0.1.0: the one local queue never
    /// lets two jobs ask at once.
    AnotherStudioJob,
    /// Under [`GpuPolicy::Ask`] the person has not answered "Unload the chat model?" yet.
    NeedsAnswer,
    /// A chat model is loading: it passed the chat load's gate and its server has not
    /// started yet. The job reads the card once it has.
    ChatModelLoading,
}

/// Why a job got no grant. It holds nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refused {
    /// The job was stopped while it waited, or the unload it waited for was cancelled.
    Cancelled,
    /// It does not fit even with the chat model unloaded, and cannot run on the processor
    /// (or the person said not to unload).
    DoesNotFit,
    /// Under [`GpuPolicy::Ask`]: ask the person, then ask the lease again with the answer.
    NeedsAnswer,
    /// The chat model on the card is not the one the job's need was judged with
    /// ([`StudioNeed::judged_with`]): one started, or another took its place, while the job
    /// waited. Judge the job again with the card as it is now, then ask again.
    JudgeAgain,
}

/// The chat model's side of the card: what it takes, and its unload and reload.
#[async_trait]
pub trait ChatSide: Send + Sync {
    /// The loaded (or loading) chat model's bytes on a graphics device; `None` when none is
    /// on one.
    fn loaded_device_bytes(&self) -> Option<u64>;
    /// The loaded chat model was started with "Keep in memory".
    fn keeps_in_memory(&self) -> bool;
    /// `before_change(when_idle = true)` then `unload_by_user()`; `on_wait(true)` while a
    /// reply is being written, `on_wait(false)` when it ends. The unloaded path.
    ///
    /// The stop comes between two chat loads, never in the middle of one, and no chat load
    /// starts until it ends. Right before it, `go_ahead` is asked: false (a chat load went
    /// first since the lease read the card) stops nothing and returns `Ok(None)`.
    async fn unload_when_idle(
        &self,
        on_wait: &(dyn Fn(bool) + Send + Sync),
        go_ahead: &(dyn Fn() -> bool + Send + Sync),
    ) -> AppResult<Option<String>>;
    /// Loads the chat model at `path` again, unless the person picked a model since: one
    /// whose load waits at the gate or is in flight (`loads_in_flight` chat loads passed the
    /// gate and have not started their server yet) is theirs, and goes ahead instead.
    async fn reload(&self, path: &str, loads_in_flight: usize);
}

/// The app's chat side: the local llama-server and everything that uses it.
pub struct RuntimeChat {
    pub runtime: Arc<RuntimeManager>,
    pub busy: Arc<BusyCounter>,
}

#[async_trait]
impl ChatSide for RuntimeChat {
    fn loaded_device_bytes(&self) -> Option<u64> {
        self.runtime.device_bytes()
    }

    fn keeps_in_memory(&self) -> bool {
        self.runtime.keeps_in_memory()
    }

    async fn unload_when_idle(
        &self,
        on_wait: &(dyn Fn(bool) + Send + Sync),
        go_ahead: &(dyn Fn() -> bool + Send + Sync),
    ) -> AppResult<Option<String>> {
        // The wait for a reply or a Model check is made here: when the job is stopped this
        // future is dropped, which leaves nothing behind (a `before_change` dropped while it
        // waits would leave its wait, and "Restarts when the reply ends", behind).
        if self.busy.is_busy() {
            on_wait(true);
            self.busy.wait_idle().await;
            on_wait(false);
        }
        let stopped = |e: tokio::task::JoinError| {
            AppError::internal(format!("the chat model's unload stopped: {e}"))
        };
        let runtime = Arc::clone(&self.runtime);
        let busy = Arc::clone(&self.busy);
        // Nothing uses the model now, so `before_change` goes ahead at once; it runs to its
        // end even if the job is stopped meanwhile. It comes before the hold on the loads
        // below: a use that began just now is waited for here, and a Model check's load
        // must not wait behind that hold meanwhile.
        tokio::spawn(async move { runtime.before_change(&busy, true).await })
            .await
            .map_err(stopped)??;
        // A chat load that passed the gate meanwhile starts its server first; from here no
        // chat load starts or passes the gate until the stop has ended. Waiting for it
        // changes nothing, so a job stopped meanwhile leaves nothing behind.
        let between = self.runtime.between_loads().await;
        // A chat load went first since the lease read the card: it stays, and the lease
        // reads the card again.
        if !go_ahead() {
            return Ok(None);
        }
        let runtime = Arc::clone(&self.runtime);
        // From here the unload runs to its end even if the job is stopped meanwhile: a
        // server stopped halfway would leave the status at "Stopping".
        tokio::spawn(async move {
            let _between = between;
            let path = runtime.status().model_path;
            runtime.unload_by_user().await;
            path
        })
        .await
        .map_err(stopped)
    }

    async fn reload(&self, path: &str, loads_in_flight: usize) {
        use tauri::Manager;
        if !reload_wanted(&self.runtime.status(), loads_in_flight, path) {
            return;
        }
        let app = self.runtime.app().clone();
        let Some(state) = app.try_state::<AppState>() else {
            return;
        };
        // With the model's saved settings, as "Unloaded to free memory · Load" does; a
        // reply that uses the model meanwhile is waited for, never stopped.
        if let Err(e) =
            crate::models_cmd::load_local_model(state, path.to_string(), None, Some(true)).await
        {
            tracing::warn!(code = %e.code, "the chat model did not load again after Studio");
        }
    }
}

/// "Reload it after Studio" loads the chat model at `path` again only while the status still
/// names it as unloaded to free memory: a model the person loaded (or unloaded) since then,
/// or removed, stays as it is. Nor while a chat load the person started waits for Studio
/// (`waiting_for_studio`) or is in flight (`loads_in_flight` passed the gate and have not
/// reached Starting, which is what clears `unloaded_path`): that load is the person's pick,
/// and the reload would replace it.
fn reload_wanted(status: &RuntimeStatusDto, loads_in_flight: usize, path: &str) -> bool {
    status.unloaded_path.as_deref() == Some(path)
        && !status.waiting_for_studio
        && loads_in_flight == 0
}

/// A local job's hold on the graphics card. Dropping it frees the card and wakes a chat load
/// that waits; it never loads the chat model again.
pub struct StudioHold {
    lease: Arc<GpuLease>,
    // Given back after `drop` has marked the card free, so a waiting job asks again only then.
    _permit: OwnedSemaphorePermit,
}

impl std::fmt::Debug for StudioHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StudioHold").finish_non_exhaustive()
    }
}

impl Drop for StudioHold {
    fn drop(&mut self) {
        // Before the card is marked free, so the chat load it wakes probes again.
        self.lease.reprobe.store(true, Ordering::Release);
        self.lease.card.send_modify(|card| card.granted = false);
    }
}

/// The card as the lease and the chat loads see it, changed only under the watch's lock so
/// that a grant and a chat load's pass through the gate never both happen.
#[derive(Debug, Clone, Copy, Default)]
struct Card {
    /// A job holds the card: from its grant to the drop of its hold.
    granted: bool,
    /// Chat loads that passed the gate and have not started their server yet.
    loads: usize,
    /// Chat loads that ever passed the gate: a card read before one passed is read again.
    passed: u64,
}

/// What a job's grant needs next.
enum Step {
    Grant(Grant),
    /// An unload first, then this grant (when no chat load took the card meanwhile).
    Unload(AfterIt),
}

/// The grant an unload is for.
#[derive(Clone, Copy)]
enum AfterIt {
    AfterUnload,
    UseProcessor,
}

/// What a job's unload did.
enum Unloaded {
    /// It stopped the chat model: its path (`None` when none was loaded by then).
    Stopped(Option<String>),
    /// A chat load passed the gate since the card was read: it went first, and nothing was
    /// stopped.
    WentFirst,
}

/// One graphics-card tenant at a time: the chat model, or a local Studio job.
pub struct GpuLease {
    chat: Arc<dyn ChatSide>,
    /// One Studio job asks, waits and runs at a time.
    permits: Arc<Semaphore>,
    /// The grant and the chat loads in flight.
    card: watch::Sender<Card>,
    /// A job freed the card since the last chat load asked.
    reprobe: AtomicBool,
}

impl GpuLease {
    pub fn new(chat: Arc<dyn ChatSide>) -> Arc<Self> {
        Arc::new(Self {
            chat,
            permits: Arc::new(Semaphore::new(1)),
            card: watch::Sender::new(Card::default()),
            reprobe: AtomicBool::new(false),
        })
    }

    /// The card for one local job. In this order: the one place (`on_wait(AnotherStudioJob)`
    /// while another job has it); no chat model on a device, or a job that fits beside it
    /// (unless "Keep in memory" pins it in shared memory) → [`Grant::Alongside`]; a job that
    /// does not fit even after an unload → [`Grant::UseProcessor`] when it can run there
    /// (after an unload on shared memory, where the processor uses the same memory), else
    /// [`Refused::DoesNotFit`]; "Keep in memory" on shared memory → an unload whatever the
    /// policy; then as `policy` says. An unload waits for a reply being written
    /// (`on_wait(ReplyRunning)`). A grant after an unload names the unloaded chat model.
    ///
    /// `answered`: the person's answer to "Unload the chat model?" under [`GpuPolicy::Ask`]
    /// (`None`: not asked yet, which ends with `on_wait(NeedsAnswer)` and
    /// [`Refused::NeedsAnswer`]). Every wait ends with [`Refused::Cancelled`] when `cancel`
    /// fires. The card counts as held only once the grant is returned: a chat load while
    /// the job waits goes first. The card is read only while no chat load is in flight
    /// (between the gate and its server starting; `on_wait(ChatModelLoading)` while one
    /// is), and read again when one passed the gate before the grant, so a job never gets
    /// the card under a chat model about to start.
    pub async fn acquire_for_studio(
        self: &Arc<Self>,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<(Grant, StudioHold), Refused> {
        let permit = self.take_place(cancel, on_wait).await?;
        // Marks the card as held when it returns a grant.
        let grant = self.decide(need, policy, answered, cancel, on_wait).await?;
        Ok((
            grant,
            StudioHold {
                lease: Arc::clone(self),
                _permit: permit,
            },
        ))
    }

    /// "Reload it after Studio": the manager calls this once, when the local queue is empty,
    /// never per job. The chat loads in flight now go with it, so a load the person started
    /// meanwhile is never replaced ([`ChatSide::reload`]).
    pub async fn reload_chat(&self, path: &str) {
        let loads_in_flight = self.card.borrow().loads;
        self.chat.reload(path, loads_in_flight).await;
    }

    /// The one place for a Studio job.
    async fn take_place(
        &self,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<OwnedSemaphorePermit, Refused> {
        if cancel.is_cancelled() {
            return Err(Refused::Cancelled);
        }
        match Arc::clone(&self.permits).try_acquire_owned() {
            Ok(permit) => return Ok(permit),
            // The semaphore is never closed.
            Err(TryAcquireError::Closed) => return Err(Refused::Cancelled),
            Err(TryAcquireError::NoPermits) => {}
        }
        on_wait(LeaseWait::AnotherStudioJob);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(Refused::Cancelled),
            permit = Arc::clone(&self.permits).acquire_owned() => {
                permit.map_err(|_| Refused::Cancelled)
            }
        }
    }

    /// The grant, in the order of [`Self::acquire_for_studio`], with the card marked as
    /// held. Each round waits until no chat load is in flight, reads the card, and grants
    /// only if no chat load passed the gate since; else, and after an unload, it reads the
    /// card again. An unload stops nothing when a chat load passed the gate since the card
    /// was read: that load went first.
    async fn decide(
        &self,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<Grant, Refused> {
        // The grant this job's unload was for, once it has unloaded the chat model.
        let mut unloaded: Option<Grant> = None;
        loop {
            let seen = self.no_load_in_flight(cancel, on_wait).await?;
            let step = match &unloaded {
                Some(grant) if self.chat.loaded_device_bytes().is_none() => {
                    Step::Grant(grant.clone())
                }
                // A chat load took the card during the unload: as if asked now.
                _ => self.step(need, policy, answered, on_wait)?,
            };
            match step {
                Step::Grant(grant) => {
                    if self.grant_if_unchanged(seen) {
                        return Ok(grant);
                    }
                }
                Step::Unload(after) => {
                    let Unloaded::Stopped(path) = self.unload(seen, cancel, on_wait).await? else {
                        // A chat load went first: the card is read again.
                        continue;
                    };
                    let earlier = match unloaded {
                        Some(
                            Grant::AfterUnload { unloaded } | Grant::UseProcessor { unloaded },
                        ) => unloaded,
                        _ => None,
                    };
                    let path = path.or(earlier);
                    unloaded = Some(match after {
                        AfterIt::AfterUnload => Grant::AfterUnload { unloaded: path },
                        AfterIt::UseProcessor => Grant::UseProcessor { unloaded: path },
                    });
                }
            }
        }
    }

    /// The decision on the card as it is now. A chat model on the card that `need` was not
    /// judged with gives no decision: [`Refused::JudgeAgain`].
    fn step(
        &self,
        need: &StudioNeed,
        policy: GpuPolicy,
        answered: Option<bool>,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<Step, Refused> {
        let Some(chat) = self.chat.loaded_device_bytes() else {
            return Ok(Step::Grant(Grant::Alongside));
        };
        if need.judged_with != Some(chat) {
            return Err(Refused::JudgeAgain);
        }
        // On shared memory the chat model's pinned memory is the job's memory too; on a
        // graphics card "Keep in memory" pins system memory only.
        let pinned = need.shared_memory && self.chat.keeps_in_memory();
        if need.alongside && !pinned {
            return Ok(Step::Grant(Grant::Alongside));
        }
        if !need.after_unload {
            if !need.processor_ok {
                return Err(Refused::DoesNotFit);
            }
            if need.shared_memory {
                return Ok(Step::Unload(AfterIt::UseProcessor));
            }
            return Ok(Step::Grant(Grant::UseProcessor { unloaded: None }));
        }
        if pinned {
            return Ok(Step::Unload(AfterIt::AfterUnload));
        }
        match (policy, answered) {
            (GpuPolicy::Unload, _) | (GpuPolicy::Ask, Some(true)) => {
                Ok(Step::Unload(AfterIt::AfterUnload))
            }
            (GpuPolicy::Ask, None) => {
                on_wait(LeaseWait::NeedsAnswer);
                Err(Refused::NeedsAnswer)
            }
            (GpuPolicy::Ask, Some(false)) if need.processor_ok => {
                Ok(Step::Grant(Grant::UseProcessor { unloaded: None }))
            }
            (GpuPolicy::Ask, Some(false)) => Err(Refused::DoesNotFit),
            (GpuPolicy::Processor, _) if need.processor_ok => {
                Ok(Step::Grant(Grant::UseProcessor { unloaded: None }))
            }
            (GpuPolicy::Processor, _) => Ok(Step::Unload(AfterIt::AfterUnload)),
        }
    }

    /// Waits until no chat load is between the gate and its server starting (one that
    /// passed is about to take the card, and the card does not show it yet), with
    /// `on_wait(ChatModelLoading)` while one is. Returns how many have passed the gate so
    /// far.
    async fn no_load_in_flight(
        &self,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<u64, Refused> {
        let mut card = self.card.subscribe();
        let mut told = false;
        loop {
            if cancel.is_cancelled() {
                return Err(Refused::Cancelled);
            }
            // Each change is read here, so a load that passes as the job begins to wait is
            // told about too.
            let now = *card.borrow_and_update();
            if now.loads == 0 {
                return Ok(now.passed);
            }
            if !told {
                on_wait(LeaseWait::ChatModelLoading);
                told = true;
            }
            tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(Refused::Cancelled),
                changed = card.changed() => {
                    // The sender lives as long as `self`, so this cannot fail.
                    if changed.is_err() {
                        return Err(Refused::Cancelled);
                    }
                }
            }
        }
    }

    /// Marks the card as held, unless a chat load passed the gate since the card was read
    /// (`seen` passes then): checked and marked at once, as the gate counts a load.
    fn grant_if_unchanged(&self, seen: u64) -> bool {
        self.card.send_if_modified(|card| {
            let unchanged = card.loads == 0 && card.passed == seen;
            if unchanged {
                card.granted = true;
            }
            unchanged
        })
    }

    /// Unloads the chat model once nothing uses it, unless a chat load passed the gate since
    /// the card was read (`seen`): that load went first and stays. This is asked where no
    /// chat load can start, right before the stop. A failed unload (its wait cancelled by a
    /// newer change of what is loaded) frees nothing, so the job gets no grant.
    async fn unload(
        &self,
        seen: u64,
        cancel: &CancellationToken,
        on_wait: &(dyn Fn(LeaseWait) + Send + Sync),
    ) -> Result<Unloaded, Refused> {
        let reply_running = |waiting: bool| {
            if waiting {
                on_wait(LeaseWait::ReplyRunning);
            }
        };
        let went_first = AtomicBool::new(false);
        let go_ahead = || {
            let card = *self.card.borrow();
            let unchanged = card.loads == 0 && card.passed == seen;
            if !unchanged {
                went_first.store(true, Ordering::Release);
            }
            unchanged
        };
        let unloaded = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Refused::Cancelled),
            unloaded = self.chat.unload_when_idle(&reply_running, &go_ahead) => {
                unloaded.map_err(|e| {
                    tracing::warn!(code = %e.code, "the chat model was not unloaded for Studio");
                    Refused::Cancelled
                })?
            }
        };
        if went_first.load(Ordering::Acquire) {
            return Ok(Unloaded::WentFirst);
        }
        Ok(Unloaded::Stopped(unloaded))
    }
}

#[async_trait]
impl StudioGate for GpuLease {
    fn holds_card(&self) -> bool {
        self.card.borrow().granted
    }

    async fn wait_until_free(&self) {
        let mut card = self.card.subscribe();
        // The sender lives as long as `self`, so this cannot fail.
        let _ = card.wait_for(|card| !card.granted).await;
    }

    fn take_reprobe(&self) -> bool {
        self.reprobe.swap(false, Ordering::AcqRel)
    }

    fn begin_load(&self) -> bool {
        self.card.send_if_modified(|card| {
            if card.granted {
                return false;
            }
            card.loads += 1;
            card.passed = card.passed.wrapping_add(1);
            true
        })
    }

    fn end_load(&self) {
        self.card
            .send_modify(|card| card.loads = card.loads.saturating_sub(1));
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::AtomicUsize;
    use std::time::Duration;

    use super::*;
    use crate::state::{BusyGuard, BusyKind};

    const GB: u64 = 1_000_000_000;
    const CHAT: &str = "/m/chat.gguf";

    /// The chat model's side, in memory: a model on the card (or none), "Keep in memory",
    /// the replies and Model checks that use it, and the unloads and reloads asked for.
    pub(crate) struct FakeChat {
        device_bytes: StdMutex<Option<u64>>,
        keeps_in_memory: bool,
        busy: Arc<BusyCounter>,
        unloads: AtomicUsize,
        /// Unloads that asked whether to go ahead (stopped or not).
        asked: AtomicUsize,
        /// Each reload asked for, with the chat loads in flight it was told about.
        reloads: StdMutex<Vec<(String, usize)>>,
    }

    impl FakeChat {
        fn with(device_bytes: Option<u64>, keeps_in_memory: bool) -> Arc<Self> {
            Arc::new(Self {
                device_bytes: StdMutex::new(device_bytes),
                keeps_in_memory,
                busy: Arc::new(BusyCounter::default()),
                unloads: AtomicUsize::new(0),
                asked: AtomicUsize::new(0),
                reloads: StdMutex::new(Vec::new()),
            })
        }

        /// The chat model loaded on the card, taking `bytes` there.
        pub(crate) fn loaded(bytes: u64) -> Arc<Self> {
            Self::with(Some(bytes), false)
        }

        /// Loaded with "Keep in memory".
        fn keeping_in_memory(bytes: u64) -> Arc<Self> {
            Self::with(Some(bytes), true)
        }

        /// No chat model on a device.
        fn none() -> Arc<Self> {
            Self::with(None, false)
        }

        /// A reply (or a Model check) uses the chat model until the guard is dropped.
        fn use_model(&self, kind: BusyKind) -> BusyGuard {
            self.busy.hold(kind)
        }

        /// A chat load reached Starting: its model now takes `bytes` on the card.
        fn set_loaded(&self, bytes: u64) {
            *self.device_bytes.lock().unwrap() = Some(bytes);
        }

        /// A chat load reached Starting on the processor: nothing of it is on the card.
        fn set_on_processor(&self) {
            *self.device_bytes.lock().unwrap() = None;
        }

        pub(crate) fn unloads(&self) -> usize {
            self.unloads.load(Ordering::SeqCst)
        }

        /// Returns once `n` unloads asked whether to go ahead (and did what it said).
        async fn until_asked(&self, n: usize) {
            within(async {
                while self.asked.load(Ordering::SeqCst) < n {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
        }

        fn reloads(&self) -> Vec<String> {
            self.reloads
                .lock()
                .unwrap()
                .iter()
                .map(|(path, _)| path.clone())
                .collect()
        }

        /// The chat loads in flight each reload was told about.
        fn reload_loads(&self) -> Vec<usize> {
            self.reloads
                .lock()
                .unwrap()
                .iter()
                .map(|(_, loads)| *loads)
                .collect()
        }
    }

    #[async_trait]
    impl ChatSide for FakeChat {
        fn loaded_device_bytes(&self) -> Option<u64> {
            *self.device_bytes.lock().unwrap()
        }

        fn keeps_in_memory(&self) -> bool {
            self.keeps_in_memory
        }

        async fn unload_when_idle(
            &self,
            on_wait: &(dyn Fn(bool) + Send + Sync),
            go_ahead: &(dyn Fn() -> bool + Send + Sync),
        ) -> AppResult<Option<String>> {
            if self.busy.is_busy() {
                on_wait(true);
                self.busy.wait_idle().await;
                on_wait(false);
            }
            let unloaded = if go_ahead() {
                let was = self.device_bytes.lock().unwrap().take();
                self.unloads.fetch_add(1, Ordering::SeqCst);
                was.map(|_| CHAT.to_string())
            } else {
                None
            };
            self.asked.fetch_add(1, Ordering::SeqCst);
            Ok(unloaded)
        }

        async fn reload(&self, path: &str, loads_in_flight: usize) {
            self.reloads
                .lock()
                .unwrap()
                .push((path.to_string(), loads_in_flight));
        }
    }

    /// A need judged with the tests' usual chat model (5 GB on the card).
    fn need(alongside: bool, after_unload: bool, processor_ok: bool) -> StudioNeed {
        StudioNeed {
            alongside,
            after_unload,
            processor_ok,
            shared_memory: false,
            judged_with: Some(5 * GB),
        }
    }

    fn shared(need: StudioNeed) -> StudioNeed {
        StudioNeed {
            shared_memory: true,
            ..need
        }
    }

    fn tok() -> CancellationToken {
        CancellationToken::new()
    }

    /// The waits a job was told about.
    #[derive(Default, Clone)]
    struct Waits(Arc<StdMutex<Vec<LeaseWait>>>);

    impl Waits {
        fn push(&self, wait: LeaseWait) {
            self.0.lock().unwrap().push(wait);
        }

        fn seen(&self) -> Vec<LeaseWait> {
            self.0.lock().unwrap().clone()
        }

        /// Returns once `wait` was seen.
        async fn until(&self, wait: LeaseWait) {
            within(async {
                while !self.seen().contains(&wait) {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await;
        }
    }

    /// `future`'s output, with a bound far above what any of these waits takes.
    async fn within<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::time::timeout(Duration::from_secs(10), future)
            .await
            .expect("finished in time")
    }

    /// Asks `lease` in a task of its own, telling `waits` about each wait.
    fn ask_in_task(
        lease: &Arc<GpuLease>,
        need: StudioNeed,
        policy: GpuPolicy,
        cancel: &CancellationToken,
        waits: &Waits,
    ) -> tokio::task::JoinHandle<Result<(Grant, StudioHold), Refused>> {
        let (lease, cancel, waits) = (Arc::clone(lease), cancel.clone(), waits.clone());
        tokio::spawn(async move {
            lease
                .acquire_for_studio(&need, policy, None, &cancel, &move |w| waits.push(w))
                .await
        })
    }

    #[tokio::test]
    async fn a_job_that_fits_beside_the_chat_model_runs_alongside() {
        let chat = FakeChat::loaded(5 * GB);
        let lease = GpuLease::new(chat.clone());
        let (g, hold) = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(g, Grant::Alongside);
        assert_eq!(chat.unloads(), 0);
        drop(hold);

        // With no chat model on a device there is nothing to unload.
        let chat = FakeChat::none();
        let lease = GpuLease::new(chat.clone());
        let (g, _hold) = lease
            .acquire_for_studio(
                &need(false, true, false),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(g, Grant::Alongside);
        assert_eq!(chat.unloads(), 0);
    }

    #[tokio::test]
    async fn a_job_that_does_not_fit_even_after_an_unload_is_refused() {
        let chat = FakeChat::loaded(5 * GB);
        let lease = GpuLease::new(chat.clone());
        let refused = lease
            .acquire_for_studio(
                &need(false, false, false),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap_err();
        assert_eq!(refused, Refused::DoesNotFit);
        assert_eq!(chat.unloads(), 0);
        assert!(!lease.holds_card());

        // A picture that can run on the processor does, beside the chat model (and the
        // refusal above gave its place back).
        let (g, hold) = within(lease.acquire_for_studio(
            &need(false, false, true),
            GpuPolicy::Unload,
            None,
            &tok(),
            &|_| {},
        ))
        .await
        .unwrap();
        assert_eq!(g, Grant::UseProcessor { unloaded: None });
        assert_eq!(chat.unloads(), 0);
        drop(hold);

        // On shared memory the processor uses the chat model's memory: it is unloaded first,
        // and the grant names it, for "Load it again" (review fix L12-unloaded).
        let (g, _hold) = lease
            .acquire_for_studio(
                &shared(need(false, false, true)),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(
            g,
            Grant::UseProcessor {
                unloaded: Some(CHAT.to_string())
            }
        );
        assert_eq!(chat.unloads(), 1);
    }

    #[tokio::test]
    async fn a_job_that_does_not_fit_waits_for_the_reply_then_unloads() {
        let chat = FakeChat::loaded(5 * GB);
        let reply = chat.use_model(BusyKind::ChatTurn);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            drop(reply);
        });
        let lease = GpuLease::new(chat.clone());
        let waits = Waits::default();
        let (g, _hold) = within(lease.acquire_for_studio(
            &need(false, true, false),
            GpuPolicy::Unload,
            None,
            &tok(),
            &|w| waits.push(w),
        ))
        .await
        .unwrap();
        assert_eq!(waits.seen(), [LeaseWait::ReplyRunning]);
        assert_eq!(
            g,
            Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            }
        );
        assert_eq!(chat.unloads(), 1);
    }

    #[tokio::test]
    async fn ask_without_an_answer_asks_and_the_answer_decides() {
        let ask = |chat: &Arc<FakeChat>, processor_ok: bool, answered: Option<bool>| {
            let lease = GpuLease::new(chat.clone());
            async move {
                let waits = Waits::default();
                let result = lease
                    .acquire_for_studio(
                        &need(false, true, processor_ok),
                        GpuPolicy::Ask,
                        answered,
                        &tok(),
                        &|w| waits.push(w),
                    )
                    .await
                    .map(|(g, _hold)| g);
                (result, waits.seen())
            }
        };

        // Not asked yet: ask, and change nothing meanwhile.
        let chat = FakeChat::loaded(5 * GB);
        let (result, seen) = ask(&chat, true, None).await;
        assert_eq!(result, Err(Refused::NeedsAnswer));
        assert_eq!(seen, [LeaseWait::NeedsAnswer]);
        assert_eq!(chat.unloads(), 0);

        // Yes: unload.
        let (result, _) = ask(&chat, true, Some(true)).await;
        assert_eq!(
            result,
            Ok(Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            })
        );
        assert_eq!(chat.unloads(), 1);

        // No: the processor for a picture that can run there, else it does not fit.
        let chat = FakeChat::loaded(5 * GB);
        assert_eq!(
            ask(&chat, true, Some(false)).await.0,
            Ok(Grant::UseProcessor { unloaded: None })
        );
        assert_eq!(
            ask(&chat, false, Some(false)).await.0,
            Err(Refused::DoesNotFit)
        );
        assert_eq!(chat.unloads(), 0);
    }

    #[tokio::test]
    async fn the_processor_policy_never_unloads() {
        let chat = FakeChat::loaded(5 * GB);
        let lease = GpuLease::new(chat.clone());
        let (g, _hold) = lease
            .acquire_for_studio(
                &need(false, true, true),
                GpuPolicy::Processor,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(g, Grant::UseProcessor { unloaded: None });
        assert_eq!(chat.unloads(), 0);
    }

    #[tokio::test]
    async fn keep_in_memory_always_unloads_on_shared_memory() {
        // Whatever the policy: the processor uses the same memory, and nobody is asked.
        for policy in [GpuPolicy::Unload, GpuPolicy::Processor, GpuPolicy::Ask] {
            let chat = FakeChat::keeping_in_memory(5 * GB);
            let lease = GpuLease::new(chat.clone());
            let (g, _hold) = lease
                .acquire_for_studio(
                    &shared(need(true, true, true)),
                    policy,
                    None,
                    &tok(),
                    &|_| {},
                )
                .await
                .unwrap();
            assert_eq!(
                g,
                Grant::AfterUnload {
                    unloaded: Some(CHAT.to_string())
                },
                "{policy:?}"
            );
            assert_eq!(chat.unloads(), 1, "{policy:?}");
        }

        // On a graphics card "Keep in memory" pins system memory only.
        let chat = FakeChat::keeping_in_memory(5 * GB);
        let lease = GpuLease::new(chat.clone());
        let (g, _hold) = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert_eq!(g, Grant::Alongside);
        assert_eq!(chat.unloads(), 0);
    }

    #[tokio::test]
    async fn one_studio_job_holds_the_card_at_a_time() {
        let lease = GpuLease::new(FakeChat::none());
        let (_, first) = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        let waits = Waits::default();
        let second = ask_in_task(
            &lease,
            need(true, true, true),
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        waits.until(LeaseWait::AnotherStudioJob).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!second.is_finished(), "two jobs held the card at once");
        drop(first);
        let (g, _hold) = within(second).await.unwrap().unwrap();
        assert_eq!(g, Grant::Alongside);
        assert_eq!(waits.seen(), [LeaseWait::AnotherStudioJob]);
    }

    #[tokio::test]
    async fn a_chat_load_waits_while_studio_holds_the_card() {
        let lease = GpuLease::new(FakeChat::none());
        assert!(!lease.holds_card());
        assert!(!lease.take_reprobe());
        within(lease.wait_until_free()).await;

        let (_, hold) = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert!(lease.holds_card());
        let load = {
            let lease = Arc::clone(&lease);
            tokio::spawn(async move { lease.wait_until_free().await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !load.is_finished(),
            "the chat load went ahead during the job"
        );
        assert!(!lease.take_reprobe(), "nothing was freed yet");

        drop(hold);
        within(load).await.unwrap();
        assert!(!lease.holds_card());
        // The next chat load probes the backends again, once.
        assert!(lease.take_reprobe());
        assert!(!lease.take_reprobe());
    }

    #[tokio::test]
    async fn a_chat_load_while_a_job_waits_for_the_card_goes_first() {
        let chat = FakeChat::loaded(5 * GB);
        // A Model check holds a use of the chat model.
        let check = chat.use_model(BusyKind::ModelCheck);
        let lease = GpuLease::new(chat.clone());
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            need(false, true, false),
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        waits.until(LeaseWait::ReplyRunning).await;
        // The job waits in the unload and holds nothing: the check's load is not held.
        assert!(!lease.holds_card());
        within(lease.wait_until_free()).await;
        assert_eq!(chat.unloads(), 0);
        assert!(!job.is_finished());

        // When the check ends the job unloads and takes the card.
        drop(check);
        let (g, hold) = within(job).await.unwrap().unwrap();
        assert_eq!(
            g,
            Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            }
        );
        assert_eq!(chat.unloads(), 1);
        assert!(lease.holds_card());
        drop(hold);
        assert!(!lease.holds_card());
    }

    /// A chat load as `RuntimeManager::load` makes it: through the gate, waiting while a
    /// job holds the card. Returns once it passed (it is then in flight).
    fn chat_load_in_task(lease: &Arc<GpuLease>) -> tokio::task::JoinHandle<()> {
        let lease = Arc::clone(lease);
        tokio::spawn(async move {
            while !lease.begin_load() {
                lease.wait_until_free().await;
            }
        })
    }

    /// Review fix L12-1: a chat load that waited for a job passes the gate when the job
    /// ends, and is not on the card until it reaches Starting. The next job's grant waits
    /// for it and reads the card after it, so it never gets "Alongside" while the chat
    /// model is about to start.
    #[tokio::test]
    async fn a_job_waits_for_a_chat_load_that_passed_the_gate() {
        // Job 1 unloaded the chat model and holds the card.
        let chat = FakeChat::none();
        let lease = GpuLease::new(chat.clone());
        let (_, first) = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        // "Load it again" waits at the gate.
        let load = chat_load_in_task(&lease);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!load.is_finished(), "the chat load went ahead during job 1");

        // Job 1 ends: the load passes the gate and probes the backends; job 2 asks.
        drop(first);
        within(load).await.unwrap();
        let waits = Waits::default();
        let second = ask_in_task(
            &lease,
            need(false, true, false),
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        // Its card says why it waits (review fix L12-wait-reason).
        waits.until(LeaseWait::ChatModelLoading).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !second.is_finished(),
            "job 2 got the card while a chat load was in flight"
        );
        assert!(!lease.holds_card(), "a job that waits holds nothing");
        assert_eq!(chat.unloads(), 0);

        // The load reaches Starting: job 2 sees the chat model and unloads it.
        chat.set_loaded(5 * GB);
        lease.end_load();
        let (g, _hold) = within(second).await.unwrap().unwrap();
        assert_eq!(
            g,
            Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            }
        );
        assert_eq!(chat.unloads(), 1);
        assert!(lease.holds_card());
        assert!(!lease.begin_load(), "a chat load passed during job 2");
        assert_eq!(waits.seen(), [LeaseWait::ChatModelLoading]);
    }

    /// Review fixes L12-1 and L12-3: a plain chat load (no model use of its own) that passes
    /// the gate while a job waits for a reply goes first. The job's unload does not stop it
    /// in flight; the job reads the card again once the load reached Starting, and unloads
    /// it then only because it does not fit beside it. The load never starts on top of the
    /// job.
    #[tokio::test]
    async fn a_chat_load_during_the_unload_is_seen_before_the_grant() {
        let chat = FakeChat::loaded(5 * GB);
        let reply = chat.use_model(BusyKind::ChatTurn);
        let lease = GpuLease::new(chat.clone());
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            need(false, true, false),
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        waits.until(LeaseWait::ReplyRunning).await;
        // The job holds nothing while it waits: a chat load passes the gate.
        assert!(lease.begin_load(), "the chat load was held");

        // The reply ends while the load is in flight: the unload stops nothing.
        drop(reply);
        chat.until_asked(1).await;
        assert_eq!(
            chat.unloads(),
            0,
            "the unload stopped a chat load in flight"
        );
        // The job now waits for that load, and says so.
        waits.until(LeaseWait::ChatModelLoading).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !job.is_finished(),
            "the job got the card while a chat load was in flight"
        );
        assert!(!lease.holds_card());

        // The load reaches Starting on the card (the same model again): the job sees it, and
        // unloads it because the job does not fit beside it.
        chat.set_loaded(5 * GB);
        lease.end_load();
        let (g, _hold) = within(job).await.unwrap().unwrap();
        assert_eq!(
            g,
            Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            }
        );
        assert_eq!(chat.unloads(), 1);
        assert!(lease.holds_card());
        assert_eq!(
            waits.seen(),
            [LeaseWait::ReplyRunning, LeaseWait::ChatModelLoading]
        );
    }

    /// Review finding mgr-stale-need-alongside: run 1 unloaded the chat model and the person
    /// pressed "Load it again" while run 2 waited. Run 2's need was judged before that load
    /// reached Starting (no chat model on the card, so it fits "beside" it). Once the load
    /// starts, the lease never grants that stale need Alongside: it asks for the job to be
    /// judged again, and the job judged with the chat model gets the card after an unload.
    #[tokio::test]
    async fn a_job_judged_before_a_chat_load_started_is_judged_again() {
        let chat = FakeChat::none();
        let lease = GpuLease::new(chat.clone());
        assert!(lease.begin_load(), "Load it again passes the gate");
        let judged_without_chat = StudioNeed {
            judged_with: None,
            ..need(true, true, false)
        };
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            judged_without_chat,
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        waits.until(LeaseWait::ChatModelLoading).await;
        chat.set_loaded(5 * GB);
        lease.end_load();
        assert_eq!(within(job).await.unwrap().unwrap_err(), Refused::JudgeAgain);
        assert!(!lease.holds_card());
        assert_eq!(chat.unloads(), 0);

        // Judged again with the chat model: it does not fit beside it.
        let (g, _hold) = within(lease.acquire_for_studio(
            &need(false, true, false),
            GpuPolicy::Unload,
            None,
            &tok(),
            &|_| {},
        ))
        .await
        .unwrap();
        assert_eq!(
            g,
            Grant::AfterUnload {
                unloaded: Some(CHAT.to_string())
            }
        );
        // Another chat model in its place (6 GB) is not the one judged with either.
        drop(_hold);
        chat.set_loaded(6 * GB);
        let refused = within(lease.acquire_for_studio(
            &need(true, true, false),
            GpuPolicy::Unload,
            None,
            &tok(),
            &|_| {},
        ))
        .await
        .unwrap_err();
        assert_eq!(refused, Refused::JudgeAgain);
    }

    /// Review fix L12-3: a chat load that passed the gate and started while a job waited for
    /// a reply went first. The job's unload, decided on the card before it, does not stop
    /// it; the job decides again on the card as it is now. This load started on the
    /// processor, so the card is free and the job runs beside it.
    #[tokio::test]
    async fn a_chat_load_that_went_first_is_not_unloaded() {
        let chat = FakeChat::loaded(5 * GB);
        let reply = chat.use_model(BusyKind::ChatTurn);
        let lease = GpuLease::new(chat.clone());
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            need(false, true, false),
            GpuPolicy::Unload,
            &tok(),
            &waits,
        );
        waits.until(LeaseWait::ReplyRunning).await;
        // The person loads a model on the processor while the reply is written.
        assert!(lease.begin_load(), "the chat load was held");
        chat.set_on_processor();
        lease.end_load();

        // The reply ends: the job's unload stops nothing, and the job runs beside it.
        drop(reply);
        let (g, _hold) = within(job).await.unwrap().unwrap();
        assert_eq!(g, Grant::Alongside);
        assert_eq!(
            chat.unloads(),
            0,
            "the unload stopped a chat load that went first"
        );
        assert!(lease.holds_card());
    }

    /// Review fix L12-3: the app's unload runs `before_change` first (it may wait for a
    /// model use, and no chat load may wait behind that), then waits until no chat load is
    /// replacing the server and keeps the next one from starting (`between_loads`), asks
    /// `go_ahead` there, and only then stops the server, holding the loads off until the
    /// stop ends. A stop in the middle of a load would reset the status of a server that
    /// then starts unseen by the lease.
    #[test]
    fn the_unload_stops_the_server_only_between_loads() {
        // A checkout on Windows ends its lines in "\r\n".
        let source = include_str!("gpu_lease.rs").replace("\r\n", "\n");
        let start = source
            .find("impl ChatSide for RuntimeChat {")
            .expect("the app's chat side");
        let body = &source[start..];
        let body = &body[body
            .find("    async fn unload_when_idle(\n")
            .expect("unload_when_idle")..];
        let body = &body[..body.find("\n    }\n").expect("the end of unload_when_idle")];
        let steps = [
            "before_change(",
            "between_loads().await",
            "if !go_ahead()",
            "let _between = between;",
            "unload_by_user()",
        ];
        let mut from = 0;
        for step in steps {
            let found = body[from..]
                .find(step)
                .unwrap_or_else(|| panic!("the unload, in order: {step}"));
            from += found + step.len();
        }
    }

    /// Review fix L12-reload-test: "Reload it after Studio" loads the chat model again only
    /// while the status still names it as unloaded to free memory. A model the person
    /// loaded since (that one or another), unloaded since, or removed is left as it is.
    /// Ruling L12-reload-guard: so is one whose load the person started and that has not
    /// reached Starting yet, waiting for Studio at the gate or in flight past it.
    #[test]
    fn reload_skips_a_model_the_person_changed_since() {
        use crate::dto::{RuntimeStateDto, RuntimeStatusDto};
        const OTHER: &str = "/m/other.gguf";
        let unloaded = |path: Option<&str>| RuntimeStatusDto {
            unloaded_path: path.map(str::to_string),
            ..RuntimeStatusDto::default()
        };
        let loaded = |path: &str| RuntimeStatusDto {
            state: RuntimeStateDto::Ready,
            model_path: Some(path.to_string()),
            ..RuntimeStatusDto::default()
        };

        // Studio unloaded it, and nothing changed since.
        assert!(reload_wanted(&unloaded(Some(CHAT)), 0, CHAT));
        // The person loaded it again, or another model: a load clears "Unloaded to free
        // memory".
        assert!(!reload_wanted(&loaded(CHAT), 0, CHAT));
        assert!(!reload_wanted(&loaded(OTHER), 0, CHAT));
        // The person loaded another model and unloaded it.
        assert!(!reload_wanted(&unloaded(Some(OTHER)), 0, CHAT));
        // The model was removed from Installed (or nothing was unloaded).
        assert!(!reload_wanted(&unloaded(None), 0, CHAT));
        // The person pressed Load on another model during the job: its load still waits for
        // Studio at the gate, or passed it and has not reached Starting (which clears
        // `unloaded_path`). Their pick goes ahead.
        let waiting = RuntimeStatusDto {
            waiting_for_studio: true,
            ..unloaded(Some(CHAT))
        };
        assert!(!reload_wanted(&waiting, 0, CHAT));
        assert!(!reload_wanted(&unloaded(Some(CHAT)), 1, CHAT));
    }

    #[tokio::test]
    async fn reload_chat_loads_the_chat_model_once_and_a_hold_never_does() {
        let chat = FakeChat::loaded(5 * GB);
        let lease = GpuLease::new(chat.clone());
        let (g, hold) = lease
            .acquire_for_studio(
                &need(false, true, false),
                GpuPolicy::Unload,
                None,
                &tok(),
                &|_| {},
            )
            .await
            .unwrap();
        assert!(matches!(g, Grant::AfterUnload { .. }));
        drop(hold);
        assert!(chat.reloads().is_empty(), "the hold loaded the chat model");

        lease.reload_chat(CHAT).await;
        assert_eq!(chat.reloads(), [CHAT]);
        assert_eq!(chat.reload_loads(), [0]);

        // Ruling L12-reload-guard: a chat load in flight (past the gate, not started) goes
        // with the reload, which then leaves it alone.
        assert!(lease.begin_load());
        lease.reload_chat(CHAT).await;
        assert_eq!(chat.reload_loads(), [0, 1]);
        lease.end_load();
    }

    #[tokio::test]
    async fn cancel_ends_any_wait() {
        // A reply that never ends.
        let chat = FakeChat::loaded(5 * GB);
        let _reply = chat.use_model(BusyKind::ChatTurn);
        let lease = GpuLease::new(chat.clone());
        let cancel = tok();
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            need(false, true, false),
            GpuPolicy::Unload,
            &cancel,
            &waits,
        );
        waits.until(LeaseWait::ReplyRunning).await;
        cancel.cancel();
        assert_eq!(within(job).await.unwrap().unwrap_err(), Refused::Cancelled);
        assert_eq!(chat.unloads(), 0);
        assert!(!lease.holds_card());

        // The wait for another job's place ends too (and the cancelled job gave it back).
        let (_, first) = within(lease.acquire_for_studio(
            &need(true, true, true),
            GpuPolicy::Unload,
            None,
            &tok(),
            &|_| {},
        ))
        .await
        .unwrap();
        let cancel = tok();
        let waits = Waits::default();
        let second = ask_in_task(
            &lease,
            need(true, true, true),
            GpuPolicy::Unload,
            &cancel,
            &waits,
        );
        waits.until(LeaseWait::AnotherStudioJob).await;
        cancel.cancel();
        assert_eq!(
            within(second).await.unwrap().unwrap_err(),
            Refused::Cancelled
        );
        drop(first);

        // The wait for a chat load in flight ends too.
        assert!(lease.begin_load());
        let cancel = tok();
        let waits = Waits::default();
        let job = ask_in_task(
            &lease,
            need(true, true, true),
            GpuPolicy::Unload,
            &cancel,
            &waits,
        );
        waits.until(LeaseWait::ChatModelLoading).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !job.is_finished(),
            "the job got the card during a chat load"
        );
        cancel.cancel();
        assert_eq!(within(job).await.unwrap().unwrap_err(), Refused::Cancelled);
        lease.end_load();
        assert!(!lease.holds_card());

        // A job stopped before it asks gets nothing.
        let cancel = tok();
        cancel.cancel();
        let refused = lease
            .acquire_for_studio(
                &need(true, true, true),
                GpuPolicy::Unload,
                None,
                &cancel,
                &|_| {},
            )
            .await
            .unwrap_err();
        assert_eq!(refused, Refused::Cancelled);
        assert!(!lease.holds_card());
    }
}
