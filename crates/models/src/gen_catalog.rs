//! Picture and video models in the signed catalog (studio-local-design.md, "the catalog
//! entry"): the `generators`, `generatorFamilies` and `generatorTiers` keys that
//! [`Catalog`] keeps as raw JSON, read here one entry at a time.
//!
//! An entry or family record that cannot be used is hidden with a reason, never an
//! error: an unknown family, role, input, kind or quirk (a newer catalog than this
//! build), a newer Studio engine, a part that is not a pinned `.gguf` or `.safetensors`
//! file, a licence whose text is not the bundled one. The
//! chat catalog never depends on any of it.
//!
//! 0.2.0: every entry names the pinned engine's recipe it runs with and the engine version
//! its model is. A catalog entry takes both from its family; a model the catalog does not
//! list is a generic entry made from its recipe alone ([`generic_entry`]).

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::catalog::{Catalog, is_hex};
use crate::hf::{Gated, validate_repo_id, validate_repo_path};
use crate::sd::companions::CompanionTable;
use crate::sd::detect::SdVersion;
use crate::sd::matching::{RecipeInput, RecipeKind, inputs_of, kind_of};
use crate::sd::recipes::{FrameRounding, Recipe, SlotId, VideoRule};

/// What a model makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenKind {
    /// Pictures from words (and, with [`GenInput::Reference`], from pictures).
    Image,
    /// Changes to a picture the user adds.
    ImageEdit,
    /// Short clips.
    Video,
}

/// The families harnessUI knows how to run. The list is compiled in because a family
/// decides engine flags and quirks; anything else in the catalog is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum GenFamily {
    /// FLUX.2 klein 4B.
    #[serde(rename = "flux2-klein-4b")]
    Flux2Klein4b,
    /// FLUX.3 Image Klein, distilled.
    #[serde(rename = "flux3-image-klein")]
    Flux3ImageKlein,
    /// Z-Image Turbo.
    #[serde(rename = "z-image-turbo")]
    ZImageTurbo,
    /// The full Wan 2.2 TI2V-5B.
    #[serde(rename = "wan2.2-ti2v-5b")]
    Wan22Ti2v5b,
    /// A 4-step distillation of Wan 2.2 TI2V-5B.
    #[serde(rename = "wan2.2-ti2v-5b-fast")]
    Wan22Ti2v5bFast,
    /// Wan 2.1 T2V 1.3B.
    #[serde(rename = "wan2.1-t2v-1.3b")]
    Wan21T2v1_3b,
}

impl GenFamily {
    /// Every family, in declaration order.
    pub const ALL: [GenFamily; 6] = [
        GenFamily::Flux2Klein4b,
        GenFamily::Flux3ImageKlein,
        GenFamily::ZImageTurbo,
        GenFamily::Wan22Ti2v5b,
        GenFamily::Wan22Ti2v5bFast,
        GenFamily::Wan21T2v1_3b,
    ];

    /// 0.2.0: the pinned engine's recipe a model of this family runs with (an id in
    /// `catalog/sd-recipes.json`), which schema 4 gives every model 0.1.x installed. The
    /// distilled Wan 2.2 TI2V-5B runs with the full model's recipe.
    pub fn recipe_id(self) -> &'static str {
        match self {
            GenFamily::Flux2Klein4b => "flux2-klein-4b",
            GenFamily::Flux3ImageKlein => "flux3-klein",
            GenFamily::ZImageTurbo => "z-image-turbo",
            GenFamily::Wan22Ti2v5b | GenFamily::Wan22Ti2v5bFast => "wan22-ti2v-5b",
            GenFamily::Wan21T2v1_3b => "wan21-t2v",
        }
    }

    /// 0.2.0: the engine version the pinned engine detects a model of this family as.
    pub fn version(self) -> SdVersion {
        match self {
            GenFamily::Flux2Klein4b => SdVersion::Flux2Klein,
            GenFamily::Flux3ImageKlein => SdVersion::Flux3,
            GenFamily::ZImageTurbo => SdVersion::ZImage,
            GenFamily::Wan22Ti2v5b | GenFamily::Wan22Ti2v5bFast => SdVersion::Wan22Ti2v,
            GenFamily::Wan21T2v1_3b => SdVersion::Wan2,
        }
    }
}

/// What a part is to the engine; each role has one fixed engine flag, and fills one of the
/// recipe slots Studio runs ([`SlotId::RUNNABLE`]). The words of 0.1.x's roles are kept, so
/// the parts 0.1.2 saved still read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GenRole {
    /// The diffusion model on its own.
    Diffusion,
    /// The high-noise diffusion model of a two-model video family.
    HighNoiseDiffusion,
    /// The unconditional diffusion model of a family that guides with one.
    UncondDiffusion,
    /// A language model used as the text encoder.
    Llm,
    /// The vision encoder of that language model.
    LlmVision,
    /// The T5-XXL text encoder.
    T5xxl,
    /// The CLIP-L text encoder.
    ClipL,
    /// The CLIP-G text encoder.
    ClipG,
    /// The CLIP vision encoder.
    ClipVision,
    /// The tokenizer file (the one role a `.json` file may fill).
    Tokenizer,
    /// The decoder.
    Vae,
    /// The tiny decoder.
    Tae,
    /// The audio decoder.
    AudioVae,
    /// The audio encoder.
    AudioEncoder,
    /// The embeddings connectors.
    EmbeddingsConnectors,
    /// One file holding the whole model.
    Model,
}

impl GenRole {
    /// Whether the part is the model itself (an entry has exactly one).
    pub fn is_model(self) -> bool {
        matches!(self, Self::Diffusion | Self::Model)
    }

    /// The role that fills recipe slot `slot`; `None` for the adapter slots, which Studio
    /// does not run.
    pub fn from_slot(slot: SlotId) -> Option<GenRole> {
        Some(match slot {
            SlotId::Model => Self::Model,
            SlotId::DiffusionModel => Self::Diffusion,
            SlotId::HighNoiseDiffusionModel => Self::HighNoiseDiffusion,
            SlotId::UncondDiffusionModel => Self::UncondDiffusion,
            SlotId::ClipL => Self::ClipL,
            SlotId::ClipG => Self::ClipG,
            SlotId::T5xxl => Self::T5xxl,
            SlotId::Llm => Self::Llm,
            SlotId::LlmVision => Self::LlmVision,
            SlotId::ClipVision => Self::ClipVision,
            SlotId::Tokenizer => Self::Tokenizer,
            SlotId::Vae => Self::Vae,
            SlotId::Tae => Self::Tae,
            SlotId::AudioVae => Self::AudioVae,
            SlotId::AudioEncoder => Self::AudioEncoder,
            SlotId::EmbeddingsConnectors => Self::EmbeddingsConnectors,
            SlotId::ControlNet
            | SlotId::IpAdapter
            | SlotId::MotionModule
            | SlotId::PhotoMaker
            | SlotId::PulidWeights => return None,
        })
    }

    /// The recipe slot this role fills.
    pub fn slot(self) -> SlotId {
        match self {
            Self::Model => SlotId::Model,
            Self::Diffusion => SlotId::DiffusionModel,
            Self::HighNoiseDiffusion => SlotId::HighNoiseDiffusionModel,
            Self::UncondDiffusion => SlotId::UncondDiffusionModel,
            Self::ClipL => SlotId::ClipL,
            Self::ClipG => SlotId::ClipG,
            Self::T5xxl => SlotId::T5xxl,
            Self::Llm => SlotId::Llm,
            Self::LlmVision => SlotId::LlmVision,
            Self::ClipVision => SlotId::ClipVision,
            Self::Tokenizer => SlotId::Tokenizer,
            Self::Vae => SlotId::Vae,
            Self::Tae => SlotId::Tae,
            Self::AudioVae => SlotId::AudioVae,
            Self::AudioEncoder => SlotId::AudioEncoder,
            Self::EmbeddingsConnectors => SlotId::EmbeddingsConnectors,
        }
    }
}

/// A picture a model takes beside the prompt: the one vocabulary of input pictures, for
/// catalog entries and for the engine's recipes (`sd::matching::inputs_of`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GenInput {
    /// Pictures to follow or change.
    Reference,
    /// The first frame of a clip.
    StartFrame,
    /// The last frame of a clip.
    EndFrame,
    /// The picture to change (the engine's `-i`), for inpainting, Fill and Flex.2.
    Source,
    /// Which part of the picture to change (`--mask`): any picture, which the engine
    /// resizes and rounds to black and white; the white areas change.
    Mask,
    /// A control picture (`--control-image`), such as edges or depth.
    Control,
}

/// A compiled-in adjustment for one platform; never free text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Quirk {
    /// `GGML_METAL_TENSOR_DISABLE=1` on macOS.
    MetalTensorOff,
    /// Animated WebP instead of WebM for video on macOS.
    VideoWebpOnMac,
}

/// Whether a licence lets people use the outputs commercially, from the least strict
/// to the strictest (the derived order is the strictness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Commercial {
    /// Free for any use, including commercial.
    Yes,
    /// Commercial use only under a yearly revenue ([`GenLicence::revenue_limit_usd`]).
    UnderRevenue,
    /// Commercial use after registering with the publisher.
    Registration,
    /// Commercial use allowed, with use rules: the OpenRAIL licences most SD 1.x and SDXL
    /// models carry, which forbid listed harmful uses (design 11a).
    WithUseRules,
    /// The licence does not say clearly.
    Unclear,
    /// Non-commercial.
    No,
}

/// The colour of a licence's badge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Badge {
    /// Free for any use, including commercial.
    Green,
    /// Commercial use with a condition.
    Amber,
    /// Non-commercial, or check the licence.
    Red,
}

/// How a repository is gated on Hugging Face.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GateMode {
    /// Not gated.
    None,
    /// Access is granted at once after accepting.
    Auto,
    /// The publisher approves each request.
    Manual,
}

/// The gate a Hugging Face listing reports (`false`, `"auto"` or `"manual"`), in the words a
/// catalog entry's gate uses.
impl From<Gated> for GateMode {
    fn from(gated: Gated) -> Self {
        match gated {
            Gated::No => Self::None,
            Gated::Auto => Self::Auto,
            Gated::Manual => Self::Manual,
        }
    }
}

/// Which frame counts a video model takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FrameRule {
    /// 4n + 1 frames (Wan).
    #[serde(rename = "4n+1")]
    FourNPlusOne,
    /// 8n + 1 frames (LTX).
    #[serde(rename = "8n+1")]
    EightNPlusOne,
}

/// A licence one or more parts are under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenLicence {
    /// Its key in the entry's `licenses`, which parts name (`apache-2.0`).
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// The upstream page at a pinned revision.
    pub url: String,
    /// SHA-256 of the licence text: the bundled canonical text's when there is one
    /// ([`BUNDLED_LICENCE_TEXTS`]).
    pub sha256: String,
    pub commercial_outputs: Commercial,
    #[serde(default)]
    pub revenue_limit_usd: Option<u64>,
    /// Regions excluded by the licence.
    #[serde(default)]
    pub excluded_regions: Vec<String>,
    #[serde(default)]
    pub obligations: Vec<String>,
    pub plain_words: String,
}

