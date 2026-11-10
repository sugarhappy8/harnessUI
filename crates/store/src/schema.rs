//! Schema migrations, tracked in `PRAGMA user_version`.
//!
//! Rules for later milestones: never edit a migration that has shipped; append a new
//! entry to [`MIGRATIONS`] instead. Pragmas (WAL, synchronous, foreign keys, busy timeout)
//! are set at open time in [`crate::actor`], outside migrations.
//!
//! A small runner instead of `rusqlite_migration`: that crate pins rusqlite 0.40, whose
//! bundled SQLite has RUSTSEC-2026-0459, and the runner is a few lines.

use rusqlite::Connection;

/// Version 1: every table listed in ARCHITECTURE.md section 12.
///
/// Every table whose rows can be deleted uses `AUTOINCREMENT`, so an id is never handed out
/// twice. Without it SQLite reuses the highest deleted rowid: audit rows of a deleted chat
/// (`audit_log.chat_id` deliberately outlives the chat) would be attributed to the next
/// chat, and a stale id held by a running turn, tool or dialog could hit a different row.
const V1: &str = r#"
CREATE TABLE chats (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    title            TEXT    NOT NULL,
    model_ref        TEXT,
    system_prompt    TEXT,
    leaf_message_id  INTEGER REFERENCES messages(id) ON DELETE SET NULL,
    tool_prefs       TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(tool_prefs)),
    working_folder   TEXT,
    created_at       INTEGER NOT NULL,
    updated_at       INTEGER NOT NULL,
    archived         INTEGER NOT NULL DEFAULT 0 CHECK (archived IN (0, 1))
);
CREATE INDEX chats_updated ON chats(updated_at DESC, id DESC);
CREATE INDEX chats_leaf ON chats(leaf_message_id);

CREATE TABLE messages (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id            INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    parent_id          INTEGER REFERENCES messages(id) ON DELETE CASCADE,
    role               TEXT    NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content            TEXT    NOT NULL CHECK (json_valid(content)),
    reasoning          TEXT,
    status             TEXT    NOT NULL DEFAULT 'complete'
                               CHECK (status IN ('streaming', 'complete', 'stopped', 'error')),
    model_ref          TEXT,
    prompt_tokens      INTEGER,
    completion_tokens  INTEGER,
    tokens_per_second  REAL,
    error              TEXT    CHECK (error IS NULL OR json_valid(error)),
    search_text        TEXT    NOT NULL DEFAULT '',
    created_at         INTEGER NOT NULL
);
CREATE INDEX messages_chat ON messages(chat_id, id);
CREATE INDEX messages_parent ON messages(parent_id);

-- A message's parent must be in the same chat.
CREATE TRIGGER messages_parent_same_chat BEFORE INSERT ON messages
WHEN NEW.parent_id IS NOT NULL
 AND (SELECT chat_id FROM messages WHERE id = NEW.parent_id) IS NOT NEW.chat_id
BEGIN
    SELECT RAISE(ABORT, 'parent message belongs to another chat');
END;

-- Full-text index over the plain-text projection callers store in messages.search_text.
-- External content: the text lives once, in messages. Trigram: substring and CJK search.
CREATE VIRTUAL TABLE messages_fts USING fts5(
    search_text,
    content = 'messages',
    content_rowid = 'id',
    tokenize = 'trigram'
);
CREATE TRIGGER messages_fts_insert AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, search_text) VALUES (NEW.id, NEW.search_text);
END;
CREATE TRIGGER messages_fts_delete AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, search_text)
        VALUES ('delete', OLD.id, OLD.search_text);
END;
CREATE TRIGGER messages_fts_update AFTER UPDATE OF search_text ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, search_text)
        VALUES ('delete', OLD.id, OLD.search_text);
    INSERT INTO messages_fts(rowid, search_text) VALUES (NEW.id, NEW.search_text);
END;

