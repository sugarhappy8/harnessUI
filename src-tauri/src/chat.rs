//! Chats, messages and streaming replies.
//!
//! M1 sends plain chat (no tools). The agent loop replaces `generate` in M3; the
//! storage shape and the streaming events stay the same.

use std::time::Instant;

use agent::{ChatMessage, ChatRequest, OpenAiClient, StreamEvent};
use futures_util::StreamExt;
use serde_json::json;
use tauri::State;
use tauri::ipc::Channel;
use tokio_util::sync::CancellationToken;

use crate::dto::{ChatDetailDto, ChatDto, ChatStreamEvent, ChatSummaryDto, MessageDto};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const NEW_CHAT_TITLE: &str = "New chat";
const DEFAULT_SYSTEM_PROMPT: &str = "You are a helpful assistant.";

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

fn chat_dto(c: &store::Chat) -> ChatDto {
    ChatDto {
        id: c.id,
        title: c.title.clone(),
        model_ref: c.model_ref.clone(),
        system_prompt: c.system_prompt.clone(),
        leaf_message_id: c.leaf_message_id,
        created_at: c.created_at,
        updated_at: c.updated_at,
    }
}

async fn message_dto(state: &AppState, m: &store::Message) -> AppResult<MessageDto> {
    let siblings = state.store.siblings(m.id).await?;
    Ok(MessageDto {
        id: m.id,
        parent_id: m.parent_id,
        role: m.role.as_str().to_string(),
        text: text_of(&m.content),
        reasoning: m.reasoning.clone(),
        status: m.status.as_str().to_string(),
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
    Ok(ChatDetailDto {
        chat: chat_dto(&chat),
        messages,
    })
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
pub async fn delete_chat(state: State<'_, AppState>, chat_id: store::ChatId) -> AppResult<()> {
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

#[tauri::command]
pub async fn stop_generation(state: State<'_, AppState>) -> AppResult<()> {
    if let Ok(guard) = state.generation.lock()
        && let Some(token) = guard.as_ref()
    {
        token.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn send_message(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    text: String,
    parent_id: Option<store::MessageId>,
    images: Option<Vec<crate::attachments::ImageInput>>,
    on_event: Channel<ChatStreamEvent>,
) -> AppResult<()> {
    let text = text.trim().to_string();
    let images = images.unwrap_or_default();
    if text.is_empty() && images.is_empty() {
        return Err(AppError::bad_request("The message is empty."));
    }
    let chat = state
        .store
        .get_chat(chat_id)
        .await?
        .ok_or_else(|| AppError::not_found("chat"))?;
    let target = resolve_target(&state, chat.model_ref.as_deref()).await?;
    if !images.is_empty() && target.local && !state.runtime.status().vision {
        return Err(AppError::new(
            "no_vision",
            "This model cannot read images.",
            "Use a model with the vision badge (it comes with an image projector), or remove the image.",
            String::new(),
        ));
    }
    let mut content = text_content(&text);
    if let Some(parts) = content.as_array_mut() {
        parts.extend(crate::attachments::save_images(&app, &state, chat_id, &images).await?);
    }
    let user_id = state
        .store
        .insert_message(store::NewMessage {
            chat_id,
            parent_id,
            role: store::Role::User,
            content,
            reasoning: None,
            status: store::MessageStatus::Complete,
            model_ref: None,
            search_text: text.clone(),
        })
        .await?;
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
    generate(&state, &chat, user_id, Some(user_id), target, on_event).await
}

#[tauri::command]
pub async fn regenerate(
    state: State<'_, AppState>,
    chat_id: store::ChatId,
    assistant_message_id: store::MessageId,
    on_event: Channel<ChatStreamEvent>,
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
    let parent = old
        .parent_id
        .ok_or_else(|| AppError::bad_request("This message has nothing to regenerate from."))?;
    let target = resolve_target(&state, chat.model_ref.as_deref()).await?;
    generate(&state, &chat, parent, None, target, on_event).await
}

/// Where a request goes.
struct Target {
    client: OpenAiClient,
    model: String,
    model_ref: String,
    local: bool,
    n_ctx: Option<u32>,
}

async fn resolve_target(state: &AppState, model_ref: Option<&str>) -> AppResult<Target> {
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
        let (client, _provider) =
            crate::providers::client_for(state, store::ProviderId(id)).await?;
        let n_ctx = client.context_length(model).await.ok().flatten();
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

/// Builds the request history: the system prompt, then every message on the path
/// from the root to `last_id`, normalised so any chat template accepts it.
async fn build_messages(
    state: &AppState,
    chat: &store::Chat,
    last_id: store::MessageId,
) -> AppResult<Vec<ChatMessage>> {
    let default_prompt: Option<String> = state
        .store
        .get_setting(crate::settings::DEFAULT_SYSTEM_PROMPT)
        .await
        .ok()
        .flatten();
    let system = chat
        .system_prompt
        .clone()
        .or(default_prompt)
        .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string());
    let mut out = vec![ChatMessage::system(system)];
    for m in state.store.path_to(chat.id, last_id).await? {
        let text = text_of(&m.content);
        match m.role {
            store::Role::User => {
                let ids = crate::attachments::image_ids(&m.content);
                if ids.is_empty() {
                    out.push(ChatMessage::user(text));
                } else {
                    let mut parts = Vec::new();
                    if !text.is_empty() {
                        parts.push(agent::ContentPart::text(text));
                    }
                    for id in ids {
                        let (mime, b64) = crate::attachments::load_base64(state, id).await?;
                        let url = agent::ImageUrl::from_base64(&mime, &b64)
                            .map_err(AppError::internal)?;
                        parts.push(agent::ContentPart::image(url));
                    }
                    out.push(ChatMessage::user(parts));
                }
            }
            store::Role::Assistant => {
                // A turn that ended without an answer still needs an assistant message,
                // or two user messages would follow each other (ARCHITECTURE.md 8.2).
                let text = if text.trim().is_empty() {
                    "(stopped)".to_string()
                } else {
                    text
                };
                out.push(ChatMessage::assistant(text));
            }
            _ => {}
        }
    }
    Ok(out)
}

async fn generate(
    state: &AppState,
    chat: &store::Chat,
    reply_to: store::MessageId,
    user_message_id: Option<store::MessageId>,
    target: Target,
    on_event: Channel<ChatStreamEvent>,
) -> AppResult<()> {
    let cancel = CancellationToken::new();
    {
        let mut guard = state.generation.lock().map_err(AppError::internal)?;
        if guard.as_ref().is_some_and(|t| !t.is_cancelled()) {
            return Err(AppError::new(
                "busy",
                "A reply is already being generated.",
                "Wait for it to finish or press Stop.",
                String::new(),
            ));
        }
        *guard = Some(cancel.clone());
    }
    let result = generate_inner(
        state,
        chat,
        reply_to,
        user_message_id,
        target,
        &on_event,
        cancel,
    )
    .await;
    if let Ok(mut guard) = state.generation.lock() {
        *guard = None;
    }
    result
}

async fn generate_inner(
    state: &AppState,
    chat: &store::Chat,
    reply_to: store::MessageId,
    user_message_id: Option<store::MessageId>,
    target: Target,
    on_event: &Channel<ChatStreamEvent>,
    cancel: CancellationToken,
) -> AppResult<()> {
    let messages = build_messages(state, chat, reply_to).await?;
    let assistant_id = state
        .store
        .insert_message(store::NewMessage {
            chat_id: chat.id,
            parent_id: Some(reply_to),
            role: store::Role::Assistant,
            content: text_content(""),
            reasoning: None,
            status: store::MessageStatus::Streaming,
            model_ref: Some(target.model_ref.clone()),
            search_text: String::new(),
        })
        .await?;
    let _ = on_event.send(ChatStreamEvent::Started {
        user_message_id,
        assistant_message_id: assistant_id,
    });

    let mut request = ChatRequest::new(target.model.clone(), messages);
    request.stream = true;
    if target.local {
        request.extra.insert("cache_prompt".into(), json!(true));
        // Leave the rest of the context for the reply, but never less than 512 tokens.
        if let (Some(n_ctx), Ok(Some(prompt))) =
            (target.n_ctx, target.client.count_tokens(&request).await)
        {
            let room = n_ctx.saturating_sub(prompt);
            request.max_tokens = Some(room.max(512).min(n_ctx));
        }
    }

    let started = Instant::now();
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut prompt_tokens: Option<u64> = None;
    let mut completion_tokens: Option<u64> = None;
    let mut tps: Option<f64> = None;
    let mut finish_reason: Option<String> = None;
    let mut failure: Option<AppError> = None;

    match target.client.stream_chat(&request, cancel.clone()).await {
        Err(e) => failure = Some(AppError::from(e)),
        Ok(stream) => {
            futures_util::pin_mut!(stream);
            while let Some(item) = stream.next().await {
                match item {
                    Ok(StreamEvent::Content(t)) => {
                        text.push_str(&t);
                        let _ = on_event.send(ChatStreamEvent::Content { text: t });
                    }
                    Ok(StreamEvent::Reasoning(t)) => {
                        reasoning.push_str(&t);
                        let _ = on_event.send(ChatStreamEvent::Reasoning { text: t });
                    }
                    Ok(StreamEvent::Usage(u)) => {
                        prompt_tokens = Some(u64::from(u.prompt_tokens));
                        completion_tokens = Some(u64::from(u.completion_tokens));
                        let _ = on_event.send(ChatStreamEvent::Usage {
                            prompt_tokens: u.prompt_tokens,
                            completion_tokens: u.completion_tokens,
                        });
                    }
                    Ok(StreamEvent::Timings(t)) => {
                        tps = t.predicted_per_second;
                        let _ = on_event.send(ChatStreamEvent::Timings {
                            predicted_per_second: t.predicted_per_second,
                            prompt_per_second: t.prompt_per_second,
                        });
                    }
                    Ok(StreamEvent::Finished { finish_reason: r }) => {
                        finish_reason = Some(format!("{r:?}").to_lowercase());
                    }
                    Ok(StreamEvent::Done) => break,
                    Ok(_) => {}
                    Err(agent::ProviderError::Cancelled) => break,
                    Err(e) => {
                        failure = Some(AppError::from(e));
                        break;
                    }
                }
            }
        }
    }

    // Remote endpoints may not report timings; estimate from wall time.
    if tps.is_none()
        && let Some(n) = completion_tokens
    {
        let secs = started.elapsed().as_secs_f64();
        if secs > 0.0 && n > 0 {
            tps = Some(n as f64 / secs);
        }
    }
    let status = if failure.is_some() {
        store::MessageStatus::Error
    } else if cancel.is_cancelled() {
        store::MessageStatus::Stopped
    } else {
        store::MessageStatus::Complete
    };
    state
        .store
        .update_message(
            assistant_id,
            store::MessagePatch {
                content: Some(text_content(&text)),
                reasoning: (!reasoning.is_empty()).then_some(reasoning),
                status: Some(status),
                prompt_tokens,
                completion_tokens,
                tokens_per_second: tps,
                error: failure
                    .as_ref()
                    .map(|e| serde_json::to_value(e).unwrap_or_default()),
                search_text: Some(text.clone()),
            },
        )
        .await?;
    if let Some(err) = failure {
        let _ = on_event.send(ChatStreamEvent::Error { error: err.clone() });
        let _ = on_event.send(ChatStreamEvent::Finished {
            status: "error".into(),
            finish_reason,
        });
        return Ok(());
    }
    let _ = on_event.send(ChatStreamEvent::Finished {
        status: status.as_str().to_string(),
        finish_reason,
    });
    Ok(())
}
