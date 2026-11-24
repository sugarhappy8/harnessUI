//! The pinned engine's built-in recipes, vendored by `scripts/vendor_sd_recipes.py`.

use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

/// `catalog/sd-recipes.json`: the pinned engine's recipes, copied at its commit.
pub const SD_RECIPES: &str = include_str!("../../../../catalog/sd-recipes.json");

/// One component file a recipe can take. The first sixteen run in Studio; the last five
/// are adapters, which put a recipe out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SlotId {
    Model,
    DiffusionModel,
    HighNoiseDiffusionModel,
    UncondDiffusionModel,
    ClipL,
    ClipG,
    T5xxl,
    Llm,
    LlmVision,
    ClipVision,
    Tokenizer,
    Vae,
    Tae,
    AudioVae,
    AudioEncoder,
    EmbeddingsConnectors,
    ControlNet,
    IpAdapter,
    MotionModule,
    PhotoMaker,
    PulidWeights,
}

impl SlotId {
    /// The slots Studio can fill, in the order of design section 7.1's table.
    pub const RUNNABLE: [SlotId; 16] = [
        SlotId::Model,
        SlotId::DiffusionModel,
        SlotId::HighNoiseDiffusionModel,
        SlotId::UncondDiffusionModel,
        SlotId::ClipL,
        SlotId::ClipG,
        SlotId::T5xxl,
        SlotId::Llm,
        SlotId::LlmVision,
        SlotId::ClipVision,
        SlotId::Tokenizer,
        SlotId::Vae,
        SlotId::Tae,
        SlotId::AudioVae,
        SlotId::AudioEncoder,
        SlotId::EmbeddingsConnectors,
    ];

