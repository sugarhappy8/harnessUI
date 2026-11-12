//! The built-in `files` server (ARCHITECTURE.md 9.3): `list_dir`, `read_file`,
//! `search_files`, `write_file`, `edit_file`, `delete_file`.
//!
//! - Every path goes through the path sandbox (13.4). A refusal is a hard limit: no
//!   prompt, the model is told which folders are valid, and it is not a mistake.
//! - Writes show a diff computed against the current file. The file's SHA-256 is
//!   recorded with the diff and the write is refused if the file changed between
//!   preview and apply (13.3: what runs is what was shown).
//! - Reads and searches are cut by whole lines to the per-result cap and end with an
//!   instruction the model can follow (8.4).
//! - Nothing inside a skill folder (`ChatContext::skill_roots`) is written, edited or
//!   deleted: that is a hard limit, checked again before the write (9.3, 10).
//! - A failure says what is wrong and what to call instead (app 0.0.4). What a failure may
//!   show depends on where it is answered. A call that the policy gate let through as a
//!   read (`read_file`, `list_dir`, `search_files`) may name similar files in the folder.
//!   A write that fails is answered by `describe`, before the gate, and its message is an
//!   app note outside the markers that say "this is data": it names lines of a file only
//!   when this chat has already read that file ([`Known`]), and never lists a folder.
//! - A result cut to the budget carries the whole text for the app to store
//!   (`ToolOutput::full_text`) and a note that says which lines were shown (`shorten`).

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{Action, CheckedPath, Risk, Scope, WriteKind, always_ask_reason};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::errors::{self, EditContext, Known, Missing, Pick};
use crate::shorten::{self, Shown};
use crate::skill_guard::{self, Reach};

/// Bytes read from a file at most (8.4 raw limit).
pub const RAW_LIMIT: u64 = 1024 * 1024;
/// The largest file `edit_file` changes: larger ones are changed with a command.
pub const MAX_EDIT_BYTES: u64 = 4 * 1024 * 1024;
/// Characters per token used to size results.
const CHARS_PER_TOKEN: usize = 3;
/// Files searched at most per call.
const MAX_SEARCH_FILES: usize = 20_000;
/// Files larger than this are skipped by search.
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Diff text kept for the card at most.
const MAX_DIFF_CHARS: usize = 200_000;

/// Files remembered per server at most (the oldest are forgotten first).
const MAX_SEEN: usize = 4096;
/// Names looked at in one folder when searching for similar ones.
const MAX_NAMES_SCANNED: usize = 5000;

/// The result of a write that found the file different from the one that was previewed.
const CHANGED_AFTER_PREVIEW: &str = "The file changed after the change was previewed, so nothing was written. Read it again and redo the change.";

/// What identifies the content a chat was given: the file's size and a hash of the part
/// that `read_file` reads (the first [`RAW_LIMIT`] bytes).
type Fingerprint = (u64, String);

fn fingerprint(bytes: &[u8], size: u64) -> Fingerprint {
    let head = &bytes[..bytes.len().min(RAW_LIMIT as usize)];
    (size, sha256_hex(head))
}

/// The files each chat has read or written with these tools, and the content it was given.
/// Kept in memory only: after a restart a file counts as not read until it is read again.
#[derive(Debug, Default)]
struct Seen {
    files: HashMap<(i64, String), (Fingerprint, u64)>,
    clock: u64,
}

/// The `files` server.
#[derive(Debug, Clone)]
pub struct FilesServer {
    flavor: Flavor,
    seen: Arc<Mutex<Seen>>,
}

impl Default for FilesServer {
    fn default() -> Self {
        Self::with_flavor(Flavor::native())
    }
}

fn s(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}
fn n(args: &Map<String, Value>, key: &str) -> Option<u64> {
    args.get(key).and_then(Value::as_u64)
}
fn b(args: &Map<String, Value>, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8192).any(|&c| c == 0)
}

fn budget_chars(chat: &ChatContext) -> usize {
    chat.result_cap_tokens.max(500) * CHARS_PER_TOKEN
}

/// Why a file could not be read or written, in plain words where there are some.
fn io_reason(e: &std::io::Error) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            "permission denied (the file or its folder is not open to this user, or is read-only)"
                .into()
        }
        _ => e.to_string(),
    }
}

/// The nearest existing thing above `path`, when it is a file: nothing can be created
/// inside a file.
fn file_on_the_way(path: &Path) -> Option<&Path> {
    path.ancestors()
        .skip(1)
        .find(|a| a.symlink_metadata().is_ok())
        .filter(|a| a.is_file())
}

/// The message for a path that goes through a file.
fn inside_a_file(path: &Path, file: &Path) -> String {
    format!(
        "{} cannot be used: {} is a file, not a folder, so nothing can be inside it.",
        policy::sanitize::visible(&path.display().to_string()),
        policy::sanitize::visible(&file.display().to_string())
    )
}

/// The names in a folder (at most [`MAX_NAMES_SCANNED`]).
fn names_in(folder: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .take(MAX_NAMES_SCANNED)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

/// `text` cut to at most `max` bytes at a character boundary.
fn cut_to(text: &str, max: usize) -> &str {
    let mut end = max.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// How many numbered lines of a stored output these lines are (`shorten::lines`).
fn stored_lines<S: AsRef<str>>(lines: &[S]) -> usize {
    lines.iter().map(|l| shorten::pieces(l.as_ref())).sum()
}

/// Reads at most `RAW_LIMIT` bytes; returns the bytes and the file's full size.
fn read_capped(path: &Path) -> std::io::Result<(Vec<u8>, u64)> {
    let f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut buf = Vec::with_capacity(size.min(RAW_LIMIT) as usize);
    f.take(RAW_LIMIT).read_to_end(&mut buf)?;
    Ok((buf, size))
}

fn unified_diff(old: &str, new: &str, path: &str) -> String {
    let diff = similar::TextDiff::from_lines(old, new);
    let mut out = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    if out.len() > MAX_DIFF_CHARS {
        let mut cut = MAX_DIFF_CHARS;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
        out.push_str("\n[... diff truncated ...]\n");
    }
    out
}

impl FilesServer {
    /// A server using another OS's path rules (tests).
    pub fn with_flavor(flavor: Flavor) -> Self {
        Self {
            flavor,
            seen: Arc::default(),
        }
    }

    /// Notes that `chat` was given the content `print` of the file at `path`: it read the
    /// file, or wrote it.
    fn remember(&self, chat: i64, path: &str, print: Fingerprint) {
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };
        seen.clock += 1;
        let now = seen.clock;
        seen.files.insert((chat, path.to_string()), (print, now));
        if seen.files.len() > MAX_SEEN
            && let Some(oldest) = seen
                .files
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(key, _)| key.clone())
        {
            seen.files.remove(&oldest);
        }
    }

    fn forget(&self, chat: i64, path: &str) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.files.remove(&(chat, path.to_string()));
        }
    }

    /// Whether `chat` was given what the file at `path` holds now (`now`). A state that
    /// cannot be read counts as "no".
    fn known(&self, chat: i64, path: &str, now: &Fingerprint) -> Known {
        let Ok(seen) = self.seen.lock() else {
            return Known::No;
        };
        match seen.files.get(&(chat, path.to_string())) {
            Some((print, _)) if print == now => Known::Yes,
            Some(_) => Known::Stale,
            None => Known::No,
        }
    }

    fn sandbox(&self, chat: &ChatContext) -> PathSandbox {
        let (sb, skipped) = PathSandbox::new(
            self.flavor,
            &chat.approved_folders,
            &chat.forbidden_folders,
            &RealFs,
            &|p| volume_case(p),
        );
        for (folder, why) in skipped {
            tracing::warn!(%folder, "approved folder skipped: {why}");
        }
        sb
    }

    fn base(&self, chat: &ChatContext, sb: &PathSandbox) -> Option<String> {
        chat.working_folder
            .clone()
            .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()))
    }

    /// The hard-limit message: why, and which folders are valid.
    fn refusal(&self, why: &policy::PathError, sb: &PathSandbox) -> String {
        // Not a question of which folder: the path itself could not be followed.
        if matches!(why, policy::PathError::Io(_)) {
            return format!("{why}.");
        }
        let folders: Vec<String> = sb.roots().iter().map(|r| r.canonical.to_string()).collect();
        if folders.is_empty() {
            format!(
                "{why}. No folder is approved for this chat yet, so the file tools cannot reach anything. The user can approve a folder in the Tools panel."
            )
        } else {
            format!(
                "{why}. Approved folders: {}. Use a path inside one of them; the user can approve another folder in the Tools panel.",
                folders.join(", ")
            )
        }
    }

    /// " Working folder: X.", or nothing when the chat has no folder at all.
    fn working_folder_note(&self, chat: &ChatContext) -> String {
        let sb = self.sandbox(chat);
        match self.base(chat, &sb) {
            Some(folder) => format!(
                " Working folder: {}.",
                policy::sanitize::one_line(&folder, 300)
            ),
            None => String::new(),
        }
    }

    /// The message for a path that does not exist, with similar names that do. Only for
    /// calls the policy gate let through as a read: the names come from the path's folder
    /// and the folder above it, and only from folders inside the approved ones.
    fn not_found(&self, chat: &ChatContext, path: &Path) -> String {
        let sb = self.sandbox(chat);
        let working = self.base(chat, &sb);
        // The first part of the path that does not exist.
        let mut first_missing = path;
        while let Some(parent) = first_missing.parent() {
            if parent.as_os_str().is_empty() || parent.exists() {
                break;
            }
            first_missing = parent;
        }
        let is_last = first_missing == path;
        let base = first_missing.parent().unwrap_or(first_missing);
        let wanted = first_missing
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut folders = vec![base];
        if is_last && let Some(above) = base.parent() {
            folders.push(above);
        }
        let mut similar: Vec<String> = Vec::new();
        for folder in folders {
            let room = errors::MAX_SIMILAR_NAMES.saturating_sub(similar.len());
            if room == 0
                || sb
                    .check(&folder.display().to_string(), None, &RealFs)
                    .is_err()
            {
                continue;
            }
            for name in errors::similar_names(&wanted, names_in(folder), room) {
                similar.push(folder.join(name).display().to_string());
            }
        }
        let missing_folder = (!is_last).then(|| first_missing.display().to_string());
        errors::not_found_message(&Missing {
            path: &path.display().to_string(),
            missing_folder: missing_folder.as_deref(),
            looked_in: &base.display().to_string(),
            similar: &similar,
            working_folder: working.as_deref(),
        })
    }

    fn check(
        &self,
        chat: &ChatContext,
        path: &str,
    ) -> Result<(CheckedPath, String, PathSandbox), String> {
        let sb = self.sandbox(chat);
        let base = self.base(chat, &sb);
        match sb.check(path, base.as_deref(), &RealFs) {
            Ok(c) => {
                let root = sb.roots()[c.root].canonical.to_string();
                Ok((c, root, sb))
            }
            Err(e) => {
                // A file named where a folder has to be ("notes.txt/more.txt").
                let written = match &base {
                    Some(base) if !Path::new(path).is_absolute() => Path::new(base).join(path),
                    _ => PathBuf::from(path),
                };
                if matches!(e, policy::PathError::Io(_))
                    && let Some(file) = file_on_the_way(&written)
                {
                    return Err(inside_a_file(&written, file));
                }
                Err(self.refusal(&e, &sb))
            }
        }
    }

    fn hard_limit(tool: &str, path: &str, why: String) -> Described {
        Described::Action(Action {
            server: "files".into(),
            tool: tool.into(),
            summary: policy::sanitize::one_line(path, 200),
            preview: policy::sanitize::visible(path),
            scope: Scope::FileRead {
                folder: String::new(),
            },
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: Some(why),
            payload: Value::Null,
        })
    }

    fn payload(c: &CheckedPath, extra: Value) -> Value {
        let mut p = json!({
            "canonical": c.canonical,
            "root": c.root,
            "exists": c.exists,
            "wasSymlink": c.was_symlink,
        });
        if let (Some(obj), Value::Object(more)) = (p.as_object_mut(), extra) {
            obj.extend(more);
        }
        p
    }

    fn checked_from(action: &Action) -> Option<CheckedPath> {
        let p = &action.payload;
        Some(CheckedPath {
            canonical: p.get("canonical")?.as_str()?.to_string(),
            root: p.get("root")?.as_u64()? as usize,
            exists: p.get("exists")?.as_bool()?,
            was_symlink: p.get("wasSymlink")?.as_bool()?,
        })
    }

    /// Step 5 of the sandbox, immediately before use.
    fn recheck(&self, chat: &ChatContext, action: &Action) -> Result<PathBuf, String> {
        let checked =
            Self::checked_from(action).ok_or("internal error: the approved action has no path")?;
        let sb = self.sandbox(chat);
        sb.recheck(&checked, &RealFs)
            .map_err(|e| self.refusal(&e, &sb))?;
        Ok(PathBuf::from(checked.canonical))
    }

    fn current_hash(path: &Path) -> Result<Option<String>, String> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("could not read the file: {e}")),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn write_action(
        &self,
        tool: &str,
        c: &CheckedPath,
        root: &str,
        kind: WriteKind,
        summary: String,
        preview: String,
        extra: Value,
    ) -> Described {
        let reason = always_ask_reason(&c.canonical, kind, self.flavor);
        let risk = if reason.is_some() {
            Risk::AlwaysAsk
        } else {
            Risk::Normal
        };
        let preview = match reason {
            Some(r) => format!("{}\n\n{preview}", capitalise(r)),
            None => preview,
        };
        Described::Action(Action {
            server: "files".into(),
            tool: tool.into(),
            summary,
            preview,
            scope: Scope::FileWrite {
                folder: root.to_string(),
            },
            risk,
            read_only: false,
            outbound: false,
            // Allow all: deletes, emptying a file and sensitive places keep asking.
            keep_asking: policy::files::keep_asking(&c.canonical, kind, self.flavor),
            network: false,
            hard_limit: None,
            payload: Self::payload(c, extra),
        })
    }

    fn read_action(tool: &str, c: &CheckedPath, root: &str, summary: String) -> Described {
        Described::Action(Action {
            server: "files".into(),
            tool: tool.into(),
            summary: summary.clone(),
            preview: summary,
            scope: Scope::FileRead {
                folder: root.to_string(),
            },
            risk: Risk::Normal,
            read_only: true,
            outbound: false,
            keep_asking: None,
            network: false,
            hard_limit: None,
            payload: Self::payload(c, Value::Null),
        })
    }
}

