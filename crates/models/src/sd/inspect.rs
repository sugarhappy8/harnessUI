//! What a weights file is (design section 7.2): a model and its layout, a LoRA, an extra
//! part for a recipe's slot, or a file Studio cannot run, and why.
//!
//! [`inspect`] checks, in this order:
//! 1. the LoRA names: a file with them is a LoRA whatever its other names, because the
//!    engine would read XLabs' `diffusion_model.double_blocks.….lora_down.weight` as FLUX;
//! 2. the layout, and the engine's detection ([`detect_version`]) once, in the slot the
//!    layout names;
//! 3. the guards: versions the engine names but Studio does not run;
//! 4. the extra-file markers;
//! 5. otherwise, unknown weights.
//!
//! A LoRA's base comes first from the SD rows (CLIP-G's `te2`, a UNet's cross-attention
//! width, SD 3's `joint_blocks`). After them come the DiT families, matched by layer name,
//! block number and the LoRA weights' widths. The primary families are told apart by:
//!
//! | Family | Width | Blocks | Also separating | Sizes from |
//! |---|---|---|---|---|
//! | FLUX.1 | 3072 | 19 double, 38 single | per-block `img_mod`/`modulation`; 4x MLP (`img_mlp.0` → 12288, `linear1` → 21504) | `flux.hpp:42-47`, fixture flux1-dev |
//! | Klein 4B | 3072 | 5, 20 | shared `double_stream_modulation_*`; gated 3x MLP (`img_mlp.0` → 18432, `linear1` → 27648) | fixture flux2-klein-4b |
//! | Klein 9B | 4096 | 8, 24 | as Klein 4B | 0.1.x klein9b table |
//! | FLUX.2 dev | 6144 | 8, 48 | as Klein 4B | black-forest-labs/FLUX.2-dev transformer/config.json |
//! | HunyuanVideo 1.5 | 2048 | 54 double | FLUX's names, or Tencent's `img_mlp.fc1`/`img_mod.linear` | `hunyuan.hpp:165-171` |
//! | Qwen-Image | 3072 | 60 | `img_mod.1`, `img_mlp.net.*`; no single blocks | fixture qwen-image |
//! | Z-Image | 3840 | 30 + 2 + 2 | `attention.qkv`/`out`, `feed_forward.w1-3` (10240) | `z_image.hpp:34-37`, fixture z-image-turbo |
//! | Wan 1.3B / 14B | 1536 / 5120 | 30 / 40 | `self_attn.q`, `cross_attn.k_img`, `ffn.0` | fixtures wan21-t2v-1.3b, wan21-i2v-14b |
//! | Wan 2.2 TI2V-5B | 3072 | 30 | as Wan | fixture wan22-ti2v-5b |
//! | LTX-2.3 | 4096 (audio 2048) | 48 | `attn1`/`attn2`, `audio_*`, `ff` 16384 | fixture ltx23 |
//!
//! One primary family left is the answer, and two are None. A family that shares a primary
//! family's names (Ovis, LLaDA, Lens, PixArt and others) is named only from a layer no other
//! family has. Longcat, SeFi, MageFlow, Ming and the MiniMax-H3 models have no such layer:
//! their LoRAs read as the primary family they match, or as None.

use std::collections::BTreeSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sd::detect::{FolderPart, LoadedFile, SdVersion, detect_folder, detect_version};
use crate::sd::index::{FileFormat, IndexError, TensorIndex, engine_ne, read_local};
use crate::sd::recipes::SlotId;
use crate::sd::{CannotRunReason, FileFacts};

/// How a model file is given to the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// One file with the diffusion model's full names or a checkpoint's own parts: `-m`,
    /// which loads it with the prefix `""`.
    Checkpoint,
    /// The diffusion model alone: `--diffusion-model`, which prefixes its names with
    /// `model.diffusion_model.`.
    DiffusionOnly,
    /// An SD 1.x, 2.x or XL diffusers folder, run with `-m <folder>`.
    Folder,
}

/// A LoRA's naming, by the engine's LoRA suffixes (`src/name_conversion.cpp:1676-1709`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoraFormat {
    Kohya,
    Peft,
    Lycoris,
    Diff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FileKind {
    /// A model the engine detects as `version`. `embeds` are the slots the file's own parts
    /// fill, in [`SlotId`] order.
    Model {
        version: SdVersion,
        layout: Layout,
        embeds: Vec<SlotId>,
    },
    /// A LoRA, with the version it was most likely made for.
    Lora {
        format: LoraFormat,
        base: Option<SdVersion>,
    },
    /// A part a recipe's slot takes: a text encoder, a VAE, …
    Extra { slot: SlotId },
    /// A file Studio does not run, with the version the engine named, if any.
    CannotRun {
        reason: CannotRunReason,
        version: Option<SdVersion>,
    },
}

/// What recipe matching (Task R7) reads beyond the version.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchFacts {
    /// The highest `joint_blocks.N`, plus 1.
    pub sd3_blocks: Option<u32>,
    /// The diffusion model's `img_in.weight` `ne[1]`.
    pub flux2_width: Option<u64>,
    /// The key `__index_timestep_zero__`, under any prefix (Qwen-Image Edit 2511;
    /// `src/pipeline/diffusion_engine.cpp:1330`).
    pub zero_cond: bool,
    /// A name holds `img_emb.emb_pos`.
    pub img_emb_pos: bool,
    /// A name holds `img_emb`.
    pub has_img_emb: bool,
}

/// The four files of a diffusers folder, by the engine's names: part, directory, file
/// (`init_from_diffusers_file`, `src/model_loader.cpp:421-441`).
pub const FOLDER_FILES: [(FolderPart, &str, &str); 4] = [
    (
        FolderPart::Unet,
        "unet",
        "diffusion_pytorch_model.safetensors",
    ),
    (
        FolderPart::Vae,
        "vae",
        "diffusion_pytorch_model.safetensors",
    ),
    (FolderPart::TextEncoder, "text_encoder", "model.safetensors"),
    (
        FolderPart::TextEncoder2,
        "text_encoder_2",
        "model.safetensors",
    ),
];

/// The prefixes the engine reads as the diffusion model's in a checkpoint: its own, and
/// those `convert_tensor_name` maps onto it (`src/name_conversion.cpp:1748-1779`). A name
/// with one is a diffusion marker for the extra-file table.
const DIT_PREFIXES: [&str; 4] = [
    "model.diffusion_model.",
    "diffusion_model.",
    "unet.",
    "transformer.",
];

/// What the file `index` was read from is.
pub fn inspect(index: &TensorIndex) -> FileFacts {
    let layout = layout(index);
    let kind = match lora_format(index) {
        Some(format) => FileKind::Lora {
            format,
            base: lora_base(index),
        },
        None => {
            // The engine's own split: `-m` loads with the prefix "", `--diffusion-model` with
            // `model.diffusion_model.`, which would wrongly prefix a checkpoint's encoders.
            let (slot, embeds) = match layout {
                Layout::Checkpoint => (SlotId::Model, embeds(index)),
                _ => (SlotId::DiffusionModel, Vec::new()),
            };
            match detect_version(&[LoadedFile { slot, index }]) {
                Some(version) => model(version, layout, embeds, |name| dit(index, layout, name)),
                None => match extra_slot(index) {
                    Some(slot) => FileKind::Extra { slot },
                    None => FileKind::CannotRun {
                        reason: CannotRunReason::UnknownWeights,
                        version: None,
                    },
                },
            }
        }
    };
    FileFacts {
        kind,
        facts: match_facts(index, layout),
        parameters: index.parameters(),
        format: index.format,
    }
}

/// What a diffusers folder of `parts` is: detected as the engine detects it
/// ([`detect_folder`]), embedding its VAE and text encoders.
pub fn inspect_folder(parts: &[(FolderPart, &TensorIndex)]) -> FileFacts {
    let unet = parts
        .iter()
        .find(|(part, _)| *part == FolderPart::Unet)
        .map(|(_, index)| *index);
    let kind = match (detect_folder(parts), unet) {
        (Some(version), Some(unet)) => {
            let embeds: BTreeSet<SlotId> = parts
                .iter()
                .filter_map(|(part, _)| match part {
                    FolderPart::Unet => None,
                    FolderPart::Vae => Some(SlotId::Vae),
                    FolderPart::TextEncoder => Some(SlotId::ClipL),
                    FolderPart::TextEncoder2 => Some(SlotId::ClipG),
                })
                .collect();
            model(
                version,
                Layout::Folder,
                embeds.into_iter().collect(),
                |name| dit(unet, Layout::Folder, name),
            )
        }
        _ => FileKind::CannotRun {
            reason: CannotRunReason::UnknownWeights,
            version: None,
        },
    };
    FileFacts {
        kind,
        facts: unet.map_or_else(MatchFacts::default, |unet| {
            match_facts(unet, Layout::Folder)
        }),
        parameters: parts
            .iter()
            .map(|(_, index)| index.parameters())
            .fold(0, u64::saturating_add),
        format: unet.map_or(FileFormat::Safetensors, |unet| unet.format),
    }
}

/// What the local file at `path` is, from its header ([`read_local`]). A pickle file is
/// named by its extension and never opened: `CannotRun { Pickle }`, with no parameters and,
/// as nothing was read, `Safetensors` standing in for its format.
pub fn inspect_local(path: &Path) -> Result<FileFacts, IndexError> {
    match read_local(path) {
        Ok(index) => Ok(inspect(&index)),
        Err(IndexError::Pickle) => Ok(FileFacts {
            kind: FileKind::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None,
            },
            facts: MatchFacts::default(),
            parameters: 0,
            format: FileFormat::Safetensors,
        }),
        Err(e) => Err(e),
    }
}

/// What the diffusers folder at `root` is, from the [`FOLDER_FILES`] under it. Like the
/// engine, it needs the UNet and reads the other three where they are
/// (`src/model_loader.cpp:427-439`).
pub fn inspect_local_folder(root: &Path) -> Result<FileFacts, IndexError> {
    let mut read = Vec::new();
    for (part, dir, file) in FOLDER_FILES {
        let path = root.join(dir).join(file);
        if part == FolderPart::Unet || path.is_file() {
            read.push((part, read_local(&path)?));
        }
    }
    let parts: Vec<(FolderPart, &TensorIndex)> =
        read.iter().map(|(part, index)| (*part, index)).collect();
    Ok(inspect_folder(&parts))
}

/// A checkpoint has the diffusion model's full names, or a part only a checkpoint carries.
fn layout(index: &TensorIndex) -> Layout {
    const CHECKPOINT: [&str; 4] = [
        "model.diffusion_model.",
        "first_stage_model.",
        "cond_stage_model.",
        "conditioner.",
    ];
    if index
        .tensors
        .iter()
        .any(|t| CHECKPOINT.iter().any(|p| t.name.starts_with(p)))
    {
        Layout::Checkpoint
    } else {
        Layout::DiffusionOnly
    }
}

/// A checkpoint's own parts by name prefix. A name takes its first match, so
/// `cond_stage_model.1.` is `clip_g` before `cond_stage_model.` is `clip_l`.
const EMBEDDED: [(&str, SlotId); 10] = [
    ("first_stage_model.", SlotId::Vae),
    ("vae.", SlotId::Vae),
    ("conditioner.embedders.1.", SlotId::ClipG),
    ("cond_stage_model.1.", SlotId::ClipG),
    ("text_encoders.clip_g.", SlotId::ClipG),
    ("cond_stage_model.", SlotId::ClipL),
    ("conditioner.embedders.0.", SlotId::ClipL),
    ("text_encoders.clip_l.", SlotId::ClipL),
    ("text_encoders.t5xxl.", SlotId::T5xxl),
    ("text_encoders.llm.", SlotId::Llm),
];

fn embeds(index: &TensorIndex) -> Vec<SlotId> {
    let slots: BTreeSet<SlotId> = index
        .tensors
        .iter()
        .filter_map(|t| {
            EMBEDDED
                .iter()
                .find(|(prefix, _)| t.name.starts_with(prefix))
                .map(|(_, slot)| *slot)
        })
        .collect();
    slots.into_iter().collect()
}

/// `version` as a model, unless a guard refuses it.
fn model(
    version: SdVersion,
    layout: Layout,
    embeds: Vec<SlotId>,
    dit: impl Fn(&str) -> Option<[u64; 4]>,
) -> FileKind {
    match guard(version, dit) {
        Some(reason) => FileKind::CannotRun {
            reason,
            version: Some(version),
        },
        None => FileKind::Model {
            version,
            layout,
            embeds,
        },
    }
}

/// The versions the engine names for files Studio does not run. `dit` gives the `ne` of a
/// diffusion-model tensor by its name under `model.diffusion_model.`.
fn guard(version: SdVersion, dit: impl Fn(&str) -> Option<[u64; 4]>) -> Option<CannotRunReason> {
    match version {
        // Lumina 2's caption embedder is Gemma-2-2B wide (2304); Z-Image's is 2560.
        SdVersion::ZImage if dit("cap_embedder.0.weight").is_some_and(|ne| ne[0] == 2304) => {
            Some(CannotRunReason::Lumina2)
        }
        SdVersion::Ltxav if dit("audio_patchify_proj.weight").is_none() => {
            Some(CannotRunReason::LtxVideo09)
        }
        // HunyuanVideo 1.5 has the ByT5 input, version 1 has not.
        SdVersion::HunyuanVideo if dit("byt5_in.fc1.weight").is_none() => {
            Some(CannotRunReason::HunyuanVideo1)
        }
        SdVersion::Svd => Some(CannotRunReason::Svd),
        _ => None,
    }
}

