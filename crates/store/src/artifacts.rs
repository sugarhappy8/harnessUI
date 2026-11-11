//! Artifacts (app 0.0.4): documents, pages, diagrams and small tools the model made in a
//! chat, each kept with its versions.
//!
//! An artifact belongs to one chat. Every method takes that chat's id and matches on it,
//! so an id from another chat reads as "not there" and changes nothing: the tools that
//! call these methods work for the chat of the turn that runs them and for no other.
//!
//! A change never rewrites a version. Saving new content adds a version (by the model or
//! by the user), and restoring an old one adds a version with the old content. Only the
//! newest [`MAX_ARTIFACT_VERSIONS`] of an artifact are kept.

use std::fmt;

use rusqlite::types::{FromSql, FromSqlResult, ToSql, ToSqlOutput, ValueRef};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};

use crate::Store;
use crate::chats::ensure_chat;
use crate::error::StoreError;
use crate::ids::{ChatId, now_ms, sql_enum};

/// The most content one version holds, in bytes.
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;
/// The longest title, in characters.
pub const MAX_ARTIFACT_TITLE_CHARS: usize = 120;
/// Versions kept per artifact. Saving one more drops the oldest.
pub const MAX_ARTIFACT_VERSIONS: u32 = 100;
/// Artifacts one chat can hold.
pub const MAX_ARTIFACTS_PER_CHAT: u32 = 200;
/// The longest note on a version, in characters.
const MAX_NOTE_CHARS: usize = 200;

/// Id of a row in `artifacts`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ArtifactId(pub i64);

impl fmt::Display for ArtifactId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl ToSql for ArtifactId {
    fn to_sql(&self) -> rusqlite::Result<ToSqlOutput<'_>> {
        Ok(ToSqlOutput::from(self.0))
    }
}

impl FromSql for ArtifactId {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        i64::column_result(value).map(Self)
    }
}

sql_enum!(
    /// What an artifact is, which decides how it is shown.
    ArtifactKind, "artifact kind" {
        /// A document in Markdown.
        Markdown => "markdown",
        /// One self-contained HTML page: a page or a small interactive tool.
        Html => "html",
        /// An SVG picture.
        Svg => "svg",
        /// A diagram in Mermaid syntax.
        Mermaid => "mermaid",
    }
);

sql_enum!(
    /// Who wrote a version.
    ArtifactAuthor, "artifact author" {
        /// The model, through a tool call.
        Model => "model",
        /// The person using the app, in the panel beside the chat.
        User => "user",
    }
);

/// Input for [`Store::create_artifact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// Display title (one line, at most [`MAX_ARTIFACT_TITLE_CHARS`] characters).
    pub title: String,
    /// What it is.
    pub kind: ArtifactKind,
    /// The content of its first version.
    pub content: String,
    /// Who wrote that version.
    pub author: ArtifactAuthor,
}

/// A row of `artifacts`, with what its versions add up to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Artifact {
    /// Row id.
    pub id: ArtifactId,
    /// The chat it belongs to.
    pub chat_id: ChatId,
    /// Display title.
    pub title: String,
    /// What it is.
    pub kind: ArtifactKind,
    /// Unix ms.
    pub created_at: i64,
    /// Unix ms of the newest version. A rename does not change it.
    pub updated_at: i64,
    /// The number of the newest version. Numbers start at 1 and are never reused.
    pub latest_version: u32,
    /// How many versions are kept.
    pub versions: u32,
}

/// One version of an artifact, with its content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersion {
    /// The artifact.
    pub artifact_id: ArtifactId,
    /// Its number, from 1.
    pub version: u32,
    /// The content.
    pub content: String,
    /// Who wrote it.
    pub author: ArtifactAuthor,
    /// A short remark, such as which version it restores.
    pub note: Option<String>,
    /// Unix ms.
    pub created_at: i64,
}

/// One version without its content, for the list of versions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactVersionInfo {
    /// Its number, from 1.
    pub version: u32,
    /// Who wrote it.
    pub author: ArtifactAuthor,
    /// A short remark, such as which version it restores.
    pub note: Option<String>,
    /// Unix ms.
    pub created_at: i64,
    /// Size of the content in bytes.
    pub bytes: u64,
}

