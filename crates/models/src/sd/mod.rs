//! 0.2.0 Studio: running any picture or video model the pinned engine supports. The
//! engine's own recipes are vendored in `catalog/sd-recipes.json` (design section 7.1).

use serde::{Deserialize, Serialize};

use crate::sd::index::FileFormat;
use crate::sd::inspect::{FileKind, MatchFacts};

pub mod companions;
pub mod detect;
pub mod index;
pub mod inspect;
pub mod lora;
pub mod matching;
pub mod recipes;
pub mod remote;
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
