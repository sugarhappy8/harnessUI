//! Process creation and the llama.cpp runtime manager.
//!
//! This crate owns every child process the app starts (ARCHITECTURE.md 4.1) and the
//! lifecycle of `llama-server` (ARCHITECTURE.md 7):
//!
//! - [`spawn`](mod@spawn): the only module allowed to create processes. Every child lands in a Job
//!   Object with kill-on-close (Windows) or leads its own process group (Unix), starts with
//!   a scrubbed environment and never opens a console window.
//! - [`registry`]: a file of child PIDs used at the next launch to kill children that
//!   survived a crash of the previous run.
//! - [`reaper`]: the macOS crash-safety helper process (a no-op on Windows).
//! - [`llama`]: the pinned build ([`llama::lock`]), backend discovery and probing, launch
//!   arguments, the managed-child protocol, failure classification, `/props`, and the
//!   server lifecycle with its restart and step-down policy.
//!
//! The crate does not depend on Tauri, so all of this compiles and tests on any OS.

#![warn(missing_docs)]

pub mod llama;
pub mod packs;
pub mod reaper;
pub mod registry;
pub mod spawn;

pub use llama::backend::{
    Backend, BackendKind, BackendSource, RuntimeLayout, VULKAN_FALLBACK_DIR, VULKAN_RUNTIME_FILE,
    discover,
};
pub use llama::devices::{
    Device, ProbeError, ProbeResult, backend_of, parse_list_devices, pick_device, probe,
    probe_with_env,
};
pub use llama::failure::{LoadFailure, classify};
pub use llama::launch::{
    GpuLayers, LaunchConfig, build_args, build_env, free_loopback_port, random_api_key,
};
pub use llama::lock::{LlamaLock, PackFile, PackSpec};
pub use llama::props::{Modalities, Props, TemplateCaps};
pub use llama::protocol::{ChildState, LogEvent, LogInfo, parse_child_line, scan_log_line};
pub use llama::server::{
    Candidate, RunningServer, ServerEvent, ServerState, StartError, StartSpec, Supervisor,
    SupervisorExit, start, step_down,
};
pub use reaper::{ReaperClient, reaper_main};
pub use registry::{ChildRegistry, process_alive};
pub use spawn::{
    EnvSpec, ManagedChild, SpawnSpec, StdioMode, command_wrap, resolve_program, spawn,
};