/// `name` under `model.diffusion_model.` as the engine holds it in `layout`'s slot; `None`
/// for a checkpoint's other parts.
fn dit_name(name: &str, layout: Layout) -> Option<&str> {
    match layout {
        // The slot's prefix is added where missing (`src/model_loader.cpp:405-411`).
        Layout::DiffusionOnly => Some(name.strip_prefix(DIT_PREFIXES[0]).unwrap_or(name)),
        Layout::Checkpoint => DIT_PREFIXES.iter().find_map(|p| name.strip_prefix(p)),
        // A folder's UNet, whose `unet.` prefix reads as the diffusion model's.
        Layout::Folder => Some(name),
    }
}

/// The `ne` of the diffusion-model tensor `name` (relative to `model.diffusion_model.`).
fn dit(index: &TensorIndex, layout: Layout, name: &str) -> Option<[u64; 4]> {
    index
        .tensors
        .iter()
        .find(|t| dit_name(&t.name, layout) == Some(name))
        .map(|t| engine_ne(t, index.format))
}

/// The first format whose names the file has.
fn lora_format(index: &TensorIndex) -> Option<LoraFormat> {
    let any = |test: &dyn Fn(&str) -> bool| index.tensors.iter().any(|t| test(&t.name));
    let ends = |suffixes: &[&str]| any(&|n| suffixes.iter().any(|s| n.ends_with(s)));
    let holds = |parts: &[&str]| any(&|n| parts.iter().any(|p| n.contains(p)));
    if ends(&[
        ".lora_down.weight",
        ".lora_up.weight",
        ".lora.down.weight",
        "_lora.down.weight",
        "_lora.up.weight",
    ]) {
        Some(LoraFormat::Kohya)
    } else if ends(&[".lora_A.weight", ".lora_B.weight", ".lora_A.default.weight"]) {
        Some(LoraFormat::Peft)
    } else if holds(&[".hada_w1_a", ".lokr_w1"]) {
        Some(LoraFormat::Lycoris)
    } else if ends(&[".diff", ".diff_b"]) {
        Some(LoraFormat::Diff)
    } else {
        None
    }
}

/// The version a LoRA was most likely made for. Names are read with `_` as `.`, so kohya's
/// `lora_unet_double_blocks_0_…` meets `double_blocks`. In order: the SD rows (CLIP-G's
/// `te2`, then a UNet's cross-attention width, then SD 3's `joint_blocks`); the DiT families
/// ([`dit_hint`]); the trainer's `ss_base_model_version`; and kohya's `lora_unet_`, which is
/// SD 1's UNet only when no DiT name was found.
fn lora_base(index: &TensorIndex) -> Option<SdVersion> {
    use SdVersion::*;
    let names: Vec<String> = index
        .tensors
        .iter()
        .map(|t| t.name.replace('_', "."))
        .collect();
    let any = |test: &dyn Fn(&str) -> bool| names.iter().any(|n| test(n));
    // A UNet's cross-attention context width: the in-features of `attn2_to_k`'s down weight,
    // `ne[0]` (`shape[1]` as safetensors writes it).
    let context = |width: u64| {
        names
            .iter()
            .zip(&index.tensors)
            .any(|(n, t)| unet_cross_attention_key(n) && engine_ne(t, index.format)[0] == width)
    };
    if any(&|n| n.starts_with("lora.te2.") || n.starts_with("te2.")) || context(2048) {
        return Some(Sdxl);
    }
    if context(1024) {
        return Some(Sd2);
    }
    if context(768) {
        return Some(Sd1);
    }
    if any(&|n| n.contains("joint.blocks")) {
        return Some(Sd3);
    }
    match dit_hint(index) {
        Hint::Base(base) => return Some(base),
        Hint::Unsure => return None,
        Hint::NoEvidence => {}
    }
    if let Some(trained_on) = index.metadata.get("ss_base_model_version") {
        let by_metadata = [
            ("sdxl", Sdxl),
            ("sd_v1", Sd1),
            ("sd_v2", Sd2),
            ("flux", Flux),
        ];
        if let Some((_, base)) = by_metadata.iter().find(|(w, _)| trained_on.starts_with(w)) {
            return Some(*base);
        }
    }
    if index
        .tensors
        .iter()
        .any(|t| t.name.starts_with("lora_unet_"))
    {
        return Some(Sd1);
    }
    None
}

/// A UNet transformer block's cross-attention key in dotted form:
/// `…blocks.N.attn2.to.k.lora.down.weight`. LTX-2's `audio_attn2_to_k` meets the same words
/// once `_` reads as `.`, but not after a block number.
fn unet_cross_attention_key(dotted: &str) -> bool {
    dotted
        .strip_suffix(".attn2.to.k.lora.down.weight")
        .and_then(|head| head.rsplit_once('.'))
        .is_some_and(|(container, n)| {
            container.ends_with("blocks") && !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())
        })
}

// ---- The base of a DiT LoRA --------------------------------------------------------------

/// What a DiT LoRA's names and sizes say.
enum Hint {
    Base(SdVersion),
    /// They fit two families, or a family that is no label.
    Unsure,
    /// No name of any family's diffusion model.
    NoEvidence,
}

/// One layer a family's LoRAs can train.
struct Layer {
    /// The name in dotted parts: `#` is the block number, `*` any one part.
    parts: Vec<String>,
    /// How many blocks `#` counts; 0 where no size is fixed.
    blocks: u32,
    /// In- and out-features; 0 where no size is fixed.
    input: u64,
    output: u64,
}

/// The LoRA-visible layers of a model family.
struct Family {
    /// `None` for one diffusion model that several families share, which names none of them.
    base: Option<SdVersion>,
    /// One of the families whose sizes are known (the module doc's table).
    primary: bool,
    layers: Vec<Layer>,
}

/// Any size.
const ANY: u64 = 0;

impl Family {
    fn new(base: Option<SdVersion>, primary: bool) -> Self {
        Family {
            base,
            primary,
            layers: Vec::new(),
        }
    }

    /// `count` blocks named `container.N`, each with the `(name, in, out)` layers.
    fn blocks(mut self, container: &str, count: u32, layers: &[(&str, u64, u64)]) -> Self {
        for &(name, input, output) in layers {
            self.layers.push(Layer {
                parts: dotted_parts(&format!("{container}.#.{name}")),
                blocks: count,
                input,
                output,
            });
        }
        self
    }

    /// Layers of no fixed size in blocks of no fixed count.
    fn any_blocks(self, container: &str, names: &[&str]) -> Self {
        let layers: Vec<(&str, u64, u64)> = names.iter().map(|n| (*n, ANY, ANY)).collect();
        self.blocks(container, 0, &layers)
    }

    /// Layers outside the numbered blocks.
    fn top(mut self, layers: &[(&str, u64, u64)]) -> Self {
        for &(name, input, output) in layers {
            self.layers.push(Layer {
                parts: dotted_parts(name),
                blocks: 0,
                input,
                output,
            });
        }
        self
    }

    /// One of the family's layers has `module`'s name.
    fn names(&self, module: &Module) -> bool {
        self.layers.iter().any(|l| locate(module, l).is_some())
    }

    /// One of the family's layers has `module`'s name, block number and sizes.
    fn fits(&self, module: &Module) -> bool {
        let fixed = |want: u64, got: Option<u64>| want == ANY || got.is_none_or(|got| got == want);
        self.layers.iter().any(|l| {
            locate(module, l).is_some_and(|block| {
                (l.blocks == 0 || block.is_none_or(|b| b < l.blocks))
                    && fixed(l.input, module.input)
                    && fixed(l.output, module.output)
            })
        })
    }
}

fn dotted_parts(name: &str) -> Vec<String> {
    name.replace('_', ".")
        .split('.')
        .map(str::to_string)
        .collect()
}

/// What may come before a layer's name: kohya's `lora_unet_` (and ai-toolkit's
/// `lora_transformer_`), LyCORIS's `lycoris_`, ComfyUI's `diffusion_model.`, the engine's
/// `model.diffusion_model.`, diffusers' and PEFT's `transformer.`, `unet.` and
/// `base_model.model.`, and the engine's own `net.` (Anima, `name_conversion.cpp:913-919`)
/// and `model.` (HiDream-O1, `model_loader.cpp:502-505`). Anything else is part of another
/// name, so `single_transformer_blocks` never reads as `transformer_blocks`, nor a UNet's
/// `input_blocks` as `blocks`.
const WRAPPERS: [&[&str]; 15] = [
    &[],
    &["lora", "unet"],
    &["lora", "transformer"],
    &["lycoris"],
    &["diffusion", "model"],
    &["model", "diffusion", "model"],
    &["model"],
    &["transformer"],
    &["unet"],
    &["base", "model", "model"],
    &["base", "model", "model", "transformer"],
    &["net"],
    &["diffusion", "model", "net"],
    &["model", "diffusion", "model", "net"],
    &["lora", "unet", "net"],
];

/// A module a LoRA trains: its name in dotted parts and the sizes its weights show.
struct Module {
    parts: Vec<String>,
    input: Option<u64>,
    output: Option<u64>,
}

/// Where `module` names `layer`: `Some` with the block number, if the layer has one.
fn locate(module: &Module, layer: &Layer) -> Option<Option<u32>> {
    let at = module.parts.len().checked_sub(layer.parts.len())?;
    let (wrapper, name) = module.parts.split_at(at);
    if !WRAPPERS
        .iter()
        .any(|w| w.iter().copied().eq(wrapper.iter().map(String::as_str)))
    {
        return None;
    }
    let mut block = None;
    for (got, want) in name.iter().zip(&layer.parts) {
        match want.as_str() {
            "#" if !got.is_empty() && got.bytes().all(|b| b.is_ascii_digit()) => {
                block = Some(got.parse().ok()?);
            }
            "*" => {}
            want if want == got => {}
            _ => return None,
        }
    }
    Some(block)
}

/// The engine's LoRA suffixes (`name_conversion.cpp:1676-1709`) and LyCORIS's `.dora_scale`:
/// the down weights show the in-features (`ne[0]`), the up weights the out-features
/// (`ne[1]`), a `.diff` both.
const DOWN: [&str; 5] = [
    ".lora_down.weight",
    ".lora.down.weight",
    "_lora.down.weight",
    ".lora_A.weight",
    ".lora_A.default.weight",
];
const UP: [&str; 5] = [
    ".lora_up.weight",
    ".lora.up.weight",
    "_lora.up.weight",
    ".lora_B.weight",
    ".lora_B.default.weight",
];
const SIZELESS: [&str; 22] = [
    ".lora_mid.weight",
    ".lora.mid.weight",
    "_lora.mid.weight",
    ".lora_A",
    ".lora_B",
    ".lora_linear",
    ".alpha",
    ".scale",
    ".diff_b",
    ".hada_w1_a",
    ".hada_w1_b",
    ".hada_w2_a",
    ".hada_w2_b",
    ".hada_t1",
    ".hada_t2",
    ".lokr_w1",
    ".lokr_w1_a",
    ".lokr_w1_b",
    ".lokr_w2",
    ".lokr_w2_a",
    ".lokr_w2_b",
    ".dora_scale",
];

/// The modules the LoRA `index` trains.
fn lora_modules(index: &TensorIndex) -> Vec<Module> {
    let mut modules: std::collections::BTreeMap<&str, (Option<u64>, Option<u64>)> =
        Default::default();
    for t in &index.tensors {
        let strip = |suffixes: &[&str]| suffixes.iter().find_map(|s| t.name.strip_suffix(s));
        let ne = engine_ne(t, index.format);
        let (module, input, output) = if let Some(m) = strip(&DOWN) {
            (m, Some(ne[0]), None)
        } else if let Some(m) = strip(&UP) {
            (m, None, Some(ne[1]))
        } else if let Some(m) = t.name.strip_suffix(".diff") {
            let flat = t.shape.len() == 2;
            (m, flat.then_some(ne[0]), flat.then_some(ne[1]))
        } else if let Some(m) = strip(&SIZELESS) {
            (m, None, None)
        } else {
            continue;
        };
        let entry = modules.entry(module).or_default();
        entry.0 = entry.0.or(input);
        entry.1 = entry.1.or(output);
    }
    modules
        .into_iter()
        .map(|(name, (input, output))| Module {
            parts: dotted_parts(name),
            input,
            output,
        })
        .collect()
}

/// The family of a DiT LoRA. A family stays a candidate while one of its layers has each
/// recognised module's name, block number and sizes. A single primary family is the answer;
/// two are none. Without a primary family, a sharing family needs a module whose name no
/// other family has.
fn dit_hint(index: &TensorIndex) -> Hint {
    let families = families();
    let modules: Vec<Module> = lora_modules(index)
        .into_iter()
        .filter(|m| families.iter().any(|f| f.names(m)))
        .collect();
    if modules.is_empty() {
        return Hint::NoEvidence;
    }
    let fitting: Vec<&Family> = families
        .iter()
        .filter(|f| modules.iter().all(|m| f.fits(m)))
        .collect();
    let mut chosen: BTreeSet<Option<SdVersion>> = fitting
        .iter()
        .filter(|f| f.primary)
        .map(|f| f.base)
        .collect();
    if chosen.is_empty() {
        let its_own = |base: Option<SdVersion>| {
            modules.iter().any(|m| {
                families
                    .iter()
                    .filter(|f| f.names(m))
                    .all(|f| f.base == base)
            })
        };
        chosen = fitting
            .iter()
            .map(|f| f.base)
            .filter(|b| its_own(*b))
            .collect();
    }
    match (chosen.len(), chosen.first()) {
        (1, Some(Some(base))) => Hint::Base(*base),
        _ => Hint::Unsure,
    }
}

