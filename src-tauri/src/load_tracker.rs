//! Which model load is the current one.
//!
//! Loading a model can take minutes, and the user can pick again meanwhile. Two things
//! went wrong before this existed, both reported from a real PC:
//!
//! - Picking the same model again restarted the load, and the first attempt reported
//!   "Loading was cancelled" although the model went on to load.
//! - Picking another model made the replaced load report the same error.
//!
//! A load that is replaced by a newer one must end quietly, and a second request for the
//! load already in progress must wait for it instead of restarting it. "The same load"
//! compares the resolved choices (0.0.5 D2b): Automatic is Automatic, whatever number it
//! produced.
//!
//! [`ChangeGate`] (0.0.5 D3) holds the one change of what is loaded that waits for the
//! model to be free ("Restart when it ends"), and the users of the model that "Stop and
//! restart" stops first.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::load_settings::ResolvedLoad;
use crate::state::BusyCounter;

struct InProgress {
    seq: u64,
    model: PathBuf,
    resolved: ResolvedLoad,
}

#[derive(Default)]
struct State {
    /// The number of the newest load that was started.
    newest: u64,
    in_progress: Option<InProgress>,
}

pub struct LoadTracker {
    state: Mutex<State>,
    /// The highest load number that has finished (ready, failed, replaced or cancelled).
    finished: watch::Sender<u64>,
}

impl Default for LoadTracker {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::default()),
            finished: watch::Sender::new(0),
        }
    }
}

impl LoadTracker {
    /// The number of the load in progress, if it is for this model with the same choices.
    pub fn same_in_progress(&self, model: &Path, resolved: &ResolvedLoad) -> Option<u64> {
        let state = self.state.lock().ok()?;
        state
            .in_progress
            .as_ref()
            .filter(|p| p.model == model && p.resolved.same_launch(resolved))
            .map(|p| p.seq)
    }

    /// Starts a load. It is the current one until another starts.
    pub fn begin(&self, model: &Path, resolved: &ResolvedLoad) -> u64 {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        state.newest += 1;
        let seq = state.newest;
        state.in_progress = Some(InProgress {
            seq,
            model: model.to_path_buf(),
            resolved: resolved.clone(),
        });
        seq
    }

    /// Whether no newer load has started since load `seq`.
    pub fn is_current(&self, seq: u64) -> bool {
        self.state.lock().is_ok_and(|s| s.newest == seq)
    }

    /// Marks load `seq` as over, however it ended, and wakes whoever waits for it.
    pub fn finish(&self, seq: u64) {
        if let Ok(mut state) = self.state.lock()
            && state.in_progress.as_ref().is_some_and(|p| p.seq == seq)
        {
            state.in_progress = None;
        }
        self.finished.send_if_modified(|done| {
            let newer = seq > *done;
            if newer {
                *done = seq;
            }
            newer
        });
    }

    /// Waits until load `seq` is over.
    pub async fn finished(&self, seq: u64) {
        let mut rx = self.finished.subscribe();
        // The sender lives as long as `self`, so this cannot fail.
        let _ = rx.wait_for(|done| *done >= seq).await;
    }
}

/// Marks a load as over when the function that runs it returns, on every path.
pub struct LoadTicket<'a> {
    tracker: &'a LoadTracker,
    pub seq: u64,
}

impl<'a> LoadTicket<'a> {
    pub fn begin(tracker: &'a LoadTracker, model: &Path, resolved: &ResolvedLoad) -> Self {
        Self {
            tracker,
            seq: tracker.begin(model, resolved),
        }
    }

    pub fn is_current(&self) -> bool {
        self.tracker.is_current(self.seq)
    }
}

impl Drop for LoadTicket<'_> {
    fn drop(&mut self) {
        self.tracker.finish(self.seq);
    }
}

/// A waiting change was cancelled by a newer one (or by `cancel_pending_reload`): it
/// changes nothing and ends with `load_replaced`.
#[derive(Debug, PartialEq, Eq)]
pub struct Replaced;

/// The change of what is loaded that waits for the model to be free, and the users of the
/// model that can be stopped (0.0.5 D3).
#[derive(Default)]
pub struct ChangeGate {
    waiting: Mutex<Option<(u64, CancellationToken)>>,
    users: Mutex<Vec<(u64, CancellationToken)>>,
    next: AtomicU64,
}

impl ChangeGate {
    /// Cancels the change that waits, if one does. True when one was waiting.
    pub fn cancel_waiting(&self) -> bool {
        let taken = self.waiting.lock().ok().and_then(|mut w| w.take());
        match taken {
            Some((_, token)) => {
                token.cancel();
                true
            }
            None => false,
        }
    }

