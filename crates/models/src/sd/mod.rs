//! 0.2.0 Studio: running any picture or video model the pinned engine supports. The
//! engine's own recipes are vendored in `catalog/sd-recipes.json` (design section 7.1).

use serde::{Deserialize, Serialize};

use crate::gen_catalog::GenRole;
use crate::sd::detect::SdVersion;
use crate::sd::index::FileFormat;
use crate::sd::inspect::{FileKind, Layout, MatchFacts};
use crate::sd::matching::{Match, NoisePart, RecipeKind, kind_of, match_recipe};

pub mod candidates;
pub mod companions;
pub mod detect;
pub mod index;
pub mod inspect;
pub mod lora;
pub mod matching;
pub mod names;
pub mod recipes;
pub mod remote;
pub mod setup;
#[cfg(any(test, feature = "test-support"))]
pub mod testing;

/// Why Studio cannot run a file or a recipe: the one list, for files (Task R6), out-of-scope
/// recipes (Task R7) and their labels (Task S5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CannotRunReason {
    /// Lumina 2, which the engine detects as Z-Image.
    Lumina2,
    /// LTX-Video 0.9, which the engine detects as `ltxav` though it has no audio part.
    LtxVideo09,
    /// HunyuanVideo 1, which the engine detects as `hunyuan-video` (version 1.5).
    HunyuanVideo1,
    Svd,
    UnknownWeights,
    Sharded,
    Pickle,
    NoSha,
    Adapter,
    NeedsControlFrames,
    NeedsAudio,
    Upscaler,
    FolderModel,
    NotThisEngine,
    Ltx2,
    /// Packed in bitsandbytes' 4-bit formats (NF4 or FP4), made for another program: the
    /// engine drops the packed weights, so nothing loads.
    Bnb4,
    /// Packed in bitsandbytes' 8-bit format (LLM.int8): the engine reads its weights without
    /// their row scales, so the picture would be noise.
    Bnb8,
    /// Packed by torchao (float8 and other tensor subclasses): the engine has no such weights.
    Torchao,
    /// Packed in ComfyUI's 4-bit block format (NVFP4, MXFP4): the engine drops or refuses it.
    Nvfp4,
}

/// What a file is, the facts recipe matching reads, and its size.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileFacts {
    pub kind: FileKind,
    pub facts: MatchFacts,
    /// The whole file's parameter count ([`index::TensorIndex::parameters`]).
    pub parameters: u64,
    /// Means nothing for `CannotRun { Pickle }`: a pickle file is never read, so the kind is
    /// what counts.
    pub format: FileFormat,
}

/// Why a file of a repo was not read when the repo opened (Task S5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NotRead {
    /// The repo is gated and there is no Hugging Face token.
    NeedsToken,
    /// The repo is gated and its terms are not accepted.
    NeedsTerms,
    /// Past the files read for one repo ([`remote::MAX_FILES_PER_REPO`]).
    Budget,
    /// The read failed.
    Failed,
}

/// What a file of a repo is, as its row says (design section 7.3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum FileLabel {
    /// A model Studio runs with `recipe`. `part`: the half of a high/low pair its name says;
    /// `folder`: the UNet of an SD 1.x, 2.x or XL diffusers folder, run as the folder.
    Works {
        version: SdVersion,
        recipe: String,
        kind: RecipeKind,
        part: Option<NoisePart>,
        parameters: u64,
        folder: bool,
    },
    Lora {
        base: Option<SdVersion>,
    },
    /// A part a recipe's slot takes.
    Extra {
        role: GenRole,
    },
    CannotRun {
        reason: CannotRunReason,
        version: Option<SdVersion>,
    },
    NotRead {
        reason: NotRead,
    },
}

/// What a repo offers, from its files' labels ([`verdict_of`]) or its name alone
/// ([`crate::gen_search::verdict_hint`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Verdict {
    /// `recipe`: the recipe of the file that works, which names the model's type where its
    /// version does not (Wan 2.2 A14B files detect as `wan2`; only `wan22-t2v` says 2.2);
    /// `None` for a hint from a name that says no more than the version.
    Works {
        version: SdVersion,
        kind: RecipeKind,
        #[serde(default)]
        recipe: Option<String>,
    },
    /// A model Studio recognises and has a recipe for, none of whose files can start: each
    /// lacks a part nothing fills (the companion table has none for it, and the repository
    /// none), so it is not *Works in Studio*. Made only where the plans are (a repository's
    /// opening), never by [`verdict_of`], which reads labels.
    NeedsFiles {
        version: SdVersion,
        kind: RecipeKind,
        #[serde(default)]
        recipe: Option<String>,
    },
    Lora {
        base: Option<SdVersion>,
    },
    CannotRun {
        reason: CannotRunReason,
        version: Option<SdVersion>,
    },
    Unknown,
}

/// The label of a file whose header says `facts`, by its name (`"<repo>/<path>"`), which
/// picks the recipe ([`match_recipe`]) and the high or low part. A diffusers folder's UNet
/// works only as an SD 1.x, 2.x or XL folder; another version is `FolderModel`. A part for
/// an adapter slot is `Adapter`.
pub fn label_for(facts: &FileFacts, name: &str) -> FileLabel {
    match &facts.kind {
        FileKind::Model {
            version,
            layout,
            embeds,
        } => {
            let folder = *layout == Layout::Folder;
            if folder && !version.is_unet() {
                return FileLabel::CannotRun {
                    reason: CannotRunReason::FolderModel,
                    version: Some(*version),
                };
            }
            match match_recipe(*version, &facts.facts, embeds, name) {
                Match::Recipe { id, part } => FileLabel::Works {
                    version: *version,
                    recipe: id.to_string(),
                    kind: kind_of(id),
                    part,
                    parameters: facts.parameters,
                    folder,
                },
                Match::Refused(reason) => FileLabel::CannotRun {
                    reason,
                    version: Some(*version),
                },
            }
        }
        FileKind::Lora { base, .. } => FileLabel::Lora { base: *base },
        FileKind::Extra { slot } => match GenRole::from_slot(*slot) {
            Some(role) => FileLabel::Extra { role },
            None => FileLabel::CannotRun {
                reason: CannotRunReason::Adapter,
                version: None,
            },
        },
        FileKind::CannotRun { reason, version } => FileLabel::CannotRun {
            reason: *reason,
            version: *version,
        },
    }
}

/// A repo's verdict from its files' labels: the `Works` label with the most parameters (the
/// first of equals); else the first LoRA; else the first `CannotRun`; else `Unknown`.
pub fn verdict_of(labels: &[FileLabel]) -> Verdict {
    let mut best: Option<(u64, Verdict)> = None;
    for label in labels {
        if let FileLabel::Works {
            version,
            kind,
            parameters,
            recipe,
            ..
        } = label
            && best.as_ref().is_none_or(|(most, _)| parameters > most)
        {
            best = Some((
                *parameters,
                Verdict::Works {
                    version: *version,
                    kind: *kind,
                    recipe: Some(recipe.clone()),
                },
            ));
        }
    }
    if let Some((_, works)) = best {
        return works;
    }
    let lora = labels.iter().find_map(|l| match l {
        FileLabel::Lora { base } => Some(Verdict::Lora { base: *base }),
        _ => None,
    });
    let cannot_run = || {
        labels.iter().find_map(|l| match l {
            FileLabel::CannotRun { reason, version } => Some(Verdict::CannotRun {
                reason: *reason,
                version: *version,
            }),
            _ => None,
        })
    };
    lora.or_else(cannot_run).unwrap_or(Verdict::Unknown)
}
