//! The pinned engine's model-version detection, ported: `ModelLoader::get_sd_version`
//! (`src/model_loader.cpp:443-763`) over the catalog the engine builds from the files it is
//! given, with its two passes (design section 7). Line numbers are at the pin.
//!
//! The engine loads each slot's file under a name prefix, in a fixed order
//! (`src/pipeline/diffusion_engine.cpp:691-836`), into one insertion-ordered catalog. It
//! detects on those names, renames every tensor with that first answer
//! (`rebuild_catalog`, `src/model_loader_files.cpp:46-71`; `convert_tensor_name`,
//! `src/name_conversion.cpp:1741-1781`), and detects again. The second answer is the one it
//! runs (`diffusion_engine.cpp:977`).
//!
//! Of `convert_tensor_name` the port emulates the two prefix maps and the one diffusers
//! rename that reaches a detection row (SD 1 and SD 2's `up_blocks.2.attentions.1.`, for
//! SDXS-09); it leaves out every other rename: the per-family diffusers-to-original names (so
//! a diffusers-layout DiT keeps its diffusers names in pass 2), the text-encoder renames that
//! run whatever the version (open_clip and llama.cpp names to Hugging Face ones,
//! `name_conversion.cpp:155-243`), and the LoRA and IP-Adapter names.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::sd::index::{TensorIndex, engine_ne};
use crate::sd::recipes::SlotId;

macro_rules! versions {
    ($($variant:ident = $word:literal,)*) => {
        /// A model version as the engine names it (`enum SDVersion`, `src/model.h:13-76`), in
        /// the engine's order. `VERSION_ESRGAN` (an upscaler, never detected) is left out, and
        /// `VERSION_COUNT`, the engine's "no answer", is `None`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        pub enum SdVersion {
            $(#[serde(rename = $word)] $variant,)*
        }

        impl SdVersion {
            pub const ALL: [SdVersion; 60] = [$(SdVersion::$variant,)*];

            /// The version's word: kebab-case of its name, with the engine's dots kept.
            pub fn word(self) -> &'static str {
                match self {
                    $(SdVersion::$variant => $word,)*
                }
            }
        }
    };
}

versions! {
    Sd1 = "sd1",
    Sd1Inpaint = "sd1-inpaint",
    Sd1Pix2pix = "sd1-pix2pix",
    Sd1TinyUnet = "sd1-tiny-unet",
    Sd2 = "sd2",
    Sd2Inpaint = "sd2-inpaint",
    Sd2TinyUnet = "sd2-tiny-unet",
    Sdxs512Ds = "sdxs-512-ds",
    Sdxs09 = "sdxs-09",
    Sdxl = "sdxl",
    SdxlInpaint = "sdxl-inpaint",
    SdxlPix2pix = "sdxl-pix2pix",
    SdxlVega = "sdxl-vega",
    SdxlSsd1b = "sdxl-ssd1b",
    Svd = "svd",
    Sd3 = "sd3",
    Flux = "flux",
    FluxFill = "flux-fill",
    FluxControls = "flux-controls",
    Flex2 = "flex-2",
    ChromaRadiance = "chroma-radiance",
    Wan2 = "wan2",
    Wan22I2v = "wan2.2-i2v",
    Wan22Ti2v = "wan2.2-ti2v",
    Wan22S2v = "wan2.2-s2v",
    LingbotVideo = "lingbot-video",
    QwenImage = "qwen-image",
    QwenImageLayered = "qwen-image-layered",
    QwenImage21 = "qwen-image-2.1",
    HunyuanVideo = "hunyuan-video",
    Anima = "anima",
    Flux2 = "flux2",
    Flux2Klein = "flux2-klein",
    Ltxav = "ltxav",
    MinimaxH3 = "minimax-h3",
    HidreamO1 = "hidream-o1",
    ZImage = "z-image",
    BooguImage = "boogu-image",
    OvisImage = "ovis-image",
    ErnieImage = "ernie-image",
    Lens = "lens",
    Minit2i = "minit2i",
    Longcat = "longcat",
    Pid = "pid",
    Ideogram4 = "ideogram4",
    SefiImage = "sefi-image",
    Krea2 = "krea2",
    MageFlow = "mage-flow",
    SensenovaU15 = "sensenova-u1.5",
    LladaImage = "llada-image",
    Pixart = "pixart",
    MingImage = "ming-image",
    ZImageL2p = "z-image-l2p",
    Iris = "iris",
    Flux3 = "flux3",
    MossImage = "moss-image",
    Fibo2 = "fibo2",
    Chroma = "chroma",
    PH3Image = "p-h3-image",
    MinimaxH3Image = "minimax-h3-image",
}

impl SdVersion {
    pub fn from_word(word: &str) -> Option<SdVersion> {
        SdVersion::ALL.into_iter().find(|v| v.word() == word)
    }

    /// `sd_version_is_unet` (`src/model.h:123-130`): the SD 1, SD 2 and SDXL groups
    /// (`model.h:102-121`), SDXS-512 in SD 1's and SDXS-09 in SD 2's.
    pub fn is_unet(self) -> bool {
        self.unet_group().is_some()
    }

    /// `sd_version_is_flux` (`src/model.h:139-150`).
    pub fn is_flux(self) -> bool {
        use SdVersion::*;
        matches!(
            self,
            Flux | FluxFill | FluxControls | Flex2 | OvisImage | Chroma | ChromaRadiance
        )
    }

    /// The version a LoRA made for this one is labelled with (a plan decision; the engine
    /// has no such grouping).
    pub fn lora_family(self) -> SdVersion {
        use SdVersion::*;
        if let Some(group) = self.unet_group() {
            return group;
        }
        match self {
            Flux | Flex2 | Chroma | ChromaRadiance | FluxFill | FluxControls => Flux,
            Wan22I2v => Wan2,
            ZImageL2p => ZImage,
            QwenImageLayered | QwenImage21 => QwenImage,
            other => other,
        }
    }

