//! Pinned instructions (0.0.4): short texts the user keeps in front of the model, for one
//! chat or for every chat. They follow the system prompt in every request of a turn
//! (`agent::prompt`), so trimming and summarising never remove them.
//!
//! What must hold:
//!
//! - A pin is an instruction from the user, and only the user makes or changes one: the
//!   commands below, which are granted to the main window alone and check that it is the
//!   caller. They take the text the user typed in the Context tab. No tool reaches this
//!   module (there is no tool for pins), nothing the model writes is read here, and no
//!   other module writes a pin (a test reads the sources to keep it so).
//! - A pin changes the prompt and nothing else. Its text is stored and sent; the app never
//!   reads it for meaning. Whatever it says, approvals, grants, approved folders and tool
//!   switches stay as they are (`crates/store/src/pinned.rs` writes to one table).
//! - Pins are plain text in the app's database. The Context tab says not to put passwords
//!   or keys in one. Pin text is never logged.
//! - Limits live in the store: 20 pins that apply to a chat, 1,000 characters each, none
//!   empty. A refusal comes back as the store's own sentence.
//! - Pins are never dropped to make a request fit. A turn whose pins take more than their
//!   share of the context does not start (`pinned_do_not_fit`, with the numbers).
//! - A helper started by `run_subagent` gets the chat's pins like the chat's system
//!   prompt: `turns::run` sets them before it copies the turn's input for helpers.

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// Event to the main window: pins changed. The payload is the chat whose own pins
/// changed, or `null` when pins for every chat did. It carries no text; the window reads
/// the list again.
const CHANGED: &str = "pinned-changed";

/// One pin, as the Context tab shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinDto {
    pub id: i64,
    /// The instruction. The user's own text: shown as plain text.
    pub text: String,
    /// For every chat (`true`) or for this chat only.
    pub every_chat: bool,
}

/// The pins that apply to a chat, in the order the model gets them: those for every chat,
/// then the chat's own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PinsDto {
    pub pins: Vec<PinDto>,
    /// The most pins that may apply to one chat.
    pub max: usize,
    /// The most characters a pin may have.
    pub max_chars: usize,
}

impl From<store::Pin> for PinDto {
    fn from(pin: store::Pin) -> Self {
        Self {
            every_chat: pin.for_every_chat(),
            id: pin.id,
            text: pin.text,
        }
    }
}

/// A store error as the Context tab shows it. A refusal (an empty or over-long pin, too
/// many pins) is the store's own sentence and is not written to the log: it can name a
/// chat, and it is not a fault of the app.
fn shown(error: store::StoreError) -> AppError {
    match error {
        store::StoreError::InvalidInput(message) => AppError {
            code: "pinned_refused".into(),
            message,
            next_step: String::new(),
            details: String::new(),
        },
        store::StoreError::NotFound { what: "chat", .. } => {
            AppError::bad_request("That chat no longer exists.")
        }
        store::StoreError::NotFound {
            what: "message", ..
        } => AppError::bad_request("Only a message you wrote in this chat can be pinned."),
        store::StoreError::NotFound { .. } => AppError::not_found("pinned instruction"),
        other => other.into(),
    }
}

/// Only the main window may change a pin (the capability files grant these commands to
/// it alone; this is the same rule checked again where the change is made).
fn only_main_window(label: &str) -> AppResult<()> {
    if label == crate::MAIN_WINDOW {
        Ok(())
    } else {
        Err(AppError::bad_request(
            "Only the main window can read or change pinned instructions.",
        ))
    }
}

async fn listing(store: &store::Store, chat_id: i64) -> AppResult<PinsDto> {
    let pins = store
        .pins_for_chat(store::ChatId(chat_id))
        .await
        .map_err(shown)?;
    Ok(PinsDto {
        pins: pins.into_iter().map(PinDto::from).collect(),
        max: store::MAX_PINS_PER_CHAT,
        max_chars: store::MAX_PIN_CHARS,
    })
}

/// Tells the main window that pins changed: for one chat, or (`None`) for every chat.
fn tell_window(app: &AppHandle, chat_id: Option<i64>) {
    let _ = app.emit_to(crate::MAIN_WINDOW, CHANGED, chat_id);
}

