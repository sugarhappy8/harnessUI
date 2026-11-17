//! Studio (0.1.0): pictures and video, made on this computer or by a cloud company.
//!
//! - [`spend`]: when a cloud run asks before it is sent, and the daily spending limit,
//!   counted per local calendar day.
//! - [`gallery`]: the gallery folder: where results, sidecars, thumbnails and added pictures
//!   go, and the type check a file passes before it is kept.

pub mod gallery;
pub mod spend;
#[cfg(test)]
pub(crate) mod test_support;
