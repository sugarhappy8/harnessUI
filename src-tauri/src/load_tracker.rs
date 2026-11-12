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
//! load already in progress must wait for it instead of restarting it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::sync::watch;

use crate::dto::LocalModelSettings;

struct InProgress {
    seq: u64,
    model: PathBuf,
    settings: LocalModelSettings,
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
    /// The number of the load in progress, if it is for this model with these settings.
    pub fn same_in_progress(&self, model: &Path, settings: &LocalModelSettings) -> Option<u64> {
        let state = self.state.lock().ok()?;
        state
            .in_progress
            .as_ref()
            .filter(|p| p.model == model && &p.settings == settings)
            .map(|p| p.seq)
    }

    /// Starts a load. It is the current one until another starts.
    pub fn begin(&self, model: &Path, settings: &LocalModelSettings) -> u64 {
        let Ok(mut state) = self.state.lock() else {
            return 0;
        };
        state.newest += 1;
        let seq = state.newest;
        state.in_progress = Some(InProgress {
            seq,
            model: model.to_path_buf(),
            settings: settings.clone(),
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
    pub fn begin(tracker: &'a LoadTracker, model: &Path, settings: &LocalModelSettings) -> Self {
        Self {
            tracker,
            seq: tracker.begin(model, settings),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    fn settings(ctx: u32) -> LocalModelSettings {
        LocalModelSettings {
            ctx,
            ..Default::default()
        }
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
