//! Stored tool output (app 0.0.4): the full text of a tool result the model saw
//! shortened, so the model can read more of it (`read_output`) and the user can open it
//! from the tool card.
//!
//! - An output belongs to one chat and is deleted with it. Every read names the chat, and
//!   an id from another chat reads as "not found".
//! - One output holds at most [`MAX_OUTPUT_BYTES`]; a longer one keeps its head and tail
//!   with a marker between them.
//! - A chat keeps its [`MAX_OUTPUTS_PER_CHAT`] newest outputs.

use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::chats::ensure_chat;
use crate::error::StoreError;
use crate::ids::{ChatId, now_ms};

/// One stored output holds at most this many bytes.
pub const MAX_OUTPUT_BYTES: usize = 2 * 1024 * 1024;

/// Stored outputs kept per chat (the newest).
pub const MAX_OUTPUTS_PER_CHAT: usize = 200;

/// A line of stored output longer than this many characters counts as several lines of at
/// most this length (see [`output_lines`]).
pub const MAX_LINE_CHARS: usize = 2000;

/// How far a cut may move to land on a line break.
const LINE_BREAK_WINDOW: usize = 4096;

/// Room kept for the marker of a capped output.
const MARKER_ROOM: usize = 200;

/// Input for [`Store::add_tool_output`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToolOutput {
    /// The chat whose turn made the call.
    pub chat_id: ChatId,
    /// The id of the tool call the output belongs to (may be empty).
    pub call_id: String,
    /// The tool's name as the model called it.
    pub tool: String,
    /// The full text.
    pub content: String,
}

/// A stored output without its text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolOutputInfo {
    /// Row id: the number the model and the tool card name the output by.
    pub id: i64,
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// The id of the tool call it came from (may be empty).
    pub call_id: String,
    /// The tool's name.
    pub tool: String,
    /// Bytes of the stored text.
    pub bytes: u64,
    /// Lines of the stored text, counted as [`output_lines`] counts them.
    pub lines: u64,
    /// Unix ms.
    pub created_at: i64,
}

/// A stored output with its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToolOutput {
    /// What it is.
    pub info: ToolOutputInfo,
    /// The stored text.
    pub content: String,
}

/// One line of a stored output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputLine<'a> {
    /// The line's text, without its line break.
    pub text: &'a str,
    /// The next line continues this one: a line longer than [`MAX_LINE_CHARS`] was split.
    pub continued: bool,
}

/// The lines of a stored output, as line numbers count them everywhere (the `lines`
/// column, `read_output` ranges and matches): split at line breaks like [`str::lines`],
/// and a line longer than [`MAX_LINE_CHARS`] characters is split into pieces of that
/// length, so that one huge line (minified code, a JSON document) can still be read in
/// parts.
pub fn output_lines(text: &str) -> Vec<OutputLine<'_>> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        loop {
            match rest.char_indices().nth(MAX_LINE_CHARS) {
                Some((cut, _)) => {
                    out.push(OutputLine {
                        text: &rest[..cut],
                        continued: true,
                    });
                    rest = &rest[cut..];
                }
                None => {
                    out.push(OutputLine {
                        text: rest,
                        continued: false,
                    });
                    break;
                }
            }
        }
    }
    out
}

/// Cuts `content` to [`MAX_OUTPUT_BYTES`]: the head and the tail are kept, with a marker
/// line between them that says how much is missing.
fn cap(content: String) -> String {
    if content.len() <= MAX_OUTPUT_BYTES {
        return content;
    }
    let keep = MAX_OUTPUT_BYTES - MARKER_ROOM;
    // The head ends after a line break when one is near, else on a character boundary.
    let mut head_end = keep / 2;
    while !content.is_char_boundary(head_end) {
        head_end -= 1;
    }
    if let Some(pos) = content[..head_end].rfind('\n')
        && head_end - pos <= LINE_BREAK_WINDOW
    {
        head_end = pos + 1;
    }
    // The tail starts after a line break when one is near.
    let mut tail_start = content.len() - (keep - keep / 2);
    while !content.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    if let Some(pos) = content[tail_start..].find('\n')
        && pos < LINE_BREAK_WINDOW
    {
        tail_start += pos + 1;
    }
    let head = &content[..head_end];
    let tail = &content[tail_start..];
    let marker = format!(
        "[... {} bytes in the middle of this output were not kept: it was larger than 2 MB ...]",
        tail_start - head_end
    );
    let mut out = String::with_capacity(head.len() + marker.len() + tail.len() + 2);
    out.push_str(head);
    if !head.is_empty() && !head.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&marker);
    out.push('\n');
    out.push_str(tail);
    out
}

