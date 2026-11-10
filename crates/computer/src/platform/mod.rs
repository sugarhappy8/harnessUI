//! The platform boundary (ARCHITECTURE.md 9.3): two small traits, [`Capture`] and
//! [`Injector`], implemented natively on Windows and macOS only, plus [`fake`] for tests
//! on every OS.
//!
//! The traits are deliberately thin. Every safety rule (never this app, the granted
//! application only, held keys recorded and released on Stop) lives above them, in
//! [`crate::guard`], so it is the same code on both platforms and is unit-tested on Linux.
//!
//! **Main thread.** enigo must only be called on the app's main thread (the released
//! version crashes on macOS otherwise). The app implements [`MainThread`] with Tauri's
//! `run_on_main_thread`, and the native injectors route every enigo call through it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::apps::Target;
use crate::geometry::{GlobalPoint, MonitorInfo, Rect};
use crate::keys::Key;

pub mod fake;
#[cfg(any(test, target_os = "macos"))]
mod hit;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "windows")]
pub mod windows;

/// Runs closures on the app's main thread.
///
/// The app implements it with `AppHandle::run_on_main_thread`. Requirement: when `run`
/// is called *on* the main thread it runs `f` before returning (Tauri does), so a release
/// at shutdown, which happens on the main thread, cannot deadlock.
pub trait MainThread: Send + Sync {
    fn run(&self, f: Box<dyn FnOnce() + Send>);
}

/// How long a main-thread call may take before the input is abandoned. Short, because a
/// held key or button waits on it (holds are capped at 2 s, 4.2).
pub const MAIN_THREAD_TIMEOUT: Duration = Duration::from_secs(2);

/// Runs `f` on the main thread and waits for its result.
///
/// When the main thread does not start `f` within `timeout`, `f` is abandoned: it will
/// never run, even if the main thread gets to it later, and [`InputError::Timeout`]
/// therefore means "not sent". Without this, a key press or click queued behind a stalled
/// main thread would land seconds later, after the tool reported failure and without a
/// fresh check of the window it reaches. Releases use [`run_on_main_even_late`] instead.
pub fn run_on_main<T: Send + 'static>(
    main: &dyn MainThread,
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, InputError> {
    const PENDING: u8 = 0;
    const RUNNING: u8 = 1;
    const ABANDONED: u8 = 2;
    let state = Arc::new(AtomicU8::new(PENDING));
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let claim = state.clone();
    main.run(Box::new(move || {
        if claim
            .compare_exchange(PENDING, RUNNING, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let _ = tx.send(f());
        }
    }));
    match rx.recv_timeout(timeout) {
        Ok(v) => Ok(v),
        Err(_) => {
            if state
                .compare_exchange(PENDING, ABANDONED, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                return Err(InputError::Timeout);
            }
            // `f` started just as the wait ended: it is one short platform call, so wait
            // for it (bounded) rather than report "not sent" for something that was.
            rx.recv_timeout(timeout).map_err(|_| InputError::Timeout)
        }
    }
}

/// Like [`run_on_main`], but `f` still runs when the main thread gets to it after the
/// timeout. For releases only: a release that lands late is better than a key or button
/// left held.
pub fn run_on_main_even_late<T: Send + 'static>(
    main: &dyn MainThread,
    timeout: Duration,
    f: impl FnOnce() -> T + Send + 'static,
) -> Result<T, InputError> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    main.run(Box::new(move || {
        let _ = tx.send(f());
    }));
    rx.recv_timeout(timeout).map_err(|_| InputError::Timeout)
}