/// The tools that create, change or delete files.
fn is_write(tool: &str) -> bool {
    matches!(tool, "write_file" | "edit_file" | "delete_file")
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn path_schema(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

#[async_trait]
impl BuiltinServer for FilesServer {
    fn name(&self) -> &str {
        "files"
    }

    fn tools(&self) -> Vec<ToolDef> {
        let def =
            |name: &str, description: &str, schema: Value, read_only: bool, destructive: bool| {
                ToolDef {
                    name: name.into(),
                    description: description.into(),
                    input_schema: schema,
                    destructive: Some(destructive),
                    read_only: Some(read_only),
                }
            };
        vec![
            def(
                "list_dir",
                "List a folder's entries (name, type, size). Paths are relative to the working folder unless absolute.",
                json!({"type":"object","properties":{
                    "path": path_schema("Folder to list"),
                    "depth": {"type":"integer","minimum":1,"maximum":3,"description":"Levels to descend (default 1)"}
                },"required":["path"],"additionalProperties":false}),
                true,
                false,
            ),
            def(
                "read_file",
                "Read a text file. Long files are returned in parts: use offset (first line, 1-based) and limit (number of lines) to continue.",
                json!({"type":"object","properties":{
                    "path": path_schema("File to read"),
                    "offset": {"type":"integer","minimum":1,"description":"First line to return (1-based)"},
                    "limit": {"type":"integer","minimum":1,"description":"Number of lines to return"}
                },"required":["path"],"additionalProperties":false}),
                true,
                false,
            ),
            def(
                "search_files",
                "Search file contents under a folder. Returns matching lines as path:line: text. Use glob to filter file names (for example *.rs) and offset to page through matches.",
                json!({"type":"object","properties":{
                    "path": path_schema("Folder to search"),
                    "query": {"type":"string","minLength":1,"description":"Text to find (case-insensitive), or a regular expression when regex is true"},
                    "regex": {"type":"boolean"},
                    "glob": {"type":"string","description":"File-name filter such as *.md"},
                    "offset": {"type":"integer","minimum":0,"description":"Matches to skip"}
                },"required":["path","query"],"additionalProperties":false}),
                true,
                false,
            ),
            def(
                "write_file",
                "Create a file or replace its whole content. For changes to an existing file prefer edit_file. Write large files in several smaller steps.",
                json!({"type":"object","properties":{
                    "path": path_schema("File to write"),
                    "content": {"type":"string"}
                },"required":["path","content"],"additionalProperties":false}),
                false,
                false,
            ),
            def(
                "edit_file",
                "Replace exact text in an existing file. old_text must match exactly once, unless occurrence says which match to change (1 is the first) or replace_all is true.",
                json!({"type":"object","properties":{
                    "path": path_schema("File to edit"),
                    "old_text": {"type":"string","minLength":1},
                    "new_text": {"type":"string"},
                    "occurrence": {"type":"integer","minimum":1},
                    "replace_all": {"type":"boolean"}
                },"required":["path","old_text","new_text"],"additionalProperties":false}),
                false,
                false,
            ),
            def(
                "delete_file",
                "Delete a file. Always asks the user.",
                json!({"type":"object","properties":{"path": path_schema("File to delete")},"required":["path"],"additionalProperties":false}),
                false,
                true,
            ),
        ]
    }

    async fn describe(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        chat: &ChatContext,
    ) -> Described {
        let Some(path) = s(args, "path") else {
            return Described::Invalid("path is required".into());
        };
        let (c, root, _) = match self.check(chat, &path) {
            Ok(v) => v,
            Err(why) => return Self::hard_limit(tool, &path, why),
        };
        if is_write(tool)
            && let Err(why) =
                skill_guard::check(&c.canonical, &chat.skill_roots, self.flavor, Reach::Inside)
        {
            return Self::hard_limit(tool, &path, why);
        }
        let shown = policy::sanitize::visible(&c.canonical);
        match tool {
            "list_dir" => Self::read_action(tool, &c, &root, format!("List {shown}")),
            "read_file" => Self::read_action(tool, &c, &root, format!("Read {shown}")),
            "search_files" => {
                let q = policy::sanitize::one_line(&s(args, "query").unwrap_or_default(), 120);
                Self::read_action(tool, &c, &root, format!("Search {shown} for \"{q}\""))
            }
            "write_file" => {
                let content = s(args, "content").unwrap_or_default();
                let path_buf = PathBuf::from(&c.canonical);
                if path_buf.is_dir() {
                    return Described::Invalid(format!(
                        "{shown} is a folder, not a file. write_file needs the path of a file inside it, for example {}. list_dir shows what is in the folder.",
                        policy::sanitize::visible(&path_buf.join("notes.txt").display().to_string())
                    ));
                }
                if let Some(file) = file_on_the_way(&path_buf) {
                    return Described::Invalid(inside_a_file(&path_buf, file));
                }
                let (old, hash) = match std::fs::read(&path_buf) {
                    Ok(bytes) if is_binary(&bytes) => {
                        return Described::Invalid(format!(
                            "{shown} is a binary file, so it was not replaced: write_file only writes text. Write the text to another path, or delete the file with delete_file first if it should be replaced."
                        ));
                    }
                    Ok(bytes) => {
                        // What this chat read is no longer what is there: replacing it now
                        // would throw away a change the model has not seen.
                        let now = fingerprint(&bytes, bytes.len() as u64);
                        if self.known(chat.chat_id, &c.canonical, &now) == Known::Stale {
                            return Described::Invalid(format!(
                                "{shown} has changed since it was read in this chat, so it was not replaced. Call read_file on it again to see what is there now, then write_file or edit_file."
                            ));
                        }
                        (
                            String::from_utf8_lossy(&bytes).into_owned(),
                            Some(sha256_hex(&bytes)),
                        )
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
                    Err(e) => {
                        return Described::Invalid(format!(
                            "could not read {shown}: {}",
                            io_reason(&e)
                        ));
                    }
                };
                let kind = if hash.is_some() {
                    WriteKind::Overwrite {
                        existing_len: old.len() as u64,
                        new_len: content.len() as u64,
                    }
                } else {
                    WriteKind::Create
                };
                let verb = if hash.is_some() { "Replace" } else { "Create" };
                let diff = unified_diff(&old, &content, &c.canonical);
                self.write_action(
                    tool,
                    &c,
                    &root,
                    kind,
                    format!("{verb} {shown} ({} bytes)", content.len()),
                    policy::sanitize::visible(&diff),
                    json!({ "expectedSha256": hash }),
                )
            }
            "edit_file" => {
                let old_text = s(args, "old_text").unwrap_or_default();
                let new_text = s(args, "new_text").unwrap_or_default();
                let pick = match Pick::from_args(n(args, "occurrence"), b(args, "replace_all")) {
                    Ok(pick) => pick,
                    Err(why) => return Described::Invalid(why),
                };
                let path_buf = PathBuf::from(&c.canonical);
                if path_buf.is_dir() {
                    return Described::Invalid(format!(
                        "{shown} is a folder, not a file. edit_file changes one file; list_dir shows what is in the folder."
                    ));
                }
                // Answered before anyone was asked, so nothing of the folder is listed.
                let missing = || {
                    Described::Invalid(format!(
                        "{shown} does not exist, so there is nothing to edit.{} list_dir shows the names that exist; write_file creates a file.",
                        self.working_folder_note(chat)
                    ))
                };
                let unreadable = |e: &std::io::Error| {
                    Described::Invalid(format!("could not read {shown}: {}", io_reason(e)))
                };
                let size = match std::fs::metadata(&path_buf) {
                    Ok(meta) => meta.len(),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return missing(),
                    Err(e) => return unreadable(&e),
                };
                if size > MAX_EDIT_BYTES {
                    return Described::Invalid(format!(
                        "{shown} is too large for edit_file: it has {size} bytes and the limit is {MAX_EDIT_BYTES} (4 MB). Change it with a command (run_command) instead."
                    ));
                }
                let bytes = match std::fs::read(&path_buf) {
                    Ok(b) => b,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return missing(),
                    Err(e) => return unreadable(&e),
                };
                if is_binary(&bytes) {
                    return Described::Invalid(format!(
                        "{shown} is a binary file. edit_file only changes text files."
                    ));
                }
                let current = String::from_utf8_lossy(&bytes).into_owned();
                let plan = match errors::plan_edit(&current, &old_text, &new_text, pick) {
                    Ok(plan) => plan,
                    Err(problem) => {
                        // Lines of the file are quoted only when this chat has read it.
                        let now = fingerprint(&bytes, bytes.len() as u64);
                        return Described::Invalid(errors::edit_problem_message(
                            &problem,
                            &EditContext {
                                path: &c.canonical,
                                current: &current,
                                old_text: &old_text,
                                known: self.known(chat.chat_id, &c.canonical, &now),
                            },
                        ));
                    }
                };
                let diff = unified_diff(&current, &plan.updated, &c.canonical);
                let kind = WriteKind::Overwrite {
                    existing_len: current.len() as u64,
                    new_len: plan.updated.len() as u64,
                };
                let what = match pick {
                    Pick::Nth(nth) => {
                        format!("match {nth} of {}, at line {}", plan.total, plan.line)
                    }
                    _ => format!(
                        "{} replacement{}",
                        plan.replaced,
                        if plan.replaced == 1 { "" } else { "s" }
                    ),
                };
                self.write_action(
                    tool,
                    &c,
                    &root,
                    kind,
                    format!("Edit {shown} ({what})"),
                    policy::sanitize::visible(&diff),
                    json!({ "expectedSha256": sha256_hex(&bytes) }),
                )
            }
            "delete_file" => {
                let p = PathBuf::from(&c.canonical);
                if p.is_dir() {
                    return Described::Invalid(format!(
                        "{shown} is a folder. delete_file only deletes files; a folder is removed with a command (run_command)."
                    ));
                }
                if p.symlink_metadata().is_err() {
                    // Answered before anyone was asked, so nothing of the folder is listed.
                    return Described::Invalid(format!(
                        "{shown} does not exist, so there is nothing to delete."
                    ));
                }
                if !p.is_file() {
                    return Described::Invalid(format!(
                        "{shown} is not a regular file, so delete_file leaves it alone."
                    ));
                }
                let hash = std::fs::read(&p).ok().map(|b| sha256_hex(&b));
                self.write_action(
                    tool,
                    &c,
                    &root,
                    WriteKind::Delete,
                    format!("Delete {shown}"),
                    format!("Delete {shown}"),
                    json!({ "expectedSha256": hash }),
                )
            }
            other => Described::Invalid(format!("the files server has no tool named {other}")),
        }
    }

    async fn call(
        &self,
        tool: &str,
        args: &Map<String, Value>,
        action: &Action,
        ctx: CallContext,
    ) -> ToolOutput {
        if ctx.cancel.is_cancelled() {
            return ToolOutput::error("cancelled");
        }
        let path = match self.recheck(&ctx.chat, action) {
            Ok(p) => p,
            Err(why) => return ToolOutput::error(why),
        };
        // A skill folder may have appeared since the preview (the shared one created as a
        // link to this folder).
        if is_write(tool)
            && let Err(why) = skill_guard::check(
                &path.display().to_string(),
                &ctx.chat.skill_roots,
                self.flavor,
                Reach::Inside,
            )
        {
            return ToolOutput::error(why);
        }
        let budget = budget_chars(&ctx.chat);
        let chat_id = ctx.chat.chat_id;
        let key = path.display().to_string();
        // A read of something that is not there. The gate let this call read the folder,
        // so the answer may name what is in it.
        if matches!(tool, "list_dir" | "read_file" | "search_files") && !path.exists() {
            return ToolOutput::error(self.not_found(&ctx.chat, &path));
        }
        match tool {
            "list_dir" => {
                let depth = n(args, "depth").unwrap_or(1).clamp(1, 3) as usize;
                let cancel = ctx.cancel.clone();
                tokio::task::spawn_blocking(move || list_dir(&path, depth, budget, &cancel))
                    .await
                    .unwrap_or_else(|e| ToolOutput::error(e.to_string()))
            }
            "read_file" => {
                let offset = n(args, "offset").unwrap_or(1).max(1) as usize;
                let limit = n(args, "limit").map(|l| l as usize);
                let (out, print) =
                    tokio::task::spawn_blocking(move || read_file(&path, offset, limit, budget))
                        .await
                        .unwrap_or_else(|e| (ToolOutput::error(e.to_string()), None));
                // From here on an edit that fails may quote this file's lines.
                if let Some(print) = print {
                    self.remember(chat_id, &key, print);
                }
                out
            }
            "search_files" => {
                let query = s(args, "query").unwrap_or_default();
                let regex = b(args, "regex");
                let glob = s(args, "glob");
                let offset = n(args, "offset").unwrap_or(0) as usize;
                let cancel = ctx.cancel.clone();
                tokio::task::spawn_blocking(move || {
                    search(
                        &path,
                        &query,
                        regex,
                        glob.as_deref(),
                        offset,
                        budget,
                        &cancel,
                    )
                })
                .await
                .unwrap_or_else(|e| ToolOutput::error(e.to_string()))
            }
            "write_file" | "edit_file" | "delete_file" => {
                // Refuse if the file changed since the preview.
                let expected = action
                    .payload
                    .get("expectedSha256")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let could_not_write = |e: &std::io::Error| {
                    ToolOutput::error(format!(
                        "could not write {}: {}",
                        path.display(),
                        io_reason(e)
                    ))
                };
                if tool == "edit_file" {
                    // The bytes that are hashed are the bytes the edit is applied to.
                    let bytes = match std::fs::read(&path) {
                        Ok(bytes) => bytes,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            return ToolOutput::error(CHANGED_AFTER_PREVIEW);
                        }
                        Err(e) => {
                            return ToolOutput::error(format!(
                                "could not read {}: {}",
                                path.display(),
                                io_reason(&e)
                            ));
                        }
                    };
                    if Some(sha256_hex(&bytes)) != expected {
                        return ToolOutput::error(CHANGED_AFTER_PREVIEW);
                    }
                    let old_text = s(args, "old_text").unwrap_or_default();
                    let new_text = s(args, "new_text").unwrap_or_default();
                    let pick = match Pick::from_args(n(args, "occurrence"), b(args, "replace_all"))
                    {
                        Ok(pick) => pick,
                        Err(why) => return ToolOutput::error(why),
                    };
                    let current = String::from_utf8_lossy(&bytes).into_owned();
                    // The same text and the same arguments gave a plan for the preview.
                    let Ok(plan) = errors::plan_edit(&current, &old_text, &new_text, pick) else {
                        return ToolOutput::error(CHANGED_AFTER_PREVIEW);
                    };
                    let print = fingerprint(&bytes, bytes.len() as u64);
                    let was_known = self.known(chat_id, &key, &print) == Known::Yes;
                    return match write_atomic(&path, plan.updated.as_bytes()) {
                        Ok(()) => {
                            // Known text with a known change is still known text.
                            if was_known {
                                let updated = plan.updated.as_bytes();
                                self.remember(
                                    chat_id,
                                    &key,
                                    fingerprint(updated, updated.len() as u64),
                                );
                            }
                            let what = match pick {
                                Pick::Nth(nth) => format!(
                                    "replaced match {nth} of {}, at line {}",
                                    plan.total, plan.line
                                ),
                                Pick::All if plan.replaced > 1 => {
                                    format!("replaced all {} matches", plan.replaced)
                                }
                                _ => format!("replaced the text at line {}", plan.line),
                            };
                            ToolOutput::text(format!("Edited {}: {what}.", path.display()))
                        }
                        Err(e) => could_not_write(&e),
                    };
                }
                let current = match Self::current_hash(&path) {
                    Ok(h) => h,
                    Err(e) => return ToolOutput::error(e),
                };
                if current != expected {
                    return ToolOutput::error(CHANGED_AFTER_PREVIEW);
                }
                if tool == "write_file" {
                    let content = s(args, "content").unwrap_or_default();
                    match write_atomic(&path, content.as_bytes()) {
                        Ok(()) => {
                            // The model wrote every byte of it.
                            self.remember(
                                chat_id,
                                &key,
                                fingerprint(content.as_bytes(), content.len() as u64),
                            );
                            ToolOutput::text(format!(
                                "Wrote {} ({} bytes).",
                                path.display(),
                                content.len()
                            ))
                        }
                        Err(e) => could_not_write(&e),
                    }
                } else {
                    match std::fs::remove_file(&path) {
                        Ok(()) => {
                            self.forget(chat_id, &key);
                            ToolOutput::text(format!("Deleted {}.", path.display()))
                        }
                        Err(e) => ToolOutput::error(format!(
                            "could not delete {}: {}",
                            path.display(),
                            io_reason(&e)
                        )),
                    }
                }
            }
            other => ToolOutput::error(format!("the files server has no tool named {other}")),
        }
    }
}

/// Writes through a temporary file in the same folder and renames it into place.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_file_name(format!(
        ".{}.{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        std::process::id()
    ));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