    /// `sd_version_is_sd1`, `_sd2` and `_sdxl` (`src/model.h:102-121`) as the group's base
    /// version.
    fn unet_group(self) -> Option<SdVersion> {
        use SdVersion::*;
        match self {
            Sd1 | Sd1Inpaint | Sd1Pix2pix | Sd1TinyUnet | Sdxs512Ds => Some(Sd1),
            Sd2 | Sd2Inpaint | Sd2TinyUnet | Sdxs09 => Some(Sd2),
            Sdxl | SdxlInpaint | SdxlPix2pix | SdxlSsd1b | SdxlVega => Some(Sdxl),
            _ => None,
        }
    }

    /// `sd_version_is_minimax_h3` (`src/model.h:170-172`).
    fn is_minimax_h3(self) -> bool {
        matches!(
            self,
            SdVersion::MinimaxH3 | SdVersion::PH3Image | SdVersion::MinimaxH3Image
        )
    }
}

/// A file given to the engine in a slot.
#[derive(Debug, Clone, Copy)]
pub struct LoadedFile<'a> {
    pub slot: SlotId,
    pub index: &'a TensorIndex,
}

/// The order the engine loads its slots in (`src/pipeline/diffusion_engine.cpp:691-836`).
/// The tokenizer is not a tensor file and never enters the catalog.
pub const LOAD_ORDER: [SlotId; 20] = [
    SlotId::Model,
    SlotId::DiffusionModel,
    SlotId::HighNoiseDiffusionModel,
    SlotId::UncondDiffusionModel,
    SlotId::ClipL,
    SlotId::ClipG,
    SlotId::ClipVision,
    SlotId::T5xxl,
    SlotId::PulidWeights,
    SlotId::Llm,
    SlotId::LlmVision,
    SlotId::Vae,
    SlotId::Tae,
    SlotId::EmbeddingsConnectors,
    SlotId::AudioVae,
    SlotId::AudioEncoder,
    SlotId::MotionModule,
    SlotId::ControlNet,
    SlotId::IpAdapter,
    SlotId::PhotoMaker,
];

/// The prefix the engine puts on a slot's tensor names (`diffusion_engine.cpp:691-836`);
/// `""` for the tokenizer, which is never loaded.
pub fn slot_prefix(slot: SlotId) -> &'static str {
    match slot {
        SlotId::Model
        | SlotId::Tokenizer
        | SlotId::AudioVae
        | SlotId::ControlNet
        | SlotId::IpAdapter => "",
        SlotId::DiffusionModel | SlotId::PulidWeights => "model.diffusion_model.",
        SlotId::HighNoiseDiffusionModel => "model.high_noise_diffusion_model.",
        SlotId::UncondDiffusionModel => "model.diffusion_model.uncond.",
        SlotId::ClipL => "clip_l.",
        SlotId::ClipG => "clip_g.",
        SlotId::ClipVision => "clip_vision.",
        SlotId::T5xxl => "text_encoders.t5xxl.transformer.",
        SlotId::Llm => "text_encoders.llm.",
        SlotId::LlmVision => "text_encoders.llm.visual.",
        SlotId::Vae => "vae.",
        SlotId::Tae => "tae.",
        SlotId::EmbeddingsConnectors => "embeddings_connectors.",
        SlotId::AudioEncoder => "wav2vec2.",
        SlotId::MotionModule => "model.diffusion_model.motion_module.",
        SlotId::PhotoMaker => "pmid.",
    }
}

/// The version the engine detects with `files` in their slots, or `None` where it says
/// `VERSION_COUNT`. The files are loaded in [`LOAD_ORDER`], whatever their order here.
pub fn detect_version(files: &[LoadedFile<'_>]) -> Option<SdVersion> {
    let mut loaded: Vec<(usize, &str, &TensorIndex)> = files
        .iter()
        .filter_map(|f| {
            let at = LOAD_ORDER.iter().position(|s| *s == f.slot)?;
            Some((at, slot_prefix(f.slot), f.index))
        })
        .collect();
    loaded.sort_by_key(|(at, _, _)| *at);
    detect(
        &loaded
            .iter()
            .map(|&(_, prefix, index)| (prefix, index))
            .collect::<Vec<_>>(),
    )
}

/// A part of a diffusers folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPart {
    /// `unet/diffusion_pytorch_model.safetensors`
    Unet,
    /// `vae/diffusion_pytorch_model.safetensors`
    Vae,
    /// `text_encoder/model.safetensors`
    TextEncoder,
    /// `text_encoder_2/model.safetensors`
    TextEncoder2,
}

impl FolderPart {
    /// The engine reads the parts in this order (`src/model_loader.cpp:421-441`).
    const ORDER: [FolderPart; 4] = [
        FolderPart::Unet,
        FolderPart::Vae,
        FolderPart::TextEncoder,
        FolderPart::TextEncoder2,
    ];

    fn prefix(self) -> &'static str {
        match self {
            FolderPart::Unet => "unet.",
            FolderPart::Vae => "vae.",
            FolderPart::TextEncoder => "te.",
            FolderPart::TextEncoder2 => "te.1.",
        }
    }
}

/// The version the engine detects for a diffusers folder given as the model
/// (`init_from_diffusers_file`, `src/model_loader.cpp:421-441`). Without its UNet the engine
/// loads nothing from the folder (`model_loader.cpp:427-429`).
pub fn detect_folder(parts: &[(FolderPart, &TensorIndex)]) -> Option<SdVersion> {
    if !parts.iter().any(|(part, _)| *part == FolderPart::Unet) {
        return None;
    }
    let mut parts = parts.to_vec();
    parts.sort_by_key(|(part, _)| FolderPart::ORDER.iter().position(|p| p == part));
    detect(
        &parts
            .iter()
            .map(|&(part, index)| (part.prefix(), index))
            .collect::<Vec<_>>(),
    )
}

