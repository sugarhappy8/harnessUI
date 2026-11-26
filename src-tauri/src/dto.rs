//! Data sent to the webview. Field names are camelCase and match src/lib/types.ts.

use agent::media::{
    Company, EndpointCompany, MediaKind, MediaModel, ParamSpec, Price, RefRole, Routing,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use store::{ChatId, MessageId, ProviderId};

use models::gen_catalog::GenRole;
use models::sd::detect::SdVersion;
use models::sd::inspect::LoraFormat;
use models::sd::matching::RecipeKind;
use models::sd::setup::PartSource;
use models::sd::{FileLabel, Verdict};

use crate::error::AppError;
use crate::load_settings::{ContextSource, GpuUse, ThinkingLimit};
use crate::studio::prefs::RunTarget;
use crate::studio::spend::ConfirmReason;

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
    /// [0.0.7 P5] Where the chat's messages go when its model is on an endpoint on the
    /// internet (the header's "Cloud" mark): the endpoint's host. `None` for a model on this
    /// computer or the local network.
    pub cloud_host: Option<String>,
    /// [0.0.7 P5] The chat's model is on an endpoint whose thinking the app switches
    /// (Claude), so the header offers Thinking on and off for it.
    pub endpoint_thinking: bool,
    /// [0.0.7 P5] ...but the model's thinking cannot stop, so "off" asks it to think little
    /// (the header says "Low").
    pub endpoint_thinking_low: bool,
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
    /// [0.0.5 memory] The summary the model gets instead of the earlier messages of the
    /// shown branch, if there is one.
    pub summary: Option<EarlierSummaryDto>,
}

/// [0.0.5 memory] A summary of earlier messages, for the line the chat draws under the last
/// message it stands for ("Above this line, the model sees a summary instead of the
/// messages. Show summary").
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EarlierSummaryDto {
    /// The newest message it stands for; every message above it on the branch is covered.
    pub last_message_id: store::MessageId,
    /// What the model wrote. Model output: the chat shows it as plain text, never markup.
    pub text: String,
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
    /// 0.0.5: a change of what is loaded waits for the model to be free (D3 "Restart when
    /// it ends"): a reload (`reload_local_model`), a load of another model
    /// (`load_local_model`) or an unload (`unload_model`), each called with `when_idle`.
    /// Shown as "Restarts when the reply ends · Cancel"; `cancel_pending_reload` cancels
    /// it. The status still describes the model loaded now.
    pub reload_waiting: bool,
    /// 0.1.0: a load waits while a local Studio job holds the graphics card ("Studio is
    /// using the graphics card. The chat model loads when it finishes · Stop Studio job").
    /// The status still describes the model loaded now (none, after Studio unloaded it).
    pub waiting_for_studio: bool,
    /// 0.0.5: the model the user unloaded (`unload_model`), for "Unloaded to free memory ·
    /// Load". Cleared by the next load and by any other unload (removing the model or its
    /// backend). `None` until WP1b sets it.
    pub unloaded_path: Option<String>,
    /// 0.0.6 item 6 (WP10d): where the loaded model went, from the ready line's memory
    /// report (`offload::placement_from_report`). `None` until the report arrives (about
    /// when the model is ready); cleared by each load and each retry.
    pub placement: Option<Placement>,
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
            waiting_for_studio: false,
            unloaded_path: None,
            placement: None,
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

/// What is using the local model right now (0.0.6 D3): chat turns, model checks and
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
    /// [0.0.7 P5] The server is on the internet (not this computer or the local network).
    pub cloud: bool,
    /// [0.0.7 P5] The context size chosen for this endpoint; `None` is Automatic.
    pub context_length: Option<u32>,
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
    /// [0.0.7 P5] The context size chosen for this endpoint; absent or `None` is Automatic.
    #[serde(default)]
    pub context_length: Option<u32>,
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
    /// 0.0.5: for a mixture of experts, how many experts it has and how many it uses for
    /// each token (from the header); `None` for other models.
    pub expert_count: Option<u64>,
    pub expert_used_count: Option<u64>,
    /// 0.0.5: for a mixture of experts, the parameters used at a time ("35 billion, using
    /// 3 billion at a time"), when they can be worked out from the file; `None` otherwise.
    pub active_parameters: Option<u64>,
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

/// 0.0.6 item 6 (WP10d): what the loaded model takes on its graphics device. JSON
/// `{"name": "AMD Radeon RX 9070 XT", "use": {"model": …, "context": …, "compute": …,
/// "imageReader": …}}`.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct GraphicsUse {
    /// The probe's description of the device ("Graphics card" / "Apple graphics" when not
    /// known).
    pub name: String,
    #[serde(rename = "use")]
    pub parts: models::placement::Parts,
}