CREATE TABLE tool_calls (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id      INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    call_id         TEXT    NOT NULL,
    server          TEXT    NOT NULL,
    tool            TEXT    NOT NULL,
    arguments       TEXT    NOT NULL CHECK (json_valid(arguments)),
    status          TEXT    NOT NULL CHECK (status IN
                        ('pending', 'running', 'ok', 'error', 'denied', 'cancelled', 'not_run', 'interrupted')),
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    duration_ms     INTEGER,
    result_summary  TEXT,
    result_ref      TEXT,
    UNIQUE (message_id, call_id)
);

CREATE TABLE summaries (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id          INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    last_message_id  INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    text             TEXT    NOT NULL,
    created_at       INTEGER NOT NULL
);
CREATE INDEX summaries_chat ON summaries(chat_id, last_message_id);
CREATE INDEX summaries_last_message ON summaries(last_message_id);

CREATE TABLE attachments (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    sha256          TEXT    NOT NULL,
    mime            TEXT    NOT NULL,
    file_name       TEXT    NOT NULL,
    size_bytes      INTEGER NOT NULL CHECK (size_bytes >= 0),
    bytes_path      TEXT    NOT NULL,
    extracted_text  TEXT,
    created_at      INTEGER NOT NULL
);
CREATE INDEX attachments_chat ON attachments(chat_id);
CREATE INDEX attachments_bytes_path ON attachments(bytes_path);

-- Append-only: one row per tool call or held-navigation decision (section 13.7).
-- chat_id is deliberately not a foreign key: deleting a chat must not erase its audit trail.
CREATE TABLE audit_log (
    id               INTEGER PRIMARY KEY,
    ts               INTEGER NOT NULL,
    chat_id          INTEGER,
    server           TEXT    NOT NULL,
    tool             TEXT    NOT NULL,
    arguments        TEXT    NOT NULL CHECK (json_valid(arguments)),
    preview          TEXT    NOT NULL,
    risk             TEXT    NOT NULL,
    decision         TEXT    NOT NULL CHECK (decision IN
                        ('allow_once', 'allow_for_chat', 'allowed_by_grant', 'allowed_without_prompt',
                         'denied', 'cancelled', 'refused_by_policy')),
    decided_by       TEXT    NOT NULL,
    note             TEXT,
    result_status    TEXT,
    result_summary   TEXT,
    duration_ms      INTEGER,
    parent_audit_id  INTEGER REFERENCES audit_log(id)
);
CREATE INDEX audit_log_ts ON audit_log(ts, id);
CREATE INDEX audit_log_chat ON audit_log(chat_id, ts);
CREATE INDEX audit_log_parent ON audit_log(parent_audit_id);

CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log BEGIN
    SELECT RAISE(ABORT, 'audit_log is append-only');
END;
-- The only permitted update fills in the result of a row that has none yet.
CREATE TRIGGER audit_log_no_rewrite BEFORE UPDATE ON audit_log
WHEN OLD.result_status IS NOT NULL
  OR NEW.id IS NOT OLD.id
  OR NEW.ts IS NOT OLD.ts
  OR NEW.chat_id IS NOT OLD.chat_id
  OR NEW.server IS NOT OLD.server
  OR NEW.tool IS NOT OLD.tool
  OR NEW.arguments IS NOT OLD.arguments
  OR NEW.preview IS NOT OLD.preview
  OR NEW.risk IS NOT OLD.risk
  OR NEW.decision IS NOT OLD.decision
  OR NEW.decided_by IS NOT OLD.decided_by
  OR NEW.note IS NOT OLD.note
  OR NEW.parent_audit_id IS NOT OLD.parent_audit_id
BEGIN
    SELECT RAISE(ABORT, 'audit_log is append-only');
END;

CREATE TABLE models (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    path          TEXT    NOT NULL UNIQUE,
    repo          TEXT,
    file_name     TEXT    NOT NULL,
    size_bytes    INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256        TEXT,
    gguf_summary  TEXT    NOT NULL DEFAULT '{}' CHECK (json_valid(gguf_summary)),
    added_at      INTEGER NOT NULL,
    source        TEXT    NOT NULL CHECK (source IN ('download', 'import'))
);

