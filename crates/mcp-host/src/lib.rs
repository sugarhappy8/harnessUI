//! MCP host: connections to built-in and external tool servers, the tool registry, config
//! import and per-call cancellation. See ARCHITECTURE.md sections 4.2, 5, 9 and 13.3.
//!
//! - [`Host`] holds one connection per server and one registry of tools. Built-in servers
//!   ([`BuiltinServer`]) are called directly in memory; external servers run over stdio or
//!   Streamable HTTP through rmcp. Above the registry there is no difference.
//! - [`config`] imports `mcpServers` / `servers` JSON, scans it for credentials and
//!   redacts URLs for logs; [`secrets`] resolves `${secret:<name>}` placeholders.
//! - [`schema`] compiles and validates tool schemas offline and simplifies them for
//!   llama-server; [`describe`] is the generic describer for external tools.
//! - [`winquote`] holds the Windows `cmd.exe` quoting rules for `.cmd` shims.
//!
//! Every child process is created through the `runtime` crate's spawn module.

#![warn(missing_docs)]

pub mod builtin;
pub mod config;
mod connection;
pub mod describe;
mod error;
mod host;
mod http;
// rea's own server (item5-spec.md WP9b; the contract stub is WP9a's).
pub mod managed;
pub mod names;
mod result;
pub mod schema;
pub mod secrets;
mod stderr;
mod stdio;
pub mod winquote;

pub use builtin::{
    BuiltinServer, CallContext, ChatContext, Content, Described, ProgressFn, ToolDef, ToolOutput,
};
pub use config::{
    ConfigError, CredentialFinding, CredentialLocation, ServerConfig, ServerEntry, apply_findings,
    exposed_name, parse_config, redact_url, scan_credentials,
};
pub use connection::{Lifecycle, MAX_TOOLS_PER_SERVER};
pub use describe::{ExternalTool, ToolPrefs, describe_external};
pub use error::{HostError, MissingRuntime};
pub use host::{
    DEFAULT_CALL_TIMEOUT, DEFAULT_START_TIMEOUT, Host, HostEvent, HostOptions, KILL_AFTER_CANCEL,
    MAX_DESCRIPTION_CHARS, RegisteredTool, ServerKind, ServerState, ServerStatus,
};
pub use http::{CONNECT_TIMEOUT, MAX_SSE_EVENT_BYTES};
pub use managed::{ManagedServer, ManagedSpec, ToolAllowList};
pub use result::MAX_RESULT_BYTES;
pub use schema::{CompiledSchema, SchemaError, canonical_json, simplify_for_llama};
pub use secrets::SecretResolver;
pub use stderr::{MAX_LINE_BYTES, MAX_LOG_FILE_BYTES, TAIL_LINES};
pub use stdio::{alternative_to_stub, developer_tools_installed, is_developer_tools_stub};
