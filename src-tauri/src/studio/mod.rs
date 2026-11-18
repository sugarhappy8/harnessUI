//! Studio (0.1.0): pictures and video, made on this computer or by a cloud company.
//!
//! - [`catalog`]: each endpoint's saved picture and video model lists, fetched only when the
//!   user asks (Test, *Get the list*, *Update model lists*, picking a picture model).
//! - [`spend`]: when a cloud run asks before it is sent, and the daily spending limit,
//!   counted per local calendar day.
//! - [`gallery`]: the gallery folder: where results, sidecars, thumbnails and added pictures
//!   go, and the type check a file passes before it is kept.
//! - [`prefs`]: Settings → Studio, kept as one setting, and the rules a change must pass.
//! - [`errors`]: Studio's backend wording.
//! - [`manager`]: the runs: starting them under the spending rules, their queues, cancel,
//!   their audit rows, deleting them and the preferences they read; [`deps`] holds what the
//!   manager reaches the world through. The app makes one at start-up (`lib.rs`), which picks
//!   up the runs the app left unfinished and stops every run task at exit.
//! - [`commands`]: the read commands (the saved model lists, and fetching them or a picture's
//!   price when asked; the estimate and the exact request; the runs; a result's bytes; today's
//!   spending; the preferences) and the changing ones (start, cancel, *Get it anyway*; a
//!   video's poster frame; star, delete, export, show in folder; added pictures; *Send to
//!   chat*; the preferences).
//! - [`actions`]: what the gallery actions behind those commands do: export (with the run's
//!   settings beside the copy), a video's poster frame, *Send to chat* and *Use in prompt*.
//! - [`engine_cmd`]: the Studio engine's state on this computer, and the Windows engine pack's
//!   download (with the first local model, or from its own button), cancel and remove.
//! - [`local_models`]: downloading a picture or video model to this computer, part by part,
//!   once the note about local models and every licence of every part are accepted (with the
//!   Windows engine pack on first use), and deleting one with the part files nothing else uses.

pub mod actions;
pub mod catalog;
pub mod commands;
pub mod deps;
pub mod engine_cmd;
pub mod errors;
pub mod gallery;
pub mod local_models;
pub mod manager;
#[cfg(test)]
mod manager_tests;
pub mod prefs;
pub mod spend;
#[cfg(test)]
pub(crate) mod test_support;
