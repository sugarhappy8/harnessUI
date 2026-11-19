//! Quitting while a local Studio job works (0.1.0): a local job cannot outlive the app, so
//! closing the main window, or quitting from the app menu or with ⌘Q, asks first. `lib.rs`
//! prevents the close or the exit and sends [`QUIT_REQUESTED_EVENT`] to the main window, which
//! shows "Quitting stops it. [Keep working] [Quit and stop]"; *Quit and stop* is
//! `studio_quit_now`, which stops the local runs and exits with a code, so it is not asked
//! again.

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager};

use super::manager::StudioManager;

/// The webview event that asks whether to quit while a local job works.
pub const QUIT_REQUESTED_EVENT: &str = "studio-quit-requested";

/// Closing the main window (`exit_code` None) or an exit request without a code (the app menu's
/// Quit, ⌘Q) asks while a local job works; `app.exit(0)` from `studio_quit_now` carries a code
/// and is never asked again.
pub fn ask_before_quit(local_running: bool, exit_code: Option<i32>) -> bool {
    local_running && exit_code.is_none()
}

/// Whether to hold a close or an exit with `exit_code` back: true when
/// [`ask_before_quit`] says to ask, after [`QUIT_REQUESTED_EVENT`] went to the main window.
pub fn hold_back(app: &AppHandle, exit_code: Option<i32>) -> bool {
    let local_running = app
        .try_state::<Arc<StudioManager>>()
        .is_some_and(|studio| studio.local_running());
    if !ask_before_quit(local_running, exit_code) {
        return false;
    }
    if let Err(e) = app.emit_to(crate::MAIN_WINDOW, QUIT_REQUESTED_EVENT, ()) {
        // Nothing could ask: quitting goes ahead rather than leave the app unable to close.
        tracing::warn!("the question about quitting was not sent: {e}");
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_and_the_app_menu_ask_the_same_way() {
        assert!(ask_before_quit(true, None));
        assert!(!ask_before_quit(false, None));
        // studio_quit_now's own app.exit(0).
        assert!(!ask_before_quit(true, Some(0)));
        assert!(!ask_before_quit(false, Some(0)));
    }
}
