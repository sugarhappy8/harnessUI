//! The built-in tool server `reverse` (item5-spec.md WP9g `server.rs`, R10 to R13, R19,
//! R21): always the 37 allow-listed tools, `describe` through [`crate::describe`], and
//! `call` holding rea's lock while it checks the chat's folders, binding and result ids
//! again immediately before forwarding.
//!
//! One `Arc<ReverseServer>` per app: `toolset::build_host` (WP9g) builds it, registers it,
//! sets [`handle`] and manages [`ReverseHandle`] for the Settings commands (WP9d) and the
//! Tools panel (WP9h).
//!
//! TODO(0.0.6): WP9g builds `call`, the session and the result handling. This is WP9a's
//! contract stub: the signatures are frozen; `status` and `unavailable_reason` already
//! answer from the real install state; `call` refuses with a plain message.

use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use serde_json::{Map, Value};

use crate::allow::{self, ToolClass};
use crate::describe::{self, Availability, DescribeEnv};
use crate::install::Installer;
use crate::layout::ReverseLayout;
use crate::platform::{self, Platform};
use crate::process::{ReaProcess, RestartReason};
use crate::session::SessionView;
use crate::walk::RealWalkFs;
use crate::{AppleTools, Component, ComponentState, Decompiler, ReverseStatus};

/// The server's name in the registry and the Tools panel.
pub const SERVER_NAME: &str = "reverse";

/// The `reverse` server.
pub struct ReverseServer {
    layout: ReverseLayout,
    platform: Platform,
    installer: Arc<Installer>,
    process: Arc<ReaProcess>,
    decompiler: Mutex<Option<Decompiler>>,
}

/// Tauri managed state for the Settings commands.
pub struct ReverseHandle(pub Arc<ReverseServer>);

static HANDLE: OnceLock<Arc<ReverseServer>> = OnceLock::new();

/// The app's server, once `toolset::build_host` has set it.
pub fn handle() -> Option<Arc<ReverseServer>> {
    HANDLE.get().cloned()
}

/// Sets the app's server; only the first call counts (returns whether it did).
pub fn set_handle(server: Arc<ReverseServer>) -> bool {
    HANDLE.set(server).is_ok()
}

impl ReverseServer {
    /// The server for these folders on this platform (nothing starts).
    pub fn new(layout: ReverseLayout, platform: Platform) -> Arc<Self> {
        Arc::new(Self {
            installer: Arc::new(Installer::new(layout.clone(), platform)),
            process: Arc::new(ReaProcess::new(layout.clone(), platform)),
            layout,
            platform,
            decompiler: Mutex::new(None),
        })
    }

    /// A server over an installer built elsewhere (tests: a local lock and fetcher).
    pub fn with_installer(installer: Installer) -> Arc<Self> {
        let layout = installer.layout().clone();
        let platform = installer.platform();
        Arc::new(Self {
            process: Arc::new(ReaProcess::new(layout.clone(), platform)),
            installer: Arc::new(installer),
            layout,
            platform,
            decompiler: Mutex::new(None),
        })
    }

    /// Downloads, removal and status of the rows (WP9d's commands).
    pub fn installer(&self) -> &Arc<Installer> {
        &self.installer
    }

    /// The rea process.
    pub fn process(&self) -> &Arc<ReaProcess> {
        &self.process
    }

    /// The folders.
    pub fn layout(&self) -> &ReverseLayout {
        &self.layout
    }

    /// Re-reads the install state (after installs, removals, a switch, Apple's tools).
    ///
    /// TODO(0.0.6): WP9g. The stub has nothing cached.
    pub fn refresh(&self) {}

