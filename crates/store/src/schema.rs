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

/// Version 2 (app 0.0.4): artifacts, sources, task state, pinned instructions and stored
/// tool output. Not shipped until 0.0.4 is released; until then each block may still be
/// changed by the feature that owns it (the name in brackets), and only by that feature.
const V2: &str = r#"
-- [artifacts] Documents, pages, diagrams and small tools the model made, shown beside the chat.
CREATE TABLE artifacts (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id     INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    title       TEXT    NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('markdown', 'html', 'svg', 'mermaid')),
    created_at  INTEGER NOT NULL,
    updated_at  INTEGER NOT NULL
);
CREATE INDEX artifacts_chat ON artifacts(chat_id, updated_at DESC, id DESC);

CREATE TABLE artifact_versions (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    artifact_id  INTEGER NOT NULL REFERENCES artifacts(id) ON DELETE CASCADE,
    version      INTEGER NOT NULL CHECK (version >= 1),
    content      TEXT    NOT NULL,
    author       TEXT    NOT NULL CHECK (author IN ('model', 'user')),
    note         TEXT,
    created_at   INTEGER NOT NULL,
    -- 1 when content from outside the conversation (files, web pages, tool results) had
    -- entered the chat before this version was written, or when the version it was made
    -- from has the mark. A page with the mark runs no script until the user allows it.
    after_outside INTEGER NOT NULL DEFAULT 0 CHECK (after_outside IN (0, 1)),
    UNIQUE (artifact_id, version)
);
-- [/artifacts]

-- [sources] What a web search retrieved, numbered per chat so an answer can cite [n].
CREATE TABLE sources (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id         INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    n               INTEGER NOT NULL CHECK (n >= 1),
    url             TEXT    NOT NULL,
    title           TEXT    NOT NULL,
    site            TEXT    NOT NULL,
    snippet         TEXT    NOT NULL DEFAULT '',
    excerpt         TEXT    NOT NULL DEFAULT '',
    content_sha256  TEXT,
    official        INTEGER NOT NULL DEFAULT 0 CHECK (official IN (0, 1)),
    version_note    TEXT,
    query           TEXT    NOT NULL DEFAULT '',
    fetched_at      INTEGER,
    created_at      INTEGER NOT NULL,
    UNIQUE (chat_id, n)
);
-- An address is one source in a chat: found again, it keeps its number.
CREATE UNIQUE INDEX sources_chat_url ON sources(chat_id, url);
-- [/sources]

-- [task] One task record per chat: goal, constraints, steps, changed files, checks, next action.
CREATE TABLE tasks (
    chat_id     INTEGER PRIMARY KEY REFERENCES chats(id) ON DELETE CASCADE,
    state       TEXT    NOT NULL CHECK (json_valid(state)),
    status      TEXT    NOT NULL CHECK (status IN ('active', 'paused', 'done')),
    updated_at  INTEGER NOT NULL
);
-- [/task]

-- [pinned] Instructions the user pinned. chat_id NULL: every chat. The text is plain text,
-- 1 to 1,000 characters (pinned.rs checks it first and says why; this is the backstop).
CREATE TABLE pinned_instructions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id     INTEGER REFERENCES chats(id) ON DELETE CASCADE,
    text        TEXT    NOT NULL CHECK (length(text) BETWEEN 1 AND 1000),
    position    INTEGER NOT NULL DEFAULT 0,
    created_at  INTEGER NOT NULL
);
CREATE INDEX pinned_instructions_chat ON pinned_instructions(chat_id, position, id);
-- [/pinned]

-- [outputs] The full text of tool results the model saw shortened, for read_output.
CREATE TABLE tool_outputs (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    chat_id     INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE,
    call_id     TEXT    NOT NULL,
    tool        TEXT    NOT NULL,
    content     TEXT    NOT NULL,
    bytes       INTEGER NOT NULL CHECK (bytes >= 0),
    lines       INTEGER NOT NULL CHECK (lines >= 0),
    created_at  INTEGER NOT NULL
);
CREATE INDEX tool_outputs_chat ON tool_outputs(chat_id, id);
-- [/outputs]