/// One capture.
#[derive(Debug, Clone)]
pub struct Captured {
    pub width: u32,
    pub height: u32,
    /// `width × height × 4` bytes, RGBA, row-major, no padding.
    pub rgba: Vec<u8>,
    /// This app's windows were left out of the capture.
    pub self_excluded: bool,
    /// A limitation to tell the model and the card about, if any.
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CaptureError {
    /// Screen Recording is not granted (macOS).
    #[error("{0}")]
    NoPermission(String),
    #[error("that monitor is no longer connected")]
    NoMonitor,
    #[error("the screen could not be captured: {0}")]
    Platform(String),
}

/// Screen capture.
pub trait Capture: Send + Sync {
    /// Connected monitors, in any order.
    fn monitors(&self) -> Result<Vec<MonitorInfo>, CaptureError>;
    /// Captures one whole monitor, leaving this app's windows out where the platform
    /// allows (say so in [`Captured::self_excluded`] / [`Captured::note`]).
    fn capture(&self, monitor: &MonitorInfo) -> Result<Captured, CaptureError>;
    /// The frame of the window in front, to pick the default monitor.
    fn foreground_window_rect(&self) -> Option<Rect>;
    /// Where the pointer is (for the cursor marker).
    fn cursor_position(&self) -> Option<GlobalPoint>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub const ALL: [MouseButton; 3] = [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

    pub fn label(self) -> &'static str {
        match self {
            MouseButton::Left => "left",
            MouseButton::Right => "right",
            MouseButton::Middle => "middle",
        }
    }
}

/// One input event, in global input coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum InputEvent {
    /// Moves the pointer (a drag while a button is held).
    Move {
        to: GlobalPoint,
    },
    Button {
        button: MouseButton,
        down: bool,
    },
    /// Wheel notches at the pointer: positive `dy` scrolls down, positive `dx` right.
    Scroll {
        dx: i32,
        dy: i32,
    },
    Key {
        key: Key,
        down: bool,
    },
    /// Text typed as characters (no modifiers).
    Text {
        text: String,
    },
}

impl InputEvent {
    /// A release: it always goes out, whatever window is under the pointer or in front,
    /// so nothing stays held.
    pub fn is_release(&self) -> bool {
        matches!(
            self,
            InputEvent::Button { down: false, .. } | InputEvent::Key { down: false, .. }
        )
    }
}

/// Lets an event out only while the operation that queued it has not been stopped.
///
/// Stop bumps a shared epoch; every gate created before that is closed for good. Native
/// injectors check the gate *inside* the same serialised step that sends the event (the
/// main-thread closure, or under the injector's send lock), so an event queued before
/// Stop can never land after Stop's releases.
#[derive(Debug, Clone)]
pub struct Gate {
    epoch: Arc<AtomicU64>,
    expected: u64,
}

impl Gate {
    const ALWAYS: u64 = u64::MAX;

    pub(crate) fn new(epoch: Arc<AtomicU64>) -> Self {
        let expected = epoch.load(Ordering::SeqCst);
        Self { epoch, expected }
    }

    /// A gate that never closes (for releases).
    pub fn always() -> Self {
        Self {
            epoch: Arc::new(AtomicU64::new(0)),
            expected: Self::ALWAYS,
        }
    }

    pub fn is_open(&self) -> bool {
        self.expected == Self::ALWAYS || self.epoch.load(Ordering::SeqCst) == self.expected
    }
}

/// A key or button whose OS state can be queried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pressable {
    Key(Key),
    Button(MouseButton),
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum InputError {
    /// Accessibility is not granted (macOS) or input cannot be simulated.
    #[error("{0}")]
    NoPermission(String),
    /// The operation was stopped; nothing was sent.
    #[error("stopped by the user")]
    Stopped,
    #[error("the app's main thread did not respond in time; the input was not sent")]
    Timeout,
    #[error("{0}")]
    NoTarget(String),
    #[error("input could not be sent: {0}")]
    Platform(String),
}

/// Input injection. Implementations only look up targets and send events; the rules are
/// in [`crate::guard`].
pub trait Injector: Send + Sync {
    /// The window a pointer event at `p` would reach.
    fn target_at(&self, p: GlobalPoint) -> Result<Target, InputError>;
    /// The window keyboard input would reach (the foreground window / frontmost app).
    fn keyboard_target(&self) -> Result<Target, InputError>;
    /// Sends one event if `gate` is open; [`InputError::Stopped`] otherwise. The gate check
    /// and the send must be one serialised step (see [`Gate`]).
    fn send(&self, event: &InputEvent, gate: &Gate) -> Result<(), InputError>;
    /// Whether the OS reports this key or button as down (including injected input).
    /// `None` when the platform cannot tell; Stop then releases it anyway.
    fn is_pressed(&self, _what: Pressable) -> Option<bool> {
        None
    }
    /// Key names on cards and shortcut rules follow macOS (Cmd, Option).
    fn mac_keys(&self) -> bool {
        cfg!(target_os = "macos")
    }
}