/// The pinned instructions a turn in this chat carries, in order: those for every chat,
/// then the chat's own.
///
/// Fails closed: if the database cannot be read, the turn does not start (nothing has been
/// sent yet), rather than going on without instructions the user relies on. For a real
/// turn (`turn` true) the end-of-turn clean-up (`computer_cmd::end_turn`) is done here on
/// that path, because the caller returns at once; the Context tab's preview (`turn` false)
/// began nothing and changes nothing.
pub async fn for_turn(
    app: &AppHandle,
    chat_id: store::ChatId,
    turn: bool,
) -> AppResult<Vec<String>> {
    let state = app.state::<AppState>();
    let pins = texts(state.store.pins_for_chat(chat_id).await);
    if pins.is_err() && turn {
        crate::computer_cmd::end_turn(app);
    }
    pins
}

/// The texts of the pins read for a turn, or the refusal that stops the turn.
fn texts(read: Result<Vec<store::Pin>, store::StoreError>) -> AppResult<Vec<String>> {
    match read {
        Ok(pins) => Ok(pins.into_iter().map(|pin| pin.text).collect()),
        Err(e) => Err(AppError::new(
            "pinned_unreadable",
            "The app could not read this chat's pinned instructions, so nothing was sent.",
            "Try again. If it keeps happening, restart the app.",
            e.to_string(),
        )),
    }
}

/// The advice under a turn that did not start because the pins do not fit
/// (`pinned_do_not_fit`). `local` is true for the app's own model server; the app does not
/// choose a remote endpoint's context size.
pub fn next_step(local: bool) -> &'static str {
    if local {
        "Remove or shorten pinned instructions in the Context tab, or choose Model settings to let this model remember more."
    } else {
        "Remove or shorten pinned instructions in the Context tab. If the server's model has a larger context, set it in Settings → Endpoints."
    }
}

/// The pins that apply to a chat.
#[tauri::command]
pub async fn list_pins(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    chat_id: i64,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    listing(&state.store, chat_id).await
}

/// Pins what the user typed, for this chat or for every chat. Returns the chat's pins.
#[tauri::command]
pub async fn add_pin(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    text: String,
    every_chat: bool,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    state
        .store
        .add_pin(store::ChatId(chat_id), every_chat, text)
        .await
        .map_err(shown)?;
    tracing::info!(chat_id, every_chat, "an instruction was pinned by the user");
    tell_window(&app, (!every_chat).then_some(chat_id));
    listing(&state.store, chat_id).await
}

/// Pins one of the user's own messages for this chat. Takes the message's id only: the text
/// is read from the database, and the store refuses a message that is not the user's or
/// not in this chat, so nothing but the user's own stored words can become a pin. Returns
/// the chat's pins.
#[tauri::command]
pub async fn pin_message(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    message_id: i64,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    state
        .store
        .pin_message(store::ChatId(chat_id), message_id)
        .await
        .map_err(shown)?;
    tracing::info!(chat_id, "a message was pinned by the user");
    tell_window(&app, Some(chat_id));
    listing(&state.store, chat_id).await
}

/// Replaces a pin's text with what the user typed. Returns the chat's pins.
#[tauri::command]
pub async fn edit_pin(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    text: String,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    let pin = state
        .store
        .edit_pin(store::ChatId(chat_id), id, text)
        .await
        .map_err(shown)?;
    tell_window(&app, pin.chat_id.map(|chat| chat.0));
    listing(&state.store, chat_id).await
}

/// Removes a pin. Returns the chat's pins.
#[tauri::command]
pub async fn remove_pin(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    state
        .store
        .remove_pin(store::ChatId(chat_id), id)
        .await
        .map_err(shown)?;
    // Which kind it was is gone with it: every list is read again.
    tell_window(&app, None);
    listing(&state.store, chat_id).await
}

/// Moves a pin one place up or down among the pins of its kind. Returns the chat's pins.
#[tauri::command]
pub async fn move_pin(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    up: bool,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    state
        .store
        .move_pin(store::ChatId(chat_id), id, up)
        .await
        .map_err(shown)?;
    tell_window(&app, None);
    listing(&state.store, chat_id).await
}

/// Makes a pin one for every chat, or one for this chat only. Returns the chat's pins.
#[tauri::command]
pub async fn set_pin_scope(
    window: tauri::WebviewWindow,
    app: AppHandle,
    state: State<'_, AppState>,
    chat_id: i64,
    id: i64,
    every_chat: bool,
) -> AppResult<PinsDto> {
    only_main_window(window.label())?;
    state
        .store
        .set_pin_scope(store::ChatId(chat_id), id, every_chat)
        .await
        .map_err(shown)?;
    // Either way the pins for every chat changed.
    tell_window(&app, None);
    listing(&state.store, chat_id).await
}

#[cfg(test)]
mod tests;