-- [allow all] The audit log learns one more decision, 'allowed_by_mode': a call that the
-- chat's Allow all mode let through without a card. SQLite cannot change a CHECK in place,
-- so the table is rebuilt: every row is copied with its id, then the indexes and the
-- append-only triggers are created again exactly as in version 1. Dropping a table fires no
-- triggers, and renaming the new table carries its own parent reference along.
CREATE TABLE audit_log_v2 (
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
                         'allowed_by_mode', 'denied', 'cancelled', 'refused_by_policy')),
    decided_by       TEXT    NOT NULL,
    note             TEXT,
    result_status    TEXT,
    result_summary   TEXT,
    duration_ms      INTEGER,
    parent_audit_id  INTEGER REFERENCES audit_log_v2(id)
);
INSERT INTO audit_log_v2 (id, ts, chat_id, server, tool, arguments, preview, risk, decision,
    decided_by, note, result_status, result_summary, duration_ms, parent_audit_id)
SELECT id, ts, chat_id, server, tool, arguments, preview, risk, decision,
    decided_by, note, result_status, result_summary, duration_ms, parent_audit_id
FROM audit_log ORDER BY id;
DROP TABLE audit_log;
ALTER TABLE audit_log_v2 RENAME TO audit_log;
CREATE INDEX audit_log_ts ON audit_log(ts, id);
CREATE INDEX audit_log_chat ON audit_log(chat_id, ts);
CREATE INDEX audit_log_parent ON audit_log(parent_audit_id);

CREATE TRIGGER audit_log_no_delete BEFORE DELETE ON audit_log BEGIN
    SELECT RAISE(ABORT, 'audit_log is append-only');
END;
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
-- [/allow all]
"#;

/// Version 3 (app 0.1.0): Studio. The pictures and videos made in Studio, by a company's
/// computers through an endpoint or by the Studio engine on this computer, the files they
/// made, the picture and video models installed for the engine, and the model licences the
/// user accepted. Not shipped until 0.1.0 is released.
const V3: &str = r#"
-- One press of Create. A run whose endpoint is deleted keeps the endpoint's name and host;
-- a run on this computer has no endpoint (runs_on keeps the two apart). A run deleted on the
-- day it was made stays as a spending stub (deleted_at set; prompt, settings and added
-- pictures cleared) until the next day, so deleting a result never lowers today's spending.
CREATE TABLE creation_runs (
    id             TEXT    PRIMARY KEY,
    kind           TEXT    NOT NULL CHECK (kind IN ('image', 'video')),
    runs_on        TEXT    NOT NULL DEFAULT 'cloud' CHECK (runs_on IN ('cloud', 'local')),
    provider_id    INTEGER REFERENCES providers(id) ON DELETE SET NULL,
    provider_name  TEXT    NOT NULL,
    provider_host  TEXT    NOT NULL,
    model          TEXT    NOT NULL,
    model_label    TEXT    NOT NULL,
    prompt         TEXT    NOT NULL,
    -- Exactly what was sent, never a key.
    params_json    TEXT    NOT NULL CHECK (json_valid(params_json)),
    -- The added pictures: sha256 and role of each.
    refs_json      TEXT    NOT NULL DEFAULT '[]' CHECK (json_valid(refs_json)),
    status         TEXT    NOT NULL CHECK (status IN
                       ('queued', 'sending', 'waiting', 'running', 'downloading',
                        'done', 'failed', 'blocked', 'cancelled', 'interrupted')),
    remote_id      TEXT,
    progress       REAL,
    estimate_usd   REAL,
    cost_usd       REAL,
    -- The error and the company's code, and whether the run was charged.
    error_json     TEXT    CHECK (error_json IS NULL OR json_valid(error_json)),
    -- The run's audit row, completed when the run ends (also after a restart).
    audit_id       INTEGER,
    -- The company OpenRouter ran it at, when known.
    upstream       TEXT,
    -- A run on this computer: the installed model, the engine's build number, the device
    -- it ran on and how long it took.
    gen_model_id   TEXT,
    engine_build   INTEGER,
    device         TEXT,
    elapsed_ms     INTEGER,
    created_at     INTEGER NOT NULL,
    started_at     INTEGER,
    finished_at    INTEGER,
    deleted_at     INTEGER
);
CREATE INDEX creation_runs_created ON creation_runs(created_at);
CREATE INDEX creation_runs_live ON creation_runs(status)
    WHERE status IN ('queued', 'sending', 'waiting', 'running', 'downloading');

