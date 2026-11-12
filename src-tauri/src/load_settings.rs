//! One model's load settings (0.0.5 items 1 and 2): the per-model overrides, what a load
//! resolves them to, and what the settings panel is told about a model.
//!
//! The resolver: per model, then the defaults for all models, then the projector, then
//! the trained-length cap, then Automatic ([`resolve`]); Automatic sizing ([`Machine`],
//! `models::fit::auto_ctx`); the upgrade rule and a fresh install ([`defaults_from_stored`]);
//! the panel's facts ([`build_info`]) and their validation ([`validate`]); the per-request
//! thinking limit ([`thinking_for`]).
//!
//! How exact Automatic is: weights are the file sizes; the conversation memory is counted
//! for every attention layer at full length (sliding-window layers need less, so the
//! estimate is on the safe side) at 2 bytes a value, or 34 bytes per 32 values with Save
//! memory; the compute allowance is 256 MiB plus the output buffer; 1 GB stays spare, and
//! 2 GB of system memory stays for the computer. Free graphics memory comes from the
//! backend probe, cached since it last ran; the model that a load replaces is counted as
//! free (its estimated size is added back when it was loaded while the probe ran, and its
//! conversation memory is added back to system memory when it runs on the processor).
//! Measured against the pinned llama-server on the processor
//! (`the_memory_estimate_matches_llama_server`): the conversation memory per token is
//! llama.cpp's own figure, in f16 and in q8_0; the compute allowance is several times
//! what the processor backend reserves; what a graphics card reserves is not measured.
//!
//! Storage (no migration, D9): the defaults for all models stay in `localModel`
//! ([`LocalModelSettings`]); one model's overrides are in
//! [`settings_key`]`(file name, size)` as [`ModelLoadSettings`], written only by
//! `set_model_load_settings` and deleted by `reset_model_load_settings`.

use models::Verdict;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use agent::ThinkingLimit;
pub use runtime::GpuUse;

use crate::dto::LocalModelSettings;

/// The start of every per-model settings key.
pub const KEY_PREFIX: &str = "modelLoad:";

/// The settings key of one model's overrides: `modelLoad:` and the first 16 hex digits of
/// SHA-256("local\n<file name>\n<size in bytes>"). Per model means per model file (D1): the
/// same file in another folder shares its settings, and the backend is not part of it.
pub fn settings_key(file_name: &str, size: u64) -> String {
    let digest = Sha256::digest(format!("local\n{file_name}\n{size}").as_bytes());
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("{KEY_PREFIX}{hex}")
}

/// "How much it can remember" for one model. JSON: `"default"`, `"auto"`, `{"fixed": 8192}`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextChoice {
    /// Same as the defaults for all models.
    #[default]
    Default,
    /// Automatic: the largest size that fits (D2b).
    Auto,
    /// This many tokens.
    Fixed(u32),
}

/// A context choice after the defaults are applied: Automatic or a number. This is what
/// the "already loaded" check, joining a load and the "older settings" flag compare (D2b),
/// never the number Automatic produced. JSON: `"auto"`, `{"fixed": 8192}`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextSize {
    /// Automatic.
    Auto,
    /// This many tokens.
    Fixed(u32),
}

/// "Use the graphics card" for one model. JSON: `"default"`, `"all"`, `"fit"`, `"off"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GpuChoice {
    /// Same as the defaults for all models.
    #[default]
    Default,
    /// All of it.
    All,
    /// As much as fits.
    Fit,
    /// Don't use it.
    Off,
}

impl GpuChoice {
    /// The choice, with `Default` meaning `default`.
    pub fn resolve(self, default: GpuUse) -> GpuUse {
        match self {
            Self::Default => default,
            Self::All => GpuUse::All,
            Self::Fit => GpuUse::Fit,
            Self::Off => GpuUse::Off,
        }
    }
}

/// An on/off setting of one model (Save memory, Keep the model in memory).
/// JSON: `"default"`, `"on"`, `"off"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Toggle {
    /// Same as the defaults for all models.
    #[default]
    Default,
    On,
    Off,
}

impl Toggle {
    /// The setting, with `Default` meaning `default`.
    pub fn resolve(self, default: bool) -> bool {
        match self {
            Self::Default => default,
            Self::On => true,
            Self::Off => false,
        }
    }
}

/// "Which graphics card" for one model. JSON: `"default"`, `"auto"`, `{"named": "Vulkan0"}`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DeviceChoice {
    /// Same as the defaults for all models.
    #[default]
    Default,
    /// The app picks (the card with the most memory).
    Auto,
    /// This device, by the name the backend's probe reports (`Vulkan0`). Never shown.
    Named(String),
}

impl DeviceChoice {
    /// The device name to ask for, `None` meaning the app picks; `Default` means `default`.
    pub fn resolve(&self, default: Option<&str>) -> Option<String> {
        match self {
            Self::Default => default.map(str::to_owned),
            Self::Auto => None,
            Self::Named(name) => Some(name.clone()),
        }
    }
}

/// "How long it may think" for one model.
/// JSON: `"default"`, `"noLimit"`, `"short"`, `"medium"`, `"long"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingChoice {
    /// Same as the defaults for all models.
    #[default]
    Default,
    /// No limit from the app (shown as "Default" in the list of limits).
    NoLimit,
    Short,
    Medium,
    Long,
}

impl ThinkingChoice {
    /// The limit, with `Default` meaning `default`.
    pub fn resolve(self, default: ThinkingLimit) -> ThinkingLimit {
        match self {
            Self::Default => default,
            Self::NoLimit => ThinkingLimit::NoLimit,
            Self::Short => ThinkingLimit::Short,
            Self::Medium => ThinkingLimit::Medium,
            Self::Long => ThinkingLimit::Long,
        }
    }
}

/// One model's overrides (settings key [`settings_key`]). Every field defaults to
/// "same as the defaults for all models", so `{}` and a missing key mean the same.
/// JSON keys: `context`, `gpu`, `saveMemory`, `keepInMemory`, `device`, `thinkingLimit`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLoadSettings {
    #[serde(default)]
    pub context: ContextChoice,
    #[serde(default)]
    pub gpu: GpuChoice,
    #[serde(default)]
    pub save_memory: Toggle,
    #[serde(default)]
    pub keep_in_memory: Toggle,
    #[serde(default)]
    pub device: DeviceChoice,
    #[serde(default)]
    pub thinking_limit: ThinkingChoice,
}

impl ModelLoadSettings {
    /// Every field follows the defaults for all models.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Why the loaded context is the size it is (the ready line of D1:
/// "Remembers about 24,000 words (Automatic)").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ContextSource {
    /// Automatic picked it.
    Automatic,
    /// This model's own setting.
    ModelSetting,
    /// The defaults for all models.
    DefaultSetting,
    /// Capped at the length the model was trained for.
    TrainedLimit,
}

/// What a load uses, after the model's overrides, the defaults, the projector, the
/// trained-length cap and Automatic (`RuntimeManager::load`, `LoadTracker` and the
/// "already loaded" check use this, WP1b).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLoad {
    /// The context to launch with (`-c`): Automatic's number when `ctx_choice` is `Auto`.
    pub ctx: u32,
    /// The choice, compared instead of `ctx` (D2b).
    pub ctx_choice: ContextSize,
    /// Why `ctx` is what it is.
    pub ctx_source: ContextSource,
    pub gpu: GpuUse,
    pub save_memory: bool,
    pub keep_in_memory: bool,
    /// The per-request limit (WP3a), not a launch flag.
    pub thinking_limit: ThinkingLimit,
    /// The backend (global, D4). `None` means Auto.
    pub backend: Option<String>,
    /// The device name. `None` means the app picks.
    pub device: Option<String>,
    /// The image projector to load with the model.
    pub mmproj: Option<String>,
}

/// One "How much it can remember" option with its fit mark.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextOption {
    /// `Auto` or a size (sizes run up to the trained length).
    pub choice: ContextSize,
    /// The tokens it means now: Automatic's number for `Auto` (0 when nothing fits).
    pub tokens: u32,
    /// Whether it fits with the model's other settings.
    pub fit: Verdict,
}

/// One "Use the graphics card" option with its fit mark.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuOption {
    pub gpu: GpuUse,
    /// Whether the model fits this way at its resolved context.
    pub fit: Verdict,
}

/// One device for "Which graphics card" (shown only with more than one). The webview
/// labels it from `description` and `total_mib`, never from `name`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOption {
    /// The probe's name (`Vulkan0`): stored in [`DeviceChoice::Named`], never shown.
    pub name: String,
    /// The device's own name ("AMD Radeon RX 9070 XT").
    pub description: String,
    pub total_mib: u64,
    pub free_mib: u64,
    /// The one the app would pick.
    pub recommended: bool,
}

/// Everything the settings panel shows about one model file (`model_load_info`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelLoadInfo {
    pub path: String,
    pub file_name: String,
    pub size_bytes: u64,
    /// The context the model was trained for, from its header.
    pub trained_ctx: Option<u32>,
    /// The model can think (offer "How long it may think").
    pub thinking: bool,
    /// An image projector is loaded with it.
    pub vision: bool,
    /// The saved overrides; all `Default` when none are saved.
    pub overrides: ModelLoadSettings,
    /// The defaults for all models, for the "Same as default (…)" labels.
    pub defaults: LocalModelSettings,
    /// What a load would use now.
    pub resolved: ResolvedLoad,
    /// The fit mark at the resolved settings (`None` when the header cannot be read).
    pub fit: Option<Verdict>,
    /// Not even the shortest memory fits with these settings (D2b: "Too large for this
    /// computer, even with the shortest memory…").
    pub too_large: bool,
    /// Fit per context choice: Automatic first, then sizes up to the trained length.
    pub context_options: Vec<ContextOption>,
    /// Fit per GPU choice, in the order All, Fit, Off.
    pub gpu_options: Vec<GpuOption>,
    /// The devices of the chosen backend.
    pub devices: Vec<DeviceOption>,
    /// This file is the loaded model.
    pub loaded: bool,
    /// The loaded copy uses older settings than the saved ones (compares choices).
    pub older_settings_loaded: bool,
    /// The number Automatic produced for the current load, when it is loaded with
    /// Automatic.
    pub auto_ctx: Option<u32>,
}

