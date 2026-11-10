//! Built-in tool servers. See ARCHITECTURE.md sections 5 and 9.3.
//!
//! Each server implements `mcp_host::BuiltinServer`: `describe` turns arguments into a
//! policy action with a preview (no side effects) and `call` runs the approved action,
//! re-checking it immediately before any side effect.

pub mod files;
pub mod git;
pub mod pdf;
pub mod skills;
pub mod terminal;

pub use files::FilesServer;
pub use git::{GitConfig, GitServer};
pub use skills::{Skill, SkillsServer};
pub use terminal::{TerminalConfig, TerminalServer, TerminalShell};
