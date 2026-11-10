//! Chats, the message tree and history search.
//!
//! Messages form a tree through `parent_id`, so regenerate and edit-and-resend add a sibling
//! and keep the earlier branch (ARCHITECTURE.md section 8.2). `chats.leaf_message_id` marks
//! the branch the user is looking at; [`Store::current_path`] walks from it to the root.
//! The history invariant (every tool call has one result, no empty or trailing assistant
//! message) is enforced by the agent loop when it builds a request, not by this crate:
//! content is stored opaque.

use rusqlite::types::Value as SqlValue;
use rusqlite::{Connection, OptionalExtension, Row, named_params, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::error::StoreError;
use crate::ids::{
    ChatId, JsonColumn, MessageId, json_text, like_contains, now_ms, sql_enum, sql_limit,
    sql_opt_text, sql_u64, update_columns,
};

sql_enum!(
    /// Who wrote a message.
    Role, "role" {
        /// The system prompt.
        System => "system",
        /// The person using the app.
        User => "user",
        /// The model.
        Assistant => "assistant",
        /// A tool result.
        Tool => "tool",
    }
);

sql_enum!(
    /// Lifecycle of a message.
    MessageStatus, "message status" {
        /// Tokens are still arriving.
        Streaming => "streaming",
        /// Finished normally.
        Complete => "complete",
        /// The user pressed Stop.
        Stopped => "stopped",
        /// Generation failed; see [`Message::error`].
        Error => "error",
    }
);

/// Input for [`Store::create_chat`].
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewChat {
    /// Display title.
    pub title: String,
    /// The model (or provider model) this chat uses, if chosen.
    pub model_ref: Option<String>,
    /// Per-chat system prompt; `None` uses the global default.
    pub system_prompt: Option<String>,
}

/// A row of `chats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chat {
    /// Row id.
    pub id: ChatId,
    /// Display title.
    pub title: String,
    /// The model this chat uses.
    pub model_ref: Option<String>,
    /// Per-chat system prompt.
    pub system_prompt: Option<String>,
    /// Tip of the branch being shown; `None` for a chat with no messages.
    pub leaf_message_id: Option<MessageId>,
    /// Per-chat tool toggles (a JSON object, shape owned by the agent crate).
    pub tool_prefs: Value,
    /// Working folder for file and terminal tools.
    pub working_folder: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the last activity: creation or the newest message.
    pub updated_at: i64,
    /// Hidden from the default chat list.
    pub archived: bool,
}

/// Input for [`Store::list_chats`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListChats {
    /// Search text. Matches chat titles (substring) and message text: full-text trigram
    /// search for 3 or more characters, a plain substring scan for 1–2 characters.
    pub query: Option<String>,
    /// Maximum rows; 0 means no limit.
    pub limit: u32,
    /// Rows to skip.
    pub offset: u32,
    /// Include archived chats.
    pub include_archived: bool,
}

/// One row of [`Store::list_chats`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatSummary {
    /// Row id.
    pub id: ChatId,
    /// Display title.
    pub title: String,
    /// Unix ms of the last activity.
    pub updated_at: i64,
    /// Plain text around the match in the newest matching message (no markup; the UI
    /// highlights the query itself). `None` without a query or for a title-only match.
    pub snippet: Option<String>,
}

/// Input for [`Store::update_chat`]. `None` leaves a field unchanged; for nullable columns
/// `Some(None)` clears it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatPatch {
    /// New model.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::double_option"
    )]
    pub model_ref: Option<Option<String>>,
    /// New system prompt.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::double_option"
    )]
    pub system_prompt: Option<Option<String>>,
    /// New tool toggles; must be a JSON object.
    pub tool_prefs: Option<Value>,
    /// New working folder.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "crate::ids::double_option"
    )]
    pub working_folder: Option<Option<String>>,
    /// Archive or unarchive.
    pub archived: Option<bool>,
}

/// Input for [`Store::insert_message`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewMessage {
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// The message it answers; `None` for a root. Must be in the same chat.
    pub parent_id: Option<MessageId>,
    /// Author.
    pub role: Role,
    /// Array of content parts, stored as given.
    pub content: Value,
    /// Thinking text, if any.
    pub reasoning: Option<String>,
    /// Initial status (`Streaming` for a reply that is being generated).
    pub status: MessageStatus,
    /// The model that wrote it.
    pub model_ref: Option<String>,
    /// Plain-text projection indexed for history search.
    pub search_text: String,
}

/// Input for [`Store::update_message`]. `None` leaves a field unchanged.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessagePatch {
    /// New content parts.
    pub content: Option<Value>,
    /// New thinking text.
    pub reasoning: Option<String>,
    /// New status.
    pub status: Option<MessageStatus>,
    /// Prompt token count reported by the server.
    pub prompt_tokens: Option<u64>,
    /// Completion token count reported by the server.
    pub completion_tokens: Option<u64>,
    /// Generation speed.
    pub tokens_per_second: Option<f64>,
    /// Structured error for a failed message.
    pub error: Option<Value>,
    /// New plain-text projection for search.
    pub search_text: Option<String>,
}

