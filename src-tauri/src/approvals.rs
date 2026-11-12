//! Approval modes: *Ask first* (0.0.1), *Allow all* (0.0.4) and *Full access* (0.0.5).
//!
//! A chat is in one of three modes. *Ask* is the behaviour of ARCHITECTURE.md 13.3. In
//! *Allow all* the policy engine answers the approval card itself, except for the actions
//! that keep asking (deleting, destroying, sensitive places, paying or publishing,
//! protected applications, tools marked destructive, commands that hide what they run)
//! and for actions that can send data out once untrusted content has entered the turn
//! (`policy::evaluate`, D9). In *Full access* nothing asks: every action that passes the
//! hard limits runs, and the file tools reach the whole computer except the app's own
//! folders (`turns::assemble`). The owner chose this on 2026-11-12: "run absolutely
//! everything; only the app's own database and saved keys stay protected".
//!
//! What must hold:
//!
//! - Only the user changes a mode: the `set_…` commands below, which are granted to the
//!   main window alone and check that it is the caller. No tool reaches this module,
//!   nothing the model writes is read here, and the generic `set_setting` command refuses
//!   the keys of the default (neither [`APPROVAL_MODE_DEFAULT`] nor [`ALLOW_ALL_DEFAULT`]
//!   is on its list).
//! - A chat's own mode lives in memory, like grants (D12): it ends when the app quits or
//!   the user switches it. Only the mode every chat starts in is a saved setting.
//! - The policy gate reads the mode on every decision ([`for_decision`]), so a switch
//!   reaches a turn that is already running, in both directions. A helper started by
//!   `run_subagent` is decided by a gate built for the same chat, so it is in the same
//!   mode as the chat.
//! - Hard limits are checked before the mode (`policy::evaluate` steps 1 and 2): the app's
//!   own folders stay out of reach in every mode.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The saved mode every chat starts in (a JSON string: "ask", "allowAll", "fullAccess").
/// Changed only by [`set_default_approval_mode`] and [`set_allow_all_default`].
pub const APPROVAL_MODE_DEFAULT: &str = "approvalModeDefault";

/// The 0.0.4 setting behind "Allow all in every chat" (a JSON boolean). Read when
/// [`APPROVAL_MODE_DEFAULT`] is absent, and still written (true unless the default is Ask) so
/// that going back to 0.0.4 keeps the closest mode it knows.
pub const ALLOW_ALL_DEFAULT: &str = "allowAllDefault";

/// The note on an audit row of an action that ran because the chat was in Full access. The
/// audit table's list of decisions is not changed for it (no migration): the row's decision
/// is `allowed_by_mode` and this note names the mode.
pub const FULL_ACCESS_NOTE: &str = "Full access";

/// How a chat's tool calls are approved.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalMode {
    /// Every action asks first (ARCHITECTURE.md 13.3).
    #[default]
    Ask,
    /// The engine answers the card, except for actions that keep asking (0.0.4).
    AllowAll,
    /// Nothing asks; the file tools reach the whole computer except the app's own folders
    /// (0.0.5).
    FullAccess,
}

/// Which mode each chat is in.
#[derive(Default)]
pub struct ApprovalModes(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    /// The mode every chat starts in, unless switched by hand.
    default: ApprovalMode,
    /// Chats the user switched by hand during this run.
    chats: HashMap<i64, ApprovalMode>,
    /// Whether untrusted content had entered the turn when the chat's latest call was
    /// decided (for navigations the browser holds while that call runs).
    untrusted: HashMap<i64, bool>,
    /// The saved default is Full access and the user has not confirmed it in this run. Until
    /// then every chat starts in Ask first: a saved value is a row in a database a command
    /// can write, so it alone never turns Full access on.
    full_access_pending: bool,
}

impl ApprovalModes {
    /// The chat's mode. A state that cannot be read counts as Ask.
    pub fn mode(&self, chat: i64) -> ApprovalMode {
        self.0
            .lock()
            .map(|m| m.chats.get(&chat).copied().unwrap_or(m.default))
            .unwrap_or(ApprovalMode::Ask)
    }