impl ResolvedLoad {
    /// Whether a model loaded with `self` already runs the way `other` asks: what the
    /// "already loaded" check, joining a load in progress and the "older settings" flag
    /// compare (D2b). The context is compared as chosen (Automatic, never the number it
    /// produced) and the thinking limit not at all: it is a per-request limit, so changing
    /// it needs no restart.
    pub fn same_launch(&self, other: &Self) -> bool {
        self.ctx_choice == other.ctx_choice
            && self.gpu == other.gpu
            && self.save_memory == other.save_memory
            && self.keep_in_memory == other.keep_in_memory
            && self.backend == other.backend
            && self.device == other.device
            && self.mmproj == other.mmproj
    }
}

// ---- The defaults for all models: fresh install and the upgrade rule (D2) -------------

/// The 0.0.4 default context, which the 0.0.4 screen saved whenever the backend changed.
/// A stored `ctx` of exactly this, with no `contextAuto` key, reads as Automatic (D2).
pub const OLD_DEFAULT_CTX: u32 = 8192;

/// What a load uses when the model's header cannot be read, so Automatic has nothing to
/// size: the 0.0.4 default.
pub const UNKNOWN_AUTO_CTX: u32 = OLD_DEFAULT_CTX;

/// The context sizes a setting may hold (`set_local_model_settings` refuses others). A
/// stored size outside it (a damaged or hand-edited settings table) is never launched:
/// `-c 0` would mean "the trained length", which can be far more than this computer has.
pub const CTX_RANGE: std::ops::RangeInclusive<u32> = 512..=1_048_576;

/// The defaults for all models as a load reads them.
#[derive(Clone, Debug, PartialEq)]
pub struct Defaults {
    /// With `context_auto` after the upgrade rule; `mmproj` and `upgrade_note` empty.
    pub settings: LocalModelSettings,
    /// A size the upgrade rule kept (stored by 0.0.4, not 8,192): the one-time note.
    pub upgrade_note: Option<u32>,
}

/// The defaults for all models from the stored `localModel` value (D2 and the amendment
/// for a fresh install):
/// - nothing stored (a fresh install), or a value that cannot be read: Automatic;
/// - stored by 0.0.4 (no `contextAuto` key) with `ctx` exactly 8,192: Automatic;
/// - stored by 0.0.4 with any other size: that size stays, with the one-time note;
/// - stored by 0.0.5 (the key is there): as stored;
/// - a size outside [`CTX_RANGE`] (no version saves one): Automatic, the rest as stored.
pub fn defaults_from_stored(raw: Option<&serde_json::Value>) -> Defaults {
    let fresh = || Defaults {
        settings: LocalModelSettings {
            context_auto: true,
            ..LocalModelSettings::default()
        },
        upgrade_note: None,
    };
    let Some(raw) = raw else { return fresh() };
    let Ok(mut settings) = serde_json::from_value::<LocalModelSettings>(raw.clone()) else {
        return fresh();
    };
    settings.mmproj = None;
    settings.upgrade_note = None;
    if !CTX_RANGE.contains(&settings.ctx) {
        settings.ctx = OLD_DEFAULT_CTX;
        settings.context_auto = true;
        return Defaults {
            settings,
            upgrade_note: None,
        };
    }
    let mut upgrade_note = None;
    if raw.get("contextAuto").is_none() {
        if settings.ctx == OLD_DEFAULT_CTX {
            settings.context_auto = true;
        } else {
            upgrade_note = Some(settings.ctx);
        }
    }
    Defaults {
        settings,
        upgrade_note,
    }
}

/// The defaults for all models, as every load reads them.
pub async fn read_defaults(store: &store::Store) -> Defaults {
    let raw: Option<serde_json::Value> = store
        .get_setting(crate::settings::LOCAL_MODEL)
        .await
        .ok()
        .flatten();
    defaults_from_stored(raw.as_ref())
}

/// The note kept on the first start after the update, for the rest of that run.
static UPGRADE_NOTE_THIS_RUN: std::sync::Mutex<Option<u32>> = std::sync::Mutex::new(None);

/// The defaults for Settings → Local models (`get_local_model_settings`), with the
/// one-time upgrade note on the first start after the update: the size kept from 0.0.4
/// is written back with an explicit `contextAuto: false`, which ends the upgrade, and the
/// note is shown for the rest of this run (as long as that size is still the choice).
pub async fn settings_page_defaults(store: &store::Store) -> LocalModelSettings {
    page_defaults_with(store, &UPGRADE_NOTE_THIS_RUN).await
}

async fn page_defaults_with(
    store: &store::Store,
    this_run: &std::sync::Mutex<Option<u32>>,
) -> LocalModelSettings {
    let defaults = read_defaults(store).await;
    let mut settings = defaults.settings;
    if let Some(kept) = defaults.upgrade_note {
        if let Err(e) = store
            .set_setting(crate::settings::LOCAL_MODEL, &settings)
            .await
        {
            tracing::warn!("could not save the upgraded model defaults: {e}");
        }
        if let Ok(mut note) = this_run.lock() {
            *note = Some(kept);
        }
        settings.upgrade_note = Some(kept);
    } else if let Ok(note) = this_run.lock()
        && let Some(kept) = *note
        && !settings.context_auto
        && settings.ctx == kept
    {
        settings.upgrade_note = Some(kept);
    }
    settings
}

/// One model's saved overrides; all `Default` when none are saved or they cannot be read.
pub async fn read_overrides(store: &store::Store, file_name: &str, size: u64) -> ModelLoadSettings {
    let stored: ModelLoadSettings = store
        .get_setting(&settings_key(file_name, size))
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
    sane_overrides(stored)
}

/// Stored overrides as a load may use them: a fixed size outside [`CTX_RANGE`], which
/// `set_model_load_settings` never saves, follows the defaults instead. Every other value
/// is one of a fixed list when it is read, and a device name is only ever matched against
/// the probed devices.
fn sane_overrides(mut overrides: ModelLoadSettings) -> ModelLoadSettings {
    if let ContextChoice::Fixed(n) = overrides.context
        && !CTX_RANGE.contains(&n)
    {
        overrides.context = ContextChoice::Default;
    }
    overrides
}

/// Saves one model's overrides; all `Default` deletes the key ("Reset to defaults").
pub async fn save_overrides(
    store: &store::Store,
    file_name: &str,
    size: u64,
    overrides: &ModelLoadSettings,
) -> Result<(), store::StoreError> {
    let key = settings_key(file_name, size);
    if overrides.is_default() {
        store.delete_setting(&key).await.map(|_| ())
    } else {
        store.set_setting(&key, overrides).await
    }
}

/// The file name and size a model's settings are keyed by (D1).
pub fn file_identity(path: &std::path::Path) -> Option<(String, u64)> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let size = std::fs::metadata(path).ok()?.len();
    Some((name, size))
}

/// "How long it may think" for the model at `path` (WP3a's per-request limit): its own
/// choice, else the defaults for all models. Read on every request, so a change applies
/// to the next reply without a restart.
pub async fn thinking_for(state: &crate::state::AppState, path: &str) -> ThinkingLimit {
    thinking_with(&state.store, path).await
}

async fn thinking_with(store: &store::Store, path: &str) -> ThinkingLimit {
    let defaults = read_defaults(store).await.settings;
    match file_identity(std::path::Path::new(path)) {
        Some((name, size)) => read_overrides(store, &name, size)
            .await
            .thinking_limit
            .resolve(defaults.thinking_limit),
        None => defaults.thinking_limit,
    }
}

// ---- What the model file says ---------------------------------------------------------

/// What the memory estimate needs from a model's header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeaderFacts {
    /// The context the model was trained for.
    pub trained_ctx: Option<u32>,
    /// Conversation memory per token: f16, and q8_0 (Save memory).
    pub kv_f16: u64,
    pub kv_q8: u64,
    pub vocab: u64,
    /// The chat template can think.
    pub thinking: bool,
}

impl HeaderFacts {
    pub fn from_summary(s: &models::ModelSummary) -> Self {
        use models::summary::KvCacheType;
        Self {
            trained_ctx: s
                .context_length
                .map(|t| u32::try_from(t).unwrap_or(u32::MAX)),
            kv_f16: s.kv_bytes_per_token_as(KvCacheType::F16),
            kv_q8: s.kv_bytes_per_token_as(KvCacheType::Q8_0),
            vocab: s.vocab_size.unwrap_or(0),
            thinking: s.template_thinking,
        }
    }
}

/// One model file as the resolver sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelFacts {
    pub file_name: String,
    /// This file's size (with the name, the settings key).
    pub size_bytes: u64,
    /// Every part of a split model.
    pub weights_bytes: u64,
    /// The image projector loaded with it.
    pub projector_bytes: u64,
    /// `None` when the header cannot be read.
    pub header: Option<HeaderFacts>,
}

impl ModelFacts {
    /// The memory estimate's input, with the conversation memory as Save memory stores
    /// it. `None` without a header.
    pub fn fit_input(&self, save_memory: bool) -> Option<models::FitInput> {
        let h = self.header.as_ref()?;
        Some(models::FitInput {
            weights_bytes: self.weights_bytes,
            projector_bytes: self.projector_bytes,
            kv_bytes_per_token: if save_memory { h.kv_q8 } else { h.kv_f16 },
            vocab_size: h.vocab,
            ctx: 0,
            max_ctx: h.trained_ctx.map(u64::from),
        })
    }

    pub fn trained_ctx(&self) -> Option<u32> {
        self.header.as_ref().and_then(|h| h.trained_ctx)
    }
}

/// The other files of a split model (`name-00001-of-00003.gguf`), first included, or
/// `None` for a single file.
fn split_parts(file_name: &str) -> Option<Vec<String>> {
    let stem = file_name.strip_suffix(".gguf")?;
    let (head, total) = stem.rsplit_once("-of-")?;
    let (prefix, index) = head.rsplit_once('-')?;
    let digits = |s: &str| s.len() == 5 && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(total) || !digits(index) {
        return None;
    }
    let count: u32 = total.parse().ok()?;
    (count > 1).then(|| {
        (1..=count)
            .map(|i| format!("{prefix}-{i:05}-of-{total}.gguf"))
            .collect()
    })
}

/// A model file as its header facts are remembered: path, size and modification time.
type HeaderKey = (std::path::PathBuf, u64, Option<std::time::SystemTime>);

/// Header facts already read. Reading a large model's header takes about a second on a
/// busy computer, and the panel, the Tools panel and every load ask again; a file that
/// changes gets another size or modification time, so it is read again.
static HEADER_CACHE: std::sync::Mutex<Vec<(HeaderKey, Option<HeaderFacts>)>> =
    std::sync::Mutex::new(Vec::new());
