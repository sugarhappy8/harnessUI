//! The main window steps aside while computer use acts (ARCHITECTURE.md 9.3, "Stepping
//! aside").
//!
//! Screenshots leave this app's windows out, so the model sees the applications behind
//! them, and input never goes to this app (the guard refuses it). So when an approved input
//! action is about to run and this app's main window is in its way, the app minimises the
//! window, and it brings the window back when the user is needed:
//!
//! - an approval card of a chat that minimised it: shown in front of the other windows,
//!   without taking the keyboard from the application in front ([`Comeback::InFront`]);
//! - the end of that turn: restored where it was, without taking the keyboard, and the
//!   user is told (the taskbar button flashes, the Dock icon bounces) ([`Comeback::Quietly`]);
//! - Stop: restored and given the keyboard, since the user asked for it
//!   ([`Comeback::Focused`]).
//!
//! Only a window the app minimised itself and that is still minimised comes back. A window
//! the user brought back by hand is theirs: it is left where it is, and the next approved
//! input may minimise it again. A window the user minimised is never brought back by the
//! app. Who minimised the window is kept per turn, so a stopped turn that ends late cannot
//! bring the window back in the middle of the chat's next turn.
//!
//! [`WindowAside`] holds the decisions as plain state. [`Aside`] runs them against a
//! [`MainWindow`] (Tauri's window in the app, a fake in the tests): one minimise or restore
//! at a time, never for a call that was stopped meanwhile. Both are tested on every OS.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

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
    /// An approved computer input action of this chat's turn is about to run, and the main
    /// window is in its way.
    InputApproved { chat: i64, turn: u64 },
    /// An approval card of this chat waits for the user.
    ApprovalNeeded { chat: i64 },
    /// This turn of the chat ended: done, failed or stopped.
    TurnEnded { chat: i64, turn: u64 },
    /// The user pressed Stop (the stop hotkey or the indicator's Stop button).
    Stopped,
}

/// How the window comes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Comeback {
    /// Restored and given the keyboard: the user pressed Stop.
    Focused,
    /// Shown in front of the other windows without taking the keyboard from the
    /// application in front, and the user is told: an approval card needs them.
    InFront,
    /// Restored where it was, without taking the keyboard, and the user is told: the turn
    /// ended.
    Quietly,
}

/// What to do with the main window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStep {
    Leave,
    Minimise,
    Restore(Comeback),
}

/// Who minimised the main window: the turns whose input did, for as long as it stays
/// minimised.
#[derive(Debug, Default)]
pub struct WindowAside {
    /// (chat, turn) pairs.
    holders: BTreeSet<(i64, u64)>,
    /// A minimise was asked for and is not over yet (the window may still be on screen).
    minimising: bool,
}

impl WindowAside {
    pub const fn new() -> Self {
        Self {
            holders: BTreeSet::new(),
            minimising: false,
        }
    }

