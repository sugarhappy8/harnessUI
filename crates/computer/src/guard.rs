//! The one injector all input goes through (ARCHITECTURE.md 9.3, 4.2, 13.5).
//!
//! - **Never this app.** Before every event the guard asks the platform which window the
//!   event would reach (under the point for the pointer, the foreground window for keys)
//!   and refuses if any process in that window's chain is this app or one of its
//!   children. Without this a pointer grant would let the model press Allow on its own
//!   approval card.
//! - **Only the approved application** (D7): the target's application must be the one on
//!   the approval card, checked before every event, not once per call. A target that
//!   always asks (a terminal or console window, a password manager, typing into the
//!   Windows shell, …) is refused unless the user approved this very action on an
//!   always-ask card, so a grant never carries over to such a window of the same
//!   application.
//! - **Elevated windows** (Windows) are refused with a clear message instead of failing
//!   silently.
//! - **Stop.** Every key and button the guard presses is recorded before it is sent. Stop
//!   ([`StopHandle::release_all`], the call's cancellation token), a panic inside an
//!   operation (the [`Op`] drop guard) and shutdown ([`InputGuard::shutdown`], and dropping
//!   the guard) release everything recorded plus every modifier and mouse button the OS
//!   reports down. Stop also closes the [`Gate`] of every operation in progress, so an
//!   event that was already queued cannot land after the releases.
//! - **Releases always go out**, whatever is under the pointer or in front, so nothing
//!   stays held.
//!
//! There is a gap of microseconds between looking up the target and sending the event; a
//! window that appears in exactly that gap receives the event. The platforms offer no
//! atomic "send only to this window" for synthetic input.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use crate::apps::{Target, target_always_ask_reason};
use crate::geometry::GlobalPoint;
use crate::keys::Key;
use crate::platform::{
    Gate, Injector, InputError, InputEvent, MouseButton, Pressable, SelfProcess,
};

/// The message the spec fixes for elevated windows (9.3).
pub const ELEVATED_REFUSAL: &str =
    "that window is running as administrator and can't be controlled";

/// Why the guard refused an event. The text is written for the model.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Refusal {
    #[error(
        "That point is covered by this app's own window. Computer use never sends input to this app. Ask the user to minimise or move this app's window, then take a new screenshot."
    )]
    SelfPointer,
    #[error(
        "This app's own window has the keyboard focus. Computer use never types into this app. Click the window you want to type into first, or ask the user to bring it to the front."
    )]
    SelfKeyboard,
    #[error("{ELEVATED_REFUSAL}")]
    Elevated,
    #[error(
        "That point is on the computer-use indicator's Stop button, which only the user may press."
    )]
    Protected,
    #[error(
        "The input would now go to {now}, not to {approved}, which this action was approved for, so it was not sent. Take a new screenshot and try again."
    )]
    OtherApp { approved: String, now: String },
    /// The target now always needs the user's approval (a console window, say), but the
    /// action was approved without that card, possibly by a grant.
    #[error(
        "The input would now go to a window that always needs the user's approval ({why}), so it was not sent. Take a new screenshot and try again."
    )]
    NeedsApproval { why: String },
}

/// Why an operation ended early.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum OpError {
    #[error("{0}")]
    Refused(Refusal),
    #[error("Stopped by the user. Every key and mouse button was released.")]
    Stopped,
    #[error("{0}")]
    Input(InputError),
}

impl From<InputError> for OpError {
    fn from(e: InputError) -> Self {
        match e {
            InputError::Stopped => OpError::Stopped,
            other => OpError::Input(other),
        }
    }
}

/// The self and elevation rules for a target (no application check).
pub fn refuse_target(me: &SelfProcess, target: &Target, keyboard: bool) -> Option<Refusal> {
    if me.owns_any(&target.pids) {
        return Some(if keyboard {
            Refusal::SelfKeyboard
        } else {
            Refusal::SelfPointer
        });
    }
    if target.elevated {
        return Some(Refusal::Elevated);
    }
    None
}

/// The protected-area rule for a point (the overlay's Stop button).
pub fn refuse_point(me: &SelfProcess, p: GlobalPoint) -> Option<Refusal> {
    me.protected_rects()
        .iter()
        .any(|r| r.contains(p))
        .then_some(Refusal::Protected)
}

