//! When the task note goes with a turn.
//!
//! The note is added to the turn's user message and not stored, so the next request shows
//! that message without it. A local server can then reuse its cached prompt only up to
//! that message and has to read the whole previous turn again. So the note is sent only
//! when the model's picture of the task may be out of date:
//!
//! 1. the first real turn of the chat since the app started (the cache is cold anyway);
//! 2. the user changed, paused, resumed or finished the task since the model last saw it;
//! 3. the chat's summary of earlier messages changed since the last note (the cache is
//!    invalid anyway, and the model's own `update_task` results may have left the context);
//! 4. a refresher: [`REFRESH_TURNS`] real turns have passed since the last note (old tool
//!    results are trimmed from requests without the app knowing).
//!
//! Otherwise the model has its own `update_task` calls and their results in the
//! conversation. The decision is a pure function ([`reason`]); the memory per chat lives
//! in [`TurnMemory`], in memory only, so a restart starts every chat cold.

use std::collections::BTreeMap;
use std::sync::Mutex;

use store::{Task, TaskStatus};

use super::{edits, note};

/// Real turns after which the note is sent again, whatever else happened.
pub const REFRESH_TURNS: u32 = 8;

/// What the app remembers about a chat's task notes since it started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Seen {
    /// The summary of earlier messages (its row id) when the last note was sent, or when
    /// the chat's first turn since the start ran without one.
    pub summary: Option<i64>,
    /// Real turns started since the turn that carried the last note, that one included.
    pub turns: u32,
}

/// Why the next real turn carries the note.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    UserChanged,
    FirstSinceStart,
    SummaryChanged,
    Refresher,
}

/// Why the next real turn carries the note, or `None` when it does not. `seen` is `None`
/// for a chat with no real turn since the app started; `summary` is the chat's summary of
/// earlier messages now.
pub fn reason(task: &Task, seen: Option<Seen>, summary: Option<i64>) -> Option<Reason> {
    let user = &task.state.user;
    if !user.changed.is_empty() || user.paused || user.done {
        return Some(Reason::UserChanged);
    }
    let Some(seen) = seen else {
        return Some(Reason::FirstSinceStart);
    };
    if seen.summary != summary {
        return Some(Reason::SummaryChanged);
    }
    if seen.turns >= REFRESH_TURNS {
        return Some(Reason::Refresher);
    }
    None
}

/// The note the next real turn carries for `task`: the rendered note when one is due.
pub fn note_if_due(task: &Task, seen: Option<Seen>, summary: Option<i64>) -> Option<String> {
    reason(task, seen, summary)?;
    // TODO(0.0.5): store the note with its message so it never has to be taken out of a later request
    note::for_turn(task)
}

/// What the app remembers after a real turn started: a note resets the count, a turn
/// without one adds to it.
pub fn after_turn(seen: Option<Seen>, sent: bool, summary: Option<i64>) -> Seen {
    match seen {
        Some(seen) if !sent => Seen {
            summary: seen.summary,
            turns: seen.turns.saturating_add(1),
        },
        _ => Seen { summary, turns: 1 },
    }
}

/// For the Context inspector: one plain sentence for the user saying whether the task
/// state goes with the next message, and why.
pub fn status_sentence(task: &Task, seen: Option<Seen>, summary: Option<i64>) -> String {
    if task.status == TaskStatus::Done && !task.state.user.done {
        return "Not sent with the next message: the task is done.".into();
    }
    if !edits::has_content(&task.state) {
        return "Not sent with the next message: the task has no goal, steps or next action yet, only the files and checks the app noted.".into();
    }
    match reason(task, seen, summary) {
        Some(Reason::UserChanged) => "Sent with the next message: you changed, paused, resumed or finished the task since the model last saw it.".into(),
        Some(Reason::FirstSinceStart) => "Sent with the next message: it is the first message in this chat since the app started.".into(),
        Some(Reason::SummaryChanged) => "Sent with the next message: earlier messages were summarised since the model last saw the task state.".into(),
        Some(Reason::Refresher) => format!("Sent with the next message: it is sent again every {REFRESH_TURNS} messages, and {REFRESH_TURNS} have passed."),
        None => format!("Not sent with the next message: the model already has the task state from this conversation. It is sent again after you change the task, after a restart, when earlier messages are summarised, and every {REFRESH_TURNS} messages."),
    }
}

/// The memory per chat. One lives for the whole app ([`MEMORY`]); tests make their own.
#[derive(Debug, Default)]
pub struct TurnMemory {
    chats: Mutex<BTreeMap<i64, Seen>>,
}