const HEADER_CACHE_LEN: usize = 32;

/// Reads what the resolver needs from the file at `path` (its header on a blocking
/// thread, once per file version). `None` when the file is not there.
pub async fn read_facts(path: &std::path::Path, mmproj: Option<&str>) -> Option<ModelFacts> {
    let (file_name, size_bytes) = file_identity(path)?;
    let weights_bytes = match (split_parts(&file_name), path.parent()) {
        (Some(parts), Some(dir)) => parts
            .iter()
            .filter_map(|p| std::fs::metadata(dir.join(p)).ok())
            .map(|m| m.len())
            .sum::<u64>()
            .max(size_bytes),
        _ => size_bytes,
    };
    let projector_bytes = mmproj
        .and_then(|p| std::fs::metadata(p).ok())
        .map_or(0, |m| m.len());
    let key: HeaderKey = (
        path.to_path_buf(),
        size_bytes,
        std::fs::metadata(path).and_then(|m| m.modified()).ok(),
    );
    let cached = HEADER_CACHE
        .lock()
        .ok()
        .and_then(|c| c.iter().find(|(k, _)| *k == key).map(|(_, h)| h.clone()));
    let header = match cached {
        Some(header) => header,
        None => {
            let header_path = path.to_path_buf();
            let header = tokio::task::spawn_blocking(move || {
                models::read_file(&header_path)
                    .ok()
                    .map(|h| HeaderFacts::from_summary(&models::ModelSummary::from_header(&h)))
            })
            .await
            .ok()
            .flatten();
            if let Ok(mut c) = HEADER_CACHE.lock() {
                if c.len() >= HEADER_CACHE_LEN {
                    c.remove(0);
                }
                c.push((key, header.clone()));
            }
            header
        }
    };
    Some(ModelFacts {
        file_name,
        size_bytes,
        weights_bytes,
        projector_bytes,
        header,
    })
}

// ---- This computer's memory -----------------------------------------------------------

/// Memory that a load frees by replacing the loaded model (D2b: "the memory of a model
/// that will be unloaded counted as available").
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Freed {
    /// The loaded model's estimated size on a graphics device, when it was loaded while
    /// that device's free memory was probed: (backend name, device name, bytes).
    pub gpu: Option<(String, String, u64)>,
    /// The loaded model's conversation memory and compute buffers, when it runs on the
    /// processor. Its weights are mapped from the file, which the system already counts as
    /// memory it can reclaim on Linux and macOS (not on Windows: the estimate is lower
    /// there, which is the safe side).
    pub ram_bytes: u64,
}

/// What a loaded model takes, for [`Freed`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Everything: weights, projector, conversation memory, compute.
    pub needed: u64,
    /// The conversation memory and compute buffers only.
    pub anon: u64,
}

const MIB: u64 = 1024 * 1024;

/// Where a load would run and how much memory it has there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    /// The backend a load starts on: the user's choice, or Auto's pick. `None` when none
    /// works (the room is then system memory, and the load will say why it cannot start).
    pub backend: Option<runtime::BackendKind>,
    /// Free memory of the device it would use (the Metal working set on a Mac), with the
    /// replaced model counted as free.
    pub gpu_free_bytes: u64,
    /// System memory available, with the replaced model counted as free.
    pub ram_free_bytes: u64,
    /// The devices of that backend, for "Which graphics card".
    pub devices: Vec<DeviceOption>,
}

impl Machine {
    /// The memory for one "Use the graphics card" choice.
    pub fn room(&self, gpu: GpuUse) -> models::fit::Room {
        use models::fit::Room;
        use runtime::BackendKind as K;
        match self.backend {
            Some(K::Metal) if gpu != GpuUse::Off => {
                Room::apple(self.gpu_free_bytes, self.ram_free_bytes)
            }
            Some(K::Vulkan | K::Cuda | K::Rocm) if gpu != GpuUse::Off => {
                Room::graphics_card(self.gpu_free_bytes, self.ram_free_bytes)
            }
            _ => Room::processor(self.ram_free_bytes),
        }
    }

    /// The machine for a load with `backend` (`None`: Auto) and `device` (`None`: the app
    /// picks), from the backend probes and the system's available memory.
    pub fn from_probes(
        found: &[(runtime::Backend, runtime::ProbeResult)],
        backend: Option<&str>,
        device: Option<&str>,
        ram_available: u64,
        freed: &Freed,
    ) -> Self {
        let ram_free_bytes = ram_available.saturating_add(freed.ram_bytes);
        let Some(kind) = crate::runtime_manager::planned_backend(found, backend, device) else {
            return Self {
                ram_free_bytes,
                ..Self::default()
            };
        };
        let probed: &[runtime::Device] = found
            .iter()
            .find(|(b, p)| b.kind == kind && p.is_usable())
            .map_or(&[], |(_, p)| p.devices.as_slice());
        if kind == runtime::BackendKind::Cpu {
            return Self {
                backend: Some(kind),
                ram_free_bytes,
                ..Self::default()
            };
        }
        let recommended = runtime::pick_device(probed).map(|d| d.name.clone());
        let used = device
            .and_then(|name| probed.iter().find(|d| d.name == name))
            .or_else(|| runtime::pick_device(probed));
        let gpu_free_bytes = used.map_or(0, |d| {
            let mut free = d.free_mib.saturating_mul(MIB);
            if let Some((b, name, bytes)) = &freed.gpu
                && b == kind.as_str()
                && name == &d.name
            {
                free = free.saturating_add(*bytes);
                if d.total_mib > 0 {
                    free = free.min(d.total_mib.saturating_mul(MIB));
                }
            }
            free
        });
        Self {
            backend: Some(kind),
            gpu_free_bytes,
            ram_free_bytes,
            devices: probed
                .iter()
                .map(|d| DeviceOption {
                    name: d.name.clone(),
                    description: d.description.clone(),
                    total_mib: d.total_mib,
                    free_mib: d.free_mib,
                    recommended: recommended.as_deref() == Some(d.name.as_str()),
                })
                .collect(),
        }
    }
}

// ---- The resolver -----------------------------------------------------------------------

/// The device a load asks for: the model's own choice, else the default.
pub fn device_choice(
    overrides: &ModelLoadSettings,
    defaults: &LocalModelSettings,
) -> Option<String> {
    overrides.device.resolve(defaults.device.as_deref())
}

/// Automatic's pick for `facts` in `room` (`None` without a header, or when not even the
/// floor fits).
fn automatic(
    facts: Option<&ModelFacts>,
    room: models::fit::Room,
    save_memory: bool,
) -> Option<models::fit::AutoCtx> {
    let input = facts?.fit_input(save_memory)?;
    models::fit::auto_ctx(&input, room)
}

/// The shortest memory Automatic would pick ("even with the shortest memory", D2b).
fn floor_ctx(trained: Option<u32>) -> u32 {
    let floor = models::fit::AUTO_FLOOR_CTX as u32;
    trained.map_or(floor, |t| t.min(floor))
}

/// What a load of the model uses (D1, D2): the model's own choices, else the defaults
/// for all models; the projector; a fixed size capped at the trained length; Automatic
/// sized for `machine` (built for this backend and [`device_choice`]).
pub fn resolve(
    overrides: &ModelLoadSettings,
    defaults: &LocalModelSettings,
    mmproj: Option<String>,
    facts: Option<&ModelFacts>,
    machine: &Machine,
) -> ResolvedLoad {
    let gpu = overrides.gpu.resolve(defaults.gpu);
    let save_memory = overrides.save_memory.resolve(defaults.save_memory);
    let (ctx_choice, source) = match overrides.context {
        ContextChoice::Default if defaults.context_auto => {
            (ContextSize::Auto, ContextSource::Automatic)
        }
        ContextChoice::Default => (
            ContextSize::Fixed(defaults.ctx),
            ContextSource::DefaultSetting,
        ),
        ContextChoice::Auto => (ContextSize::Auto, ContextSource::Automatic),
        ContextChoice::Fixed(n) => (ContextSize::Fixed(n), ContextSource::ModelSetting),
    };
    let trained = facts.and_then(ModelFacts::trained_ctx);
    let (ctx, ctx_source) = match ctx_choice {
        ContextSize::Fixed(n) => match trained {
            Some(t) if n > t => (t, ContextSource::TrainedLimit),
            _ => (n, source),
        },
        ContextSize::Auto => {
            let tokens = match automatic(facts, machine.room(gpu), save_memory) {
                Some(auto) => auto.ctx as u32,
                // Too large even at the floor: "Load anyway" starts with the floor.
                None if facts.is_some_and(|f| f.header.is_some()) => floor_ctx(trained),
                None => UNKNOWN_AUTO_CTX,
            };
            (tokens, ContextSource::Automatic)
        }
    };
    ResolvedLoad {
        ctx,
        ctx_choice,
        ctx_source,
        gpu,
        save_memory,
        keep_in_memory: overrides.keep_in_memory.resolve(defaults.keep_in_memory),
        thinking_limit: overrides.thinking_limit.resolve(defaults.thinking_limit),
        backend: defaults.backend.clone(),
        device: device_choice(overrides, defaults),
        mmproj,
    }
}

// ---- The panel's facts ------------------------------------------------------------------

/// The sizes "How much it can remember" offers: powers of two from 2,048, up to the
/// trained length, and the trained length itself (131,072 at most when it is unknown).
pub fn context_sizes(trained: Option<u32>) -> Vec<u32> {
    let max = trained.unwrap_or(131_072).min(1_048_576);
    let mut sizes: Vec<u32> = (11..=20).map(|p| 1u32 << p).filter(|&n| n <= max).collect();
    if trained.is_some() && !sizes.contains(&max) && max >= 512 {
        sizes.push(max);
    }
    sizes
}

/// The fit mark of a fixed size, or of Automatic, in `room`. Unknown (no header) is
/// yellow: the app cannot say.
fn mark(
    facts: &ModelFacts,
    choice: ContextSize,
    room: models::fit::Room,
    save_memory: bool,
) -> Verdict {
    let Some(input) = facts.fit_input(save_memory) else {
        return Verdict::Yellow;
    };
    match choice {
        ContextSize::Auto => {
            models::fit::auto_ctx(&input, room).map_or(Verdict::Red, |a| a.verdict)
        }
        ContextSize::Fixed(n) => models::fit::verdict_at(&input, u64::from(n), room),
    }
}