/// Keys and buttons currently pressed by the guard, in press order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Held {
    pub keys: Vec<Key>,
    pub buttons: Vec<MouseButton>,
}

struct Inner {
    injector: Arc<dyn Injector>,
    me: SelfProcess,
    epoch: Arc<AtomicU64>,
    held: Mutex<Held>,
}

impl Inner {
    fn held(&self) -> MutexGuard<'_, Held> {
        self.held.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sends a release, never letting a panic in the platform code escape (this runs from
    /// drop guards during unwinding).
    fn send_release(&self, event: InputEvent) {
        let r = catch_unwind(AssertUnwindSafe(|| {
            self.injector.send(&event, &Gate::always())
        }));
        match r {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!(?event, error = %e, "release failed"),
            Err(_) => tracing::error!(?event, "release panicked"),
        }
    }

    fn should_release(&self, what: Pressable) -> bool {
        catch_unwind(AssertUnwindSafe(|| self.injector.is_pressed(what)))
            .ok()
            .flatten()
            != Some(false)
    }

    /// Closes every open gate, then releases everything recorded (in reverse press order)
    /// plus every mouse button and modifier the OS reports down, or cannot report on.
    fn release_everything(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let held = std::mem::take(&mut *self.held());
        let mut buttons: Vec<MouseButton> = held.buttons.iter().rev().copied().collect();
        for b in MouseButton::ALL {
            if !buttons.contains(&b) && self.should_release(Pressable::Button(b)) {
                buttons.push(b);
            }
        }
        for b in buttons {
            self.send_release(InputEvent::Button {
                button: b,
                down: false,
            });
        }
        let mut keys: Vec<Key> = held.keys.iter().rev().copied().collect();
        for k in Key::MODIFIERS {
            if !keys.contains(&k) && self.should_release(Pressable::Key(k)) {
                keys.push(k);
            }
        }
        for k in keys {
            self.send_release(InputEvent::Key {
                key: k,
                down: false,
            });
        }
    }
}

/// Owns the injector. One per app; the computer server holds it.
pub struct InputGuard {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for InputGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputGuard")
            .field("held", &*self.inner.held())
            .finish_non_exhaustive()
    }
}

impl InputGuard {
    pub fn new(injector: Arc<dyn Injector>, me: SelfProcess) -> Self {
        Self {
            inner: Arc::new(Inner {
                injector,
                me,
                epoch: Arc::new(AtomicU64::new(0)),
                held: Mutex::new(Held::default()),
            }),
        }
    }

    /// A cheap handle for the stop hotkey, the overlay's Stop button and the panic hook.
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle {
            inner: self.inner.clone(),
        }
    }

    /// Starts an operation (one tool call's worth of events) for an action the user did
    /// not approve on an always-ask card: events whose target always asks (a terminal, a
    /// console window, …) are refused.
    pub fn begin(&self, cancel: CancellationToken) -> Op {
        self.begin_with(cancel, false)
    }

    /// Starts an operation. `always_ask_approved` is true only when the user approved
    /// this very action on an always-ask card (Allow once); otherwise a target that always
    /// asks is refused before each event, so a grant for an application never carries
    /// over to one of its windows that always asks.
    pub fn begin_with(&self, cancel: CancellationToken, always_ask_approved: bool) -> Op {
        Op {
            inner: self.inner.clone(),
            gate: Gate::new(self.inner.epoch.clone()),
            cancel,
            always_ask_approved,
            keys: Vec::new(),
            buttons: Vec::new(),
        }
    }

    /// What the guard currently holds.
    pub fn held(&self) -> Held {
        self.inner.held().clone()
    }

    pub fn injector(&self) -> &Arc<dyn Injector> {
        &self.inner.injector
    }

    pub fn self_process(&self) -> &SelfProcess {
        &self.inner.me
    }

    /// Releases everything now and waits for it. For shutdown (`RunEvent::Exit`, which
    /// runs on the main thread: the main-thread hop runs inline there). Do not call it on
    /// the main thread while an operation may be waiting for the main thread; use
    /// [`StopHandle::release_all`] for that.
    pub fn shutdown(&self) {
        self.inner.release_everything();
    }
}

