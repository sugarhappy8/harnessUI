//! Data sent to the webview. Field names are camelCase and match src/lib/types.ts.

use serde::{Deserialize, Serialize};

use store::{ChatId, MessageId, ProviderId};

use crate::error::AppError;
use crate::load_settings::{ContextSource, GpuUse, ThinkingLimit};

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
    /// 0.0.5: where the load is, as a typed value (the plain words are in
    /// `src/lib/words.ts`). `stage` above keeps the old text until the webview switches.
    pub load_stage: Option<runtime::LoadStage>,
    /// 0.0.5: what the current load had to change to start (step-downs and retries), in
    /// order. Empty when it loaded as asked. Kept after Ready, for the slow-state lines.
    pub adjustments: Vec<runtime::LoadStage>,
    /// 0.0.5: why the loaded context is the size it is, for the ready line (D1). `None`
    /// until WP1b's resolver fills it.
    pub ctx_source: Option<ContextSource>,
    /// 0.0.5: the loaded copy uses older settings than the ones saved now (D3: "Settings
    /// changed. Restart the model to use them.").
    pub settings_changed: bool,
    /// 0.0.5: a reload waits for the model to be free (`reload_local_model(when_idle)`),
    /// shown as "Restarts when the reply ends · Cancel".
    pub reload_waiting: bool,
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
            load_stage: None,
            adjustments: Vec::new(),
            ctx_source: None,
            settings_changed: false,
            reload_waiting: false,
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

/// The defaults for all models (settings key `localModel`). 0.0.5 adds the fields after
/// `mmproj`, each `#[serde(default)]`, so the JSON saved by 0.0.4 still reads and the 0.0.4
/// command shape `{ctx, backend, device}` still works. One model's own choices are in
/// [`crate::load_settings::ModelLoadSettings`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelSettings {
    /// The context size in tokens. Kept as it is while `context_auto` is on, so turning
    /// Automatic off again shows the size chosen before.
    pub ctx: u32,
    /// None means Auto.
    pub backend: Option<String>,
    pub device: Option<String>,
    /// Image projector to load with the model. Set per load from the installed-model
    /// record, never stored in the global settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mmproj: Option<String>,
    /// 0.0.5: "How much it can remember" is Automatic (D2b). Absent in 0.0.4 JSON, where
    /// it reads as false; the upgrade rule (stored ctx exactly 8,192 and no `contextAuto`
    /// key means Automatic) is WP1b's and needs the raw JSON to tell absent from false.
    #[serde(default)]
    pub context_auto: bool,
    /// 0.0.5: "Use the graphics card" (default All of it, the 0.0.4 behaviour).
    #[serde(default)]
    pub gpu: GpuUse,
    /// 0.0.5: "Save memory" (off by default).
    #[serde(default)]
    pub save_memory: bool,
    /// 0.0.5: "Keep the model in memory" (off by default).
    #[serde(default)]
    pub keep_in_memory: bool,
    /// 0.0.5: "How long it may think" (default: no limit, shown as "Default").
    #[serde(default)]
    pub thinking_limit: ThinkingLimit,
    /// 0.0.5, response only: the size the upgrade rule kept (D2), on the one read that
    /// should show the one-time note under Defaults for all models ("Every model now uses
    /// the size you chose before (N tokens, about M words)…"). Never stored:
    /// `set_local_model_settings` drops it. WP1b fills it; until then always `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_note: Option<u32>,
}

impl Default for LocalModelSettings {
    fn default() -> Self {
        Self {
            ctx: 8192,
            backend: None,
            device: None,
            mmproj: None,
            context_auto: false,
            gpu: GpuUse::All,
            save_memory: false,
            keep_in_memory: false,
            thinking_limit: ThinkingLimit::NoLimit,
            upgrade_note: None,
        }
    }
}

/// What is using the local model right now (0.0.5 D3): chat turns, model checks and
/// benchmarks, and requests from other apps. The live counter is
/// [`crate::state::BusyCounter`]; this is a snapshot of it (`model_busy`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBusy {
    /// Chat turns writing a reply.
    pub chat_turns: u32,
    /// Model checks and benchmarks running.
    pub model_checks: u32,
    /// Requests from other apps being answered (WP5a).
    pub other_apps: u32,
}

