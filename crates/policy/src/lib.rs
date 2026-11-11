//! Policy: the path sandbox, the permission rules and safe card text.
//! See ARCHITECTURE.md sections 13.3–13.5.
//!
//! Nothing here changes policy state. Grants and approved folders are inputs; they are
//! changed only by commands the UI sends.

pub mod allow_all;
pub mod command;
pub mod engine;
pub mod files;
pub mod path;
pub mod sanitize;

pub use engine::{
    Action, AskReason, CardKind, Decision, Grants, KeepAsking, Risk, Scope, TurnContext, evaluate,
    external_risk,
};
pub use files::{WriteKind, always_ask_reason};
pub use path::{CaseRule, CheckedPath, Flavor, Fs, LexicalPath, PathError, PathSandbox, RealFs};