fn list_dir(
    root: &Path,
    depth: usize,
    budget: usize,
    cancel: &tokio_util::sync::CancellationToken,
) -> ToolOutput {
    if !root.is_dir() {
        let parent = root.parent().unwrap_or(root);
        return ToolOutput::error(format!(
            "{} is a file, not a folder. Use read_file to read it, or list_dir on {}.",
            root.display(),
            parent.display()
        ));
    }
    let mut lines = Vec::new();
    let mut total = 0usize;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, level)) = stack.pop() {
        if cancel.is_cancelled() {
            return ToolOutput::error("cancelled");
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            total += 1;
            let rel = e
                .path()
                .strip_prefix(root)
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let Ok(ft) = e.file_type() else { continue };
            let line = if ft.is_symlink() {
                format!("{rel}  (link)")
            } else if ft.is_dir() {
                if level + 1 < depth {
                    stack.push((e.path(), level + 1));
                }
                format!("{rel}/")
            } else {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                format!("{rel}  ({size} bytes)")
            };
            lines.push(line);
        }
    }
    // Every entry if they fit; otherwise as many as fit with room left for the notes.
    let fit = |room: usize| {
        let mut shown = String::new();
        let mut count = 0;
        for l in &lines {
            if shown.len() + l.len() + 1 > room {
                break;
            }
            shown.push_str(l);
            shown.push('\n');
            count += 1;
        }
        (shown, count)
    };
    let (mut shown, mut count) = fit(budget);
    if count < lines.len() {
        (shown, count) = fit(budget.saturating_sub(shorten::NOTE_ROOM));
        shown.push_str(&format!(
            "[Showing {count} of {total} entries. List a subfolder to see more.]"
        ));
        let through = stored_lines(&lines[..count]);
        let note = Shown {
            lines: through,
            next: Shown::next_range(through + 1, through, stored_lines(&lines)),
        };
        ToolOutput {
            full_text: Some(lines.join("\n")),
            ui: note.attach(None),
            ..ToolOutput::text(shown)
        }
    } else if lines.is_empty() {
        ToolOutput::text("(empty folder)")
    } else {
        ToolOutput::text(shown.trim_end().to_string())
    }
}