/// A row of `messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Row id.
    pub id: MessageId,
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// The message it answers.
    pub parent_id: Option<MessageId>,
    /// Author.
    pub role: Role,
    /// Array of content parts, as stored.
    pub content: Value,
    /// Thinking text.
    pub reasoning: Option<String>,
    /// Lifecycle state.
    pub status: MessageStatus,
    /// The model that wrote it.
    pub model_ref: Option<String>,
    /// Prompt token count.
    pub prompt_tokens: Option<u64>,
    /// Completion token count.
    pub completion_tokens: Option<u64>,
    /// Generation speed.
    pub tokens_per_second: Option<f64>,
    /// Structured error.
    pub error: Option<Value>,
    /// Plain-text projection used for search.
    pub search_text: String,
    /// Unix ms.
    pub created_at: i64,
}

const CHAT_COLUMNS: &str = "id, title, model_ref, system_prompt, leaf_message_id, tool_prefs, \
     working_folder, created_at, updated_at, archived";

const MESSAGE_COLUMNS: &str = "m.id, m.chat_id, m.parent_id, m.role, m.content, m.reasoning, \
     m.status, m.model_ref, m.prompt_tokens, m.completion_tokens, m.tokens_per_second, m.error, \
     m.search_text, m.created_at";

fn chat_from_row(row: &Row<'_>) -> rusqlite::Result<Chat> {
    Ok(Chat {
        id: row.get(0)?,
        title: row.get(1)?,
        model_ref: row.get(2)?,
        system_prompt: row.get(3)?,
        leaf_message_id: row.get(4)?,
        tool_prefs: row.get::<_, JsonColumn>(5)?.0,
        working_folder: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        archived: row.get(9)?,
    })
}

fn message_from_row(row: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        parent_id: row.get(2)?,
        role: row.get(3)?,
        content: row.get::<_, JsonColumn>(4)?.0,
        reasoning: row.get(5)?,
        status: row.get(6)?,
        model_ref: row.get(7)?,
        prompt_tokens: row.get(8)?,
        completion_tokens: row.get(9)?,
        tokens_per_second: row.get(10)?,
        error: row.get::<_, Option<JsonColumn>>(11)?.map(|j| j.0),
        search_text: row.get(12)?,
        created_at: row.get(13)?,
    })
}

/// Returns the chat a message belongs to, or `NotFound`.
pub(crate) fn message_chat(conn: &Connection, id: MessageId) -> Result<ChatId, StoreError> {
    conn.query_row("SELECT chat_id FROM messages WHERE id = ?1", [id], |r| {
        r.get(0)
    })
    .optional()?
    .ok_or_else(|| StoreError::not_found("message", id.0))
}

/// Fails with `NotFound` unless the chat exists.
pub(crate) fn ensure_chat(conn: &Connection, id: ChatId) -> Result<(), StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM chats WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    if exists {
        Ok(())
    } else {
        Err(StoreError::not_found("chat", id.0))
    }
}

/// Fails unless `message` exists and belongs to `chat`.
pub(crate) fn ensure_message_in_chat(
    conn: &Connection,
    chat: ChatId,
    message: MessageId,
) -> Result<(), StoreError> {
    let owner = message_chat(conn, message)?;
    if owner == chat {
        Ok(())
    } else {
        Err(StoreError::InvalidInput(format!(
            "message {message} belongs to chat {owner}, not chat {chat}"
        )))
    }
}

fn create_chat(conn: &Connection, new: NewChat) -> Result<ChatId, StoreError> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO chats (title, model_ref, system_prompt, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![new.title, new.model_ref, new.system_prompt, now],
    )?;
    Ok(ChatId(conn.last_insert_rowid()))
}

