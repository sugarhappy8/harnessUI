//! Turning a reverse tool call into a policy action (item5-spec.md WP9c `describe.rs`,
//! section 6): paths through the sandbox (the whole computer in Full access, the app's
//! folders never), folders through [`crate::walk`], session calls against the chat's
//! binding, result ids against the chat's ledger, and what this computer offers first.
//! Opening or checking a file: scope `FileAnalysis { folder }`; session calls:
//! `FileAnalysisSession`. No side effects: `describe` only reads (the file system, the
//! session snapshot), and refusing a session call changes nothing (closing rea's file is
//! `call`'s job, WP9g).
//!
//! What `call` (WP9g) reads back is the action's [`Payload`]: the exact arguments rea
//! receives (canonical paths, the fixed `detail`), and what was checked (the file's identity
//! or "folder", the binding or the result id as seen here), so it can check again
//! immediately before forwarding that nothing changed.

use mcp_host::{ChatContext, Described};
use policy::path::{CheckedPath, Flavor, PathError, PathSandbox};
use policy::{Action, Risk, Scope};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::allow::{self, AllowedTool, TargetKind, ToolClass};
use crate::platform::{self, Os, Platform};
use crate::session::{ClosedReason, SessionView};
use crate::walk::{self, EntryKind, Hops, WalkFs, shown};
use crate::{AppleTools, Component, Decompiler, FileIdentity};

/// The server's name on every action.
const SERVER: &str = "reverse";

/// The `kind` of every reverse [`Payload`].
pub const PAYLOAD_KIND: &str = "reverse";