// ---------------------------------------------------------------------------------------
// Checks

/// A title as it is stored: trimmed, one line, not empty, not too long.
fn checked_title(title: &str) -> Result<String, StoreError> {
    let title = title.trim();
    if title.is_empty() {
        return Err(StoreError::InvalidInput("the title is empty".into()));
    }
    if title.contains(['\n', '\r']) {
        return Err(StoreError::InvalidInput(
            "the title must be one line".into(),
        ));
    }
    let chars = title.chars().count();
    if chars > MAX_ARTIFACT_TITLE_CHARS {
        return Err(StoreError::InvalidInput(format!(
            "the title is {chars} characters; the limit is {MAX_ARTIFACT_TITLE_CHARS}"
        )));
    }
    Ok(title.to_owned())
}

fn check_content(content: &str) -> Result<(), StoreError> {
    if content.len() > MAX_ARTIFACT_BYTES {
        return Err(StoreError::InvalidInput(format!(
            "the content is {} bytes; the limit is {MAX_ARTIFACT_BYTES}",
            content.len()
        )));
    }
    Ok(())
}

fn checked_note(note: Option<String>) -> Option<String> {
    note.map(|n| n.trim().chars().take(MAX_NOTE_CHARS).collect::<String>())
        .filter(|n| !n.is_empty())
}

// ---------------------------------------------------------------------------------------
// Rows

/// An artifact with the count and the highest number of its versions.
const ARTIFACT_SELECT: &str = "SELECT a.id, a.chat_id, a.title, a.kind, a.created_at, a.updated_at, \
     (SELECT ifnull(max(v.version), 0) FROM artifact_versions v WHERE v.artifact_id = a.id), \
     (SELECT count(*) FROM artifact_versions v WHERE v.artifact_id = a.id) \
     FROM artifacts a";

fn artifact_from_row(r: &Row<'_>) -> rusqlite::Result<Artifact> {
    Ok(Artifact {
        id: r.get(0)?,
        chat_id: r.get(1)?,
        title: r.get(2)?,
        kind: r.get(3)?,
        created_at: r.get(4)?,
        updated_at: r.get(5)?,
        latest_version: r.get(6)?,
        versions: r.get(7)?,
    })
}

fn version_from_row(r: &Row<'_>) -> rusqlite::Result<ArtifactVersion> {
    Ok(ArtifactVersion {
        artifact_id: r.get(0)?,
        version: r.get(1)?,
        content: r.get(2)?,
        author: r.get(3)?,
        note: r.get(4)?,
        created_at: r.get(5)?,
    })
}

fn get_artifact(
    conn: &Connection,
    chat: ChatId,
    id: ArtifactId,
) -> Result<Option<Artifact>, StoreError> {
    let sql = format!("{ARTIFACT_SELECT} WHERE a.id = ?1 AND a.chat_id = ?2");
    Ok(conn
        .query_row(&sql, params![id, chat], artifact_from_row)
        .optional()?)
}

/// The artifact, or `NotFound` when this chat has none with that id.
fn require_artifact(
    conn: &Connection,
    chat: ChatId,
    id: ArtifactId,
) -> Result<Artifact, StoreError> {
    get_artifact(conn, chat, id)?.ok_or_else(|| StoreError::not_found("artifact", id.0))
}

fn read_version(
    conn: &Connection,
    id: ArtifactId,
    version: Option<u32>,
) -> Result<Option<ArtifactVersion>, StoreError> {
    const COLUMNS: &str = "artifact_id, version, content, author, note, created_at";
    let found = match version {
        Some(n) => conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM artifact_versions WHERE artifact_id = ?1 AND version = ?2"
                ),
                params![id, n],
                version_from_row,
            )
            .optional()?,
        None => conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM artifact_versions WHERE artifact_id = ?1 \
                     ORDER BY version DESC LIMIT 1"
                ),
                params![id],
                version_from_row,
            )
            .optional()?,
    };
    Ok(found)
}

// ---------------------------------------------------------------------------------------
// Writes