    /// Clears the gate before a load, unload or reload (D3). A change that waits is
    /// cancelled first: a newer one always wins. Then:
    /// - `when_idle` while something uses the model: waits until nothing does, calling
    ///   `on_wait(true)` when the wait starts and `on_wait(false)` when it ends; `Ok(true)`.
    ///   A newer change, or [`Self::cancel_waiting`], ends the wait with [`Replaced`] (and
    ///   sets the waiting flag itself).
    /// - otherwise: stops the users that can be stopped (chat turns, model checks) and
    ///   waits up to `stop_wait` for them to wind down, so a reply keeps what it wrote;
    ///   requests from other apps end when the server stops. `Ok(false)`.
    pub async fn before_change(
        &self,
        busy: &BusyCounter,
        when_idle: bool,
        stop_wait: Duration,
        on_wait: impl Fn(bool),
    ) -> Result<bool, Replaced> {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        if when_idle && busy.is_busy() {
            let token = CancellationToken::new();
            let older = self
                .waiting
                .lock()
                .ok()
                .and_then(|mut w| w.replace((id, token.clone())));
            if let Some((_, older)) = older {
                older.cancel();
            }
            on_wait(true);
            let idle = tokio::select! {
                _ = busy.wait_idle() => true,
                _ = token.cancelled() => false,
            };
            if !idle {
                self.waiting_flag_after_replaced(&on_wait);
                return Err(Replaced);
            }
            let still_mine = self.waiting.lock().is_ok_and(|mut w| {
                let mine = w.as_ref().is_some_and(|(i, _)| *i == id);
                if mine {
                    *w = None;
                }
                mine
            });
            if !still_mine {
                // Replaced just as the model became free: the newer change goes ahead.
                self.waiting_flag_after_replaced(&on_wait);
                return Err(Replaced);
            }
            on_wait(false);
            return Ok(true);
        }
        if self.cancel_waiting() {
            on_wait(false);
        }
        let users: Vec<CancellationToken> = self
            .users
            .lock()
            .map(|u| u.iter().map(|(_, t)| t.clone()).collect())
            .unwrap_or_default();
        if users.is_empty() {
            return Ok(false);
        }
        for token in users {
            token.cancel();
        }
        let mut counts = busy.subscribe();
        let _ = tokio::time::timeout(
            stop_wait,
            counts.wait_for(|b| b.chat_turns == 0 && b.model_checks == 0),
        )
        .await;
        Ok(false)
    }

    /// After a wait was replaced: the waiting flag goes off unless a newer change waits now.
    /// (A Cancel that lands between the start of a wait and `on_wait(true)` would otherwise
    /// leave "Restarts when the reply ends" on with nothing waiting.)
    fn waiting_flag_after_replaced(&self, on_wait: &impl Fn(bool)) {
        if self.waiting.lock().is_ok_and(|w| w.is_none()) {
            on_wait(false);
        }
    }

