//! Where a load is, as typed values (0.0.6, "LoadStage"). The plain words for each value
//! live in the webview (`src/lib/words.ts`); nothing here is shown as it is.
//!
//! Stage names reported by llama-server's managed-child protocol map to
//! [`LoadStage::from_llama_stage`]; a name this build does not know becomes the generic
//! [`LoadStage::Loading`], so a new llama.cpp stage never shows a raw identifier.
//! Transitions that have no [`ServerEvent`] of their own (the supervisor's retries on the
//! same backend) are sent as [`ServerEvent::Stage`]. [`LoadStage::of_event`] is the one
//! mapping from events to stages.

use serde::{Deserialize, Serialize};

use super::backend::BackendKind;
use super::failure::LoadFailure;
use super::server::ServerEvent;

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
    /// on that it can cause (0.0.6 D2c). Sent by the supervisor as [`ServerEvent::Stage`].
    /// With `backend` the processor it always means the model itself refused Save memory
    /// (its log said so); on a graphics backend it may be the model or the card.
    RetryingWithoutSaveMemory {
        /// The backend being retried.
        backend: BackendKind,
    },
    /// Trying again on the same backend with only as much of the model on the graphics
    /// card as fits, after it ran out of memory with all of it there. Sent by the
    /// supervisor as [`ServerEvent::Stage`].
    RetryingWithLessGpu {
        /// The backend being retried.
        backend: BackendKind,
    },
    /// Moving to the next backend down after `from` failed. Sent by the supervisor as
    /// [`ServerEvent::SteppedDown`], never as [`ServerEvent::Stage`].
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

    /// The stage an event moves the load to, or `None` when the event does not change it.
    ///
    /// [`ServerEvent::State`] maps to nothing: the app shows [`LoadStage::Starting`] when a
    /// load begins, and a later `Starting` state belongs to a restart, retry or step-down
    /// whose own stage stays shown until the new process reports progress.
    pub fn of_event(event: &ServerEvent) -> Option<Self> {
        match event {
            ServerEvent::Progress { stage, .. } => stage.as_deref().map(Self::from_llama_stage),
            ServerEvent::Restarting { attempt, max } => Some(Self::Restarting {
                attempt: *attempt,
                max: *max,
            }),
            ServerEvent::SteppedDown { from, to, reason } => Some(Self::SteppingDown {
                from: *from,
                to: *to,
                reason: reason.clone(),
            }),
            ServerEvent::Stage(stage) => Some(stage.clone()),
            ServerEvent::State(_)
            | ServerEvent::Log(_)
            | ServerEvent::Offloaded { .. }
            | ServerEvent::Exited { .. } => None,
        }
    }

    /// Whether the stage changes how the model runs for the rest of the load (a setting
    /// turned down, or another backend), so the app keeps it after the load ends
    /// (`adjustments` in the runtime status).
    pub fn is_adjustment(&self) -> bool {
        matches!(
            self,
            Self::RetryingWithoutSaveMemory { .. }
                | Self::RetryingWithLessGpu { .. }
                | Self::SteppingDown { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llama::server::ServerState;

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

    #[test]
    fn events_map_to_stages() {
        let progress = |stage: Option<&str>| ServerEvent::Progress {
            stage: stage.map(str::to_owned),
            value: Some(0.5),
        };
        assert_eq!(
            LoadStage::of_event(&progress(Some("text_model"))),
            Some(LoadStage::ReadingModel)
        );
        assert_eq!(
            LoadStage::of_event(&progress(Some("mmproj_model"))),
            Some(LoadStage::ReadingImageReader)
        );
        assert_eq!(
            LoadStage::of_event(&progress(Some("spec_model"))),
            Some(LoadStage::Loading)
        );
        assert_eq!(LoadStage::of_event(&progress(None)), None);
        assert_eq!(
            LoadStage::of_event(&ServerEvent::Restarting { attempt: 2, max: 3 }),
            Some(LoadStage::Restarting { attempt: 2, max: 3 })
        );
        assert_eq!(
            LoadStage::of_event(&ServerEvent::SteppedDown {
                from: BackendKind::Vulkan,
                to: BackendKind::Cpu,
                reason: LoadFailure::OutOfMemory,
            }),
            Some(LoadStage::SteppingDown {
                from: BackendKind::Vulkan,
                to: BackendKind::Cpu,
                reason: LoadFailure::OutOfMemory,
            })
        );
        for retry in [
            LoadStage::RetryingWithoutSaveMemory {
                backend: BackendKind::Metal,
            },
            LoadStage::RetryingWithLessGpu {
                backend: BackendKind::Vulkan,
            },
        ] {
            assert_eq!(
                LoadStage::of_event(&ServerEvent::Stage(retry.clone())),
                Some(retry)
            );
        }
        for quiet in [
            ServerEvent::State(ServerState::Starting),
            ServerEvent::State(ServerState::Ready),
            ServerEvent::Log("line".into()),
            ServerEvent::Offloaded {
                layers: 1,
                total: 2,
            },
            ServerEvent::Exited { code: Some(0) },
        ] {
            assert_eq!(LoadStage::of_event(&quiet), None, "{quiet:?}");
        }
    }

    #[test]
    fn adjustments_are_the_retries_and_step_downs() {
        let adjustments = [
            LoadStage::RetryingWithoutSaveMemory {
                backend: BackendKind::Vulkan,
            },
            LoadStage::RetryingWithLessGpu {
                backend: BackendKind::Vulkan,
            },
            LoadStage::SteppingDown {
                from: BackendKind::Vulkan,
                to: BackendKind::Cpu,
                reason: LoadFailure::Unknown,
            },
        ];
        for stage in &adjustments {
            assert!(stage.is_adjustment(), "{stage:?}");
        }
        for stage in [
            LoadStage::Starting,
            LoadStage::ReadingModel,
            LoadStage::ReadingImageReader,
            LoadStage::Loading,
            LoadStage::Restarting { attempt: 1, max: 3 },
        ] {
            assert!(!stage.is_adjustment(), "{stage:?}");
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
