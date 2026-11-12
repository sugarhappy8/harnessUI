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
//! - [`profile`]: the settings a check found to work best for one model on one backend,
//!   and how they are applied to a chat's requests.
//! - [`probes`]: the capability check (three probes under a few settings).
//! - [`tasks`] and [`verify`]: the benchmark tasks (shipped data) and their verification,
//!   run by the app after the model finished.
//! - [`bench`]: a benchmark run and its saved results.
//! - [`commands`]: the Tauri commands, the run's audit rows and progress events.

pub mod bench;
pub mod commands;
pub mod harness;
pub mod probes;
pub mod profile;
pub mod sandbox;
pub mod tasks;
pub mod verify;
pub mod workspace;

#[cfg(test)]
mod testing;