CREATE TABLE downloads (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    group_id    TEXT    NOT NULL,
    repo        TEXT    NOT NULL,
    file        TEXT    NOT NULL,
    url         TEXT    NOT NULL,
    dest_path   TEXT    NOT NULL,
    size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
    sha256      TEXT    NOT NULL,
    bytes_done  INTEGER NOT NULL DEFAULT 0 CHECK (bytes_done >= 0),
    status      TEXT    NOT NULL CHECK (status IN
                    ('queued', 'downloading', 'paused', 'verifying', 'completed', 'failed', 'cancelled')),
    etag        TEXT,
    error       TEXT,
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX downloads_group ON downloads(group_id);

CREATE TABLE providers (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT    NOT NULL,
    kind        TEXT    NOT NULL,
    base_url    TEXT    NOT NULL,
    secret_ref  TEXT,
    enabled     INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
    created_at  INTEGER NOT NULL
);

CREATE TABLE mcp_servers (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    name             TEXT    NOT NULL UNIQUE,
    config           TEXT    NOT NULL CHECK (json_valid(config)),
    enabled          INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    acknowledged_at  INTEGER,
    protocol_era     TEXT,
    created_at       INTEGER NOT NULL
);

CREATE TABLE mcp_tool_prefs (
    server_id       INTEGER NOT NULL REFERENCES mcp_servers(id) ON DELETE CASCADE,
    tool_name       TEXT    NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    allow_any_args  INTEGER NOT NULL DEFAULT 0 CHECK (allow_any_args IN (0, 1)),
    always_ask      INTEGER NOT NULL DEFAULT 0 CHECK (always_ask IN (0, 1)),
    fingerprint     TEXT,
    PRIMARY KEY (server_id, tool_name)
);

CREATE TABLE skills (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    name         TEXT    NOT NULL UNIQUE,
    dir          TEXT    NOT NULL,
    enabled      INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
    imported_at  INTEGER NOT NULL
);

CREATE TABLE approved_folders (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    path        TEXT    NOT NULL,
    chat_id     INTEGER REFERENCES chats(id) ON DELETE CASCADE,
    created_at  INTEGER NOT NULL
);
-- NULL chat_id means global; ifnull() makes two global rows for one path collide.
CREATE UNIQUE INDEX approved_folders_unique ON approved_folders(path, ifnull(chat_id, 0));
CREATE INDEX approved_folders_chat ON approved_folders(chat_id);

CREATE TABLE settings (
    key         TEXT    PRIMARY KEY,
    value       TEXT    NOT NULL CHECK (json_valid(value)),
    updated_at  INTEGER NOT NULL
);
"#;

/// One schema step: the SQL that takes `user_version` from its index to index + 1.
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    pub sql: &'static str,
}

/// The migration list, applied in order.
#[derive(Debug, Clone, Copy)]
pub struct Migrations(&'static [Migration]);

/// All schema migrations.
pub const MIGRATIONS: Migrations = Migrations(&[Migration { sql: V1 }]);

/// Why bringing the schema up to date failed.
#[derive(Debug, thiserror::Error)]
pub enum MigrationError {
    #[error(
        "the database was written by a newer version of the app (schema {found}, this app knows {known})"
    )]
    TooNew { found: i64, known: i64 },
    #[error("schema step {step} failed: {source}")]
    Step {
        step: usize,
        source: rusqlite::Error,
    },
    #[error("schema step {step} left broken foreign keys")]
    ForeignKeys { step: usize },
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
}

