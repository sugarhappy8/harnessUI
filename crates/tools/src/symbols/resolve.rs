//! Turns a mention the user chose into the text the model gets.
//!
//! The user picks mentions, so reading one is the user's own act, like attaching a file:
//! there is no approval card. It is still held to exactly the limits of `read_file`, with
//! the same functions: the chat's approved folders only (`PathSandbox::check`, which
//! resolves symlinks before it compares), the app's own folders never, and a second check
//! (`recheck`) right before the file is opened. It is stricter in three ways: nothing
//! inside a skill folder, nothing in a place whose writes always ask (`.git`, `.ssh`, shell
//! start-up files, credential configuration: `policy::always_ask_reason`), and nothing over
//! [`RAW_LIMIT`] or binary.
//!
//! The text is file content, so it is untrusted: the caller stores it as a document
//! attachment, which the prompt wraps in the untrusted-data markers.

use std::io::Read;
use std::path::Path;

use mcp_host::ChatContext;
use policy::path::{Flavor, PathSandbox, RealFs, volume_case};
use policy::{WriteKind, always_ask_reason};

use super::find::{Lang, Symbol, SymbolKind, find_symbols};
use crate::files::RAW_LIMIT;
use crate::skill_guard::{self, Reach};

/// Mentions in one message at most.
pub const MAX_MENTIONS: usize = 8;
/// Characters per token, as the file tools count them.
const CHARS_PER_TOKEN: usize = 3;
/// Tokens one mention may take at most.
const MAX_TOKENS_PER_MENTION: usize = 8_000;
/// Characters one mention gets at least, however small the context.
const MIN_CHARS: usize = 600;

/// One mention as the webview sends it. Every field is untrusted: the path is checked like
/// a `read_file` path and the symbol is looked up again in the file as it is now.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MentionRequest {
    /// Absolute, or relative to the chat's working folder.
    pub path: String,
    /// A definition in the file; none for the whole file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Where the definition was when it was picked: chooses between two of the same name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// A mention read and cut to its share of the context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The file as the sandbox resolved it.
    pub canonical: String,
    /// What the chip and the attachment are called: `src/lib.rs` or `src/lib.rs#parse`.
    pub label: String,
    /// `file`, or the kind of the definition.
    pub kind: String,
    pub symbol: Option<String>,
    pub container: Option<String>,
    /// First and last line of what was taken (1-based); for a file, of the whole file.
    pub start_line: u32,
    pub end_line: u32,
    /// Lines in the file.
    pub total_lines: u32,
    /// Lines were left out to fit the cap.
    pub truncated: bool,
    /// The text the model gets.
    pub text: String,
}

/// The characters each of `count` mentions may take with a context of `n_ctx` tokens: the
/// smallest of 8,000 tokens, a fifth of the context, and 40% of it shared by the mentions.
pub fn cap_chars(n_ctx: usize, count: usize) -> usize {
    let count = count.max(1);
    let tokens = MAX_TOKENS_PER_MENTION
        .min(n_ctx / 5)
        .min(n_ctx * 2 / 5 / count);
    (tokens * CHARS_PER_TOKEN).max(MIN_CHARS)
}

/// The path shown for `canonical`: relative to the working folder when inside it, else
/// `<approved folder name>/<path inside it>`, always with `/`.
pub fn display_path(canonical: &str, roots: &[String], working: Option<&str>) -> String {
    let rel = |base: &str| -> Option<String> {
        let rest = Path::new(canonical).strip_prefix(base).ok()?;
        Some(
            rest.components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/"),
        )
    };
    if let Some(w) = working
        && let Some(r) = rel(w)
        && !r.is_empty()
    {
        return r;
    }
    for root in roots {
        if let Some(r) = rel(root) {
            let name = Path::new(root)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            return if r.is_empty() {
                name
            } else if name.is_empty() {
                r
            } else {
                format!("{name}/{r}")
            };
        }
    }
    canonical.replace('\\', "/")
}

