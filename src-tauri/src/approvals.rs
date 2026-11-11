//! Approval modes (0.0.4): *Ask first*, or *Allow all*.
//!
//! A chat is in one of two modes. *Ask* is the behaviour of ARCHITECTURE.md 13.3. In
//! *Allow all* the policy engine answers the approval card itself, except for the actions
//! that keep asking (deleting, destroying, sensitive places, paying or publishing,
//! protected applications, tools marked destructive, commands that hide what they run)
//! and for actions that can send data out once untrusted content has entered the turn
//! (`policy::evaluate`, D9).
//!
//! What must hold:
//!
//! - Only the user changes a mode: the two `set_…` commands below, which are granted to
//!   the main window alone and check that it is the caller. No tool reaches this module,
//!   nothing the model writes is read here, and the generic `set_setting` command refuses
//!   the key of the default ([`ALLOW_ALL_DEFAULT`] is not on its list).
//! - A chat's own mode lives in memory, like grants (D12): it ends when the app quits or
//!   the user switches it off. Only "Allow all in every chat" is a saved setting.
//! - The policy gate reads the mode on every decision ([`for_decision`]), so a switch
//!   reaches a turn that is already running, in both directions. A helper started by
//!   `run_subagent` is decided by a gate built for the same chat, so it is in the same
//!   mode as the chat.
//! - Hard limits are checked before the mode (`policy::evaluate` steps 1 and 2): a path
//!   outside the approved folders or a blocked address stays a denial.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use serde_json::json;
use tauri::{AppHandle, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The saved setting behind "Allow all in every chat" (a JSON boolean, off when absent).
/// Changed only by [`set_allow_all_default`].
pub const ALLOW_ALL_DEFAULT: &str = "allowAllDefault";

/// Which chats are in Allow all.
#[derive(Default)]
pub struct ApprovalModes(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    /// "Allow all in every chat": every chat is in Allow all unless switched back by hand.
    default_on: bool,
    /// Chats the user switched by hand during this run.
    chats: HashMap<i64, bool>,
    /// Whether untrusted content had entered the turn when the chat's latest call was
    /// decided (for navigations the browser holds while that call runs).
    untrusted: HashMap<i64, bool>,
}

impl ApprovalModes {
    /// Whether `chat` is in Allow all. A state that cannot be read counts as Ask.
    pub fn allow_all(&self, chat: i64) -> bool {
        self.0
            .lock()
            .map(|m| m.chats.get(&chat).copied().unwrap_or(m.default_on))
            .unwrap_or(false)
    }

    /// Switches one chat. Its choice is kept until the app quits, the user switches it
    /// again, or "Allow all in every chat" is switched off.
    pub fn set_chat(&self, chat: i64, allow_all: bool) {
        if let Ok(mut m) = self.0.lock() {
            if allow_all == m.default_on {
                // The same as the default: nothing to remember.
                m.chats.remove(&chat);
            } else {
                m.chats.insert(chat, allow_all);
            }
        }
    }

    /// Whether "Allow all in every chat" is on.
    pub fn default_on(&self) -> bool {
        self.0.lock().map(|m| m.default_on).unwrap_or(false)
    }

    /// Switches "Allow all in every chat". Switching it off puts every chat back to
    /// asking, also the ones switched on by hand: off means nothing runs unasked.
    pub fn set_default(&self, on: bool) {
        if let Ok(mut m) = self.0.lock() {
            m.default_on = on;
            if !on {
                m.chats.clear();
            }
        }
    }

    /// Notes whether untrusted content had entered the turn at the decision being made.
    fn note_decision(&self, chat: i64, untrusted_in_turn: bool) {
        if let Ok(mut m) = self.0.lock() {
            m.untrusted.insert(chat, untrusted_in_turn);
        }
    }

    /// Whether untrusted content had entered the turn when the chat's latest call was
    /// decided. Unknown counts as yes.
    pub fn untrusted_at_last_decision(&self, chat: i64) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|m| m.untrusted.get(&chat).copied())
            .unwrap_or(true)
    }
}

/// The one copy for the app. It is process-wide because the policy gates of a turn and of
/// its helpers read it on every decision and know only their chat's id.
static MODES: LazyLock<ApprovalModes> = LazyLock::new(ApprovalModes::default);

/// The app's approval modes.
pub fn modes() -> &'static ApprovalModes {
    &MODES
}

/// For the policy gate, on every decision: notes whether untrusted content is in the turn
/// and says whether the chat is in Allow all right now.
pub fn for_decision(chat_id: i64, untrusted_in_turn: bool) -> bool {
    let modes = modes();
    modes.note_decision(chat_id, untrusted_in_turn);
    modes.allow_all(chat_id)
}

/// Reads the saved "Allow all in every chat" setting into `modes` (at start).
async fn load_default_into(store: &store::Store, modes: &ApprovalModes) {
    let on: bool = store
        .get_setting(ALLOW_ALL_DEFAULT)
        .await
        .ok()
        .flatten()
        .unwrap_or(false);
    if on {
        modes.set_default(true);
    }
}

/// Loads the saved default when the app starts.
pub fn load_default(app: &AppHandle) {
    let state = app.state::<AppState>();
    tauri::async_runtime::block_on(load_default_into(&state.store, modes()));
    if modes().default_on() {
        tracing::info!("every chat starts in Allow all (saved setting)");
    }
}