impl ModelBusy {
    /// Nothing is using the model.
    pub fn is_idle(&self) -> bool {
        self.chat_turns == 0 && self.model_checks == 0 && self.other_apps == 0
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

// ---- Model comparison (0.0.5 item 4; commands in hub.rs, WP4a) ---------------------

/// One model to compare: exactly one of a catalog entry, a file in a Hugging Face
/// repository, or an installed file. JSON: `{"catalogId": "…"}`, `{"repo": "…", "file":
/// "…"}` or `{"installedPath": "…"}`; anything else (none, two, unknown keys, a repo
/// without its file) is refused when read.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "CompareItemWire", into = "CompareItemWire")]
pub enum CompareItem {
    /// An entry of the signed catalog, by id.
    Catalog { catalog_id: String },
    /// A GGUF file in a Hugging Face repository (`owner/name`, path in the repository).
    Repo { repo: String, file: String },
    /// A model file on this computer, by path.
    Installed { installed_path: String },
}

/// The JSON form of [`CompareItem`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompareItemWire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    catalog_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    installed_path: Option<String>,
}

impl TryFrom<CompareItemWire> for CompareItem {
    type Error = String;
    fn try_from(w: CompareItemWire) -> Result<Self, Self::Error> {
        match (w.catalog_id, w.repo, w.file, w.installed_path) {
            (Some(catalog_id), None, None, None) => Ok(Self::Catalog { catalog_id }),
            (None, Some(repo), Some(file), None) => Ok(Self::Repo { repo, file }),
            (None, None, None, Some(installed_path)) => Ok(Self::Installed { installed_path }),
            _ => Err(
                "A model to compare names exactly one of: a catalog id, a repository \
                 and file, or an installed file."
                    .to_string(),
            ),
        }
    }
}

impl From<CompareItem> for CompareItemWire {
    fn from(item: CompareItem) -> Self {
        match item {
            CompareItem::Catalog { catalog_id } => Self {
                catalog_id: Some(catalog_id),
                ..Self::default()
            },
            CompareItem::Repo { repo, file } => Self {
                repo: Some(repo),
                file: Some(file),
                ..Self::default()
            },
            CompareItem::Installed { installed_path } => Self {
                installed_path: Some(installed_path),
                ..Self::default()
            },
        }
    }
}

/// A licence: its name (or SPDX id) and a link to its text when known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseFacts {
    pub name: String,
    pub url: Option<String>,
}

/// Curated facts from the signed catalog (D6 source 1, "From the publisher's model card").
/// Attached to a column only on an exact match (sha256, repo + file, or catalog id).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogFacts {
    /// The catalog id.
    pub id: String,
    /// The display name from the catalog.
    pub name: String,
    /// A verbatim quote of at most two sentences from the publisher's model card.
    pub summary: Option<String>,
    /// The URL of the publisher's model card the summary is quoted from.
    pub source: Option<String>,
    /// Keys from the fixed vocabulary: chat, writing, coding, reasoning, math, tools,
    /// images, long-documents, languages, speed (unknown keys are dropped when read).
    pub good_at: Vec<String>,
    pub publisher: Option<String>,
    /// Parameters, in units (9 billion is 9000000000).
    pub parameters: Option<u64>,
    /// For a mixture of experts: parameters used at a time.
    pub active_parameters: Option<u64>,
    pub languages: Vec<String>,
    /// Release date as written in the catalog (`YYYY-MM-DD` or `YYYY-MM`).
    pub released: Option<String>,
    /// The catalog's capabilities: `tools`, `vision`, `thinking`.
    pub capabilities: Vec<String>,
    pub license: Option<LicenseFacts>,
}

/// Catalog text shown for a Hugging Face repository whose uploader names that catalog
/// model's source as its base model. Shown only under "The uploader says this is made
/// from <base model>. That model's publisher says:", never as a plain catalog match and
/// never with the catalog's capabilities or fit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseModelFacts {
    /// The base model as the uploader wrote it (sanitised).
    pub base_model: String,
    /// The catalog entry whose source is that base model.
    pub catalog: CatalogFacts,
}

/// What the Hugging Face uploader lists (D6 source 2, "The uploader lists"). All text is
/// sanitised (bidi and zero-width characters made visible) and capped at 300 characters;
/// unknown tags are dropped.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubFacts {
    /// `owner/name`.
    pub repo: String,
    /// The account that uploaded the repository.
    pub author: Option<String>,
    pub pipeline_tag: Option<String>,
    pub base_model: Option<String>,
    pub languages: Vec<String>,
    pub license: Option<LicenseFacts>,
    /// Known tags only.
    pub tags: Vec<String>,
    pub downloads: Option<u64>,
    pub likes: Option<u64>,
    /// As Hugging Face reports it (an ISO 8601 time).
    pub last_modified: Option<String>,
    pub gated: bool,
    /// The repository page, opened through the destination-confirm dialog
    /// ("Show model card").
    pub card_url: String,
}

