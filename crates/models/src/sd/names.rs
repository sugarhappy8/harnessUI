//! What a repo's file names say (design section 7.4): the companion slot a file's name or
//! folder names, whether it looks like a LoRA, and a diffusers folder's files. The set-up
//! planner (Task S4) and the repo labels (Task S5) read them.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::files::RepoFile;
use crate::sd::index::READ_EXTENSIONS;
use crate::sd::matching::{normalise, tokens};
use crate::sd::recipes::SlotId;

/// A file is a VAE by name only below this size: the largest VAE, Wan 2.2's, is 1.4 GB.
pub const VAE_BY_NAME_MAX_BYTES: u64 = 2_000_000_000;

/// Model tokens: a file name with one of these is never a companion by name.
const MODEL_TOKENS: [&str; 12] = [
    "image",
    "edit",
    "diffusion",
    "unet",
    "transformer",
    "dit",
    "checkpoint",
    "pruned",
    "ema",
    "baked",
    "merged",
    "with",
];

/// One row of the names table: whole tokens of the file name, or substrings of its
/// normalised form ([`normalise`]).
struct NameRow {
    slot: SlotId,
    tokens: &'static [&'static str],
    substrings: &'static [&'static str],
}

/// The names table, first match. `audio-vae` comes before `vae`, whose token
/// `audio_vae.safetensors` also has; the rows naming one model's encoder come before the
/// language models', whose `text-encoder` any encoder's name may hold.
const NAME_ROWS: [NameRow; 11] = [
    NameRow {
        slot: SlotId::AudioVae,
        tokens: &[],
        substrings: &["audio-vae"],
    },
    NameRow {
        slot: SlotId::Vae,
        tokens: &["vae", "ae"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::Tae,
        tokens: &["taesd", "tae"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::T5xxl,
        tokens: &["umt5", "t5xxl", "t5", "byt5"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::ClipL,
        tokens: &[],
        substrings: &["clip-l"],
    },
    NameRow {
        slot: SlotId::ClipG,
        tokens: &[],
        substrings: &["clip-g"],
    },
    NameRow {
        slot: SlotId::ClipVision,
        tokens: &[],
        substrings: &["clip-vision"],
    },
    NameRow {
        slot: SlotId::LlmVision,
        tokens: &["mmproj"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::EmbeddingsConnectors,
        tokens: &["connector", "connectors"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::AudioEncoder,
        tokens: &["wav2vec2"],
        substrings: &[],
    },
    NameRow {
        slot: SlotId::Llm,
        tokens: &["gemma", "mistral", "llama"],
        substrings: &[
            "qwen2.5-vl",
            "qwen-2.5-vl",
            "qwen3-",
            "qwen3vl",
            "qwen-3-",
            "text-encoder",
            "text-encoders",
        ],
    },
];

/// Folders that say what a file is, after the names: Comfy-Org's `split_files/vae/` and
/// `split_files/text_encoders/`, a diffusers folder's `vae/`.
const FOLDER_ROWS: [(&str, SlotId); 2] = [("vae", SlotId::Vae), ("text-encoders", SlotId::Llm)];

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

/// A weights file Studio reads: `.safetensors`, `.sft` or `.gguf`, in any case.
pub(crate) fn is_weights(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    READ_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
}

static SHARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)-\d{5}-of-\d{5}\.[a-z0-9]+$").expect("the shard pattern compiles")
});

/// One shard of a split file (`model-00001-of-00002.safetensors`), which is never a set-up's
/// file on its own.
pub(crate) fn is_shard(path: &str) -> bool {
    SHARD.is_match(file_name(path))
}

/// The slot a file is a companion for, by its name (the table of design section 7.4):
/// whole tokens and substrings of the file name, then its folders (`vae/`,
/// `text_encoders/`). A file name with a model token (`image`, `edit`, `diffusion`, …) is
/// never a companion by name, though its folder can still say so
/// (`split_files/vae/qwen_image_vae.safetensors`).
///
/// Only weights files Studio reads are companions, and `tokenizer*.json` files. The `vae`
/// row holds only below [`VAE_BY_NAME_MAX_BYTES`], which a path cannot tell:
/// [`companion_of`] checks it.
pub fn companion_by_name(path: &str) -> Option<SlotId> {
    let name = file_name(path);
    let lower = name.to_lowercase();
    if lower.starts_with("tokenizer") && lower.ends_with(".json") {
        return Some(SlotId::Tokenizer);
    }
    if !is_weights(name) {
        return None;
    }
    let words = tokens(name);
    if !words.iter().any(|w| MODEL_TOKENS.contains(&w.as_str())) {
        let text = normalise(name);
        let by_name = NAME_ROWS.iter().find(|row| {
            row.tokens.iter().any(|t| words.iter().any(|w| w == t))
                || row.substrings.iter().any(|s| text.contains(s))
        });
        if let Some(row) = by_name {
            return Some(row.slot);
        }
    }
    let folders: BTreeSet<String> = path.split('/').rev().skip(1).map(normalise).collect();
    FOLDER_ROWS
        .iter()
        .find(|(folder, _)| folders.contains(*folder))
        .map(|(_, slot)| *slot)
}

/// [`companion_by_name`] with the size rule: a VAE by name is under 2 GB.
pub fn companion_of(file: &RepoFile) -> Option<SlotId> {
    companion_by_name(&file.path)
        .filter(|slot| *slot != SlotId::Vae || file.size < VAE_BY_NAME_MAX_BYTES)
}

/// A `lora` token anywhere in the path.
pub fn looks_like_lora(path: &str) -> bool {
    tokens(path).iter().any(|w| w == "lora")
}

/// A file of a diffusers folder, with the engine's name for it inside the folder.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderFile {
    pub file: RepoFile,
    /// The engine's name in the folder (`unet/diffusion_pytorch_model.safetensors`, …),
    /// which is the only name it opens (`src/model_loader.cpp:423-426`).
    pub save_as: &'static str,
}

/// An SD 1.x, 2.x or XL diffusers folder in a repo: the files the engine reads from it.
#[derive(Debug, Clone, PartialEq)]
pub struct DiffusersFolder {
    /// The folder's path in the repo, ending in `/`, or `""` at the top.
    pub root: String,
    pub unet: FolderFile,
    pub vae: Option<FolderFile>,
    pub text_encoder: Option<FolderFile>,
    pub text_encoder_2: Option<FolderFile>,
    /// Whether any chosen file is an fp16 variant.
    pub fp16: bool,
}

/// The four files the engine reads from a diffusers folder, by its names
/// (`init_from_diffusers_file`, `src/model_loader.cpp:421-441`), each with its fp16
/// variant's name; in [`crate::sd::inspect::FOLDER_FILES`]'s order.
const FOLDER_NAMES: [(&str, &str); 4] = [
    (
        "unet/diffusion_pytorch_model.safetensors",
        "unet/diffusion_pytorch_model.fp16.safetensors",
    ),
    (
        "vae/diffusion_pytorch_model.safetensors",
        "vae/diffusion_pytorch_model.fp16.safetensors",
    ),
    (
        "text_encoder/model.safetensors",
        "text_encoder/model.fp16.safetensors",
    ),
    (
        "text_encoder_2/model.safetensors",
        "text_encoder_2/model.fp16.safetensors",
    ),
];

/// The diffusers folder in a repo's listing, if any: a `unet/diffusion_pytorch_model`
/// (`.safetensors` or `.fp16.safetensors`) at the top or one folder down, which the engine
/// takes as the sign of one (`src/model_loader.cpp:207-210`); `model_index.json` is not
/// needed. The top wins over a subfolder, then the first subfolder by name.
///
/// Where a part has both files and the plain one is fp32, the fp16 one is chosen: the plain
/// file is read as fp32 when it is at least 1.8 times the fp16 one, a plan decision that
/// spends no header read. `None` when a chosen file has no LFS SHA-256, as it cannot be
/// pinned.
pub fn diffusers_folder(files: &[RepoFile]) -> Option<DiffusersFolder> {
    let (unet_name, unet_fp16) = FOLDER_NAMES[0];
    let roots: BTreeSet<&str> = files
        .iter()
        .filter_map(|f| {
            let path = f.path.as_str();
            path.strip_suffix(unet_name)
                .or_else(|| path.strip_suffix(unet_fp16))
        })
        .filter(|root| root.is_empty() || root.find('/') == Some(root.len() - 1))
        .collect();
    let root = *roots.first()?;
    let find = |name: &str| {
        files
            .iter()
            .find(|f| f.path.strip_prefix(root) == Some(name))
    };
    let mut fp16 = false;
    let mut parts = Vec::new();
    for (name, fp16_name) in FOLDER_NAMES {
        let (file, is_fp16) = match (find(name), find(fp16_name)) {
            (Some(plain), Some(half)) if is_fp32(plain.size, half.size) => (half, true),
            (Some(plain), _) => (plain, false),
            (None, Some(half)) => (half, true),
            (None, None) => {
                parts.push(None);
                continue;
            }
        };
        file.sha256.as_ref()?;
        fp16 |= is_fp16;
        parts.push(Some(FolderFile {
            file: file.clone(),
            save_as: name,
        }));
    }
    let mut parts = parts.into_iter();
    let mut next = || parts.next().flatten();
    Some(DiffusersFolder {
        root: root.to_string(),
        unet: next()?,
        vae: next(),
        text_encoder: next(),
        text_encoder_2: next(),
        fp16,
    })
}

/// A plain file at least 1.8 times its fp16 variant is fp32.
fn is_fp32(plain: u64, half: u64) -> bool {
    u128::from(plain) * 5 >= u128::from(half) * 9
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_catalog::sha256_hex;
    use crate::sd::inspect::FOLDER_FILES;

    /// A listed file with a made-up SHA-256.
    fn rf(path: &str, size: u64) -> RepoFile {
        RepoFile {
            path: path.into(),
            size,
            sha256: Some(sha256_hex(path.as_bytes())),
        }
    }

    #[test]
    fn a_diffusers_folder_takes_the_fp16_files_where_the_plain_ones_are_fp32() {
        let d = diffusers_folder(&[
            rf("unet/diffusion_pytorch_model.safetensors", 3_438_167_534),
            rf(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                1_719_125_304,
            ),
            rf("text_encoder/model.safetensors", 492_265_879),
            rf("text_encoder/model.fp16.safetensors", 246_144_352),
            rf("vae/diffusion_pytorch_model.safetensors", 334_643_268),
            rf("model_index.json", 543),
            rf("safety_checker/model.safetensors", 1_215_981_830),
        ])
        .unwrap();
        assert!(d.fp16);
        assert_eq!(
            (d.unet.file.path.as_str(), d.unet.save_as),
            (
                "unet/diffusion_pytorch_model.fp16.safetensors",
                "unet/diffusion_pytorch_model.safetensors"
            )
        );
        // No fp16 variant: the plain file.
        assert_eq!(
            d.vae.as_ref().unwrap().file.path,
            "vae/diffusion_pytorch_model.safetensors"
        );
        assert_eq!(
            d.text_encoder.as_ref().unwrap().save_as,
            "text_encoder/model.safetensors"
        );
        assert!(d.text_encoder_2.is_none());
        assert_eq!(d.root, "");
        // Not an SD folder.
        assert!(
            diffusers_folder(&[rf("transformer/diffusion_pytorch_model.safetensors", 1)]).is_none()
        );
    }

    #[test]
    fn a_folder_one_level_down_keeps_its_root_and_needs_each_chosen_files_sha() {
        let files = [
            rf(
                "sdxl/unet/diffusion_pytorch_model.fp16.safetensors",
                5_135_149_760,
            ),
            // Both fp16 (the plain one is not 1.8 times the other): the plain file.
            rf("sdxl/text_encoder/model.safetensors", 246_144_152),
            rf("sdxl/text_encoder/model.fp16.safetensors", 246_144_152),
            rf("sdxl/text_encoder_2/model.safetensors", 2_778_702_264),
            rf("sdxl/text_encoder_2/model.fp16.safetensors", 1_389_382_176),
        ];
        let d = diffusers_folder(&files).unwrap();
        assert_eq!(d.root, "sdxl/");
        assert!(d.fp16);
        assert_eq!(
            d.unet.file.path,
            "sdxl/unet/diffusion_pytorch_model.fp16.safetensors"
        );
        assert_eq!(
            d.text_encoder.as_ref().unwrap().file.path,
            "sdxl/text_encoder/model.safetensors"
        );
        let te2 = d.text_encoder_2.as_ref().unwrap();
        assert_eq!(
            (te2.file.path.as_str(), te2.save_as),
            (
                "sdxl/text_encoder_2/model.fp16.safetensors",
                "text_encoder_2/model.safetensors"
            )
        );
        assert!(d.vae.is_none());

        // A chosen file without an LFS SHA-256 cannot be pinned, so there is no folder; a
        // file that was not chosen does not matter.
        let mut no_sha = files.to_vec();
        no_sha[2].sha256 = None;
        assert!(diffusers_folder(&no_sha).is_some());
        no_sha[1].sha256 = None;
        assert!(diffusers_folder(&no_sha).is_none());

        // Two levels down is not a folder Studio plans; the top wins over a subfolder.
        assert!(
            diffusers_folder(&[rf("a/b/unet/diffusion_pytorch_model.safetensors", 1)]).is_none()
        );
        let both = [
            rf("x/unet/diffusion_pytorch_model.safetensors", 1),
            rf("unet/diffusion_pytorch_model.safetensors", 2),
        ];
        assert_eq!(diffusers_folder(&both).unwrap().root, "");
    }

    #[test]
    fn the_folder_names_are_the_engines() {
        for ((_, dir, file), (name, fp16)) in FOLDER_FILES.iter().zip(FOLDER_NAMES) {
            assert_eq!(name, format!("{dir}/{file}"));
            assert_eq!(
                fp16,
                format!(
                    "{dir}/{}.fp16.safetensors",
                    file.trim_end_matches(".safetensors")
                )
            );
        }
    }

    #[test]
    fn file_names_that_say_companion() {
        assert_eq!(
            companion_by_name("split_files/vae/qwen_image_vae.safetensors"),
            Some(SlotId::Vae)
        );
        assert_eq!(companion_by_name("Qwen3-4B-Q8_0.gguf"), Some(SlotId::Llm));
        assert_eq!(
            companion_by_name("mmproj-F16.gguf"),
            Some(SlotId::LlmVision)
        );
        assert_eq!(companion_by_name("Qwen_Image-Q4_0.gguf"), None);
        assert_eq!(companion_by_name("sd15-baked-vae.safetensors"), None);

        for (path, want) in [
            ("ae.safetensors", Some(SlotId::Vae)),
            ("split_files/vae/wan_2.1_vae.safetensors", Some(SlotId::Vae)),
            // The folder says so although the name has a model token.
            ("vae/diffusion_pytorch_model.safetensors", Some(SlotId::Vae)),
            ("unet/diffusion_pytorch_model.safetensors", None),
            ("ltx-2-audio_vae.safetensors", Some(SlotId::AudioVae)),
            ("taesd_decoder.safetensors", Some(SlotId::Tae)),
            ("t5xxl_fp8_e4m3fn.safetensors", Some(SlotId::T5xxl)),
            ("umt5-xxl-encoder-Q8_0.gguf", Some(SlotId::T5xxl)),
            // Name rows come before the folder's.
            (
                "split_files/text_encoders/byt5_small_glyphxl_fp16.safetensors",
                Some(SlotId::T5xxl),
            ),
            ("text_encoders/clip_l.safetensors", Some(SlotId::ClipL)),
            ("text_encoders/clip_g.safetensors", Some(SlotId::ClipG)),
            (
                "split_files/clip_vision/clip_vision_h.safetensors",
                Some(SlotId::ClipVision),
            ),
            (
                "mmproj-Qwen2.5-VL-7B-Instruct-f16.gguf",
                Some(SlotId::LlmVision),
            ),
            (
                "ltx-2-19b-embeddings_connector_dev_bf16.safetensors",
                Some(SlotId::EmbeddingsConnectors),
            ),
            ("wav2vec2-base.safetensors", Some(SlotId::AudioEncoder)),
            ("Qwen2.5-VL-7B-Instruct-Q8_0.gguf", Some(SlotId::Llm)),
            (
                "split_files/text_encoders/qwen_2.5_vl_7b_fp8_scaled.safetensors",
                Some(SlotId::Llm),
            ),
            ("Qwen3VL-4B-Instruct-Q4_K_M.gguf", Some(SlotId::Llm)),
            ("gemma_3_12B_it.safetensors", Some(SlotId::Llm)),
            (
                "split_files/text_encoders/ovis_2.5.safetensors",
                Some(SlotId::Llm),
            ),
            ("tokenizer.json", Some(SlotId::Tokenizer)),
            ("tokenizer/tokenizer.json", Some(SlotId::Tokenizer)),
            // Only weights files are companions, and a model token keeps a name out.
            ("vae/config.json", None),
            ("clip_l.ckpt", None),
            ("v1-5-pruned-emaonly.safetensors", None),
            ("flux1-kontext-dev-Q4_K_M.gguf", None),
            (
                "split_files/diffusion_models/qwen_image_edit_2509_fp8.safetensors",
                None,
            ),
        ] {
            assert_eq!(companion_by_name(path), want, "{path}");
        }
    }

    #[test]
    fn a_vae_by_name_is_a_file_under_2_gb() {
        let file = |path: &str, size| RepoFile {
            path: path.into(),
            size,
            sha256: None,
        };
        assert_eq!(
            companion_of(&file("wan2.2_vae.safetensors", 1_409_400_960)),
            Some(SlotId::Vae)
        );
        assert_eq!(
            companion_of(&file("model-vae.safetensors", 6_938_040_682)),
            None
        );
        assert_eq!(
            companion_of(&file("vae/model.safetensors", 2_000_000_000)),
            None
        );
        // The size rule is the VAE's alone.
        assert_eq!(
            companion_of(&file("t5xxl_fp16.safetensors", 9_787_841_024)),
            Some(SlotId::T5xxl)
        );
    }

    #[test]
    fn a_lora_token_in_the_path() {
        assert!(looks_like_lora("lora/style.safetensors"));
        assert!(looks_like_lora("pixel-art-xl-LoRA.safetensors"));
        assert!(looks_like_lora(
            "Wan2.2_lightx2v_lora_high_noise.safetensors"
        ));
        assert!(!looks_like_lora("flora.safetensors"));
        assert!(!looks_like_lora("flux1-dev-Q4_0.gguf"));
    }

    #[test]
    fn shards_are_named_by_their_number() {
        assert!(is_shard("text_encoder_2/model-00001-of-00002.safetensors"));
        assert!(is_shard("x/Model-00002-OF-00002.gguf"));
        assert!(!is_shard("model-00001.safetensors"));
        assert!(!is_shard("t5xxl_fp16.safetensors"));
    }
}