    /// The decompiler the owner chose (`reverseDecompiler`), whether installed or not.
    pub fn chosen_decompiler(&self) -> Option<Decompiler> {
        *self.decompiler.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Sets the owner's choice (WP9d reads the setting at start and on each switch).
    pub fn set_decompiler(&self, decompiler: Option<Decompiler>) {
        *self.decompiler.lock().unwrap_or_else(|e| e.into_inner()) = decompiler;
    }

    /// The decompiler rea uses: the choice, when it is installed and usable.
    pub fn decompiler(&self) -> Option<Decompiler> {
        self.chosen_decompiler()
            .filter(|d| self.installer.usable(d.component()).is_ok())
    }

    /// Stops rea for a Settings change (it starts again on the next call).
    pub async fn restart(&self, reason: RestartReason) {
        self.process.restart(reason).await;
    }

    /// What this computer offers right now.
    pub fn availability(&self) -> Availability {
        let rea = self.installer.state(Component::Rea);
        let needs_update = matches!(rea, ComponentState::NeedsUpdate { .. })
            || self.chosen_decompiler().is_some_and(|d| {
                matches!(
                    self.installer.state(d.component()),
                    ComponentState::NeedsUpdate { .. }
                )
            });
        Availability {
            rea_installed: !matches!(
                rea,
                ComponentState::NotInstalled
                    | ComponentState::Partial { .. }
                    | ComponentState::Downloading { .. }
            ),
            decompiler: self.decompiler(),
            apple_tools: platform::apple_tools(self.platform),
            needs_update,
            changed_tools: Vec::new(),
        }
    }

    /// Everything the tab shows.
    pub fn status(&self) -> ReverseStatus {
        ReverseStatus {
            components: self.installer.statuses(),
            decompiler: self.decompiler(),
            apple_tools: platform::apple_tools(self.platform),
            running: self.process.state(),
            cache_bytes: self.installer.cache_bytes(),
            memory_gb: platform::memory_gb(),
        }
    }

    /// Why a tool cannot run here yet (the Tools panel's mark), or `None` when it can.
    pub fn unavailable_reason(&self, tool: &str) -> Option<String> {
        let a = self.availability();
        if !a.rea_installed {
            return Some("Not set up yet. Set it up in Settings → Reverse engineering.".into());
        }
        if tool == "open_binary" {
            return None;
        }
        if a.apple_tools == AppleTools::Missing {
            return Some(
                "Needs Apple's command line tools. Install them in Settings → Reverse engineering."
                    .into(),
            );
        }
        if a.needs_update {
            return Some("Needs an update in Settings → Reverse engineering.".into());
        }
        if a.changed_tools.iter().any(|t| t == tool) {
            return Some(
                "Changed in the installed rea. Update rea in Settings → Reverse engineering."
                    .into(),
            );
        }
        let needs_decompiler = allow::class_of(tool) == Some(ToolClass::Session);
        if a.decompiler.is_none() && needs_decompiler {
            return Some(
                "Needs a decompiler. Install one in Settings → Reverse engineering.".into(),
            );
        }
        None
    }
}

#[async_trait]
impl BuiltinServer for ReverseServer {
    fn name(&self) -> &str {
        SERVER_NAME
    }

    fn tools(&self) -> Vec<ToolDef> {
        allow::tool_defs()
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let session = SessionView::default();
        let availability = self.availability();
        describe::describe(
            tool,
            args,
            chat,
            &DescribeEnv {
                fs: &RealWalkFs,
                session: &session,
                availability: &availability,
            },
        )
    }

    /// TODO(0.0.6): WP9g. The stub runs nothing.
    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &policy::Action,
        ctx: CallContext,
    ) -> ToolOutput {
        let _ = (tool, args, action, ctx);
        ToolOutput::error("Reverse engineering is not ready in this build yet.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_installed_marks_every_tool_not_set_up() {
        let dir = tempfile::tempdir().unwrap();
        let s = ReverseServer::new(ReverseLayout::new(dir.path()), Platform::LinuxX64);
        for tool in ["open_binary", "procedure_pseudo_code", "inspect_pe_imports"] {
            assert_eq!(
                s.unavailable_reason(tool).as_deref(),
                Some("Not set up yet. Set it up in Settings → Reverse engineering.")
            );
        }
        let status = s.status();
        assert_eq!(status.components.len(), 3);
        assert!(
            status
                .components
                .iter()
                .all(|c| c.state == ComponentState::NotInstalled)
        );
        assert_eq!(s.decompiler(), None);
        assert_eq!(s.name(), "reverse");
    }
}