/// The two passes over `files` (prefix, index), in load order.
fn detect(files: &[(&str, &TensorIndex)]) -> Option<SdVersion> {
    // Each file's tensors, in the engine's order, under the file's prefix where the name
    // does not start with it already (`src/model_loader.cpp:405-411`).
    let loaded: Vec<(String, [u64; 4])> = files
        .iter()
        .flat_map(|&(prefix, index)| {
            index.engine_order().into_iter().map(move |t| {
                let name = if t.name.starts_with(prefix) {
                    t.name.clone()
                } else {
                    format!("{prefix}{}", t.name)
                };
                (name, engine_ne(t, index.format))
            })
        })
        .collect();
    let first = signatures(&Catalog::new(loaded.iter().cloned()));
    // The second catalog is rebuilt from every file's tensors again, renamed
    // (`model_loader_files.cpp:59-71`).
    signatures(&Catalog::new(
        loaded
            .into_iter()
            .map(|(name, ne)| (convert(&name, first), ne)),
    ))
}

/// The engine's tensor catalog (`String2TensorStorage`, an `OrderedMap`): names in insertion
/// order with their `ne`. An equal name replaces the value and keeps the first position
/// (`OrderedMap::operator[]`, `src/core/ordered_map.hpp:71-80`).
struct Catalog {
    entries: Vec<(String, [u64; 4])>,
    position: HashMap<String, usize>,
}

impl Catalog {
    fn new(tensors: impl IntoIterator<Item = (String, [u64; 4])>) -> Self {
        let mut c = Catalog {
            entries: Vec::new(),
            position: HashMap::new(),
        };
        for (name, ne) in tensors {
            match c.position.get(&name) {
                Some(&at) => c.entries[at].1 = ne,
                None => {
                    c.position.insert(name.clone(), c.entries.len());
                    c.entries.push((name, ne));
                }
            }
        }
        c
    }

    /// The catalog holds exactly `name` (the engine's `count` and `find`).
    fn key(&self, name: &str) -> bool {
        self.position.contains_key(name)
    }

    fn ne(&self, name: &str) -> Option<[u64; 4]> {
        self.position.get(name).map(|&at| self.entries[at].1)
    }
}

/// `convert_tensor_name`'s prefix maps (`src/name_conversion.cpp:1741-1781`) and its one
/// rename that reaches detection, for the first pass's answer `v1` (`None` is
/// `VERSION_COUNT`).
fn convert(name: &str, v1: Option<SdVersion>) -> String {
    let is = |group: fn(SdVersion) -> bool| v1.is_some_and(group);
    // The connector map: the first match, in order (1742-1746).
    let connectors = [
        ("embeddings_connectors.text_encoders.", "text_encoders."),
        (
            "embeddings_connectors.",
            if is(SdVersion::is_minimax_h3) {
                "text_encoders.ph3_image_connector."
            } else {
                ""
            },
        ),
    ];
    let name = match connectors.iter().find(|(old, _)| name.starts_with(old)) {
        Some((old, new)) => format!("{new}{}", &name[old.len()..]),
        None => name.to_string(),
    };
    // The prefix map (1748-1779).
    let (clip_l, clip_g) = if is(SdVersion::is_unet) {
        (
            "cond_stage_model.transformer.",
            "cond_stage_model.1.transformer.",
        )
    } else {
        (
            "text_encoders.clip_l.transformer.",
            "text_encoders.clip_g.transformer.",
        )
    };
    let te1 = if is(SdVersion::is_flux) {
        "text_encoders.clip_l.transformer."
    } else {
        "cond_stage_model.transformer."
    };
    let prefixes = [
        ("diffusion_model.", "model.diffusion_model."),
        ("unet.", "model.diffusion_model."),
        ("transformer.", "model.diffusion_model."),
        ("vae.", "first_stage_model."),
        (
            "text_encoders.llm.text_embedding_projection.",
            "text_embedding_projection.",
        ),
        (
            "text_encoders.llm.text_connector.",
            "text_encoders.ph3_image_connector.",
        ),
        ("text_encoder.", "cond_stage_model.transformer."),
        ("te.", "cond_stage_model.transformer."),
        ("text_encoder.2.", "cond_stage_model.1.transformer."),
        ("conditioner.embedders.0.open_clip.", "cond_stage_model."),
        ("conditioner.embedders.0.", "cond_stage_model."),
        ("conditioner.embedders.1.", "cond_stage_model.1."),
        ("te2.", "cond_stage_model.1.transformer."),
        ("te1.", te1),
        ("te3.", "text_encoders.t5xxl.transformer."),
        ("clip_vision.", "cond_stage_model.transformer."),
        ("clip_l.", clip_l),
        ("clip_g.", clip_g),
    ];
    // The engine takes the first match of a `std::unordered_map`, whose order is
    // unspecified (1781); the plan's rule is the longest matching prefix.
    let name = match prefixes
        .iter()
        .filter(|(old, _)| name.starts_with(old))
        .max_by_key(|(old, _)| old.len())
    {
        Some((old, new)) => format!("{new}{}", &name[old.len()..]),
        None => name,
    };
    // For SD 1 and SD 2, `convert_diffusion_model_name` turns diffusers UNet names into the
    // original ones (1819-1826, 1122-1124). Of that map only `up_blocks.2.attentions.1.` →
    // `output_blocks.7.1.` (327-336, found anywhere after the prefix: 377-382) reaches a
    // detection row: row 42's SDXS-09 check, which reads the original name alone. Rows 36,
    // 40, 42 and 44 list both forms; row 41 counts only under `is_xl`.
    const DIT: &str = "model.diffusion_model.";
    const DIFFUSERS: &str = "up_blocks.2.attentions.1.";
    if matches!(
        v1.and_then(SdVersion::unet_group),
        Some(SdVersion::Sd1 | SdVersion::Sd2)
    ) && let Some(rest) = name.strip_prefix(DIT)
        && let Some(at) = rest.find(DIFFUSERS)
    {
        return format!(
            "{DIT}{}output_blocks.7.1.{}",
            &rest[..at],
            &rest[at + DIFFUSERS.len()..]
        );
    }
    name
}