fn get_chat(conn: &Connection, id: ChatId) -> Result<Option<Chat>, StoreError> {
    let sql = format!("SELECT {CHAT_COLUMNS} FROM chats WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], chat_from_row).optional()?)
}

fn list_chats(conn: &Connection, req: ListChats) -> Result<Vec<ChatSummary>, StoreError> {
    let limit = sql_limit(req.limit);
    let offset = i64::from(req.offset);
    let query = req
        .query
        .as_deref()
        .map(|q| q.replace('\0', ""))
        .map(|q| q.trim().to_owned())
        .filter(|q| !q.is_empty());

    let Some(query) = query else {
        let mut stmt = conn.prepare_cached(
            "SELECT id, title, updated_at FROM chats \
             WHERE (:all OR archived = 0) \
             ORDER BY updated_at DESC, id DESC LIMIT :limit OFFSET :offset",
        )?;
        let rows = stmt.query_map(
            named_params! { ":all": req.include_archived, ":limit": limit, ":offset": offset },
            |r| {
                Ok(ChatSummary {
                    id: r.get(0)?,
                    title: r.get(1)?,
                    updated_at: r.get(2)?,
                    snippet: None,
                })
            },
        )?;
        return Ok(rows.collect::<Result<_, _>>()?);
    };

    let like = like_contains(&query);
    // FTS5 trigram matches nothing for fewer than 3 characters, so short queries scan.
    let hits = if query.chars().count() >= 3 {
        "SELECT m.chat_id AS chat_id, MAX(m.id) AS message_id \
         FROM messages_fts JOIN messages m ON m.id = messages_fts.rowid \
         WHERE messages_fts MATCH :needle GROUP BY m.chat_id"
    } else {
        "SELECT m.chat_id AS chat_id, MAX(m.id) AS message_id \
         FROM messages m WHERE m.search_text LIKE :needle ESCAPE '\\' GROUP BY m.chat_id"
    };
    let needle = if query.chars().count() >= 3 {
        fts_phrase(&query)
    } else {
        like.clone()
    };
    let sql = format!(
        "WITH hits AS ({hits}) \
         SELECT c.id, c.title, c.updated_at, hm.search_text \
         FROM chats c \
         LEFT JOIN hits h ON h.chat_id = c.id \
         LEFT JOIN messages hm ON hm.id = h.message_id \
         WHERE (c.title LIKE :like ESCAPE '\\' OR h.chat_id IS NOT NULL) \
           AND (:all OR c.archived = 0) \
         ORDER BY c.updated_at DESC, c.id DESC LIMIT :limit OFFSET :offset"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(
        named_params! {
            ":needle": needle,
            ":like": like,
            ":all": req.include_archived,
            ":limit": limit,
            ":offset": offset,
        },
        |r| {
            let text: Option<String> = r.get(3)?;
            Ok(ChatSummary {
                id: r.get(0)?,
                title: r.get(1)?,
                updated_at: r.get(2)?,
                snippet: text.map(|t| snippet(&t, &query)),
            })
        },
    )?;
    Ok(rows.collect::<Result<_, _>>()?)
}

/// Quotes user text as one FTS5 phrase, so operators and punctuation are literal.
fn fts_phrase(query: &str) -> String {
    format!("\"{}\"", query.replace('"', "\"\""))
}

/// Byte offset of the first case-insensitive occurrence of `needle` in `hay`.
fn find_case_insensitive(hay: &str, needle: &str) -> Option<usize> {
    let needle: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Some(0);
    }
    hay.char_indices().map(|(i, _)| i).find(|&i| {
        let mut rest = hay[i..].chars().flat_map(char::to_lowercase);
        needle.iter().all(|n| rest.next() == Some(*n))
    })
}

/// Up to about 100 characters of `text` around the first match of `query`, on one line.
pub(crate) fn snippet(text: &str, query: &str) -> String {
    const BEFORE: usize = 30;
    const TOTAL: usize = 100;
    let at = find_case_insensitive(text, query).unwrap_or(0);
    let start = text[..at]
        .char_indices()
        .rev()
        .nth(BEFORE - 1)
        .map_or(0, |(i, _)| i);
    let end = text[start..]
        .char_indices()
        .nth(TOTAL)
        .map_or(text.len(), |(i, _)| start + i);
    let mut out = String::with_capacity(end - start + 8);
    if start > 0 {
        out.push('…');
    }
    let mut last_space = false;
    for c in text[start..end].chars() {
        if c.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(c);
            last_space = false;
        }
    }
    if end < text.len() {
        out.push('…');
    }
    out
}

fn rename_chat(conn: &Connection, id: ChatId, title: String) -> Result<(), StoreError> {
    let n = conn.execute(
        "UPDATE chats SET title = ?1 WHERE id = ?2",
        params![title, id],
    )?;
    if n == 0 {
        Err(StoreError::not_found("chat", id.0))
    } else {
        Ok(())
    }
}

fn delete_chat(conn: &mut Connection, id: ChatId) -> Result<Vec<String>, StoreError> {
    let tx = conn.transaction()?;
    let paths: Vec<String> = {
        let mut stmt =
            tx.prepare("SELECT DISTINCT bytes_path FROM attachments WHERE chat_id = ?1")?;
        let rows = stmt.query_map([id], |r| r.get(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let n = tx.execute("DELETE FROM chats WHERE id = ?1", [id])?;
    if n == 0 {
        return Err(StoreError::not_found("chat", id.0));
    }
    let mut orphaned = Vec::new();
    for path in paths {
        let still_used: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM attachments WHERE bytes_path = ?1)",
            [&path],
            |r| r.get(0),
        )?;
        if !still_used {
            orphaned.push(path);
        }
    }
    tx.commit()?;
    Ok(orphaned)
}

fn update_chat(conn: &Connection, id: ChatId, patch: ChatPatch) -> Result<(), StoreError> {
    let mut sets = Vec::new();
    if let Some(v) = patch.model_ref {
        sets.push(("model_ref", sql_opt_text(v)));
    }
    if let Some(v) = patch.system_prompt {
        sets.push(("system_prompt", sql_opt_text(v)));
    }
    if let Some(v) = patch.tool_prefs {
        if !v.is_object() {
            return Err(StoreError::InvalidInput(
                "tool_prefs must be a JSON object".into(),
            ));
        }
        sets.push(("tool_prefs", SqlValue::Text(json_text(&v)?)));
    }
    if let Some(v) = patch.working_folder {
        sets.push(("working_folder", sql_opt_text(v)));
    }
    if let Some(v) = patch.archived {
        sets.push(("archived", SqlValue::Integer(i64::from(v))));
    }
    if sets.is_empty() {
        return ensure_chat(conn, id);
    }
    if update_columns(conn, "chats", "id", id.0, sets)? == 0 {
        return Err(StoreError::not_found("chat", id.0));
    }
    Ok(())
}

fn set_leaf(conn: &Connection, chat: ChatId, message: MessageId) -> Result<(), StoreError> {
    ensure_chat(conn, chat)?;
    ensure_message_in_chat(conn, chat, message)?;
    conn.execute(
        "UPDATE chats SET leaf_message_id = ?1 WHERE id = ?2",
        params![message, chat],
    )?;
    Ok(())
}

fn insert_message(conn: &mut Connection, new: NewMessage) -> Result<MessageId, StoreError> {
    let tx = conn.transaction()?;
    ensure_chat(&tx, new.chat_id)?;
    if let Some(parent) = new.parent_id {
        ensure_message_in_chat(&tx, new.chat_id, parent)?;
    }
    let now = now_ms();
    tx.execute(
        "INSERT INTO messages (chat_id, parent_id, role, content, reasoning, status, model_ref, \
         search_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            new.chat_id,
            new.parent_id,
            new.role,
            json_text(&new.content)?,
            new.reasoning,
            new.status,
            new.model_ref,
            new.search_text,
            now
        ],
    )?;
    let id = MessageId(tx.last_insert_rowid());
    tx.execute(
        "UPDATE chats SET updated_at = max(updated_at, ?1), leaf_message_id = ?2 WHERE id = ?3",
        params![now, id, new.chat_id],
    )?;
    tx.commit()?;
    Ok(id)
}