/// Reads a text file by lines. Also returns what identifies the content that was read
/// (for [`FilesServer::remember`]), when text of the file was shown.
fn read_file(
    path: &Path,
    offset: usize,
    limit: Option<usize>,
    budget: usize,
) -> (ToolOutput, Option<Fingerprint>) {
    if path.is_dir() {
        return (
            ToolOutput::error(format!(
                "{} is a folder, not a file. Use list_dir to see what is in it.",
                path.display()
            )),
            None,
        );
    }
    let (bytes, size) = match read_capped(path) {
        Ok(v) => v,
        Err(e) => {
            return (
                ToolOutput::error(format!(
                    "could not read {}: {}",
                    path.display(),
                    io_reason(&e)
                )),
                None,
            );
        }
    };
    if is_binary(&bytes) {
        return (
            ToolOutput::text(format!(
                "{} is a binary file ({size} bytes); read_file only shows text.",
                path.display()
            )),
            None,
        );
    }
    let print = fingerprint(&bytes, size);
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    if total == 0 {
        return (ToolOutput::text("(empty file)"), Some(print));
    }
    if offset > total {
        return (
            ToolOutput::error(format!(
                "offset {offset} is past the end of {}: it has {total} line{}. Use an offset from 1 to {total}.",
                path.display(),
                if total == 1 { "" } else { "s" }
            )),
            None,
        );
    }
    let end_wanted = limit.map_or(total, |l| (offset - 1).saturating_add(l).min(total));
    // Whole lines: everything asked for when it fits the budget, otherwise what fits with
    // room left for the notes.
    let fit = |room: usize| {
        let mut out = String::new();
        let mut last = offset - 1;
        for (i, line) in all.iter().enumerate().take(end_wanted).skip(offset - 1) {
            let numbered = format!("{:>6}\t{line}\n", i + 1);
            if out.len() + numbered.len() > room {
                break;
            }
            out.push_str(&numbered);
            last = i + 1;
        }
        (out, last)
    };
    let (mut out, mut last) = fit(budget);
    let cut = last < end_wanted;
    if cut {
        (out, last) = fit(budget.saturating_sub(shorten::NOTE_ROOM));
    }
    // Not even one line fits: show the start of it.
    let mut pieces_shown = None;
    if last < offset {
        let room = budget.saturating_sub(shorten::NOTE_ROOM + 12);
        let piece = cut_to(all[offset - 1], room);
        let chars = piece.chars().count();
        out = format!(
            "{offset:>6}\t{piece}…\n[Line {offset} is longer than one result can hold: its first {chars} characters are shown.]\n"
        );
        last = offset;
        pieces_shown = Some(chars / shorten::MAX_LINE_CHARS);
    }
    let cut_raw = size > RAW_LIMIT;
    if last < total {
        out.push_str(&format!(
            "[Showing lines {offset}–{last} of {total}{}. Call read_file with offset={} to continue.]",
            if cut_raw { " (only the first 1 MiB of the file was read)" } else { "" },
            last + 1
        ));
    } else if cut_raw {
        out.push_str(&format!(
            "[The file has {size} bytes; read_file reads only the first 1 MiB. A command (run_command) can show a later part.]"
        ));
    }
    if !cut {
        return (ToolOutput::text(out), Some(print));
    }
    // Shortened to the budget: the whole text goes to the app, with the lines that were
    // shown, counted as a stored output counts them.
    let before = stored_lines(&all[..offset - 1]);
    let (shown, first_hidden) = match pieces_shown {
        Some(whole) => (whole.max(1), before + whole + 1),
        None => {
            let shown = stored_lines(&all[offset - 1..last]);
            (shown, before + shown + 1)
        }
    };
    let note = Shown {
        lines: shown,
        next: Shown::next_range(first_hidden, shown, stored_lines(&all)),
    };
    (
        ToolOutput {
            full_text: Some(text.to_string()),
            ui: note.attach(None),
            ..ToolOutput::text(out)
        },
        Some(print),
    )
}

/// Simple `*`/`?` glob over a file name.
fn glob_match(pattern: &str, name: &str) -> bool {
    fn m(p: &[char], n: &[char]) -> bool {
        match (p.first(), n.first()) {
            (None, None) => true,
            (Some('*'), _) => m(&p[1..], n) || (!n.is_empty() && m(p, &n[1..])),
            (Some('?'), Some(_)) => m(&p[1..], &n[1..]),
            (Some(a), Some(b)) => a.eq_ignore_ascii_case(b) && m(&p[1..], &n[1..]),
            _ => false,
        }
    }
    let p: Vec<char> = pattern.chars().collect();
    let n: Vec<char> = name.chars().collect();
    m(&p, &n)
}

