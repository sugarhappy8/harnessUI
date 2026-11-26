//! The Studio engine's arguments, only from typed fields (docs/dev/0.1.0/studio-local-design.md,
//! "Arguments come only from typed fields"; docs/dev/0.2.0/design.md 7.6 and 8).
//!
//! [`build_args`] gives each model part's role one fixed flag ([`flag_for`], design 7.1's
//! table), then numbers, then whitelisted words ([`Sampler`], [`Scheduler`], [`VaeFormat`],
//! the probed device or `cpu`, `--vae-tiling`, `--offload-to-cpu`), then `-o` and the fixed
//! flags, so a signed catalog or a recipe cannot add a flag. The prompt never reaches argv:
//! `run_job` writes [`prompt_file_text`] to the files [`prompt_files`] names and the engine
//! reads them (`--prompt-file`). The LoRAs go in that file too, as `<lora:NAME:W>` tags the
//! engine reads relative to `--lora-model-dir`; the LoRA tags and image markers typed into
//! the prompt are removed first. The pinned parser matches only `arg == name`, so it has no `--prompt=<text>` form;
//! a string option takes the next argument as its value whatever it starts with, and
//! `--prompt-file` reads the whole file as bytes.
//!
//! [`FLAGS`] lists every flag [`build_args`] can emit; [`missing_from_help`] checks them, and
//! the sampler and scheduler words, against `sd-cli -h`: the unit tests on the pinned build's
//! help in `tests/fixtures/sd_help.txt`, the contract tests (`tests/sd_contract.rs`) on the
//! engine itself.

use std::ffi::{OsStr, OsString};
use std::ops::RangeInclusive;
use std::path::{Component, Path, PathBuf};

pub use super::lora_tags::{
    LORA_WEIGHT_DEFAULT, LORA_WEIGHT_RANGE, LORA_WEIGHT_STEP, LoraTag, MAX_LORAS, lora_tag,
    valid_lora_name,
};
pub use super::prompt_view::{attention_text, parse_prompt_attention, strip_prompt_markers};

/// The longest prompt, in bytes of UTF-8: 1 MiB, a safety bound against pathological input,
/// never shown as a limit (design 9.2). The prompt goes by file, never in argv.
pub const PROMPT_MAX_BYTES: usize = 1_048_576;

/// The longest local prompt, in characters, also never shown as a limit (ruling
/// G3-safety-bound). Studio's prompt check (`studio/prompt_check.rs`) reads every local
/// prompt four times, the last through the engine's own weighting
/// ([`parse_prompt_attention`]), whose time grows with the square of the brackets; at this
/// length the worst bracket nesting reads in about 0.2 s. The local lane refuses a longer
/// prompt before the check reads it, so no local prompt skips a reading.
pub const LOCAL_PROMPT_MAX_CHARS: usize = 16_384;

/// The longest local negative prompt, in characters: the most that always fit in
/// [`PROMPT_MAX_BYTES`] of UTF-8 (four bytes each), so the 1 MiB bound holds while the field
/// counts characters. The prompt check does not read a negative prompt (since 0.1.x), so it
/// needs no bound of its own; only its typed markers are removed
/// ([`strip_prompt_markers`], linear).
pub const NEGATIVE_MAX_CHARS: usize = PROMPT_MAX_BYTES / 4;

/// The sides the engine is given, in pixels.
const SIZE_RANGE: RangeInclusive<u32> = 64..=2048;

/// The prompt's file name in the run's folder.
const PROMPT_FILE: &str = "prompt.txt";

/// The negative prompt's file name in the run's folder.
const NEGATIVE_FILE: &str = "negative.txt";

/// The largest number the engine reads for an int option (`std::stoi`, which throws on a
/// larger one; the pinned CLI does not catch it and aborts).
const INT_MAX: u32 = i32::MAX.unsigned_abs();

/// The largest seed the engine reads (`std::stoll`, which throws on a larger one).
const SEED_MAX: u64 = i64::MAX.unsigned_abs();

/// What a model part is to the engine: a recipe's slot (design 7.1), or 0.1.x's catalog role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartRole {
    /// The diffusion model on its own (`--diffusion-model`).
    Diffusion,
    /// The high-noise diffusion model of a two-model video family
    /// (`--high-noise-diffusion-model`).
    HighNoiseDiffusion,
    /// The unconditional diffusion model of a family that guides with one
    /// (`--uncond-diffusion-model`).
    UncondDiffusion,
    /// A language model used as the text encoder (`--llm`).
    Llm,
    /// The vision encoder of that language model (`--llm_vision`).
    LlmVision,
    /// The T5-XXL text encoder (`--t5xxl`).
    T5xxl,
    /// The CLIP-L text encoder (`--clip_l`).
    ClipL,
    /// The CLIP-G text encoder (`--clip_g`).
    ClipG,
    /// The CLIP vision encoder (`--clip_vision`).
    ClipVision,
    /// The tokenizer file (`--tokenizer`).
    Tokenizer,
    /// The decoder (`--vae`).
    Vae,
    /// The tiny decoder (`--tae`).
    Tae,
    /// The audio decoder (`--audio-vae`).
    AudioVae,
    /// The audio encoder (`--audio-encoder`).
    AudioEncoder,
    /// The embeddings connectors (`--embeddings-connectors`).
    EmbeddingsConnectors,
    /// One file holding the whole model (`-m`).
    Single,
}

/// The one flag the engine takes a part of `role` with (design 7.1's table).
pub fn flag_for(role: PartRole) -> &'static str {
    match role {
        PartRole::Diffusion => "--diffusion-model",
        PartRole::HighNoiseDiffusion => "--high-noise-diffusion-model",
        PartRole::UncondDiffusion => "--uncond-diffusion-model",
        PartRole::Llm => "--llm",
        PartRole::LlmVision => "--llm_vision",
        PartRole::T5xxl => "--t5xxl",
        PartRole::ClipL => "--clip_l",
        PartRole::ClipG => "--clip_g",
        PartRole::ClipVision => "--clip_vision",
        PartRole::Tokenizer => "--tokenizer",
        PartRole::Vae => "--vae",
        PartRole::Tae => "--tae",
        PartRole::AudioVae => "--audio-vae",
        PartRole::AudioEncoder => "--audio-encoder",
        PartRole::EmbeddingsConnectors => "--embeddings-connectors",
        PartRole::Single => "-m",
    }
}

/// The sampling methods Studio offers: every `--sampling-method` word of the pinned CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sampler {
    /// `euler`.
    Euler,
    /// `euler_a`.
    EulerA,
    /// `heun`.
    Heun,
    /// `dpm2`.
    Dpm2,
    /// `dpm++2s_a`.
    DpmPp2sA,
    /// `dpm++2m`.
    DpmPp2m,
    /// `dpm++2mv2`.
    DpmPp2mV2,
    /// `ipndm`.
    Ipndm,
    /// `ipndm_v`.
    IpndmV,
    /// `lcm`.
    Lcm,
    /// `ddim_trailing`.
    DdimTrailing,
    /// `tcd`.
    Tcd,
    /// `res_multistep`.
    ResMultistep,
    /// `res_2s`.
    Res2s,
    /// `er_sde`.
    ErSde,
    /// `euler_cfg_pp`.
    EulerCfgPp,
    /// `euler_a_cfg_pp`.
    EulerACfgPp,
    /// `euler_ge`.
    EulerGe,
    /// `dpm++2m_sde`.
    DpmPp2mSde,
    /// `dpm++2m_sde_bt`.
    DpmPp2mSdeBt,
    /// `lms`.
    Lms,
}

impl Sampler {
    /// Every sampler, in the order the pinned CLI lists them.
    pub const ALL: &'static [Self] = &[
        Self::Euler,
        Self::EulerA,
        Self::Heun,
        Self::Dpm2,
        Self::DpmPp2sA,
        Self::DpmPp2m,
        Self::DpmPp2mV2,
        Self::Ipndm,
        Self::IpndmV,
        Self::Lcm,
        Self::DdimTrailing,
        Self::Tcd,
        Self::ResMultistep,
        Self::Res2s,
        Self::ErSde,
        Self::EulerCfgPp,
        Self::EulerACfgPp,
        Self::EulerGe,
        Self::DpmPp2mSde,
        Self::DpmPp2mSdeBt,
        Self::Lms,
    ];

    /// The CLI's word for this sampler.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Euler => "euler",
            Self::EulerA => "euler_a",
            Self::Heun => "heun",
            Self::Dpm2 => "dpm2",
            Self::DpmPp2sA => "dpm++2s_a",
            Self::DpmPp2m => "dpm++2m",
            Self::DpmPp2mV2 => "dpm++2mv2",
            Self::Ipndm => "ipndm",
            Self::IpndmV => "ipndm_v",
            Self::Lcm => "lcm",
            Self::DdimTrailing => "ddim_trailing",
            Self::Tcd => "tcd",
            Self::ResMultistep => "res_multistep",
            Self::Res2s => "res_2s",
            Self::ErSde => "er_sde",
            Self::EulerCfgPp => "euler_cfg_pp",
            Self::EulerACfgPp => "euler_a_cfg_pp",
            Self::EulerGe => "euler_ge",
            Self::DpmPp2mSde => "dpm++2m_sde",
            Self::DpmPp2mSdeBt => "dpm++2m_sde_bt",
            Self::Lms => "lms",
        }
    }

    /// The sampler whose CLI word is exactly `word`; `None` for any other word.
    pub fn from_cli(word: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == word)
    }
}

/// The schedulers Studio offers: every `--scheduler` word of the pinned CLI. No scheduler
/// (`None` in [`SdJob`]) leaves the engine its model-specific default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scheduler {
    /// `discrete`.
    Discrete,
    /// `karras`.
    Karras,
    /// `exponential`.
    Exponential,
    /// `ays`.
    Ays,
    /// `gits`.
    Gits,
    /// `sgm_uniform`.
    SgmUniform,
    /// `simple`.
    Simple,
    /// `smoothstep`.
    Smoothstep,
    /// `kl_optimal`.
    KlOptimal,
    /// `lcm`.
    Lcm,
    /// `bong_tangent`.
    BongTangent,
    /// `ltx2`.
    Ltx2,
    /// `logit_normal`.
    LogitNormal,
    /// `flux2`.
    Flux2,
    /// `flux`.
    Flux,
    /// `beta`.
    Beta,
    /// `llada_image`.
    LladaImage,
}