/// Everything the settings panel shows about one model file. `loaded` is what the loaded
/// copy of this file was started with, when it is the loaded model.
pub fn build_info(
    path: &str,
    facts: &ModelFacts,
    overrides: ModelLoadSettings,
    defaults: LocalModelSettings,
    resolved: ResolvedLoad,
    machine: &Machine,
    loaded: Option<&ResolvedLoad>,
) -> ModelLoadInfo {
    let room = machine.room(resolved.gpu);
    let save = resolved.save_memory;
    let trained = facts.trained_ctx();
    let input = facts.fit_input(save);
    let auto_tokens = match input.as_ref() {
        Some(i) => models::fit::auto_ctx(i, room).map_or(0, |a| a.ctx as u32),
        None => UNKNOWN_AUTO_CTX,
    };
    let mut context_options = vec![ContextOption {
        choice: ContextSize::Auto,
        tokens: auto_tokens,
        fit: mark(facts, ContextSize::Auto, room, save),
    }];
    context_options.extend(context_sizes(trained).into_iter().map(|n| ContextOption {
        choice: ContextSize::Fixed(n),
        tokens: n,
        fit: mark(facts, ContextSize::Fixed(n), room, save),
    }));
    // At its own resolved context (Automatic is sized again for each choice).
    let at_resolved = match resolved.ctx_choice {
        ContextSize::Auto => ContextSize::Auto,
        ContextSize::Fixed(_) => ContextSize::Fixed(resolved.ctx),
    };
    let gpu_options = [GpuUse::All, GpuUse::Fit, GpuUse::Off]
        .into_iter()
        .map(|gpu| GpuOption {
            gpu,
            fit: mark(facts, at_resolved, machine.room(gpu), save),
        })
        .collect();
    let fit = input.as_ref().map(|_| mark(facts, at_resolved, room, save));
    let too_large = input.as_ref().is_some_and(|i| match resolved.ctx_choice {
        ContextSize::Auto => models::fit::auto_ctx(i, room).is_none(),
        ContextSize::Fixed(_) => {
            models::fit::verdict_at(i, u64::from(floor_ctx(trained)), room) == Verdict::Red
        }
    });
    ModelLoadInfo {
        path: path.to_string(),
        file_name: facts.file_name.clone(),
        size_bytes: facts.size_bytes,
        trained_ctx: trained,
        thinking: facts.header.as_ref().is_some_and(|h| h.thinking),
        vision: resolved.mmproj.is_some(),
        overrides,
        defaults,
        fit,
        too_large,
        context_options,
        gpu_options,
        devices: machine.devices.clone(),
        loaded: loaded.is_some(),
        older_settings_loaded: loaded.is_some_and(|l| !l.same_launch(&resolved)),
        auto_ctx: loaded
            .filter(|l| l.ctx_choice == ContextSize::Auto)
            .map(|l| l.ctx),
        resolved,
    }
}

/// Checks one model's overrides before they are saved: a fixed size must be one the
/// panel offers (up to the trained length), and a named graphics card one of the
/// backend's devices. Every other value is already one of a fixed list when it is read.
pub fn validate(
    overrides: &ModelLoadSettings,
    trained: Option<u32>,
    machine: &Machine,
) -> Result<(), &'static str> {
    if let ContextChoice::Fixed(n) = overrides.context
        && !context_sizes(trained).contains(&n)
    {
        return Err("Choose one of the sizes in the list for how much it can remember.");
    }
    if let DeviceChoice::Named(name) = &overrides.device
        && !machine.devices.iter().any(|d| &d.name == name)
    {
        return Err("That graphics card is not available. Choose one from the list.");
    }
    Ok(())
}

// ---- Glue for the commands ----------------------------------------------------------------

/// Everything a load of one model needs, resolved now.
#[derive(Clone, Debug)]
pub struct Plan {
    pub resolved: ResolvedLoad,
    /// `None` when the file is not there (the load then says so).
    pub facts: Option<ModelFacts>,
    pub machine: Machine,
    pub overrides: ModelLoadSettings,
    pub defaults: LocalModelSettings,
}

impl Plan {
    /// The model's chat template can think.
    pub fn thinking(&self) -> bool {
        self.facts
            .as_ref()
            .and_then(|f| f.header.as_ref())
            .is_some_and(|h| h.thinking)
    }

    /// What the model will take once loaded, for [`Freed`].
    pub fn footprint(&self) -> Footprint {
        let Some(input) = self
            .facts
            .as_ref()
            .and_then(|f| f.fit_input(self.resolved.save_memory))
        else {
            return Footprint::default();
        };
        let ctx = u64::from(self.resolved.ctx);
        Footprint {
            needed: input.needed_at(ctx),
            anon: input
                .kv_bytes_per_token
                .saturating_mul(ctx)
                .saturating_add(input.compute_bytes()),
        }
    }
}

/// Where a [`Plan`] takes the settings from.
enum Source {
    /// The defaults for all models and the model's own overrides, as saved.
    Saved,
    /// The 0.0.4 command shape (`load_local_model` with settings): used as the defaults,
    /// without the model's own overrides.
    Explicit(LocalModelSettings),
    /// The saved defaults with these overrides instead of the saved ones (the panel's
    /// unsaved choices; nothing is saved).
    Draft(ModelLoadSettings),
}

/// Resolves a load of the model at `path`. `explicit` is the 0.0.4 command shape
/// (`load_local_model` with settings): those settings are used as the defaults and the
/// model's own overrides are not applied; `None` reads both from the store.
pub async fn plan(
    state: &crate::state::AppState,
    path: &str,
    explicit: Option<LocalModelSettings>,
) -> Plan {
    let source = match explicit {
        Some(s) => Source::Explicit(s),
        None => Source::Saved,
    };
    plan_from(state, path, source).await
}

/// [`plan`] with `overrides` in place of the model's saved ones, the saved defaults
/// otherwise: what the panel shows for choices that are not saved yet.
pub async fn plan_draft(
    state: &crate::state::AppState,
    path: &str,
    overrides: ModelLoadSettings,
) -> Plan {
    plan_from(state, path, Source::Draft(overrides)).await
}

/// The defaults and the model's overrides a load of `path` uses, from `source`.
async fn settings_for(
    store: &store::Store,
    path: &str,
    source: Source,
) -> (LocalModelSettings, ModelLoadSettings) {
    match source {
        Source::Explicit(mut s) => {
            s.mmproj = None;
            s.upgrade_note = None;
            (s, ModelLoadSettings::default())
        }
        Source::Draft(o) => (read_defaults(store).await.settings, o),
        Source::Saved => {
            let overrides = match file_identity(std::path::Path::new(path)) {
                Some((name, size)) => read_overrides(store, &name, size).await,
                None => ModelLoadSettings::default(),
            };
            (read_defaults(store).await.settings, overrides)
        }
    }
}

async fn plan_from(state: &crate::state::AppState, path: &str, source: Source) -> Plan {
    let (defaults, overrides) = settings_for(&state.store, path, source).await;
    let mmproj = crate::hub::projector_for(state, path).await;
    let model = std::path::Path::new(path);
    let facts = read_facts(model, mmproj.as_deref()).await;
    let found = state.runtime.backends(false).await;
    let ram = models::hardware::system_memory().map_or(0, |m| m.available_bytes);
    let machine = Machine::from_probes(
        &found,
        defaults.backend.as_deref(),
        device_choice(&overrides, &defaults).as_deref(),
        ram,
        &state.runtime.freed_memory(),
    );
    let resolved = resolve(&overrides, &defaults, mmproj, facts.as_ref(), &machine);
    Plan {
        resolved,
        facts,
        machine,
        overrides,
        defaults,
    }
}

/// The context the next load of `path` would ask for (the Tools panel's defaults before
/// the model is loaded).
pub async fn configured_ctx(state: &crate::state::AppState, path: &str) -> u32 {
    plan(state, path, None).await.resolved.ctx
}