/// What the model reads when a call is refused (section 6, exact), and the few lines WP9c
/// adds for cases section 6 does not name (marked).
pub mod messages {
    /// An open or a file check without its path.
    pub const NO_PATH: &str = "Give the file's path.";
    /// Outside the chat's folders.
    pub const OUTSIDE: &str =
        "That file is outside this chat's folders. Ask the user to add its folder to this chat.";
    /// Inside the app's own folders (or a system key store).
    pub const APP_FOLDERS: &str =
        "That file is in harnessUI's own folders, which tools can't reach.";
    /// A link out of reach inside a folder (the link's path follows, then a full stop).
    pub const LINK_OUT: &str = "That folder contains a link to a place this chat can't reach: ";
    /// A folder over the walk's limits.
    pub const TOO_LARGE: &str =
        "This folder is too large for rea to check. Point at the program file or a smaller folder.";
    /// A device or pipe inside a folder (its path follows, then a full stop).
    pub const DEVICE_IN_FOLDER: &str =
        "That folder contains a device or pipe, which rea doesn't read: ";
    /// A session call with no file open for this chat.
    pub const NO_FILE_OPEN: &str =
        "No file is open in rea for this chat. Open one with open_binary first.";
    /// The open file's folder lost its approval.
    pub const NO_LONGER_REACHABLE: &str = "The file open in rea for this chat is no longer in this chat's folders. Open a file that is.";
    /// Another chat opened a file.
    pub const CLOSED_OTHER_CHAT: &str =
        "rea closed this file because another chat opened one. Open it again with open_binary.";
    /// The decompiler was switched or updated.
    pub const CLOSED_SETTINGS: &str = "rea closed this file because the decompiler was changed or updated in Settings. Open it again with open_binary.";
    /// A result id this chat does not hold.
    pub const UNKNOWN_ID: &str = "That id isn't one of this chat's rea results, or rea no longer has it (rea was restarted or another chat opened a file). Run the step that gave it again.";
    /// No decompiler, Windows (Ghidra is the only one offered).
    pub const NO_DECOMPILER_GHIDRA: &str = "No decompiler is installed. The user can install Ghidra in Settings → Reverse engineering.";
    /// No decompiler, where both are offered.
    pub const NO_DECOMPILER_BOTH: &str = "No decompiler is installed. The user can install Kuna or Ghidra in Settings → Reverse engineering.";
    /// No decompiler, a Mac without Ghidra's Mac decompiler.
    pub const NO_DECOMPILER_KUNA: &str =
        "No decompiler is installed. The user can install Kuna in Settings → Reverse engineering.";
    /// A Mac without Apple's command line tools.
    pub const APPLE_TOOLS: &str = "On this Mac rea needs Apple's command line tools first. The user can install them in Settings → Reverse engineering.";
    /// rea or its decompiler below the lock's minimum, or not accepted.
    pub const NEEDS_UPDATE: &str = "This version of rea (or its decompiler) needs an update before it can be used. The user can update it in Settings → Reverse engineering.";
    /// A tool whose schema differs in the installed rea.
    pub const CHANGED_TOOL: &str = "This rea tool changed in the installed version of rea and is switched off. The user can update rea in Settings → Reverse engineering.";
    /// (WP9c) rea is not installed.
    pub const NOT_SET_UP: &str = "Reverse engineering is not set up on this computer yet. The user can set it up in Settings → Reverse engineering.";
    /// (WP9c) A network or device path, refused before anything is looked up.
    pub const NETWORK: &str = "That file is on a network or device path, which rea doesn't open. Ask the user to copy it into one of this chat's folders.";
    /// (WP9c) The path itself is a device, pipe or socket (its path follows, then a full stop).
    pub const DEVICE: &str = "That is a device or pipe, which rea doesn't read: ";
    /// (WP9c) A folder the walk could not read (its path follows, then a full stop).
    pub const UNREADABLE: &str =
        "rea can't check everything in that folder, so it won't look inside it: ";
    /// (WP9c) Nothing at the path (the path follows, then a full stop).
    pub const NOT_FOUND: &str = "There is no file at ";
    /// (WP9c) A folder where a file is needed (the path follows, then a full stop).
    pub const WANTS_FILE: &str = "Give the path of a file, not a folder: ";
    /// (WP9c) A folder that is not a Mac app bundle, for `open_binary` and
    /// `inspect_electron_fuses` (the path follows, then a full stop).
    pub const WANTS_PROGRAM: &str =
        "Give the path of a program or library file (or a Mac .app bundle), not a folder: ";
    /// (WP9c) A file where an app's folder is needed (the path follows, then a full stop).
    pub const WANTS_FOLDER: &str = "Give the path of the app's folder, not a file: ";
    /// (WP9c) A file that cannot be read (the path follows, then a full stop).
    pub const UNREADABLE_FILE: &str = "rea can't read that file: ";
    /// (WP9c) A result id whose file left the chat's folders.
    pub const EVIDENCE_GONE: &str =
        "That result came from a file that is no longer in this chat's folders.";
    /// (WP9c) A path inside the view of `inspect_analysis_view`.
    pub const VIEW_PATH: &str = "The view can't hold a file path. Use the view rea's earlier result suggests, without paths.";
    /// (WP9c) A view over 4 KB.
    pub const VIEW_TOO_LARGE: &str = "The view is too large. Ask for a smaller part.";
    /// (WP9c) A `source_paths` entry that is not a plain relative path (it follows, then a
    /// full stop).
    pub const SOURCE_PATH: &str =
        "source_paths must be relative to the app's folder, without .. or a drive: ";
    /// (WP9c, review round 1) A folder holding harnessUI's own folders or a key store (the
    /// folder inside follows, then a full stop).
    pub const HOLDS_APP_FOLDERS: &str =
        "That folder contains harnessUI's own folders, which tools can't reach: ";
    /// (WP9c, review round 1) A tool rea cannot run with Kuna, where Ghidra is offered.
    pub const KUNA_CANNOT: &str = "rea can't do this with Kuna. With Ghidra it can: the user can install Ghidra or switch to it in Settings → Reverse engineering. The other rea tools still work on the open file.";
    /// (WP9c, review round 1) A tool rea cannot run with Kuna, on a Mac (no Ghidra there in
    /// this version).
    pub const KUNA_CANNOT_MAC: &str = "rea can't do this with Kuna, and Ghidra isn't available for Macs in this version. The other rea tools still work on the open file.";
}