impl Scheduler {
    /// Every scheduler, in the order the pinned CLI lists them.
    pub const ALL: &'static [Self] = &[
        Self::Discrete,
        Self::Karras,
        Self::Exponential,
        Self::Ays,
        Self::Gits,
        Self::SgmUniform,
        Self::Simple,
        Self::Smoothstep,
        Self::KlOptimal,
        Self::Lcm,
        Self::BongTangent,
        Self::Ltx2,
        Self::LogitNormal,
        Self::Flux2,
        Self::Flux,
        Self::Beta,
        Self::LladaImage,
    ];

    /// The CLI's word for this scheduler.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Discrete => "discrete",
            Self::Karras => "karras",
            Self::Exponential => "exponential",
            Self::Ays => "ays",
            Self::Gits => "gits",
            Self::SgmUniform => "sgm_uniform",
            Self::Simple => "simple",
            Self::Smoothstep => "smoothstep",
            Self::KlOptimal => "kl_optimal",
            Self::Lcm => "lcm",
            Self::BongTangent => "bong_tangent",
            Self::Ltx2 => "ltx2",
            Self::LogitNormal => "logit_normal",
            Self::Flux2 => "flux2",
            Self::Flux => "flux",
            Self::Beta => "beta",
            Self::LladaImage => "llada_image",
        }
    }

    /// The scheduler whose CLI word is exactly `word`; `None` for any other word.
    pub fn from_cli(word: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == word)
    }
}

/// A recipe's VAE latent format (`--vae-format`). The engine's own `auto` is never sent: no
/// format (`None` in [`SdJob`]) leaves it to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VaeFormat {
    /// `flux`.
    Flux,
    /// `sd3`.
    Sd3,
    /// `flux2`.
    Flux2,
    /// `wan`.
    Wan,
}

impl VaeFormat {
    /// Every format Studio sends.
    pub const ALL: [Self; 4] = [Self::Flux, Self::Sd3, Self::Flux2, Self::Wan];

    /// The CLI's word for this format.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Flux => "flux",
            Self::Sd3 => "sd3",
            Self::Flux2 => "flux2",
            Self::Wan => "wan",
        }
    }

    /// The format whose CLI word (a recipe's `context.vae_format`) is exactly `word`; `None`
    /// for any other word, the engine's own `auto` included, which leaves it to the engine.
    pub fn from_cli(word: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.as_str() == word)
    }
}

/// The high-noise part's own sampling, for a two-model video family (Wan 2.2 A14B).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HighNoise {
    /// Its steps (`--high-noise-steps`); -1 lets the engine split the steps at the MoE
    /// boundary.
    pub steps: i32,
    /// Its guidance scale (`--high-noise-cfg-scale`).
    pub cfg: f32,
    /// Its sampling method (`--high-noise-sampling-method`); `None` lets the engine choose.
    pub sampler: Option<Sampler>,
}

/// The file the engine writes: a PNG picture, or a WebM (VP8) or animated WebP video.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// A PNG picture.
    Png,
    /// A WebM (VP8) video.
    Webm,
    /// An animated WebP video, the fallback when WebM does not play.
    Webp,
}

impl OutputFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Webm => "webm",
            Self::Webp => "webp",
        }
    }
}

/// One local run, in typed fields only. The numbers come checked against the catalog's or
/// the recipe's ranges (the engine reads them as 32-bit integers, the seed as a 64-bit one);
/// [`build_args`] still refuses one the engine cannot read.
#[derive(Debug, Clone, PartialEq)]
pub struct SdJob {
    /// The model's files, each with its role, in the catalog's order.
    pub parts: Vec<(PartRole, PathBuf)>,
    /// The prompt, written to its file by `run_job` as [`prompt_file_text`] gives it.
    pub prompt: String,
    /// The negative prompt; a blank one is none.
    pub negative: Option<String>,
    /// Width in pixels: a multiple of `size_multiple` from 64 to 2048.
    pub width: u32,
    /// Height in pixels: a multiple of `size_multiple` from 64 to 2048.
    pub height: u32,
    /// What the sides are a multiple of: the recipe's `size_multiple` (8 when the recipe
    /// leaves it out), or 0.1.x's catalog's `multiple_of`.
    pub size_multiple: u32,
    /// Sampling steps.
    pub steps: u32,
    /// Guidance scale.
    pub cfg: f32,
    /// The distilled guidance of models with a guidance input (`--guidance`); `None` lets
    /// the engine choose.
    pub guidance: Option<f32>,
    /// The image guidance scale of edit and inpainting models (`--img-cfg-scale`); `None`
    /// lets the engine choose.
    pub img_cfg: Option<f32>,
    /// The flow shift of flow models; `None` lets the engine choose.
    pub flow_shift: Option<f32>,
    /// The timestep shift of NitroFusion models (`--timestep-shift`); `None` lets the engine
    /// choose.
    pub timestep_shift: Option<u32>,
    /// The sampling method; `None` lets the engine choose.
    pub sampler: Option<Sampler>,
    /// The scheduler; `None` lets the engine choose.
    pub scheduler: Option<Scheduler>,
    /// The recipe's own sigmas (`--sigmas`, comma-joined); empty lets the scheduler choose.
    pub sigmas: Vec<f32>,
    /// The high-noise part's sampling; `None` lets the engine choose.
    pub high_noise: Option<HighNoise>,
    /// The timestep at which a two-model video family moves to its low-noise part
    /// (`--moe-boundary`); `None` lets the engine choose.
    pub moe_boundary: Option<f32>,
    /// How much of the picture to change is redrawn (`--strength`); `None` lets the engine
    /// choose.
    pub strength: Option<f32>,
    /// The seed Studio picked.
    pub seed: u64,
    /// How many pictures, one after another.
    pub batch: u32,
    /// Frames of a video; `Some` makes the run a video (`-M vid_gen`).
    pub frames: Option<u32>,
    /// Frames a second of a video.
    pub fps: Option<u32>,
    /// The start frame of a video, or the picture a picture starts from (`-i`).
    pub init_image: Option<PathBuf>,
    /// The end frame of a video (`--end-img`).
    pub end_image: Option<PathBuf>,
    /// The mask of the picture to change (`--mask`): only with `init_image`.
    pub mask_image: Option<PathBuf>,
    /// The control picture of a control model, FLUX.1 Canny or Depth (`--control-image`).
    /// `--control-strength` is never sent: it is for ControlNet files, which Studio does not
    /// run.
    pub control_image: Option<PathBuf>,
    /// Reference pictures, each passed with its own `-r`.
    pub ref_images: Vec<PathBuf>,
    /// The recipe's VAE latent format; `None` lets the engine choose.
    pub vae_format: Option<VaeFormat>,
    /// The recipe's extra model arguments (`--model-args`): `key=value` pairs joined by `,`.
    pub model_args: Option<String>,
    /// The Studio LoRA folder (`--lora-model-dir`), which the LoRAs' names are relative to.
    pub lora_dir: Option<PathBuf>,
    /// The attached LoRAs, at most [`MAX_LORAS`], each a tag appended to the prompt file.
    pub loras: Vec<LoraTag>,
    /// The device from [`super::devices::match_llama_device`]; `None` lets the engine choose.
    pub backend: Option<String>,
    /// Decode in tiles (`--vae-tiling`), where `tiling_allowed`.
    pub vae_tiling: bool,
    /// Keep the weights in memory until needed (`--offload-to-cpu`): only with Memory saving
    /// or on the out-of-memory retry.
    pub offload_to_cpu: bool,
    /// Whether the recipe lets the decoder tile: false for the four that disable
    /// `vae_tiling`, and then nothing tiles, `vae_tiling` or the out-of-memory retry
    /// (ruling S2-tiling-gate).
    pub tiling_allowed: bool,
    /// Run on the processor (`--backend cpu`), whatever `backend` says.
    pub processor_only: bool,
    /// Processor threads ([`processor_threads`]); `None` lets the engine choose.
    pub threads: Option<u32>,
    /// The run's folder (`<gallery>/.partial/<run>`): the prompt files and the output go in it.
    pub out_dir: PathBuf,
    /// The output's file name without its extension: one plain name, so the output stays in
    /// `out_dir`.
    pub out_stem: String,
    /// The output's format.
    pub format: OutputFormat,
    /// Variables added to the engine's environment, only from compiled-in quirks.
    pub env: Vec<(String, String)>,
}

/// Why [`build_args`] refused a job.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArgsError {
    /// The prompt, without the LoRA tags and image markers typed into it, is empty or only
    /// white space.
    #[error("the prompt is empty")]
    EmptyPrompt,
    /// The prompt is longer than [`PROMPT_MAX_BYTES`] bytes.
    #[error("the prompt is longer than {PROMPT_MAX_BYTES} bytes")]
    PromptTooLong,
    /// A path is not valid Unicode (it is never converted lossily).
    #[error("the path {} is not valid Unicode", .0.display())]
    NotUnicode(PathBuf),
    /// No part is a diffusion model (`--diffusion-model` or `-m`).
    #[error("no part is a diffusion model")]
    NoDiffusionPart,
    /// The width or height is not a multiple of `multiple` (the job's `size_multiple`) from
    /// 64 to 2048, or `multiple` is 0.
    #[error("the size is not a multiple of {multiple} from 64 to 2048")]
    BadSize {
        /// The job's `size_multiple`.
        multiple: u32,
    },
    /// The output path holds a numbering template ([`is_numbering_template`]).
    #[error("the output path holds a %d numbering template")]
    PercentInOutputPath,
    /// A number the engine cannot read, named by its [`SdJob`] field: an integer above
    /// `i32::MAX`, a seed above `i64::MAX`, or a float (a guidance scale, the flow shift, a
    /// sigma, …) that is not finite, or is subnormal (not 0, and nearer 0 than
    /// `f32::MIN_POSITIVE`). The engine aborts on an integer out of range and on a subnormal
    /// float (its `std::stof` reports ERANGE); NaN and infinity it would read, but they mean
    /// nothing as either. A LoRA's weight outside [`LORA_WEIGHT_RANGE`] is `"lora weight"`.
    #[error("the {0} is out of the engine's range")]
    NumberOutOfRange(&'static str),
    /// The output's name is not one plain file name: it is empty or absolute, or holds a
    /// separator, `.` or `..`, so `-o` would leave the run's folder.
    #[error("the output's name is not one plain file name")]
    BadOutputStem,
    /// A LoRA's name is not one the engine's tag reads back ([`valid_lora_name`]).
    #[error("the LoRA name {0:?} cannot go in the engine's LoRA tag")]
    BadLoraName(String),
    /// More than [`MAX_LORAS`] LoRAs are attached.
    #[error("more than {MAX_LORAS} LoRAs are attached")]
    TooManyLoras,
    /// LoRAs are attached without the folder their names are relative to.
    #[error("LoRAs are attached without the LoRA folder")]
    LorasWithoutFolder,
    /// The extra model arguments are not `key=value` pairs joined by `,` (each key of
    /// `[a-z0-9_]`, each value of `[A-Za-z0-9_.]`).
    #[error("the model's extra arguments are not key=value pairs")]
    BadModelArgs,
    /// A mask without a picture to change: the engine would redraw the whole picture.
    #[error("a mask needs a picture to change")]
    MaskWithoutPicture,
    /// The tokenizer's path holds both `=` and `,`: the engine reads a `--tokenizer` value with
    /// `=` as `slot=FILE` assignments split at `,`, so no value names that file.
    #[error("the tokenizer's path holds both = and ,")]
    TokenizerPath,
}