impl Drop for InputGuard {
    fn drop(&mut self) {
        self.inner.release_everything();
    }
}

/// Releases everything the app holds. Cloneable and cheap.
#[derive(Clone)]
pub struct StopHandle {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for StopHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StopHandle").finish_non_exhaustive()
    }
}

impl StopHandle {
    /// Stop: closes every operation's gate at once and releases every key and button on a
    /// background thread. Never blocks, so it is safe to call from the main thread (the
    /// global-shortcut handler) even while an operation waits for the main thread.
    pub fn release_all(&self) {
        self.inner.epoch.fetch_add(1, Ordering::SeqCst);
        let inner = self.inner.clone();
        let spawned = std::thread::Builder::new()
            .name("computer-release".into())
            .spawn(move || inner.release_everything());
        if spawned.is_err() {
            self.inner.release_everything();
        }
    }

    /// Stop, waiting until the releases were sent. Not on the main thread (see
    /// [`InputGuard::shutdown`]).
    pub fn release_all_and_wait(&self) {
        self.inner.release_everything();
    }
}

/// The application(s) an event may reach.
#[derive(Debug, Clone, Copy)]
pub enum Expect<'a> {
    /// Exactly this application (its [`crate::apps::AppInfo::id`]).
    App(&'a str),
    /// One of these.
    AnyOf(&'a [String]),
    /// Any application but this app (the middle of a drag).
    NotSelf,
}

impl Expect<'_> {
    fn allows(&self, id: &str) -> bool {
        match self {
            Expect::App(a) => *a == id,
            Expect::AnyOf(list) => list.iter().any(|a| a == id),
            Expect::NotSelf => true,
        }
    }

    fn describe(&self) -> String {
        match self {
            Expect::App(a) => policy::sanitize::one_line(a, 160),
            Expect::AnyOf(list) => policy::sanitize::one_line(&list.join(" or "), 200),
            Expect::NotSelf => "another application".into(),
        }
    }
}

/// One tool call's worth of events. Dropping it releases whatever it still holds; a
/// panic while it is alive releases everything.
pub struct Op {
    inner: Arc<Inner>,
    gate: Gate,
    cancel: CancellationToken,
    /// See [`InputGuard::begin_with`].
    always_ask_approved: bool,
    keys: Vec<Key>,
    buttons: Vec<MouseButton>,
}

/// Longest wait between two checks of Stop while an operation pauses.
const PAUSE_STEP: Duration = Duration::from_millis(10);

impl Op {
    /// Fails once Stop was pressed; a cancelled call releases everything itself, so Stop
    /// works even when nobody awaits the call any more.
    fn live(&self) -> Result<(), OpError> {
        if self.cancel.is_cancelled() {
            self.inner.release_everything();
            return Err(OpError::Stopped);
        }
        if !self.gate.is_open() {
            return Err(OpError::Stopped);
        }
        Ok(())
    }

    /// `keyboard`: the event goes to the window with the keyboard focus. `typing`: it
    /// types (a key other than a lone modifier, or text).
    fn check(
        &self,
        target: &Target,
        expect: Expect<'_>,
        keyboard: bool,
        typing: bool,
    ) -> Result<(), OpError> {
        if let Some(r) = refuse_target(&self.inner.me, target, keyboard) {
            return Err(OpError::Refused(r));
        }
        if !expect.allows(&target.app.id) {
            return Err(OpError::Refused(Refusal::OtherApp {
                approved: expect.describe(),
                now: target.app.display(),
            }));
        }
        // The same application can own a window that always asks (a console window of
        // `python.exe`) and one that does not; what was approved without the always-ask
        // card must not reach the former. The middle of a drag delivers no click.
        if !self.always_ask_approved
            && !matches!(expect, Expect::NotSelf)
            && let Some(why) = target_always_ask_reason(target, typing)
        {
            return Err(OpError::Refused(Refusal::NeedsApproval {
                why: why.to_string(),
            }));
        }
        Ok(())
    }

