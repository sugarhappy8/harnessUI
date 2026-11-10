//! Tool calls linked to their assistant message, compaction summaries and attachments.

use std::collections::HashMap;

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Store;
use crate::chats::{ensure_chat, ensure_message_in_chat, message_chat};
use crate::error::StoreError;
use crate::ids::{
    AttachmentId, ChatId, JsonColumn, MessageId, SummaryId, ToolCallId, json_text, now_ms,
    sql_enum, sql_u64,
};

sql_enum!(
    /// State of one tool call. The last four are the synthetic results the history
    /// invariant (ARCHITECTURE.md section 8.2) adds for calls that never produced output.
    ToolCallStatus, "tool call status" {
        /// Waiting for the user's approval.
        Pending => "pending",
        /// Executing.
        Running => "running",
        /// Finished successfully.
        Ok => "ok",
        /// The tool ran and reported an error.
        Error => "error",
        /// The user (or policy) denied it.
        Denied => "denied",
        /// The user pressed Stop.
        Cancelled => "cancelled",
        /// Skipped, e.g. after an earlier call in the same response was denied.
        NotRun => "not_run",
        /// The app quit or crashed while it ran.
        Interrupted => "interrupted",
    }
);

impl ToolCallStatus {
    /// True for states that end a call.
    pub const fn is_final(self) -> bool {
        !matches!(self, Self::Pending | Self::Running)
    }
}

/// Input for [`Store::insert_tool_call`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NewToolCall {
    /// The assistant message that requested the call.
    pub message_id: MessageId,
    /// The call id from the model (or one the app made up); unique per message.
    pub call_id: String,
    /// MCP server name (built-in servers too, e.g. "files").
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// Arguments; must be a JSON object (history invariant).
    pub arguments: Value,
    /// Initial state, usually `Pending` or `Running`.
    pub status: ToolCallStatus,
}

/// Input for [`Store::finish_tool_call`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCallFinish {
    /// Final state; must satisfy [`ToolCallStatus::is_final`].
    pub status: ToolCallStatus,
    /// Short text shown on the tool card.
    pub result_summary: Option<String>,
    /// Where the full result is kept (e.g. a file path), if not inline.
    pub result_ref: Option<String>,
}

/// A row of `tool_calls`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    /// Row id.
    pub id: ToolCallId,
    /// The assistant message that requested it.
    pub message_id: MessageId,
    /// The call id.
    pub call_id: String,
    /// MCP server name.
    pub server: String,
    /// Tool name.
    pub tool: String,
    /// Arguments.
    pub arguments: Value,
    /// Current state.
    pub status: ToolCallStatus,
    /// Unix ms when the row was created.
    pub started_at: i64,
    /// Unix ms when it finished.
    pub finished_at: Option<i64>,
    /// `finished_at - started_at`.
    pub duration_ms: Option<u64>,
    /// Short result text.
    pub result_summary: Option<String>,
    /// Pointer to the full result.
    pub result_ref: Option<String>,
}

/// Input for [`Store::insert_summary`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewSummary {
    /// The chat.
    pub chat_id: ChatId,
    /// The newest message the summary covers; must be in the chat.
    pub last_message_id: MessageId,
    /// Summary text.
    pub text: String,
}

/// A row of `summaries`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Row id.
    pub id: SummaryId,
    /// The chat.
    pub chat_id: ChatId,
    /// The newest message covered.
    pub last_message_id: MessageId,
    /// Summary text.
    pub text: String,
    /// Unix ms.
    pub created_at: i64,
}

/// Input for [`Store::insert_attachment`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewAttachment {
    /// The chat it was attached in.
    pub chat_id: ChatId,
    /// Hex SHA-256 of the bytes.
    pub sha256: String,
    /// MIME type.
    pub mime: String,
    /// Original file name.
    pub file_name: String,
    /// Size of the bytes.
    pub size_bytes: u64,
    /// Path of the bytes in the content-addressed folder.
    pub bytes_path: String,
    /// Text extracted for the model, if any.
    pub extracted_text: Option<String>,
}

