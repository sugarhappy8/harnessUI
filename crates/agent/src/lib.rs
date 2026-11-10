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
//! The agent loop, tool-call parsing and repair, and context management arrive in M3
//! (ARCHITECTURE.md 8).

#![warn(missing_docs)]

pub mod client;
pub mod error;
pub mod estimate;
pub mod sse;
pub mod stream;
pub mod types;

pub use client::{ChatStream, ClientConfig, ModelInfo, OpenAiClient, ProviderKind};
pub use error::ProviderError;
pub use estimate::{TokenCalibrator, estimate_tokens};
pub use sse::{SseDecoder, SseEvent};
pub use stream::{
    AccumulatedToolCall, ArgumentsStatus, ChunkParser, FinishReason, StreamDecoder, StreamEvent,
    Timings, ToolCallAccumulator, Usage,
};
pub use types::{
    ChatMessage, ChatRequest, ContentPart, FunctionCall, FunctionSpec, ImageUrl, MessageContent,
    Role, ToolCall, ToolChoice, ToolSpec,
};
