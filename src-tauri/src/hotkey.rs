//! The global stop hotkey (ARCHITECTURE.md 4.2, decision D14), registered from Rust so
//! it works when the window is not focused. It cancels the running turn's root token.
//! A user-chosen combination is test-registered before it is saved; if the hotkey
//! cannot be registered the UI shows a persistent warning.

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

/// Stops the turn (and, in M7, releases held input). Used by the hotkey and the
/// computer-use indicator's Stop button.
pub fn stop_now(app: &AppHandle) {
    if let Some(state) = app.try_state::<AppState>()
        && let Ok(guard) = state.generation.lock()
        && let Some(token) = guard.as_ref()
    {
        token.cancel();
    }
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
    if let Some(h) = app.try_state::<Hotkey>()
        && let Ok(mut s) = h.0.lock()
    {
        *s = HotkeyStatus {
            accelerator,
            registered: result.is_ok(),
            error: result.err(),
        };
    }
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
            if let Ok(mut s) = hotkey.0.lock() {
                *s = status.clone();
            }
            Ok(status)
        }
        Err(e) => {
            let restored = register(&app, &previous).is_ok();
            if let Ok(mut s) = hotkey.0.lock() {
                *s = HotkeyStatus {
                    accelerator: previous,
                    registered: restored,
                    error: if restored { None } else { Some(e.clone()) },
                };
            }
            Err(AppError::new(
                "hotkey_unavailable",
                "That key combination cannot be used.",
                "Choose a different one.",
                e,
            ))
        }
    }
}