/// Every flag [`build_args`] can emit, in its order. The contract tests check each against the
/// pinned `sd-cli -h`.
pub const FLAGS: &[&str] = &[
    "--diffusion-model",
    "--high-noise-diffusion-model",
    "--uncond-diffusion-model",
    "--llm",
    "--llm_vision",
    "--t5xxl",
    "--clip_l",
    "--clip_g",
    "--clip_vision",
    "--tokenizer",
    "--vae",
    "--tae",
    "--audio-vae",
    "--audio-encoder",
    "--embeddings-connectors",
    "-m",
    "-M",
    "--prompt-file",
    "--negative-prompt-file",
    "-W",
    "-H",
    "--steps",
    "--cfg-scale",
    "--guidance",
    "--img-cfg-scale",
    "--flow-shift",
    "--timestep-shift",
    "--sampling-method",
    "--scheduler",
    "--sigmas",
    "--high-noise-steps",
    "--high-noise-cfg-scale",
    "--high-noise-sampling-method",
    "--moe-boundary",
    "--strength",
    "--seed",
    "--batch-count",
    "--video-frames",
    "--fps",
    "-i",
    "--end-img",
    "--mask",
    "--control-image",
    "-r",
    "--vae-format",
    "--model-args",
    "--lora-model-dir",
    "--backend",
    "--threads",
    "--vae-tiling",
    "--offload-to-cpu",
    "-o",
    "--disable-image-metadata",
    "--log-level",
];

/// The engine's arguments for `job`, in a fixed order:
/// 1. the parts (one fixed flag each), `-M vid_gen` for a video, `--prompt-file` and
///    `--negative-prompt-file` (when there is a negative prompt);
/// 2. `-W`, `-H`, `--steps`, `--cfg-scale`;
/// 3. `--guidance`, `--img-cfg-scale`, `--flow-shift`, `--timestep-shift`,
///    `--sampling-method` and `--scheduler`, each when set;
/// 4. `--sigmas` (comma-joined), `--high-noise-steps`, `--high-noise-cfg-scale`,
///    `--high-noise-sampling-method`, `--moe-boundary` and `--strength`, each when set;
/// 5. `--seed`, `--batch-count`, then `--video-frames`, `--fps`, `-i`, `--end-img`, `--mask`,
///    `--control-image` and each `-r`, each when set;
/// 6. `--vae-format`, `--model-args` and `--lora-model-dir`, each when set;
/// 7. `--backend` (`cpu` when `processor_only`), `--threads` when set, `--vae-tiling` when
///    asked for and the recipe allows it (`vae_tiling && tiling_allowed`), `--offload-to-cpu`
///    when asked for, `-o <out_dir>/<out_stem>.<png|webm|webp>`,
///    `--disable-image-metadata` (the engine's own record holds the prompt) and
///    `--log-level info` (the progress reader reads the info lines).
///
/// The prompt is checked to be at most [`PROMPT_MAX_BYTES`] bytes ([`ArgsError::PromptTooLong`])
/// and not to be empty without its typed LoRA tags, and never reaches argv. Refused, whatever checked them before: a size that is not a multiple of
/// `size_multiple` ([`ArgsError::BadSize`]), a number the engine cannot read
/// ([`ArgsError::NumberOutOfRange`]), extra model arguments that are not `key=value` pairs
/// ([`ArgsError::BadModelArgs`]), a mask without a picture to change
/// ([`ArgsError::MaskWithoutPicture`]), LoRAs without the LoRA folder, more than
/// [`MAX_LORAS`] or with a name the tag cannot carry, a tokenizer path the engine cannot
/// read back ([`ArgsError::TokenizerPath`]; one with `=` goes as `main=<path>`), and an
/// `out_stem` that is not one plain file name ([`ArgsError::BadOutputStem`]).
pub fn build_args(job: &SdJob) -> Result<Vec<OsString>, ArgsError> {
    // The bound first: nothing reads a pathological prompt.
    if job.prompt.len() > PROMPT_MAX_BYTES {
        return Err(ArgsError::PromptTooLong);
    }
    if strip_prompt_markers(&job.prompt).trim().is_empty() {
        return Err(ArgsError::EmptyPrompt);
    }
    if !job
        .parts
        .iter()
        .any(|(role, _)| matches!(role, PartRole::Diffusion | PartRole::Single))
    {
        return Err(ArgsError::NoDiffusionPart);
    }
    if !(fits(job.width, job.size_multiple) && fits(job.height, job.size_multiple)) {
        return Err(ArgsError::BadSize {
            multiple: job.size_multiple,
        });
    }
    readable_numbers(job)?;
    if job.model_args.as_deref().is_some_and(|a| !is_model_args(a)) {
        return Err(ArgsError::BadModelArgs);
    }
    if job.mask_image.is_some() && job.init_image.is_none() {
        return Err(ArgsError::MaskWithoutPicture);
    }
    checked_loras(job)?;
    unicode(&job.out_dir)?;
    if !is_one_name(&job.out_stem) {
        return Err(ArgsError::BadOutputStem);
    }
    let output = job
        .out_dir
        .join(format!("{}.{}", job.out_stem, job.format.extension()));
    let output = unicode(&output)?.to_owned();
    if is_numbering_template(&output) {
        return Err(ArgsError::PercentInOutputPath);
    }
    let (prompt_file, negative_file) = prompt_files(job);

    let mut args: Vec<String> = Vec::new();
    let mut push = |flag: &str, value: String| args.extend([flag.to_owned(), value]);
    for (role, path) in &job.parts {
        let path = unicode(path)?;
        let value = match role {
            PartRole::Tokenizer => tokenizer_value(path)?,
            _ => path.to_owned(),
        };
        push(flag_for(*role), value);
    }
    if job.frames.is_some() {
        push("-M", "vid_gen".to_owned());
    }
    push("--prompt-file", unicode(&prompt_file)?.to_owned());
    if let Some(file) = &negative_file {
        push("--negative-prompt-file", unicode(file)?.to_owned());
    }
    push("-W", job.width.to_string());
    push("-H", job.height.to_string());
    push("--steps", job.steps.to_string());
    push("--cfg-scale", job.cfg.to_string());
    if let Some(guidance) = job.guidance {
        push("--guidance", guidance.to_string());
    }
    if let Some(img_cfg) = job.img_cfg {
        push("--img-cfg-scale", img_cfg.to_string());
    }
    if let Some(shift) = job.flow_shift {
        push("--flow-shift", shift.to_string());
    }
    if let Some(shift) = job.timestep_shift {
        push("--timestep-shift", shift.to_string());
    }
    if let Some(sampler) = job.sampler {
        push("--sampling-method", sampler.as_str().to_owned());
    }
    if let Some(scheduler) = job.scheduler {
        push("--scheduler", scheduler.as_str().to_owned());
    }
    if !job.sigmas.is_empty() {
        let sigmas: Vec<String> = job.sigmas.iter().map(f32::to_string).collect();
        push("--sigmas", sigmas.join(","));
    }
    if let Some(high) = &job.high_noise {
        push("--high-noise-steps", high.steps.to_string());
        push("--high-noise-cfg-scale", high.cfg.to_string());
        if let Some(sampler) = high.sampler {
            push("--high-noise-sampling-method", sampler.as_str().to_owned());
        }
    }
    if let Some(boundary) = job.moe_boundary {
        push("--moe-boundary", boundary.to_string());
    }
    if let Some(strength) = job.strength {
        push("--strength", strength.to_string());
    }
    push("--seed", job.seed.to_string());
    push("--batch-count", job.batch.to_string());
    if let Some(frames) = job.frames {
        push("--video-frames", frames.to_string());
    }
    if let Some(fps) = job.fps {
        push("--fps", fps.to_string());
    }
    let pictures = [
        ("-i", &job.init_image),
        ("--end-img", &job.end_image),
        ("--mask", &job.mask_image),
        ("--control-image", &job.control_image),
    ];
    for (flag, picture) in pictures {
        if let Some(path) = picture {
            push(flag, unicode(path)?.to_owned());
        }
    }
    for reference in &job.ref_images {
        push("-r", unicode(reference)?.to_owned());
    }
    if let Some(format) = job.vae_format {
        push("--vae-format", format.as_str().to_owned());
    }
    if let Some(model_args) = &job.model_args {
        push("--model-args", model_args.clone());
    }
    if let Some(dir) = &job.lora_dir {
        push("--lora-model-dir", unicode(dir)?.to_owned());
    }
    let device = if job.processor_only {
        Some("cpu")
    } else {
        job.backend.as_deref()
    };
    if let Some(device) = device {
        push("--backend", device.to_owned());
    }
    if let Some(threads) = job.threads {
        push("--threads", threads.to_string());
    }
    if job.vae_tiling && job.tiling_allowed {
        args.push("--vae-tiling".to_owned());
    }
    if job.offload_to_cpu {
        args.push("--offload-to-cpu".to_owned());
    }
    args.extend(
        [
            "-o",
            &output,
            "--disable-image-metadata",
            "--log-level",
            "info",
        ]
        .map(str::to_owned),
    );
    Ok(args.into_iter().map(OsString::from).collect())
}

/// Where `run_job` writes the prompt (and the negative prompt, when there is one) before the
/// engine starts: inside `out_dir`, as `prompt.txt` and `negative.txt`. A negative prompt
/// that is blank once its markers are removed ([`negative_file_text`]) has no file.
pub fn prompt_files(job: &SdJob) -> (PathBuf, Option<PathBuf>) {
    let negative = negative_file_text(job).map(|_| job.out_dir.join(NEGATIVE_FILE));
    (job.out_dir.join(PROMPT_FILE), negative)
}

