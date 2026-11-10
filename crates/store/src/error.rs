//! Error types for the database side of the crate.

use crate::ids::AuditId;

/// Everything that can go wrong in a [`Store`](crate::Store) call.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// Bringing the schema up to date failed (or the database was written by a newer app).
    #[error("database migration failed: {0}")]
    Migration(Box<crate::schema::MigrationError>),

    /// Creating the database folder or the database thread failed.
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A value could not be converted to or from JSON.
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    /// The row the call refers to does not exist.
    #[error("{what} {id} not found")]
    NotFound {
        /// Which kind of row, e.g. "chat" or "message".
        what: &'static str,
        /// The id that was looked up.
        id: i64,
    },

    /// The caller passed a value the store refuses (for example a parent message from
    /// another chat, or `tool_prefs` that are not a JSON object).
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// The write would break a uniqueness rule (for example a duplicate MCP server name).
    #[error("conflict: {0}")]
    Conflict(String),

    /// [`Store::complete_audit`](crate::Store::complete_audit) was called on a row that
    /// already has a result. The audit log is append-only.
    #[error("audit row {0} already has a result; the audit log is append-only")]
    AuditAlreadyCompleted(AuditId),

    /// A stored value does not have the shape this version of the app expects.
    #[error("stored data is invalid: {0}")]
    Corrupt(String),

    /// The database thread has stopped, so the call could not be delivered or answered.
    #[error("the database thread has stopped")]
    Closed,

    /// The query panicked on the database thread. The thread is still running.
    #[error("the database thread panicked while running a query")]
    Panicked,
}

impl From<crate::schema::MigrationError> for StoreError {
    fn from(value: crate::schema::MigrationError) -> Self {
        Self::Migration(Box::new(value))
    }
}

impl StoreError {
    /// Shorthand for [`StoreError::NotFound`].
    pub(crate) fn not_found(what: &'static str, id: i64) -> Self {
        Self::NotFound { what, id }
    }

    /// True when the error is SQLite reporting a `UNIQUE` constraint failure.
    pub(crate) fn is_unique_violation(err: &rusqlite::Error) -> bool {
        matches!(
            err,
            rusqlite::Error::SqliteFailure(e, _)
                if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    || e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
        )
    }
}

/// A string stored in an enum column did not match any known variant.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown {kind} value {value:?}")]
pub struct ParseEnumError {
    /// The enum's name, e.g. "role".
    pub kind: &'static str,
    /// The text that failed to parse.
    pub value: String,
}

impl From<ParseEnumError> for StoreError {
    fn from(value: ParseEnumError) -> Self {
        Self::Corrupt(value.to_string())
    }
}