    /// Whether `chat` is in Allow all (exactly; Full access is not Allow all).
    #[cfg(test)]
    pub fn allow_all(&self, chat: i64) -> bool {
        self.mode(chat) == ApprovalMode::AllowAll
    }

    /// Whether `chat` is in Full access.
    pub fn full_access(&self, chat: i64) -> bool {
        self.mode(chat) == ApprovalMode::FullAccess
    }

    /// Switches one chat. Its choice is kept until the app quits, the user switches it
    /// again, or the mode every chat starts in changes.
    pub fn set_chat_mode(&self, chat: i64, mode: ApprovalMode) {
        if let Ok(mut m) = self.0.lock() {
            if mode == m.default {
                // The same as the default: nothing to remember.
                m.chats.remove(&chat);
            } else {
                m.chats.insert(chat, mode);
            }
        }
    }

    /// The 0.0.4 switch of one chat: Allow all or Ask.
    pub fn set_chat(&self, chat: i64, allow_all: bool) {
        self.set_chat_mode(
            chat,
            if allow_all {
                ApprovalMode::AllowAll
            } else {
                ApprovalMode::Ask
            },
        );
    }

    /// The mode every chat starts in.
    pub fn default_mode(&self) -> ApprovalMode {
        self.0
            .lock()
            .map(|m| m.default)
            .unwrap_or(ApprovalMode::Ask)
    }

    /// Whether every chat starts in a mode other than Ask (the 0.0.4 "Allow all in every
    /// chat" switch, read by older callers).
    pub fn default_on(&self) -> bool {
        self.default_mode() != ApprovalMode::Ask
    }

    /// Changes the mode every chat starts in. Every chat follows the new default again,
    /// also the ones switched by hand: after switching the default back to Ask, nothing runs
    /// unasked anywhere.
    pub fn set_default_mode(&self, mode: ApprovalMode) {
        if let Ok(mut m) = self.0.lock() {
            if m.default != mode {
                m.chats.clear();
            }
            m.default = mode;
            if mode == ApprovalMode::Ask {
                m.chats.clear();
            }
        }
    }

    /// The 0.0.4 switch of the default: Allow all or Ask.
    #[cfg(test)]
    pub fn set_default(&self, on: bool) {
        self.set_default_mode(if on {
            ApprovalMode::AllowAll
        } else {
            ApprovalMode::Ask
        });
    }

    /// Marks the saved Full access default as waiting for the user's confirmation.
    fn set_full_access_pending(&self, pending: bool) {
        if let Ok(mut m) = self.0.lock() {
            m.full_access_pending = pending;
        }
    }

    /// Whether the saved Full access default waits for the user's confirmation.
    pub fn full_access_pending(&self) -> bool {
        self.0
            .lock()
            .map(|m| m.full_access_pending)
            .unwrap_or(false)
    }

