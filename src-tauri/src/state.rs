//! App-wide state handed to commands.

use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

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
