//! Reverse engineering with rea (item5-spec.md; spec.md Part 3b item 5).
//!
//! rea is an MCP server (`node scripts/rea.mjs mcp`) that the app downloads on request with
//! Node.js and a decompiler (Kuna or Ghidra with Java), like the ROCm pack, and runs behind
//! its own built-in tool server `reverse`: an allow-list of 37 static-analysis tools, the
//! path sandbox, `policy::evaluate` and the audit log, in every approval mode.
//!
//! Modules (work packages in brackets):
//! - [`lock`], [`install`], [`archive`], [`layout`], [`platform`], [`identity`],
//!   [`diskspace`]: the pinned downloads, verified and unpacked into the app's own folder,
//!   and what this computer offers [WP9a];
//! - [`env`], [`process`]: rea's environment and its one process [WP9b];
//! - [`allow`], [`describe`], [`walk`]: the allow-list, the describer and folder walks
//!   [WP9c];
//! - [`server`], [`session`], [`results`]: the `reverse` server, the chat's open file and
//!   result ids, and what comes back to the model [WP9g].
//!
//! This file holds the contract the other packages build against (WP9a, frozen): the
//! types below and the signatures in each module. Stubs return fixed sample data.

#![warn(missing_docs)]

pub mod allow;
pub mod archive;
pub mod describe;
pub mod diskspace;
pub mod env;
pub mod identity;
pub mod install;
pub mod layout;
pub mod lock;
pub mod platform;
pub mod process;
pub mod results;
pub mod server;
pub mod session;
pub mod walk;

use serde::{Deserialize, Serialize};

pub use allow::{AllowedTool, ToolClass};
pub use describe::{Availability, DescribeEnv};
pub use identity::FileIdentity;
pub use install::{
    DirFetch, Fetch, Installation, Installed, InstalledPart, Installer, NetFetch, ProgramFile,
    Progress,
};
pub use layout::ReverseLayout;
pub use lock::ReverseLock;
pub use platform::{Os, Platform};
pub use process::{ReaGuard, ReaProcess, RestartReason, RunConfig, Started};
pub use server::{ReverseHandle, ReverseServer, handle};
pub use session::{BindingView, ClosedReason, EvidenceView, SessionView};
pub use walk::{EntryKind, WalkFs};

/// A row of Settings → Reverse engineering.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Component {
    /// rea with Node.js.
    Rea,
    /// Kuna.
    Kuna,
    /// Ghidra with Java.
    Ghidra,
}

impl Component {
    /// Every row, in the tab's order.
    pub const ALL: [Component; 3] = [Component::Rea, Component::Kuna, Component::Ghidra];

    /// The decompiler this row provides, if any.
    pub fn decompiler(self) -> Option<Decompiler> {
        match self {
            Component::Rea => None,
            Component::Kuna => Some(Decompiler::Kuna),
            Component::Ghidra => Some(Decompiler::Ghidra),
        }
    }
}

/// The decompiler rea uses (the settings key `reverseDecompiler`: `"kuna"` or `"ghidra"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decompiler {
    /// Kuna.
    Kuna,
    /// Ghidra.
    Ghidra,
}

impl Decompiler {
    /// The Settings row that installs it.
    pub fn component(self) -> Component {
        match self {
            Decompiler::Kuna => Component::Kuna,
            Decompiler::Ghidra => Component::Ghidra,
        }
    }
    /// Its name as shown ("Kuna", "Ghidra").
    pub fn label(self) -> &'static str {
        match self {
            Decompiler::Kuna => "Kuna",
            Decompiler::Ghidra => "Ghidra",
        }
    }
}

/// The step a download is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStage {
    /// "Downloading… 23 MB of 80 MB".
    Downloading,
    /// "Checking…" (size and SHA-256).
    Checking,
    /// "Unpacking…".
    Unpacking,
}