use messages as m;

/// What this computer offers right now, for the availability messages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Availability {
    /// rea is installed (whatever its state).
    pub rea_installed: bool,
    /// The decompiler rea uses, if one is installed and chosen.
    pub decompiler: Option<Decompiler>,
    /// Apple's command line tools (checked fresh on a Mac).
    pub apple_tools: AppleTools,
    /// rea or the chosen decompiler needs an update (R15).
    pub needs_update: bool,
    /// Allow-listed tools whose schema changed in the installed rea (switched off).
    pub changed_tools: Vec<String>,
}

/// What `describe` reads.
pub struct DescribeEnv<'a> {
    /// The file system (fake in tests).
    pub fs: &'a dyn WalkFs,
    /// The session snapshot.
    pub session: &'a SessionView,
    /// What this computer offers.
    pub availability: &'a Availability,
}

/// What an Open or a file check names, as checked.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetSeen {
    /// The property that held the path (`path` or `input_path`).
    pub property: String,
    /// The canonical path (the one in [`Payload::args`]).
    pub canonical: String,
    /// The approved folder holding it: the grant's folder.
    pub root: String,
    /// The file's identity; `None` for a folder (walked again in `call`).
    pub identity: Option<FileIdentity>,
    /// Whether it is a folder (an app folder or a `.app` bundle).
    pub folder: bool,
}

/// The chat's open file, as `describe` saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BindingSeen {
    /// The chat that opened it.
    pub chat_id: i64,
    /// Its canonical path.
    pub canonical: String,
    /// Its identity when opened.
    pub identity: FileIdentity,
    /// The decompiler it was opened with.
    pub decompiler: Decompiler,
    /// rea's generation when opened.
    pub generation: u64,
    /// The open counter.
    pub open_seq: u64,
}

/// A result id's owner, as `describe` saw it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceSeen {
    /// The id (`ev_…`).
    pub id: String,
    /// The chat whose result showed it first.
    pub chat_id: i64,
    /// The store epoch it was seen in.
    pub store_epoch: u64,
    /// The canonical path it came from.
    pub canonical: String,
}

/// The payload of every reverse action: what `call` forwards and checks again.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Payload {
    /// Always [`PAYLOAD_KIND`].
    pub kind: String,
    /// rea's tool name.
    pub tool: String,
    /// Exactly what rea receives: the model's arguments (only the schema's), every path
    /// replaced by its canonical form, and the fixed `detail` when the tool has one.
    pub args: Map<String, Value>,
    /// Open and file checks: the path as checked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetSeen>,
    /// Open: the decompiler rea opens it with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decompiler: Option<Decompiler>,
    /// Session tools: the binding as seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<BindingSeen>,
    /// `inspect_analysis_view`: the id as seen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<EvidenceSeen>,
    /// The store epoch when described (session tools and the view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_epoch: Option<u64>,
}

impl Payload {
    /// The payload of a reverse action for `tool`, or `None` when the action is not one
    /// `describe` made for that tool (a refusal has none).
    pub fn of(action: &Action, tool: &str) -> Option<Payload> {
        if action.server != SERVER || action.tool != tool || action.hard_limit.is_some() {
            return None;
        }
        let p: Payload = serde_json::from_value(action.payload.clone()).ok()?;
        (p.kind == PAYLOAD_KIND && p.tool == tool).then_some(p)
    }
}

/// The "no decompiler" message for this computer: it names only what this computer can
/// install (Ghidra on Windows; Kuna on a Mac, while Ghidra's download has no Mac
/// decompiler; both on Linux).
pub fn no_decompiler_message(platform: Platform) -> &'static str {
    let lock = crate::lock::embedded();
    let offered = |c| platform::availability(lock, c, platform).0;
    match (offered(Component::Kuna), offered(Component::Ghidra)) {
        (true, true) => m::NO_DECOMPILER_BOTH,
        (false, true) => m::NO_DECOMPILER_GHIDRA,
        (true, false) => m::NO_DECOMPILER_KUNA,
        (false, false) => match platform.os() {
            Os::Windows => m::NO_DECOMPILER_GHIDRA,
            Os::Macos => m::NO_DECOMPILER_KUNA,
            Os::Linux | Os::Other => m::NO_DECOMPILER_BOTH,
        },
    }
}