/// The chat's sandbox, built exactly as the file tools build it.
pub fn sandbox(chat: &ChatContext) -> PathSandbox {
    let (sb, skipped) = PathSandbox::new(
        Flavor::native(),
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

/// The folder relative paths resolve against, as the file tools choose it.
pub fn base_folder(chat: &ChatContext, sb: &PathSandbox) -> Option<String> {
    chat.working_folder
        .clone()
        .or_else(|| sb.roots().first().map(|r| r.canonical.to_string()))
}

/// Whether a path the sandbox let through is still off limits for mentions: inside a skill
/// folder (`skill_roots`, as the chat context holds them) or in a place whose writes always
/// ask. Returns the reason.
pub fn off_limits_for(canonical: &str, skill_roots: &[String]) -> Option<&'static str> {
    let flavor = Flavor::native();
    if skill_guard::skill_root_for(
        canonical,
        skill_roots,
        flavor,
        Reach::Inside,
        &RealFs,
        &|p| volume_case(p),
    )
    .is_some()
    {
        return Some("it is in a skills folder");
    }
    always_ask_reason(canonical, WriteKind::Edit, flavor)
        .map(|_| "it is in a protected place (such as .git, .ssh or a settings file)")
}

/// Reads the mention `req` for `chat`, cut to `cap` characters. The error is a short plain
/// sentence for the user.
pub fn resolve(req: &MentionRequest, chat: &ChatContext, cap: usize) -> Result<Resolved, String> {
    let shown = policy::sanitize::one_line(&req.path, 200);
    let sb = sandbox(chat);
    let base = base_folder(chat, &sb);
    let checked = sb
        .check(&req.path, base.as_deref(), &RealFs)
        .map_err(|e| format!("{shown} cannot be mentioned: {e}."))?;
    if !checked.exists {
        return Err(format!("{shown} was not found."));
    }
    let path = Path::new(&checked.canonical);
    let meta = std::fs::metadata(path).map_err(|e| format!("{shown} could not be read: {e}."))?;
    if meta.is_dir() {
        return Err(format!("{shown} is a folder. Mention a file inside it."));
    }
    if !meta.is_file() {
        return Err(format!("{shown} is not a regular file."));
    }
    if let Some(why) = off_limits_for(&checked.canonical, &chat.skill_roots) {
        return Err(format!("{shown} cannot be mentioned: {why}."));
    }
    if meta.len() > RAW_LIMIT {
        return Err(format!(
            "{shown} is too large to mention (over 1 MB). The file tools can read parts of it."
        ));
    }
    // Resolve again right before opening: a link swapped in since the check is refused.
    sb.recheck(&checked, &RealFs)
        .map_err(|e| format!("{shown} cannot be mentioned: {e}."))?;
    let mut bytes = Vec::with_capacity(meta.len() as usize);
    std::fs::File::open(path)
        .and_then(|f| f.take(RAW_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(|e| format!("{shown} could not be read: {e}."))?;
    if bytes.len() as u64 > RAW_LIMIT {
        return Err(format!(
            "{shown} is too large to mention (over 1 MB). The file tools can read parts of it."
        ));
    }
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Err(format!("{shown} is a binary file. Only text files can be mentioned."));
    }
    let text = String::from_utf8_lossy(&bytes);
    let roots: Vec<String> = sb.roots().iter().map(|r| r.canonical.to_string()).collect();
    let rel = display_path(&checked.canonical, &roots, chat.working_folder.as_deref());
    let lines: Vec<&str> = text.lines().collect();
    let total = lines.len() as u32;
    let abs = policy::sanitize::one_line(&checked.canonical, 400);
    match req.symbol.as_deref().filter(|s| !s.is_empty()) {
        None => {
            let header = format!(
                "File: {abs} ({total} line{})\n",
                if total == 1 { "" } else { "s" }
            );
            let (body, last) = numbered(&lines, 1, total, cap.saturating_sub(header.len()));
            let truncated = last < total;
            let mut out = header + &body;
            if truncated {
                out.push_str(&left_out(last + 1, total, "of the file"));
            }
            if total == 0 {
                out.push_str("(empty file)\n");
            }
            Ok(Resolved {
                canonical: checked.canonical.clone(),
                label: rel,
                kind: "file".into(),
                symbol: None,
                container: None,
                start_line: u32::from(total > 0),
                end_line: total,
                total_lines: total,
                truncated,
                text: out,
            })
        }
        Some(wanted) => {
            let lang = Lang::from_path(&checked.canonical)
                .ok_or_else(|| format!("{shown} is not a kind of file whose definitions are known."))?;
            let all = find_symbols(&text, lang);
            let sym = pick(&all, wanted, req)
                .ok_or_else(|| format!("{} was not found in {shown} any more.", one(wanted)))?;
            let qualified = sym.qualified(lang);
            let (start, end) = (sym.lead.min(sym.line), sym.end_line.min(total));
            let header = format!(
                "File: {abs}, lines {start}-{end} ({} {qualified})\n",
                sym.kind.as_str()
            );
            let (body, last) = numbered(&lines, start, end, cap.saturating_sub(header.len()));
            let truncated = last < end;
            let mut out = header + &body;
            if truncated {
                out.push_str(&left_out(last + 1, end, "of this definition"));
            }
            Ok(Resolved {
                canonical: checked.canonical.clone(),
                label: format!("{rel}#{qualified}"),
                kind: sym.kind.as_str().into(),
                symbol: Some(sym.name.to_string()),
                container: sym.container.as_deref().map(str::to_string),
                start_line: start,
                end_line: end,
                total_lines: total,
                truncated,
                text: out,
            })
        }
    }
}

fn one(s: &str) -> String {
    policy::sanitize::one_line(s, 120)
}

/// The definition `wanted` names (`add`, `Point::new`, `Store.create`): same container and
/// kind first, then the one nearest the line it was picked at.
fn pick<'a>(all: &'a [Symbol], wanted: &str, req: &MentionRequest) -> Option<&'a Symbol> {
    let (from_name, name) = match wanted.rsplit_once("::").or_else(|| wanted.rsplit_once('.')) {
        Some((c, n)) if !c.is_empty() && !n.is_empty() => (Some(c), n),
        _ => (None, wanted),
    };
    let container = req.container.as_deref().or(from_name);
    let kind = req.kind.as_deref().and_then(SymbolKind::parse);
    let line = req.line.unwrap_or(0);
    all.iter()
        .filter(|s| &*s.name == name)
        .min_by_key(|s| {
            (
                container.is_some() && s.container.as_deref() != container,
                kind.is_some() && Some(s.kind) != kind,
                s.line.abs_diff(line),
            )
        })
}

