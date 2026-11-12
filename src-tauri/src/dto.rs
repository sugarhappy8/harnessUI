//! Data sent to the webview. Field names are camelCase and match src/lib/types.ts.

use serde::{Deserialize, Serialize};

use store::{ChatId, MessageId, ProviderId};

use crate::error::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatSummaryDto {
    pub id: ChatId,
    pub title: String,
    pub updated_at: i64,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDto {
    pub id: ChatId,
    pub title: String,
    pub model_ref: Option<String>,
    pub system_prompt: Option<String>,
    pub leaf_message_id: Option<MessageId>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Thinking is on for this chat (when the model can think).
    pub thinking: bool,
    /// The folder the user chose for relative paths; `None` means the first approved
    /// folder. A turn uses it only while it is still approved for the chat.
    pub working_folder: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageDto {
    pub id: MessageId,
    pub parent_id: Option<MessageId>,
    pub role: String,
    pub text: String,
    pub reasoning: Option<String>,
    pub status: String,
    pub model_ref: Option<String>,
    pub prompt_tokens: Option<u64>,
    pub completion_tokens: Option<u64>,
    pub tokens_per_second: Option<f64>,
    pub error: Option<AppError>,
    pub created_at: i64,
    pub sibling_ids: Vec<MessageId>,
    /// Attached images, shown through `get_attachment`.
    pub image_ids: Vec<i64>,
    /// Attached text and PDF files.
    pub documents: Vec<DocumentRef>,
    /// Tool calls the model made in this message.
    pub tool_calls: Vec<ToolCallDto>,
    /// For a tool message: the result.
    pub tool_result: Option<ToolResultDto>,
    /// For a user message: the files and definitions it mentioned (shown as chips).
    #[serde(default)]
    pub mentions: Vec<crate::mentions::MentionRef>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallDto {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
    pub recovered: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultDto {
    pub call_id: String,
    pub name: String,
    pub status: String,
    pub text: String,
    pub image_count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentRef {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatDetailDto {
    pub chat: ChatDto,
    pub messages: Vec<MessageDto>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum RuntimeStateDto {
    Stopped,
    Starting,
    Ready,
    Stopping,
    Failed,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeStatusDto {
    pub state: RuntimeStateDto,
    pub backend: Option<String>,
    pub device: Option<String>,
    pub model_path: Option<String>,
    pub model_name: Option<String>,
    pub progress: Option<f32>,
    pub stage: Option<String>,
    pub n_ctx: Option<u32>,
    pub supports_tools: bool,
    /// The template accepts reasoning in earlier assistant messages.
    pub preserve_reasoning: bool,
    /// The model's chat template can think (from its GGUF header).
    pub thinking: bool,
    /// Tokens/s of the short generation timed after the load.
    pub speed_tps: Option<f64>,
    /// A GPU backend generated far below expectation ("loaded but slow", 7.4).
    pub slow: bool,
    pub vision: bool,
    pub offloaded_layers: Option<u32>,
    pub total_layers: Option<u32>,
    pub error: Option<AppError>,
}

impl Default for RuntimeStatusDto {
    fn default() -> Self {
        Self {
            state: RuntimeStateDto::Stopped,
            backend: None,
            device: None,
            model_path: None,
            model_name: None,
            progress: None,
            stage: None,
            n_ctx: None,
            supports_tools: false,
            preserve_reasoning: false,
            thinking: false,
            speed_tps: None,
            slow: false,
            vision: false,
            offloaded_layers: None,
            total_layers: None,
            error: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDto {
    pub name: String,
    pub description: String,
    pub total_mib: u64,
    pub free_mib: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackendDto {
    pub kind: String,
    pub source: String,
    pub build: String,
    pub available: bool,
    pub devices: Vec<DeviceDto>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelSettings {
    pub ctx: u32,
    /// None means Auto.
    pub backend: Option<String>,
    pub device: Option<String>,
    /// Image projector to load with the model. Set per load from the installed-model
    /// record, never stored in the global settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
}

impl Default for LocalModelSettings {
    fn default() -> Self {
        Self {
            ctx: 8192,
            backend: None,
            device: None,
            mmproj: None,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDto {
    pub id: ProviderId,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub has_api_key: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveProvider {
    pub id: Option<ProviderId>,
    pub name: String,
    pub kind: String,
    pub base_url: String,
    pub api_key: Option<String>,
    pub clear_api_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOptionDto {
    #[serde(rename = "ref")]
    pub reference: String,
    pub label: String,
    pub source: String,
    pub context_length: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReportDto {
    pub text: String,
    pub saved_to: Option<String>,
}