/// 0.0.6 item 6 (WP10d): where the loaded model went ("Where the model went" in the usage
/// menu; WP2's "part on the processor" note).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    /// `None` when nothing is on a graphics device.
    pub graphics: Option<GraphicsUse>,
    pub computer: models::placement::Parts,
    /// llama.cpp's units ("of 41 layers"), when the model's sizes are known.
    pub units: Option<u32>,
    /// `None`: only amounts are known.
    pub layers_on_graphics: Option<u32>,
    /// Blocks on the card whose expert parts are on the processor (mixture of experts).
    pub expert_layers_on_processor: Option<u32>,
    /// As much as fits: the count is worked out from the bytes ("About").
    pub layers_about: bool,
    /// Fine control chose the layers or expert parts.
    pub custom: bool,
}

// ---- Studio runs (0.1.0) ----------------------------------------------------------------------

/// A press of Create, as the webview sends it to `studio_start` (and to the estimate and the
/// preview, which send nothing anywhere).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartRequest {
    /// Made once per press, so the same press sent twice makes one run.
    pub client_token: String,
    /// An endpoint, or this computer.
    pub target: RunTarget,
    /// Pictures or video; it must be the model's kind.
    pub kind: MediaKind,
    /// The model's id at the target.
    pub model: String,
    /// The prompt, sent as written.
    pub prompt: String,
    /// The model's typed settings, by key; a setting left out is not sent.
    pub params: Map<String, Value>,
    /// Extra settings (JSON), by name.
    pub extra: Map<String, Value>,
    /// The added pictures, by their name in the gallery's `refs/`.
    pub refs: Vec<RefInput>,
    /// Which companies may run an OpenRouter request.
    pub routing: Option<Routing>,
}

/// An added picture in a [`StartRequest`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefInput {
    /// Its SHA-256, the name it is kept under in the gallery's `refs/`.
    pub sha256: String,
    /// What it is for.
    pub role: RefRole,
}

/// An added picture of a run, as recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefDto {
    /// Its SHA-256.
    pub sha256: String,
    /// Its media type.
    pub mime: String,
    /// Its size.
    pub size_bytes: u64,
    /// What it was for, when recorded.
    pub role: Option<RefRole>,
}

/// Why a run ended without its results: a four-part error plus what the company said. It is
/// kept as the run's error JSON, so it always carries `charged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunErrorDto {
    /// The error code, for example `studio_blocked`.
    pub code: String,
    /// What happened, in plain words.
    pub message: String,
    /// What the user can do.
    pub next_step: String,
    /// Details for a bug report.
    pub details: String,
    /// Whether the run was charged; `None` when that is not known.
    pub charged: Option<bool>,
    /// The company's request id, for an appeal.
    pub request_id: Option<String>,
    /// `openrouter`, `openai` or `local`.
    pub company: String,
    /// Where a safety filter stopped it (`prompt` or `output`), when the company said.
    pub stage: Option<String>,
}

/// One file a run made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputDto {
    /// Its id.
    pub id: String,
    /// The run that made it.
    pub run_id: String,
    /// Its place among the run's files, from 0.
    pub idx: u32,
    /// Its media type, read from its bytes.
    pub mime: String,
    /// Its size.
    pub size_bytes: u64,
    /// Its width in pixels, when known.
    pub width: Option<u32>,
    /// Its height in pixels, when known.
    pub height: Option<u32>,
    /// A video's length, when known.
    pub duration_ms: Option<u64>,
    /// The seed it was made with, when known.
    pub seed: Option<u64>,
    /// Starred by the user.
    pub starred: bool,
    /// Whether its thumbnail was made.
    pub has_thumb: bool,
    /// Its absolute path in the gallery folder.
    pub path: String,
}

/// A Studio run as the webview shows it (`studio-progress`, `studio_runs`, `studio_live`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunDto {
    /// Its id.
    pub id: String,
    /// Which state of a run is newest: one count for every run, taken just before the row is
    /// read, so of two copies of the same run (an event, a command's answer) the webview keeps
    /// the one with the higher number.
    pub seq: u64,
    /// Pictures or video.
    pub kind: MediaKind,
    /// `cloud` or `local`.
    pub runs_on: String,
    /// The endpoint; `None` for a run on this computer or once the endpoint is deleted.
    pub provider_id: Option<i64>,
    /// The endpoint's name when the run was made.
    pub provider_name: String,
    /// The endpoint's host when the run was made.
    pub provider_host: String,
    /// The company OpenRouter ran it at, by name, when known.
    pub upstream: Option<String>,
    /// The model's id.
    pub model: String,
    /// The model's name as shown.
    pub model_label: String,
    /// The prompt.
    pub prompt: String,
    /// The typed settings sent (with the seed Studio picked).
    pub params: Map<String, Value>,
    /// The extra settings, as the user set them.
    pub extra: Map<String, Value>,
    /// The routing sent.
    pub routing: Option<Routing>,
    /// The added pictures.
    pub refs: Vec<RefDto>,
    /// `queued`, `sending`, `waiting`, `running`, `downloading`, `done`, `failed`,
    /// `blocked`, `cancelled` or `interrupted`.
    pub status: String,
    /// Whether Studio is following the run now (queued, or its task is running).
    pub watching: bool,
    /// A video still not ready after the watch limit, no longer checked (Task A14).
    pub stale: bool,
    /// The company's id for the request or job.
    pub remote_id: Option<String>,
    /// Progress from 0 to 1, when reported.
    pub progress: Option<f64>,
    /// The estimate in US dollars; `None` when the price was unknown.
    pub estimate_usd: Option<f64>,
    /// What the company charged, in US dollars, when known.
    pub cost_usd: Option<f64>,
    /// Why it ended without its results.
    pub error: Option<RunErrorDto>,
    /// Unix ms when it was made.
    pub created_at: i64,
    /// Unix ms when sending (or the engine) started.
    pub started_at: Option<i64>,
    /// Unix ms when it ended.
    pub finished_at: Option<i64>,
    /// Its files, in order.
    pub outputs: Vec<OutputDto>,
    /// Its place in its endpoint's queue, from 1, while it waits for a free place.
    pub queue_position: Option<u32>,
    /// How long a local run takes; `None` for a cloud run.
    pub eta: Option<EtaDto>,
    /// A local run's stage and steps; `None` for a cloud run.
    pub local: Option<LocalRunDto>,
}