impl Migrations {
    /// Number of migrations (the latest `user_version`).
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether there are no migrations.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Applies every migration newer than the database's `user_version`, each in its own
    /// transaction, checking foreign keys before it commits.
    pub fn to_latest(self, conn: &mut Connection) -> Result<(), MigrationError> {
        let known = i64::try_from(self.0.len()).unwrap_or(i64::MAX);
        let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if current > known {
            return Err(MigrationError::TooNew {
                found: current,
                known,
            });
        }
        for (step, m) in self
            .0
            .iter()
            .enumerate()
            .skip(usize::try_from(current).unwrap_or(0))
        {
            let tx = conn.transaction()?;
            tx.execute_batch(m.sql)
                .map_err(|source| MigrationError::Step {
                    step: step + 1,
                    source,
                })?;
            let broken: i64 =
                tx.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })?;
            if broken > 0 {
                return Err(MigrationError::ForeignKeys { step: step + 1 });
            }
            tx.pragma_update(
                None,
                "user_version",
                i64::try_from(step + 1).unwrap_or(i64::MAX),
            )?;
            tx.commit()?;
            tracing::info!(version = step + 1, "database schema updated");
        }
        Ok(())
    }

    /// Applies every migration to a fresh in-memory database (a test helper).
    pub fn validate(self) -> Result<(), MigrationError> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        self.to_latest(&mut conn)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_validate() {
        MIGRATIONS.validate().expect("migrations are valid");
    }

    #[test]
    fn migrations_set_user_version() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        MIGRATIONS.to_latest(&mut conn).expect("migrate");
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("user_version");
        assert_eq!(version, i64::try_from(MIGRATIONS.len()).expect("len"));
        // Running again is a no-op.
        MIGRATIONS.to_latest(&mut conn).expect("migrate again");
    }

    #[test]
    fn newer_database_is_refused() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "user_version", 99)
            .expect("pragma");
        assert!(matches!(
            MIGRATIONS.to_latest(&mut conn),
            Err(MigrationError::TooNew { found: 99, .. })
        ));
    }

    #[test]
    fn deleted_ids_are_never_reused() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        MIGRATIONS.to_latest(&mut conn).expect("migrate");
        let insert_chat = |conn: &rusqlite::Connection| -> i64 {
            conn.execute(
                "INSERT INTO chats (title, created_at, updated_at) VALUES ('t', 0, 0)",
                [],
            )
            .expect("insert chat");
            conn.last_insert_rowid()
        };
        let first = insert_chat(&conn);
        conn.execute("DELETE FROM chats WHERE id = ?1", [first])
            .expect("delete");
        let second = insert_chat(&conn);
        assert!(second > first, "chat id {first} was reused");

        conn.execute(
            "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?1, 'user', '[]', 0)",
            [second],
        )
        .expect("insert message");
        let message = conn.last_insert_rowid();
        conn.execute("DELETE FROM messages WHERE id = ?1", [message])
            .expect("delete message");
        conn.execute(
            "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?1, 'user', '[]', 0)",
            [second],
        )
        .expect("insert message again");
        assert!(conn.last_insert_rowid() > message, "message id reused");
    }

    #[test]
    fn replace_cannot_delete_audit_rows() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "recursive_triggers", "ON")
            .expect("pragma");
        MIGRATIONS.to_latest(&mut conn).expect("migrate");
        let row = "(1, 0, NULL, 's', 't', '{}', 'p', 'read', 'denied', 'user')";
        conn.execute(
            &format!(
                "INSERT INTO audit_log (id, ts, chat_id, server, tool, arguments, preview, risk, \
                 decision, decided_by) VALUES {row}"
            ),
            [],
        )
        .expect("append");
        let replaced = conn.execute(
            "INSERT OR REPLACE INTO audit_log (id, ts, chat_id, server, tool, arguments, preview, \
             risk, decision, decided_by) VALUES (1, 0, NULL, 's', 't', '{}', 'p', 'read', \
             'allow_once', 'user')",
            [],
        );
        assert!(replaced.is_err(), "REPLACE rewrote an audit row");
        let decision: String = conn
            .query_row("SELECT decision FROM audit_log WHERE id = 1", [], |r| {
                r.get(0)
            })
            .expect("row kept");
        assert_eq!(decision, "denied");
    }
}