/// Saves "Allow all in every chat" and applies it. Saved first: if that fails, nothing
/// changes.
async fn save_default(store: &store::Store, modes: &ApprovalModes, on: bool) -> AppResult<()> {
    store.set_setting(ALLOW_ALL_DEFAULT, &json!(on)).await?;
    modes.set_default(on);
    Ok(())
}

/// Only the main window may change a mode (the capability files grant these commands to
/// it alone; this is the same rule checked again where the change is made).
fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window can change how approvals work.",
        ))
    }
}

/// Whether the chat is in Allow all.
#[tauri::command]
pub fn get_approval_mode(chat_id: i64) -> bool {
    modes().allow_all(chat_id)
}

/// Switches one chat between Ask first and Allow all. Returns the mode it is in now.
#[tauri::command]
pub async fn set_approval_mode(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
    allow_all: bool,
) -> AppResult<bool> {
    only_main_window(window.label())?;
    if state
        .store
        .get_chat(store::ChatId(chat_id))
        .await?
        .is_none()
    {
        return Err(AppError::bad_request("That chat no longer exists."));
    }
    modes().set_chat(chat_id, allow_all);
    tracing::info!(chat_id, allow_all, "approval mode changed by the user");
    Ok(modes().allow_all(chat_id))
}

/// Whether every chat starts in Allow all.
#[tauri::command]
pub fn get_allow_all_default() -> bool {
    modes().default_on()
}

/// Switches "Allow all in every chat".
#[tauri::command]
pub async fn set_allow_all_default(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    on: bool,
) -> AppResult<()> {
    only_main_window(window.label())?;
    save_default(&state.store, modes(), on).await?;
    tracing::info!(on, "\"Allow all in every chat\" changed by the user");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chat's own choice wins over the default, and switching the default off puts
    /// every chat back to asking.
    #[test]
    fn a_chat_follows_the_default_until_it_is_switched_by_hand() {
        let modes = ApprovalModes::default();
        assert!(!modes.allow_all(1), "a new chat asks");
        assert!(!modes.default_on());

        modes.set_chat(1, true);
        assert!(modes.allow_all(1));
        assert!(!modes.allow_all(2), "another chat is not switched");
        modes.set_chat(1, false);
        assert!(!modes.allow_all(1));
        assert!(modes.0.lock().unwrap().chats.is_empty());

        // "Allow all in every chat": every chat, except one switched back by hand.
        modes.set_default(true);
        assert!(modes.allow_all(1) && modes.allow_all(2) && modes.allow_all(999));
        modes.set_chat(2, false);
        assert!(!modes.allow_all(2));
        assert!(modes.allow_all(1));
        modes.set_chat(2, true);
        assert!(modes.allow_all(2));

        // Off: nothing runs unasked, also in a chat switched on by hand before.
        modes.set_default(false);
        modes.set_chat(3, true);
        modes.set_default(true);
        modes.set_default(false);
        for chat in [1, 2, 3, 999] {
            assert!(!modes.allow_all(chat), "chat {chat} still allows all");
        }
    }

    /// What the browser's held navigations are judged by: unknown counts as untrusted.
    #[test]
    fn the_latest_decision_says_whether_untrusted_content_is_in_the_turn() {
        let modes = ApprovalModes::default();
        assert!(modes.untrusted_at_last_decision(1), "unknown is untrusted");
        modes.note_decision(1, false);
        assert!(!modes.untrusted_at_last_decision(1));
        assert!(modes.untrusted_at_last_decision(2));
        modes.note_decision(1, true);
        assert!(modes.untrusted_at_last_decision(1));
    }

    /// The default is a saved setting: it survives a restart, and a failed save changes
    /// nothing. A chat's own choice is never saved.
    #[tokio::test]
    async fn the_default_is_saved_and_a_chats_own_mode_is_not() {
        let store = store::Store::open_in_memory().unwrap();
        let modes = ApprovalModes::default();
        load_default_into(&store, &modes).await;
        assert!(!modes.default_on(), "off unless the user switched it on");

        modes.set_chat(5, true);
        save_default(&store, &modes, true).await.unwrap();
        assert!(modes.default_on());
        let saved: Option<bool> = store.get_setting(ALLOW_ALL_DEFAULT).await.unwrap();
        assert_eq!(saved, Some(true));

        // After a restart: the default is back, a chat's own choice is not.
        let after_restart = ApprovalModes::default();
        load_default_into(&store, &after_restart).await;
        assert!(after_restart.default_on());
        assert!(after_restart.0.lock().unwrap().chats.is_empty());

        save_default(&store, &after_restart, false).await.unwrap();
        let fresh = ApprovalModes::default();
        fresh.set_chat(5, true);
        load_default_into(&store, &fresh).await;
        assert!(!fresh.default_on());
        assert!(
            fresh.allow_all(5),
            "loading an off default switches nothing"
        );
    }

    #[test]
    fn only_the_main_window_changes_a_mode() {
        assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
        for other in [crate::OVERLAY_WINDOW, "indicator", "", "Main", "main2"] {
            assert!(only_main_window(other).is_err(), "{other:?}");
        }
    }
}