/// How long a local run takes. The backend always fills it through
/// `gen_eta::round_for_display` (`left_ms` included), so every view shows the same time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EtaDto {
    /// The shortest time it should take, in ms.
    pub low_ms: u64,
    /// The longest time it should take, in ms.
    pub high_ms: u64,
    /// Measured on this computer (else the catalog's first guess).
    pub measured: bool,
    /// During a run: the time left, in ms.
    pub left_ms: Option<u64>,
}

/// Where a run on this computer is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRunDto {
    /// `loading`, `readingPrompt`, `drawing`, `decoding` or `saving`.
    pub stage: Option<String>,
    /// The step being drawn.
    pub step: Option<u32>,
    /// The steps of each picture.
    pub steps: Option<u32>,
    /// The picture being drawn.
    pub picture: Option<u32>,
    /// The pictures the run makes.
    pub pictures: Option<u32>,
    /// The pass being run.
    pub pass: Option<u32>,
    /// The passes the run makes.
    pub passes: Option<u32>,
    /// What a queued local run waits for: `reply` (a chat reply being written), `studioJob`,
    /// `answer` (the user's answer about unloading the chat model) or `chatModelLoading` (a
    /// chat model starting, which goes first). `studioJob` is a defensive value, unreachable in
    /// 0.1.0: the single local queue never lets two Studio jobs ask for the graphics card at
    /// once, so a run behind another shows `queue_position` instead.
    pub waiting_for: Option<String>,
    /// The path of the chat model unloaded to make room.
    pub unloaded_chat: Option<String>,
    /// The engine is trying again with less memory.
    pub retrying: bool,
    /// No progress for 10 minutes.
    pub stalled: bool,
    /// The device it runs on, as shown.
    pub device: Option<String>,
    /// The Studio engine's build number.
    pub engine_build: Option<u32>,
    /// How long it has run, in ms.
    pub elapsed_ms: Option<u64>,
    /// Its files are labelled as AI-made.
    pub labelled: bool,
    /// It runs beside the loaded chat model.
    pub alongside: bool,
    /// It runs on the processor.
    pub on_processor: bool,
}

/// The daily limit as it stands for a run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitDto {
    /// The limit in US dollars; `None` when it is off.
    pub limit_usd: Option<f64>,
    /// What ended and finished runs spent today, in US dollars.
    pub spent_today_usd: f64,
    /// What runs still going count, in US dollars.
    pub pending_usd: f64,
    /// Whether `spend::check_limit` would allow this run (`true`: it may be sent).
    pub would_pass: bool,
    /// Why it would not: `over` (it would take today's spending past the limit) or
    /// `unknownNeedsReserve` (its price is unknown and less than `reserve_usd` of the limit is
    /// left); `None` when it would pass.
    pub refusal: Option<LimitRefusalDto>,
    /// What a run whose price is unknown counts against the limit, in US dollars.
    pub reserve_usd: f64,
}

/// Why the daily limit refuses a run (`LimitDto::refusal`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum LimitRefusalDto {
    Over,
    UnknownNeedsReserve,
}

/// What a run would cost and meet before it is sent, without sending anything.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EstimateDto {
    /// The estimate in US dollars; `None` when the price is unknown.
    pub usd: Option<f64>,
    /// Why the run would ask before it is sent; empty when it would not.
    pub reasons: Vec<ConfirmReason>,
    /// The amount from which a cloud run asks, in US dollars; `None` when that is off.
    pub confirm_at_usd: Option<f64>,
    /// The daily limit as it stands for this run.
    pub limit: LimitDto,
    /// A plain sentence for each setting the model would refuse.
    pub invalid: Vec<String>,
    /// How long a local run takes; `None` for a cloud run.
    pub eta: Option<EtaDto>,
    /// Whether a local run fits this computer; `None` for a cloud run.
    pub local: Option<LocalEstimateDto>,
}

