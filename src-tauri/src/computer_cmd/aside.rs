//! The main window steps aside while computer use acts (ARCHITECTURE.md 9.3, "Stepping
//! aside"). What to do and in which order is [`computer::Aside`]'s, tested there with a fake
//! window; this module reads and moves Tauri's main window for it, and tells it about turns
//! and approval cards.
//!
//! Before approved input that the main window is in the way of, the window is minimised
//! when it is on screen. It comes back, if the app minimised it and it is still minimised:
//!
//! - for an approval card of that chat: in front of the other windows without taking the
//!   keyboard (Windows: raised as always-on-top for a moment; macOS: ordered in front), and
//!   its taskbar button flashes or its Dock icon bounces;
//! - at the end of that turn: restored without taking the keyboard, and the user is told
//!   the same way;
//! - on Stop: restored and given the keyboard, since the user asked for it. A restore that
//!   wins the race against Stop's own (the end of the stopped turn) gives the keyboard too.
//!
//! The page is told just before the window comes back and again once it is on screen, and
//! every approval card and Allow all / Full access confirmation on screen starts its input
//! delay again, so a click meant for another application cannot land on it.
//!
//! The quiet restores never activate the window: on Windows `ShowWindow(SW_SHOWNOACTIVATE)`
//! ([`computer::restore_window_without_activating`]; Tauri's own restore is
//! `ShowWindow(SW_RESTORE)`, which activates it whenever Windows lets this app take the
//! foreground), on macOS `deminiaturize`, which leaves the active application active. Only
//! Stop focuses the window (`set_focus`, which on Windows may tap Alt to be allowed to).
//!
//! A window event (focus, a resize) while the window is not minimised means the user brought
//! it back. Events while it is still minimised do not: Windows activates a minimised window
//! when the window in front of it closes, and a change of display scale resizes it.
//!
//! The red border and the Stop window are windows of their own and stay.

use std::sync::Once;

use computer::{Aside, Comeback, MainWindow, WindowNow};
use tauri::{AppHandle, Emitter, Manager, UserAttentionType, WebviewWindow, WindowEvent};
use tokio_util::sync::CancellationToken;

use crate::state::AppState;

/// Sent to the main window each time the app brought it back on screen.
pub const MAIN_WINDOW_SHOWN: &str = "main-window-shown";

static ASIDE: Aside = Aside::new();
/// The window events that tell the app the user brought the window back are followed from
/// the first minimise on.
static FOLLOW: Once = Once::new();

struct TauriWindow {
    app: AppHandle,
    w: WebviewWindow,
}

impl TauriWindow {
    fn get(app: &AppHandle) -> Option<Self> {
        let w = app.get_webview_window(crate::MAIN_WINDOW)?;
        Some(Self {
            app: app.clone(),
            w,
        })
    }

    fn tell_the_user(&self) {
        let _ = self
            .w
            .request_user_attention(Some(UserAttentionType::Informational));
    }
}

impl MainWindow for TauriWindow {
    fn now(&self) -> WindowNow {
        WindowNow {
            visible: self.w.is_visible().unwrap_or(false),
            minimised: self.w.is_minimized().unwrap_or(false),
        }
    }

    fn minimise(&self) {
        let _ = self.w.minimize();
    }

    fn before_show(&self) {
        let _ = self.app.emit_to(crate::MAIN_WINDOW, MAIN_WINDOW_SHOWN, ());
    }

    fn unminimise(&self, how: Comeback) {
        #[cfg(target_os = "windows")]
        if how != Comeback::Focused {
            match restore_without_activating(&self.app, &self.w) {
                Quiet::Done => {
                    // Brings Tauri's idea of the window up to date; with the window back on
                    // screen nothing happens there.
                    if self.w.is_minimized().is_ok_and(|m| !m) {
                        let _ = self.w.unminimize();
                    }
                    return;
                }
                // The main thread is busy; the restore runs once it is free.
                Quiet::Late => return,
                Quiet::NoHandle => {}
            }
        }
        let _ = how;
        let _ = self.w.unminimize();
    }

