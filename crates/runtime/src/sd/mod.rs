//! The stable-diffusion.cpp engine that makes Studio's local pictures and videos
//! (docs/dev/0.1.0/studio-local-design.md, L2).
//!
//! The pinned build lives in [`lock`]; [`engine`] finds `sd-cli` on this computer and
//! installs the Windows engine pack. Every assumption about `sd-cli` belongs in this
//! module, so a bump of the pin in `sd.lock.json` has one place to re-check, as for
//! llama.cpp (ARCHITECTURE.md 7.1).

pub mod engine;
pub mod lock;