fn create_artifact(conn: &mut Connection, new: NewArtifact) -> Result<Artifact, StoreError> {
    let title = checked_title(&new.title)?;
    check_content(&new.content)?;
    ensure_chat(conn, new.chat_id)?;
    let tx = conn.transaction()?;
    let held: u32 = tx.query_row(
        "SELECT count(*) FROM artifacts WHERE chat_id = ?1",
        [new.chat_id],
        |r| r.get(0),
    )?;
    if held >= MAX_ARTIFACTS_PER_CHAT {
        return Err(StoreError::InvalidInput(format!(
            "this chat already has {MAX_ARTIFACTS_PER_CHAT} artifacts, the most a chat can hold"
        )));
    }
    let now = now_ms();
    tx.execute(
        "INSERT INTO artifacts (chat_id, title, kind, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?4)",
        params![new.chat_id, title, new.kind, now],
    )?;
    let id = ArtifactId(tx.last_insert_rowid());
    tx.execute(
        "INSERT INTO artifact_versions (artifact_id, version, content, author, created_at) \
         VALUES (?1, 1, ?2, ?3, ?4)",
        params![id, new.content, new.author, now],
    )?;
    let artifact = require_artifact(&tx, new.chat_id, id)?;
    tx.commit()?;
    Ok(artifact)
}

fn add_version(
    conn: &mut Connection,
    chat: ChatId,
    id: ArtifactId,
    content: String,
    author: ArtifactAuthor,
    note: Option<String>,
) -> Result<Artifact, StoreError> {
    check_content(&content)?;
    let tx = conn.transaction()?;
    let current = require_artifact(&tx, chat, id)?;
    let next = current.latest_version + 1;
    // Later than every earlier change even when the clock stands still or steps back, so
    // "most recently changed first" is the order of the changes.
    let now = now_ms().max(current.updated_at);
    tx.execute(
        "INSERT INTO artifact_versions (artifact_id, version, content, author, note, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![id, next, content, author, checked_note(note), now],
    )?;
    tx.execute(
        "DELETE FROM artifact_versions WHERE artifact_id = ?1 AND version <= ?2",
        params![id, i64::from(next) - i64::from(MAX_ARTIFACT_VERSIONS)],
    )?;
    tx.execute(
        "UPDATE artifacts SET updated_at = ?1 WHERE id = ?2",
        params![now, id],
    )?;
    let artifact = require_artifact(&tx, chat, id)?;
    tx.commit()?;
    Ok(artifact)
}

impl Store {
    /// Creates an artifact with its first version.
    ///
    /// Fails with [`StoreError::InvalidInput`] for an empty or over-long title, content
    /// over [`MAX_ARTIFACT_BYTES`], or a chat that already holds
    /// [`MAX_ARTIFACTS_PER_CHAT`]; with [`StoreError::NotFound`] for a missing chat.
    pub async fn create_artifact(&self, new: NewArtifact) -> Result<Artifact, StoreError> {
        self.call(move |c| create_artifact(c, new)).await
    }