/// A row of `attachments`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attachment {
    /// Row id (referenced from message content parts as `attachment_id`).
    pub id: AttachmentId,
    /// The chat.
    pub chat_id: ChatId,
    /// Hex SHA-256 of the bytes.
    pub sha256: String,
    /// MIME type.
    pub mime: String,
    /// Original file name.
    pub file_name: String,
    /// Size of the bytes.
    pub size_bytes: u64,
    /// Path of the bytes.
    pub bytes_path: String,
    /// Extracted text.
    pub extracted_text: Option<String>,
    /// Unix ms.
    pub created_at: i64,
}

const TOOL_CALL_COLUMNS: &str = "id, message_id, call_id, server, tool, arguments, status, \
     started_at, finished_at, duration_ms, result_summary, result_ref";

fn tool_call_from_row(row: &Row<'_>) -> rusqlite::Result<ToolCall> {
    Ok(ToolCall {
        id: row.get(0)?,
        message_id: row.get(1)?,
        call_id: row.get(2)?,
        server: row.get(3)?,
        tool: row.get(4)?,
        arguments: row.get::<_, JsonColumn>(5)?.0,
        status: row.get(6)?,
        started_at: row.get(7)?,
        finished_at: row.get(8)?,
        duration_ms: row.get(9)?,
        result_summary: row.get(10)?,
        result_ref: row.get(11)?,
    })
}

fn insert_tool_call(conn: &Connection, new: NewToolCall) -> Result<ToolCallId, StoreError> {
    message_chat(conn, new.message_id)?;
    if !new.arguments.is_object() {
        return Err(StoreError::InvalidInput(
            "tool call arguments must be a JSON object".into(),
        ));
    }
    let result = conn.execute(
        "INSERT INTO tool_calls (message_id, call_id, server, tool, arguments, status, started_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            new.message_id,
            new.call_id,
            new.server,
            new.tool,
            json_text(&new.arguments)?,
            new.status,
            now_ms()
        ],
    );
    match result {
        Ok(_) => Ok(ToolCallId(conn.last_insert_rowid())),
        Err(e) if StoreError::is_unique_violation(&e) => Err(StoreError::Conflict(format!(
            "message {} already has a tool call with id {:?}",
            new.message_id, new.call_id
        ))),
        Err(e) => Err(e.into()),
    }
}

fn set_tool_call_status(
    conn: &Connection,
    id: ToolCallId,
    status: ToolCallStatus,
) -> Result<(), StoreError> {
    if status.is_final() {
        return Err(StoreError::InvalidInput(format!(
            "use finish_tool_call for the final status {status}"
        )));
    }
    let n = conn.execute(
        "UPDATE tool_calls SET status = ?1 WHERE id = ?2 AND finished_at IS NULL",
        params![status, id],
    )?;
    if n == 0 {
        return Err(not_found_or_finished(conn, id)?);
    }
    Ok(())
}

fn finish_tool_call(
    conn: &Connection,
    id: ToolCallId,
    fin: ToolCallFinish,
) -> Result<(), StoreError> {
    if !fin.status.is_final() {
        return Err(StoreError::InvalidInput(format!(
            "{} is not a final status",
            fin.status
        )));
    }
    let now = now_ms();
    let n = conn.execute(
        "UPDATE tool_calls SET status = ?1, finished_at = ?2, \
         duration_ms = max(0, ?2 - started_at), result_summary = ?3, result_ref = ?4 \
         WHERE id = ?5 AND finished_at IS NULL",
        params![fin.status, now, fin.result_summary, fin.result_ref, id],
    )?;
    if n == 0 {
        return Err(not_found_or_finished(conn, id)?);
    }
    Ok(())
}

/// Builds the error for an update that matched no unfinished row.
fn not_found_or_finished(conn: &Connection, id: ToolCallId) -> Result<StoreError, StoreError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM tool_calls WHERE id = ?1)",
        [id],
        |r| r.get(0),
    )?;
    Ok(if exists {
        StoreError::Conflict(format!("tool call {id} has already finished"))
    } else {
        StoreError::not_found("tool call", id.0)
    })
}