fn update_message(conn: &Connection, id: MessageId, patch: MessagePatch) -> Result<(), StoreError> {
    let mut sets = Vec::new();
    if let Some(v) = patch.content {
        sets.push(("content", SqlValue::Text(json_text(&v)?)));
    }
    if let Some(v) = patch.reasoning {
        sets.push(("reasoning", SqlValue::Text(v)));
    }
    if let Some(v) = patch.status {
        sets.push(("status", SqlValue::Text(v.as_str().to_owned())));
    }
    if let Some(v) = patch.prompt_tokens {
        sets.push(("prompt_tokens", sql_u64("prompt_tokens", v)?));
    }
    if let Some(v) = patch.completion_tokens {
        sets.push(("completion_tokens", sql_u64("completion_tokens", v)?));
    }
    if let Some(v) = patch.tokens_per_second {
        if !v.is_finite() {
            return Err(StoreError::InvalidInput(
                "tokens_per_second must be finite".into(),
            ));
        }
        sets.push(("tokens_per_second", SqlValue::Real(v)));
    }
    if let Some(v) = patch.error {
        sets.push(("error", SqlValue::Text(json_text(&v)?)));
    }
    if let Some(v) = patch.search_text {
        sets.push(("search_text", SqlValue::Text(v)));
    }
    if sets.is_empty() {
        return message_chat(conn, id).map(|_| ());
    }
    if update_columns(conn, "messages", "id", id.0, sets)? == 0 {
        return Err(StoreError::not_found("message", id.0));
    }
    Ok(())
}

fn get_message(conn: &Connection, id: MessageId) -> Result<Option<Message>, StoreError> {
    let sql = format!("SELECT {MESSAGE_COLUMNS} FROM messages m WHERE m.id = ?1");
    Ok(conn.query_row(&sql, [id], message_from_row).optional()?)
}

/// Upper bound on tree depth when walking to the root; guards against a corrupted cycle.
const MAX_DEPTH: i64 = 1_000_000;