/// Whether a local run fits this computer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEstimateDto {
    /// `fits`, `slow`, `afterUnload` or `notHere`.
    pub fit: String,
    /// The chat model would be unloaded to make room.
    pub unload_chat: bool,
    /// The run would take longer than 15 minutes, or runs on the processor.
    pub very_slow: bool,
    /// The run would take longer than 15 minutes.
    pub over_15_min: bool,
    /// Why it does not fit, when `fit` is `notHere` ("needs 6 GB of graphics memory").
    pub reason: Option<String>,
}

/// Today's spending and the gallery's size, for Studio's top bar and Settings → Studio.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageDto {
    /// What ended and finished cloud runs spent today, in US dollars.
    pub spent_today_usd: f64,
    /// What cloud runs still going count, in US dollars.
    pub pending_usd: f64,
    /// The daily limit in US dollars; `None` when it is off.
    pub limit_usd: Option<f64>,
    /// The gallery folder.
    pub folder: String,
    /// The bytes of the results and added pictures kept.
    pub bytes: u64,
    /// The pictures kept.
    pub pictures: u64,
    /// The videos kept.
    pub videos: u64,
}

// ---- Studio models and the gallery's pages (0.1.0) --------------------------------------------

/// A model's price as the picker hints it: per what, and the lowest and highest price of one.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceHintDto {
    /// `image`, `second`, `token`, `unknown` or `free`.
    pub unit: String,
    /// The lowest price of one picture or one second, in US dollars, when there is a number.
    pub low: Option<f64>,
    /// The highest price of one picture or one second, in US dollars, when there is a number.
    pub high: Option<f64>,
}

/// A picture or video model as Studio's picker shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaModelDto {
    /// The id sent to the company.
    pub id: String,
    /// The name shown.
    pub display_name: String,
    /// The maker and the model's name without its version.
    pub family: String,
    /// Pictures or video.
    pub kind: MediaKind,
    /// Whether the model takes added pictures.
    pub accepts_images: bool,
    /// The controls, simple ones first.
    pub params: Vec<ParamSpec>,
    /// Its price, as the picker hints it.
    pub price: PriceHintDto,
    /// Whether the price has a number (a run on this computer is free, which counts).
    pub price_known: bool,
    /// When the price was fetched. `None` on an OpenRouter picture means "price shown when
    /// picked"; a time with `price_known` false means "price unknown".
    pub price_fetched_at: Option<i64>,
    /// The companies that run an OpenRouter picture model, once its price was fetched.
    pub companies: Vec<EndpointCompany>,
    /// Plain sentences shown with the model.
    pub notes: Vec<String>,
    /// Why Studio cannot use the model, when it cannot.
    pub usable: Option<String>,
    /// When the company added the model, in seconds since 1970.
    pub created: Option<i64>,
}

/// An endpoint that makes pictures or video, with the models of its saved lists.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioEndpointDto {
    /// The endpoint.
    pub provider_id: i64,
    /// Its name as shown.
    pub name: String,
    /// The company behind it.
    pub company: Company,
    /// The host its requests go to, for example `openrouter.ai`.
    pub host: String,
    /// Whether the endpoint is on the internet.
    pub cloud: bool,
    /// Whether it may have a key (its models are greyed out without one).
    pub has_key: bool,
    /// When its lists were fetched; `None` when none are saved ("Get the list").
    pub fetched_at: Option<i64>,
    /// The models of its saved lists: pictures, then videos.
    pub models: Vec<MediaModelDto>,
}

/// Every endpoint that makes pictures or video, with its saved models (`studio_models`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StudioModelsDto {
    /// The endpoints, in the order of Settings → Endpoints.
    pub endpoints: Vec<StudioEndpointDto>,
}

/// `model` as the picker shows it, with when its price was fetched. A model on this
/// computer (`Price::Free`) has the unit `free`, a range of 0 to 0 and a known price.
pub fn media_model_dto(model: &MediaModel, price_fetched_at: Option<i64>) -> MediaModelDto {
    let unit = match model.price {
        Price::PerImage(_) => "image",
        Price::PerSecond(_) => "second",
        Price::PerToken => "token",
        Price::Unknown => "unknown",
        Price::Free => "free",
    };
    let range = model.price.range();
    MediaModelDto {
        id: model.id.clone(),
        display_name: model.display_name.clone(),
        family: model.family.clone(),
        kind: model.kind,
        accepts_images: model.accepts_images,
        params: model.params.clone(),
        price: PriceHintDto {
            unit: unit.to_owned(),
            low: range.map(|(low, _)| low),
            high: range.map(|(_, high)| high),
        },
        price_known: range.is_some(),
        price_fetched_at,
        companies: model.companies.clone(),
        notes: model.notes.clone(),
        usable: model.usable.clone(),
        created: model.created,
    }
}