type PidsFn = Arc<dyn Fn() -> Vec<u32> + Send + Sync>;
type WindowsFn = Arc<dyn Fn() -> Vec<u64> + Send + Sync>;
type RectsFn = Arc<dyn Fn() -> Vec<Rect> + Send + Sync>;

/// This app's own process tree, which input must never reach (9.3, 13.5).
#[derive(Clone)]
pub struct SelfProcess {
    pid: u32,
    child_pids: PidsFn,
    click_through: WindowsFn,
    protected: RectsFn,
}

impl std::fmt::Debug for SelfProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelfProcess")
            .field("pid", &self.pid)
            .field("child_pids", &(self.child_pids)())
            .finish_non_exhaustive()
    }
}

impl SelfProcess {
    /// `pid` is the app's own process id; `child_pids` returns the webview's child
    /// processes (WebView2 browser and renderer processes on Windows; WebKit's helpers own
    /// no windows on macOS but may be listed too). Called before every event, so keep it
    /// cheap.
    pub fn new(pid: u32, child_pids: impl Fn() -> Vec<u32> + Send + Sync + 'static) -> Self {
        Self {
            pid,
            child_pids: Arc::new(child_pids),
            click_through: Arc::new(Vec::new),
            protected: Arc::new(Vec::new),
        }
    }

    /// The current process.
    pub fn current(child_pids: impl Fn() -> Vec<u32> + Send + Sync + 'static) -> Self {
        Self::new(std::process::id(), child_pids)
    }

    /// macOS: window numbers of this app's windows that ignore the mouse (the overlay while
    /// it is click-through). The window list cannot see `ignoresMouseEvents`, so these are
    /// skipped when looking for the window under a point. Ignored on Windows, whose hit
    /// test sees click-through windows itself.
    pub fn with_click_through_windows(
        mut self,
        f: impl Fn() -> Vec<u64> + Send + Sync + 'static,
    ) -> Self {
        self.click_through = Arc::new(f);
        self
    }

