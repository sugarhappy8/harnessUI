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

use std::io::Read;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use mcp_host::{BuiltinServer, CallContext, ChatContext, Described, ToolDef, ToolOutput};
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{Action, CheckedPath, Risk, Scope, WriteKind, always_ask_reason};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Bytes read from a file at most (8.4 raw limit).
pub const RAW_LIMIT: u64 = 1024 * 1024;
/// Characters per token used to size results.
const CHARS_PER_TOKEN: usize = 3;
/// Files searched at most per call.
const MAX_SEARCH_FILES: usize = 20_000;
/// Files larger than this are skipped by search.
const MAX_SEARCH_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Diff text kept for the card at most.
const MAX_DIFF_CHARS: usize = 200_000;

/// The `files` server.
#[derive(Debug, Clone)]
pub struct FilesServer {
    flavor: Flavor,
}

impl Default for FilesServer {
    fn default() -> Self {
        Self {
            flavor: Flavor::native(),
        }
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
        Self { flavor }
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
        let folders: Vec<String> = sb.roots().iter().map(|r| r.canonical.to_string()).collect();
        if folders.is_empty() {
            format!(
                "{why}. No folder is approved for this chat yet; ask the user to approve one in the chat's permissions panel."
            )
        } else {
            format!("{why}. Approved folders: {}.", folders.join(", "))
        }
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
            Err(e) => Err(self.refusal(&e, &sb)),
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
            hard_limit: None,
            payload: Self::payload(c, Value::Null),
        })
    }
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
                "Replace exact text in an existing file. old_text must match exactly once unless replace_all is true.",
                json!({"type":"object","properties":{
                    "path": path_schema("File to edit"),
                    "old_text": {"type":"string","minLength":1},
                    "new_text": {"type":"string"},
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
                let (old, hash) = match std::fs::read(&path_buf) {
                    Ok(bytes) if is_binary(&bytes) => {
                        return Described::Invalid(format!(
                            "{shown} is a binary file; write_file only writes text"
                        ));
                    }
                    Ok(bytes) => (
                        String::from_utf8_lossy(&bytes).into_owned(),
                        Some(sha256_hex(&bytes)),
                    ),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => (String::new(), None),
                    Err(e) => return Described::Invalid(format!("could not read {shown}: {e}")),
                };
                if path_buf.is_dir() {
                    return Described::Invalid(format!("{shown} is a folder"));
                }
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
                let replace_all = b(args, "replace_all");
                let bytes = match std::fs::read(&c.canonical) {
                    Ok(b) => b,
                    Err(e) => return Described::Invalid(format!("could not read {shown}: {e}")),
                };
                if is_binary(&bytes) {
                    return Described::Invalid(format!("{shown} is a binary file"));
                }
                let current = String::from_utf8_lossy(&bytes).into_owned();
                let count = current.matches(&old_text).count();
                if count == 0 {
                    return Described::Invalid(format!(
                        "old_text was not found in {shown}. Read the file again and copy the text exactly, including whitespace."
                    ));
                }
                if count > 1 && !replace_all {
                    return Described::Invalid(format!(
                        "old_text appears {count} times in {shown}. Include more surrounding text so it matches once, or set replace_all."
                    ));
                }
                let updated = if replace_all {
                    current.replace(&old_text, &new_text)
                } else {
                    current.replacen(&old_text, &new_text, 1)
                };
                let diff = unified_diff(&current, &updated, &c.canonical);
                let kind = WriteKind::Overwrite {
                    existing_len: current.len() as u64,
                    new_len: updated.len() as u64,
                };
                self.write_action(
                    tool,
                    &c,
                    &root,
                    kind,
                    format!(
                        "Edit {shown} ({count} replacement{})",
                        if count == 1 { "" } else { "s" }
                    ),
                    policy::sanitize::visible(&diff),
                    json!({ "expectedSha256": sha256_hex(&bytes) }),
                )
            }
            "delete_file" => {
                let p = PathBuf::from(&c.canonical);
                if !p.is_file() {
                    return Described::Invalid(format!("{shown} is not a file"));
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
        let budget = budget_chars(&ctx.chat);
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
                tokio::task::spawn_blocking(move || read_file(&path, offset, limit, budget))
                    .await
                    .unwrap_or_else(|e| ToolOutput::error(e.to_string()))
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
                let current = match Self::current_hash(&path) {
                    Ok(h) => h,
                    Err(e) => return ToolOutput::error(e),
                };
                if current != expected {
                    return ToolOutput::error(
                        "The file changed after the change was previewed, so nothing was written. Read it again and redo the change.",
                    );
                }
                match tool {
                    "write_file" => {
                        let content = s(args, "content").unwrap_or_default();
                        write_atomic(&path, content.as_bytes())
                            .map(|()| {
                                ToolOutput::text(format!(
                                    "Wrote {} ({} bytes).",
                                    path.display(),
                                    content.len()
                                ))
                            })
                            .unwrap_or_else(|e| {
                                ToolOutput::error(format!(
                                    "could not write {}: {e}",
                                    path.display()
                                ))
                            })
                    }
                    "edit_file" => {
                        let old_text = s(args, "old_text").unwrap_or_default();
                        let new_text = s(args, "new_text").unwrap_or_default();
                        let current = match std::fs::read(&path) {
                            Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                            Err(e) => {
                                return ToolOutput::error(format!(
                                    "could not read {}: {e}",
                                    path.display()
                                ));
                            }
                        };
                        let count = current.matches(&old_text).count();
                        let updated = if b(args, "replace_all") {
                            current.replace(&old_text, &new_text)
                        } else {
                            current.replacen(&old_text, &new_text, 1)
                        };
                        write_atomic(&path, updated.as_bytes())
                            .map(|()| {
                                ToolOutput::text(format!(
                                    "Edited {} ({count} replacement(s)).",
                                    path.display()
                                ))
                            })
                            .unwrap_or_else(|e| {
                                ToolOutput::error(format!(
                                    "could not write {}: {e}",
                                    path.display()
                                ))
                            })
                    }
                    _ => std::fs::remove_file(&path)
                        .map(|()| ToolOutput::text(format!("Deleted {}.", path.display())))
                        .unwrap_or_else(|e| {
                            ToolOutput::error(format!("could not delete {}: {e}", path.display()))
                        }),
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
        return ToolOutput::error(format!("{} is not a folder", root.display()));
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
    let full = lines.join("\n");
    let mut shown = String::new();
    let mut count = 0;
    for l in &lines {
        if shown.len() + l.len() + 1 > budget {
            break;
        }
        shown.push_str(l);
        shown.push('\n');
        count += 1;
    }
    if count < lines.len() {
        shown.push_str(&format!(
            "[Showing {count} of {total} entries. List a subfolder to see more.]"
        ));
        ToolOutput {
            full_text: Some(full),
            ..ToolOutput::text(shown)
        }
    } else if lines.is_empty() {
        ToolOutput::text("(empty folder)")
    } else {
        ToolOutput::text(shown.trim_end().to_string())
    }
}

fn read_file(path: &Path, offset: usize, limit: Option<usize>, budget: usize) -> ToolOutput {
    if path.is_dir() {
        return ToolOutput::error(format!("{} is a folder; use list_dir", path.display()));
    }
    let (bytes, size) = match read_capped(path) {
        Ok(v) => v,
        Err(e) => return ToolOutput::error(format!("could not read {}: {e}", path.display())),
    };
    if is_binary(&bytes) {
        return ToolOutput::text(format!(
            "{} is a binary file ({size} bytes); it cannot be shown as text.",
            path.display()
        ));
    }
    let text = String::from_utf8_lossy(&bytes);
    let all: Vec<&str> = text.lines().collect();
    let total = all.len();
    if total == 0 {
        return ToolOutput::text("(empty file)");
    }
    if offset > total {
        return ToolOutput::error(format!(
            "offset {offset} is past the end; the file has {total} lines"
        ));
    }
    let end_wanted = limit.map_or(total, |l| (offset - 1 + l).min(total));
    let mut out = String::new();
    let mut last = offset - 1;
    for (i, line) in all.iter().enumerate().take(end_wanted).skip(offset - 1) {
        let numbered = format!("{:>6}\t{line}\n", i + 1);
        if !out.is_empty() && out.len() + numbered.len() > budget {
            break;
        }
        out.push_str(&numbered);
        last = i + 1;
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
            "[The file is {size} bytes; only the first 1 MiB was read.]"
        ));
    }
    ToolOutput::text(out)
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
            Err(e) => return ToolOutput::error(format!("invalid regular expression: {e}")),
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
    let mut out = String::new();
    let mut shown = 0;
    for m in matches.iter().skip(offset) {
        if !out.is_empty() && out.len() + m.len() + 1 > budget {
            break;
        }
        out.push_str(m);
        out.push('\n');
        shown += 1;
    }
    if offset + shown < total {
        out.push_str(&format!(
            "[Showing matches {}–{} of {total}. Call search_files with offset={} to continue.]",
            offset + 1,
            offset + shown,
            offset + shown
        ));
    }
    ToolOutput::text(out)
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
            result_cap_tokens: 2000,
        }
    }

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
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
