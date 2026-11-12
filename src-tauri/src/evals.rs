//! Tested settings per model, and coding benchmarks (0.0.4).
//!
//! Both are built on one headless harness ([`harness`]): the real agent loop
//! (`agent::turn::run_turn`) against the loaded model or a configured endpoint, with the
//! real file tools (and, for benchmarks, the terminal) working in a temporary folder made
//! for the run. No chat is involved and no approval card can appear: every call is
//! decided by the fixed rule in [`sandbox`], and what that rule does not let run is
//! refused and counted as blocked.
//!
//! - [`sandbox`]: the rule, and the approver that answers no.
//! - [`workspace`]: the run's folder, made under the system's temporary folder with a
//!   random name and removed when the run ends, however it ends.
//! - [`harness`]: one headless turn, with everything it did recorded.

// Wired up piece by piece while the feature is being built.
#![allow(dead_code)]

pub mod sandbox;

#[cfg(test)]
mod testing;
