//! The computer-use indicator (ARCHITECTURE.md 9.3, "Indicator"): a click-through red
//! border over the monitor being controlled, and a small always-on-top window with the
//! text "AI is controlling this computer. Press <hotkey> to stop" and a Stop button.
//!
//! Both windows belong to this app, so the input injector refuses to send events to
//! them, and screenshots leave them out.

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};

use crate::hotkey::Hotkey;

/// Label of the Stop-button window (its capability grants only `overlay_stop`).
pub const INDICATOR_WINDOW: &str = "indicator";

/// The indicator's Stop button.
#[tauri::command]
pub fn overlay_stop(app: AppHandle) {
    crate::hotkey::stop_now(&app);
    hide(&app);
}

/// Shows the border on `monitor` (or the primary monitor) and the Stop window at the top
/// centre of it. Returns false when the indicator could not be shown; the computer tool
/// then refuses to act (9.3).
// TODO(M7): remove the allow when the computer tool calls it.
#[allow(dead_code)]
pub fn show(app: &AppHandle, monitor: Option<tauri::Monitor>) -> bool {
    let (Some(border), Some(indicator)) = (
        app.get_webview_window(crate::OVERLAY_WINDOW),
        app.get_webview_window(INDICATOR_WINDOW),
    ) else {
        return false;
    };
    let monitor = monitor.or_else(|| border.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return false };
    let pos = *monitor.position();
    let size = *monitor.size();
    let ok = border
        .set_position(PhysicalPosition::new(pos.x, pos.y))
        .is_ok()
        && border
            .set_size(PhysicalSize::new(size.width, size.height))
            .is_ok()
        && border.show().is_ok()
        && border.set_ignore_cursor_events(true).is_ok();
    let scale = monitor.scale_factor();
    let w = (460.0 * scale) as u32;
    let h = (52.0 * scale) as u32;
    let x = pos.x + (size.width.saturating_sub(w) / 2) as i32;
    let y = pos.y + (16.0 * scale) as i32;
    let ok = ok
        && indicator.set_size(PhysicalSize::new(w, h)).is_ok()
        && indicator.set_position(PhysicalPosition::new(x, y)).is_ok()
        && indicator.show().is_ok();
    let hotkey = app
        .state::<Hotkey>()
        .accelerator()
        .unwrap_or_else(|| "the stop hotkey".into());
    let _ = app.emit_to(INDICATOR_WINDOW, "indicator-hotkey", hotkey);
    ok
}

/// Hides both indicator windows.
pub fn hide(app: &AppHandle) {
    for label in [crate::OVERLAY_WINDOW, INDICATOR_WINDOW] {
        if let Some(w) = app.get_webview_window(label) {
            let _ = w.hide();
        }
    }
}