/// Which runs the gallery shows (`studio_runs`). Every field left out does not filter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RunsFilter {
    /// Only pictures, or only videos.
    pub kind: Option<MediaKind>,
    /// Only runs with a starred result.
    pub starred: bool,
    /// Only runs whose prompt holds this text (any case); blank does not filter.
    pub text: String,
    /// Only runs of this model id.
    pub model: Option<String>,
    /// Only runs made at or after this Unix ms.
    pub since: Option<i64>,
    /// Only runs made before this Unix ms.
    pub until: Option<i64>,
}

/// A place in the gallery's order (newest first, then by id): the last run of a page, so the
/// next page starts just after it. Both are needed, as runs can share a creation time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunsCursorDto {
    /// The run's `created_at`.
    pub created_at: i64,
    /// The run's id.
    pub id: String,
}

impl From<RunsCursorDto> for store::RunCursor {
    fn from(cursor: RunsCursorDto) -> Self {
        Self {
            created_at: cursor.created_at,
            id: cursor.id,
        }
    }
}

/// One page of the gallery's runs.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunsPage {
    /// The runs, newest first.
    pub runs: Vec<RunDto>,
    /// Where the next page starts: the last run's place when the page is full, else `None`.
    pub next: Option<RunsCursorDto>,
}

// ---- The Studio engine (0.1.0) ----------------------------------------------------------------

/// The Studio engine on this computer (`studio_engine_status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatusDto {
    /// "builtIn" (macOS, or a development engine), "notInstalled", "partial", "downloading",
    /// "installed" (the Windows pack) or "unavailable" (no engine for this computer).
    pub state: String,
    /// The pinned stable-diffusion.cpp build, as its release tag (`master-1106-532d2c1`).
    pub build: String,
    /// The Windows pack's download size; 0 when the engine is built in or unavailable.
    pub download_bytes: u64,
    pub license_name: String,
    pub license_url: String,
    /// Bytes downloaded so far, while the pack is `partial` or `downloading`.
    pub bytes_done: Option<u64>,
}

/// The `studio-engine-progress` event: the Windows pack's download, at most every 300 ms,
/// then once with `finished` set (and `error` when it failed; a cancelled download has none).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineProgressDto {
    pub done: u64,
    pub total: u64,
    /// The download is complete and the pack is being unpacked.
    pub unpacking: bool,
    pub finished: bool,
    pub error: Option<AppError>,
}

// ---- Get models (0.1.0) -----------------------------------------------------------------

/// What Get models' header says about this computer: "This computer: Radeon RX 7600, 8
/// GB graphics memory · 32 GB memory · 412 GB free".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HardwareLineDto {
    /// The graphics device's name, or "Processor" on a computer that runs models there.
    pub device: String,
    /// A graphics card's total memory, as its probe says (its free memory with the chat
    /// model's share counted as free when the probe gives no total); on Apple silicon, the
    /// share of memory Metal may use; `None` on the processor.
    pub graphics_bytes: Option<u64>,
    /// Apple silicon: the graphics device shares the computer's memory.
    pub shared_memory: bool,
    /// The computer's memory, when the system says.
    pub ram_bytes: Option<u64>,
    /// The free space on the models folder's disk, when the system says.
    pub free_disk_bytes: Option<u64>,
}

/// A model licence as Get models and the download dialog show it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenceDto {
    /// Its key among the model's licences, which parts name (`apache-2.0`).
    pub id: String,
    pub name: String,
    /// The upstream page at a pinned revision.
    pub url: String,
    /// The licence fingerprint supplied with the model metadata.
    pub sha256: String,
    /// `green`, `amber` or `red`.
    pub badge: String,
    /// What it allows for what you make: `yes`, `underRevenue`, `registration`,
    /// `withUseRules`, `unclear` or `no`.
    pub commercial: String,
    /// The yearly revenue under which commercial use is allowed, in US dollars, when
    /// `commercial` is `underRevenue` and the licence states one.
    pub revenue_limit_usd: Option<u64>,
    pub plain_words: String,
    pub obligations: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub excluded_regions: Vec<String>,
}

/// How a model's Hugging Face repository is gated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GateDto {
    /// `none`, `auto` (access at once after accepting) or `manual` (the publisher approves).
    pub mode: String,
    /// What the publisher collects when access is asked for.
    pub collects: Vec<String>,
}

/// One file of a model, as the download dialog and a set-up plan list it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartDto {
    /// The slot it fills (`diffusion`, `llm`, `t5xxl`, `vae`, …): the webview words it.
    pub role: GenRole,
    /// `owner/name` on Hugging Face; empty for a file from this computer.
    pub repo: String,
    /// Its path in the repository; for a file from this computer, where it is.
    pub path: String,
    pub size_bytes: u64,
    /// The id of its licence among the model's licences; empty for a file from this computer.
    pub licence_id: String,
    /// Its file is already in place (another model's shared part): checked, not fetched.
    pub already_here: bool,
    /// A file from this computer (the person's own), used where it is.
    pub local: bool,
}