/// What `run_job` writes to the negative prompt's file: the negative prompt without the image
/// markers and LoRA tags typed into it ([`strip_prompt_markers`], ruling
/// S2-negative-markers), or `None` when that is blank. The engine reads image markers in the
/// negative prompt as it does in the prompt (`src/pipeline/image.cpp:613-620`,
/// `video.cpp:1239-1241`), so a typed one would load any file it names, or throw when left
/// open. It reads no LoRA tag there, so removing one changes nothing.
pub fn negative_file_text(job: &SdJob) -> Option<String> {
    job.negative
        .as_deref()
        .map(strip_prompt_markers)
        .filter(|negative| !negative.trim().is_empty())
}

/// What `run_job` writes to the prompt's file: the prompt without the LoRA tags typed into
/// it, nor any image marker ([`strip_prompt_markers`]), then a space and [`lora_tag`] for each
/// attached LoRA, in order.
pub fn prompt_file_text(job: &SdJob) -> String {
    let mut text = strip_prompt_markers(&job.prompt);
    for tag in &job.loras {
        text.push(' ');
        text.push_str(&lora_tag(tag));
    }
    text
}

/// Threads for a processor-only run: physical cores minus one, at least 1; physical cores
/// are the logical count halved on x86-64.
pub fn processor_threads(logical_cores: usize, x86_64: bool) -> u32 {
    let physical = if x86_64 {
        logical_cores / 2
    } else {
        logical_cores
    };
    u32::try_from(physical.saturating_sub(1).max(1)).unwrap_or(u32::MAX)
}

/// Whether `path` holds `%` followed by up to three digits and `d` (`%d`, `%3d`, `%03d`):
/// `sd-cli` reads an `-o` path holding one as a numbering template (the pinned CLI's
/// `format_specifier_regex`). Stricter than the engine, which skips a `%` escaped as `%%`.
pub fn is_numbering_template(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes
        .iter()
        .enumerate()
        .filter(|&(_, &b)| b == b'%')
        .any(|(i, _)| {
            let rest = &bytes[i + 1..];
            let digits = rest
                .iter()
                .take(3)
                .take_while(|d| d.is_ascii_digit())
                .count();
            rest.get(digits) == Some(&b'd')
        })
}

/// What Studio can send that the `sd-cli -h` text `help` does not list: each [`FLAGS`] entry
/// that is no option's name, and each [`Sampler`] and [`Scheduler`] word missing from its
/// option's `one of [...]` list. Empty for the pinned build.
pub fn missing_from_help(help: &str) -> Vec<&'static str> {
    let options = help_options(help);
    let listed = |flag: &str| options.iter().find(|option| option.names.contains(&flag));
    let choices = |flag: &str| listed(flag).map(HelpOption::choices).unwrap_or_default();
    let samplers = choices("--sampling-method");
    let schedulers = choices("--scheduler");
    let mut missing: Vec<&'static str> = FLAGS
        .iter()
        .copied()
        .filter(|flag| listed(flag).is_none())
        .collect();
    missing.extend(
        Sampler::ALL
            .iter()
            .map(|sampler| sampler.as_str())
            .filter(|word| !samplers.contains(word)),
    );
    missing.extend(
        Scheduler::ALL
            .iter()
            .map(|scheduler| scheduler.as_str())
            .filter(|word| !schedulers.contains(word)),
    );
    missing
}

/// One option of `sd-cli -h`: its names (`-o`, `--output`) and its description, its wrapped
/// lines joined.
struct HelpOption<'a> {
    names: Vec<&'a str>,
    description: String,
}

impl HelpOption<'_> {
    /// The words of the description's `one of [a, b, c]` list.
    fn choices(&self) -> Vec<&str> {
        self.description
            .split_once("one of [")
            .and_then(|(_, rest)| rest.split_once(']'))
            .map(|(list, _)| list.split(',').map(str::trim).collect())
            .unwrap_or_default()
    }
}

/// The options of `sd-cli -h`. The pinned CLI prints each as two spaces, its names
/// (`-o, --output <string>`), at least two spaces and its description, wrapped onto lines
/// indented further.
fn help_options(help: &str) -> Vec<HelpOption<'_>> {
    let mut options: Vec<HelpOption<'_>> = Vec::new();
    for line in help.lines() {
        if let Some(entry) = line.strip_prefix("  ").filter(|rest| rest.starts_with('-')) {
            let (names, description) = entry.split_once("  ").unwrap_or((entry, ""));
            options.push(HelpOption {
                names: names
                    .split(", ")
                    .filter_map(|name| name.split_whitespace().next())
                    .collect(),
                description: description.trim().to_owned(),
            });
        } else if line.starts_with(' ')
            && !line.trim().is_empty()
            && let Some(option) = options.last_mut()
        {
            option.description.push(' ');
            option.description.push_str(line.trim());
        }
    }
    options
}

/// The `--tokenizer` value for the tokenizer file at `path`. The engine reads a value
/// without `=` as the main slot's one path, `,` and all, and a value with `=` as `slot=FILE`
/// assignments split at `,` (`TokenizerConfig`, `src/tokenizers/tokenizer_config.cpp:13` at
/// the pinned commit). So a path with `=` goes as `main=<path>`, which it splits at its first
/// `=`; one with both `=` and `,` cannot be named: [`ArgsError::TokenizerPath`].
fn tokenizer_value(path: &str) -> Result<String, ArgsError> {
    match (path.contains('='), path.contains(',')) {
        (false, _) => Ok(path.to_owned()),
        (true, false) => Ok(format!("main={path}")),
        (true, true) => Err(ArgsError::TokenizerPath),
    }
}

/// Whether `side` is a multiple of `multiple` (never of 0) within [`SIZE_RANGE`].
fn fits(side: u32, multiple: u32) -> bool {
    multiple != 0 && side.is_multiple_of(multiple) && SIZE_RANGE.contains(&side)
}

/// Refuses the first number in `job` the engine cannot read, naming its field.
fn readable_numbers(job: &SdJob) -> Result<(), ArgsError> {
    let ints = [
        ("steps", Some(job.steps)),
        ("batch", Some(job.batch)),
        ("frames", job.frames),
        ("fps", job.fps),
        ("threads", job.threads),
        ("timestep_shift", job.timestep_shift),
    ];
    for (field, value) in ints {
        if value.is_some_and(|n| n > INT_MAX) {
            return Err(ArgsError::NumberOutOfRange(field));
        }
    }
    if job.seed > SEED_MAX {
        return Err(ArgsError::NumberOutOfRange("seed"));
    }
    let floats = [
        ("cfg", Some(job.cfg)),
        ("guidance", job.guidance),
        ("img_cfg", job.img_cfg),
        ("flow_shift", job.flow_shift),
        ("high_noise_cfg", job.high_noise.map(|high| high.cfg)),
        ("moe_boundary", job.moe_boundary),
        ("strength", job.strength),
    ];
    for (field, value) in floats {
        if value.is_some_and(|x| !readable_float(x)) {
            return Err(ArgsError::NumberOutOfRange(field));
        }
    }
    if !job.sigmas.iter().copied().all(readable_float) {
        return Err(ArgsError::NumberOutOfRange("sigmas"));
    }
    Ok(())
}

/// Refuses LoRAs without the LoRA folder, more than [`MAX_LORAS`], a name the tag cannot
/// carry ([`valid_lora_name`]) and a weight outside [`LORA_WEIGHT_RANGE`] or one the engine
/// cannot read.
fn checked_loras(job: &SdJob) -> Result<(), ArgsError> {
    if job.loras.is_empty() {
        return Ok(());
    }
    if job.lora_dir.is_none() {
        return Err(ArgsError::LorasWithoutFolder);
    }
    if job.loras.len() > MAX_LORAS {
        return Err(ArgsError::TooManyLoras);
    }
    for tag in &job.loras {
        if !valid_lora_name(&tag.name) {
            return Err(ArgsError::BadLoraName(tag.name.clone()));
        }
        if !(readable_float(tag.weight) && LORA_WEIGHT_RANGE.contains(&tag.weight)) {
            return Err(ArgsError::NumberOutOfRange("lora weight"));
        }
    }
    Ok(())
}

/// Whether `args` is `key=value` pairs joined by `,`, each matching
/// `[a-z0-9_]+=[A-Za-z0-9_.]+`.
fn is_model_args(args: &str) -> bool {
    args.split(',').all(|pair| {
        pair.split_once('=').is_some_and(|(key, value)| {
            !key.is_empty()
                && key
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
                && !value.is_empty()
                && value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
        })
    })
}

/// A float option the engine reads back as given: finite, and 0 or normal (a subnormal one
/// makes its `std::stof` throw, which the pinned CLI does not catch).
fn readable_float(x: f32) -> bool {
    x.is_finite() && (x == 0.0 || x.is_normal())
}

/// Whether `stem` is exactly one [`Component::Normal`], spelled as given (so a trailing
/// separator or `.` is refused too): joined to `out_dir`, it names a file in that folder.
fn is_one_name(stem: &str) -> bool {
    let mut components = Path::new(stem).components();
    matches!(components.next(), Some(Component::Normal(name)) if name == OsStr::new(stem))
        && components.next().is_none()
}