impl TurnMemory {
    pub const fn new() -> Self {
        Self {
            chats: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn get(&self, chat_id: i64) -> Option<Seen> {
        self.chats.lock().ok()?.get(&chat_id).copied()
    }

    pub fn set(&self, chat_id: i64, seen: Seen) {
        if let Ok(mut chats) = self.chats.lock() {
            chats.insert(chat_id, seen);
        }
    }

    /// The chat's task was cleared: its next turn starts cold.
    pub fn forget(&self, chat_id: i64) {
        if let Ok(mut chats) = self.chats.lock() {
            chats.remove(&chat_id);
        }
    }
}

/// The app's memory of which chats have been sent the task note.
pub static MEMORY: TurnMemory = TurnMemory::new();

#[cfg(test)]
mod tests {
    use store::{ChatId, StepStatus, TaskStep};

    use super::*;

    fn task() -> Task {
        let mut task = Task::new(ChatId(1));
        task.state.goal = "Put the switch in the header".into();
        task.state.steps = vec![TaskStep {
            text: "Add the switch".into(),
            status: StepStatus::Doing,
        }];
        task
    }

    /// Seen recently, with the summary as it is now: nothing is due.
    const QUIET: Seen = Seen {
        summary: Some(4),
        turns: 3,
    };

    #[test]
    fn nothing_new_sends_nothing() {
        assert_eq!(reason(&task(), Some(QUIET), Some(4)), None);
        assert_eq!(note_if_due(&task(), Some(QUIET), Some(4)), None);
        // Also without any summary.
        let seen = Seen {
            summary: None,
            turns: REFRESH_TURNS - 1,
        };
        assert_eq!(reason(&task(), Some(seen), None), None);
    }

    #[test]
    fn each_reason_alone_sends_the_note() {
        // 1. The first real turn since the app started.
        assert_eq!(
            reason(&task(), None, Some(4)),
            Some(Reason::FirstSinceStart)
        );
        // 2. Each mark of the user's, alone.
        for mark in 0..3 {
            let mut t = task();
            match mark {
                0 => t.state.user.changed = vec![edits::GOAL.into()],
                1 => t.state.user.paused = true,
                _ => t.state.user.done = true,
            }
            assert_eq!(
                reason(&t, Some(QUIET), Some(4)),
                Some(Reason::UserChanged),
                "{mark}"
            );
        }
        // 3. The summary changed, appeared, or went away (another branch).
        for now in [Some(5), None] {
            assert_eq!(
                reason(&task(), Some(QUIET), now),
                Some(Reason::SummaryChanged)
            );
        }
        let unsummarised = Seen {
            summary: None,
            turns: 1,
        };
        assert_eq!(
            reason(&task(), Some(unsummarised), Some(1)),
            Some(Reason::SummaryChanged)
        );
        // 4. The refresher.
        let old = Seen {
            summary: Some(4),
            turns: REFRESH_TURNS,
        };
        assert_eq!(reason(&task(), Some(old), Some(4)), Some(Reason::Refresher));
        // A due note is the rendered note.
        assert_eq!(note_if_due(&task(), None, None), note::for_turn(&task()));
        assert!(note_if_due(&task(), None, None).is_some());
    }

    #[test]
    fn a_due_note_is_still_nothing_for_a_task_with_nothing_to_tell() {
        let mut facts = Task::new(ChatId(1));
        facts
            .state
            .record_file("/w/a.rs", store::FileChange::Edited, 1);
        assert_eq!(note_if_due(&facts, None, None), None);
        let mut done = task();
        done.status = TaskStatus::Done;
        assert_eq!(note_if_due(&done, None, None), None);
    }

    #[test]
    fn the_count_runs_from_the_last_note() {
        // A note: the count starts again, with the summary of now.
        assert_eq!(
            after_turn(Some(QUIET), true, Some(9)),
            Seen {
                summary: Some(9),
                turns: 1
            }
        );
        // No note: one more turn, and the summary of the last note is kept.
        assert_eq!(
            after_turn(Some(QUIET), false, Some(9)),
            Seen {
                summary: Some(4),
                turns: 4
            }
        );
        // The first turn since the start without a note (no task yet).
        assert_eq!(
            after_turn(None, false, Some(2)),
            Seen {
                summary: Some(2),
                turns: 1
            }
        );

        // A note, then turns without one: the eighth turn after it carries it again.
        let mut seen = after_turn(None, true, None);
        let mut sent_at = Vec::new();
        for turn in 1..=20 {
            let sent = reason(&task(), Some(seen), None).is_some();
            if sent {
                sent_at.push(turn);
            }
            seen = after_turn(Some(seen), sent, None);
        }
        assert_eq!(sent_at, [8, 16]);
    }

    #[test]
    fn the_sentence_for_the_user_says_whether_and_why() {
        let sent = |s: String| s.starts_with("Sent with the next message: ");
        assert!(sent(status_sentence(&task(), None, None)));
        assert!(status_sentence(&task(), None, None).contains("first message"));
        let mut changed = task();
        changed.state.user.paused = true;
        assert!(status_sentence(&changed, Some(QUIET), Some(4)).contains("you changed"));
        assert!(status_sentence(&task(), Some(QUIET), Some(5)).contains("summarised"));
        let old = Seen {
            summary: Some(4),
            turns: REFRESH_TURNS,
        };
        assert!(status_sentence(&task(), Some(old), Some(4)).contains("every 8 messages, and 8"));
        assert_eq!(
            status_sentence(&task(), Some(QUIET), Some(4)),
            "Not sent with the next message: the model already has the task state from this conversation. It is sent again after you change the task, after a restart, when earlier messages are summarised, and every 8 messages."
        );
        let mut done = task();
        done.status = TaskStatus::Done;
        assert_eq!(
            status_sentence(&done, None, None),
            "Not sent with the next message: the task is done."
        );
        let mut facts = Task::new(ChatId(1));
        facts
            .state
            .record_file("/w/a.rs", store::FileChange::Edited, 1);
        assert!(status_sentence(&facts, None, None).starts_with("Not sent"));
    }

    #[test]
    fn the_memory_is_per_chat_and_can_be_forgotten() {
        let memory = TurnMemory::new();
        assert_eq!(memory.get(1), None);
        memory.set(1, QUIET);
        assert_eq!((memory.get(1), memory.get(2)), (Some(QUIET), None));
        memory.forget(1);
        assert_eq!(memory.get(1), None);
    }
}