/// Session tools that rea 6.9.0 cannot run with Kuna 1.990: its Kuna provider does not
/// declare them, so rea answers every call with `capability_unavailable` (checked with the
/// real rea and Kuna on `hello-linux-x64` in WP9c's review). Revisit with each rea pin.
pub const KUNA_CANNOT_RUN: [&str; 5] = [
    "procedure_info",
    "procedure_references",
    "list_names",
    "list_segments",
    "find_xrefs_to_name",
];

/// What the model reads for a tool rea cannot run with Kuna, on this computer.
pub fn kuna_cannot_message(platform: Platform) -> &'static str {
    if platform::availability(crate::lock::embedded(), Component::Ghidra, platform).0 {
        m::KUNA_CANNOT
    } else {
        m::KUNA_CANNOT_MAC
    }
}

/// Why `tool` cannot run on this computer now, checked before anything else (section 6).
pub fn unavailable(tool: &AllowedTool, a: &Availability, platform: Platform) -> Option<String> {
    let why = if !a.rea_installed {
        m::NOT_SET_UP
    } else if a.apple_tools == AppleTools::Missing {
        m::APPLE_TOOLS
    } else if a.needs_update {
        m::NEEDS_UPDATE
    } else if a.changed_tools.iter().any(|t| t == tool.name) {
        m::CHANGED_TOOL
    } else if a.decompiler.is_none() && matches!(tool.class, ToolClass::Open | ToolClass::Session) {
        no_decompiler_message(platform)
    } else {
        return None;
    };
    Some(why.to_string())
}

/// The action for one reverse tool call, or why it cannot be one, on this computer.
pub fn describe(
    tool: &str,
    args: &Map<String, Value>,
    chat: &ChatContext,
    env: &DescribeEnv<'_>,
) -> Described {
    describe_on(Platform::current(), tool, args, chat, env)
}

/// [`describe`] as on `platform` (its path rules and messages), for tests of every system.
pub fn describe_on(
    platform: Platform,
    tool: &str,
    args: &Map<String, Value>,
    chat: &ChatContext,
    env: &DescribeEnv<'_>,
) -> Described {
    let Some(t) = allow::get(tool) else {
        return Described::Invalid(format!(
            "there is no tool named {}",
            policy::sanitize::one_line(tool, 80)
        ));
    };
    if let Some(why) = unavailable(t, env.availability, platform) {
        return Described::Invalid(why);
    }
    for p in t.paths {
        if args
            .get(*p)
            .and_then(Value::as_str)
            .is_none_or(|s| s.trim().is_empty())
        {
            return Described::Invalid(m::NO_PATH.into());
        }
    }
    if let Err(problems) = allow::validate(tool, args) {
        return Described::Invalid(format!(
            "The arguments do not fit {tool}: {}",
            problems.join("; ")
        ));
    }
    let flavor = match platform.os() {
        Os::Windows => Flavor::Windows,
        _ => Flavor::Posix,
    };
    let cx = Cx {
        t,
        args,
        chat,
        env,
        flavor,
        platform,
    };
    match t.class {
        ToolClass::Open | ToolClass::FileCheck => cx.path_call(),
        ToolClass::Session => cx.session_call(),
        ToolClass::View => cx.view_call(),
    }
}

/// One call being described.
struct Cx<'a, 'b> {
    t: &'static AllowedTool,
    args: &'a Map<String, Value>,
    chat: &'a ChatContext,
    env: &'a DescribeEnv<'b>,
    flavor: Flavor,
    platform: Platform,
}

