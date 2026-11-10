//! The llama.cpp runtime: the pinned build, backends, probing, launch and lifecycle
//! (ARCHITECTURE.md 7).
//!
//! Every assumption about llama-server's command line, log lines, managed-child protocol
//! and HTTP fields lives in this module, so a bump of the pin in `llama.lock.json` has one
//! place to re-check (ARCHITECTURE.md 7.1, "No interface guarantee").

pub mod backend;
pub mod devices;
pub mod failure;
pub mod launch;
pub mod lock;
pub mod props;
pub mod protocol;
pub mod server;

mod lines;
