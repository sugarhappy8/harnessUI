//! The main window steps aside while computer use acts (ARCHITECTURE.md 9.3, "Stepping
//! aside").
//!
//! Screenshots leave this app's windows out, so the model sees the applications behind
//! them, and input never goes to this app (the guard refuses it). So before an approved
//! input action runs, the app minimises its main window when it is on screen, and it brings
//! the window back when the user is needed: an approval card of a chat that minimised it,
//! the end of that chat's turn, and Stop. Only a window the app minimised itself and that
//! is still minimised comes back. A window the user brought back by hand is left where it
//! is; the next approved input may minimise it again. A window the user minimised is never
//! brought back by the app.
//!
//! [`WindowAside`] holds these decisions as plain state, tested on every OS. The app reads
//! the window, asks [`WindowAside::decide`] what to do, and makes the window calls.

use std::collections::BTreeSet;

/// The main window as the app reads it just before a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowNow {
    /// Shown, not hidden. macOS reports a minimised window as not visible, Windows as
    /// visible; only `minimised` is read for a minimised window.
    pub visible: bool,
    pub minimised: bool,
}

/// What happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsideEvent {
    /// An approved computer input action of this chat is about to run.
    InputApproved(i64),
    /// An approval card of this chat waits for the user.
    ApprovalNeeded(i64),
    /// This chat's turn ended: done, failed or stopped.
    TurnEnded(i64),
    /// The user pressed Stop (the stop hotkey or the indicator's Stop button).
    Stopped,
}

/// What to do with the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStep {
    Leave,
    Minimise,
    /// Restore it and give it the focus.
    Restore,
}

/// Who minimised the main window, and for which chats.
#[derive(Debug, Default)]
pub struct WindowAside {
    /// The chats whose input minimised the window, for as long as it stays minimised.
    chats: BTreeSet<i64>,
    /// A minimise was asked for and is not over yet (the window may still be on screen).
    minimising: bool,
}

impl WindowAside {
    pub const fn new() -> Self {
        Self {
            chats: BTreeSet::new(),
            minimising: false,
        }
    }

    /// Decides what to do with the window after `event`, given how it looks `now`.
    pub fn decide(&mut self, event: AsideEvent, now: WindowNow) -> WindowStep {
        // The window is back on screen and this app is not minimising it: the user (or
        // something else) brought it back, so it is no longer this app's to restore.
        if !now.minimised && !self.minimising {
            self.chats.clear();
        }
        let down_by_us = !self.chats.is_empty();
        match event {
            AsideEvent::InputApproved(chat) => {
                if now.minimised || self.minimising {
                    // Already out of the way. If this app put it there, this chat counts on
                    // it too; a window the user minimised stays the user's.
                    if down_by_us {
                        self.chats.insert(chat);
                    }
                    WindowStep::Leave
                } else if now.visible {
                    self.chats.insert(chat);
                    self.minimising = true;
                    WindowStep::Minimise
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::ApprovalNeeded(chat) => {
                if self.chats.contains(&chat) {
                    self.bring_back()
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::TurnEnded(chat) => {
                // Another chat whose input is still running keeps it down.
                if self.chats.remove(&chat) && self.chats.is_empty() {
                    WindowStep::Restore
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::Stopped => {
                if down_by_us {
                    self.bring_back()
                } else {
                    WindowStep::Leave
                }
            }
        }
    }

    fn bring_back(&mut self) -> WindowStep {
        self.chats.clear();
        WindowStep::Restore
    }

    /// The minimise [`Self::decide`] asked for is over: the window went, or the wait for
    /// it ended.
    pub fn minimise_finished(&mut self) {
        self.minimising = false;
    }

    /// Whether this app minimised the window for this chat's input and has not brought it
    /// back yet.
    pub fn minimised_for(&self, chat: i64) -> bool {
        self.chats.contains(&chat)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AsideEvent::{ApprovalNeeded, InputApproved, Stopped, TurnEnded};
    use WindowStep::{Leave, Minimise, Restore};

    const CHAT: i64 = 7;
    const OTHER: i64 = 8;

    fn shown() -> WindowNow {
        WindowNow {
            visible: true,
            minimised: false,
        }
    }

    /// As Windows reports a minimised window (macOS reports it as not visible; both mean
    /// the same here).
    fn down() -> WindowNow {
        WindowNow {
            visible: true,
            minimised: true,
        }
    }

    fn down_mac() -> WindowNow {
        WindowNow {
            visible: false,
            minimised: true,
        }
    }

    fn hidden() -> WindowNow {
        WindowNow {
            visible: false,
            minimised: false,
        }
    }

    /// Minimised for `chat`, the minimise confirmed.
    fn minimised_for(chat: i64) -> WindowAside {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(chat), shown()), Minimise);
        a.minimise_finished();
        a
    }

    #[test]
    fn approved_input_minimises_a_window_on_screen() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        assert!(a.minimised_for(CHAT));
        a.minimise_finished();
        // The next input of the turn finds it out of the way already.
        assert_eq!(a.decide(InputApproved(CHAT), down()), Leave);
        assert_eq!(a.decide(InputApproved(CHAT), down_mac()), Leave);
        assert!(a.minimised_for(CHAT));
        // A hidden window is left alone.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), hidden()), Leave);
        assert!(!a.minimised_for(CHAT));
    }

    #[test]
    fn an_approval_card_brings_the_window_back_and_the_next_input_minimises_it_again() {
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(ApprovalNeeded(CHAT), down()), Restore);
        assert!(!a.minimised_for(CHAT));
        // The user answers the card; the approved input steps aside again.
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        // A card when nothing was minimised changes nothing.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(ApprovalNeeded(CHAT), shown()), Leave);
    }

    #[test]
    fn a_card_of_another_chat_does_not_bring_it_back() {
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(ApprovalNeeded(OTHER), down()), Leave);
        assert!(a.minimised_for(CHAT));
    }

    #[test]
    fn the_end_of_the_turn_and_stop_bring_it_back() {
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(TurnEnded(CHAT), down()), Restore);
        assert!(!a.minimised_for(CHAT));
        // Only once.
        assert_eq!(a.decide(TurnEnded(CHAT), down()), Leave);

        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(Stopped, down_mac()), Restore);
        assert_eq!(a.decide(Stopped, down()), Leave);
        // Stop with nothing minimised by the app.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(Stopped, down()), Leave);
    }