impl Cx<'_, '_> {
    /// The chat's sandbox. In Full access the caller has widened the folders to the whole
    /// computer (`approvals::widen`); the top of `near`'s drive is added in case it did not.
    fn sandbox(&self, near: &str) -> PathSandbox {
        let mut approved = self.chat.approved_folders.clone();
        if self.chat.full_access
            && let Some(top) = top_of(near, self.flavor)
            && !approved.contains(&top)
        {
            approved.push(top);
        }
        let fs = self.env.fs;
        let (sandbox, skipped) = PathSandbox::new(
            self.flavor,
            &approved,
            &self.chat.forbidden_folders,
            fs,
            &|p| fs.case_of(p),
        );
        for (folder, why) in skipped {
            tracing::warn!(%folder, "approved folder skipped: {why}");
        }
        sandbox
    }

    /// The folder relative paths resolve against.
    fn base(&self, sandbox: &PathSandbox) -> Option<String> {
        self.chat
            .working_folder
            .clone()
            .or_else(|| sandbox.roots().first().map(|r| r.canonical.to_string()))
    }

    /// A refusal the policy engine turns into "denied, no card" in every mode.
    fn denied(&self, path: &str, why: String) -> Described {
        Described::Action(Action {
            server: SERVER.into(),
            tool: self.t.name.into(),
            summary: policy::sanitize::one_line(path, 200),
            preview: policy::sanitize::visible(path),
            scope: Scope::FileAnalysis {
                folder: String::new(),
            },
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: Some(why),
            payload: json!({"kind": PAYLOAD_KIND, "tool": self.t.name, "refused": true}),
        })
    }

    /// The arguments rea receives: the model's (already only the schema's), with the fixed
    /// `detail`.
    fn forwarded(&self) -> Map<String, Value> {
        let mut out = self.args.clone();
        if let Some(detail) = self.t.fixed_detail {
            out.insert("detail".into(), json!(detail));
        }
        out
    }

    fn action(
        &self,
        scope: Scope,
        summary: String,
        preview: String,
        payload: Payload,
    ) -> Described {
        Described::Action(Action {
            server: SERVER.into(),
            tool: self.t.name.into(),
            summary: policy::sanitize::one_line(&summary, 200),
            preview: policy::sanitize::visible(&preview),
            scope,
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: serde_json::to_value(payload).unwrap_or(Value::Null),
        })
    }

    // ---- Open and file checks ----------------------------------------------------------

