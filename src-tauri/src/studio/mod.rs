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
//! - [`manager`]: the runs: starting them under the spending rules, their queues (the cloud
//!   ones, and the one local lane), cancel, their audit rows, deleting them and the
//!   preferences they read; [`deps`] holds what the manager reaches the world through. The app makes one at start-up (`lib.rs`), which picks
//!   up the runs the app left unfinished and stops every run task at exit.
//! - [`commands`]: the read commands (the saved model lists, and fetching them or a picture's
//!   price when asked; the estimate and the exact request; the runs; a result's bytes; today's
//!   spending; the preferences) and the changing ones (start, cancel, *Get it anyway*; a
//!   video's poster frame; star, delete, export, show in folder; added pictures; *Send to
//!   chat*; the preferences).
//! - [`actions`]: what the gallery actions behind those commands do: export (with the run's
//!   settings beside the copy), a video's poster frame, *Send to chat* and *Use in prompt*.
//! - [`dropped`]: the files last dropped on the main window (Tauri's native drop), which the
//!   prompt can add as pictures, the webview never sending a path.
//! - [`engine_cmd`]: the Studio engine's state on this computer, and the Windows engine pack's
//!   download (with the first local model, or from its own button), cancel and remove.
//! - [`local_cmd`]: Get local models' commands: what this computer can run (the installed
//!   models and the recommended list, each with its fit, licences and parts), Hugging Face
//!   search and a repository's files, a fit at a size, download, delete and the note about
//!   local models.
//! - [`local_models`]: downloading a picture or video model to this computer, part by part,
//!   after the user acknowledges the local model notice (with the
//!   Windows engine pack on first use), and deleting one with the part files nothing else uses.
//! - [`local`]: the runs on this computer behind the same media provider as the cloud
//!   companies: the installed models with their controls, free; the exact command; the checks
//!   before the Studio engine starts; one engine process per run, its progress, Stop and its
//!   results.
//! - [`label`]: the AI label written into every local result (XMP in PNG and WebP, Matroska
//!   tags in WebM), never with the prompt.
//! - [`prompt_check`]: the small, always-on check that refuses a local prompt pairing a sexual
//!   word with a word for children.
//! - [`quit`]: closing the main window, or quitting from the app menu, asks first while a
//!   local job works.

pub mod actions;
pub mod catalog;
pub mod commands;
pub mod deps;
pub mod dropped;
pub mod engine_cmd;
pub mod errors;
pub mod gallery;
pub mod label;
pub mod local;
pub mod local_cmd;
pub mod local_models;
pub mod manager;
#[cfg(test)]
mod manager_tests;
pub mod prefs;
pub mod prompt_check;
pub mod quit;
pub mod spend;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
mod wire_tests;
