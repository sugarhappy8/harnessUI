//! Chats, messages and streaming replies.
//!
//! Every reply is an agent turn (`turns::run`): the model may call the tools switched on
//! for the chat, and each message of the turn is stored as it completes.

use agent::OpenAiClient;
use serde_json::json;
use tauri::ipc::Channel;
use tauri::{Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::dto::{ChatDetailDto, ChatDto, ChatSummaryDto, MessageDto};
use crate::error::{AppError, AppResult};
use crate::state::{AppState, Generation};
use crate::turns::TurnStreamEvent;

const NEW_CHAT_TITLE: &str = "New chat";

/// Extracts the plain text of a stored message.
pub fn text_of(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts
            .iter()
            .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn text_content(text: &str) -> serde_json::Value {
    json!([{ "type": "text", "text": text }])
}

/// A user message's content: its text, then its attachment parts (images, documents and
/// mentions) in order.
fn user_content(text: &str, parts: Vec<serde_json::Value>) -> serde_json::Value {
    let mut content = text_content(text);
    if let Some(all) = content.as_array_mut() {
        all.extend(parts);
    }
    content
}

/// [0.0.7 P6] Edit and resend: the user message being edited, and the attachments of it
/// (images, documents and mentions, by attachment id) that the new message keeps.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CopyAttachments {
    pub message_id: store::MessageId,
    pub attachment_ids: Vec<i64>,
}

/// [0.0.7 P6] The attachment parts of the edited message that the new one keeps, as they were
/// stored, in their order: an image is the same file, a document or a mention the same text
/// the model was sent (a mention is not read from its file again). Only a user message of
/// `chat_id` lends its attachments, and only ones of that chat; an id it does not carry is
/// refused, never dropped without a word.
async fn copied_parts(
    store: &store::Store,
    chat_id: store::ChatId,
    copy: &CopyAttachments,
) -> AppResult<Vec<serde_json::Value>> {
    let from = store
        .get_message(copy.message_id)
        .await?
        .filter(|m| m.chat_id == chat_id && m.role == store::Role::User)
        .ok_or_else(|| {
            AppError::bad_request("That message's attachments can't be sent again here.")
        })?;
    let mut parts = Vec::new();
    let mut found = std::collections::HashSet::new();
    for part in from.content.as_array().into_iter().flatten() {
        if !matches!(
            part.get("type").and_then(|t| t.as_str()),
            Some("image" | "document")
        ) {
            continue;
        }
        let Some(id) = part.get("attachment_id").and_then(|i| i.as_i64()) else {
            continue;
        };
        if !copy.attachment_ids.contains(&id) || !found.insert(id) {
            continue;
        }
        let row = store.get_attachment(store::AttachmentId(id)).await?;
        if row.is_none_or(|a| a.chat_id != chat_id) {
            return Err(AppError::new(
                "attachment_missing",
                "An attachment of that message is no longer there.",
                "Attach the file again.",
                format!("attachment {id}"),
            ));
        }
        parts.push(part.clone());
    }
    if copy.attachment_ids.iter().any(|id| !found.contains(id)) {
        return Err(AppError::new(
            "attachment_missing",
            "An attachment of that message is no longer there.",
            "Attach the file again.",
            String::new(),
        ));
    }
    Ok(parts)
}

/// [0.0.7 P6] The main window's event naming the chat whose reply is being written (`null`
/// when none): sent when a turn takes the slot and when it gives it back.
const TURN_RUNNING: &str = "turn-running";

/// [0.0.7 P6] A chat's hold on the one reply the app writes at a time. A send takes it before
/// anything of the turn is stored, so a send refused as busy leaves no message behind; it is
/// given back when dropped, also on an early error. The main window is told which chat holds
/// it ([`TURN_RUNNING`]), and can ask after a reload ([`running_turn`]).
struct TurnSlot<'a> {
    generation: &'a Generation,
    id: u64,
    chat_id: store::ChatId,
    cancel: CancellationToken,
    /// Where to say so; `None` in tests.
    app: Option<tauri::AppHandle>,
}

impl<'a> TurnSlot<'a> {
    fn take(
        generation: &'a Generation,
        chat_id: store::ChatId,
        app: Option<&tauri::AppHandle>,
    ) -> AppResult<Self> {
        let Some((id, cancel)) = generation.start(chat_id.0) else {
            return Err(AppError::new(
                "busy",
                "A reply is already being generated.",
                "Wait for it to finish or press Stop.",
                String::new(),
            ));
        };
        if let Some(app) = app {
            let _ = app.emit_to(crate::MAIN_WINDOW, TURN_RUNNING, Some(chat_id.0));
        }
        Ok(Self {
            generation,
            id,
            chat_id,
            cancel,
            app: app.cloned(),
        })
    }
}

impl Drop for TurnSlot<'_> {
    fn drop(&mut self) {
        self.generation.finish(self.id);
        if let Some(app) = &self.app {
            // A newer turn may hold it already (one started while this one wound down).
            let _ = app.emit_to(
                crate::MAIN_WINDOW,
                TURN_RUNNING,
                self.generation.running_chat(),
            );
        }
    }
}

/// [0.0.7 P6] Stores the user's message for the turn that holds `slot`, in the slot's chat:
/// a message is never stored for a turn that cannot start.
async fn store_user_message(
    store: &store::Store,
    slot: &TurnSlot<'_>,
    parent_id: Option<store::MessageId>,
    content: serde_json::Value,
    search_text: String,
) -> AppResult<store::MessageId> {
    Ok(store
        .insert_message(store::NewMessage {
            chat_id: slot.chat_id,
            parent_id,
            role: store::Role::User,
            content,
            reasoning: None,
            status: store::MessageStatus::Complete,
            model_ref: None,
            search_text,
        })
        .await?)
}

