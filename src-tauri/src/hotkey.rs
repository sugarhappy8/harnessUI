//! The global stop hotkey (ARCHITECTURE.md 4.2, decision D14), registered from Rust so
//! it works when the window is not focused. It cancels the running turn's root token.
//! A user-chosen combination is test-registered before it is saved; if the hotkey
//! cannot be registered the UI shows a persistent warning. Every change of its status is
//! sent to the main window as a `stop-hotkey-status` event.

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Ctrl+Alt+Backspace on Windows; Ctrl+Option+Delete on macOS (Delete is Backspace).
/// Not Ctrl+Alt+<letter>, which AltGr keyboards type with.
pub const DEFAULT_HOTKEY: &str = "Control+Alt+Backspace";
const SETTING: &str = "stopHotkey";
/// Sent to the main window with a [`HotkeyStatus`] when the hotkey is registered at start
/// and whenever `set_stop_hotkey` changes it. The window warns while it is not registered.
pub const STATUS_EVENT: &str = "stop-hotkey-status";

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyStatus {
    pub accelerator: String,
    pub registered: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct Hotkey(Mutex<HotkeyStatus>);

impl Hotkey {
    /// The current hotkey, as text for the indicator.
    pub fn accelerator(&self) -> Option<String> {
        self.0
            .lock()
            .ok()
            .filter(|s| s.registered)
            .map(|s| s.accelerator.clone())
    }
}

/// Stores the hotkey's status and sends it to the main window.
fn publish(app: &AppHandle, status: HotkeyStatus) {
    if let Some(h) = app.try_state::<Hotkey>()
        && let Ok(mut s) = h.0.lock()
    {
        *s = status.clone();
    }
    let _ = app.emit_to(crate::MAIN_WINDOW, STATUS_EVENT, status);
}

/// The status after a new combination could not be registered: the previous one, if it
/// could be registered again.
fn after_failed_change(previous: String, restored: bool, error: String) -> HotkeyStatus {
    HotkeyStatus {
        accelerator: previous,
        registered: restored,
        error: (!restored).then_some(error),
    }
}

/// Stops the turn (and, in M7, releases held input). Used by the hotkey and the
/// computer-use indicator's Stop button.
pub fn stop_now(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>() {
        state.generation.cancel();
    }
    crate::computer_cmd::release_all(app);
    crate::turns::cancel_all_pending(app);
    let _ = app.emit_to(crate::MAIN_WINDOW, "stop-requested", ());
    tracing::info!("stop hotkey pressed");
}

fn register(app: &AppHandle, accelerator: &str) -> Result<(), String> {
    let shortcut: Shortcut = accelerator
        .parse()
        .map_err(|e| format!("\"{accelerator}\" is not a valid key combination: {e}"))?;
    app.global_shortcut()
        .on_shortcut(shortcut, |app, _shortcut, event| {
            if event.state == ShortcutState::Pressed {
                stop_now(app);
            }
        })
        .map_err(|e| format!("another program is using {accelerator} ({e})"))
}

/// Registers the saved hotkey (or the default) at startup.
pub async fn init(app: &AppHandle) {
    let state = app.state::<AppState>();
    let accelerator: String = state
        .store
        .get_setting(SETTING)
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| DEFAULT_HOTKEY.to_string());
    let result = register(app, &accelerator);
    if let Err(e) = &result {
        tracing::warn!("stop hotkey not registered: {e}");
    }
    publish(
        app,
        HotkeyStatus {
            accelerator,
            registered: result.is_ok(),
            error: result.err(),
        },
    );
}

#[tauri::command]
pub async fn stop_hotkey_status(hotkey: State<'_, Hotkey>) -> AppResult<HotkeyStatus> {
    Ok(hotkey.0.lock().map(|s| s.clone()).unwrap_or_default())
}

/// Test-registers `accelerator`; saves it only if registration works, otherwise the
/// previous hotkey stays.
#[tauri::command]
pub async fn set_stop_hotkey(
    app: AppHandle,
    state: State<'_, AppState>,
    hotkey: State<'_, Hotkey>,
    accelerator: String,
) -> AppResult<HotkeyStatus> {
    let accelerator = accelerator.trim().to_string();
    let previous = hotkey
        .0
        .lock()
        .map(|s| s.accelerator.clone())
        .unwrap_or_default();
    if let Ok(old) = previous.parse::<Shortcut>() {
        let _ = app.global_shortcut().unregister(old);
    }
    match register(&app, &accelerator) {
        Ok(()) => {
            state.store.set_setting(SETTING, &accelerator).await?;
            let status = HotkeyStatus {
                accelerator,
                registered: true,
                error: None,
            };
            publish(&app, status.clone());
            Ok(status)
        }
        Err(e) => {
            let restored = register(&app, &previous).is_ok();
            publish(&app, after_failed_change(previous, restored, e.clone()));
            Err(AppError::new(
                "hotkey_unavailable",
                "That key combination cannot be used.",
                "Choose a different one.",
                e,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `stop-hotkey-status` payload the main window reads (src/lib/types.ts
    /// `HotkeyStatus`).
    #[test]
    fn the_status_event_has_the_shape_the_window_reads() {
        assert_eq!(STATUS_EVENT, "stop-hotkey-status");
        let v = serde_json::to_value(HotkeyStatus {
            accelerator: DEFAULT_HOTKEY.into(),
            registered: false,
            error: Some("another program is using it".into()),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "accelerator": "Control+Alt+Backspace",
                "registered": false,
                "error": "another program is using it",
            })
        );
    }

    /// A combination that cannot be used keeps the previous hotkey. If that cannot be
    /// registered again either, the status says so and the window keeps warning.
    #[test]
    fn a_failed_change_reports_whether_the_previous_hotkey_still_works() {
        let kept = after_failed_change("Control+Alt+Backspace".into(), true, "busy".into());
        assert!(kept.registered);
        assert_eq!(kept.accelerator, "Control+Alt+Backspace");
        assert_eq!(kept.error, None);
        let lost = after_failed_change("Control+Alt+Backspace".into(), false, "busy".into());
        assert!(!lost.registered);
        assert_eq!(lost.accelerator, "Control+Alt+Backspace");
        assert_eq!(lost.error.as_deref(), Some("busy"));
    }
}