fn tool_calls_for_message(conn: &Connection, id: MessageId) -> Result<Vec<ToolCall>, StoreError> {
    let sql =
        format!("SELECT {TOOL_CALL_COLUMNS} FROM tool_calls WHERE message_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([id], tool_call_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

fn insert_summary(conn: &Connection, new: NewSummary) -> Result<SummaryId, StoreError> {
    ensure_chat(conn, new.chat_id)?;
    ensure_message_in_chat(conn, new.chat_id, new.last_message_id)?;
    conn.execute(
        "INSERT INTO summaries (chat_id, last_message_id, text, created_at) VALUES (?1, ?2, ?3, ?4)",
        params![new.chat_id, new.last_message_id, new.text, now_ms()],
    )?;
    Ok(SummaryId(conn.last_insert_rowid()))
}

fn latest_summary_on_path(
    conn: &Connection,
    chat: ChatId,
    path: &[MessageId],
) -> Result<Option<Summary>, StoreError> {
    let position: HashMap<MessageId, usize> =
        path.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let mut stmt = conn.prepare_cached(
        "SELECT id, chat_id, last_message_id, text, created_at FROM summaries WHERE chat_id = ?1",
    )?;
    let rows = stmt.query_map([chat], |r| {
        Ok(Summary {
            id: r.get(0)?,
            chat_id: r.get(1)?,
            last_message_id: r.get(2)?,
            text: r.get(3)?,
            created_at: r.get(4)?,
        })
    })?;
    let mut best: Option<(usize, Summary)> = None;
    for row in rows {
        let summary = row?;
        let Some(&pos) = position.get(&summary.last_message_id) else {
            continue;
        };
        // Deepest covered message wins; for the same message, the newest summary.
        let better = match &best {
            None => true,
            Some((best_pos, best_summary)) => {
                pos > *best_pos || (pos == *best_pos && summary.id > best_summary.id)
            }
        };
        if better {
            best = Some((pos, summary));
        }
    }
    Ok(best.map(|(_, s)| s))
}

const ATTACHMENT_COLUMNS: &str = "id, chat_id, sha256, mime, file_name, size_bytes, bytes_path, \
     extracted_text, created_at";

fn attachment_from_row(row: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        sha256: row.get(2)?,
        mime: row.get(3)?,
        file_name: row.get(4)?,
        size_bytes: row.get(5)?,
        bytes_path: row.get(6)?,
        extracted_text: row.get(7)?,
        created_at: row.get(8)?,
    })
}

fn insert_attachment(conn: &Connection, new: NewAttachment) -> Result<AttachmentId, StoreError> {
    ensure_chat(conn, new.chat_id)?;
    let size = sql_u64("size_bytes", new.size_bytes)?;
    conn.execute(
        "INSERT INTO attachments (chat_id, sha256, mime, file_name, size_bytes, bytes_path, \
         extracted_text, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            new.chat_id,
            new.sha256,
            new.mime,
            new.file_name,
            size,
            new.bytes_path,
            new.extracted_text,
            now_ms()
        ],
    )?;
    Ok(AttachmentId(conn.last_insert_rowid()))
}

fn get_attachment(conn: &Connection, id: AttachmentId) -> Result<Option<Attachment>, StoreError> {
    let sql = format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE id = ?1");
    Ok(conn.query_row(&sql, [id], attachment_from_row).optional()?)
}