/// The diffusers attention of a double block, which the engine converts for the FLUX
/// families (`name_conversion.cpp:617-741`) and which Qwen-Image uses as its own names
/// (`qwen_image.hpp:146-168`).
const JOINT_ATTENTION: [&str; 8] = [
    "attn.to_q",
    "attn.to_k",
    "attn.to_v",
    "attn.add_q_proj",
    "attn.add_k_proj",
    "attn.add_v_proj",
    "attn.to_out.0",
    "attn.to_add_out",
];

fn sized(names: &[&'static str], input: u64, output: u64) -> Vec<(&'static str, u64, u64)> {
    names.iter().map(|n| (*n, input, output)).collect()
}

/// FLUX.1, with Chroma, Flex.2, Fill and Controls: 3072 wide, 19 double and 38 single
/// blocks (`flux.hpp:42-47`, fixture flux1-dev), a 4x MLP (325-330) and per-block
/// modulation (449-450, 462-463, 633-634); its own names, and the diffusers names the engine
/// converts (`name_conversion.cpp:617-741`).
fn flux1() -> Family {
    let (h, double, single) = (3072, 19, 38);
    let mut diffusers = sized(&JOINT_ATTENTION, h, h);
    diffusers.extend([
        ("norm1.linear", h, 6 * h),
        ("norm1_context.linear", h, 6 * h),
        ("ff.net.0.proj", h, 4 * h),
        ("ff.net.2", 4 * h, h),
        ("ff_context.net.0.proj", h, 4 * h),
        ("ff_context.net.2", 4 * h, h),
    ]);
    Family::new(Some(SdVersion::Flux), true)
        .blocks(
            "double_blocks",
            double,
            &[
                ("img_mod.lin", h, 6 * h),
                ("txt_mod.lin", h, 6 * h),
                ("img_attn.qkv", h, 3 * h),
                ("txt_attn.qkv", h, 3 * h),
                ("img_attn.proj", h, h),
                ("txt_attn.proj", h, h),
                ("img_mlp.0", h, 4 * h),
                ("txt_mlp.0", h, 4 * h),
                ("img_mlp.2", 4 * h, h),
                ("txt_mlp.2", 4 * h, h),
            ],
        )
        .blocks(
            "single_blocks",
            single,
            &[
                ("linear1", h, 7 * h),
                ("linear2", 5 * h, h),
                ("modulation.lin", h, 3 * h),
            ],
        )
        .blocks("transformer_blocks", double, &diffusers)
        .blocks(
            "single_transformer_blocks",
            single,
            &[
                ("norm.linear", h, 3 * h),
                ("attn.to_q", h, h),
                ("attn.to_k", h, h),
                ("attn.to_v", h, h),
                ("proj_mlp", h, 4 * h),
                ("proj_out", 5 * h, h),
            ],
        )
}

/// A FLUX.2 model `h` wide: modulation shared by every block (`flux.hpp:986-990`) and a
/// gated 3x MLP (89-102, 325-330, 623-629), in its own names and FLUX's diffusers names.
fn flux2(base: SdVersion, h: u64, double: u32, single: u32) -> Family {
    let mut diffusers = sized(&JOINT_ATTENTION, h, h);
    diffusers.extend([
        ("ff.net.0.proj", h, 6 * h),
        ("ff.net.2", 3 * h, h),
        ("ff_context.net.0.proj", h, 6 * h),
        ("ff_context.net.2", 3 * h, h),
    ]);
    Family::new(Some(base), true)
        .blocks(
            "double_blocks",
            double,
            &[
                ("img_attn.qkv", h, 3 * h),
                ("txt_attn.qkv", h, 3 * h),
                ("img_attn.proj", h, h),
                ("txt_attn.proj", h, h),
                ("img_mlp.0", h, 6 * h),
                ("txt_mlp.0", h, 6 * h),
                ("img_mlp.2", 3 * h, h),
                ("txt_mlp.2", 3 * h, h),
            ],
        )
        .blocks(
            "single_blocks",
            single,
            &[("linear1", h, 9 * h), ("linear2", 4 * h, h)],
        )
        .top(&[
            ("double_stream_modulation_img.lin", h, 6 * h),
            ("double_stream_modulation_txt.lin", h, 6 * h),
            ("single_stream_modulation.lin", h, 3 * h),
        ])
        .blocks("transformer_blocks", double, &diffusers)
        .blocks(
            "single_transformer_blocks",
            single,
            &[
                ("attn.to_q", h, h),
                ("attn.to_k", h, h),
                ("attn.to_v", h, h),
                ("proj_mlp", h, 6 * h),
                ("proj_out", 4 * h, h),
            ],
        )
}

/// Qwen-Image and Qwen-Image Layered: 3072 wide, 60 blocks (fixture qwen-image), its
/// attention, modulation and 4x MLPs (`qwen_image.hpp:146-168, 267-280`; `block.hpp:259-287`).
fn qwen_image() -> Family {
    let h = 3072;
    let mut layers = sized(&JOINT_ATTENTION, h, h);
    layers.extend([
        ("img_mod.1", h, 6 * h),
        ("txt_mod.1", h, 6 * h),
        ("img_mlp.net.0.proj", h, 4 * h),
        ("txt_mlp.net.0.proj", h, 4 * h),
        ("img_mlp.net.2", 4 * h, h),
        ("txt_mlp.net.2", 4 * h, h),
    ]);
    Family::new(Some(SdVersion::QwenImage), true).blocks("transformer_blocks", 60, &layers)
}

/// Z-Image: 3840 wide, 30 layers and two refiners of each kind (`z_image.hpp:34-37`;
/// fixture z-image-turbo: QKV 11520 out, MLP 10240, modulation 256 in). Fused or split
/// attention (145-156; the diffusers split, `name_conversion.cpp:874-911`), a gated MLP
/// (264-272) and, but in the context refiner, modulation (322-329).
fn z_image() -> Family {
    let (h, mlp) = (3840, 10240);
    let block = |modulated: bool| {
        let mut layers = vec![
            ("attention.qkv", h, 3 * h),
            ("attention.out", h, h),
            ("attention.to_q", h, h),
            ("attention.to_k", h, h),
            ("attention.to_v", h, h),
            ("attention.to_out.0", h, h),
            ("feed_forward.w1", h, mlp),
            ("feed_forward.w2", mlp, h),
            ("feed_forward.w3", h, mlp),
        ];
        if modulated {
            layers.push(("adaLN_modulation.0", 256, 4 * h));
        }
        layers
    };
    Family::new(Some(SdVersion::ZImage), true)
        .blocks("layers", 30, &block(true))
        .blocks("noise_refiner", 2, &block(true))
        .blocks("context_refiner", 2, &block(false))
        .top(&[("cap_embedder.1", 2560, h)])
}

/// A Wan 2.x model `h` wide (`wan.hpp:115-118, 221-222, 332-349`): Wan 2.1 1.3B (1536, 30
/// blocks, fixture wan21-t2v-1.3b), Wan 2.1 and 2.2 14B (5120, 40, fixture wan21-i2v-14b)
/// and Wan 2.2 TI2V-5B (3072, 30, fixture wan22-ti2v-5b).
fn wan(base: SdVersion, h: u64, blocks: u32) -> Family {
    Family::new(Some(base), true).blocks(
        "blocks",
        blocks,
        &[
            ("self_attn.q", h, h),
            ("self_attn.k", h, h),
            ("self_attn.v", h, h),
            ("self_attn.o", h, h),
            ("cross_attn.q", h, h),
            ("cross_attn.k", h, h),
            ("cross_attn.v", h, h),
            ("cross_attn.o", h, h),
            ("cross_attn.k_img", h, h),
            ("cross_attn.v_img", h, h),
            ("ffn.0", h, ANY),
            ("ffn.2", ANY, h),
        ],
    )
}

/// HunyuanVideo 1.5: FLUX's double block (`hunyuan.hpp:327`), 2048 wide, 54 blocks, a 4x
/// MLP (165-171); in FLUX's names and Tencent's, which the engine converts
/// (`name_conversion.cpp:762-829`).
fn hunyuan_video() -> Family {
    let h = 2048;
    let mut layers = Vec::new();
    for stream in ["img", "txt"] {
        for (name, input, output) in [
            ("mod.lin", h, 6 * h),
            ("mod.linear", h, 6 * h),
            ("attn.qkv", h, 3 * h),
            ("attn.proj", h, h),
            ("mlp.0", h, 4 * h),
            ("mlp.fc1", h, 4 * h),
            ("mlp.2", 4 * h, h),
            ("mlp.fc2", 4 * h, h),
        ] {
            layers.push((format!("{stream}_{name}"), input, output));
        }
    }
    let layers: Vec<(&str, u64, u64)> = layers
        .iter()
        .map(|(n, i, o)| (n.as_str(), *i, *o))
        .collect();
    Family::new(Some(SdVersion::HunyuanVideo), true).blocks("double_blocks", 54, &layers)
}

/// LTX-2.3: 48 blocks, 4096 wide for video and 2048 for audio (fixture ltx23), each with
/// self- and cross-attention, 4x MLPs and cross-modal attention (`ltxv.hpp:671-679,
/// 1159-1166`).
fn ltx() -> Family {
    let (v, a) = (4096, 2048);
    let mut layers = Vec::new();
    for (attn, width, context) in [
        ("attn1", v, v),
        ("attn2", v, ANY),
        ("audio_attn1", a, a),
        ("audio_attn2", a, ANY),
    ] {
        for (name, input, output) in [
            ("to_q", width, width),
            ("to_k", context, width),
            ("to_v", context, width),
            ("to_out.0", width, width),
            ("to_gate_logits", width, ANY),
        ] {
            layers.push((format!("{attn}.{name}"), input, output));
        }
    }
    for attn in ["audio_to_video_attn", "video_to_audio_attn"] {
        for name in ["to_q", "to_k", "to_v", "to_out.0", "to_gate_logits"] {
            layers.push((format!("{attn}.{name}"), ANY, ANY));
        }
    }
    for (ff, width) in [("ff", v), ("audio_ff", a)] {
        layers.push((format!("{ff}.net.0.proj"), width, 4 * width));
        layers.push((format!("{ff}.net.2"), 4 * width, width));
    }
    let layers: Vec<(&str, u64, u64)> = layers
        .iter()
        .map(|(n, i, o)| (n.as_str(), *i, *o))
        .collect();
    Family::new(Some(SdVersion::Ltxav), true).blocks("transformer_blocks", 48, &layers)
}

/// Every family's layers, the primary families first.
fn families() -> &'static [Family] {
    static FAMILIES: std::sync::OnceLock<Vec<Family>> = std::sync::OnceLock::new();
    FAMILIES.get_or_init(|| {
        use SdVersion::*;
        let own = |base| Family::new(Some(base), false);
        vec![
            flux1(),
            // fixture flux2-klein-4b; 0.1.x klein9b table; black-forest-labs/FLUX.2-dev
            // transformer/config.json (8 + 48 layers, 48 heads of 128).
            flux2(Flux2Klein, 3072, 5, 20),
            flux2(Flux2Klein, 4096, 8, 24),
            flux2(Flux2, 6144, 8, 48),
            qwen_image(),
            z_image(),
            wan(Wan2, 1536, 30),
            wan(Wan2, 5120, 40),
            wan(Wan22Ti2v, 3072, 30),
            hunyuan_video(),
            ltx(),
            // Ovis-Image: FLUX's blocks with a gated MLP (`flux.hpp:86-89, 349-353`).
            own(OvisImage)
                .any_blocks(
                    "double_blocks",
                    &[
                        "img_mod.lin",
                        "txt_mod.lin",
                        "img_attn.qkv",
                        "txt_attn.qkv",
                        "img_attn.proj",
                        "txt_attn.proj",
                        "img_mlp.gate_proj",
                        "img_mlp.up_proj",
                        "img_mlp.down_proj",
                        "txt_mlp.gate_proj",
                        "txt_mlp.up_proj",
                        "txt_mlp.down_proj",
                    ],
                )
                .any_blocks("single_blocks", &["linear1", "linear2", "modulation.lin"]),
            // FLUX.3 (`flux3.hpp:66-73, 123-136`; diffusers, `name_conversion.cpp:744-760`).
            own(Flux3)
                .any_blocks("single_blocks", &FLUX3_BLOCK)
                .any_blocks("content_mode_blocks.*", &FLUX3_BLOCK)
                .any_blocks("single_transformer_blocks", &FLUX3_DIFFUSERS)
                .any_blocks("mode_blocks.*", &FLUX3_DIFFUSERS)
                .top(&[
                    ("early_stream_modulations.*.lin", ANY, ANY),
                    ("single_stream_modulations.*.lin", ANY, ANY),
                ]),
            // Qwen-Image 2.1's own MLP (`qwen_image_2_1.hpp:172-178, 255-264`).
            own(QwenImage).any_blocks(
                "transformer_blocks",
                &[
                    "attn.to_q",
                    "attn.to_k",
                    "attn.to_v",
                    "attn.to_out.0",
                    "img_mlp.gate_up",
                    "img_mlp.proj",
                    "img_mlp.gate_layer",
                    "img_mlp.out",
                ],
            ),
            // Lens (`lens.hpp:97-99, 124-133, 208-216, 296`).
            own(Lens).any_blocks(
                "transformer_blocks",
                &[
                    "attn.img_qkv",
                    "attn.txt_qkv",
                    "attn.to_out.0",
                    "attn.to_add_out",
                    "img_mod.1",
                    "txt_mod.1",
                    "img_mlp.w1",
                    "img_mlp.w2",
                    "img_mlp.w3",
                    "txt_mlp.w1",
                    "txt_mlp.w2",
                    "txt_mlp.w3",
                ],
            ),
            // MiniT2I (`minit2i.hpp:201-203, 290-294, 326-337, 402-405`).
            own(Minit2i)
                .any_blocks(
                    "double_blocks",
                    &[
                        "img_qkv",
                        "txt_qkv",
                        "img_attn_proj",
                        "txt_attn_proj",
                        "img_mlp.w1",
                        "img_mlp.w2",
                        "img_mlp.w3",
                        "txt_mlp.w1",
                        "txt_mlp.w2",
                        "txt_mlp.w3",
                    ],
                )
                .any_blocks(
                    "txt_preamble_blocks",
                    &["qkv", "attn_proj", "mlp.w1", "mlp.w2", "mlp.w3"],
                ),
            // Anima, which the engine keeps under `net.` (`anima.hpp:188-196, 269-270,
            // 287-288, 305-310, 398-403, 473-479`).
            own(Anima)
                .any_blocks(
                    "blocks",
                    &[
                        "self_attn.q_proj",
                        "self_attn.k_proj",
                        "self_attn.v_proj",
                        "self_attn.output_proj",
                        "cross_attn.q_proj",
                        "cross_attn.k_proj",
                        "cross_attn.v_proj",
                        "cross_attn.output_proj",
                        "mlp.layer1",
                        "mlp.layer2",
                        "adaln_modulation_self_attn.1",
                        "adaln_modulation_self_attn.2",
                        "adaln_modulation_cross_attn.1",
                        "adaln_modulation_cross_attn.2",
                        "adaln_modulation_mlp.1",
                        "adaln_modulation_mlp.2",
                    ],
                )
                .any_blocks(
                    "llm_adapter.blocks",
                    &[
                        "self_attn.q_proj",
                        "self_attn.k_proj",
                        "self_attn.v_proj",
                        "self_attn.o_proj",
                        "cross_attn.q_proj",
                        "cross_attn.k_proj",
                        "cross_attn.v_proj",
                        "cross_attn.o_proj",
                        "mlp.0",
                        "mlp.2",
                    ],
                ),
            // PixArt, and its diffusers names (`pixart.hpp:131-136, 168-171, 236`;
            // `name_conversion.cpp:1009-1069`).
            own(Pixart)
                .any_blocks(
                    "blocks",
                    &[
                        "attn.qkv",
                        "attn.proj",
                        "cross_attn.q_linear",
                        "cross_attn.kv_linear",
                        "cross_attn.proj",
                        "mlp.fc1",
                        "mlp.fc2",
                    ],
                )
                .any_blocks(
                    "transformer_blocks",
                    &[
                        "attn1.to_q",
                        "attn1.to_k",
                        "attn1.to_v",
                        "attn1.to_out.0",
                        "attn2.to_q",
                        "attn2.to_k",
                        "attn2.to_v",
                        "attn2.to_out.0",
                        "ff.net.0.proj",
                        "ff.net.2",
                    ],
                ),
            // LLaDA-Image: Z-Image's block, with a refiner of its own; its context and
            // sigvq refiners have no modulation (`llada_image.hpp:13, 158-172`).
            own(LladaImage)
                .any_blocks("layers", &Z_IMAGE_BLOCK)
                .any_blocks("noise_refiner", &Z_IMAGE_BLOCK)
                .any_blocks("context_refiner", &Z_IMAGE_BLOCK[..9])
                .any_blocks("sigvq_refiner", &Z_IMAGE_BLOCK[..9])
                .top(&[
                    ("cap_embedder.1", ANY, ANY),
                    ("semantic_embedder.1", ANY, ANY),
                    ("sigvq_embedder.1", ANY, ANY),
                ]),
            // ERNIE-Image (`ernie_image.hpp:145-150, 198-200, 222-228, 305`).
            own(ErnieImage).any_blocks(
                "layers",
                &[
                    "self_attention.to_q",
                    "self_attention.to_k",
                    "self_attention.to_v",
                    "self_attention.to_out.0",
                    "mlp.gate_proj",
                    "mlp.up_proj",
                    "mlp.linear_fc2",
                ],
            ),
            // FIBO 2 (`fibo2.hpp:89-99, 156-157, 176-178, 224-233, 288-294`;
            // `name_conversion.cpp:1092-1102`).
            own(Fibo2)
                .any_blocks("layers", &FIBO2_BLOCK)
                .any_blocks("noise_refiner", &FIBO2_BLOCK)
                .any_blocks(
                    "perceiver.layers",
                    &[
                        "cross_attention.to_q",
                        "cross_attention.to_kv",
                        "cross_attention.to_out.0",
                        "self_attention.to_q",
                        "self_attention.to_kv",
                        "self_attention.to_out.0",
                        "ffn.w13",
                        "ffn.w2",
                    ],
                ),
            // Ideogram 4 (`ideogram4.hpp:206-209, 243-245, 264-270, 368`).
            own(Ideogram4).any_blocks(
                "layers",
                &[
                    "attention.qkv",
                    "attention.o",
                    "feed_forward.w1",
                    "feed_forward.w2",
                    "feed_forward.w3",
                    "adaln_modulation",
                ],
            ),
            // HiDream-O1's language model (`hidream_o1.hpp:238`; `llm.hpp:303-306, 471-472,
            // 522`).
            own(HidreamO1).any_blocks(
                "language_model.layers",
                &[
                    "self_attn.q_proj",
                    "self_attn.k_proj",
                    "self_attn.v_proj",
                    "self_attn.o_proj",
                    "mlp.gate_proj",
                    "mlp.up_proj",
                    "mlp.down_proj",
                ],
            ),
            // Iris (`iris.hpp:85-86, 125-132, 158-163, 190-208, 284-291, 326-333`;
            // `pid.hpp:248-250`).
            own(Iris)
                .any_blocks("blocks", &IRIS_TRUNK)
                .any_blocks("layer_blocks", &IRIS_TEXT)
                .any_blocks("refiner.blocks", &IRIS_TEXT)
                .any_blocks(
                    "pixel_blocks",
                    &[
                        "compress",
                        "expand",
                        "adaln",
                        "attn.qkv",
                        "attn.proj",
                        "mlp.fc1",
                        "mlp.fc2",
                    ],
                )
                .top(&[
                    ("modulation_cores.adaln_img", ANY, ANY),
                    ("modulation_cores.adaln_txt", ANY, ANY),
                ]),
            // Krea 2, and its diffusers names (`krea2.hpp:201-203, 255-261, 357-360,
            // 383-388, 428-432, 618-622`; `name_conversion.cpp:921-955`).
            own(Krea2)
                .any_blocks("blocks", &KREA2_BLOCK)
                .any_blocks("txtfusion.layerwise_blocks", &KREA2_BLOCK)
                .any_blocks("txtfusion.refiner_blocks", &KREA2_BLOCK)
                .any_blocks(
                    "transformer_blocks",
                    &[
                        "attn.to_q",
                        "attn.to_k",
                        "attn.to_v",
                        "attn.to_out.0",
                        "attn.to_out",
                        "attn.to_gate",
                        "ff.gate",
                        "ff.up",
                        "ff.down",
                    ],
                ),
            // LingBot-Video (`lingbot_video.hpp:180-185, 223-225, 277, 442-455, 521`).
            own(LingbotVideo).any_blocks(
                "blocks",
                &[
                    "attn.to_q",
                    "attn.to_k",
                    "attn.to_v",
                    "attn.to_out",
                    "ffn.gate_proj",
                    "ffn.up_proj",
                    "ffn.down_proj",
                    "ffn.shared_experts.gate_proj",
                    "ffn.shared_experts.up_proj",
                    "ffn.shared_experts.down_proj",
                ],
            ),
            // MOSS-Image (`moss_image.hpp:63-71, 102-109, 149-156`;
            // `name_conversion.cpp:1071-1079`).
            own(MossImage)
                .any_blocks(
                    "layers",
                    &[
                        "proj.qkv",
                        "proj.q",
                        "proj.k",
                        "proj.v",
                        "proj.o",
                        "proj.ffn_up_gate",
                        "proj.ffn_down",
                    ],
                )
                .any_blocks(
                    "text_embedder.blocks",
                    &["qkv", "proj", "ffn_up", "ffn_gate", "ffn_down"],
                ),
            // Boogu-Image (`boogu.hpp:173, 197-199, 245-250, 298-301, 357-367, 432-440,
            // 547-568`).
            own(BooguImage)
                .any_blocks("noise_refiner", &BOOGU_REFINER)
                .any_blocks("context_refiner", &BOOGU_REFINER)
                .any_blocks(
                    "double_stream_layers",
                    &[
                        "img_instruct_attn.processor.img_to_q",
                        "img_instruct_attn.processor.img_to_k",
                        "img_instruct_attn.processor.img_to_v",
                        "img_instruct_attn.processor.instruct_to_q",
                        "img_instruct_attn.processor.instruct_to_k",
                        "img_instruct_attn.processor.instruct_to_v",
                        "img_instruct_attn.processor.img_out",
                        "img_instruct_attn.processor.instruct_out",
                        "img_instruct_attn.to_out.0",
                        "img_self_attn.to_q",
                        "img_self_attn.to_k",
                        "img_self_attn.to_v",
                        "img_self_attn.to_out.0",
                        "img_feed_forward.linear_1",
                        "img_feed_forward.linear_2",
                        "img_feed_forward.linear_3",
                        "instruct_feed_forward.linear_1",
                        "instruct_feed_forward.linear_2",
                        "instruct_feed_forward.linear_3",
                        "img_norm1.linear",
                        "img_norm2.linear",
                        "img_norm3.linear",
                        "instruct_norm1.linear",
                        "instruct_norm2.linear",
                    ],
                ),
            // MiniMax-H3, MiniMax-H3-Image and P-H3-Image share one diffusion model, which
            // names none of them (`minimax_h3.hpp:240-241, 323-331, 475-481, 500, 589-600,
            // 755`; `name_conversion.cpp:1104-1120`).
            Family::new(None, false)
                .any_blocks("blocks", &MINIMAX_BLOCK)
                .any_blocks("token_refiner.blocks", &MINIMAX_BLOCK)
                .any_blocks("transformer_blocks", &MINIMAX_DIFFUSERS)
                .any_blocks("token_refiner.refiner_blocks", &MINIMAX_DIFFUSERS),
        ]
    })
}

