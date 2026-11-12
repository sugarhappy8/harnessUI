//! One model's load settings (0.0.5 items 1 and 2): the per-model overrides, what a load
//! resolves them to, and what the settings panel is told about a model.
//!
//! WP0 wrote the types, the settings key and the fixed sample data the command stubs
//! return. WP1b owns this file from here: the resolver (per model, then the defaults for
//! all models, then the projector, then the trained-length cap, then Automatic), Automatic
//! sizing, the upgrade rule and `thinking_for(path)`.
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
#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
pub const KEY_PREFIX: &str = "modelLoad:";

/// The settings key of one model's overrides: `modelLoad:` and the first 16 hex digits of
/// SHA-256("local\n<file name>\n<size in bytes>"). Per model means per model file (D1): the
/// same file in another folder shares its settings, and the backend is not part of it.
#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

#[allow(
    dead_code,
    reason = "0.0.5 contract: WP1b's resolver and commands use it"
)]
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

/// The fixed sample the command stubs return until WP1b lands (the path and file name
/// are the caller's; every other value is a made-up example).
// TODO(WP1b): replace with the real model_load_info; remove this sample.
pub fn sample_info(path: &str) -> ModelLoadInfo {
    let file_name = std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string());
    let defaults = LocalModelSettings::default();
    let option = |choice, tokens, fit| ContextOption {
        choice,
        tokens,
        fit,
    };
    ModelLoadInfo {
        path: path.to_string(),
        file_name,
        size_bytes: 4_920_000_000,
        trained_ctx: Some(40_960),
        thinking: true,
        vision: false,
        overrides: ModelLoadSettings::default(),
        resolved: ResolvedLoad {
            ctx: defaults.ctx,
            ctx_choice: ContextSize::Fixed(defaults.ctx),
            ctx_source: ContextSource::DefaultSetting,
            gpu: defaults.gpu,
            save_memory: defaults.save_memory,
            keep_in_memory: defaults.keep_in_memory,
            thinking_limit: defaults.thinking_limit,
            backend: None,
            device: None,
            mmproj: None,
        },
        defaults,
        fit: Some(Verdict::Green),
        too_large: false,
        context_options: vec![
            option(ContextSize::Auto, 24_576, Verdict::Green),
            option(ContextSize::Fixed(4096), 4096, Verdict::Green),
            option(ContextSize::Fixed(8192), 8192, Verdict::Green),
            option(ContextSize::Fixed(16_384), 16_384, Verdict::Green),
            option(ContextSize::Fixed(32_768), 32_768, Verdict::Yellow),
            option(ContextSize::Fixed(40_960), 40_960, Verdict::Red),
        ],
        gpu_options: vec![
            GpuOption {
                gpu: GpuUse::All,
                fit: Verdict::Green,
            },
            GpuOption {
                gpu: GpuUse::Fit,
                fit: Verdict::Green,
            },
            GpuOption {
                gpu: GpuUse::Off,
                fit: Verdict::Yellow,
            },
        ],
        devices: vec![DeviceOption {
            name: "Vulkan0".into(),
            description: "Sample graphics card".into(),
            total_mib: 16_384,
            free_mib: 15_000,
            recommended: true,
        }],
        loaded: false,
        older_settings_loaded: false,
        auto_ctx: None,
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

    #[test]
    fn the_sample_is_complete_and_serialises() {
        let info = sample_info("/models/a/file.gguf");
        assert_eq!(info.file_name, "file.gguf");
        assert_eq!(info.path, "/models/a/file.gguf");
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
        assert_eq!(v["resolved"]["ctxChoice"], json!({"fixed": 8192}));
        assert_eq!(v["contextOptions"][0]["choice"], json!("auto"));
        assert_eq!(v["gpuOptions"][1]["gpu"], json!("fit"));
        assert_eq!(v["fit"], json!("green"));
    }
}