    #[test]
    fn the_end_of_another_chats_turn_leaves_it_down() {
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(TurnEnded(OTHER), down()), Leave);
        assert!(a.minimised_for(CHAT));
        // Two chats acting: it comes back when the last of them ends.
        assert_eq!(a.decide(InputApproved(OTHER), down()), Leave);
        assert!(a.minimised_for(OTHER));
        assert_eq!(a.decide(TurnEnded(CHAT), down()), Leave);
        assert_eq!(a.decide(TurnEnded(OTHER), down()), Restore);
    }

    #[test]
    fn a_window_the_user_brought_back_is_left_alone() {
        let mut a = minimised_for(CHAT);
        // The user restored it by hand: it is theirs, nothing brings it "back".
        assert_eq!(a.decide(ApprovalNeeded(CHAT), shown()), Leave);
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(TurnEnded(CHAT), shown()), Leave);
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(Stopped, shown()), Leave);
        assert!(!a.minimised_for(CHAT));
        // The next approved input may minimise it again.
        let mut a = minimised_for(CHAT);
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
    }

    #[test]
    fn a_window_the_user_minimised_is_never_brought_back() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), down()), Leave);
        assert!(!a.minimised_for(CHAT));
        assert_eq!(a.decide(ApprovalNeeded(CHAT), down()), Leave);
        assert_eq!(a.decide(TurnEnded(CHAT), down()), Leave);
        assert_eq!(a.decide(Stopped, down_mac()), Leave);
    }

    #[test]
    fn stop_during_the_minimise_still_brings_it_back() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        // The window has not gone yet when Stop is pressed.
        assert_eq!(a.decide(Stopped, shown()), Restore);
        a.minimise_finished();
        // The same for the end of a turn cut short meanwhile.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        assert_eq!(a.decide(TurnEnded(CHAT), shown()), Restore);
        // Another chat's turn ending meanwhile does not take the window from this one.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        assert_eq!(a.decide(TurnEnded(OTHER), shown()), Leave);
        assert!(a.minimised_for(CHAT));
    }

    #[test]
    fn a_minimise_that_never_happened_is_forgotten() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
        a.minimise_finished();
        // The window is still on screen: nothing to bring back, and the next input tries
        // again.
        assert_eq!(a.decide(TurnEnded(CHAT), shown()), Leave);
        assert_eq!(a.decide(InputApproved(CHAT), shown()), Minimise);
    }
}