    /// The chat's artifacts, most recently changed first.
    pub async fn list_artifacts(&self, chat_id: ChatId) -> Result<Vec<Artifact>, StoreError> {
        self.call(move |c| {
            let sql = format!(
                "{ARTIFACT_SELECT} WHERE a.chat_id = ?1 ORDER BY a.updated_at DESC, a.id DESC"
            );
            let mut stmt = c.prepare_cached(&sql)?;
            let rows = stmt.query_map([chat_id], artifact_from_row)?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
    }

    /// One artifact of the chat, or `None` (also for an artifact of another chat).
    pub async fn get_artifact(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
    ) -> Result<Option<Artifact>, StoreError> {
        self.call(move |c| get_artifact(c, chat_id, id)).await
    }

    /// One version of an artifact of the chat with its content: `version`, or the newest
    /// when `None`. `None` when the artifact or the version is not there.
    pub async fn artifact_version(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
        version: Option<u32>,
    ) -> Result<Option<ArtifactVersion>, StoreError> {
        self.call(move |c| {
            if get_artifact(c, chat_id, id)?.is_none() {
                return Ok(None);
            }
            read_version(c, id, version)
        })
        .await
    }

    /// The kept versions of an artifact of the chat, oldest first, without content. Empty
    /// when the artifact is not there.
    pub async fn artifact_versions(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
    ) -> Result<Vec<ArtifactVersionInfo>, StoreError> {
        self.call(move |c| {
            let mut stmt = c.prepare_cached(
                "SELECT v.version, v.author, v.note, v.created_at, length(CAST(v.content AS BLOB)) \
                 FROM artifact_versions v JOIN artifacts a ON a.id = v.artifact_id \
                 WHERE v.artifact_id = ?1 AND a.chat_id = ?2 ORDER BY v.version",
            )?;
            let rows = stmt.query_map(params![id, chat_id], |r| {
                Ok(ArtifactVersionInfo {
                    version: r.get(0)?,
                    author: r.get(1)?,
                    note: r.get(2)?,
                    created_at: r.get(3)?,
                    bytes: r.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<_, _>>()?)
        })
        .await
    }

    /// Saves `content` as the next version of an artifact of the chat and returns the
    /// artifact as it now stands. Versions beyond [`MAX_ARTIFACT_VERSIONS`] are dropped,
    /// oldest first.
    ///
    /// Fails with [`StoreError::NotFound`] when the chat has no such artifact, and with
    /// [`StoreError::InvalidInput`] for content over [`MAX_ARTIFACT_BYTES`].
    pub async fn add_artifact_version(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
        content: String,
        author: ArtifactAuthor,
        note: Option<String>,
    ) -> Result<Artifact, StoreError> {
        self.call(move |c| add_version(c, chat_id, id, content, author, note))
            .await
    }

    /// Changes the title of an artifact of the chat.
    pub async fn rename_artifact(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
        title: String,
    ) -> Result<(), StoreError> {
        self.call(move |c| {
            let title = checked_title(&title)?;
            let n = c.execute(
                "UPDATE artifacts SET title = ?1 WHERE id = ?2 AND chat_id = ?3",
                params![title, id, chat_id],
            )?;
            if n == 0 {
                Err(StoreError::not_found("artifact", id.0))
            } else {
                Ok(())
            }
        })
        .await
    }

    /// Deletes an artifact of the chat with all its versions; returns whether it existed.
    pub async fn delete_artifact(
        &self,
        chat_id: ChatId,
        id: ArtifactId,
    ) -> Result<bool, StoreError> {
        self.call(move |c| {
            Ok(c.execute(
                "DELETE FROM artifacts WHERE id = ?1 AND chat_id = ?2",
                params![id, chat_id],
            )? > 0)
        })
        .await
    }

    /// One version of any artifact, with the artifact, for showing it in the preview
    /// frame. The frame's address carries no chat; nothing a tool does goes through here.
    pub async fn artifact_for_preview(
        &self,
        id: ArtifactId,
        version: u32,
    ) -> Result<Option<(Artifact, ArtifactVersion)>, StoreError> {
        self.call(move |c| {
            let sql = format!("{ARTIFACT_SELECT} WHERE a.id = ?1");
            let Some(artifact) = c.query_row(&sql, [id], artifact_from_row).optional()? else {
                return Ok(None);
            };
            Ok(read_version(c, id, Some(version))?.map(|v| (artifact, v)))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chats::NewChat;

    async fn chat(store: &Store, title: &str) -> ChatId {
        store
            .create_chat(NewChat {
                title: title.into(),
                ..NewChat::default()
            })
            .await
            .expect("create chat")
    }

    fn new(chat_id: ChatId, title: &str, kind: ArtifactKind, content: &str) -> NewArtifact {
        NewArtifact {
            chat_id,
            title: title.into(),
            kind,
            content: content.into(),
            author: ArtifactAuthor::Model,
        }
    }

    #[tokio::test]
    async fn create_makes_version_one_and_reads_back() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Tip calculator", ArtifactKind::Html, "<p>hi</p>"))
            .await
            .expect("create");
        assert_eq!(a.chat_id, c);
        assert_eq!(a.title, "Tip calculator");
        assert_eq!(a.kind, ArtifactKind::Html);
        assert_eq!((a.latest_version, a.versions), (1, 1));
        assert!(a.created_at > 0 && a.updated_at == a.created_at);

        assert_eq!(
            store.get_artifact(c, a.id).await.expect("get"),
            Some(a.clone())
        );
        let v = store
            .artifact_version(c, a.id, None)
            .await
            .expect("version")
            .expect("exists");
        assert_eq!(v.version, 1);
        assert_eq!(v.content, "<p>hi</p>");
        assert_eq!(v.author, ArtifactAuthor::Model);
        assert_eq!(v.note, None);
        let infos = store.artifact_versions(c, a.id).await.expect("versions");
        assert_eq!(infos.len(), 1);
        assert_eq!((infos[0].version, infos[0].bytes), (1, 9));
    }

    #[tokio::test]
    async fn each_save_adds_a_version_and_old_ones_stay_readable() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Doc", ArtifactKind::Markdown, "one"))
            .await
            .expect("create");
        let a2 = store
            .add_artifact_version(c, a.id, "two".into(), ArtifactAuthor::User, None)
            .await
            .expect("second");
        assert_eq!((a2.latest_version, a2.versions), (2, 2));
        assert!(a2.updated_at >= a.updated_at);
        let a3 = store
            .add_artifact_version(
                c,
                a.id,
                "one".into(),
                ArtifactAuthor::User,
                Some("Restored from version 1".into()),
            )
            .await
            .expect("third");
        assert_eq!((a3.latest_version, a3.versions), (3, 3));

        let read = |v: Option<u32>| {
            let store = store.clone();
            async move {
                store
                    .artifact_version(c, a.id, v)
                    .await
                    .expect("read")
                    .map(|v| (v.version, v.content, v.author, v.note))
            }
        };
        assert_eq!(
            read(Some(1)).await,
            Some((1, "one".into(), ArtifactAuthor::Model, None))
        );
        assert_eq!(
            read(Some(2)).await,
            Some((2, "two".into(), ArtifactAuthor::User, None))
        );
        assert_eq!(
            read(None).await,
            Some((
                3,
                "one".into(),
                ArtifactAuthor::User,
                Some("Restored from version 1".into())
            ))
        );
        assert_eq!(read(Some(4)).await, None);
        assert_eq!(read(Some(0)).await, None);
        let infos = store.artifact_versions(c, a.id).await.expect("versions");
        assert_eq!(
            infos.iter().map(|i| i.version).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(infos[2].note.as_deref(), Some("Restored from version 1"));
    }

    /// What the tools rely on: an artifact of another chat cannot be read or changed.
    #[tokio::test]
    async fn an_artifact_of_another_chat_cannot_be_read_or_changed() {
        let store = Store::open_in_memory().expect("open");
        let mine = chat(&store, "mine").await;
        let other = chat(&store, "other").await;
        let theirs = store
            .create_artifact(new(other, "Private", ArtifactKind::Markdown, "secret"))
            .await
            .expect("create");

        assert_eq!(
            store.get_artifact(mine, theirs.id).await.expect("get"),
            None
        );
        assert_eq!(
            store
                .artifact_version(mine, theirs.id, None)
                .await
                .expect("version"),
            None
        );
        assert_eq!(
            store
                .artifact_version(mine, theirs.id, Some(1))
                .await
                .expect("version"),
            None
        );
        assert!(
            store
                .artifact_versions(mine, theirs.id)
                .await
                .expect("versions")
                .is_empty()
        );
        assert!(store.list_artifacts(mine).await.expect("list").is_empty());
        assert!(matches!(
            store
                .add_artifact_version(mine, theirs.id, "x".into(), ArtifactAuthor::Model, None)
                .await,
            Err(StoreError::NotFound { .. })
        ));
        assert!(matches!(
            store
                .rename_artifact(mine, theirs.id, "Mine now".into())
                .await,
            Err(StoreError::NotFound { .. })
        ));
        assert!(
            !store
                .delete_artifact(mine, theirs.id)
                .await
                .expect("delete")
        );

        // Nothing changed for its own chat.
        let still = store
            .get_artifact(other, theirs.id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(still, theirs);
        assert_eq!(
            store
                .artifact_version(other, theirs.id, None)
                .await
                .expect("version")
                .expect("exists")
                .content,
            "secret"
        );
    }

    #[tokio::test]
    async fn list_is_newest_change_first() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "A", ArtifactKind::Svg, "<svg/>"))
            .await
            .expect("a");
        let b = store
            .create_artifact(new(c, "B", ArtifactKind::Mermaid, "graph TD"))
            .await
            .expect("b");
        let titles = |list: Vec<Artifact>| list.into_iter().map(|a| a.title).collect::<Vec<_>>();
        // Same millisecond or not, the newer row comes first.
        assert_eq!(
            titles(store.list_artifacts(c).await.expect("list")),
            ["B", "A"]
        );
        tokio::time::sleep(std::time::Duration::from_millis(3)).await;
        store
            .add_artifact_version(c, a.id, "<svg></svg>".into(), ArtifactAuthor::Model, None)
            .await
            .expect("update a");
        assert_eq!(
            titles(store.list_artifacts(c).await.expect("list")),
            ["A", "B"]
        );
        let _ = b;
    }

    #[tokio::test]
    async fn limits_are_refused_with_a_reason() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let invalid = |r: Result<Artifact, StoreError>| match r {
            Err(StoreError::InvalidInput(m)) => m,
            other => panic!("expected InvalidInput, got {other:?}"),
        };
        let big = "x".repeat(MAX_ARTIFACT_BYTES + 1);
        assert!(
            invalid(
                store
                    .create_artifact(new(c, "Big", ArtifactKind::Html, &big))
                    .await
            )
            .contains("1048576")
        );
        assert!(
            invalid(
                store
                    .create_artifact(new(c, "  ", ArtifactKind::Html, "x"))
                    .await
            )
            .contains("title")
        );
        let long = "t".repeat(MAX_ARTIFACT_TITLE_CHARS + 1);
        assert!(
            invalid(
                store
                    .create_artifact(new(c, &long, ArtifactKind::Html, "x"))
                    .await
            )
            .contains("120")
        );
        assert!(
            invalid(
                store
                    .create_artifact(new(c, "two\nlines", ArtifactKind::Html, "x"))
                    .await
            )
            .contains("one line")
        );
        // Exactly at the limits is fine.
        let exact = "x".repeat(MAX_ARTIFACT_BYTES);
        let title = "t".repeat(MAX_ARTIFACT_TITLE_CHARS);
        let a = store
            .create_artifact(new(c, &title, ArtifactKind::Html, &exact))
            .await
            .expect("at the limit");
        assert!(
            invalid(
                store
                    .add_artifact_version(c, a.id, big, ArtifactAuthor::User, None)
                    .await
            )
            .contains("1048576")
        );
        assert!(matches!(
            store.rename_artifact(c, a.id, String::new()).await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store.rename_artifact(c, a.id, long).await,
            Err(StoreError::InvalidInput(_))
        ));
        assert!(matches!(
            store
                .create_artifact(new(ChatId(404), "x", ArtifactKind::Html, "x"))
                .await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn a_chat_holds_a_limited_number_of_artifacts() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        for i in 0..MAX_ARTIFACTS_PER_CHAT {
            store
                .create_artifact(new(c, &format!("a{i}"), ArtifactKind::Markdown, "x"))
                .await
                .expect("create");
        }
        match store
            .create_artifact(new(c, "one too many", ArtifactKind::Markdown, "x"))
            .await
        {
            Err(StoreError::InvalidInput(m)) => assert!(m.contains("200"), "{m}"),
            other => panic!("expected a refusal, got {other:?}"),
        }
        // Another chat is not affected.
        let other = chat(&store, "other").await;
        store
            .create_artifact(new(other, "fine", ArtifactKind::Markdown, "x"))
            .await
            .expect("another chat");
    }

    #[tokio::test]
    async fn only_the_newest_versions_are_kept_and_numbers_go_on() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Doc", ArtifactKind::Markdown, "v1"))
            .await
            .expect("create");
        let mut last = a.clone();
        for n in 2..=MAX_ARTIFACT_VERSIONS + 5 {
            last = store
                .add_artifact_version(c, a.id, format!("v{n}"), ArtifactAuthor::Model, None)
                .await
                .expect("add");
        }
        assert_eq!(last.latest_version, MAX_ARTIFACT_VERSIONS + 5);
        assert_eq!(last.versions, MAX_ARTIFACT_VERSIONS);
        let infos = store.artifact_versions(c, a.id).await.expect("versions");
        assert_eq!(infos.len() as u32, MAX_ARTIFACT_VERSIONS);
        assert_eq!(infos.first().map(|i| i.version), Some(6));
        assert_eq!(
            infos.last().map(|i| i.version),
            Some(MAX_ARTIFACT_VERSIONS + 5)
        );
        assert_eq!(
            store
                .artifact_version(c, a.id, Some(5))
                .await
                .expect("read"),
            None
        );
        assert_eq!(
            store
                .artifact_version(c, a.id, Some(6))
                .await
                .expect("read")
                .map(|v| v.content),
            Some("v6".into())
        );
    }

    #[tokio::test]
    async fn rename_and_delete() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Old", ArtifactKind::Markdown, "x"))
            .await
            .expect("create");
        store
            .rename_artifact(c, a.id, "  New name ".into())
            .await
            .expect("rename");
        let got = store
            .get_artifact(c, a.id)
            .await
            .expect("get")
            .expect("exists");
        assert_eq!(got.title, "New name");
        assert_eq!(got.latest_version, 1, "a rename is not a version");
        assert!(store.delete_artifact(c, a.id).await.expect("delete"));
        assert!(!store.delete_artifact(c, a.id).await.expect("delete again"));
        assert_eq!(store.get_artifact(c, a.id).await.expect("get"), None);
        assert_eq!(
            store.artifact_version(c, a.id, None).await.expect("read"),
            None
        );
        assert!(matches!(
            store
                .add_artifact_version(c, a.id, "x".into(), ArtifactAuthor::User, None)
                .await,
            Err(StoreError::NotFound { .. })
        ));
    }

    #[tokio::test]
    async fn deleting_the_chat_deletes_its_artifacts() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Doc", ArtifactKind::Markdown, "x"))
            .await
            .expect("create");
        assert!(
            store
                .artifact_for_preview(a.id, 1)
                .await
                .expect("preview")
                .is_some()
        );
        store.delete_chat(c).await.expect("delete chat");
        assert_eq!(
            store.artifact_for_preview(a.id, 1).await.expect("preview"),
            None
        );
        let left: i64 = store
            .call(|conn| {
                Ok(conn.query_row("SELECT count(*) FROM artifact_versions", [], |r| r.get(0))?)
            })
            .await
            .expect("count");
        assert_eq!(left, 0, "versions of a deleted chat's artifacts stayed");
    }

    #[tokio::test]
    async fn the_preview_reads_one_exact_version() {
        let store = Store::open_in_memory().expect("open");
        let c = chat(&store, "c").await;
        let a = store
            .create_artifact(new(c, "Page", ArtifactKind::Html, "<b>1</b>"))
            .await
            .expect("create");
        store
            .add_artifact_version(c, a.id, "<b>2</b>".into(), ArtifactAuthor::Model, None)
            .await
            .expect("add");
        let (artifact, version) = store
            .artifact_for_preview(a.id, 1)
            .await
            .expect("preview")
            .expect("exists");
        assert_eq!(
            (artifact.kind, artifact.latest_version),
            (ArtifactKind::Html, 2)
        );
        assert_eq!(version.content, "<b>1</b>");
        assert_eq!(
            store.artifact_for_preview(a.id, 3).await.expect("none"),
            None
        );
        assert_eq!(
            store
                .artifact_for_preview(ArtifactId(a.id.0 + 1), 1)
                .await
                .expect("none"),
            None
        );
    }

    #[test]
    fn kinds_and_authors_have_fixed_names() {
        assert_eq!(
            ArtifactKind::ALL
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>(),
            ["markdown", "html", "svg", "mermaid"]
        );
        assert_eq!("svg".parse::<ArtifactKind>().ok(), Some(ArtifactKind::Svg));
        assert!("pdf".parse::<ArtifactKind>().is_err());
        assert_eq!(ArtifactAuthor::User.as_str(), "user");
        assert_eq!(
            serde_json::to_string(&ArtifactKind::Mermaid).expect("json"),
            "\"mermaid\""
        );
    }
}
