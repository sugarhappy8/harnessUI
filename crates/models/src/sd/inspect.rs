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
    if ends(&[".lora_down.weight", ".lora_up.weight", ".lora.down.weight"]) {
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

/// A test on a LoRA tensor name with `_` read as `.`.
type DottedTest = fn(&str) -> bool;

/// The version a LoRA was most likely made for: the first row that matches. The markers
/// are matched on each name with `_` read as `.`, so kohya's `lora_unet_double_blocks_0_…`
/// meets `double_blocks`; they are written here in that dotted form.
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
        names.iter().zip(&index.tensors).any(|(n, t)| {
            n.ends_with("attn2.to.k.lora.down.weight") && engine_ne(t, index.format)[0] == width
        })
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
    let rows: [(SdVersion, DottedTest); 6] = [
        (Flux2, |n| n.contains("double.stream.modulation")),
        (Flux, |n| {
            n.contains("double.blocks") || n.contains("single.blocks")
        }),
        (Sd3, |n| n.contains("joint.blocks")),
        (QwenImage, |n| {
            numbered(n, "transformer.blocks.", ".img.mod")
                || numbered(n, "transformer.blocks.", ".img.mlp")
        }),
        (Wan2, |n| {
            numbered(n, "blocks.", ".cross.attn") || numbered(n, "blocks.", ".self.attn")
        }),
        (ZImage, |n| {
            n.contains("cap.embedder") || numbered(n, "layers.", ".adaLN.modulation")
        }),
    ];
    if let Some((base, _)) = rows.iter().find(|(_, test)| any(test)) {
        return Some(*base);
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
    // The UNet fallback, after every DiT row: kohya names a FLUX, SD 3 or Wan LoRA
    // `lora_unet_…` too.
    if index
        .tensors
        .iter()
        .any(|t| t.name.starts_with("lora_unet_"))
    {
        return Some(Sd1);
    }
    None
}

/// The numbers that follow `head` in `name`, each with the rest of the name after it.
fn numbers_after<'a>(name: &'a str, head: &'a str) -> impl Iterator<Item = (u32, &'a str)> + 'a {
    name.match_indices(head).filter_map(move |(at, _)| {
        let rest = &name[at + head.len()..];
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        Some((rest[..digits].parse().ok()?, &rest[digits..]))
    })
}

/// `name` holds `head`, a number, then `tail` (`blocks.` 12 `.cross.attn`).
fn numbered(name: &str, head: &str, tail: &str) -> bool {
    numbers_after(name, head).any(|(_, rest)| rest.starts_with(tail))
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
        assert_eq!(
            lora("lycoris_double_blocks_0_img_attn_qkv.hada_w1_a"),
            FileKind::Lora {
                format: LoraFormat::Lycoris,
                base: Some(SdVersion::Flux)
            }
        );
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
        for (module, want) in [
            // Kohya names DiT LoRAs `lora_unet_…` too: the DiT rows come first.
            ("lora_unet_double_blocks_0_img_attn_proj", Some(Flux)),
            ("lora_unet_single_blocks_37_linear1", Some(Flux)),
            ("lora_unet_double_stream_modulation_img_lin", Some(Flux2)),
            ("lora_unet_joint_blocks_0_x_block_attn_qkv", Some(Sd3)),
            ("transformer_blocks.0.img_mlp.net.0.proj", Some(QwenImage)),
            ("lora_unet_transformer_blocks_59_img_mod_1", Some(QwenImage)),
            ("diffusion_model.blocks.0.cross_attn.k", Some(Wan2)),
            ("lora_unet_blocks_39_self_attn_o", Some(Wan2)),
            ("diffusion_model.layers.0.adaLN_modulation.0", Some(ZImage)),
            ("cap_embedder.1", Some(ZImage)),
            ("lora_te2_text_model_encoder_layers_0_mlp_fc1", Some(Sdxl)),
            ("te2.text_model.encoder.layers.0.mlp.fc1", Some(Sdxl)),
            ("lora_unet_input_blocks_4_1_proj_in", Some(Sd1)),
            ("unknown.module", None),
        ] {
            assert_eq!(base(&[(&down(module), &[16, 3072])]), want, "{module}");
        }
        let to_k = down("lora_unet_input_blocks_1_1_transformer_blocks_0_attn2_to_k");
        for (width, want) in [(2048, Sdxl), (1024, Sd2), (768, Sd1)] {
            assert_eq!(base(&[(&to_k, &[8, width])]), Some(want), "{width}");
        }
        // The width row beats a DiT marker in the same file.
        assert_eq!(
            base(&[(&to_k, &[8, 768]), (&down("double_blocks.0.x"), &[8, 8])]),
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
}