fn unicode(path: &Path) -> Result<&str, ArgsError> {
    path.to_str()
        .ok_or_else(|| ArgsError::NotUnicode(path.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;
    use std::path::PathBuf;

    use super::*;

    fn p(path: &str) -> PathBuf {
        PathBuf::from(path)
    }

    fn strings(args: Vec<OsString>) -> Vec<String> {
        args.into_iter()
            .map(|arg| arg.into_string().expect("Unicode"))
            .collect()
    }

    /// Where `flag` is in `args`.
    fn index(args: &[String], flag: &str) -> usize {
        args.iter()
            .position(|arg| arg == flag)
            .unwrap_or_else(|| panic!("{flag} is not in {args:?}"))
    }

    /// The argument after `flag`.
    fn after<'a>(args: &'a [String], flag: &str) -> &'a str {
        &args[index(args, flag) + 1]
    }

    /// A FLUX.2 klein picture: four steps, no negative prompt, on the default device, with
    /// none of the recipe's own fields set.
    fn klein_job() -> SdJob {
        SdJob {
            parts: vec![
                (PartRole::Diffusion, p("/m/d.gguf")),
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.safetensors")),
            ],
            prompt: "a red kite over the sea".into(),
            negative: None,
            width: 1024,
            height: 1024,
            size_multiple: 16,
            steps: 4,
            cfg: 1.0,
            guidance: None,
            img_cfg: None,
            flow_shift: None,
            timestep_shift: None,
            sampler: Some(Sampler::Euler),
            scheduler: Some(Scheduler::Simple),
            sigmas: Vec::new(),
            high_noise: None,
            moe_boundary: None,
            strength: None,
            seed: 81723,
            batch: 1,
            frames: None,
            fps: None,
            init_image: None,
            end_image: None,
            mask_image: None,
            control_image: None,
            ref_images: Vec::new(),
            vae_format: None,
            model_args: None,
            lora_dir: None,
            loras: Vec::new(),
            backend: None,
            vae_tiling: false,
            offload_to_cpu: false,
            tiling_allowed: true,
            processor_only: false,
            threads: None,
            out_dir: p("/g/.partial/r1"),
            out_stem: "r1".into(),
            format: OutputFormat::Png,
            env: Vec::new(),
        }
    }

    fn job_with(parts: Vec<(PartRole, PathBuf)>) -> SdJob {
        SdJob {
            parts,
            ..klein_job()
        }
    }

    /// A Wan 2.2 TI2V video from a start frame: 49 frames at 24 fps with a flow shift.
    fn video() -> SdJob {
        SdJob {
            parts: vec![
                (PartRole::Diffusion, p("/m/wan.gguf")),
                (PartRole::T5xxl, p("/m/umt5.gguf")),
                (PartRole::Vae, p("/m/wan_vae.safetensors")),
            ],
            width: 832,
            height: 480,
            steps: 8,
            cfg: 5.0,
            flow_shift: Some(3.0),
            sampler: None,
            scheduler: None,
            frames: Some(49),
            fps: Some(24),
            init_image: Some(p("/g/refs/start.png")),
            format: OutputFormat::Webm,
            ..klein_job()
        }
    }

    /// A Wan 2.2 T2V A14B video: two diffusion parts, UMT5-XXL and the Wan 2.1 VAE, 33 frames
    /// at 16 fps with the recipe's flow shift and size multiple.
    fn wan_a14b_job() -> SdJob {
        SdJob {
            parts: vec![
                (
                    PartRole::Diffusion,
                    p("/m/wan2.2_t2v_low_noise_14B_Q4_K_M.gguf"),
                ),
                (
                    PartRole::HighNoiseDiffusion,
                    p("/m/wan2.2_t2v_high_noise_14B_Q4_K_M.gguf"),
                ),
                (PartRole::T5xxl, p("/m/umt5-xxl-encoder-Q8_0.gguf")),
                (PartRole::Vae, p("/m/wan_2.1_vae.safetensors")),
            ],
            width: 832,
            height: 480,
            size_multiple: 16,
            steps: 20,
            cfg: 3.0,
            flow_shift: Some(12.0),
            sampler: Some(Sampler::Euler),
            scheduler: None,
            frames: Some(33),
            fps: Some(16),
            format: OutputFormat::Webm,
            ..klein_job()
        }
    }

    /// Every part role, each with a file of its own.
    fn every_part() -> Vec<(PartRole, PathBuf)> {
        [
            PartRole::Diffusion,
            PartRole::HighNoiseDiffusion,
            PartRole::UncondDiffusion,
            PartRole::Llm,
            PartRole::LlmVision,
            PartRole::T5xxl,
            PartRole::ClipL,
            PartRole::ClipG,
            PartRole::ClipVision,
            PartRole::Tokenizer,
            PartRole::Vae,
            PartRole::Tae,
            PartRole::AudioVae,
            PartRole::AudioEncoder,
            PartRole::EmbeddingsConnectors,
            PartRole::Single,
        ]
        .into_iter()
        .enumerate()
        .map(|(i, role)| (role, p(&format!("/m/part{i}.gguf"))))
        .collect()
    }

    #[test]
    fn each_part_role_has_one_fixed_flag() {
        let a = strings(
            build_args(&job_with(vec![
                (PartRole::Diffusion, p("/m/d.gguf")),
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.safetensors")),
            ]))
            .unwrap(),
        );
        assert_eq!(
            &a[..6],
            [
                "--diffusion-model",
                "/m/d.gguf",
                "--llm",
                "/m/q.gguf",
                "--vae",
                "/m/v.safetensors"
            ]
        );
        // design 7.1's table: one flag a slot
        let roles = [
            (PartRole::Diffusion, "--diffusion-model"),
            (PartRole::HighNoiseDiffusion, "--high-noise-diffusion-model"),
            (PartRole::UncondDiffusion, "--uncond-diffusion-model"),
            (PartRole::Llm, "--llm"),
            (PartRole::LlmVision, "--llm_vision"),
            (PartRole::T5xxl, "--t5xxl"),
            (PartRole::ClipL, "--clip_l"),
            (PartRole::ClipG, "--clip_g"),
            (PartRole::ClipVision, "--clip_vision"),
            (PartRole::Tokenizer, "--tokenizer"),
            (PartRole::Vae, "--vae"),
            (PartRole::Tae, "--tae"),
            (PartRole::AudioVae, "--audio-vae"),
            (PartRole::AudioEncoder, "--audio-encoder"),
            (PartRole::EmbeddingsConnectors, "--embeddings-connectors"),
            (PartRole::Single, "-m"),
        ];
        for (role, flag) in roles {
            assert_eq!(flag_for(role), flag, "{role:?}");
        }
        // every part passes its own flag, in the parts' order, before anything else
        let a = strings(build_args(&job_with(every_part())).unwrap());
        let flags: Vec<&str> = a[..32].iter().step_by(2).map(String::as_str).collect();
        assert_eq!(flags, roles.map(|(_, flag)| flag));
    }

    #[test]
    fn the_prompt_goes_by_file_and_never_reaches_argv() {
        let job = SdJob {
            prompt: "-o /etc/x".into(),
            negative: Some("--vae evil".into()),
            ..klein_job()
        };
        let a = strings(build_args(&job).unwrap());
        let (prompt, negative) = prompt_files(&job);
        assert_eq!(after(&a, "--prompt-file"), prompt.to_str().unwrap());
        assert_eq!(
            after(&a, "--negative-prompt-file"),
            negative.unwrap().to_str().unwrap()
        );
        assert_eq!(prompt, job.out_dir.join("prompt.txt"));
        assert!(
            !a.iter()
                .any(|x| x.contains("/etc/x") || x.contains("evil") || x.starts_with("--prompt="))
        );
        assert_eq!(a.iter().filter(|x| *x == "-o").count(), 1); // only the output's own
    }

    #[test]
    fn a_blank_negative_prompt_is_no_negative_prompt() {
        for negative in [None, Some(String::new()), Some(" \n".into())] {
            let job = SdJob {
                negative,
                ..klein_job()
            };
            assert_eq!(prompt_files(&job).1, None);
            let a = strings(build_args(&job).unwrap());
            assert!(!a.contains(&"--negative-prompt-file".into()));
        }
        let job = SdJob {
            negative: Some("blurry".into()),
            ..klein_job()
        };
        assert_eq!(prompt_files(&job).1, Some(job.out_dir.join("negative.txt")));
    }

    /// One hidden bound of 1 MiB of UTF-8 against pathological input (design 9.2): no
    /// character cap any more.
    #[test]
    fn a_prompt_up_to_1_mib_is_accepted_and_one_byte_more_refused() {
        let mut j = klein_job();
        j.prompt = "a".repeat(5_000);
        assert!(build_args(&j).is_ok());
        j.prompt = "a".repeat(PROMPT_MAX_BYTES);
        assert!(build_args(&j).is_ok());
        j.prompt.push('a');
        assert_eq!(build_args(&j), Err(ArgsError::PromptTooLong));
        // Bytes, not characters: 349,525 three-byte characters fit, one more does not.
        j.prompt = "風".repeat(PROMPT_MAX_BYTES / 3);
        assert!(build_args(&j).is_ok());
        j.prompt.push('風');
        assert_eq!(build_args(&j), Err(ArgsError::PromptTooLong));
    }

    #[test]
    fn an_empty_prompt_is_refused() {
        for prompt in ["", "  \n\t"] {
            assert_eq!(
                build_args(&SdJob {
                    prompt: prompt.into(),
                    ..klein_job()
                }),
                Err(ArgsError::EmptyPrompt),
                "{prompt:?}"
            );
        }
    }

    #[test]
    fn an_output_folder_that_looks_like_a_numbering_template_is_refused() {
        for dir in ["/g/shots %d", "/g/%03d", "/g/a%12d"] {
            assert_eq!(
                build_args(&SdJob {
                    out_dir: dir.into(),
                    ..klein_job()
                }),
                Err(ArgsError::PercentInOutputPath),
                "{dir}"
            );
        }
        assert!(
            build_args(&SdJob {
                out_dir: "/g/100% real".into(),
                ..klein_job()
            })
            .is_ok()
        );
    }

    /// The edges of the numbering-template check, which `build_args` and the gallery-folder
    /// check in `prefs.rs` share.
    #[test]
    fn a_numbering_template_is_percent_up_to_three_digits_and_d() {
        for template in ["%d", "%03d", "%%d", "/g/shots %d", "/g/%03d/x", "50%%d"] {
            assert!(is_numbering_template(template), "{template:?}");
        }
        for plain in ["%1234d", "%", "/g/100%", "/g/%1234d", "100% real", "%x", ""] {
            assert!(!is_numbering_template(plain), "{plain:?}");
        }
    }

    #[test]
    fn an_output_name_that_is_not_one_plain_name_is_refused() {
        let mut stems = vec!["../x", "a/b", "/abs", "", "..", ".", "a/", "./a"];
        if cfg!(windows) {
            stems.extend([r"C:\abs", r"a\b", r"..\x", "C:x"]);
        }
        for stem in stems {
            assert_eq!(
                build_args(&SdJob {
                    out_stem: stem.into(),
                    ..klein_job()
                }),
                Err(ArgsError::BadOutputStem),
                "{stem:?}"
            );
        }
        let a = strings(
            build_args(&SdJob {
                out_stem: "run-1".into(),
                ..klein_job()
            })
            .unwrap(),
        );
        assert_eq!(
            after(&a, "-o"),
            p("/g/.partial/r1").join("run-1.png").to_str().unwrap()
        );
    }

    /// The engine reads its int options with `std::stoi` and the seed with `std::stoll`, and
    /// aborts on a number out of their range, so `build_args` refuses one, naming its field.
    #[test]
    fn an_integer_the_engine_cannot_read_is_refused() {
        let int_max = u32::try_from(i32::MAX).unwrap();
        let over = int_max + 1;
        /// A job with one integer field set to the number given.
        type With = fn(u32) -> SdJob;
        let jobs: [(&str, With); 5] = [
            ("steps", |n| SdJob {
                steps: n,
                ..klein_job()
            }),
            ("batch", |n| SdJob {
                batch: n,
                ..klein_job()
            }),
            ("frames", |n| SdJob {
                frames: Some(n),
                ..video()
            }),
            ("fps", |n| SdJob {
                fps: Some(n),
                ..video()
            }),
            ("threads", |n| SdJob {
                threads: Some(n),
                ..klein_job()
            }),
        ];
        for (field, with) in jobs {
            let a = strings(build_args(&with(int_max)).unwrap());
            assert!(a.contains(&int_max.to_string()), "{field}: {a:?}");
            for n in [over, u32::MAX] {
                assert_eq!(
                    build_args(&with(n)),
                    Err(ArgsError::NumberOutOfRange(field)),
                    "{field} = {n}"
                );
            }
        }
        let seed_max = u64::try_from(i64::MAX).unwrap();
        let a = strings(
            build_args(&SdJob {
                seed: seed_max,
                ..klein_job()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--seed"), seed_max.to_string());
        for seed in [seed_max + 1, u64::MAX] {
            assert_eq!(
                build_args(&SdJob {
                    seed,
                    ..klein_job()
                }),
                Err(ArgsError::NumberOutOfRange("seed")),
                "{seed}"
            );
        }
    }

    #[test]
    fn a_guidance_or_flow_shift_that_is_not_finite_or_is_subnormal_is_refused() {
        let subnormal = f32::MIN_POSITIVE / 2.0;
        assert!(subnormal.is_finite() && subnormal != 0.0 && !subnormal.is_normal());
        for bad in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            subnormal,
            -subnormal,
            f32::from_bits(1),
        ] {
            assert_eq!(
                build_args(&SdJob {
                    cfg: bad,
                    ..klein_job()
                }),
                Err(ArgsError::NumberOutOfRange("cfg")),
                "{bad}"
            );
            assert_eq!(
                build_args(&SdJob {
                    flow_shift: Some(bad),
                    ..video()
                }),
                Err(ArgsError::NumberOutOfRange("flow_shift")),
                "{bad}"
            );
        }
        let a = strings(
            build_args(&SdJob {
                cfg: f32::MAX,
                flow_shift: Some(-0.5),
                ..video()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--cfg-scale"), f32::MAX.to_string());
        assert_eq!(after(&a, "--flow-shift"), "-0.5");
        // 0 (either sign) and the smallest normal number are read as given.
        for good in [0.0, -0.0, f32::MIN_POSITIVE] {
            let a = strings(
                build_args(&SdJob {
                    cfg: good,
                    flow_shift: Some(good),
                    ..video()
                })
                .unwrap(),
            );
            assert_eq!(after(&a, "--cfg-scale"), good.to_string());
            assert_eq!(after(&a, "--flow-shift"), good.to_string());
        }
    }

    #[test]
    fn the_metadata_flag_is_always_there() {
        for job in [klein_job(), video()] {
            assert!(
                strings(build_args(&job).unwrap()).contains(&"--disable-image-metadata".into())
            );
        }
    }

    #[test]
    fn tiling_and_offload_are_each_their_own_and_processor_only_picks_cpu_and_threads() {
        let a = strings(
            build_args(&SdJob {
                vae_tiling: true,
                offload_to_cpu: true,
                ..klein_job()
            })
            .unwrap(),
        );
        assert!(a.contains(&"--vae-tiling".into()) && a.contains(&"--offload-to-cpu".into()));
        // tiling alone: a video or a picture above 1024×1024 on the engine's own auto-fit
        let a = strings(
            build_args(&SdJob {
                vae_tiling: true,
                ..klein_job()
            })
            .unwrap(),
        );
        assert!(a.contains(&"--vae-tiling".into()) && !a.contains(&"--offload-to-cpu".into()));
        // offload alone: Memory saving where the recipe disables tiling
        let a = strings(
            build_args(&SdJob {
                offload_to_cpu: true,
                tiling_allowed: false,
                ..klein_job()
            })
            .unwrap(),
        );
        assert!(!a.contains(&"--vae-tiling".into()) && a.contains(&"--offload-to-cpu".into()));
        let a = strings(
            build_args(&SdJob {
                processor_only: true,
                threads: Some(5),
                backend: Some("vulkan0".into()),
                ..klein_job()
            })
            .unwrap(),
        );
        assert_eq!(
            (after(&a, "--backend"), after(&a, "--threads")),
            ("cpu", "5")
        );
        assert_eq!(
            (
                processor_threads(12, true),
                processor_threads(10, false),
                processor_threads(2, true)
            ),
            (5, 9, 1)
        );
        assert_eq!(
            (processor_threads(1, false), processor_threads(0, true)),
            (1, 1)
        );
    }

    /// The four recipes that disable `vae_tiling` are never tiled, whatever built the job
    /// (ruling S2-tiling-gate).
    #[test]
    fn a_recipe_that_disables_tiling_is_never_tiled() {
        for offload_to_cpu in [false, true] {
            let a = strings(
                build_args(&SdJob {
                    vae_tiling: true,
                    offload_to_cpu,
                    tiling_allowed: false,
                    ..klein_job()
                })
                .unwrap(),
            );
            assert!(!a.contains(&"--vae-tiling".into()), "{a:?}");
            assert_eq!(a.contains(&"--offload-to-cpu".into()), offload_to_cpu);
        }
    }

    #[test]
    fn the_probed_device_is_passed_and_no_device_means_the_engines_choice() {
        let a = strings(
            build_args(&SdJob {
                backend: Some("vulkan0".into()),
                ..klein_job()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--backend"), "vulkan0");
        let a = strings(build_args(&klein_job()).unwrap());
        assert!(!a.contains(&"--backend".into()) && !a.contains(&"--threads".into()));
        assert!(!a.contains(&"--vae-tiling".into()) && !a.contains(&"--offload-to-cpu".into()));
    }

    #[test]
    fn no_scheduler_means_the_engines_own() {
        let a = strings(
            build_args(&SdJob {
                scheduler: None,
                sampler: None,
                ..klein_job()
            })
            .unwrap(),
        );
        assert!(!a.contains(&"--scheduler".into()) && !a.contains(&"--sampling-method".into()));
        let a = strings(build_args(&klein_job()).unwrap());
        assert_eq!(
            (after(&a, "--sampling-method"), after(&a, "--scheduler")),
            ("euler", "simple")
        );
    }

    #[test]
    fn a_video_job_asks_for_frames_fps_shift_and_webm() {
        let a = strings(build_args(&video()).unwrap());
        assert_eq!(after(&a, "-M"), "vid_gen");
        assert_eq!(after(&a, "--video-frames"), "49");
        assert_eq!(after(&a, "--fps"), "24");
        assert_eq!(after(&a, "--flow-shift"), "3");
        assert_eq!(after(&a, "-i"), "/g/refs/start.png");
        assert!(after(&a, "-o").ends_with(".webm"), "{a:?}");
        let picture = strings(build_args(&klein_job()).unwrap());
        for flag in ["-M", "--video-frames", "--fps", "--flow-shift", "-i"] {
            assert!(!picture.contains(&flag.into()), "{flag}");
        }
        let webp = SdJob {
            format: OutputFormat::Webp,
            ..video()
        };
        assert!(after(&strings(build_args(&webp).unwrap()), "-o").ends_with(".webp"));
    }

    #[test]
    fn the_arguments_come_in_their_fixed_order() {
        let job = SdJob {
            negative: Some("blurry".into()),
            cfg: 3.5,
            batch: 2,
            ref_images: vec![p("/g/refs/a.png"), p("/g/refs/b.png")],
            backend: Some("vulkan0".into()),
            threads: Some(7),
            vae_tiling: true,
            offload_to_cpu: true,
            ..video()
        };
        let out = |name: &str| job.out_dir.join(name).to_str().unwrap().to_owned();
        assert_eq!(
            strings(build_args(&job).unwrap()),
            [
                "--diffusion-model",
                "/m/wan.gguf",
                "--t5xxl",
                "/m/umt5.gguf",
                "--vae",
                "/m/wan_vae.safetensors",
                "-M",
                "vid_gen",
                "--prompt-file",
                &out("prompt.txt"),
                "--negative-prompt-file",
                &out("negative.txt"),
                "-W",
                "832",
                "-H",
                "480",
                "--steps",
                "8",
                "--cfg-scale",
                "3.5",
                "--flow-shift",
                "3",
                "--seed",
                "81723",
                "--batch-count",
                "2",
                "--video-frames",
                "49",
                "--fps",
                "24",
                "-i",
                "/g/refs/start.png",
                "-r",
                "/g/refs/a.png",
                "-r",
                "/g/refs/b.png",
                "--backend",
                "vulkan0",
                "--threads",
                "7",
                "--vae-tiling",
                "--offload-to-cpu",
                "-o",
                &out("r1.webm"),
                "--disable-image-metadata",
                "--log-level",
                "info",
            ]
        );
    }

    #[test]
    fn a_job_needs_a_diffusion_model() {
        for parts in [
            vec![
                (PartRole::Llm, p("/m/q.gguf")),
                (PartRole::Vae, p("/m/v.gguf")),
            ],
            vec![(PartRole::HighNoiseDiffusion, p("/m/high.gguf"))],
            Vec::new(),
        ] {
            assert_eq!(
                build_args(&job_with(parts)),
                Err(ArgsError::NoDiffusionPart)
            );
        }
        let a = strings(
            build_args(&job_with(vec![(PartRole::Single, p("/m/all.safetensors"))])).unwrap(),
        );
        assert_eq!(&a[..2], ["-m", "/m/all.safetensors"]);
    }

    #[test]
    fn sizes_are_multiples_of_the_jobs_multiple_from_64_to_2048() {
        for (width, height) in [(64, 64), (2048, 2048), (832, 480)] {
            assert!(
                build_args(&SdJob {
                    width,
                    height,
                    ..klein_job()
                })
                .is_ok(),
                "{width}x{height}"
            );
        }
        for (width, height) in [(1000, 1024), (1024, 1000), (48, 64), (64, 2064), (0, 0)] {
            assert_eq!(
                build_args(&SdJob {
                    width,
                    height,
                    ..klein_job()
                }),
                Err(ArgsError::BadSize { multiple: 16 }),
                "{width}x{height}"
            );
        }
    }

    /// A path that is not valid Unicode: never converted lossily, always refused.
    #[cfg(unix)]
    fn not_unicode() -> PathBuf {
        use std::os::unix::ffi::OsStrExt;
        PathBuf::from(std::ffi::OsStr::from_bytes(b"/m/\xff.gguf"))
    }

    /// A path that is not valid Unicode: never converted lossily, always refused.
    #[cfg(windows)]
    fn not_unicode() -> PathBuf {
        use std::os::windows::ffi::OsStringExt;
        PathBuf::from(OsString::from_wide(&[0x43, 0x3a, 0x5c, 0xd800]))
    }

    #[test]
    fn a_path_that_is_not_unicode_is_refused() {
        let bad = not_unicode();
        let jobs = [
            job_with(vec![(PartRole::Diffusion, bad.clone())]),
            SdJob {
                init_image: Some(bad.clone()),
                ..klein_job()
            },
            SdJob {
                ref_images: vec![p("/g/refs/a.png"), bad.clone()],
                ..klein_job()
            },
            SdJob {
                out_dir: bad.clone(),
                ..klein_job()
            },
            SdJob {
                end_image: Some(bad.clone()),
                ..video()
            },
            SdJob {
                init_image: Some(p("/g/refs/in.png")),
                mask_image: Some(bad.clone()),
                ..klein_job()
            },
            SdJob {
                control_image: Some(bad.clone()),
                ..klein_job()
            },
            SdJob {
                lora_dir: Some(bad.clone()),
                ..klein_job()
            },
        ];
        for job in jobs {
            assert_eq!(build_args(&job), Err(ArgsError::NotUnicode(bad.clone())));
        }
    }

    #[test]
    fn the_whitelisted_words_are_the_pinned_cli_words() {
        let samplers: Vec<&str> = Sampler::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            samplers,
            [
                "euler",
                "euler_a",
                "heun",
                "dpm2",
                "dpm++2s_a",
                "dpm++2m",
                "dpm++2mv2",
                "ipndm",
                "ipndm_v",
                "lcm",
                "ddim_trailing",
                "tcd",
                "res_multistep",
                "res_2s",
                "er_sde",
                "euler_cfg_pp",
                "euler_a_cfg_pp",
                "euler_ge",
                "dpm++2m_sde",
                "dpm++2m_sde_bt",
                "lms"
            ]
        );
        let schedulers: Vec<&str> = Scheduler::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(
            schedulers,
            [
                "discrete",
                "karras",
                "exponential",
                "ays",
                "gits",
                "sgm_uniform",
                "simple",
                "smoothstep",
                "kl_optimal",
                "lcm",
                "bong_tangent",
                "ltx2",
                "logit_normal",
                "flux2",
                "flux",
                "beta",
                "llada_image"
            ]
        );
        for sampler in Sampler::ALL {
            assert_eq!(Sampler::from_cli(sampler.as_str()), Some(*sampler));
        }
        for scheduler in Scheduler::ALL {
            assert_eq!(Scheduler::from_cli(scheduler.as_str()), Some(*scheduler));
        }
        for word in ["", "Euler", "ddim", "euler "] {
            assert_eq!(Sampler::from_cli(word), None, "{word:?}");
        }
        for word in ["", "Karras", "normal", "auto"] {
            assert_eq!(Scheduler::from_cli(word), None, "{word:?}");
        }
    }

    /// A job with every field set: each part role, a negative prompt, every recipe field, the
    /// four input pictures and two references, a LoRA, a device, threads, tiling and offload.
    fn everything() -> SdJob {
        SdJob {
            parts: every_part(),
            negative: Some("blurry".into()),
            guidance: Some(3.5),
            img_cfg: Some(1.5),
            timestep_shift: Some(250),
            sampler: Some(Sampler::DpmPp2m),
            scheduler: Some(Scheduler::Karras),
            sigmas: vec![1.0, 0.5, 0.0],
            high_noise: Some(HighNoise {
                steps: -1,
                cfg: 4.0,
                sampler: Some(Sampler::Euler),
            }),
            moe_boundary: Some(0.875),
            strength: Some(0.75),
            end_image: Some(p("/g/refs/end.png")),
            mask_image: Some(p("/g/refs/mask.png")),
            control_image: Some(p("/g/refs/edges.png")),
            ref_images: vec![p("/g/refs/a.png"), p("/g/refs/b.png")],
            vae_format: Some(VaeFormat::Wan),
            model_args: Some("qwen_image_zero_cond_t=true".into()),
            lora_dir: Some(p("/m/@loras")),
            loras: vec![LoraTag {
                name: "style".into(),
                weight: 0.8,
                high_noise: false,
            }],
            backend: Some("vulkan0".into()),
            threads: Some(3),
            vae_tiling: true,
            offload_to_cpu: true,
            ..video()
        }
    }

    /// The flags of `args`, each once (the `-r` of each reference read as one): every flag
    /// but the three switches takes the next argument as its value, as the pinned parser
    /// reads them (a value may start with `-`, as `--high-noise-steps -1` does).
    fn flags_of(args: &[String]) -> Vec<&str> {
        const SWITCHES: [&str; 3] = [
            "--vae-tiling",
            "--offload-to-cpu",
            "--disable-image-metadata",
        ];
        let mut flags = Vec::new();
        let mut rest = args.iter();
        while let Some(flag) = rest.next() {
            if !SWITCHES.contains(&flag.as_str()) {
                rest.next().unwrap_or_else(|| panic!("{flag} has no value"));
            }
            flags.push(flag.as_str());
        }
        flags.dedup();
        flags
    }

    /// `FLAGS` is exactly what `build_args` can emit, in its order: a job with every field set
    /// gives each flag in turn, and nothing outside the list.
    #[test]
    fn flags_lists_every_flag_build_args_emits() {
        let a = strings(build_args(&everything()).unwrap());
        assert_eq!(flags_of(&a), FLAGS);
    }

    #[test]
    fn every_flag_is_in_the_help_text() {
        let help = include_str!("../../tests/fixtures/sd_help.txt");
        assert_eq!(missing_from_help(help), Vec::<&str>::new());
        for flag in FLAGS {
            assert!(help.contains(flag), "{flag}");
        }
    }

    #[test]
    fn the_help_check_reads_option_names_and_choices_whole() {
        // `--fps-max` does not list `--fps`, nor does a description that mentions it.
        let help = "Generation Options:\n  --fps-max <int>             most frames a second\n  \
                    --sampling-method           sampling method, one of [euler, heun,\n                              \
                    dpm2], default: euler (see --fps)\n";
        let missing = missing_from_help(help);
        assert!(
            missing.contains(&"--fps") && missing.contains(&"-o"),
            "{missing:?}"
        );
        assert!(!missing.contains(&"--sampling-method"), "{missing:?}");
        for word in ["euler_a", "dpm++2m", "lcm", "simple"] {
            assert!(missing.contains(&word), "{word}: {missing:?}");
        }
        for word in ["euler", "heun", "dpm2"] {
            assert!(!missing.contains(&word), "{word}: {missing:?}");
        }
    }

    #[test]
    fn recipe_fields_reach_their_flags_in_order() {
        let mut j = wan_a14b_job();
        j.guidance = Some(3.5);
        j.high_noise = Some(HighNoise {
            steps: -1,
            cfg: 4.0,
            sampler: Some(Sampler::Euler),
        });
        j.moe_boundary = Some(0.875);
        j.sigmas = vec![1.0, 0.5, 0.0];
        j.vae_format = Some(VaeFormat::Wan);
        j.model_args = Some("qwen_image_zero_cond_t=true".into());
        let a = strings(build_args(&j).unwrap());
        assert_eq!(after(&a, "--high-noise-steps"), "-1");
        assert_eq!(after(&a, "--sigmas"), "1,0.5,0");
        assert!(
            index(&a, "--guidance") < index(&a, "--flow-shift")
                && index(&a, "--moe-boundary") < index(&a, "--seed")
        );
        assert_eq!(after(&a, "-M"), "vid_gen");
        assert_eq!(
            (
                after(&a, "--high-noise-cfg-scale"),
                after(&a, "--high-noise-sampling-method"),
                after(&a, "--moe-boundary"),
            ),
            ("4", "euler", "0.875")
        );
        assert_eq!(
            (after(&a, "--vae-format"), after(&a, "--model-args")),
            ("wan", "qwen_image_zero_cond_t=true")
        );
        // the fields left empty send nothing; the engine's `auto` VAE format is never sent
        for flag in [
            "--img-cfg-scale",
            "--timestep-shift",
            "--strength",
            "--lora-model-dir",
        ] {
            assert!(!a.contains(&flag.to_owned()), "{flag}");
        }
        let a = strings(build_args(&wan_a14b_job()).unwrap());
        for flag in [
            "--guidance",
            "--sigmas",
            "--high-noise-steps",
            "--vae-format",
        ] {
            assert!(!a.contains(&flag.to_owned()), "{flag}");
        }
        assert_eq!(
            [
                VaeFormat::Flux,
                VaeFormat::Sd3,
                VaeFormat::Flux2,
                VaeFormat::Wan
            ]
            .map(VaeFormat::as_str),
            ["flux", "sd3", "flux2", "wan"]
        );
        // a high-noise part without its own sampler leaves the engine its own
        let a = strings(
            build_args(&SdJob {
                high_noise: Some(HighNoise {
                    steps: 8,
                    cfg: 3.5,
                    sampler: None,
                }),
                ..wan_a14b_job()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--high-noise-steps"), "8");
        assert!(!a.contains(&"--high-noise-sampling-method".to_owned()));
    }

    #[test]
    fn the_size_follows_the_recipes_multiple() {
        let mut j = klein_job();
        j.size_multiple = 64;
        j.width = 1000;
        assert_eq!(build_args(&j), Err(ArgsError::BadSize { multiple: 64 }));
        j.size_multiple = 8;
        assert!(build_args(&j).is_ok());
        j.size_multiple = 0;
        assert_eq!(build_args(&j), Err(ArgsError::BadSize { multiple: 0 }));
        j.width = 1024;
        assert_eq!(build_args(&j), Err(ArgsError::BadSize { multiple: 0 }));
        // still from 64 to 2048, whatever the multiple
        for (multiple, side) in [(8, 56), (8, 2056), (1, 63)] {
            let j = SdJob {
                size_multiple: multiple,
                height: side,
                ..klein_job()
            };
            assert_eq!(
                build_args(&j),
                Err(ArgsError::BadSize { multiple }),
                "{side}"
            );
        }
    }

    #[test]
    fn every_sampler_and_scheduler_is_the_engines_word() {
        // against the help's own "one of [...]" lists, not a loose `contains`
        let help = include_str!("../../tests/fixtures/sd_help.txt");
        let choices = |flag: &str| -> Vec<String> {
            help_options(help)
                .into_iter()
                .find(|o| o.names.contains(&flag))
                .unwrap()
                .choices()
                .into_iter()
                .map(str::to_owned)
                .collect()
        };
        // 21, in the engine's order
        assert_eq!(
            Sampler::ALL.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            choices("--sampling-method")
        );
        assert_eq!(
            choices("--high-noise-sampling-method"),
            choices("--sampling-method")
        );
        // 17
        assert_eq!(
            Scheduler::ALL
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            choices("--scheduler")
        );
        assert_eq!((Sampler::ALL.len(), Scheduler::ALL.len()), (21, 17));
    }

    #[test]
    fn typed_lora_tags_are_removed_and_attached_ones_appended() {
        let mut j = klein_job();
        j.prompt = "a cat <lora:evil:3> on a mat".into();
        j.lora_dir = Some("/m/@loras".into());
        j.loras = vec![LoraTag {
            name: "local/pixel art ü".into(),
            weight: 0.8,
            high_noise: false,
        }];
        assert_eq!(
            prompt_file_text(&j),
            "a cat  on a mat <lora:local/pixel art ü:0.8>"
        );
        assert_eq!(
            after(&strings(build_args(&j).unwrap()), "--lora-model-dir"),
            "/m/@loras"
        );
        // without LoRAs the prompt is written as given, typed tags still removed
        j.loras.clear();
        assert_eq!(prompt_file_text(&j), "a cat  on a mat");
        j.prompt = "two lines\nand 風".into();
        assert_eq!(prompt_file_text(&j), j.prompt);
        // a prompt that is only typed tags is empty
        j.prompt = " <lora:x:1> ".into();
        assert_eq!(build_args(&j), Err(ArgsError::EmptyPrompt));
        // with a LoRA appended, the engine reads only the tag appended
        let j = SdJob {
            prompt: "a fox <lora:evil:3".into(),
            lora_dir: Some(p("/m/@loras")),
            loras: vec![LoraTag {
                name: "x".into(),
                weight: 1.0,
                high_noise: false,
            }],
            ..klein_job()
        };
        let text = prompt_file_text(&j);
        assert_eq!(text, "a fox evil:3 <lora:x:1>");
        assert_eq!(text.matches("<lora:").count(), 1);
    }

    #[test]
    fn mask_and_control_pictures_reach_their_flags() {
        let mut j = klein_job();
        j.init_image = Some("/r/in.png".into());
        j.mask_image = Some("/r/mask.png".into());
        j.control_image = Some("/r/edges.png".into());
        let a = strings(build_args(&j).unwrap());
        assert_eq!(
            (
                after(&a, "-i"),
                after(&a, "--mask"),
                after(&a, "--control-image")
            ),
            ("/r/in.png", "/r/mask.png", "/r/edges.png")
        );
        // the ControlNet strength is never sent
        assert!(!a.contains(&"--control-strength".to_owned()));
        j.init_image = None;
        assert_eq!(build_args(&j), Err(ArgsError::MaskWithoutPicture));
        // a control picture needs no picture to change; an end frame follows the start frame
        j.mask_image = None;
        assert!(build_args(&j).is_ok());
        let a = strings(
            build_args(&SdJob {
                end_image: Some(p("/g/refs/end.png")),
                ..video()
            })
            .unwrap(),
        );
        assert_eq!(after(&a, "--end-img"), "/g/refs/end.png");
        assert_eq!(index(&a, "-i") + 2, index(&a, "--end-img"));
    }

    #[test]
    fn lora_names_the_engine_could_misread_are_refused() {
        for bad in [
            "",
            "C:/x",
            "a>b",
            "a<b",
            "|high_noise|x",
            "../x",
            "a\\b",
            "a/./b",
        ] {
            assert!(!valid_lora_name(bad), "{bad}");
        }
        for bad in ["/abs", "a//b", "a/", "a/..", ".", "a\nb", "a\u{7f}b", "x:1"] {
            assert!(!valid_lora_name(bad), "{bad:?}");
        }
        for good in ["local/pixel art ü", "o/r/日本語 スタイル", "x"] {
            assert!(valid_lora_name(good), "{good}");
        }
        let mut j = klein_job();
        j.loras = vec![LoraTag {
            name: "x".into(),
            weight: 1.0,
            high_noise: false,
        }];
        assert_eq!(build_args(&j), Err(ArgsError::LorasWithoutFolder));
        j.lora_dir = Some("/m/@loras".into());
        j.loras[0].weight = 2.5;
        assert_eq!(
            build_args(&j),
            Err(ArgsError::NumberOutOfRange("lora weight"))
        );
        for weight in [-1.01, f32::NAN, f32::INFINITY, f32::MIN_POSITIVE / 2.0] {
            j.loras[0].weight = weight;
            assert_eq!(
                build_args(&j),
                Err(ArgsError::NumberOutOfRange("lora weight")),
                "{weight}"
            );
        }
        for weight in [-1.0, 0.0, 2.0] {
            j.loras[0].weight = weight;
            assert!(build_args(&j).is_ok(), "{weight}");
        }
        j.loras[0].name = "../x".into();
        assert_eq!(build_args(&j), Err(ArgsError::BadLoraName("../x".into())));
    }

    #[test]
    fn at_most_eight_loras() {
        let tag = |i: usize| LoraTag {
            name: format!("l{i}"),
            weight: 1.0,
            high_noise: i % 2 == 1,
        };
        let j = SdJob {
            lora_dir: Some(p("/m/@loras")),
            loras: (0..MAX_LORAS).map(tag).collect(),
            ..klein_job()
        };
        assert!(build_args(&j).is_ok());
        assert_eq!(prompt_file_text(&j).matches("<lora:").count(), 8);
        let j = SdJob {
            loras: (0..=MAX_LORAS).map(tag).collect(),
            ..j
        };
        assert_eq!(build_args(&j), Err(ArgsError::TooManyLoras));
    }

    #[test]
    fn model_args_are_key_value_pairs() {
        for good in [
            "qwen_image_zero_cond_t=true",
            "pixart_vae_scale_factor=0.18215",
            "pixart_vae_scale_factor=0.18215,pixart_pos_embed_base_size=32,pixart_interpolation_scale=1",
        ] {
            let j = SdJob {
                model_args: Some(good.into()),
                ..klein_job()
            };
            assert_eq!(
                after(&strings(build_args(&j).unwrap()), "--model-args"),
                good
            );
        }
        for bad in [
            "",
            "a",
            "=b",
            "a=",
            "a=b,",
            ",a=b",
            "A=b",
            "a=b=c",
            "a=b c",
            "a=-1",
            "a=b;--vae x",
            "a=b,,c=d",
        ] {
            let j = SdJob {
                model_args: Some(bad.into()),
                ..klein_job()
            };
            assert_eq!(build_args(&j), Err(ArgsError::BadModelArgs), "{bad:?}");
        }
    }

    /// The recipe's numbers are refused as the 0.1.x ones are: an integer above `i32::MAX`,
    /// and a float that is not finite or is subnormal, named by its field.
    #[test]
    fn a_recipe_number_the_engine_cannot_read_is_refused() {
        let over = u32::try_from(i32::MAX).unwrap() + 1;
        assert_eq!(
            build_args(&SdJob {
                timestep_shift: Some(over),
                ..klein_job()
            }),
            Err(ArgsError::NumberOutOfRange("timestep_shift"))
        );
        let subnormal = f32::MIN_POSITIVE / 2.0;
        type With = fn(f32) -> SdJob;
        let jobs: [(&str, With); 7] = [
            ("guidance", |x| SdJob {
                guidance: Some(x),
                ..klein_job()
            }),
            ("img_cfg", |x| SdJob {
                img_cfg: Some(x),
                ..klein_job()
            }),
            ("high_noise_cfg", |x| SdJob {
                high_noise: Some(HighNoise {
                    steps: -1,
                    cfg: x,
                    sampler: None,
                }),
                ..wan_a14b_job()
            }),
            ("moe_boundary", |x| SdJob {
                moe_boundary: Some(x),
                ..wan_a14b_job()
            }),
            ("strength", |x| SdJob {
                strength: Some(x),
                ..klein_job()
            }),
            ("sigmas", |x| SdJob {
                sigmas: vec![1.0, x, 0.0],
                ..klein_job()
            }),
            ("cfg", |x| SdJob {
                cfg: x,
                ..klein_job()
            }),
        ];
        for (field, with) in jobs {
            for bad in [f32::NAN, f32::INFINITY, subnormal] {
                assert_eq!(
                    build_args(&with(bad)),
                    Err(ArgsError::NumberOutOfRange(field)),
                    "{field} = {bad}"
                );
            }
            assert!(build_args(&with(0.5)).is_ok(), "{field}");
        }
    }

    /// A recipe's `vae_format` word reads back as its format; the engine's own `auto` and any
    /// other word as none, which leaves the format to the engine.
    #[test]
    fn a_recipes_vae_format_word_reads_back() {
        for format in [
            VaeFormat::Flux,
            VaeFormat::Sd3,
            VaeFormat::Flux2,
            VaeFormat::Wan,
        ] {
            assert_eq!(VaeFormat::from_cli(format.as_str()), Some(format));
        }
        assert_eq!(VaeFormat::from_cli("auto"), None);
        assert_eq!(VaeFormat::from_cli("Flux"), None);
    }

    /// The engine reads a `--tokenizer` value that holds `=` as `slot=FILE` assignments split
    /// at `,` (`src/tokenizers/tokenizer_config.cpp:13` at the pinned commit): a path with `=`
    /// goes as the main slot's assignment, and one that also holds `,` is refused.
    #[test]
    fn a_tokenizer_path_with_an_equals_sign_is_passed_as_the_main_slot() {
        let tokenizer = |path: &str| {
            let mut job = klein_job();
            job.parts.push((PartRole::Tokenizer, p(path)));
            job
        };
        let a = strings(build_args(&tokenizer("/m/tokenizer.json")).unwrap());
        assert_eq!(after(&a, "--tokenizer"), "/m/tokenizer.json");
        // A comma alone is read as part of the one path.
        let a = strings(build_args(&tokenizer("/m/a,b/tokenizer.json")).unwrap());
        assert_eq!(after(&a, "--tokenizer"), "/m/a,b/tokenizer.json");
        let a = strings(build_args(&tokenizer("/m/size=4/tokenizer.json")).unwrap());
        assert_eq!(after(&a, "--tokenizer"), "main=/m/size=4/tokenizer.json");
        assert_eq!(
            build_args(&tokenizer("/m/a=1,b/tokenizer.json")),
            Err(ArgsError::TokenizerPath)
        );
        // Only the tokenizer: another part's path is passed as it is.
        let a = strings(
            build_args(&job_with(vec![
                (PartRole::Diffusion, p("/m/x=1,y/d.gguf")),
                (PartRole::Llm, p("/m/q.gguf")),
            ]))
            .unwrap(),
        );
        assert_eq!(after(&a, "--diffusion-model"), "/m/x=1,y/d.gguf");
    }
}
