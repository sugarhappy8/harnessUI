//! rea's exact environment and command line (item5-spec.md WP9b `env.rs`, R8, R9, R10,
//! R20): `node <rea>/scripts/rea.mjs mcp` in `reverse/work`, with `HOME`, temp folders and
//! caches inside the reverse folder, the tool families, the chosen decompiler and nothing
//! else of the app's environment except a fixed few names.
//!
//! TODO(0.0.6): WP9b builds `run_config` (every variable of the spec, golden tests per
//! flavour and decompiler). This is WP9a's contract stub: the signature is frozen and it
//! returns a fixed sample configuration.

use std::path::PathBuf;
use std::time::Duration;

use crate::Decompiler;
use crate::install::Installation;
use crate::layout::ReverseLayout;
use crate::platform::{Os, Platform};
use crate::process::RunConfig;

/// Start limit on the first start after an install or repair (antivirus scans 10,585 new
/// files).
pub const FIRST_START_TIMEOUT: Duration = Duration::from_secs(180);
/// Start limit after that.
pub const START_TIMEOUT: Duration = Duration::from_secs(60);

/// Which system's variables and `PATH` to build (any flavour can be built on any OS, for
/// the golden tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    /// Windows.
    Windows,
    /// macOS.
    Macos,
    /// Linux.
    Linux,
}

impl Flavor {
    /// The flavour of a platform (`Linux` for anything that is not Windows or macOS).
    pub fn of(platform: Platform) -> Self {
        match platform.os() {
            Os::Windows => Flavor::Windows,
            Os::Macos => Flavor::Macos,
            Os::Linux | Os::Other => Flavor::Linux,
        }
    }
    /// The flavour of this build.
    pub fn current() -> Self {
        Self::of(Platform::current())
    }
}

/// How to start rea for this installation and decompiler.
///
/// TODO(0.0.6): WP9b. The stub sets only `REA_ANALYSIS_PROVIDER` (never `auto`).
pub fn run_config(
    layout: &ReverseLayout,
    installed: &Installation,
    decompiler: Option<Decompiler>,
    flavor: Flavor,
    first_start: bool,
) -> RunConfig {
    let _ = flavor;
    let provider = match decompiler {
        Some(Decompiler::Kuna) => "kuna",
        Some(Decompiler::Ghidra) | None => "ghidra",
    };
    RunConfig {
        program: installed.node().unwrap_or_else(|| PathBuf::from("node")),
        args: vec![
            installed
                .rea_entry
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "scripts/rea.mjs".into()),
            "mcp".into(),
        ],
        env: runtime::EnvSpec {
            inherit: Vec::new(),
            set: vec![("REA_ANALYSIS_PROVIDER".into(), provider.into())],
        },
        cwd: layout.work(),
        start_timeout: if first_start {
            FIRST_START_TIMEOUT
        } else {
            START_TIMEOUT
        },
    }
}