fn attachments_for_chat(conn: &Connection, chat: ChatId) -> Result<Vec<Attachment>, StoreError> {
    let sql =
        format!("SELECT {ATTACHMENT_COLUMNS} FROM attachments WHERE chat_id = ?1 ORDER BY id");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([chat], attachment_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Records a tool call requested by an assistant message.
    ///
    /// Fails with [`StoreError::Conflict`] if the message already has a call with this
    /// `call_id`, and with [`StoreError::InvalidInput`] if the arguments are not an object.
    pub async fn insert_tool_call(&self, new: NewToolCall) -> Result<ToolCallId, StoreError> {
        self.call(move |c| insert_tool_call(c, new)).await
    }

    /// Moves an unfinished call between `Pending` and `Running`.
    pub async fn set_tool_call_status(
        &self,
        id: ToolCallId,
        status: ToolCallStatus,
    ) -> Result<(), StoreError> {
        self.call(move |c| set_tool_call_status(c, id, status))
            .await
    }

    /// Records the end of a call: final status, finish time, duration and result.
    /// A call can be finished once; a second attempt is a [`StoreError::Conflict`].
    pub async fn finish_tool_call(
        &self,
        id: ToolCallId,
        fin: ToolCallFinish,
    ) -> Result<(), StoreError> {
        self.call(move |c| finish_tool_call(c, id, fin)).await
    }

    /// The calls a message requested, in the order they were recorded.
    pub async fn tool_calls_for_message(
        &self,
        message_id: MessageId,
    ) -> Result<Vec<ToolCall>, StoreError> {
        self.call(move |c| tool_calls_for_message(c, message_id))
            .await
    }

    /// Stores a compaction summary covering the chat up to `last_message_id`.
    pub async fn insert_summary(&self, new: NewSummary) -> Result<SummaryId, StoreError> {
        self.call(move |c| insert_summary(c, new)).await
    }

    /// The summary to use for a branch: among the chat's summaries whose `last_message_id`
    /// is on `path` (root first), the one covering the deepest message; newest on a tie.
    /// A summary from another branch is never returned.
    pub async fn latest_summary_on_path(
        &self,
        chat_id: ChatId,
        path: &[MessageId],
    ) -> Result<Option<Summary>, StoreError> {
        let path = path.to_vec();
        self.call(move |c| latest_summary_on_path(c, chat_id, &path))
            .await
    }

    /// Records an attachment's metadata (the bytes are written by the caller).
    pub async fn insert_attachment(&self, new: NewAttachment) -> Result<AttachmentId, StoreError> {
        self.call(move |c| insert_attachment(c, new)).await
    }

    /// Returns the attachment, or `None`.
    pub async fn get_attachment(&self, id: AttachmentId) -> Result<Option<Attachment>, StoreError> {
        self.call(move |c| get_attachment(c, id)).await
    }

    /// The chat's attachments, oldest first.
    pub async fn attachments_for_chat(
        &self,
        chat_id: ChatId,
    ) -> Result<Vec<Attachment>, StoreError> {
        self.call(move |c| attachments_for_chat(c, chat_id)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::{MessageStatus, NewChat, NewMessage, Role};
    use serde_json::json;

    async fn setup() -> (Store, ChatId, MessageId) {
        let store = Store::open_in_memory().expect("open");
        let chat = store
            .create_chat(NewChat {
                title: "t".into(),
                ..NewChat::default()
            })
            .await
            .expect("chat");
        let msg = store
            .insert_message(NewMessage {
                chat_id: chat,
                parent_id: None,
                role: Role::Assistant,
                content: json!([]),
                reasoning: None,
                status: MessageStatus::Complete,
                model_ref: None,
                search_text: String::new(),
            })
            .await
            .expect("message");
        (store, chat, msg)
    }

    async fn child(store: &Store, chat: ChatId, parent: MessageId) -> MessageId {
        store
            .insert_message(NewMessage {
                chat_id: chat,
                parent_id: Some(parent),
                role: Role::User,
                content: json!([]),
                reasoning: None,
                status: MessageStatus::Complete,
                model_ref: None,
                search_text: String::new(),
            })
            .await
            .expect("child")
    }

    #[tokio::test]
    async fn tool_call_lifecycle() {
        let (store, _chat, msg) = setup().await;
        let id = store
            .insert_tool_call(NewToolCall {
                message_id: msg,
                call_id: "call_1".into(),
                server: "files".into(),
                tool: "read_file".into(),
                arguments: json!({ "path": "a.txt" }),
                status: ToolCallStatus::Pending,
            })
            .await
            .expect("insert");
        store
            .set_tool_call_status(id, ToolCallStatus::Running)
            .await
            .expect("running");
        let err = store
            .set_tool_call_status(id, ToolCallStatus::Ok)
            .await
            .expect_err("final via set");
        assert!(matches!(err, StoreError::InvalidInput(_)));
        store
            .finish_tool_call(
                id,
                ToolCallFinish {
                    status: ToolCallStatus::Ok,
                    result_summary: Some("12 lines".into()),
                    result_ref: None,
                },
            )
            .await
            .expect("finish");
        let err = store
            .finish_tool_call(
                id,
                ToolCallFinish {
                    status: ToolCallStatus::Error,
                    result_summary: None,
                    result_ref: None,
                },
            )
            .await
            .expect_err("twice");
        assert!(matches!(err, StoreError::Conflict(_)));

        let calls = store.tool_calls_for_message(msg).await.expect("list");
        assert_eq!(calls.len(), 1);
        let c = &calls[0];
        assert_eq!(c.status, ToolCallStatus::Ok);
        assert_eq!(c.arguments, json!({ "path": "a.txt" }));
        assert_eq!(c.result_summary.as_deref(), Some("12 lines"));
        assert!(c.finished_at.is_some());
        assert!(c.duration_ms.is_some());

        let dup = store
            .insert_tool_call(NewToolCall {
                message_id: msg,
                call_id: "call_1".into(),
                server: "files".into(),
                tool: "read_file".into(),
                arguments: json!({}),
                status: ToolCallStatus::Pending,
            })
            .await
            .expect_err("duplicate call id");
        assert!(matches!(dup, StoreError::Conflict(_)));
        let bad = store
            .insert_tool_call(NewToolCall {
                message_id: msg,
                call_id: "call_2".into(),
                server: "files".into(),
                tool: "read_file".into(),
                arguments: json!("not an object"),
                status: ToolCallStatus::Pending,
            })
            .await
            .expect_err("bad args");
        assert!(matches!(bad, StoreError::InvalidInput(_)));
        let missing = store
            .finish_tool_call(
                ToolCallId(999),
                ToolCallFinish {
                    status: ToolCallStatus::Ok,
                    result_summary: None,
                    result_ref: None,
                },
            )
            .await
            .expect_err("missing");
        assert!(matches!(missing, StoreError::NotFound { .. }));
    }

    #[tokio::test]
    async fn summary_is_used_only_on_its_branch() {
        let (store, chat, root) = setup().await;
        let a = child(&store, chat, root).await;
        let b = child(&store, chat, a).await;
        let other = child(&store, chat, root).await; // a sibling branch of `a`

        store
            .insert_summary(NewSummary {
                chat_id: chat,
                last_message_id: a,
                text: "covers a".into(),
            })
            .await
            .expect("s1");
        let s_other = store
            .insert_summary(NewSummary {
                chat_id: chat,
                last_message_id: other,
                text: "other".into(),
            })
            .await
            .expect("s2");

        let on_main = store
            .latest_summary_on_path(chat, &[root, a, b])
            .await
            .expect("main");
        assert_eq!(on_main.map(|s| s.text), Some("covers a".to_owned()));
        let on_other = store
            .latest_summary_on_path(chat, &[root, other])
            .await
            .expect("other");
        assert_eq!(on_other.map(|s| s.id), Some(s_other));
        assert!(
            store
                .latest_summary_on_path(chat, &[root])
                .await
                .expect("root")
                .is_none()
        );

        // A deeper summary wins; a newer summary of the same message wins over an older one.
        store
            .insert_summary(NewSummary {
                chat_id: chat,
                last_message_id: b,
                text: "covers b".into(),
            })
            .await
            .expect("s3");
        let newer = store
            .insert_summary(NewSummary {
                chat_id: chat,
                last_message_id: b,
                text: "covers b v2".into(),
            })
            .await
            .expect("s4");
        let best = store
            .latest_summary_on_path(chat, &[root, a, b])
            .await
            .expect("best");
        assert_eq!(best.map(|s| s.id), Some(newer));
    }

    #[tokio::test]
    async fn attachments_and_orphaned_files() {
        let (store, chat, _msg) = setup().await;
        let other = store
            .create_chat(NewChat {
                title: "other".into(),
                ..NewChat::default()
            })
            .await
            .expect("other");
        let new = |chat_id, path: &str| NewAttachment {
            chat_id,
            sha256: "ab".repeat(32),
            mime: "text/plain".into(),
            file_name: "notes.txt".into(),
            size_bytes: 5,
            bytes_path: path.into(),
            extracted_text: Some("hello".into()),
        };
        let a = store
            .insert_attachment(new(chat, "blobs/shared"))
            .await
            .expect("a");
        store
            .insert_attachment(new(chat, "blobs/only-here"))
            .await
            .expect("b");
        store
            .insert_attachment(new(other, "blobs/shared"))
            .await
            .expect("c");
        let got = store.get_attachment(a).await.expect("get").expect("exists");
        assert_eq!(got.size_bytes, 5);
        assert_eq!(
            store.attachments_for_chat(chat).await.expect("list").len(),
            2
        );

        let orphaned = store.delete_chat(chat).await.expect("delete");
        assert_eq!(orphaned, vec!["blobs/only-here".to_owned()]);
        assert!(store.get_attachment(a).await.expect("get").is_none());
        assert_eq!(
            store.attachments_for_chat(other).await.expect("list").len(),
            1
        );
    }
}