const FLUX3_BLOCK: [&str; 6] = [
    "q_proj", "k_proj", "v_proj", "attn_out", "mlp_in", "mlp_out",
];
const FLUX3_DIFFUSERS: [&str; 6] = [
    "attn.to_q",
    "attn.to_k",
    "attn.to_v",
    "attn.to_out.0",
    "ff.linear_in",
    "ff.linear_out",
];
/// Z-Image's block; the last layer is the modulation an unmodulated block leaves out.
const Z_IMAGE_BLOCK: [&str; 10] = [
    "attention.qkv",
    "attention.out",
    "attention.to_q",
    "attention.to_k",
    "attention.to_v",
    "attention.to_out.0",
    "feed_forward.w1",
    "feed_forward.w2",
    "feed_forward.w3",
    "adaLN_modulation.0",
];
const FIBO2_BLOCK: [&str; 15] = [
    "attention.qkv",
    "attention.out",
    "attention.to_q",
    "attention.to_kv",
    "attention.to_k",
    "attention.to_v",
    "attention.to_out.0",
    "feed_forward.w13",
    "feed_forward.w2",
    "adaLN_modulation.0",
    "cross_attention.to_q",
    "cross_attention.to_kv",
    "cross_attention.to_k",
    "cross_attention.to_v",
    "cross_attention.to_out.0",
];
const IRIS_TRUNK: [&str; 24] = [
    "q_proj",
    "k_proj",
    "v_proj",
    "attn_proj",
    "attn_gate",
    "mlp.w1",
    "mlp.w2",
    "mlp.w3",
    "attn.q_proj_x",
    "attn.k_proj_x",
    "attn.v_proj_x",
    "attn.proj_x",
    "attn_gate_x",
    "mlp_x.w1",
    "mlp_x.w2",
    "mlp_x.w3",
    "attn.q_proj_y",
    "attn.k_proj_y",
    "attn.v_proj_y",
    "attn.proj_y",
    "attn_gate_y",
    "mlp_y.w1",
    "mlp_y.w2",
    "mlp_y.w3",
];
const IRIS_TEXT: [&str; 7] = [
    "attn.qkv",
    "attn.proj",
    "mlp.0",
    "mlp.2",
    "mlp.w1",
    "mlp.w2",
    "mlp.w3",
];
const KREA2_BLOCK: [&str; 8] = [
    "attn.wq",
    "attn.wk",
    "attn.wv",
    "attn.wo",
    "attn.gate",
    "mlp.gate",
    "mlp.up",
    "mlp.down",
];
const BOOGU_REFINER: [&str; 8] = [
    "attn.to_q",
    "attn.to_k",
    "attn.to_v",
    "attn.to_out.0",
    "feed_forward.linear_1",
    "feed_forward.linear_2",
    "feed_forward.linear_3",
    "norm1.linear",
];
const MINIMAX_BLOCK: [&str; 7] = [
    "attn.qkv_proj",
    "attn.q_proj",
    "attn.k_proj",
    "attn.v_proj",
    "attn.out_proj",
    "mlp.fc1",
    "mlp.fc2",
];
const MINIMAX_DIFFUSERS: [&str; 6] = [
    "attn.to_q",
    "attn.to_k",
    "attn.to_v",
    "attn.to_out.0",
    "ff.net.0.proj",
    "ff.net.2",
];