    /// Screen rectangles that never receive pointer input, in global input coordinates
    /// (the overlay's Stop button).
    pub fn with_protected_rects(
        mut self,
        f: impl Fn() -> Vec<Rect> + Send + Sync + 'static,
    ) -> Self {
        self.protected = Arc::new(f);
        self
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// The app and its children.
    pub fn pids(&self) -> Vec<u32> {
        let mut v = (self.child_pids)();
        v.push(self.pid);
        v.sort_unstable();
        v.dedup();
        v
    }

    /// True when any of `pids` is this app or one of its children.
    pub fn owns_any(&self, pids: &[u32]) -> bool {
        if pids.contains(&self.pid) {
            return true;
        }
        let children = (self.child_pids)();
        pids.iter().any(|p| children.contains(p))
    }

    pub fn click_through_windows(&self) -> Vec<u64> {
        (self.click_through)()
    }

    pub fn protected_rects(&self) -> Vec<Rect> {
        (self.protected)()
    }
}

/// A capture and an injector, as the native constructors return them.
pub type Backends = (Arc<dyn Capture>, Arc<dyn Injector>);

/// Why the native backends could not start.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct PlatformError(pub String);

/// The native capture and injector for this OS.
#[cfg(target_os = "windows")]
pub fn native(main: Arc<dyn MainThread>, me: SelfProcess) -> Result<Backends, PlatformError> {
    windows::native(main, me)
}

/// The native capture and injector for this OS.
#[cfg(target_os = "macos")]
pub fn native(main: Arc<dyn MainThread>, me: SelfProcess) -> Result<Backends, PlatformError> {
    macos::native(main, me)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Inline;
    impl MainThread for Inline {
        fn run(&self, f: Box<dyn FnOnce() + Send>) {
            f();
        }
    }

    struct Never;
    impl MainThread for Never {
        fn run(&self, _f: Box<dyn FnOnce() + Send>) {}
    }

    struct OtherThread;
    impl MainThread for OtherThread {
        fn run(&self, f: Box<dyn FnOnce() + Send>) {
            std::thread::spawn(f);
        }
    }

    #[test]
    fn run_on_main_returns_the_result_or_times_out() {
        assert_eq!(
            run_on_main(&Inline, Duration::from_millis(100), || 7).unwrap(),
            7
        );
        assert_eq!(
            run_on_main(&OtherThread, Duration::from_secs(5), || "ok").unwrap(),
            "ok"
        );
        assert_eq!(
            run_on_main(&Never, Duration::from_millis(20), || 1),
            Err(InputError::Timeout)
        );
    }

    /// A main thread that is stalled: closures queue up and run only when pumped.
    #[derive(Default)]
    struct Stalled(std::sync::Mutex<Vec<Box<dyn FnOnce() + Send>>>);
    impl MainThread for Stalled {
        fn run(&self, f: Box<dyn FnOnce() + Send>) {
            self.0.lock().unwrap().push(f);
        }
    }
    impl Stalled {
        fn pump(&self) {
            let queued: Vec<_> = std::mem::take(&mut *self.0.lock().unwrap());
            for f in queued {
                f();
            }
        }
    }

    #[test]
    fn a_timed_out_main_thread_call_never_runs_later() {
        let main = Stalled::default();
        let sent = Arc::new(AtomicU64::new(0));
        let s = sent.clone();
        let r = run_on_main(&main, Duration::from_millis(20), move || {
            s.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(r, Err(InputError::Timeout));
        // The main thread recovers: the abandoned press must not go out now.
        main.pump();
        assert_eq!(sent.load(Ordering::SeqCst), 0);
        // A release still goes out late.
        let s = sent.clone();
        let r = run_on_main_even_late(&main, Duration::from_millis(20), move || {
            s.fetch_add(10, Ordering::SeqCst);
        });
        assert_eq!(r, Err(InputError::Timeout));
        main.pump();
        assert_eq!(sent.load(Ordering::SeqCst), 10);
    }

    #[test]
    fn gates_close_when_the_epoch_moves() {
        let epoch = Arc::new(AtomicU64::new(3));
        let g = Gate::new(epoch.clone());
        assert!(g.is_open());
        epoch.fetch_add(1, Ordering::SeqCst);
        assert!(!g.is_open());
        // A gate made after Stop is open again; "always" never closes.
        assert!(Gate::new(epoch.clone()).is_open());
        assert!(Gate::always().is_open());
    }

    #[test]
    fn self_process_covers_children() {
        let me = SelfProcess::new(100, || vec![200, 201]);
        assert!(me.owns_any(&[100]));
        assert!(me.owns_any(&[5, 201]));
        assert!(!me.owns_any(&[5, 6]));
        assert!(!me.owns_any(&[]));
        assert_eq!(me.pids(), vec![100, 200, 201]);
        assert!(me.click_through_windows().is_empty());
        let me = me
            .with_click_through_windows(|| vec![42])
            .with_protected_rects(|| vec![Rect::new(0.0, 0.0, 10.0, 10.0)]);
        assert_eq!(me.click_through_windows(), vec![42]);
        assert_eq!(me.protected_rects().len(), 1);
        assert!(format!("{me:?}").contains("100"));
    }

    #[test]
    fn releases_are_recognised() {
        assert!(
            InputEvent::Key {
                key: Key::Shift,
                down: false
            }
            .is_release()
        );
        assert!(
            !InputEvent::Button {
                button: MouseButton::Left,
                down: true
            }
            .is_release()
        );
        assert!(
            !InputEvent::Move {
                to: GlobalPoint::new(0.0, 0.0)
            }
            .is_release()
        );
    }
}