impl GenLicence {
    /// Green when outputs are free for any use; amber with a condition (a revenue limit,
    /// a registration or use rules); red when non-commercial or unclear.
    pub fn badge(&self) -> Badge {
        match self.commercial_outputs {
            Commercial::Yes => Badge::Green,
            Commercial::UnderRevenue | Commercial::Registration | Commercial::WithUseRules => {
                Badge::Amber
            }
            Commercial::Unclear | Commercial::No => Badge::Red,
        }
    }
}

/// How strict a licence is: by [`Commercial`], then a lower revenue limit is stricter
/// (an unstated limit counts as the lowest).
pub(crate) fn strictness(l: &GenLicence) -> (Commercial, Reverse<u64>, bool) {
    let limit = match l.commercial_outputs {
        Commercial::UnderRevenue => l.revenue_limit_usd.unwrap_or(0),
        _ => 0,
    };
    (
        l.commercial_outputs,
        Reverse(limit),
        !l.excluded_regions.is_empty(),
    )
}

/// The strictest of `licences`, the first of equals.
fn strictest<'a>(licences: impl Iterator<Item = &'a GenLicence>) -> Option<&'a GenLicence> {
    licences.reduce(|best, l| {
        if strictness(l) > strictness(best) {
            l
        } else {
            best
        }
    })
}

/// One pinned file of a model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenPart {
    pub role: GenRole,
    /// `owner/name` on Hugging Face; empty for a file from this computer.
    pub repo: String,
    /// 40-hex commit; empty for a file from this computer.
    pub revision: String,
    /// Path inside the repository.
    pub path: String,
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
    /// A key of the entry's `licenses`; empty for a file from this computer (the person's
    /// own).
    pub license: String,
    /// 0.2.0: a file from this computer, where it is: its repository and revision are then
    /// empty, and its SHA-256 the one worked out when it was imported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<PathBuf>,
    /// 0.2.0: a part of a diffusers folder: its engine name in the folder
    /// (`unet/diffusion_pytorch_model.safetensors`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
}

/// What Hugging Face asks before it serves the files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenGate {
    pub mode: GateMode,
    /// What the publisher collects when access is asked for.
    #[serde(default)]
    pub collects: Vec<String>,
}

/// The settings a run starts from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenDefaults {
    pub steps: u32,
    pub cfg: f32,
    #[serde(default)]
    pub flow_shift: Option<f32>,
    /// An engine sampler word; `None` is the engine's default.
    #[serde(default)]
    pub sampler: Option<String>,
    /// An engine scheduler word; `None` is the engine's default.
    #[serde(default)]
    pub scheduler: Option<String>,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub frames: Option<u32>,
    #[serde(default)]
    pub fps: Option<u32>,
    /// 0.2.0: the distilled guidance (`--guidance`).
    #[serde(default)]
    pub guidance: Option<f32>,
    /// 0.2.0: the picture guidance of an edit model (`--img-cfg-scale`).
    #[serde(default)]
    pub img_cfg: Option<f32>,
    /// 0.2.0: the shifted timestep (`--timestep-shift`).
    #[serde(default)]
    pub timestep_shift: Option<u32>,
    /// 0.2.0: custom sigmas (`--sigmas`); empty is the scheduler's own.
    #[serde(default)]
    pub sigmas: Vec<f32>,
    /// 0.2.0: the high-noise part's sampling, for a two-model video family.
    #[serde(default)]
    pub high_noise: Option<HighNoiseDefaults>,
    /// 0.2.0: where the high-noise part hands over (`--moe-boundary`).
    #[serde(default)]
    pub moe_boundary: Option<f32>,
    /// 0.2.0: how much a picture to change is changed (`--strength`).
    #[serde(default)]
    pub strength: Option<f32>,
}

/// The high-noise part's own sampling, for a two-model video family (Wan 2.2 A14B).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HighNoiseDefaults {
    /// Its steps; -1 lets the engine split the steps at the MoE boundary.
    pub steps: i32,
    pub cfg: f32,
    /// An engine sampler word; `None` is the engine's choice.
    #[serde(default)]
    pub sampler: Option<String>,
}

/// The sizes a model takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenLimits {
    /// Width and height are multiples of this.
    pub multiple_of: u32,
    pub max_side: u32,
    #[serde(default)]
    pub frame_rule: Option<FrameRule>,
    #[serde(default)]
    pub max_frames: Option<u32>,
    /// 0.2.0: a generic video entry's frame-count rule: its recipe's.
    #[serde(default)]
    pub video: Option<VideoRule>,
}

impl GenLimits {
    /// The frame-count rule of a video model: its recipe's, else its catalog frame rule's
    /// (4n + 1: steps of 4 from 1, rounded down; 8n + 1 the same in steps of 8), up to
    /// [`max_frames`](Self::max_frames) or the engine's most; `None` for a picture model.
    pub fn video_rule(&self) -> Option<VideoRule> {
        if let Some(rule) = self.video {
            return Some(rule);
        }
        let frame_step = match self.frame_rule? {
            FrameRule::FourNPlusOne => 4,
            FrameRule::EightNPlusOne => 8,
        };
        Some(VideoRule {
            frame_step,
            frame_offset: 1,
            rounding: FrameRounding::Floor,
            min_frames: 1,
            max_frames: self.max_frames.unwrap_or(ENGINE_MAX_FRAMES),
            fixed_fps: None,
        })
    }
}

/// The most frames the engine takes when a recipe states none (`Recipe::video`).
const ENGINE_MAX_FRAMES: u32 = 4096;

/// Conservative coefficients for the fit check when no measured point applies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitFallback {
    pub act_base_bytes: u64,
    pub act_bytes_per_token: u64,
    pub vae_bytes_per_pixel_frame: u64,
    pub vae_tile_cap_bytes: u64,
    /// Pixels per latent token along each side: 16, or 32 for Wan 2.2 TI2V.
    pub token_div: u32,
    /// Frames per latent frame: 4 for Wan.
    #[serde(default)]
    pub frame_div: Option<u32>,
    /// Frames decoded at once (Wan decodes one latent frame, 4 frames, at a time);
    /// `None` decodes all of them together.
    #[serde(default)]
    pub vae_chunk_frames: Option<u32>,
}

/// A measured peak of memory for one size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryPoint {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub peak_bytes: u64,
    pub device: String,
    /// The engine build it was measured with.
    pub build: u32,
}

/// A measured run time for one size on one class of device.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeedPoint {
    /// `<backend>-<GB>` ("vulkan-8", "metal-16"), or "cpu".
    pub device_class: String,
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub load_seconds: f64,
    pub seconds_per_step: f64,
    pub decode_seconds: f64,
    /// Where and when it was measured.
    pub measured_on: String,
}

/// A picture or video model that can be offered on this build.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenEntry {
    pub id: String,
    pub name: String,
    /// One line about what it is good for.
    pub summary: String,
    /// Who made the model (not who converted it).
    pub publisher: String,
    pub kind: GenKind,
    /// The 0.1.x family a catalog entry or a 0.1.x searched model is; `None` for a generic
    /// model ([`generic_entry`]).
    pub family: Option<GenFamily>,
    /// 0.2.0: the pinned engine's recipe it runs with (an id in `catalog/sd-recipes.json`).
    /// A catalog entry's is its family's.
    #[serde(default)]
    pub recipe: String,
    /// 0.2.0: the engine version its model is, when known. A catalog entry's is its
    /// family's.
    #[serde(default)]
    pub version: Option<SdVersion>,
    /// 0.2.0: a set-up the person made themselves whose recipe was never matched to the
    /// model (*Set it up yourself*): any readable weights are taken for it.
    #[serde(default)]
    pub untested: bool,
    pub inputs: Vec<GenInput>,
    /// 0.2.0: how many pictures of each input a generic entry takes, from its recipe; empty
    /// for a catalog entry, whose `inputs` say it.
    #[serde(default)]
    pub input_counts: Vec<RecipeInput>,
    /// Exactly one `diffusion` or `model` part, plus encoders and the decoder.
    pub parts: Vec<GenPart>,
    /// Every licence a part names, by id.
    pub licenses: BTreeMap<String, GenLicence>,
    pub gate: GenGate,
    pub defaults: GenDefaults,
    pub limits: GenLimits,
    pub fallback: FitFallback,
    pub memory: Vec<MemoryPoint>,
    pub speed: Vec<SpeedPoint>,
    pub quirks: Vec<Quirk>,
    /// The oldest Studio engine build that runs it.
    pub min_build: u32,
    pub runs_on_apple: bool,
    /// The date it last passed the catalog test.
    pub tested: Option<String>,
}

impl GenEntry {
    /// Every part's size added up: what a download from nothing takes.
    pub fn total_bytes(&self) -> u64 {
        self.parts.iter().map(|p| p.size).sum()
    }

    /// The part holding the model (`diffusion` or `model`).
    ///
    /// # Panics
    /// When there is none, which [`gen_catalog`] never lets through.
    pub fn diffusion(&self) -> &GenPart {
        self.parts
            .iter()
            .find(|p| p.role.is_model())
            .expect("an entry has a model part (gen_catalog hides one without)")
    }

    /// The strictest licence any part is under: it decides the badge. An entry built by
    /// hand whose parts name none of its licences gets the strictest of them all.
    ///
    /// # Panics
    /// When the entry has no licence at all, which [`gen_catalog`] never lets through.
    pub fn effective_licence(&self) -> &GenLicence {
        let named = self
            .parts
            .iter()
            .filter_map(|p| self.licenses.get(&p.license));
        strictest(named)
            .or_else(|| strictest(self.licenses.values()))
            .expect("an entry has a licence (gen_catalog hides a part without one)")
    }
}

/// What harnessUI knows about a family: what a searched repository of it needs beside
/// its diffusion file, and the figures entries of the family take when they state none.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FamilyRecord {
    pub family: GenFamily,
    pub name: String,
    pub publisher: String,
    pub kind: GenKind,
    pub inputs: Vec<GenInput>,
    /// The licence of the family's upstream model (a key of `licenses`), which a
    /// searched diffusion file is under at least.
    pub upstream_license: String,
    /// The encoders and the decoder, never the model itself.
    pub companions: Vec<GenPart>,
    pub licenses: BTreeMap<String, GenLicence>,
    pub defaults: GenDefaults,
    pub limits: GenLimits,
    pub fallback: FitFallback,
    pub memory: Vec<MemoryPoint>,
    pub speed: Vec<SpeedPoint>,
    pub quirks: Vec<Quirk>,
    pub min_build: u32,
    pub runs_on_apple: bool,
    /// The diffusion model's parameter count band, inclusive (4B klein: 3.5e9..=4.5e9),
    /// checked when a listing states it.
    pub parameters: (u64, u64),
}

