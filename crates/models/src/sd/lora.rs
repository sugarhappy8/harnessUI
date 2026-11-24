//! What a LoRA is (design section 7.2): its format, by the engine's LoRA suffixes, and the
//! model version it was most likely made for.
//!
//! The base comes first from the SD rows (CLIP-G's `te2`, a UNet's cross-attention
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

use serde::{Deserialize, Serialize};

use crate::sd::detect::SdVersion;
use crate::sd::index::{TensorIndex, engine_ne};

/// A LoRA's naming, by the engine's LoRA suffixes (`src/name_conversion.cpp:1676-1709`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoraFormat {
    Kohya,
    Peft,
    Lycoris,
    Diff,
}

/// The first format whose names the file has.
pub(crate) fn format(index: &TensorIndex) -> Option<LoraFormat> {
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
pub(crate) fn base(index: &TensorIndex) -> Option<SdVersion> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::index::{FileFormat, TensorInfo};
    use crate::sd::inspect::{FileKind, inspect};
    use crate::sd::testing::index_of;

    const S: FileFormat = FileFormat::Safetensors;

    fn kind(format: FileFormat, names: &[(&str, &[u64])]) -> FileKind {
        inspect(&index_of(format, names)).kind
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
        let mut ix = index_of(S, &[(&down("unknown.module"), &[16, 3072])]);
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