/// The numbers that follow `head` in `name`, each with the rest of the name after it.
fn numbers_after<'a>(name: &'a str, head: &'a str) -> impl Iterator<Item = (u32, &'a str)> + 'a {
    name.match_indices(head).filter_map(move |(at, _)| {
        let rest = &name[at + head.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        Some((rest[..digits].parse().ok()?, &rest[digits..]))
    })
}

/// The GGUF architectures of the language models recipes take as `llm`.
const LLM_ARCHITECTURES: [&str; 9] = [
    "qwen2", "qwen2vl", "qwen3", "qwen3vl", "qwen35", "mistral3", "llama", "gemma2", "gemma3",
];

/// The slot a part file fills, by its first marker (for a file with no version and no LoRA
/// names). A marker ending in `.weight` is an exact name, one ending in `.` a prefix, and
/// `embeddings_connector` is found anywhere in a name.
fn extra_slot(index: &TensorIndex) -> Option<SlotId> {
    let names = || index.tensors.iter().map(|t| t.name.as_str());
    let named = |name: &str| names().any(|n| n == name);
    let prefixed = |prefix: &str| names().any(|n| n.starts_with(prefix));
    let architecture = match index.format {
        FileFormat::Gguf => index
            .metadata
            .get("general.architecture")
            .map(String::as_str),
        FileFormat::Safetensors => None,
    };
    let diffusion = DIT_PREFIXES.iter().any(|p| prefixed(p));
    if architecture == Some("clip") && (prefixed("mm.") || prefixed("v.blk.")) {
        return Some(SlotId::LlmVision);
    }
    if architecture.is_some_and(|a| LLM_ARCHITECTURES.contains(&a)) {
        return Some(SlotId::Llm);
    }
    // A llama.cpp language model without metadata, like 0.1.x's qwen3-4b table.
    if named("token_embd.weight") && named("blk.0.attn_q.weight") && !diffusion {
        return Some(SlotId::Llm);
    }
    if named("model.layers.0.self_attn.q_proj.weight") {
        return Some(SlotId::Llm);
    }
    if named("encoder.block.0.layer.0.SelfAttention.q.weight") || prefixed("enc.blk.0.") {
        return Some(SlotId::T5xxl);
    }
    match clip_width(index) {
        Some(768) => return Some(SlotId::ClipL),
        Some(1280) => return Some(SlotId::ClipG),
        _ => {}
    }
    if prefixed("vision_model.encoder.layers.0.") {
        return Some(SlotId::ClipVision);
    }
    if (named("decoder.conv_in.weight") || prefixed("first_stage_model.decoder.")) && !diffusion {
        return Some(SlotId::Vae);
    }
    // The Wan 2.1 and 2.2 VAE, also Qwen-Image's, in its own names: the decoder's `conv1`
    // and `head.2` (`src/model/vae/wan_vae.hpp:871-876, 918-927`; Wan 2.2 TI2V-5B differs
    // only inside its up blocks, 889-899) and the top-level `conv2` (1105-1110). The runner
    // probes `decoder.conv1.weight` itself (1326). Diffusers names meet the row above, which
    // the engine converts (`src/name_conversion.cpp:1253-1344`).
    if named("decoder.conv1.weight")
        && named("decoder.head.2.weight")
        && named("conv2.weight")
        && !diffusion
    {
        return Some(SlotId::Vae);
    }
    // The HunyuanVideo 1.5 and LTX causal VAEs, whose decoder `conv_in` and `conv_out` are
    // causal convolutions holding a `conv` (`src/model/vae/hunyuan_vae.hpp:64-78, 670, 689`;
    // `src/model/vae/ltx_vae.hpp:86-100, 907, 943`). The LTX audio VAE's are under
    // `audio_vae.` (`ltx_audio_vae.hpp:81-82`).
    if named("decoder.conv_in.conv.weight") && named("decoder.conv_out.conv.weight") && !diffusion {
        return Some(SlotId::Vae);
    }
    // TAESD.
    if prefixed("decoder.layers.") && prefixed("encoder.layers.") {
        return Some(SlotId::Tae);
    }
    if prefixed("wav2vec2.") || prefixed("feature_extractor.conv_layers.") {
        return Some(SlotId::AudioEncoder);
    }
    if prefixed("audio_vae.") || prefixed("vocoder.") {
        return Some(SlotId::AudioVae);
    }
    if prefixed("connectors.") || names().any(|n| n.contains("embeddings_connector")) {
        return Some(SlotId::EmbeddingsConnectors);
    }
    None
}

/// A CLIP text encoder's hidden width: its first layer's `layer_norm1` `ne[0]`.
fn clip_width(index: &TensorIndex) -> Option<u64> {
    index
        .tensors
        .iter()
        .find(|t| {
            t.name
                .strip_prefix("text_model.encoder.layers.0.layer_norm1.")
                .is_some_and(|rest| rest == "weight" || rest == "bias")
        })
        .map(|t| engine_ne(t, index.format)[0])
}