fn chat_dto(c: &store::Chat) -> ChatDto {
    ChatDto {
        id: c.id,
        title: c.title.clone(),
        model_ref: c.model_ref.clone(),
        system_prompt: c.system_prompt.clone(),
        leaf_message_id: c.leaf_message_id,
        created_at: c.created_at,
        updated_at: c.updated_at,
        thinking: !serde_json::from_value::<crate::turns::ChatTools>(c.tool_prefs.clone())
            .unwrap_or_default()
            .thinking_off,
        working_folder: c.working_folder.clone(),
        // [0.0.7 P5] Filled by `get_chat`, which reads the endpoint.
        cloud_host: None,
        endpoint_thinking: false,
    }
}

async fn message_dto(state: &AppState, m: &store::Message) -> AppResult<MessageDto> {
    let siblings = state.store.siblings(m.id).await?;
    // The agent loop stores a placeholder answer when a turn is stopped, so the history
    // stays valid for the next request; the chat shows it as a "Stopped" label.
    let mut text = text_of(&m.content);
    let mut status = m.status.as_str().to_string();
    if m.role == store::Role::Assistant && text.trim() == agent::history::STOPPED_TEXT {
        text.clear();
        status = store::MessageStatus::Stopped.as_str().to_string();
    }
    Ok(MessageDto {
        id: m.id,
        parent_id: m.parent_id,
        role: m.role.as_str().to_string(),
        text,
        reasoning: m.reasoning.clone(),
        status,
        model_ref: m.model_ref.clone(),
        prompt_tokens: m.prompt_tokens,
        completion_tokens: m.completion_tokens,
        tokens_per_second: m.tokens_per_second,
        error: m
            .error
            .as_ref()
            .and_then(|e| serde_json::from_value::<AppErrorShape>(e.clone()).ok())
            .map(|e| AppError {
                code: e.code,
                message: e.message,
                next_step: e.next_step,
                details: e.details,
            }),
        created_at: m.created_at,
        sibling_ids: siblings,
        image_ids: crate::attachments::image_ids(&m.content),
        documents: crate::attachments::document_refs(&m.content)
            .into_iter()
            .map(|(id, name)| crate::dto::DocumentRef { id, name })
            .collect(),
        tool_calls: tool_calls_of(&m.content),
        tool_result: tool_result_of(&m.content),
        mentions: crate::mentions::mention_refs(&m.content),
    })
}

fn tool_calls_of(content: &serde_json::Value) -> Vec<crate::dto::ToolCallDto> {
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    parts
        .iter()
        .filter(|p| p.get("type").and_then(|t| t.as_str()) == Some("tool_calls"))
        .filter_map(|p| {
            serde_json::from_value::<Vec<agent::CallRecord>>(p.get("calls")?.clone()).ok()
        })
        .flatten()
        .map(|c| crate::dto::ToolCallDto {
            id: c.id,
            name: c.name,
            arguments: serde_json::Value::Object(c.arguments),
            recovered: c.recovered,
        })
        .collect()
}

