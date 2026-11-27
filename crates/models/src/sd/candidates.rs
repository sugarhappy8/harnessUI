//! Which files of a repo Studio reads when it opens (design section 7.3), and the labels
//! its file names alone decide.
//!
//! A repo's diffusers folder is read as one model ([`Candidates::folder`]). Every other
//! weights file, in this order:
//! 1. in a component folder (`transformer/`, `text_encoder/`, `vae/`, …, or any folder with a
//!    `config.json`) of a diffusers folder model without a UNet: `FolderModel`. Other
//!    folders beside it (SD 3.5's `text_encoders/`) take the rules below;
//! 2. a pickle (`.ckpt`, `.pt`, `.pth`, `.bin`): `Pickle`;
//! 3. a shard of a split file, and its `.index.json`: `Sharded`, its first shard read for the
//!    version;
//! 4. no SHA-256: `NoSha`;
//! 5. an obvious companion by name ([`companion_of`]): `Extra`, never read;
//! 6. under [`MIN_CANDIDATE_BYTES`] and no LoRA by name: not listed;
//! 7. otherwise, a member of the group of its [`group_key`]: the quantisations of one model,
//!    of which one header read labels all.
//!
//! The groups are read in the order [`read_order`] gives: one part of each model, then the
//! second, then LoRAs, then what a read cannot make runnable.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::files::{RepoFile, quant_span};
use crate::gen_catalog::GenRole;
use crate::sd::index::PICKLE_EXTENSIONS;
use crate::sd::matching::{tokens, without_part_words};
use crate::sd::names::{
    DiffusersFolder, companion_of, diffusers_folder, is_weights, looks_like_lora,
};
use crate::sd::{CannotRunReason, FileLabel};

/// Weights files under this size are read only when their name says LoRA (10 MB).
pub const MIN_CANDIDATE_BYTES: u64 = 10_000_000;

/// The quantisations of one model file: one header read, of `read`, labels them all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuantGroup {
    /// [`group_key`] of its members; for a split file, the path its `.index.json` has (or
    /// would have), lower-cased.
    pub key: String,
    /// The smallest member; for a split file, its first shard.
    pub read: String,
    /// Sorted by path.
    pub members: Vec<String>,
}

/// What a repo's listing says before any read.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidates {
    /// The SD 1.x, 2.x or XL diffusers folder, read as one model: its UNet and text
    /// encoders.
    pub folder: Option<DiffusersFolder>,
    /// The groups to read, in the order the budget is spent ([`read_order`]): the first
    /// [`crate::sd::remote::MAX_FILES_PER_REPO`] are read.
    pub groups: Vec<QuantGroup>,
    /// The labels the names decide, by path: never read, except a split file's first shard
    /// for its version.
    pub by_name: BTreeMap<String, FileLabel>,
}

/// The folders of a diffusers folder Studio does not list, beside the four the engine reads
/// and `tokenizer*/`.
const OTHER_FOLDER_PARTS: [&str; 3] = ["safety_checker", "feature_extractor", "scheduler"];

/// The folders the engine reads from a diffusers folder.
const FOLDER_PARTS: [&str; 4] = ["unet", "vae", "text_encoder", "text_encoder_2"];

static SHARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?P<base>.*)-\d{5}-of-\d{5}(?P<ext>\.[a-z0-9]+)$")
        .expect("the shard pattern compiles")
});

/// A picture model's precision word in a file name, which the chat side's quantisation
/// label does not know: `fp8`, `fp8_e4m3fn`, `fp8_e5m2`, each with or without `_scaled`,
/// and `int8` or `int8_convrot` (Comfy-Org's and Kijai's safetensors variants).
static PRECISION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[-._])(?P<p>fp8(?:[-_]e4m3fn|[-_]e5m2)?(?:[-_]scaled)?|int8(?:[-_]convrot)?)(?:[-._]|$)",
    )
    .expect("the precision pattern compiles")
});

/// A word of a file's name (or its folder's) that says its weights are packed for another
/// program (`nf4`, `fp4`, `nvfp4`, `mxfp4`, `bnb`, `bitsandbytes`, `torchao`, `awq`, `w4a8`,
/// `w6a8`): tensors the engine does not read, which no other file of the model shares.
static PACKED_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[-._/\s])(?:nf4|fp4|nvfp4|mxfp4|bnb|bitsandbytes|torchao|awq|w4a8|w6a8)(?:[-._/\s]|$)")
        .expect("the packed-word pattern compiles")
});

