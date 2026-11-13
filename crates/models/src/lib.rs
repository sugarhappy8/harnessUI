//! Model files: the GGUF header reader, Hugging Face file classification and the
//! fit check. See ARCHITECTURE.md sections 5 and 11.

pub mod files;
pub mod fit;
pub mod gguf;
pub mod summary;

pub use files::{RepoFile, RepoModels, Variant, classify, quant_label};
pub use fit::{FitInput, FitResult, Target, Verdict, check};
pub use gguf::{GgufError, GgufHeader, read_file};
pub use summary::ModelSummary;
pub mod arch;
pub mod catalog;
pub mod download;
pub mod hardware;
pub mod hf;
// 0.0.6 item 6 (WP10b): what each part of a model weighs and where it goes.
pub mod layers;
pub mod placement;
pub use gguf::{TensorEntry, read_tensor_table};
pub use layers::{LayerSize, LayerSizes};