const COLUMNS: &str = "id, chat_id, call_id, tool, bytes, lines, created_at";

fn info_from_row(r: &Row<'_>) -> rusqlite::Result<ToolOutputInfo> {
    Ok(ToolOutputInfo {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        call_id: r.get(2)?,
        tool: r.get(3)?,
        bytes: r.get(4)?,
        lines: r.get(5)?,
        created_at: r.get(6)?,
    })
}

fn short(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn add(conn: &mut Connection, new: NewToolOutput) -> Result<ToolOutputInfo, StoreError> {
    ensure_chat(conn, new.chat_id)?;
    let content = cap(new.content);
    let bytes = i64::try_from(content.len()).unwrap_or(i64::MAX);
    let lines = i64::try_from(output_lines(&content).len()).unwrap_or(i64::MAX);
    let created_at = now_ms();
    let call_id = short(&new.call_id, 200);
    let tool = short(&new.tool, 200);
    let tx = conn.transaction()?;
    tx.execute(
        "INSERT INTO tool_outputs (chat_id, call_id, tool, content, bytes, lines, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![new.chat_id, call_id, tool, content, bytes, lines, created_at],
    )?;
    let id = tx.last_insert_rowid();
    // The chat keeps its newest outputs.
    tx.execute(
        "DELETE FROM tool_outputs WHERE chat_id = ?1 AND id NOT IN \
         (SELECT id FROM tool_outputs WHERE chat_id = ?1 ORDER BY id DESC LIMIT ?2)",
        params![
            new.chat_id,
            i64::try_from(MAX_OUTPUTS_PER_CHAT).unwrap_or(i64::MAX)
        ],
    )?;
    tx.commit()?;
    Ok(ToolOutputInfo {
        id,
        chat_id: new.chat_id,
        call_id,
        tool,
        bytes: u64::try_from(bytes).unwrap_or(0),
        lines: u64::try_from(lines).unwrap_or(0),
        created_at,
    })
}

fn get(conn: &Connection, chat: ChatId, id: i64) -> Result<Option<StoredToolOutput>, StoreError> {
    // The chat is part of the key: an id alone never reads an output.
    let sql = format!("SELECT {COLUMNS}, content FROM tool_outputs WHERE id = ?1 AND chat_id = ?2");
    Ok(conn
        .query_row(&sql, params![id, chat], |r| {
            Ok(StoredToolOutput {
                info: info_from_row(r)?,
                content: r.get(7)?,
            })
        })
        .optional()?)
}

fn list(conn: &Connection, chat: ChatId) -> Result<Vec<ToolOutputInfo>, StoreError> {
    let sql = format!("SELECT {COLUMNS} FROM tool_outputs WHERE chat_id = ?1 ORDER BY id DESC");
    let mut stmt = conn.prepare_cached(&sql)?;
    let rows = stmt.query_map([chat], info_from_row)?;
    Ok(rows.collect::<Result<_, _>>()?)
}

impl Store {
    /// Stores the full text of a tool result for `new.chat_id` and returns what was
    /// stored. The text is cut to [`MAX_OUTPUT_BYTES`] (head and tail kept), and the
    /// chat's outputs beyond the [`MAX_OUTPUTS_PER_CHAT`] newest are deleted.
    pub async fn add_tool_output(&self, new: NewToolOutput) -> Result<ToolOutputInfo, StoreError> {
        self.call(move |c| add(c, new)).await
    }

    /// The stored output `id`, if it belongs to `chat`. An output of another chat is
    /// `None`, exactly like one that does not exist.
    pub async fn get_tool_output(
        &self,
        chat: ChatId,
        id: i64,
    ) -> Result<Option<StoredToolOutput>, StoreError> {
        self.call(move |c| get(c, chat, id)).await
    }

    /// The chat's stored outputs without their text, newest first.
    pub async fn list_tool_outputs(&self, chat: ChatId) -> Result<Vec<ToolOutputInfo>, StoreError> {
        self.call(move |c| list(c, chat)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NewChat;

    async fn chat(store: &Store) -> ChatId {
        store
            .create_chat(NewChat {
                title: "t".into(),
                ..NewChat::default()
            })
            .await
            .expect("create chat")
    }

    fn new(chat_id: ChatId, content: &str) -> NewToolOutput {
        NewToolOutput {
            chat_id,
            call_id: "call_1".into(),
            tool: "run_command".into(),
            content: content.into(),
        }
    }

    #[tokio::test]
    async fn an_output_is_stored_and_read_back() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store).await;
        let info = store
            .add_tool_output(new(c, "one\ntwo\nthree\n"))
            .await
            .expect("add");
        assert_eq!(info.chat_id, c);
        assert_eq!(info.tool, "run_command");
        assert_eq!(info.call_id, "call_1");
        assert_eq!(info.bytes, 14);
        assert_eq!(info.lines, 3);
        let stored = store
            .get_tool_output(c, info.id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(stored.content, "one\ntwo\nthree\n");
        assert_eq!(stored.info, info);
        assert_eq!(store.list_tool_outputs(c).await.expect("list"), vec![info]);
    }

    /// The per-chat rule: an id from another chat reads as if it did not exist.
    #[tokio::test]
    async fn another_chat_cannot_read_an_output() {
        let store = Store::open_in_memory().expect("open");
        let mine = chat(&store).await;
        let other = chat(&store).await;
        let info = store
            .add_tool_output(new(mine, "secret of my chat"))
            .await
            .expect("add");
        assert!(
            store
                .get_tool_output(other, info.id)
                .await
                .expect("get")
                .is_none(),
            "another chat read the output"
        );
        assert!(
            store
                .list_tool_outputs(other)
                .await
                .expect("list")
                .is_empty()
        );
        assert!(
            store
                .get_tool_output(mine, info.id)
                .await
                .expect("get")
                .is_some()
        );
        assert!(
            store
                .get_tool_output(mine, info.id + 1)
                .await
                .expect("get")
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_huge_output_keeps_its_head_and_tail() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store).await;
        let mut text = String::from("FIRST LINE\n");
        while text.len() < 3 * 1024 * 1024 {
            text.push_str("filler filler filler filler filler filler filler\n");
        }
        text.push_str("LAST LINE\n");
        let total = text.len();
        let info = store.add_tool_output(new(c, &text)).await.expect("add");
        let stored = store
            .get_tool_output(c, info.id)
            .await
            .expect("get")
            .expect("exists")
            .content;
        assert!(stored.len() <= MAX_OUTPUT_BYTES, "{}", stored.len());
        assert!(
            stored.len() > MAX_OUTPUT_BYTES - 3 * LINE_BREAK_WINDOW,
            "{}",
            stored.len()
        );
        assert_eq!(info.bytes, stored.len() as u64);
        assert!(stored.starts_with("FIRST LINE\n"));
        assert!(stored.ends_with("LAST LINE\n"));
        let marker = stored
            .lines()
            .find(|l| l.starts_with("[... "))
            .expect("a marker line");
        let omitted = total - (stored.len() - marker.len() - 1);
        assert_eq!(
            marker,
            format!(
                "[... {omitted} bytes in the middle of this output were not kept: it was larger than 2 MB ...]"
            )
        );
        // Whole lines on both sides of the marker.
        let (head, tail) = stored.split_once(marker).expect("split");
        assert!(head.ends_with("filler\n"), "{:?}", &head[head.len() - 20..]);
        assert!(tail.starts_with("\nfiller filler"), "{:?}", &tail[..20]);
    }

    #[test]
    fn capping_never_splits_a_character() {
        // No line breaks at all, and multi-byte characters across both cut points.
        let text = "é".repeat(MAX_OUTPUT_BYTES);
        let capped = cap(text);
        assert!(capped.len() <= MAX_OUTPUT_BYTES);
        assert!(capped.starts_with("éé") && capped.ends_with("éé"));
        assert!(capped.contains("were not kept"));
        // At the limit nothing changes.
        let exact = "a".repeat(MAX_OUTPUT_BYTES);
        assert_eq!(cap(exact.clone()), exact);
    }

    #[tokio::test]
    async fn a_chat_keeps_its_newest_outputs() {
        let store = Store::open_in_memory().expect("open");
        let busy = chat(&store).await;
        let quiet = chat(&store).await;
        let kept_elsewhere = store
            .add_tool_output(new(quiet, "other chat"))
            .await
            .expect("add");
        let mut ids = Vec::new();
        for i in 0..MAX_OUTPUTS_PER_CHAT + 5 {
            let info = store
                .add_tool_output(new(busy, &format!("output {i}")))
                .await
                .expect("add");
            ids.push(info.id);
        }
        let listed = store.list_tool_outputs(busy).await.expect("list");
        assert_eq!(listed.len(), MAX_OUTPUTS_PER_CHAT);
        // Newest first, and the five oldest are gone.
        assert_eq!(listed[0].id, *ids.last().expect("ids"));
        assert_eq!(listed.last().expect("listed").id, ids[5]);
        for old in &ids[..5] {
            assert!(
                store
                    .get_tool_output(busy, *old)
                    .await
                    .expect("get")
                    .is_none()
            );
        }
        // Another chat's outputs are not counted or touched.
        assert!(
            store
                .get_tool_output(quiet, kept_elsewhere.id)
                .await
                .expect("get")
                .is_some()
        );
    }

    #[tokio::test]
    async fn outputs_go_with_their_chat_and_need_one() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store).await;
        let info = store.add_tool_output(new(c, "x")).await.expect("add");
        store.delete_chat(c).await.expect("delete chat");
        assert!(
            store
                .get_tool_output(c, info.id)
                .await
                .expect("get")
                .is_none()
        );
        let err = store
            .add_tool_output(new(ChatId(9999), "x"))
            .await
            .expect_err("no such chat");
        assert!(matches!(err, StoreError::NotFound { .. }), "{err:?}");
    }

    #[test]
    fn lines_are_counted_like_text_lines_and_long_ones_are_split() {
        let texts = |text: &str| -> Vec<String> {
            output_lines(text)
                .iter()
                .map(|l| l.text.to_string())
                .collect()
        };
        assert!(output_lines("").is_empty());
        assert_eq!(texts("a\nb"), ["a", "b"]);
        assert_eq!(texts("a\nb\n"), ["a", "b"]);
        assert_eq!(texts("a\r\n\r\nb\r\n"), ["a", "", "b"]);
        // A line of 2.5 times the limit becomes three lines; the pieces say they continue.
        let long = "é".repeat(MAX_LINE_CHARS * 2 + MAX_LINE_CHARS / 2);
        let text = format!("start\n{long}\nend");
        let lines = output_lines(&text);
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[1].text.chars().count(), MAX_LINE_CHARS);
        assert_eq!(lines[3].text.chars().count(), MAX_LINE_CHARS / 2);
        assert_eq!(
            lines.iter().map(|l| l.continued).collect::<Vec<_>>(),
            [false, true, true, false, false]
        );
        let joined: String = lines[1..4].iter().map(|l| l.text).collect();
        assert_eq!(joined, long);
        // Exactly at the limit: one line.
        assert_eq!(output_lines(&"x".repeat(MAX_LINE_CHARS)).len(), 1);
    }
}