/// The context Browse's memory estimates assume: the default size, or with Automatic its
/// cap, so a model that fits only with less shows the size Automatic would pick.
pub async fn browse_fit_ctx(state: &crate::state::AppState) -> u32 {
    let defaults = read_defaults(&state.store).await.settings;
    if defaults.context_auto {
        models::fit::AUTO_CAP_CTX as u32
    } else {
        defaults.ctx
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_key_is_file_name_and_size_only() {
        assert_eq!(
            settings_key("model.gguf", 4_920_000_000),
            "modelLoad:63f81cec99ade548"
        );
        assert_eq!(
            settings_key("Some-File-Q4_K_M.gguf", 123),
            "modelLoad:48414125dfef475c"
        );
        assert_ne!(
            settings_key("model.gguf", 4_920_000_001),
            settings_key("model.gguf", 4_920_000_000)
        );
    }

    /// The webview sends and receives exactly these shapes (src/lib/types.ts).
    #[test]
    fn choices_serialise_as_documented() {
        let all = ModelLoadSettings {
            context: ContextChoice::Fixed(8192),
            gpu: GpuChoice::Fit,
            save_memory: Toggle::On,
            keep_in_memory: Toggle::Off,
            device: DeviceChoice::Named("Vulkan1".into()),
            thinking_limit: ThinkingChoice::Short,
        };
        let v = serde_json::to_value(&all).unwrap();
        assert_eq!(
            v,
            json!({
                "context": {"fixed": 8192},
                "gpu": "fit",
                "saveMemory": "on",
                "keepInMemory": "off",
                "device": {"named": "Vulkan1"},
                "thinkingLimit": "short"
            })
        );
        assert_eq!(serde_json::from_value::<ModelLoadSettings>(v).unwrap(), all);
        let defaults = serde_json::to_value(ModelLoadSettings::default()).unwrap();
        assert_eq!(
            defaults,
            json!({
                "context": "default",
                "gpu": "default",
                "saveMemory": "default",
                "keepInMemory": "default",
                "device": "default",
                "thinkingLimit": "default"
            })
        );
        // A missing field follows the defaults.
        let empty: ModelLoadSettings = serde_json::from_value(json!({})).unwrap();
        assert!(empty.is_default());
        let auto: ModelLoadSettings =
            serde_json::from_value(json!({"context": "auto", "device": "auto"})).unwrap();
        assert_eq!(auto.context, ContextChoice::Auto);
        assert_eq!(auto.device, DeviceChoice::Auto);
        assert_eq!(
            serde_json::to_value(ContextSize::Auto).unwrap(),
            json!("auto")
        );
        assert_eq!(
            serde_json::to_value(ContextSize::Fixed(4096)).unwrap(),
            json!({"fixed": 4096})
        );
        assert_eq!(
            serde_json::to_value(ContextSource::TrainedLimit).unwrap(),
            json!("trainedLimit")
        );
        // Values outside the fixed lists are refused when read.
        for bad in [
            json!({"gpu": "half"}),
            json!({"context": {"fixed": "8192"}}),
            json!({"saveMemory": true}),
            json!({"thinkingLimit": "forever"}),
        ] {
            assert!(
                serde_json::from_value::<ModelLoadSettings>(bad.clone()).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn default_means_the_defaults_for_all_models() {
        assert_eq!(GpuChoice::Default.resolve(GpuUse::Fit), GpuUse::Fit);
        assert_eq!(GpuChoice::Off.resolve(GpuUse::All), GpuUse::Off);
        assert_eq!(GpuChoice::All.resolve(GpuUse::Off), GpuUse::All);
        assert_eq!(GpuChoice::Fit.resolve(GpuUse::All), GpuUse::Fit);
        assert!(Toggle::Default.resolve(true));
        assert!(!Toggle::Default.resolve(false));
        assert!(Toggle::On.resolve(false));
        assert!(!Toggle::Off.resolve(true));
        assert_eq!(
            DeviceChoice::Default.resolve(Some("Vulkan1")).as_deref(),
            Some("Vulkan1")
        );
        assert_eq!(DeviceChoice::Auto.resolve(Some("Vulkan1")), None);
        assert_eq!(
            DeviceChoice::Named("Vulkan0".into())
                .resolve(None)
                .as_deref(),
            Some("Vulkan0")
        );
        assert_eq!(
            ThinkingChoice::Default.resolve(ThinkingLimit::Long),
            ThinkingLimit::Long
        );
        assert_eq!(
            ThinkingChoice::NoLimit.resolve(ThinkingLimit::Long),
            ThinkingLimit::NoLimit
        );
        assert_eq!(
            ThinkingChoice::Medium.resolve(ThinkingLimit::NoLimit),
            ThinkingLimit::Medium
        );
    }

    // ---- 0.0.5 WP1b ---------------------------------------------------------------------

    const GB: u64 = 1_000_000_000;

    /// A folder of its own under the system's temporary folder, removed afterwards.
    struct Scratch(std::path::PathBuf);

    impl Scratch {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("load-settings-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// An 8B-class model at Q4_K_M, trained for 40,960 tokens, that can think.
    fn facts() -> ModelFacts {
        ModelFacts {
            file_name: "model-Q4_K_M.gguf".into(),
            size_bytes: 4_900_000_000,
            weights_bytes: 4_900_000_000,
            projector_bytes: 0,
            header: Some(HeaderFacts {
                trained_ctx: Some(40_960),
                kv_f16: 131_072,
                kv_q8: 69_632,
                vocab: 128_256,
                thinking: true,
            }),
        }
    }

    fn backend(kind: runtime::BackendKind) -> runtime::Backend {
        runtime::Backend {
            kind,
            dir: std::path::PathBuf::from("/b"),
            server: std::path::PathBuf::from("/b/llama-server"),
            fit_params: None,
            build: "b1".into(),
            source: runtime::BackendSource::Installed,
        }
    }

    fn probe(
        kind: runtime::BackendKind,
        devices: &[(&str, &str, u64, u64)],
    ) -> (runtime::Backend, runtime::ProbeResult) {
        (
            backend(kind),
            runtime::ProbeResult {
                backend_kind: kind,
                devices: devices
                    .iter()
                    .map(|(name, description, total, free)| runtime::Device {
                        name: (*name).into(),
                        description: (*description).into(),
                        total_mib: *total,
                        free_mib: *free,
                    })
                    .collect(),
                error: None,
                output: Vec::new(),
            },
        )
    }

    /// The owner's PC: an integrated GPU and an RX 9070 XT (16 GB, 15 GB free) on Vulkan,
    /// and the processor.
    fn pc() -> Vec<(runtime::Backend, runtime::ProbeResult)> {
        vec![
            probe(
                runtime::BackendKind::Vulkan,
                &[
                    ("Vulkan0", "AMD Radeon(TM) Graphics", 512, 400),
                    ("Vulkan1", "AMD Radeon RX 9070 XT", 16_304, 15_000),
                ],
            ),
            probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
        ]
    }

    fn auto_defaults() -> LocalModelSettings {
        LocalModelSettings {
            context_auto: true,
            ..LocalModelSettings::default()
        }
    }

    #[test]
    fn a_fresh_install_and_the_old_default_read_as_automatic() {
        // Nothing stored: Automatic, like an upgrade from the 0.0.4 default (Amendments).
        let fresh = defaults_from_stored(None);
        assert!(fresh.settings.context_auto);
        assert_eq!(fresh.upgrade_note, None);
        // 0.0.4 saved 8,192 whenever the backend changed: Automatic, and no note.
        let old = json!({"ctx": 8192, "backend": "vulkan", "device": "Vulkan1"});
        let d = defaults_from_stored(Some(&old));
        assert!(d.settings.context_auto);
        assert_eq!(d.settings.backend.as_deref(), Some("vulkan"));
        assert_eq!(d.settings.device.as_deref(), Some("Vulkan1"));
        assert_eq!(d.upgrade_note, None);
        // A value that cannot be read: the same as a fresh install.
        assert!(
            defaults_from_stored(Some(&json!({"ctx": "big"})))
                .settings
                .context_auto
        );
        assert!(
            defaults_from_stored(Some(&json!([1, 2])))
                .settings
                .context_auto
        );
    }

    #[test]
    fn any_other_old_size_stays_with_the_one_time_note() {
        let old = json!({"ctx": 16384, "backend": null, "device": null});
        let d = defaults_from_stored(Some(&old));
        assert!(!d.settings.context_auto);
        assert_eq!(d.settings.ctx, 16_384);
        assert_eq!(d.upgrade_note, Some(16_384));
        // Saved by 0.0.5: as stored, 8,192 included, and never a note.
        for (stored, auto) in [
            (
                json!({"ctx": 8192, "backend": null, "device": null, "contextAuto": false}),
                false,
            ),
            (
                json!({"ctx": 8192, "backend": null, "device": null, "contextAuto": true}),
                true,
            ),
            (
                json!({"ctx": 16384, "backend": null, "device": null, "contextAuto": false}),
                false,
            ),
        ] {
            let d = defaults_from_stored(Some(&stored));
            assert_eq!(d.settings.context_auto, auto, "{stored}");
            assert_eq!(d.upgrade_note, None, "{stored}");
        }
        // A stored projector or note is never used.
        let d = defaults_from_stored(Some(
            &json!({"ctx": 4096, "backend": null, "device": null, "mmproj": "/x", "upgradeNote": 3}),
        ));
        assert_eq!((d.settings.mmproj, d.settings.upgrade_note), (None, None));
    }

    /// A size no version of the app saves (a damaged or hand-edited settings table) is
    /// never launched: `-c 0` would mean the whole trained length. It reads as Automatic,
    /// with the rest as stored and no note.
    #[test]
    fn a_stored_size_outside_the_range_reads_as_automatic() {
        for stored in [
            json!({"ctx": 0, "backend": "vulkan", "device": "Vulkan1"}),
            json!({"ctx": 100, "backend": "vulkan", "device": "Vulkan1", "contextAuto": false}),
            json!({"ctx": 4_000_000, "backend": "vulkan", "device": "Vulkan1", "contextAuto": false, "saveMemory": true}),
        ] {
            let d = defaults_from_stored(Some(&stored));
            assert!(d.settings.context_auto, "{stored}");
            assert_eq!(d.settings.ctx, OLD_DEFAULT_CTX, "{stored}");
            assert_eq!(d.upgrade_note, None, "{stored}");
            assert_eq!(d.settings.backend.as_deref(), Some("vulkan"), "{stored}");
            assert_eq!(d.settings.device.as_deref(), Some("Vulkan1"), "{stored}");
        }
        let d = defaults_from_stored(Some(
            &json!({"ctx": 4_000_000, "saveMemory": true, "contextAuto": false}),
        ));
        assert!(d.settings.save_memory, "the other fields stay as stored");
        // The ends of the range are sizes a user can save.
        for ctx in [512, 1_048_576] {
            let d = defaults_from_stored(Some(&json!({"ctx": ctx, "contextAuto": false})));
            assert_eq!((d.settings.ctx, d.settings.context_auto), (ctx, false));
        }
    }

    /// The same for one model's stored overrides: a fixed size outside the range follows
    /// the defaults; overrides that cannot be read at all are all "same as default".
    #[tokio::test]
    async fn stored_overrides_outside_the_range_follow_the_defaults() {
        let store = store::Store::open_in_memory().unwrap();
        let key = settings_key("m.gguf", 16);
        for (stored, context) in [
            (
                json!({"context": {"fixed": 0}, "saveMemory": "on"}),
                ContextChoice::Default,
            ),
            (
                json!({"context": {"fixed": 4_000_000}, "saveMemory": "on"}),
                ContextChoice::Default,
            ),
            (
                json!({"context": {"fixed": 4096}, "saveMemory": "on"}),
                ContextChoice::Fixed(4096),
            ),
        ] {
            store.set_setting(&key, &stored).await.unwrap();
            let o = read_overrides(&store, "m.gguf", 16).await;
            assert_eq!(o.context, context, "{stored}");
            assert_eq!(o.save_memory, Toggle::On, "{stored}");
        }
        store
            .set_setting(&key, &json!({"gpu": "half", "saveMemory": "on"}))
            .await
            .unwrap();
        assert!(read_overrides(&store, "m.gguf", 16).await.is_default());
    }

    /// Where a load takes its settings from: as saved (the defaults and the model's own),
    /// the panel's unsaved choices instead of the model's own, or the 0.0.4 command shape
    /// as the defaults with nothing of the model's own.
    #[tokio::test]
    async fn settings_come_from_the_saved_values_a_draft_or_the_old_command() {
        let dir = Scratch::new();
        let path = dir.path().join("m.gguf");
        std::fs::write(&path, vec![0u8; 16]).unwrap();
        let path = path.to_string_lossy().into_owned();
        let store = store::Store::open_in_memory().unwrap();
        let saved_defaults = LocalModelSettings {
            ctx: 16_384,
            context_auto: false,
            gpu: GpuUse::Fit,
            ..LocalModelSettings::default()
        };
        store
            .set_setting(crate::settings::LOCAL_MODEL, &saved_defaults)
            .await
            .unwrap();
        let own = ModelLoadSettings {
            save_memory: Toggle::On,
            ..ModelLoadSettings::default()
        };
        save_overrides(&store, "m.gguf", 16, &own).await.unwrap();

        let (defaults, overrides) = settings_for(&store, &path, Source::Saved).await;
        assert_eq!((defaults.ctx, defaults.gpu), (16_384, GpuUse::Fit));
        assert_eq!(overrides, own);

        let draft = ModelLoadSettings {
            gpu: GpuChoice::Off,
            ..ModelLoadSettings::default()
        };
        let (defaults, overrides) = settings_for(&store, &path, Source::Draft(draft.clone())).await;
        assert_eq!(defaults.ctx, 16_384, "the saved defaults");
        assert_eq!(overrides, draft, "the draft, not the saved overrides");
        let stored = read_overrides(&store, "m.gguf", 16).await;
        assert_eq!(stored, own, "a draft saves nothing");

        let old = LocalModelSettings {
            ctx: 4096,
            mmproj: Some("/x/mmproj.gguf".into()),
            upgrade_note: Some(3),
            ..LocalModelSettings::default()
        };
        let (defaults, overrides) = settings_for(&store, &path, Source::Explicit(old)).await;
        assert_eq!(defaults.ctx, 4096);
        assert_eq!((defaults.mmproj, defaults.upgrade_note), (None, None));
        assert!(overrides.is_default(), "the old shape is used as given");

        // A file that is not there has no overrides of its own.
        let (_, overrides) = settings_for(&store, "/no/such/m.gguf", Source::Saved).await;
        assert!(overrides.is_default());
    }

    /// The note shows on the first start after the update (for that whole run, while the
    /// size is still the choice) and never after a restart.
    #[tokio::test]
    async fn the_upgrade_note_shows_on_the_first_start_only() {
        let store = store::Store::open_in_memory().unwrap();
        store
            .set_setting(
                crate::settings::LOCAL_MODEL,
                &json!({"ctx": 16384, "backend": "vulkan", "device": null}),
            )
            .await
            .unwrap();
        let run = std::sync::Mutex::new(None);
        let first = page_defaults_with(&store, &run).await;
        assert_eq!(first.upgrade_note, Some(16_384));
        assert!(!first.context_auto);
        assert_eq!(first.backend.as_deref(), Some("vulkan"));
        // The upgrade is over: the flag is stored now.
        let stored: serde_json::Value = store
            .get_setting(crate::settings::LOCAL_MODEL)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored["contextAuto"], json!(false));
        assert!(stored.get("upgradeNote").is_none());
        assert_eq!(read_defaults(&store).await.upgrade_note, None);
        // Read again in the same run: still shown.
        assert_eq!(
            page_defaults_with(&store, &run).await.upgrade_note,
            Some(16_384)
        );
        // The next start: gone.
        let next_run = std::sync::Mutex::new(None);
        assert_eq!(
            page_defaults_with(&store, &next_run).await.upgrade_note,
            None
        );
        // Choosing Automatic in the same run hides it too.
        store
            .set_setting(
                crate::settings::LOCAL_MODEL,
                &LocalModelSettings {
                    context_auto: true,
                    ctx: 16_384,
                    ..LocalModelSettings::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page_defaults_with(&store, &run).await.upgrade_note, None);
        // A fresh install never shows it.
        let fresh = store::Store::open_in_memory().unwrap();
        let none = page_defaults_with(&fresh, &std::sync::Mutex::new(None)).await;
        assert!(none.context_auto && none.upgrade_note.is_none());
    }

    #[test]
    fn the_resolver_takes_the_model_first_then_the_defaults() {
        let machine = Machine::from_probes(&pc(), None, None, 16 * GB, &Freed::default());
        let defaults = LocalModelSettings {
            ctx: 16_384,
            backend: Some("vulkan".into()),
            device: Some("Vulkan1".into()),
            gpu: GpuUse::Fit,
            save_memory: true,
            thinking_limit: ThinkingLimit::Long,
            ..LocalModelSettings::default()
        };
        // Nothing of its own: every value from the defaults.
        let r = resolve(
            &ModelLoadSettings::default(),
            &defaults,
            Some("/models/mmproj.gguf".into()),
            Some(&facts()),
            &machine,
        );
        assert_eq!(
            r,
            ResolvedLoad {
                ctx: 16_384,
                ctx_choice: ContextSize::Fixed(16_384),
                ctx_source: ContextSource::DefaultSetting,
                gpu: GpuUse::Fit,
                save_memory: true,
                keep_in_memory: false,
                thinking_limit: ThinkingLimit::Long,
                backend: Some("vulkan".into()),
                device: Some("Vulkan1".into()),
                mmproj: Some("/models/mmproj.gguf".into()),
            }
        );
        // Its own choices win, field by field.
        let own = ModelLoadSettings {
            context: ContextChoice::Fixed(8192),
            gpu: GpuChoice::Off,
            save_memory: Toggle::Off,
            keep_in_memory: Toggle::On,
            device: DeviceChoice::Auto,
            thinking_limit: ThinkingChoice::Short,
        };
        let r = resolve(&own, &defaults, None, Some(&facts()), &machine);
        assert_eq!(
            (r.ctx, r.ctx_choice, r.ctx_source),
            (8192, ContextSize::Fixed(8192), ContextSource::ModelSetting)
        );
        assert_eq!(
            (r.gpu, r.save_memory, r.keep_in_memory),
            (GpuUse::Off, false, true)
        );
        assert_eq!(r.thinking_limit, ThinkingLimit::Short);
        assert_eq!(r.device, None, "Auto: the app picks");
        assert_eq!(
            r.backend.as_deref(),
            Some("vulkan"),
            "the backend stays global"
        );
        // A size above the trained length is capped and says why; the choice stays.
        let big = ModelLoadSettings {
            context: ContextChoice::Fixed(65_536),
            ..ModelLoadSettings::default()
        };
        let r = resolve(&big, &defaults, None, Some(&facts()), &machine);
        assert_eq!(
            (r.ctx, r.ctx_choice, r.ctx_source),
            (
                40_960,
                ContextSize::Fixed(65_536),
                ContextSource::TrainedLimit
            )
        );
        // Automatic from the model, or from the defaults.
        let auto = ModelLoadSettings {
            context: ContextChoice::Auto,
            ..ModelLoadSettings::default()
        };
        let r = resolve(&auto, &defaults, None, Some(&facts()), &machine);
        assert_eq!(
            (r.ctx_choice, r.ctx_source),
            (ContextSize::Auto, ContextSource::Automatic)
        );
        let r = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&facts()),
            &machine,
        );
        assert_eq!(
            (r.ctx_choice, r.ctx_source),
            (ContextSize::Auto, ContextSource::Automatic)
        );
        // Without a header: a fixed size is used as it is; Automatic falls back to 8,192.
        let no_header = ModelFacts {
            header: None,
            ..facts()
        };
        let r = resolve(&big, &defaults, None, Some(&no_header), &machine);
        assert_eq!(r.ctx, 65_536);
        let r = resolve(&auto, &defaults, None, Some(&no_header), &machine);
        assert_eq!((r.ctx, r.ctx_source), (8192, ContextSource::Automatic));
        let r = resolve(&auto, &defaults, None, None, &machine);
        assert_eq!(r.ctx, 8192);
    }

    /// Automatic through the whole resolver, on the three kinds of computer, with the
    /// model's other settings.
    #[test]
    fn automatic_on_this_computer() {
        let auto = ModelLoadSettings::default();
        let resolve_on = |machine: &Machine, own: &ModelLoadSettings| {
            resolve(own, &auto_defaults(), None, Some(&facts()), machine).ctx
        };
        // The PC: Auto picks Vulkan and the RX 9070 XT (15 GB free): the cap.
        let card = Machine::from_probes(&pc(), None, None, 16 * GB, &Freed::default());
        assert_eq!(card.backend, Some(runtime::BackendKind::Vulkan));
        assert_eq!(card.gpu_free_bytes, 15_000 * MIB);
        assert_eq!(resolve_on(&card, &auto), 32_768);
        // The integrated GPU picked by hand (400 MiB): it does not fit there, so
        // Automatic sizes for part of it on the processor (16 GB available).
        let igpu = Machine::from_probes(&pc(), None, Some("Vulkan0"), 16 * GB, &Freed::default());
        assert_eq!(igpu.gpu_free_bytes, 400 * MIB);
        assert_eq!(resolve_on(&igpu, &auto), 32_768);
        let r = resolve(&auto, &auto_defaults(), None, Some(&facts()), &igpu);
        let info = build_info(
            "/m",
            &facts(),
            auto.clone(),
            auto_defaults(),
            r,
            &igpu,
            None,
        );
        assert_eq!(
            info.fit,
            Some(Verdict::Yellow),
            "part runs on the processor"
        );
        // "Don't use it": the processor with 12 GB available (10 GB after the reserve).
        let off = ModelLoadSettings {
            gpu: GpuChoice::Off,
            ..ModelLoadSettings::default()
        };
        let pc12 = Machine::from_probes(&pc(), None, None, 12 * GB, &Freed::default());
        assert_eq!(resolve_on(&pc12, &off), 26_624);
        // Save memory on the same: the cap.
        let off_saving = ModelLoadSettings {
            save_memory: Toggle::On,
            ..off.clone()
        };
        assert_eq!(resolve_on(&pc12, &off_saving), 32_768);
        // A processor-only computer.
        let cpu = Machine::from_probes(
            &[probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])],
            None,
            None,
            12 * GB,
            &Freed::default(),
        );
        assert_eq!(cpu.backend, Some(runtime::BackendKind::Cpu));
        assert!(cpu.devices.is_empty());
        assert_eq!(resolve_on(&cpu, &auto), 26_624);
        // A 16 GB Mac with Apple silicon: a 7.5 GB working set.
        let mac = Machine::from_probes(
            &[
                probe(
                    runtime::BackendKind::Metal,
                    &[("MTL0", "Apple M2", 10_922, 7_152)],
                ),
                probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)]),
            ],
            None,
            None,
            9 * GB,
            &Freed::default(),
        );
        assert_eq!(mac.backend, Some(runtime::BackendKind::Metal));
        // 7.5 GB − 6.43 GB = 1.07 GB of cache: 8,164 tokens → 7,168.
        assert_eq!(resolve_on(&mac, &auto), 7_168);
        let mac_saving = ModelLoadSettings {
            save_memory: Toggle::On,
            ..ModelLoadSettings::default()
        };
        assert_eq!(resolve_on(&mac, &mac_saving), 14_336);
    }

    /// The model a load replaces is counted as free: its size on the card when it was
    /// loaded while the card was probed, its conversation memory on the processor.
    #[test]
    fn the_replaced_model_counts_as_free() {
        // Probed with a 9 GB model on the card: 6 GB free.
        let probed = vec![probe(
            runtime::BackendKind::Vulkan,
            &[("Vulkan1", "AMD Radeon RX 9070 XT", 16_304, 6_000)],
        )];
        let freed = Freed {
            gpu: Some(("vulkan".into(), "Vulkan1".into(), 9_000 * MIB)),
            ram_bytes: 0,
        };
        let m = Machine::from_probes(&probed, None, None, 8 * GB, &freed);
        assert_eq!(m.gpu_free_bytes, 15_000 * MIB);
        // Never more than the card has.
        let much = Freed {
            gpu: Some(("vulkan".into(), "Vulkan1".into(), 90_000 * MIB)),
            ram_bytes: 0,
        };
        let m = Machine::from_probes(&probed, None, None, 8 * GB, &much);
        assert_eq!(m.gpu_free_bytes, 16_304 * MIB);
        // Another device or backend is not freed.
        let other = Freed {
            gpu: Some(("cuda".into(), "Vulkan1".into(), 9_000 * MIB)),
            ram_bytes: 0,
        };
        assert_eq!(
            Machine::from_probes(&probed, None, None, 8 * GB, &other).gpu_free_bytes,
            6_000 * MIB
        );
        // On the processor: the loaded model's memory is added to what is available.
        let cpu = [probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])];
        let freed = Freed {
            gpu: None,
            ram_bytes: 3 * GB,
        };
        let m = Machine::from_probes(&cpu, None, None, 9 * GB, &freed);
        assert_eq!(m.ram_free_bytes, 12 * GB);
        let r = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&facts()),
            &m,
        );
        assert_eq!(r.ctx, 26_624, "the same as with 12 GB available");
    }

    /// D2b: when not even the floor fits, the panel says so and "Load anyway" uses the
    /// floor.
    #[test]
    fn too_large_even_with_the_shortest_memory() {
        let small = Machine::from_probes(
            &[probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])],
            None,
            None,
            8 * GB,
            &Freed::default(),
        );
        let auto = ModelLoadSettings::default();
        let r = resolve(&auto, &auto_defaults(), None, Some(&facts()), &small);
        assert_eq!((r.ctx, r.ctx_source), (4096, ContextSource::Automatic));
        let info = build_info("/m", &facts(), auto, auto_defaults(), r, &small, None);
        assert!(info.too_large);
        assert_eq!(info.fit, Some(Verdict::Red));
        assert_eq!(info.context_options[0].choice, ContextSize::Auto);
        assert_eq!(info.context_options[0].tokens, 0);
        assert_eq!(info.context_options[0].fit, Verdict::Red);
        assert!(info.context_options.iter().all(|o| o.fit == Verdict::Red));
        // A fixed size: too large only when the floor does not fit either.
        let fixed = ModelLoadSettings {
            context: ContextChoice::Fixed(32_768),
            ..ModelLoadSettings::default()
        };
        let roomy = Machine {
            ram_free_bytes: 10 * GB,
            ..small.clone()
        };
        let r = resolve(&fixed, &auto_defaults(), None, Some(&facts()), &roomy);
        let info = build_info(
            "/m",
            &facts(),
            fixed.clone(),
            auto_defaults(),
            r,
            &roomy,
            None,
        );
        assert!(!info.too_large);
        assert_eq!(info.fit, Some(Verdict::Red), "32,768 itself does not fit");
        let r = resolve(&fixed, &auto_defaults(), None, Some(&facts()), &small);
        let info = build_info("/m", &facts(), fixed, auto_defaults(), r, &small, None);
        assert!(info.too_large);
        // A short trained length lowers the floor.
        let short = ModelFacts {
            header: Some(HeaderFacts {
                trained_ctx: Some(2048),
                ..facts().header.unwrap()
            }),
            ..facts()
        };
        assert_eq!(floor_ctx(short.trained_ctx()), 2048);
        assert_eq!(floor_ctx(None), 4096);
    }

    /// D2b: the "already loaded" check, joining a load and "older settings" compare the
    /// choices: Automatic is Automatic whatever number it produced, and the thinking limit
    /// (a per-request limit) never needs a restart.
    #[test]
    fn loads_are_compared_by_their_choices() {
        let m = Machine::from_probes(&pc(), None, None, 16 * GB, &Freed::default());
        let a = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&facts()),
            &m,
        );
        let mut b = a.clone();
        b.ctx = 12_288;
        assert!(
            a.same_launch(&b),
            "another Automatic number is the same choice"
        );
        b.thinking_limit = ThinkingLimit::Short;
        assert!(a.same_launch(&b), "the thinking limit needs no restart");
        b.ctx_source = ContextSource::TrainedLimit;
        assert!(a.same_launch(&b));
        for change in [
            |r: &mut ResolvedLoad| r.ctx_choice = ContextSize::Fixed(r.ctx),
            |r: &mut ResolvedLoad| r.gpu = GpuUse::Fit,
            |r: &mut ResolvedLoad| r.save_memory = true,
            |r: &mut ResolvedLoad| r.keep_in_memory = true,
            |r: &mut ResolvedLoad| r.backend = Some("cpu".into()),
            |r: &mut ResolvedLoad| r.device = Some("Vulkan0".into()),
            |r: &mut ResolvedLoad| r.mmproj = Some("/p.gguf".into()),
        ] {
            let mut c = a.clone();
            change(&mut c);
            assert!(!a.same_launch(&c), "{c:?}");
        }
        // The panel: loaded with Automatic, settings unchanged, then Save memory saved.
        let info = build_info(
            "/m",
            &facts(),
            ModelLoadSettings::default(),
            auto_defaults(),
            a.clone(),
            &m,
            Some(&b),
        );
        assert!(info.loaded && !info.older_settings_loaded);
        assert_eq!(
            info.auto_ctx,
            Some(12_288),
            "the number of the current load"
        );
        let saving = ModelLoadSettings {
            save_memory: Toggle::On,
            ..ModelLoadSettings::default()
        };
        let r = resolve(&saving, &auto_defaults(), None, Some(&facts()), &m);
        let info = build_info("/m", &facts(), saving, auto_defaults(), r, &m, Some(&b));
        assert!(info.older_settings_loaded);
        // Not the loaded model.
        let info = build_info(
            "/m",
            &facts(),
            ModelLoadSettings::default(),
            auto_defaults(),
            a,
            &m,
            None,
        );
        assert!(!info.loaded && !info.older_settings_loaded && info.auto_ctx.is_none());
    }

    #[test]
    fn the_panel_lists_every_choice_with_a_mark() {
        let m = Machine::from_probes(&pc(), None, None, 16 * GB, &Freed::default());
        let r = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&facts()),
            &m,
        );
        let info = build_info(
            "/models/model-Q4_K_M.gguf",
            &facts(),
            ModelLoadSettings::default(),
            auto_defaults(),
            r,
            &m,
            None,
        );
        assert_eq!(info.trained_ctx, Some(40_960));
        assert!(info.thinking && !info.vision);
        let choices: Vec<ContextSize> = info.context_options.iter().map(|o| o.choice).collect();
        assert_eq!(
            choices,
            [
                ContextSize::Auto,
                ContextSize::Fixed(2048),
                ContextSize::Fixed(4096),
                ContextSize::Fixed(8192),
                ContextSize::Fixed(16_384),
                ContextSize::Fixed(32_768),
                ContextSize::Fixed(40_960),
            ]
        );
        assert_eq!(info.context_options[0].tokens, 32_768);
        assert_eq!(info.context_options[0].fit, Verdict::Green);
        // 15 GB free: 40,960 tokens need 4.9 + 5.4 + 0.5 GB, still green.
        assert!(info.context_options.iter().all(|o| o.fit == Verdict::Green));
        let gpus: Vec<GpuUse> = info.gpu_options.iter().map(|o| o.gpu).collect();
        assert_eq!(gpus, [GpuUse::All, GpuUse::Fit, GpuUse::Off]);
        assert_eq!(
            info.gpu_options[2].fit,
            Verdict::Green,
            "the processor has 14 GB"
        );
        assert_eq!(info.fit, Some(Verdict::Green));
        assert!(!info.too_large);
        // The devices of the backend, labelled by their own names, the card recommended.
        assert_eq!(info.devices.len(), 2);
        assert_eq!(info.devices[1].description, "AMD Radeon RX 9070 XT");
        assert!(info.devices[1].recommended && !info.devices[0].recommended);
        let v = serde_json::to_value(&info).unwrap();
        for key in [
            "path",
            "fileName",
            "sizeBytes",
            "trainedCtx",
            "thinking",
            "vision",
            "overrides",
            "defaults",
            "resolved",
            "fit",
            "tooLarge",
            "contextOptions",
            "gpuOptions",
            "devices",
            "loaded",
            "olderSettingsLoaded",
            "autoCtx",
        ] {
            assert!(v.get(key).is_some(), "{key} missing: {v}");
        }
        assert_eq!(v["resolved"]["ctxChoice"], json!("auto"));
        assert_eq!(v["contextOptions"][0]["choice"], json!("auto"));
        assert_eq!(v["gpuOptions"][1]["gpu"], json!("fit"));
        // An unreadable header: no mark, and the options say "not sure".
        let unknown = ModelFacts {
            header: None,
            ..facts()
        };
        let r = resolve(
            &ModelLoadSettings::default(),
            &auto_defaults(),
            None,
            Some(&unknown),
            &m,
        );
        let info = build_info(
            "/m",
            &unknown,
            ModelLoadSettings::default(),
            auto_defaults(),
            r,
            &m,
            None,
        );
        assert_eq!(info.fit, None);
        assert!(!info.too_large);
        assert_eq!(info.context_options[0].tokens, 8192);
        assert!(
            info.context_options
                .iter()
                .all(|o| o.fit == Verdict::Yellow)
        );
        assert_eq!(
            *info.context_options.last().map(|o| &o.choice).unwrap(),
            ContextSize::Fixed(131_072)
        );
    }

    #[test]
    fn context_sizes_run_up_to_the_trained_length() {
        assert_eq!(
            context_sizes(Some(40_960)),
            [2048, 4096, 8192, 16_384, 32_768, 40_960]
        );
        assert_eq!(
            context_sizes(Some(32_768)),
            [2048, 4096, 8192, 16_384, 32_768]
        );
        assert_eq!(context_sizes(Some(1000)), [1000]);
        assert_eq!(context_sizes(Some(4_000_000)).last(), Some(&1_048_576));
        assert_eq!(context_sizes(None).last(), Some(&131_072));
    }

    #[test]
    fn overrides_are_checked_before_they_are_saved() {
        let m = Machine::from_probes(&pc(), None, None, 16 * GB, &Freed::default());
        let ok = ModelLoadSettings {
            context: ContextChoice::Fixed(16_384),
            device: DeviceChoice::Named("Vulkan1".into()),
            ..ModelLoadSettings::default()
        };
        assert_eq!(validate(&ok, Some(40_960), &m), Ok(()));
        assert_eq!(
            validate(&ModelLoadSettings::default(), Some(40_960), &m),
            Ok(())
        );
        for bad in [
            ContextChoice::Fixed(12_345),
            ContextChoice::Fixed(65_536),
            ContextChoice::Fixed(0),
        ] {
            let o = ModelLoadSettings {
                context: bad,
                ..ModelLoadSettings::default()
            };
            assert!(validate(&o, Some(40_960), &m).is_err(), "{bad:?}");
        }
        let o = ModelLoadSettings {
            device: DeviceChoice::Named("Vulkan7".into()),
            ..ModelLoadSettings::default()
        };
        assert!(validate(&o, Some(40_960), &m).is_err());
        // The processor has no graphics card to name.
        let cpu = Machine::from_probes(
            &[probe(runtime::BackendKind::Cpu, &[("CPU", "CPU", 0, 0)])],
            None,
            None,
            8 * GB,
            &Freed::default(),
        );
        let o = ModelLoadSettings {
            device: DeviceChoice::Named("CPU".into()),
            ..ModelLoadSettings::default()
        };
        assert!(validate(&o, Some(40_960), &cpu).is_err());
    }

    #[tokio::test]
    async fn overrides_are_stored_per_file_and_reset_deletes_them() {
        let store = store::Store::open_in_memory().unwrap();
        let own = ModelLoadSettings {
            save_memory: Toggle::On,
            ..ModelLoadSettings::default()
        };
        save_overrides(&store, "a.gguf", 10, &own).await.unwrap();
        assert_eq!(read_overrides(&store, "a.gguf", 10).await, own);
        assert!(read_overrides(&store, "a.gguf", 11).await.is_default());
        save_overrides(&store, "a.gguf", 10, &ModelLoadSettings::default())
            .await
            .unwrap();
        let raw: Option<serde_json::Value> = store
            .get_setting(&settings_key("a.gguf", 10))
            .await
            .unwrap();
        assert_eq!(raw, None, "all defaults deletes the key");
    }

    #[tokio::test]
    async fn the_thinking_limit_is_the_models_own_or_the_default() {
        let dir = Scratch::new();
        let path = dir.path().join("think.gguf");
        std::fs::write(&path, b"not a real model").unwrap();
        let path_str = path.to_string_lossy().into_owned();
        let store = store::Store::open_in_memory().unwrap();
        assert_eq!(
            thinking_with(&store, &path_str).await,
            ThinkingLimit::NoLimit
        );
        store
            .set_setting(
                crate::settings::LOCAL_MODEL,
                &LocalModelSettings {
                    thinking_limit: ThinkingLimit::Medium,
                    ..LocalModelSettings::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            thinking_with(&store, &path_str).await,
            ThinkingLimit::Medium
        );
        let own = ModelLoadSettings {
            thinking_limit: ThinkingChoice::Short,
            ..ModelLoadSettings::default()
        };
        save_overrides(&store, "think.gguf", 16, &own)
            .await
            .unwrap();
        assert_eq!(thinking_with(&store, &path_str).await, ThinkingLimit::Short);
        // A missing file: the default.
        assert_eq!(
            thinking_with(&store, "/no/such/file.gguf").await,
            ThinkingLimit::Medium
        );
    }

    #[tokio::test]
    async fn facts_sum_every_part_of_a_split_model() {
        assert_eq!(
            split_parts("Big-Q4_K_M-00001-of-00003.gguf"),
            Some(vec![
                "Big-Q4_K_M-00001-of-00003.gguf".to_string(),
                "Big-Q4_K_M-00002-of-00003.gguf".to_string(),
                "Big-Q4_K_M-00003-of-00003.gguf".to_string(),
            ])
        );
        assert_eq!(split_parts("model-Q4_K_M.gguf"), None);
        assert_eq!(split_parts("a-1-of-2.gguf"), None);
        assert_eq!(split_parts("a-00001-of-00001.gguf"), None);
        let dir = Scratch::new();
        for (i, len) in [(1, 100usize), (2, 50), (3, 25)] {
            std::fs::write(
                dir.path().join(format!("m-0000{i}-of-00003.gguf")),
                vec![0u8; len],
            )
            .unwrap();
        }
        std::fs::write(dir.path().join("p.gguf"), vec![0u8; 7]).unwrap();
        let first = dir.path().join("m-00001-of-00003.gguf");
        let projector = dir.path().join("p.gguf");
        let f = read_facts(&first, projector.to_str()).await.unwrap();
        assert_eq!(
            (f.size_bytes, f.weights_bytes, f.projector_bytes),
            (100, 175, 7)
        );
        assert_eq!(f.header, None, "not a real header");
        assert!(
            read_facts(&dir.path().join("none.gguf"), None)
                .await
                .is_none()
        );
    }

    /// Header facts are read once per file version: a file replaced by a real model under
    /// the same name is read again.
    #[tokio::test]
    async fn a_changed_file_is_read_again() {
        let dir = Scratch::new();
        let path = dir.path().join("swap.gguf");
        std::fs::write(&path, b"not a model").unwrap();
        assert_eq!(read_facts(&path, None).await.unwrap().header, None);
        let gguf = models::gguf::testing::Writer::new()
            .str("general.architecture", "llama")
            .u32("llama.context_length", 8192)
            .u32("llama.block_count", 2)
            .u32("llama.embedding_length", 256)
            .u32("llama.attention.head_count", 4)
            .u32("llama.attention.head_count_kv", 2)
            .build();
        std::fs::write(&path, gguf).unwrap();
        let header = read_facts(&path, None)
            .await
            .unwrap()
            .header
            .expect("read again");
        assert_eq!(header.trained_ctx, Some(8192));
        // 2 layers × 2 rows × 2 heads × 64 values: 2 bytes each, or 4 blocks of 34 bytes.
        assert_eq!(header.kv_f16, 1024);
        assert_eq!(header.kv_q8, 2 * 2 * 4 * 34);
    }

    /// How exact Automatic is, against the pinned llama-server and a real model (set
    /// `LLAMA_SERVER_BIN` and `TEST_GGUF`; skipped without them, and off Linux, where the
    /// memory is read from `/proc`): the memory a longer context takes, with and without
    /// Save memory, is what the estimate counts per token, within 10 %. On the build
    /// machine it matched llama.cpp's own figure exactly (114,688 bytes a token in f16 and
    /// 60,928 with q8_0 for the 0.6B test model).
    #[cfg(target_os = "linux")]
    #[tokio::test(flavor = "multi_thread")]
    async fn the_memory_estimate_matches_llama_server() {
        let bin = std::env::var_os("LLAMA_SERVER_BIN").filter(|v| !v.is_empty());
        let gguf = std::env::var_os("TEST_GGUF").filter(|v| !v.is_empty());
        let (Some(bin), Some(gguf)) = (bin, gguf) else {
            eprintln!("skipping: set LLAMA_SERVER_BIN and TEST_GGUF to run it");
            return;
        };
        let (bin, gguf) = (
            std::path::PathBuf::from(bin),
            std::path::PathBuf::from(gguf),
        );
        let dir = bin.parent().expect("a folder").to_path_buf();
        let layout = runtime::RuntimeLayout {
            installed_dir: None,
            packs_dir: dir.join("no-packs-here"),
            dev_override: Some(dir),
        };
        let backend = runtime::discover(&layout)
            .into_iter()
            .find(|b| b.kind == runtime::BackendKind::Cpu)
            .expect("the processor backend next to LLAMA_SERVER_BIN");
        let header = read_facts(&gguf, None)
            .await
            .and_then(|f| f.header)
            .expect("the model's header");
        assert!(header.kv_f16 > 0 && header.kv_q8 > 0);
        // The resident memory of a server started with `ctx` tokens, once it is ready.
        let rss_at = |ctx: u32, save_memory: bool| {
            let (backend, gguf) = (backend.clone(), gguf.clone());
            async move {
                let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
                let launch = runtime::LaunchConfig {
                    gpu: GpuUse::Off,
                    save_memory,
                    cache_ram_mib: 0,
                    ..runtime::LaunchConfig::new(
                        gguf,
                        ctx,
                        runtime::free_loopback_port().expect("a port"),
                        runtime::random_api_key(),
                    )
                };
                let spec = runtime::StartSpec {
                    backend,
                    launch,
                    load_timeout: std::time::Duration::from_secs(180),
                };
                let server = match runtime::start(spec, tx).await {
                    Ok(server) => server,
                    Err(e) => panic!("start failed: {e}\n{}", e.stderr_tail().join("\n")),
                };
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                let pid = server.pid().expect("a process");
                let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
                let kib: u64 = status
                    .lines()
                    .find_map(|l| l.strip_prefix("VmRSS:"))
                    .and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok())
                    .expect("VmRSS");
                server.stop().await;
                kib * 1024
            }
        };
        const MORE: u32 = 8192;
        for (save_memory, per_token) in [(false, header.kv_f16), (true, header.kv_q8)] {
            let short = rss_at(4096, save_memory).await;
            let long = rss_at(4096 + MORE, save_memory).await;
            let measured = long.saturating_sub(short) as f64;
            let estimated = (per_token * u64::from(MORE)) as f64;
            eprintln!(
                "save memory {save_memory}: {MORE} more tokens took {:.1} MiB; the estimate is {:.1} MiB",
                measured / MIB as f64,
                estimated / MIB as f64
            );
            assert!(
                (measured - estimated).abs() <= estimated * 0.1,
                "save memory {save_memory}: measured {measured} bytes, estimated {estimated}"
            );
        }
    }
}
