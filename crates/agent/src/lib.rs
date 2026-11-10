//! Model providers for the app (ARCHITECTURE.md 5 and 7.5).
//!
//! In M1 this crate is the provider layer only:
//!
//! - [`types`]: chat-completion request and message types, ready for tool calls, tool results
//!   and images;
//! - [`sse`]: a transport-agnostic incremental server-sent-events decoder;
//! - [`stream`]: the tolerant mapping from chunks to [`StreamEvent`]s, and the
//!   [`ToolCallAccumulator`] that folds tool-call events into calls;
//! - [`client`]: [`OpenAiClient`], a streaming client for llama-server and other
//!   OpenAI-compatible servers (Ollama, LM Studio, vLLM), with context-length discovery and
//!   llama-server's exact token counter;
//! - [`estimate`]: character-based token estimates for servers that cannot count.
//!
//! On top of it, the agent loop (ARCHITECTURE.md 8):
//!
//! - [`history`]: the stored message model ([`HistoryMessage`]) and the history invariant
//!   ([`normalize`]);
//! - [`parse`]: the fallback tool-call parser for calls written as text, and schema-guided
//!   argument repair;
//! - [`prompt`]: the single system message, delimited tool results, the projection of history
//!   onto the current model, and the one [`Sanitizer`] for untrusted text;
//! - [`context`]: the context budget, compaction and truncation;
//! - [`turn`]: [`run_turn`], one agent turn from the user's message to the final answer.

#![warn(missing_docs)]

pub mod client;
pub mod context;
pub mod error;
pub mod estimate;
pub mod history;
pub mod parse;
pub mod prompt;
pub mod sse;
pub mod stream;
pub mod turn;
pub mod types;

pub use client::{ChatStream, ClientConfig, ModelInfo, OpenAiClient, ProviderKind, Token};
pub use context::{ContextBudget, ToolsDoNotFit, truncate_head_tail};
pub use error::ProviderError;
pub use estimate::{TokenCalibrator, estimate_tokens};
pub use history::{CallRecord, CallStatus, HistoryMessage, Part, ResultRecord, normalize};
pub use parse::{
    Recovered, Recovery, RecoveryContext, ToolInfo, recover_tool_calls, repair_arguments,
};
pub use prompt::{LoadedSkill, Preamble, Sanitizer};
pub use sse::{SseDecoder, SseEvent};
pub use stream::{
    AccumulatedToolCall, ArgumentsStatus, ChunkParser, FinishReason, StreamDecoder, StreamEvent,
    Timings, ToolCallAccumulator, Usage,
};
pub use turn::{
    Approval, ApprovalRequest, Approver, CallDecision, CompactionSummary, ModelCaps, PolicyGate,
    RequestTemplate, ToolHost, TurnDeps, TurnEvent, TurnInput, TurnOutcome, TurnSink, TurnStatus,
    UsageTotals, run_turn,
};
pub use types::{
    ChatMessage, ChatRequest, ContentPart, FunctionCall, FunctionSpec, ImageUrl, MessageContent,
    Role, ToolCall, ToolChoice, ToolSpec,
};
