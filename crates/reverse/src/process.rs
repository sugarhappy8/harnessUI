//! The one rea process of the app (item5-spec.md WP9b `process.rs`, R9, R12): every call
//! holds its lock ([`ReaGuard`]), so calls run one at a time and the checks of WP9g run
//! while nothing else can reach rea. Started on first use, stopped after 15 minutes
//! without a call, on [`ReaProcess::restart`] and when the app quits.
//!
//! TODO(0.0.6): WP9b builds this (refusals before a start: a Mac without Apple's tools, a
//! missing, broken or not-accepted install; `reverse/tmp` emptied at each start; the idle
//! stop; `Failed` with the masked stderr tail). This is WP9a's contract stub: the types and
//! signatures are frozen; it starts nothing and answers with fixed sample data.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use mcp_host::{ProgressFn, ToolAllowList, ToolOutput};
use serde_json::{Map, Value};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tokio_util::sync::CancellationToken;

use crate::layout::ReverseLayout;
use crate::platform::Platform;
use crate::{ReverseError, RunState};

/// Stopped after this long without a call.
pub const IDLE_STOP: Duration = Duration::from_secs(15 * 60);
/// Grace given to rea to end Kuna or Java itself before its tree is killed.
pub const STOP_GRACE: Duration = Duration::from_secs(3);

/// How to start rea (built by [`crate::env::run_config`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunConfig {
    /// The pack's `node`.
    pub program: PathBuf,
    /// `[<rea>/scripts/rea.mjs, "mcp"]`.
    pub args: Vec<String>,
    /// The whole environment.
    pub env: runtime::EnvSpec,
    /// `reverse/work`.
    pub cwd: PathBuf,
    /// 180 s on the first start after an install or repair, 60 s after.
    pub start_timeout: Duration,
}

/// Why rea is restarted (each ends every chat's open file).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartReason {
    /// The owner chose the other decompiler.
    DecompilerChanged,
    /// rea or a decompiler was updated or repaired.
    Updated,
    /// rea or a decompiler was removed.
    Removed,
    /// The app quits.
    Quit,
}

/// What [`ReaGuard::ensure_started`] found.
#[derive(Clone, Debug, PartialEq)]
pub struct Started {
    /// Grows at each start of rea.
    pub generation: u64,
    /// rea's `tools/list`: name and input schema, for the live check (WP9c).
    pub tools: Vec<(String, Value)>,
    /// True when this call started rea.
    pub fresh: bool,
}

#[derive(Default)]
struct Inner {
    started: bool,
}

/// The one rea process.
pub struct ReaProcess {
    layout: ReverseLayout,
    platform: Platform,
    inner: Arc<Mutex<Inner>>,
    generation: AtomicU64,
}

/// Exclusive use of rea for one call (and the app's own housekeeping calls with it).
pub struct ReaGuard {
    generation: u64,
    inner: OwnedMutexGuard<Inner>,
}

impl ReaProcess {
    /// Nothing runs until the first call.
    pub fn new(layout: ReverseLayout, platform: Platform) -> Self {
        Self {
            layout,
            platform,
            inner: Arc::new(Mutex::new(Inner::default())),
            generation: AtomicU64::new(0),
        }
    }

    /// The folders rea runs in.
    pub fn layout(&self) -> &ReverseLayout {
        &self.layout
    }

    /// The platform.
    pub fn platform(&self) -> Platform {
        self.platform
    }

    /// Waits for rea to be free and holds it.
    pub async fn lock(&self) -> ReaGuard {
        let inner = self.inner.clone().lock_owned().await;
        ReaGuard {
            generation: self.generation(),
            inner,
        }
    }

    /// Grows at each start of rea.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// For the tab's status line.
    ///
    /// TODO(0.0.6): WP9b. The stub is always stopped.
    pub fn state(&self) -> RunState {
        RunState::Stopped
    }

    /// Stops rea (it starts again on the next call).
    ///
    /// TODO(0.0.6): WP9b.
    pub async fn restart(&self, reason: RestartReason) {
        let _ = reason;
        self.stop(STOP_GRACE).await;
    }

    /// Closes rea's standard input, waits up to `grace`, then ends its tree.
    ///
    /// TODO(0.0.6): WP9b.
    pub async fn stop(&self, grace: Duration) {
        let _ = grace;
        let mut inner = self.inner.lock().await;
        inner.started = false;
    }
}

impl ReaGuard {
    /// Starts rea with `config` unless it runs already.
    ///
    /// TODO(0.0.6): WP9b. The stub starts nothing and lists no tools.
    pub async fn ensure_started(&mut self, config: RunConfig) -> Result<Started, ReverseError> {
        let _ = config;
        let fresh = !self.inner.started;
        self.inner.started = true;
        Ok(Started {
            generation: self.generation + u64::from(fresh),
            tools: Vec::new(),
            fresh,
        })
    }

    /// Calls `name`, which must be in `allow` (refused before anything reaches rea).
    ///
    /// TODO(0.0.6): WP9b. The stub answers allowed names with a fixed sample result.
    pub async fn call(
        &mut self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<ToolOutput, ReverseError> {
        let _ = (args, timeout, progress);
        if !allow.contains(name) {
            return Err(ReverseError::NotAllowed {
                tool: name.to_string(),
            });
        }
        if cancel.is_cancelled() {
            return Err(ReverseError::Cancelled);
        }
        Ok(ToolOutput::text(format!("Sample result of {name}.")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_guard_refuses_names_outside_the_allow_list() {
        let p = ReaProcess::new(ReverseLayout::new("/tmp/reverse"), Platform::LinuxX64);
        let mut g = p.lock().await;
        let allow = ToolAllowList::new(["open_binary"]);
        let e = g
            .call(
                &allow,
                "instrument_with_frida",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            e,
            ReverseError::NotAllowed {
                tool: "instrument_with_frida".into()
            }
        );
    }
}