/// `path` lower-cased, without its quantisation label ([`crate::files::quant_label`]) or a
/// picture model's precision word ([`PRECISION`]): the key the quantisations of one file
/// share, except that a file whose name says it is packed for another program
/// ([`PACKED_WORD`]) is a group of its own, read by itself (`HighNoise/Wan-HighNoise-Q4_K_M.gguf` → `highnoise/wan-highnoise-.gguf`;
/// `wan2.2_t2v_high_noise_14B_fp16.safetensors` and its `_fp8_scaled` file →
/// `wan2.2_t2v_high_noise_14b_.safetensors`). Their tensor names are the same, so one read
/// labels them all.
pub fn group_key(path: &str) -> String {
    if PACKED_WORD.is_match(path) {
        return path.to_lowercase();
    }
    let key = match quant_span(path) {
        Some((start, end)) => format!("{}{}", &path[..start], &path[end..]),
        None => path.to_string(),
    };
    let at = key.rfind('/').map_or(0, |i| i + 1);
    let name = &key[at..];
    match PRECISION
        .captures_iter(name)
        .last()
        .and_then(|c| c.name("p"))
    {
        Some(m) => format!("{}{}{}", &key[..at], &name[..m.start()], &name[m.end()..]),
        None => key.clone(),
    }
    .to_lowercase()
}

/// The words of a name the matching table refuses whatever its header says: VACE, which
/// needs control frames, and S2V, which needs audio (`matching::OUT_OF_SCOPE`).
const REFUSED_WORDS: [&str; 2] = ["vace", "s2v"];

/// The order the read budget is spent in (final review, area 1, I-2), as `(tier, round)`:
/// - tier 0: model groups; tier 1: LoRA-named groups; tier 2: what a read cannot make
///   runnable: names the matching table refuses ([`REFUSED_WORDS`]) and split files, read
///   only for their version;
/// - within a tier, round 0 holds the first group of each model, a high/low pair's halves
///   being one model ([`without_part_words`]); round 1 the second; and so on.
///
/// So every model is read once before any is read twice, and every model before any LoRA.
fn read_order(groups: &mut [QuantGroup], splits: &BTreeSet<String>) {
    let tier = |g: &QuantGroup| {
        let words = tokens(&g.key);
        if splits.contains(&g.key) || words.iter().any(|w| REFUSED_WORDS.contains(&w.as_str())) {
            2
        } else if looks_like_lora(&g.key) {
            1
        } else {
            0
        }
    };
    groups.sort_by(|a, b| a.key.cmp(&b.key));
    let mut seen: BTreeMap<(u8, Vec<String>), usize> = BTreeMap::new();
    let mut ranked: Vec<((u8, usize), QuantGroup)> = groups
        .iter()
        .map(|g| {
            let t = tier(g);
            let round = seen.entry((t, without_part_words(&g.key))).or_insert(0);
            let rank = (t, *round);
            *round += 1;
            (rank, g.clone())
        })
        .collect();
    // Stable: within a rank, by key.
    ranked.sort_by_key(|(rank, _)| *rank);
    for (slot, (_, g)) in groups.iter_mut().zip(ranked) {
        *slot = g;
    }
}

fn is_pickle(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    PICKLE_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
}

fn is_index(path: &str) -> bool {
    path.to_ascii_lowercase().ends_with(".index.json")
}

fn cannot_run(reason: CannotRunReason) -> FileLabel {
    FileLabel::CannotRun {
        reason,
        version: None,
    }
}

/// The folder a repo path is in under `root`, if it is in one: `unet` for
/// `sd/unet/x.safetensors` under `sd/`.
fn folder_under<'a>(root: &str, path: &'a str) -> Option<&'a str> {
    let (dir, _) = path.strip_prefix(root)?.split_once('/')?;
    Some(dir)
}

/// Diffusers component folders by name, beside `transformer*/` and `text_encoder*/`.
const COMPONENT_FOLDERS: [&str; 5] = [
    "vae",
    "image_encoder",
    "audio_vae",
    "safety_checker",
    "controlnet",
];

/// Whether a folder's name is a diffusers component's: `transformer`, `text_encoder` (with a
/// `_N` suffix or none: `text_encoders/` is Comfy's folder of companions), or
/// [`COMPONENT_FOLDERS`].
fn is_component(dir: &str) -> bool {
    ["transformer", "text_encoder"].iter().any(|name| {
        dir.strip_prefix(name)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('_'))
    }) || COMPONENT_FOLDERS.contains(&dir)
}

/// The component folders (`"<root><folder>/"`) of a diffusers folder model with no UNet: a
/// `model_index.json` at the top or one folder down, no `unet/` beside it, and its folders
/// named as components ([`is_component`]) or holding a `config.json`.
fn folder_model_parts(files: &[RepoFile]) -> BTreeSet<String> {
    let paths: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let roots = files
        .iter()
        .filter_map(|f| f.path.strip_suffix("model_index.json"))
        .filter(|root| root.is_empty() || root.find('/') == Some(root.len() - 1))
        .filter(|root| {
            !files
                .iter()
                .any(|f| folder_under(root, &f.path) == Some("unet"))
        });
    let mut parts = BTreeSet::new();
    for root in roots {
        for f in files {
            if let Some(dir) = folder_under(root, &f.path)
                && (is_component(dir)
                    || paths.contains(format!("{root}{dir}/config.json").as_str()))
            {
                parts.insert(format!("{root}{dir}/"));
            }
        }
    }
    parts
}

