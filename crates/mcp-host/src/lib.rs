//! MCP host: connections to built-in and external tool servers, the tool registry and
//! per-call cancellation. See ARCHITECTURE.md sections 5 and 9.

pub mod builtin;

pub use builtin::{
    BuiltinServer, CallContext, ChatContext, Content, Described, ProgressFn, ToolDef, ToolOutput,
};
