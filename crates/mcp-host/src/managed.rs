//! A stdio MCP server the app owns and runs itself (rea; item5-spec.md WP9b), not one of the
//! user's servers: a fixed program, argument list, exact environment and working folder,
//! started through `runtime::spawn`, and calls limited to a typed allow-list that is checked
//! before anything is written to the server's standard input.
//!
//! TODO(0.0.6): WP9b builds this module (spawn, handshake, paged `tools/list`, cancellable
//! calls with a grace period, the stderr log). This is WP9a's contract stub: the types and
//! signatures are frozen; `start` and `call` return fixed sample data and start nothing.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

use crate::builtin::{ProgressFn, ToolOutput};
use crate::error::HostError;

/// How to start a managed server.
#[derive(Clone, Debug)]
pub struct ManagedSpec {
    /// Name used in logs (`reverse`).
    pub name: String,
    /// Absolute path of the program.
    pub program: PathBuf,
    /// Fixed argument list.
    pub args: Vec<String>,
    /// The whole environment (nothing else is inherited).
    pub env: runtime::EnvSpec,
    /// Working folder.
    pub cwd: PathBuf,
    /// Where stderr is logged (size-capped), if anywhere.
    pub log_file: Option<PathBuf>,
    /// How long the start and handshake may take.
    pub start_timeout: Duration,
}

/// The tool names a call may use; any other name is refused before the server hears of it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolAllowList(BTreeSet<String>);

impl ToolAllowList {
    /// An allow-list of exactly these names.
    pub fn new<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self(names.into_iter().map(Into::into).collect())
    }
    /// Whether `name` may be called.
    pub fn contains(&self, name: &str) -> bool {
        self.0.contains(name)
    }
    /// The names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
    /// How many names.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether it allows nothing.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// One managed server process.
pub struct ManagedServer {
    spec: ManagedSpec,
    running: AtomicBool,
}

impl ManagedServer {
    /// A server that is not started yet.
    pub fn new(spec: ManagedSpec) -> Self {
        Self {
            spec,
            running: AtomicBool::new(false),
        }
    }

    /// How it is started.
    pub fn spec(&self) -> &ManagedSpec {
        &self.spec
    }

    /// Starts the server and returns its tools (name and input schema).
    ///
    /// TODO(0.0.6): WP9b. The stub starts nothing and lists no tools.
    pub async fn start(&self) -> Result<Vec<(String, Value)>, HostError> {
        self.running.store(true, Ordering::SeqCst);
        Ok(Vec::new())
    }

    /// Calls `name`, which must be in `allow`; progress only keeps the call alive.
    ///
    /// TODO(0.0.6): WP9b. The stub refuses names outside `allow` and answers others with a
    /// fixed sample result.
    pub async fn call(
        &self,
        allow: &ToolAllowList,
        name: &str,
        args: Map<String, Value>,
        timeout: Duration,
        cancel: &CancellationToken,
        progress: Option<ProgressFn>,
    ) -> Result<ToolOutput, HostError> {
        let _ = (args, timeout, progress);
        if !allow.contains(name) {
            return Err(HostError::NotFound(format!(
                "there is no tool named {name}"
            )));
        }
        if cancel.is_cancelled() {
            return Ok(ToolOutput::error("Stopped."));
        }
        Ok(ToolOutput::text(format!("Sample result of {name}.")))
    }

    /// Closes standard input, waits up to `grace`, then ends the whole process tree.
    ///
    /// TODO(0.0.6): WP9b.
    pub async fn stop(&self, grace: Duration) {
        let _ = grace;
        self.running.store(false, Ordering::SeqCst);
    }

    /// The last lines the server wrote to stderr.
    pub fn stderr_tail(&self) -> Vec<String> {
        Vec::new()
    }

    /// Whether the process runs.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_name_outside_the_allow_list_is_refused() {
        let server = ManagedServer::new(ManagedSpec {
            name: "reverse".into(),
            program: PathBuf::from("/nonexistent/node"),
            args: vec!["rea.mjs".into(), "mcp".into()],
            env: runtime::EnvSpec::default(),
            cwd: std::env::temp_dir(),
            log_file: None,
            start_timeout: Duration::from_secs(60),
        });
        let allow = ToolAllowList::new(["open_binary"]);
        let e = server
            .call(
                &allow,
                "start_gdb_session",
                Map::new(),
                Duration::from_secs(1),
                &CancellationToken::new(),
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(e, HostError::NotFound(_)), "{e}");
        assert!(allow.contains("open_binary") && allow.len() == 1);
    }
}
