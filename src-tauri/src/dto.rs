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

/// Events streamed to the webview while a reply is generated.
#[derive(Debug, Clone, Serialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ChatStreamEvent {
    Started {
        user_message_id: Option<MessageId>,
        assistant_message_id: MessageId,
    },
    Content {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Usage {
        prompt_tokens: u32,
        completion_tokens: u32,
    },
    Timings {
        predicted_per_second: Option<f64>,
        prompt_per_second: Option<f64>,
    },
    Finished {
        status: String,
        finish_reason: Option<String>,
    },
    Error {
        error: AppError,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticsReportDto {
    pub text: String,
    pub saved_to: Option<String>,
}