    /// Clears the wait and says whether there was one.
    fn take_full_access_pending(&self) -> bool {
        self.0
            .lock()
            .map(|mut m| std::mem::take(&mut m.full_access_pending))
            .unwrap_or(false)
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

/// The app, for telling the main window that a mode changed (set once at start).
static APP: OnceLock<AppHandle> = OnceLock::new();

/// Event to the main window: a chat's mode changed (`Some(chat)`) or every chat's may
/// have (`None`). The window reads the mode again; the event carries no mode itself.
const MODE_CHANGED: &str = "approval-mode-changed";

fn tell_window(chat_id: Option<i64>) {
    if let Some(app) = APP.get() {
        let _ = app.emit_to(crate::MAIN_WINDOW, MODE_CHANGED, chat_id);
    }
}

/// Puts a chat back to asking first. For "Forget everything I allowed in this chat": what
/// the user allowed there includes Allow all and Full access.
pub fn ask_first(chat_id: i64) {
    modes().set_chat_mode(chat_id, ApprovalMode::Ask);
    tell_window(Some(chat_id));
}

/// For the policy gate, on every decision: notes whether untrusted content is in the turn
/// and says which mode the chat is in right now.
pub fn for_decision(chat_id: i64, untrusted_in_turn: bool) -> ApprovalMode {
    let modes = modes();
    modes.note_decision(chat_id, untrusted_in_turn);
    modes.mode(chat_id)
}

/// Whether a navigation the browser holds is let through without a card (13.3, "Approvals
/// raised during a call"). In Full access, any hold of the chat's own call: one from the tab
/// the call works in or a tab it opened (`during_tool` is set only then). The browser is
/// shared by every chat, so a hold outside a call, or from another tab, is not this chat's
/// and keeps its card. In Allow all, only what
/// [`browser::HeldNavigation::released_in_allow_all`] says: a page moving its tab to
/// another site during a call, while no untrusted content had entered the turn. Otherwise
/// a held submission, a background send and anything held outside a call keep their card.
fn releases(modes: &ApprovalModes, chat_id: i64, held: &browser::HeldNavigation) -> bool {
    match modes.mode(chat_id) {
        ApprovalMode::FullAccess => held.during_tool.is_some(),
        ApprovalMode::AllowAll => {
            held.released_in_allow_all(modes.untrusted_at_last_decision(chat_id))
        }
        ApprovalMode::Ask => false,
    }
}

/// For the browser's approver: `true` when the held navigation is let through by the
/// chat's mode. The decision gets its own audit row, like the answers to such cards do.
pub async fn release_held_navigation(
    app: &AppHandle,
    chat_id: i64,
    held: &browser::HeldNavigation,
) -> bool {
    if !releases(modes(), chat_id, held) {
        return false;
    }
    let full = modes().full_access(chat_id);
    let mut entry = store::AuditEntry {
        ts: None,
        chat_id: Some(store::ChatId(chat_id)),
        server: "browser".into(),
        tool: held
            .during_tool
            .clone()
            .unwrap_or_else(|| "browser_navigate".into()),
        arguments: json!({ "held": held.summary }),
        preview: held.preview.chars().take(20_000).collect(),
        risk: "alwaysAsk".into(),
        decision: store::AuditDecision::AllowedByMode,
        decided_by: "policy".into(),
        note: full.then(|| FULL_ACCESS_NOTE.to_string()),
        parent_audit_id: None,
        result: None,
    };
    // A navigation held while a helper works in this chat came from the helper.
    if let Some(row) = crate::subagent::running_in(chat_id) {
        crate::subagent::mark_audit(&mut entry, row);
    }
    if let Err(e) = app.state::<AppState>().store.append_audit(entry).await {
        tracing::error!("could not write the audit log: {e}");
    }
    true
}

/// Reads the saved default into `modes` (at start). A missing or unreadable value is read
/// as missing, and then the 0.0.4 "Allow all in every chat" decides (Allow all or Ask). If
/// 0.0.4 switched that off after 0.0.5 saved a mode, off wins: the user's latest choice.
/// A saved Full access is not applied: every chat starts in Ask first until the user
/// confirms it in this run ([`answer_full_access_default`]).
async fn load_default_into(store: &store::Store, modes: &ApprovalModes) {
    let saved: Option<ApprovalMode> = store
        .get_setting(APPROVAL_MODE_DEFAULT)
        .await
        .ok()
        .flatten();
    let old: Option<bool> = store.get_setting(ALLOW_ALL_DEFAULT).await.ok().flatten();
    let mode = match (saved, old) {
        (Some(mode), Some(false)) if mode != ApprovalMode::Ask => ApprovalMode::Ask,
        (Some(mode), _) => mode,
        (None, Some(true)) => ApprovalMode::AllowAll,
        (None, _) => ApprovalMode::Ask,
    };
    match mode {
        ApprovalMode::FullAccess => modes.set_full_access_pending(true),
        ApprovalMode::AllowAll => modes.set_default_mode(mode),
        ApprovalMode::Ask => {}
    }
}

/// Loads the saved default when the app starts.
pub fn load_default(app: &AppHandle) {
    let _ = APP.set(app.clone());
    let state = app.state::<AppState>();
    tauri::async_runtime::block_on(load_default_into(&state.store, modes()));
    if modes().full_access_pending() {
        tracing::warn!(
            "the saved setting says every chat starts in Full access; waiting for the user to confirm"
        );
    } else if modes().default_mode() == ApprovalMode::AllowAll {
        tracing::info!("every chat starts in Allow all (saved setting)");
    }
}

/// Saves the default and applies it. Saved first: if that fails, nothing changes.
async fn save_default(
    store: &store::Store,
    modes: &ApprovalModes,
    mode: ApprovalMode,
) -> AppResult<()> {
    store
        .set_setting(APPROVAL_MODE_DEFAULT, &json!(mode))
        .await?;
    store
        .set_setting(ALLOW_ALL_DEFAULT, &json!(mode != ApprovalMode::Ask))
        .await?;
    modes.set_default_mode(mode);
    // The user chose the default in this run: nothing waits for a confirmation any more.
    modes.set_full_access_pending(false);
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

async fn chat_exists(state: &AppState, chat_id: i64) -> AppResult<()> {
    if state
        .store
        .get_chat(store::ChatId(chat_id))
        .await?
        .is_none()
    {
        return Err(AppError::bad_request("That chat no longer exists."));
    }
    Ok(())
}

/// Whether the chat runs anything without asking first (Allow all or Full access). Kept for
/// 0.0.4 callers; the window reads [`get_chat_approval_mode`].
#[tauri::command]
pub fn get_approval_mode(chat_id: i64) -> bool {
    modes().mode(chat_id) != ApprovalMode::Ask
}

/// Switches one chat between Ask first and Allow all (the 0.0.4 command). Returns whether
/// it is in a mode other than Ask now.
#[tauri::command]
pub async fn set_approval_mode(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
    allow_all: bool,
) -> AppResult<bool> {
    only_main_window(window.label())?;
    chat_exists(&state, chat_id).await?;
    modes().set_chat(chat_id, allow_all);
    tracing::info!(chat_id, allow_all, "approval mode changed by the user");
    tell_window(Some(chat_id));
    Ok(modes().mode(chat_id) != ApprovalMode::Ask)
}

/// The chat's approval mode.
#[tauri::command]
pub fn get_chat_approval_mode(chat_id: i64) -> ApprovalMode {
    modes().mode(chat_id)
}

/// Switches one chat to a mode. Returns the mode it is in now.
#[tauri::command]
pub async fn set_chat_approval_mode(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
    mode: ApprovalMode,
) -> AppResult<ApprovalMode> {
    only_main_window(window.label())?;
    chat_exists(&state, chat_id).await?;
    modes().set_chat_mode(chat_id, mode);
    tracing::info!(chat_id, ?mode, "approval mode changed by the user");
    tell_window(Some(chat_id));
    Ok(modes().mode(chat_id))
}

/// Whether every chat starts in a mode other than Ask (the 0.0.4 command).
#[tauri::command]
pub fn get_allow_all_default() -> bool {
    modes().default_on()
}

/// Switches "Allow all in every chat" (the 0.0.4 command): Allow all or Ask.
#[tauri::command]
pub async fn set_allow_all_default(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    on: bool,
) -> AppResult<()> {
    only_main_window(window.label())?;
    let mode = if on {
        ApprovalMode::AllowAll
    } else {
        ApprovalMode::Ask
    };
    save_default(&state.store, modes(), mode).await?;
    tracing::info!(on, "\"Allow all in every chat\" changed by the user");
    tell_window(None);
    Ok(())
}

/// The mode every chat starts in.
#[tauri::command]
pub fn get_default_approval_mode() -> ApprovalMode {
    modes().default_mode()
}

/// Changes the mode every chat starts in, saved across restarts.
#[tauri::command]
pub async fn set_default_approval_mode(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    mode: ApprovalMode,
) -> AppResult<()> {
    only_main_window(window.label())?;
    save_default(&state.store, modes(), mode).await?;
    tracing::info!(?mode, "the mode every chat starts in changed by the user");
    tell_window(None);
    Ok(())
}

/// Whether the saved setting says every chat starts in Full access and the user has not
/// confirmed it since the app started. The main window then asks.
#[tauri::command]
pub fn get_full_access_default_pending() -> bool {
    modes().full_access_pending()
}

/// The user's answer to that question: `on` makes every chat start in Full access for this
/// run (and keeps it saved); otherwise every chat starts in Ask first, saved. Does nothing
/// when no question is open.
#[tauri::command]
pub async fn answer_full_access_default(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    on: bool,
) -> AppResult<()> {
    only_main_window(window.label())?;
    if !modes().take_full_access_pending() {
        return Ok(());
    }
    let mode = if on {
        ApprovalMode::FullAccess
    } else {
        ApprovalMode::Ask
    };
    if let Err(e) = save_default(&state.store, modes(), mode).await {
        // Not saved: ask again rather than leave the question unanswered.
        modes().set_full_access_pending(true);
        return Err(e);
    }
    tracing::info!(on, "the saved Full access default answered by the user");
    tell_window(None);
    Ok(())
}

// ---- The whole computer, in Full access ---------------------------------------------

/// The folders the file and terminal tools reach in Full access: the home folder first
/// (relative paths resolve against it when the chat has no folder of its own), then the top
/// of every drive. The app's own folders stay forbidden (`ChatContext::forbidden_folders`
/// is not changed).
pub fn whole_computer(home: Option<&std::path::Path>) -> Vec<String> {
    let mut out: Vec<String> = home.map(|h| h.display().to_string()).into_iter().collect();
    #[cfg(windows)]
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        if std::path::Path::new(&root).is_dir() {
            out.push(root);
        }
    }
    #[cfg(not(windows))]
    out.push("/".into());
    out
}

/// The tool context of a chat that is in Full access right now: its own folders first (so
/// a path inside one keeps that folder for its grants and card), then the whole computer,
/// and the home folder as the working folder when the chat has none.
pub fn widen(chat: &mut mcp_host::ChatContext, whole: &[String]) {
    chat.full_access = true;
    for f in whole {
        if !chat.approved_folders.contains(f) {
            chat.approved_folders.push(f.clone());
        }
    }
    if chat.working_folder.is_none() {
        chat.working_folder = whole.first().cloned();
    }
}

/// Whether a tool is offered only in Full access (`make_folder`, `move_path`,
/// `delete_folder` of the built-in files server).
pub fn only_in_full_access(tool: &mcp_host::RegisteredTool) -> bool {
    tool.server == "files" && tools::files::organize::is_tool(&tool.original)
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
        save_default(&store, &modes, ApprovalMode::AllowAll)
            .await
            .unwrap();
        assert!(modes.default_on());
        let saved: Option<bool> = store.get_setting(ALLOW_ALL_DEFAULT).await.unwrap();
        assert_eq!(saved, Some(true));

        // After a restart: the default is back, a chat's own choice is not.
        let after_restart = ApprovalModes::default();
        load_default_into(&store, &after_restart).await;
        assert!(after_restart.default_on());
        assert!(after_restart.0.lock().unwrap().chats.is_empty());

        save_default(&store, &after_restart, ApprovalMode::Ask)
            .await
            .unwrap();
        let fresh = ApprovalModes::default();
        fresh.set_chat(5, true);
        load_default_into(&store, &fresh).await;
        assert!(!fresh.default_on());
        assert!(
            fresh.allow_all(5),
            "loading an off default switches nothing"
        );
    }

    /// A navigation the browser holds: let through only in Allow all, only when a page
    /// moves its tab to another site during a call, and only before untrusted content
    /// entered the turn. A held submission always keeps its card.
    #[test]
    fn allow_all_releases_only_a_move_to_another_site_before_untrusted_content() {
        let url = url::Url::parse("https://other.example/").unwrap();
        let held = |kind, tool: Option<&str>| {
            browser::HeldNavigation::new(kind, "GET", &url, None, true, tool)
        };
        let leaves = held(browser::HoldKind::LeavesSite, Some("browser_navigate"));
        let submits = held(browser::HoldKind::Submission, Some("browser_click"));
        let modes = ApprovalModes::default();

        // Ask mode: every held navigation gets its card.
        modes.note_decision(1, false);
        assert!(!releases(&modes, 1, &leaves));

        modes.set_chat(1, true);
        assert!(releases(&modes, 1, &leaves));
        assert!(!releases(&modes, 1, &submits), "a held submission asks");
        assert!(
            !releases(&modes, 1, &held(browser::HoldKind::LeavesSite, None)),
            "outside a call the page acted alone"
        );
        // Untrusted content entered the turn before the call in progress was decided.
        modes.note_decision(1, true);
        assert!(!releases(&modes, 1, &leaves));
        // A chat nothing is known about counts as untrusted; another chat's mode is its own.
        modes.set_chat(2, true);
        assert!(!releases(&modes, 2, &leaves));
        modes.note_decision(3, false);
        assert!(!releases(&modes, 3, &leaves), "chat 3 is in Ask");
    }

    /// The three modes, as the window sends and reads them.
    #[test]
    fn the_modes_are_named_as_the_window_names_them() {
        for (mode, name) in [
            (ApprovalMode::Ask, "ask"),
            (ApprovalMode::AllowAll, "allowAll"),
            (ApprovalMode::FullAccess, "fullAccess"),
        ] {
            assert_eq!(json!(mode), json!(name));
            assert_eq!(
                serde_json::from_value::<ApprovalMode>(json!(name)).unwrap(),
                mode
            );
        }
        assert!(serde_json::from_value::<ApprovalMode>(json!("full_access")).is_err());
        assert!(serde_json::from_value::<ApprovalMode>(json!(true)).is_err());
    }

    /// Full access is a mode of its own: a chat in it is not "in Allow all", a default of
    /// Full access reaches every chat, and changing the default ends every chat's own
    /// choice.
    #[test]
    fn full_access_is_a_third_mode_with_the_same_rules() {
        let modes = ApprovalModes::default();
        modes.set_chat_mode(1, ApprovalMode::FullAccess);
        assert!(modes.full_access(1) && !modes.allow_all(1));
        assert_eq!(modes.mode(2), ApprovalMode::Ask);
        // The 0.0.4 switch turns a Full access chat into an Allow all one, or back to Ask.
        modes.set_chat(1, true);
        assert_eq!(modes.mode(1), ApprovalMode::AllowAll);
        modes.set_chat(1, false);
        assert_eq!(modes.mode(1), ApprovalMode::Ask);

        modes.set_chat_mode(3, ApprovalMode::AllowAll);
        modes.set_default_mode(ApprovalMode::FullAccess);
        for chat in [1, 3, 999] {
            assert!(modes.full_access(chat), "chat {chat}");
        }
        assert!(modes.default_on());
        modes.set_chat_mode(3, ApprovalMode::Ask);
        assert_eq!(modes.mode(3), ApprovalMode::Ask);
        modes.set_default_mode(ApprovalMode::AllowAll);
        assert_eq!(
            modes.mode(3),
            ApprovalMode::AllowAll,
            "a new default reaches it"
        );
        modes.set_chat_mode(4, ApprovalMode::FullAccess);
        modes.set_default_mode(ApprovalMode::Ask);
        for chat in [1, 3, 4, 999] {
            assert_eq!(modes.mode(chat), ApprovalMode::Ask, "chat {chat}");
        }
    }

    /// The saved default: the 0.0.5 key wins; without it the 0.0.4 key is read; both are
    /// written, the old one as "anything but Ask"; an unreadable value is Ask.
    #[tokio::test]
    async fn the_saved_default_reads_the_old_setting_and_writes_both() {
        let store = store::Store::open_in_memory().unwrap();
        store
            .set_setting(ALLOW_ALL_DEFAULT, &json!(true))
            .await
            .unwrap();
        let modes = ApprovalModes::default();
        load_default_into(&store, &modes).await;
        assert_eq!(modes.default_mode(), ApprovalMode::AllowAll, "from 0.0.4");

        save_default(&store, &modes, ApprovalMode::FullAccess)
            .await
            .unwrap();
        let saved: Option<String> = store.get_setting(APPROVAL_MODE_DEFAULT).await.unwrap();
        assert_eq!(saved.as_deref(), Some("fullAccess"));
        let old: Option<bool> = store.get_setting(ALLOW_ALL_DEFAULT).await.unwrap();
        assert_eq!(old, Some(true));
        // A saved Full access waits for the user: every chat starts in Ask first.
        let after_restart = ApprovalModes::default();
        load_default_into(&store, &after_restart).await;
        assert_eq!(after_restart.default_mode(), ApprovalMode::Ask);
        assert!(after_restart.full_access_pending());
        assert_eq!(after_restart.mode(1), ApprovalMode::Ask);
        assert!(after_restart.take_full_access_pending());
        assert!(!after_restart.full_access_pending());

        // 0.0.4 switched "Allow all in every chat" off afterwards: off wins.
        store
            .set_setting(ALLOW_ALL_DEFAULT, &json!(false))
            .await
            .unwrap();
        let downgraded = ApprovalModes::default();
        load_default_into(&store, &downgraded).await;
        assert_eq!(downgraded.default_mode(), ApprovalMode::Ask);
        assert!(!downgraded.full_access_pending());
        store
            .set_setting(ALLOW_ALL_DEFAULT, &json!(true))
            .await
            .unwrap();

        save_default(&store, &after_restart, ApprovalMode::Ask)
            .await
            .unwrap();
        let old: Option<bool> = store.get_setting(ALLOW_ALL_DEFAULT).await.unwrap();
        assert_eq!(old, Some(false));

        // Something that is not a mode is read as missing: the 0.0.4 setting decides.
        store
            .set_setting(APPROVAL_MODE_DEFAULT, &json!("everything"))
            .await
            .unwrap();
        store
            .set_setting(ALLOW_ALL_DEFAULT, &json!(false))
            .await
            .unwrap();
        let odd = ApprovalModes::default();
        load_default_into(&store, &odd).await;
        assert_eq!(odd.default_mode(), ApprovalMode::Ask);
    }

    /// In Full access every held navigation of the chat's own call is let through: a
    /// submission, a move after untrusted content. One held outside a call (or from a tab the
    /// call does not work in) is not the chat's: the browser is shared, so it keeps its card.
    #[test]
    fn full_access_releases_every_held_navigation_of_its_own_call() {
        let url = url::Url::parse("https://other.example/").unwrap();
        let held = |kind, tool: Option<&str>| {
            browser::HeldNavigation::new(kind, "GET", &url, None, true, tool)
        };
        let modes = ApprovalModes::default();
        modes.set_chat_mode(1, ApprovalMode::FullAccess);
        modes.note_decision(1, true);
        for h in [
            held(browser::HoldKind::LeavesSite, Some("browser_navigate")),
            held(browser::HoldKind::Submission, Some("browser_click")),
        ] {
            assert!(releases(&modes, 1, &h));
            assert!(!releases(&modes, 2, &h), "chat 2 is in Ask");
        }
        for kind in [browser::HoldKind::LeavesSite, browser::HoldKind::Submission] {
            assert!(!releases(&modes, 1, &held(kind, None)), "outside a call");
        }
    }

    /// The generic `set_setting` command takes a fixed list of plain preferences. The
    /// default mode must never be on it: it has its own command, which only the user's
    /// click in the main window reaches.
    #[test]
    fn the_generic_settings_command_cannot_switch_the_default() {
        let source = include_str!("models_cmd.rs");
        let start = source
            .find("const ALLOWED: &[&str] = &[")
            .expect("the allow-list of set_setting");
        let list = &source[start..start + source[start..].find("];").expect("its end")];
        assert!(list.contains("\"theme\""), "this is the list: {list}");
        assert!(!list.contains(ALLOW_ALL_DEFAULT), "{list}");
        assert!(!list.contains(APPROVAL_MODE_DEFAULT), "{list}");
        assert!(!list.to_lowercase().contains("fullaccess"), "{list}");
        assert!(!list.to_lowercase().contains("allow_all"), "{list}");
        assert!(!list.contains("approvals::"), "{list}");
    }

    /// "Forget everything I allowed in this chat" ends Allow all in that chat, also when
    /// every chat starts in it.
    #[test]
    fn forgetting_what_was_allowed_puts_the_chat_back_to_asking() {
        // Chat ids of this test alone: `ask_first` works on the app's own modes.
        const CHAT: i64 = 904_201;
        const OTHER: i64 = 904_202;
        modes().set_chat(CHAT, true);
        modes().set_chat(OTHER, true);
        ask_first(CHAT);
        assert!(!modes().allow_all(CHAT));
        assert!(modes().allow_all(OTHER), "another chat is not touched");
        modes().set_chat(OTHER, false);

        // Full access ends the same way.
        modes().set_chat_mode(CHAT, ApprovalMode::FullAccess);
        ask_first(CHAT);
        assert_eq!(modes().mode(CHAT), ApprovalMode::Ask);

        // With "Allow all in every chat" on, the chat still goes back to asking.
        let all = ApprovalModes::default();
        all.set_default(true);
        all.set_chat(1, false);
        assert!(!all.allow_all(1) && all.allow_all(2));
    }

    /// Full access reaches the whole computer: the chat's own folders keep their place, the
    /// home folder becomes the working folder when there is none, and the app's own
    /// folders stay forbidden.
    #[test]
    fn full_access_widens_the_folders_but_keeps_the_app_folders_forbidden() {
        let home = std::env::temp_dir();
        let whole = whole_computer(Some(&home));
        assert_eq!(whole[0], home.display().to_string());
        assert!(whole.len() >= 2, "a drive: {whole:?}");
        #[cfg(not(windows))]
        assert_eq!(whole[1], "/");
        #[cfg(windows)]
        assert!(
            whole[1..]
                .iter()
                .all(|r| r.len() == 3 && r.ends_with(":\\")),
            "{whole:?}"
        );

        let mut chat = mcp_host::ChatContext {
            chat_id: 1,
            approved_folders: vec!["/work".into()],
            working_folder: None,
            forbidden_folders: vec!["/app-data".into()],
            skill_folders: vec![],
            skill_roots: vec!["/app-data/skills".into()],
            result_cap_tokens: 2000,
            full_access: false,
        };
        let before = chat.clone();
        widen(&mut chat, &whole);
        assert!(chat.full_access);
        assert_eq!(chat.approved_folders[0], "/work");
        assert_eq!(&chat.approved_folders[1..], &whole[..]);
        assert_eq!(chat.working_folder.as_deref(), Some(whole[0].as_str()));
        assert_eq!(chat.forbidden_folders, before.forbidden_folders);
        assert_eq!(chat.skill_roots, before.skill_roots);
        // A chat with its own working folder keeps it.
        let mut own = mcp_host::ChatContext {
            working_folder: Some("/work".into()),
            ..before
        };
        widen(&mut own, &whole);
        assert_eq!(own.working_folder.as_deref(), Some("/work"));
    }

    #[test]
    fn only_the_main_window_changes_a_mode() {
        assert!(only_main_window(crate::MAIN_WINDOW).is_ok());
        for other in [crate::OVERLAY_WINDOW, "indicator", "", "Main", "main2"] {
            assert!(only_main_window(other).is_err(), "{other:?}");
        }
    }
}