/// Token embeddings (row 43, `src/model_loader.cpp:659-664`).
const TOKEN_EMBEDDINGS: [&str; 6] = [
    "cond_stage_model.transformer.text_model.embeddings.token_embedding.weight",
    "cond_stage_model.model.token_embedding.weight",
    "text_model.embeddings.token_embedding.weight",
    "te.text_model.embeddings.token_embedding.weight",
    "conditioner.embedders.0.model.token_embedding.weight",
    "conditioner.embedders.0.transformer.text_model.embeddings.token_embedding.weight",
];

/// Input layers (row 44, `model_loader.cpp:668-671`).
const INPUTS: [&str; 4] = [
    "model.diffusion_model.input_blocks.0.0.weight",
    "model.diffusion_model.conv_in.weight",
    "model.diffusion_model.img_in.weight",
    "unet.conv_in.weight",
];

/// `ModelLoader::get_sd_version` (`src/model_loader.cpp:443-763`), row by row as the plan's
/// table numbers them. "Has" is a substring of the name; a key is an exact catalog name.
/// `logical_ne0()` is `ne[0]`: the engine doubles it only for its own w4 "convrot" weights,
/// which a header read cannot tell apart and no published file uses.
fn signatures(c: &Catalog) -> Option<SdVersion> {
    use SdVersion::*;
    // Row 1 (446-448).
    if c.key("model.diffusion_model.perceiver.latent_queries") {
        return Some(Fibo2);
    }
    // Row 2 (450-453).
    if c.key("model.diffusion_model.emb_in.image.weight")
        && c.key("model.diffusion_model.txt_mode_blocks.0.q_proj.weight")
    {
        return Some(Flux3);
    }
    // Row 3 (455-458).
    if c.key("model.diffusion_model.video_embedder.weight")
        && c.key("model.diffusion_model.shared_block_modulation.weight")
    {
        return Some(MossImage);
    }

    // 444, 460-476. A tensor the loop never takes keeps a default `TensorStorage`'s `ne`,
    // all ones (`src/model_io/tensor_storage.h:28`).
    let mut has_multiple_encoders = false;
    let mut is_unet = false;
    let mut is_xl = false;
    let mut is_flux = false;
    let mut is_flux2 = false;
    let mut has_single_block_47 = false;
    let mut is_wan = false;
    let mut is_s2v = false;
    let mut patch_channels = 0u64;
    let mut has_img_emb = false;
    let mut has_middle_block_1 = false;
    let mut has_output_block_311 = false;
    let mut has_output_block_71 = false;
    let mut has_attn_1024 = false;
    let mut has_distilled_guidance = false;
    let mut has_sdxl_add_embedding = false;
    let mut token = [1u64; 4];
    let mut input = [1u64; 4];
    let mut context = [1u64; 4];

    for (name, ne) in &c.entries {
        let has = |s: &str| name.contains(s);
        // Row 4 (479-482).
        if has("model.diffusion_model.double_blocks.")
            || has("model.diffusion_model.single_transformer_blocks.")
        {
            is_flux = true;
        }
        // Row 5 (483-485).
        if has("model.diffusion_model.net.lq_proj.latent_proj.0.weight") {
            return Some(Pid);
        }
        // Row 6 (486-488).
        if has("embed_image_indicator.weight") {
            return Some(Ideogram4);
        }
        // Row 7 (489-492).
        if has("model.diffusion_model.txtfusion.projector.weight")
            || has("model.diffusion_model.text_fusion.projector.weight")
        {
            return Some(Krea2);
        }
        // Row 8 (493-495).
        if has("model.diffusion_model.nerf_final_layer_conv.") {
            return Some(ChromaRadiance);
        }
        // Row 9 (496-498).
        if has("distilled_guidance_layer.in_proj.weight") {
            has_distilled_guidance = true;
        }
        // Row 10 (499-501).
        if has("model.diffusion_model.joint_blocks.") {
            return Some(Sd3);
        }
        // Row 11 (502-505).
        if has("model.x_embedder.proj1.weight")
            && c.key("model.language_model.layers.0.self_attn.q_proj.weight")
        {
            return Some(HidreamO1);
        }
        // Row 12 (506-509).
        if has("model.diffusion_model.transformer_blocks.0.attn.norm_added_q.weight")
            && c.key("model.diffusion_model.transformer_blocks.0.img_mlp.w1.weight")
        {
            return Some(Lens);
        }
        // Row 13 (510-512).
        if has("net.img_embedder.proj1.weight") {
            return Some(Minit2i);
        }
        // Row 14 (513-515).
        if has("language_model.model.layers.0.self_attn.q_proj_mot_gen.weight") {
            return Some(SensenovaU15);
        }
        // Row 15 (516-518).
        if name == "model.diffusion_model.txt_in.text_norm.weight" {
            return Some(QwenImage21);
        }
        // Row 16 (519-528).
        if has("model.diffusion_model.transformer_blocks.0.img_mod.1.weight") {
            if c.ne("model.diffusion_model.img_in.weight")
                .is_some_and(|img_in| img_in[0] == 128)
            {
                return Some(MageFlow);
            }
            if c.key("model.diffusion_model.time_text_embed.addition_t_embedding.weight") {
                return Some(QwenImageLayered);
            }
            return Some(QwenImage);
        }
        // Row 17 (529-531).
        if has(
            "model.diffusion_model.txt_in.individual_token_refiner.blocks.0.adaLN_modulation.1.weight",
        ) {
            return Some(HunyuanVideo);
        }
        // Row 18 (532-534).
        if has("llm_adapter.blocks.0.cross_attn.q_proj.weight") {
            return Some(Anima);
        }
        // Row 19 (535-537).
        if has("model.diffusion_model.double_stream_modulation_img.lin.weight") {
            is_flux2 = true;
        }
        // Row 20 (538-540).
        if has("dual_time_embed.semantic_embedder.linear_1.weight") {
            return Some(SefiImage);
        }
        // Row 21 (541-543).
        if has("single_blocks.47.linear1.weight") {
            has_single_block_47 = true;
        }
        // Row 22 (544-546).
        if has("model.diffusion_model.double_blocks.0.img_mlp.gate_proj.weight") {
            return Some(OvisImage);
        }
        // Row 23 (547-549).
        if has("model.diffusion_model.sigvq_embedder.1.weight") {
            return Some(LladaImage);
        }
        // Row 24 (550-558).
        if has("model.diffusion_model.cap_embedder.0.weight") {
            if c.key("text_encoders.llm.connector.layers.0.self_attn.q_proj.weight") {
                return Some(MingImage);
            }
            if c.key("model.diffusion_model.local_decoder.out_conv.weight") {
                return Some(ZImageL2p);
            }
            return Some(ZImage);
        }
        // Row 25 (559-561).
        if has("double_stream_layers.0.img_instruct_attn.processor.img_to_q.weight") {
            return Some(BooguImage);
        }
        // Row 26 (562-564).
        if has("model.diffusion_model.layers.0.adaLN_sa_ln.weight") {
            return Some(ErnieImage);
        }
        // Row 27 (565-569).
        if has("model.diffusion_model.t_block.1.weight")
            && c.key("model.diffusion_model.x_embedder.proj.weight")
            && !c.key("model.diffusion_model.audio_patchify_proj.weight")
        {
            return Some(Pixart);
        }
        // Row 28 (570-578).
        if has("model.diffusion_model.adaln_single.emb.timestep_embedder.linear_1.bias") {
            if c.key("model.diffusion_model.pos_embed.proj.weight")
                && c.key("model.diffusion_model.adaln_single.linear.weight")
                && !c.key("model.diffusion_model.audio_patchify_proj.weight")
            {
                return Some(Pixart);
            }
            return Some(Ltxav);
        }
        // Row 29 (579-593).
        if has("model.diffusion_model.video_patch_proj.weight")
            && c.key("model.diffusion_model.condition_proj.weight")
        {
            if !c.key("model.diffusion_model.audio_patch_proj.weight") {
                return Some(MinimaxH3Image);
            }
            let decoder = c
                .ne("first_stage_model.decoder.proj_out.weight")
                .or_else(|| c.ne("vae.decoder.proj_out.weight"));
            if decoder.is_some_and(|decoder| decoder[1] == 768) {
                return Some(PH3Image);
            }
            return Some(MinimaxH3);
        }
        // Row 30 (594-597).
        if name == "model.diffusion_model.y_embedder.layer_pool.weight"
            && c.key("model.diffusion_model.modulation_cores.adaln_img.weight")
        {
            return Some(Iris);
        }
        // Row 31 (598-600).
        if has("model.diffusion_model.blocks.0.cross_attn.norm_k.weight") {
            is_wan = true;
        }
        // Row 32 (601-605).
        if has("casual_audio_encoder.weights") || has("audio_injector.injector.0.q.weight") {
            is_s2v = true;
        }
        // Row 33 (606-608).
        if has("model.diffusion_model.patch_embedder.weight") {
            return Some(LingbotVideo);
        }
        // Row 34 (609-611).
        if has("model.diffusion_model.patch_embedding.weight") {
            patch_channels = ne[3];
        }
        // Row 35 (612-614).
        if has("model.diffusion_model.img_emb") {
            has_img_emb = true;
        }
        // Row 36 (615-622).
        if has("model.diffusion_model.input_blocks.")
            || has("model.diffusion_model.down_blocks.")
            || has("unet.down_blocks.")
        {
            is_unet = true;
            if has_multiple_encoders {
                is_xl = true;
            }
        }
        // Row 37 (623-630).
        if has("conditioner.embedders.1") || has("cond_stage_model.1") || has("te.1") {
            has_multiple_encoders = true;
            if is_unet {
                is_xl = true;
            }
        }
        // Row 38 (633-636).
        if has("model.diffusion_model.add_embedding.")
            || has("model.diffusion_model.label_emb.0.0.")
        {
            has_sdxl_add_embedding = true;
        }
        // Row 39 (637-639).
        if has("model.diffusion_model.input_blocks.8.0.time_mixer.mix_factor") {
            return Some(Svd);
        }
        // Row 40 (640-644).
        if has("model.diffusion_model.middle_block.1.")
            || has("model.diffusion_model.mid_block.resnets.1.")
            || has("unet.mid_block.resnets.1.")
        {
            has_middle_block_1 = true;
        }
        // Row 41 (645-649).
        if has("model.diffusion_model.output_blocks.3.1.transformer_blocks.1")
            || has("model.diffusion_model.up_blocks.1.attentions.0.transformer_blocks.1")
            || has("unet.up_blocks.1.attentions.0.transformer_blocks.1")
        {
            has_output_block_311 = true;
        }
        // Row 42 (650-658).
        if has("model.diffusion_model.output_blocks.7.1")
            || has("model.diffusion_model.up_blocks.2.attentions.1")
            || has("unet.up_blocks.2.attentions.1")
        {
            has_output_block_71 = true;
            if has("model.diffusion_model.output_blocks.7.1.transformer_blocks.0.attn1.to_k.weight")
                && ne[0] == 1024
            {
                has_attn_1024 = true;
            }
        }
        // Row 43 (659-667).
        if TOKEN_EMBEDDINGS.contains(&name.as_str()) {
            token = *ne;
        }
        // Row 44 (668-673).
        if INPUTS.contains(&name.as_str()) {
            input = *ne;
        }
        // Row 45 (674-676).
        if name == "model.diffusion_model.txt_in.weight"
            || name == "model.diffusion_model.context_embedder.weight"
        {
            context = *ne;
        }
    }

    // Row 46 (678-690).
    if is_wan {
        if is_s2v {
            return Some(Wan22S2v);
        }
        if patch_channels == 184_320 && !has_img_emb {
            return Some(Wan22I2v);
        }
        if patch_channels == 147_456 && !has_img_emb {
            return Some(Wan22Ti2v);
        }
        return Some(Wan2);
    }
    // Row 47 (691-695).
    let inpaint = input[2] == 9;
    let ip2p = input[2] == 8;
    if is_unet && has_sdxl_add_embedding {
        is_xl = true;
    }
    // Row 48 (696-710).
    if is_xl {
        if inpaint {
            return Some(SdxlInpaint);
        }
        if ip2p {
            return Some(SdxlPix2pix);
        }
        if !has_middle_block_1 {
            if !has_output_block_311 {
                return Some(SdxlVega);
            }
            return Some(SdxlSsd1b);
        }
        return Some(Sdxl);
    }
    // Row 49 (712-730).
    if is_flux && !is_flux2 {
        if has_distilled_guidance {
            return Some(Chroma);
        }
        if context[0] == 3584 {
            return Some(Longcat);
        }
        if input[0] == 384 {
            return Some(FluxFill);
        }
        if input[0] == 128 {
            return Some(FluxControls);
        }
        if input[0] == 196 {
            return Some(Flex2);
        }
        return Some(Flux);
    }
    // Row 50 (732-737).
    if is_flux2 {
        if has_single_block_47 {
            return Some(Flux2);
        }
        return Some(Flux2Klein);
    }
    // Row 51 (739-752).
    if token[0] == 768 {
        if inpaint {
            return Some(Sd1Inpaint);
        }
        if ip2p {
            return Some(Sd1Pix2pix);
        }
        if !has_middle_block_1 {
            if !has_output_block_71 {
                return Some(Sdxs512Ds);
            }
            return Some(Sd1TinyUnet);
        }
        return Some(Sd1);
    }
    // Row 52 (753-761).
    if token[0] == 1024 {
        if inpaint {
            return Some(Sd2Inpaint);
        }
        if !has_middle_block_1 {
            return Some(if has_attn_1024 { Sdxs09 } else { Sd2TinyUnet });
        }
        return Some(Sd2);
    }
    // Row 53 (762): VERSION_COUNT.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sd::index::{FileFormat, TensorInfo};
    use crate::sd::testing::{fixture, gguf_txt_fixture};

    fn as_diffusion(name: &str) -> Option<SdVersion> {
        let ix = fixture(name);
        detect_version(&[LoadedFile {
            slot: SlotId::DiffusionModel,
            index: &ix,
        }])
    }

    fn as_model(name: &str) -> Option<SdVersion> {
        let ix = fixture(name);
        detect_version(&[LoadedFile {
            slot: SlotId::Model,
            index: &ix,
        }])
    }

    /// A GGUF index of `names` with the given `ne`, a leading `D.` read as
    /// `model.diffusion_model.`.
    fn names_index(names: &[(&str, [u64; 4])]) -> TensorIndex {
        TensorIndex {
            format: FileFormat::Gguf,
            tensors: names
                .iter()
                .map(|(name, ne)| TensorInfo {
                    name: match name.strip_prefix("D.") {
                        Some(rest) => format!("model.diffusion_model.{rest}"),
                        None => name.to_string(),
                    },
                    dtype: "f16".into(),
                    shape: ne.to_vec(),
                })
                .collect(),
            metadata: Default::default(),
            parameters_hint: None,
        }
    }

    fn detect_names(names: &[(&str, [u64; 4])]) -> Option<SdVersion> {
        detect_version(&[LoadedFile {
            slot: SlotId::DiffusionModel,
            index: &names_index(names),
        }])
    }

    fn detect_names_as_model(names: &[(&str, [u64; 4])]) -> Option<SdVersion> {
        detect_version(&[LoadedFile {
            slot: SlotId::Model,
            index: &names_index(names),
        }])
    }

    #[test]
    fn each_fixture_is_detected_as_the_engine_detects_it() {
        use SdVersion::*;
        for (name, v) in [("sd15", Sd1), ("sdxl", Sdxl), ("sd35-medium", Sd3)] {
            assert_eq!(as_model(name), Some(v), "{name}");
        }
        // Black Forest Labs' bare-name transformer: row 4 needs `model.diffusion_model.`
        // names, which only the diffusion slot's prefix gives it.
        assert_eq!(as_diffusion("flux1-dev"), Some(Flux));
        assert_eq!(as_model("flux1-dev"), None);
        for (name, v) in [
            ("flux1-kontext", Flux),
            ("chroma", Chroma),
            ("qwen-image", QwenImage),
            ("qwen-image-edit-2509", QwenImage),
            ("z-image-turbo", ZImage),
            ("flux2-klein-4b", Flux2Klein),
            ("wan21-t2v-1.3b", Wan2),
            ("wan21-i2v-14b", Wan2),
            ("wan22-ti2v-5b", Wan22Ti2v),
            ("wan22-t2v-a14b-high", Wan2),
            ("wan22-t2v-a14b-low", Wan2),
            ("hunyuan-video-15", HunyuanVideo),
            ("ltx23", Ltxav),
            // Lumina 2 is Z-Image to the engine (Task R6 refuses it).
            ("lumina2", ZImage),
        ] {
            assert_eq!(as_diffusion(name), Some(v), "{name}");
        }
    }

    #[test]
    fn the_01x_family_files_detect_as_their_recipes_versions() {
        use SdVersion::*;
        for (txt, v) in [
            ("klein4b", Flux2Klein),
            ("klein9b", Flux2Klein),
            ("zimage", ZImage),
            ("wan22-ti2v-5b", Wan22Ti2v),
            ("wan21-t2v-1.3b", Wan2),
            ("flux3-klein", Flux3),
        ] {
            assert_eq!(
                detect_version(&[LoadedFile {
                    slot: SlotId::DiffusionModel,
                    index: &gguf_txt_fixture(txt)
                }]),
                Some(v),
                "{txt}"
            );
        }
    }

    #[test]
    fn companions_and_loras_answer_as_the_engine_would() {
        // The engine's own answers, row by row; Task R6 keeps these files out of "Works in
        // Studio" by its LoRA check and layout rule.
        for name in ["ae-flux1", "clip-l", "umt5-xxl-q5km", "sdxl-lora-pixel-art"] {
            assert_eq!(as_diffusion(name), None, "{name}");
        }
        for name in ["ae-flux1", "umt5-xxl-q5km", "sdxl-lora-pixel-art"] {
            assert_eq!(as_model(name), None, "{name}");
        }
        // clip_l in the model slot: row 43 takes text_model.embeddings.token_embedding.weight
        // (ne[0] 768) and row 51, with no UNet guard, says sdxs-512-ds.
        assert_eq!(as_model("clip-l"), Some(SdVersion::Sdxs512Ds));
        // XLabs' "diffusion_model.double_blocks.…" names: row 4 sees them in the diffusion
        // slot, and pass 2's prefix map in the model slot.
        assert_eq!(
            (
                as_diffusion("flux-lora-realism"),
                as_model("flux-lora-realism")
            ),
            (Some(SdVersion::Flux), Some(SdVersion::Flux))
        );
    }

    #[test]
    fn a_diffusers_folder_is_detected_from_its_parts() {
        assert_eq!(
            detect_folder(&[
                (FolderPart::Unet, &fixture("sd14-diffusers-unet")),
                (
                    FolderPart::TextEncoder,
                    &fixture("sd14-diffusers-text-encoder")
                )
            ]),
            Some(SdVersion::Sd1)
        );
    }

    #[test]
    fn the_row_order_decides_like_the_engine() {
        // A FLUX file that also carries the Chroma Radiance marker returns chroma-radiance at
        // row 8, before row 49.
        assert_eq!(
            detect_names(&[
                ("D.double_blocks.0.img_attn.qkv.weight", [3072, 9216, 1, 1]),
                ("D.nerf_final_layer_conv.weight", [1, 1, 1, 1])
            ]),
            Some(SdVersion::ChromaRadiance)
        );
        // The Wan TI2V check reads ne[3] after folding (147456 = 48 × 3072).
        assert_eq!(
            detect_names(&[
                ("D.blocks.0.cross_attn.norm_k.weight", [3072, 1, 1, 1]),
                ("D.patch_embedding.weight", [2, 2, 1, 147456])
            ]),
            Some(SdVersion::Wan22Ti2v)
        );
        // Pass 2: a bare "diffusion_model." checkpoint becomes "model.diffusion_model.".
        assert_eq!(
            detect_names_as_model(&[(
                "diffusion_model.joint_blocks.0.x_block.attn.qkv.weight",
                [1536, 4608, 1, 1]
            )]),
            Some(SdVersion::Sd3)
        );
    }

    #[test]
    fn every_version_has_a_word_that_reads_back() {
        assert_eq!(SdVersion::ALL.len(), 60);
        for v in SdVersion::ALL {
            assert_eq!(SdVersion::from_word(v.word()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.word());
        }
        assert_eq!(SdVersion::Wan22Ti2v.word(), "wan2.2-ti2v");
        assert_eq!(SdVersion::SensenovaU15.word(), "sensenova-u1.5");
    }

    // Beyond the brief: the parts of the port its tests above do not reach.

    #[test]
    fn files_load_in_the_engines_order_whatever_the_caller_lists() {
        // The diffusion model loads before the LLM, so its SD3 marker (row 10) is visited
        // before the LLM's Ideogram 4 marker (row 6) would be.
        let llm = names_index(&[("embed_image_indicator.weight", [3072, 1, 1, 1])]);
        let dit = names_index(&[(
            "D.joint_blocks.0.x_block.attn.qkv.weight",
            [1536, 4608, 1, 1],
        )]);
        let files = [
            LoadedFile {
                slot: SlotId::Llm,
                index: &llm,
            },
            LoadedFile {
                slot: SlotId::DiffusionModel,
                index: &dit,
            },
        ];
        assert_eq!(detect_version(&files), Some(SdVersion::Sd3));
        assert_eq!(detect_version(&files[..1]), Some(SdVersion::Ideogram4));
    }

    #[test]
    fn an_equal_name_takes_the_later_shape_in_the_first_position() {
        let c = Catalog::new([
            ("a".to_string(), [1, 1, 1, 1]),
            ("b".to_string(), [2, 1, 1, 1]),
            ("a".to_string(), [3, 1, 1, 1]),
        ]);
        assert_eq!(
            c.entries,
            [
                ("a".to_string(), [3, 1, 1, 1]),
                ("b".to_string(), [2, 1, 1, 1])
            ]
        );
        // A FLUX checkpoint whose input layer a later diffusion-model file replaces.
        let model = names_index(&[
            ("D.double_blocks.0.img_attn.qkv.weight", [3072, 9216, 1, 1]),
            ("D.img_in.weight", [64, 3072, 1, 1]),
        ]);
        let fill = names_index(&[("img_in.weight", [384, 3072, 1, 1])]);
        assert_eq!(
            detect_version(&[
                LoadedFile {
                    slot: SlotId::Model,
                    index: &model,
                },
                LoadedFile {
                    slot: SlotId::DiffusionModel,
                    index: &fill,
                },
            ]),
            Some(SdVersion::FluxFill)
        );
    }

    #[test]
    fn the_prefix_maps_take_the_longest_match_and_follow_the_first_answer() {
        use SdVersion::*;
        for (name, v1, want) in [
            ("text_encoder.2.a", None, "cond_stage_model.1.transformer.a"),
            ("text_encoder.a", None, "cond_stage_model.transformer.a"),
            (
                "conditioner.embedders.0.open_clip.a",
                None,
                "cond_stage_model.a",
            ),
            ("conditioner.embedders.0.a", None, "cond_stage_model.a"),
            ("te1.a", Some(Flux), "text_encoders.clip_l.transformer.a"),
            ("te1.a", Some(Sd3), "cond_stage_model.transformer.a"),
            (
                "clip_l.a",
                Some(Sdxs512Ds),
                "cond_stage_model.transformer.a",
            ),
            ("clip_g.a", Some(Sdxl), "cond_stage_model.1.transformer.a"),
            ("clip_l.a", None, "text_encoders.clip_l.transformer.a"),
            (
                "embeddings_connectors.text_encoders.llm.a",
                None,
                "text_encoders.llm.a",
            ),
            (
                "embeddings_connectors.a",
                Some(PH3Image),
                "text_encoders.ph3_image_connector.a",
            ),
            ("embeddings_connectors.a", Some(Ltxav), "a"),
            // Both maps apply, the connector map first.
            ("embeddings_connectors.vae.a", None, "first_stage_model.a"),
            (
                "model.diffusion_model.a",
                Some(Flux),
                "model.diffusion_model.a",
            ),
        ] {
            assert_eq!(convert(name, v1), want, "{name} {v1:?}");
        }
    }

    #[test]
    fn the_version_groups_follow_the_engine() {
        use SdVersion::*;
        assert_eq!(SdVersion::ALL.iter().filter(|v| v.is_unet()).count(), 14);
        assert!(Sdxs512Ds.is_unet() && Sdxs09.is_unet() && !Svd.is_unet() && !Sd3.is_unet());
        assert_eq!(SdVersion::ALL.iter().filter(|v| v.is_flux()).count(), 7);
        assert!(OvisImage.is_flux() && !Flux2.is_flux() && !Longcat.is_flux());
        for (v, family) in [
            (Sdxs512Ds, Sd1),
            (Sd1Pix2pix, Sd1),
            (Sdxs09, Sd2),
            (SdxlSsd1b, Sdxl),
            (ChromaRadiance, Flux),
            (FluxControls, Flux),
            (OvisImage, OvisImage),
            (Wan22I2v, Wan2),
            (Wan22Ti2v, Wan22Ti2v),
            (ZImageL2p, ZImage),
            (QwenImage21, QwenImage),
            (QwenImageLayered, QwenImage),
            (Flux2Klein, Flux2Klein),
            (Svd, Svd),
        ] {
            assert_eq!(v.lora_family(), family, "{v:?}");
        }
    }

    #[test]
    fn the_load_order_is_every_slot_but_the_tokenizer() {
        let slots: std::collections::BTreeSet<SlotId> = LOAD_ORDER.into_iter().collect();
        assert_eq!(slots.len(), 20);
        assert!(!slots.contains(&SlotId::Tokenizer));
        assert_eq!(slot_prefix(SlotId::PulidWeights), "model.diffusion_model.");
    }

    #[test]
    fn a_folder_without_its_unet_loads_nothing() {
        assert_eq!(
            detect_folder(&[(
                FolderPart::TextEncoder,
                &fixture("sd14-diffusers-text-encoder")
            )]),
            None
        );
    }

    /// A safetensors index of `names`, shapes as written (outermost first).
    fn safetensors_index(names: &[(&str, &[u64])]) -> TensorIndex {
        TensorIndex {
            format: FileFormat::Safetensors,
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

    #[test]
    fn a_diffusers_sdxs_09_folder_is_sdxs_09_after_the_sd2_rename() {
        // IDKiro/sdxs-512-0.9: no mid block, a 1024-wide attn1.to_k in up_blocks.2.attentions.1
        // and SD 2's 1024-wide text encoder. Pass 1 says sd2-tiny-unet; pass 2's rename to
        // output_blocks.7.1 lets row 42 see the 1024.
        let te = safetensors_index(&[(
            "text_model.embeddings.token_embedding.weight",
            &[49408, 1024],
        )]);
        let folder = |to_k: &[u64]| {
            let unet = safetensors_index(&[
                ("down_blocks.0.resnets.0.conv1.weight", &[320, 320, 3, 3]),
                (
                    "up_blocks.2.attentions.1.transformer_blocks.0.attn1.to_k.weight",
                    to_k,
                ),
            ]);
            detect_folder(&[(FolderPart::Unet, &unet), (FolderPart::TextEncoder, &te)])
        };
        assert_eq!(folder(&[320, 1024]), Some(SdVersion::Sdxs09));
        // bk-sdm-v2-tiny and -small: SD 2's 640-wide attention there, still the tiny UNet.
        assert_eq!(folder(&[640, 640]), Some(SdVersion::Sd2TinyUnet));
    }

    #[test]
    fn the_sd2_rename_leaves_the_other_small_unets_as_they_were() {
        let te = |width: u64| {
            safetensors_index(&[(
                "text_model.embeddings.token_embedding.weight",
                &[49408, width],
            )])
        };
        let unet = |names: &[&str]| {
            let mut tensors: Vec<(&str, &[u64])> =
                vec![("down_blocks.0.resnets.0.conv1.weight", &[320, 320, 3, 3])];
            tensors.extend(names.iter().map(|n| (*n, &[320u64, 1024][..])));
            safetensors_index(&tensors)
        };
        // tiny-sd and sdxs-512-dreamshaper (SD 1, no mid block): row 51 never reads the
        // 1024 check.
        let up_2_1 = "up_blocks.2.attentions.1.transformer_blocks.0.attn1.to_k.weight";
        let (te1, sd1_tiny, sd1_ds) = (te(768), unet(&[up_2_1]), unet(&[]));
        for (u, want) in [
            (&sd1_tiny, SdVersion::Sd1TinyUnet),
            (&sd1_ds, SdVersion::Sdxs512Ds),
        ] {
            assert_eq!(
                detect_folder(&[(FolderPart::Unet, u), (FolderPart::TextEncoder, &te1)]),
                Some(want)
            );
        }
        // SSD-1B and Segmind-Vega (SDXL, no mid block): not in the SD 1 or SD 2 group.
        let add = "add_embedding.linear_1.weight";
        let up_1_0_1 = "up_blocks.1.attentions.0.transformer_blocks.1.attn1.to_k.weight";
        let (ssd1b, vega, te2) = (
            unet(&[add, up_1_0_1, up_2_1]),
            unet(&[add, up_2_1]),
            te(1280),
        );
        for (u, want) in [(&ssd1b, SdVersion::SdxlSsd1b), (&vega, SdVersion::SdxlVega)] {
            assert_eq!(
                detect_folder(&[
                    (FolderPart::Unet, u),
                    (FolderPart::TextEncoder, &te1),
                    (FolderPart::TextEncoder2, &te2),
                ]),
                Some(want)
            );
        }
    }
}