    /// Registers a user of the model that "Stop and restart" stops with `stop`.
    pub fn add_user(&self, stop: CancellationToken) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        if let Ok(mut users) = self.users.lock() {
            users.push((id, stop));
        }
        id
    }

    pub fn remove_user(&self, id: u64) {
        if let Ok(mut users) = self.users.lock() {
            users.retain(|(i, _)| *i != id);
        }
    }

    #[cfg(test)]
    fn user_count(&self) -> usize {
        self.users.lock().map(|u| u.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_settings::{ContextSize, ContextSource, GpuUse, ThinkingLimit};
    use crate::state::BusyKind;
    use std::sync::Arc;

    fn settings(ctx: u32) -> ResolvedLoad {
        ResolvedLoad {
            ctx,
            ctx_choice: ContextSize::Fixed(ctx),
            ctx_source: ContextSource::DefaultSetting,
            gpu: GpuUse::All,
            save_memory: false,
            keep_in_memory: false,
            thinking_limit: ThinkingLimit::NoLimit,
            backend: None,
            device: None,
            mmproj: None,
            fine: Default::default(),
        }
    }

    fn automatic(ctx: u32) -> ResolvedLoad {
        ResolvedLoad {
            ctx_choice: ContextSize::Auto,
            ctx_source: ContextSource::Automatic,
            ..settings(ctx)
        }
    }

    /// D2b: a load with Automatic joins a load with Automatic in progress even when the
    /// memory free now makes Automatic produce another number; a fixed size does not join
    /// an Automatic load of the same number, and Save memory is another load.
    #[test]
    fn joining_compares_the_choices_not_the_automatic_number() {
        let t = LoadTracker::default();
        let a = Path::new("/models/a.gguf");
        let seq = t.begin(a, &automatic(24_576));
        assert_eq!(t.same_in_progress(a, &automatic(20_480)), Some(seq));
        assert_eq!(t.same_in_progress(a, &settings(24_576)), None);
        let saving = ResolvedLoad {
            save_memory: true,
            ..automatic(24_576)
        };
        assert_eq!(t.same_in_progress(a, &saving), None);
        let thinking = ResolvedLoad {
            thinking_limit: ThinkingLimit::Long,
            ..automatic(24_576)
        };
        assert_eq!(t.same_in_progress(a, &thinking), Some(seq));
    }

    const STOP_WAIT: Duration = Duration::from_secs(5);

    /// "Restart when it ends": the change waits while a reply is written and goes ahead
    /// when it ends, with the waiting flag on meanwhile.
    #[tokio::test]
    async fn a_waiting_change_goes_ahead_when_the_model_is_free() {
        let gate = Arc::new(ChangeGate::default());
        let busy = Arc::new(BusyCounter::default());
        let turn = busy.hold(BusyKind::ChatTurn);
        let flags = Arc::new(Mutex::new(Vec::new()));
        let waiter = {
            let (gate, busy, flags) = (gate.clone(), busy.clone(), flags.clone());
            tokio::spawn(async move {
                gate.before_change(&busy, true, STOP_WAIT, |w| flags.lock().unwrap().push(w))
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiter.is_finished(),
            "went ahead while a reply was written"
        );
        assert_eq!(*flags.lock().unwrap(), [true]);
        drop(turn);
        let outcome = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("went ahead once free")
            .unwrap();
        assert_eq!(outcome, Ok(true));
        assert_eq!(*flags.lock().unwrap(), [true, false]);
        // Nothing busy: at once, without waiting.
        assert_eq!(
            gate.before_change(&busy, true, STOP_WAIT, |_| {}).await,
            Ok(false)
        );
    }

    /// "Restarts when the reply ends" stays on while a newer change waits, and goes off
    /// when a Cancel leaves nothing waiting, whatever order the two land in.
    #[tokio::test]
    async fn the_waiting_flag_ends_off_only_when_nothing_waits() {
        let gate = Arc::new(ChangeGate::default());
        let busy = Arc::new(BusyCounter::default());
        let _app = busy.hold(BusyKind::OtherApp);
        let flags = Arc::new(Mutex::new(Vec::new()));
        let spawn_wait = || {
            let (gate, busy, flags) = (gate.clone(), busy.clone(), flags.clone());
            tokio::spawn(async move {
                gate.before_change(&busy, true, STOP_WAIT, |w| flags.lock().unwrap().push(w))
                    .await
            })
        };
        let first = spawn_wait();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let second = spawn_wait();
        assert_eq!(first.await.unwrap(), Err(Replaced));
        assert_eq!(
            flags.lock().unwrap().last(),
            Some(&true),
            "the newer one waits"
        );
        assert!(gate.cancel_waiting());
        assert_eq!(second.await.unwrap(), Err(Replaced));
        assert_eq!(
            flags.lock().unwrap().last(),
            Some(&false),
            "nothing waits now"
        );
    }

    /// A newer change, waiting or not, cancels the one that waits; so does Cancel.
    #[tokio::test]
    async fn a_newer_change_or_cancel_ends_the_wait_with_replaced() {
        let gate = Arc::new(ChangeGate::default());
        let busy = Arc::new(BusyCounter::default());
        let _turn = busy.hold(BusyKind::OtherApp);
        let spawn_wait = || {
            let (gate, busy) = (gate.clone(), busy.clone());
            tokio::spawn(async move { gate.before_change(&busy, true, STOP_WAIT, |_| {}).await })
        };
        let first = spawn_wait();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let second = spawn_wait();
        let first = tokio::time::timeout(Duration::from_secs(2), first)
            .await
            .expect("the older wait ended")
            .unwrap();
        assert_eq!(first, Err(Replaced));
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!second.is_finished(), "the newer one still waits");
        // Cancel ("Restarts when the reply ends · Cancel").
        assert!(gate.cancel_waiting());
        let second = tokio::time::timeout(Duration::from_secs(2), second)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(second, Err(Replaced));
        assert!(!gate.cancel_waiting(), "nothing waits now; not an error");
        // A change that does not wait cancels a waiting one and goes ahead itself.
        let third = spawn_wait();
        tokio::time::sleep(Duration::from_millis(30)).await;
        let flags = Mutex::new(Vec::new());
        assert_eq!(
            gate.before_change(&busy, false, STOP_WAIT, |w| flags.lock().unwrap().push(w))
                .await,
            Ok(false)
        );
        assert_eq!(*flags.lock().unwrap(), [false], "the waiting flag goes off");
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), third)
                .await
                .unwrap()
                .unwrap(),
            Err(Replaced)
        );
    }

    /// "Stop and restart": the reply and the model check are stopped, and the change waits
    /// until they have wound down (not for another app's request).
    #[tokio::test]
    async fn stop_and_restart_stops_the_users_first() {
        let gate = Arc::new(ChangeGate::default());
        let busy = Arc::new(BusyCounter::default());
        let other_app = busy.hold(BusyKind::OtherApp);
        let mut users = Vec::new();
        for kind in [BusyKind::ChatTurn, BusyKind::ModelCheck] {
            let stop = CancellationToken::new();
            let id = gate.add_user(stop.clone());
            let guard = busy.hold(kind);
            let gate = gate.clone();
            // The user winds down a little after it is told to stop.
            users.push(tokio::spawn(async move {
                stop.cancelled().await;
                tokio::time::sleep(Duration::from_millis(80)).await;
                gate.remove_user(id);
                drop(guard);
            }));
        }
        assert_eq!(gate.user_count(), 2);
        let started = std::time::Instant::now();
        assert_eq!(
            gate.before_change(&busy, false, STOP_WAIT, |_| {}).await,
            Ok(false)
        );
        let took = started.elapsed();
        assert!(took >= Duration::from_millis(70), "did not wait: {took:?}");
        assert!(
            took < Duration::from_secs(2),
            "waited for the other app: {took:?}"
        );
        let counts = busy.snapshot();
        assert_eq!(
            (counts.chat_turns, counts.model_checks, counts.other_apps),
            (0, 0, 1)
        );
        assert_eq!(gate.user_count(), 0);
        drop(other_app);
        // A user that never winds down holds the change up for `stop_wait` at most.
        let stuck = busy.hold(BusyKind::ChatTurn);
        let id = gate.add_user(CancellationToken::new());
        let started = std::time::Instant::now();
        assert_eq!(
            gate.before_change(&busy, false, Duration::from_millis(100), |_| {})
                .await,
            Ok(false)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        gate.remove_user(id);
        drop(stuck);
    }

    #[test]
    fn the_same_model_picked_again_is_recognised_while_it_loads() {
        let t = LoadTracker::default();
        let a = Path::new("/models/a.gguf");
        assert_eq!(t.same_in_progress(a, &settings(4096)), None);
        let seq = t.begin(a, &settings(4096));
        assert_eq!(t.same_in_progress(a, &settings(4096)), Some(seq));
        // Another file, or the same file with other settings, is a different load.
        assert_eq!(
            t.same_in_progress(Path::new("/models/b.gguf"), &settings(4096)),
            None
        );
        assert_eq!(t.same_in_progress(a, &settings(8192)), None);
        t.finish(seq);
        assert_eq!(t.same_in_progress(a, &settings(4096)), None);
    }

    #[test]
    fn a_newer_load_replaces_the_current_one() {
        let t = LoadTracker::default();
        let first = t.begin(Path::new("/models/a.gguf"), &settings(4096));
        assert!(t.is_current(first));
        let second = t.begin(Path::new("/models/b.gguf"), &settings(4096));
        assert!(!t.is_current(first), "the first load was replaced");
        assert!(t.is_current(second));
        // The replaced load ending must not clear the newer one.
        t.finish(first);
        assert_eq!(
            t.same_in_progress(Path::new("/models/b.gguf"), &settings(4096)),
            Some(second)
        );
        assert!(t.is_current(second));
    }

    #[test]
    fn a_ticket_finishes_its_load_on_every_way_out() {
        let t = LoadTracker::default();
        let a = Path::new("/models/a.gguf");
        let seq = {
            let ticket = LoadTicket::begin(&t, a, &settings(4096));
            assert!(ticket.is_current());
            assert_eq!(t.same_in_progress(a, &settings(4096)), Some(ticket.seq));
            ticket.seq
        };
        assert_eq!(t.same_in_progress(a, &settings(4096)), None);
        assert_eq!(*t.finished.borrow(), seq);
    }

    #[tokio::test]
    async fn waiting_for_a_load_ends_when_it_finishes() {
        let t = Arc::new(LoadTracker::default());
        let seq = t.begin(Path::new("/models/a.gguf"), &settings(4096));
        let waiter = {
            let t = Arc::clone(&t);
            tokio::spawn(async move { t.finished(seq).await })
        };
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!waiter.is_finished(), "the load is still running");
        t.finish(seq);
        tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("the waiter was woken")
            .expect("no panic");
        // A load that is already over does not block.
        tokio::time::timeout(Duration::from_secs(2), t.finished(seq))
            .await
            .expect("returns at once");
    }
}