    /// The window a pointer event at `p` would reach, checked against the rules.
    pub fn check_pointer(&self, p: GlobalPoint, expect: Expect<'_>) -> Result<Target, OpError> {
        if let Some(r) = refuse_point(&self.inner.me, p) {
            return Err(OpError::Refused(r));
        }
        let t = self.inner.injector.target_at(p)?;
        self.check(&t, expect, false, false)?;
        Ok(t)
    }

    /// The window keyboard input would reach, checked against the rules (for typing).
    pub fn check_keyboard(&self, expect: Expect<'_>) -> Result<Target, OpError> {
        self.check_keyboard_for(expect, true)
    }

    fn check_keyboard_for(&self, expect: Expect<'_>, typing: bool) -> Result<Target, OpError> {
        let t = self.inner.injector.keyboard_target()?;
        self.check(&t, expect, true, typing)?;
        Ok(t)
    }

    fn send(&self, event: InputEvent) -> Result<(), OpError> {
        self.live()?;
        self.inner.injector.send(&event, &self.gate)?;
        Ok(())
    }

    pub fn move_to(&mut self, p: GlobalPoint, expect: Expect<'_>) -> Result<(), OpError> {
        self.live()?;
        self.check_pointer(p, expect)?;
        self.send(InputEvent::Move { to: p })
    }

    /// Presses a button at `at` (the pointer must already be there).
    pub fn button_down(
        &mut self,
        button: MouseButton,
        at: GlobalPoint,
        expect: Expect<'_>,
    ) -> Result<(), OpError> {
        self.live()?;
        self.check_pointer(at, expect)?;
        // Recorded before it is sent, so a Stop in between still releases it.
        self.buttons.push(button);
        self.inner.held().buttons.push(button);
        self.send(InputEvent::Button { button, down: true })
    }

    /// Releases a button. Always sent.
    pub fn button_up(&mut self, button: MouseButton) -> Result<(), OpError> {
        let r = self.inner.injector.send(
            &InputEvent::Button {
                button,
                down: false,
            },
            &Gate::always(),
        );
        forget(&mut self.buttons, button);
        forget(&mut self.inner.held().buttons, button);
        r.map_err(OpError::from)
    }

    pub fn scroll(
        &mut self,
        at: GlobalPoint,
        dx: i32,
        dy: i32,
        expect: Expect<'_>,
    ) -> Result<(), OpError> {
        self.live()?;
        self.check_pointer(at, expect)?;
        self.send(InputEvent::Scroll { dx, dy })
    }

    pub fn key_down(&mut self, key: Key, expect: Expect<'_>) -> Result<(), OpError> {
        self.live()?;
        self.check_keyboard_for(expect, !key.is_modifier())?;
        self.keys.push(key);
        self.inner.held().keys.push(key);
        self.send(InputEvent::Key { key, down: true })
    }

    /// Releases a key. Always sent.
    pub fn key_up(&mut self, key: Key) -> Result<(), OpError> {
        let r = self
            .inner
            .injector
            .send(&InputEvent::Key { key, down: false }, &Gate::always());
        forget(&mut self.keys, key);
        forget(&mut self.inner.held().keys, key);
        r.map_err(OpError::from)
    }

    pub fn text(&mut self, text: &str, expect: Expect<'_>) -> Result<(), OpError> {
        self.live()?;
        self.check_keyboard(expect)?;
        self.send(InputEvent::Text {
            text: text.to_string(),
        })
    }

    /// Waits, checking for Stop every few milliseconds.
    pub fn pause(&self, d: Duration) -> Result<(), OpError> {
        let end = Instant::now() + d;
        loop {
            self.live()?;
            let now = Instant::now();
            if now >= end {
                return Ok(());
            }
            std::thread::sleep((end - now).min(PAUSE_STEP));
        }
    }

    /// Keys and buttons this operation still holds.
    pub fn holding(&self) -> (usize, usize) {
        (self.keys.len(), self.buttons.len())
    }
}

fn forget<T: PartialEq>(v: &mut Vec<T>, x: T) {
    if let Some(i) = v.iter().rposition(|y| *y == x) {
        v.remove(i);
    }
}