fn search(
    root: &Path,
    query: &str,
    regex: bool,
    glob: Option<&str>,
    offset: usize,
    budget: usize,
    cancel: &tokio_util::sync::CancellationToken,
) -> ToolOutput {
    let matcher: Box<dyn Fn(&str) -> bool> = if regex {
        match regex::RegexBuilder::new(query)
            .case_insensitive(true)
            .size_limit(1 << 20)
            .build()
        {
            Ok(re) => Box::new(move |l: &str| re.is_match(l)),
            Err(e) => {
                return ToolOutput::error(format!(
                    "query is not a valid regular expression: {e}. To search for the text itself, leave regex out."
                ));
            }
        }
    } else {
        let q = query.to_lowercase();
        Box::new(move |l: &str| l.to_lowercase().contains(&q))
    };
    let mut files_seen = 0usize;
    let mut matches: Vec<String> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let single = root.is_file();
    if single {
        stack.clear();
    }
    let visit = |path: &Path, matches: &mut Vec<String>| {
        let Ok(meta) = std::fs::metadata(path) else {
            return;
        };
        if meta.len() > MAX_SEARCH_FILE_BYTES {
            return;
        }
        let Ok(bytes) = std::fs::read(path) else {
            return;
        };
        if is_binary(&bytes) {
            return;
        }
        let text = String::from_utf8_lossy(&bytes);
        let rel = path
            .strip_prefix(root)
            .map(|p| p.display().to_string())
            .unwrap_or_else(|_| path.display().to_string());
        for (i, line) in text.lines().enumerate() {
            if matcher(line) {
                let shown: String = line.trim().chars().take(300).collect();
                matches.push(format!(
                    "{}:{}: {}",
                    if rel.is_empty() {
                        path.display().to_string()
                    } else {
                        rel.clone()
                    },
                    i + 1,
                    shown
                ));
            }
        }
    };
    if single {
        visit(root, &mut matches);
    }
    while let Some(dir) = stack.pop() {
        if cancel.is_cancelled() {
            return ToolOutput::error("cancelled");
        }
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries.into_iter().rev() {
            let Ok(ft) = e.file_type() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if name != ".git" && name != "node_modules" && name != "target" {
                    stack.push(e.path());
                }
                continue;
            }
            if glob.is_some_and(|g| !glob_match(g, &name)) {
                continue;
            }
            files_seen += 1;
            if files_seen > MAX_SEARCH_FILES || cancel.is_cancelled() {
                break;
            }
            visit(&e.path(), &mut matches);
        }
    }
    if matches.is_empty() {
        return ToolOutput::text(format!(
            "No matches for \"{query}\" ({files_seen} files searched)."
        ));
    }
    let total = matches.len();
    if offset >= total {
        return ToolOutput::error(format!(
            "offset {offset} is past the end: \"{query}\" has {total} match{}. Use an offset from 0 to {}.",
            if total == 1 { "" } else { "es" },
            total - 1
        ));
    }
    // Whole matches: all of them when they fit the budget, otherwise what fits with room
    // left for the notes (and always one).
    let fit = |room: usize| {
        let mut out = String::new();
        let mut shown = 0;
        for m in matches.iter().skip(offset) {
            if !out.is_empty() && out.len() + m.len() + 1 > room {
                break;
            }
            out.push_str(m);
            out.push('\n');
            shown += 1;
        }
        (out, shown)
    };
    let (mut out, mut shown) = fit(budget);
    if offset + shown >= total {
        return ToolOutput::text(out);
    }
    (out, shown) = fit(budget.saturating_sub(shorten::NOTE_ROOM));
    out.push_str(&format!(
        "[Showing matches {}–{} of {total}. Call search_files with offset={} to continue.]",
        offset + 1,
        offset + shown,
        offset + shown
    ));
    let before = stored_lines(&matches[..offset]);
    let lines = stored_lines(&matches[offset..offset + shown]);
    let note = Shown {
        lines,
        next: Shown::next_range(before + lines + 1, lines, stored_lines(&matches)),
    };
    ToolOutput {
        full_text: Some(matches.join("\n")),
        ui: note.attach(None),
        ..ToolOutput::text(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_util::sync::CancellationToken;

    fn chat(dir: &Path) -> ChatContext {
        ChatContext {
            chat_id: 1,
            approved_folders: vec![dir.display().to_string()],
            working_folder: None,
            forbidden_folders: vec![],
            skill_folders: vec![],
            skill_roots: vec![],
            result_cap_tokens: 2000,
        }
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    /// Skill folders are written only by the skills UI: the file tools refuse to create,
    /// change or delete anything inside one, whatever was approved, and still read them.
    #[tokio::test]
    async fn skill_folders_cannot_be_written_edited_or_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let skills = root.join("skills");
        std::fs::create_dir_all(skills.join("x")).unwrap();
        std::fs::write(skills.join("x/SKILL.md"), "---\nname: x\n---\nbody\n").unwrap();
        let shared = root.join("home/.agents/skills"); // does not exist yet
        let c = ChatContext {
            working_folder: Some(root.display().to_string()),
            skill_roots: vec![skills.display().to_string(), shared.display().to_string()],
            ..chat(&root)
        };
        let fs = FilesServer::default();
        let refused = |why: &str| why.contains("Settings → Skills");
        for (tool, a) in [
            (
                "write_file",
                json!({"path": skills.join("y/SKILL.md").display().to_string(), "content": "x"}),
            ),
            // Relative to the working folder.
            ("write_file", json!({"path": "skills/z.md", "content": "x"})),
            (
                "write_file",
                json!({"path": skills.join("x/SKILL.md").display().to_string(), "content": "x"}),
            ),
            (
                "edit_file",
                json!({"path": skills.join("x/SKILL.md").display().to_string(), "old_text": "body", "new_text": "obey"}),
            ),
            (
                "delete_file",
                json!({"path": skills.join("x/SKILL.md").display().to_string()}),
            ),
            // A skill folder that does not exist yet would be created.
            (
                "write_file",
                json!({"path": shared.join("evil/SKILL.md").display().to_string(), "content": "x"}),
            ),
        ] {
            let why = run(&fs, &c, tool, a.clone()).await.unwrap_err();
            assert!(refused(&why), "{tool} {a}: {why}");
        }
        assert_eq!(
            std::fs::read_to_string(skills.join("x/SKILL.md")).unwrap(),
            "---\nname: x\n---\nbody\n"
        );
        assert!(!skills.join("y").exists() && !root.join("home").exists());
        // Reading is fine, and so is writing next to the skill folders.
        let (_, out) = run(
            &fs,
            &c,
            "read_file",
            json!({"path": skills.join("x/SKILL.md").display().to_string()}),
        )
        .await
        .unwrap();
        assert!(out.joined_text().contains("body"));
        let (_, out) = run(
            &fs,
            &c,
            "write_file",
            json!({"path": "skillsx/notes.md", "content": "x"}),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{out:?}");
    }

    /// A link to a skill folder, or a skill folder named through a link, is still one.
    #[cfg(unix)]
    #[tokio::test]
    async fn skill_folders_reached_through_links_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let skills = root.join("skills");
        std::fs::create_dir_all(&skills).unwrap();
        std::os::unix::fs::symlink(&skills, root.join("link")).unwrap();
        let real = root.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, root.join("named")).unwrap();
        let c = ChatContext {
            skill_roots: vec![
                skills.display().to_string(),
                root.join("named").display().to_string(),
            ],
            ..chat(&root)
        };
        let fs = FilesServer::default();
        for path in [root.join("link/a/SKILL.md"), real.join("b/SKILL.md")] {
            let why = run(
                &fs,
                &c,
                "write_file",
                json!({"path": path.display().to_string(), "content": "x"}),
            )
            .await
            .unwrap_err();
            assert!(
                why.contains("Settings → Skills"),
                "{}: {why}",
                path.display()
            );
        }
    }

    /// An approved write cannot land in a skill folder that appeared after the preview (the
    /// shared folder created as a link to the folder being written).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_write_is_checked_against_skill_folders_again_before_it_runs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let project = root.join("project");
        std::fs::create_dir_all(&project).unwrap();
        let shared = root.join("home/.agents/skills");
        let c = ChatContext {
            skill_roots: vec![shared.display().to_string()],
            ..chat(&root)
        };
        let fs = FilesServer::default();
        let a = args(json!({"path": "project/x/SKILL.md", "content": "x"}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!("not described")
        };
        assert!(action.hard_limit.is_none(), "{action:?}");
        std::fs::create_dir_all(shared.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&project, &shared).unwrap();
        let out = fs.call("write_file", &a, &action, ctx(c.clone())).await;
        assert!(out.is_error, "{out:?}");
        assert!(out.joined_text().contains("Settings → Skills"), "{out:?}");
        assert!(!project.join("x").exists());
    }

    #[test]
    fn skill_folders_compare_by_the_volume_case_rule() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("skills")).unwrap();
        let roots = vec![root.join("skills").display().to_string()];
        let other_case = root.join("SKILLS/x/SKILL.md").display().to_string();
        let insensitive = |_: &str| policy::CaseRule::Insensitive;
        let sensitive = |_: &str| policy::CaseRule::Sensitive;
        let found = crate::skill_guard::skill_root_for(
            &other_case,
            &roots,
            Flavor::native(),
            crate::skill_guard::Reach::Inside,
            &RealFs,
            &insensitive,
        );
        assert!(found.is_some());
        let found = crate::skill_guard::skill_root_for(
            &other_case,
            &roots,
            Flavor::native(),
            crate::skill_guard::Reach::Inside,
            &RealFs,
            &sensitive,
        );
        assert!(found.is_none());
    }

    fn ctx(c: ChatContext) -> CallContext {
        CallContext {
            chat: c,
            cancel: CancellationToken::new(),
            progress: None,
        }
    }

    async fn run(
        fs: &FilesServer,
        chat: &ChatContext,
        tool: &str,
        a: Value,
    ) -> Result<(Action, ToolOutput), String> {
        let a = args(a);
        match fs.describe(tool, &a, chat).await {
            Described::Invalid(why) => Err(why),
            Described::Action(action) => {
                if let Some(h) = &action.hard_limit {
                    return Err(h.clone());
                }
                let out = fs.call(tool, &a, &action, ctx(chat.clone())).await;
                Ok((action, out))
            }
        }
    }

    #[tokio::test]
    async fn read_write_edit_delete_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();

        let (a, out) = run(
            &fs,
            &c,
            "write_file",
            json!({"path": "notes/a.txt", "content": "one\ntwo\n"}),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{out:?}");
        assert_eq!(
            a.scope,
            Scope::FileWrite {
                folder: root.display().to_string()
            }
        );
        assert_eq!(a.risk, Risk::Normal);
        assert!(a.preview.contains("+one"));
        assert_eq!(
            std::fs::read_to_string(root.join("notes/a.txt")).unwrap(),
            "one\ntwo\n"
        );

        let (_, out) = run(&fs, &c, "read_file", json!({"path": "notes/a.txt"}))
            .await
            .unwrap();
        assert!(out.joined_text().contains("     1\tone"));

        let (a, out) = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "notes/a.txt", "old_text": "two", "new_text": "2"}),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert!(a.preview.contains("-two") && a.preview.contains("+2"));
        assert_eq!(
            std::fs::read_to_string(root.join("notes/a.txt")).unwrap(),
            "one\n2\n"
        );

        let (a, out) = run(&fs, &c, "delete_file", json!({"path": "notes/a.txt"}))
            .await
            .unwrap();
        assert_eq!(a.risk, Risk::AlwaysAsk);
        assert!(!out.is_error);
        assert!(!root.join("notes/a.txt").exists());
    }

    #[tokio::test]
    async fn outside_approved_folder_is_a_hard_limit_naming_valid_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("ok")).unwrap();
        let c = chat(&root.join("ok"));
        let fs = FilesServer::default();
        let why = run(&fs, &c, "read_file", json!({"path": "../secret.txt"}))
            .await
            .unwrap_err();
        assert!(why.contains("outside the approved folders"), "{why}");
        assert!(
            why.contains(&root.join("ok").display().to_string()),
            "{why}"
        );
        let none = ChatContext {
            approved_folders: vec![],
            ..c.clone()
        };
        let why = run(&fs, &none, "list_dir", json!({"path": "/"}))
            .await
            .unwrap_err();
        assert!(why.contains("No folder is approved"), "{why}");
    }

    #[tokio::test]
    async fn write_is_refused_when_the_file_changed_after_preview() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("f.txt"), "v1").unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let a = args(json!({"path": "f.txt", "content": "v2"}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!()
        };
        std::fs::write(root.join("f.txt"), "changed by someone else").unwrap();
        let out = fs.call("write_file", &a, &action, ctx(c.clone())).await;
        assert!(out.is_error);
        assert!(out.joined_text().contains("changed after"));
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "changed by someone else"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_planted_after_preview_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.join("w")).unwrap();
        let c = chat(&root.join("w"));
        let fs = FilesServer::default();
        let a = args(json!({"path": "target.txt", "content": "x"}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!()
        };
        std::os::unix::fs::symlink(outside.path().join("victim.txt"), root.join("w/target.txt"))
            .unwrap();
        let out = fs.call("write_file", &a, &action, ctx(c.clone())).await;
        assert!(out.is_error, "{out:?}");
        assert!(!outside.path().join("victim.txt").exists());
    }

    #[tokio::test]
    async fn sensitive_writes_always_ask() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("keep.txt"), "data").unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let a = args(json!({"path": ".git/config", "content": "[core]"}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!()
        };
        assert_eq!(action.risk, Risk::AlwaysAsk);
        let a = args(json!({"path": "keep.txt", "content": ""}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!()
        };
        assert_eq!(action.risk, Risk::AlwaysAsk, "emptying a file always asks");
    }

    #[tokio::test]
    async fn edit_requires_a_unique_match() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("f.txt"), "a a a").unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let why = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "f.txt", "old_text": "a", "new_text": "b"}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("3 times"));
        let (_, out) = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "f.txt", "old_text": "a", "new_text": "b", "replace_all": true}),
        )
        .await
        .unwrap();
        assert!(!out.is_error);
        assert_eq!(
            std::fs::read_to_string(root.join("f.txt")).unwrap(),
            "b b b"
        );
        let why = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "f.txt", "old_text": "zzz", "new_text": "b"}),
        )
        .await
        .unwrap_err();
        assert!(why.contains("not found"));
    }

    const CALC: &str = "fn price() {\n    let total = price * qty;\n    total\n}\n\nfn tax() {\n    let total = price * qty;\n    total * 0.2\n}\n";
    const TOTAL: &str = "let total = price * qty;";

    /// A file tree in a temporary folder: (the guard, its canonical path).
    fn tree(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        for (name, content) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        (dir, root)
    }

    fn shown(path: &Path) -> String {
        path.display().to_string()
    }

    #[tokio::test]
    async fn an_ambiguous_edit_lists_its_matches_and_occurrence_picks_one() {
        let (_dir, root) = tree(&[("calc.rs", CALC)]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let file = root.join("calc.rs");
        // The model reads the file first, so the error may quote its lines.
        run(&fs, &c, "read_file", json!({"path": "calc.rs"}))
            .await
            .unwrap();
        let why = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "calc.rs", "old_text": TOTAL, "new_text": "let total = 1;"}),
        )
        .await
        .unwrap_err();
        assert_eq!(
            why,
            format!(
                "old_text was found 2 times in {}, so it is not clear which one to change. Matches:\n\
                 line 2:     let total = price * qty;\n\
                 line 7:     let total = price * qty;\n\
                 Send the call again with one of these: occurrence (1 to 2, counted from the top) to change one match; replace_all: true to change all 2; or more of the surrounding lines in old_text, so that it matches only once.",
                shown(&file)
            )
        );
        assert_eq!(std::fs::read_to_string(&file).unwrap(), CALC);
        // The second call names the match: the preview and the write change the same one.
        let (action, out) = run(
            &fs,
            &c,
            "edit_file",
            json!({"path": "calc.rs", "old_text": TOTAL, "new_text": "let total = 1;", "occurrence": 2}),
        )
        .await
        .unwrap();
        assert!(!out.is_error, "{out:?}");
        assert_eq!(
            action.summary,
            format!("Edit {} (match 2 of 2, at line 7)", shown(&file))
        );
        assert!(
            action.preview.contains("@@ -4,6 +4,6 @@") && action.preview.contains("+    let total = 1;"),
            "{}",
            action.preview
        );
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            CALC.replacen("    let total = price * qty;\n    total * 0.2", "    let total = 1;\n    total * 0.2", 1)
        );
        assert_eq!(
            out.joined_text(),
            format!("Edited {}: replaced match 2 of 2, at line 7.", shown(&file))
        );
    }

    /// An edit that fails is answered before anyone approved a read, outside the markers
    /// that say "this is data": it must not show what is in a file the model has not read.
    #[tokio::test]
    async fn an_edit_error_never_quotes_a_file_that_was_not_read() {
        let (_dir, root) = tree(&[("calc.rs", CALC)]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let file = shown(&root.join("calc.rs"));
        let quotes_the_file = |message: &str| {
            CALC.lines()
                .map(str::trim)
                .filter(|l| l.len() > 3)
                .any(|l| l != TOTAL && message.contains(l))
        };
        // More than one match: where they are, not what they are.
        let ambiguous = json!({"path": "calc.rs", "old_text": TOTAL, "new_text": "x"});
        let why = run(&fs, &c, "edit_file", ambiguous.clone()).await.unwrap_err();
        assert_eq!(
            why,
            format!(
                "old_text was found 2 times in {file} (at lines 2 and 7), so it is not clear which one to change. Send the call again with one of these: occurrence (1 to 2, counted from the top) to change one match; replace_all: true to change all 2; or more of the surrounding lines in old_text, so that it matches only once. The matching lines are not shown because this file has not been read in this chat; read_file shows them."
            )
        );
        assert!(!why.contains("line 2:") && !quotes_the_file(&why), "{why}");
        // Not found: the closest line by number, and the call that shows it.
        let missing = json!({"path": "calc.rs", "old_text": "total * 0.25", "new_text": "x"});
        let why = run(&fs, &c, "edit_file", missing.clone()).await.unwrap_err();
        assert!(why.starts_with(&format!("old_text was not found in {file}. The closest lines are 8 and 3. This file has not been read in this chat: call read_file ")), "{why}");
        assert!(!quotes_the_file(&why), "{why}");
        // Another chat reading the file changes nothing for this one.
        let other = ChatContext { chat_id: 2, ..c.clone() };
        run(&fs, &other, "read_file", json!({"path": "calc.rs"})).await.unwrap();
        let why = run(&fs, &c, "edit_file", ambiguous.clone()).await.unwrap_err();
        assert!(!why.contains("line 2:") && !quotes_the_file(&why), "{why}");
        // Read here: now the lines are shown.
        run(&fs, &c, "read_file", json!({"path": "calc.rs"})).await.unwrap();
        let why = run(&fs, &c, "edit_file", missing.clone()).await.unwrap_err();
        assert!(why.contains("Closest lines in the file:\nline 8:     total * 0.2\n"), "{why}");
        // Changed on disk since: what was read is no longer what is there.
        std::fs::write(root.join("calc.rs"), CALC.replace("fn tax", "fn vat")).unwrap();
        let why = run(&fs, &c, "edit_file", ambiguous).await.unwrap_err();
        assert!(why.ends_with("The matching lines are not shown because the file has changed since it was read in this chat; read_file shows them."), "{why}");
        assert!(!why.contains("line 2:") && !quotes_the_file(&why), "{why}");
        let why = run(&fs, &c, "edit_file", missing).await.unwrap_err();
        assert!(why.contains("The file has changed since it was read in this chat: call read_file "), "{why}");
        assert!(!quotes_the_file(&why), "{why}");
    }

    #[tokio::test]
    async fn occurrence_and_replace_all_are_checked_and_previewed_as_they_run() {
        let (_dir, root) = tree(&[("f.txt", "a\nb\na\nb\na\n")]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let file = shown(&root.join("f.txt"));
        let edit = |extra: Value| {
            let mut a = json!({"path": "f.txt", "old_text": "a", "new_text": "A"});
            a.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
            a
        };
        // Both at once.
        assert_eq!(
            run(&fs, &c, "edit_file", edit(json!({"occurrence": 1, "replace_all": true}))).await.unwrap_err(),
            "Pass either occurrence or replace_all, not both: occurrence changes one match, replace_all changes every match."
        );
        // A match that is not there.
        assert_eq!(
            run(&fs, &c, "edit_file", edit(json!({"occurrence": 4}))).await.unwrap_err(),
            format!("occurrence 4 is out of range: old_text was found 3 times in {file} (at lines 1, 3 and 5). Pass an occurrence from 1 to 3, or replace_all: true.")
        );
        // Nothing would change.
        assert_eq!(
            run(&fs, &c, "edit_file", json!({"path": "f.txt", "old_text": "b", "new_text": "b", "replace_all": true})).await.unwrap_err(),
            format!("old_text and new_text are the same, so the edit would change nothing in {file}. Put the new wording in new_text.")
        );
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "a\nb\na\nb\na\n");
        // The preview shows the change that is then made.
        let (action, out) = run(&fs, &c, "edit_file", edit(json!({"occurrence": 3}))).await.unwrap();
        assert!(action.preview.contains(" b\n-a\n+A\n"), "{}", action.preview);
        assert_eq!(action.preview.matches("\n+A").count(), 1, "{}", action.preview);
        assert!(!out.is_error);
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "a\nb\na\nb\nA\n");
        let (action, out) = run(&fs, &c, "edit_file", edit(json!({"replace_all": true}))).await.unwrap();
        assert_eq!(action.summary, format!("Edit {file} (2 replacements)"));
        assert_eq!(action.preview.matches("\n+A").count(), 2, "{}", action.preview);
        assert_eq!(out.joined_text(), format!("Edited {file}: replaced all 2 matches."));
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "A\nb\nA\nb\nA\n");
    }

    /// The new arguments change which text is replaced, never what the user is asked.
    #[tokio::test]
    async fn occurrence_and_replace_all_ask_exactly_like_a_plain_edit() {
        let (_dir, root) = tree(&[(".git/config", "[core]\n\tx = 1\n\tx = 1\n"), ("keep.txt", "data\ndata\n")]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let describe = |a: Value| {
            let fs = fs.clone();
            let c = c.clone();
            async move {
                match fs.describe("edit_file", &args(a), &c).await {
                    Described::Action(action) => action,
                    Described::Invalid(why) => panic!("{why}"),
                }
            }
        };
        for extra in [json!({"replace_all": true}), json!({"occurrence": 2})] {
            // A sensitive place.
            let mut a = json!({"path": ".git/config", "old_text": "x = 1", "new_text": "x = 2"});
            a.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
            let action = describe(a).await;
            assert_eq!(action.risk, Risk::AlwaysAsk, "{extra}");
            assert_eq!(action.keep_asking, Some(policy::KeepAsking::Sensitive), "{extra}");
            assert_eq!(action.scope, Scope::FileWrite { folder: shown(&root) });
            assert!(!action.read_only && !action.outbound && !action.network);
            // An ordinary file.
            let mut a = json!({"path": "keep.txt", "old_text": "data", "new_text": "words"});
            a.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap());
            let action = describe(a).await;
            assert_eq!(action.risk, Risk::Normal, "{extra}");
            assert_eq!(action.keep_asking, None, "{extra}");
            assert_eq!(action.scope, Scope::FileWrite { folder: shown(&root) });
        }
        // Leaving the file empty still asks, however the matches were picked.
        let action = describe(json!({"path": "keep.txt", "old_text": "data\n", "new_text": "", "replace_all": true})).await;
        assert_eq!(action.risk, Risk::AlwaysAsk);
        assert_eq!(action.keep_asking, Some(policy::KeepAsking::Delete));
    }

    /// Names of files are what `list_dir` shows, and that asks first: so similar names are
    /// listed by a call the user allowed to read the folder, never by a failed edit.
    #[tokio::test]
    async fn a_missing_file_gets_similar_names_once_a_read_was_allowed() {
        let (_dir, root) = tree(&[
            ("docs/README.md", "x"),
            ("docs/readme.txt", "x"),
            ("docs/notes.txt", "x"),
            ("README.rst", "x"),
            ("src/components/App.tsx", "x"),
        ]);
        let c = ChatContext { working_folder: Some(shown(&root)), ..chat(&root) };
        let fs = FilesServer::default();
        let p = |rel: &str| shown(&root.join(rel));
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "docs/readme.md"})).await.unwrap();
        assert!(out.is_error);
        assert_eq!(
            out.joined_text(),
            format!(
                "{} does not exist. Similar names: {}, {}, {}. Working folder: {}.",
                p("docs/readme.md"), p("docs/README.md"), p("docs/readme.txt"), p("README.rst"), shown(&root)
            )
        );
        // A folder that is misspelled.
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "src/componets/App.tsx"})).await.unwrap();
        assert_eq!(
            out.joined_text(),
            format!(
                "{} does not exist: there is no folder {}. Similar names: {}. Working folder: {}.",
                p("src/componets/App.tsx"), p("src/componets"), p("src/components"), shown(&root)
            )
        );
        // Nothing alike.
        let (_, out) = run(&fs, &c, "list_dir", json!({"path": "zzz"})).await.unwrap();
        assert_eq!(
            out.joined_text(),
            format!(
                "{} does not exist. Nothing with a similar name is in {} or the folder above it. list_dir shows what a folder holds. Working folder: {}.",
                p("zzz"), shown(&root), shown(&root)
            )
        );
        // A failed edit or delete is answered before anyone was asked: no names.
        for (tool, a, expected) in [
            (
                "edit_file",
                json!({"path": "docs/readme.md", "old_text": "a", "new_text": "b"}),
                format!("{} does not exist, so there is nothing to edit. Working folder: {}. list_dir shows the names that exist; write_file creates a file.", p("docs/readme.md"), shown(&root)),
            ),
            (
                "delete_file",
                json!({"path": "docs/readme.md"}),
                format!("{} does not exist, so there is nothing to delete.", p("docs/readme.md")),
            ),
        ] {
            let why = run(&fs, &c, tool, a).await.unwrap_err();
            assert_eq!(why, expected);
            assert!(!why.contains("README") && !why.contains("readme.txt"), "{why}");
        }
    }

    /// Similar names come only from folders the chat may read.
    #[tokio::test]
    async fn similar_names_never_come_from_outside_the_approved_folders() {
        let (_dir, root) = tree(&[("secret-notes.txt", "x"), ("ok/other.txt", "x")]);
        let c = chat(&root.join("ok"));
        let fs = FilesServer::default();
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "secret-note.txt"})).await.unwrap();
        let text = out.joined_text();
        assert!(out.is_error && !text.contains("secret-notes.txt"), "{text}");
        assert!(text.contains("Nothing with a similar name"), "{text}");
    }

    #[tokio::test]
    async fn a_folder_given_for_a_file_and_a_file_for_a_folder_are_named() {
        let (_dir, root) = tree(&[("docs/a.txt", "x\n")]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let (docs, a) = (shown(&root.join("docs")), shown(&root.join("docs/a.txt")));
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "docs"})).await.unwrap();
        assert!(out.is_error);
        assert_eq!(out.joined_text(), format!("{docs} is a folder, not a file. Use list_dir to see what is in it."));
        let (_, out) = run(&fs, &c, "list_dir", json!({"path": "docs/a.txt"})).await.unwrap();
        assert!(out.is_error);
        assert_eq!(out.joined_text(), format!("{a} is a file, not a folder. Use read_file to read it, or list_dir on {docs}."));
        assert_eq!(
            run(&fs, &c, "write_file", json!({"path": "docs", "content": "x"})).await.unwrap_err(),
            format!("{docs} is a folder, not a file. write_file needs the path of a file inside it, for example {}. list_dir shows what is in the folder.", shown(&root.join("docs/notes.txt")))
        );
        assert_eq!(
            run(&fs, &c, "write_file", json!({"path": "docs/a.txt/b.txt", "content": "x"})).await.unwrap_err(),
            format!("{} cannot be used: {a} is a file, not a folder, so nothing can be inside it.", shown(&root.join("docs/a.txt/b.txt")))
        );
        assert_eq!(
            run(&fs, &c, "edit_file", json!({"path": "docs", "old_text": "a", "new_text": "b"})).await.unwrap_err(),
            format!("{docs} is a folder, not a file. edit_file changes one file; list_dir shows what is in the folder.")
        );
        assert_eq!(
            run(&fs, &c, "delete_file", json!({"path": "docs"})).await.unwrap_err(),
            format!("{docs} is a folder. delete_file only deletes files; a folder is removed with a command (run_command).")
        );
        assert!(root.join("docs/a.txt").is_file());
    }

    #[tokio::test]
    async fn a_file_that_changed_since_it_was_read_is_not_replaced() {
        let (_dir, root) = tree(&[("f.txt", "v1\n")]);
        let c = chat(&root);
        let fs = FilesServer::default();
        let file = shown(&root.join("f.txt"));
        run(&fs, &c, "read_file", json!({"path": "f.txt"})).await.unwrap();
        std::fs::write(root.join("f.txt"), "changed by the user\n").unwrap();
        let write = json!({"path": "f.txt", "content": "v2\n"});
        assert_eq!(
            run(&fs, &c, "write_file", write.clone()).await.unwrap_err(),
            format!("{file} has changed since it was read in this chat, so it was not replaced. Call read_file on it again to see what is there now, then write_file or edit_file.")
        );
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "changed by the user\n");
        // Read again: the write goes ahead, and what was written counts as known.
        run(&fs, &c, "read_file", json!({"path": "f.txt"})).await.unwrap();
        let (_, out) = run(&fs, &c, "write_file", write).await.unwrap();
        assert!(!out.is_error, "{out:?}");
        let (_, out) = run(&fs, &c, "write_file", json!({"path": "f.txt", "content": "v3\n"})).await.unwrap();
        assert!(!out.is_error, "{out:?}");
        // A file this chat never read is replaced as before (the card shows the change).
        std::fs::write(root.join("g.txt"), "x\n").unwrap();
        let (_, out) = run(&fs, &c, "write_file", json!({"path": "g.txt", "content": "y\n"})).await.unwrap();
        assert!(!out.is_error, "{out:?}");
    }

    #[tokio::test]
    async fn binary_and_very_large_files_say_what_to_do_instead() {
        let (_dir, root) = tree(&[]);
        std::fs::write(root.join("x.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(root.join("big.txt"), vec![b'a'; (MAX_EDIT_BYTES + 1) as usize]).unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let (bin, big) = (shown(&root.join("x.bin")), shown(&root.join("big.txt")));
        assert_eq!(
            run(&fs, &c, "write_file", json!({"path": "x.bin", "content": "text"})).await.unwrap_err(),
            format!("{bin} is a binary file, so it was not replaced: write_file only writes text. Write the text to another path, or delete the file with delete_file first if it should be replaced.")
        );
        assert_eq!(
            run(&fs, &c, "edit_file", json!({"path": "x.bin", "old_text": "a", "new_text": "b"})).await.unwrap_err(),
            format!("{bin} is a binary file. edit_file only changes text files.")
        );
        assert_eq!(
            run(&fs, &c, "edit_file", json!({"path": "big.txt", "old_text": "a", "new_text": "b"})).await.unwrap_err(),
            format!("{big} is too large for edit_file: it has {} bytes and the limit is {MAX_EDIT_BYTES} (4 MB). Change it with a command (run_command) instead.", MAX_EDIT_BYTES + 1)
        );
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "x.bin"})).await.unwrap();
        assert_eq!(out.joined_text(), format!("{bin} is a binary file (4 bytes); read_file only shows text."));
        // A file larger than one read holds says so, and how to see the rest.
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "big.txt"})).await.unwrap();
        let text = out.joined_text();
        assert!(text.contains(&format!("[Line 1 is longer than one result can hold: its first ")), "{}", &text[text.len() - 300..]);
        assert!(text.ends_with(&format!("[The file has {} bytes; read_file reads only the first 1 MiB. A command (run_command) can show a later part.]", MAX_EDIT_BYTES + 1)), "{}", &text[text.len() - 300..]);
    }

    #[tokio::test]
    async fn a_refused_path_names_the_approved_folders_and_where_to_add_one() {
        let (_dir, root) = tree(&[("ok/a.txt", "x")]);
        let c = chat(&root.join("ok"));
        let fs = FilesServer::default();
        for tool in ["read_file", "list_dir", "delete_file"] {
            assert_eq!(
                run(&fs, &c, tool, json!({"path": "../secret.txt"})).await.unwrap_err(),
                format!("the path is outside the approved folders. Approved folders: {}. Use a path inside one of them; the user can approve another folder in the Tools panel.", shown(&root.join("ok")))
            );
        }
        let none = ChatContext { approved_folders: vec![], ..c.clone() };
        assert_eq!(
            run(&fs, &none, "write_file", json!({"path": "/tmp/x.txt", "content": "x"})).await.unwrap_err(),
            "the path is outside the approved folders. No folder is approved for this chat yet, so the file tools cannot reach anything. The user can approve a folder in the Tools panel."
        );
    }

    #[tokio::test]
    async fn an_offset_past_the_end_says_how_long_the_file_is() {
        let (_dir, root) = tree(&[("f.txt", "one\ntwo\nthree\n")]);
        let (_, out) = run(&FilesServer::default(), &chat(&root), "read_file", json!({"path": "f.txt", "offset": 9})).await.unwrap();
        assert!(out.is_error);
        assert_eq!(
            out.joined_text(),
            format!("offset 9 is past the end of {}: it has 3 lines. Use an offset from 1 to 3.", shown(&root.join("f.txt")))
        );
    }

    /// A result cut to the budget carries the whole text for the app to store, and says
    /// what was shown.
    #[tokio::test]
    async fn a_shortened_read_search_or_listing_keeps_the_whole_text() {
        let body: String = (1..=3000).map(|i| format!("line number {i} with a needle\n")).collect();
        let (_dir, root) = tree(&[("big.txt", &body)]);
        for i in 0..300 {
            std::fs::write(root.join(format!("many-{i:03}.txt")), "x").unwrap();
        }
        let mut c = chat(&root);
        c.result_cap_tokens = 500;
        let fs = FilesServer::default();
        let hint = |out: &ToolOutput| {
            let mut ui = out.ui.clone();
            crate::shorten::Shown::take(&mut ui).expect("a note for the app")
        };
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "big.txt"})).await.unwrap();
        let text = out.joined_text();
        let shown_lines = text.lines().filter(|l| l.contains("\tline number ")).count();
        assert!(shown_lines > 10 && text.len() + 200 <= 1500, "{} lines, {} bytes", shown_lines, text.len());
        assert!(text.ends_with(&format!("[Showing lines 1–{shown_lines} of 3000. Call read_file with offset={} to continue.]", shown_lines + 1)), "{}", &text[text.len() - 120..]);
        assert_eq!(out.full_text.as_deref(), Some(body.as_str()));
        let note = hint(&out);
        assert_eq!(note.lines, shown_lines);
        assert_eq!(note.next, Some((shown_lines + 1, 2 * shown_lines)));
        // From an offset: the numbers are still the file's.
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "big.txt", "offset": 1000})).await.unwrap();
        let note = hint(&out);
        assert_eq!(note.next.map(|(a, _)| a), Some(1000 + note.lines));
        // Asked for a part that fits: nothing was shortened.
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "big.txt", "offset": 5, "limit": 3})).await.unwrap();
        assert!(out.full_text.is_none() && out.ui.is_none(), "{out:?}");
        assert!(out.joined_text().contains("[Showing lines 5–7 of 3000."));

        let (_, out) = run(&fs, &c, "search_files", json!({"path": "big.txt", "query": "needle"})).await.unwrap();
        let text = out.joined_text();
        let full = out.full_text.clone().expect("every match");
        assert_eq!(full.lines().count(), 3000);
        let note = hint(&out);
        assert!(text.contains(&format!("[Showing matches 1–{} of 3000. Call search_files with offset={} to continue.]", note.lines, note.lines)), "{}", &text[text.len() - 120..]);
        assert!(text.len() + 200 <= 1500);

        let (_, out) = run(&fs, &c, "list_dir", json!({"path": "."})).await.unwrap();
        let text = out.joined_text();
        let full = out.full_text.clone().expect("every entry");
        assert_eq!(full.lines().count(), 301);
        let note = hint(&out);
        assert!(text.ends_with(&format!("[Showing {} of 301 entries. List a subfolder to see more.]", note.lines)), "{}", &text[text.len() - 120..]);
        assert!(text.len() + 200 <= 1500);
    }

    #[tokio::test]
    async fn long_files_are_paged_by_whole_lines() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let body: String = (1..=5000)
            .map(|i| format!("line number {i} with some text\n"))
            .collect();
        std::fs::write(root.join("big.txt"), body).unwrap();
        let mut c = chat(&root);
        c.result_cap_tokens = 500;
        let fs = FilesServer::default();
        let (_, out) = run(&fs, &c, "read_file", json!({"path": "big.txt"}))
            .await
            .unwrap();
        let t = out.joined_text();
        assert!(
            t.contains("of 5000. Call read_file with offset="),
            "{}",
            &t[t.len().saturating_sub(200)..]
        );
        let next: usize = t
            .rsplit("offset=")
            .next()
            .unwrap()
            .trim_end_matches(" to continue.]")
            .parse()
            .unwrap();
        let (_, out) = run(
            &fs,
            &c,
            "read_file",
            json!({"path": "big.txt", "offset": next, "limit": 2}),
        )
        .await
        .unwrap();
        let t2 = out.joined_text();
        assert!(t2.contains(&format!("line number {next} ")));
        assert!(t2.contains(&format!("line number {} ", next + 1)));
        assert!(t.len() <= 500 * CHARS_PER_TOKEN + 200);
    }

    #[tokio::test]
    async fn binary_files_get_a_description() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::write(root.join("x.bin"), [0u8, 1, 2, 3]).unwrap();
        let (_, out) = run(
            &FilesServer::default(),
            &chat(&root),
            "read_file",
            json!({"path": "x.bin"}),
        )
        .await
        .unwrap();
        assert!(out.joined_text().contains("binary file"));
    }

    #[tokio::test]
    async fn search_finds_lines_skips_git_and_pages() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn main() {}\n// TODO: fix\n").unwrap();
        std::fs::write(root.join("src/b.md"), "todo list\n").unwrap();
        std::fs::write(root.join(".git/config"), "todo hidden\n").unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let (_, out) = run(
            &fs,
            &c,
            "search_files",
            json!({"path": ".", "query": "todo"}),
        )
        .await
        .unwrap();
        let t = out.joined_text();
        assert!(t.contains("a.rs:2: // TODO: fix"), "{t}");
        assert!(t.contains("b.md:1: todo list"));
        assert!(!t.contains("hidden"));
        let (_, out) = run(
            &fs,
            &c,
            "search_files",
            json!({"path": ".", "query": "todo", "glob": "*.rs"}),
        )
        .await
        .unwrap();
        assert!(!out.joined_text().contains("b.md"));
        let (_, out) = run(
            &fs,
            &c,
            "search_files",
            json!({"path": ".", "query": "fn\\s+main", "regex": true}),
        )
        .await
        .unwrap();
        assert!(out.joined_text().contains("a.rs:1"));
        let (_, out) = run(
            &fs,
            &c,
            "search_files",
            json!({"path": ".", "query": "(", "regex": true}),
        )
        .await
        .unwrap();
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn list_dir_marks_links_and_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        std::fs::write(root.join("sub/f.txt"), "x").unwrap();
        let (_, out) = run(
            &FilesServer::default(),
            &chat(&root),
            "list_dir",
            json!({"path": ".", "depth": 2}),
        )
        .await
        .unwrap();
        let t = out.joined_text();
        assert!(t.contains("sub/"), "{t}");
        assert!(t.contains("f.txt  (1 bytes)"), "{t}");
    }

    #[test]
    fn globs() {
        assert!(glob_match("*.rs", "main.rs"));
        assert!(glob_match("*.RS", "main.rs"));
        assert!(glob_match("a?c", "abc"));
        assert!(!glob_match("*.rs", "main.rsx"));
    }

    #[tokio::test]
    async fn cancelled_calls_do_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(dir.path()).unwrap();
        let c = chat(&root);
        let fs = FilesServer::default();
        let a = args(json!({"path": "x.txt", "content": "x"}));
        let Described::Action(action) = fs.describe("write_file", &a, &c).await else {
            panic!()
        };
        let cx = ctx(c.clone());
        cx.cancel.cancel();
        let out = fs.call("write_file", &a, &action, cx).await;
        assert!(out.is_error);
        assert!(!root.join("x.txt").exists());
    }
}