    /// The engine flag that takes this slot's file; `None` for the adapter slots.
    pub fn flag(self) -> Option<&'static str> {
        Some(match self {
            SlotId::Model => "-m",
            SlotId::DiffusionModel => "--diffusion-model",
            SlotId::HighNoiseDiffusionModel => "--high-noise-diffusion-model",
            SlotId::UncondDiffusionModel => "--uncond-diffusion-model",
            SlotId::ClipL => "--clip_l",
            SlotId::ClipG => "--clip_g",
            SlotId::T5xxl => "--t5xxl",
            SlotId::Llm => "--llm",
            SlotId::LlmVision => "--llm_vision",
            SlotId::ClipVision => "--clip_vision",
            SlotId::Tokenizer => "--tokenizer",
            SlotId::Vae => "--vae",
            SlotId::Tae => "--tae",
            SlotId::AudioVae => "--audio-vae",
            SlotId::AudioEncoder => "--audio-encoder",
            SlotId::EmbeddingsConnectors => "--embeddings-connectors",
            SlotId::ControlNet
            | SlotId::IpAdapter
            | SlotId::MotionModule
            | SlotId::PhotoMaker
            | SlotId::PulidWeights => return None,
        })
    }

    pub fn is_adapter(self) -> bool {
        self.flag().is_none()
    }

    /// The recipe's own id for the slot.
    pub fn word(self) -> &'static str {
        match self {
            SlotId::Model => "model",
            SlotId::DiffusionModel => "diffusion_model",
            SlotId::HighNoiseDiffusionModel => "high_noise_diffusion_model",
            SlotId::UncondDiffusionModel => "uncond_diffusion_model",
            SlotId::ClipL => "clip_l",
            SlotId::ClipG => "clip_g",
            SlotId::T5xxl => "t5xxl",
            SlotId::Llm => "llm",
            SlotId::LlmVision => "llm_vision",
            SlotId::ClipVision => "clip_vision",
            SlotId::Tokenizer => "tokenizer",
            SlotId::Vae => "vae",
            SlotId::Tae => "tae",
            SlotId::AudioVae => "audio_vae",
            SlotId::AudioEncoder => "audio_encoder",
            SlotId::EmbeddingsConnectors => "embeddings_connectors",
            SlotId::ControlNet => "control_net",
            SlotId::IpAdapter => "ip_adapter",
            SlotId::MotionModule => "motion_module",
            SlotId::PhotoMaker => "photo_maker",
            SlotId::PulidWeights => "pulid_weights",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeSlot {
    pub id: SlotId,
    #[serde(default)]
    pub label: Option<String>,
    pub required: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeDefaults {
    pub sampling_method: Option<String>,
    pub scheduler: Option<String>,
    pub cfg_scale: Option<f32>,
    pub steps: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub distilled_guidance: Option<f32>,
    pub flow_shift: Option<f32>,
    pub video_frames: Option<u32>,
    pub fps: Option<u32>,
    #[serde(default)]
    pub custom_sigmas: Vec<f32>,
    pub img_cfg_scale: Option<f32>,
    pub high_noise_steps: Option<i32>,
    pub high_noise_cfg_scale: Option<f32>,
    pub high_noise_sampling_method: Option<String>,
    pub high_noise_flow_shift: Option<f32>,
    pub moe_boundary: Option<f32>,
    pub shifted_timestep: Option<u32>,
    pub strength: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FrameRounding {
    Floor,
    Ceil,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VideoConstraints {
    pub frame_step: Option<u32>,
    pub frame_offset: Option<u32>,
    pub frame_rounding: Option<FrameRounding>,
    pub min_frames: Option<u32>,
    pub max_frames: Option<u32>,
    pub fixed_fps: Option<u32>,
}

/// A recipe's frame-count rule with the engine's defaults filled in: legal counts are
/// `frame_step * n + frame_offset` within `min_frames..=max_frames`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoRule {
    pub frame_step: u32,
    pub frame_offset: u32,
    pub rounding: FrameRounding,
    pub min_frames: u32,
    pub max_frames: u32,
    pub fixed_fps: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecipeContext {
    pub vae_format: Option<String>,
    pub model_args: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub model_url: Option<String>,
    pub slots: Vec<RecipeSlot>,
    #[serde(default)]
    pub required_any: Vec<Vec<SlotId>>,
    pub defaults: RecipeDefaults,
    #[serde(default)]
    pub features: Vec<String>,
    #[serde(default)]
    pub disabled_features: Vec<String>,
    pub size_multiple: Option<u32>,
    pub video_constraints: Option<VideoConstraints>,
    pub context: Option<RecipeContext>,
}

/// Features the engine's server turns on for every recipe (`examples/server/api.md`).
const DEFAULT_FEATURES: [&str; 7] = [
    "img2img",
    "mask_image",
    "lora",
    "vae_tiling",
    "cache",
    "cache_dit",
    "preview",
];

impl Recipe {
    /// Width and height must be a multiple of this; 8 when the recipe says nothing.
    pub fn size_multiple(&self) -> u32 {
        self.size_multiple.unwrap_or(8)
    }

    /// The frame-count rule of a video recipe, with the engine's defaults filled in.
    pub fn video(&self) -> Option<VideoRule> {
        let v = self.video_constraints.as_ref()?;
        Some(VideoRule {
            frame_step: v.frame_step.unwrap_or(1),
            frame_offset: v.frame_offset.unwrap_or(0),
            rounding: v.frame_rounding.unwrap_or(FrameRounding::Floor),
            min_frames: v.min_frames.unwrap_or(1),
            max_frames: v.max_frames.unwrap_or(4096),
            fixed_fps: v.fixed_fps,
        })
    }

    pub fn is_video(&self) -> bool {
        self.video_constraints.is_some()
    }

    /// Whether a feature is on: by default, by a slot the recipe has, or by `features`;
    /// `disabled_features` always wins.
    pub fn feature(&self, name: &str) -> bool {
        if self.disabled_features.iter().any(|f| f == name) {
            return false;
        }
        let by_slot = match name {
            "controlnet" => Some(SlotId::ControlNet),
            "ip_adapter" => Some(SlotId::IpAdapter),
            "high_noise" => Some(SlotId::HighNoiseDiffusionModel),
            "preview_tae" => Some(SlotId::Tae),
            _ => None,
        };
        DEFAULT_FEATURES.contains(&name)
            || by_slot.is_some_and(|id| self.slot(id).is_some())
            || self.features.iter().any(|f| f == name)
    }

    pub fn slot(&self, id: SlotId) -> Option<&RecipeSlot> {
        self.slots.iter().find(|s| s.id == id)
    }

    /// The slot that holds the main weights: `model` for a checkpoint, else `diffusion_model`.
    pub fn main_slot(&self) -> SlotId {
        if self.slot(SlotId::Model).is_some() {
            SlotId::Model
        } else {
            SlotId::DiffusionModel
        }
    }
}

#[derive(Deserialize)]
struct RecipeFile {
    #[serde(rename = "sdCommit")]
    sd_commit: String,
    recipes: Vec<Recipe>,
}

fn file() -> &'static RecipeFile {
    static FILE: OnceLock<RecipeFile> = OnceLock::new();
    FILE.get_or_init(|| {
        // Compiled in: a broken file must not ship, so a parse error is a panic.
        let mut f: RecipeFile =
            serde_json::from_str(SD_RECIPES).expect("catalog/sd-recipes.json parses");
        f.recipes.sort_by(|a, b| a.id.cmp(&b.id));
        f
    })
}

/// The engine commit the recipes were copied from.
pub fn sd_commit() -> &'static str {
    &file().sd_commit
}

/// Every recipe, sorted by id.
pub fn recipes() -> &'static [Recipe] {
    &file().recipes
}

pub fn recipe(id: &str) -> Option<&'static Recipe> {
    recipes().iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn the_recipes_name_the_pinned_engine_commit() {
        let lock: Value = serde_json::from_str(include_str!("../../../../sd.lock.json")).unwrap();
        assert_eq!(sd_commit(), lock["sd_cpp"]["commit"].as_str().unwrap());
        assert_eq!(sd_commit(), "532d2c15c430a785a712b2d94281f04e733008a4");
    }
    #[test]
    fn every_recipe_parses_with_known_fields() {
        assert_eq!(recipes().len(), 112);
        assert!(recipes().iter().all(|r| r.schema_version == 1));
        assert!(recipes().iter().all(|r| {
            r.slots
                .iter()
                .any(|s| s.required && matches!(s.id, SlotId::Model | SlotId::DiffusionModel))
        }));
    }
    #[test]
    fn slots_map_to_the_flags_of_the_design() {
        assert_eq!(
            SlotId::RUNNABLE.map(|s| s.flag().unwrap()),
            [
                "-m",
                "--diffusion-model",
                "--high-noise-diffusion-model",
                "--uncond-diffusion-model",
                "--clip_l",
                "--clip_g",
                "--t5xxl",
                "--llm",
                "--llm_vision",
                "--clip_vision",
                "--tokenizer",
                "--vae",
                "--tae",
                "--audio-vae",
                "--audio-encoder",
                "--embeddings-connectors"
            ]
        );
        for s in [
            SlotId::ControlNet,
            SlotId::IpAdapter,
            SlotId::MotionModule,
            SlotId::PhotoMaker,
            SlotId::PulidWeights,
        ] {
            assert!(s.is_adapter() && s.flag().is_none());
        }
    }
    #[test]
    fn size_multiple_defaults_to_8() {
        assert_eq!(recipe("sdxl-checkpoint").unwrap().size_multiple(), 64);
        assert_eq!(recipe("flux1-dev").unwrap().size_multiple(), 8);
        assert_eq!(recipe("chroma-radiance").unwrap().size_multiple(), 1);
    }
    #[test]
    fn video_rules_default_like_the_engine() {
        assert_eq!(
            recipe("wan22-t2v").unwrap().video(),
            Some(VideoRule {
                frame_step: 4,
                frame_offset: 1,
                rounding: FrameRounding::Floor,
                min_frames: 1,
                max_frames: 4096,
                fixed_fps: None
            })
        );
        assert_eq!(
            recipe("minimax-h3-fl2va")
                .unwrap()
                .video()
                .unwrap()
                .fixed_fps,
            Some(24)
        );
        assert_eq!(recipes().iter().filter(|r| r.is_video()).count(), 20);
        assert!(recipe("flux1-dev").unwrap().video().is_none());
    }
    #[test]
    fn features_follow_the_api_defaults() {
        let wan = recipe("wan22-t2v").unwrap();
        assert!(wan.feature("lora") && wan.feature("high_noise") && !wan.feature("img2img"));
        assert!(!recipe("z-image-l2p").unwrap().feature("vae_tiling"));
        assert!(recipe("sdxl-checkpoint").unwrap().feature("vae_tiling"));
        assert_eq!(
            recipe("wan22-t2v").unwrap().main_slot(),
            SlotId::DiffusionModel
        );
        assert_eq!(
            recipe("sdxl-checkpoint").unwrap().main_slot(),
            SlotId::Model
        );
    }
}
