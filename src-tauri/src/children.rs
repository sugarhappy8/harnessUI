//! Crash safety for child processes (ARCHITECTURE.md 4.1): a file of child PIDs swept
//! at the next start, and on macOS a reaper process that kills registered process
//! groups if the app dies. The reaper registers every child started through
//! `runtime::spawn` on its own once it is installed globally.

use std::path::Path;
use std::sync::Mutex;

use runtime::ChildRegistry;
use tauri::{AppHandle, Manager};

/// Hidden command-line flag that turns the app binary into the reaper.
pub const REAPER_FLAG: &str = "--internal-reaper";

pub struct Children {
    registry: Option<ChildRegistry>,
    /// The llama-server pid currently recorded in the registry.
    tracked: Mutex<Option<u32>>,
}

impl Children {
    /// Sweeps children left by a crashed previous run, then starts the reaper.
    /// Must run inside the Tokio runtime (Tauri's setup hook does).
    pub fn init(app: &AppHandle) -> Self {
        let registry = app
            .path()
            .app_local_data_dir()
            .ok()
            .map(|d| {
                let _ = std::fs::create_dir_all(&d);
                d.join("children.json")
            })
            .map(ChildRegistry::open);
        if let Some(reg) = registry.as_ref() {
            let killed = reg.sweep_stale();
            if !killed.is_empty() {
                tracing::warn!(
                    ?killed,
                    "stopped child processes left over from a previous run"
                );
            }
        }
        #[cfg(unix)]
        match std::env::current_exe() {
            Ok(exe) => match runtime::ReaperClient::start(&exe, REAPER_FLAG) {
                Ok(client) => {
                    if client.install_global().is_err() {
                        tracing::warn!("a reaper was already installed");
                    }
                }
                Err(e) => tracing::warn!("could not start the reaper: {e}"),
            },
            Err(e) => tracing::warn!("could not find the app executable for the reaper: {e}"),
        }
        Self {
            registry,
            tracked: Mutex::new(None),
        }
    }
}

/// Records the running llama-server so a crashed app's successor can stop it.
pub fn track(app: &AppHandle, pid: u32, exe: &Path) {
    let Some(children) = app.try_state::<Children>() else {
        return;
    };
    let Some(reg) = children.registry.as_ref() else {
        return;
    };
    if let Ok(mut t) = children.tracked.lock() {
        if let Some(old) = t.take() {
            let _ = reg.unregister(old);
        }
        if reg.register(pid, exe).is_ok() {
            *t = Some(pid);
        }
    }
}

/// Forgets the llama-server once it has stopped.
pub fn untrack(app: &AppHandle) {
    let Some(children) = app.try_state::<Children>() else {
        return;
    };
    let Some(reg) = children.registry.as_ref() else {
        return;
    };
    if let Ok(mut t) = children.tracked.lock()
        && let Some(old) = t.take()
    {
        let _ = reg.unregister(old);
    }
}