    fn path_call(&self) -> Described {
        let property = self.t.paths[0];
        let given = self
            .args
            .get(property)
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(bad) = self.bad_source_path() {
            return Described::Invalid(format!("{}{}.", m::SOURCE_PATH, shown(bad)));
        }
        // A network or device path is refused before anything is looked up.
        if walk::is_network_or_device(given, self.flavor) {
            return self.denied(given, m::NETWORK.into());
        }
        let given = strip_verbatim(given);
        let sandbox = self.sandbox(given);
        let base = self.base(&sandbox);
        // The same once its dot segments are read (`/tmp/../net/…`), still from the text;
        // then every link on the way, by its text, before the sandbox resolves the path.
        let base_lex = base
            .as_deref()
            .and_then(|b| policy::path::lexical(b, None, self.flavor).ok());
        if let Ok(lex) = policy::path::lexical(given, base_lex.as_ref(), self.flavor) {
            let normal = lex.to_string();
            if walk::is_network_or_device(&normal, self.flavor) {
                return self.denied(given, m::NETWORK.into());
            }
            match walk::safe_to_resolve(self.env.fs, &sandbox, &normal, self.flavor) {
                Hops::Clear => {}
                Hops::Network => return self.denied(given, m::NETWORK.into()),
                Hops::Unclear => {
                    return Described::Invalid(format!(
                        "{}{}.",
                        m::UNREADABLE_FILE,
                        shown(&normal)
                    ));
                }
            }
        }
        let checked = match sandbox.check(given, base.as_deref(), self.env.fs) {
            Ok(c) => c,
            Err(e) => return self.path_refusal(given, &e),
        };
        let canonical = checked.canonical.clone();
        if walk::is_network_or_device(&canonical, self.flavor) {
            return self.denied(given, m::NETWORK.into());
        }
        if !checked.exists {
            return Described::Invalid(format!("{}{}.", m::NOT_FOUND, shown(&canonical)));
        }
        let name = walk::file_name(&canonical, self.flavor).to_string();
        let folder = match self.env.fs.kind(&canonical) {
            Some(EntryKind::File) => false,
            Some(EntryKind::Dir) => true,
            Some(EntryKind::Device | EntryKind::Pipe | EntryKind::Socket | EntryKind::Other) => {
                return self.denied(&canonical, format!("{}{}.", m::DEVICE, shown(&canonical)));
            }
            Some(EntryKind::Link) | None => {
                return Described::Invalid(format!("{}{}.", m::NOT_FOUND, shown(&canonical)));
            }
        };
        let wrong = match (self.t.target, folder) {
            (TargetKind::File, true) => Some(m::WANTS_FILE),
            (TargetKind::FileOrApp, true) if !is_app_bundle(&name) => Some(m::WANTS_PROGRAM),
            (TargetKind::Folder, false) => Some(m::WANTS_FOLDER),
            _ => None,
        };
        if let Some(wrong) = wrong {
            return Described::Invalid(format!("{wrong}{}.", shown(&canonical)));
        }
        let identity = if folder {
            // A folder holding harnessUI's own folders or a key store: refused before it is
            // walked (the walk refuses them too, where it meets them).
            if let Some(inside) = walk::forbidden_inside(
                self.env.fs,
                &canonical,
                &self.chat.forbidden_folders,
                self.flavor,
            ) {
                return self.denied(
                    &canonical,
                    format!("{}{}.", m::HOLDS_APP_FOLDERS, shown(&inside)),
                );
            }
            if let Err(refusal) =
                walk::walk_folder(self.env.fs, &canonical, &sandbox, self.chat.full_access)
            {
                return self.denied(&canonical, refusal.message());
            }
            None
        } else {
            match self.env.fs.identity(&canonical) {
                Some(id) => Some(id),
                None => {
                    return Described::Invalid(format!(
                        "{}{}.",
                        m::UNREADABLE_FILE,
                        shown(&canonical)
                    ));
                }
            }
        };
        self.path_action(&sandbox, &checked, name, folder, identity, property)
    }

    fn path_refusal(&self, given: &str, e: &PathError) -> Described {
        match e {
            PathError::Forbidden => self.denied(given, m::APP_FOLDERS.into()),
            PathError::OutsideApprovedFolders | PathError::Relative => {
                self.denied(given, m::OUTSIDE.into())
            }
            PathError::NetworkOrDevicePath
            | PathError::RemoteResolution
            | PathError::DeviceName(_)
            | PathError::AlternateStream(_) => self.denied(given, m::NETWORK.into()),
            other => Described::Invalid(format!(
                "That path can't be used: {}. Give the file's full path.",
                policy::sanitize::one_line(&other.to_string(), 300)
            )),
        }
    }

    fn path_action(
        &self,
        sandbox: &PathSandbox,
        checked: &CheckedPath,
        name: String,
        folder: bool,
        identity: Option<FileIdentity>,
        property: &str,
    ) -> Described {
        let canonical = &checked.canonical;
        let root = sandbox
            .roots()
            .get(checked.root)
            .map(|r| r.canonical.to_string())
            .unwrap_or_default();
        let decompiler = (self.t.class == ToolClass::Open)
            .then_some(self.env.availability.decompiler)
            .flatten();
        let mut args = self.forwarded();
        args.insert(property.into(), json!(canonical));
        let summary = match decompiler {
            Some(d) => format!("Open {name} in rea ({})", d.label()),
            None => format!("Check {name} with rea"),
        };
        let preview = card(self.t, canonical, &root, folder, decompiler);
        let payload = Payload {
            kind: PAYLOAD_KIND.into(),
            tool: self.t.name.into(),
            args,
            target: Some(TargetSeen {
                property: property.into(),
                canonical: canonical.clone(),
                root: root.clone(),
                identity,
                folder,
            }),
            decompiler,
            binding: None,
            evidence: None,
            store_epoch: None,
        };
        self.action(
            Scope::FileAnalysis { folder: root },
            summary,
            preview,
            payload,
        )
    }