/// Why an installed row cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BrokenWhy {
    /// A file the app or rea runs is gone or has another size ("perhaps removed by antivirus
    /// software"; Repair).
    Missing,
    /// A file is present but the system refuses to open it (antivirus, Smart App Control).
    Blocked,
}

/// Why an installed row needs an update before it is used again (R15).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum UpdateWhy {
    /// A part is below the lock's `minimum_version`: "This version of <part> has a known
    /// problem."
    SecurityFix {
        /// `rea`, `node`, `kuna`, `ghidra` or `java`.
        part: String,
    },
    /// The installed rea has no reviewed allow-list in this app: "This version of harnessUI
    /// needs rea <need>."
    NotAccepted {
        /// The pinned rea version.
        need: String,
    },
}

/// Where a row stands on this computer.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ComponentState {
    /// Nothing on disk.
    NotInstalled,
    /// A paused download ("Paused at 23 MB of 80 MB").
    Partial {
        /// Bytes already downloaded.
        bytes: u64,
    },
    /// A download in progress (this app run).
    Downloading {
        /// Bytes so far (downloaded, checked or unpacked, by stage).
        done: u64,
        /// Bytes in all.
        total: u64,
        /// The step.
        stage: DownloadStage,
    },
    /// Installed and usable.
    Installed,
    /// Installed but a file is missing or blocked.
    Broken {
        /// Which.
        why: BrokenWhy,
    },
    /// Installed but never started until updated.
    NeedsUpdate {
        /// Why.
        why: UpdateWhy,
    },
}

/// A part's name and version, for the row's detail line ("rea 6.9.0, with Node.js 24.22.3").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PartInfo {
    /// `rea`, `node`, `kuna`, `ghidra` or `java`.
    pub name: String,
    /// The installed version, or the pinned one when the part is not installed.
    pub version: String,
}

/// An update the row can take ("Update available: rea 6.9.0 → <new>. <n> MB.").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// The part `from` and `to` belong to: the row's main part (`rea`, `kuna`, `ghidra`)
    /// when it changes, else the first part that does (`node` or `java`), so an update of
    /// Node.js alone reads "Node.js 24.22.3 → 24.22.4", never "rea 6.9.0 → 6.9.0".
    pub part: String,
    /// Installed version of that part.
    pub from: String,
    /// Its pinned version.
    pub to: String,
    /// Bytes to download (every part that changes).
    pub download_bytes: u64,
}

/// A licence named in the download dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    /// The part it covers.
    pub part: String,
    /// SPDX-style name.
    pub name: String,
    /// Where the text lives (opened through the destination-confirm dialog).
    pub url: String,
}

/// One row's status for the tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComponentStatus {
    /// The row.
    pub component: Component,
    /// Whether this computer can install it.
    pub available: bool,
    /// Why not ("rea can't use Kuna on Windows yet."), when not.
    pub reason: Option<String>,
    /// Where it stands.
    pub state: ComponentState,
    /// Installed version of the row's main part (rea, Kuna or Ghidra).
    pub installed_version: Option<String>,
    /// Pinned version of the row's main part.
    pub pinned_version: String,
    /// Every part of the row.
    pub parts: Vec<PartInfo>,
    /// Bytes to download for this row's own parts (when rea is missing, a decompiler's
    /// download brings rea along: add rea's row).
    pub download_bytes: u64,
    /// Bytes on disk: measured when installed, else what the pinned parts unpack to.
    pub disk_bytes: u64,
    /// An update the installed copy can take.
    pub update: Option<UpdateInfo>,
    /// Licences of the row's parts.
    pub licenses: Vec<LicenseInfo>,
}

/// Whether rea is running (the tab's status line).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum RunState {
    /// "rea is not running."
    Stopped,
    /// Starting.
    Starting,
    /// "rea is running (<Kuna|Ghidra>)."
    Ready,
    /// "rea is working on a file…"
    Busy {
        /// The chat whose call runs.
        chat_id: i64,
    },
    /// "rea stopped with a problem…" with Show details.
    Failed {
        /// The masked, sanitised stderr tail.
        reason: String,
    },
}