fn tool_result_of(content: &serde_json::Value) -> Option<crate::dto::ToolResultDto> {
    let parts = content.as_array()?;
    let p = parts
        .iter()
        .find(|p| p.get("type").and_then(|t| t.as_str()) == Some("tool_result"))?;
    let r: agent::ResultRecord = serde_json::from_value(p.get("result")?.clone()).ok()?;
    let text = crate::turns::shown_result_text(&r);
    Some(crate::dto::ToolResultDto {
        call_id: r.call_id,
        name: r.name,
        status: r.status.label().to_string(),
        text,
        image_count: r.images.len(),
    })
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppErrorShape {
    code: String,
    message: String,
    next_step: String,
    #[serde(default)]
    details: String,
}

#[tauri::command]
pub async fn list_chats(
    state: State<'_, AppState>,
    query: Option<String>,
) -> AppResult<Vec<ChatSummaryDto>> {
    let rows = state
        .store
        .list_chats(store::ListChats {
            query,
            limit: 500,
            offset: 0,
            include_archived: false,
        })
        .await?;
    Ok(rows
        .into_iter()
        .map(|c| ChatSummaryDto {
            id: c.id,
            title: c.title,
            updated_at: c.updated_at,
            snippet: c.snippet,
        })
        .collect())
}

#[tauri::command]
pub async fn create_chat(
    state: State<'_, AppState>,
    model_ref: Option<String>,
) -> AppResult<ChatDto> {
    let id = state
        .store
        .create_chat(store::NewChat {
            title: NEW_CHAT_TITLE.to_string(),
            model_ref,
            system_prompt: None,
        })
        .await?;
    let chat = state
        .store
        .get_chat(id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    Ok(chat_dto(&chat))
}

#[tauri::command]
pub async fn get_chat(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
) -> AppResult<ChatDetailDto> {
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let path = state.store.current_path(chat_id).await?;
    let mut messages = Vec::with_capacity(path.len());
    for m in &path {
        messages.push(message_dto(&state, m).await?);
    }
    let mut dto = chat_dto(&chat);
    // [0.0.4 profile] Thinking as the next turn has it: the user's choice, else the
    // model's tested settings, else on.
    dto.thinking = crate::evals::profile::chat_thinking(&state, &chat).await;
    // [0.0.7 P5] The "Cloud" mark and Claude's Thinking switch.
    (dto.cloud_host, dto.endpoint_thinking) =
        crate::providers::chat_marks(&state, chat.model_ref.as_deref()).await;
    Ok(ChatDetailDto {
        chat: dto,
        messages,
        summary: earlier_summary(&state.store, chat_id, &path).await, // [0.0.5 memory]
    })
}

/// [0.0.5 memory] The summary the model is sent instead of the earlier messages of `path`
/// (the shown branch), chosen as a turn chooses it (`turns::latest_summary`). A summary
/// that cannot be read is left out: the chat still opens. [0.0.6] The summary is model
/// output: characters that change how text is shown without being seen (tag characters,
/// bidirectional overrides, zero-width characters) come as visible escapes, as on approval
/// cards (`policy::sanitize::visible`); see `visible_summary` for exactly which.
async fn earlier_summary(
    store: &store::Store,
    chat_id: store::ChatId,
    path: &[store::Message],
) -> Option<crate::dto::EarlierSummaryDto> {
    let ids: Vec<store::MessageId> = path.iter().map(|m| m.id).collect();
    let summary = store
        .latest_summary_on_path(chat_id, &ids)
        .await
        .ok()
        .flatten()?;
    Some(crate::dto::EarlierSummaryDto {
        last_message_id: summary.last_message_id,
        text: visible_summary(&summary.text),
    })
}

/// [0.0.6] A summary as Show summary shows it: the characters `policy::sanitize::visible`
/// escapes, plus the soft hyphen and the combining grapheme joiner, as visible escapes, and
/// Windows line ends as plain ones. Other hidden characters stay as written: the blank
/// Braille pattern, the Arabic number signs and the other format controls outside that
/// set, and the reserved code points U+FFF0..FFF8, U+E0080..E00FF and U+E01F0..E0FFF. The
/// model gets the text as `agent::prompt::defang_app_notes` leaves it: chat-template
/// control tokens broken, block tags (also when split by hidden characters) escaped,
/// "[App note]" markers bracketed, control and tag characters and the default-ignorable
/// code points assigned to nothing (those reserved ones included) escaped, and other
/// hidden characters escaped only in and right next to a marker or a split block tag.
fn visible_summary(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in policy::sanitize::visible(&text.replace("\r\n", "\n")).chars() {
        if matches!(c, '\u{00AD}' | '\u{034F}') {
            out.push_str(&format!("\\u{{{:04X}}}", c as u32));
        } else {
            out.push(c);
        }
    }
    out
}

#[tauri::command]
pub async fn rename_chat(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    title: String,
) -> AppResult<()> {
    let title = title.trim();
    if title.is_empty() {
        return Err(AppError::bad_request("A chat needs a title."));
    }
    let title: String = title.chars().take(200).collect();
    state.store.rename_chat(chat_id, title).await?;
    Ok(())
}

#[tauri::command]
pub async fn delete_chat(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: store::ChatId,
) -> AppResult<()> {
    // A reply being generated in this chat is stopped first, with its tools: nothing keeps
    // running, or acting under the chat's grants, for a chat that is gone. The turn then
    // stores its last message before the chat's rows are deleted.
    if state.generation.cancel_chat(chat_id.0) {
        crate::computer_cmd::release_all(&app);
        crate::turns::cancel_all_pending(&app);
        for _ in 0..200 {
            if !state.generation.running_in(chat_id.0) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }
    state.store.delete_chat(chat_id).await?;
    Ok(())
}

#[tauri::command]
pub async fn set_chat_model(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    model_ref: Option<String>,
) -> AppResult<()> {
    state
        .store
        .update_chat(
            chat_id,
            store::ChatPatch {
                model_ref: Some(model_ref),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}

#[tauri::command]
pub async fn set_system_prompt(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    system_prompt: Option<String>,
) -> AppResult<()> {
    state
        .store
        .update_chat(
            chat_id,
            store::ChatPatch {
                system_prompt: Some(system_prompt),
                ..Default::default()
            },
        )
        .await?;
    Ok(())
}

/// Makes `message_id`'s branch current: follows the newest child at each step down
/// to a leaf.
#[tauri::command]
pub async fn switch_branch(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    message_id: store::MessageId,
) -> AppResult<()> {
    let mut leaf = message_id;
    for _ in 0..10_000 {
        let children = state.store.children(leaf).await?;
        match children.last() {
            Some(&next) => leaf = next,
            None => break,
        }
    }
    state.store.set_leaf(chat_id, leaf).await?;
    Ok(())
}

/// Stop. [0.0.7 P6] With a chat (the Stop of a chat on screen), only that chat's reply: never
/// one that runs in another chat. Without one, whatever runs.
#[tauri::command]
pub async fn stop_generation(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: Option<store::ChatId>,
) -> AppResult<()> {
    if stop_reply(&state.generation, chat_id) {
        crate::computer_cmd::release_all(&app);
        crate::turns::cancel_all_pending(&app);
    }
    Ok(())
}

/// [0.0.7 P6] Stops the reply being written: with `chat_id`, only when it is that chat's.
/// True when its tools and cards are to be stopped too (always without a chat).
fn stop_reply(generation: &Generation, chat_id: Option<store::ChatId>) -> bool {
    match chat_id {
        Some(chat) => generation.cancel_chat(chat.0),
        None => {
            generation.cancel();
            true
        }
    }
}

/// [0.0.7 P6] The chat whose reply is being written, if any. A window that reloaded while a
/// reply ran learns of it here: that reply's own events went to the window before.
#[tauri::command]
pub fn running_turn(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
) -> AppResult<Option<i64>> {
    crate::commands::only_main_window(window.label())?;
    Ok(state.generation.running_chat())
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri commands take their arguments flat.
pub async fn send_message(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    text: String,
    parent_id: Option<store::MessageId>,
    images: Option<Vec<crate::attachments::ImageInput>>,
    documents: Option<Vec<crate::attachments::DocumentInput>>,
    mentions: Option<Vec<tools::symbols::resolve::MentionRequest>>,
    copy_attachments: Option<CopyAttachments>,
    on_event: Channel<TurnStreamEvent>,
) -> AppResult<()> {
    let text = text.trim().to_string();
    let images = images.unwrap_or_default();
    let documents = documents.unwrap_or_default();
    let mentions = mentions.unwrap_or_default();
    // [0.0.7 P6] Edit and resend: the edited message's attachments that the new one keeps.
    let copied = match &copy_attachments {
        Some(copy) => copied_parts(&state.store, chat_id, copy).await?,
        None => Vec::new(),
    };
    if text.is_empty()
        && images.is_empty()
        && documents.is_empty()
        && mentions.is_empty()
        && copied.is_empty()
    {
        return Err(AppError::bad_request("The message is empty."));
    }
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let target = resolve_target(&state, chat.model_ref.as_deref()).await?;
    let copied_image = copied
        .iter()
        .any(|p| p.get("type").and_then(|t| t.as_str()) == Some("image"));
    if (!images.is_empty() || copied_image) && target.local && !state.runtime.status().vision {
        return Err(AppError::new(
            "no_vision",
            "This model was loaded without an image file, so it can't read images.",
            "Download its image file in Models → Installed, pick a model with the vision badge, or remove the image.",
            String::new(),
        ));
    }
    // [0.0.7 P6] One reply at a time: a send while another runs (in any chat, also one the
    // window does not know of after a reload) is refused before anything is stored.
    let slot = TurnSlot::take(&state.generation, chat_id, Some(&app))?;
    // [0.0.4 mentions] The chips the user chose, read through the path sandbox first: one
    // that cannot be read fails the send before anything is stored.
    let n_ctx = target
        .n_ctx
        .unwrap_or(crate::settings::DEFAULT_REMOTE_CONTEXT) as usize;
    // [0.0.5 memory] A document's share of the context counts the message's mentions too,
    // and [0.0.6] a mention's share counts its documents: 40% of the context for them all.
    let shared_by = documents.len() + mentions.len();
    // [0.0.6] A local model's server counts each share in its own tokens, documents' and
    // mentions' alike.
    let counter = target.local.then_some(&target.client);
    let mention_parts = crate::mentions::attach(
        &app,
        &state,
        chat_id,
        mentions,
        n_ctx,
        documents.len(),
        counter,
    )
    .await?;
    let mut parts = copied;
    parts.extend(crate::attachments::save_images(&app, &state, chat_id, &images).await?);
    parts.extend(
        crate::attachments::save_documents(
            &app, &state, chat_id, &documents, n_ctx, shared_by, counter,
        )
        .await?,
    );
    parts.extend(mention_parts);
    let content = user_content(&text, parts);
    let user_id = store_user_message(&state.store, &slot, parent_id, content, text.clone()).await?;
    if chat.title == NEW_CHAT_TITLE {
        let first = if text.is_empty() {
            "Image"
        } else {
            text.lines().next().unwrap_or("")
        };
        let title: String = first.chars().take(48).collect();
        if !title.trim().is_empty() {
            let _ = state
                .store
                .rename_chat(chat_id, title.trim().to_string())
                .await;
        }
    }
    generate(
        &app,
        &state,
        &chat,
        user_id,
        Some(user_id),
        target,
        on_event,
        slot,
    )
    .await
}

#[tauri::command]
pub async fn regenerate(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    assistant_message_id: store::MessageId,
    on_event: Channel<TurnStreamEvent>,
) -> AppResult<()> {
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let old = state
        .store
        .get_message(assistant_message_id)
        .await?
        .ok_or_else(|| AppError::not_found("message"))?;
    // A turn may hold several messages (tool calls and results); regenerate answers the
    // user message the turn started from.
    let path = state.store.path_to(chat_id, old.id).await?;
    let parent = path
        .iter()
        .rev()
        .find(|m| m.role == store::Role::User)
        .map(|m| m.id)
        .ok_or_else(|| AppError::bad_request("This message has nothing to regenerate from."))?;
    let target = resolve_target(&state, chat.model_ref.as_deref()).await?;
    let slot = TurnSlot::take(&state.generation, chat_id, Some(&app))?;
    generate(&app, &state, &chat, parent, None, target, on_event, slot).await
}

/// Where a request goes.
pub(crate) struct Target {
    pub(crate) client: OpenAiClient,
    model: String,
    pub(crate) model_ref: String,
    pub(crate) local: bool,
    n_ctx: Option<u32>,
}

async fn resolve_target(state: &AppState, model_ref: Option<&str>) -> AppResult<Target> {
    target_for(state, model_ref, true).await
}

/// Where a turn's requests would go. `remember` false (the Context tab's view of the next
/// request) asks the same questions and records nothing: what a remote endpoint reports
/// is not kept for the Tools panel.
pub(crate) async fn target_for(
    state: &AppState,
    model_ref: Option<&str>,
    remember: bool,
) -> AppResult<Target> {
    let Some(model_ref) = model_ref else {
        return Err(AppError::new(
            "no_model",
            "No model is selected for this chat.",
            "Choose a model from the list at the top of the chat.",
            String::new(),
        ));
    };
    if let Some(path) = model_ref.strip_prefix("local:") {
        let Some((base_url, key, loaded, n_ctx)) = state.runtime.endpoint().await else {
            return Err(AppError::new(
                "model_not_loaded",
                "The model for this chat is not loaded.",
                "Pick it again from the list at the top of the chat to load it.",
                path.to_string(),
            ));
        };
        if loaded.to_string_lossy() != path {
            return Err(AppError::new(
                "model_not_loaded",
                "A different model is loaded right now.",
                "Pick this chat's model again from the list at the top to load it.",
                format!("loaded: {}\nwanted: {path}", loaded.display()),
            ));
        }
        let client = crate::runtime_manager::local_client(base_url, key)?;
        return Ok(Target {
            client,
            model: "local".to_string(),
            model_ref: model_ref.to_string(),
            local: true,
            n_ctx: Some(n_ctx),
        });
    }
    if let Some(rest) = model_ref.strip_prefix("remote:") {
        let (id, model) = rest
            .split_once(':')
            .ok_or_else(|| AppError::bad_request("This chat's model reference is malformed."))?;
        let id: i64 = id
            .parse()
            .map_err(|_| AppError::bad_request("This chat's model reference is malformed."))?;
        let (client, provider) =
            crate::providers::client_for(state, store::ProviderId(id)).await?;
        // A model that reports no context (or a failed check) gets the user's setting,
        // 4,096 by default (8.4); [0.0.7 P5] a cloud company's model the size the company
        // documents, and every endpoint at most the size chosen for it in Settings →
        // Endpoints.
        let reported = client.context_length(model).await.ok().flatten();
        if remember {
            crate::mcp_cmd::remember_context(model_ref, reported);
        }
        let n_ctx = Some(crate::providers::context_for(state, &provider, reported).await);
        return Ok(Target {
            client,
            model: model.to_string(),
            model_ref: model_ref.to_string(),
            local: false,
            n_ctx,
        });
    }
    Err(AppError::bad_request(
        "This chat's model reference is not recognised.",
    ))
}

/// Runs the turn that holds `slot` ([0.0.7 P6] taken by the caller before anything of the
/// turn was stored) and gives the slot back when it ends.
#[allow(clippy::too_many_arguments)]
async fn generate(
    app: &tauri::AppHandle,
    state: &AppState,
    chat: &store::Chat,
    reply_to: store::MessageId,
    user_message_id: Option<store::MessageId>,
    target: Target,
    on_event: Channel<TurnStreamEvent>,
    slot: TurnSlot<'_>,
) -> AppResult<()> {
    let cancel = slot.cancel.clone();
    let _busy = crate::runtime_manager::use_local_model(
        state,
        &target.model_ref,
        crate::state::BusyKind::ChatTurn,
        &cancel,
    );
    let (caps, template) = turn_settings(state, &target);
    let req = crate::turns::TurnRequest {
        chat: chat.clone(),
        reply_to,
        user_message_id,
        client: target.client,
        model_ref: target.model_ref,
        caps,
        template,
    };
    let result = crate::turns::run(app, req, on_event, cancel).await;
    drop(slot);
    result
}

/// What the model can do and the request template, for a turn sent to `target`.
pub(crate) fn turn_settings(
    state: &AppState,
    target: &Target,
) -> (agent::ModelCaps, agent::RequestTemplate) {
    let status = state.runtime.status();
    let kind = target.client.config().kind;
    let caps = agent::ModelCaps {
        tools: if target.local {
            status.supports_tools
        } else {
            true
        },
        vision: if target.local { status.vision } else { true },
        n_ctx: target
            .n_ctx
            .unwrap_or(crate::settings::DEFAULT_REMOTE_CONTEXT),
        local: target.local,
        // [0.0.7 P5] Claude's thinking is switched through its own API.
        thinking: (target.local && status.thinking) || kind == agent::ProviderKind::Anthropic,
        preserve_reasoning: target.local && status.preserve_reasoning,
        // [0.0.7 P5] Not at OpenAI, whose tool messages take text only.
        images_in_tool_results: !target.local && agent::images_in_tool_results(kind),
    };
    let mut template = agent::RequestTemplate {
        model: target.model.clone(),
        ..Default::default()
    };
    if target.local {
        template.extra.insert("cache_prompt".into(), json!(true));
    }
    (caps, template)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chat sent to the webview carries its working folder as `workingFolder`
    /// (src/lib/types.ts `Chat`).
    #[test]
    fn the_chat_dto_carries_the_working_folder() {
        let mut chat = store::Chat {
            id: store::ChatId(3),
            title: "t".into(),
            model_ref: None,
            system_prompt: None,
            leaf_message_id: None,
            tool_prefs: json!({}),
            working_folder: Some("/work/a".into()),
            created_at: 1,
            updated_at: 2,
            archived: false,
        };
        let v = serde_json::to_value(chat_dto(&chat)).unwrap();
        assert_eq!(v["workingFolder"], json!("/work/a"));
        chat.working_folder = None;
        let v = serde_json::to_value(chat_dto(&chat)).unwrap();
        assert_eq!(v["workingFolder"], serde_json::Value::Null);
    }

    /// [0.0.6] The summary is model output that can carry hidden text copied from a web page
    /// or a file (tag characters, a right-to-left override, zero-width characters). Show
    /// summary is where the user sees what the model is sent instead of the messages, so
    /// those characters come as visible escapes, as on approval cards; line breaks and tabs
    /// stay.
    #[tokio::test]
    async fn the_chat_detail_shows_hidden_characters_of_the_summary() {
        let store = store::Store::open_in_memory().unwrap();
        let chat = store
            .create_chat(store::NewChat {
                title: "t".into(),
                model_ref: None,
                system_prompt: None,
            })
            .await
            .unwrap();
        let first = store
            .insert_message(store::NewMessage {
                chat_id: chat,
                parent_id: None,
                role: store::Role::User,
                content: text_content("question"),
                reasoning: None,
                status: store::MessageStatus::Complete,
                model_ref: None,
                search_text: "question".into(),
            })
            .await
            .unwrap();
        store
            .insert_summary(store::NewSummary {
                chat_id: chat,
                last_message_id: first,
                text: "Goal: tidy up.\u{E0049}\u{E0047}\u{E004E}\r\n\tDone: \u{202E}txt.exe \
                       a\u{200B}b re\u{00AD}ad"
                    .into(),
            })
            .await
            .unwrap();
        let path = store.path_to(chat, first).await.unwrap();
        let summary = earlier_summary(&store, chat, &path)
            .await
            .expect("a summary");
        assert_eq!(
            summary.text,
            "Goal: tidy up.\\u{E0049}\\u{E0047}\\u{E004E}\n\tDone: \\u{202E}txt.exe \
             a\\u{200B}b re\\u{00AD}ad"
        );
    }

    /// [0.0.5 memory] The chat detail carries the summary of the shown branch, with the
    /// last message it stands for (where the chat draws its line), as `summary`
    /// (src/lib/types.ts `ChatDetail`); a branch the summary is not on gets none.
    #[tokio::test]
    async fn the_chat_detail_carries_the_summary_of_the_shown_branch() {
        let store = store::Store::open_in_memory().unwrap();
        let chat = store
            .create_chat(store::NewChat {
                title: "t".into(),
                model_ref: None,
                system_prompt: None,
            })
            .await
            .unwrap();
        let mut parent = None;
        let mut ids = Vec::new();
        for (role, text) in [
            (store::Role::User, "first question"),
            (store::Role::Assistant, "first answer"),
            (store::Role::User, "second question"),
        ] {
            let id = store
                .insert_message(store::NewMessage {
                    chat_id: chat,
                    parent_id: parent,
                    role,
                    content: text_content(text),
                    reasoning: None,
                    status: store::MessageStatus::Complete,
                    model_ref: None,
                    search_text: text.into(),
                })
                .await
                .unwrap();
            parent = Some(id);
            ids.push(id);
        }
        let path = store.path_to(chat, ids[2]).await.unwrap();
        assert_eq!(earlier_summary(&store, chat, &path).await, None);
        store
            .insert_summary(store::NewSummary {
                chat_id: chat,
                last_message_id: ids[1],
                text: "Goal: <b>answer</b> two questions.".into(),
            })
            .await
            .unwrap();
        let summary = earlier_summary(&store, chat, &path)
            .await
            .expect("a summary");
        assert_eq!(summary.last_message_id, ids[1]);
        let dto = ChatDetailDto {
            chat: chat_dto(&store.get_chat(chat).await.unwrap().unwrap()),
            messages: Vec::new(),
            summary: Some(summary),
        };
        let v = serde_json::to_value(&dto).unwrap();
        assert_eq!(v["summary"]["lastMessageId"], json!(ids[1].0));
        // The text as the model wrote it: escaping is the webview's (plain text).
        assert_eq!(
            v["summary"]["text"],
            json!("Goal: <b>answer</b> two questions.")
        );
        // Another branch from the first question has no summary.
        let other = store
            .insert_message(store::NewMessage {
                chat_id: chat,
                parent_id: Some(ids[0]),
                role: store::Role::Assistant,
                content: text_content("another answer"),
                reasoning: None,
                status: store::MessageStatus::Complete,
                model_ref: None,
                search_text: String::new(),
            })
            .await
            .unwrap();
        let other_path = store.path_to(chat, other).await.unwrap();
        assert_eq!(earlier_summary(&store, chat, &other_path).await, None);
        let none = ChatDetailDto {
            summary: None,
            ..dto
        };
        assert_eq!(
            serde_json::to_value(&none).unwrap()["summary"],
            serde_json::Value::Null
        );
    }

    async fn new_chat(store: &store::Store) -> store::ChatId {
        store
            .create_chat(store::NewChat {
                title: "t".into(),
                model_ref: None,
                system_prompt: None,
            })
            .await
            .unwrap()
    }

    async fn attachment(store: &store::Store, chat: store::ChatId, mime: &str, name: &str) -> i64 {
        store
            .insert_attachment(store::NewAttachment {
                chat_id: chat,
                sha256: format!("{name}-sha"),
                mime: mime.into(),
                file_name: name.into(),
                size_bytes: 3,
                bytes_path: format!("/attachments/{name}"),
                extracted_text: (mime == "text/plain").then(|| "def add(a, b):".into()),
            })
            .await
            .unwrap()
            .0
    }

    async fn insert(
        store: &store::Store,
        chat: store::ChatId,
        parent: Option<store::MessageId>,
        role: store::Role,
        content: serde_json::Value,
    ) -> store::MessageId {
        store
            .insert_message(store::NewMessage {
                chat_id: chat,
                parent_id: parent,
                role,
                content,
                reasoning: None,
                status: store::MessageStatus::Complete,
                model_ref: None,
                search_text: String::new(),
            })
            .await
            .unwrap()
    }

    /// [0.0.7 P6] Edit and resend keeps the attachments left in the edit box: the new user
    /// message carries the same image, document and mention parts as stored (the same files,
    /// the same text the model was sent), in the message's order, and leaves out the ones
    /// removed. Before, it carried the new text only, and the model answered without them.
    #[tokio::test]
    async fn an_edited_message_keeps_the_attachments_it_was_sent_with() {
        let store = store::Store::open_in_memory().unwrap();
        let chat = new_chat(&store).await;
        let image = attachment(&store, chat, "image/png", "screenshot.png").await;
        let doc = attachment(&store, chat, "text/plain", "notes.txt").await;
        let mention = attachment(&store, chat, "text/plain", "calc.py:add").await;
        let image_part = json!({ "type": "image", "attachment_id": image });
        let doc_part = json!({
            "type": "document", "attachment_id": doc, "name": "notes.txt",
            "cut": { "kept": 10, "total": 20 },
        });
        let mention_part = json!({
            "type": "document", "attachment_id": mention, "name": "calc.py:add",
            "mention": {
                "path": "/w/calc.py", "kind": "function", "symbol": "add", "container": null,
                "startLine": 4, "endLine": 5, "truncated": false,
            },
        });
        let hello = insert(&store, chat, None, store::Role::User, text_content("hi")).await;
        let reply = insert(
            &store,
            chat,
            Some(hello),
            store::Role::Assistant,
            text_content("hello"),
        )
        .await;
        let original = insert(
            &store,
            chat,
            Some(reply),
            store::Role::User,
            user_content(
                "What's wrong here?",
                vec![image_part.clone(), doc_part.clone(), mention_part.clone()],
            ),
        )
        .await;

        // The text is edited and the document removed in the edit box.
        let copy = CopyAttachments {
            message_id: original,
            attachment_ids: vec![mention, image],
        };
        let parts = copied_parts(&store, chat, &copy).await.unwrap();
        assert_eq!(parts, vec![image_part.clone(), mention_part.clone()]);
        let generation = Generation::default();
        let slot = TurnSlot::take(&generation, chat, None).unwrap();
        let text = "What is wrong in this picture?";
        let edited = store_user_message(
            &store,
            &slot,
            Some(reply),
            user_content(text, parts),
            text.into(),
        )
        .await
        .unwrap();
        drop(slot);
        let new = store.get_message(edited).await.unwrap().unwrap();
        assert_eq!((new.chat_id, new.role), (chat, store::Role::User));
        assert_eq!(text_of(&new.content), text);
        assert_eq!(crate::attachments::image_ids(&new.content), vec![image]);
        assert!(crate::attachments::document_refs(&new.content).is_empty());
        let mentions = crate::mentions::mention_refs(&new.content);
        assert_eq!(mentions.len(), 1);
        assert_eq!(
            (mentions[0].attachment_id, mentions[0].label.as_str()),
            (mention, "calc.py:add")
        );
        // A new version beside the original, which keeps all of its attachments.
        assert_eq!(
            store.siblings(edited).await.unwrap(),
            vec![original, edited]
        );
        let old = store.get_message(original).await.unwrap().unwrap();
        assert_eq!(crate::attachments::document_refs(&old.content).len(), 1);

        // A message with only an image can be sent again: the image and no text.
        let only = CopyAttachments {
            message_id: original,
            attachment_ids: vec![image],
        };
        assert_eq!(
            user_content("", copied_parts(&store, chat, &only).await.unwrap()),
            json!([{ "type": "text", "text": "" }, image_part])
        );
    }

    /// [0.0.7 P6] Only a user message of the same chat lends its attachments, and only ones it
    /// carries and that belong to that chat; anything else is refused, never dropped silently.
    #[tokio::test]
    async fn attachments_are_copied_only_from_a_user_message_of_the_chat() {
        let store = store::Store::open_in_memory().unwrap();
        let chat = new_chat(&store).await;
        let other = new_chat(&store).await;
        let image = attachment(&store, chat, "image/png", "a.png").await;
        let elsewhere = attachment(&store, other, "image/png", "b.png").await;
        let part = |id: i64| json!({ "type": "image", "attachment_id": id });
        let asked = insert(
            &store,
            chat,
            None,
            store::Role::User,
            user_content("look", vec![part(image)]),
        )
        .await;
        let copy = |message_id, ids: Vec<i64>| CopyAttachments {
            message_id,
            attachment_ids: ids,
        };
        assert!(
            copied_parts(&store, chat, &copy(asked, vec![image]))
                .await
                .is_ok()
        );
        // Another chat's message.
        assert!(
            copied_parts(&store, other, &copy(asked, vec![image]))
                .await
                .is_err()
        );
        // A reply of the model.
        let answer = insert(
            &store,
            chat,
            Some(asked),
            store::Role::Assistant,
            user_content("", vec![part(image)]),
        )
        .await;
        assert!(
            copied_parts(&store, chat, &copy(answer, vec![image]))
                .await
                .is_err()
        );
        // A message that does not exist.
        let gone = store::MessageId(asked.0 + 1000);
        assert!(
            copied_parts(&store, chat, &copy(gone, vec![image]))
                .await
                .is_err()
        );
        // An attachment the message does not carry.
        let err = copied_parts(&store, chat, &copy(asked, vec![image, image + 1000]))
            .await
            .unwrap_err();
        assert_eq!(err.code, "attachment_missing");
        // A part that names another chat's attachment.
        let odd = insert(
            &store,
            chat,
            None,
            store::Role::User,
            user_content("odd", vec![part(elsewhere)]),
        )
        .await;
        let err = copied_parts(&store, chat, &copy(odd, vec![elsewhere]))
            .await
            .unwrap_err();
        assert_eq!(err.code, "attachment_missing");
        // The text part is never copied: the new text replaces it.
        let parts = copied_parts(&store, chat, &copy(asked, vec![image]))
            .await
            .unwrap();
        assert!(parts.iter().all(|p| p["type"] != "text"));
    }

    /// [0.0.7 P6] A send while another reply runs (in any chat, also one the window does not
    /// know of because it reloaded meanwhile) is refused as busy before anything is stored:
    /// before, the user message was stored first and stayed without an answer. The turn's
    /// hold on the slot ends with it.
    #[tokio::test]
    async fn a_send_while_a_reply_runs_stores_nothing() {
        let store = store::Store::open_in_memory().unwrap();
        let chat = new_chat(&store).await;
        let generation = Generation::default();
        let (running, _) = generation.start(chat.0 + 1).unwrap();
        let err = TurnSlot::take(&generation, chat, None)
            .err()
            .expect("refused while another reply runs");
        assert_eq!(err.code, "busy");
        assert!(store.current_path(chat).await.unwrap().is_empty());
        assert_eq!(generation.running_chat(), Some(chat.0 + 1));

        generation.finish(running);
        let slot = TurnSlot::take(&generation, chat, None).unwrap();
        assert_eq!(generation.running_chat(), Some(chat.0));
        store_user_message(
            &store,
            &slot,
            None,
            user_content("hello", Vec::new()),
            "hello".into(),
        )
        .await
        .unwrap();
        assert_eq!(store.current_path(chat).await.unwrap().len(), 1);
        // A second send in the same chat while this turn runs is refused too.
        assert_eq!(
            TurnSlot::take(&generation, chat, None).err().unwrap().code,
            "busy"
        );
        drop(slot);
        assert_eq!(generation.running_chat(), None);
    }

    /// [0.0.7 P6] The Stop of a chat stops that chat's reply only, never one running in
    /// another chat; Stop without a chat (the stop hotkey's path, older callers) stops
    /// whatever runs.
    #[test]
    fn stop_in_a_chat_never_stops_another_chats_reply() {
        let g = Generation::default();
        let (_, token) = g.start(1).unwrap();
        assert!(!stop_reply(&g, Some(store::ChatId(2))));
        assert!(!token.is_cancelled());
        assert!(stop_reply(&g, Some(store::ChatId(1))));
        assert!(token.is_cancelled());
        let g = Generation::default();
        assert!(!stop_reply(&g, Some(store::ChatId(1))), "nothing runs");
        let (_, token) = g.start(1).unwrap();
        assert!(stop_reply(&g, None));
        assert!(token.is_cancelled());
    }

    /// [0.0.7 P6] `running_turn` is the main window's alone: granted there only, registered,
    /// and it checks the calling window first, like its neighbours.
    #[test]
    fn running_turn_is_for_the_main_window_only() {
        assert!(include_str!("../capabilities/main.json").contains("\"allow-running-turn\""));
        for other in [
            include_str!("../capabilities/overlay.json"),
            include_str!("../capabilities/indicator.json"),
        ] {
            assert!(!other.contains("running-turn"));
        }
        assert!(
            include_str!("../commands.list")
                .lines()
                .any(|l| l.trim() == "running_turn")
        );
        assert!(include_str!("lib.rs").contains("chat::running_turn,"));
        let body = include_str!("chat.rs")
            .split_once("pub fn running_turn(")
            .unwrap()
            .1;
        let body = body.split_once('{').unwrap().1;
        let first = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap();
        assert_eq!(first, "crate::commands::only_main_window(window.label())?;");
    }

    /// [0.0.7 P6] `copyAttachments` as the webview sends it (src/lib/ipc.ts `CopyAttachments`).
    #[test]
    fn copy_attachments_reads_the_webview_names() {
        let c: CopyAttachments =
            serde_json::from_value(json!({ "messageId": 3, "attachmentIds": [5, 7] })).unwrap();
        assert_eq!(c.message_id, store::MessageId(3));
        assert_eq!(c.attachment_ids, vec![5, 7]);
    }

    /// [0.0.7 P6] The webview listens for the event this file sends and calls the commands by
    /// these names, with the arguments as Tauri names them (`chat_id` is `chatId`).
    #[test]
    fn the_webview_uses_these_names() {
        let ipc = include_str!("../../src/lib/ipc.ts");
        assert!(ipc.contains(&format!("listen<number | null>(\"{TURN_RUNNING}\"")));
        assert!(ipc.contains("invoke<number | null>(\"running_turn\")"));
        assert!(ipc.contains("invoke<void>(\"stop_generation\", { chatId: chatId ?? null })"));
        assert!(ipc.contains("copyAttachments?: CopyAttachments;"));
    }
}