    /// The first `source_paths` entry that is not a plain relative path.
    fn bad_source_path(&self) -> Option<&str> {
        self.args
            .get("source_paths")?
            .as_array()?
            .iter()
            .filter_map(Value::as_str)
            .find(|p| !plain_relative(p))
    }

    // ---- Session tools -----------------------------------------------------------------

    fn session_call(&self) -> Described {
        let session = self.env.session;
        let chat = self.chat.chat_id;
        let Some(b) = session.binding.as_ref().filter(|b| b.chat_id == chat) else {
            return Described::Invalid(
                match session.closed_reason(chat) {
                    Some(ClosedReason::OtherChat) => m::CLOSED_OTHER_CHAT,
                    Some(ClosedReason::SettingsChanged) => m::CLOSED_SETTINGS,
                    None => m::NO_FILE_OPEN,
                }
                .into(),
            );
        };
        if self.env.availability.decompiler != Some(b.decompiler) {
            return Described::Invalid(m::CLOSED_SETTINGS.into());
        }
        if !self.reachable(&b.canonical) {
            return Described::Invalid(m::NO_LONGER_REACHABLE.into());
        }
        if b.decompiler == Decompiler::Kuna && KUNA_CANNOT_RUN.contains(&self.t.name) {
            return Described::Invalid(kuna_cannot_message(self.platform).into());
        }
        let args = self.forwarded();
        let name = walk::file_name(&b.canonical, self.flavor).to_string();
        let summary = format!("{} on {name} (rea)", self.t.name);
        let preview = format!(
            "rea works on {}, opened in this chat with {}.\nArguments: {}",
            shown(&b.canonical),
            b.decompiler.label(),
            mcp_host::canonical_json(&Value::Object(args.clone()))
        );
        let payload = Payload {
            kind: PAYLOAD_KIND.into(),
            tool: self.t.name.into(),
            args,
            target: None,
            decompiler: None,
            binding: Some(BindingSeen {
                chat_id: b.chat_id,
                canonical: b.canonical.clone(),
                identity: b.identity.clone(),
                decompiler: b.decompiler,
                generation: b.generation,
                open_seq: b.open_seq,
            }),
            evidence: None,
            store_epoch: Some(session.store_epoch),
        };
        self.action(Scope::FileAnalysisSession, summary, preview, payload)
    }

    /// Whether a canonical path still passes this chat's sandbox now (and exists). A folder
    /// on the way that was swapped for a link to a network place since is found by its
    /// text, before anything resolves it.
    fn reachable(&self, canonical: &str) -> bool {
        if walk::is_network_or_device(canonical, self.flavor) {
            return false;
        }
        let sandbox = self.sandbox(canonical);
        walk::safe_to_resolve(self.env.fs, &sandbox, canonical, self.flavor) == Hops::Clear
            && sandbox
                .check(canonical, None, self.env.fs)
                .is_ok_and(|c| c.exists && !walk::is_network_or_device(&c.canonical, self.flavor))
    }

    // ---- inspect_analysis_view ---------------------------------------------------------