/// Apple's command line tools (R7; macOS only).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppleTools {
    /// Not a Mac.
    NotNeeded,
    /// Installed.
    Present,
    /// Not installed: rea is never started.
    Missing,
}

/// Everything the tab shows.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReverseStatus {
    /// rea, Kuna and Ghidra, in that order.
    pub components: Vec<ComponentStatus>,
    /// The decompiler rea uses, if one is installed and chosen.
    pub decompiler: Option<Decompiler>,
    /// Apple's command line tools, checked fresh.
    pub apple_tools: AppleTools,
    /// Whether rea is running.
    pub running: RunState,
    /// Kuna's saved analysis, in bytes.
    pub cache_bytes: u64,
    /// This computer's memory in GiB (the 16 GB line), when known.
    pub memory_gb: Option<u32>,
}

/// Everything that can go wrong, typed so the tab can say it in plain words (WP9d maps each
/// to its exact text).
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ReverseError {
    /// The row is not offered on this computer.
    #[error("not available here: {reason}")]
    NotAvailable {
        /// The row's reason.
        reason: String,
    },
    /// The row is not installed.
    #[error("{component:?} is not installed")]
    NotInstalled {
        /// The row.
        component: Component,
    },
    /// rea is working, or a download of this row runs already (`force` false).
    #[error("busy")]
    Busy,
    /// The user cancelled.
    #[error("cancelled")]
    Cancelled,
    /// A download's size or SHA-256 differs from the lock: nothing was installed.
    #[error("{part} does not match the pinned copy")]
    Mismatch {
        /// The part.
        part: String,
    },
    /// The connection dropped after bytes arrived; the download resumes.
    #[error("the download of {part} was interrupted")]
    Interrupted {
        /// The part.
        part: String,
    },
    /// No connection could be made to the host at all.
    #[error("can't reach {host}")]
    CantReach {
        /// The host.
        host: String,
    },
    /// The host was reached but answered the download with an error status (404 when the
    /// file was withdrawn, 403 when it refused, 5xx when it has a problem). Suggested text
    /// for WP9d: "<host> didn't send the file (error <status>). Try again later. If it
    /// keeps failing, report it."
    #[error("{host} answered HTTP {status}")]
    ServerError {
        /// The host.
        host: String,
        /// The HTTP status.
        status: u16,
    },
    /// The URL or a redirect left the part's hosts.
    #[error("redirected to {host}, which is not trusted")]
    BlockedHost {
        /// The host.
        host: String,
    },
    /// Not enough free space, before anything was downloaded (or while writing).
    #[error("the disk is full ({need_bytes} bytes needed)")]
    DiskFull {
        /// Bytes needed in all (0 when the disk filled while writing).
        need_bytes: u64,
    },
    /// The reverse folder's path is too long for Ghidra's files on Windows.
    #[error("the folder path is too long ({chars} characters, at most {max})")]
    PathTooLong {
        /// Characters of the longest path.
        chars: usize,
        /// The limit.
        max: usize,
    },
    /// The reverse folder is not on a local NTFS or ReFS drive (Windows).
    #[error("the folder is not on an NTFS or ReFS drive")]
    NotNtfs,
    /// A file the app or rea runs is missing (Repair).
    #[error("part of {part} is missing")]
    Missing {
        /// The part.
        part: String,
    },
    /// A file is present but the system refuses to open it (antivirus; Repair).
    #[error("{part} is blocked by the system")]
    Blocked {
        /// The part.
        part: String,
    },
    /// The installed part must be updated first (R15).
    #[error("{part} needs an update")]
    NeedsUpdate {
        /// The part.
        part: String,
    },
    /// A download was damaged or unsafe and was not unpacked.
    #[error("{part} could not be unpacked: {message}")]
    Archive {
        /// The part.
        part: String,
        /// What was wrong.
        message: String,
    },
    /// A Mac without Apple's command line tools: rea is never started (R7).
    #[error("Apple's command line tools are missing")]
    AppleToolsMissing,
    /// A tool name outside the allow-list (refused before anything reaches rea).
    #[error("there is no tool named {tool}")]
    NotAllowed {
        /// The name.
        tool: String,
    },
    /// rea did not start (the masked stderr tail).
    #[error("rea did not start: {message}")]
    StartFailed {
        /// What happened.
        message: String,
    },
    /// A call took longer than its limit.
    #[error("the call took too long")]
    Timeout,
    /// rea stopped during a call.
    #[error("rea stopped")]
    Stopped,
    /// rea answered something that is not MCP.
    #[error("rea answered unexpectedly: {message}")]
    Protocol {
        /// What happened.
        message: String,
    },
    /// Another file-system error.
    #[error("{message}")]
    Io {
        /// What happened.
        message: String,
    },
}

