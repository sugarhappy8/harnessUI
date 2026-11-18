//! The stable-diffusion.cpp engine that makes Studio's local pictures and videos
//! (docs/dev/0.1.0/studio-local-design.md, L2).
//!
//! The pinned build lives in [`lock`]; [`engine`] finds `sd-cli` on this computer and
//! installs the Windows engine pack; [`devices`] reads its devices and finds llama.cpp's
//! graphics card among them; [`args`] builds its arguments from typed fields only;
//! [`progress`] reads its progress bar and stages from its output; [`failure`] names why
//! a run failed; [`run`] runs one job, with Stop, the out-of-memory retry and the stall, and
//! lists the engine's devices. `testing` is the fake engine the tests start. Every assumption
//! about `sd-cli` belongs in this module, so a bump of the pin in `sd.lock.json` has one place
//! to re-check, as for llama.cpp (ARCHITECTURE.md 7.1).

pub mod args;
pub mod devices;
pub mod engine;
pub mod failure;
pub mod lock;
pub mod progress;
pub mod run;
#[doc(hidden)]
pub mod testing;

pub use run::{RunEvent, RunLimits, RunOutput, list_devices, run_job};