/// Recommended entries by usable memory in GB, best first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GenTiers {
    pub image: BTreeMap<u64, Vec<String>>,
    pub video: BTreeMap<u64, Vec<String>>,
}

/// An entry (by id), a family record (`family:<key>`) or an entry's place in a list of
/// the other kind (`tier:<kind>:<id>`, the entry itself still usable) left out, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hidden {
    pub id: String,
    pub reason: String,
}

/// The catalog's picture and video models as this build can use them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GenCatalog {
    pub entries: BTreeMap<String, GenEntry>,
    pub families: BTreeMap<GenFamily, FamilyRecord>,
    pub tiers: GenTiers,
    pub hidden: Vec<Hidden>,
    /// 0.2.0: the companion table a generic model's set-up takes its shared files from
    /// ([`CompanionTable::from_catalog`]); empty when it does not read.
    pub companions: CompanionTable,
}

/// The only file types a part may be (`.ckpt`, `.pt`, `.pth` and other pickle files can
/// run code): the ones Studio reads. Compared without regard to case. A `tokenizer` part may
/// also be a `.json` file ([`TOKENIZER_EXTENSION`]).
pub const ALLOWED_PART_EXTENSIONS: [&str; 3] = crate::sd::index::READ_EXTENSIONS;

/// The one more file type a `tokenizer` part may be.
pub(crate) const TOKENIZER_EXTENSION: &str = ".json";

/// Canonical licence texts bundled with the app (third_party/licenses/models/), by licence
/// id: the catalog pins their SHA-256.
pub const BUNDLED_LICENCE_TEXTS: &[(&str, &str)] = &[(
    "apache-2.0",
    include_str!("../../../third_party/licenses/models/apache-2.0.txt"),
)];

const UNKNOWN: &str = "unknown family, role, input, kind or quirk";
const NEWER_ENGINE: &str = "needs a newer Studio engine";
/// Every file type a part may be ([`ALLOWED_PART_EXTENSIONS`], and a tokenizer's `.json`).
const NOT_ALLOWED_FILE: &str = "not a .safetensors, .sft or .gguf file, or a tokenizer's .json";
const NOT_PINNED: &str = "a part is not pinned";
const LOCAL_PART: &str = "a part names a file on this computer";
const NO_LICENCE: &str = "a part has no licence";
const TEXT_MISMATCH: &str = "licence text does not match";
const NO_MODEL_PART: &str = "no model part";
const SEVERAL_MODEL_PARTS: &str = "more than one model part";
const COMPANION_IS_MODEL: &str = "a companion is a model part";
const NO_FAMILY_RECORD: &str = "its family record is missing";
const VIDEO_AMONG_PICTURES: &str = "a video model listed among the picture models";
const PICTURE_AMONG_VIDEOS: &str = "a picture model listed among the video models";

/// An entry as the catalog writes it: the figures its family gives may be left out.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GenEntryRaw {
    name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    publisher: String,
    kind: GenKind,
    family: GenFamily,
    #[serde(default)]
    inputs: Vec<GenInput>,
    parts: Vec<GenPart>,
    licenses: BTreeMap<String, GenLicence>,
    gate: GenGate,
    defaults: GenDefaults,
    #[serde(default)]
    limits: Option<GenLimits>,
    #[serde(default)]
    fallback: Option<FitFallback>,
    #[serde(default)]
    memory: Vec<MemoryPoint>,
    #[serde(default)]
    speed: Option<Vec<SpeedPoint>>,
    #[serde(default)]
    quirks: Vec<Quirk>,
    #[serde(default)]
    min_build: Option<u32>,
    #[serde(default)]
    runs_on_apple: Option<bool>,
    #[serde(default)]
    tested: Option<String>,
}

/// A family record as the catalog writes it; its family is its key.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FamilyRecordRaw {
    name: String,
    #[serde(default)]
    publisher: String,
    kind: GenKind,
    #[serde(default)]
    inputs: Vec<GenInput>,
    upstream_license: String,
    #[serde(default)]
    companions: Vec<GenPart>,
    licenses: BTreeMap<String, GenLicence>,
    defaults: GenDefaults,
    limits: GenLimits,
    fallback: FitFallback,
    #[serde(default)]
    memory: Vec<MemoryPoint>,
    #[serde(default)]
    speed: Vec<SpeedPoint>,
    #[serde(default)]
    quirks: Vec<Quirk>,
    min_build: u32,
    #[serde(default = "runs_on_apple_unless_said")]
    runs_on_apple: bool,
    parameters: (u64, u64),
}

fn runs_on_apple_unless_said() -> bool {
    true
}

/// Reads the catalog's picture and video models for an engine of `engine_build` (the app
/// passes `runtime::sd::lock::embedded().sd_cpp.build`). Never fails: what cannot be
/// used is listed in `hidden`, a family record as `family:<key>` and an entry recommended
/// among models of the other kind as `tier:<kind>:<id>`.
pub fn gen_catalog(catalog: &Catalog, engine_build: u32) -> GenCatalog {
    let mut hidden = Vec::new();
    // Every record that reads and passes its checks: entries take their figures from it
    // whatever engine it needs (an entry taking its build is then hidden for it).
    let mut records = BTreeMap::new();
    for (key, raw) in &catalog.generator_families {
        match read_family(key, raw) {
            Ok(record) => {
                records.insert(record.family, record);
            }
            Err(reason) => hidden.push(Hidden {
                id: format!("family:{key}"),
                reason,
            }),
        }
    }
    let mut entries = BTreeMap::new();
    for (id, raw) in &catalog.generators {
        match read_entry(id, raw, &records, engine_build) {
            Ok(entry) => {
                entries.insert(id.clone(), entry);
            }
            Err(reason) => hidden.push(Hidden {
                id: id.clone(),
                reason,
            }),
        }
    }
    let mut families = BTreeMap::new();
    for (family, record) in records {
        if record.min_build <= engine_build {
            families.insert(family, record);
        } else {
            hidden.push(Hidden {
                id: format!("family:{}", family_key(family)),
                reason: NEWER_ENGINE.into(),
            });
        }
    }
    let tiers = read_tiers(&catalog.generator_tiers, &entries, &mut hidden);
    let companions = CompanionTable::from_catalog(catalog).unwrap_or_else(|reason| {
        tracing::warn!(%reason, "the companion table is not used");
        CompanionTable::default()
    });
    GenCatalog {
        entries,
        families,
        tiers,
        hidden,
        companions,
    }
}

/// The reason a value did not read: an enum word this build does not know, or another
/// mistake (`what` names the thing, with serde's message).
fn unreadable(e: &serde_json::Error, what: &str) -> String {
    if e.to_string().starts_with("unknown variant") {
        UNKNOWN.into()
    } else {
        format!("not a valid {what}: {e}")
    }
}

fn family_key(family: GenFamily) -> String {
    match serde_json::to_value(family) {
        Ok(Value::String(s)) => s,
        _ => format!("{family:?}"),
    }
}

fn read_entry(
    id: &str,
    raw: &Value,
    records: &BTreeMap<GenFamily, FamilyRecord>,
    engine_build: u32,
) -> Result<GenEntry, String> {
    let raw: GenEntryRaw =
        serde_json::from_value(raw.clone()).map_err(|e| unreadable(&e, "entry"))?;
    let record = records.get(&raw.family);
    let limits = own_or_family(raw.limits, record, |r| r.limits.clone())?;
    let fallback = own_or_family(raw.fallback, record, |r| r.fallback.clone())?;
    let min_build = own_or_family(raw.min_build, record, |r| r.min_build)?;
    let speed = raw
        .speed
        .or_else(|| record.map(|r| r.speed.clone()))
        .unwrap_or_default();
    let runs_on_apple = raw
        .runs_on_apple
        .or_else(|| record.map(|r| r.runs_on_apple))
        .unwrap_or(true);
    if min_build > engine_build {
        return Err(NEWER_ENGINE.into());
    }
    let licenses = keyed(raw.licenses);
    check_parts(&raw.parts, &licenses)?;
    check_licences(&licenses)?;
    match raw.parts.iter().filter(|p| p.role.is_model()).count() {
        0 => return Err(NO_MODEL_PART.into()),
        1 => {}
        _ => return Err(SEVERAL_MODEL_PARTS.into()),
    }
    Ok(GenEntry {
        id: id.to_string(),
        name: raw.name,
        summary: raw.summary,
        publisher: raw.publisher,
        kind: raw.kind,
        family: Some(raw.family),
        recipe: raw.family.recipe_id().to_owned(),
        version: Some(raw.family.version()),
        untested: false,
        inputs: raw.inputs,
        input_counts: Vec::new(),
        parts: raw.parts,
        licenses,
        gate: raw.gate,
        defaults: raw.defaults,
        limits,
        fallback,
        memory: raw.memory,
        speed,
        quirks: raw.quirks,
        min_build,
        runs_on_apple,
        tested: raw.tested,
    })
}

/// The entry's own value, else its family record's; hidden when it has neither.
fn own_or_family<T>(
    own: Option<T>,
    record: Option<&FamilyRecord>,
    take: impl FnOnce(&FamilyRecord) -> T,
) -> Result<T, String> {
    own.or_else(|| record.map(take))
        .ok_or_else(|| NO_FAMILY_RECORD.to_string())
}

/// A family record with every check an entry gets except its engine build, which
/// [`gen_catalog`] applies after entries have taken their figures.
fn read_family(key: &str, raw: &Value) -> Result<FamilyRecord, String> {
    let family: GenFamily = serde_json::from_value(Value::String(key.to_string()))
        .map_err(|e| unreadable(&e, "family"))?;
    let raw: FamilyRecordRaw =
        serde_json::from_value(raw.clone()).map_err(|e| unreadable(&e, "family record"))?;
    let licenses = keyed(raw.licenses);
    check_parts(&raw.companions, &licenses)?;
    if !licenses.contains_key(&raw.upstream_license) {
        return Err(NO_LICENCE.into());
    }
    check_licences(&licenses)?;
    if raw.companions.iter().any(|p| p.role.is_model()) {
        return Err(COMPANION_IS_MODEL.into());
    }
    Ok(FamilyRecord {
        family,
        name: raw.name,
        publisher: raw.publisher,
        kind: raw.kind,
        inputs: raw.inputs,
        upstream_license: raw.upstream_license,
        companions: raw.companions,
        licenses,
        defaults: raw.defaults,
        limits: raw.limits,
        fallback: raw.fallback,
        memory: raw.memory,
        speed: raw.speed,
        quirks: raw.quirks,
        min_build: raw.min_build,
        runs_on_apple: raw.runs_on_apple,
        parameters: raw.parameters,
    })
}