-- One file a run made, in the gallery folder.
CREATE TABLE creation_outputs (
    id           TEXT    PRIMARY KEY,
    run_id       TEXT    NOT NULL REFERENCES creation_runs(id) ON DELETE CASCADE,
    idx          INTEGER NOT NULL,
    path         TEXT    NOT NULL,
    thumb_path   TEXT,
    mime         TEXT    NOT NULL,
    size_bytes   INTEGER NOT NULL CHECK (size_bytes >= 0),
    width        INTEGER,
    height       INTEGER,
    duration_ms  INTEGER,
    seed         INTEGER,
    starred      INTEGER NOT NULL DEFAULT 0 CHECK (starred IN (0, 1)),
    created_at   INTEGER NOT NULL
);
CREATE INDEX creation_outputs_run ON creation_outputs(run_id, idx);

-- Picture and video models installed for the Studio engine. Never in `models`, which feeds
-- the chat picker.
CREATE TABLE gen_models (
    -- The catalog id, or "hf:<owner>/<repo>:<file>" for a searched model.
    id          TEXT    PRIMARY KEY,
    catalog_id  TEXT,
    name        TEXT    NOT NULL,
    kind        TEXT    NOT NULL CHECK (kind IN ('image', 'image-edit', 'video')),
    family      TEXT    NOT NULL,
    -- [{role, repo, revision, path, file, size, sha256, license}]
    parts_json  TEXT    NOT NULL CHECK (json_valid(parts_json)),
    size_bytes  INTEGER NOT NULL CHECK (size_bytes >= 0),
    source      TEXT    NOT NULL CHECK (source IN ('catalog', 'search')),
    added_at    INTEGER NOT NULL
);