fn path_to(conn: &Connection, chat: ChatId, leaf: MessageId) -> Result<Vec<Message>, StoreError> {
    ensure_message_in_chat(conn, chat, leaf)?;
    let sql = format!(
        "WITH RECURSIVE path(id, depth) AS ( \
             SELECT ?1, 0 \
             UNION ALL \
             SELECT p.parent_id, path.depth + 1 FROM messages p JOIN path ON p.id = path.id \
             WHERE p.parent_id IS NOT NULL AND path.depth < {MAX_DEPTH} \
         ) \
         SELECT {MESSAGE_COLUMNS} FROM path JOIN messages m ON m.id = path.id \
         WHERE m.chat_id = ?2 ORDER BY path.depth DESC"
    );
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map(params![leaf, chat], message_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn current_path(conn: &Connection, chat: ChatId) -> Result<Vec<Message>, StoreError> {
    let leaf: Option<MessageId> = conn
        .query_row(
            "SELECT leaf_message_id FROM chats WHERE id = ?1",
            [chat],
            |r| r.get(0),
        )
        .optional()?
        .ok_or_else(|| StoreError::not_found("chat", chat.0))?;
    match leaf {
        Some(leaf) => path_to(conn, chat, leaf),
        None => Ok(Vec::new()),
    }
}

fn children(conn: &Connection, id: MessageId) -> Result<Vec<MessageId>, StoreError> {
    message_chat(conn, id)?;
    let mut stmt =
        conn.prepare_cached("SELECT id FROM messages WHERE parent_id = ?1 ORDER BY id")?;
    let rows = stmt.query_map([id], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn siblings(conn: &Connection, id: MessageId) -> Result<Vec<MessageId>, StoreError> {
    let (chat, parent): (ChatId, Option<MessageId>) = conn
        .query_row(
            "SELECT chat_id, parent_id FROM messages WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| StoreError::not_found("message", id.0))?;
    let mut stmt = conn.prepare_cached(
        "SELECT id FROM messages WHERE chat_id = ?1 AND parent_id IS ?2 ORDER BY id",
    )?;
    let rows = stmt.query_map(params![chat, parent], |r| r.get(0))?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Creates a chat and returns its id.
    pub async fn create_chat(&self, new: NewChat) -> Result<ChatId, StoreError> {
        self.call(move |c| create_chat(c, new)).await
    }

    /// Returns the chat, or `None` if it does not exist.
    pub async fn get_chat(&self, id: ChatId) -> Result<Option<Chat>, StoreError> {
        self.call(move |c| get_chat(c, id)).await
    }

    /// Lists chats, most recent activity first, optionally filtered by a search query
    /// (see [`ListChats::query`]).
    pub async fn list_chats(&self, req: ListChats) -> Result<Vec<ChatSummary>, StoreError> {
        self.call(move |c| list_chats(c, req)).await
    }

    /// Changes a chat's title.
    pub async fn rename_chat(&self, id: ChatId, title: String) -> Result<(), StoreError> {
        self.call(move |c| rename_chat(c, id, title)).await
    }

    /// Deletes a chat with its messages, tool calls, summaries, attachment rows and
    /// chat-scoped approved folders. Audit rows are kept.
    ///
    /// Returns the attachment `bytes_path`s that no remaining attachment row refers to, so
    /// the caller can delete those files from the content-addressed folder.
    pub async fn delete_chat(&self, id: ChatId) -> Result<Vec<String>, StoreError> {
        self.call(move |c| delete_chat(c, id)).await
    }

    /// Changes the fields set in `patch`.
    pub async fn update_chat(&self, id: ChatId, patch: ChatPatch) -> Result<(), StoreError> {
        self.call(move |c| update_chat(c, id, patch)).await
    }

    /// Selects the branch ending at `message` (which must belong to the chat).
    pub async fn set_leaf(&self, chat_id: ChatId, message_id: MessageId) -> Result<(), StoreError> {
        self.call(move |c| set_leaf(c, chat_id, message_id)).await
    }

    /// Inserts a message, makes it the chat's leaf and bumps the chat's `updated_at`.
    pub async fn insert_message(&self, new: NewMessage) -> Result<MessageId, StoreError> {
        self.call(move |c| insert_message(c, new)).await
    }

    /// Changes the fields set in `patch` (streaming updates, final token counts, errors).
    pub async fn update_message(
        &self,
        id: MessageId,
        patch: MessagePatch,
    ) -> Result<(), StoreError> {
        self.call(move |c| update_message(c, id, patch)).await
    }

    /// Returns the message, or `None` if it does not exist.
    pub async fn get_message(&self, id: MessageId) -> Result<Option<Message>, StoreError> {
        self.call(move |c| get_message(c, id)).await
    }

    /// The messages from the root down to `leaf`, following `parent_id`.
    pub async fn path_to(
        &self,
        chat_id: ChatId,
        leaf: MessageId,
    ) -> Result<Vec<Message>, StoreError> {
        self.call(move |c| path_to(c, chat_id, leaf)).await
    }

    /// The branch the chat currently shows (root to `leaf_message_id`); empty for a new chat.
    pub async fn current_path(&self, chat_id: ChatId) -> Result<Vec<Message>, StoreError> {
        self.call(move |c| current_path(c, chat_id)).await
    }

    /// Direct replies to a message, oldest first.
    pub async fn children(&self, message_id: MessageId) -> Result<Vec<MessageId>, StoreError> {
        self.call(move |c| children(c, message_id)).await
    }

    /// The alternatives at this point of the tree: every message in the same chat with the
    /// same parent, including `message_id` itself, oldest first. For root messages these are
    /// the chat's other roots.
    pub async fn siblings(&self, message_id: MessageId) -> Result<Vec<MessageId>, StoreError> {
        self.call(move |c| siblings(c, message_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(s: &str) -> Value {
        json!([{ "type": "text", "text": s }])
    }

    fn msg(chat: ChatId, parent: Option<MessageId>, role: Role, s: &str) -> NewMessage {
        NewMessage {
            chat_id: chat,
            parent_id: parent,
            role,
            content: text(s),
            reasoning: None,
            status: MessageStatus::Complete,
            model_ref: None,
            search_text: s.to_owned(),
        }
    }

    async fn chat(store: &Store, title: &str) -> ChatId {
        store
            .create_chat(NewChat {
                title: title.into(),
                ..NewChat::default()
            })
            .await
            .expect("create chat")
    }

    fn ids(path: &[Message]) -> Vec<MessageId> {
        path.iter().map(|m| m.id).collect()
    }

    #[tokio::test]
    async fn chat_crud_round_trip() {
        let store = Store::open_in_memory().expect("open");
        let id = store
            .create_chat(NewChat {
                title: "First".into(),
                model_ref: Some("qwen3".into()),
                system_prompt: Some("be brief".into()),
            })
            .await
            .expect("create");
        let c = store.get_chat(id).await.expect("get").expect("exists");
        assert_eq!(c.title, "First");
        assert_eq!(c.model_ref.as_deref(), Some("qwen3"));
        assert_eq!(c.tool_prefs, json!({}));
        assert!(c.leaf_message_id.is_none());
        assert!(!c.archived);

        store
            .rename_chat(id, "Renamed".into())
            .await
            .expect("rename");
        store
            .update_chat(
                id,
                ChatPatch {
                    model_ref: Some(None),
                    tool_prefs: Some(json!({ "files": true })),
                    working_folder: Some(Some("/tmp/w".into())),
                    ..ChatPatch::default()
                },
            )
            .await
            .expect("patch");
        let c = store.get_chat(id).await.expect("get").expect("exists");
        assert_eq!(c.title, "Renamed");
        assert_eq!(c.model_ref, None);
        assert_eq!(c.system_prompt.as_deref(), Some("be brief"));
        assert_eq!(c.tool_prefs, json!({ "files": true }));
        assert_eq!(c.working_folder.as_deref(), Some("/tmp/w"));

        let err = store
            .update_chat(
                id,
                ChatPatch {
                    tool_prefs: Some(json!([1])),
                    ..ChatPatch::default()
                },
            )
            .await
            .expect_err("array is refused");
        assert!(matches!(err, StoreError::InvalidInput(_)));
        let err = store
            .rename_chat(ChatId(999), "x".into())
            .await
            .expect_err("missing");
        assert!(matches!(
            err,
            StoreError::NotFound {
                what: "chat",
                id: 999
            }
        ));
        assert!(store.get_chat(ChatId(999)).await.expect("get").is_none());
    }

    #[tokio::test]
    async fn branched_tree_regenerate_and_switch() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "tree").await;
        let sys = store
            .insert_message(msg(c, None, Role::System, "sys"))
            .await
            .expect("sys");
        let u1 = store
            .insert_message(msg(c, Some(sys), Role::User, "hello"))
            .await
            .expect("u1");
        let a1 = store
            .insert_message(msg(c, Some(u1), Role::Assistant, "first"))
            .await
            .expect("a1");
        let u2 = store
            .insert_message(msg(c, Some(a1), Role::User, "more"))
            .await
            .expect("u2");
        let a2 = store
            .insert_message(msg(c, Some(u2), Role::Assistant, "second"))
            .await
            .expect("a2");

        // Regenerate the first answer: a sibling of a1 under u1. Leaf follows the new message.
        let a1b = store
            .insert_message(msg(c, Some(u1), Role::Assistant, "first again"))
            .await
            .expect("a1b");
        let current = store.current_path(c).await.expect("current");
        assert_eq!(ids(&current), vec![sys, u1, a1b]);

        // The old branch is intact and reachable.
        let old = store.path_to(c, a2).await.expect("old branch");
        assert_eq!(ids(&old), vec![sys, u1, a1, u2, a2]);
        assert_eq!(old[3].search_text, "more");
        assert_eq!(old[4].content, text("second"));

        assert_eq!(store.siblings(a1).await.expect("siblings"), vec![a1, a1b]);
        assert_eq!(store.siblings(a1b).await.expect("siblings"), vec![a1, a1b]);
        assert_eq!(store.children(u1).await.expect("children"), vec![a1, a1b]);
        assert_eq!(store.siblings(sys).await.expect("root siblings"), vec![sys]);
        assert!(store.children(a2).await.expect("leaf children").is_empty());

        // Switch back to the old branch.
        store.set_leaf(c, a2).await.expect("set leaf");
        assert_eq!(
            ids(&store.current_path(c).await.expect("current")),
            vec![sys, u1, a1, u2, a2]
        );
        assert_eq!(
            store
                .get_chat(c)
                .await
                .expect("get")
                .expect("chat")
                .leaf_message_id,
            Some(a2)
        );
    }

    #[tokio::test]
    async fn cross_chat_parents_and_leaves_are_refused() {
        let store = Store::open_in_memory().expect("open");
        let c1 = chat(&store, "one").await;
        let c2 = chat(&store, "two").await;
        let m1 = store
            .insert_message(msg(c1, None, Role::User, "a"))
            .await
            .expect("m1");
        let err = store
            .insert_message(msg(c2, Some(m1), Role::User, "b"))
            .await
            .expect_err("cross");
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
        let err = store.set_leaf(c2, m1).await.expect_err("cross leaf");
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
        let err = store.path_to(c2, m1).await.expect_err("cross path");
        assert!(matches!(err, StoreError::InvalidInput(_)), "{err:?}");
        let err = store
            .insert_message(msg(ChatId(77), None, Role::User, "x"))
            .await
            .expect_err("no chat");
        assert!(matches!(err, StoreError::NotFound { what: "chat", .. }));
        let err = store
            .insert_message(msg(c1, Some(MessageId(555)), Role::User, "x"))
            .await
            .expect_err("no parent");
        assert!(matches!(
            err,
            StoreError::NotFound {
                what: "message",
                ..
            }
        ));
        assert!(store.current_path(c2).await.expect("empty").is_empty());
    }

    #[tokio::test]
    async fn update_message_patches_fields() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "u").await;
        let mut new = msg(c, None, Role::Assistant, "");
        new.status = MessageStatus::Streaming;
        let id = store.insert_message(new).await.expect("insert");
        store
            .update_message(
                id,
                MessagePatch {
                    content: Some(text("done")),
                    reasoning: Some("thought".into()),
                    status: Some(MessageStatus::Complete),
                    prompt_tokens: Some(12),
                    completion_tokens: Some(34),
                    tokens_per_second: Some(56.5),
                    search_text: Some("done".into()),
                    ..MessagePatch::default()
                },
            )
            .await
            .expect("patch");
        let m = store.get_message(id).await.expect("get").expect("exists");
        assert_eq!(m.content, text("done"));
        assert_eq!(m.reasoning.as_deref(), Some("thought"));
        assert_eq!(m.status, MessageStatus::Complete);
        assert_eq!((m.prompt_tokens, m.completion_tokens), (Some(12), Some(34)));
        assert_eq!(m.tokens_per_second, Some(56.5));
        assert_eq!(m.error, None);

        store
            .update_message(
                id,
                MessagePatch {
                    status: Some(MessageStatus::Error),
                    error: Some(json!({ "kind": "server", "message": "500" })),
                    ..MessagePatch::default()
                },
            )
            .await
            .expect("error patch");
        let m = store.get_message(id).await.expect("get").expect("exists");
        assert_eq!(m.status, MessageStatus::Error);
        assert_eq!(m.error, Some(json!({ "kind": "server", "message": "500" })));

        // Search follows the new text, and the index matches the content table.
        let found = store
            .list_chats(ListChats {
                query: Some("done".into()),
                ..ListChats::default()
            })
            .await
            .expect("search");
        assert_eq!(found.len(), 1);
        store
            .update_message(
                id,
                MessagePatch {
                    search_text: Some("replaced wording".into()),
                    ..MessagePatch::default()
                },
            )
            .await
            .expect("new search text");
        let old_hits: i64 = store
            .call(|conn| {
                conn.execute(
                    "INSERT INTO messages_fts(messages_fts, rank) VALUES ('integrity-check', 1)",
                    [],
                )?;
                Ok(conn.query_row(
                    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH '\"done\"'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .expect("index check");
        assert_eq!(old_hits, 0, "the old text left the index");

        let err = store
            .update_message(
                MessageId(404),
                MessagePatch {
                    status: Some(MessageStatus::Stopped),
                    ..MessagePatch::default()
                },
            )
            .await
            .expect_err("missing");
        assert!(matches!(
            err,
            StoreError::NotFound {
                what: "message",
                id: 404
            }
        ));
        let err = store
            .update_message(
                id,
                MessagePatch {
                    prompt_tokens: Some(u64::MAX),
                    ..MessagePatch::default()
                },
            )
            .await
            .expect_err("too big");
        assert!(matches!(err, StoreError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn fts_finds_substrings_including_cjk() {
        let store = Store::open_in_memory().expect("open");
        let tokyo = chat(&store, "Travel").await;
        let other = chat(&store, "Recipes").await;
        store
            .insert_message(msg(tokyo, None, Role::User, "明日は東京タワーに行きます"))
            .await
            .expect("cjk");
        store
            .insert_message(msg(
                other,
                None,
                Role::User,
                "Knead the dough for ten minutes",
            ))
            .await
            .expect("latin");

        let hits = store
            .list_chats(ListChats {
                query: Some("京タワ".into()),
                ..ListChats::default()
            })
            .await
            .expect("cjk search");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, tokyo);
        let snip = hits[0].snippet.as_deref().expect("snippet");
        assert!(snip.contains("東京タワー"), "{snip}");

        // Substring inside a word, case-insensitive.
        let hits = store
            .list_chats(ListChats {
                query: Some("NEAD".into()),
                ..ListChats::default()
            })
            .await
            .expect("substring");
        assert_eq!(hits.iter().map(|h| h.id).collect::<Vec<_>>(), vec![other]);

        // FTS syntax characters are literal.
        let none = store
            .list_chats(ListChats {
                query: Some("dough\" OR \"x".into()),
                ..ListChats::default()
            })
            .await
            .expect("quoted");
        assert!(none.is_empty());

        // A title-only match has no snippet.
        let hits = store
            .list_chats(ListChats {
                query: Some("Recip".into()),
                ..ListChats::default()
            })
            .await
            .expect("title");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].snippet, None);
    }

    #[tokio::test]
    async fn short_queries_fall_back_to_like() {
        let store = Store::open_in_memory().expect("open");
        let a = chat(&store, "alpha").await;
        let b = chat(&store, "beta").await;
        store
            .insert_message(msg(a, None, Role::User, "x京y"))
            .await
            .expect("a");
        store
            .insert_message(msg(b, None, Role::User, "go 100% now"))
            .await
            .expect("b");

        let one = store
            .list_chats(ListChats {
                query: Some("京".into()),
                ..ListChats::default()
            })
            .await
            .expect("1 char");
        assert_eq!(one.iter().map(|h| h.id).collect::<Vec<_>>(), vec![a]);
        assert_eq!(one[0].snippet.as_deref(), Some("x京y"));

        let two = store
            .list_chats(ListChats {
                query: Some("0%".into()),
                ..ListChats::default()
            })
            .await
            .expect("2 chars");
        assert_eq!(two.iter().map(|h| h.id).collect::<Vec<_>>(), vec![b]);

        // `%` is literal, not a wildcard: "%" alone matches only the message containing it.
        let pct = store
            .list_chats(ListChats {
                query: Some("%".into()),
                ..ListChats::default()
            })
            .await
            .expect("percent");
        assert_eq!(pct.iter().map(|h| h.id).collect::<Vec<_>>(), vec![b]);

        // Two characters through FTS would match nothing; the scan finds a title too.
        let title = store
            .list_chats(ListChats {
                query: Some("ph".into()),
                ..ListChats::default()
            })
            .await
            .expect("title");
        assert_eq!(title.iter().map(|h| h.id).collect::<Vec<_>>(), vec![a]);
    }

    #[tokio::test]
    async fn list_orders_by_activity_and_pages() {
        let store = Store::open_in_memory().expect("open");
        let first = chat(&store, "first").await;
        let second = chat(&store, "second").await;
        let third = chat(&store, "third").await;
        store
            .call(|conn| Ok(conn.execute("UPDATE chats SET updated_at = 1000", [])?))
            .await
            .expect("age chats");
        // New activity in `first` moves it to the top.
        store
            .insert_message(msg(first, None, Role::User, "bump"))
            .await
            .expect("bump");
        let all = store.list_chats(ListChats::default()).await.expect("list");
        assert_eq!(all[0].id, first);
        assert_eq!(all.len(), 3);
        let page = store
            .list_chats(ListChats {
                limit: 1,
                offset: 1,
                ..ListChats::default()
            })
            .await
            .expect("page");
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].id, all[1].id);

        store
            .update_chat(
                second,
                ChatPatch {
                    archived: Some(true),
                    ..ChatPatch::default()
                },
            )
            .await
            .expect("archive");
        let visible = store.list_chats(ListChats::default()).await.expect("list");
        assert_eq!(visible.len(), 2);
        assert!(!visible.iter().any(|c| c.id == second));
        let everything = store
            .list_chats(ListChats {
                include_archived: true,
                ..ListChats::default()
            })
            .await
            .expect("list all");
        assert_eq!(everything.len(), 3);
        assert!(everything.iter().any(|c| c.id == third));
    }

    #[tokio::test]
    async fn delete_cascades_and_cleans_the_index() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "doomed").await;
        let keep = chat(&store, "kept").await;
        let u = store
            .insert_message(msg(c, None, Role::User, "unique needle text"))
            .await
            .expect("u");
        let a = store
            .insert_message(msg(c, Some(u), Role::Assistant, "reply"))
            .await
            .expect("a");
        store
            .insert_message(msg(keep, None, Role::User, "other"))
            .await
            .expect("keep");
        store
            .call(move |conn| {
                conn.execute(
                    "INSERT INTO tool_calls (message_id, call_id, server, tool, arguments, status, started_at) \
                     VALUES (?1, 'c1', 'files', 'read_file', '{}', 'ok', 0)",
                    [a],
                )?;
                Ok(())
            })
            .await
            .expect("tool call");

        store.delete_chat(c).await.expect("delete");
        assert!(store.get_chat(c).await.expect("get").is_none());
        assert!(store.get_message(u).await.expect("get").is_none());
        let (messages, calls, fts_ok): (i64, i64, bool) = store
            .call(move |conn| {
                let m = conn.query_row(
                    "SELECT count(*) FROM messages WHERE chat_id = ?1",
                    [c],
                    |r| r.get(0),
                )?;
                let t = conn.query_row("SELECT count(*) FROM tool_calls", [], |r| r.get(0))?;
                // rank = 1 also compares the index with the external content table; the
                // plain form only checks the index's internal consistency.
                conn.execute(
                    "INSERT INTO messages_fts(messages_fts, rank) VALUES ('integrity-check', 1)",
                    [],
                )?;
                // No index entry survives for the deleted rows (the join in list_chats
                // would hide a stale one, so look at the index directly).
                let stale: i64 = conn.query_row(
                    "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH '\"needle\"'",
                    [],
                    |r| r.get(0),
                )?;
                Ok((m, t, stale == 0))
            })
            .await
            .expect("counts");
        assert_eq!((messages, calls, fts_ok), (0, 0, true));
        let hits = store
            .list_chats(ListChats {
                query: Some("needle".into()),
                ..ListChats::default()
            })
            .await
            .expect("search");
        assert!(hits.is_empty());
        assert!(matches!(
            store.delete_chat(c).await,
            Err(StoreError::NotFound { .. })
        ));
        assert_eq!(store.current_path(keep).await.expect("kept").len(), 1);
    }

    #[tokio::test]
    async fn a_new_chat_never_inherits_a_deleted_chats_id_or_audit_rows() {
        use crate::audit::{AuditDecision, AuditEntry, AuditQuery};

        let store = Store::open_in_memory().expect("open");
        let old = chat(&store, "old").await;
        let old_msg = store
            .insert_message(msg(old, None, Role::User, "hi"))
            .await
            .expect("message");
        store
            .append_audit(AuditEntry {
                ts: None,
                chat_id: Some(old),
                server: "files".into(),
                tool: "delete_file".into(),
                arguments: json!({ "path": "a" }),
                preview: "Delete a".into(),
                risk: "destructive".into(),
                decision: AuditDecision::AllowOnce,
                decided_by: "user".into(),
                note: None,
                parent_audit_id: None,
                result: None,
            })
            .await
            .expect("audit");
        store.delete_chat(old).await.expect("delete");

        let new = chat(&store, "new").await;
        assert_ne!(new, old, "chat id reused");
        let new_msg = store
            .insert_message(msg(new, None, Role::User, "hello"))
            .await
            .expect("message");
        assert_ne!(new_msg, old_msg, "message id reused");
        // A stale handle from the deleted chat cannot touch the new chat's message.
        assert!(matches!(
            store
                .update_message(
                    old_msg,
                    MessagePatch {
                        status: Some(MessageStatus::Stopped),
                        ..MessagePatch::default()
                    }
                )
                .await,
            Err(StoreError::NotFound { .. })
        ));
        let rows = store
            .list_audit(AuditQuery {
                chat_id: Some(new),
                ..AuditQuery::default()
            })
            .await
            .expect("audit for the new chat");
        assert!(rows.is_empty(), "{rows:?}");
    }

    #[test]
    fn snippet_windows_and_flattens() {
        let long = format!("{}needle{}", "a ".repeat(40), "b\n".repeat(80));
        let s = snippet(&long, "NEEDLE");
        assert!(s.starts_with('…'), "{s}");
        assert!(s.ends_with('…'), "{s}");
        assert!(s.contains("needle"));
        assert!(!s.contains('\n'));
        assert_eq!(snippet("short", "zzz"), "short");
        assert_eq!(snippet("", "x"), "");
        assert_eq!(find_case_insensitive("ÄbC", "äB"), Some(0));
        assert_eq!(find_case_insensitive("xyz", "q"), None);
    }
}
