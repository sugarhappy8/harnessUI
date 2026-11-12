//! Where a load is, as typed values (0.0.5, "LoadStage"). The plain words for each value
//! live in the webview (`src/lib/words.ts`); nothing here is shown as it is.
//!
//! Stage names reported by llama-server's managed-child protocol map to
//! [`LoadStage::from_llama_stage`]; a name this build does not know becomes the generic
//! [`LoadStage::Loading`], so a new llama.cpp stage never shows a raw identifier.
//! Transitions that have no [`ServerEvent`](super::server::ServerEvent) of their own
//! (the retries WP1a adds) are sent as [`ServerEvent::Stage`](super::server::ServerEvent::Stage).

use serde::{Deserialize, Serialize};

use super::backend::BackendKind;
use super::failure::LoadFailure;

/// One step of loading a model, or a transition between attempts.
///
/// Serialised with a `kind` tag, camelCase: `{"kind":"readingModel"}`,
/// `{"kind":"restarting","attempt":1,"max":3}`,
/// `{"kind":"steppingDown","from":"vulkan","to":"cpu","reason":{"code":"out_of_memory"}}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LoadStage {
    /// The server process is starting and has reported nothing yet.
    Starting,
    /// Reading the model file (llama.cpp's `text_model`).
    ReadingModel,
    /// Reading the image reader, the projector (llama.cpp's `mmproj_model`).
    ReadingImageReader,
    /// Any other stage llama.cpp reports (for example `spec_model`, or a name a newer
    /// build adds).
    Loading,
    /// Starting again after the server stopped unexpectedly: restart `attempt` of `max`
    /// within the restart window.
    Restarting {
        /// Which restart this is (1-based).
        attempt: u32,
        /// How many restarts the window allows.
        max: u32,
    },
    /// Trying again on the same backend with Save memory off, after a failure with it
    /// on (0.0.5 D2c). Sent by the supervisor (WP1a).
    RetryingWithoutSaveMemory {
        /// The backend being retried.
        backend: BackendKind,
    },
    /// Trying again on the same backend with only as much of the model on the graphics
    /// card as fits, after it ran out of memory. Sent by the supervisor (WP1a).
    RetryingWithLessGpu {
        /// The backend being retried.
        backend: BackendKind,
    },
    /// Moving to the next backend down after `from` failed.
    SteppingDown {
        /// The backend that failed.
        from: BackendKind,
        /// The backend tried next.
        to: BackendKind,
        /// Why `from` failed.
        reason: LoadFailure,
    },
}

impl LoadStage {
    /// The stage for a name from the managed-child protocol (`text_model`, `mmproj_model`
    /// and so on). Unknown names are [`LoadStage::Loading`].
    pub fn from_llama_stage(name: &str) -> Self {
        match name {
            "text_model" => Self::ReadingModel,
            "mmproj_model" => Self::ReadingImageReader,
            _ => Self::Loading,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn llama_stage_names_map_and_unknown_names_are_generic() {
        assert_eq!(
            LoadStage::from_llama_stage("text_model"),
            LoadStage::ReadingModel
        );
        assert_eq!(
            LoadStage::from_llama_stage("mmproj_model"),
            LoadStage::ReadingImageReader
        );
        for other in ["spec_model", "", "some_new_stage", "TEXT_MODEL"] {
            assert_eq!(LoadStage::from_llama_stage(other), LoadStage::Loading);
        }
    }

    /// The webview matches on these exact shapes (src/lib/types.ts `LoadStage`).
    #[test]
    fn serialised_shapes() {
        let cases = [
            (LoadStage::Starting, r#"{"kind":"starting"}"#),
            (LoadStage::ReadingModel, r#"{"kind":"readingModel"}"#),
            (
                LoadStage::ReadingImageReader,
                r#"{"kind":"readingImageReader"}"#,
            ),
            (LoadStage::Loading, r#"{"kind":"loading"}"#),
            (
                LoadStage::Restarting { attempt: 1, max: 3 },
                r#"{"kind":"restarting","attempt":1,"max":3}"#,
            ),
            (
                LoadStage::RetryingWithoutSaveMemory {
                    backend: BackendKind::Vulkan,
                },
                r#"{"kind":"retryingWithoutSaveMemory","backend":"vulkan"}"#,
            ),
            (
                LoadStage::RetryingWithLessGpu {
                    backend: BackendKind::Metal,
                },
                r#"{"kind":"retryingWithLessGpu","backend":"metal"}"#,
            ),
            (
                LoadStage::SteppingDown {
                    from: BackendKind::Vulkan,
                    to: BackendKind::Cpu,
                    reason: LoadFailure::OutOfMemory,
                },
                r#"{"kind":"steppingDown","from":"vulkan","to":"cpu","reason":{"code":"out_of_memory"}}"#,
            ),
        ];
        for (stage, json) in cases {
            assert_eq!(serde_json::to_string(&stage).expect("json"), json);
            let back: LoadStage = serde_json::from_str(json).expect("reads");
            assert_eq!(back, stage);
        }
    }
}