-- Model licences the user accepted before a download: one row per licence text (its
-- SHA-256). catalog_id records which model it was accepted for (NULL: a searched model or
-- the engine).
CREATE TABLE license_acceptances (
    sha256       TEXT    PRIMARY KEY,
    license_id   TEXT    NOT NULL,
    catalog_id   TEXT,
    accepted_at  INTEGER NOT NULL
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
pub const MIGRATIONS: Migrations = Migrations(&[
    Migration { sql: V1 },
    Migration { sql: V2 },
    Migration { sql: V3 },
]);

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

    /// [allow all] Version 2 rebuilds the audit log so it accepts 'allowed_by_mode'. A
    /// database written by version 1 keeps every row with its id, its result and its
    /// link to a parent row; the log stays append-only; unknown decisions are still
    /// refused.
    #[test]
    fn the_audit_log_of_a_version_1_database_survives_the_rebuild() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("pragma");
        conn.pragma_update(None, "recursive_triggers", "ON")
            .expect("pragma");
        // A database as version 1 left it, with an audit trail.
        Migrations(&[Migration { sql: V1 }])
            .to_latest(&mut conn)
            .expect("version 1");
        let insert = "INSERT INTO audit_log (id, ts, chat_id, server, tool, arguments, preview, \
                      risk, decision, decided_by, note, result_status, result_summary, \
                      duration_ms, parent_audit_id) VALUES ";
        for row in [
            "(1, 100, 7, 'files', 'write_file', '{\"path\":\"a\"}', 'p1', 'normal', 'allow_once', \
             'user', NULL, 'ok', 'Wrote a', 12, NULL)",
            "(2, 200, 7, 'browser', 'browser_click', '{}', 'p2', 'alwaysask', 'denied', 'user', \
             'not this one', 'denied', NULL, NULL, 1)",
            "(5, 300, NULL, 'files', 'read_file', '{}', 'p5', 'normal', 'allowed_by_grant', \
             'policy', NULL, NULL, NULL, NULL, NULL)",
        ] {
            conn.execute(&format!("{insert}{row}"), [])
                .expect("a version 1 row");
        }
        // Version 1 does not know the new decision.
        assert!(
            conn.execute(
                &format!(
                    "{insert}(9, 1, 7, 's', 't', '{{}}', 'p', 'normal', 'allowed_by_mode', \
                     'policy', NULL, NULL, NULL, NULL, NULL)"
                ),
                [],
            )
            .is_err()
        );

        MIGRATIONS.to_latest(&mut conn).expect("version 2");

        // id, ts, chat, decision, preview, result status, parent.
        type Kept = (
            i64,
            i64,
            Option<i64>,
            String,
            String,
            Option<String>,
            Option<i64>,
        );
        let rows: Vec<Kept> = conn
            .prepare(
                "SELECT id, ts, chat_id, decision, preview, result_status, parent_audit_id \
                 FROM audit_log ORDER BY id",
            )
            .expect("prepare")
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        assert_eq!(
            rows,
            vec![
                (
                    1,
                    100,
                    Some(7),
                    "allow_once".into(),
                    "p1".into(),
                    Some("ok".into()),
                    None
                ),
                (
                    2,
                    200,
                    Some(7),
                    "denied".into(),
                    "p2".into(),
                    Some("denied".into()),
                    Some(1)
                ),
                (
                    5,
                    300,
                    None,
                    "allowed_by_grant".into(),
                    "p5".into(),
                    None,
                    None
                ),
            ]
        );
        let (note, summary, duration): (Option<String>, Option<String>, Option<i64>) = conn
            .query_row(
                "SELECT (SELECT note FROM audit_log WHERE id = 2), result_summary, duration_ms \
                 FROM audit_log WHERE id = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("columns");
        assert_eq!(
            (note.as_deref(), summary.as_deref(), duration),
            (Some("not this one"), Some("Wrote a"), Some(12))
        );

        // The new decision is accepted, with the next free id; an unknown one is not.
        conn.execute(
            "INSERT INTO audit_log (ts, chat_id, server, tool, arguments, preview, risk, \
             decision, decided_by, parent_audit_id) VALUES (400, 7, 'files', 'write_file', '{}', \
             'p', 'normal', 'allowed_by_mode', 'policy', 5)",
            [],
        )
        .expect("the new decision");
        assert_eq!(conn.last_insert_rowid(), 6);
        assert!(
            conn.execute(
                "INSERT INTO audit_log (ts, server, tool, arguments, preview, risk, decision, \
                 decided_by) VALUES (0, 's', 't', '{}', 'p', 'normal', 'allowed_anyway', 'x')",
                [],
            )
            .is_err(),
            "an unknown decision was stored"
        );

        // Still append-only: no delete, no rewrite, no replace; a result is filled in once.
        assert!(
            conn.execute("DELETE FROM audit_log WHERE id = 1", [])
                .is_err()
        );
        assert!(
            conn.execute(
                "UPDATE audit_log SET decision = 'allowed_by_mode' WHERE id = 2",
                []
            )
            .is_err()
        );
        assert!(
            conn.execute(
                "INSERT OR REPLACE INTO audit_log (id, ts, server, tool, arguments, preview, risk, \
                 decision, decided_by) VALUES (1, 0, 's', 't', '{}', 'p', 'r', 'denied', 'user')",
                [],
            )
            .is_err()
        );
        conn.execute("UPDATE audit_log SET result_status = 'ok' WHERE id = 5", [])
            .expect("the one permitted update");
        assert!(
            conn.execute(
                "UPDATE audit_log SET result_status = 'error' WHERE id = 5",
                []
            )
            .is_err()
        );

        // The parent link points at the rebuilt table, and is enforced.
        assert!(
            conn.execute(
                "INSERT INTO audit_log (ts, server, tool, arguments, preview, risk, decision, \
                 decided_by, parent_audit_id) VALUES (0, 's', 't', '{}', 'p', 'r', 'denied', \
                 'user', 4242)",
                [],
            )
            .is_err(),
            "a row may not point at a parent that does not exist"
        );
        let broken: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .expect("check");
        assert_eq!(broken, 0);
        // Nothing of the rebuild is left behind, and the indexes and triggers are back.
        let names: Vec<String> = conn
            .prepare(
                "SELECT name FROM sqlite_master WHERE tbl_name LIKE 'audit_log%' ORDER BY name",
            )
            .expect("prepare")
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("names");
        assert_eq!(
            names,
            vec![
                "audit_log",
                "audit_log_chat",
                "audit_log_no_delete",
                "audit_log_no_rewrite",
                "audit_log_parent",
                "audit_log_ts",
            ]
        );
        let sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'audit_log'",
                [],
                |r| r.get(0),
            )
            .expect("sql");
        assert!(!sql.contains("audit_log_v2"), "{sql}");
    }

    fn table_exists(conn: &rusqlite::Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [name],
            |r| r.get::<_, i64>(0),
        )
        .expect("sqlite_master")
            == 1
    }

    /// [studio] Version 3 adds Studio's tables to a database version 2 left, and leaves
    /// the rows already there as they were.
    #[test]
    fn version_3_adds_the_studio_tables_to_a_version_2_database() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("pragma");
        Migrations(&[Migration { sql: V1 }, Migration { sql: V2 }])
            .to_latest(&mut conn)
            .expect("version 2");
        conn.execute(
            "INSERT INTO providers (name, kind, base_url, secret_ref, enabled, created_at) \
             VALUES ('OpenRouter', 'openrouter', 'https://openrouter.ai/api/v1', \
             'provider-api-key:1', 1, 5)",
            [],
        )
        .expect("provider");
        conn.execute(
            "INSERT INTO chats (title, model_ref, created_at, updated_at) \
             VALUES ('kept', 'local:a.gguf', 1, 2)",
            [],
        )
        .expect("chat");
        type Rows = (
            (i64, String, String, String, Option<String>, i64, i64),
            (i64, String, Option<String>, i64, i64),
        );
        let rows = |conn: &rusqlite::Connection| -> Rows {
            let provider = conn
                .query_row(
                    "SELECT id, name, kind, base_url, secret_ref, enabled, created_at \
                     FROM providers",
                    [],
                    |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                            r.get(6)?,
                        ))
                    },
                )
                .expect("provider row");
            let chat = conn
                .query_row(
                    "SELECT id, title, model_ref, created_at, updated_at FROM chats",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
                )
                .expect("chat row");
            (provider, chat)
        };
        let before = rows(&conn);
        assert!(!table_exists(&conn, "creation_runs"));

        MIGRATIONS.to_latest(&mut conn).expect("version 3");

        let version: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("user_version");
        assert_eq!(version, 3);
        assert!(table_exists(&conn, "creation_runs"));
        assert!(table_exists(&conn, "creation_outputs"));
        assert_eq!(rows(&conn), before);
    }

    /// [studio] The local addendum's part of version 3: where a run ran, the engine facts,
    /// installed picture and video models, and licence acceptances.
    #[test]
    fn version_3_has_the_local_columns_and_tables() {
        let mut conn = rusqlite::Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON")
            .expect("pragma");
        MIGRATIONS.to_latest(&mut conn).expect("migrate");

        conn.execute(
            "INSERT INTO creation_runs (id, kind, provider_name, provider_host, model, \
             model_label, prompt, params_json, status, created_at) \
             VALUES ('r', 'image', 'OpenRouter', 'openrouter.ai', 'm', 'M', 'p', '{}', \
             'queued', 0)",
            [],
        )
        .expect("a run without runs_on");
        let runs_on: String = conn
            .query_row(
                "SELECT runs_on FROM creation_runs WHERE id = 'r'",
                [],
                |r| r.get(0),
            )
            .expect("runs_on");
        assert_eq!(runs_on, "cloud");
        assert!(
            conn.execute(
                "UPDATE creation_runs SET runs_on = 'gpu' WHERE id = 'r'",
                []
            )
            .is_err(),
            "runs_on is cloud or local"
        );
        conn.execute(
            "UPDATE creation_runs SET runs_on = 'local', status = 'running', \
             gen_model_id = 'flux2-klein-4b-q4', engine_build = 1106, device = 'vulkan0', \
             elapsed_ms = 228000, upstream = NULL, deleted_at = NULL WHERE id = 'r'",
            [],
        )
        .expect("the local columns and the running status");

        assert!(table_exists(&conn, "gen_models"));
        assert!(table_exists(&conn, "license_acceptances"));
        let gen_model = |kind: &str| {
            conn.execute(
                "INSERT INTO gen_models (id, catalog_id, name, kind, family, parts_json, \
                 size_bytes, source, added_at) \
                 VALUES (?1, 'flux2-klein-4b-q4', 'FLUX.2 klein', ?2, 'flux2-klein-4b', '[]', 0, \
                 'catalog', 0)",
                [format!("id-{kind}"), kind.to_owned()],
            )
        };
        assert!(gen_model("audio").is_err(), "audio is not a kind");
        gen_model("image").expect("a picture model");
        gen_model("image-edit").expect("a picture-editing model");
        gen_model("video").expect("a video model");

        // One row per licence text, whichever model accepted it (and none for searched
        // models, whose catalog id is NULL).
        let accept = |catalog: Option<&str>| {
            conn.execute(
                "INSERT OR IGNORE INTO license_acceptances (sha256, license_id, catalog_id, \
                 accepted_at) VALUES ('ab', 'apache-2.0', ?1, 1)",
                [catalog],
            )
            .expect("accept")
        };
        assert_eq!(accept(None), 1);
        assert_eq!(accept(None), 0, "accepted again: still one row");
        assert_eq!(accept(Some("z-image-turbo")), 0);
        let held: i64 = conn
            .query_row("SELECT count(*) FROM license_acceptances", [], |r| r.get(0))
            .expect("count");
        assert_eq!(held, 1);
    }
}
