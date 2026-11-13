//! App-wide state handed to commands.

use std::sync::{Arc, Mutex};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::dto::ModelBusy;
use crate::runtime_manager::RuntimeManager;

pub struct AppState {
    pub store: store::Store,
    pub secrets: Arc<dyn store::secrets::SecretStore>,
    pub runtime: Arc<RuntimeManager>,
    /// The reply being generated (one at a time in v1).
    pub generation: Generation,
    /// Tool servers: built-in and the user's MCP servers.
    pub host: mcp_host::Host,
    /// The skills server (also registered in `host`), for the prompt's skills index.
    pub skills: Arc<tools::SkillsServer>,
    /// "Allow for this chat" grants (memory only, D12). Shared with the policy gate of a
    /// running turn, which reads them on every decision.
    pub grants: Arc<crate::turns::Grants>,
    /// 0.0.6 D3: everything that is using the local model now (chat turns, model checks
    /// and benchmarks, other apps), so a reload can ask first or wait. Each user holds a
    /// [`BusyGuard`] while it runs.
    pub model_busy: Arc<BusyCounter>,
    /// 0.0.6 item 6 (WP10d): llama.cpp's measurements for "Where the model goes", one at a
    /// time, with their cache.
    pub offload: crate::offload::Measurements,
    /// 0.0.6 item 6 (WP10d): the usage line's reader thread.
    pub usage: crate::usage_cmd::UsageReader,
}

/// What holds a [`BusyGuard`].
#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b (chat turns, model checks) and WP5a (other apps) hold the guards"
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BusyKind {
    /// A chat turn writing a reply (WP1b holds it in `turns.rs`).
    ChatTurn,
    /// A model check or benchmark (WP1b holds it in `evals/`).
    ModelCheck,
    /// A request from another app through the sharing proxy (WP5a).
    OtherApp,
}

/// The one "model in use" counter (0.0.6 D3). Counts go up with [`BusyCounter::hold`] and
/// down when the returned guard is dropped, so a count can never be left behind by an
/// early return or a panic.
pub struct BusyCounter {
    counts: watch::Sender<ModelBusy>,
}

impl Default for BusyCounter {
    fn default() -> Self {
        Self {
            counts: watch::Sender::new(ModelBusy::default()),
        }
    }
}

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b (chat turns, model checks) and WP5a (other apps) hold the guards"
)]
impl BusyCounter {
    /// Counts one more user of `kind` until the guard is dropped.
    #[must_use = "the count goes down again as soon as the guard is dropped"]
    pub fn hold(self: &Arc<Self>, kind: BusyKind) -> BusyGuard {
        self.counts.send_modify(|c| *slot(c, kind) += 1);
        BusyGuard {
            counter: Arc::clone(self),
            kind,
        }
    }

    /// The counts now.
    pub fn snapshot(&self) -> ModelBusy {
        *self.counts.borrow()
    }

    /// Something is using the model.
    pub fn is_busy(&self) -> bool {
        !self.snapshot().is_idle()
    }

    /// Returns once nothing is using the model (at once when nothing is). Cancel it by
    /// dropping the future, for example in a `tokio::select!` with a cancellation token.
    pub async fn wait_idle(&self) {
        let mut rx = self.counts.subscribe();
        let _ = rx.wait_for(ModelBusy::is_idle).await;
    }

    /// A receiver that sees every change of the counts.
    pub fn subscribe(&self) -> watch::Receiver<ModelBusy> {
        self.counts.subscribe()
    }
}

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b (chat turns, model checks) and WP5a (other apps) hold the guards"
)]
fn slot(c: &mut ModelBusy, kind: BusyKind) -> &mut u32 {
    match kind {
        BusyKind::ChatTurn => &mut c.chat_turns,
        BusyKind::ModelCheck => &mut c.model_checks,
        BusyKind::OtherApp => &mut c.other_apps,
    }
}

/// One user of the model, counted while this guard lives.
#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b (chat turns, model checks) and WP5a (other apps) hold the guards"
)]
#[must_use = "the count goes down again as soon as the guard is dropped"]
pub struct BusyGuard {
    counter: Arc<BusyCounter>,
    kind: BusyKind,
}

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b (chat turns, model checks) and WP5a (other apps) hold the guards"
)]
impl BusyGuard {
    /// What this guard counts.
    pub fn kind(&self) -> BusyKind {
        self.kind
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        let kind = self.kind;
        self.counter.counts.send_modify(|c| {
            let n = slot(c, kind);
            *n = n.saturating_sub(1);
        });
    }
}

/// The one reply being generated, if any: which chat it belongs to and how to stop it.
#[derive(Default)]
pub struct Generation {
    running: Mutex<Option<Running>>,
    next_id: std::sync::atomic::AtomicU64,
}

struct Running {
    id: u64,
    chat: i64,
    cancel: CancellationToken,
}