impl From<std::io::Error> for ReverseError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::StorageFull
            || e.raw_os_error() == Some(28)
            || (cfg!(windows) && e.raw_os_error() == Some(112))
        {
            ReverseError::DiskFull { need_bytes: 0 }
        } else {
            ReverseError::Io {
                message: e.to_string(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shapes_for_the_tab() {
        let s = serde_json::to_value(ComponentState::Downloading {
            done: 1,
            total: 2,
            stage: DownloadStage::Checking,
        })
        .unwrap();
        assert_eq!(
            s,
            serde_json::json!({"kind":"downloading","done":1,"total":2,"stage":"checking"})
        );
        let s = serde_json::to_value(ComponentState::NeedsUpdate {
            why: UpdateWhy::SecurityFix {
                part: "ghidra".into(),
            },
        })
        .unwrap();
        assert_eq!(
            s,
            serde_json::json!({"kind":"needsUpdate","why":{"kind":"securityFix","part":"ghidra"}})
        );
        assert_eq!(
            serde_json::to_value(ComponentState::Broken {
                why: BrokenWhy::Blocked
            })
            .unwrap(),
            serde_json::json!({"kind":"broken","why":"blocked"})
        );
        assert_eq!(
            serde_json::to_value(ReverseError::CantReach {
                host: "nodejs.org".into()
            })
            .unwrap(),
            serde_json::json!({"kind":"cantReach","host":"nodejs.org"})
        );
        assert_eq!(
            serde_json::to_value(ReverseError::ServerError {
                host: "github.com".into(),
                status: 404
            })
            .unwrap(),
            serde_json::json!({"kind":"serverError","host":"github.com","status":404})
        );
        assert_eq!(
            serde_json::to_value(UpdateInfo {
                part: "node".into(),
                from: "24.22.3".into(),
                to: "24.22.4".into(),
                download_bytes: 5
            })
            .unwrap(),
            serde_json::json!({"part":"node","from":"24.22.3","to":"24.22.4","downloadBytes":5})
        );
        assert_eq!(
            serde_json::to_value(ReverseError::DiskFull { need_bytes: 9 }).unwrap(),
            serde_json::json!({"kind":"diskFull","needBytes":9})
        );
        assert_eq!(serde_json::to_value(Decompiler::Kuna).unwrap(), "kuna");
        assert_eq!(serde_json::to_value(Component::Ghidra).unwrap(), "ghidra");
        assert_eq!(
            serde_json::to_value(RunState::Busy { chat_id: 7 }).unwrap(),
            serde_json::json!({"kind":"busy","chatId":7})
        );
    }

    #[test]
    fn components_and_decompilers_map_both_ways() {
        for d in [Decompiler::Kuna, Decompiler::Ghidra] {
            assert_eq!(d.component().decompiler(), Some(d));
        }
        assert_eq!(Component::Rea.decompiler(), None);
    }
}