    fn view_call(&self) -> Described {
        let view = self.args.get("view").cloned().unwrap_or(Value::Null);
        if view.to_string().len() > allow::MAX_VIEW_BYTES {
            return Described::Invalid(m::VIEW_TOO_LARGE.into());
        }
        if holds_a_path(&view) {
            return Described::Invalid(m::VIEW_PATH.into());
        }
        let id = self
            .args
            .get("source")
            .and_then(|s| s.get("evidence_id"))
            .and_then(Value::as_str)
            .unwrap_or("");
        let session = self.env.session;
        let Some(ev) = session
            .evidence(id)
            .filter(|e| e.chat_id == self.chat.chat_id && e.store_epoch == session.store_epoch)
        else {
            return Described::Invalid(m::UNKNOWN_ID.into());
        };
        if !self.reachable(&ev.canonical) {
            return Described::Invalid(m::EVIDENCE_GONE.into());
        }
        let args = self.forwarded();
        let name = walk::file_name(&ev.canonical, self.flavor).to_string();
        let summary = format!("Read more of a rea result about {name}");
        let preview = format!(
            "rea reads more of an earlier result of this chat about {}.\nArguments: {}",
            shown(&ev.canonical),
            mcp_host::canonical_json(&Value::Object(args.clone()))
        );
        let payload = Payload {
            kind: PAYLOAD_KIND.into(),
            tool: self.t.name.into(),
            args,
            target: None,
            decompiler: None,
            binding: None,
            evidence: Some(EvidenceSeen {
                id: id.to_string(),
                chat_id: ev.chat_id,
                store_epoch: ev.store_epoch,
                canonical: ev.canonical.clone(),
            }),
            store_epoch: Some(session.store_epoch),
        };
        self.action(Scope::FileAnalysisSession, summary, preview, payload)
    }
}

/// The card's three lines for an Open or a file check (section 6), every path the
/// canonical one made safe to show.
pub fn card(
    tool: &AllowedTool,
    canonical: &str,
    root: &str,
    folder: bool,
    decompiler: Option<Decompiler>,
) -> String {
    // The decompiler first, the path last: a file's own name may hold "<decompiler>".
    let first = tool
        .card_line
        .unwrap_or("rea will read <path>.")
        .replace("<decompiler>", decompiler.map_or("", Decompiler::label))
        .replace("<path>", &shown(canonical));
    let second = if folder {
        "It does not run anything in it or change it."
    } else {
        "It does not run the file or change it. What rea finds goes to the model as data from the file."
    };
    format!(
        "{first}\n{second}\nAllow for this chat: rea may open or check other files in {} in this chat.",
        shown(root)
    )
}

/// `C:\x` for `\\?\C:\x` (the model may copy a path in that form); anything else as it is.
fn strip_verbatim(path: &str) -> &str {
    for prefix in ["\\\\?\\", "//?/"] {
        if let Some(rest) = path.strip_prefix(prefix) {
            let b = rest.as_bytes();
            if b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                return rest;
            }
        }
    }
    path
}

/// The top of the drive holding `path`: `/`, or `C:\`.
fn top_of(path: &str, flavor: Flavor) -> Option<String> {
    match flavor {
        Flavor::Posix => Some("/".into()),
        Flavor::Windows => {
            let b = path.as_bytes();
            (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
                .then(|| format!("{}:\\", (b[0] as char).to_ascii_uppercase()))
        }
    }
}

/// A Mac app bundle: a folder whose name ends in `.app`.
fn is_app_bundle(name: &str) -> bool {
    name.len() > 4 && name.to_ascii_lowercase().ends_with(".app")
}

/// A plain relative path: not empty, not absolute, no drive, no `..`, no NUL.
fn plain_relative(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\0')
        && !looks_like_a_path(p)
        && !p.split(['/', '\\']).any(|part| part == "..")
}

/// Whether a string is an absolute, home-relative, UNC or drive path, or a `file:` URL.
fn looks_like_a_path(s: &str) -> bool {
    let b = s.as_bytes();
    s.starts_with('/')
        || s.starts_with('\\')
        || s.starts_with('~')
        || (b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':')
        || s.to_ascii_lowercase().starts_with("file:")
}

/// Whether any string in a value (keys included) looks like a path.
fn holds_a_path(v: &Value) -> bool {
    match v {
        Value::String(s) => looks_like_a_path(s),
        Value::Array(items) => items.iter().any(holds_a_path),
        Value::Object(map) => map
            .iter()
            .any(|(k, v)| looks_like_a_path(k) || holds_a_path(v)),
        _ => false,
    }
}

#[cfg(test)]
mod tests;