impl Generation {
    /// Starts a generation in `chat`; `None` while another one runs. A stopped one that is
    /// still winding down does not block. The id is for [`Generation::finish`].
    pub fn start(&self, chat: i64) -> Option<(u64, CancellationToken)> {
        let mut guard = self.running.lock().ok()?;
        if guard.as_ref().is_some_and(|r| !r.cancel.is_cancelled()) {
            return None;
        }
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let cancel = CancellationToken::new();
        *guard = Some(Running {
            id,
            chat,
            cancel: cancel.clone(),
        });
        Some((id, cancel))
    }

    /// Ends generation `id`. A newer one, started while this one was winding down, stays.
    pub fn finish(&self, id: u64) {
        if let Ok(mut guard) = self.running.lock()
            && guard.as_ref().is_some_and(|r| r.id == id)
        {
            *guard = None;
        }
    }

    /// Stops whatever is being generated.
    pub fn cancel(&self) {
        if let Ok(guard) = self.running.lock()
            && let Some(r) = guard.as_ref()
        {
            r.cancel.cancel();
        }
    }

    /// Stops the generation if it runs in `chat`. True when there was one.
    pub fn cancel_chat(&self, chat: i64) -> bool {
        match self.running.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(r) if r.chat == chat => {
                    r.cancel.cancel();
                    true
                }
                _ => false,
            },
            Err(_) => false,
        }
    }

    /// True until the generation in `chat` has finished (also while it winds down).
    pub fn running_in(&self, chat: i64) -> bool {
        self.running
            .lock()
            .is_ok_and(|g| g.as_ref().is_some_and(|r| r.chat == chat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_guards_count_while_they_live() {
        let busy = Arc::new(BusyCounter::default());
        assert!(!busy.is_busy());
        let turn = busy.hold(BusyKind::ChatTurn);
        let check = busy.hold(BusyKind::ModelCheck);
        let app1 = busy.hold(BusyKind::OtherApp);
        let app2 = busy.hold(BusyKind::OtherApp);
        assert_eq!(turn.kind(), BusyKind::ChatTurn);
        assert_eq!(
            busy.snapshot(),
            ModelBusy {
                chat_turns: 1,
                model_checks: 1,
                other_apps: 2
            }
        );
        assert!(busy.is_busy());
        drop(app1);
        drop(turn);
        assert_eq!(
            busy.snapshot(),
            ModelBusy {
                chat_turns: 0,
                model_checks: 1,
                other_apps: 1
            }
        );
        drop((check, app2));
        assert!(!busy.is_busy());
        assert_eq!(busy.snapshot(), ModelBusy::default());
    }

    /// A guard dropped by a panic still counts down.
    #[test]
    fn a_panic_does_not_leave_a_count_behind() {
        let busy = Arc::new(BusyCounter::default());
        let held = Arc::clone(&busy);
        let result = std::thread::spawn(move || {
            let _guard = held.hold(BusyKind::ChatTurn);
            panic!("the turn failed");
        })
        .join();
        assert!(result.is_err());
        assert!(!busy.is_busy());
    }

    #[tokio::test]
    async fn wait_idle_returns_when_the_last_guard_goes() {
        let busy = Arc::new(BusyCounter::default());
        // Nothing running: at once.
        tokio::time::timeout(std::time::Duration::from_secs(1), busy.wait_idle())
            .await
            .expect("idle at once");
        let a = busy.hold(BusyKind::ChatTurn);
        let b = busy.hold(BusyKind::OtherApp);
        let waiter = {
            let busy = Arc::clone(&busy);
            tokio::spawn(async move { busy.wait_idle().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        drop(a);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(
            !waiter.is_finished(),
            "returned while another app was still using it"
        );
        drop(b);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("returned after the last guard")
            .expect("task");
    }

    #[test]
    fn one_generation_at_a_time_and_it_knows_its_chat() {
        let g = Generation::default();
        let (id, token) = g.start(7).expect("nothing is running");
        assert!(g.start(8).is_none(), "a second generation started");
        assert!(g.running_in(7) && !g.running_in(8));

        // Deleting another chat leaves it alone; deleting its chat stops it.
        assert!(!g.cancel_chat(8));
        assert!(!token.is_cancelled());
        assert!(g.cancel_chat(7));
        assert!(token.is_cancelled());
        // Still winding down: it counts as running in its chat until it finishes.
        assert!(g.running_in(7));
        g.finish(id);
        assert!(!g.running_in(7));
    }

    /// A stopped generation that is still winding down does not block the next one, and
    /// when it ends it must not clear the newer one (Stop would no longer reach it).
    #[test]
    fn a_finishing_generation_does_not_clear_a_newer_one() {
        let g = Generation::default();
        let (old, old_token) = g.start(1).unwrap();
        g.cancel();
        assert!(old_token.is_cancelled());
        let (new, new_token) = g.start(2).expect("a stopped generation does not block");
        g.finish(old);
        assert!(
            g.running_in(2),
            "the old generation's end cleared the new one"
        );
        g.cancel();
        assert!(
            new_token.is_cancelled(),
            "Stop no longer reaches the new generation"
        );
        g.finish(new);
        assert!(!g.running_in(2));
        // Stop with nothing running is harmless.
        g.cancel();
    }
}