/// Facts read from the GGUF file's header (before download from its first bytes, or
/// saved at download in `gguf_summary`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub architecture: Option<String>,
    pub parameters: Option<u64>,
    /// The context the model was trained for ("Longest conversation it was made for").
    pub context_length: Option<u64>,
    /// The chat template can call tools.
    pub tools: bool,
    /// The chat template can think.
    pub thinking: bool,
    /// An image projector comes with it.
    pub vision: bool,
    /// A mixture of experts.
    pub moe: bool,
    /// The quantisation name (`Q4_K_M`).
    pub quantization: Option<String>,
}

/// Which kind of local result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum YourResultKind {
    /// A 0.0.4 model check ("Tested settings").
    ModelCheck,
    /// A coding benchmark run.
    Benchmark,
}

/// One of the user's own results with this model file on this computer (D6 source 3,
/// "Your results"), matched by file name and size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct YourResult {
    pub kind: YourResultKind,
    /// The profile's settings key or the benchmark run's id, to open it.
    pub id: String,
    /// When it ran (milliseconds since 1970).
    pub at: i64,
    /// The backend it ran on (`vulkan`, `cpu`…).
    pub backend: Option<String>,
    pub passed: u32,
    pub total: u32,
}

/// Why some facts of a column are missing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CompareProblem {
    /// Hugging Face could not be reached.
    Offline,
    /// Hugging Face answered 429.
    RateLimited,
    /// The repository needs a Hugging Face token (or its terms accepted).
    Gated,
    /// The repository, file, catalog entry or installed file was not found.
    NotFound,
    /// Anything else.
    Failed,
}

/// Everything known about one model to compare (`compare_facts`). The webview picks the
/// answer to each row from these parts and names the part as the row's source.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompareFacts {
    /// The item asked for, as given.
    pub item: CompareItem,
    /// A short name for the column (catalog name, repository id, or file name).
    pub title: String,
    /// The GGUF file the column is about (the first part of a split model).
    pub file: Option<String>,
    /// The quantisation name from the file name or header (`Q4_K_M`).
    pub quant: Option<String>,
    /// Download size of all the model's files, in bytes.
    pub size_bytes: Option<u64>,
    /// The path when this model is installed on this computer.
    pub installed_path: Option<String>,
    /// The context the fit was checked at ("Compare at …").
    pub ctx: u32,
    /// One fit verdict at `ctx` ("Will it fit on this computer?").
    pub fit: Option<models::FitResult>,
    /// Exact catalog match only.
    pub catalog: Option<CatalogFacts>,
    /// The uploader names a catalog model's source as its base model.
    pub base_catalog: Option<BaseModelFacts>,
    /// From Hugging Face (known, or fetched for this one repository).
    pub hub: Option<HubFacts>,
    /// From the GGUF header.
    pub file_facts: Option<FileFacts>,
    /// The user's own results with this file.
    pub results: Vec<YourResult>,
    /// Why facts are missing, if some are.
    pub problem: Option<CompareProblem>,
}

