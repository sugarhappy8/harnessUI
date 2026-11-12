//! Chats, messages and streaming replies.
//!
//! Every reply is an agent turn (`turns::run`): the model may call the tools switched on
//! for the chat, and each message of the turn is stored as it completes.

use agent::OpenAiClient;
use serde_json::json;
use tauri::State;
use tauri::ipc::Channel;

use crate::dto::{ChatDetailDto, ChatDto, ChatSummaryDto, MessageDto};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
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
    Ok(ChatDetailDto {
        chat: dto,
        messages,
        summary: earlier_summary(&state.store, chat_id, &path).await, // [0.0.5 memory]
    })
}

/// [0.0.5 memory] The summary the model is sent instead of the earlier messages of `path`
/// (the shown branch), chosen as a turn chooses it (`turns::latest_summary`). A summary
/// that cannot be read is left out: the chat still opens.
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
        text: summary.text,
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

#[tauri::command]
pub async fn stop_generation(app: tauri::AppHandle, state: State<'_, AppState>) -> AppResult<()> {
    state.generation.cancel();
    crate::computer_cmd::release_all(&app);
    crate::turns::cancel_all_pending(&app);
    Ok(())
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
    on_event: Channel<TurnStreamEvent>,
) -> AppResult<()> {
    let text = text.trim().to_string();
    let images = images.unwrap_or_default();
    let documents = documents.unwrap_or_default();
    let mentions = mentions.unwrap_or_default();
    if text.is_empty() && images.is_empty() && documents.is_empty() && mentions.is_empty() {
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
    // [0.0.4 mentions] The chips the user chose, read through the path sandbox first: one
    // that cannot be read fails the send before anything is stored.
    let n_ctx = target
        .n_ctx
        .unwrap_or(crate::settings::DEFAULT_REMOTE_CONTEXT) as usize;
    // [0.0.5 memory] A document's share of the context counts the message's mentions too.
    let shared_by = documents.len() + mentions.len();
    // [0.0.6] A local model's server counts each share in its own tokens.
    let counter = target.local.then_some(&target.client);
    let mention_parts = crate::mentions::attach(&app, &state, chat_id, mentions, n_ctx).await?;
    let mut content = text_content(&text);
    if let Some(parts) = content.as_array_mut() {
        parts.extend(crate::attachments::save_images(&app, &state, chat_id, &images).await?);
        parts.extend(
            crate::attachments::save_documents(
                &app, &state, chat_id, &documents, n_ctx, shared_by, counter,
            )
            .await?,
        );
        parts.extend(mention_parts);
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
    generate(
        &app,
        &state,
        &chat,
        user_id,
        Some(user_id),
        target,
        on_event,
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
    generate(&app, &state, &chat, parent, None, target, on_event).await
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
        let (client, _provider) =
            crate::providers::client_for(state, store::ProviderId(id)).await?;
        // A model that reports no context (or a failed check) gets the user's setting,
        // 4,096 by default (8.4).
        let reported = client.context_length(model).await.ok().flatten();
        if remember {
            crate::mcp_cmd::remember_context(model_ref, reported);
        }
        let n_ctx = match reported {
            Some(n) => n,
            None => {
                let stored: Option<serde_json::Value> = state
                    .store
                    .get_setting(crate::settings::REMOTE_CONTEXT_LENGTH)
                    .await
                    .ok()
                    .flatten();
                crate::settings::remote_context_length(stored.as_ref())
            }
        };
        let n_ctx = Some(n_ctx);
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

async fn generate(
    app: &tauri::AppHandle,
    state: &AppState,
    chat: &store::Chat,
    reply_to: store::MessageId,
    user_message_id: Option<store::MessageId>,
    target: Target,
    on_event: Channel<TurnStreamEvent>,
) -> AppResult<()> {
    let Some((generation_id, cancel)) = state.generation.start(chat.id.0) else {
        return Err(AppError::new(
            "busy",
            "A reply is already being generated.",
            "Wait for it to finish or press Stop.",
            String::new(),
        ));
    };
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
    state.generation.finish(generation_id);
    result
}

/// What the model can do and the request template, for a turn sent to `target`.
pub(crate) fn turn_settings(
    state: &AppState,
    target: &Target,
) -> (agent::ModelCaps, agent::RequestTemplate) {
    let status = state.runtime.status();
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
        thinking: target.local && status.thinking,
        preserve_reasoning: target.local && status.preserve_reasoning,
        images_in_tool_results: !target.local,
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
}