/// Whether a model fits this computer at one size, and how long a run takes here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFitDto {
    /// `fits`, `slow`, `afterUnload` or `notHere`.
    pub word: String,
    /// The colour of the fit it would run with: `green` (whole on the device), `yellow` (one
    /// part at a time, or partly on the processor) or `red`.
    pub verdict: String,
    /// Why not, when `word` is `notHere` ("needs 6 GB of graphics memory").
    pub reason: Option<String>,
    /// The largest preset size that fits, when this size does not ("Fits at 832×480, 49
    /// frames").
    pub suggestion: Option<String>,
    /// How long a run takes here, rounded as people read it; `None` before anything was
    /// measured ("Time shown after your first run here").
    pub eta: Option<EtaDto>,
    /// Not here, or a run would take longer than 15 minutes: Studio offers a cloud model.
    pub very_slow: bool,
    /// A video that would run on the processor (this computer runs models there): very slow.
    pub video_on_processor: bool,
}

/// A model of the recommended list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalEntryDto {
    /// Its catalog id (`studio_local_download`'s `{"catalog":{"entryId":…}}`).
    pub id: String,
    pub name: String,
    /// One line about what it is good for.
    pub summary: String,
    pub publisher: String,
    pub kind: MediaKind,
    /// Every part's size: a download from nothing.
    pub size_bytes: u64,
    /// The strictest licence of its parts: it decides the badge.
    pub licence: LicenceDto,
    /// Every licence shown with the download, each once: its parts', then the Studio
    /// engine's when the engine comes with it.
    pub licences: Vec<LicenceDto>,
    pub gate: GateDto,
    /// At its default size.
    pub fit: LocalFitDto,
    /// "Recommended for this computer": one per kind at most.
    pub recommended: bool,
    pub installed: bool,
    pub parts: Vec<PartDto>,
    /// What the download fetches: the parts not already in place.
    pub to_fetch_bytes: u64,
    /// The Studio engine downloads with it (Windows, before the first model).
    pub needs_engine: bool,
    /// Less than 10 GB would be left on the disk after the download.
    pub low_disk: bool,
}

/// An installed picture or video model. One this build cannot read (its family record hidden
/// by a newer signed catalog, a part's licence no longer known, a family this build does not
/// know), and one with no recipe yet (`needs_setup`), are listed too, so they can be deleted
/// or set up: its fit `notHere` with why, and its `model` without controls, `usable` saying
/// why.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledLocalDto {
    /// Its installed id (`studio_local_delete`, `{"installed":{"genModelId":…}}`).
    pub id: String,
    pub name: String,
    pub kind: MediaKind,
    /// Its parts' total size.
    pub size_bytes: u64,
    /// At its default size.
    pub fit: LocalFitDto,
    /// As Studio's picker offers it: free, with its controls.
    pub model: MediaModelDto,
    /// The recipe it runs with (the Model type menu's words); `None` before it is set up.
    pub recipe: Option<RecipeChoiceDto>,
    /// The engine version its model file was detected as; `None` when none was.
    pub version: Option<SdVersion>,
    /// Its set-up is untested: its recipe is not one its version runs, or its version is
    /// unknown (the person chose it): a run's settings line says "untested set-up".
    pub untested: bool,
    /// It has no recipe yet: *Set it up yourself* first.
    pub needs_setup: bool,
    /// Its recipe takes a high-noise part (a two-model video).
    pub high_noise: bool,
    /// Its recipe takes LoRAs.
    pub takes_loras: bool,
    /// The family of LoRAs it takes (its version's), when its version is known.
    pub lora_base: Option<SdVersion>,
    /// Where it came from: `catalog`, `search` or `import`.
    pub source: String,
    /// A file from this computer used where it is (not Studio's copy of it): the person's
    /// own, which deleting the model never deletes.
    pub in_place: bool,
}

/// Get models (`studio_local_models`). Nothing in it comes from the network.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalModelsDto {
    pub hardware: HardwareLineDto,
    pub engine: EngineStatusDto,
    /// Newest first.
    pub installed: Vec<InstalledLocalDto>,
    /// The recommended list's entries that run on this computer.
    pub recommended: Vec<LocalEntryDto>,
    /// The entries that may not fit this computer, each with its reason: shown with the
    /// warning, never refused.
    pub not_here: Vec<LocalEntryDto>,
    /// The note about local models was accepted.
    pub notice_accepted: bool,
    /// This computer runs models on the processor: a video is made there, very slowly.
    pub video_on_processor: bool,
    /// The disk the installed picture and video models take, a shared file counted once.
    pub installed_bytes: u64,
    /// The entries of `recommended` not installed yet whose word is `fits` or `afterUnload`
    /// ("2 more fit this computer"); one that is only `slow` is not counted.
    pub more_fit: u32,
    /// 0.2.0 (Task S10): the LoRAs, newest first.
    pub loras: Vec<LoraDto>,
}

