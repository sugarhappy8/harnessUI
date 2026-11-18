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
//!   and their audit rows; [`deps`] holds what the manager reaches the world through.

pub mod catalog;
pub mod deps;
pub mod errors;
pub mod gallery;
pub mod manager;
#[cfg(test)]
mod manager_tests;
pub mod prefs;
pub mod spend;
#[cfg(test)]
pub(crate) mod test_support;
