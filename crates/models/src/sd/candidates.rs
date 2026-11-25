//! Which files of a repo Studio reads when it opens (design section 7.3), and the labels
//! its file names alone decide.
//!
//! A repo's diffusers folder is read as one model ([`Candidates::folder`]). Every other
//! weights file, in this order:
//! 1. in a diffusers folder model without a UNet (a `transformer/` model): `FolderModel`;
//! 2. a pickle (`.ckpt`, `.pt`, `.pth`, `.bin`): `Pickle`;
//! 3. a shard of a split file, and its `.index.json`: `Sharded`, its first shard read for the
//!    version;
//! 4. no SHA-256: `NoSha`;
//! 5. an obvious companion by name ([`companion_of`]): `Extra`, never read;
//! 6. under [`MIN_CANDIDATE_BYTES`] and no LoRA by name: not listed;
//! 7. otherwise, a member of the group of its [`group_key`]: the quantisations of one model,
//!    of which one header read labels all.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::files::{RepoFile, quant_span};
use crate::gen_catalog::GenRole;
use crate::sd::index::PICKLE_EXTENSIONS;
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
    /// The groups to read, sorted by key: the first [`crate::sd::remote::MAX_FILES_PER_REPO`]
    /// are read.
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

/// `path` lower-cased, without its quantisation label ([`crate::files::quant_label`]): the
/// key the quantisations of one file share (`HighNoise/Wan-HighNoise-Q4_K_M.gguf` →
/// `highnoise/wan-highnoise-.gguf`).
pub fn group_key(path: &str) -> String {
    match quant_span(path) {
        Some((start, end)) => format!("{}{}", &path[..start], &path[end..]).to_lowercase(),
        None => path.to_lowercase(),
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

/// The roots (`""` or `"<folder>/"`) of a diffusers folder model with no UNet: a
/// `model_index.json` at the top or one folder down, and no `unet/` beside it.
fn folder_model_roots(files: &[RepoFile]) -> Vec<&str> {
    files
        .iter()
        .filter_map(|f| f.path.strip_suffix("model_index.json"))
        .filter(|root| root.is_empty() || root.find('/') == Some(root.len() - 1))
        .filter(|root| {
            !files
                .iter()
                .any(|f| folder_under(root, &f.path) == Some("unet"))
        })
        .collect()
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
    let folder_models = match folder {
        Some(_) => Vec::new(),
        None => folder_model_roots(files),
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
            && folder_models
                .iter()
                .any(|root| folder_under(root, path).is_some())
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
    for (base, mut shards) in splits {
        shards.sort_by(|a, b| a.path.cmp(&b.path));
        let index = format!("{base}.index.json");
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
    out.sort_by(|a, b| a.key.cmp(&b.key));
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
        assert_eq!(reads(&c), ["lora/style.safetensors", "m-Q4_0.gguf"]);
        // A small file that is no LoRA is not listed at all.
        assert!(!c.by_name.contains_key("tiny.safetensors"));
        assert_eq!(c.groups[1].members, ["m-Q4_0.gguf", "m-Q8_0.gguf"]);
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
                kind: RecipeKind::Picture
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
}