/// 0.2.0 (Task S10): a LoRA Studio's engine can apply (`studio::loras`), as Get models
/// and the LoRA picker list it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoraDto {
    /// `hf:<owner>/<repo>:<path>` for a downloaded LoRA, `local:<SHA-256>` for an imported
    /// one: what a draft attaches.
    pub id: String,
    /// Its file's stem, as written (spaces and any Unicode kept).
    pub name: String,
    /// The version of the models it was most likely made for, from its header; `None` when
    /// that cannot be told.
    pub base: Option<SdVersion>,
    pub format: LoraFormat,
    pub size_bytes: u64,
    /// `search` (downloaded from Hugging Face) or `import` (added from this computer).
    pub source: String,
    /// The Hugging Face repository it was downloaded from; `None` when imported.
    pub repo: Option<String>,
    /// Its file is gone.
    pub missing: bool,
}

/// A recipe a model can run with, as the Model type menu offers it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecipeChoiceDto {
    /// The recipe's id (`flux1-kontext`): never shown.
    pub id: String,
    /// The recipe's own name.
    pub name: String,
    pub kind: RecipeKind,
}

/// One slot of a set-up plan and the file that fills it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedSlotDto {
    pub role: GenRole,
    /// The plan cannot run without it.
    pub required: bool,
    /// Where its file comes from; `missing` when nothing fills it.
    pub source: PartSource,
    /// `None` when the main file carries the part (`embedded`) and when nothing fills it.
    pub part: Option<PartDto>,
}

/// What setting a model up takes (`studio_setup_plan`, and each file of a repository): its
/// recipe, each slot's file, the licences, the sizes and its fit here.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupPlanDto {
    pub recipe: RecipeChoiceDto,
    /// The recipes the model's version can run with, the default first.
    pub recipes: Vec<RecipeChoiceDto>,
    /// In the recipe's slot order.
    pub slots: Vec<PlannedSlotDto>,
    /// Every licence shown with the download, each once: its parts', then the Studio
    /// engine's when the engine comes with it.
    pub licences: Vec<LicenceDto>,
    /// Every part's size.
    pub total_bytes: u64,
    /// The parts not already on this computer: what is fetched.
    pub to_fetch_bytes: u64,
    /// At the recipe's default size.
    pub fit: LocalFitDto,
    /// Every slot the plan needs has a file.
    pub can_download: bool,
    /// Less than 10 GB would be left on the disk after the download.
    pub low_disk: bool,
    /// The Studio engine downloads with it.
    pub needs_engine: bool,
    /// The recipe was never matched to the model (the person chose it).
    pub untested: bool,
}

/// An installed model's files (`studio_model_files`, *Set it up yourself*): its recipe, the
/// recipes it can be set up as, and each slot with its file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelFilesDto {
    pub gen_model_id: String,
    /// `None` for a model with no recipe yet.
    pub recipe: Option<RecipeChoiceDto>,
    /// The recipes its version runs, the default first; for an untested set-up, or a model of
    /// no recipe yet, every recipe Studio runs, by kind and then name.
    pub recipes: Vec<RecipeChoiceDto>,
    /// In the recipe's slot order: its model file `main`, its other files `installed`, a
    /// file the person chose `chosen`, one to fetch from where it comes. With no recipe yet,
    /// its own files.
    pub slots: Vec<PlannedSlotDto>,
    /// The recipe is not one its version runs, or its version is unknown.
    pub untested: bool,
    /// Every slot its recipe needs has a file.
    pub can_run: bool,
}

/// What *Set it up yourself* did (`studio_set_model_files`): the model's files as they are
/// now, or as the download that brings the missing ones makes them (`download_group`; the
/// model keeps its old set-up until it is done).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetModelFilesDto {
    pub files: ModelFilesDto,
    pub download_group: Option<String>,
}

/// A file of a repository with what Studio found it is.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFileDto {
    pub path: String,
    pub size_bytes: u64,
    /// Its quantisation (`Q4_K_M`), when its name says.
    pub quant: Option<String>,
    pub label: FileLabel,
}

/// A main file of a searched repository that works in Studio, with its set-up plan.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalFileChoiceDto {
    /// Its path in the repository (`studio_setup_plan`'s and `studio_local_download`'s
    /// `{"search":{…,"file":…}}`); a diffusers folder's UNet file.
    pub file: String,
    /// Its quantisation (`Q4_K_M`), when its name says; `fp16` or `fp32` for a diffusers
    /// folder.
    pub quant: String,
    pub label: FileLabel,
    /// With nothing chosen: the file's own recipe and the files the planner picks.
    pub plan: SetupPlanDto,
    /// "Best for this computer": the largest whose fit word is `fits` or `afterUnload`.
    pub best: bool,
}

