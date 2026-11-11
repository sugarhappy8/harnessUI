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
mod artifacts;
mod audit;
mod chats;
mod config;
mod error;
mod ids;
mod models;
mod outputs;
mod schema;
pub mod secrets;
mod sources;
mod tasks;
mod tools;

pub use actor::Store;
pub use artifacts::{
    Artifact, ArtifactAuthor, ArtifactId, ArtifactKind, ArtifactVersion, ArtifactVersionInfo,
    MAX_ARTIFACT_BYTES, MAX_ARTIFACT_TITLE_CHARS, MAX_ARTIFACT_VERSIONS, MAX_ARTIFACTS_PER_CHAT,
    NewArtifact,
};
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
pub use outputs::{
    MAX_LINE_CHARS, MAX_OUTPUT_BYTES, MAX_OUTPUTS_PER_CHAT, NewToolOutput, OutputLine,
    StoredToolOutput, ToolOutputInfo, output_lines,
};
pub use schema::MIGRATIONS;
pub use secrets::{
    ChunkedSecretStore, KeychainSecretStore, MemorySecretStore, SecretError, SecretRef,
    SecretStore, platform_default,
};
pub use sources::{
    EXCERPT_FULL_NOTE, EXCERPT_GAP, MAX_EXCERPT_CHARS, MAX_SNIPPET_CHARS, MAX_SOURCE_TITLE_CHARS,
    MAX_SOURCE_URL_CHARS, MAX_SOURCES_PER_CHAT, NewSource, Source, merge_text,
};
pub use tasks::{
    ChangedFile, FileChange, MAX_CHANGED_FILES, MAX_CHECKS, StepStatus, Task, TaskCheck, TaskNote,
    TaskState, TaskStatus, TaskStep, UserMarks,
};
pub use tools::{
    Attachment, NewAttachment, NewSummary, NewToolCall, Summary, ToolCall, ToolCallFinish,
    ToolCallStatus,
};