fn match_facts(index: &TensorIndex, layout: Layout) -> MatchFacts {
    const ZERO_COND: &str = "__index_timestep_zero__";
    let names = || index.tensors.iter().map(|t| t.name.as_str());
    MatchFacts {
        sd3_blocks: names()
            .flat_map(|n| numbers_after(n, "joint_blocks."))
            .map(|(n, _)| n)
            .max()
            .map(|n| n.saturating_add(1)),
        flux2_width: dit(index, layout, "img_in.weight").map(|ne| ne[1]),
        zero_cond: names().any(|n| {
            n.strip_suffix(ZERO_COND)
                .is_some_and(|before| before.is_empty() || before.ends_with('.'))
        }),
        img_emb_pos: names().any(|n| n.contains("img_emb.emb_pos")),
        has_img_emb: names().any(|n| n.contains("img_emb")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::index::TensorInfo;
    use crate::sd::testing::{fixture, gguf_txt_fixture, safetensors, without};
    use serde_json::{Value, json};

    #[test]
    fn checkpoints_say_what_they_embed() {
        assert_eq!(
            inspect(&fixture("sd15")).kind,
            FileKind::Model {
                version: SdVersion::Sd1,
                layout: Layout::Checkpoint,
                embeds: vec![SlotId::ClipL, SlotId::Vae]
            }
        );
        assert!(
            matches!(inspect(&fixture("sdxl")).kind, FileKind::Model { version: SdVersion::Sdxl, ref embeds, .. } if embeds.contains(&SlotId::ClipG))
        );
        assert!(
            matches!(inspect(&fixture("sd35-medium")).kind, FileKind::Model { ref embeds, .. } if embeds == &vec![SlotId::Vae])
        );
        assert!(matches!(
            inspect(&fixture("flux1-kontext")).kind,
            FileKind::Model {
                layout: Layout::DiffusionOnly,
                ..
            }
        ));
    }

    #[test]
    fn lumina_2_is_refused_although_the_engine_calls_it_z_image() {
        assert_eq!(
            inspect(&fixture("lumina2")).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: Some(SdVersion::ZImage)
            }
        );
        assert!(matches!(
            inspect(&fixture("z-image-turbo")).kind,
            FileKind::Model {
                version: SdVersion::ZImage,
                ..
            }
        ));
    }

    #[test]
    fn ltx_video_09_and_hunyuan_video_1_are_refused() {
        assert!(matches!(
            inspect(&fixture("ltx23")).kind,
            FileKind::Model {
                version: SdVersion::Ltxav,
                ..
            }
        ));
        assert_eq!(
            inspect(&without(&fixture("ltx23"), "audio_patchify_proj")).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::LtxVideo09,
                version: Some(SdVersion::Ltxav)
            }
        );
        assert!(matches!(
            inspect(&fixture("hunyuan-video-15")).kind,
            FileKind::Model {
                version: SdVersion::HunyuanVideo,
                ..
            }
        ));
        assert_eq!(
            inspect(&without(&fixture("hunyuan-video-15"), "byt5_in")).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::HunyuanVideo1,
                version: Some(SdVersion::HunyuanVideo)
            }
        );
    }

    #[test]
    fn loras_are_named_with_a_base_hint() {
        assert_eq!(
            inspect(&fixture("sdxl-lora-pixel-art")).kind,
            FileKind::Lora {
                format: LoraFormat::Kohya,
                base: Some(SdVersion::Sdxl)
            }
        );
        assert_eq!(
            inspect(&fixture("flux-lora-realism")).kind,
            FileKind::Lora {
                format: LoraFormat::Kohya,
                base: Some(SdVersion::Flux)
            }
        );
    }

    #[test]
    fn companions_are_extra_files_for_their_slot() {
        assert_eq!(
            inspect(&fixture("ae-flux1")).kind,
            FileKind::Extra { slot: SlotId::Vae }
        );
        assert_eq!(
            inspect(&fixture("clip-l")).kind,
            FileKind::Extra {
                slot: SlotId::ClipL
            }
        );
        assert_eq!(
            inspect(&fixture("umt5-xxl-q5km")).kind,
            FileKind::Extra {
                slot: SlotId::T5xxl
            }
        );
        assert_eq!(
            inspect(&gguf_txt_fixture("qwen3-4b")).kind,
            FileKind::Extra { slot: SlotId::Llm }
        );
    }

    #[test]
    fn facts_for_matching_and_labels() {
        assert_eq!(inspect(&fixture("sd35-medium")).facts.sd3_blocks, Some(24));
        assert_eq!(
            inspect(&fixture("flux2-klein-4b")).facts.flux2_width,
            Some(3072)
        );
        assert!(inspect(&fixture("wan21-i2v-14b")).facts.has_img_emb);
        assert_eq!(
            inspect(&fixture("wan22-t2v-a14b-high")).parameters / 1_000_000_000,
            14
        );
    }

    #[test]
    fn a_pickle_file_is_named_without_being_read() {
        assert_eq!(
            inspect_local(Path::new("/nope/m.ckpt")).unwrap().kind,
            FileKind::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None
            }
        );
    }

    // Beyond the brief: the rows of the tables the tests above do not reach.

    const S: FileFormat = FileFormat::Safetensors;
    const G: FileFormat = FileFormat::Gguf;

    /// An index of `names` with their shapes as written.
    fn names_index(format: FileFormat, names: &[(&str, &[u64])]) -> TensorIndex {
        TensorIndex {
            format,
            tensors: names
                .iter()
                .map(|(name, shape)| TensorInfo {
                    name: name.to_string(),
                    dtype: "F16".into(),
                    shape: shape.to_vec(),
                })
                .collect(),
            metadata: Default::default(),
            parameters_hint: None,
        }
    }

    fn kind(format: FileFormat, names: &[(&str, &[u64])]) -> FileKind {
        inspect(&names_index(format, names)).kind
    }

    /// `index` with every name under `model.diffusion_model.`, as an all-in-one checkpoint
    /// holds its diffusion model.
    fn prefixed(mut index: TensorIndex) -> TensorIndex {
        for t in &mut index.tensors {
            t.name = format!("model.diffusion_model.{}", t.name);
        }
        index
    }

    #[test]
    fn a_checkpoint_lists_each_embedded_part_once_in_slot_order() {
        let sd3 = kind(
            S,
            &[
                (
                    "model.diffusion_model.joint_blocks.0.x_block.attn.qkv.weight",
                    &[4608, 1536],
                ),
                ("text_encoders.t5xxl.transformer.shared.weight", &[4]),
                ("text_encoders.clip_g.transformer.a.weight", &[4]),
                ("text_encoders.clip_l.transformer.a.weight", &[4]),
                ("text_encoders.clip_l.transformer.b.weight", &[4]),
                ("vae.decoder.conv_in.weight", &[4]),
            ],
        );
        assert_eq!(
            sd3,
            FileKind::Model {
                version: SdVersion::Sd3,
                layout: Layout::Checkpoint,
                embeds: vec![SlotId::ClipL, SlotId::ClipG, SlotId::T5xxl, SlotId::Vae]
            }
        );
        // `cond_stage_model.1.` is clip_g alone.
        assert!(matches!(
            kind(S, &[
                ("model.diffusion_model.input_blocks.0.0.weight", &[320, 4, 3, 3]),
                ("cond_stage_model.1.transformer.a.weight", &[4]),
            ]),
            FileKind::Model { ref embeds, .. } if embeds == &vec![SlotId::ClipG]
        ));
        // The guards read the diffusion model under the checkpoint's own prefix.
        let z_image = prefixed(fixture("z-image-turbo"));
        let mut with_llm = z_image.clone();
        with_llm.tensors.push(TensorInfo {
            name: "text_encoders.llm.model.layers.0.self_attn.q_proj.weight".into(),
            dtype: "BF16".into(),
            shape: vec![4],
        });
        assert_eq!(
            inspect(&with_llm).kind,
            FileKind::Model {
                version: SdVersion::ZImage,
                layout: Layout::Checkpoint,
                embeds: vec![SlotId::Llm]
            }
        );
        assert_eq!(
            inspect(&prefixed(fixture("lumina2"))).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: Some(SdVersion::ZImage)
            }
        );
    }

    #[test]
    fn lora_formats_follow_the_engines_suffixes() {
        let lora = |name: &str| kind(S, &[(name, &[16, 3072])]);
        assert!(matches!(
            lora("transformer.single_transformer_blocks.0.attn.to_k.lora_A.weight"),
            FileKind::Lora {
                format: LoraFormat::Peft,
                ..
            }
        ));
        assert!(matches!(
            lora("base.lora_A.default.weight"),
            FileKind::Lora {
                format: LoraFormat::Peft,
                ..
            }
        ));
        assert!(matches!(
            lora("unet.a.lora.down.weight"),
            FileKind::Lora {
                format: LoraFormat::Kohya,
                ..
            }
        ));
        // LoHa weights carry no widths: per-block modulation leaves out FLUX.2, a single
        // block leaves out HunyuanVideo, and FLUX.1 is left.
        assert_eq!(
            kind(
                S,
                &[
                    ("lycoris_double_blocks_18_img_mod_lin.hada_w1_a", &[16, 16]),
                    ("lycoris_single_blocks_37_linear1.hada_w1_a", &[16, 16]),
                ]
            ),
            FileKind::Lora {
                format: LoraFormat::Lycoris,
                base: Some(SdVersion::Flux)
            }
        );
        // The engine's `_lora.down.weight` and `_lora.up.weight` (name_conversion.cpp:1683-1685).
        assert!(matches!(
            lora("transformer.single_transformer_blocks.0.attn.to_q_lora.down.weight"),
            FileKind::Lora {
                format: LoraFormat::Kohya,
                ..
            }
        ));
        assert!(matches!(
            lora("transformer.single_transformer_blocks.0.attn.to_q_lora.up.weight"),
            FileKind::Lora {
                format: LoraFormat::Kohya,
                ..
            }
        ));
        assert!(matches!(
            lora("lycoris_x.lokr_w1"),
            FileKind::Lora {
                format: LoraFormat::Lycoris,
                ..
            }
        ));
        assert_eq!(
            lora("diffusion_model.joint_blocks.0.x_block.attn.qkv.diff_b"),
            FileKind::Lora {
                format: LoraFormat::Diff,
                base: Some(SdVersion::Sd3)
            }
        );
    }

    #[test]
    fn the_lora_base_rows_go_in_order_with_the_unet_fallback_last() {
        use SdVersion::*;
        let base = |names: &[(&str, &[u64])]| match kind(S, names) {
            FileKind::Lora { base, .. } => base,
            other => panic!("not a LoRA: {other:?}"),
        };
        let down = |name: &str| format!("{name}.lora_down.weight");
        for (module, width, want) in [
            // Kohya names DiT LoRAs `lora_unet_…` too: the DiT rows come first, and an
            // ambiguous one is None, never the SD 1 fallback.
            ("lora_unet_double_blocks_0_img_attn_proj", 3072, None),
            ("lora_unet_single_blocks_37_linear1", 3072, Some(Flux)),
            (
                "lora_unet_double_stream_modulation_img_lin",
                3072,
                Some(Flux2Klein),
            ),
            ("lora_unet_joint_blocks_0_x_block_attn_qkv", 1536, Some(Sd3)),
            (
                "transformer_blocks.0.img_mlp.net.0.proj",
                3072,
                Some(QwenImage),
            ),
            (
                "lora_unet_transformer_blocks_59_img_mod_1",
                3072,
                Some(QwenImage),
            ),
            ("diffusion_model.blocks.0.cross_attn.k", 5120, Some(Wan2)),
            (
                "diffusion_model.blocks.0.cross_attn.k",
                3072,
                Some(Wan22Ti2v),
            ),
            ("lora_unet_blocks_39_self_attn_o", 3072, None),
            (
                "diffusion_model.layers.0.adaLN_modulation.0",
                256,
                Some(ZImage),
            ),
            ("cap_embedder.1", 2560, Some(ZImage)),
            (
                "lora_te2_text_model_encoder_layers_0_mlp_fc1",
                1280,
                Some(Sdxl),
            ),
            ("te2.text_model.encoder.layers.0.mlp.fc1", 1280, Some(Sdxl)),
            ("lora_unet_input_blocks_4_1_proj_in", 640, Some(Sd1)),
            ("unknown.module", 3072, None),
        ] {
            assert_eq!(
                base(&[(&down(module), &[16, width])]),
                want,
                "{module} {width}"
            );
        }
        let to_k = down("lora_unet_input_blocks_1_1_transformer_blocks_0_attn2_to_k");
        for (width, want) in [(2048, Sdxl), (1024, Sd2), (768, Sd1)] {
            assert_eq!(base(&[(&to_k, &[8, width])]), Some(want), "{width}");
        }
        // The width row beats a DiT marker in the same file.
        assert_eq!(
            base(&[
                (&to_k, &[8, 768]),
                (&down("double_blocks.18.img_attn.qkv"), &[8, 3072])
            ]),
            Some(Sd1)
        );
        // The metadata row, after every name row.
        let mut ix = names_index(S, &[(&down("unknown.module"), &[16, 3072])]);
        for (trained_on, want) in [
            ("sd_v2_768_v", Some(Sd2)),
            ("sd_v1", Some(Sd1)),
            ("flux1", Some(Flux)),
            ("sdxl_base_v1-0", Some(Sdxl)),
            ("sd3", None),
        ] {
            ix.metadata
                .insert("ss_base_model_version".into(), trained_on.into());
            assert!(
                matches!(inspect(&ix).kind, FileKind::Lora { base, .. } if base == want),
                "{trained_on}"
            );
        }
    }

    #[test]
    fn extra_files_take_their_first_marker() {
        let gguf = |architecture: &str, names: &[(&str, &[u64])]| {
            let mut ix = names_index(G, names);
            ix.metadata
                .insert("general.architecture".into(), architecture.into());
            inspect(&ix).kind
        };
        let extra = |slot| FileKind::Extra { slot };
        assert_eq!(
            gguf(
                "clip",
                &[("mm.0.weight", &[4]), ("v.blk.0.attn_q.weight", &[4])]
            ),
            extra(SlotId::LlmVision)
        );
        assert_eq!(
            gguf("qwen2vl", &[("output_norm.weight", &[4])]),
            extra(SlotId::Llm)
        );
        // The architecture is GGUF's: a safetensors metadata key of the same name is not.
        let mut st = names_index(S, &[("output_norm.weight", &[4])]);
        st.metadata
            .insert("general.architecture".into(), "llama".into());
        assert_eq!(
            inspect(&st).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::UnknownWeights,
                version: None
            }
        );
        for (names, slot) in [
            (
                &[("model.layers.0.self_attn.q_proj.weight", &[4u64][..])][..],
                SlotId::Llm,
            ),
            (
                &[("encoder.block.0.layer.0.SelfAttention.q.weight", &[4][..])],
                SlotId::T5xxl,
            ),
            (
                &[(
                    "text_model.encoder.layers.0.layer_norm1.weight",
                    &[1280][..],
                )],
                SlotId::ClipG,
            ),
            (
                &[(
                    "vision_model.encoder.layers.0.layer_norm1.weight",
                    &[1024][..],
                )],
                SlotId::ClipVision,
            ),
            (
                &[(
                    "first_stage_model.decoder.conv_in.weight",
                    &[512, 4, 3, 3][..],
                )],
                SlotId::Vae,
            ),
            (
                &[
                    ("decoder.layers.0.weight", &[4][..]),
                    ("encoder.layers.0.weight", &[4][..]),
                ],
                SlotId::Tae,
            ),
            (
                &[("feature_extractor.conv_layers.0.conv.weight", &[4][..])],
                SlotId::AudioEncoder,
            ),
            (
                &[("wav2vec2.masked_spec_embed", &[4][..])],
                SlotId::AudioEncoder,
            ),
            // Not the picture VAE's exact `decoder.conv_in.weight`.
            (
                &[("audio_vae.decoder.conv_in.conv.weight", &[4][..])],
                SlotId::AudioVae,
            ),
            (
                &[("vocoder.vocoder.conv_pre.weight", &[4][..])],
                SlotId::AudioVae,
            ),
            (
                &[("video_embeddings_connector.learnable_registers", &[4][..])],
                SlotId::EmbeddingsConnectors,
            ),
            (
                &[("connectors.0.weight", &[4][..])],
                SlotId::EmbeddingsConnectors,
            ),
        ] {
            assert_eq!(kind(S, names), extra(slot), "{names:?}");
        }
        // A diffusion marker keeps llama.cpp names and a VAE decoder from reading as parts.
        for names in [
            &[
                ("token_embd.weight", &[4u64][..]),
                ("blk.0.attn_q.weight", &[4][..]),
                ("transformer.x.weight", &[4][..]),
            ][..],
            &[
                ("decoder.conv_in.weight", &[4][..]),
                ("diffusion_model.x.weight", &[4][..]),
            ],
        ] {
            assert_eq!(
                kind(G, names),
                FileKind::CannotRun {
                    reason: CannotRunReason::UnknownWeights,
                    version: None
                },
                "{names:?}"
            );
        }
    }

    #[test]
    fn svd_is_refused_and_unknown_weights_say_so() {
        assert_eq!(
            kind(
                S,
                &[(
                    "model.diffusion_model.input_blocks.8.0.time_mixer.mix_factor",
                    &[1]
                )]
            ),
            FileKind::CannotRun {
                reason: CannotRunReason::Svd,
                version: Some(SdVersion::Svd)
            }
        );
        assert_eq!(
            kind(S, &[("something.weight", &[4])]),
            FileKind::CannotRun {
                reason: CannotRunReason::UnknownWeights,
                version: None
            }
        );
    }

    #[test]
    fn facts_read_zero_cond_img_emb_pos_and_a_checkpoints_width() {
        let facts = |names: &[(&str, &[u64])]| inspect(&names_index(S, names)).facts;
        for (name, want) in [
            ("model.diffusion_model.__index_timestep_zero__", true),
            ("__index_timestep_zero__", true),
            ("x__index_timestep_zero__", false),
        ] {
            assert_eq!(facts(&[(name, &[1])]).zero_cond, want, "{name}");
        }
        let wan = facts(&[("img_emb.emb_pos", &[1, 514, 1280])]);
        assert!(wan.img_emb_pos && wan.has_img_emb);
        assert_eq!(
            facts(&[("model.diffusion_model.img_in.weight", &[3072, 128])]).flux2_width,
            Some(3072)
        );
        // In a checkpoint, a bare `img_in.weight` is not the diffusion model's.
        assert_eq!(
            facts(&[
                ("first_stage_model.x", &[1]),
                ("img_in.weight", &[3072, 128])
            ])
            .flux2_width,
            None
        );
        assert_eq!(facts(&[("x", &[1])]), MatchFacts::default());
    }

    /// Writes `index` as a header-only safetensors file at `path`.
    fn write_safetensors(path: &Path, index: &TensorIndex) {
        let header: serde_json::Map<String, Value> = index
            .tensors
            .iter()
            .map(|t| {
                let entry = json!({"dtype": t.dtype, "shape": t.shape, "data_offsets": [0, 0]});
                (t.name.clone(), entry)
            })
            .collect();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, safetensors(&Value::Object(header))).unwrap();
    }

    #[test]
    fn a_diffusers_folder_is_a_folder_model_with_its_parts() {
        let (unet, te) = (
            fixture("sd14-diffusers-unet"),
            fixture("sd14-diffusers-text-encoder"),
        );
        let facts = inspect_folder(&[(FolderPart::TextEncoder, &te), (FolderPart::Unet, &unet)]);
        assert_eq!(
            facts.kind,
            FileKind::Model {
                version: SdVersion::Sd1,
                layout: Layout::Folder,
                embeds: vec![SlotId::ClipL]
            }
        );
        assert_eq!(facts.parameters, 859_520_964 + 123_060_557);
        assert_eq!(
            inspect_folder(&[(FolderPart::TextEncoder, &te)]).kind,
            FileKind::CannotRun {
                reason: CannotRunReason::UnknownWeights,
                version: None
            }
        );

        // On disk, by the engine's names, under a Unicode path with a space.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("sd 1.4 ü");
        write_safetensors(&root.join("text_encoder").join("model.safetensors"), &te);
        assert!(matches!(
            inspect_local_folder(&root),
            Err(IndexError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound
        ));
        write_safetensors(
            &root
                .join("unet")
                .join("diffusion_pytorch_model.safetensors"),
            &unet,
        );
        write_safetensors(
            &root.join("vae").join("diffusion_pytorch_model.safetensors"),
            &fixture("ae-flux1"),
        );
        assert_eq!(
            inspect_local_folder(&root).unwrap().kind,
            FileKind::Model {
                version: SdVersion::Sd1,
                layout: Layout::Folder,
                embeds: vec![SlotId::ClipL, SlotId::Vae]
            }
        );
    }

    #[test]
    fn a_local_file_is_inspected_from_its_header() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("日本語 スタイル.safetensors");
        write_safetensors(&path, &fixture("clip-l"));
        let facts = inspect_local(&path).unwrap();
        assert_eq!(
            facts.kind,
            FileKind::Extra {
                slot: SlotId::ClipL
            }
        );
        assert_eq!(facts.format, FileFormat::Safetensors);
        assert!(matches!(
            inspect_local(&dir.path().join("missing.gguf")),
            Err(IndexError::Io(_))
        ));
    }

    #[test]
    fn file_facts_serialize_for_the_webview() {
        assert_eq!(
            serde_json::to_value(inspect(&fixture("sd15"))).unwrap(),
            json!({
                "kind": {"model": {"version": "sd1", "layout": "checkpoint", "embeds": ["clip_l", "vae"]}},
                "facts": {"sd3Blocks": null, "flux2Width": null, "zeroCond": false, "imgEmbPos": false, "hasImgEmb": false},
                "parameters": 1_066_247_386u64,
                "format": "safetensors"
            })
        );
        for (kind, want) in [
            (
                FileKind::CannotRun {
                    reason: CannotRunReason::LtxVideo09,
                    version: Some(SdVersion::Ltxav),
                },
                json!({"cannotRun": {"reason": "ltxVideo09", "version": "ltxav"}}),
            ),
            (
                FileKind::Lora {
                    format: LoraFormat::Kohya,
                    base: None,
                },
                json!({"lora": {"format": "kohya", "base": null}}),
            ),
            (
                FileKind::Extra {
                    slot: SlotId::AudioVae,
                },
                json!({"extra": {"slot": "audio_vae"}}),
            ),
            (
                FileKind::Model {
                    version: SdVersion::Flux,
                    layout: Layout::DiffusionOnly,
                    embeds: vec![],
                },
                json!({"model": {"version": "flux", "layout": "diffusionOnly", "embeds": []}}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&kind).unwrap(), want);
            assert_eq!(serde_json::from_value::<FileKind>(want).unwrap(), kind);
        }
    }

    #[test]
    fn every_fixture_keeps_its_answer() {
        use SdVersion::*;
        let model = |version, layout, embeds: &[SlotId]| FileKind::Model {
            version,
            layout,
            embeds: embeds.to_vec(),
        };
        let dit = |version| model(version, Layout::DiffusionOnly, &[]);
        let extra = |slot| FileKind::Extra { slot };
        let fixtures = [
            (
                "sd15",
                model(Sd1, Layout::Checkpoint, &[SlotId::ClipL, SlotId::Vae]),
            ),
            (
                "sdxl",
                model(
                    Sdxl,
                    Layout::Checkpoint,
                    &[SlotId::ClipL, SlotId::ClipG, SlotId::Vae],
                ),
            ),
            (
                "sd35-medium",
                model(Sd3, Layout::Checkpoint, &[SlotId::Vae]),
            ),
            ("flux1-dev", dit(Flux)),
            ("flux1-kontext", dit(Flux)),
            ("chroma", dit(Chroma)),
            ("qwen-image", dit(QwenImage)),
            ("qwen-image-edit-2509", dit(QwenImage)),
            ("z-image-turbo", dit(ZImage)),
            ("flux2-klein-4b", dit(Flux2Klein)),
            ("wan21-t2v-1.3b", model(Wan2, Layout::Checkpoint, &[])),
            ("wan21-i2v-14b", dit(Wan2)),
            ("wan22-ti2v-5b", dit(Wan22Ti2v)),
            ("wan22-t2v-a14b-high", dit(Wan2)),
            ("wan22-t2v-a14b-low", dit(Wan2)),
            ("hunyuan-video-15", dit(HunyuanVideo)),
            ("ltx23", dit(Ltxav)),
            (
                "sdxl-lora-pixel-art",
                FileKind::Lora {
                    format: LoraFormat::Kohya,
                    base: Some(Sdxl),
                },
            ),
            (
                "flux-lora-realism",
                FileKind::Lora {
                    format: LoraFormat::Kohya,
                    base: Some(Flux),
                },
            ),
            (
                "lumina2",
                FileKind::CannotRun {
                    reason: CannotRunReason::Lumina2,
                    version: Some(ZImage),
                },
            ),
            (
                "sd14-diffusers-unet",
                FileKind::CannotRun {
                    reason: CannotRunReason::UnknownWeights,
                    version: None,
                },
            ),
            ("sd14-diffusers-text-encoder", extra(SlotId::ClipL)),
            ("ae-flux1", extra(SlotId::Vae)),
            ("clip-l", extra(SlotId::ClipL)),
            ("umt5-xxl-q5km", extra(SlotId::T5xxl)),
        ];
        assert_eq!(fixtures.len(), 25);
        for (name, want) in fixtures {
            assert_eq!(inspect(&fixture(name)).kind, want, "{name}");
        }
        for (name, want) in [
            ("klein4b", dit(Flux2Klein)),
            ("klein9b", dit(Flux2Klein)),
            ("zimage", dit(ZImage)),
            ("wan22-ti2v-5b", dit(Wan22Ti2v)),
            ("wan21-t2v-1.3b", model(Wan2, Layout::Checkpoint, &[])),
            ("flux3-klein", dit(Flux3)),
            ("qwen3-4b", extra(SlotId::Llm)),
        ] {
            assert_eq!(inspect(&gguf_txt_fixture(name)).kind, want, "{name}");
        }
    }

    const UNKNOWN: FileKind = FileKind::CannotRun {
        reason: CannotRunReason::UnknownWeights,
        version: None,
    };

    #[test]
    fn a_wan_vae_is_an_extra_vae_in_both_its_versions() {
        let vae = FileKind::Extra { slot: SlotId::Vae };
        // Wan 2.1, also Qwen-Image's: z_dim 16, decoder width 384, RGB out.
        let wan21: &[(&str, &[u64])] = &[
            ("conv1.weight", &[32, 32, 1, 1, 1]),
            ("conv2.weight", &[16, 16, 1, 1, 1]),
            ("decoder.conv1.bias", &[384]),
            ("decoder.conv1.weight", &[384, 16, 3, 3, 3]),
            ("decoder.head.0.gamma", &[96, 1, 1, 1]),
            ("decoder.head.2.weight", &[3, 96, 3, 3, 3]),
            ("decoder.middle.0.residual.0.gamma", &[384, 1, 1, 1]),
            (
                "decoder.upsamples.0.residual.2.weight",
                &[384, 384, 3, 3, 3],
            ),
            ("encoder.conv1.weight", &[96, 3, 3, 3, 3]),
            ("encoder.head.2.weight", &[32, 384, 3, 3, 3]),
        ];
        assert_eq!(kind(S, wan21), vae);
        // Wan 2.2 TI2V-5B: z_dim 48, decoder width 1024, 12 patchified channels out, and
        // the nested `upsamples.N.upsamples.M` of its up blocks.
        let wan22: &[(&str, &[u64])] = &[
            ("conv1.weight", &[96, 96, 1, 1, 1]),
            ("conv2.weight", &[48, 48, 1, 1, 1]),
            ("decoder.conv1.weight", &[1024, 48, 3, 3, 3]),
            ("decoder.head.2.weight", &[12, 256, 3, 3, 3]),
            (
                "decoder.upsamples.0.upsamples.0.residual.2.weight",
                &[1024, 1024, 3, 3, 3],
            ),
            ("encoder.conv1.weight", &[160, 12, 3, 3, 3]),
        ];
        assert_eq!(kind(S, wan22), vae);
        // A decode-only export, and a 2D one with 4-D weights (`wan_vae.hpp:1326-1327`).
        assert_eq!(
            kind(
                G,
                &[
                    ("conv2.weight", &[1, 1, 16, 16]),
                    ("decoder.conv1.weight", &[3, 3, 16, 384]),
                    ("decoder.head.2.weight", &[3, 3, 96, 3]),
                ]
            ),
            vae
        );
        // It takes all three names.
        assert_eq!(kind(S, &wan21[3..4]), UNKNOWN);
        assert_eq!(kind(S, &[wan21[1], wan21[3]]), UNKNOWN, "no decoder.head.2");
        assert_eq!(kind(S, &[wan21[3], wan21[5]]), UNKNOWN, "no conv2");
        assert_eq!(kind(S, &[wan21[1], wan21[5]]), UNKNOWN, "no decoder.conv1");
    }

    #[test]
    fn hunyuan_and_ltx_causal_vaes_are_extra_vaes() {
        let vae = FileKind::Extra { slot: SlotId::Vae };
        // HunyuanVideo 1.5: 32 latent channels, RMS norms, `mid.block_1`, `up.N.block.M`.
        let hunyuan: &[(&str, &[u64])] = &[
            ("decoder.conv_in.conv.bias", &[1024]),
            ("decoder.conv_in.conv.weight", &[1024, 32, 3, 3, 3]),
            ("decoder.conv_out.conv.weight", &[3, 128, 3, 3, 3]),
            (
                "decoder.mid.block_1.conv1.conv.weight",
                &[1024, 1024, 3, 3, 3],
            ),
            ("decoder.mid.block_1.norm1.gamma", &[1024, 1, 1, 1]),
            ("decoder.norm_out.gamma", &[128, 1, 1, 1]),
            (
                "decoder.up.0.block.0.conv1.conv.weight",
                &[1024, 1024, 3, 3, 3],
            ),
            ("encoder.conv_in.conv.weight", &[128, 3, 3, 3, 3]),
        ];
        assert_eq!(kind(S, hunyuan), vae);
        // LTX: 128 latent channels, `up_blocks.N.res_blocks.M`, 48 patchified channels out,
        // and the per-channel statistics.
        let ltx: &[(&str, &[u64])] = &[
            ("decoder.conv_in.conv.bias", &[1024]),
            ("decoder.conv_in.conv.weight", &[1024, 128, 3, 3, 3]),
            ("decoder.conv_out.conv.weight", &[48, 128, 3, 3, 3]),
            (
                "decoder.up_blocks.0.res_blocks.0.conv1.conv.weight",
                &[1024, 1024, 3, 3, 3],
            ),
            ("encoder.conv_in.conv.weight", &[128, 48, 3, 3, 3]),
            ("per_channel_statistics.mean-of-means", &[128]),
            ("per_channel_statistics.std-of-means", &[128]),
        ];
        assert_eq!(kind(S, ltx), vae);
        // A decoder needs both ends.
        assert_eq!(kind(S, &ltx[1..2]), UNKNOWN);
        // The LTX audio VAE's same names sit under `audio_vae.`.
        assert_eq!(
            kind(
                S,
                &[
                    ("audio_vae.decoder.conv_in.conv.weight", &[512, 8, 3, 3]),
                    ("audio_vae.decoder.conv_out.conv.weight", &[2, 128, 3, 3]),
                ]
            ),
            FileKind::Extra {
                slot: SlotId::AudioVae
            }
        );
    }

    #[test]
    fn no_model_lora_or_marked_file_reads_as_one_of_these_vaes() {
        let wan_vae: [(&str, &[u64]); 3] = [
            ("conv2.weight", &[16, 16, 1, 1, 1]),
            ("decoder.conv1.weight", &[384, 16, 3, 3, 3]),
            ("decoder.head.2.weight", &[3, 96, 3, 3, 3]),
        ];
        let causal_vae: [(&str, &[u64]); 2] = [
            ("decoder.conv_in.conv.weight", &[1024, 128, 3, 3, 3]),
            ("decoder.conv_out.conv.weight", &[48, 128, 3, 3, 3]),
        ];
        let with = |mut index: TensorIndex, names: &[(&str, &[u64])]| {
            index
                .tensors
                .extend(names_index(index.format, names).tensors);
            index
        };
        // A diffusion model that carries the VAE's names is still the model.
        assert!(matches!(
            inspect(&with(fixture("wan21-i2v-14b"), &wan_vae)).kind,
            FileKind::Model {
                version: SdVersion::Wan2,
                ..
            }
        ));
        assert!(matches!(
            inspect(&with(fixture("ltx23"), &causal_vae)).kind,
            FileKind::Model {
                version: SdVersion::Ltxav,
                ..
            }
        ));
        // With a diffusion marker the engine does not recognise, they are unknown weights.
        for vae in [&wan_vae[..], &causal_vae[..]] {
            for marker in ["model.diffusion_model.x.weight", "diffusion_model.x.weight"] {
                let names = with(names_index(S, &[(marker, &[4])]), vae);
                assert_eq!(inspect(&names).kind, UNKNOWN, "{marker}");
            }
        }
        // A LoRA for a VAE is a LoRA.
        assert!(matches!(
            kind(
                S,
                &[
                    ("decoder.conv_in.conv.lora_down.weight", &[8, 1024]),
                    ("decoder.conv1.lora_down.weight", &[8, 384]),
                ]
            ),
            FileKind::Lora { .. }
        ));
    }

    /// A kohya LoRA with a down and an up weight for each `(module, in, out)`.
    fn lora_of(modules: &[(&str, u64, u64)]) -> TensorIndex {
        let mut tensors = Vec::new();
        for (module, input, output) in modules {
            for (suffix, shape) in [
                ("lora_down", vec![16, *input]),
                ("lora_up", vec![*output, 16]),
            ] {
                tensors.push(TensorInfo {
                    name: format!("{module}.{suffix}.weight"),
                    dtype: "BF16".into(),
                    shape,
                });
            }
        }
        TensorIndex {
            format: S,
            tensors,
            metadata: Default::default(),
            parameters_hint: None,
        }
    }

    fn base_of(modules: &[(&str, u64, u64)]) -> Option<SdVersion> {
        match inspect(&lora_of(modules)).kind {
            FileKind::Lora { base, .. } => base,
            other => panic!("not a LoRA: {other:?}"),
        }
    }

    #[test]
    fn a_flux_1_lora_is_told_from_flux_2_by_its_blocks_and_mlp() {
        use SdVersion::*;
        // FLUX.1: 19 double and 38 single blocks, a 4x MLP (flux.hpp:43-47, 325-330).
        assert_eq!(
            base_of(&[
                ("diffusion_model.double_blocks.18.img_attn.qkv", 3072, 9216),
                ("diffusion_model.double_blocks.18.img_mlp.0", 3072, 12288),
                ("diffusion_model.single_blocks.37.linear1", 3072, 21504),
            ]),
            Some(Flux)
        );
        // Block 0 alone: the MLP's width decides.
        assert_eq!(
            base_of(&[("lora_unet_double_blocks_0_img_mlp_0", 3072, 12288)]),
            Some(Flux)
        );
        assert_eq!(
            base_of(&[("lora_unet_double_blocks_0_img_mlp_0", 3072, 18432)]),
            Some(Flux2Klein)
        );
        // Per-block modulation is FLUX.1's; FLUX.2 shares one (flux.hpp:449-450, 986-990).
        assert_eq!(
            base_of(&[("diffusion_model.double_blocks.0.img_mod.lin", 3072, 18432)]),
            Some(Flux)
        );
    }

    #[test]
    fn klein_4b_klein_9b_and_flux_2_dev_loras_are_told_apart_by_width() {
        use SdVersion::*;
        assert_eq!(
            base_of(&[
                ("diffusion_model.double_blocks.4.img_attn.qkv", 3072, 9216),
                ("diffusion_model.single_blocks.19.linear1", 3072, 27648),
                ("diffusion_model.single_blocks.19.linear2", 12288, 3072),
            ]),
            Some(Flux2Klein)
        );
        assert_eq!(
            base_of(&[("double_stream_modulation_img.lin", 3072, 18432)]),
            Some(Flux2Klein)
        );
        assert_eq!(
            base_of(&[
                ("diffusion_model.double_blocks.7.img_attn.qkv", 4096, 12288),
                ("diffusion_model.single_blocks.23.linear1", 4096, 36864),
            ]),
            Some(Flux2Klein)
        );
        assert_eq!(
            base_of(&[("diffusion_model.single_blocks.47.linear1", 6144, 55296)]),
            Some(Flux2)
        );
    }

    #[test]
    fn diffusers_named_dit_loras_get_a_base_where_the_engine_converts_them() {
        use SdVersion::*;
        // The engine's FLUX map (name_conversion.cpp:617-741): a 4x MLP is FLUX.1's.
        assert_eq!(
            base_of(&[
                (
                    "transformer.single_transformer_blocks.37.attn.to_q",
                    3072,
                    3072
                ),
                (
                    "transformer.single_transformer_blocks.37.proj_mlp",
                    3072,
                    12288
                ),
            ]),
            Some(Flux)
        );
        // ai-toolkit's kohya-style prefix, with FLUX.2's 6x gated MLP.
        assert_eq!(
            base_of(&[(
                "lora_transformer_single_transformer_blocks_0_proj_mlp",
                3072,
                18432
            )]),
            Some(Flux2Klein)
        );
        // PEFT names: lora_A is the down weight, lora_B the up weight.
        let peft = |input: u64, output: u64| TensorIndex {
            format: S,
            tensors: vec![
                TensorInfo {
                    name: "transformer.transformer_blocks.0.ff.net.0.proj.lora_A.weight".into(),
                    dtype: "BF16".into(),
                    shape: vec![16, input],
                },
                TensorInfo {
                    name: "transformer.transformer_blocks.0.ff.net.0.proj.lora_B.weight".into(),
                    dtype: "BF16".into(),
                    shape: vec![output, 16],
                },
            ],
            metadata: Default::default(),
            parameters_hint: None,
        };
        assert!(matches!(
            inspect(&peft(3072, 18432)).kind,
            FileKind::Lora {
                format: LoraFormat::Peft,
                base: Some(Flux2Klein)
            }
        ));
        // A single block's attention leaves out Qwen-Image, which has none.
        assert_eq!(
            base_of(&[
                ("transformer.transformer_blocks.18.attn.to_q", 3072, 3072),
                (
                    "transformer.single_transformer_blocks.0.attn.to_q",
                    3072,
                    3072
                ),
            ]),
            Some(Flux)
        );
    }

    #[test]
    fn qwen_image_and_z_image_loras_have_their_own_names() {
        use SdVersion::*;
        assert_eq!(
            base_of(&[
                ("transformer.transformer_blocks.59.attn.to_q", 3072, 3072),
                (
                    "transformer.transformer_blocks.59.img_mlp.net.0.proj",
                    3072,
                    12288
                ),
            ]),
            Some(QwenImage)
        );
        assert_eq!(
            base_of(&[
                ("diffusion_model.layers.29.attention.qkv", 3840, 11520),
                ("diffusion_model.layers.29.feed_forward.w1", 3840, 10240),
                (
                    "diffusion_model.noise_refiner.1.adaLN_modulation.0",
                    256,
                    15360
                ),
            ]),
            Some(ZImage)
        );
        // The diffusers split attention the engine converts (name_conversion.cpp:874-911).
        assert_eq!(
            base_of(&[("transformer.layers.0.attention.to_q", 3840, 3840)]),
            Some(ZImage)
        );
    }

    #[test]
    fn wan_loras_are_wan_2_or_ti2v_5b_by_width() {
        use SdVersion::*;
        assert_eq!(
            base_of(&[
                ("diffusion_model.blocks.29.self_attn.q", 1536, 1536),
                ("diffusion_model.blocks.29.ffn.0", 1536, 8960),
            ]),
            Some(Wan2)
        );
        assert_eq!(
            base_of(&[("lora_unet_blocks_39_cross_attn_k_img", 5120, 5120)]),
            Some(Wan2)
        );
        assert_eq!(
            base_of(&[("diffusion_model.blocks.29.self_attn.q", 3072, 3072)]),
            Some(Wan22Ti2v)
        );
    }

    #[test]
    fn hunyuan_video_loras_are_no_longer_flux() {
        // FLUX's double-block names at 2048 wide, 54 blocks (hunyuan.hpp:165-171, 327).
        assert_eq!(
            base_of(&[
                ("diffusion_model.double_blocks.53.img_attn.qkv", 2048, 6144),
                ("diffusion_model.double_blocks.53.img_mlp.0", 2048, 8192),
            ]),
            Some(SdVersion::HunyuanVideo)
        );
        // Tencent's own names, which the engine converts (name_conversion.cpp:762-829).
        assert_eq!(
            base_of(&[
                ("double_blocks.0.img_mlp.fc1", 2048, 8192),
                ("double_blocks.0.img_mod.linear", 2048, 12288),
            ]),
            Some(SdVersion::HunyuanVideo)
        );
    }

    #[test]
    fn ltx_loras_are_ltx_and_their_audio_cross_attention_is_not_sdxl() {
        assert_eq!(
            base_of(&[
                (
                    "diffusion_model.transformer_blocks.47.attn1.to_q",
                    4096,
                    4096
                ),
                (
                    "diffusion_model.transformer_blocks.47.audio_attn1.to_q",
                    2048,
                    2048
                ),
                (
                    "diffusion_model.transformer_blocks.47.ff.net.0.proj",
                    4096,
                    16384
                ),
            ]),
            Some(SdVersion::Ltxav)
        );
        // `audio_attn2_to_k` meets SD's `attn2_to_k` once `_` reads as `.`; its 2048 is
        // not SDXL's context.
        assert_eq!(
            base_of(&[(
                "lora_unet_transformer_blocks_0_audio_attn2_to_k",
                2048,
                2048
            )]),
            Some(SdVersion::Ltxav)
        );
    }

    #[test]
    fn evidence_that_fits_two_primary_families_gives_none() {
        // A 3072-wide double block below 5: FLUX.1 or Klein 4B.
        assert_eq!(
            base_of(&[("diffusion_model.double_blocks.0.img_attn.qkv", 3072, 9216)]),
            None
        );
        // FLUX's diffusers attention names are Qwen-Image's too.
        assert_eq!(
            base_of(&[("transformer.transformer_blocks.0.attn.to_q", 3072, 3072)]),
            None
        );
    }

    #[test]
    fn a_sharing_familys_lora_needs_evidence_of_its_own() {
        use SdVersion::*;
        // Ming- or LLaDA-Image names that are Z-Image's: the primary family wins.
        assert_eq!(
            base_of(&[("diffusion_model.layers.0.attention.qkv", 3840, 11520)]),
            Some(ZImage)
        );
        // With LLaDA's own refiner, it is LLaDA-Image (llada_image.hpp:167-172).
        assert_eq!(
            base_of(&[
                ("diffusion_model.layers.0.attention.qkv", 3840, 11520),
                ("diffusion_model.sigvq_refiner.0.attention.qkv", 3840, 11520),
            ]),
            Some(LladaImage)
        );
        // Names a primary family has, at sizes it has not, and nothing else: None.
        for modules in [
            // Lumina 2's width on Z-Image's names.
            &[("diffusion_model.layers.0.attention.qkv", 2304, 6912)][..],
            // HunyuanVideo 1's blocks on FLUX.1's names.
            &[
                ("diffusion_model.double_blocks.19.img_attn.qkv", 3072, 9216),
                ("diffusion_model.single_blocks.39.linear1", 3072, 21504),
            ],
            // FLUX.2's names at a width no FLUX.2 has (SeFi-Image's, say).
            &[("diffusion_model.single_blocks.0.linear1", 2048, 18432)],
            // Ovis-Image shares FLUX's attention; this LoRA trained nothing else.
            &[("diffusion_model.double_blocks.0.img_attn.qkv", 2560, 7680)],
        ] {
            assert_eq!(base_of(modules), None, "{modules:?}");
        }
        // Anima and PixArt no longer read as Wan.
        assert_eq!(
            base_of(&[("diffusion_model.net.blocks.0.self_attn.q_proj", 2048, 2048)]),
            Some(Anima)
        );
        assert_eq!(
            base_of(&[("diffusion_model.blocks.27.cross_attn.q_linear", 1152, 1152)]),
            Some(Pixart)
        );
    }

    #[test]
    fn every_other_family_is_reached_by_names_only_it_has() {
        use SdVersion::*;
        for (module, want) in [
            (
                "diffusion_model.double_blocks.0.img_mlp.gate_proj",
                Some(OvisImage),
            ),
            ("diffusion_model.single_blocks.0.mlp_in", Some(Flux3)),
            (
                "transformer.single_transformer_blocks.0.ff.linear_in",
                Some(Flux3),
            ),
            (
                "transformer.transformer_blocks.0.img_mlp.gate_up",
                Some(QwenImage),
            ),
            (
                "diffusion_model.transformer_blocks.0.attn.img_qkv",
                Some(Lens),
            ),
            ("diffusion_model.double_blocks.0.img_qkv", Some(Minit2i)),
            (
                "diffusion_model.txt_preamble_blocks.0.attn_proj",
                Some(Minit2i),
            ),
            (
                "diffusion_model.blocks.0.self_attn.output_proj",
                Some(Anima),
            ),
            (
                "diffusion_model.llm_adapter.blocks.0.self_attn.o_proj",
                Some(Anima),
            ),
            ("transformer.transformer_blocks.0.attn2.to_k", None),
            (
                "diffusion_model.layers.0.self_attention.to_q",
                Some(ErnieImage),
            ),
            ("diffusion_model.layers.0.feed_forward.w13", Some(Fibo2)),
            ("diffusion_model.layers.0.attention.o", Some(Ideogram4)),
            (
                "model.language_model.layers.0.self_attn.q_proj",
                Some(HidreamO1),
            ),
            ("diffusion_model.blocks.0.q_proj", Some(Iris)),
            ("diffusion_model.pixel_blocks.0.compress", Some(Iris)),
            ("diffusion_model.blocks.0.attn.wq", Some(Krea2)),
            ("transformer.transformer_blocks.0.ff.gate", Some(Krea2)),
            ("diffusion_model.blocks.0.ffn.gate_proj", Some(LingbotVideo)),
            ("diffusion_model.layers.0.proj.ffn_up_gate", Some(MossImage)),
            (
                "diffusion_model.double_stream_layers.0.img_instruct_attn.processor.img_to_q",
                Some(BooguImage),
            ),
            // MiniMax-H3, MiniMax-H3-Image and P-H3-Image share one DiT: None, and never the
            // SD 1 fallback for its kohya names.
            ("lora_unet_blocks_0_attn_qkv_proj", None),
            ("diffusion_model.token_refiner.blocks.0.mlp.fc1", None),
        ] {
            // 1152 is no primary family's width and no UNet's context.
            assert_eq!(base_of(&[(module, 1152, 1152)]), want, "{module}");
        }
    }

    #[test]
    fn every_labelled_sharing_family_has_a_name_of_its_own() {
        let families = families();
        for f in families.iter().filter(|f| !f.primary && f.base.is_some()) {
            let own = f.layers.iter().any(|l| {
                families
                    .iter()
                    .filter(|g| g.layers.iter().any(|m| m.parts == l.parts))
                    .all(|g| g.base == f.base)
            });
            assert!(own, "{:?}", f.base);
        }
    }

    #[test]
    fn every_lora_family_is_reachable_or_listed_as_not() {
        use SdVersion::*;
        // R7's matching table refuses svd, wan2.2-s2v, pid and sensenova-u1.5, and every other
        // version has an in-scope recipe; every recipe takes LoRAs (`recipes.rs`, "lora" is a
        // default feature).
        let in_scope: BTreeSet<SdVersion> = SdVersion::ALL
            .into_iter()
            .filter(|v| ![Svd, Wan22S2v, Pid, SensenovaU15].contains(v))
            .map(SdVersion::lora_family)
            .collect();
        assert_eq!(in_scope.len(), 36);
        let sd_rows = [Sd1, Sd2, Sdxl, Sd3];
        let by_names: BTreeSet<SdVersion> = families().iter().filter_map(|f| f.base).collect();
        // Their LoRAs carry another family's names, and nothing the engine fixes tells them
        // apart: they read as that family, or as None.
        let unreachable = [
            Longcat,
            SefiImage,
            MageFlow,
            MingImage,
            MinimaxH3,
            MinimaxH3Image,
            PH3Image,
        ];
        for family in in_scope {
            let ways = [
                sd_rows.contains(&family),
                by_names.contains(&family),
                unreachable.contains(&family),
            ];
            assert_eq!(ways.iter().filter(|w| **w).count(), 1, "{family:?}");
        }
    }
}