    /// Decides what to do with the window after `event`, given how it looks `now`.
    pub fn decide(&mut self, event: AsideEvent, now: WindowNow) -> WindowStep {
        // The window is back on screen and this app is not minimising it: the user (or
        // something else) brought it back, so it is no longer this app's to restore.
        if !now.minimised && !self.minimising {
            self.holders.clear();
        }
        let down_by_us = !self.holders.is_empty();
        match event {
            AsideEvent::InputApproved { chat, turn } => {
                if now.minimised || self.minimising {
                    // Already out of the way. If this app put it there, this turn counts on
                    // it too; a window the user minimised stays the user's.
                    if down_by_us {
                        self.holders.insert((chat, turn));
                    }
                    WindowStep::Leave
                } else if now.visible {
                    self.holders.insert((chat, turn));
                    self.minimising = true;
                    WindowStep::Minimise
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::ApprovalNeeded { chat } => {
                if self.holders.iter().any(|&(c, _)| c == chat) {
                    self.bring_back(Comeback::InFront)
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::TurnEnded { chat, turn } => {
                // Another turn whose input is still running keeps it down.
                if self.holders.remove(&(chat, turn)) && self.holders.is_empty() {
                    WindowStep::Restore(Comeback::Quietly)
                } else {
                    WindowStep::Leave
                }
            }
            AsideEvent::Stopped => {
                if down_by_us {
                    self.bring_back(Comeback::Focused)
                } else {
                    WindowStep::Leave
                }
            }
        }
    }

    fn bring_back(&mut self, how: Comeback) -> WindowStep {
        self.holders.clear();
        WindowStep::Restore(how)
    }

    /// The minimise [`Self::decide`] asked for is over: the window went, or the wait for
    /// it ended.
    pub fn minimise_finished(&mut self) {
        self.minimising = false;
    }

    /// The window came back on screen or got the focus, and not by this app's doing: it is
    /// the user's again, even if they minimise it once more before the next decision.
    pub fn brought_back(&mut self) {
        if !self.minimising {
            self.holders.clear();
        }
    }

    /// Whether this app minimised the window for this chat's input and has not brought it
    /// back yet.
    pub fn minimised_for(&self, chat: i64) -> bool {
        self.holders.iter().any(|&(c, _)| c == chat)
    }

    /// Whether `event` can lead to a step at all, so the window need not be read for the
    /// others (an approval card of a chat that minimised nothing, say).
    pub fn concerns(&self, event: AsideEvent) -> bool {
        match event {
            AsideEvent::InputApproved { .. } => true,
            AsideEvent::ApprovalNeeded { chat } => self.minimised_for(chat),
            AsideEvent::TurnEnded { chat, turn } => self.holders.contains(&(chat, turn)),
            AsideEvent::Stopped => !self.holders.is_empty(),
        }
    }
}

/// The main window as the app controls it: Tauri's window in the app, a fake in the tests.
/// Every call may wait for the app's main thread, so [`Aside`] never makes one on it.
pub trait MainWindow: Send + Sync {
    fn now(&self) -> WindowNow;
    fn minimise(&self);
    fn unminimise(&self);
    /// The window is back on screen: finish bringing it back as `how` says, and tell the
    /// page (approval cards start their input delay again).
    fn present(&self, how: Comeback);
}

/// How long the app waits for its window to go or to come back (both are animated).
const WINDOW_WAIT: Duration = Duration::from_secs(1);
const WINDOW_POLL: Duration = Duration::from_millis(25);
/// After the window reports minimised, while the system may still be animating it away.
const MINIMISE_SETTLE: Duration = Duration::from_millis(100);
/// An approval on a card counts for the input call that follows it within this time.
const CARD_APPROVAL_FRESH: Duration = Duration::from_secs(10);
/// Window events this soon after the app brought the window back are its own doing.
const OWN_RESTORE_GRACE: Duration = Duration::from_millis(500);

/// Runs [`WindowAside`]'s decisions against the main window: one minimise or restore at a
/// time, under a lock the app's main thread never takes, so a restore and the minimise that
/// follows it cannot interleave. The decision state has its own lock, never held across a
/// window call. Also remembers which turn of each chat runs, and whether the user just
/// approved a chat's input on its card.
#[derive(Debug)]
pub struct Aside {
    state: Mutex<WindowAside>,
    ops: Mutex<()>,
    turns: Mutex<BTreeMap<i64, u64>>,
    next_turn: AtomicU64,
    cards: Mutex<BTreeMap<i64, Instant>>,
    restored_at: Mutex<Option<Instant>>,
}

impl Default for Aside {
    fn default() -> Self {
        Self::new()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Aside {
    pub const fn new() -> Self {
        Self {
            state: Mutex::new(WindowAside::new()),
            ops: Mutex::new(()),
            turns: Mutex::new(BTreeMap::new()),
            next_turn: AtomicU64::new(1),
            cards: Mutex::new(BTreeMap::new()),
            restored_at: Mutex::new(None),
        }
    }

    /// A turn of this chat starts: returns its key for [`Self::turn_ended`].
    pub fn turn_started(&self, chat: i64) -> u64 {
        let turn = self.next_turn.fetch_add(1, Ordering::Relaxed);
        lock(&self.turns).insert(chat, turn);
        lock(&self.cards).remove(&chat);
        turn
    }

    fn current_turn(&self, chat: i64) -> u64 {
        lock(&self.turns).get(&chat).copied().unwrap_or(0)
    }

    /// The user approved computer input of this chat on its card just now.
    pub fn card_approved(&self, chat: i64) {
        lock(&self.cards).insert(chat, Instant::now());
    }

    /// Whether the user just approved this chat's input on its card. Asked once per input
    /// call: the answer is used up.
    pub fn take_card_approval(&self, chat: i64) -> bool {
        lock(&self.cards)
            .remove(&chat)
            .is_some_and(|at| at.elapsed() <= CARD_APPROVAL_FRESH)
    }

    /// Before approved input of this chat with the main window in its way (blocking):
    /// minimises the window when it is on screen and waits until it is gone, at most about
    /// a second. A call stopped meanwhile does not minimise it: its turn is ending, and
    /// Stop's restore may already have looked and found nothing to bring back.
    pub fn before_input(&self, w: &dyn MainWindow, chat: i64, cancel: &CancellationToken) {
        let _one = lock(&self.ops);
        if cancel.is_cancelled() {
            return;
        }
        let seen = w.now();
        // Reading the window waits for the main thread; Stop may have come meanwhile.
        if cancel.is_cancelled() {
            return;
        }
        let event = AsideEvent::InputApproved {
            chat,
            turn: self.current_turn(chat),
        };
        if lock(&self.state).decide(event, seen) != WindowStep::Minimise {
            return;
        }
        w.minimise();
        if self.wait_until(w, true) {
            std::thread::sleep(MINIMISE_SETTLE);
        }
        lock(&self.state).minimise_finished();
    }

    /// An approval card of this chat waits for the user (blocking).
    pub fn approval_needed(&self, w: &dyn MainWindow, chat: i64) {
        self.bring_back(w, AsideEvent::ApprovalNeeded { chat });
    }

    /// This turn of the chat ended (blocking).
    pub fn turn_ended(&self, w: &dyn MainWindow, chat: i64, turn: u64) {
        {
            let mut turns = lock(&self.turns);
            if turns.get(&chat) == Some(&turn) {
                turns.remove(&chat);
                lock(&self.cards).remove(&chat);
            }
        }
        self.bring_back(w, AsideEvent::TurnEnded { chat, turn });
    }

    /// The user pressed Stop (blocking).
    pub fn stopped(&self, w: &dyn MainWindow) {
        lock(&self.cards).clear();
        self.bring_back(w, AsideEvent::Stopped);
    }

    /// The window came back on screen or got the focus (a window event), and not just now
    /// by this app's own restore: it is the user's again.
    pub fn brought_back(&self) {
        let own = lock(&self.restored_at).is_some_and(|at| at.elapsed() < OWN_RESTORE_GRACE);
        if !own {
            lock(&self.state).brought_back();
        }
    }

    /// Whether the app minimised the window for this chat's input and has not brought it
    /// back yet.
    pub fn minimised_for(&self, chat: i64) -> bool {
        lock(&self.state).minimised_for(chat)
    }

    fn bring_back(&self, w: &dyn MainWindow, event: AsideEvent) {
        let _one = lock(&self.ops);
        // Decided under the operation lock: a minimise in progress finishes first, so Stop
        // or the end of a turn always sees it.
        if !lock(&self.state).concerns(event) {
            return;
        }
        let seen = w.now();
        let WindowStep::Restore(how) = lock(&self.state).decide(event, seen) else {
            return;
        };
        w.unminimise();
        self.wait_until(w, false);
        *lock(&self.restored_at) = Some(Instant::now());
        w.present(how);
    }

    /// Waits until the window is (or is no longer) minimised, at most [`WINDOW_WAIT`].
    fn wait_until(&self, w: &dyn MainWindow, minimised: bool) -> bool {
        let start = Instant::now();
        loop {
            if w.now().minimised == minimised {
                return true;
            }
            if start.elapsed() >= WINDOW_WAIT {
                return false;
            }
            std::thread::sleep(WINDOW_POLL);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use AsideEvent::{ApprovalNeeded, InputApproved, Stopped, TurnEnded};
    use WindowStep::{Leave, Minimise, Restore};
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};

    const CHAT: i64 = 7;
    const OTHER: i64 = 8;
    const TURN: u64 = 1;
    const NEXT: u64 = 2;

    fn input(chat: i64, turn: u64) -> AsideEvent {
        InputApproved { chat, turn }
    }

    fn ended(chat: i64, turn: u64) -> AsideEvent {
        TurnEnded { chat, turn }
    }

    fn card(chat: i64) -> AsideEvent {
        ApprovalNeeded { chat }
    }

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

    /// Minimised for this turn of the chat, the minimise confirmed.
    fn minimised_for(chat: i64, turn: u64) -> WindowAside {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(chat, turn), shown()), Minimise);
        a.minimise_finished();
        a
    }

    #[test]
    fn approved_input_minimises_a_window_on_screen() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        assert!(a.minimised_for(CHAT));
        a.minimise_finished();
        // The next input of the turn finds it out of the way already.
        assert_eq!(a.decide(input(CHAT, TURN), down()), Leave);
        assert_eq!(a.decide(input(CHAT, TURN), down_mac()), Leave);
        assert!(a.minimised_for(CHAT));
        // A hidden window is left alone.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), hidden()), Leave);
        assert!(!a.minimised_for(CHAT));
    }

    #[test]
    fn each_occasion_brings_the_window_back_its_own_way() {
        // A card: in front, without the keyboard.
        let mut a = minimised_for(CHAT, TURN);
        assert_eq!(a.decide(card(CHAT), down()), Restore(Comeback::InFront));
        assert!(!a.minimised_for(CHAT));
        // The user answers the card; the approved input steps aside again.
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        a.minimise_finished();
        // The end of the turn: where it was, without the keyboard.
        assert_eq!(
            a.decide(ended(CHAT, TURN), down()),
            Restore(Comeback::Quietly)
        );
        // Only once.
        assert_eq!(a.decide(ended(CHAT, TURN), down()), Leave);
        // Stop: with the keyboard.
        let mut a = minimised_for(CHAT, TURN);
        assert_eq!(a.decide(Stopped, down_mac()), Restore(Comeback::Focused));
        assert_eq!(a.decide(Stopped, down()), Leave);
        // Nothing minimised by the app: nothing comes back.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(card(CHAT), shown()), Leave);
        assert_eq!(a.decide(Stopped, down()), Leave);
    }

    #[test]
    fn a_card_of_another_chat_does_not_bring_it_back() {
        let mut a = minimised_for(CHAT, TURN);
        assert_eq!(a.decide(card(OTHER), down()), Leave);
        assert!(a.minimised_for(CHAT));
    }

    #[test]
    fn the_end_of_another_turn_leaves_it_down() {
        let mut a = minimised_for(CHAT, TURN);
        assert_eq!(a.decide(ended(OTHER, TURN), down()), Leave);
        // A stopped turn of the same chat that ends late, during the chat's next turn.
        let mut b = minimised_for(CHAT, NEXT);
        assert_eq!(b.decide(ended(CHAT, TURN), down()), Leave);
        assert!(b.minimised_for(CHAT));
        assert_eq!(
            b.decide(ended(CHAT, NEXT), down()),
            Restore(Comeback::Quietly)
        );
        // Two turns acting: it comes back when the last of them ends.
        assert_eq!(a.decide(input(OTHER, NEXT), down()), Leave);
        assert!(a.minimised_for(OTHER));
        assert_eq!(a.decide(ended(CHAT, TURN), down()), Leave);
        assert_eq!(
            a.decide(ended(OTHER, NEXT), down()),
            Restore(Comeback::Quietly)
        );
    }

    #[test]
    fn a_window_the_user_brought_back_is_left_alone() {
        for event in [card(CHAT), ended(CHAT, TURN), Stopped] {
            let mut a = minimised_for(CHAT, TURN);
            assert_eq!(a.decide(event, shown()), Leave, "{event:?}");
            assert!(!a.minimised_for(CHAT));
        }
        // The next approved input may minimise it again.
        let mut a = minimised_for(CHAT, TURN);
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
    }

    #[test]
    fn a_window_the_user_brought_back_and_minimised_again_stays_theirs() {
        let mut a = minimised_for(CHAT, TURN);
        // Seen as a window event, between two decisions: the user restored it…
        a.brought_back();
        // …and minimised it again before the turn ended.
        assert_eq!(a.decide(ended(CHAT, TURN), down()), Leave);
        assert_eq!(a.decide(card(CHAT), down()), Leave);
        assert_eq!(a.decide(Stopped, down()), Leave);
        // While the app is minimising, its own window events do not count.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        a.brought_back();
        a.minimise_finished();
        assert_eq!(
            a.decide(ended(CHAT, TURN), down()),
            Restore(Comeback::Quietly)
        );
    }

    #[test]
    fn a_window_the_user_minimised_is_never_brought_back() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), down()), Leave);
        assert!(!a.minimised_for(CHAT));
        assert_eq!(a.decide(card(CHAT), down()), Leave);
        assert_eq!(a.decide(ended(CHAT, TURN), down()), Leave);
        assert_eq!(a.decide(Stopped, down_mac()), Leave);
    }