/// Each licence with its id set to its key.
pub(crate) fn keyed(licenses: BTreeMap<String, GenLicence>) -> BTreeMap<String, GenLicence> {
    licenses
        .into_iter()
        .map(|(id, mut l)| {
            l.id.clone_from(&id);
            (id, l)
        })
        .collect()
}

/// Every part of a catalog entry or family record is an allowed file, pinned, and names one
/// of `licenses`. Only a `tokenizer` part may be a `.json` file. A catalog part never names a
/// file on this computer (`local`): that would waive its repository, revision and licence and
/// point a run at any file, so such an entry is hidden (final review, area 1, M-1). Parts
/// from this computer exist only in the person's own models, checked by the store.
pub(crate) fn check_parts(
    parts: &[GenPart],
    licenses: &BTreeMap<String, GenLicence>,
) -> Result<(), String> {
    for p in parts {
        if p.local.is_some() {
            return Err(LOCAL_PART.into());
        }
        if validate_repo_id(&p.repo).is_err() || validate_repo_path(&p.path).is_err() {
            return Err(NOT_PINNED.into());
        }
        let file = p.path.to_ascii_lowercase();
        let tokenizer = p.role == GenRole::Tokenizer && file.ends_with(TOKENIZER_EXTENSION);
        if !tokenizer
            && !ALLOWED_PART_EXTENSIONS
                .iter()
                .any(|ext| file.ends_with(ext))
        {
            return Err(NOT_ALLOWED_FILE.into());
        }
        if !is_hex(&p.revision, 40) || !is_hex(&p.sha256, 64) || p.size == 0 {
            return Err(NOT_PINNED.into());
        }
        if !licenses.contains_key(&p.license) {
            return Err(NO_LICENCE.into());
        }
    }
    Ok(())
}

/// Each licence with a bundled text pins that text.
pub(crate) fn check_licences(licenses: &BTreeMap<String, GenLicence>) -> Result<(), String> {
    for l in licenses.values() {
        let bundled = BUNDLED_LICENCE_TEXTS
            .iter()
            .find(|(id, _)| id.eq_ignore_ascii_case(&l.id));
        if let Some((_, text)) = bundled
            && sha256_hex(text.as_bytes()) != l.sha256
        {
            return Err(TEXT_MISMATCH.into());
        }
    }
    Ok(())
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// `generatorTiers` as `{"image": {"<GB>": [ids]}, "video": {…}}`: keys that are not
/// numbers, ids that are not usable entries and tiers left empty are dropped. A video
/// entry under `image`, or a picture entry under `video` (a catalog mistake), is dropped
/// as well and listed in `hidden` as `tier:<kind>:<id>`, once per kind.
fn read_tiers(
    raw: &Value,
    entries: &BTreeMap<String, GenEntry>,
    hidden: &mut Vec<Hidden>,
) -> GenTiers {
    let mut kind = |name: &str, video: bool| -> BTreeMap<u64, Vec<String>> {
        let Some(tiers) = raw.get(name).and_then(Value::as_object) else {
            return BTreeMap::new();
        };
        let mut misplaced = BTreeSet::new();
        let read = tiers
            .iter()
            .filter_map(|(gb, ids)| {
                let gb = gb.parse::<u64>().ok()?;
                let ids: Vec<String> = ids
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|id| match entries.get(*id) {
                        None => false,
                        Some(e) if (e.kind == GenKind::Video) == video => true,
                        Some(_) => {
                            misplaced.insert(id.to_string());
                            false
                        }
                    })
                    .map(str::to_string)
                    .collect();
                (!ids.is_empty()).then_some((gb, ids))
            })
            .collect();
        let reason = if video {
            PICTURE_AMONG_VIDEOS
        } else {
            VIDEO_AMONG_PICTURES
        };
        hidden.extend(misplaced.into_iter().map(|id| Hidden {
            id: format!("tier:{name}:{id}"),
            reason: reason.into(),
        }));
        read
    };
    GenTiers {
        image: kind("image", false),
        video: kind("video", true),
    }
}

// ---- generic entries (0.2.0) -------------------------------------------------------------

/// What a generic entry is made from: a model the catalog does not list, with the recipe it
/// runs with.
#[derive(Debug, Clone)]
pub struct GenericSpec<'a> {
    pub id: String,
    pub name: String,
    pub publisher: String,
    pub recipe: &'a Recipe,
    /// The engine version its model was detected as, when known.
    pub version: Option<SdVersion>,
    pub parts: Vec<GenPart>,
    pub licenses: BTreeMap<String, GenLicence>,
    pub gate: GenGate,
    /// A set-up whose recipe was never matched to the model ([`GenEntry::untested`]).
    pub untested: bool,
}

/// The engine's own steps and cfg, which a recipe that states none runs with.
const ENGINE_STEPS: u32 = 20;
const ENGINE_CFG: f32 = 7.0;
/// The high-noise steps that let the engine split the steps at the MoE boundary.
const ENGINE_HIGH_NOISE_STEPS: i32 = -1;
/// The longest side a generic entry is offered at.
const GENERIC_MAX_SIDE: u32 = 2048;
/// The engine build that has every recipe (the pin).
const GENERIC_MIN_BUILD: u32 = 1106;
/// The recipes of Z-Image's versions, which run with 0.1.x's Z-Image quirk.
const METAL_TENSOR_OFF_RECIPES: [&str; 3] = ["z-image", "z-image-turbo", "z-image-l2p"];

/// The entry of a model the catalog does not list, from its `recipe`: its kind and input
/// pictures, its defaults field for field (the engine's own steps, cfg and size where the
/// recipe states none), its sizes and frame rule, and the fit's fallback for its kind
/// ([`fallback_for`]). No measured memory or speed; never tested by the catalog.
pub fn generic_entry(spec: GenericSpec<'_>) -> GenEntry {
    let recipe = spec.recipe;
    let video = recipe.video();
    let input_counts = inputs_of(&recipe.id);
    GenEntry {
        id: spec.id,
        name: spec.name,
        summary: String::new(),
        publisher: spec.publisher,
        kind: match kind_of(&recipe.id) {
            RecipeKind::Picture => GenKind::Image,
            RecipeKind::PictureEdit => GenKind::ImageEdit,
            RecipeKind::Video => GenKind::Video,
        },
        family: None,
        recipe: recipe.id.clone(),
        version: spec.version,
        untested: spec.untested,
        inputs: input_counts.iter().map(|input| input.role).collect(),
        input_counts,
        parts: spec.parts,
        licenses: spec.licenses,
        gate: spec.gate,
        defaults: recipe_defaults(recipe, spec.version),
        limits: GenLimits {
            multiple_of: recipe.size_multiple(),
            max_side: GENERIC_MAX_SIDE,
            frame_rule: None,
            max_frames: video.map(|v| v.max_frames),
            video,
        },
        fallback: fallback_for(recipe),
        memory: Vec::new(),
        speed: Vec::new(),
        quirks: if METAL_TENSOR_OFF_RECIPES.contains(&recipe.id.as_str()) {
            vec![Quirk::MetalTensorOff]
        } else {
            Vec::new()
        },
        min_build: GENERIC_MIN_BUILD,
        runs_on_apple: true,
        tested: None,
    }
}

/// A recipe's defaults under Studio's names. Where it states no size: 512 × 512 for an
/// SD 1 or SD 2 model, else 1024 × 1024 (`instruct-pix2pix`, `cosxl-edit` and
/// `flux1-kontext` state none: they take the size of the picture they change).
fn recipe_defaults(recipe: &Recipe, version: Option<SdVersion>) -> GenDefaults {
    let d = &recipe.defaults;
    let side = if version.is_some_and(is_sd1_or_sd2) {
        512
    } else {
        1024
    };
    let high_noise = (d.high_noise_steps.is_some()
        || d.high_noise_cfg_scale.is_some()
        || d.high_noise_sampling_method.is_some())
    .then(|| HighNoiseDefaults {
        steps: d.high_noise_steps.unwrap_or(ENGINE_HIGH_NOISE_STEPS),
        cfg: d.high_noise_cfg_scale.unwrap_or(ENGINE_CFG),
        sampler: d.high_noise_sampling_method.clone(),
    });
    GenDefaults {
        steps: d.steps.unwrap_or(ENGINE_STEPS),
        cfg: d.cfg_scale.unwrap_or(ENGINE_CFG),
        flow_shift: d.flow_shift,
        sampler: d.sampling_method.clone(),
        scheduler: d.scheduler.clone(),
        width: d.width.unwrap_or(side),
        height: d.height.unwrap_or(side),
        frames: d.video_frames,
        fps: d.fps,
        guidance: d.distilled_guidance,
        img_cfg: d.img_cfg_scale,
        timestep_shift: d.shifted_timestep,
        sigmas: d.custom_sigmas.clone(),
        high_noise,
        moe_boundary: d.moe_boundary,
        strength: d.strength,
    }
}

/// The SD 1 and SD 2 versions (the engine's `sd_version_is_sd1` and `_sd2` groups,
/// `src/model.h:102-114`), which run at 512 × 512.
fn is_sd1_or_sd2(version: SdVersion) -> bool {
    use SdVersion::*;
    matches!(
        version,
        Sd1 | Sd1Inpaint
            | Sd1Pix2pix
            | Sd1TinyUnet
            | Sdxs512Ds
            | Sd2
            | Sd2Inpaint
            | Sd2TinyUnet
            | Sdxs09
    )
}