/// One entry of the signed catalog for Suggested models (`catalog_list`).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogListEntry {
    pub catalog: CatalogFacts,
    /// `owner/name` on Hugging Face.
    pub repo: String,
    /// The first model file's path in the repository.
    pub file: String,
    /// The quantisation as the catalog names it.
    pub quant: String,
    /// Download size of every file (model parts and projector), in bytes.
    pub size_bytes: u64,
    /// One fit verdict at the context asked for.
    pub fit: Option<models::FitResult>,
    /// The path when this entry is installed.
    pub installed_path: Option<String>,
    /// The user's own results with this file.
    pub results: Vec<YourResult>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The defaults for all models as 0.0.4 wrote them, field for field.
    #[derive(Debug, Deserialize, Serialize, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct LocalModelSettings004 {
        ctx: u32,
        backend: Option<String>,
        device: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mmproj: Option<String>,
    }

    /// JSON saved by 0.0.4 reads in 0.0.5 with every new field at its default, and the
    /// 0.0.4 command shape `{ctx, backend, device}` (e2e scripts) still works.
    #[test]
    fn json_from_0_0_4_reads() {
        let old = json!({"ctx": 16384, "backend": "vulkan", "device": "Vulkan1"});
        let s: LocalModelSettings = serde_json::from_value(old).unwrap();
        assert_eq!(
            s,
            LocalModelSettings {
                ctx: 16384,
                backend: Some("vulkan".into()),
                device: Some("Vulkan1".into()),
                ..LocalModelSettings::default()
            }
        );
        assert!(!s.context_auto && !s.save_memory && !s.keep_in_memory);
        assert_eq!(s.gpu, GpuUse::All);
        assert_eq!(s.thinking_limit, ThinkingLimit::NoLimit);
        assert_eq!(s.upgrade_note, None);
        let nulls: LocalModelSettings =
            serde_json::from_value(json!({"ctx": 8192, "backend": null, "device": null})).unwrap();
        assert_eq!(nulls, LocalModelSettings::default());
    }

    /// JSON saved by 0.0.5 still reads in 0.0.4 (someone going back a version): the new
    /// fields are ignored and the backend is kept.
    #[test]
    fn json_from_0_0_5_reads_in_0_0_4() {
        let new = LocalModelSettings {
            ctx: 32768,
            backend: Some("rocm".into()),
            device: Some("ROCm0".into()),
            context_auto: true,
            gpu: GpuUse::Fit,
            save_memory: true,
            keep_in_memory: true,
            thinking_limit: ThinkingLimit::Short,
            ..LocalModelSettings::default()
        };
        let v = serde_json::to_value(&new).unwrap();
        assert_eq!(
            v,
            json!({
                "ctx": 32768,
                "backend": "rocm",
                "device": "ROCm0",
                "contextAuto": true,
                "gpu": "fit",
                "saveMemory": true,
                "keepInMemory": true,
                "thinkingLimit": "short"
            })
        );
        let old: LocalModelSettings004 = serde_json::from_value(v).unwrap();
        assert_eq!(
            old,
            LocalModelSettings004 {
                ctx: 32768,
                backend: Some("rocm".into()),
                device: Some("ROCm0".into()),
                mmproj: None,
            }
        );
    }

    #[test]
    fn a_compare_item_names_exactly_one_model() {
        let cases = [
            (
                json!({"catalogId": "sample-small"}),
                CompareItem::Catalog {
                    catalog_id: "sample-small".into(),
                },
            ),
            (
                json!({"repo": "owner/name", "file": "a/b.gguf"}),
                CompareItem::Repo {
                    repo: "owner/name".into(),
                    file: "a/b.gguf".into(),
                },
            ),
            (
                json!({"installedPath": "/models/x.gguf"}),
                CompareItem::Installed {
                    installed_path: "/models/x.gguf".into(),
                },
            ),
        ];
        for (v, item) in cases {
            assert_eq!(
                serde_json::from_value::<CompareItem>(v.clone()).unwrap(),
                item
            );
            assert_eq!(serde_json::to_value(&item).unwrap(), v);
        }
        for bad in [
            json!({}),
            json!({"repo": "owner/name"}),
            json!({"file": "a.gguf"}),
            json!({"catalogId": "x", "installedPath": "/m.gguf"}),
            json!({"catalogId": "x", "repo": "o/n", "file": "f.gguf"}),
            json!({"installedPath": "/m.gguf", "extra": 1}),
            json!({"catalog_id": "x"}),
            json!("x"),
        ] {
            assert!(
                serde_json::from_value::<CompareItem>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn busy_snapshot_shape() {
        let b = ModelBusy {
            chat_turns: 1,
            model_checks: 0,
            other_apps: 2,
        };
        assert!(!b.is_idle());
        assert!(ModelBusy::default().is_idle());
        assert_eq!(
            serde_json::to_value(b).unwrap(),
            json!({"chatTurns": 1, "modelChecks": 0, "otherApps": 2})
        );
    }

    /// The new status fields have defaults that change nothing on screen.
    #[test]
    fn runtime_status_defaults() {
        let v = serde_json::to_value(RuntimeStatusDto::default()).unwrap();
        assert_eq!(v["loadStage"], json!(null));
        assert_eq!(v["adjustments"], json!([]));
        assert_eq!(v["ctxSource"], json!(null));
        assert_eq!(v["settingsChanged"], json!(false));
        assert_eq!(v["reloadWaiting"], json!(false));
        assert_eq!(v["stage"], json!(null));
    }
}