    #[test]
    fn stop_during_the_minimise_still_brings_it_back() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        // The window has not gone yet when Stop is pressed.
        assert_eq!(a.decide(Stopped, shown()), Restore(Comeback::Focused));
        a.minimise_finished();
        // The same for the end of a turn cut short meanwhile.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        assert_eq!(
            a.decide(ended(CHAT, TURN), shown()),
            Restore(Comeback::Quietly)
        );
        // Another chat's turn ending meanwhile does not take the window from this one.
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        assert_eq!(a.decide(ended(OTHER, TURN), shown()), Leave);
        assert!(a.minimised_for(CHAT));
    }

    #[test]
    fn a_minimise_that_never_happened_is_forgotten() {
        let mut a = WindowAside::new();
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
        a.minimise_finished();
        // The window is still on screen: nothing to bring back, and the next input tries
        // again.
        assert_eq!(a.decide(ended(CHAT, TURN), shown()), Leave);
        assert_eq!(a.decide(input(CHAT, TURN), shown()), Minimise);
    }

    #[test]
    fn only_events_that_can_move_the_window_concern_it() {
        let a = WindowAside::new();
        assert!(a.concerns(input(CHAT, TURN)));
        assert!(!a.concerns(card(CHAT)));
        assert!(!a.concerns(ended(CHAT, TURN)));
        assert!(!a.concerns(Stopped));
        let a = minimised_for(CHAT, TURN);
        assert!(a.concerns(card(CHAT)));
        assert!(a.concerns(ended(CHAT, TURN)));
        assert!(a.concerns(Stopped));
        assert!(!a.concerns(card(OTHER)));
        assert!(!a.concerns(ended(OTHER, TURN)));
        assert!(!a.concerns(ended(CHAT, NEXT)));
        // Whatever `concerns` skips, `decide` would have left alone.
        for event in [card(OTHER), ended(OTHER, TURN), ended(CHAT, NEXT)] {
            for now in [shown(), down(), down_mac(), hidden()] {
                let mut b = minimised_for(CHAT, TURN);
                assert_eq!(b.decide(event, now), Leave, "{event:?} {now:?}");
            }
        }
        for event in [card(CHAT), ended(CHAT, TURN), Stopped] {
            for now in [shown(), down(), down_mac(), hidden()] {
                let mut b = WindowAside::new();
                assert_eq!(b.decide(event, now), Leave, "{event:?} {now:?}");
            }
        }
    }

    // ---- The controller, against a fake window ------------------------------------------

    /// One step of a fake window call that a test can hold: the call says it has started,
    /// then waits to be let go.
    struct Hold {
        started: Sender<()>,
        go: Receiver<()>,
    }

    /// A test's end of a [`Hold`].
    struct Holder {
        started: Receiver<()>,
        go: Sender<()>,
    }

    fn hold() -> (Hold, Holder) {
        let (started_tx, started_rx) = channel();
        let (go_tx, go_rx) = channel();
        (
            Hold {
                started: started_tx,
                go: go_rx,
            },
            Holder {
                started: started_rx,
                go: go_tx,
            },
        )
    }

    #[derive(Debug, Default)]
    struct Shown {
        minimised: bool,
        focused: bool,
        minimise_calls: usize,
        presented: Vec<Comeback>,
    }

    /// A main window that minimises and comes back at once, and records what happened.
    #[derive(Default)]
    struct FakeWindow {
        state: Mutex<Shown>,
        /// Holds the next reading of the window (the main-thread round trip).
        hold_now: Mutex<Option<Hold>>,
        /// Holds the next minimise.
        hold_minimise: Mutex<Option<Hold>>,
    }

    fn wait_at(slot: &Mutex<Option<Hold>>) {
        let hold = lock(slot).take();
        if let Some(h) = hold {
            let _ = h.started.send(());
            let _ = h.go.recv();
        }
    }

    impl FakeWindow {
        fn shown(&self) -> std::sync::MutexGuard<'_, Shown> {
            lock(&self.state)
        }
    }

    impl MainWindow for FakeWindow {
        fn now(&self) -> WindowNow {
            wait_at(&self.hold_now);
            let s = self.shown();
            WindowNow {
                visible: true,
                minimised: s.minimised,
            }
        }
        fn minimise(&self) {
            wait_at(&self.hold_minimise);
            let mut s = self.shown();
            s.minimised = true;
            s.focused = false;
            s.minimise_calls += 1;
        }
        fn unminimise(&self) {
            self.shown().minimised = false;
        }
        fn present(&self, how: Comeback) {
            let mut s = self.shown();
            s.presented.push(how);
            if how == Comeback::Focused {
                s.focused = true;
            }
        }
    }

    #[test]
    fn the_controller_minimises_and_brings_back_each_way() {
        let aside = Aside::new();
        let w = FakeWindow::default();
        let turn = aside.turn_started(CHAT);
        let go = CancellationToken::new();
        aside.before_input(&w, CHAT, &go);
        assert!(w.shown().minimised);
        assert!(aside.minimised_for(CHAT));
        aside.approval_needed(&w, CHAT);
        assert!(!w.shown().minimised);
        aside.before_input(&w, CHAT, &go);
        aside.turn_ended(&w, CHAT, turn);
        assert!(!w.shown().minimised);
        let turn = aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &go);
        aside.stopped(&w);
        assert!(!w.shown().minimised);
        assert!(w.shown().focused);
        assert_eq!(
            w.shown().presented,
            vec![Comeback::InFront, Comeback::Quietly, Comeback::Focused]
        );
        // A turn that ended after Stop brought the window back changes nothing.
        aside.turn_ended(&w, CHAT, turn);
        assert_eq!(w.shown().presented.len(), 3);
    }

    #[test]
    fn a_stopped_call_never_minimises() {
        let aside = Aside::new();
        let w = FakeWindow::default();
        aside.turn_started(CHAT);
        let stop = CancellationToken::new();
        stop.cancel();
        aside.before_input(&w, CHAT, &stop);
        assert_eq!(w.shown().minimise_calls, 0);
    }

    /// Stop lands while the minimise reads the window, before it decides: the window must
    /// not be minimised for the stopped call (Stop's restore finds nothing to bring back).
    #[test]
    fn stop_while_the_window_is_read_leaves_it_on_screen() {
        let aside = Arc::new(Aside::new());
        let w = Arc::new(FakeWindow::default());
        aside.turn_started(CHAT);
        let (h, holder) = hold();
        *lock(&w.hold_now) = Some(h);
        let call = CancellationToken::new();
        let input = {
            let (aside, w, call) = (aside.clone(), w.clone(), call.clone());
            std::thread::spawn(move || aside.before_input(&*w, CHAT, &call))
        };
        holder.started.recv().unwrap();
        // Stop: the call is cancelled and the app asks for the window back.
        call.cancel();
        let stop = {
            let (aside, w) = (aside.clone(), w.clone());
            std::thread::spawn(move || aside.stopped(&*w))
        };
        holder.go.send(()).unwrap();
        input.join().unwrap();
        stop.join().unwrap();
        assert_eq!(w.shown().minimise_calls, 0);
        assert!(!w.shown().minimised);
        assert!(!aside.minimised_for(CHAT));
    }

    /// Stop, or the end of the turn, lands while the window is being minimised: the restore
    /// waits for the minimise and then brings the window back.
    #[test]
    fn stop_or_the_turns_end_during_the_minimise_brings_it_back() {
        for stop_it in [true, false] {
            let aside = Arc::new(Aside::new());
            let w = Arc::new(FakeWindow::default());
            let turn = aside.turn_started(CHAT);
            let (h, holder) = hold();
            *lock(&w.hold_minimise) = Some(h);
            let call = CancellationToken::new();
            let input = {
                let (aside, w, call) = (aside.clone(), w.clone(), call.clone());
                std::thread::spawn(move || aside.before_input(&*w, CHAT, &call))
            };
            holder.started.recv().unwrap();
            call.cancel();
            let back = {
                let (aside, w) = (aside.clone(), w.clone());
                std::thread::spawn(move || {
                    if stop_it {
                        aside.stopped(&*w);
                    } else {
                        aside.turn_ended(&*w, CHAT, turn);
                    }
                })
            };
            holder.go.send(()).unwrap();
            input.join().unwrap();
            back.join().unwrap();
            let s = w.shown();
            assert_eq!(s.minimise_calls, 1, "stop {stop_it}");
            assert!(!s.minimised, "stop {stop_it}: left minimised");
            let expected = if stop_it {
                Comeback::Focused
            } else {
                Comeback::Quietly
            };
            assert_eq!(s.presented, vec![expected], "stop {stop_it}");
        }
    }

    #[test]
    fn a_late_end_of_a_stopped_turn_does_not_bring_the_window_back_mid_turn() {
        let aside = Aside::new();
        let w = FakeWindow::default();
        let go = CancellationToken::new();
        let first = aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &go);
        aside.stopped(&w);
        // The next turn of the same chat starts before the stopped one has wound down.
        let second = aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &go);
        aside.turn_ended(&w, CHAT, first);
        assert!(
            w.shown().minimised,
            "brought back in the middle of the next turn"
        );
        aside.turn_ended(&w, CHAT, second);
        assert!(!w.shown().minimised);
    }

    #[test]
    fn a_window_the_user_brought_back_between_decisions_stays_theirs() {
        let aside = Aside::new();
        let w = FakeWindow::default();
        let turn = aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &CancellationToken::new());
        // The user restores the window (a window event says so) and minimises it again.
        w.unminimise();
        aside.brought_back();
        w.shown().minimised = true;
        aside.turn_ended(&w, CHAT, turn);
        assert!(w.shown().minimised);
        assert!(w.shown().presented.is_empty());
    }

    #[test]
    fn the_apps_own_restore_is_not_taken_for_the_users() {
        let aside = Aside::new();
        let w = FakeWindow::default();
        let go = CancellationToken::new();
        aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &go);
        aside.approval_needed(&w, CHAT);
        // The user answers the card at once; the input steps aside again, and a window
        // event left over from the app's restore arrives only now.
        let turn = aside.turn_started(CHAT);
        aside.before_input(&w, CHAT, &go);
        aside.brought_back();
        aside.turn_ended(&w, CHAT, turn);
        assert!(!w.shown().minimised);
        assert_eq!(
            w.shown().presented,
            vec![Comeback::InFront, Comeback::Quietly]
        );
    }

    #[test]
    fn a_card_approval_counts_once_and_only_in_its_turn() {
        let aside = Aside::new();
        let turn = aside.turn_started(CHAT);
        assert!(!aside.take_card_approval(CHAT));
        aside.card_approved(CHAT);
        assert!(!aside.take_card_approval(OTHER));
        assert!(aside.take_card_approval(CHAT));
        assert!(!aside.take_card_approval(CHAT));
        // Left over when the turn ends, when the next turn starts, or after Stop: gone.
        let w = FakeWindow::default();
        aside.card_approved(CHAT);
        aside.turn_ended(&w, CHAT, turn);
        assert!(!aside.take_card_approval(CHAT));
        aside.card_approved(CHAT);
        aside.turn_started(CHAT);
        assert!(!aside.take_card_approval(CHAT));
        aside.card_approved(CHAT);
        aside.stopped(&w);
        assert!(!aside.take_card_approval(CHAT));
    }
}