/// Which files of `files` are read and how the rest are labelled (see the module's docs).
pub fn candidates(files: &[RepoFile]) -> Candidates {
    let folder = diffusers_folder(files);
    let mut by_name = BTreeMap::new();
    let mut groups: BTreeMap<String, Vec<&RepoFile>> = BTreeMap::new();
    // A split file's base path (`transformer.safetensors`) → its shards.
    let mut splits: BTreeMap<String, Vec<&RepoFile>> = BTreeMap::new();
    let mut indexes = BTreeSet::new();

    if let Some(folder) = &folder {
        for (part, role) in [
            (&folder.vae, GenRole::Vae),
            (&folder.text_encoder, GenRole::ClipL),
            (&folder.text_encoder_2, GenRole::ClipG),
        ] {
            if let Some(part) = part {
                by_name.insert(part.file.path.clone(), FileLabel::Extra { role });
            }
        }
    }
    let folder_model_parts = match folder {
        Some(_) => BTreeSet::new(),
        None => folder_model_parts(files),
    };

    for f in files {
        let path = f.path.as_str();
        // The diffusers folder's own files: its parts are listed above, the rest not.
        if let Some(folder) = &folder
            && folder_under(&folder.root, path).is_some_and(|dir| {
                FOLDER_PARTS.contains(&dir)
                    || OTHER_FOLDER_PARTS.contains(&dir)
                    || dir.starts_with("tokenizer")
            })
        {
            continue;
        }
        let weights = is_weights(path);
        if (weights || is_index(path))
            && folder_model_parts
                .iter()
                .any(|part| path.starts_with(part.as_str()))
        {
            by_name.insert(f.path.clone(), cannot_run(CannotRunReason::FolderModel));
            continue;
        }
        if is_pickle(path) {
            by_name.insert(f.path.clone(), cannot_run(CannotRunReason::Pickle));
            continue;
        }
        if is_index(path) {
            indexes.insert(path);
            continue;
        }
        if !weights {
            continue;
        }
        if let Some(c) = SHARD.captures(path) {
            splits
                .entry(format!("{}{}", &c["base"], &c["ext"]))
                .or_default()
                .push(f);
            continue;
        }
        if f.sha256.is_none() {
            by_name.insert(f.path.clone(), cannot_run(CannotRunReason::NoSha));
            continue;
        }
        if let Some(role) = companion_of(f).and_then(GenRole::from_slot) {
            by_name.insert(f.path.clone(), FileLabel::Extra { role });
            continue;
        }
        if f.size < MIN_CANDIDATE_BYTES && !looks_like_lora(path) {
            continue;
        }
        groups.entry(group_key(path)).or_default().push(f);
    }

    let mut out: Vec<QuantGroup> = groups
        .into_iter()
        .map(|(key, mut members)| {
            members.sort_by(|a, b| a.path.cmp(&b.path));
            let read = members
                .iter()
                .min_by_key(|f| (f.size, &f.path))
                .map(|f| f.path.clone())
                .unwrap_or_default();
            QuantGroup {
                key,
                read,
                members: members.iter().map(|f| f.path.clone()).collect(),
            }
        })
        .collect();
    let mut split_keys = BTreeSet::new();
    for (base, mut shards) in splits {
        shards.sort_by(|a, b| a.path.cmp(&b.path));
        let index = format!("{base}.index.json");
        split_keys.insert(index.to_lowercase());
        let mut members: Vec<String> = shards.iter().map(|f| f.path.clone()).collect();
        if indexes.contains(index.as_str()) {
            members.push(index.clone());
        }
        for path in &members {
            by_name.insert(path.clone(), cannot_run(CannotRunReason::Sharded));
        }
        // The first shard is read for the version, when it can be pinned.
        if shards[0].sha256.is_some() {
            out.push(QuantGroup {
                key: index.to_lowercase(),
                read: shards[0].path.clone(),
                members,
            });
        }
    }
    read_order(&mut out, &split_keys);
    Candidates {
        folder,
        groups: out,
        by_name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gen_catalog::{GenRole, sha256_hex};
    use crate::sd::detect::SdVersion;
    use crate::sd::inspect::inspect;
    use crate::sd::matching::{NoisePart, RecipeKind};
    use crate::sd::testing::fixture;
    use crate::sd::{CannotRunReason, FileLabel, Verdict, label_for, verdict_of};

    /// A listed file of `size` bytes with a made-up SHA-256.
    fn f(path: &str, size: f64) -> RepoFile {
        RepoFile {
            path: path.into(),
            size: size as u64,
            sha256: Some(sha256_hex(path.as_bytes())),
        }
    }

    fn f_no_sha(path: &str, size: f64) -> RepoFile {
        RepoFile {
            sha256: None,
            ..f(path, size)
        }
    }

    fn reads(c: &Candidates) -> Vec<&str> {
        c.groups.iter().map(|g| g.read.as_str()).collect()
    }

    /// One header read labels every quantisation of a group, because their tensor names are the
    /// same. A file packed for another program (NF4, NVFP4, bitsandbytes, torchao) has
    /// different tensors, so it never shares a group: else the smallest file's reading, often
    /// the packed one, would call the bf16 file unrunnable, or the other way round call the
    /// packed one *Works in Studio*.
    #[test]
    fn a_file_packed_for_another_program_is_read_by_itself() {
        let c = candidates(&[
            f("qwen_image_2.1_bf16.safetensors", 40e9),
            f("qwen_image_2.1_fp8_scaled.safetensors", 20e9),
            f("qwen_image_2.1_nvfp4.safetensors", 11e9),
            f("qwen_image_2.1_int8_convrot.safetensors", 21e9),
            f("model-NF4-bnb/diffusion_pytorch_model.safetensors", 3.7e9),
            f("model-FP8-torchao/diffusion_pytorch_model.safetensors", 7e9),
            f("model-INT8-bnb/diffusion_pytorch_model.safetensors", 7e9),
        ]);
        let group_of = |path: &str| {
            c.groups
                .iter()
                .find(|g| g.members.iter().any(|m| m == path))
                .map(|g| g.members.clone())
                .unwrap()
        };
        // The runnable precisions still share one reading.
        assert_eq!(
            group_of("qwen_image_2.1_bf16.safetensors"),
            [
                "qwen_image_2.1_bf16.safetensors",
                "qwen_image_2.1_fp8_scaled.safetensors",
                "qwen_image_2.1_int8_convrot.safetensors"
            ]
        );
        for packed in [
            "qwen_image_2.1_nvfp4.safetensors",
            "model-NF4-bnb/diffusion_pytorch_model.safetensors",
            "model-FP8-torchao/diffusion_pytorch_model.safetensors",
            "model-INT8-bnb/diffusion_pytorch_model.safetensors",
        ] {
            assert_eq!(group_of(packed), [packed], "{packed}");
        }
    }

    #[test]
    fn candidates_name_what_they_skip() {
        let c = candidates(&[
            f("m.ckpt", 4e9),
            f("t5xxl_fp8_e4m3fn.safetensors", 4.8e9),
            f("tiny.safetensors", 1e6),
            f("lora/style.safetensors", 2e6),
            f("m-Q4_0.gguf", 7e9),
            f("m-Q8_0.gguf", 12e9),
            f_no_sha("x.safetensors", 3e9),
        ]);
        assert_eq!(
            c.by_name["m.ckpt"],
            FileLabel::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None
            }
        );
        assert_eq!(
            c.by_name["t5xxl_fp8_e4m3fn.safetensors"],
            FileLabel::Extra {
                role: GenRole::T5xxl
            }
        );
        assert_eq!(
            c.by_name["x.safetensors"],
            FileLabel::CannotRun {
                reason: CannotRunReason::NoSha,
                version: None
            }
        );
        // The model before the LoRA (final review, area 1, I-2).
        assert_eq!(reads(&c), ["m-Q4_0.gguf", "lora/style.safetensors"]);
        // A small file that is no LoRA is not listed at all.
        assert!(!c.by_name.contains_key("tiny.safetensors"));
        assert_eq!(c.groups[0].members, ["m-Q4_0.gguf", "m-Q8_0.gguf"]);
        assert!(c.folder.is_none());
    }

    #[test]
    fn model_files_with_companion_words_are_still_read() {
        let c = candidates(&[
            f("Qwen_Image-Q4_0.gguf", 11.8e9),
            f("sd15-baked-vae.safetensors", 4.2e9),
            f("Qwen3-4B-Q8_0.gguf", 4.3e9),
        ]);
        assert_eq!(
            reads(&c),
            ["Qwen_Image-Q4_0.gguf", "sd15-baked-vae.safetensors"]
        );
        assert_eq!(
            c.by_name["Qwen3-4B-Q8_0.gguf"],
            FileLabel::Extra { role: GenRole::Llm }
        );
        // A 6.9 GB file named like a VAE is a model: the VAE name rule holds under 2 GB.
        let c = candidates(&[f("sdxl-vae-fp16-fix.safetensors", 6.9e9)]);
        assert_eq!(reads(&c), ["sdxl-vae-fp16-fix.safetensors"]);
    }

    #[test]
    fn lumina_reads_as_cant_run_and_kontext_as_a_picture_edit() {
        assert_eq!(
            label_for(&inspect(&fixture("lumina2")), "Comfy-Org/Lumina-Image-2/x"),
            FileLabel::CannotRun {
                reason: CannotRunReason::Lumina2,
                version: Some(SdVersion::ZImage)
            }
        );
        assert!(matches!(
            label_for(
                &inspect(&fixture("flux1-kontext")),
                "QuantStack/FLUX.1-Kontext-dev-GGUF/flux1-kontext-dev-Q4_K_M.gguf"
            ),
            FileLabel::Works {
                kind: RecipeKind::PictureEdit,
                ..
            }
        ));
    }

    #[test]
    fn a_label_names_the_recipe_the_part_and_the_size() {
        let high = inspect(&fixture("wan22-t2v-a14b-high"));
        let name =
            "QuantStack/Wan2.2-T2V-A14B-GGUF/HighNoise/Wan2.2-T2V-A14B-HighNoise-Q4_K_M.gguf";
        assert_eq!(
            label_for(&high, name),
            FileLabel::Works {
                version: SdVersion::Wan2,
                recipe: "wan22-t2v".into(),
                kind: RecipeKind::Video,
                part: Some(NoisePart::High),
                parameters: high.parameters,
                folder: false,
            }
        );
        assert_eq!(
            label_for(
                &inspect(&fixture("sdxl-lora-pixel-art")),
                "x/pixel-art-xl.safetensors"
            ),
            FileLabel::Lora {
                base: Some(SdVersion::Sdxl)
            }
        );
        assert_eq!(
            label_for(&inspect(&fixture("clip-l")), "x/clip_l.safetensors"),
            FileLabel::Extra {
                role: GenRole::ClipL
            }
        );
        // A name the matching table refuses (VACE needs a folder of control frames).
        assert_eq!(
            label_for(
                &inspect(&fixture("wan21-t2v-1.3b")),
                "Wan-AI/Wan2.1-VACE-1.3B/x.safetensors"
            ),
            FileLabel::CannotRun {
                reason: CannotRunReason::NeedsControlFrames,
                version: Some(SdVersion::Wan2)
            }
        );
    }

    #[test]
    fn every_quantisation_of_one_part_is_one_group_read_at_its_smallest() {
        let c = candidates(&[
            f("HighNoise/Wan2.2-T2V-A14B-HighNoise-Q8_0.gguf", 15.4e9),
            f("HighNoise/Wan2.2-T2V-A14B-HighNoise-Q2_K.gguf", 5.3e9),
            f("LowNoise/Wan2.2-T2V-A14B-LowNoise-Q2_K.gguf", 5.3e9),
            f("LowNoise/Wan2.2-T2V-A14B-LowNoise-F16.gguf", 28.6e9),
            f("README.md", 1e3),
        ]);
        assert_eq!(
            c.groups
                .iter()
                .map(|g| (g.key.as_str(), g.read.as_str(), g.members.len()))
                .collect::<Vec<_>>(),
            [
                (
                    "highnoise/wan2.2-t2v-a14b-highnoise-.gguf",
                    "HighNoise/Wan2.2-T2V-A14B-HighNoise-Q2_K.gguf",
                    2
                ),
                (
                    "lownoise/wan2.2-t2v-a14b-lownoise-.gguf",
                    "LowNoise/Wan2.2-T2V-A14B-LowNoise-Q2_K.gguf",
                    2
                ),
            ]
        );
        assert!(c.by_name.is_empty());
        assert_eq!(group_key("Model-BF16.SafeTensors"), "model-.safetensors");
        assert_eq!(group_key("flux1-dev.safetensors"), "flux1-dev.safetensors");
    }

    #[test]
    fn a_sharded_file_cannot_run_and_its_first_shard_is_read() {
        let c = candidates(&[
            f("transformer-00002-of-00002.safetensors", 4e9),
            f("transformer-00001-of-00002.safetensors", 5e9),
            f("transformer.safetensors.index.json", 1e5),
            // A split GGUF has no index, and is a split file all the same.
            f("big-Q8_0-00001-of-00002.gguf", 9e9),
            f("big-Q8_0-00002-of-00002.gguf", 9e9),
            // Pickle shards are pickles.
            f("pytorch_model-00001-of-00002.bin", 9e9),
        ]);
        let sharded = FileLabel::CannotRun {
            reason: CannotRunReason::Sharded,
            version: None,
        };
        for path in [
            "transformer-00001-of-00002.safetensors",
            "transformer-00002-of-00002.safetensors",
            "transformer.safetensors.index.json",
            "big-Q8_0-00001-of-00002.gguf",
            "big-Q8_0-00002-of-00002.gguf",
        ] {
            assert_eq!(c.by_name[path], sharded, "{path}");
        }
        assert_eq!(
            c.by_name["pytorch_model-00001-of-00002.bin"],
            FileLabel::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None
            }
        );
        assert_eq!(
            reads(&c),
            [
                "big-Q8_0-00001-of-00002.gguf",
                "transformer-00001-of-00002.safetensors"
            ]
        );
        assert_eq!(
            c.groups[1].members,
            [
                "transformer-00001-of-00002.safetensors",
                "transformer-00002-of-00002.safetensors",
                "transformer.safetensors.index.json"
            ]
        );
    }

    #[test]
    fn a_diffusers_folder_lists_its_parts_and_not_its_other_folders() {
        let c = candidates(&[
            f("model_index.json", 541.0),
            f("unet/diffusion_pytorch_model.safetensors", 3_438_167_534.0),
            f(
                "unet/diffusion_pytorch_model.fp16.safetensors",
                1_719_125_304.0,
            ),
            f("unet/diffusion_pytorch_model.bin", 3_438_375_973.0),
            f("text_encoder/model.safetensors", 492_265_879.0),
            f("text_encoder/model.fp16.safetensors", 246_144_352.0),
            f("vae/diffusion_pytorch_model.safetensors", 334_643_268.0),
            f("safety_checker/model.safetensors", 1_215_981_830.0),
            f("tokenizer/vocab.json", 1_059_962.0),
            f("sd-v1-4.ckpt", 4_265_380_512.0),
            f("sd-v1-4-full-ema.ckpt", 7_703_807_346.0),
        ]);
        let folder = c.folder.as_ref().unwrap();
        assert_eq!(
            folder.unet.file.path,
            "unet/diffusion_pytorch_model.fp16.safetensors"
        );
        assert_eq!(
            c.by_name.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "sd-v1-4-full-ema.ckpt",
                "sd-v1-4.ckpt",
                "text_encoder/model.fp16.safetensors",
                "vae/diffusion_pytorch_model.safetensors"
            ]
        );
        assert_eq!(
            c.by_name["vae/diffusion_pytorch_model.safetensors"],
            FileLabel::Extra { role: GenRole::Vae }
        );
        assert_eq!(
            c.by_name["text_encoder/model.fp16.safetensors"],
            FileLabel::Extra {
                role: GenRole::ClipL
            }
        );
        // The UNet is read with the folder, not as a group of its own.
        assert!(c.groups.is_empty());
    }

    #[test]
    fn a_folder_model_without_a_unet_cannot_run() {
        let c = candidates(&[
            f("model_index.json", 536.0),
            f("flux1-dev.safetensors", 23.8e9),
            f("ae.safetensors", 335e6),
            f(
                "transformer/diffusion_pytorch_model-00001-of-00003.safetensors",
                9.98e9,
            ),
            f(
                "transformer/diffusion_pytorch_model.safetensors.index.json",
                121e3,
            ),
            f("text_encoder_2/model-00001-of-00002.safetensors", 4.99e9),
            f("vae/diffusion_pytorch_model.safetensors", 168e6),
            f("tokenizer/vocab.json", 1e6),
        ]);
        let folder_model = FileLabel::CannotRun {
            reason: CannotRunReason::FolderModel,
            version: None,
        };
        for path in [
            "transformer/diffusion_pytorch_model-00001-of-00003.safetensors",
            "transformer/diffusion_pytorch_model.safetensors.index.json",
            "text_encoder_2/model-00001-of-00002.safetensors",
            "vae/diffusion_pytorch_model.safetensors",
        ] {
            assert_eq!(c.by_name[path], folder_model, "{path}");
        }
        assert!(!c.by_name.contains_key("tokenizer/vocab.json"));
        assert_eq!(
            c.by_name["ae.safetensors"],
            FileLabel::Extra { role: GenRole::Vae }
        );
        assert_eq!(reads(&c), ["flux1-dev.safetensors"]);
    }

    /// stabilityai/stable-diffusion-3.5-large: a diffusers folder model without a UNet, a
    /// checkpoint at the top, and Comfy's `text_encoders/` folder of its companions (no
    /// `config.json` there, only a README).
    #[test]
    fn sd35_large_lists_its_text_encoders_as_extra_files() {
        let c = candidates(&[
            f("model_index.json", 706.0),
            f("sd3.5_large.safetensors", 16_460_379_262.0),
            f("scheduler/scheduler_config.json", 141.0),
            f("text_encoder/config.json", 574.0),
            f("text_encoder/model.safetensors", 247_323_896.0),
            f("text_encoder_2/config.json", 570.0),
            f("text_encoder_2/model.safetensors", 1_389_382_176.0),
            f("text_encoder_3/config.json", 740.0),
            f(
                "text_encoder_3/model-00001-of-00002.safetensors",
                4_994_582_104.0,
            ),
            f(
                "text_encoder_3/model-00002-of-00002.safetensors",
                4_530_066_248.0,
            ),
            f("text_encoder_3/model.safetensors.index.json", 19_885.0),
            f("text_encoders/README.md", 1_046.0),
            f("text_encoders/clip_g.safetensors", 1_389_382_176.0),
            f("text_encoders/clip_l.safetensors", 246_144_152.0),
            f("text_encoders/t5xxl_fp16.safetensors", 9_787_841_024.0),
            f(
                "text_encoders/t5xxl_fp8_e4m3fn.safetensors",
                4_893_934_904.0,
            ),
            f("tokenizer/vocab.json", 1_059_962.0),
            f("transformer/config.json", 427.0),
            f(
                "transformer/diffusion_pytorch_model-00001-of-00002.safetensors",
                9_986_746_168.0,
            ),
            f(
                "transformer/diffusion_pytorch_model-00002-of-00002.safetensors",
                6_299_502_112.0,
            ),
            f(
                "transformer/diffusion_pytorch_model.safetensors.index.json",
                129_394.0,
            ),
            f("vae/config.json", 809.0),
            f("vae/diffusion_pytorch_model.safetensors", 167_666_902.0),
        ]);
        for (path, role) in [
            ("text_encoders/clip_g.safetensors", GenRole::ClipG),
            ("text_encoders/clip_l.safetensors", GenRole::ClipL),
            ("text_encoders/t5xxl_fp16.safetensors", GenRole::T5xxl),
            ("text_encoders/t5xxl_fp8_e4m3fn.safetensors", GenRole::T5xxl),
        ] {
            assert_eq!(c.by_name[path], FileLabel::Extra { role }, "{path}");
        }
        let folder_model = FileLabel::CannotRun {
            reason: CannotRunReason::FolderModel,
            version: None,
        };
        for path in [
            "text_encoder/model.safetensors",
            "text_encoder_2/model.safetensors",
            "text_encoder_3/model-00001-of-00002.safetensors",
            "text_encoder_3/model.safetensors.index.json",
            "transformer/diffusion_pytorch_model-00002-of-00002.safetensors",
            "transformer/diffusion_pytorch_model.safetensors.index.json",
            "vae/diffusion_pytorch_model.safetensors",
        ] {
            assert_eq!(c.by_name[path], folder_model, "{path}");
        }
        assert_eq!(reads(&c), ["sd3.5_large.safetensors"]);
        // A folder with a `config.json` is a component whatever its name.
        let c = candidates(&[
            f("model_index.json", 500.0),
            f("prior/config.json", 500.0),
            f("prior/diffusion_pytorch_model.safetensors", 3e9),
            f("split_files/diffusion_models/model.safetensors", 3e9),
        ]);
        assert_eq!(
            c.by_name["prior/diffusion_pytorch_model.safetensors"],
            folder_model
        );
        assert_eq!(
            reads(&c),
            ["split_files/diffusion_models/model.safetensors"]
        );
    }

    #[test]
    fn the_verdict_is_the_largest_model_then_a_lora_then_a_reason() {
        let works = |parameters, version| FileLabel::Works {
            version,
            recipe: "x".into(),
            kind: RecipeKind::Picture,
            part: None,
            parameters,
            folder: false,
        };
        let pickle = FileLabel::CannotRun {
            reason: CannotRunReason::Pickle,
            version: None,
        };
        let lora = FileLabel::Lora {
            base: Some(SdVersion::Flux),
        };
        let extra = FileLabel::Extra { role: GenRole::Vae };
        assert_eq!(
            verdict_of(&[
                pickle.clone(),
                works(2, SdVersion::Sd1),
                lora.clone(),
                works(12, SdVersion::Flux),
                works(3, SdVersion::Sdxl),
            ]),
            Verdict::Works {
                version: SdVersion::Flux,
                kind: RecipeKind::Picture,
                recipe: Some("x".into())
            }
        );
        assert_eq!(
            verdict_of(&[pickle.clone(), extra.clone(), lora.clone()]),
            Verdict::Lora {
                base: Some(SdVersion::Flux)
            }
        );
        assert_eq!(
            verdict_of(&[extra.clone(), pickle.clone()]),
            Verdict::CannotRun {
                reason: CannotRunReason::Pickle,
                version: None
            }
        );
        assert_eq!(
            verdict_of(&[
                extra,
                FileLabel::NotRead {
                    reason: crate::sd::NotRead::Budget
                }
            ]),
            Verdict::Unknown
        );
    }

    // ---- Final review, area 1, I-2: how the read budget is spent -------------------------

    const SPLIT: &str = "split_files/diffusion_models/";

    /// The files whose groups the first [`MAX_FILES_PER_REPO`] reads label.
    fn within_budget(c: &Candidates) -> BTreeSet<&str> {
        c.groups
            .iter()
            .take(crate::sd::remote::MAX_FILES_PER_REPO)
            .flat_map(|g| g.members.iter().map(String::as_str))
            .collect()
    }

    #[test]
    fn fp8_and_int8_variants_are_quantisations_of_their_model() {
        for (a, b) in [
            ("m_14B_fp16.safetensors", "m_14B_fp8_scaled.safetensors"),
            ("m_14B_bf16.safetensors", "m_14B_int8_convrot.safetensors"),
            ("m_fp16.safetensors", "m_fp8_e4m3fn.safetensors"),
            ("m_fp16.safetensors", "m_fp8_e4m3fn_scaled.safetensors"),
            ("m-fp16.safetensors", "m-fp8-e5m2.safetensors"),
            ("dir/M.FP8.safetensors", "dir/M.BF16.safetensors"),
            ("m_int8.safetensors", "m_fp16.safetensors"),
        ] {
            assert_eq!(group_key(a), group_key(b), "{a} and {b}");
        }
        // Only a whole word: a name that merely holds the letters keeps them.
        assert_ne!(
            group_key("fp8model.safetensors"),
            group_key("model.safetensors")
        );
        assert_ne!(
            group_key("a/fp8/m.safetensors"),
            group_key("a/m.safetensors")
        );
        // The chat side's quantisation label is unchanged.
        assert_eq!(crate::files::quant_label("m_fp8_scaled.safetensors"), None);
    }

    #[test]
    fn comfys_wan_2_2_repo_reads_its_t2v_ti2v_and_lora_files_within_the_cap() {
        let c = candidates(&crate::sd::testing::comfy_wan22_listing());
        let read = within_budget(&c);
        let must = [
            "wan2.2_t2v_high_noise_14B_fp16.safetensors",
            "wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors",
            "wan2.2_t2v_low_noise_14B_fp16.safetensors",
            "wan2.2_t2v_low_noise_14B_fp8_scaled.safetensors",
            "wan2.2_ti2v_5B_fp16.safetensors",
            "wan2.2_i2v_high_noise_14B_fp16.safetensors",
            "wan2.2_i2v_low_noise_14B_fp8_scaled.safetensors",
        ];
        for name in must {
            assert!(
                read.contains(format!("{SPLIT}{name}").as_str()),
                "{name} is read"
            );
        }
        for lora in [
            "chronoedit_distill_lora.safetensors",
            "wan2.2_animate_14B_relight_lora_bf16.safetensors",
            "wan2.2_i2v_lightx2v_4steps_lora_v1_high_noise.safetensors",
            "wan2.2_i2v_lightx2v_4steps_lora_v1_low_noise.safetensors",
            "wan2.2_t2v_lightx2v_4steps_lora_v1.1_high_noise.safetensors",
            "wan2.2_t2v_lightx2v_4steps_lora_v1.1_low_noise.safetensors",
        ] {
            assert!(
                read.contains(format!("split_files/loras/{lora}").as_str()),
                "{lora} is read"
            );
        }
        // One read for the fp16 and fp8 files of a part, of the smaller.
        let t2v_high = c
            .groups
            .iter()
            .find(|g| {
                g.members
                    .iter()
                    .any(|m| m.ends_with("t2v_high_noise_14B_fp16.safetensors"))
            })
            .unwrap();
        assert_eq!(
            t2v_high.members,
            [
                format!("{SPLIT}wan2.2_t2v_high_noise_14B_fp16.safetensors"),
                format!("{SPLIT}wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors")
            ]
        );
        assert_eq!(
            t2v_high.read,
            format!("{SPLIT}wan2.2_t2v_high_noise_14B_fp8_scaled.safetensors")
        );
    }

    #[test]
    fn the_budget_reads_one_part_of_each_model_before_a_second_and_loras_after_models() {
        let c = candidates(&crate::sd::testing::comfy_wan22_listing());
        let order: Vec<&str> = c.groups.iter().map(|g| g.read.as_str()).collect();
        let at = |needle: &str| {
            order
                .iter()
                .position(|p| p.contains(needle))
                .unwrap_or_else(|| panic!("{needle} in {order:#?}"))
        };
        // The first part of every model comes before the second part of any.
        let firsts = [
            "t2v_high",
            "i2v_high",
            "fun_camera_high",
            "fun_control_high",
            "ti2v_5B",
        ];
        let seconds = ["t2v_low", "i2v_low", "fun_camera_low", "fun_control_low"];
        let last_first = firsts.iter().map(|n| at(n)).max().unwrap();
        let first_second = seconds.iter().map(|n| at(n)).min().unwrap();
        assert!(last_first < first_second, "{order:#?}");
        // LoRAs after every model file Studio may run, and before what it refuses by name.
        let loras = order.iter().filter(|p| p.contains("/loras/")).count();
        assert_eq!(loras, 6);
        let first_lora = order.iter().position(|p| p.contains("/loras/")).unwrap();
        assert!(seconds.iter().all(|n| at(n) < first_lora), "{order:#?}");
        assert!(
            at("fun_vace") > first_lora && at("s2v") > first_lora,
            "{order:#?}"
        );
    }

    #[test]
    fn a_small_repo_still_reads_every_group() {
        let c = candidates(&[
            f("b-model.safetensors", 7e9),
            f("a-lora.safetensors", 2e8),
            f("c_vace_model.safetensors", 7e9),
        ]);
        assert_eq!(c.groups.len(), 3);
        assert_eq!(
            reads(&c),
            [
                "b-model.safetensors",
                "a-lora.safetensors",
                "c_vace_model.safetensors"
            ]
        );
    }

    /// Final review, area 3, I3: a Wan 2.2 A14B file detects as `wan2`, and only its recipe
    /// says 2.2, so the verdict carries the recipe for the webview to name the type by.
    #[test]
    fn a_works_verdict_carries_its_recipe_in_camel_case() {
        let label = FileLabel::Works {
            version: SdVersion::Wan2,
            recipe: "wan22-t2v".into(),
            kind: RecipeKind::Video,
            part: Some(NoisePart::High),
            parameters: 14_000_000_000,
            folder: false,
        };
        let verdict = verdict_of(&[label]);
        assert_eq!(
            verdict,
            Verdict::Works {
                version: SdVersion::Wan2,
                kind: RecipeKind::Video,
                recipe: Some("wan22-t2v".into())
            }
        );
        assert_eq!(
            serde_json::to_value(&verdict).unwrap(),
            serde_json::json!({"works": {"version": "wan2", "kind": "video", "recipe": "wan22-t2v"}})
        );
        // A verdict saved without a recipe still reads.
        let old: Verdict = serde_json::from_value(
            serde_json::json!({"works": {"version": "wan2", "kind": "video"}}),
        )
        .unwrap();
        assert_eq!(
            old,
            Verdict::Works {
                version: SdVersion::Wan2,
                kind: RecipeKind::Video,
                recipe: None
            }
        );
    }
}