    fn present(&self, how: Comeback) {
        match how {
            Comeback::Focused => {
                let _ = self.w.set_focus();
                #[cfg(target_os = "macos")]
                order_front(&self.app, &self.w);
            }
            Comeback::InFront => {
                in_front(&self.app, &self.w);
                self.tell_the_user();
            }
            Comeback::Quietly => self.tell_the_user(),
        }
        let _ = self.app.emit_to(crate::MAIN_WINDOW, MAIN_WINDOW_SHOWN, ());
    }
}

/// How a restore without activation went.
#[cfg(target_os = "windows")]
enum Quiet {
    Done,
    Late,
    NoHandle,
}

/// Restores the minimised window without activating it, on the main thread, which owns it.
#[cfg(target_os = "windows")]
fn restore_without_activating(app: &AppHandle, w: &WebviewWindow) -> Quiet {
    let Ok(hwnd) = w.hwnd() else {
        return Quiet::NoHandle;
    };
    let addr = hwnd.0 as usize;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let sent = app.run_on_main_thread(move || {
        // SAFETY: the main window is one of this app's windows; this runs on the main
        // thread, which owns it.
        let _ = tx.send(unsafe {
            computer::restore_window_without_activating(addr as *mut std::ffi::c_void)
        });
    });
    if sent.is_err() {
        return Quiet::NoHandle;
    }
    match rx.recv_timeout(std::time::Duration::from_secs(1)) {
        Ok(_) => Quiet::Done,
        Err(_) => Quiet::Late,
    }
}

/// In front of the other windows, without taking the keyboard.
fn in_front(app: &AppHandle, w: &WebviewWindow) {
    #[cfg(target_os = "macos")]
    order_front(app, w);
    #[cfg(not(target_os = "macos"))]
    {
        // Always-on-top windows are above all others; leaving that state keeps the window
        // at the top of the rest. Neither step activates it.
        let _ = app;
        let _ = w.set_always_on_top(true);
        let _ = w.set_always_on_top(false);
    }
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

/// The user brought the window back (from the taskbar, the Dock, Alt+Tab): it is theirs
/// until the next approved input that it is in the way of. The app's own restore, and
/// events while the window is still minimised, are told apart by
/// [`computer::Aside::brought_back`].
fn follow_window_events(w: &WebviewWindow) {
    FOLLOW.call_once(|| {
        let win = w.clone();
        w.on_window_event(move |event| {
            let back = match event {
                WindowEvent::Focused(true) => true,
                WindowEvent::Resized(size) => size.width > 0 && size.height > 0,
                _ => false,
            };
            if back {
                // Window events run on the main thread, where reading the window is direct.
                ASIDE.brought_back(WindowNow {
                    visible: win.is_visible().unwrap_or(false),
                    minimised: win.is_minimized().unwrap_or(true),
                });
            }
        });
    });
}

/// Before approved input that the main window is in the way of, on a blocking thread (the
/// computer server's hook): minimises the window when it is on screen and waits until it
/// is gone, at most about a second.
pub(super) fn before_input(app: &AppHandle, chat_id: i64, cancel: &CancellationToken) {
    let Some(w) = TauriWindow::get(app) else {
        return;
    };
    follow_window_events(&w.w);
    ASIDE.before_input(&w, chat_id, cancel);
}

/// Whether the user approved this chat's computer input on its card just now (the computer
/// server asks once per input action; see [`computer::StepAside::approved_on_card`]).
pub(super) fn take_card_approval(chat_id: i64) -> bool {
    ASIDE.take_card_approval(chat_id)
}

/// A turn of this chat starts: returns the key [`turn_ended`] takes.
pub fn turn_started(chat_id: i64) -> u64 {
    ASIDE.turn_started(chat_id)
}

/// The user answered an approval card of this chat with Allow. Counts for stepping aside
/// only for computer input.
pub fn card_approved(chat_id: i64, action: &policy::Action) {
    if matches!(action.scope, policy::Scope::ComputerInput { .. }) {
        ASIDE.card_approved(chat_id);
    }
}

/// Runs `f` on a blocking thread when this OS has computer use (window calls wait for the
/// main thread).
fn on_blocking_thread(app: &AppHandle, f: impl FnOnce(&TauriWindow) + Send + 'static) {
    let has_server = app
        .try_state::<super::ComputerHandle>()
        .is_some_and(|h| h.0.is_some());
    if !has_server {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        if let Some(w) = TauriWindow::get(&app) {
            f(&w);
        }
    });
}

