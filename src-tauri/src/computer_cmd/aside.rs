//! The main window steps aside while computer use acts (ARCHITECTURE.md 9.3, "Stepping
//! aside"): the window calls for [`computer::WindowAside`], which makes the decisions.
//!
//! Before an approved input action the main window is minimised when it is on screen. It
//! comes back (restored and focused) when an approval card of that chat needs the user,
//! when that chat's turn ends and on Stop, if the app minimised it and it is still
//! minimised. The red border and the Stop window are windows of their own and stay.
//!
//! The decisions are made under a lock that is never held across a window call: those wait
//! for the main thread, which may itself want the lock (the stop hotkey runs there). A
//! minimise and a restore run one at a time, on blocking threads, under a second lock that
//! the main thread never takes, so a restore for a card answered at once cannot land in the
//! middle of the minimise that follows it.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use computer::{AsideEvent, WindowAside, WindowNow, WindowStep};
use tauri::{AppHandle, Manager, WebviewWindow};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// How long the app waits for its window to go or to come back (both are animated).
const WAIT: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(25);
/// After the window reports minimised, while the system may still be animating it away.
const SETTLE: Duration = Duration::from_millis(100);

static ASIDE: Mutex<WindowAside> = Mutex::new(WindowAside::new());
/// Held for a whole minimise or restore. Never taken on the main thread.
static WINDOW_OPS: Mutex<()> = Mutex::new(());

fn aside() -> MutexGuard<'static, WindowAside> {
    ASIDE.lock().unwrap_or_else(|e| e.into_inner())
}

fn one_at_a_time() -> MutexGuard<'static, ()> {
    WINDOW_OPS.lock().unwrap_or_else(|e| e.into_inner())
}

fn main_window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(crate::MAIN_WINDOW)
}

fn now(w: &WebviewWindow) -> WindowNow {
    WindowNow {
        visible: w.is_visible().unwrap_or(false),
        minimised: w.is_minimized().unwrap_or(false),
    }
}

/// Waits until the window is (or is no longer) minimised, at most [`WAIT`].
fn wait_until_minimised(w: &WebviewWindow, minimised: bool) -> bool {
    let start = Instant::now();
    loop {
        if w.is_minimized().is_ok_and(|m| m == minimised) {
            return true;
        }
        if start.elapsed() >= WAIT {
            return false;
        }
        std::thread::sleep(POLL);
    }
}

/// Before an approved input action, on a blocking thread (the computer server's hook):
/// minimises the main window when it is on screen and waits until it is gone, at most
/// about a second. A call stopped meanwhile (while a restore held the window) does not
/// minimise it: its turn is ending and nothing would bring the window back.
pub(super) fn before_input(app: &AppHandle, chat_id: i64, cancel: &CancellationToken) {
    let Some(w) = main_window(app) else { return };
    let _one = one_at_a_time();
    if cancel.is_cancelled() {
        return;
    }
    let seen = now(&w);
    let step = aside().decide(AsideEvent::InputApproved(chat_id), seen);
    if step != WindowStep::Minimise {
        return;
    }
    if w.minimize().is_ok() && wait_until_minimised(&w, true) {
        std::thread::sleep(SETTLE);
    }
    aside().minimise_finished();
}

/// Brings the main window back if `event` calls for it, on a blocking thread.
fn bring_back_if_needed(app: &AppHandle, event: AsideEvent) {
    // Nothing to do without computer use, or for a chat that minimised nothing.
    let has_server = app
        .try_state::<super::ComputerHandle>()
        .is_some_and(|h| h.0.is_some());
    if !has_server || !aside().concerns(event) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let Some(w) = main_window(&app) else { return };
        let _one = one_at_a_time();
        let seen = now(&w);
        if aside().decide(event, seen) != WindowStep::Restore {
            return;
        }
        let _ = w.unminimize();
        // Focusing a window that is still coming back does nothing.
        wait_until_minimised(&w, false);
        let _ = w.set_focus();
        #[cfg(target_os = "macos")]
        order_front(&app, &w);
    });
}

/// macOS 14 and later may not let the app activate itself while another application is
/// in front: the window is put in front of the others anyway, so the user sees it.
#[cfg(target_os = "macos")]
fn order_front(app: &AppHandle, w: &WebviewWindow) {
    let Ok(ptr) = w.ns_window() else { return };
    let addr = ptr as usize;
    let _ = app.run_on_main_thread(move || {
        // SAFETY: the main window is one of this app's live NSWindows; this runs on the
        // main thread.
        if let Err(e) = unsafe { computer::order_window_front(addr as *mut std::ffi::c_void) } {
            tracing::warn!("main window to the front: {e}");
        }
    });
}

/// An approval card of this chat waits for the user.
pub fn approval_needed(app: &AppHandle, chat_id: i64) {
    bring_back_if_needed(app, AsideEvent::ApprovalNeeded(chat_id));
}

/// This chat's turn ended: done, failed or stopped.
pub fn turn_ended(app: &AppHandle, chat_id: i64) {
    bring_back_if_needed(app, AsideEvent::TurnEnded(chat_id));
}

/// The user pressed Stop: the stop hotkey or the indicator's Stop button.
pub fn stopped(app: &AppHandle) {
    bring_back_if_needed(app, AsideEvent::Stopped);
}

/// The "Minimise this window while the agent works" button of a tool card. For a chat
/// whose turn is running it is the same step aside as before input, so the window comes
/// back with that chat's next card, the end of its turn or Stop; otherwise a plain
/// minimise, which the app never undoes.
pub fn minimise_on_request(app: &AppHandle, chat_id: Option<i64>) {
    let running = chat_id.filter(|&chat| {
        app.try_state::<AppState>()
            .is_some_and(|s| s.generation.running_in(chat))
    });
    match running {
        Some(chat) => {
            let app = app.clone();
            tauri::async_runtime::spawn_blocking(move || {
                before_input(&app, chat, &CancellationToken::new())
            });
        }
        None => {
            if let Some(w) = main_window(app) {
                let _ = w.minimize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// The window comes back wherever the user is needed: every approval card the app
    /// raises (in a turn and during a call), the end of every turn, and Stop. The decisions
    /// themselves are tested in `computer::window_aside`.
    #[test]
    fn the_window_is_brought_back_wherever_the_user_is_needed() {
        let turns = include_str!("../turns.rs");
        let cards = turns.matches("\"approval-request\"").count();
        assert!(cards >= 2, "{cards}");
        assert_eq!(
            turns
                .matches("crate::computer_cmd::approval_needed(")
                .count(),
            cards,
            "every approval card brings the window back"
        );
        assert_eq!(
            turns
                .matches("crate::computer_cmd::turn_ended(app, chat_id.0);")
                .count(),
            1
        );
        let hotkey = include_str!("../hotkey.rs");
        assert!(hotkey.contains("crate::computer_cmd::stopped(app);"));
    }
}
