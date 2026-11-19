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
// 0.1.0 Studio: the signed catalog's picture and video entries.
pub mod gen_catalog;
// 0.1.0 Studio: how long a local picture or video takes, and the fit's word on screen.
pub mod gen_eta;
// 0.1.0 Studio: whether a picture or video model fits this computer.
pub mod gen_fit;
// 0.1.0 Studio: picture and video models found on Hugging Face, and local files' families.
pub mod gen_search;
pub mod hardware;
pub mod hf;
// 0.0.6 item 6 (WP10b): what each part of a model weighs and where it goes.
pub mod layers;
pub mod placement;
pub use gguf::{TensorEntry, TensorShape, read_tensor_shapes, read_tensor_table};
pub use layers::{LayerSize, LayerSizes};
// 0.0.7 P1: the conversation memory as llama.cpp allocates it, layer by layer.
pub mod kv;
pub use kv::{KvLayout, KvSize};