/// A searched repository at its latest commit, as Studio reads it (`studio_local_repo`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRepoDto {
    pub repo: String,
    /// The commit the listing describes; a download pins it.
    pub commit: String,
    /// The repository's owner.
    pub author: String,
    /// The model card's first paragraph, made safe; `None` when the card was not read.
    pub summary: Option<String>,
    /// From its files' labels, or its name's hint while files that could change it are not
    /// read.
    pub verdict: Verdict,
    /// One per main file that works, largest set-up first: one per quantisation (an A14B
    /// pair's low-noise half within its high-noise half's plan), a diffusers folder once.
    pub files: Vec<LocalFileChoiceDto>,
    /// The LoRAs among its files.
    pub loras: Vec<RepoFileDto>,
    /// Every file Studio labelled, by path.
    pub file_views: Vec<RepoFileDto>,
    /// The repository's own licence, as its label says.
    pub licence: Option<LicenceDto>,
    /// Every licence shown with its files, each once.
    pub licences: Vec<LicenceDto>,
    /// "The repository says MIT; FLUX.2 klein 4B is Apache 2.0." when its label is not its
    /// family's licence; `None` for a generic model, which has no family upstream.
    pub label_mismatch: Option<String>,
    pub gate: GateDto,
    /// Files whose header was read.
    pub read_files: u32,
    /// Files labelled not read.
    pub unread_files: u32,
    /// When a file of it can be set up by hand (*Set it up yourself*: weights the engine does
    /// not recognise, or a file whose header was not read for the budget or a failed read):
    /// the model types it may be set up as, every recipe Studio runs, by kind and then name.
    /// Empty otherwise.
    pub by_hand_recipes: Vec<RecipeChoiceDto>,
}

/// A picture or video repository Hugging Face found.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSearchHitDto {
    /// `owner/name`.
    pub repo: String,
    /// The owner.
    pub author: String,
    pub downloads: u64,
    pub likes: u64,
    /// When it last changed, as Hugging Face gives it.
    pub updated_at: Option<String>,
    /// `none`, `auto` or `manual`, as [`GateDto::mode`] says it.
    pub gated: String,
    /// The repository's own licence label (`apache-2.0`), as it gives it.
    pub license_label: Option<String>,
    /// From its files' labels once it was opened (`exact`), else from its name.
    pub verdict: Verdict,
    pub exact: bool,
}

/// A Hugging Face search for picture or video models: one page, matching models first.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalSearchPage {
    pub items: Vec<LocalSearchHitDto>,
}

/// What a search looks for: `pictures`, `video` or `loras`.
pub type SearchFilter = models::hf::GenSearchKind;

/// The person's choices for a set-up: another recipe, and files for some slots.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupChoice {
    /// A recipe id; `None` keeps the file's own.
    pub recipe: Option<String>,
    #[serde(default)]
    pub overrides: Vec<SlotOverride>,
}

/// A file the person chose for one slot.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotOverride {
    pub role: GenRole,
    pub file: SlotFile,
}

/// Where a chosen file is: `{"hub":{…}}`, `{"installed":{…}}`, `{"disk":{…}}` or `"none"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SlotFile {
    /// A file of a Hugging Face repository at a commit.
    Hub {
        repo: String,
        commit: String,
        path: String,
    },
    /// The file an installed model uses for `role`.
    Installed { gen_model_id: String, role: GenRole },
    /// A file on this computer: untrusted, checked before it is read.
    Disk { path: String },
    /// No file: clears an optional slot.
    None,
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

    /// Facts saved before 0.0.5 (without the expert fields) still read; the new fields are
    /// camelCase.
    #[test]
    fn file_facts_expert_fields() {
        let saved = json!({
            "architecture": "sample", "parameters": 4000000000u64, "contextLength": 32768,
            "tools": true, "thinking": false, "vision": false, "moe": false,
            "quantization": "Q4_K_M"
        });
        let f: FileFacts = serde_json::from_value(saved).unwrap();
        assert_eq!(
            (f.expert_count, f.expert_used_count, f.active_parameters),
            (None, None, None)
        );
        let moe = FileFacts {
            moe: true,
            expert_count: Some(128),
            expert_used_count: Some(8),
            active_parameters: Some(3_000_000_000),
            ..f
        };
        let v = serde_json::to_value(&moe).unwrap();
        assert_eq!(v["expertCount"], json!(128));
        assert_eq!(v["expertUsedCount"], json!(8));
        assert_eq!(v["activeParameters"], json!(3_000_000_000u64));
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
        assert_eq!(v["waitingForStudio"], json!(false));
        assert_eq!(v["unloadedPath"], json!(null));
        assert_eq!(v["stage"], json!(null));
        assert_eq!(v["placement"], json!(null));
    }
}