/// An approval card of this chat waits for the user.
pub fn approval_needed(app: &AppHandle, chat_id: i64) {
    on_blocking_thread(app, move |w| ASIDE.approval_needed(w, chat_id));
}

/// This turn of the chat ended: done, failed or stopped.
pub fn turn_ended(app: &AppHandle, chat_id: i64, turn: u64) {
    on_blocking_thread(app, move |w| ASIDE.turn_ended(w, chat_id, turn));
}

/// The user pressed Stop: call before the turns are cancelled, so a restore that wins the
/// race against Stop's own (the end of the stopped turn) gives the keyboard too.
pub fn stop_requested() {
    ASIDE.stop_requested();
}

/// The user pressed Stop: the stop hotkey or the indicator's Stop button.
pub fn stopped(app: &AppHandle) {
    ASIDE.stop_requested();
    on_blocking_thread(app, |w| ASIDE.stopped(w));
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
            if let Some(w) = app.get_webview_window(crate::MAIN_WINDOW) {
                let _ = w.minimize();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// The window comes back wherever the user is needed: every approval card the app
    /// raises (in a turn and during a call), the end of every turn, keyed by the turn that
    /// started, and Stop. An answer on a card counts for stepping aside. The decisions
    /// themselves are tested in `computer::window_aside`, and the step aside in
    /// `computer::server`.
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
                .matches("let aside_turn = crate::computer_cmd::turn_started(chat_id.0);")
                .count(),
            1
        );
        assert_eq!(
            turns
                .matches("crate::computer_cmd::turn_ended(app, chat_id.0, aside_turn);")
                .count(),
            1
        );
        assert_eq!(
            turns.matches("crate::computer_cmd::card_approved(").count(),
            1
        );
        let hotkey = include_str!("../hotkey.rs");
        assert!(hotkey.contains("crate::computer_cmd::stopped(app);"));
        // Stop is known before the turns are cancelled, and so before any turn can end.
        let stop_now = &hotkey[hotkey.find("pub fn stop_now(").expect("stop_now")..];
        let pending = stop_now
            .find("crate::computer_cmd::stop_requested();")
            .expect("stop_requested");
        let cancel = stop_now.find("generation.cancel()").expect("cancel");
        assert!(pending < cancel);
        let wiring = include_str!("../computer_cmd.rs");
        assert!(wiring.contains("approved_on_card: Arc::new(aside::take_card_approval)"));
    }

    /// The page listens for the event the app sends when it brings the window back:
    /// approval cards and the Allow all / Full access confirmations start their input delay
    /// again.
    #[test]
    fn the_page_listens_for_the_window_coming_back() {
        let page = include_str!("../../../src/lib/windowShown.ts");
        assert!(page.contains(&format!(
            "export const MAIN_WINDOW_SHOWN = \"{}\";",
            super::MAIN_WINDOW_SHOWN
        )));
        let delay = include_str!("../../../src/lib/useInputDelay.ts");
        assert!(delay.contains("onMainWindowShown(restart)"));
        let card = include_str!("../../../src/components/ApprovalCard.tsx");
        assert!(card.contains("useInputDelay(CARD_INPUT_DELAY_MS)"));
        let confirms = include_str!("../../../src/components/AllowAllButton.tsx");
        assert!(confirms.contains("useInputDelay(CONFIRM_INPUT_DELAY_MS)"));
    }
}