/// The fit's coefficients for a generic model of `recipe`'s kind: 0.1.x's factors of the
/// nearest catalog entry, rounded up (FLUX.2 klein's for a picture, Wan 2.2 TI2V-5B's for a
/// video). A latent token is 32 pixels a side when sizes are multiples of 32 or more, else 16.
pub fn fallback_for(recipe: &Recipe) -> FitFallback {
    let token_div = if recipe.size_multiple() >= 32 { 32 } else { 16 };
    match recipe.video() {
        None => FitFallback {
            act_base_bytes: 500_000_000,
            act_bytes_per_token: 100_000,
            vae_bytes_per_pixel_frame: 300,
            vae_tile_cap_bytes: 500_000_000,
            token_div,
            frame_div: None,
            vae_chunk_frames: None,
        },
        Some(video) => FitFallback {
            act_base_bytes: 1_000_000_000,
            act_bytes_per_token: 250_000,
            vae_bytes_per_pixel_frame: 400,
            vae_tile_cap_bytes: 2_500_000_000,
            token_div,
            frame_div: Some(video.frame_step),
            vae_chunk_frames: Some(4),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    /// One chat model, two usable entries (`sample-image`, `sample-video`), three broken
    /// ones, family records (one of a family this build does not know) and tiers. Every
    /// name, repository and number is made up; the licence hash is the bundled text's.
    const GEN: &str = include_str!("../tests/fixtures/catalog/generators.json");

    /// `json` with its top-level keys replaced by `fields` (`"key": value, …`).
    fn with(json: &str, fields: &str) -> String {
        let mut v: Value = serde_json::from_str(json).unwrap();
        let extra: Value = serde_json::from_str(&format!("{{{fields}}}")).unwrap();
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        v.to_string()
    }

    /// A Hugging Face listing's gate reads as a catalog entry's, in the same words.
    #[test]
    fn a_listings_gate_is_a_gate_mode() {
        for (gated, mode, word) in [
            (Gated::No, GateMode::None, "none"),
            (Gated::Auto, GateMode::Auto, "auto"),
            (Gated::Manual, GateMode::Manual, "manual"),
        ] {
            assert_eq!(GateMode::from(gated), mode);
            assert_eq!(serde_json::to_value(mode).unwrap(), json!(word));
        }
    }

    /// The fixture read at build 1106 after `change` to its whole JSON.
    fn read_with(change: impl FnOnce(&mut Value)) -> GenCatalog {
        let mut v: Value = serde_json::from_str(GEN).unwrap();
        change(&mut v);
        gen_catalog(&Catalog::parse(&v.to_string()).unwrap(), 1106)
    }

    /// The fixture read at build 1106 after `change` to `sample-image`.
    fn read_changed(change: impl FnOnce(&mut Value)) -> GenCatalog {
        read_with(|v| change(&mut v["generators"]["sample-image"]))
    }

    fn reason(g: &GenCatalog, id: &str) -> Option<String> {
        g.hidden
            .iter()
            .find(|h| h.id == id)
            .map(|h| h.reason.clone())
    }

    /// `sample-image` with `change` is hidden, for exactly `why`.
    fn hidden_for(why: &str, change: impl FnOnce(&mut Value)) {
        let g = read_changed(change);
        assert!(!g.entries.contains_key("sample-image"), "{why}");
        assert_eq!(reason(&g, "sample-image").as_deref(), Some(why));
        assert!(g.entries.contains_key("sample-video"), "only that entry");
    }

    fn hidden_with_part_path(path: &str) -> bool {
        let g = read_changed(|e| e["parts"][0]["path"] = path.into());
        if g.entries.contains_key("sample-image") {
            return false;
        }
        assert_eq!(
            reason(&g, "sample-image").as_deref(),
            Some("not a .safetensors, .sft or .gguf file, or a tokenizer's .json")
        );
        true
    }

    fn licence(id: &str, commercial: Commercial, limit: Option<u64>) -> GenLicence {
        GenLicence {
            id: id.into(),
            name: id.into(),
            url: format!("https://example.com/{id}"),
            sha256: "ab".repeat(32),
            commercial_outputs: commercial,
            revenue_limit_usd: limit,
            excluded_regions: Vec::new(),
            obligations: Vec::new(),
            plain_words: String::new(),
        }
    }

    /// `sample-image` with its parts naming `ids` in turn (the first part the first id).
    fn entry_with_licences(ids: &[&str]) -> GenEntry {
        let mut e = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106)
            .entries
            .remove("sample-image")
            .unwrap();
        assert!(ids.len() <= e.parts.len());
        let known = [
            licence("apache-2.0", Commercial::Yes, None),
            licence("kommunity", Commercial::UnderRevenue, Some(5_000_000)),
            licence("tiny-revenue", Commercial::UnderRevenue, Some(1_000_000)),
            licence("sign-up", Commercial::Registration, None),
            licence("openrail", Commercial::WithUseRules, None),
            licence("unclear", Commercial::Unclear, None),
            licence("flux-nc", Commercial::No, None),
        ];
        e.licenses = known
            .into_iter()
            .filter(|l| ids.contains(&l.id.as_str()))
            .map(|l| (l.id.clone(), l))
            .collect();
        for (part, id) in e.parts.iter_mut().zip(ids.iter().cycle()) {
            part.license = (*id).to_string();
        }
        e
    }

    #[test]
    fn generators_are_read_beside_the_chat_models() {
        let c = Catalog::parse(GEN).unwrap();
        assert_eq!(c.models.len(), 1);
        let g = gen_catalog(&c, 1106);
        assert_eq!(
            g.entries.keys().map(String::as_str).collect::<Vec<_>>(),
            ["sample-image", "sample-video"]
        );
        assert_eq!(
            (
                g.entries["sample-video"].kind,
                g.entries["sample-video"].limits.frame_rule
            ),
            (GenKind::Video, Some(FrameRule::FourNPlusOne))
        );
        assert_eq!(g.tiers.image[&8], ["sample-image"]);
    }

    /// Every field of a usable entry, as the fixture writes it (and its family gives).
    #[test]
    fn an_entry_is_read_field_by_field() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        let v = &g.entries["sample-video"];
        assert_eq!(v.id, "sample-video");
        assert_eq!(
            (v.name.as_str(), v.publisher.as_str()),
            ("Sample Clip 5B, fast", "Example Labs")
        );
        assert_eq!(v.family, Some(GenFamily::Wan22Ti2v5bFast));
        assert_eq!(v.inputs, [GenInput::StartFrame]);
        assert_eq!(
            v.parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Diffusion, GenRole::T5xxl, GenRole::Vae]
        );
        assert_eq!(v.diffusion().path, "Sample-Clip-5B-q6_k.gguf");
        assert_eq!(
            v.total_bytes(),
            4_212_000_000 + 4_148_000_000 + 1_409_400_000
        );
        assert_eq!(
            v.gate,
            GenGate {
                mode: GateMode::None,
                collects: vec![]
            }
        );
        assert_eq!(
            (
                v.defaults.flow_shift,
                v.defaults.frames,
                v.defaults.fps,
                v.defaults.sampler.as_deref()
            ),
            (Some(3.0), Some(49), Some(24), None)
        );
        assert_eq!((v.limits.multiple_of, v.limits.max_frames), (32, Some(121)));
        assert_eq!(
            (
                v.fallback.token_div,
                v.fallback.frame_div,
                v.fallback.vae_chunk_frames
            ),
            (32, Some(4), Some(4))
        );
        assert_eq!(
            v.fallback.act_base_bytes, 1_000_000_000,
            "its own, not the family's"
        );
        assert_eq!(v.speed[0].device_class, "vulkan-8");
        assert_eq!(v.quirks, [Quirk::VideoWebpOnMac]);
        assert_eq!(
            (v.min_build, v.tested.as_deref()),
            (1100, Some("2026-11-20"))
        );
        assert!(!v.runs_on_apple, "its family's");
        let licence = v.effective_licence();
        assert_eq!(
            (licence.id.as_str(), licence.badge()),
            ("apache-2.0", Badge::Green)
        );

        let i = &g.entries["sample-image"];
        assert_eq!(
            (i.kind, i.family),
            (GenKind::Image, Some(GenFamily::Flux2Klein4b))
        );
        assert_eq!(i.inputs, [GenInput::Reference]);
        assert_eq!(i.memory[0].peak_bytes, 6_200_000_000);
        assert!(i.runs_on_apple);
        assert_eq!(g.tiers.video[&8], ["sample-video"]);
    }

    /// The wire words downstream code and the catalog share.
    #[test]
    fn enums_read_and_write_the_catalog_words() {
        let words = |v: Value| serde_json::to_string(&v).unwrap();
        for (value, word) in [
            (json!(GenKind::Image), "image"),
            (json!(GenKind::ImageEdit), "image-edit"),
            (json!(GenKind::Video), "video"),
            (json!(GenFamily::Flux2Klein4b), "flux2-klein-4b"),
            (json!(GenFamily::ZImageTurbo), "z-image-turbo"),
            (json!(GenFamily::Wan22Ti2v5bFast), "wan2.2-ti2v-5b-fast"),
            (json!(GenFamily::Wan21T2v1_3b), "wan2.1-t2v-1.3b"),
            (json!(GenRole::HighNoiseDiffusion), "high-noise-diffusion"),
            (json!(GenRole::T5xxl), "t5xxl"),
            (json!(GenRole::ClipL), "clip-l"),
            (json!(GenRole::ClipVision), "clip-vision"),
            (json!(GenRole::Model), "model"),
            (json!(GenInput::Reference), "reference"),
            (json!(GenInput::StartFrame), "startFrame"),
            (json!(GenInput::EndFrame), "endFrame"),
            (json!(GenInput::Source), "source"),
            (json!(GenInput::Mask), "mask"),
            (json!(GenInput::Control), "control"),
            (json!(Quirk::MetalTensorOff), "metal-tensor-off"),
            (json!(Quirk::VideoWebpOnMac), "video-webp-on-mac"),
            (json!(Commercial::UnderRevenue), "underRevenue"),
            (json!(Commercial::Registration), "registration"),
            (json!(Commercial::WithUseRules), "withUseRules"),
            (json!(Badge::Amber), "amber"),
            (json!(GateMode::Manual), "manual"),
            (json!(FrameRule::FourNPlusOne), "4n+1"),
            (json!(FrameRule::EightNPlusOne), "8n+1"),
        ] {
            assert_eq!(words(value), format!("\"{word}\""));
        }
        assert_eq!(
            serde_json::from_str::<GenFamily>("\"wan2.1-t2v-1.3b\"").unwrap(),
            GenFamily::Wan21T2v1_3b
        );
    }

    /// 0.2.0: each of 0.1.x's families runs with one of the pinned engine's recipes, the
    /// one schema 4 gives its installed models, and is the engine version that recipe is
    /// matched from.
    #[test]
    fn each_01x_family_names_its_recipe_and_version() {
        use GenFamily::*;
        assert_eq!(
            [
                Flux2Klein4b,
                Flux3ImageKlein,
                ZImageTurbo,
                Wan22Ti2v5b,
                Wan22Ti2v5bFast,
                Wan21T2v1_3b
            ]
            .map(|f| (f.recipe_id(), f.version())),
            [
                ("flux2-klein-4b", SdVersion::Flux2Klein),
                ("flux3-klein", SdVersion::Flux3),
                ("z-image-turbo", SdVersion::ZImage),
                ("wan22-ti2v-5b", SdVersion::Wan22Ti2v),
                ("wan22-ti2v-5b", SdVersion::Wan22Ti2v),
                ("wan21-t2v", SdVersion::Wan2)
            ]
        );
        for f in GenFamily::ALL {
            assert!(crate::sd::recipes::recipe(f.recipe_id()).is_some(), "{f:?}");
            assert!(
                crate::sd::matching::in_scope_recipes().contains(&f.recipe_id()),
                "{f:?}"
            );
            assert!(
                crate::sd::matching::recipes_for(f.version()).contains(&f.recipe_id()),
                "{f:?}: a file of its version can take its recipe"
            );
            // The name rules agree with the family's recipe: read under the recipe's own id,
            // only a file's facts can pick another (`check_main_file` in the app).
            assert!(
                matches!(
                    crate::sd::matching::match_recipe(
                        f.version(),
                        &crate::sd::inspect::MatchFacts::default(),
                        &[],
                        f.recipe_id()
                    ),
                    crate::sd::matching::Match::Recipe { id, .. } if id == f.recipe_id()
                ),
                "{f:?}: its recipe's id names its recipe"
            );
        }
    }

    #[test]
    fn an_unknown_family_role_or_quirk_hides_only_that_entry() {
        let reasons: BTreeMap<_, _> = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106)
            .hidden
            .into_iter()
            .map(|h| (h.id, h.reason))
            .collect();
        for id in ["bad-family", "bad-role", "bad-quirk"] {
            assert_eq!(
                reasons.get(id).map(String::as_str),
                Some("unknown family, role, input, kind or quirk"),
                "{id}"
            );
        }
        hidden_for("unknown family, role, input, kind or quirk", |e| {
            e["inputs"] = json!(["audio"])
        });
        hidden_for("unknown family, role, input, kind or quirk", |e| {
            e["kind"] = json!("audio")
        });
    }

    #[test]
    fn an_entry_that_cannot_be_read_says_why() {
        let g = read_changed(|e| e["defaults"]["steps"] = json!("four"));
        let why = reason(&g, "sample-image").unwrap();
        assert!(why.starts_with("not a valid entry: "), "{why}");
    }

    #[test]
    fn an_entry_needing_a_newer_engine_is_hidden() {
        // sample-image has "minBuild": 1106.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(
            g.hidden
                .iter()
                .any(|h| h.id == "sample-image" && h.reason == "needs a newer Studio engine")
        );
        assert!(g.entries.contains_key("sample-video"), "minBuild 1100");
    }

    #[test]
    fn pickle_files_are_refused() {
        for ext in [".ckpt", ".pt", ".pth", ".bin"] {
            assert!(hidden_with_part_path(&format!("model{ext}")), "{ext}");
        }
        assert!(!hidden_with_part_path("Model.SAFETENSORS"));
        assert!(!hidden_with_part_path("dir/model.GGUF"));
    }

    #[test]
    fn every_part_is_pinned() {
        for (part, field, value) in [
            (0, "revision", json!("2".repeat(39))),
            (2, "revision", json!("4".repeat(41))),
            (1, "revision", json!("main")),
            (1, "revision", json!("A".repeat(40))),
            (0, "sha256", json!("b".repeat(63))),
            (2, "sha256", json!("g".repeat(64))),
            (1, "size", json!(0)),
            (2, "path", json!("split_files/../../vae.safetensors")),
            (0, "path", json!("/sample.gguf")),
            (1, "repo", json!("no-owner")),
            (2, "repo", json!("example/../evil")),
        ] {
            hidden_for("a part is not pinned", |e| e["parts"][part][field] = value);
        }
    }

    #[test]
    fn every_part_names_one_of_its_licences() {
        hidden_for("a part has no licence", |e| {
            e["parts"][1]["license"] = json!("mit")
        });
    }

    #[test]
    fn an_entry_has_exactly_one_model_part() {
        hidden_for("no model part", |e| {
            e["parts"][0]["role"] = json!("high-noise-diffusion")
        });
        hidden_for("more than one model part", |e| {
            e["parts"][1]["role"] = json!("model")
        });
        let g = read_changed(|e| e["parts"][0]["role"] = json!("model"));
        assert_eq!(g.entries["sample-image"].diffusion().role, GenRole::Model);
    }

    #[test]
    fn the_strictest_licence_of_all_parts_decides() {
        // kommunity: UnderRevenue, 5_000_000.
        let e = entry_with_licences(&["apache-2.0", "kommunity"]);
        assert_eq!(
            (
                e.effective_licence().id.as_str(),
                e.effective_licence().badge()
            ),
            ("kommunity", Badge::Amber)
        );
        assert_eq!(
            entry_with_licences(&["apache-2.0", "kommunity", "flux-nc"])
                .effective_licence()
                .badge(),
            Badge::Red
        );
        let strictest = |ids: &[&str]| entry_with_licences(ids).effective_licence().id.clone();
        assert_eq!(
            strictest(&["kommunity", "tiny-revenue"]),
            "tiny-revenue",
            "a lower limit"
        );
        assert_eq!(
            strictest(&["kommunity", "sign-up", "apache-2.0"]),
            "sign-up"
        );
        assert_eq!(strictest(&["unclear", "sign-up"]), "unclear");
        // Use rules are stricter than a registration and laxer than an unclear licence.
        assert_eq!(strictest(&["sign-up", "openrail"]), "openrail");
        assert_eq!(strictest(&["openrail", "unclear"]), "unclear");
        assert_eq!(strictest(&["unclear", "flux-nc"]), "flux-nc");
        assert_eq!(strictest(&["apache-2.0"]), "apache-2.0");
    }

    #[test]
    fn each_licence_has_its_badge() {
        for (commercial, badge) in [
            (Commercial::Yes, Badge::Green),
            (Commercial::UnderRevenue, Badge::Amber),
            (Commercial::Registration, Badge::Amber),
            (Commercial::WithUseRules, Badge::Amber),
            (Commercial::Unclear, Badge::Red),
            (Commercial::No, Badge::Red),
        ] {
            assert_eq!(
                licence("x", commercial, None).badge(),
                badge,
                "{commercial:?}"
            );
        }
    }

    #[test]
    fn a_region_limited_licence_keeps_its_terms() {
        let catalog =
            read_changed(|e| e["licenses"]["apache-2.0"]["excludedRegions"] = json!(["EU"]));
        assert_eq!(
            catalog.entries["sample-image"].licenses["apache-2.0"].excluded_regions,
            ["EU"]
        );
    }

    #[test]
    fn a_licence_hash_is_the_bundled_texts() {
        hidden_for("licence text does not match", |e| {
            e["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64))
        });
        // A licence id with no bundled text keeps its own pinned SHA-256.
        let g = read_changed(|e| {
            let mut l = e["licenses"]["apache-2.0"].take();
            l["sha256"] = json!("ab".repeat(32));
            e["licenses"] = json!({ "example-licence": l });
            for p in e["parts"].as_array_mut().unwrap() {
                p["license"] = json!("example-licence");
            }
        });
        let e = &g.entries["sample-image"];
        assert_eq!(e.effective_licence().id, "example-licence");
        assert_eq!(e.effective_licence().sha256, "ab".repeat(32));
    }

    /// The canonical Apache-2.0 text from apache.org, byte for byte (`.gitattributes` keeps
    /// Git from changing its line endings on Windows, which would change the hash).
    #[test]
    fn the_bundled_apache_text_is_the_canonical_one() {
        let (id, text) = BUNDLED_LICENCE_TEXTS[0];
        assert_eq!(id, "apache-2.0");
        assert_eq!(
            sha256_hex(text.as_bytes()),
            "cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30"
        );
    }

    #[test]
    fn broken_generators_never_fail_the_catalog() {
        let c = Catalog::parse(&with(
            GEN,
            r#""generators": {"x": 5}, "generatorTiers": "nope""#,
        ))
        .unwrap();
        let g = gen_catalog(&c, 1106);
        assert!(
            g.entries.is_empty()
                && g.tiers.image.is_empty()
                && g.hidden.iter().any(|h| h.id == "x")
        );
        assert_eq!(c.models.len(), 1);
    }

    #[test]
    fn tiers_keep_numbers_and_known_entries() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        // "lots" is not a number, "16" is not a list, "bad-family" is hidden and
        // "no-such-entry" and 42 are not entries.
        assert_eq!(g.tiers.image.keys().copied().collect::<Vec<_>>(), [0, 8]);
        assert_eq!(g.tiers.image[&0], ["sample-image"]);
        assert_eq!(g.tiers.video.keys().copied().collect::<Vec<_>>(), [8]);
        // A tier left with no entry is dropped.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(g.tiers.image.is_empty(), "{:?}", g.tiers.image);
    }

    /// A video entry listed under `image` (or a picture entry under `video`) is never
    /// recommended there; the mistake is listed once per kind and the entry stays usable.
    #[test]
    fn a_tier_keeps_only_entries_of_its_kind() {
        let g = read_with(|v| {
            v["generators"]["sample-image"]["kind"] = "image-edit".into();
            v["generatorTiers"]["image"]["8"] = json!(["sample-video", "sample-image"]);
            v["generatorTiers"]["image"]["12"] = json!(["sample-video"]);
            v["generatorTiers"]["video"]["8"] = json!(["sample-image", "sample-video"]);
        });
        assert_eq!(
            g.tiers.image[&8],
            ["sample-image"],
            "an edit model is a picture"
        );
        assert!(!g.tiers.image.contains_key(&12), "{:?}", g.tiers.image);
        assert_eq!(g.tiers.video[&8], ["sample-video"]);
        assert!(g.entries.contains_key("sample-image") && g.entries.contains_key("sample-video"));
        let rows: Vec<_> = g
            .hidden
            .iter()
            .filter(|h| h.id.starts_with("tier:"))
            .map(|h| (h.id.as_str(), h.reason.as_str()))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    "tier:image:sample-video",
                    "a video model listed among the picture models"
                ),
                (
                    "tier:video:sample-image",
                    "a picture model listed among the video models"
                ),
            ]
        );
        assert_eq!(reason(&g, "sample-video"), None, "the entry is not hidden");
    }

    #[test]
    fn an_entry_without_a_fallback_takes_its_familys() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        let e = &g.entries["sample-image"];
        let family = &g.families[&GenFamily::Flux2Klein4b];
        assert_eq!(e.fallback, family.fallback);
        assert_eq!(e.speed, family.speed, "speed too");
        assert_eq!(e.runs_on_apple, family.runs_on_apple);
    }

    #[test]
    fn an_entry_without_limits_or_min_build_takes_its_familys() {
        let g = read_with(|v| {
            let e = v["generators"]["sample-image"].as_object_mut().unwrap();
            e.remove("limits");
            e.remove("minBuild");
            v["generatorFamilies"]["flux2-klein-4b"]["limits"]["maxSide"] = json!(1536);
            v["generatorFamilies"]["flux2-klein-4b"]["minBuild"] = json!(1104);
        });
        let e = &g.entries["sample-image"];
        assert_eq!((e.limits.max_side, e.min_build), (1536, 1104));
        assert_eq!(e.limits, g.families[&GenFamily::Flux2Klein4b].limits);
    }

    #[test]
    fn an_entry_with_nothing_to_take_from_is_hidden() {
        let g = read_with(|v| {
            v["generatorFamilies"]
                .as_object_mut()
                .unwrap()
                .remove("flux2-klein-4b");
        });
        assert_eq!(
            reason(&g, "sample-image").as_deref(),
            Some("its family record is missing")
        );
        // With its own figures it needs no record.
        let g = read_with(|v| {
            v["generatorFamilies"]
                .as_object_mut()
                .unwrap()
                .remove("flux2-klein-4b");
            v["generators"]["sample-image"]["fallback"] =
                v["generators"]["sample-video"]["fallback"].clone();
        });
        let e = &g.entries["sample-image"];
        assert!(e.speed.is_empty() && e.runs_on_apple, "the defaults");
    }

    #[test]
    fn family_records_are_read_by_family() {
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1106);
        assert_eq!(
            g.families.keys().copied().collect::<Vec<_>>(),
            [GenFamily::Flux2Klein4b, GenFamily::Wan22Ti2v5bFast]
        );
        let klein = &g.families[&GenFamily::Flux2Klein4b];
        assert_eq!(klein.family, GenFamily::Flux2Klein4b);
        assert_eq!(klein.parameters, (3_500_000_000, 4_500_000_000));
        assert_eq!(klein.upstream_license, "apache-2.0");
        assert_eq!(klein.licenses["apache-2.0"].id, "apache-2.0");
        assert_eq!(
            klein.companions.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Llm, GenRole::Vae]
        );
        assert_eq!(
            (klein.kind, klein.inputs.as_slice()),
            (GenKind::Image, [GenInput::Reference].as_slice())
        );
        assert_eq!(
            reason(&g, "family:sd3.5-medium").as_deref(),
            Some("unknown family, role, input, kind or quirk")
        );
    }

    // ---- 0.2.0: generic models (Task S3) ---------------------------------------------------

    use crate::sd::recipes::recipe;

    const SHA: &str = "abababababababababababababababababababababababababababababababab";

    /// The licence map the test parts name.
    fn apache() -> BTreeMap<String, GenLicence> {
        BTreeMap::from([(
            "apache-2.0".to_string(),
            licence("apache-2.0", Commercial::Yes, None),
        )])
    }

    /// A pinned Hub part of `role` at `path`.
    fn part(role: GenRole, path: &str, size: u64) -> GenPart {
        GenPart {
            role,
            repo: "example/model".into(),
            revision: "1".repeat(40),
            path: path.into(),
            size,
            sha256: SHA.into(),
            license: "apache-2.0".into(),
            local: None,
            folder: None,
        }
    }

    /// A part from a file on this computer: no repository, revision or licence.
    fn local_part(role: GenRole, file: &str, sha256: &str) -> GenPart {
        GenPart {
            role,
            repo: String::new(),
            revision: String::new(),
            path: String::new(),
            size: 6_938_078_334,
            sha256: sha256.into(),
            license: String::new(),
            local: Some(PathBuf::from(file)),
            folder: None,
        }
    }

    fn spec(recipe: &Recipe, version: Option<SdVersion>, parts: Vec<GenPart>) -> GenericSpec<'_> {
        GenericSpec {
            id: "hf:o/r:x.safetensors".into(),
            name: "A model".into(),
            publisher: String::new(),
            recipe,
            version,
            parts,
            licenses: apache(),
            gate: GenGate {
                mode: GateMode::None,
                collects: Vec::new(),
            },
            untested: false,
        }
    }

    /// Wan 2.2 T2V A14B: its two diffusion models, the text encoder and the decoder.
    fn wan_a14b_parts() -> Vec<GenPart> {
        vec![
            part(
                GenRole::HighNoiseDiffusion,
                "wan2.2_t2v_high_noise_14B_Q4_K_M.gguf",
                9_651_800_000,
            ),
            part(
                GenRole::Diffusion,
                "wan2.2_t2v_low_noise_14B_Q4_K_M.gguf",
                9_651_800_000,
            ),
            part(
                GenRole::T5xxl,
                "umt5-xxl-encoder-Q5_K_M.gguf",
                4_145_878_880,
            ),
            part(GenRole::Vae, "wan_2.1_vae.safetensors", 253_815_318),
        ]
    }

    #[test]
    fn catalog_entries_carry_their_recipe_and_version() {
        let c = gen_catalog(&Catalog::bundled(), 1106);
        let klein = &c.entries["flux2-klein-4b-q4"];
        assert_eq!(
            (klein.recipe.as_str(), klein.version),
            ("flux2-klein-4b", Some(SdVersion::Flux2Klein))
        );
        // The catalog's tested defaults stay; recipe defaults are for generic models only.
        assert_eq!(klein.defaults.steps, 4);
        assert_eq!(klein.family, Some(GenFamily::Flux2Klein4b));
        assert!(!klein.untested && klein.input_counts.is_empty());
        for e in c.entries.values() {
            let family = e.family.unwrap();
            assert_eq!(
                (e.recipe.as_str(), e.version),
                (family.recipe_id(), Some(family.version())),
                "{}",
                e.id
            );
        }
    }

    #[test]
    fn a_generic_sdxl_entry_takes_the_recipes_defaults_and_limits() {
        let e = generic_entry(spec(
            recipe("sdxl-checkpoint").unwrap(),
            Some(SdVersion::Sdxl),
            vec![part(GenRole::Model, "x.safetensors", 6_938_078_334)],
        ));
        assert_eq!(
            (
                e.kind,
                e.defaults.steps,
                e.defaults.cfg,
                e.defaults.width,
                e.limits.multiple_of
            ),
            (GenKind::Image, 20, 5.0, 1024, 64)
        );
        assert_eq!(e.fallback.token_div, 32);
        assert_eq!(
            (e.id.as_str(), e.name.as_str(), e.recipe.as_str()),
            ("hf:o/r:x.safetensors", "A model", "sdxl-checkpoint")
        );
        assert_eq!((e.family, e.version), (None, Some(SdVersion::Sdxl)));
        assert_eq!(
            (e.limits.max_side, e.limits.video, e.limits.max_frames),
            (2048, None, None)
        );
        assert!(e.inputs.is_empty() && e.input_counts.is_empty());
        assert!(e.quirks.is_empty() && e.memory.is_empty() && e.speed.is_empty());
        assert_eq!(
            (
                e.min_build,
                e.runs_on_apple,
                e.tested.as_deref(),
                e.untested
            ),
            (1106, true, None, false)
        );
        assert_eq!(e.diffusion().path, "x.safetensors");
        assert_eq!(e.effective_licence().id, "apache-2.0");
    }

    #[test]
    fn a_generic_wan_a14b_entry_is_a_video_with_two_diffusion_parts() {
        let e = generic_entry(spec(
            recipe("wan22-t2v").unwrap(),
            Some(SdVersion::Wan2),
            wan_a14b_parts(),
        ));
        assert_eq!(e.kind, GenKind::Video);
        assert_eq!(e.limits.video_rule().unwrap().frame_step, 4);
        assert_eq!(e.defaults.high_noise.as_ref().unwrap().cfg, 4.0);
        assert_eq!(e.defaults.moe_boundary, Some(0.875));
        assert_eq!(
            e.defaults.high_noise,
            Some(HighNoiseDefaults {
                steps: -1,
                cfg: 4.0,
                sampler: Some("euler".into())
            })
        );
        assert_eq!(
            (
                e.defaults.frames,
                e.defaults.fps,
                e.defaults.flow_shift,
                e.defaults.width,
                e.defaults.height
            ),
            (Some(33), Some(16), Some(12.0), 1280, 720)
        );
        assert_eq!(e.limits.max_frames, Some(4096));
        assert_eq!(e.diffusion().path, "wan2.2_t2v_low_noise_14B_Q4_K_M.gguf");
        assert_eq!(
            e.fallback,
            FitFallback {
                act_base_bytes: 1_000_000_000,
                act_bytes_per_token: 250_000,
                vae_bytes_per_pixel_frame: 400,
                vae_tile_cap_bytes: 2_500_000_000,
                token_div: 16,
                frame_div: Some(4),
                vae_chunk_frames: Some(4),
            }
        );
    }

    /// Every defaults field a recipe states reaches the entry under Studio's name.
    #[test]
    fn a_generic_entry_takes_every_default_the_recipe_states() {
        let r = recipe("sdxl-checkpoint").unwrap();
        let mut r = r.clone();
        r.defaults = crate::sd::recipes::RecipeDefaults {
            sampling_method: Some("dpm++2m".into()),
            scheduler: Some("karras".into()),
            cfg_scale: Some(3.5),
            steps: Some(28),
            width: Some(832),
            height: Some(1216),
            distilled_guidance: Some(2.5),
            flow_shift: Some(3.0),
            video_frames: Some(49),
            fps: Some(24),
            custom_sigmas: vec![1.0, 0.5, 0.0],
            img_cfg_scale: Some(1.5),
            high_noise_steps: Some(2),
            high_noise_cfg_scale: None,
            high_noise_sampling_method: None,
            high_noise_flow_shift: Some(5.0),
            moe_boundary: Some(0.9),
            shifted_timestep: Some(250),
            strength: Some(0.75),
        };
        let d = generic_entry(spec(&r, Some(SdVersion::Sdxl), vec![])).defaults;
        assert_eq!(
            d,
            GenDefaults {
                steps: 28,
                cfg: 3.5,
                flow_shift: Some(3.0),
                sampler: Some("dpm++2m".into()),
                scheduler: Some("karras".into()),
                width: 832,
                height: 1216,
                frames: Some(49),
                fps: Some(24),
                guidance: Some(2.5),
                img_cfg: Some(1.5),
                timestep_shift: Some(250),
                sigmas: vec![1.0, 0.5, 0.0],
                // The engine's own cfg when only the steps are stated.
                high_noise: Some(HighNoiseDefaults {
                    steps: 2,
                    cfg: 7.0,
                    sampler: None
                }),
                moe_boundary: Some(0.9),
                strength: Some(0.75),
            }
        );
    }

    /// A recipe that leaves them out takes the engine's own steps and cfg, and 512 × 512 for
    /// an SD 1 or SD 2 version, else 1024 × 1024.
    #[test]
    fn a_recipe_without_defaults_takes_the_engines() {
        let e = generic_entry(spec(
            recipe("cosxl-edit").unwrap(),
            Some(SdVersion::SdxlPix2pix),
            vec![],
        ));
        assert_eq!(
            (
                e.defaults.steps,
                e.defaults.cfg,
                e.defaults.width,
                e.defaults.height
            ),
            (20, 7.0, 1024, 1024)
        );
        assert_eq!(
            (e.defaults.img_cfg, e.defaults.high_noise.as_ref()),
            (Some(1.5), None)
        );
        let size = |id: &str, version: Option<SdVersion>| {
            let d = generic_entry(spec(recipe(id).unwrap(), version, vec![])).defaults;
            (d.width, d.height)
        };
        assert_eq!(
            size("instruct-pix2pix", Some(SdVersion::Sd1Pix2pix)),
            (512, 512)
        );
        assert_eq!(size("flux1-kontext", Some(SdVersion::Flux)), (1024, 1024));
        assert_eq!(size("flux1-kontext", None), (1024, 1024));
        assert_eq!(
            size("sd2-512-checkpoint", Some(SdVersion::Sd2)),
            (512, 512),
            "its own"
        );
        let r = recipe("sd1-checkpoint").unwrap();
        let mut sizeless = r.clone();
        (sizeless.defaults.width, sizeless.defaults.height) = (None, None);
        for v in [SdVersion::Sd2Inpaint, SdVersion::Sdxs512Ds, SdVersion::Sd1] {
            let d = generic_entry(spec(&sizeless, Some(v), vec![])).defaults;
            assert_eq!((d.width, d.height), (512, 512), "{v:?}");
        }
        let d = generic_entry(spec(&sizeless, Some(SdVersion::Sdxl), vec![])).defaults;
        assert_eq!((d.width, d.height), (1024, 1024));
    }

    #[test]
    fn only_z_image_recipes_turn_metal_tensors_off() {
        for id in ["z-image", "z-image-turbo", "z-image-l2p"] {
            let e = generic_entry(spec(recipe(id).unwrap(), None, vec![]));
            assert_eq!(e.quirks, [Quirk::MetalTensorOff], "{id}");
        }
        let e = generic_entry(spec(recipe("flux1-dev").unwrap(), None, vec![]));
        assert!(e.quirks.is_empty());
    }

    #[test]
    fn an_untested_set_up_is_kept_on_the_entry() {
        let mut s = spec(recipe("flux1-dev").unwrap(), None, vec![]);
        s.untested = true;
        assert!(generic_entry(s).untested);
    }

    #[test]
    fn a_picture_fallback_reads_its_token_size_from_the_multiple() {
        let f = fallback_for(recipe("flux1-dev").unwrap());
        assert_eq!(
            f,
            FitFallback {
                act_base_bytes: 500_000_000,
                act_bytes_per_token: 100_000,
                vae_bytes_per_pixel_frame: 300,
                vae_tile_cap_bytes: 500_000_000,
                token_div: 16,
                frame_div: None,
                vae_chunk_frames: None,
            }
        );
        assert_eq!(
            fallback_for(recipe("sdxl-checkpoint").unwrap()).token_div,
            32
        );
        let wan5b = fallback_for(recipe("wan22-ti2v-5b").unwrap());
        assert_eq!((wan5b.token_div, wan5b.frame_div), (32, Some(4)));
    }

    #[test]
    fn the_video_rule_is_the_recipes_else_the_frame_rule() {
        let limits = |frame_rule, video| GenLimits {
            multiple_of: 16,
            max_side: 1280,
            frame_rule,
            max_frames: Some(121),
            video,
        };
        let rule = |step| VideoRule {
            frame_step: step,
            frame_offset: 1,
            rounding: FrameRounding::Floor,
            min_frames: 1,
            max_frames: 121,
            fixed_fps: None,
        };
        assert_eq!(
            limits(Some(FrameRule::FourNPlusOne), None).video_rule(),
            Some(rule(4))
        );
        assert_eq!(
            limits(Some(FrameRule::EightNPlusOne), None).video_rule(),
            Some(rule(8))
        );
        assert_eq!(limits(None, None).video_rule(), None);
        let own = recipe("minimax-h3-fl2va").unwrap().video().unwrap();
        assert_eq!(
            limits(Some(FrameRule::FourNPlusOne), Some(own)).video_rule(),
            Some(own)
        );
    }

    #[test]
    fn the_tokenizer_role_takes_a_json_file_and_no_other_role_does() {
        let l = apache();
        assert!(check_parts(&[part(GenRole::Tokenizer, "tokenizer.json", 1)], &l).is_ok());
        assert!(check_parts(&[part(GenRole::Vae, "vae.json", 1)], &l).is_err());
        assert!(check_parts(&[part(GenRole::Diffusion, "m.sft", 1)], &l).is_ok());
    }

    #[test]
    fn a_catalog_part_may_not_name_a_file_on_this_computer() {
        // A signed catalog's `local` would waive the repository, revision and licence checks
        // and point a run at any file (final review, area 1, M-1).
        let l = apache();
        let refused = |p: GenPart| check_parts(&[p], &l).unwrap_err();
        assert_eq!(
            refused(local_part(GenRole::Model, "/home/me/m.safetensors", SHA)),
            "a part names a file on this computer"
        );
        let mut pinned = part(GenRole::Model, "m.gguf", 1);
        pinned.local = Some(PathBuf::from("/etc/m.gguf"));
        assert_eq!(refused(pinned), "a part names a file on this computer");
        // A Hub part still needs all of it.
        let mut hub = part(GenRole::Model, "m.gguf", 1);
        hub.license = String::new();
        assert_eq!(refused(hub), "a part has no licence");
        let mut hub = part(GenRole::Model, "m.gguf", 1);
        hub.revision = String::new();
        assert_eq!(refused(hub), "a part is not pinned");
    }

    #[test]
    fn an_entry_or_a_family_with_a_local_part_is_hidden() {
        hidden_for("a part names a file on this computer", |e| {
            e["parts"][0]["local"] = json!("/home/me/m.gguf");
        });
        let g = read_with(|v| {
            v["generatorFamilies"]["flux2-klein-4b"]["companions"][0]["local"] =
                json!("C:\\Users\\me\\llm.safetensors");
        });
        assert!(!g.families.contains_key(&GenFamily::Flux2Klein4b));
        assert_eq!(
            reason(&g, "family:flux2-klein-4b").as_deref(),
            Some("a part names a file on this computer")
        );
    }

    #[test]
    fn every_slot_has_a_role_and_back() {
        for s in SlotId::RUNNABLE {
            assert_eq!(GenRole::from_slot(s).unwrap().slot(), s);
        }
        for s in [
            SlotId::ControlNet,
            SlotId::IpAdapter,
            SlotId::MotionModule,
            SlotId::PhotoMaker,
            SlotId::PulidWeights,
        ] {
            assert_eq!(GenRole::from_slot(s), None, "{s:?}");
        }
        assert_eq!(GenRole::Model.slot(), SlotId::Model);
        assert_eq!(GenRole::Diffusion.slot(), SlotId::DiffusionModel);
    }

    #[test]
    fn the_new_roles_have_their_kebab_words() {
        for (role, word) in [
            (GenRole::UncondDiffusion, "uncond-diffusion"),
            (GenRole::LlmVision, "llm-vision"),
            (GenRole::ClipG, "clip-g"),
            (GenRole::Tokenizer, "tokenizer"),
            (GenRole::Tae, "tae"),
            (GenRole::AudioVae, "audio-vae"),
            (GenRole::AudioEncoder, "audio-encoder"),
            (GenRole::EmbeddingsConnectors, "embeddings-connectors"),
            (GenRole::Diffusion, "diffusion"),
            (GenRole::Llm, "llm"),
            (GenRole::Vae, "vae"),
        ] {
            assert_eq!(json!(role), json!(word));
            assert_eq!(
                serde_json::from_value::<GenRole>(json!(word)).unwrap(),
                role
            );
        }
    }

    /// An installed model's parts as 0.1.2 saved them still read, with no local file or
    /// folder name, and write back without the new fields.
    #[test]
    fn a_0_1_2_parts_json_still_parses() {
        let saved = json!([
            {"role": "diffusion", "repo": "leejet/FLUX.2-klein-4B-GGUF",
             "revision": "a".repeat(40), "path": "flux-2-klein-4b-Q4_0.gguf",
             "file": "/models/leejet/FLUX.2-klein-4B-GGUF/flux-2-klein-4b-Q4_0.gguf",
             "size": 2_460_378_560u64, "sha256": "b".repeat(64), "license": "apache-2.0"},
            {"role": "llm", "repo": "unsloth/Qwen3-4B-GGUF", "revision": "c".repeat(40),
             "path": "Qwen3-4B-Q4_K_M.gguf", "file": "/models/q.gguf", "size": 2_497_281_312u64,
             "sha256": "d".repeat(64), "license": "apache-2.0"},
            {"role": "vae", "repo": "Comfy-Org/Flux-2-Klein-4B", "revision": "e".repeat(40),
             "path": "split_files/vae/flux2-vae.safetensors", "file": "/models/v.safetensors",
             "size": 336_211_292u64, "sha256": "f".repeat(64), "license": "apache-2.0"}
        ]);
        let parts: Vec<GenPart> = serde_json::from_value(saved).unwrap();
        assert_eq!(
            parts.iter().map(|p| p.role).collect::<Vec<_>>(),
            [GenRole::Diffusion, GenRole::Llm, GenRole::Vae]
        );
        assert!(
            parts
                .iter()
                .all(|p| p.local.is_none() && p.folder.is_none())
        );
        let written = serde_json::to_value(&parts[0]).unwrap();
        assert!(written.get("local").is_none() && written.get("folder").is_none());
        assert_eq!(written["path"], "flux-2-klein-4b-Q4_0.gguf");
    }

    #[test]
    fn an_inpainting_entry_takes_a_picture_to_change_and_a_mask() {
        let e = generic_entry(spec(
            recipe("sdxl-inpainting").unwrap(),
            Some(SdVersion::SdxlInpaint),
            vec![part(GenRole::Model, "x.safetensors", 1)],
        ));
        assert_eq!(
            (e.kind, e.inputs.clone()),
            (GenKind::ImageEdit, vec![GenInput::Source, GenInput::Mask])
        );
        assert_eq!(
            e.input_counts,
            crate::sd::matching::inputs_of("sdxl-inpainting")
        );
        assert_eq!(
            (e.input_counts[0].min, e.input_counts[0].max),
            (1, 1),
            "each row's min and max"
        );
    }

    #[test]
    fn a_family_record_is_checked_like_an_entry() {
        let family = |change: fn(&mut Value)| {
            let g = read_with(|v| change(&mut v["generatorFamilies"]["flux2-klein-4b"]));
            assert!(!g.families.contains_key(&GenFamily::Flux2Klein4b));
            reason(&g, "family:flux2-klein-4b").unwrap()
        };
        assert_eq!(
            family(|f| f["companions"][0]["size"] = json!(0)),
            "a part is not pinned"
        );
        assert_eq!(
            family(|f| f["companions"][1]["path"] = json!("vae.pt")),
            "not a .safetensors, .sft or .gguf file, or a tokenizer's .json"
        );
        assert_eq!(
            family(|f| f["upstreamLicense"] = json!("mit")),
            "a part has no licence"
        );
        assert_eq!(
            family(|f| f["companions"][0]["license"] = json!("mit")),
            "a part has no licence"
        );
        assert_eq!(
            family(|f| f["licenses"]["apache-2.0"]["sha256"] = json!("0".repeat(64))),
            "licence text does not match"
        );
        assert_eq!(
            family(|f| f["companions"][0]["role"] = json!("diffusion")),
            "a companion is a model part"
        );
        assert!(family(|f| f["parameters"] = json!(4)).starts_with("not a valid family record: "));
        // A record needing a newer engine is left out; so is an entry that takes its build.
        let g = gen_catalog(&Catalog::parse(GEN).unwrap(), 1105);
        assert!(!g.families.contains_key(&GenFamily::Flux2Klein4b));
        assert_eq!(
            reason(&g, "family:flux2-klein-4b").as_deref(),
            Some("needs a newer Studio engine")
        );
    }
}
