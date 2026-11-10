//! Local storage for the app (ARCHITECTURE.md section 12).
//!
//! - [`Store`]: the SQLite database (chats and the message tree, history search, tool calls,
//!   compaction summaries, attachments, the append-only audit log, models, downloads and
//!   configuration). One dedicated OS thread owns the connection; every method is async
//!   and talks to that thread over a channel, so no async task blocks on disk.
//! - [`secrets`]: OS keychain storage for API keys and tokens. SQLite holds only
//!   [`SecretRef`]s.
//!
//! The database lives in the app's *local* data folder (never the roaming one) in WAL mode.

#![warn(missing_docs)]

mod actor;
mod audit;
mod chats;
mod config;
mod error;
mod ids;
mod models;
mod schema;
pub mod secrets;
mod tools;

pub use actor::Store;
pub use audit::{
    AUDIT_CSV_COLUMNS, AuditDecision, AuditEntry, AuditQuery, AuditResult, AuditRow,
    audit_rows_to_csv, audit_rows_to_jsonl,
};
pub use chats::{
    Chat, ChatPatch, ChatSummary, ListChats, Message, MessagePatch, MessageStatus, NewChat,
    NewMessage, Role,
};
pub use config::{
    ApprovedFolder, McpServer, McpServerInput, McpToolPref, Provider, ProviderInput, Skill,
    SkillInput,
};
pub use error::{ParseEnumError, StoreError};
pub use ids::{
    AttachmentId, AuditId, ChatId, DownloadId, FolderId, McpServerId, MessageId, ModelId,
    ProviderId, SkillId, SummaryId, ToolCallId,
};
pub use models::{
    Download, DownloadPatch, DownloadStatus, ModelInput, ModelRecord, ModelSource, NewDownload,
};
pub use schema::MIGRATIONS;
pub use secrets::{
    ChunkedSecretStore, KeychainSecretStore, MemorySecretStore, SecretError, SecretRef,
    SecretStore, platform_default,
};
pub use tools::{
    Attachment, NewAttachment, NewSummary, NewToolCall, Summary, ToolCall, ToolCallFinish,
    ToolCallStatus,
};