/// Lines `from..=to` (1-based) in `read_file`'s format, as many as fit in `room`
/// characters, and the last line given.
fn numbered(lines: &[&str], from: u32, to: u32, room: usize) -> (String, u32) {
    let mut out = String::new();
    let mut last = from.saturating_sub(1);
    // Room for the note about what was left out.
    let room = room.saturating_sub(160);
    for n in from..=to {
        let Some(line) = lines.get(n as usize - 1) else {
            break;
        };
        let numbered = format!("{n:>6}\t{line}\n");
        if out.len() + numbered.len() > room && n > from {
            break;
        }
        if out.len() + numbered.len() > room {
            // Not even the first line fits: its start.
            let mut cut = room.saturating_sub(12).min(line.len());
            while !line.is_char_boundary(cut) {
                cut -= 1;
            }
            out.push_str(&format!("{n:>6}\t{}…\n", &line[..cut]));
            last = n;
            break;
        }
        out.push_str(&numbered);
        last = n;
    }
    (out, last)
}

fn left_out(first: u32, last: u32, what: &str) -> String {
    let n = last - first + 1;
    format!(
        "[Lines {first}-{last} {what} were left out ({n} line{}). The file tools can read more: call read_file with offset={first}.]\n",
        if n == 1 { "" } else { "s" }
    )
}

#[cfg(test)]
mod tests;