impl Drop for Op {
    fn drop(&mut self) {
        let panicking = std::thread::panicking();
        for b in std::mem::take(&mut self.buttons).into_iter().rev() {
            self.inner.send_release(InputEvent::Button {
                button: b,
                down: false,
            });
            forget(&mut self.inner.held().buttons, b);
        }
        for k in std::mem::take(&mut self.keys).into_iter().rev() {
            self.inner.send_release(InputEvent::Key {
                key: k,
                down: false,
            });
            forget(&mut self.inner.held().keys, k);
        }
        if panicking {
            self.inner.release_everything();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::AppInfo;
    use crate::geometry::{MonitorInfo, Rect};
    use crate::platform::fake::{FakeDesktop, FakeWindow, Fault};

    const ME: u32 = 1000;
    const WEBVIEW: u32 = 1001;

    fn app(name: &str) -> AppInfo {
        AppInfo::from_windows_path(&format!(r"C:\Apps\{name}.exe"))
    }

    fn desktop() -> FakeDesktop {
        let d = FakeDesktop::new(vec![MonitorInfo {
            id: 1,
            name: "primary".into(),
            bounds: Rect::new(0.0, 0.0, 1000.0, 800.0),
            pixels_per_unit: 1.0,
            ui_scale: 1.0,
            primary: true,
        }]);
        // Notepad behind, this app's window in front on the right half.
        d.push_front(FakeWindow::new(
            1,
            Rect::new(0.0, 0.0, 1000.0, 800.0),
            50,
            app("notepad"),
        ));
        let mut mine = FakeWindow::new(
            2,
            Rect::new(500.0, 0.0, 500.0, 800.0),
            WEBVIEW,
            app("harniss"),
        );
        mine.root_pid = Some(ME);
        d.push_front(mine);
        d
    }

    fn guard(d: &FakeDesktop) -> InputGuard {
        InputGuard::new(d.injector(), SelfProcess::new(ME, || vec![WEBVIEW]))
    }

    fn notepad() -> String {
        app("notepad").id
    }

    fn p(x: f64, y: f64) -> GlobalPoint {
        GlobalPoint::new(x, y)
    }

    #[test]
    fn pointer_input_to_this_app_is_refused_and_nothing_is_sent() {
        let d = desktop();
        let g = guard(&d);
        let mut op = g.begin(CancellationToken::new());
        let id = notepad();
        // Over this app's window (owned by the webview child, rooted in the app).
        let e = op.move_to(p(600.0, 100.0), Expect::App(&id)).unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::SelfPointer));
        let e = op
            .button_down(MouseButton::Left, p(600.0, 100.0), Expect::NotSelf)
            .unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::SelfPointer));
        assert!(d.events().is_empty());
        // Over Notepad it goes through.
        op.move_to(p(100.0, 100.0), Expect::App(&id)).unwrap();
        assert_eq!(d.events().len(), 1);
    }

    #[test]
    fn a_window_whose_only_link_to_this_app_is_its_root_is_still_refused() {
        let d = desktop();
        // A window owned by an unrelated pid whose top-level window is this app's.
        let mut w = FakeWindow::new(3, Rect::new(0.0, 0.0, 100.0, 100.0), 77, app("other"));
        w.root_pid = Some(ME);
        d.push_front(w);
        let g = guard(&d);
        let op = g.begin(CancellationToken::new());
        assert_eq!(
            op.check_pointer(p(10.0, 10.0), Expect::NotSelf)
                .unwrap_err(),
            OpError::Refused(Refusal::SelfPointer)
        );
    }

    #[test]
    fn keyboard_input_to_this_app_is_refused() {
        let d = desktop();
        let g = guard(&d);
        let mut op = g.begin(CancellationToken::new());
        // This app has the focus (it was pushed last).
        let e = op.key_down(Key::Char('a'), Expect::NotSelf).unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::SelfKeyboard));
        let e = op.text("hello", Expect::NotSelf).unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::SelfKeyboard));
        assert!(d.events().is_empty());
        assert_eq!(op.holding(), (0, 0));
        d.state().foreground = Some(1);
        op.text("hello", Expect::App(&notepad())).unwrap();
        assert_eq!(d.events().len(), 1);
    }

    #[test]
    fn the_target_application_is_rechecked_before_every_event() {
        let d = desktop();
        let g = guard(&d);
        let mut op = g.begin(CancellationToken::new());
        let id = notepad();
        op.move_to(p(100.0, 100.0), Expect::App(&id)).unwrap();
        // A different application's window appears under the pointer.
        d.push_front(FakeWindow::new(
            9,
            Rect::new(0.0, 0.0, 400.0, 400.0),
            60,
            app("mail"),
        ));
        let e = op
            .button_down(MouseButton::Left, p(100.0, 100.0), Expect::App(&id))
            .unwrap_err();
        match e {
            OpError::Refused(Refusal::OtherApp { approved, now }) => {
                assert!(approved.contains("notepad"));
                assert!(now.contains("mail"));
            }
            other => panic!("{other:?}"),
        }
        // Only the move went out; no button was pressed.
        assert_eq!(d.events().len(), 1);
        assert!(d.state().buttons_down.is_empty());
        // Keyboard: the focus moved to the mail window.
        let e = op.key_down(Key::Enter, Expect::App(&id)).unwrap_err();
        assert!(matches!(e, OpError::Refused(Refusal::OtherApp { .. })));
    }

    #[test]
    fn elevated_and_protected_targets_are_refused() {
        let d = desktop();
        let mut admin = FakeWindow::new(5, Rect::new(0.0, 0.0, 200.0, 200.0), 70, app("taskmgr"));
        admin.elevated = true;
        d.push_front(admin);
        let me = SelfProcess::new(ME, || vec![WEBVIEW])
            .with_protected_rects(|| vec![Rect::new(300.0, 300.0, 50.0, 20.0)]);
        let g = InputGuard::new(d.injector(), me);
        let mut op = g.begin(CancellationToken::new());
        let e = op.move_to(p(10.0, 10.0), Expect::NotSelf).unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::Elevated));
        assert_eq!(
            e.to_string(),
            "that window is running as administrator and can't be controlled"
        );
        let e = op.move_to(p(310.0, 305.0), Expect::NotSelf).unwrap_err();
        assert_eq!(e, OpError::Refused(Refusal::Protected));
        // Keyboard into the elevated window.
        assert_eq!(
            op.key_down(Key::Char('x'), Expect::NotSelf).unwrap_err(),
            OpError::Refused(Refusal::Elevated)
        );
        assert!(d.events().is_empty());
    }

    #[test]
    fn targets_that_always_ask_need_an_always_ask_approval() {
        let d = desktop();
        // A console window of an ordinary program (Windows reports it as the program's).
        let mut console = FakeWindow::new(6, Rect::new(0.0, 0.0, 300.0, 300.0), 71, app("python"));
        console.console = true;
        d.push_front(console);
        let g = guard(&d);
        let id = app("python").id;
        let mut op = g.begin(CancellationToken::new());
        assert!(matches!(
            op.move_to(p(10.0, 10.0), Expect::App(&id)),
            Err(OpError::Refused(Refusal::NeedsApproval { .. }))
        ));
        assert!(matches!(
            op.text("import os", Expect::App(&id)),
            Err(OpError::Refused(Refusal::NeedsApproval { .. }))
        ));
        // Passing over it in the middle of a drag delivers no click and is allowed.
        op.check_pointer(p(10.0, 10.0), Expect::NotSelf).unwrap();
        assert!(d.events().is_empty());
        drop(op);
        // Approved on an always-ask card, the same events go out.
        let mut op = g.begin_with(CancellationToken::new(), true);
        op.move_to(p(10.0, 10.0), Expect::App(&id)).unwrap();
        op.text("import os", Expect::App(&id)).unwrap();
        assert_eq!(d.events().len(), 2);
    }

    #[test]
    fn modifiers_but_not_typing_reach_the_windows_shell_without_an_always_ask_approval() {
        let d = desktop();
        let explorer = AppInfo::from_windows_path(r"C:\Windows\explorer.exe");
        d.push_front(FakeWindow::new(
            6,
            Rect::new(0.0, 0.0, 300.0, 300.0),
            72,
            explorer.clone(),
        ));
        let g = guard(&d);
        let mut op = g.begin(CancellationToken::new());
        // Ctrl held for a Ctrl-click goes through; a typed key does not.
        op.key_down(Key::Control, Expect::App(&explorer.id))
            .unwrap();
        op.key_up(Key::Control).unwrap();
        assert!(matches!(
            op.key_down(Key::Enter, Expect::App(&explorer.id)),
            Err(OpError::Refused(Refusal::NeedsApproval { .. }))
        ));
        assert!(matches!(
            op.text("cmd", Expect::App(&explorer.id)),
            Err(OpError::Refused(Refusal::NeedsApproval { .. }))
        ));
        assert!(d.state().keys_down.is_empty());
    }

    #[test]
    fn dropping_an_operation_releases_what_it_holds() {
        let d = desktop();
        d.state().foreground = Some(1);
        let g = guard(&d);
        {
            let mut op = g.begin(CancellationToken::new());
            op.key_down(Key::Control, Expect::App(&notepad())).unwrap();
            op.key_down(Key::Char('c'), Expect::App(&notepad()))
                .unwrap();
            op.move_to(p(100.0, 100.0), Expect::NotSelf).unwrap();
            op.button_down(MouseButton::Left, p(100.0, 100.0), Expect::NotSelf)
                .unwrap();
            assert_eq!(g.held().keys, vec![Key::Control, Key::Char('c')]);
            assert_eq!(op.holding(), (2, 1));
        }
        let s = d.state();
        assert!(s.keys_down.is_empty(), "{:?}", s.keys_down);
        assert!(s.buttons_down.is_empty());
        // Released in reverse order: button first, then 'c', then Ctrl.
        let tail: Vec<_> = s.events[s.events.len() - 3..].to_vec();
        assert_eq!(
            tail,
            vec![
                InputEvent::Button {
                    button: MouseButton::Left,
                    down: false
                },
                InputEvent::Key {
                    key: Key::Char('c'),
                    down: false
                },
                InputEvent::Key {
                    key: Key::Control,
                    down: false
                },
            ]
        );
        drop(s);
        assert_eq!(g.held(), Held::default());
    }

    #[test]
    fn a_panic_mid_operation_releases_everything() {
        let d = desktop();
        d.state().foreground = Some(1);
        let g = guard(&d);
        // Event 0: Shift down, event 1: 'a' down -> panic.
        d.state().fault = Some((1, Fault::Panic));
        let r = catch_unwind(AssertUnwindSafe(|| {
            let mut op = g.begin(CancellationToken::new());
            op.key_down(Key::Shift, Expect::App(&notepad())).unwrap();
            let _ = op.key_down(Key::Char('a'), Expect::App(&notepad()));
        }));
        assert!(r.is_err());
        let s = d.state();
        assert!(s.keys_down.is_empty(), "{:?}", s.keys_down);
        assert!(s.events.contains(&InputEvent::Key {
            key: Key::Shift,
            down: false
        }));
        drop(s);
        assert_eq!(g.held(), Held::default());
    }

    #[test]
    fn cancelling_the_call_releases_everything_and_stops_further_events() {
        let d = desktop();
        d.state().foreground = Some(1);
        let g = guard(&d);
        let cancel = CancellationToken::new();
        let mut op = g.begin(cancel.clone());
        op.key_down(Key::Alt, Expect::App(&notepad())).unwrap();
        op.move_to(p(100.0, 100.0), Expect::NotSelf).unwrap();
        op.button_down(MouseButton::Left, p(100.0, 100.0), Expect::NotSelf)
            .unwrap();
        cancel.cancel();
        // The next step notices, releases everything and refuses.
        assert_eq!(op.pause(Duration::from_millis(50)), Err(OpError::Stopped));
        {
            let s = d.state();
            assert!(s.keys_down.is_empty());
            assert!(s.buttons_down.is_empty());
        }
        let before = d.events().len();
        assert_eq!(
            op.move_to(p(110.0, 100.0), Expect::NotSelf),
            Err(OpError::Stopped)
        );
        assert_eq!(d.events().len(), before);
        drop(op);
        assert!(d.state().keys_down.is_empty());
    }

    #[test]
    fn release_all_from_another_thread_closes_running_operations() {
        let d = desktop();
        d.state().foreground = Some(1);
        let g = guard(&d);
        let stop = g.stop_handle();
        let mut op = g.begin(CancellationToken::new());
        op.key_down(Key::Control, Expect::App(&notepad())).unwrap();
        stop.release_all_and_wait();
        assert!(d.state().keys_down.is_empty());
        assert_eq!(g.held(), Held::default());
        // The operation's gate is closed: nothing more goes out, even a key press that
        // passed its checks.
        assert_eq!(
            op.key_down(Key::Char('v'), Expect::App(&notepad())),
            Err(OpError::Stopped)
        );
        assert!(d.state().keys_down.is_empty());
        drop(op);
        // A new operation works again.
        let mut op = g.begin(CancellationToken::new());
        op.key_down(Key::Char('v'), Expect::App(&notepad()))
            .unwrap();
        op.key_up(Key::Char('v')).unwrap();
    }

    #[test]
    fn an_event_queued_before_stop_never_lands_after_it() {
        let d = desktop();
        d.state().foreground = Some(1);
        // Every send takes 30 ms, so Stop arrives while the key press is in flight.
        d.state().send_delay = Some(Duration::from_millis(30));
        let g = guard(&d);
        let stop = g.stop_handle();
        let mut op = g.begin(CancellationToken::new());
        let id = notepad();
        let t = std::thread::spawn(move || {
            let _ = op.key_down(Key::Shift, Expect::App(&id));
            op
        });
        std::thread::sleep(Duration::from_millis(10));
        stop.release_all();
        let op = t.join().unwrap();
        drop(op);
        // Wait for the background release thread.
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            if d.state().keys_down.is_empty() && g.held() == Held::default() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let s = d.state();
        assert!(s.keys_down.is_empty(), "{:?}", s.keys_down);
        // Either the press was refused at the gate, or it went out and was released.
        let downs = s
            .events
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    InputEvent::Key {
                        key: Key::Shift,
                        down: true
                    }
                )
            })
            .count();
        assert!(
            s.gated == 1 || downs == 1,
            "gated {} downs {downs}",
            s.gated
        );
    }

    #[test]
    fn stop_releases_unrecorded_modifiers_and_buttons_only_when_down() {
        let d = desktop();
        let g = guard(&d);
        // Something left Ctrl and the right button down outside the guard's records.
        {
            let mut s = d.state();
            s.keys_down.insert(Key::Control);
            s.buttons_down.insert(MouseButton::Right);
        }
        g.stop_handle().release_all_and_wait();
        let ev = d.events();
        assert_eq!(
            ev,
            vec![
                InputEvent::Button {
                    button: MouseButton::Right,
                    down: false
                },
                InputEvent::Key {
                    key: Key::Control,
                    down: false
                },
            ]
        );
        // A platform that cannot report state gets every release.
        d.clear_events();
        d.state().reports_state = false;
        g.shutdown();
        assert_eq!(
            d.events().len(),
            MouseButton::ALL.len() + Key::MODIFIERS.len()
        );
    }

    #[test]
    fn dropping_the_guard_releases_everything() {
        let d = desktop();
        d.state().foreground = Some(1);
        let g = guard(&d);
        let mut op = g.begin(CancellationToken::new());
        op.key_down(Key::Meta, Expect::App(&notepad())).unwrap();
        // Leak the operation's own guard so only the InputGuard's drop can release.
        std::mem::forget(op);
        assert!(d.state().keys_down.contains(&Key::Meta));
        drop(g);
        assert!(d.state().keys_down.is_empty());
    }

    #[test]
    fn a_failed_press_is_still_released() {
        let d = desktop();
        d.state().foreground = Some(1);
        d.state().fault = Some((0, Fault::Fail));
        let g = guard(&d);
        {
            let mut op = g.begin(CancellationToken::new());
            let e = op
                .key_down(Key::Char('z'), Expect::App(&notepad()))
                .unwrap_err();
            assert!(matches!(e, OpError::Input(InputError::Platform(_))));
        }
        // The drop guard sent the release anyway (harmless if the press never landed).
        assert!(d.events().contains(&InputEvent::Key {
            key: Key::Char('z'),
            down: false
        }));
        assert_eq!(g.held(), Held::default());
    }
}
